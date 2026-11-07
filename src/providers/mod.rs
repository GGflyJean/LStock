use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue};

use crate::config::ProviderConfig;
use crate::models::{
    CnSentimentRaw, DelayKind, Fundamentals, IndexPayload, MacroItem, MacroPayload, Market,
    MoverItem, MoversPayload, NewsPayload, Quote, SectorItem, SectorPayload, UsSentimentRaw,
    WatchPayload,
};
use crate::rankings::DEFAULT_US_POOL;

pub mod eastmoney;
pub mod sina;
pub mod stooq;
pub mod tencent;
pub mod yahoo;

#[async_trait]
pub trait QuoteProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn quotes(&self, market: Market, symbols: &[String]) -> Result<Vec<Quote>>;
}

#[async_trait]
pub trait IndexProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn indices(&self, market: Market) -> Result<Vec<Quote>>;
}

#[async_trait]
pub trait CnSentimentProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn sentiment(&self) -> Result<CnSentimentRaw>;
}

#[async_trait]
pub trait UsSentimentProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn sentiment(&self) -> Result<UsSentimentRaw>;
}

#[async_trait]
pub trait FundamentalProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fundamentals(&self, market: Market, symbol: &str) -> Result<Fundamentals>;
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    retries: u32,
}

