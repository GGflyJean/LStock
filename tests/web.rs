mod support;

use axum::body::Body;
use axum::http::{header, StatusCode};
use chrono::Utc;
use l_stock::models::{
    Board, DelayKind, IndexPayload, InsufficientItem, MacroItem, MacroPayload, Market, MoverItem,
    MoversPayload, NewsItem, NewsPayload, Quote, RefreshStatus, ScopeKey, SectorItem,
    SectorPayload, SentimentMetric, SentimentReport, Tab, WatchPayload,
};
use tower::ServiceExt;

fn quote(market: Market, symbol: &str, name: &str, price: f64) -> Quote {
    Quote {
        market,
        symbol: symbol.to_string(),
        name: name.to_string(),
        price,
        prev_close: price - 1.0,
        change: 1.0,
        change_pct: 0.5,
        volume: 1000.0,
        amount: 1000.0,
        volume_ratio: Some(1.2),
        turnover: Some(2.0),
        fund_flow: Some(100.0),
        breakout_pct: Some(0.5),
        volatility: Some(1.0),
        source: "测试源".to_string(),
        delay: DelayKind::Realtime,
        as_of_utc: Utc::now(),
    }
}

#[tokio::test]
async fn unauthenticated_redirects_to_login() {
    let h = support::app(&[]);
    let request = axum::http::Request::builder()
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let response = h.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/login");
}

