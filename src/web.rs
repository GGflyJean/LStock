use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tower_http::services::ServeDir;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::auth::{
    self, clear_cookie, client_key, is_secure_request, session_cookie, session_token, Auth,
};
use crate::config::Config;
use crate::db::Db;
use crate::market::session_state;
use crate::models::{
    normalize_symbol, Board, IndexPayload, InsufficientItem, MacroItem, MacroPayload, Market,
    MoverItem, MoversPayload, NewsItem, NewsPayload, RankingItem, RankingPayload, RefreshRun,
    ScopeKey, SectorItem, SectorPayload, SentimentMetric, SentimentReport, Tab, WatchItem,
    WatchPayload,
};
use crate::rankings;
use crate::service::{scopes_for, RefreshService};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub auth: Arc<Auth>,
    pub service: RefreshService,
    pub static_dir: PathBuf,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: String,
    next: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    view: String,
    tab: String,
    board: String,
    csrf: String,
    tab_supported: bool,
    tabs: Vec<TabLink>,
    views: Vec<TabLink>,
    boards: Vec<TabLink>,
    panes: Vec<PaneView>,
    weights: Vec<WeightField>,
    next_cn: String,
    next_us: String,
    now_utc: String,
}

#[derive(Template)]
#[template(path = "status.html")]
struct StatusTemplate {
    runs: Vec<RunView>,
    running: Vec<String>,
    health: Vec<HealthRow>,
    config: Vec<ConfigRow>,
    next_cn: String,
    next_us: String,
    now_utc: String,
}

#[derive(Debug, Clone)]
struct TabLink {
    key: String,
    label: String,
    active: bool,
    supported: bool,
}

#[derive(Debug, Clone)]
struct WeightField {
    key: String,
    label: String,
    value: String,
}

#[derive(Debug, Clone)]
struct PaneView {
    market: String,
    market_label: String,
    session: String,
    as_of_local: String,
    as_of_utc: String,
    age: String,
    stale: bool,
    failed: bool,
    error: String,
    source_note: String,
    placeholder: String,
    show_watch: bool,
    show_index: bool,
    show_sentiment: bool,
    show_ranking: bool,
    show_sectors: bool,
    show_movers: bool,
    show_news: bool,
    show_macro: bool,
    show_placeholder: bool,
    quotes: Vec<crate::models::Quote>,
    indices: Vec<crate::models::Quote>,
    watch_items: Vec<WatchItem>,
    temperature: String,
    band: String,
    band_class: String,
    metrics: Vec<SentimentMetric>,
    missing_text: String,
    ranking_items: Vec<RankingItem>,
    ranking_insufficient: Vec<InsufficientItem>,
    ranking_pool_size: usize,
    ranking_weights: String,
    board_label: String,
    sectors: Vec<SectorItem>,
    movers_gainers: Vec<MoverItem>,
    movers_losers: Vec<MoverItem>,
    news: Vec<NewsItem>,
    macro_items: Vec<MacroItem>,
}

#[derive(Debug, Clone)]
struct RunView {
    time: String,
    market: String,
    scope: String,
    trigger: String,
    status: String,
    duration: String,
    error: String,
}

#[derive(Debug, Clone)]
struct ConfigRow {
    key: String,
    value: String,
}

#[derive(Debug, Clone)]
struct HealthRow {
    source: String,
    ok: bool,
    checked: String,
    error: String,
}

