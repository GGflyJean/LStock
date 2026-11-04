use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use crate::config::Config;
use crate::db::Db;
use crate::market::{is_trading_day, next_trigger, NextTrigger};
use crate::models::{Board, Market, RefreshStatus, ScopeKey, Snapshot, Tab, Trigger};
use crate::providers::Providers;
use crate::rankings::{self, RankingInput};
use crate::sentiment;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RefreshOutcome {
    Success {
        scope: String,
        as_of_utc: DateTime<Utc>,
    },
    Merged {
        scope: String,
    },
    Cooldown {
        scope: String,
        remaining_secs: u64,
    },
    Failed {
        scope: String,
        error: String,
    },
}

impl RefreshOutcome {
    pub fn scope(&self) -> String {
        match self {
            RefreshOutcome::Success { scope, .. }
            | RefreshOutcome::Merged { scope }
            | RefreshOutcome::Cooldown { scope, .. }
            | RefreshOutcome::Failed { scope, .. } => scope.clone(),
        }
    }
}

struct ScopeState {
    lock: Arc<AsyncMutex<()>>,
    last_started: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct RefreshService {
    pub db: Db,
    pub config: Arc<Config>,
    pub providers: Providers,
    scopes: Arc<Mutex<HashMap<ScopeKey, Arc<ScopeState>>>>,
    running: Arc<Mutex<HashSet<ScopeKey>>>,
    semaphore: Arc<Semaphore>,
}

impl RefreshService {
    pub fn new(db: Db, config: Arc<Config>, providers: Providers) -> Self {
        let permits = config.max_concurrency.max(1);
        Self {
            db,
            config,
            providers,
            scopes: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(HashSet::new())),
            semaphore: Arc::new(Semaphore::new(permits)),
        }
    }

    fn state_for(&self, key: ScopeKey) -> Arc<ScopeState> {
        let mut scopes = self.scopes.lock().expect("scope lock");
        scopes
            .entry(key)
            .or_insert_with(|| {
                Arc::new(ScopeState {
                    lock: Arc::new(AsyncMutex::new(())),
                    last_started: Mutex::new(None),
                })
            })
            .clone()
    }

    pub fn running_scopes(&self) -> Vec<String> {
        let mut list: Vec<String> = self
            .running
            .lock()
            .map(|set| set.iter().map(|key| key.to_string()).collect())
            .unwrap_or_default();
        list.sort();
        list
    }

    pub async fn request(&self, key: ScopeKey, trigger: Trigger) -> RefreshOutcome {
        let state = self.state_for(key);
        match state.lock.clone().try_lock_owned() {
            Ok(_guard) => {
                if let Ok(mut last) = state.last_started.lock() {
                    if let Some(started) = *last {
                        let elapsed = started.elapsed();
                        let cooldown = Duration::from_secs(self.config.cooldown_secs);
                        if elapsed < cooldown {
                            let remaining = (cooldown - elapsed).as_secs().max(1);
                            return RefreshOutcome::Cooldown {
                                scope: key.to_string(),
                                remaining_secs: remaining,
                            };
                        }
                    }
                    *last = Some(Instant::now());
                }
                if let Ok(mut running) = self.running.lock() {
                    running.insert(key);
                }
                let result = self.run_scope(key, trigger).await;
                if let Ok(mut running) = self.running.lock() {
                    running.remove(&key);
                }
                match result {
                    Ok(as_of_utc) => RefreshOutcome::Success {
                        scope: key.to_string(),
                        as_of_utc,
                    },
                    Err(error) => RefreshOutcome::Failed {
                        scope: key.to_string(),
                        error: format!("{error:#}"),
                    },
                }
            }
            Err(_) => {
                let _guard = state.lock.clone().lock_owned().await;
                RefreshOutcome::Merged {
                    scope: key.to_string(),
                }
            }
        }
    }

    pub async fn request_many(&self, keys: &[ScopeKey], trigger: Trigger) -> Vec<RefreshOutcome> {
        let mut outcomes = Vec::new();
        for key in keys {
            outcomes.push(self.request(*key, trigger).await);
        }
        outcomes
    }

    async fn run_scope(&self, key: ScopeKey, trigger: Trigger) -> Result<DateTime<Utc>> {
        let run_id = self.db.insert_refresh_run(
            key.market,
            &key.scope(),
            trigger.as_str(),
            RefreshStatus::Running.as_str(),
        )?;
        let _permit = self
            .semaphore
            .acquire()
            .await
            .context("获取刷新并发许可失败")?;
        let result = self.build_and_store(key).await;
        match result {
            Ok(as_of) => {
                self.db.finish_refresh_run(
                    run_id,
                    RefreshStatus::Success.as_str(),
                    Some(as_of),
                    None,
                )?;
                Ok(as_of)
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.db.finish_refresh_run(
                    run_id,
                    RefreshStatus::Failed.as_str(),
                    None,
                    Some(&message),
                )?;
                Err(error)
            }
        }
    }

