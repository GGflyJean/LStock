use std::collections::BTreeMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::Value;

use crate::market::is_trading_day;
use crate::models::{
    CnSentimentRaw, Fundamentals, MacroItem, MacroPayload, Market, MoverItem, MoversPayload,
    NewsItem, NewsPayload, SectorItem, SectorPayload,
};

use super::{tencent::TencentProvider, CnSentimentProvider, FundamentalProvider, HttpClient};

#[derive(Clone)]
pub struct EastmoneyProvider {
    http: HttpClient,
    bases: Vec<String>,
    datacenter_base: String,
    push2ex_base: String,
    news_base: String,
    tencent: Option<TencentProvider>,
}

impl EastmoneyProvider {
    pub fn new(
        http: HttpClient,
        base: String,
        datacenter_base: String,
        push2ex_base: String,
        news_base: String,
        fallbacks: Vec<String>,
        tencent: Option<TencentProvider>,
    ) -> Self {
        let mut bases = vec![base];
        for fallback in fallbacks {
            if !bases.iter().any(|item| item == &fallback) {
                bases.push(fallback);
            }
        }
        Self {
            http,
            bases,
            datacenter_base,
            push2ex_base,
            news_base,
            tencent,
        }
    }

    async fn push2_json(&self, path: &str, headers: &[(&str, &str)]) -> Result<Value> {
        let mut last_error = None;
        for base in &self.bases {
            let url = format!("{}{}", base.trim_end_matches('/'), path);
            match self.http.get_json(&url, headers).await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    tracing::warn!(
                        base = %base,
                        error = %format!("{error:#}"),
                        "东财 push2 请求失败，尝试下一个地址"
                    );
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("没有可用的东财 push2 地址")))
    }
}

impl EastmoneyProvider {
    async fn report(&self, name: &str) -> Result<Value> {
        let url = format!(
            "{}/api/data/v1/get?reportName={name}&columns=ALL&pageNumber=1&pageSize=1&sortColumns=REPORT_DATE&sortTypes=-1",
            self.datacenter_base.trim_end_matches('/')
        );
        self.http
            .get_json(&url, &[("Referer", "https://data.eastmoney.com/")])
            .await
            .with_context(|| format!("东方财富宏观数据失败: {name}"))
    }

    pub async fn sectors(&self) -> Result<SectorPayload> {
        let json = self
            .push2_json(
                "/api/qt/clist/get?pn=1&pz=40&po=1&np=1&fltt=2&invt=2&fid=f3&fs=m:90+t:2&fields=f12,f14,f3,f128",
                &[("Referer", "https://quote.eastmoney.com/")],
            )
            .await
            .with_context(|| "东方财富行业板块请求失败")?;
        parse_sectors(&json)
    }

    async fn mover_list(&self, order: &str) -> Result<Vec<MoverItem>> {
        let path = format!(
            "/api/qt/clist/get?pn=1&pz=15&po={order}&np=1&fltt=2&invt=2&fid=f3&fs=m:0+t:6,m:0+t:80,m:1+t:2,m:1+t:23&fields=f12,f14,f2,f3,f6"
        );
        let json = self
            .push2_json(&path, &[("Referer", "https://quote.eastmoney.com/")])
            .await
            .with_context(|| "东方财富涨跌榜请求失败")?;
        parse_movers(&json)
    }

    pub async fn movers_cn(&self) -> Result<MoversPayload> {
        let (gainers, losers) = tokio::join!(self.mover_list("1"), self.mover_list("0"));
        let gainers = gainers?;
        let losers = losers?;
        Ok(MoversPayload {
            gainers,
            losers,
            source_note: "来源：东方财富涨跌榜".to_string(),
            as_of_utc: Utc::now(),
        })
    }

    pub async fn news(&self) -> Result<NewsPayload> {
        let url = format!(
            "{}/comm/web/getFastNewsList?client=web&biz=web_724&fastColumn=102&sortEnd=&pageSize=30&req_trace=1",
            self.news_base.trim_end_matches('/')
        );
        let json = self
            .http
            .get_json(&url, &[("Referer", "https://kuaixun.eastmoney.com/")])
            .await
            .with_context(|| "东方财富快讯请求失败")?;
        parse_news(&json)
    }