#[derive(Debug, Deserialize)]
struct DashboardQuery {
    view: Option<String>,
    tab: Option<String>,
    board: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CsrfForm {
    csrf: String,
    view: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WatchForm {
    csrf: String,
    symbol: String,
    name: Option<String>,
    view: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshForm {
    csrf: String,
    market: String,
    tab: String,
    board: Option<String>,
}

pub fn router(state: AppState) -> Router {
    let static_dir = state.static_dir.clone();
    Router::new()
        .route("/", get(dashboard))
        .route("/login", get(login_get).post(login_post))
        .route("/logout", post(logout))
        .route("/status", get(status_page))
        .route("/health", get(health))
        .route("/api/status", get(api_status))
        .route("/api/refresh", post(api_refresh))
        .route("/watch/{market}", post(add_watch))
        .route("/watch/{market}/delete/{id}", post(remove_watch))
        .route("/weights/{board}", post(save_weights))
        .nest_service("/static", ServeDir::new(static_dir))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DashboardQuery>,
) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    let view = normalize_view(query.view.as_deref());
    let tab = query
        .tab
        .as_deref()
        .and_then(|value| value.parse::<Tab>().ok())
        .unwrap_or(Tab::Watch);
    let board = query
        .board
        .as_deref()
        .and_then(|value| value.parse::<Board>().ok())
        .unwrap_or(Board::Short);
    let markets: Vec<Market> = match view.as_str() {
        "cn" => vec![Market::Cn],
        "us" => vec![Market::Us],
        _ => vec![Market::Cn, Market::Us],
    };
    let panes: Vec<PaneView> = markets
        .into_iter()
        .map(|market| build_pane(&state, market, tab, board))
        .collect();
    let tabs = Tab::ALL
        .iter()
        .map(|item| TabLink {
            key: item.key().to_string(),
            label: item.label().to_string(),
            active: *item == tab,
            supported: item.supported(),
        })
        .collect();
    let views = [("cn", "A股"), ("us", "美股"), ("dual", "双屏")]
        .into_iter()
        .map(|(key, label)| TabLink {
            key: key.to_string(),
            label: label.to_string(),
            active: key == view,
            supported: true,
        })
        .collect();
    let boards = Board::ALL
        .into_iter()
        .map(|item| TabLink {
            key: item.key().to_string(),
            label: item.label().to_string(),
            active: item == board,
            supported: true,
        })
        .collect();
    let weights = match rankings::load_weights(&state.db, board) {
        Ok(specs) => specs
            .into_iter()
            .map(|spec| WeightField {
                key: spec.key.to_string(),
                label: spec.label.to_string(),
                value: format!("{:.0}", spec.weight * 100.0),
            })
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "读取权重失败");
            Vec::new()
        }
    };
    tracing::debug!(
        view,
        tab = tab.key(),
        board = board.key(),
        panes = panes.len(),
        "渲染盯盘页面"
    );
    let now = Utc::now();
    let template = DashboardTemplate {
        view,
        tab: tab.key().to_string(),
        board: board.key().to_string(),
        csrf: csrf_for(&state, &headers),
        tab_supported: tab.supported(),
        tabs,
        views,
        boards,
        panes,
        weights,
        next_cn: next_label(&state, Market::Cn, now),
        next_us: next_label(&state, Market::Us, now),
        now_utc: now.format("%H:%M:%S").to_string(),
    };
    render(template)
}