impl HttpClient {
    pub fn new(config: &ProviderConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .user_agent(config.user_agent.clone())
            .default_headers(headers);
        let proxy = resolve_proxy(config.proxy.clone());
        if let Some(proxy) = proxy {
            tracing::info!("行情请求已启用代理");
            builder = builder.proxy(
                reqwest::Proxy::all(&proxy).with_context(|| format!("代理配置无效: {proxy}"))?,
            );
        }
        Ok(Self {
            client: builder.build()?,
            retries: config.retries,
        })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub async fn get_bytes(&self, url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>> {
        let mut last_error = None;
        for attempt in 0..=self.retries {
            let mut request = self.client.get(url);
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
            tracing::debug!(url, attempt, "发起行情请求");
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        tracing::debug!(url, attempt, %status, "行情请求成功");
                        return Ok(response.bytes().await?.to_vec());
                    }
                    tracing::warn!(url, attempt, %status, "行情请求返回非成功状态");
                    last_error = Some(anyhow::anyhow!("HTTP {status}"));
                }
                Err(error) => {
                    tracing::warn!(url, attempt, %error, "行情请求失败");
                    last_error = Some(anyhow::anyhow!("{error}"));
                }
            }
            if attempt < self.retries {
                tokio::time::sleep(Duration::from_millis(200 * (attempt as u64 + 1))).await;
            }
        }
        let error = last_error.unwrap_or_else(|| anyhow::anyhow!("请求失败"));
        tracing::error!(url, %error, "行情请求最终失败");
        Err(error)
    }

    pub async fn get_text(&self, url: &str, headers: &[(&str, &str)]) -> Result<String> {
        let bytes = self.get_bytes(url, headers).await?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    pub async fn get_json(&self, url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value> {
        let text = self.get_text(url, headers).await?;
        serde_json::from_str(&text).with_context(|| format!("无法解析 JSON: {}", truncate(&text)))
    }
}

fn truncate(value: &str) -> String {
    value.chars().take(200).collect()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthView {
    pub source: String,
    pub ok: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub success_count: u64,
    pub failure_count: u64,
}

#[derive(Debug, Clone, Default)]
struct HealthEntry {
    ok: bool,
    checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    success_count: u64,
    failure_count: u64,
}

#[derive(Clone, Default)]
pub struct SourceHealth {
    inner: Arc<Mutex<HashMap<String, HealthEntry>>>,
}

impl SourceHealth {
    pub fn record_ok(&self, source: &str) {
        if let Ok(mut map) = self.inner.lock() {
            let entry = map.entry(source.to_string()).or_default();
            entry.ok = true;
            entry.checked_at = Some(Utc::now());
            entry.last_error = None;
            entry.success_count += 1;
        }
    }

    pub fn record_err(&self, source: &str, error: &str) {
        if let Ok(mut map) = self.inner.lock() {
            let entry = map.entry(source.to_string()).or_default();
            entry.ok = false;
            entry.checked_at = Some(Utc::now());
            entry.last_error = Some(error.to_string());
            entry.failure_count += 1;
        }
    }

    pub fn snapshot(&self) -> Vec<HealthView> {
        let Ok(map) = self.inner.lock() else {
            return Vec::new();
        };
        let mut views: Vec<HealthView> = map
            .iter()
            .map(|(source, entry)| HealthView {
                source: source.clone(),
                ok: entry.ok,
                checked_at: entry.checked_at,
                last_error: entry.last_error.clone(),
                success_count: entry.success_count,
                failure_count: entry.failure_count,
            })
            .collect();
        views.sort_by(|a, b| a.source.cmp(&b.source));
        views
    }
}

#[derive(Clone)]
pub struct Providers {
    pub http: HttpClient,
    pub tencent: tencent::TencentProvider,
    pub sina: sina::SinaProvider,
    pub eastmoney: eastmoney::EastmoneyProvider,
    pub yahoo: yahoo::YahooProvider,
    pub stooq: stooq::StooqProvider,
    pub health: SourceHealth,
}

impl Providers {
    pub fn new(config: &ProviderConfig) -> Result<Self> {
        let http = HttpClient::new(config)?;
        let tencent = tencent::TencentProvider::new(http.clone(), config.tencent_base.clone());
        Ok(Self {
            tencent: tencent.clone(),
            sina: sina::SinaProvider::new(http.clone(), config.sina_base.clone()),
            eastmoney: eastmoney::EastmoneyProvider::new(
                http.clone(),
                config.eastmoney_base.clone(),
                config.eastmoney_datacenter_base.clone(),
                config.eastmoney_push2ex_base.clone(),
                config.eastmoney_news_base.clone(),
                config.eastmoney_fallback_bases.clone(),
                Some(tencent),
            ),
            yahoo: yahoo::YahooProvider::new(http.clone(), config.yahoo_base.clone()),
            stooq: stooq::StooqProvider::new(http.clone(), config.stooq_base.clone()),
            http,
            health: SourceHealth::default(),
        })
    }

    pub async fn quotes(&self, market: Market, symbols: &[String]) -> Result<WatchPayload> {
        if symbols.is_empty() {
            return Ok(WatchPayload {
                quotes: Vec::new(),
                missing: Vec::new(),
                source_note: "自选为空".to_string(),
                as_of_utc: Utc::now(),
            });
        }
        let mut errors = Vec::new();
        let attempts: Vec<(&str, &dyn QuoteProvider)> = match market {
            Market::Cn => vec![("tencent", &self.tencent), ("sina", &self.sina)],
            Market::Us => vec![
                ("tencent", &self.tencent),
                ("sina", &self.sina),
                ("yahoo", &self.yahoo),
            ],
        };
        for (name, provider) in attempts {
            let started = Instant::now();
            tracing::info!(
                market = market.code(),
                provider = provider.name(),
                symbols = symbols.len(),
                "尝试行情源"
            );
            match provider.quotes(market, symbols).await {
                Ok(quotes) if !quotes.is_empty() => {
                    tracing::info!(
                        market = market.code(),
                        provider = provider.name(),
                        count = quotes.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "行情源成功"
                    );
                    self.health.record_ok(name);
                    let found: std::collections::HashSet<&str> =
                        quotes.iter().map(|quote| quote.symbol.as_str()).collect();
                    let missing = symbols
                        .iter()
                        .filter(|symbol| !found.contains(symbol.as_str()))
                        .cloned()
                        .collect();
                    let mut source_note = format!("来源：{}", provider.name());
                    if !errors.is_empty() {
                        source_note.push_str(&format!("（降级：{}）", errors.join("；")));
                    }
                    let as_of = quotes
                        .iter()
                        .map(|quote| quote.as_of_utc)
                        .max()
                        .unwrap_or_else(Utc::now);
                    return Ok(WatchPayload {
                        quotes,
                        missing,
                        source_note,
                        as_of_utc: as_of,
                    });
                }
                Ok(_) => {
                    let message = format!("{name} 返回空数据");
                    tracing::warn!(
                        market = market.code(),
                        provider = provider.name(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "行情源返回空数据"
                    );
                    self.health.record_err(name, &message);
                    errors.push(message);
                }
                Err(error) => {
                    tracing::warn!(
                        market = market.code(),
                        provider = provider.name(),
                        elapsed_ms = started.elapsed().as_millis(),
                        error = %format!("{error:#}"),
                        "行情源失败"
                    );
                    self.health.record_err(name, &format!("{error:#}"));
                    errors.push(format!("{name}: {error:#}"));
                }
            }
        }
        let error = anyhow::anyhow!("所有行情源均失败：{}", errors.join("；"));
        tracing::error!(market = market.code(), error = %error, "所有行情源失败");
        Err(error)
    }

    pub async fn indices(&self, market: Market) -> Result<IndexPayload> {
        let mut errors = Vec::new();
        let attempts: Vec<(&str, &dyn IndexProvider)> = match market {
            Market::Cn => vec![("tencent", &self.tencent), ("sina", &self.sina)],
            Market::Us => vec![
                ("tencent", &self.tencent),
                ("sina", &self.sina),
                ("stooq", &self.stooq),
                ("yahoo", &self.yahoo),
            ],
        };
        for (name, provider) in attempts {
            let started = Instant::now();
            tracing::info!(
                market = market.code(),
                provider = provider.name(),
                "尝试指数源"
            );
            match provider.indices(market).await {
                Ok(indices) if !indices.is_empty() => {
                    tracing::info!(
                        market = market.code(),
                        provider = provider.name(),
                        count = indices.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "指数源成功"
                    );
                    self.health.record_ok(name);
                    let mut source_note = format!("来源：{}", provider.name());
                    if !errors.is_empty() {
                        source_note.push_str(&format!("（降级：{}）", errors.join("；")));
                    }
                    let as_of = indices
                        .iter()
                        .map(|quote| quote.as_of_utc)
                        .max()
                        .unwrap_or_else(Utc::now);
                    return Ok(IndexPayload {
                        indices,
                        source_note,
                        as_of_utc: as_of,
                    });
                }
                Ok(_) => {
                    let message = format!("{name} 返回空数据");
                    tracing::warn!(
                        market = market.code(),
                        provider = provider.name(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "指数源返回空数据"
                    );
                    self.health.record_err(name, &message);
                    errors.push(message);
                }
                Err(error) => {
                    tracing::warn!(
                        market = market.code(),
                        provider = provider.name(),
                        elapsed_ms = started.elapsed().as_millis(),
                        error = %format!("{error:#}"),
                        "指数源失败"
                    );
                    self.health.record_err(name, &format!("{error:#}"));
                    errors.push(format!("{name}: {error:#}"));
                }
            }
        }
        let error = anyhow::anyhow!("所有指数源均失败：{}", errors.join("；"));
        tracing::error!(market = market.code(), error = %error, "所有指数源失败");
        Err(error)
    }

    pub async fn cn_sentiment(&self) -> Result<CnSentimentRaw> {
        let started = Instant::now();
        tracing::info!("尝试 A股情绪源 东方财富");
        match self.eastmoney.sentiment().await {
            Ok(value) => {
                tracing::info!(
                    up = value.up,
                    down = value.down,
                    elapsed_ms = started.elapsed().as_millis(),
                    "A股情绪源成功"
                );
                self.health.record_ok("eastmoney-sentiment");
                Ok(value)
            }
            Err(error) => {
                tracing::warn!(
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %format!("{error:#}"),
                    "A股情绪源失败"
                );
                self.health
                    .record_err("eastmoney-sentiment", &format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub async fn us_sentiment(&self) -> Result<UsSentimentRaw> {
        let started = Instant::now();
        tracing::info!("尝试美股情绪源 腾讯 VIX");
        match self.tencent.sentiment().await {
            Ok(mut value) => {
                value.source = "腾讯行情 VIX（Yahoo 不可用，缺失指标已标注）".to_string();
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis(),
                    vix = ?value.vix,
                    "美股情绪源腾讯 VIX 成功"
                );
                self.health.record_ok("tencent-sentiment");
                return Ok(value);
            }
            Err(error) => {
                tracing::warn!(
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %format!("{error:#}"),
                    "美股情绪源腾讯 VIX 失败，尝试 Yahoo"
                );
                self.health
                    .record_err("tencent-sentiment", &format!("{error:#}"));
            }
        }
        let started = Instant::now();
        match self.yahoo.sentiment().await {
            Ok(value) => {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis(),
                    "美股情绪源 Yahoo 成功"
                );
                self.health.record_ok("yahoo-sentiment");
                Ok(value)
            }
            Err(error) => {
                tracing::error!(
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %format!("{error:#}"),
                    "美股情绪源全部失败"
                );
                self.health
                    .record_err("yahoo-sentiment", &format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub async fn sectors(&self, market: Market) -> Result<SectorPayload> {
        match market {
            Market::Cn => match self.eastmoney.sectors().await {
                Ok(value) => {
                    self.health.record_ok("eastmoney-sectors");
                    Ok(value)
                }
                Err(error) => {
                    self.health
                        .record_err("eastmoney-sectors", &format!("{error:#}"));
                    Err(error)
                }
            },
            Market::Us => {
                const US_SECTORS: [(&str, &str); 11] = [
                    ("XLK", "科技"),
                    ("XLF", "金融"),
                    ("XLE", "能源"),
                    ("XLV", "医疗"),
                    ("XLY", "可选消费"),
                    ("XLP", "日常消费"),
                    ("XLI", "工业"),
                    ("XLB", "材料"),
                    ("XLRE", "房地产"),
                    ("XLU", "公用事业"),
                    ("XLC", "通信服务"),
                ];
                let symbols: Vec<String> = US_SECTORS
                    .iter()
                    .map(|(symbol, _)| symbol.to_string())
                    .collect();
                let payload = self.quotes(Market::Us, &symbols).await?;
                let sectors = US_SECTORS
                    .iter()
                    .filter_map(|(symbol, label)| {
                        payload
                            .quotes
                            .iter()
                            .find(|quote| quote.symbol == *symbol)
                            .map(|quote| SectorItem {
                                code: symbol.to_string(),
                                name: label.to_string(),
                                change_pct: quote.change_pct,
                                leading: quote.name.clone(),
                                source: quote.source.clone(),
                            })
                    })
                    .collect::<Vec<_>>();
                if sectors.is_empty() {
                    return Err(anyhow::anyhow!("美股板块数据为空"));
                }
                Ok(SectorPayload {
                    sectors,
                    source_note: payload.source_note,
                    as_of_utc: payload.as_of_utc,
                })
            }
        }
    }

    pub async fn movers(&self, market: Market, extra_symbols: &[String]) -> Result<MoversPayload> {
        match market {
            Market::Cn => match self.eastmoney.movers_cn().await {
                Ok(value) => {
                    self.health.record_ok("eastmoney-movers");
                    Ok(value)
                }
                Err(error) => {
                    self.health
                        .record_err("eastmoney-movers", &format!("{error:#}"));
                    Err(error)
                }
            },
            Market::Us => {
                let mut symbols: Vec<String> = DEFAULT_US_POOL
                    .iter()
                    .map(|(symbol, _)| symbol.to_string())
                    .collect();
                symbols.extend(extra_symbols.iter().cloned());
                symbols.sort();
                symbols.dedup();
                let payload = self.quotes(Market::Us, &symbols).await?;
                let mut items: Vec<MoverItem> = payload
                    .quotes
                    .iter()
                    .map(|quote| MoverItem {
                        symbol: quote.symbol.clone(),
                        name: if quote.name.is_empty() {
                            quote.symbol.clone()
                        } else {
                            quote.name.clone()
                        },
                        price: quote.price,
                        change_pct: quote.change_pct,
                        amount: quote.amount,
                    })
                    .collect();
                items.sort_by(|a, b| {
                    b.change_pct
                        .partial_cmp(&a.change_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let gainers = items.iter().take(15).cloned().collect();
                let losers = items.iter().rev().take(15).cloned().collect();
                Ok(MoversPayload {
                    gainers,
                    losers,
                    source_note: payload.source_note,
                    as_of_utc: payload.as_of_utc,
                })
            }
        }
    }

    pub async fn news(&self) -> Result<NewsPayload> {
        match self.eastmoney.news().await {
            Ok(value) => {
                self.health.record_ok("eastmoney-news");
                Ok(value)
            }
            Err(error) => {
                self.health
                    .record_err("eastmoney-news", &format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub async fn macro_data(&self, market: Market) -> Result<MacroPayload> {
        match market {
            Market::Cn => match self.eastmoney.macro_cn().await {
                Ok(value) => {
                    self.health.record_ok("eastmoney-macro");
                    Ok(value)
                }
                Err(error) => {
                    self.health
                        .record_err("eastmoney-macro", &format!("{error:#}"));
                    Err(error)
                }
            },
            Market::Us => {
                let mut items = Vec::new();
                if let Ok(raw) = self.us_sentiment().await {
                    if let Some(vix) = raw.vix {
                        items.push(MacroItem {
                            label: "VIX".to_string(),
                            value: format!("{vix:.2}"),
                            note: raw
                                .vix_change_pct
                                .map(|value| format!("{value:+.2}%"))
                                .unwrap_or_default(),
                            unit: String::new(),
                            frequency: "day".to_string(),
                            series: Vec::new(),
                        });
                    }
                }
                if let Ok(indices) = self.indices(Market::Us).await {
                    for quote in indices.indices {
                        items.push(MacroItem {
                            label: if quote.name.is_empty() {
                                quote.symbol.clone()
                            } else {
                                quote.name.clone()
                            },
                            value: quote.price_text(),
                            note: quote.pct_text(),
                            unit: String::new(),
                            frequency: "day".to_string(),
                            series: Vec::new(),
                        });
                    }
                }
                items.push(MacroItem {
                    label: "10Y 美债".to_string(),
                    value: "数据不足".to_string(),
                    note: "Yahoo 在当前网络不可用".to_string(),
                    unit: "%".to_string(),
                    frequency: "day".to_string(),
                    series: Vec::new(),
                });
                items.push(MacroItem {
                    label: "美元指数".to_string(),
                    value: "数据不足".to_string(),
                    note: "Yahoo 在当前网络不可用".to_string(),
                    unit: String::new(),
                    frequency: "day".to_string(),
                    series: Vec::new(),
                });
                Ok(MacroPayload {
                    items,
                    source_note: "来源：腾讯行情（VIX / 指数）".to_string(),
                    as_of_utc: Utc::now(),
                })
            }
        }
    }

    pub async fn fundamentals(&self, market: Market, symbol: &str) -> Result<Fundamentals> {
        let started = Instant::now();
        tracing::debug!(market = market.code(), symbol, "抓取基本面");
        if market == Market::Cn {
            match self.eastmoney.fundamentals(market, symbol).await {
                Ok(mut value) => {
                    if value.dividend_yield.is_none() || value.fcf_coverage.is_none() {
                        if value.price.is_none() {
                            if let Ok(payload) =
                                self.quotes(Market::Cn, &[symbol.to_string()]).await
                            {
                                value.price = payload.quotes.first().map(|quote| quote.price);
                            }
                        }
                        if let Some(price) = value.price {
                            if let Some(dps) = value.latest_dividend_per_share {
                                if value.dividend_yield.is_none() {
                                    value.dividend_yield = Some(dps / price * 100.0);
                                }
                                if value.fcf_coverage.is_none() {
                                    if let (Some(fcf), Some(shares)) =
                                        (value.free_cash_flow, value.total_shares)
                                    {
                                        if dps > 0.0 && shares > 0.0 {
                                            value.fcf_coverage = Some(fcf / (dps * shares));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    value.data_complete = value.price.is_some() && value.total_shares.is_some();
                    tracing::info!(
                        market = market.code(),
                        symbol,
                        elapsed_ms = started.elapsed().as_millis(),
                        "A股基本面抓取成功（东方财富 F10）"
                    );
                    self.health.record_ok("eastmoney-fundamentals");
                    return Ok(value);
                }
                Err(error) => {
                    tracing::warn!(
                        market = market.code(),
                        symbol,
                        elapsed_ms = started.elapsed().as_millis(),
                        error = %format!("{error:#}"),
                        "东方财富基本面失败，尝试 Yahoo"
                    );
                    self.health
                        .record_err("eastmoney-fundamentals", &format!("{error:#}"));
                }
            }
        }
        match self.yahoo.fundamentals(market, symbol).await {
            Ok(value) => {
                tracing::debug!(
                    market = market.code(),
                    symbol,
                    elapsed_ms = started.elapsed().as_millis(),
                    "基本面抓取成功"
                );
                self.health.record_ok("yahoo-fundamentals");
                Ok(value)
            }
            Err(error) => {
                tracing::warn!(
                    market = market.code(),
                    symbol,
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %format!("{error:#}"),
                    "基本面抓取失败"
                );
                self.health
                    .record_err("yahoo-fundamentals", &format!("{error:#}"));
                Err(error)
            }
        }
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_proxy(config_proxy: Option<String>) -> Option<String> {
    non_empty(config_proxy).or_else(|| {
        ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
            .iter()
            .find_map(|key| non_empty(std::env::var(key).ok()))
    })
}

pub fn source_meta(source: &str, delay: DelayKind) -> (String, DelayKind) {
    (source.to_string(), delay)
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    #[test]
    fn blank_proxy_values_are_ignored() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("   ".to_string())), None);
        assert_eq!(
            non_empty(Some(" http://proxy:3128 ".to_string())),
            Some("http://proxy:3128".to_string())
        );
    }
}