    pub async fn macro_cn(&self) -> Result<MacroPayload> {
        let (gdp, cpi, ppi, pmi, m2) = tokio::join!(
            self.report("RPT_ECONOMY_GDP"),
            self.report("RPT_ECONOMY_CPI"),
            self.report("RPT_ECONOMY_PPI"),
            self.report("RPT_ECONOMY_PMI"),
            self.report("RPT_ECONOMY_CURRENCY_SUPPLY")
        );
        let mut items = Vec::new();
        if let Some(row) = latest_row(&gdp.unwrap_or_default()) {
            items.push(macro_pct("GDP 同比", row, "SUM_SAME"));
        }
        if let Some(row) = latest_row(&cpi.unwrap_or_default()) {
            items.push(macro_pct("CPI 同比", row, "NATIONAL_SAME"));
        }
        if let Some(row) = latest_row(&ppi.unwrap_or_default()) {
            items.push(macro_pct("PPI 同比", row, "BASE_SAME"));
        }
        if let Some(row) = latest_row(&pmi.unwrap_or_default()) {
            items.push(macro_value("制造业 PMI", row, "MAKE_INDEX"));
        }
        if let Some(row) = latest_row(&m2.unwrap_or_default()) {
            items.push(macro_pct("M2 同比", row, "CURRENCY_SAME"));
        }
        if items.is_empty() {
            return Err(anyhow::anyhow!("东方财富宏观数据全部不可用"));
        }
        Ok(MacroPayload {
            items,
            source_note: "来源：东方财富 / 国家统计局".to_string(),
            as_of_utc: Utc::now(),
        })
    }
}

pub fn parse_sectors(json: &Value) -> Result<SectorPayload> {
    let rows = rows(json)?;
    let sectors = rows
        .iter()
        .filter_map(|row| {
            let name = row.get("f14")?.as_str()?.to_string();
            Some(SectorItem {
                code: row
                    .get("f12")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name,
                change_pct: number(row, "f3").unwrap_or(0.0),
                leading: row
                    .get("f128")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                source: "东方财富行业板块".to_string(),
            })
        })
        .collect::<Vec<_>>();
    if sectors.is_empty() {
        return Err(anyhow::anyhow!("行业板块数据为空"));
    }
    Ok(SectorPayload {
        sectors,
        source_note: "来源：东方财富行业板块".to_string(),
        as_of_utc: Utc::now(),
    })
}

pub fn parse_movers(json: &Value) -> Result<Vec<MoverItem>> {
    let rows = rows(json)?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let name = row.get("f14")?.as_str()?.to_string();
            Some(MoverItem {
                symbol: row
                    .get("f12")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name,
                price: number(row, "f2").unwrap_or(0.0),
                change_pct: number(row, "f3").unwrap_or(0.0),
                amount: number(row, "f6").unwrap_or(0.0),
            })
        })
        .collect())
}

pub fn parse_news(json: &Value) -> Result<NewsPayload> {
    let items = json
        .get("data")
        .and_then(|data| data.get("fastNewsList"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("快讯响应为空"))?
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let code = item.get("code").and_then(Value::as_str).unwrap_or_default();
            Some(NewsItem {
                title,
                summary: item
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                time: item
                    .get("showTime")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: if code.is_empty() {
                    String::new()
                } else {
                    format!("https://finance.eastmoney.com/a/{code}.html")
                },
            })
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return Err(anyhow::anyhow!("快讯数据为空"));
    }
    Ok(NewsPayload {
        items,
        source_note: "来源：东方财富 7×24 快讯".to_string(),
        as_of_utc: Utc::now(),
    })
}