fn build_pane(state: &AppState, market: Market, tab: Tab, board: Board) -> PaneView {
    let now = Utc::now();
    let scope_key = ScopeKey::new(market, tab, Some(board));
    let scope = scope_key.scope();
    let latest_run = state.db.latest_refresh_run(market, &scope).ok().flatten();
    let failed = latest_run
        .as_ref()
        .map(|run| run.status == "failed")
        .unwrap_or(false);
    let error = latest_run
        .as_ref()
        .and_then(|run| run.error.clone())
        .unwrap_or_default();
    let watch_items = state.db.watchlist(market).unwrap_or_default();

    let mut pane = PaneView {
        market: market.code().to_string(),
        market_label: market.label().to_string(),
        session: session_state(market, now).label().to_string(),
        as_of_local: "—".to_string(),
        as_of_utc: "—".to_string(),
        age: "无快照".to_string(),
        stale: false,
        failed,
        error,
        source_note: String::new(),
        placeholder: String::new(),
        show_watch: tab == Tab::Watch,
        show_index: tab == Tab::Index,
        show_sentiment: tab == Tab::Sentiment,
        show_ranking: tab == Tab::Ranking,
        show_sectors: tab == Tab::Sectors,
        show_movers: tab == Tab::Movers,
        show_news: tab == Tab::News,
        show_macro: tab == Tab::Macro,
        show_placeholder: !tab.supported(),
        quotes: Vec::new(),
        indices: Vec::new(),
        watch_items,
        temperature: String::new(),
        band: String::new(),
        band_class: String::new(),
        metrics: Vec::new(),
        missing_text: String::new(),
        ranking_items: Vec::new(),
        ranking_insufficient: Vec::new(),
        ranking_pool_size: 0,
        ranking_weights: String::new(),
        board_label: board.label().to_string(),
        sectors: Vec::new(),
        movers_gainers: Vec::new(),
        movers_losers: Vec::new(),
        news: Vec::new(),
        macro_items: Vec::new(),
    };

    if !tab.supported() {
        pane.placeholder = format!("{} tab 暂未开放，数据准备中。", tab.label());
        return pane;
    }

    let mut as_of: Option<DateTime<Utc>> = None;
    match tab {
        Tab::Watch => {
            if let Ok(Some((snapshot, payload))) = state
                .service
                .latest_snapshot::<WatchPayload>(market, "watch")
            {
                pane.quotes = payload.quotes;
                pane.source_note = payload.source_note;
                as_of = Some(snapshot.as_of_utc);
            }
            if pane.quotes.is_empty() {
                pane.placeholder = if pane.watch_items.is_empty() {
                    "尚未添加自选，先在下方添加代码，再点击手动刷新。".to_string()
                } else {
                    "暂无自选快照，点击手动刷新抓取。".to_string()
                };
            }
        }
        Tab::Index => {
            if let Ok(Some((snapshot, payload))) = state
                .service
                .latest_snapshot::<IndexPayload>(market, "index")
            {
                pane.indices = payload.indices;
                pane.source_note = payload.source_note;
                as_of = Some(snapshot.as_of_utc);
            }
            if pane.indices.is_empty() {
                pane.placeholder = "暂无指数快照，点击手动刷新抓取。".to_string();
            }
        }
        Tab::Sentiment => {
            if let Ok(Some((snapshot, report))) = state
                .service
                .latest_snapshot::<SentimentReport>(market, "sentiment")
            {
                pane.temperature = report.temperature_text();
                pane.band = report.band.clone();
                pane.band_class = report.band_class().to_string();
                pane.metrics = report.metrics;
                pane.missing_text = report.missing.join("、");
                pane.source_note = format!("来源：{} · {}", report.source, report.delay.label());
                as_of = Some(snapshot.as_of_utc);
            }
            if as_of.is_none() {
                pane.placeholder = "暂无情绪快照，点击手动刷新抓取。".to_string();
            }
        }
        Tab::Ranking => {
            if let Ok(Some((snapshot, payload))) = state
                .service
                .latest_snapshot::<RankingPayload>(market, &scope)
            {
                pane.ranking_items = payload.items;
                pane.ranking_insufficient = payload.insufficient;
                pane.ranking_pool_size = payload.pool_size;
                pane.ranking_weights = payload.weights_summary;
                pane.board_label = payload.board.label().to_string();
                as_of = Some(snapshot.as_of_utc);
            }
            if as_of.is_none() {
                pane.placeholder = format!("暂无{}快照，点击手动刷新抓取。", board.label());
            }
        }
        Tab::Sectors => {
            if let Ok(Some((snapshot, payload))) = state
                .service
                .latest_snapshot::<SectorPayload>(market, "sectors")
            {
                pane.sectors = payload.sectors;
                pane.source_note = payload.source_note;
                as_of = Some(snapshot.as_of_utc);
            }
            if pane.sectors.is_empty() {
                pane.placeholder = "暂无板块快照，点击手动刷新抓取。".to_string();
            }
        }
        Tab::Movers => {
            if let Ok(Some((snapshot, payload))) = state
                .service
                .latest_snapshot::<MoversPayload>(market, "movers")
            {
                pane.movers_gainers = payload.gainers;
                pane.movers_losers = payload.losers;
                pane.source_note = payload.source_note;
                as_of = Some(snapshot.as_of_utc);
            }
            if pane.movers_gainers.is_empty() && pane.movers_losers.is_empty() {
                pane.placeholder = "暂无涨跌榜快照，点击手动刷新抓取。".to_string();
            }
        }
        Tab::News => {
            if let Ok(Some((snapshot, payload))) =
                state.service.latest_snapshot::<NewsPayload>(market, "news")
            {
                pane.news = payload.items;
                pane.source_note = payload.source_note;
                as_of = Some(snapshot.as_of_utc);
            }
            if pane.news.is_empty() {
                pane.placeholder = "暂无新闻快照，点击手动刷新抓取。".to_string();
            }
        }
        Tab::Macro => {
            if let Ok(Some((snapshot, payload))) = state
                .service
                .latest_snapshot::<MacroPayload>(market, "macro")
            {
                pane.macro_items = payload.items;
                pane.source_note = payload.source_note;
                as_of = Some(snapshot.as_of_utc);
            }
            if pane.macro_items.is_empty() {
                pane.placeholder = "暂无宏观快照，点击手动刷新抓取。".to_string();
            }
        }
    }

    if let Some(as_of) = as_of {
        let local = as_of.with_timezone(&market.tz());
        pane.as_of_local = format!("{} {}", local.format("%m-%d %H:%M"), market.tz_label());
        pane.as_of_utc = format!("{} UTC", as_of.format("%m-%d %H:%M:%S"));
        let age_secs = (now - as_of).num_seconds().max(0);
        pane.stale = age_secs > state.config.stale_after_secs;
        pane.age = human_age(age_secs);
    }
    pane
}

