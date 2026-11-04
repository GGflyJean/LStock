use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use chrono::NaiveTime;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub tencent_base: String,
    pub sina_base: String,
    pub eastmoney_base: String,
    pub eastmoney_datacenter_base: String,
    pub eastmoney_push2ex_base: String,
    pub yahoo_base: String,
    pub stooq_base: String,
    pub timeout_secs: u64,
    pub retries: u32,
    pub user_agent: String,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub bind_addr: SocketAddr,
    pub auth_user: String,
    pub auth_password_hash: String,
    pub refresh_times_cn: Vec<NaiveTime>,
    pub refresh_times_us: Vec<NaiveTime>,
    pub refresh_open_offset_secs: i64,
    pub cooldown_secs: u64,
    pub snapshot_retention: i64,
    pub stale_after_secs: i64,
    pub max_concurrency: usize,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub provider: ProviderConfig,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("缺少必需的环境变量 {0}")]
    Missing(&'static str),
    #[error("环境变量 {0} 的值无效: {1}")]
    Invalid(&'static str, String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let vars: HashMap<String, String> = std::env::vars().collect();
        Self::from_map(&vars)
    }

    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let get = |key: &str| vars.get(key).cloned().filter(|value| !value.is_empty());

        let data_dir = PathBuf::from(get("DATA_DIR").unwrap_or_else(|| "./data".to_string()));
        let db_path = get("DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("l_stock.db"));
        let bind_raw = get("BIND_ADDR").unwrap_or_else(|| "127.0.0.1:8080".to_string());
        let bind_addr = bind_raw
            .parse()
            .map_err(|error| ConfigError::Invalid("BIND_ADDR", format!("{error}")))?;

        let auth_user = get("AUTH_USER").ok_or(ConfigError::Missing("AUTH_USER"))?;
        let auth_password_hash =
            get("AUTH_PASSWORD_HASH").ok_or(ConfigError::Missing("AUTH_PASSWORD_HASH"))?;

        let refresh_times_cn = parse_times(
            "REFRESH_TIMES_CN",
            get("REFRESH_TIMES_CN").unwrap_or_else(|| "09:30,13:00,14:45".to_string()),
        )?;
        let refresh_times_us = parse_times(
            "REFRESH_TIMES_US",
            get("REFRESH_TIMES_US").unwrap_or_else(|| "09:30,12:00,15:45".to_string()),
        )?;

        let refresh_open_offset_secs = parse_number(
            "REFRESH_OPEN_OFFSET_SECS",
            get("REFRESH_OPEN_OFFSET_SECS"),
            60,
        )?;
        let cooldown_secs =
            parse_number("REFRESH_COOLDOWN_SECS", get("REFRESH_COOLDOWN_SECS"), 60)?;
        let snapshot_retention = parse_number("SNAPSHOT_RETENTION", get("SNAPSHOT_RETENTION"), 30)?;
        let stale_after_secs = parse_number("STALE_AFTER_SECS", get("STALE_AFTER_SECS"), 6 * 3600)?;
        let max_concurrency = parse_number("MAX_CONCURRENCY", get("MAX_CONCURRENCY"), 4)?;
        let timeout_secs = parse_number("PROVIDER_TIMEOUT_SECS", get("PROVIDER_TIMEOUT_SECS"), 10)?;
        let retries = parse_number("PROVIDER_RETRIES", get("PROVIDER_RETRIES"), 2)?;

        let http_proxy = get("HTTP_PROXY").or_else(|| get("http_proxy"));
        let https_proxy = get("HTTPS_PROXY").or_else(|| get("https_proxy"));

        let provider = ProviderConfig {
            tencent_base: get("TENCENT_BASE_URL")
                .unwrap_or_else(|| "https://qt.gtimg.cn".to_string()),
            sina_base: get("SINA_BASE_URL").unwrap_or_else(|| "https://hq.sinajs.cn".to_string()),
            eastmoney_base: get("EASTMONEY_BASE_URL")
                .unwrap_or_else(|| "https://push2.eastmoney.com".to_string()),
            eastmoney_datacenter_base: get("EASTMONEY_DATACENTER_BASE_URL")
                .unwrap_or_else(|| "https://datacenter-web.eastmoney.com".to_string()),
            eastmoney_push2ex_base: get("EASTMONEY_PUSH2EX_BASE_URL")
                .unwrap_or_else(|| "https://push2ex.eastmoney.com".to_string()),
            yahoo_base: get("YAHOO_BASE_URL")
                .unwrap_or_else(|| "https://query1.finance.yahoo.com".to_string()),
            stooq_base: get("STOOQ_BASE_URL").unwrap_or_else(|| "https://stooq.com".to_string()),
            timeout_secs,
            retries,
            user_agent: get("PROVIDER_USER_AGENT")
                .unwrap_or_else(|| "l_stock/0.1 (+self-hosted)".to_string()),
            proxy: https_proxy.clone().or_else(|| http_proxy.clone()),
        };

        Ok(Self {
            data_dir,
            db_path,
            bind_addr,
            auth_user,
            auth_password_hash,
            refresh_times_cn,
            refresh_times_us,
            refresh_open_offset_secs,
            cooldown_secs,
            snapshot_retention,
            stale_after_secs,
            max_concurrency,
            http_proxy,
            https_proxy,
            provider,
        })
    }

    pub fn snapshot_dir(&self) -> PathBuf {
        self.data_dir.join("snapshots")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn proxy_url(&self) -> Option<&str> {
        self.https_proxy.as_deref().or(self.http_proxy.as_deref())
    }

    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                anyhow::anyhow!("数据目录不可写或无法创建 {}: {error}", parent.display())
            })?;
        }
        for dir in [&self.data_dir, &self.snapshot_dir(), &self.logs_dir()] {
            std::fs::create_dir_all(dir).map_err(|error| {
                anyhow::anyhow!("数据目录不可写或无法创建 {}: {error}", dir.display())
            })?;
        }
        verify_writable(&self.data_dir)?;
        if let Some(parent) = self.db_path.parent() {
            verify_writable(parent)?;
        }
        Ok(())
    }
}

