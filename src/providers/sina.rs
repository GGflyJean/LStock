use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use crate::models::{normalize_symbol, sina_symbol, DelayKind, Market, Quote};

use super::{HttpClient, IndexProvider, QuoteProvider};

const CN_INDICES: [&str; 3] = ["sh000001", "sz399001", "sz399006"];

#[derive(Clone)]
pub struct SinaProvider {
    http: HttpClient,
    base: String,
}

impl SinaProvider {
    pub fn new(http: HttpClient, base: String) -> Self {
        Self { http, base }
    }

    async fn fetch(&self, market: Market, symbols: &[String]) -> Result<Vec<Quote>> {
        let query: Vec<String> = symbols
            .iter()
            .map(|symbol| sina_symbol(market, symbol))
            .collect();
        let url = format!(
            "{}?list={}",
            self.base.trim_end_matches('/'),
            query.join(",")
        );
        let bytes = self
            .http
            .get_bytes(&url, &[("Referer", "https://finance.sina.com.cn/")])
            .await?;
        parse_response(&bytes, market)
    }
}

#[async_trait]
impl QuoteProvider for SinaProvider {
    fn name(&self) -> &'static str {
        "新浪行情"
    }

    async fn quotes(&self, market: Market, symbols: &[String]) -> Result<Vec<Quote>> {
        self.fetch(market, symbols).await
    }
}

#[async_trait]
impl IndexProvider for SinaProvider {
    fn name(&self) -> &'static str {
        "新浪行情"
    }

    async fn indices(&self, market: Market) -> Result<Vec<Quote>> {
        match market {
            Market::Cn => {
                let symbols: Vec<String> =
                    CN_INDICES.iter().map(|value| value.to_string()).collect();
                self.fetch(Market::Cn, &symbols).await
            }
            Market::Us => Err(anyhow::anyhow!("新浪暂不支持美股指数")),
        }
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
        let Some(symbol) = key.trim().strip_prefix("var hq_str_") else {
            continue;
        };
        let value = value.trim().trim_end_matches(';').trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        let fields: Vec<&str> = value.split(',').collect();
        if fields.len() < 4 {
            continue;
        }
        let is_us = symbol.starts_with("gb_");
        let canonical = if is_us {
            normalize_symbol(Market::Us, &symbol[3..]).map_err(|e| anyhow::anyhow!("{e}"))?
        } else {
            normalize_symbol(market, symbol).map_err(|e| anyhow::anyhow!("{e}"))?
        };
        let name = fields[0].to_string();
        let (price, prev_close, change, change_pct, volume, amount) = if is_us {
            let price = parse_f64(fields.get(1)).unwrap_or_default();
            let change_pct = parse_f64(fields.get(2)).unwrap_or_default();
            let prev_close = if change_pct.abs() > f64::EPSILON {
                price / (1.0 + change_pct / 100.0)
            } else {
                price
            };
            let volume = parse_f64(fields.get(10)).unwrap_or_default();
            let amount = parse_f64(fields.get(11)).unwrap_or_default();
            (
                price,
                prev_close,
                price - prev_close,
                change_pct,
                volume,
                amount,
            )
        } else {
            let prev_close = parse_f64(fields.get(2)).unwrap_or_default();
            let price = parse_f64(fields.get(3)).unwrap_or_default();
            let volume = parse_f64(fields.get(8)).unwrap_or_default();
            let amount = parse_f64(fields.get(9)).unwrap_or_default();
            let change = price - prev_close;
            let change_pct = if prev_close.abs() > f64::EPSILON {
                change / prev_close * 100.0
            } else {
                0.0
            };
            (price, prev_close, change, change_pct, volume, amount)
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
            volume_ratio: None,
            turnover: None,
            fund_flow: None,
            breakout_pct: None,
            volatility: None,
            source: "新浪行情".to_string(),
            delay: DelayKind::Realtime,
            as_of_utc: as_of,
        });
    }
    if quotes.is_empty() {
        return Err(anyhow::anyhow!("新浪行情响应为空"));
    }
    Ok(quotes)
}

fn parse_f64(value: Option<&&str>) -> Option<f64> {
    value
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cn_quote() {
        let text = r#"var hq_str_sh600519="贵州茅台,1500.0,1490.0,1510.0,1520.0,1480.0,0,0,12345,67890.0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,2026-11-01,15:00:00,00";"#;
        let (bytes, _, _) = encoding_rs::GBK.encode(text);
        let quotes = parse_response(&bytes, Market::Cn).expect("parse");
        assert_eq!(quotes[0].symbol, "sh600519");
        assert_eq!(quotes[0].name, "贵州茅台");
        assert!((quotes[0].price - 1510.0).abs() < 1e-6);
        assert!((quotes[0].change - 20.0).abs() < 1e-6);
    }

    #[test]
    fn parses_us_quote() {
        let text =
            r#"var hq_str_gb_aapl="苹果,210.5,1.20,209.0,211.0,208.0,0,0,0,0,123456,7890123,0";"#;
        let (bytes, _, _) = encoding_rs::GBK.encode(text);
        let quotes = parse_response(&bytes, Market::Us).expect("parse");
        assert_eq!(quotes[0].symbol, "AAPL");
        assert!((quotes[0].price - 210.5).abs() < 1e-6);
        assert!((quotes[0].change_pct - 1.20).abs() < 1e-6);
    }
}