fn human_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds} 秒前")
    } else if seconds < 3600 {
        format!("{} 分钟前", seconds / 60)
    } else if seconds < 86400 {
        format!("{} 小时前", seconds / 3600)
    } else {
        format!("{} 天前", seconds / 86400)
    }
}

fn next_label(state: &AppState, market: Market, now: DateTime<Utc>) -> String {
    match state.service.next_trigger_for(market, now) {
        Some(next) => {
            let local = next.at_utc.with_timezone(&market.tz());
            format!(
                "{} {} ({})",
                local.format("%m-%d %H:%M"),
                market.tz_label(),
                next.at_utc.format("%H:%M UTC")
            )
        }
        None => "—".to_string(),
    }
}

fn normalize_view(value: Option<&str>) -> String {
    match value {
        Some("cn") => "cn".to_string(),
        Some("us") => "us".to_string(),
        _ => "dual".to_string(),
    }
}

async fn login_get(State(_state): State<AppState>, Query(query): Query<LoginQuery>) -> Response {
    render(LoginTemplate {
        error: String::new(),
        next: sanitize_next(query.next.as_deref()),
    })
}

async fn login_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let client = client_key(&headers);
    match state.auth.login(&form.username, &form.password, &client) {
        Ok(login) => {
            let secure = is_secure_request(&headers);
            let mut response = Redirect::to(&sanitize_next(form.next.as_deref())).into_response();
            if let Ok(value) = HeaderValue::from_str(&session_cookie(&login.token, secure)) {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
            response
        }
        Err(error) => {
            let message = match error {
                auth::LoginError::InvalidCredentials => "用户名或密码错误".to_string(),
                auth::LoginError::Locked { remaining_secs } => {
                    format!("尝试次数过多，请 {remaining_secs} 秒后再试")
                }
            };
            render(LoginTemplate {
                error: message,
                next: sanitize_next(form.next.as_deref()),
            })
        }
    }
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    let Some(token) = session_token(&headers) else {
        return Redirect::to("/login").into_response();
    };
    if !state.auth.verify_csrf(&token, &form.csrf) {
        return forbidden();
    }
    state.auth.logout(&token);
    let secure = is_secure_request(&headers);
    let mut response = Redirect::to("/login").into_response();
    if let Ok(value) = HeaderValue::from_str(&clear_cookie(secure)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn add_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(market_raw): Path<String>,
    Form(form): Form<WatchForm>,
) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    if !verify_write(&state, &headers, &form.csrf) {
        return forbidden();
    }
    let market = match market_raw.parse::<Market>() {
        Ok(market) => market,
        Err(_) => return bad_request("未知市场"),
    };
    let symbol = match normalize_symbol(market, &form.symbol) {
        Ok(symbol) => symbol,
        Err(error) => return bad_request(&error),
    };
    let name = form
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::models::display_symbol(market, &symbol));
    if let Err(error) = state.db.add_watch(market, &symbol, &name) {
        return internal(error);
    }
    let view = normalize_view(form.view.as_deref());
    Redirect::to(&format!("/?view={view}&tab=watch")).into_response()
}

async fn remove_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((market_raw, id)): Path<(String, i64)>,
    Form(form): Form<CsrfForm>,
) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    if !verify_write(&state, &headers, &form.csrf) {
        return forbidden();
    }
    let market = match market_raw.parse::<Market>() {
        Ok(market) => market,
        Err(_) => return bad_request("未知市场"),
    };
    if let Err(error) = state.db.remove_watch_id(market, id) {
        return internal(error);
    }
    let view = normalize_view(form.view.as_deref());
    Redirect::to(&format!("/?view={view}&tab=watch")).into_response()
}

