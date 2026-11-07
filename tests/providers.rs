use l_stock::config::ProviderConfig;
use l_stock::models::{Board, Market};
use l_stock::providers::{
    eastmoney::EastmoneyProvider, sina::SinaProvider, stooq::StooqProvider,
    tencent::TencentProvider, yahoo::YahooProvider, CnSentimentProvider, FundamentalProvider,
    HttpClient, IndexProvider, Providers, QuoteProvider,
};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider_config(server: &MockServer) -> ProviderConfig {
    ProviderConfig {
        tencent_base: server.uri(),
        sina_base: server.uri(),
        eastmoney_base: server.uri(),
        eastmoney_datacenter_base: server.uri(),
        eastmoney_push2ex_base: server.uri(),
        eastmoney_news_base: server.uri(),
        eastmoney_fallback_bases: Vec::new(),
        yahoo_base: server.uri(),
        stooq_base: server.uri(),
        timeout_secs: 5,
        retries: 0,
        user_agent: "l_stock-test".to_string(),
        proxy: None,
    }
}

fn tencent_payload(name: &str, symbol: &str, price: &str, prev: &str) -> Vec<u8> {
    let mut fields = vec!["1", name, symbol, price, prev, prev, "1000"];
    while fields.len() < 33 {
        fields.push("0");
    }
    fields[31] = "1.0";
    fields[32] = "0.5";
    let line = format!("v_{symbol}=\"{}\";", fields.join("~"));
    encoding_rs::GBK.encode(&line).0.to_vec()
}

fn tencent_valuation_payload() -> Vec<u8> {
    let mut fields = vec![
        "1",
        "贵州茅台",
        "600519",
        "1446.44",
        "1480.0",
        "1470.0",
        "1000",
    ];
    while fields.len() < 73 {
        fields.push("0");
    }
    fields[52] = "21.04";
    fields[72] = "1250081601";
    let line = format!("v_sh600519=\"{}\"", fields.join("~"));
    encoding_rs::GBK.encode(&line).0.to_vec()
}

#[tokio::test]
async fn tencent_parses_gbk_via_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tencent_payload(
            "贵州茅台",
            "sh600519",
            "1500.0",
            "1490.0",
        )))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = TencentProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let quotes = provider
        .quotes(Market::Cn, &["sh600519".to_string()])
        .await
        .unwrap();
    assert_eq!(quotes[0].name, "贵州茅台");
    assert_eq!(quotes[0].symbol, "sh600519");
}

#[tokio::test]
async fn tencent_failure_falls_back_to_sina() {
    let tencent = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&tencent)
        .await;
    let sina = MockServer::start().await;
    let body =
        r#"var hq_str_sh600519="贵州茅台,1490.0,1490.0,1500.0,1510.0,1480.0,0,0,1000,1500000.0";"#;
    let (bytes, _, _) = encoding_rs::GBK.encode(body);
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&sina)
        .await;
    let mut config = provider_config(&tencent);
    config.sina_base = sina.uri();
    let providers = Providers::new(&config).unwrap();
    let payload = providers
        .quotes(Market::Cn, &["sh600519".to_string()])
        .await
        .unwrap();
    assert!(payload.source_note.contains("新浪"));
    assert!(payload.source_note.contains("降级"));
    let health = providers.health.snapshot();
    assert!(health
        .iter()
        .any(|item| item.source == "tencent" && !item.ok));
    assert!(health.iter().any(|item| item.source == "sina" && item.ok));
}

#[tokio::test]
async fn proxy_configuration_is_applied() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tencent_payload(
            "贵州茅台",
            "sh600519",
            "1500.0",
            "1490.0",
        )))
        .mount(&server)
        .await;
    let mut config = provider_config(&server);
    config.proxy = Some("http://127.0.0.1:1".to_string());
    let provider = TencentProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let result = provider.quotes(Market::Cn, &["sh600519".to_string()]).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn eastmoney_parses_sentiment_via_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/qt/ulist.np/get"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"data":{"diff":[{"f104":1849,"f105":440,"f106":68,"f6":953265155867.1},{"f104":2547,"f105":331,"f106":60,"f6":1084426578752.9}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/getTopicZTPool"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"data":{"tc":118,"pool":[{"lbc":5},{"lbc":2}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/getTopicDTPool"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(r#"{"data":{"tc":10}}"#, "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/getTopicZBPool"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(r#"{"data":{"tc":33}}"#, "application/json"),
        )
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = EastmoneyProvider::new(
        HttpClient::new(&config).unwrap(),
        server.uri(),
        server.uri(),
        server.uri(),
        server.uri(),
        Vec::new(),
        None,
    );
    let raw = provider.sentiment().await.unwrap();
    assert_eq!(raw.up, 4396);
    assert_eq!(raw.down, 771);
    assert_eq!(raw.limit_up, 118);
    assert_eq!(raw.limit_down, 10);
    assert_eq!(raw.max_streak, 5);
}

