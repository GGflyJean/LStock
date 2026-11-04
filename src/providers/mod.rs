use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue};

use crate::config::ProviderConfig;
use crate::models::{
    CnSentimentRaw, DelayKind, Fundamentals, IndexPayload, Market, Quote, UsSentimentRaw,
    WatchPayload,
};

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
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response.bytes().await?.to_vec());
                    }
                    last_error = Some(anyhow::anyhow!("HTTP {status}"));
                }
                Err(error) => last_error = Some(anyhow::anyhow!("{error}")),
            }
            if attempt < self.retries {
                tokio::time::sleep(Duration::from_millis(200 * (attempt as u64 + 1))).await;
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("请求失败")))
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
        Ok(Self {
            tencent: tencent::TencentProvider::new(http.clone(), config.tencent_base.clone()),
            sina: sina::SinaProvider::new(http.clone(), config.sina_base.clone()),
            eastmoney: eastmoney::EastmoneyProvider::new(
                http.clone(),
                config.eastmoney_base.clone(),
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
            Market::Us => vec![("tencent", &self.tencent), ("yahoo", &self.yahoo)],
        };
        for (name, provider) in attempts {
            match provider.quotes(market, symbols).await {
                Ok(quotes) if !quotes.is_empty() => {
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
                    self.health.record_err(name, &message);
                    errors.push(message);
                }
                Err(error) => {
                    self.health.record_err(name, &format!("{error:#}"));
                    errors.push(format!("{name}: {error:#}"));
                }
            }
        }
        Err(anyhow::anyhow!("所有行情源均失败：{}", errors.join("；")))
    }

    pub async fn indices(&self, market: Market) -> Result<IndexPayload> {
        let mut errors = Vec::new();
        let attempts: Vec<(&str, &dyn IndexProvider)> = match market {
            Market::Cn => vec![("tencent", &self.tencent), ("sina", &self.sina)],
            Market::Us => vec![
                ("yahoo", &self.yahoo),
                ("stooq", &self.stooq),
                ("tencent", &self.tencent),
            ],
        };
        for (name, provider) in attempts {
            match provider.indices(market).await {
                Ok(indices) if !indices.is_empty() => {
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
                    self.health.record_err(name, &message);
                    errors.push(message);
                }
                Err(error) => {
                    self.health.record_err(name, &format!("{error:#}"));
                    errors.push(format!("{name}: {error:#}"));
                }
            }
        }
        Err(anyhow::anyhow!("所有指数源均失败：{}", errors.join("；")))
    }

    pub async fn cn_sentiment(&self) -> Result<CnSentimentRaw> {
        match self.eastmoney.sentiment().await {
            Ok(value) => {
                self.health.record_ok("eastmoney-sentiment");
                Ok(value)
            }
            Err(error) => {
                self.health
                    .record_err("eastmoney-sentiment", &format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub async fn us_sentiment(&self) -> Result<UsSentimentRaw> {
        match self.yahoo.sentiment().await {
            Ok(value) => {
                self.health.record_ok("yahoo-sentiment");
                Ok(value)
            }
            Err(error) => {
                self.health
                    .record_err("yahoo-sentiment", &format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub async fn fundamentals(&self, market: Market, symbol: &str) -> Result<Fundamentals> {
        match self.yahoo.fundamentals(market, symbol).await {
            Ok(value) => {
                self.health.record_ok("yahoo-fundamentals");
                Ok(value)
            }
            Err(error) => {
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