    async fn build_and_store(&self, key: ScopeKey) -> Result<DateTime<Utc>> {
        match key.tab {
            Tab::Watch => {
                let symbols: Vec<String> = self
                    .db
                    .watchlist(key.market)?
                    .into_iter()
                    .map(|item| item.symbol)
                    .collect();
                let payload = self.providers.quotes(key.market, &symbols).await?;
                let as_of = payload.as_of_utc;
                self.store_snapshot(key.market, "watch", as_of, &payload)?;
                Ok(as_of)
            }
            Tab::Index => {
                let payload = self.providers.indices(key.market).await?;
                let as_of = payload.as_of_utc;
                self.store_snapshot(key.market, "index", as_of, &payload)?;
                Ok(as_of)
            }
            Tab::Sentiment => match key.market {
                Market::Cn => {
                    let raw = self.providers.cn_sentiment().await?;
                    let report = sentiment::compute_cn(&raw);
                    let as_of = report.as_of_utc;
                    self.store_snapshot(key.market, "sentiment", as_of, &report)?;
                    Ok(as_of)
                }
                Market::Us => {
                    let raw = self.providers.us_sentiment().await?;
                    let report = sentiment::compute_us(&raw);
                    let as_of = report.as_of_utc;
                    self.store_snapshot(key.market, "sentiment", as_of, &report)?;
                    Ok(as_of)
                }
            },
            Tab::Ranking => {
                let board = key.board_or_short();
                let as_of = Utc::now();
                rankings::seed_default_pool(&self.db)?;
                rankings::ensure_watch_in_pool(&self.db, key.market)?;
                let pool = self.db.pool(key.market)?;
                let symbols: Vec<String> = pool.iter().map(|item| item.symbol.clone()).collect();
                if board != Board::Short {
                    self.refresh_fundamentals(key.market, &symbols).await;
                }
                let quotes = if board == Board::Short {
                    self.providers.quotes(key.market, &symbols).await?.quotes
                } else {
                    Vec::new()
                };
                let mut fundamentals = HashMap::new();
                for instrument in &pool {
                    if let Some(item) = self.db.get_fundamentals(key.market, &instrument.symbol)? {
                        fundamentals.insert(instrument.symbol.clone(), item);
                    }
                }
                let specs = rankings::load_weights(&self.db, board)?;
                let payload = rankings::compute(RankingInput {
                    board,
                    market: key.market,
                    as_of,
                    pool: &pool,
                    quotes: &quotes,
                    fundamentals: &fundamentals,
                    specs,
                });
                self.db.insert_ranking(&payload)?;
                self.db.prune_rankings(self.config.snapshot_retention)?;
                self.store_snapshot(key.market, &key.scope(), as_of, &payload)?;
                Ok(as_of)
            }
            Tab::Sectors | Tab::Movers | Tab::News | Tab::Macro => {
                Err(anyhow::anyhow!("{} tab 暂未开放", key.tab.label()))
            }
        }
    }

    async fn refresh_fundamentals(&self, market: Market, symbols: &[String]) {
        for symbol in symbols {
            match self.db.get_fundamentals(market, symbol) {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, market = market.code(), symbol, "读取基本面失败");
                    continue;
                }
            }
            match self.providers.fundamentals(market, symbol).await {
                Ok(fundamentals) => {
                    if let Err(error) = self.db.upsert_fundamentals(&fundamentals, Utc::now()) {
                        tracing::warn!(%error, symbol, "写入基本面失败");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, market = market.code(), symbol, "抓取基本面失败");
                }
            }
        }
    }

    fn store_snapshot<T: Serialize>(
        &self,
        market: Market,
        scope: &str,
        as_of: DateTime<Utc>,
        payload: &T,
    ) -> Result<()> {
        let json = serde_json::to_string(payload)?;
        self.db.insert_snapshot(market, scope, as_of, &json)?;
        self.db
            .prune_snapshots(market, scope, self.config.snapshot_retention)?;
        Ok(())
    }

    pub fn latest_snapshot<T: serde::de::DeserializeOwned>(
        &self,
        market: Market,
        scope: &str,
    ) -> Result<Option<(Snapshot, T)>> {
        let Some(snapshot) = self.db.latest_snapshot(market, scope)? else {
            return Ok(None);
        };
        let payload = serde_json::from_value(snapshot.payload.clone())
            .with_context(|| format!("快照 {} 解析失败", scope))?;
        Ok(Some((snapshot, payload)))
    }

    pub fn next_trigger_for(&self, market: Market, now: DateTime<Utc>) -> Option<NextTrigger> {
        let times = match market {
            Market::Cn => &self.config.refresh_times_cn,
            Market::Us => &self.config.refresh_times_us,
        };
        next_trigger(market, times, now, self.config.refresh_open_offset_secs)
    }

    pub async fn run_scheduled_market(&self, market: Market) -> Vec<RefreshOutcome> {
        self.run_scheduled_market_at(market, Utc::now()).await
    }

    pub async fn run_scheduled_market_at(
        &self,
        market: Market,
        now: DateTime<Utc>,
    ) -> Vec<RefreshOutcome> {
        let local_date = now.with_timezone(&market.tz()).date_naive();
        if !is_trading_day(market, local_date) {
            let mut outcomes = Vec::new();
            for key in scheduled_scopes(market) {
                if let Err(error) = self.db.record_skipped(market, &key.scope(), "休市") {
                    tracing::warn!(%error, %key, "记录休市跳过失败");
                }
                outcomes.push(RefreshOutcome::Failed {
                    scope: key.to_string(),
                    error: "休市，已跳过".to_string(),
                });
            }
            return outcomes;
        }
        let keys = scheduled_scopes(market);
        self.request_many(&keys, Trigger::Scheduled).await
    }
}