fn rows(json: &Value) -> Result<&Vec<Value>> {
    json.get("data")
        .and_then(|data| data.get("diff"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("响应缺少 data.diff"))
}

fn latest_row(json: &Value) -> Option<&Value> {
    json.get("result")
        .and_then(|result| result.get("data"))
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
}

fn macro_pct(label: &str, row: &Value, key: &str) -> MacroItem {
    let value = number(row, key);
    MacroItem {
        label: label.to_string(),
        value: value
            .map(|value| format!("{:+.2}%", value))
            .unwrap_or_else(|| "数据不足".to_string()),
        note: row
            .get("TIME")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn macro_value(label: &str, row: &Value, key: &str) -> MacroItem {
    let value = number(row, key);
    MacroItem {
        label: label.to_string(),
        value: value
            .map(|value| format!("{:.1}", value))
            .unwrap_or_else(|| "数据不足".to_string()),
        note: row
            .get("TIME")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

#[async_trait]
impl CnSentimentProvider for EastmoneyProvider {
    fn name(&self) -> &'static str {
        "东方财富"
    }

    async fn sentiment(&self) -> Result<CnSentimentRaw> {
        let tz = Market::Cn.tz();
        let mut date = Utc::now().with_timezone(&tz).date_naive();
        for _ in 0..10 {
            if is_trading_day(Market::Cn, date) {
                break;
            }
            date -= Duration::days(1);
        }
        let date_str = date.format("%Y%m%d").to_string();
        let breadth = self
            .push2_json(
                "/api/qt/ulist.np/get?fltt=2&secids=1.000001,0.399106&fields=f12,f14,f104,f105,f106,f6",
                &[("Referer", "https://quote.eastmoney.com/")],
            )
            .await
            .with_context(|| "东方财富涨跌家数请求失败")?;

        let pool = |endpoint: &str, pagesize: i64| {
            format!(
                "{}/{}?ut=7eea3edcaed734bea9cbfc24409ed989&dpt=wz.ztzt&Pageindex=0&pagesize={pagesize}&sort=fbt:asc&date={date_str}",
                self.push2ex_base.trim_end_matches('/'),
                endpoint
            )
        };
        let zt = self
            .http
            .get_json(&pool("getTopicZTPool", 300), &[])
            .await
            .ok();
        let dt = self
            .http
            .get_json(&pool("getTopicDTPool", 1), &[])
            .await
            .ok();
        let zb = self
            .http
            .get_json(&pool("getTopicZBPool", 1), &[])
            .await
            .ok();
        parse_sentiment(&breadth, zt.as_ref(), dt.as_ref(), zb.as_ref())
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

        let stock_path = format!(
            "/api/qt/stock/get?secid={}&fltt=2&fields=f43,f84,f162",
            secid(symbol)?
        );
        let mut stock = self
            .push2_json(&stock_path, &[("Referer", "https://quote.eastmoney.com/")])
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(
                    symbol,
                    error = %format!("{error:#}"),
                    "东财估值/股本数据全部失败，尝试腾讯"
                );
                serde_json::json!({})
            });
        let missing_valuation = stock
            .get("data")
            .map(|data| {
                number(data, "f43").is_none()
                    || number(data, "f84").is_none()
                    || number(data, "f162").is_none()
            })
            .unwrap_or(true);
        if missing_valuation {
            if let Some(tencent) = &self.tencent {
                if let Ok(valuation) = tencent.valuation(Market::Cn, symbol).await {
                    tracing::info!(symbol, "使用腾讯估值数据补齐 PE/股本/价格");
                    stock = serde_json::json!({
                        "data": {
                            "f43": valuation.price,
                            "f84": valuation.total_shares,
                            "f162": valuation.pe,
                        }
                    });
                }
            }
        }

        parse_fundamentals(&main, &dividends, &stock, symbol, market)
    }
}

pub fn parse_sentiment(
    breadth: &Value,
    zt: Option<&Value>,
    dt: Option<&Value>,
    zb: Option<&Value>,
) -> Result<CnSentimentRaw> {
    let rows = breadth
        .get("data")
        .and_then(|data| data.get("diff"))
        .and_then(|diff| diff.as_array())
        .ok_or_else(|| anyhow::anyhow!("响应缺少涨跌统计"))?;
    let mut up = 0;
    let mut down = 0;
    let mut flat = 0;
    let mut amount = 0.0;
    for row in rows {
        up += number(row, "f104").unwrap_or(0.0) as i64;
        down += number(row, "f105").unwrap_or(0.0) as i64;
        flat += number(row, "f106").unwrap_or(0.0) as i64;
        amount += number(row, "f6").unwrap_or(0.0);
    }
    if up + down + flat == 0 {
        return Err(anyhow::anyhow!("涨跌家数全为 0，数据可能不可用"));
    }

    let mut missing = vec![
        "量能变化".to_string(),
        "涨跌中位数".to_string(),
        "新高新低".to_string(),
    ];
    let limit_up = zt
        .and_then(|value| value.get("data"))
        .and_then(|data| data.get("tc"))
        .and_then(Value::as_i64);
    let limit_down = dt
        .and_then(|value| value.get("data"))
        .and_then(|data| data.get("tc"))
        .and_then(Value::as_i64);
    let broken_boards = zb
        .and_then(|value| value.get("data"))
        .and_then(|data| data.get("tc"))
        .and_then(Value::as_i64);
    let max_streak = zt
        .and_then(|value| value.get("data"))
        .and_then(|data| data.get("pool"))
        .and_then(Value::as_array)
        .map(|pool| {
            pool.iter()
                .filter_map(|item| item.get("lbc").and_then(Value::as_i64))
                .max()
                .unwrap_or(0)
        });

    if limit_up.is_none() || limit_down.is_none() {
        missing.push("涨停跌停".to_string());
    }
    if broken_boards.is_none() {
        missing.push("炸板率".to_string());
    }
    if max_streak.is_none() {
        missing.push("连板高度".to_string());
    }

    Ok(CnSentimentRaw {
        up,
        down,
        flat,
        limit_up: limit_up.unwrap_or(0),
        limit_down: limit_down.unwrap_or(0),
        broken_boards: broken_boards.unwrap_or(0),
        max_streak: max_streak.unwrap_or(0),
        amount,
        amount_change_pct: 0.0,
        median_change: 0.0,
        new_high: 0,
        new_low: 0,
        main_net_inflow: None,
        missing,
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
        version: crate::models::FUNDAMENTALS_VERSION,
        data_complete: price.is_some() && total_shares.is_some(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_breadth_and_limit_fields() {
        let breadth: Value = serde_json::from_str(
            r#"{"data":{"diff":[{"f104":1849,"f105":440,"f106":68,"f6":953265155867.1},{"f104":2547,"f105":331,"f106":60,"f6":1084426578752.9}]}}"#,
        )
        .unwrap();
        let zt: Value =
            serde_json::from_str(r#"{"data":{"tc":118,"pool":[{"lbc":1},{"lbc":5},{"lbc":3}]}}"#)
                .unwrap();
        let dt: Value = serde_json::from_str(r#"{"data":{"tc":10}}"#).unwrap();
        let zb: Value = serde_json::from_str(r#"{"data":{"tc":33}}"#).unwrap();
        let raw = parse_sentiment(&breadth, Some(&zt), Some(&dt), Some(&zb)).expect("parse");
        assert_eq!(raw.up, 4396);
        assert_eq!(raw.down, 771);
        assert_eq!(raw.flat, 128);
        assert_eq!(raw.limit_up, 118);
        assert_eq!(raw.limit_down, 10);
        assert_eq!(raw.broken_boards, 33);
        assert_eq!(raw.max_streak, 5);
        assert!(raw.missing.contains(&"涨跌中位数".to_string()));
        assert!(!raw.missing.contains(&"涨停跌停".to_string()));
    }

    #[test]
    fn missing_data_is_error() {
        let breadth: Value = serde_json::from_str(r#"{"data":{}}"#).unwrap();
        assert!(parse_sentiment(&breadth, None, None, None).is_err());
    }

    #[test]
    fn parses_sectors_and_movers() {
        let sectors: Value = serde_json::from_str(
            r#"{"data":{"diff":[{"f12":"BK1326","f14":"半导体设备","f3":15.22,"f128":"托伦斯"},{"f12":"BK1325","f14":"半导体材料","f3":11.89,"f128":"先导基电"}]}}"#,
        )
        .unwrap();
        let payload = parse_sectors(&sectors).unwrap();
        assert_eq!(payload.sectors.len(), 2);
        assert_eq!(payload.sectors[0].name, "半导体设备");
        assert!((payload.sectors[0].change_pct - 15.22).abs() < 1e-6);

        let movers: Value = serde_json::from_str(
            r#"{"data":{"diff":[{"f12":"688517","f14":"金冠电气","f2":21.46,"f3":20.02,"f6":348222307}]}}"#,
        )
        .unwrap();
        let rows = parse_movers(&movers).unwrap();
        assert_eq!(rows[0].symbol, "688517");
        assert!((rows[0].change_pct - 20.02).abs() < 1e-6);
    }

    #[test]
    fn parses_news() {
        let news: Value = serde_json::from_str(
            r#"{"data":{"fastNewsList":[{"title":"测试快讯","summary":"摘要","showTime":"2026-11-04 22:01:37","code":"202611043922272811"}]}}"#,
        )
        .unwrap();
        let payload = parse_news(&news).unwrap();
        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].title, "测试快讯");
        assert!(payload.items[0].url.contains("202611043922272811"));
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
