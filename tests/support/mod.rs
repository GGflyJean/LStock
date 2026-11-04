#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response};
use http_body_util::BodyExt;
use l_stock::auth::{hash_password, Auth};
use l_stock::config::Config;
use l_stock::db::Db;
use l_stock::providers::Providers;
use l_stock::service::RefreshService;
use l_stock::web::{self, AppState};
use tempfile::TempDir;

pub struct Harness {
    pub state: AppState,
    pub token: String,
    pub csrf: String,
    _dir: TempDir,
}

impl Harness {
    pub fn router(&self) -> axum::Router {
        web::router(self.state.clone())
    }

    pub fn request(&self, method: &str, uri: &str) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("l_stock_session={}", self.token)).unwrap(),
        );
        request
    }

    pub fn form_request(&self, uri: &str, pairs: &[(&str, &str)]) -> Request<Body> {
        let body = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let mut request = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("l_stock_session={}", self.token)).unwrap(),
        );
        request
    }
}

pub fn app(overrides: &[(&str, &str)]) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut vars: HashMap<String, String> = HashMap::new();
    vars.insert("AUTH_USER".to_string(), "trader".to_string());
    vars.insert(
        "AUTH_PASSWORD_HASH".to_string(),
        hash_password("secret").expect("hash"),
    );
    vars.insert("DATA_DIR".to_string(), dir.path().display().to_string());
    vars.insert("REFRESH_COOLDOWN_SECS".to_string(), "0".to_string());
    for (key, value) in overrides {
        vars.insert(key.to_string(), value.to_string());
    }
    let config = Config::from_map(&vars).expect("config");
    config.ensure_dirs().expect("dirs");
    let db = Db::open(&config.db_path).expect("db");
    l_stock::rankings::seed_default_pool(&db).expect("pool");
    let auth = Arc::new(Auth::new(
        config.auth_user.clone(),
        config.auth_password_hash.clone(),
    ));
    let login = auth.login("trader", "secret", "test").expect("login");
    let providers = Providers::new(&config.provider).expect("providers");
    let service = RefreshService::new(db.clone(), Arc::new(config.clone()), providers);
    Harness {
        state: AppState {
            config: Arc::new(config),
            db,
            auth,
            service,
            static_dir: PathBuf::from("static"),
        },
        token: login.token,
        csrf: login.csrf,
        _dir: dir,
    }
}

pub async fn body_text(response: Response<Body>) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}
