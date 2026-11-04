use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::db::{Db, Instrument};
use crate::models::{
    Board, FactorScore, Fundamentals, InsufficientItem, Market, Quote, RankingItem, RankingPayload,
};

pub const DEFAULT_CN_POOL: &[(&str, &str)] = &[
    ("sh600519", "贵州茅台"),
    ("sh601318", "中国平安"),
    ("sh600036", "招商银行"),
    ("sh601166", "兴业银行"),
    ("sh600900", "长江电力"),
    ("sh601088", "中国神华"),
    ("sh600030", "中信证券"),
    ("sh601012", "隆基绿能"),
    ("sh688981", "中芯国际"),
    ("sh600276", "恒瑞医药"),
    ("sz000333", "美的集团"),
    ("sz000651", "格力电器"),
    ("sz000858", "五粮液"),
    ("sz000001", "平安银行"),
    ("sz300750", "宁德时代"),
    ("sz300059", "东方财富"),
    ("sz300760", "迈瑞医疗"),
    ("sz002594", "比亚迪"),
    ("sz002415", "海康威视"),
    ("sz002475", "立讯精密"),
];

pub const DEFAULT_US_POOL: &[(&str, &str)] = &[
    ("AAPL", "Apple"),
    ("MSFT", "Microsoft"),
    ("NVDA", "NVIDIA"),
    ("AMZN", "Amazon"),
    ("GOOGL", "Alphabet"),
    ("META", "Meta Platforms"),
    ("TSLA", "Tesla"),
    ("AVGO", "Broadcom"),
    ("BRK.B", "Berkshire Hathaway"),
    ("JPM", "JPMorgan Chase"),
    ("V", "Visa"),
    ("MA", "Mastercard"),
    ("UNH", "UnitedHealth"),
    ("XOM", "Exxon Mobil"),
    ("JNJ", "Johnson & Johnson"),
    ("PG", "Procter & Gamble"),
    ("HD", "Home Depot"),
    ("KO", "Coca-Cola"),
    ("PEP", "PepsiCo"),
    ("COST", "Costco"),
];

#[derive(Debug, Clone)]
pub struct FactorSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub weight: f64,
    pub higher_is_better: bool,
}

pub fn specs(board: Board) -> Vec<FactorSpec> {
    let rows: &[(&str, &str, f64, bool)] = match board {
        Board::Short => &[
            ("momentum", "动量", 0.25, true),
            ("relative_strength", "相对强度", 0.15, true),
            ("volume_ratio", "量比", 0.15, true),
            ("turnover", "换手率", 0.10, true),
            ("fund_flow", "资金流", 0.15, true),
            ("breakout", "突破距离", 0.10, true),
            ("volatility", "波动率", 0.10, false),
        ],
        Board::Long => &[
            ("revenue_growth", "营收增速", 0.15, true),
            ("eps_growth", "EPS 增速", 0.15, true),
            ("roe", "ROE", 0.15, true),
            ("roic", "ROIC", 0.10, true),
            ("gross_margin_trend", "毛利率趋势", 0.10, true),
            ("free_cash_flow", "自由现金流", 0.15, true),
            ("debt_to_equity", "杠杆", 0.10, false),
            ("valuation_percentile", "估值分位", 0.10, false),
        ],
        Board::Dividend => &[
            ("dividend_yield", "股息率", 0.25, true),
            ("payout_ratio", "派息率", 0.15, true),
            ("dividend_years", "连续分红年数", 0.15, true),
            ("dividend_growth", "股息增速", 0.15, true),
            ("fcf_coverage", "FCF 覆盖", 0.15, true),
            ("debt_to_equity", "杠杆", 0.075, false),
            ("valuation_percentile", "估值分位", 0.075, false),
        ],
    };
    rows.iter()
        .map(|(key, label, weight, higher_is_better)| FactorSpec {
            key,
            label,
            weight: *weight,
            higher_is_better: *higher_is_better,
        })
        .collect()
}

pub fn apply_weights(board: Board, overrides: &HashMap<String, f64>) -> Vec<FactorSpec> {
    let mut specs = specs(board);
    let mut changed = false;
    for spec in &mut specs {
        if let Some(weight) = overrides.get(spec.key) {
            if weight.is_finite() && *weight >= 0.0 {
                spec.weight = *weight;
                changed = true;
            }
        }
    }
    let total: f64 = specs.iter().map(|spec| spec.weight).sum();
    if changed && total > 0.0 {
        for spec in &mut specs {
            spec.weight /= total;
        }
    }
    specs
}

