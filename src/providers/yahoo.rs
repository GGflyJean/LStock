use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::models::{yahoo_symbol, DelayKind, Fundamentals, Market, Quote, UsSentimentRaw};

use super::{FundamentalProvider, HttpClient, IndexProvider, QuoteProvider, UsSentimentProvider};

const US_INDICES: [(&str, &str); 3] = [("^DJI", "^DJI"), ("^IXIC", "^IXIC"), ("^GSPC", "^GSPC")];

#[derive(Clone)]
pub struct YahooProvider {
    http: HttpClient,
    base: String,
}

impl YahooProvider {
    pub fn new(http: HttpClient, base: String) -> Self {
        Self { http, base }
    }

    async fn chart(&self, market: Market, symbol: &str) -> Result<Quote> {
        let url = format!(
            "{}/v8/finance/chart/{}?interval=1d&range=1d",
            self.base.trim_end_matches('/'),
            percent_encoding::utf8_percent_encode(symbol, percent_encoding::NON_ALPHANUMERIC)
        );
        let json = self
            .http
            .get_json(&url, &[("Accept", "application/json")])
            .await
            .with_context(|| format!("Yahoo 行情请求失败: {symbol}"))?;
        parse_chart(&json, market)
    }
}

#[async_trait]
impl QuoteProvider for YahooProvider {
    fn name(&self) -> &'static str {
        "Yahoo Finance"
    }

    async fn quotes(&self, market: Market, symbols: &[String]) -> Result<Vec<Quote>> {
        let mut quotes = Vec::new();
        let mut last_error = None;
        for symbol in symbols {
            let request_symbol = yahoo_symbol(market, symbol);
            match self.chart(market, &request_symbol).await {
                Ok(mut quote) => {
                    quote.symbol = symbol.clone();
                    quotes.push(quote);
                }
                Err(error) => last_error = Some(error),
            }
        }
        if quotes.is_empty() {
            return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Yahoo 行情响应为空")));
        }
        Ok(quotes)
    }
}

#[async_trait]
impl IndexProvider for YahooProvider {
    fn name(&self) -> &'static str {
        "Yahoo Finance"
    }

    async fn indices(&self, market: Market) -> Result<Vec<Quote>> {
        match market {
            Market::Us => {
                let mut quotes = Vec::new();
                let mut last_error = None;
                for (request_symbol, canonical) in US_INDICES {
                    match self.chart(Market::Us, request_symbol).await {
                        Ok(mut quote) => {
                            quote.symbol = canonical.to_string();
                            quotes.push(quote);
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                if quotes.is_empty() {
                    return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Yahoo 指数响应为空")));
                }
                Ok(quotes)
            }
            Market::Cn => Err(anyhow::anyhow!("Yahoo 暂不支持 A股指数")),
        }
    }
}

#[async_trait]
impl UsSentimentProvider for YahooProvider {
    fn name(&self) -> &'static str {
        "Yahoo Finance"
    }

    async fn sentiment(&self) -> Result<UsSentimentRaw> {
        let vix = self.chart(Market::Us, "^VIX").await.ok();
        let ten_year = self.chart(Market::Us, "^TNX").await.ok();
        let dxy = self.chart(Market::Us, "DX-Y.NYB").await.ok();
        if vix.is_none() && ten_year.is_none() && dxy.is_none() {
            return Err(anyhow::anyhow!("Yahoo 情绪数据全部不可用"));
        }
        Ok(UsSentimentRaw {
            vix: vix.as_ref().map(|quote| quote.price),
            vix_change_pct: vix.as_ref().map(|quote| quote.change_pct),
            advance: None,
            decline: None,
            new_high: None,
            new_low: None,
            pcr: None,
            ten_year: ten_year.as_ref().map(|quote| quote.price),
            dxy: dxy.as_ref().map(|quote| quote.price),
            source: "Yahoo Finance".to_string(),
            as_of_utc: Utc::now(),
        })
    }
}

#[async_trait]
impl FundamentalProvider for YahooProvider {
    fn name(&self) -> &'static str {
        "Yahoo Finance"
    }

    async fn fundamentals(&self, market: Market, symbol: &str) -> Result<Fundamentals> {
        let request_symbol = yahoo_symbol(market, symbol);
        let url = format!(
            "{}/v10/finance/quoteSummary/{}?modules=financialData,defaultKeyStatistics,summaryDetail",
            self.base.trim_end_matches('/'),
            percent_encoding::utf8_percent_encode(&request_symbol, percent_encoding::NON_ALPHANUMERIC)
        );
        let json = self
            .http
            .get_json(&url, &[("Accept", "application/json")])
            .await
            .with_context(|| format!("Yahoo 基本面请求失败: {symbol}"))?;
        parse_fundamentals(&json, market, symbol)
    }
}

