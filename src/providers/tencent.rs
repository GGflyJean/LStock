use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;

use crate::models::{normalize_symbol, tencent_symbol, DelayKind, Market, Quote, UsSentimentRaw};

use super::{HttpClient, IndexProvider, QuoteProvider, UsSentimentProvider};

const CN_INDICES: [&str; 3] = ["sh000001", "sz399001", "sz399006"];
const US_INDICES: [(&str, &str); 3] = [("usDJI", "^DJI"), ("usIXIC", "^IXIC"), ("usINX", "^GSPC")];

#[derive(Clone)]
pub struct TencentProvider {
    http: HttpClient,
    base: String,
}

impl TencentProvider {
    pub fn new(http: HttpClient, base: String) -> Self {
        Self { http, base }
    }

    async fn fetch(&self, market: Market, symbols: &[String]) -> Result<Vec<Quote>> {
        let query: Vec<String> = symbols
            .iter()
            .map(|symbol| tencent_symbol(market, symbol))
            .collect();
        self.fetch_codes(market, &query).await
    }

    async fn fetch_codes(&self, market: Market, codes: &[String]) -> Result<Vec<Quote>> {
        let url = format!("{}?q={}", self.base.trim_end_matches('/'), codes.join(","));
        let bytes = self
            .http
            .get_bytes(&url, &[("Referer", "https://finance.qq.com/")])
            .await?;
        parse_response(&bytes, market)
    }
}

#[async_trait]
impl QuoteProvider for TencentProvider {
    fn name(&self) -> &'static str {
        "腾讯行情"
    }

    async fn quotes(&self, market: Market, symbols: &[String]) -> Result<Vec<Quote>> {
        self.fetch(market, symbols).await
    }
}

#[async_trait]
impl IndexProvider for TencentProvider {
    fn name(&self) -> &'static str {
        "腾讯行情"
    }

    async fn indices(&self, market: Market) -> Result<Vec<Quote>> {
        match market {
            Market::Cn => {
                let symbols: Vec<String> =
                    CN_INDICES.iter().map(|value| value.to_string()).collect();
                self.fetch(Market::Cn, &symbols).await
            }
            Market::Us => {
                let codes: Vec<String> =
                    US_INDICES.iter().map(|(raw, _)| raw.to_string()).collect();
                let mut quotes = self.fetch_codes(Market::Us, &codes).await?;
                for quote in &mut quotes {
                    let raw = format!("us{}", quote.symbol);
                    if let Some((_, canonical)) = US_INDICES
                        .iter()
                        .find(|(code, _)| code.eq_ignore_ascii_case(&raw))
                    {
                        quote.symbol = canonical.to_string();
                    }
                }
                Ok(quotes)
            }
        }
    }
}

#[async_trait]
impl UsSentimentProvider for TencentProvider {
    fn name(&self) -> &'static str {
        "腾讯行情 VIX"
    }

    async fn sentiment(&self) -> Result<UsSentimentRaw> {
        let quotes = self.fetch_codes(Market::Us, &["usVIX".to_string()]).await?;
        let vix = quotes
            .first()
            .ok_or_else(|| anyhow::anyhow!("腾讯 VIX 响应为空"))?;
        Ok(UsSentimentRaw {
            vix: Some(vix.price),
            vix_change_pct: Some(vix.change_pct),
            advance: None,
            decline: None,
            new_high: None,
            new_low: None,
            pcr: None,
            ten_year: None,
            dxy: None,
            source: "腾讯行情 VIX".to_string(),
            as_of_utc: vix.as_of_utc,
        })
    }
}

