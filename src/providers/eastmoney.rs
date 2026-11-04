use std::collections::BTreeMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use crate::models::{CnSentimentRaw, Fundamentals, Market};

use super::{CnSentimentProvider, FundamentalProvider, HttpClient};

#[derive(Clone)]
pub struct EastmoneyProvider {
    http: HttpClient,
    base: String,
    datacenter_base: String,
}

impl EastmoneyProvider {
    pub fn new(http: HttpClient, base: String, datacenter_base: String) -> Self {
        Self {
            http,
            base,
            datacenter_base,
        }
    }
}

#[async_trait]
impl CnSentimentProvider for EastmoneyProvider {
    fn name(&self) -> &'static str {
        "东方财富"
    }

    async fn sentiment(&self) -> Result<CnSentimentRaw> {
        let url = format!(
            "{}/api/qt/clist/get?pn=1&pz=1&po=1&np=1&fltt=2&invt=2&fid=f3&fs=m:0+t:6,m:0+t:80,m:1+t:2,m:1+t:23&fields=f104,f105,f106,f107,f108,f109,f110,f111,f112,f113,f114,f115,f116",
            self.base.trim_end_matches('/')
        );
        let json = self
            .http
            .get_json(&url, &[("Referer", "https://quote.eastmoney.com/")])
            .await
            .with_context(|| "东方财富情绪请求失败")?;
        parse_sentiment(&json)
    }
}

#[async_trait]
impl FundamentalProvider for EastmoneyProvider {
    fn name(&self) -> &'static str {
        "东方财富 F10"
    }

    async fn fundamentals(&self, market: Market, symbol: &str) -> Result<Fundamentals> {
        if market != Market::Cn {
            return Err(anyhow::anyhow!("东方财富 F10 仅支持 A股"));
        }
        let secu = secu_code(symbol)?;
        let digits = symbol
            .strip_prefix("sh")
            .or_else(|| symbol.strip_prefix("sz"))
            .unwrap_or(symbol);
        let main_url = format!(
            "{}/api/data/v1/get?reportName=RPT_F10_FINANCE_MAINFINADATA&columns=ALL&filter=(SECUCODE%3D%22{secu}%22)&pageNumber=1&pageSize=2&sortColumns=REPORT_DATE&sortTypes=-1",
            self.datacenter_base.trim_end_matches('/')
        );
        let main = self
            .http
            .get_json(
                &main_url,
                &[("Referer", "https://emweb.securities.eastmoney.com/")],
            )
            .await
            .with_context(|| format!("东方财富 F10 财务数据失败: {symbol}"))?;

        let dividend_url = format!(
            "{}/api/data/v1/get?reportName=RPT_SHAREBONUS_DET&columns=ALL&filter=(SECURITY_CODE%3D%22{digits}%22)&pageNumber=1&pageSize=50&sortColumns=REPORT_DATE&sortTypes=-1",
            self.datacenter_base.trim_end_matches('/')
        );
        let dividends = self
            .http
            .get_json(&dividend_url, &[("Referer", "https://emweb.securities.eastmoney.com/")])
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(symbol, error = %format!("{error:#}"), "分红数据抓取失败，股息字段留空");
                serde_json::json!({})
            });

        let stock_url = format!(
            "{}/api/qt/stock/get?secid={}&fltt=2&fields=f43,f84,f162",
            self.base.trim_end_matches('/'),
            secid(symbol)?
        );
        let stock = self
            .http
            .get_json(&stock_url, &[("Referer", "https://quote.eastmoney.com/")])
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(symbol, error = %format!("{error:#}"), "估值/股本数据抓取失败");
                serde_json::json!({})
            });

        parse_fundamentals(&main, &dividends, &stock, symbol, market)
    }
}

pub fn parse_sentiment(json: &Value) -> Result<CnSentimentRaw> {
    let data = json
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("响应缺少 data"))?;
    let item = data
        .get("diff")
        .and_then(|diff| diff.as_array())
        .and_then(|items| items.first())
        .or_else(|| data.get("summary"))
        .ok_or_else(|| anyhow::anyhow!("响应缺少涨跌统计"))?;

    let up = pick_i64(item, "f104", "up").unwrap_or(0);
    let down = pick_i64(item, "f105", "down").unwrap_or(0);
    let flat = pick_i64(item, "f106", "flat").unwrap_or(0);
    let limit_up = pick_i64(item, "f107", "limit_up").unwrap_or(0);
    let limit_down = pick_i64(item, "f108", "limit_down").unwrap_or(0);
    let broken_boards = pick_i64(item, "f109", "broken_boards").unwrap_or(0);
    let max_streak = pick_i64(item, "f110", "max_streak").unwrap_or(0);
    let amount = pick_f64(item, "f111", "amount").unwrap_or(0.0);
    let amount_change_pct = pick_f64(item, "f112", "amount_change_pct").unwrap_or(0.0);
    let median_change = pick_f64(item, "f113", "median_change").unwrap_or(0.0);
    let new_high = pick_i64(item, "f114", "new_high").unwrap_or(0);
    let new_low = pick_i64(item, "f115", "new_low").unwrap_or(0);
    let main_net_inflow = pick_f64(item, "f116", "main_net_inflow");

    if up + down + flat == 0 {
        return Err(anyhow::anyhow!("涨跌家数全为 0，数据可能不可用"));
    }

    Ok(CnSentimentRaw {
        up,
        down,
        flat,
        limit_up,
        limit_down,
        broken_boards,
        max_streak,
        amount,
        amount_change_pct,
        median_change,
        new_high,
        new_low,
        main_net_inflow,
        source: "东方财富".to_string(),
        as_of_utc: Utc::now(),
    })
}

