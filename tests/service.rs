mod support;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use l_stock::auth::hash_password;
use l_stock::config::Config;
use l_stock::db::Db;
use l_stock::models::{
    IndexPayload, Market, ScopeKey, SentimentReport, Tab, Trigger, WatchPayload,
};
use l_stock::providers::Providers;
use l_stock::service::{scopes_for, RefreshOutcome, RefreshService};
use l_stock::web::{self, AppState};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn tencent_line(symbol: &str, name: &str, price: &str) -> String {
    let mut fields = vec!["1", name, symbol, price, price, price, "1000"];
    while fields.len() < 50 {
        fields.push("0");
    }
    fields[31] = "1.0";
    fields[32] = "0.5";
    fields[38] = "1.5";
    fields[49] = "1.2";
    format!("v_{symbol}=\"{}\";", fields.join("~"))
}

fn build(server: &MockServer, cooldown: u64) -> (RefreshService, tempfile::TempDir) {
    build_with(server, cooldown, 4)
}

fn build_with(
    server: &MockServer,
    cooldown: u64,
    max_concurrency: u64,
) -> (RefreshService, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut vars = HashMap::new();
    vars.insert("AUTH_USER".to_string(), "trader".to_string());
    vars.insert(
        "AUTH_PASSWORD_HASH".to_string(),
        hash_password("secret").unwrap(),
    );
    vars.insert("DATA_DIR".to_string(), dir.path().display().to_string());
    vars.insert("REFRESH_COOLDOWN_SECS".to_string(), cooldown.to_string());
    vars.insert("MAX_CONCURRENCY".to_string(), max_concurrency.to_string());
    vars.insert("TENCENT_BASE_URL".to_string(), server.uri());
    vars.insert("SINA_BASE_URL".to_string(), server.uri());
    vars.insert("EASTMONEY_BASE_URL".to_string(), server.uri());
    vars.insert("YAHOO_BASE_URL".to_string(), server.uri());
    vars.insert("STOOQ_BASE_URL".to_string(), server.uri());
    vars.insert("PROVIDER_RETRIES".to_string(), "0".to_string());
    let config = Config::from_map(&vars).expect("config");
    config.ensure_dirs().expect("dirs");
    let db = Db::open(&config.db_path).expect("db");
    l_stock::rankings::seed_default_pool(&db).expect("pool");
    let providers = Providers::new(&config.provider).expect("providers");
    let service = RefreshService::new(db, Arc::new(config), providers);
    (service, dir)
}

