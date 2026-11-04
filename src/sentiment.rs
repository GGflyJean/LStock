use crate::models::{
    CnSentimentRaw, DelayKind, Market, SentimentMetric, SentimentReport, UsSentimentRaw,
};

pub fn compute_cn(raw: &CnSentimentRaw) -> SentimentReport {
    let total = (raw.up + raw.down + raw.flat).max(1) as f64;
    let breadth = raw.up as f64 / total * 100.0;
    let limit_score = (50.0 + (raw.limit_up - raw.limit_down) as f64).clamp(0.0, 100.0);
    let broken_rate = if raw.limit_up + raw.broken_boards > 0 {
        raw.broken_boards as f64 / (raw.limit_up + raw.broken_boards) as f64 * 100.0
    } else {
        0.0
    };
    let broken_score = (100.0 - broken_rate).clamp(0.0, 100.0);
    let streak_score = (raw.max_streak as f64 * 15.0).clamp(0.0, 100.0);
    let amount_score = (50.0 + raw.amount_change_pct * 2.0).clamp(0.0, 100.0);
    let median_score = (50.0 + raw.median_change * 20.0).clamp(0.0, 100.0);
    let new_score = if raw.new_high + raw.new_low > 0 {
        (50.0 + (raw.new_high - raw.new_low) as f64 / (raw.new_high + raw.new_low) as f64 * 50.0)
            .clamp(0.0, 100.0)
    } else {
        50.0
    };

    let temperature = breadth * 0.30
        + limit_score * 0.20
        + broken_score * 0.10
        + streak_score * 0.10
        + amount_score * 0.10
        + median_score * 0.10
        + new_score * 0.10;

    let metrics = vec![
        metric(
            "breadth",
            "涨跌家数",
            format!("涨 {} / 平 {} / 跌 {}", raw.up, raw.flat, raw.down),
            Some(breadth),
            "上涨占比",
        ),
        metric(
            "limit",
            "涨停 / 跌停",
            format!("{} / {}", raw.limit_up, raw.limit_down),
            Some(limit_score),
            "",
        ),
        metric(
            "broken",
            "炸板率",
            format!("{broken_rate:.1}%"),
            Some(broken_rate),
            "越低越好",
        ),
        metric(
            "streak",
            "连板高度",
            format!("{} 板", raw.max_streak),
            Some(raw.max_streak as f64),
            "",
        ),
        metric(
            "amount",
            "两市成交额",
            format!("{:.2} 万亿", raw.amount / 1_0000_0000_0000.0),
            Some(raw.amount),
            format!("量能变化 {:+.1}%", raw.amount_change_pct),
        ),
        metric(
            "median",
            "涨跌中位数",
            format!("{:+.2}%", raw.median_change),
            Some(raw.median_change),
            "",
        ),
        metric(
            "new",
            "新高 / 新低",
            format!("{} / {}", raw.new_high, raw.new_low),
            Some(raw.new_high as f64 - raw.new_low as f64),
            "",
        ),
    ];
    let mut metrics = metrics;
    if let Some(inflow) = raw.main_net_inflow {
        metrics.push(metric(
            "fund_flow",
            "主力净流入",
            format!("{:.2} 亿", inflow / 1_0000_0000.0),
            Some(inflow),
            "",
        ));
    }

    SentimentReport {
        market: Market::Cn,
        temperature: round1(temperature),
        band: band(temperature).to_string(),
        metrics,
        missing: Vec::new(),
        source: raw.source.clone(),
        delay: DelayKind::Realtime,
        as_of_utc: raw.as_of_utc,
    }
}

