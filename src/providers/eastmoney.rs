use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use crate::models::CnSentimentRaw;

use super::{CnSentimentProvider, HttpClient};

#[derive(Clone)]
pub struct EastmoneyProvider {
    http: HttpClient,
    base: String,
}

impl EastmoneyProvider {
    pub fn new(http: HttpClient, base: String) -> Self {
        Self { http, base }
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
}