pub fn parse_chart(json: &Value, market: Market) -> Result<Quote> {
    let result = json
        .get("chart")
        .and_then(|chart| chart.get("result"))
        .and_then(|result| result.as_array())
        .and_then(|items| items.first())
        .ok_or_else(|| anyhow::anyhow!("Yahoo 响应缺少 chart.result"))?;
    let meta = result
        .get("meta")
        .ok_or_else(|| anyhow::anyhow!("Yahoo 响应缺少 meta"))?;
    let price = meta
        .get("regularMarketPrice")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Yahoo 响应缺少价格"))?;
    let prev_close = meta
        .get("chartPreviousClose")
        .and_then(Value::as_f64)
        .or_else(|| meta.get("previousClose").and_then(Value::as_f64))
        .unwrap_or(price);
    let change = price - prev_close;
    let change_pct = if prev_close.abs() > f64::EPSILON {
        change / prev_close * 100.0
    } else {
        0.0
    };
    let name = meta
        .get("shortName")
        .or_else(|| meta.get("longName"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let symbol = meta
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let as_of = meta
        .get("regularMarketTime")
        .and_then(Value::as_i64)
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
        .unwrap_or_else(Utc::now);
    Ok(Quote {
        market,
        symbol,
        name,
        price,
        prev_close,
        change,
        change_pct,
        volume: meta
            .get("regularMarketVolume")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        amount: 0.0,
        volume_ratio: None,
        turnover: None,
        fund_flow: None,
        breakout_pct: None,
        volatility: None,
        source: "Yahoo Finance".to_string(),
        delay: DelayKind::Delayed,
        as_of_utc: as_of,
    })
}

pub fn parse_fundamentals(json: &Value, market: Market, symbol: &str) -> Result<Fundamentals> {
    let result = json
        .get("quoteSummary")
        .and_then(|summary| summary.get("result"))
        .and_then(|result| result.as_array())
        .and_then(|items| items.first())
        .ok_or_else(|| anyhow::anyhow!("Yahoo 基本面响应为空"))?;
    let financial = result.get("financialData");
    let stats = result.get("defaultKeyStatistics");
    let summary = result.get("summaryDetail");
    let numeric = |section: Option<&Value>, key: &str| -> Option<f64> {
        section.and_then(|value| value.get(key)).and_then(|value| {
            value
                .get("raw")
                .and_then(Value::as_f64)
                .or_else(|| value.as_f64())
        })
    };
    let any_present = numeric(financial, "revenueGrowth").is_some()
        || numeric(financial, "earningsGrowth").is_some()
        || numeric(financial, "returnOnEquity").is_some()
        || numeric(financial, "freeCashflow").is_some()
        || numeric(summary, "dividendYield").is_some();
    let fundamentals = Fundamentals {
        version: crate::models::FUNDAMENTALS_VERSION,
        data_complete: any_present,
        symbol: symbol.to_string(),
        market,
        revenue_growth: numeric(financial, "revenueGrowth").map(|value| value * 100.0),
        eps_growth: numeric(financial, "earningsGrowth").map(|value| value * 100.0),
        roe: numeric(financial, "returnOnEquity").map(|value| value * 100.0),
        roic: None,
        gross_margin_trend: None,
        free_cash_flow: numeric(financial, "freeCashflow"),
        debt_to_equity: numeric(financial, "debtToEquity"),
        valuation_percentile: numeric(summary, "trailingPE"),
        dividend_yield: numeric(summary, "dividendYield").map(|value| value * 100.0),
        payout_ratio: numeric(summary, "payoutRatio").map(|value| value * 100.0),
        dividend_years: None,
        dividend_growth: numeric(summary, "fiveYearAvgDividendYield"),
        fcf_coverage: None,
        price: None,
        total_shares: None,
        latest_dividend_per_share: None,
    };
    if !any_present {
        return Err(anyhow::anyhow!("Yahoo 基本面字段全部缺失"));
    }
    let _ = stats;
    Ok(fundamentals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chart() {
        let json: Value = serde_json::from_str(
            r#"{"chart":{"result":[{"meta":{"symbol":"AAPL","shortName":"Apple Inc.","regularMarketPrice":210.5,"chartPreviousClose":208.0,"regularMarketTime":1793520000,"regularMarketVolume":1000000}}]}}"#,
        )
        .unwrap();
        let quote = parse_chart(&json, Market::Us).expect("parse");
        assert!((quote.price - 210.5).abs() < 1e-6);
        assert!((quote.change - 2.5).abs() < 1e-6);
        assert_eq!(quote.delay, DelayKind::Delayed);
    }

    #[test]
    fn parses_fundamentals() {
        let json: Value = serde_json::from_str(
            r#"{"quoteSummary":{"result":[{"financialData":{"revenueGrowth":{"raw":0.12},"earningsGrowth":{"raw":0.08},"returnOnEquity":{"raw":0.25},"freeCashflow":{"raw":1000000},"debtToEquity":{"raw":50}},"summaryDetail":{"dividendYield":{"raw":0.015},"payoutRatio":{"raw":0.4},"trailingPE":{"raw":20}}}]}}"#,
        )
        .unwrap();
        let fundamentals = parse_fundamentals(&json, Market::Us, "AAPL").expect("parse");
        assert!((fundamentals.revenue_growth.unwrap() - 12.0).abs() < 1e-6);
        assert!((fundamentals.roe.unwrap() - 25.0).abs() < 1e-6);
        assert!((fundamentals.dividend_yield.unwrap() - 1.5).abs() < 1e-6);
    }
}