async fn save_weights(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(board_raw): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    let csrf = form.get("csrf").cloned().unwrap_or_default();
    if !verify_write(&state, &headers, &csrf) {
        return forbidden();
    }
    let board = match board_raw.parse::<Board>() {
        Ok(board) => board,
        Err(_) => return bad_request("未知榜单"),
    };
    let view = normalize_view(form.get("view").map(String::as_str));
    let mut weights = HashMap::new();
    for spec in rankings::specs(board) {
        if let Some(raw) = form.get(spec.key) {
            if let Ok(percent) = raw.trim().parse::<f64>() {
                if percent.is_finite() && percent >= 0.0 {
                    weights.insert(spec.key.to_string(), percent / 100.0);
                }
            }
        }
    }
    if weights.is_empty() {
        return bad_request("没有有效的权重");
    }
    if let Err(error) = rankings::save_weights(&state.db, board, &weights) {
        return internal(error);
    }
    Redirect::to(&format!("/?view={view}&tab=ranking&board={}", board.key())).into_response()
}

async fn api_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RefreshForm>,
) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    if !verify_write(&state, &headers, &form.csrf) {
        return forbidden();
    }
    let tab = match form.tab.parse::<Tab>() {
        Ok(tab) => tab,
        Err(_) => return bad_request("未知 tab"),
    };
    if !tab.supported() {
        return bad_request("该 tab 暂未开放刷新");
    }
    let board = form
        .board
        .as_deref()
        .and_then(|value| value.parse::<Board>().ok());
    let markets: Vec<Market> = match form.market.as_str() {
        "cn" => vec![Market::Cn],
        "us" => vec![Market::Us],
        "both" => vec![Market::Cn, Market::Us],
        _ => return bad_request("未知市场"),
    };
    let mut keys = Vec::new();
    for market in markets {
        keys.extend(scopes_for(market, tab, board));
    }
    if keys.is_empty() {
        return bad_request("没有可刷新的范围");
    }
    tracing::info!(
        market = %form.market,
        tab = tab.key(),
        board = ?board,
        scopes = ?keys,
        "开始手动刷新当前页面"
    );
    let outcomes = state
        .service
        .request_many(&keys, crate::models::Trigger::Manual)
        .await;
    for outcome in &outcomes {
        match outcome {
            crate::service::RefreshOutcome::Success { scope, as_of_utc } => {
                tracing::info!(scope = %scope, as_of = %as_of_utc, "手动刷新项成功")
            }
            crate::service::RefreshOutcome::Merged { scope } => {
                tracing::info!(scope = %scope, "手动刷新项已合并到运行中任务")
            }
            crate::service::RefreshOutcome::Cooldown {
                scope,
                remaining_secs,
            } => tracing::info!(
                scope = %scope,
                remaining_secs,
                "手动刷新项处于冷却"
            ),
            crate::service::RefreshOutcome::Failed { scope, error } => {
                tracing::error!(scope = %scope, error = %error, "手动刷新项失败")
            }
        }
    }
    Json(serde_json::json!({
        "outcomes": outcomes,
        "refreshed_at": Utc::now(),
    }))
    .into_response()
}