#[tokio::test]
async fn yahoo_marks_delayed_and_parses_indices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("/v8/finance/chart/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"chart":{"result":[{"meta":{"symbol":"AAPL","shortName":"Apple","regularMarketPrice":210.5,"chartPreviousClose":208.0,"regularMarketTime":1793520000}}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = YahooProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let quotes = provider
        .quotes(Market::Us, &["AAPL".to_string()])
        .await
        .unwrap();
    assert_eq!(quotes[0].delay, l_stock::models::DelayKind::Delayed);
    let indices = provider.indices(Market::Us).await.unwrap();
    assert_eq!(indices.len(), 3);
}

#[tokio::test]
async fn us_quote_falls_back_to_yahoo() {
    let tencent = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&tencent)
        .await;
    let yahoo = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("/v8/finance/chart/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"chart":{"result":[{"meta":{"symbol":"AAPL","shortName":"Apple","regularMarketPrice":210.5,"chartPreviousClose":208.0}}]}}"#,
            "application/json",
        ))
        .mount(&yahoo)
        .await;
    let mut config = provider_config(&tencent);
    config.yahoo_base = yahoo.uri();
    let providers = Providers::new(&config).unwrap();
    let payload = providers
        .quotes(Market::Us, &["AAPL".to_string()])
        .await
        .unwrap();
    assert!(payload.source_note.contains("Yahoo"));
}

#[tokio::test]
async fn stooq_parses_csv_index_fallback() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/q/l/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "Symbol,Date,Time,Open,High,Low,Close,Volume\n^spx,2026-10-31,16:00:00,6800.5,6850,6790,6840,1000\n",
            "text/csv",
        ))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = StooqProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let indices = provider.indices(Market::Us).await.unwrap();
    assert_eq!(indices.len(), 3);
}