#[tokio::test]
async fn dual_view_renders_both_markets_from_snapshots() {
    let h = support::app(&[]);
    let now = Utc::now();
    let cn = IndexPayload {
        indices: vec![quote(Market::Cn, "sh000001", "上证指数", 3891.0)],
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    let us = IndexPayload {
        indices: vec![quote(Market::Us, "^IXIC", "纳斯达克", 18000.0)],
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    h.state
        .db
        .insert_snapshot(
            Market::Cn,
            "index",
            now,
            &serde_json::to_string(&cn).unwrap(),
        )
        .unwrap();
    h.state
        .db
        .insert_snapshot(
            Market::Us,
            "index",
            now,
            &serde_json::to_string(&us).unwrap(),
        )
        .unwrap();
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=dual&tab=index"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = support::body_text(response).await;
    assert!(body.contains("上证指数"));
    assert!(body.contains("纳斯达克"));
    assert!(body.contains("A股"));
    assert!(body.contains("美股"));
    assert!(body.contains("panes-dual"));
    assert!(body.contains("A股 数据时效"));
    assert!(body.contains("美股 数据时效"));
    assert!(body.contains("utc-clock"));
}

#[tokio::test]
async fn refresh_rejects_missing_csrf_and_accepts_valid_csrf() {
    let h = support::app(&[]);
    let missing = h
        .router()
        .oneshot(h.form_request(
            "/api/refresh",
            &[("market", "cn"), ("tab", "watch"), ("csrf", "")],
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let wrong = h
        .router()
        .oneshot(h.form_request(
            "/api/refresh",
            &[("market", "cn"), ("tab", "watch"), ("csrf", "wrong")],
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);

    let ok = h
        .router()
        .oneshot(h.form_request(
            "/api/refresh",
            &[("market", "cn"), ("tab", "watch"), ("csrf", &h.csrf)],
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = support::body_text(ok).await;
    assert!(body.contains("success"));
    let snapshot = h.state.db.latest_snapshot(Market::Cn, "watch").unwrap();
    assert!(snapshot.is_some());
}

#[tokio::test]
async fn watchlist_is_separate_between_markets() {
    let h = support::app(&[]);
    let add = h
        .router()
        .oneshot(h.form_request(
            "/watch/cn",
            &[
                ("csrf", &h.csrf),
                ("symbol", "600519"),
                ("name", "贵州茅台"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::SEE_OTHER);

    let cn = h
        .router()
        .oneshot(h.request("GET", "/?view=cn&tab=watch"))
        .await
        .unwrap();
    let cn_body = support::body_text(cn).await;
    assert!(cn_body.contains("贵州茅台"));
    assert!(cn_body.contains("600519"));

    let us = h
        .router()
        .oneshot(h.request("GET", "/?view=us&tab=watch"))
        .await
        .unwrap();
    let us_body = support::body_text(us).await;
    assert!(!us_body.contains("贵州茅台"));

    let item = h.state.db.watchlist(Market::Cn).unwrap().pop().unwrap();
    let remove = h
        .router()
        .oneshot(h.form_request(
            &format!("/watch/cn/delete/{}", item.id),
            &[("csrf", &h.csrf)],
        ))
        .await
        .unwrap();
    assert_eq!(remove.status(), StatusCode::SEE_OTHER);
    assert!(h.state.db.watchlist(Market::Cn).unwrap().is_empty());
}

#[tokio::test]
async fn invalid_symbol_is_rejected() {
    let h = support::app(&[]);
    let response = h
        .router()
        .oneshot(h.form_request("/watch/cn", &[("csrf", &h.csrf), ("symbol", "not-valid")]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unsupported_tab_shows_placeholder() {
    let h = support::app(&[]);
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=cn&tab=news"))
        .await
        .unwrap();
    let body = support::body_text(response).await;
    assert!(body.contains("暂无新闻快照"));
    assert!(!body.contains("暂未开放"));
}

#[tokio::test]
async fn ranking_page_renders_items_and_weights_form() {
    let h = support::app(&[]);
    let now = Utc::now();
    let payload = l_stock::models::RankingPayload {
        board: Board::Short,
        market: Market::Cn,
        as_of_utc: now,
        items: vec![l_stock::models::RankingItem {
            rank: 1,
            symbol: "sh600519".to_string(),
            name: "贵州茅台".to_string(),
            score: 88.0,
            factors: vec![l_stock::models::FactorScore {
                key: "momentum".to_string(),
                label: "动量".to_string(),
                raw: Some(1.0),
                z: 1.0,
                weight: 0.3,
                contribution: 0.3,
            }],
        }],
        insufficient: Vec::new(),
        pool_size: 1,
        weights_summary: "动量 30%".to_string(),
    };
    h.state
        .db
        .insert_snapshot(
            Market::Cn,
            "ranking:short",
            now,
            &serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=cn&tab=ranking&board=short"))
        .await
        .unwrap();
    let body = support::body_text(response).await;
    assert!(body.contains("贵州茅台"));
    assert!(body.contains("88.0"));
    assert!(body.contains("调整因子权重"));
    assert!(body.contains("/weights/short"));
    assert!(body.contains("info-icon"));
    assert!(body.contains("data-info-key=\"momentum\""));
}

#[tokio::test]
async fn weights_form_persists_preferences() {
    let h = support::app(&[]);
    let response = h
        .router()
        .oneshot(h.form_request(
            "/weights/short",
            &[
                ("csrf", &h.csrf),
                ("view", "cn"),
                ("momentum", "50"),
                ("relative_strength", "20"),
                ("volume_ratio", "10"),
                ("turnover", "10"),
                ("fund_flow", "10"),
                ("breakout", "0"),
                ("volatility", "0"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let stored = h.state.db.get_pref("ranking_weights").unwrap();
    assert!(stored.unwrap().contains("momentum"));
}

#[tokio::test]
async fn status_and_health_pages() {
    let h = support::app(&[]);
    let health = h
        .router()
        .oneshot(
            axum::http::Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    h.state
        .db
        .insert_refresh_run(
            Market::Cn,
            "watch",
            "manual",
            RefreshStatus::Running.as_str(),
        )
        .unwrap();
    let status = h
        .router()
        .oneshot(h.request("GET", "/status"))
        .await
        .unwrap();
    let body = support::body_text(status).await;
    assert!(body.contains("刷新状态"));
    assert!(body.contains("watch"));
}

#[tokio::test]
async fn scope_keys_match_tabs_and_boards() {
    assert_eq!(
        ScopeKey::new(Market::Cn, Tab::Ranking, Some(Board::Long)).scope(),
        "ranking:long"
    );
    assert_eq!(ScopeKey::new(Market::Us, Tab::Index, None).scope(), "index");
}

#[tokio::test]
async fn login_sets_cookie_and_supports_logout() {
    let h = support::app(&[]);
    let body = "username=trader&password=secret&next=/?view=cn";
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let response = h.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/?view=cn"
    );
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("l_stock_session="));

    let logout = h
        .router()
        .oneshot(h.form_request("/logout", &[("csrf", &h.csrf)]))
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::SEE_OTHER);
    assert_eq!(logout.headers().get(header::LOCATION).unwrap(), "/login");
}

#[tokio::test]
async fn api_status_returns_json_state() {
    let h = support::app(&[]);
    let response = h
        .router()
        .oneshot(h.request("GET", "/api/status"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = support::body_text(response).await;
    assert!(body.contains("next_cn"));
    assert!(body.contains("health"));
}

#[tokio::test]
async fn stale_and_failed_snapshot_is_highlighted() {
    let h = support::app(&[]);
    let old = Utc::now() - chrono::Duration::hours(8);
    let payload = IndexPayload {
        indices: vec![quote(Market::Cn, "sh000001", "上证指数", 3891.0)],
        source_note: "来源：测试".to_string(),
        as_of_utc: old,
    };
    h.state
        .db
        .insert_snapshot(
            Market::Cn,
            "index",
            old,
            &serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();
    let run_id = h
        .state
        .db
        .insert_refresh_run(
            Market::Cn,
            "index",
            "scheduled",
            RefreshStatus::Failed.as_str(),
        )
        .unwrap();
    h.state
        .db
        .finish_refresh_run(run_id, "failed", None, Some("数据源不可用"))
        .unwrap();
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=cn&tab=index"))
        .await
        .unwrap();
    let body = support::body_text(response).await;
    assert!(body.contains("已过期"));
    assert!(body.contains("上次刷新失败"));
    assert!(body.contains("数据源不可用"));
}

#[tokio::test]
async fn watch_snapshot_renders_quotes() {
    let h = support::app(&[]);
    let now = Utc::now();
    let payload = WatchPayload {
        quotes: vec![quote(Market::Cn, "sh600519", "贵州茅台", 1500.0)],
        missing: Vec::new(),
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    h.state
        .db
        .insert_snapshot(
            Market::Cn,
            "watch",
            now,
            &serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=cn&tab=watch"))
        .await
        .unwrap();
    let body = support::body_text(response).await;
    assert!(body.contains("贵州茅台"));
    assert!(body.contains("1500.00"));
}

#[tokio::test]
async fn dual_sentiment_renders_two_temperatures() {
    let h = support::app(&[]);
    let now = Utc::now();
    for market in [Market::Cn, Market::Us] {
        let report = SentimentReport {
            market,
            temperature: if market == Market::Cn { 62.0 } else { 38.0 },
            band: "中性".to_string(),
            metrics: vec![SentimentMetric {
                key: "breadth".to_string(),
                label: "涨跌家数".to_string(),
                value: "涨 3000 / 跌 1800".to_string(),
                raw: Some(60.0),
                note: String::new(),
            }],
            missing: Vec::new(),
            source: "测试源".to_string(),
            delay: DelayKind::Realtime,
            as_of_utc: now,
        };
        h.state
            .db
            .insert_snapshot(
                market,
                "sentiment",
                now,
                &serde_json::to_string(&report).unwrap(),
            )
            .unwrap();
    }
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=dual&tab=sentiment"))
        .await
        .unwrap();
    let body = support::body_text(response).await;
    assert_eq!(body.matches("情绪温度").count(), 2);
    assert!(body.contains("62.0"));
    assert!(body.contains("38.0"));
}

#[tokio::test]
async fn ranking_insufficient_list_is_rendered() {
    let h = support::app(&[]);
    let now = Utc::now();
    let payload = l_stock::models::RankingPayload {
        board: Board::Long,
        market: Market::Cn,
        as_of_utc: now,
        items: Vec::new(),
        insufficient: vec![InsufficientItem {
            symbol: "sh600036".to_string(),
            name: "招商银行".to_string(),
            missing: vec!["ROIC".to_string()],
            factors: vec![
                l_stock::models::DetailValue {
                    key: "revenue_growth".to_string(),
                    label: "营收增速".to_string(),
                    value: "2.10".to_string(),
                    missing: false,
                },
                l_stock::models::DetailValue {
                    key: "roic".to_string(),
                    label: "ROIC".to_string(),
                    value: "缺失".to_string(),
                    missing: true,
                },
            ],
        }],
        pool_size: 1,
        weights_summary: "测试权重".to_string(),
    };
    h.state
        .db
        .insert_snapshot(
            Market::Cn,
            "ranking:long",
            now,
            &serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();
    let response = h
        .router()
        .oneshot(h.request("GET", "/?view=cn&tab=ranking&board=long"))
        .await
        .unwrap();
    let body = support::body_text(response).await;
    assert!(body.contains("数据不足"));
    assert!(body.contains("招商银行"));
    assert!(body.contains("ROIC"));
    assert!(body.contains("营收增速"));
    assert!(body.contains("2.10"));
    assert!(body.contains("缺失"));
    assert!(body.contains("data-info-key=\"roic\""));
    assert!(body.contains("data-info-key=\"revenue_growth\""));
}

#[tokio::test]
async fn new_tabs_render_snapshots() {
    let h = support::app(&[]);
    let now = Utc::now();
    let sectors = SectorPayload {
        sectors: vec![SectorItem {
            code: "BK1326".to_string(),
            name: "半导体设备".to_string(),
            change_pct: 2.5,
            leading: "托伦斯".to_string(),
            source: "测试".to_string(),
        }],
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    let movers = MoversPayload {
        gainers: vec![MoverItem {
            symbol: "sh600519".to_string(),
            name: "贵州茅台".to_string(),
            price: 1500.0,
            change_pct: 3.2,
            amount: 1_0000_0000.0,
        }],
        losers: vec![MoverItem {
            symbol: "sh600036".to_string(),
            name: "招商银行".to_string(),
            price: 40.0,
            change_pct: -2.1,
            amount: 2_0000_0000.0,
        }],
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    let news = NewsPayload {
        items: vec![NewsItem {
            title: "测试快讯".to_string(),
            summary: "摘要".to_string(),
            time: "2026-11-04 21:00:00".to_string(),
            url: "https://example.com".to_string(),
        }],
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    let macro_data = MacroPayload {
        items: vec![MacroItem {
            label: "CPI 同比".to_string(),
            value: "+0.90%".to_string(),
            note: "2026年09月份".to_string(),
            unit: "%".to_string(),
            frequency: "month".to_string(),
            series: vec![
                l_stock::models::MacroPoint {
                    date: "2026-08-01".to_string(),
                    value: 0.6,
                },
                l_stock::models::MacroPoint {
                    date: "2026-09-01".to_string(),
                    value: 0.9,
                },
            ],
        }],
        source_note: "来源：测试".to_string(),
        as_of_utc: now,
    };
    for (scope, payload) in [
        ("sectors", serde_json::to_string(&sectors).unwrap()),
        ("movers", serde_json::to_string(&movers).unwrap()),
        ("news", serde_json::to_string(&news).unwrap()),
        ("macro", serde_json::to_string(&macro_data).unwrap()),
    ] {
        h.state
            .db
            .insert_snapshot(Market::Cn, scope, now, &payload)
            .unwrap();
    }
    let checks = [
        ("sectors", "半导体设备"),
        ("movers", "涨幅榜"),
        ("news", "测试快讯"),
        ("macro", "macro-canvas"),
    ];
    for (tab, expected) in checks {
        let response = h
            .router()
            .oneshot(h.request("GET", &format!("/?view=cn&tab={tab}")))
            .await
            .unwrap();
        let body = support::body_text(response).await;
        assert!(body.contains(expected), "tab={tab} missing {expected}");
        assert!(!body.contains("暂未开放"), "tab={tab} still pending");
        if tab == "macro" {
            assert!(body.contains("时间粒度"));
            assert!(body.contains("data-series"));
        }
    }
}