pub fn parse_response(bytes: &[u8], market: Market) -> Result<Vec<Quote>> {
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    let as_of = Utc::now();
    let mut quotes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().strip_prefix("v_").unwrap_or(key.trim());
        let value = value.trim().trim_end_matches(';').trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        let fields: Vec<&str> = value.split('~').collect();
        if fields.len() < 6 {
            continue;
        }
        let canonical = canonical_from_key(key, market)?;
        let name = fields.get(1).copied().unwrap_or_default().to_string();
        let price = parse_f64(fields.get(3)).unwrap_or_default();
        let prev_close = parse_f64(fields.get(4)).unwrap_or_default();
        let mut change = parse_f64(fields.get(31)).unwrap_or(0.0);
        let mut change_pct = parse_f64(fields.get(32)).unwrap_or(0.0);
        if change == 0.0 && prev_close != 0.0 {
            change = price - prev_close;
        }
        if change_pct == 0.0 && prev_close != 0.0 {
            change_pct = change / prev_close * 100.0;
        }
        let volume = parse_f64(fields.get(6)).unwrap_or_default();
        let amount = if market == Market::Cn {
            parse_f64(fields.get(37)).unwrap_or_default() * 10000.0
        } else {
            0.0
        };
        quotes.push(Quote {
            market,
            symbol: canonical,
            name,
            price,
            prev_close,
            change,
            change_pct,
            volume,
            amount,
            volume_ratio: parse_f64(fields.get(49)),
            turnover: parse_f64(fields.get(38)),
            fund_flow: parse_f64(fields.get(52)),
            breakout_pct: None,
            volatility: None,
            source: "腾讯行情".to_string(),
            delay: if market == Market::Us {
                DelayKind::Delayed
            } else {
                DelayKind::Realtime
            },
            as_of_utc: as_of,
        });
    }
    if quotes.is_empty() {
        return Err(anyhow::anyhow!("腾讯行情响应为空"));
    }
    Ok(quotes)
}

fn canonical_from_key(key: &str, market: Market) -> Result<String> {
    let lower = key.to_ascii_lowercase();
    if lower.starts_with("us") {
        return normalize_symbol(Market::Us, &key[2..]).map_err(|e| anyhow::anyhow!("{e}"));
    }
    normalize_symbol(market, &lower)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("腾讯行情代码 {key} 无效"))
}

fn parse_f64(value: Option<&&str>) -> Option<f64> {
    value
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_line(fields: &[&str]) -> String {
        format!("v_sh000001=\"{}\";", fields.join("~"))
    }

    #[test]
    fn parses_cn_quote() {
        let mut fields = vec![
            "1",
            "上证指数",
            "000001",
            "3891.11",
            "3889.05",
            "3885.00",
            "123456",
        ];
        while fields.len() < 33 {
            fields.push("0");
        }
        fields[31] = "2.06";
        fields[32] = "0.05";
        let line = sample_line(&fields);
        let (bytes, _, _) = encoding_rs::GBK.encode(&line);
        let quotes = parse_response(&bytes, Market::Cn).expect("parse");
        assert_eq!(quotes[0].symbol, "sh000001");
        assert_eq!(quotes[0].name, "上证指数");
        assert!((quotes[0].price - 3891.11).abs() < 1e-6);
        assert!((quotes[0].change_pct - 0.05).abs() < 1e-6);
    }

    #[test]
    fn parses_us_quote() {
        let mut fields = vec!["105", "苹果", "AAPL", "210.5", "208.0", "209.0", "1000"];
        while fields.len() < 33 {
            fields.push("0");
        }
        fields[31] = "2.5";
        fields[32] = "1.20";
        let line = format!("v_usAAPL=\"{}\";", fields.join("~"));
        let (bytes, _, _) = encoding_rs::GBK.encode(&line);
        let quotes = parse_response(&bytes, Market::Us).expect("parse");
        assert_eq!(quotes[0].symbol, "AAPL");
        assert!((quotes[0].change - 2.5).abs() < 1e-6);
    }

    #[test]
    fn decodes_gbk_names() {
        let mut fields = vec!["1", "上证指数", "000001", "1.0", "1.0", "1.0", "0"];
        while fields.len() < 33 {
            fields.push("0");
        }
        let line = sample_line(&fields);
        let (bytes, _, _) = encoding_rs::GBK.encode(&line);
        let quotes = parse_response(&bytes, Market::Cn).expect("parse");
        assert_eq!(quotes[0].name, "上证指数");
    }
}