#[tokio::test]
async fn eastmoney_fundamentals_parse_finance_dividend_and_valuation() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/data/v1/get"))
        .and(query_param("reportName", "RPT_F10_FINANCE_MAINFINADATA"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"result":{"data":[{"TOTALOPERATEREVETZ":1.82,"PARENTNETPROFITTZ":-0.25,"ROEJQ":24.47,"ROIC":23.27,"XSMLL_TB":-1.88,"FCFF_FORWARD":33869111226.9,"ZCFZL":11.9}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/data/v1/get"))
        .and(query_param("reportName", "RPT_SHAREBONUS_DET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"result":{"data":[
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":280.2423,"REPORT_DATE":"2025-12-31 00:00:00","BASIC_EPS":65.66},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":239.57,"REPORT_DATE":"2025-09-30 00:00:00","BASIC_EPS":51.53},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":276.73,"REPORT_DATE":"2024-12-31 00:00:00","BASIC_EPS":68.64},
                {"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":238.82,"REPORT_DATE":"2024-09-30 00:00:00","BASIC_EPS":48.42}
            ]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/qt/stock/get"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"data":{"f43":1446.44,"f84":1250081601.0,"f162":21.04}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = EastmoneyProvider::new(
        HttpClient::new(&config).unwrap(),
        server.uri(),
        server.uri(),
        server.uri(),
        server.uri(),
        Vec::new(),
        None,
    );
    let fundamentals = provider.fundamentals(Market::Cn, "sh600519").await.unwrap();
    assert!((fundamentals.revenue_growth.unwrap() - 1.82).abs() < 1e-6);
    assert!((fundamentals.roic.unwrap() - 23.27).abs() < 1e-6);
    assert!(fundamentals.dividend_yield.unwrap() > 3.0);
    assert!(fundamentals.dividend_years.unwrap() >= 2.0);
}

#[tokio::test]
async fn yahoo_fundamentals_are_persisted_shape() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("/v10/finance/quoteSummary/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"quoteSummary":{"result":[{"financialData":{"revenueGrowth":{"raw":0.12},"returnOnEquity":{"raw":0.25},"freeCashflow":{"raw":1000},"debtToEquity":{"raw":40}},"summaryDetail":{"dividendYield":{"raw":0.02},"payoutRatio":{"raw":0.3},"trailingPE":{"raw":15}}}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = YahooProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let fundamentals = provider.fundamentals(Market::Us, "AAPL").await.unwrap();
    assert!((fundamentals.revenue_growth.unwrap() - 12.0).abs() < 1e-6);
    assert_eq!(fundamentals.market, Market::Us);
    assert!(fundamentals
        .missing_for(Board::Long)
        .contains(&"ROIC".to_string()));
}

#[tokio::test]
async fn tencent_us_indices_are_mapped_to_canonical_symbols() {
    let server = MockServer::start().await;
    let mut body = Vec::new();
    for (index, (name, symbol, price, prev)) in [
        ("道琼斯", "usDJI", "51786.72", "51983.98"),
        ("纳斯达克", "usIXIC", "25372.01", "25164.36"),
        ("标普500", "usINX", "7568.92", "7541.40"),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            body.push(b'\n');
        }
        body.extend_from_slice(&tencent_payload(name, symbol, price, prev));
    }
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = TencentProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let indices = provider.indices(Market::Us).await.unwrap();
    let symbols: Vec<&str> = indices.iter().map(|quote| quote.symbol.as_str()).collect();
    assert_eq!(symbols, vec!["^DJI", "^IXIC", "^GSPC"]);
    assert_eq!(indices[0].delay, l_stock::models::DelayKind::Delayed);
}

#[tokio::test]
async fn sina_us_indices_are_supported() {
    let server = MockServer::start().await;
    let body = concat!(
        "var hq_str_gb_$dji=\"道琼斯,51786.7188,-0.38,2026-11-04 05:35:05,-197.26,51932.6,51932.6,51593.1\";\n",
        "var hq_str_gb_ixic=\"纳斯达克,25372.0094,0.83,2026-11-04 06:30:00,207.64,25246.6,25474.4,25246.6\";\n",
        "var hq_str_gb_inx=\"标普500,7568.92,0.36,2026-11-04 05:30:00,27.52,7552.9,7594.6,7545.3\";\n"
    );
    let (bytes, _, _) = encoding_rs::GBK.encode(body);
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let provider = SinaProvider::new(HttpClient::new(&config).unwrap(), server.uri());
    let indices = provider.indices(Market::Us).await.unwrap();
    let symbols: Vec<&str> = indices.iter().map(|quote| quote.symbol.as_str()).collect();
    assert_eq!(symbols, vec!["^DJI", "^IXIC", "^GSPC"]);
    assert!((indices[0].price - 51786.7188).abs() < 1e-6);
    assert!((indices[0].change + 197.26).abs() < 1e-6);
}

#[tokio::test]
async fn eastmoney_push2_uses_fallback_hosts() {
    let primary = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&primary)
        .await;
    let fallback = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/qt/ulist.np/get"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"data":{"diff":[{"f104":1849,"f105":440,"f106":68,"f6":953265155867.1}]}}"#,
            "application/json",
        ))
        .mount(&fallback)
        .await;
    for (endpoint, body) in [
        ("getTopicZTPool", r#"{"data":{"tc":10,"pool":[{"lbc":2}]}}"#),
        ("getTopicDTPool", r#"{"data":{"tc":1}}"#),
        ("getTopicZBPool", r#"{"data":{"tc":2}}"#),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&fallback)
            .await;
    }
    let config = provider_config(&fallback);
    let provider = EastmoneyProvider::new(
        HttpClient::new(&config).unwrap(),
        primary.uri(),
        primary.uri(),
        fallback.uri(),
        fallback.uri(),
        vec![fallback.uri()],
        None,
    );
    let raw = provider.sentiment().await.unwrap();
    assert_eq!(raw.up, 1849);
}

#[tokio::test]
async fn eastmoney_uses_tencent_valuation_when_push2_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/data/v1/get"))
        .and(query_param("reportName", "RPT_F10_FINANCE_MAINFINADATA"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"result":{"data":[{"TOTALOPERATEREVETZ":1.82,"PARENTNETPROFITTZ":-0.25,"ROEJQ":24.47,"ROIC":23.27,"XSMLL_TB":-1.88,"FCFF_FORWARD":33869111226.9,"ZCFZL":11.9}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/data/v1/get"))
        .and(query_param("reportName", "RPT_SHAREBONUS_DET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"result":{"data":[{"ASSIGN_PROGRESS":"实施分配","PRETAX_BONUS_RMB":280.2423,"REPORT_DATE":"2025-12-31 00:00:00","BASIC_EPS":65.66}]}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/qt/stock/get"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tencent_valuation_payload()))
        .mount(&server)
        .await;
    let config = provider_config(&server);
    let http = HttpClient::new(&config).unwrap();
    let tencent = TencentProvider::new(http.clone(), server.uri());
    let provider = EastmoneyProvider::new(
        http,
        server.uri(),
        server.uri(),
        server.uri(),
        server.uri(),
        Vec::new(),
        Some(tencent),
    );
    let fundamentals = provider.fundamentals(Market::Cn, "sh600519").await.unwrap();
    assert!((fundamentals.valuation_percentile.unwrap() - 21.04).abs() < 1e-6);
    assert!(fundamentals.dividend_yield.unwrap() > 0.0);
    assert!(fundamentals.fcf_coverage.unwrap() > 0.0);
}