async fn mount_market_data(server: &MockServer) {
    let watch = tencent_line("sh600519", "贵州茅台", "1500.0");
    let watch_bytes = encoding_rs::GBK.encode(&watch).0.to_vec();
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", "sh600519"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(watch_bytes))
        .mount(server)
        .await;

    let index = [
        tencent_line("sh000001", "上证指数", "3891.0"),
        tencent_line("sz399001", "深证成指", "12000.0"),
        tencent_line("sz399006", "创业板指", "2500.0"),
    ]
    .join("\n");
    let index_bytes = encoding_rs::GBK.encode(&index).0.to_vec();
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", "sh000001,sz399001,sz399006"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(index_bytes))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/qt/clist/get"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"data":{"diff":[{"f104":3000,"f105":1800,"f106":100,"f107":40,"f108":5,"f109":10,"f110":4,"f111":120000000000,"f112":3.0,"f113":0.2,"f114":50,"f115":10,"f116":500000000}]}}"#,
            "application/json",
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn scheduled_pipeline_fetches_writes_snapshot_and_renders() {
    let server = MockServer::start().await;
    mount_market_data(&server).await;
    let (service, _dir) = build(&server, 0);
    service
        .db
        .add_watch(Market::Cn, "sh600519", "贵州茅台")
        .unwrap();

    let watch_key = ScopeKey::new(Market::Cn, Tab::Watch, None);
    let watch = service.request(watch_key, Trigger::Scheduled).await;
    assert!(matches!(watch, RefreshOutcome::Success { .. }), "{watch:?}");
    let (_, payload): (_, WatchPayload) = service
        .latest_snapshot::<WatchPayload>(Market::Cn, "watch")
        .unwrap()
        .expect("watch snapshot");
    assert_eq!(payload.quotes.len(), 1);
    assert_eq!(payload.quotes[0].symbol, "sh600519");

    let index_key = ScopeKey::new(Market::Cn, Tab::Index, None);
    let index = service.request(index_key, Trigger::Scheduled).await;
    assert!(matches!(index, RefreshOutcome::Success { .. }), "{index:?}");
    let watch_run = service
        .db
        .latest_refresh_run(Market::Cn, "watch")
        .unwrap()
        .unwrap();
    let watch_snapshot = service
        .db
        .latest_snapshot(Market::Cn, "watch")
        .unwrap()
        .unwrap();
    assert_eq!(watch_run.as_of_utc, Some(watch_snapshot.as_of_utc));
    let (index_snapshot, index_payload): (_, IndexPayload) = service
        .latest_snapshot::<IndexPayload>(Market::Cn, "index")
        .unwrap()
        .expect("index snapshot");
    assert_eq!(index_payload.indices.len(), 3);
    assert!(index_snapshot.as_of_utc <= Utc::now());

    let sentiment_key = ScopeKey::new(Market::Cn, Tab::Sentiment, None);
    let sentiment = service.request(sentiment_key, Trigger::Scheduled).await;
    assert!(
        matches!(sentiment, RefreshOutcome::Success { .. }),
        "{sentiment:?}"
    );
    let index_run = service
        .db
        .latest_refresh_run(Market::Cn, "index")
        .unwrap()
        .unwrap();
    assert_eq!(index_run.as_of_utc, Some(index_snapshot.as_of_utc));
    let (_, report): (_, SentimentReport) = service
        .latest_snapshot::<SentimentReport>(Market::Cn, "sentiment")
        .unwrap()
        .expect("sentiment snapshot");
    assert!(report.temperature > 0.0);
    let sentiment_run = service
        .db
        .latest_refresh_run(Market::Cn, "sentiment")
        .unwrap()
        .unwrap();
    let sentiment_snapshot = service
        .db
        .latest_snapshot(Market::Cn, "sentiment")
        .unwrap()
        .unwrap();
    assert_eq!(sentiment_run.as_of_utc, Some(sentiment_snapshot.as_of_utc));

    let runs = service.db.recent_refresh_runs(10).unwrap();
    assert!(runs.iter().all(|run| run.status == "success"));
    assert!(runs.iter().all(|run| run.as_of_utc.is_some()));

    let state = AppState {
        config: service.config.clone(),
        db: service.db.clone(),
        auth: Arc::new(l_stock::auth::Auth::new(
            "trader",
            hash_password("secret").unwrap(),
        )),
        service: service.clone(),
        static_dir: std::path::PathBuf::from("static"),
    };
    let login = state.auth.login("trader", "secret", "test").unwrap();
    let request = axum::http::Request::builder()
        .uri("/?view=cn&tab=index")
        .header(
            axum::http::header::COOKIE,
            format!("l_stock_session={}", login.token),
        )
        .body(axum::body::Body::empty())
        .unwrap();
    let response = web::router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = support::body_text(response).await;
    assert!(body.contains("上证指数"));
    assert!(body.contains("深证成指"));

    let request = axum::http::Request::builder()
        .uri("/?view=cn&tab=sentiment")
        .header(
            axum::http::header::COOKIE,
            format!("l_stock_session={}", login.token),
        )
        .body(axum::body::Body::empty())
        .unwrap();
    let response = web::router(state.clone()).oneshot(request).await.unwrap();
    let body = support::body_text(response).await;
    assert!(body.contains("情绪温度"));
    assert!(body.contains("涨跌家数"));
}

#[tokio::test]
async fn holiday_schedule_records_skipped_without_requests() {
    let server = MockServer::start().await;
    let (service, _dir) = build(&server, 0);
    let sunday = Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
    let outcomes = service.run_scheduled_market_at(Market::Cn, sunday).await;
    assert_eq!(outcomes.len(), 6);
    assert!(outcomes.iter().all(
        |outcome| matches!(outcome, RefreshOutcome::Failed { error, .. } if error.contains("休市"))
    ));
    let runs = service.db.recent_refresh_runs(10).unwrap();
    assert_eq!(runs.len(), 6);
    assert!(runs.iter().all(|run| run.status == "skipped"));
}

#[tokio::test]
async fn dual_manual_refresh_only_touches_current_scope_and_respects_cooldown() {
    let server = MockServer::start().await;
    let (service, _dir) = build(&server, 60);
    let keys = vec![
        ScopeKey::new(Market::Cn, Tab::Watch, None),
        ScopeKey::new(Market::Us, Tab::Watch, None),
    ];
    let first = service.request_many(&keys, Trigger::Manual).await;
    assert_eq!(first.len(), 2);
    assert!(first
        .iter()
        .all(|outcome| matches!(outcome, RefreshOutcome::Success { .. })));
    assert!(service
        .db
        .latest_snapshot(Market::Cn, "index")
        .unwrap()
        .is_none());
    assert!(service
        .db
        .latest_snapshot(Market::Us, "index")
        .unwrap()
        .is_none());

    let second = service.request_many(&keys, Trigger::Manual).await;
    assert!(second
        .iter()
        .all(|outcome| matches!(outcome, RefreshOutcome::Cooldown { .. })));
}

