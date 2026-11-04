use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use l_stock::auth::{self, Auth};
use l_stock::config::Config;
use l_stock::db::Db;
use l_stock::models::{Board, Market, Tab};
use l_stock::providers::Providers;
use l_stock::rankings;
use l_stock::service::{scopes_for, RefreshService, Scheduler};
use l_stock::web::{self, AppState};

#[derive(Debug, Parser)]
#[command(name = "l_stock", version, about = "A股/美股盯盘快照服务")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 启动 Web 服务
    Serve,
    /// 生成 Argon2 密码哈希
    HashPassword { password: String },
    /// 手动刷新指定市场与 tab
    Refresh {
        #[arg(long, default_value = "both")]
        market: String,
        #[arg(long, default_value = "index")]
        tab: String,
        #[arg(long, default_value = "short")]
        board: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve().await,
        Command::HashPassword { password } => {
            println!("{}", auth::hash_password(&password)?);
            Ok(())
        }
        Command::Refresh { market, tab, board } => refresh_once(&market, &tab, &board).await,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn build_runtime() -> Result<(Config, Db, Arc<Auth>, RefreshService)> {
    let config = Config::from_env()?;
    config.ensure_dirs()?;
    let db = Db::open(&config.db_path)?;
    rankings::seed_default_pool(&db)?;
    let auth = Arc::new(Auth::new(
        config.auth_user.clone(),
        config.auth_password_hash.clone(),
    ));
    let providers = Providers::new(&config.provider).context("初始化行情源失败")?;
    let service = RefreshService::new(db.clone(), Arc::new(config.clone()), providers);
    Ok((config, db, auth, service))
}

async fn serve() -> Result<()> {
    init_tracing();
    let (config, db, auth, service) = build_runtime()?;

    tracing::info!(
        address = %config.bind_addr,
        data_dir = %config.data_dir.display(),
        db = %config.db_path.display(),
        refresh_cn = %format_times(&config.refresh_times_cn),
        refresh_us = %format_times(&config.refresh_times_us),
        cooldown_secs = config.cooldown_secs,
        snapshot_retention = config.snapshot_retention,
        max_concurrency = config.max_concurrency,
        proxy = config.proxy_url().is_some(),
        timeout_secs = config.provider.timeout_secs,
        retries = config.provider.retries,
        "LStock 配置"
    );
    if config.proxy_url().is_some() {
        tracing::info!("已启用出站代理");
    }

    let scheduler_cn = Scheduler::spawn(service.clone(), Market::Cn);
    let scheduler_us = Scheduler::spawn(service.clone(), Market::Us);

    let state = AppState {
        config: Arc::new(config.clone()),
        db,
        auth,
        service,
        static_dir: PathBuf::from("static"),
    };
    let app = web::router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("无法监听 {}", config.bind_addr))?;
    tracing::info!(address = %config.bind_addr, "LStock 已启动");
    let result = axum::serve(listener, app).await;
    scheduler_cn.abort();
    scheduler_us.abort();
    result?;
    Ok(())
}

async fn refresh_once(market_raw: &str, tab_raw: &str, board_raw: &str) -> Result<()> {
    init_tracing();
    let (_config, _db, _auth, service) = build_runtime()?;
    let tab: Tab = tab_raw.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
    let board: Board = board_raw.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
    let markets: Vec<Market> = match market_raw {
        "cn" => vec![Market::Cn],
        "us" => vec![Market::Us],
        "both" => vec![Market::Cn, Market::Us],
        other => return Err(anyhow::anyhow!("未知市场 {other}")),
    };
    let mut keys = Vec::new();
    for market in markets {
        keys.extend(scopes_for(market, tab, Some(board)));
    }
    let outcomes = service
        .request_many(&keys, l_stock::models::Trigger::Manual)
        .await;
    println!("{}", serde_json::to_string_pretty(&outcomes)?);
    Ok(())
}

fn format_times(times: &[chrono::NaiveTime]) -> String {
    times
        .iter()
        .map(|time| time.format("%H:%M").to_string())
        .collect::<Vec<_>>()
        .join(",")
}