pub fn compute_us(raw: &UsSentimentRaw) -> SentimentReport {
    let mut components: Vec<(f64, f64)> = Vec::new();
    let mut metrics = Vec::new();
    let mut missing = Vec::new();

    if let Some(vix) = raw.vix {
        let score = (100.0 - (vix - 10.0) * 5.0).clamp(0.0, 100.0);
        components.push((score, 0.25));
        metrics.push(metric(
            "vix",
            "VIX",
            format!("{vix:.2}"),
            Some(vix),
            raw.vix_change_pct
                .map(|value| format!("{:+.1}%", value))
                .unwrap_or_else(|| "变化未知".to_string()),
        ));
    } else {
        missing.push("VIX".to_string());
    }

    if let (Some(advance), Some(decline)) = (raw.advance, raw.decline) {
        if advance + decline > 0.0 {
            let score = advance / (advance + decline) * 100.0;
            components.push((score, 0.25));
            metrics.push(metric(
                "breadth",
                "涨跌家数",
                format!("涨 {:.0} / 跌 {:.0}", advance, decline),
                Some(score),
                "上涨占比",
            ));
        }
    } else {
        missing.push("涨跌家数".to_string());
    }

    if let (Some(high), Some(low)) = (raw.new_high, raw.new_low) {
        if high + low > 0.0 {
            let score = (50.0 + (high - low) / (high + low) * 50.0).clamp(0.0, 100.0);
            components.push((score, 0.15));
            metrics.push(metric(
                "new",
                "新高 / 新低",
                format!("{high:.0} / {low:.0}"),
                Some(high - low),
                "",
            ));
        }
    } else {
        missing.push("新高新低".to_string());
    }

    if let Some(pcr) = raw.pcr {
        let score = (50.0 + (1.0 - pcr) * 50.0).clamp(0.0, 100.0);
        components.push((score, 0.10));
        metrics.push(metric("pcr", "PCR", format!("{pcr:.2}"), Some(pcr), ""));
    } else {
        missing.push("PCR".to_string());
    }

    if let Some(ten_year) = raw.ten_year {
        let score = (50.0 - (ten_year - 4.0) * 10.0).clamp(0.0, 100.0);
        components.push((score, 0.125));
        metrics.push(metric(
            "ten_year",
            "10Y 美债",
            format!("{ten_year:.2}%"),
            Some(ten_year),
            "利率上行偏空",
        ));
    } else {
        missing.push("10Y 美债".to_string());
    }

    if let Some(dxy) = raw.dxy {
        let score = (50.0 - (dxy - 100.0) * 5.0).clamp(0.0, 100.0);
        components.push((score, 0.125));
        metrics.push(metric(
            "dxy",
            "美元指数",
            format!("{dxy:.2}"),
            Some(dxy),
            "",
        ));
    } else {
        missing.push("美元指数".to_string());
    }

    let (temperature, total_weight) = components
        .iter()
        .fold((0.0, 0.0), |(sum, weight), (score, item_weight)| {
            (sum + score * item_weight, weight + item_weight)
        });
    let temperature = if total_weight > 0.0 {
        temperature / total_weight
    } else {
        50.0
    };

    SentimentReport {
        market: Market::Us,
        temperature: round1(temperature),
        band: band(temperature).to_string(),
        metrics,
        missing,
        source: raw.source.clone(),
        delay: if raw.source.contains("Yahoo") || raw.source.contains("腾讯") {
            DelayKind::Delayed
        } else {
            DelayKind::Realtime
        },
        as_of_utc: raw.as_of_utc,
    }
}

fn metric(
    key: &str,
    label: &str,
    value: String,
    raw: Option<f64>,
    note: impl Into<String>,
) -> SentimentMetric {
    SentimentMetric {
        key: key.to_string(),
        label: label.to_string(),
        value,
        raw,
        note: note.into(),
    }
}

pub fn band(temperature: f64) -> &'static str {
    match temperature {
        value if value < 20.0 => "冰点",
        value if value < 40.0 => "偏冷",
        value if value < 60.0 => "中性",
        value if value < 80.0 => "偏热",
        _ => "过热",
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn cn_raw() -> CnSentimentRaw {
        CnSentimentRaw {
            up: 3120,
            down: 1890,
            flat: 120,
            limit_up: 58,
            limit_down: 6,
            broken_boards: 22,
            max_streak: 5,
            amount: 1_4200_0000_0000.0,
            amount_change_pct: 8.0,
            median_change: 0.35,
            new_high: 80,
            new_low: 12,
            main_net_inflow: Some(-1_200_000_000.0),
            source: "东方财富".to_string(),
            as_of_utc: Utc::now(),
        }
    }

    #[test]
    fn cn_temperature_is_hot_for_broad_rally() {
        let report = compute_cn(&cn_raw());
        assert!(report.temperature > 55.0);
        assert!(["中性", "偏热", "过热"].contains(&report.band.as_str()));
        assert!(report.metrics.iter().any(|metric| metric.key == "breadth"));
    }

    #[test]
    fn cn_broken_rate_metric() {
        let report = compute_cn(&cn_raw());
        let broken = report
            .metrics
            .iter()
            .find(|metric| metric.key == "broken")
            .expect("broken");
        assert!(broken.value.ends_with('%'));
    }

    #[test]
    fn us_temperature_with_missing_indicators() {
        let raw = UsSentimentRaw {
            vix: Some(18.4),
            vix_change_pct: Some(6.0),
            advance: None,
            decline: None,
            new_high: None,
            new_low: None,
            pcr: None,
            ten_year: Some(4.21),
            dxy: Some(104.2),
            source: "Yahoo Finance".to_string(),
            as_of_utc: Utc::now(),
        };
        let report = compute_us(&raw);
        assert!(report.temperature > 0.0 && report.temperature < 100.0);
        assert!(report.missing.contains(&"PCR".to_string()));
        assert!(report.missing.contains(&"涨跌家数".to_string()));
        assert_eq!(report.delay, DelayKind::Delayed);
    }

    #[test]
    fn bands_are_stable() {
        assert_eq!(band(10.0), "冰点");
        assert_eq!(band(30.0), "偏冷");
        assert_eq!(band(50.0), "中性");
        assert_eq!(band(70.0), "偏热");
        assert_eq!(band(90.0), "过热");
    }
}