#[tokio::test]
async fn concurrent_requests_single_flight() {
    let server = MockServer::start().await;
    let index = [
        tencent_line("sh000001", "上证指数", "3891.0"),
        tencent_line("sz399001", "深证成指", "12000.0"),
        tencent_line("sz399006", "创业板指", "2500.0"),
    ]
    .join("\n");
    let bytes = encoding_rs::GBK.encode(&index).0.to_vec();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(bytes)
                .set_delay(std::time::Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    let (service, _dir) = build(&server, 0);
    let key = ScopeKey::new(Market::Cn, Tab::Index, None);
    let (a, b) = tokio::join!(
        service.request(key, Trigger::Manual),
        service.request(key, Trigger::Manual)
    );
    let outcomes = [a, b];
    assert!(outcomes
        .iter()
        .any(|outcome| matches!(outcome, RefreshOutcome::Success { .. })));
    assert!(outcomes
        .iter()
        .any(|outcome| matches!(outcome, RefreshOutcome::Merged { .. })));
    assert_eq!(service.db.snapshot_count(Market::Cn, "index").unwrap(), 1);
}

#[tokio::test]
async fn ranking_pipeline_writes_ranking_snapshot() {
    let server = MockServer::start().await;
    let pool = l_stock::rankings::DEFAULT_CN_POOL;
    let mut symbols: Vec<&str> = pool.iter().map(|(symbol, _)| *symbol).collect();
    symbols.sort();
    let query = symbols.join(",");
    let mut lines = Vec::new();
    for (index, (symbol, name)) in pool.iter().enumerate() {
        lines.push(tencent_line(symbol, name, &format!("{}.0", 10 + index)));
    }
    let body = encoding_rs::GBK.encode(&lines.join("\n")).0.to_vec();
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", query.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;
    let (service, _dir) = build(&server, 0);
    let key = ScopeKey::new(
        Market::Cn,
        Tab::Ranking,
        Some(l_stock::models::Board::Short),
    );
    let outcome = service.request(key, Trigger::Manual).await;
    assert!(
        matches!(outcome, RefreshOutcome::Success { .. }),
        "{outcome:?}"
    );
    let ranking = service
        .db
        .latest_ranking(l_stock::models::Board::Short, Market::Cn)
        .unwrap()
        .expect("ranking snapshot");
    assert_eq!(ranking.items.len(), pool.len());
    assert!(service
        .db
        .latest_snapshot(Market::Cn, "ranking:short")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn unsupported_tab_refresh_is_empty_scope() {
    assert!(scopes_for(Market::Cn, Tab::Macro, None).is_empty());
}

#[tokio::test]
async fn global_concurrency_limits_parallel_requests() {
    let server = MockServer::start().await;
    let index = [
        tencent_line("sh000001", "上证指数", "3891.0"),
        tencent_line("sz399001", "深证成指", "12000.0"),
        tencent_line("sz399006", "创业板指", "2500.0"),
    ]
    .join("\n");
    let index_bytes = encoding_rs::GBK.encode(&index).0.to_vec();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(index_bytes)
                .set_delay(std::time::Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/qt/clist/get"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(
                    r#"{"data":{"diff":[{"f104":3000,"f105":1800,"f106":100}]}}"#,
                    "application/json",
                )
                .set_delay(std::time::Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let (service, _dir) = build_with(&server, 0, 1);
    let index_key = ScopeKey::new(Market::Cn, Tab::Index, None);
    let sentiment_key = ScopeKey::new(Market::Cn, Tab::Sentiment, None);
    let started = std::time::Instant::now();
    let (a, b) = tokio::join!(
        service.request(index_key, Trigger::Manual),
        service.request(sentiment_key, Trigger::Manual)
    );
    let elapsed = started.elapsed();
    assert!(matches!(a, RefreshOutcome::Success { .. }), "{a:?}");
    assert!(matches!(b, RefreshOutcome::Success { .. }), "{b:?}");
    assert!(
        elapsed >= std::time::Duration::from_millis(350),
        "elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn all_sources_failure_keeps_old_snapshot_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (service, _dir) = build(&server, 0);
    service
        .db
        .add_watch(Market::Cn, "sh600519", "贵州茅台")
        .unwrap();
    let key = ScopeKey::new(Market::Cn, Tab::Watch, None);
    let outcome = service.request(key, Trigger::Manual).await;
    assert!(
        matches!(outcome, RefreshOutcome::Failed { .. }),
        "{outcome:?}"
    );
    assert!(service
        .db
        .latest_snapshot(Market::Cn, "watch")
        .unwrap()
        .is_none());
    let run = service
        .db
        .latest_refresh_run(Market::Cn, "watch")
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "failed");
    assert!(run.error.unwrap().contains("所有行情源均失败"));
}

#[tokio::test]
async fn default_pools_seed_both_markets() {
    let server = MockServer::start().await;
    let (service, _dir) = build(&server, 0);
    assert_eq!(service.db.pool(Market::Cn).unwrap().len(), 20);
    assert_eq!(service.db.pool(Market::Us).unwrap().len(), 20);
}

#[tokio::test]
async fn saved_weights_are_loaded_for_next_ranking() {
    let server = MockServer::start().await;
    let (service, _dir) = build(&server, 0);
    let mut weights = std::collections::HashMap::new();
    weights.insert("momentum".to_string(), 0.5);
    l_stock::rankings::save_weights(&service.db, l_stock::models::Board::Short, &weights).unwrap();
    let specs =
        l_stock::rankings::load_weights(&service.db, l_stock::models::Board::Short).unwrap();
    let momentum = specs.iter().find(|spec| spec.key == "momentum").unwrap();
    assert!(momentum.weight > 0.35);
}