pub fn parse_fundamentals(
    main: &Value,
    dividends: &Value,
    stock: &Value,
    symbol: &str,
    market: Market,
) -> Result<Fundamentals> {
    let row = main
        .get("result")
        .and_then(|result| result.get("data"))
        .and_then(|data| data.as_array())
        .and_then(|rows| rows.first())
        .ok_or_else(|| anyhow::anyhow!("东方财富财务数据为空"))?;

    let revenue_growth = number(row, "TOTALOPERATEREVETZ");
    let eps_growth = number(row, "EPSJBTZ")
        .filter(|value| value.abs() > 1e-9)
        .or_else(|| number(row, "PARENTNETPROFITTZ"));
    let roe = number(row, "ROEJQ");
    let roic = number(row, "ROIC");
    let gross_margin_trend = number(row, "XSMLL_TB");
    let free_cash_flow = number(row, "FCFF_FORWARD")
        .or_else(|| number(row, "FCFF_BACK"))
        .or_else(|| number(row, "NETCASH_OPERATE_PK"));
    let debt_to_equity = number(row, "ZCFZL").and_then(|ratio| {
        if ratio > 0.0 && ratio < 100.0 {
            Some(ratio / (100.0 - ratio))
        } else {
            None
        }
    });

    let stock_data = stock.get("data");
    let price = stock_data.and_then(|data| number(data, "f43"));
    let total_shares = stock_data.and_then(|data| number(data, "f84"));
    let pe = stock_data
        .and_then(|data| number(data, "f162"))
        .filter(|value| *value > 0.0);

    let mut yearly: BTreeMap<i32, f64> = BTreeMap::new();
    let mut yearly_eps: BTreeMap<i32, f64> = BTreeMap::new();
    if let Some(rows) = dividends
        .get("result")
        .and_then(|result| result.get("data"))
        .and_then(|data| data.as_array())
    {
        for item in rows {
            let progress = item
                .get("ASSIGN_PROGRESS")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if progress != "实施分配" {
                continue;
            }
            let Some(bonus) = number(item, "PRETAX_BONUS_RMB") else {
                continue;
            };
            let dps = bonus / 10.0;
            if !dps.is_finite() || dps <= 0.0 {
                continue;
            }
            let Some(date) = item.get("REPORT_DATE").and_then(Value::as_str) else {
                continue;
            };
            let Ok(year) = date.get(0..4).unwrap_or_default().parse::<i32>() else {
                continue;
            };
            *yearly.entry(year).or_insert(0.0) += dps;
            if let Some(eps) = number(item, "BASIC_EPS").filter(|value| *value > 0.0) {
                yearly_eps
                    .entry(year)
                    .and_modify(|value| *value = value.max(eps))
                    .or_insert(eps);
            }
        }
    }

    let latest_year = yearly.keys().next_back().copied();
    let latest_total = latest_year.and_then(|year| yearly.get(&year).copied());
    let previous_total = latest_year
        .map(|year| year - 1)
        .and_then(|year| yearly.get(&year).copied());

    let dividend_yield = match (latest_total, price) {
        (Some(dps), Some(price)) if price > 0.0 => Some(dps / price * 100.0),
        _ => None,
    };
    let payout_ratio = match (
        latest_total,
        latest_year.and_then(|year| yearly_eps.get(&year).copied()),
    ) {
        (Some(dps), Some(eps)) if eps > 0.0 => Some(dps / eps * 100.0),
        _ => None,
    };
    let dividend_years = latest_year.map(|latest| {
        let mut count = 0;
        let mut year = latest;
        while count < 30 && yearly.contains_key(&year) {
            count += 1;
            year -= 1;
        }
        count as f64
    });
    let dividend_growth = match (latest_total, previous_total) {
        (Some(latest), Some(previous)) if previous > 0.0 => {
            Some((latest - previous) / previous * 100.0)
        }
        _ => None,
    };
    let fcf_coverage = match (free_cash_flow, latest_total, total_shares) {
        (Some(fcf), Some(dps), Some(shares)) if dps > 0.0 && shares > 0.0 => {
            Some(fcf / (dps * shares))
        }
        _ => None,
    };

    Ok(Fundamentals {
        symbol: symbol.to_string(),
        market,
        revenue_growth,
        eps_growth,
        roe,
        roic,
        gross_margin_trend,
        free_cash_flow,
        debt_to_equity,
        valuation_percentile: pe,
        dividend_yield,
        payout_ratio,
        dividend_years,
        dividend_growth,
        fcf_coverage,
    })
}

