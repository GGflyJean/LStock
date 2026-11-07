use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Market {
    Cn,
    Us,
}

impl Market {
    pub const ALL: [Market; 2] = [Market::Cn, Market::Us];

    pub fn code(self) -> &'static str {
        match self {
            Market::Cn => "cn",
            Market::Us => "us",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Market::Cn => "A股",
            Market::Us => "美股",
        }
    }

    pub fn tz(self) -> Tz {
        match self {
            Market::Cn => chrono_tz::Asia::Shanghai,
            Market::Us => chrono_tz::America::New_York,
        }
    }

    pub fn tz_label(self) -> &'static str {
        match self {
            Market::Cn => "CST",
            Market::Us => "ET",
        }
    }
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Market {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cn" | "CN" | "a" | "A" => Ok(Market::Cn),
            "us" | "US" => Ok(Market::Us),
            other => Err(format!("未知市场 {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    Watch,
    Index,
    Sentiment,
    Ranking,
    Sectors,
    Movers,
    News,
    Macro,
}

impl Tab {
    pub const ALL: [Tab; 8] = [
        Tab::Watch,
        Tab::Index,
        Tab::Sentiment,
        Tab::Ranking,
        Tab::Sectors,
        Tab::Movers,
        Tab::News,
        Tab::Macro,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Tab::Watch => "watch",
            Tab::Index => "index",
            Tab::Sentiment => "sentiment",
            Tab::Ranking => "ranking",
            Tab::Sectors => "sectors",
            Tab::Movers => "movers",
            Tab::News => "news",
            Tab::Macro => "macro",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Watch => "自选",
            Tab::Index => "指数",
            Tab::Sentiment => "情绪",
            Tab::Ranking => "策略榜单",
            Tab::Sectors => "板块",
            Tab::Movers => "涨跌榜",
            Tab::News => "新闻",
            Tab::Macro => "宏观",
        }
    }

    pub fn supported(self) -> bool {
        true
    }
}

impl fmt::Display for Tab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl FromStr for Tab {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Tab::ALL
            .into_iter()
            .find(|tab| tab.key() == value)
            .ok_or_else(|| format!("未知 tab {value}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Board {
    Short,
    Long,
    Dividend,
}

impl Board {
    pub const ALL: [Board; 3] = [Board::Short, Board::Long, Board::Dividend];

    pub fn key(self) -> &'static str {
        match self {
            Board::Short => "short",
            Board::Long => "long",
            Board::Dividend => "dividend",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Board::Short => "短期投资",
            Board::Long => "长期投资",
            Board::Dividend => "股息投资价值",
        }
    }
}

impl FromStr for Board {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Board::ALL
            .into_iter()
            .find(|board| board.key() == value)
            .ok_or_else(|| format!("未知榜单 {value}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeKey {
    pub market: Market,
    pub tab: Tab,
    pub board: Option<Board>,
}

impl ScopeKey {
    pub fn new(market: Market, tab: Tab, board: Option<Board>) -> Self {
        let board = if tab == Tab::Ranking {
            Some(board.unwrap_or(Board::Short))
        } else {
            None
        };
        Self { market, tab, board }
    }

    pub fn scope(&self) -> String {
        match self.tab {
            Tab::Ranking => format!("ranking:{}", self.board.unwrap_or(Board::Short).key()),
            other => other.key().to_string(),
        }
    }

    pub fn board_or_short(&self) -> Board {
        self.board.unwrap_or(Board::Short)
    }
}

impl fmt::Display for ScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.market.code(), self.scope())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DelayKind {
    Realtime,
    Delayed,
    Daily,
}

impl DelayKind {
    pub fn label(self) -> &'static str {
        match self {
            DelayKind::Realtime => "实时",
            DelayKind::Delayed => "延迟",
            DelayKind::Daily => "日频",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub market: Market,
    pub symbol: String,
    pub name: String,
    pub price: f64,
    #[serde(default)]
    pub prev_close: f64,
    #[serde(default)]
    pub change: f64,
    #[serde(default)]
    pub change_pct: f64,
    #[serde(default)]
    pub volume: f64,
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub volume_ratio: Option<f64>,
    #[serde(default)]
    pub turnover: Option<f64>,
    #[serde(default)]
    pub fund_flow: Option<f64>,
    #[serde(default)]
    pub breakout_pct: Option<f64>,
    #[serde(default)]
    pub volatility: Option<f64>,
    pub source: String,
    pub delay: DelayKind,
    pub as_of_utc: DateTime<Utc>,
}

impl Quote {
    pub fn symbol_label(&self) -> String {
        display_symbol(self.market, &self.symbol)
    }

    pub fn price_text(&self) -> String {
        format!("{:.2}", self.price)
    }

    pub fn change_text(&self) -> String {
        format!("{:+.2}", self.change)
    }

    pub fn pct_text(&self) -> String {
        format!("{:+.2}%", self.change_pct)
    }

    pub fn change_class(&self) -> &'static str {
        if self.change_pct > 0.0 {
            "up"
        } else if self.change_pct < 0.0 {
            "down"
        } else {
            "flat"
        }
    }

    pub fn source_label(&self) -> String {
        format!("{} · {}", self.source, self.delay.label())
    }

    pub fn volume_text(&self) -> String {
        if self.market == Market::Us && self.volume > 0.0 {
            return format!("{:.0}", self.volume);
        }
        if self.amount > 0.0 {
            format!("{:.2} 亿", self.amount / 1_0000_0000.0)
        } else if self.volume > 0.0 {
            format!("{:.0}", self.volume)
        } else {
            "-".to_string()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchPayload {
    pub quotes: Vec<Quote>,
    pub missing: Vec<String>,
    pub source_note: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPayload {
    pub indices: Vec<Quote>,
    pub source_note: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentimentMetric {
    pub key: String,
    pub label: String,
    pub value: String,
    pub raw: Option<f64>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentimentReport {
    pub market: Market,
    pub temperature: f64,
    pub band: String,
    pub metrics: Vec<SentimentMetric>,
    pub missing: Vec<String>,
    pub source: String,
    pub delay: DelayKind,
    pub as_of_utc: DateTime<Utc>,
}

impl SentimentReport {
    pub fn temperature_text(&self) -> String {
        format!("{:.1}", self.temperature)
    }

    pub fn band_class(&self) -> &'static str {
        match self.temperature {
            value if value < 20.0 => "cold",
            value if value < 40.0 => "cool",
            value if value < 60.0 => "neutral",
            value if value < 80.0 => "warm",
            _ => "hot",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactorScore {
    pub key: String,
    pub label: String,
    pub raw: Option<f64>,
    pub z: f64,
    pub weight: f64,
    pub contribution: f64,
}

impl FactorScore {
    pub fn raw_text(&self) -> String {
        self.raw
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn z_text(&self) -> String {
        format!("{:+.2}", self.z)
    }

    pub fn weight_text(&self) -> String {
        format!("{:.0}%", self.weight * 100.0)
    }

    pub fn contribution_text(&self) -> String {
        format!("{:+.2}", self.contribution)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingItem {
    pub rank: i64,
    pub symbol: String,
    pub name: String,
    pub score: f64,
    pub factors: Vec<FactorScore>,
}

impl RankingItem {
    pub fn score_text(&self) -> String {
        format!("{:.1}", self.score)
    }
}

impl InsufficientItem {
    pub fn missing_text(&self) -> String {
        self.missing.join("、")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailValue {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsufficientItem {
    pub symbol: String,
    pub name: String,
    pub missing: Vec<String>,
    #[serde(default)]
    pub factors: Vec<DetailValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingPayload {
    pub board: Board,
    pub market: Market,
    pub as_of_utc: DateTime<Utc>,
    pub items: Vec<RankingItem>,
    pub insufficient: Vec<InsufficientItem>,
    pub pool_size: usize,
    pub weights_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fundamentals {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub data_complete: bool,
    pub symbol: String,
    pub market: Market,
    #[serde(default)]
    pub revenue_growth: Option<f64>,
    #[serde(default)]
    pub eps_growth: Option<f64>,
    #[serde(default)]
    pub roe: Option<f64>,
    #[serde(default)]
    pub roic: Option<f64>,
    #[serde(default)]
    pub gross_margin_trend: Option<f64>,
    #[serde(default)]
    pub free_cash_flow: Option<f64>,
    #[serde(default)]
    pub debt_to_equity: Option<f64>,
    #[serde(default)]
    pub valuation_percentile: Option<f64>,
    #[serde(default)]
    pub dividend_yield: Option<f64>,
    #[serde(default)]
    pub payout_ratio: Option<f64>,
    #[serde(default)]
    pub dividend_years: Option<f64>,
    #[serde(default)]
    pub dividend_growth: Option<f64>,
    #[serde(default)]
    pub fcf_coverage: Option<f64>,
}

impl Fundamentals {
    pub fn missing_for(&self, board: Board) -> Vec<String> {
        let mut missing = Vec::new();
        match board {
            Board::Short => {}
            Board::Long => {
                for (_key, label, value) in [
                    ("revenue_growth", "营收增速", self.revenue_growth),
                    ("eps_growth", "EPS 增速", self.eps_growth),
                    ("roe", "ROE", self.roe),
                    ("roic", "ROIC", self.roic),
                    ("free_cash_flow", "自由现金流", self.free_cash_flow),
                    ("debt_to_equity", "杠杆", self.debt_to_equity),
                    (
                        "valuation_percentile",
                        "估值(PE)",
                        self.valuation_percentile,
                    ),
                ] {
                    if value.is_none() {
                        missing.push(label.to_string());
                    }
                }
            }
            Board::Dividend => {
                for (_key, label, value) in [
                    ("dividend_yield", "股息率", self.dividend_yield),
                    ("payout_ratio", "派息率", self.payout_ratio),
                    ("dividend_years", "连续分红年数", self.dividend_years),
                    ("dividend_growth", "股息增速", self.dividend_growth),
                    ("fcf_coverage", "FCF 覆盖", self.fcf_coverage),
                    (
                        "valuation_percentile",
                        "估值(PE)",
                        self.valuation_percentile,
                    ),
                ] {
                    if value.is_none() {
                        missing.push(label.to_string());
                    }
                }
            }
        }
        missing
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorItem {
    pub code: String,
    pub name: String,
    pub change_pct: f64,
    pub leading: String,
    pub source: String,
}

impl SectorItem {
    pub fn pct_text(&self) -> String {
        format!("{:+.2}%", self.change_pct)
    }

    pub fn change_class(&self) -> &'static str {
        if self.change_pct > 0.0 {
            "up"
        } else if self.change_pct < 0.0 {
            "down"
        } else {
            "flat"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorPayload {
    pub sectors: Vec<SectorItem>,
    pub source_note: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoverItem {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change_pct: f64,
    pub amount: f64,
}

impl MoverItem {
    pub fn price_text(&self) -> String {
        format!("{:.2}", self.price)
    }

    pub fn pct_text(&self) -> String {
        format!("{:+.2}%", self.change_pct)
    }

    pub fn change_class(&self) -> &'static str {
        if self.change_pct > 0.0 {
            "up"
        } else if self.change_pct < 0.0 {
            "down"
        } else {
            "flat"
        }
    }

    pub fn amount_text(&self) -> String {
        if self.amount >= 1_0000_0000.0 {
            format!("{:.2} 亿", self.amount / 1_0000_0000.0)
        } else if self.amount > 0.0 {
            format!("{:.0}", self.amount)
        } else {
            "-".to_string()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoversPayload {
    pub gainers: Vec<MoverItem>,
    pub losers: Vec<MoverItem>,
    pub source_note: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub title: String,
    pub summary: String,
    pub time: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsPayload {
    pub items: Vec<NewsItem>,
    pub source_note: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroPoint {
    pub date: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroItem {
    pub label: String,
    pub value: String,
    pub note: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub frequency: String,
    #[serde(default)]
    pub series: Vec<MacroPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroPayload {
    pub items: Vec<MacroItem>,
    pub source_note: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub market: Market,
    pub scope: String,
    pub as_of_utc: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Scheduled,
    Manual,
    Startup,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Scheduled => "scheduled",
            Trigger::Manual => "manual",
            Trigger::Startup => "startup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefreshStatus {
    Running,
    Success,
    Failed,
    Skipped,
}

impl RefreshStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshStatus::Running => "running",
            RefreshStatus::Success => "success",
            RefreshStatus::Failed => "failed",
            RefreshStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRun {
    pub id: i64,
    pub market: Market,
    pub scope: String,
    pub trigger: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub as_of_utc: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchItem {
    pub id: i64,
    pub market: Market,
    pub symbol: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CnSentimentRaw {
    pub up: i64,
    pub down: i64,
    pub flat: i64,
    pub limit_up: i64,
    pub limit_down: i64,
    pub broken_boards: i64,
    pub max_streak: i64,
    pub amount: f64,
    pub amount_change_pct: f64,
    pub median_change: f64,
    pub new_high: i64,
    pub new_low: i64,
    #[serde(default)]
    pub main_net_inflow: Option<f64>,
    #[serde(default)]
    pub missing: Vec<String>,
    pub source: String,
    pub as_of_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsSentimentRaw {
    pub vix: Option<f64>,
    pub vix_change_pct: Option<f64>,
    pub advance: Option<f64>,
    pub decline: Option<f64>,
    pub new_high: Option<f64>,
    pub new_low: Option<f64>,
    pub pcr: Option<f64>,
    pub ten_year: Option<f64>,
    pub dxy: Option<f64>,
    pub source: String,
    pub as_of_utc: DateTime<Utc>,
}

pub const FUNDAMENTALS_VERSION: u32 = 2;

pub fn normalize_symbol(market: Market, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("代码不能为空".to_string());
    }
    match market {
        Market::Cn => {
            let lower = raw.to_ascii_lowercase();
            let (prefix, digits) = if let Some(value) = lower.strip_prefix("sh") {
                ("sh", value)
            } else if let Some(value) = lower.strip_prefix("sz") {
                ("sz", value)
            } else if let Some(value) = lower.strip_suffix(".sh") {
                ("sh", value)
            } else if let Some(value) = lower.strip_suffix(".sz") {
                ("sz", value)
            } else {
                let exchange = match lower.chars().next() {
                    Some('6') | Some('5') => "sh",
                    Some('0') | Some('3') | Some('1') => "sz",
                    _ => return Err(format!("无法识别的 A股代码 {raw}")),
                };
                (exchange, lower.as_str())
            };
            if digits.len() != 6 || !digits.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(format!("无法识别的 A股代码 {raw}"));
            }
            Ok(format!("{prefix}{digits}"))
        }
        Market::Us => {
            let upper = raw
                .trim_start_matches("us")
                .trim_start_matches("US")
                .to_ascii_uppercase();
            if upper.is_empty()
                || !upper
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '^'))
            {
                return Err(format!("无法识别的美股代码 {raw}"));
            }
            Ok(upper)
        }
    }
}

pub fn display_symbol(market: Market, canonical: &str) -> String {
    match market {
        Market::Cn => canonical
            .strip_prefix("sh")
            .or_else(|| canonical.strip_prefix("sz"))
            .unwrap_or(canonical)
            .to_string(),
        Market::Us => canonical.to_string(),
    }
}

pub fn tencent_symbol(market: Market, canonical: &str) -> String {
    match market {
        Market::Cn => canonical.to_string(),
        Market::Us => format!("us{canonical}"),
    }
}

pub fn sina_symbol(market: Market, canonical: &str) -> String {
    match market {
        Market::Cn => canonical.to_string(),
        Market::Us => format!("gb_{}", canonical.to_ascii_lowercase()),
    }
}

pub fn yahoo_symbol(market: Market, canonical: &str) -> String {
    match market {
        Market::Cn => {
            if let Some(digits) = canonical.strip_prefix("sh") {
                format!("{digits}.SS")
            } else if let Some(digits) = canonical.strip_prefix("sz") {
                format!("{digits}.SZ")
            } else {
                canonical.to_string()
            }
        }
        Market::Us => canonical.to_string(),
    }
}

#[cfg(test)]
mod serialization_tests {
    use super::*;

    #[test]
    fn quote_serializes_source_delay_and_as_of() {
        let quote = Quote {
            market: Market::Us,
            symbol: "AAPL".to_string(),
            name: "Apple".to_string(),
            price: 210.5,
            prev_close: 208.0,
            change: 2.5,
            change_pct: 1.2,
            volume: 1000.0,
            amount: 0.0,
            volume_ratio: None,
            turnover: None,
            fund_flow: None,
            breakout_pct: None,
            volatility: None,
            source: "Yahoo Finance".to_string(),
            delay: DelayKind::Delayed,
            as_of_utc: Utc::now(),
        };
        let value = serde_json::to_value(&quote).expect("serialize");
        assert_eq!(value["source"], "Yahoo Finance");
        assert_eq!(value["delay"], "delayed");
        assert!(value["as_of_utc"].is_string());
        let roundtrip: Quote = serde_json::from_value(value).expect("deserialize");
        assert_eq!(roundtrip.market, Market::Us);
        assert_eq!(roundtrip.delay, DelayKind::Delayed);
    }
}

#[cfg(test)]
mod symbol_tests {
    use super::*;

    #[test]
    fn normalizes_cn_symbols() {
        assert_eq!(normalize_symbol(Market::Cn, "600519").unwrap(), "sh600519");
        assert_eq!(normalize_symbol(Market::Cn, "000001").unwrap(), "sz000001");
        assert_eq!(normalize_symbol(Market::Cn, "300750").unwrap(), "sz300750");
        assert_eq!(
            normalize_symbol(Market::Cn, "SH600519").unwrap(),
            "sh600519"
        );
        assert_eq!(
            normalize_symbol(Market::Cn, "600519.SH").unwrap(),
            "sh600519"
        );
        assert_eq!(display_symbol(Market::Cn, "sh600519"), "600519");
        assert!(normalize_symbol(Market::Cn, "abc").is_err());
    }

    #[test]
    fn normalizes_us_symbols() {
        assert_eq!(normalize_symbol(Market::Us, "aapl").unwrap(), "AAPL");
        assert_eq!(normalize_symbol(Market::Us, "usAAPL").unwrap(), "AAPL");
        assert_eq!(display_symbol(Market::Us, "AAPL"), "AAPL");
        assert!(normalize_symbol(Market::Us, "bad symbol").is_err());
    }

    #[test]
    fn provider_symbols_convert() {
        assert_eq!(tencent_symbol(Market::Cn, "sh600519"), "sh600519");
        assert_eq!(tencent_symbol(Market::Us, "AAPL"), "usAAPL");
        assert_eq!(sina_symbol(Market::Us, "AAPL"), "gb_aapl");
        assert_eq!(yahoo_symbol(Market::Cn, "sh600519"), "600519.SS");
        assert_eq!(yahoo_symbol(Market::Cn, "sz000001"), "000001.SZ");
        assert_eq!(yahoo_symbol(Market::Us, "^IXIC"), "^IXIC");
    }
}
