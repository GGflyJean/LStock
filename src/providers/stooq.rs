use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use crate::models::{DelayKind, Market, Quote};

use super::{HttpClient, IndexProvider};

const US_INDICES: [(&str, &str); 3] = [("^spx", "^GSPC"), ("^dji", "^DJI"), ("^ndq", "^IXIC")];

#[derive(Clone)]
pub struct StooqProvider {
    http: HttpClient,
    base: String,
}

impl StooqProvider {
    pub fn new(http: HttpClient, base: String) -> Self {
        Self { http, base }
    }

    async fn fetch(&self, symbol: &str) -> Result<Quote> {
        let url = format!(
            "{}/q/l/?s={}&f=sd2t2ohlcv&h&e=csv",
            self.base.trim_end_matches('/'),
            symbol
        );
        let text = self.http.get_text(&url, &[]).await?;
        parse_csv(&text, symbol)
    }
}

#[async_trait]
impl IndexProvider for StooqProvider {
    fn name(&self) -> &'static str {
        "Stooq"
    }

    async fn indices(&self, market: Market) -> Result<Vec<Quote>> {
        if market != Market::Us {
            return Err(anyhow::anyhow!("Stooq 仅用于美股指数备源"));
        }
        let mut quotes = Vec::new();
        let mut last_error = None;
        for (request_symbol, canonical) in US_INDICES {
            match self.fetch(request_symbol).await {
                Ok(mut quote) => {
                    quote.symbol = canonical.to_string();
                    quotes.push(quote);
                }
                Err(error) => last_error = Some(error),
            }
        }
        if quotes.is_empty() {
            return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Stooq 指数响应为空")));
        }
        Ok(quotes)
    }
}

pub fn parse_csv(text: &str, symbol: &str) -> Result<Quote> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let _header = lines.next();
    let line = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("Stooq 响应为空"))?;
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() < 8 {
        return Err(anyhow::anyhow!("Stooq 响应格式异常"));
    }
    let open = fields[3].parse::<f64>().unwrap_or_default();
    let close = fields[6].parse::<f64>().unwrap_or_default();
    if close <= 0.0 {
        return Err(anyhow::anyhow!("Stooq 收盘价无效"));
    }
    let volume = fields[7].parse::<f64>().unwrap_or_default();
    let change = close - open;
    let change_pct = if open.abs() > f64::EPSILON {
        change / open * 100.0
    } else {
        0.0
    };
    Ok(Quote {
        market: Market::Us,
        symbol: symbol.to_string(),
        name: symbol.to_string(),
        price: close,
        prev_close: open,
        change,
        change_pct,
        volume,
        amount: 0.0,
        volume_ratio: None,
        turnover: None,
        fund_flow: None,
        breakout_pct: None,
        volatility: None,
        source: "Stooq".to_string(),
        delay: DelayKind::Daily,
        as_of_utc: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv_line() {
        let text = "Symbol,Date,Time,Open,High,Low,Close,Volume\n^spx,2026-10-31,16:00:00,6800.5,6850.0,6790.0,6840.0,1234567\n";
        let quote = parse_csv(text, "^spx").expect("parse");
        assert!((quote.price - 6840.0).abs() < 1e-6);
        assert!((quote.change - 39.5).abs() < 1e-6);
        assert_eq!(quote.delay, DelayKind::Daily);
    }
}
