use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{Board, Fundamentals, Market, RankingPayload, RefreshRun, Snapshot, WatchItem};

const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE IF NOT EXISTS snapshots (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        market TEXT NOT NULL,
        scope TEXT NOT NULL,
        as_of_utc TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_snapshots_lookup
        ON snapshots(market, scope, as_of_utc DESC);

    CREATE TABLE IF NOT EXISTS refresh_runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        market TEXT NOT NULL,
        scope TEXT NOT NULL,
        trigger_type TEXT NOT NULL,
        status TEXT NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT,
        as_of_utc TEXT,
        error TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_refresh_runs_lookup
        ON refresh_runs(market, scope, started_at DESC);

    CREATE TABLE IF NOT EXISTS ranking_snapshots (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        board TEXT NOT NULL,
        market TEXT NOT NULL,
        as_of_utc TEXT NOT NULL,
        rank INTEGER NOT NULL,
        symbol TEXT NOT NULL,
        name TEXT NOT NULL,
        score REAL NOT NULL,
        factors_json TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_ranking_lookup
        ON ranking_snapshots(board, market, as_of_utc DESC, rank);

    CREATE TABLE IF NOT EXISTS instruments (
        market TEXT NOT NULL,
        symbol TEXT NOT NULL,
        name TEXT NOT NULL,
        in_pool INTEGER NOT NULL DEFAULT 1,
        updated_at TEXT NOT NULL,
        PRIMARY KEY(market, symbol)
    );

    CREATE TABLE IF NOT EXISTS fundamentals (
        market TEXT NOT NULL,
        symbol TEXT NOT NULL,
        as_of_utc TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        PRIMARY KEY(market, symbol)
    );

    CREATE TABLE IF NOT EXISTS watchlist (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        market TEXT NOT NULL,
        symbol TEXT NOT NULL,
        name TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(market, symbol)
    );

    CREATE TABLE IF NOT EXISTS prefs (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    "#];

#[derive(Debug, Clone)]
pub struct Db {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Instrument {
    pub market: Market,
    pub symbol: String,
    pub name: String,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Self { path };
        db.migrate()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn health(&self) -> Result<()> {
        let conn = self.connection()?;
        conn.query_row("SELECT 1", [], |_| Ok(()))?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)
            .with_context(|| format!("无法打开数据库 {}", self.path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        Ok(conn)
    }

    pub fn migrate(&self) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;
        for (index, sql) in MIGRATIONS.iter().enumerate() {
            let version = index as i64 + 1;
            let applied: Option<i64> = conn
                .query_row(
                    "SELECT version FROM schema_migrations WHERE version = ?1",
                    [version],
                    |row| row.get(0),
                )
                .optional()?;
            if applied.is_some() {
                continue;
            }
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                params![version, now_string()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn insert_snapshot(
        &self,
        market: Market,
        scope: &str,
        as_of: DateTime<Utc>,
        payload_json: &str,
    ) -> Result<i64> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO snapshots(market, scope, as_of_utc, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                market.code(),
                scope,
                as_of.to_rfc3339(),
                payload_json,
                now_string()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn latest_snapshot(&self, market: Market, scope: &str) -> Result<Option<Snapshot>> {
        let conn = self.connection()?;
        let row = conn
            .query_row(
                "SELECT market, scope, as_of_utc, payload_json, created_at
                 FROM snapshots WHERE market = ?1 AND scope = ?2
                 ORDER BY as_of_utc DESC, id DESC LIMIT 1",
                params![market.code(), scope],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(market_raw, scope, as_of, payload, created)| {
            Ok(Snapshot {
                market: market_raw.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
                scope,
                as_of_utc: parse_time(&as_of)?,
                payload: serde_json::from_str(&payload)?,
                created_at: parse_time(&created)?,
            })
        })
        .transpose()
    }

    pub fn snapshot_count(&self, market: Market, scope: &str) -> Result<i64> {
        let conn = self.connection()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM snapshots WHERE market = ?1 AND scope = ?2",
            params![market.code(), scope],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn prune_snapshots(&self, market: Market, scope: &str, keep: i64) -> Result<usize> {
        let keep = keep.max(0);
        let conn = self.connection()?;
        let removed = conn.execute(
            "DELETE FROM snapshots WHERE id IN (
                SELECT id FROM snapshots WHERE market = ?1 AND scope = ?2
                ORDER BY as_of_utc DESC, id DESC LIMIT -1 OFFSET ?3
             )",
            params![market.code(), scope, keep],
        )?;
        Ok(removed)
    }

    pub fn insert_refresh_run(
        &self,
        market: Market,
        scope: &str,
        trigger: &str,
        status: &str,
    ) -> Result<i64> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO refresh_runs(market, scope, trigger_type, status, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![market.code(), scope, trigger, status, now_string()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_refresh_run(
        &self,
        id: i64,
        status: &str,
        as_of: Option<DateTime<Utc>>,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.connection()?;
        conn.execute(
            "UPDATE refresh_runs SET status = ?1, finished_at = ?2, as_of_utc = ?3, error = ?4
             WHERE id = ?5",
            params![
                status,
                now_string(),
                as_of.map(|value| value.to_rfc3339()),
                error,
                id
            ],
        )?;
        Ok(())
    }

    pub fn record_skipped(&self, market: Market, scope: &str, reason: &str) -> Result<i64> {
        let conn = self.connection()?;
        let now = now_string();
        conn.execute(
            "INSERT INTO refresh_runs(market, scope, trigger_type, status, started_at, finished_at, error)
             VALUES (?1, ?2, 'scheduled', 'skipped', ?3, ?3, ?4)",
            params![market.code(), scope, now, reason],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn latest_refresh_run(&self, market: Market, scope: &str) -> Result<Option<RefreshRun>> {
        let conn = self.connection()?;
        let row = conn
            .query_row(
                "SELECT id, market, scope, trigger_type, status, started_at, finished_at, as_of_utc, error
                 FROM refresh_runs WHERE market = ?1 AND scope = ?2
                 ORDER BY id DESC LIMIT 1",
                params![market.code(), scope],
                map_refresh_run,
            )
            .optional()?;
        row.transpose()
    }

    pub fn recent_refresh_runs(&self, limit: i64) -> Result<Vec<RefreshRun>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, market, scope, trigger_type, status, started_at, finished_at, as_of_utc, error
             FROM refresh_runs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], map_refresh_run)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row??);
        }
        Ok(runs)
    }

    pub fn watchlist(&self, market: Market) -> Result<Vec<WatchItem>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, market, symbol, name FROM watchlist WHERE market = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([market.code()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (id, market_raw, symbol, name) = row?;
            items.push(WatchItem {
                id,
                market: market_raw.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
                symbol,
                name,
            });
        }
        Ok(items)
    }

    pub fn add_watch(&self, market: Market, symbol: &str, name: &str) -> Result<bool> {
        let conn = self.connection()?;
        let changed = conn.execute(
            "INSERT OR IGNORE INTO watchlist(market, symbol, name, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![market.code(), symbol, name, now_string()],
        )?;
        Ok(changed > 0)
    }

    pub fn remove_watch_id(&self, market: Market, id: i64) -> Result<bool> {
        let conn = self.connection()?;
        let changed = conn.execute(
            "DELETE FROM watchlist WHERE market = ?1 AND id = ?2",
            params![market.code(), id],
        )?;
        Ok(changed > 0)
    }

    pub fn remove_watch(&self, market: Market, symbol: &str) -> Result<bool> {
        let conn = self.connection()?;
        let changed = conn.execute(
            "DELETE FROM watchlist WHERE market = ?1 AND symbol = ?2",
            params![market.code(), symbol],
        )?;
        Ok(changed > 0)
    }

    pub fn get_pref(&self, key: &str) -> Result<Option<String>> {
        let conn = self.connection()?;
        let value = conn
            .query_row("SELECT value FROM prefs WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value)
    }

    pub fn set_pref(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO prefs(key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now_string()],
        )?;
        Ok(())
    }

    pub fn upsert_instrument(&self, market: Market, symbol: &str, name: &str) -> Result<()> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO instruments(market, symbol, name, in_pool, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT(market, symbol) DO UPDATE SET name = excluded.name, in_pool = 1,
             updated_at = excluded.updated_at",
            params![market.code(), symbol, name, now_string()],
        )?;
        Ok(())
    }

    pub fn pool(&self, market: Market) -> Result<Vec<Instrument>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT market, symbol, name FROM instruments
             WHERE market = ?1 AND in_pool = 1 ORDER BY symbol",
        )?;
        let rows = stmt.query_map([market.code()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (market_raw, symbol, name) = row?;
            items.push(Instrument {
                market: market_raw.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
                symbol,
                name,
            });
        }
        Ok(items)
    }

    pub fn upsert_fundamentals(
        &self,
        fundamentals: &Fundamentals,
        as_of: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.connection()?;
        let payload = serde_json::to_string(fundamentals)?;
        conn.execute(
            "INSERT INTO fundamentals(market, symbol, as_of_utc, payload_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(market, symbol) DO UPDATE SET as_of_utc = excluded.as_of_utc,
             payload_json = excluded.payload_json",
            params![
                fundamentals.market.code(),
                fundamentals.symbol,
                as_of.to_rfc3339(),
                payload
            ],
        )?;
        Ok(())
    }

    pub fn get_fundamentals(&self, market: Market, symbol: &str) -> Result<Option<Fundamentals>> {
        let conn = self.connection()?;
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM fundamentals WHERE market = ?1 AND symbol = ?2",
                params![market.code(), symbol],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn insert_ranking(&self, payload: &RankingPayload) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        for item in &payload.items {
            tx.execute(
                "INSERT INTO ranking_snapshots(board, market, as_of_utc, rank, symbol, name, score, factors_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    payload.board.key(),
                    payload.market.code(),
                    payload.as_of_utc.to_rfc3339(),
                    item.rank,
                    item.symbol,
                    item.name,
                    item.score,
                    serde_json::to_string(&item.factors)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn latest_ranking(&self, board: Board, market: Market) -> Result<Option<RankingPayload>> {
        let conn = self.connection()?;
        let as_of: Option<String> = conn
            .query_row(
                "SELECT as_of_utc FROM ranking_snapshots WHERE board = ?1 AND market = ?2
                 ORDER BY as_of_utc DESC LIMIT 1",
                params![board.key(), market.code()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(as_of) = as_of else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT rank, symbol, name, score, factors_json FROM ranking_snapshots
             WHERE board = ?1 AND market = ?2 AND as_of_utc = ?3 ORDER BY rank",
        )?;
        let rows = stmt.query_map(params![board.key(), market.code(), as_of], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (rank, symbol, name, score, factors) = row?;
            items.push(crate::models::RankingItem {
                rank,
                symbol,
                name,
                score,
                factors: serde_json::from_str(&factors)?,
            });
        }
        Ok(Some(RankingPayload {
            board,
            market,
            as_of_utc: parse_time(&as_of)?,
            items,
            insufficient: Vec::new(),
            pool_size: 0,
            weights_summary: String::new(),
        }))
    }

    pub fn prune_rankings(&self, keep_per_board: i64) -> Result<usize> {
        let keep = keep_per_board.max(1);
        let conn = self.connection()?;
        let removed = conn.execute(
            "DELETE FROM ranking_snapshots WHERE id IN (
                SELECT id FROM ranking_snapshots r
                WHERE as_of_utc NOT IN (
                    SELECT as_of_utc FROM ranking_snapshots
                    WHERE board = r.board AND market = r.market
                    ORDER BY as_of_utc DESC LIMIT ?1
                )
            )",
            [keep],
        )?;
        Ok(removed)
    }
}

fn map_refresh_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<RefreshRun>> {
    let market_raw: String = row.get(1)?;
    let started_at: String = row.get(5)?;
    let finished_at: Option<String> = row.get(6)?;
    let as_of: Option<String> = row.get(7)?;
    Ok((|| {
        Ok(RefreshRun {
            id: row.get(0)?,
            market: market_raw.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
            scope: row.get(2)?,
            trigger: row.get(3)?,
            status: row.get(4)?,
            started_at: parse_time(&started_at)?,
            finished_at: finished_at.map(|value| parse_time(&value)).transpose()?,
            as_of_utc: as_of.map(|value| parse_time(&value)).transpose()?,
            error: row.get(8)?,
        })
    })())
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("无效时间 {value}"))?
        .with_timezone(&Utc))
}

fn now_string() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FactorScore, RankingItem};

    fn db() -> Db {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        std::mem::forget(dir);
        Db::open(path).expect("db")
    }

    #[test]
    fn migrations_are_repeatable() {
        let db = db();
        db.migrate().expect("migrate twice");
        let conn = db.connection().expect("conn");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[test]
    fn snapshot_roundtrip_and_prune() {
        let db = db();
        let base = Utc::now();
        for index in 0..3 {
            db.insert_snapshot(
                Market::Cn,
                "index",
                base + chrono::Duration::minutes(index),
                &format!("{{\"n\":{index}}}"),
            )
            .expect("insert");
        }
        let latest = db.latest_snapshot(Market::Cn, "index").unwrap().unwrap();
        assert_eq!(latest.as_of_utc, base + chrono::Duration::minutes(2));
        db.prune_snapshots(Market::Cn, "index", 2).expect("prune");
        assert_eq!(db.snapshot_count(Market::Cn, "index").unwrap(), 2);
        let latest = db.latest_snapshot(Market::Cn, "index").unwrap().unwrap();
        assert_eq!(latest.as_of_utc, base + chrono::Duration::minutes(2));
    }

    #[test]
    fn refresh_runs_record_statuses() {
        let db = db();
        let id = db
            .insert_refresh_run(Market::Cn, "watch", "manual", "running")
            .expect("insert");
        db.finish_refresh_run(id, "success", Some(Utc::now()), None)
            .expect("finish");
        db.record_skipped(Market::Us, "index", "休市")
            .expect("skipped");
        let success = db.latest_refresh_run(Market::Cn, "watch").unwrap().unwrap();
        assert_eq!(success.status, "success");
        let skipped = db.latest_refresh_run(Market::Us, "index").unwrap().unwrap();
        assert_eq!(skipped.status, "skipped");
    }

    #[test]
    fn watchlist_is_isolated_per_market() {
        let db = db();
        assert!(db.add_watch(Market::Cn, "600519", "贵州茅台").unwrap());
        assert!(!db.add_watch(Market::Cn, "600519", "贵州茅台").unwrap());
        assert!(db.add_watch(Market::Us, "AAPL", "Apple").unwrap());
        assert_eq!(db.watchlist(Market::Cn).unwrap().len(), 1);
        assert_eq!(db.watchlist(Market::Us).unwrap().len(), 1);
        assert!(db.remove_watch(Market::Cn, "600519").unwrap());
        assert_eq!(db.watchlist(Market::Cn).unwrap().len(), 0);
        assert_eq!(db.watchlist(Market::Us).unwrap().len(), 1);
    }

    #[test]
    fn ranking_roundtrip() {
        let db = db();
        let payload = RankingPayload {
            board: Board::Short,
            market: Market::Cn,
            as_of_utc: Utc::now(),
            items: vec![RankingItem {
                rank: 1,
                symbol: "600519".to_string(),
                name: "贵州茅台".to_string(),
                score: 88.0,
                factors: vec![FactorScore {
                    key: "momentum".to_string(),
                    label: "动量".to_string(),
                    raw: Some(1.0),
                    z: 1.2,
                    weight: 0.3,
                    contribution: 0.36,
                }],
            }],
            insufficient: Vec::new(),
            pool_size: 1,
            weights_summary: "测试".to_string(),
        };
        db.insert_ranking(&payload).expect("insert");
        let latest = db
            .latest_ranking(Board::Short, Market::Cn)
            .unwrap()
            .unwrap();
        assert_eq!(latest.items.len(), 1);
        assert_eq!(latest.items[0].symbol, "600519");
    }

    #[test]
    fn ranking_history_is_queryable_by_as_of() {
        let db = db();
        let base = Utc::now() - chrono::Duration::hours(1);
        let mut older = RankingPayload {
            board: Board::Short,
            market: Market::Cn,
            as_of_utc: base,
            items: vec![RankingItem {
                rank: 1,
                symbol: "sh600519".to_string(),
                name: "贵州茅台".to_string(),
                score: 70.0,
                factors: Vec::new(),
            }],
            insufficient: Vec::new(),
            pool_size: 1,
            weights_summary: "旧".to_string(),
        };
        db.insert_ranking(&older).expect("insert old");
        older.as_of_utc = base + chrono::Duration::minutes(30);
        older.items[0].score = 90.0;
        db.insert_ranking(&older).expect("insert new");
        let latest = db
            .latest_ranking(Board::Short, Market::Cn)
            .unwrap()
            .unwrap();
        assert_eq!(latest.as_of_utc, older.as_of_utc);
        assert_eq!(latest.items[0].score, 90.0);
    }

    #[test]
    fn prefs_roundtrip() {
        let db = db();
        assert!(db.get_pref("weights").unwrap().is_none());
        db.set_pref("weights", "{}").unwrap();
        assert_eq!(db.get_pref("weights").unwrap().as_deref(), Some("{}"));
    }
}