pub fn load_weights(db: &Db, board: Board) -> Result<Vec<FactorSpec>> {
    let overrides = db
        .get_pref("ranking_weights")?
        .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
        .and_then(|value| value.get(board.key()).cloned())
        .and_then(|value| serde_json::from_value::<HashMap<String, f64>>(value).ok())
        .unwrap_or_default();
    Ok(apply_weights(board, &overrides))
}

pub fn save_weights(db: &Db, board: Board, weights: &HashMap<String, f64>) -> Result<()> {
    let mut all = db
        .get_pref("ranking_weights")?
        .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let object = all
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("权重配置格式错误"))?;
    object.insert(board.key().to_string(), serde_json::to_value(weights)?);
    db.set_pref("ranking_weights", &serde_json::to_string(&all)?)?;
    Ok(())
}

pub fn seed_default_pool(db: &Db) -> Result<usize> {
    let mut count = 0;
    for (symbol, name) in DEFAULT_CN_POOL {
        db.upsert_instrument(Market::Cn, symbol, name)?;
        count += 1;
    }
    for (symbol, name) in DEFAULT_US_POOL {
        db.upsert_instrument(Market::Us, symbol, name)?;
        count += 1;
    }
    Ok(count)
}

pub fn ensure_watch_in_pool(db: &Db, market: Market) -> Result<()> {
    for item in db.watchlist(market)? {
        db.upsert_instrument(market, &item.symbol, &item.name)?;
    }
    Ok(())
}

type ScoredRow = (Instrument, HashMap<String, f64>, f64, Vec<FactorScore>);

pub struct RankingInput<'a> {
    pub board: Board,
    pub market: Market,
    pub as_of: DateTime<Utc>,
    pub pool: &'a [Instrument],
    pub quotes: &'a [Quote],
    pub fundamentals: &'a HashMap<String, Fundamentals>,
    pub specs: Vec<FactorSpec>,
}

