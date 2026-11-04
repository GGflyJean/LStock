use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use base64::Engine;
use rand::RngCore;

pub const SESSION_COOKIE: &str = "l_stock_session";

#[derive(Debug, Clone)]
struct Session {
    expires: Instant,
    csrf: String,
}

#[derive(Debug, Clone)]
struct FailureState {
    count: u32,
    locked_until: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    pub token: String,
    pub csrf: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    InvalidCredentials,
    Locked { remaining_secs: u64 },
}

#[derive(Clone)]
pub struct Auth {
    user: Arc<String>,
    password_hash: Arc<String>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    failures: Arc<Mutex<HashMap<String, FailureState>>>,
    ttl: Duration,
    max_failures: u32,
    lockout: Duration,
}

impl Auth {
    pub fn new(user: impl Into<String>, password_hash: impl Into<String>) -> Self {
        Self {
            user: Arc::new(user.into()),
            password_hash: Arc::new(password_hash.into()),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            failures: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(7 * 24 * 60 * 60),
            max_failures: 5,
            lockout: Duration::from_secs(60),
        }
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn verify_password(&self, user: &str, password: &str) -> bool {
        if user != self.user.as_str() {
            return false;
        }
        let Ok(parsed) = PasswordHash::new(&self.password_hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    pub fn locked_for(&self, client: &str) -> Option<u64> {
        let now = Instant::now();
        let mut failures = self.failures.lock().ok()?;
        if let Some(state) = failures.get_mut(client) {
            if let Some(until) = state.locked_until {
                if until > now {
                    return Some((until - now).as_secs().max(1));
                }
                state.locked_until = None;
                state.count = 0;
            }
        }
        None
    }

    pub fn login(
        &self,
        user: &str,
        password: &str,
        client: &str,
    ) -> std::result::Result<LoginSuccess, LoginError> {
        if let Some(remaining) = self.locked_for(client) {
            return Err(LoginError::Locked {
                remaining_secs: remaining,
            });
        }
        if !self.verify_password(user, password) {
            self.record_failure(client);
            return Err(LoginError::InvalidCredentials);
        }
        if let Ok(mut failures) = self.failures.lock() {
            failures.remove(client);
        }
        let token = random_token();
        let csrf = random_token();
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(
                token.clone(),
                Session {
                    expires: Instant::now() + self.ttl,
                    csrf: csrf.clone(),
                },
            );
        }
        Ok(LoginSuccess { token, csrf })
    }

    fn record_failure(&self, client: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            let state = failures.entry(client.to_string()).or_insert(FailureState {
                count: 0,
                locked_until: None,
            });
            state.count += 1;
            if state.count >= self.max_failures {
                state.locked_until = Some(Instant::now() + self.lockout);
            }
        }
    }

    pub fn check_session(&self, token: &str) -> bool {
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        let now = Instant::now();
        sessions.retain(|_, session| session.expires > now);
        sessions.contains_key(token)
    }

    pub fn session_csrf(&self, token: &str) -> Option<String> {
        let Ok(mut sessions) = self.sessions.lock() else {
            return None;
        };
        let now = Instant::now();
        sessions.retain(|_, session| session.expires > now);
        sessions.get(token).map(|session| session.csrf.clone())
    }

    pub fn verify_csrf(&self, token: &str, csrf: &str) -> bool {
        self.session_csrf(token)
            .map(|expected| constant_time_eq(expected.as_bytes(), csrf.as_bytes()))
            .unwrap_or(false)
    }

    pub fn logout(&self, token: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(token);
        }
    }

    pub fn authenticate(&self, headers: &HeaderMap) -> bool {
        if let Some(token) = session_token(headers) {
            if self.check_session(&token) {
                return true;
            }
        }
        false
    }
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| anyhow::anyhow!("生成密码哈希失败: {error}"))
}

pub fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{SESSION_COOKIE}=")) {
            return Some(value.to_string());
        }
    }
    None
}

pub fn client_key(headers: &HeaderMap) -> String {
    for header in ["x-forwarded-for", "x-real-ip"] {
        if let Some(value) = headers.get(header).and_then(|value| value.to_str().ok()) {
            if let Some(first) = value.split(',').next() {
                let first = first.trim();
                if !first.is_empty() {
                    return first.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

pub fn is_secure_request(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800{secure_attr}")
}

pub fn clear_cookie(secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure_attr}")
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        let hash = hash_password("secret").expect("hash");
        Auth::new("trader", hash)
    }

    #[test]
    fn verifies_password() {
        let auth = auth();
        assert!(auth.verify_password("trader", "secret"));
        assert!(!auth.verify_password("trader", "wrong"));
        assert!(!auth.verify_password("other", "secret"));
    }

    #[test]
    fn session_roundtrip_and_csrf() {
        let auth = auth();
        let login = auth.login("trader", "secret", "1.2.3.4").expect("login");
        assert!(auth.check_session(&login.token));
        assert!(auth.verify_csrf(&login.token, &login.csrf));
        assert!(!auth.verify_csrf(&login.token, "wrong"));
        auth.logout(&login.token);
        assert!(!auth.check_session(&login.token));
    }

    #[test]
    fn repeated_failures_lock_client() {
        let auth = auth();
        for _ in 0..5 {
            assert_eq!(
                auth.login("trader", "wrong", "9.9.9.9"),
                Err(LoginError::InvalidCredentials)
            );
        }
        assert!(matches!(
            auth.login("trader", "secret", "9.9.9.9"),
            Err(LoginError::Locked { .. })
        ));
        assert!(auth.login("trader", "secret", "8.8.8.8").is_ok());
    }

    #[test]
    fn secure_cookie_attribute() {
        assert!(session_cookie("token", true).contains("Secure"));
        assert!(!session_cookie("token", false).contains("Secure"));
        assert!(clear_cookie(true).contains("Secure"));
    }
}