fn parse_times(key: &'static str, raw: String) -> Result<Vec<NaiveTime>, ConfigError> {
    let mut times = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let time = NaiveTime::parse_from_str(part, "%H:%M")
            .map_err(|error| ConfigError::Invalid(key, format!("{part}: {error}")))?;
        times.push(time);
    }
    times.sort();
    times.dedup();
    if times.is_empty() {
        return Err(ConfigError::Invalid(
            key,
            "至少需要一个刷新时间".to_string(),
        ));
    }
    Ok(times)
}

fn parse_number<T>(key: &'static str, raw: Option<String>, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
{
    match raw {
        Some(value) => value.parse().map_err(|_| ConfigError::Invalid(key, value)),
        None => Ok(default),
    }
}

fn verify_writable(dir: &Path) -> anyhow::Result<()> {
    let probe = dir.join(".l-stock-write-test");
    std::fs::write(&probe, b"ok")
        .map_err(|error| anyhow::anyhow!("数据目录不可写 {}: {error}", dir.display()))?;
    std::fs::remove_file(&probe)
        .map_err(|error| anyhow::anyhow!("无法清理写测试文件 {}: {error}", probe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_map() -> HashMap<String, String> {
        HashMap::from([
            ("AUTH_USER".to_string(), "trader".to_string()),
            ("AUTH_PASSWORD_HASH".to_string(), "hash-value".to_string()),
        ])
    }

    #[test]
    fn defaults_are_applied() {
        let config = Config::from_map(&base_map()).expect("config");
        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.db_path, PathBuf::from("./data/l_stock.db"));
        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:8080");
        assert_eq!(config.refresh_times_cn.len(), 3);
        assert_eq!(
            config.refresh_times_cn[0],
            NaiveTime::from_hms_opt(9, 30, 0).unwrap()
        );
        assert_eq!(
            config.refresh_times_cn[2],
            NaiveTime::from_hms_opt(14, 45, 0).unwrap()
        );
        assert_eq!(
            config.refresh_times_us[1],
            NaiveTime::from_hms_opt(12, 0, 0).unwrap()
        );
        assert_eq!(config.refresh_open_offset_secs, 60);
        assert_eq!(config.cooldown_secs, 60);
        assert_eq!(config.snapshot_retention, 30);
        assert_eq!(config.max_concurrency, 4);
        assert!(config.http_proxy.is_none());
        assert!(config.https_proxy.is_none());
    }

    #[test]
    fn overrides_are_applied() {
        let mut vars = base_map();
        vars.insert("DATA_DIR".to_string(), "/tmp/lstock".to_string());
        vars.insert("DB_PATH".to_string(), "/tmp/lstock/custom.db".to_string());
        vars.insert("BIND_ADDR".to_string(), "0.0.0.0:9000".to_string());
        vars.insert("REFRESH_TIMES_CN".to_string(), "10:00,14:00".to_string());
        vars.insert("REFRESH_TIMES_US".to_string(), "09:35,15:30".to_string());
        vars.insert("REFRESH_COOLDOWN_SECS".to_string(), "15".to_string());
        vars.insert("SNAPSHOT_RETENTION".to_string(), "5".to_string());
        vars.insert("HTTPS_PROXY".to_string(), "http://proxy:3128".to_string());
        let config = Config::from_map(&vars).expect("config");
        assert_eq!(config.db_path, PathBuf::from("/tmp/lstock/custom.db"));
        assert_eq!(config.bind_addr.to_string(), "0.0.0.0:9000");
        assert_eq!(config.refresh_times_cn.len(), 2);
        assert_eq!(config.cooldown_secs, 15);
        assert_eq!(config.snapshot_retention, 5);
        assert_eq!(config.https_proxy.as_deref(), Some("http://proxy:3128"));
        assert_eq!(config.proxy_url(), Some("http://proxy:3128"));
    }

    #[test]
    fn missing_auth_user_fails() {
        let mut vars = base_map();
        vars.remove("AUTH_USER");
        assert_eq!(
            Config::from_map(&vars).unwrap_err(),
            ConfigError::Missing("AUTH_USER")
        );
    }

    #[test]
    fn missing_password_hash_fails() {
        let mut vars = base_map();
        vars.remove("AUTH_PASSWORD_HASH");
        assert_eq!(
            Config::from_map(&vars).unwrap_err(),
            ConfigError::Missing("AUTH_PASSWORD_HASH")
        );
    }

    #[test]
    fn invalid_bind_address_fails() {
        let mut vars = base_map();
        vars.insert("BIND_ADDR".to_string(), "not-an-address".to_string());
        assert!(matches!(
            Config::from_map(&vars).unwrap_err(),
            ConfigError::Invalid("BIND_ADDR", _)
        ));
    }

    #[test]
    fn invalid_refresh_time_fails() {
        let mut vars = base_map();
        vars.insert("REFRESH_TIMES_CN".to_string(), "09:30,25:00".to_string());
        assert!(matches!(
            Config::from_map(&vars).unwrap_err(),
            ConfigError::Invalid("REFRESH_TIMES_CN", _)
        ));
    }

    #[test]
    fn ensure_dirs_creates_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut vars = base_map();
        vars.insert("DATA_DIR".to_string(), dir.path().display().to_string());
        let config = Config::from_map(&vars).expect("config");
        config.ensure_dirs().expect("dirs");
        assert!(config.snapshot_dir().is_dir());
        assert!(config.logs_dir().is_dir());
        assert!(config.db_path.parent().unwrap().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_data_dir_reports_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("readonly");
        std::fs::create_dir(&path).expect("mkdir");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).expect("chmod");

        let mut vars = base_map();
        vars.insert("DATA_DIR".to_string(), path.display().to_string());
        let config = Config::from_map(&vars).expect("config");
        let result = config.ensure_dirs();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("restore");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("不可写"));
    }
}