pub fn compute(input: RankingInput<'_>) -> RankingPayload {
    let quote_map: HashMap<&str, &Quote> = input
        .quotes
        .iter()
        .map(|quote| (quote.symbol.as_str(), quote))
        .collect();
    let mut rows: Vec<(Instrument, HashMap<String, f64>)> = Vec::new();
    let mut insufficient = Vec::new();

    for instrument in input.pool {
        let quote = quote_map.get(instrument.symbol.as_str()).copied();
        let fundamentals = input.fundamentals.get(&instrument.symbol);
        let mut missing = Vec::new();
        let mut values = HashMap::new();

        match input.board {
            Board::Short => {
                let Some(quote) = quote else {
                    missing.push("缺少行情".to_string());
                    insufficient.push(InsufficientItem {
                        symbol: instrument.symbol.clone(),
                        name: instrument.name.clone(),
                        missing,
                    });
                    continue;
                };
                insert(&mut values, "momentum", Some(quote.change_pct));
                insert(&mut values, "relative_strength", Some(quote.change_pct));
                insert(&mut values, "volume_ratio", quote.volume_ratio);
                insert(&mut values, "turnover", quote.turnover);
                insert(&mut values, "fund_flow", quote.fund_flow);
                insert(&mut values, "breakout", quote.breakout_pct);
                insert(&mut values, "volatility", quote.volatility);
                for spec in &input.specs {
                    if !values.contains_key(spec.key) {
                        missing.push(spec.label.to_string());
                    }
                }
                for core in ["momentum", "volume_ratio", "turnover"] {
                    if !values.contains_key(core) {
                        missing.push(format!("核心因子 {core}"));
                    }
                }
                if missing.iter().any(|item| item.starts_with("核心因子")) {
                    insufficient.push(InsufficientItem {
                        symbol: instrument.symbol.clone(),
                        name: instrument.name.clone(),
                        missing,
                    });
                    continue;
                }
            }
            Board::Long | Board::Dividend => {
                let Some(fundamentals) = fundamentals else {
                    missing.push("缺少基本面".to_string());
                    insufficient.push(InsufficientItem {
                        symbol: instrument.symbol.clone(),
                        name: instrument.name.clone(),
                        missing,
                    });
                    continue;
                };
                missing = fundamentals.missing_for(input.board);
                if !missing.is_empty() {
                    insufficient.push(InsufficientItem {
                        symbol: instrument.symbol.clone(),
                        name: instrument.name.clone(),
                        missing,
                    });
                    continue;
                }
                for spec in &input.specs {
                    if let Some(value) = fundamental_value(fundamentals, spec.key) {
                        if value.is_finite() {
                            values.insert(spec.key.to_string(), value);
                        }
                    }
                }
            }
        }
        rows.push((instrument.clone(), values));
    }

    let mut stats: HashMap<String, (f64, f64)> = HashMap::new();
    for spec in &input.specs {
        let values: Vec<f64> = rows
            .iter()
            .filter_map(|(_, map)| map.get(spec.key).copied())
            .collect();
        if values.is_empty() {
            stats.insert(spec.key.to_string(), (0.0, 0.0));
            continue;
        }
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let variance = values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / values.len() as f64;
        stats.insert(spec.key.to_string(), (mean, variance.sqrt()));
    }

    let mut scored: Vec<ScoredRow> = Vec::new();
    for (instrument, values) in rows {
        let mut factors = Vec::new();
        let mut composite = 0.0;
        for spec in &input.specs {
            let raw = values.get(spec.key).copied();
            let (mean, std) = stats.get(spec.key).copied().unwrap_or((0.0, 0.0));
            let z = match raw {
                Some(value) if std > f64::EPSILON => {
                    let z = (value - mean) / std;
                    if spec.higher_is_better {
                        z
                    } else {
                        -z
                    }
                }
                _ => 0.0,
            };
            let contribution = spec.weight * z;
            composite += contribution;
            factors.push(FactorScore {
                key: spec.key.to_string(),
                label: spec.label.to_string(),
                raw: raw.map(round2),
                z: round4(z),
                weight: round4(spec.weight),
                contribution: round4(contribution),
            });
        }
        scored.push((instrument, values, composite, factors));
    }

    let min = scored
        .iter()
        .map(|(_, _, composite, _)| *composite)
        .fold(f64::INFINITY, f64::min);
    let max = scored
        .iter()
        .map(|(_, _, composite, _)| *composite)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = if min.is_finite() && max.is_finite() && (max - min) > f64::EPSILON {
        max - min
    } else {
        0.0
    };
    for (_, _, composite, _) in &mut scored {
        *composite = if range > 0.0 {
            ((*composite - min) / range * 100.0).clamp(0.0, 100.0)
        } else {
            50.0
        };
    }
    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    let items = scored
        .into_iter()
        .enumerate()
        .map(
            |(index, (instrument, _values, score, factors))| RankingItem {
                rank: index as i64 + 1,
                symbol: instrument.symbol,
                name: instrument.name,
                score: round2(score),
                factors,
            },
        )
        .collect();

    let weights_summary = input
        .specs
        .iter()
        .map(|spec| format!("{} {:.0}%", spec.label, spec.weight * 100.0))
        .collect::<Vec<_>>()
        .join(" · ");

    RankingPayload {
        board: input.board,
        market: input.market,
        as_of_utc: input.as_of,
        items,
        insufficient,
        pool_size: input.pool.len(),
        weights_summary,
    }
}

fn insert(map: &mut HashMap<String, f64>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        if value.is_finite() {
            map.insert(key.to_string(), value);
        }
    }
}