pub fn scheduled_scopes(market: Market) -> Vec<ScopeKey> {
    let mut keys = vec![
        ScopeKey::new(market, Tab::Watch, None),
        ScopeKey::new(market, Tab::Index, None),
        ScopeKey::new(market, Tab::Sentiment, None),
    ];
    for board in Board::ALL {
        keys.push(ScopeKey::new(market, Tab::Ranking, Some(board)));
    }
    keys
}

pub fn scopes_for(market: Market, tab: Tab, board: Option<Board>) -> Vec<ScopeKey> {
    if !tab.supported() {
        return Vec::new();
    }
    match tab {
        Tab::Ranking => vec![ScopeKey::new(
            market,
            tab,
            Some(board.unwrap_or(Board::Short)),
        )],
        _ => vec![ScopeKey::new(market, tab, None)],
    }
}

pub struct Scheduler;

impl Scheduler {
    pub fn spawn(service: RefreshService, market: Market) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let now = Utc::now();
                let Some(next) = service.next_trigger_for(market, now) else {
                    tracing::warn!(market = market.code(), "无法计算下一次刷新时间");
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    continue;
                };
                let wait = (next.at_utc - now)
                    .to_std()
                    .unwrap_or_else(|_| Duration::from_secs(1));
                tracing::info!(
                    market = market.code(),
                    at = %next.at_utc,
                    local = %next.local_time,
                    "等待下一次定时刷新"
                );
                tokio::time::sleep(wait + Duration::from_secs(1)).await;
                let outcomes = service.run_scheduled_market(market).await;
                tracing::info!(
                    market = market.code(),
                    count = outcomes.len(),
                    "定时刷新完成"
                );
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn service() -> RefreshService {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(dir.path().join("test.db")).expect("db");
        std::mem::forget(dir);
        let mut vars = HashMap::new();
        vars.insert("AUTH_USER".to_string(), "trader".to_string());
        vars.insert("AUTH_PASSWORD_HASH".to_string(), "hash".to_string());
        vars.insert("REFRESH_COOLDOWN_SECS".to_string(), "60".to_string());
        let config = Config::from_map(&vars).expect("config");
        let providers = Providers::new(&config.provider).expect("providers");
        RefreshService::new(db, Arc::new(config), providers)
    }

    #[test]
    fn scheduled_scopes_cover_p0_tabs_and_boards() {
        let keys = scheduled_scopes(Market::Cn);
        assert_eq!(keys.len(), 6);
        assert!(keys.iter().any(|key| key.scope() == "ranking:short"));
        assert!(keys.iter().any(|key| key.scope() == "ranking:dividend"));
    }

    #[test]
    fn unsupported_tabs_have_no_scope() {
        assert!(scopes_for(Market::Cn, Tab::News, None).is_empty());
        assert_eq!(
            scopes_for(Market::Cn, Tab::Ranking, Some(Board::Long)).len(),
            1
        );
    }

    #[tokio::test]
    async fn empty_watchlist_refreshes_locally_and_respects_cooldown() {
        let service = service();
        let key = ScopeKey::new(Market::Cn, Tab::Watch, None);
        let first = service.request(key, Trigger::Manual).await;
        assert!(matches!(first, RefreshOutcome::Success { .. }));
        let second = service.request(key, Trigger::Manual).await;
        assert!(matches!(second, RefreshOutcome::Cooldown { .. }));
        let snapshot = service.latest_snapshot::<crate::models::WatchPayload>(Market::Cn, "watch");
        assert!(snapshot.unwrap().is_some());
    }
}