async fn status_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    let runs: Vec<RunView> = state
        .db
        .recent_refresh_runs(30)
        .unwrap_or_default()
        .into_iter()
        .map(run_view)
        .collect();
    let health = state
        .providers_health()
        .into_iter()
        .map(|view| HealthRow {
            source: view.source,
            ok: view.ok,
            checked: view
                .checked_at
                .map(|value| value.format("%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "—".to_string()),
            error: view.last_error.unwrap_or_default(),
        })
        .collect();
    tracing::info!(
        runs = runs.len(),
        running = state.service.running_scopes().len(),
        "查看刷新状态页"
    );
    let now = Utc::now();
    let config = vec![
        config_row("监听地址", state.config.bind_addr.to_string()),
        config_row("数据目录", state.config.data_dir.display().to_string()),
        config_row("数据库", state.config.db_path.display().to_string()),
        config_row("A股刷新", format_times(&state.config.refresh_times_cn)),
        config_row("美股刷新", format_times(&state.config.refresh_times_us)),
        config_row("冷却", format!("{} 秒", state.config.cooldown_secs)),
        config_row(
            "快照保留",
            format!("{} 份", state.config.snapshot_retention),
        ),
        config_row("并发上限", state.config.max_concurrency.to_string()),
        config_row(
            "代理",
            if state.config.proxy_url().is_some() {
                "已启用".to_string()
            } else {
                "未启用".to_string()
            },
        ),
    ];
    render(StatusTemplate {
        runs,
        running: state.service.running_scopes(),
        health,
        config,
        next_cn: next_label(&state, Market::Cn, now),
        next_us: next_label(&state, Market::Us, now),
        now_utc: now.format("%H:%M:%S").to_string(),
    })
}

fn run_view(run: RefreshRun) -> RunView {
    let duration = match run.finished_at {
        Some(finished) => format!("{} 秒", (finished - run.started_at).num_seconds().max(0)),
        None => "进行中".to_string(),
    };
    RunView {
        time: run
            .started_at
            .with_timezone(&chrono::Local)
            .format("%m-%d %H:%M:%S")
            .to_string(),
        market: run.market.label().to_string(),
        scope: run.scope,
        trigger: if run.trigger == "scheduled" {
            "定时"
        } else {
            "手动"
        }
        .to_string(),
        status: match run.status.as_str() {
            "success" => "成功",
            "failed" => "失败",
            "skipped" => "跳过",
            "running" => "运行中",
            other => other,
        }
        .to_string(),
        duration,
        error: run.error.unwrap_or_default(),
    }
}

async fn api_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(response) = require_web(&state, &headers) {
        return response;
    }
    let runs = state.db.recent_refresh_runs(20).unwrap_or_default();
    let now = Utc::now();
    Json(serde_json::json!({
        "running": state.service.running_scopes(),
        "runs": runs,
        "health": state.providers_health(),
        "next_cn": state.service.next_trigger_for(Market::Cn, now),
        "next_us": state.service.next_trigger_for(Market::Us, now),
        "now_utc": now,
    }))
    .into_response()
}

async fn health(State(state): State<AppState>) -> Response {
    match state.db.health() {
        Ok(()) => (StatusCode::OK, "ok").into_response(),
        Err(error) => {
            tracing::warn!(%error, "健康检查失败");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response()
        }
    }
}

trait ProviderHealthExt {
    fn providers_health(&self) -> Vec<crate::providers::HealthView>;
}

impl ProviderHealthExt for AppState {
    fn providers_health(&self) -> Vec<crate::providers::HealthView> {
        self.service.providers.health.snapshot()
    }
}

fn config_row(key: &str, value: String) -> ConfigRow {
    ConfigRow {
        key: key.to_string(),
        value,
    }
}

fn format_times(times: &[chrono::NaiveTime]) -> String {
    times
        .iter()
        .map(|time| time.format("%H:%M").to_string())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn require_web(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let Some(token) = session_token(headers) else {
        return Some(Redirect::to("/login").into_response());
    };
    if !state.auth.check_session(&token) {
        return Some(Redirect::to("/login").into_response());
    }
    None
}

fn csrf_for(state: &AppState, headers: &HeaderMap) -> String {
    session_token(headers)
        .and_then(|token| state.auth.session_csrf(&token))
        .unwrap_or_default()
}

fn verify_write(state: &AppState, headers: &HeaderMap, csrf: &str) -> bool {
    let Some(token) = session_token(headers) else {
        return false;
    };
    state.auth.verify_csrf(&token, csrf)
}

fn sanitize_next(value: Option<&str>) -> String {
    match value {
        Some(value) if value.starts_with('/') && !value.starts_with("//") => value.to_string(),
        _ => "/".to_string(),
    }
}

fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(error) => {
            tracing::error!(%error, "模板渲染失败");
            internal(anyhow::anyhow!("模板渲染失败: {error}"))
        }
    }
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, message.to_string()).into_response()
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "CSRF 校验失败").into_response()
}

fn internal(error: anyhow::Error) -> Response {
    tracing::error!(%error, "请求处理失败");
    (StatusCode::INTERNAL_SERVER_ERROR, "内部错误").into_response()
}