fn secu_code(symbol: &str) -> Result<String> {
    if let Some(digits) = symbol.strip_prefix("sh") {
        Ok(format!("{digits}.SH"))
    } else if let Some(digits) = symbol.strip_prefix("sz") {
        Ok(format!("{digits}.SZ"))
    } else {
        Err(anyhow::anyhow!("无法识别的 A股代码 {symbol}"))
    }
}

fn secid(symbol: &str) -> Result<String> {
    if let Some(digits) = symbol.strip_prefix("sh") {
        Ok(format!("1.{digits}"))
    } else if let Some(digits) = symbol.strip_prefix("sz") {
        Ok(format!("0.{digits}"))
    } else {
        Err(anyhow::anyhow!("无法识别的 A股代码 {symbol}"))
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|item| {
            item.as_f64()
                .or_else(|| item.as_str().and_then(|text| text.parse::<f64>().ok()))
        })
        .filter(|value| value.is_finite())
}

fn pick_value<'a>(item: &'a Value, code: &str, name: &str) -> Option<&'a Value> {
    item.get(code).or_else(|| item.get(name))
}

fn pick_i64(item: &Value, code: &str, name: &str) -> Option<i64> {
    let value = pick_value(item, code, name)?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn pick_f64(item: &Value, code: &str, name: &str) -> Option<f64> {
    let value = pick_value(item, code, name)?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_breadth_and_limit_fields() {
        let json: Value = serde_json::from_str(
            r#"{"data":{"diff":[{"f104":3120,"f105":1890,"f106":120,"f107":58,"f108":6,"f109":22,"f110":5,"f111":142000000000,"f112":8.0,"f113":0.35,"f114":80,"f115":12,"f116":-1200000000}]}}"#,
        )
        .unwrap();
        let raw = parse_sentiment(&json).expect("parse");
        assert_eq!(raw.up, 3120);
        assert_eq!(raw.down, 1890);
        assert_eq!(raw.limit_up, 58);
        assert_eq!(raw.limit_down, 6);
        assert_eq!(raw.max_streak, 5);
        assert_eq!(raw.main_net_inflow, Some(-1_200_000_000.0));
    }

    #[test]
    fn missing_data_is_error() {
        let json: Value = serde_json::from_str(r#"{"data":{}}"#).unwrap();
        assert!(parse_sentiment(&json).is_err());
    }

    #[test]
    fn parses_fundamentals_and_dividends() {
        let main: Value = serde_json::from_str(
            r#"{"result":{"data":[{"TOTALOPERATEREVETZ":1.82,"PARENTNETPROFITTZ":-0.25,"ROEJQ":24.47,"ROIC":23.27,"XSMLL_TB":-1.88,"FCFF_FORWARD":33869111226.9,"ZCFZL":11.9}]}}"#,
        )
        .unwrap();
        let dividends: Value = serde_json::from_str(
            r#"{"result":{"data":[
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":280.2423,"REPORT_DATE":"2025-12-31 00:00:00","BASIC_EPS":65.66},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":239.57,"REPORT_DATE":"2025-09-30 00:00:00","BASIC_EPS":51.53},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":276.73,"REPORT_DATE":"2024-12-31 00:00:00","BASIC_EPS":68.64},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":238.82,"REPORT_DATE":"2024-09-30 00:00:00","BASIC_EPS":48.42},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":300.0,"REPORT_DATE":"2023-12-31 00:00:00","BASIC_EPS":60.0}
            ]}}"#,
        )
        .unwrap();
        let stock: Value =
            serde_json::from_str(r#"{"data":{"f43":1446.44,"f84":1250081601.0,"f162":21.04}}"#)
                .unwrap();
        let fundamentals =
            parse_fundamentals(&main, &dividends, &stock, "sh600519", Market::Cn).unwrap();
        assert!((fundamentals.revenue_growth.unwrap() - 1.82).abs() < 1e-6);
        assert!((fundamentals.roic.unwrap() - 23.27).abs() < 1e-6);
        assert!((fundamentals.debt_to_equity.unwrap() - 0.135).abs() < 0.01);
        assert!(fundamentals.dividend_yield.unwrap() > 3.0);
        assert!(fundamentals.dividend_years.unwrap() >= 3.0);
        assert!(fundamentals.payout_ratio.unwrap() > 40.0);
    }
}