fn fundamental_value(fundamentals: &Fundamentals, key: &str) -> Option<f64> {
    match key {
        "revenue_growth" => fundamentals.revenue_growth,
        "eps_growth" => fundamentals.eps_growth,
        "roe" => fundamentals.roe,
        "roic" => fundamentals.roic,
        "gross_margin_trend" => fundamentals.gross_margin_trend,
        "free_cash_flow" => fundamentals.free_cash_flow,
        "debt_to_equity" => fundamentals.debt_to_equity,
        "valuation_percentile" => fundamentals.valuation_percentile,
        "dividend_yield" => fundamentals.dividend_yield,
        "payout_ratio" => fundamentals.payout_ratio,
        "dividend_years" => fundamentals.dividend_years,
        "dividend_growth" => fundamentals.dividend_growth,
        "fcf_coverage" => fundamentals.fcf_coverage,
        _ => None,
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round4(value: f64) -> f64 {
    (value * 10000.0).round() / 10000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DelayKind;

    fn quote(symbol: &str, change_pct: f64, volume_ratio: f64, turnover: f64) -> Quote {
        Quote {
            market: Market::Cn,
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            price: 10.0,
            prev_close: 9.8,
            change: 0.2,
            change_pct,
            volume: 1000.0,
            amount: 10000.0,
            volume_ratio: Some(volume_ratio),
            turnover: Some(turnover),
            fund_flow: Some(change_pct * 1000.0),
            breakout_pct: Some(change_pct),
            volatility: Some(change_pct.abs()),
            source: "test".to_string(),
            delay: DelayKind::Realtime,
            as_of_utc: Utc::now(),
        }
    }

    fn instrument(symbol: &str) -> Instrument {
        Instrument {
            market: Market::Cn,
            symbol: symbol.to_string(),
            name: symbol.to_string(),
        }
    }

    fn fundamentals(symbol: &str, roe: f64, yield_value: f64) -> Fundamentals {
        Fundamentals {
            version: crate::models::FUNDAMENTALS_VERSION,
            data_complete: true,
            symbol: symbol.to_string(),
            market: Market::Cn,
            revenue_growth: Some(10.0),
            eps_growth: Some(8.0),
            roe: Some(roe),
            roic: Some(12.0),
            gross_margin_trend: Some(1.0),
            free_cash_flow: Some(1_000_000.0),
            debt_to_equity: Some(40.0),
            valuation_percentile: Some(30.0),
            dividend_yield: Some(yield_value),
            payout_ratio: Some(40.0),
            dividend_years: Some(10.0),
            dividend_growth: Some(5.0),
            fcf_coverage: Some(2.0),
        }
    }

    #[test]
    fn short_ranking_sorts_by_score_and_explains_factors() {
        let pool = vec![
            instrument("sh600519"),
            instrument("sh600036"),
            instrument("sh601318"),
        ];
        let quotes = vec![
            quote("sh600519", 1.0, 1.2, 2.0),
            quote("sh600036", 3.0, 2.4, 4.0),
            quote("sh601318", -1.0, 0.8, 1.0),
        ];
        let fundamentals = HashMap::new();
        let payload = compute(RankingInput {
            board: Board::Short,
            market: Market::Cn,
            as_of: Utc::now(),
            pool: &pool,
            quotes: &quotes,
            fundamentals: &fundamentals,
            specs: specs(Board::Short),
        });
        assert_eq!(payload.items.len(), 3);
        assert_eq!(payload.items[0].symbol, "sh600036");
        assert!(payload.items[0].factors.len() >= 3);
        assert!(payload.items[0].score >= payload.items[1].score);
    }

    #[test]
    fn long_ranking_excludes_missing_fundamentals() {
        let pool = vec![instrument("sh600519"), instrument("sh600036")];
        let quotes = vec![
            quote("sh600519", 1.0, 1.2, 2.0),
            quote("sh600036", 3.0, 2.4, 4.0),
        ];
        let mut fundamental_map = HashMap::new();
        fundamental_map.insert("sh600519".to_string(), fundamentals("sh600519", 20.0, 1.0));
        let payload = compute(RankingInput {
            board: Board::Long,
            market: Market::Cn,
            as_of: Utc::now(),
            pool: &pool,
            quotes: &quotes,
            fundamentals: &fundamental_map,
            specs: specs(Board::Long),
        });
        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].symbol, "sh600519");
        assert_eq!(payload.insufficient.len(), 1);
        assert_eq!(payload.insufficient[0].symbol, "sh600036");
    }

    #[test]
    fn weight_override_is_normalized_and_changes_scores() {
        let mut overrides = HashMap::new();
        overrides.insert("momentum".to_string(), 10.0);
        let adjusted = apply_weights(Board::Short, &overrides);
        let total: f64 = adjusted.iter().map(|spec| spec.weight).sum();
        assert!((total - 1.0).abs() < 1e-9);
        let momentum = adjusted.iter().find(|spec| spec.key == "momentum").unwrap();
        assert!(momentum.weight > 0.5);
    }

    #[test]
    fn dividend_ranking_requires_dividend_fields() {
        let pool = vec![instrument("sh600519")];
        let quotes = vec![quote("sh600519", 1.0, 1.2, 2.0)];
        let mut complete = fundamentals("sh600519", 20.0, 3.5);
        let mut map = HashMap::new();
        map.insert("sh600519".to_string(), complete.clone());
        let payload = compute(RankingInput {
            board: Board::Dividend,
            market: Market::Cn,
            as_of: Utc::now(),
            pool: &pool,
            quotes: &quotes,
            fundamentals: &map,
            specs: specs(Board::Dividend),
        });
        assert_eq!(payload.items.len(), 1);
        complete.dividend_years = None;
        let mut map = HashMap::new();
        map.insert("sh600519".to_string(), complete);
        let payload = compute(RankingInput {
            board: Board::Dividend,
            market: Market::Cn,
            as_of: Utc::now(),
            pool: &pool,
            quotes: &quotes,
            fundamentals: &map,
            specs: specs(Board::Dividend),
        });
        assert_eq!(payload.items.len(), 0);
    }
}
