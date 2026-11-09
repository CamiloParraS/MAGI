//! An in-process stand-in for Supabase Auth and the Magi server, in the style
//! of `MAGI_FAKE_EMBEDDER`: `MAGI_FAKE_ASK=1` runs the app against it, so tests
//! and CI need no account and no network. Cloning shares the same state.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};

use super::{Endpoints, HttpRequest, HttpResponse, Method, Transport, unix_now};

const SUPABASE: &str = "https://fake.supabase.test";
const WORKER: &str = "https://fake.worker.test";
const DAILY_LIMIT: u32 = 20;

#[derive(Default)]
struct State {
    /// email -> password
    users: HashMap<String, String>,
    /// access token -> (email, expires_at)
    access: HashMap<String, (String, u64)>,
    /// refresh tokens that have not been spent: token -> email
    refresh: HashMap<String, String>,
    spent: HashSet<String>,
    asked: HashMap<String, u32>,
    issued: u64,
    lifetime_secs: u64,
    log: Vec<String>,
}

#[derive(Clone)]
pub struct FakeBackend {
    state: Arc<Mutex<State>>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn reply(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        body: body.to_string(),
    }
}

impl FakeBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                lifetime_secs: 3600,
                ..State::default()
            })),
        }
    }

    pub fn endpoints() -> Endpoints {
        Endpoints {
            supabase_url: SUPABASE.into(),
            anon_key: "fake-anon-key".into(),
            worker_url: WORKER.into(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How long a new access token lasts; 0 means it is already expired.
    pub fn set_lifetime_secs(&self, secs: u64) {
        self.state().lifetime_secs = secs;
    }

    /// Every request so far, as `METHOD /path`, plus anything passed to [`note`](Self::note).
    pub fn log(&self) -> Vec<String> {
        self.state().log.clone()
    }

    pub fn requests(&self) -> usize {
        self.state()
            .log
            .iter()
            .filter(|l| !l.starts_with("note "))
            .count()
    }

    pub fn count(&self, needle: &str) -> usize {
        self.state()
            .log
            .iter()
            .filter(|l| l.contains(needle))
            .count()
    }

    /// Lets a test put its own events (a keychain save) in the request order.
    pub fn note(&self, event: &str) {
        self.state().log.push(format!("note {event}"));
    }

    /// Makes the server forget every session, like a revoked refresh token.
    pub fn revoke_all_sessions(&self) {
        let mut s = self.state();
        s.refresh.clear();
        s.access.clear();
    }

    fn issue(s: &mut State, email: &str) -> Value {
        s.issued += 1;
        let access = format!("access-{}", s.issued);
        let refresh = format!("refresh-{}", s.issued);
        let expires_at = unix_now() + s.lifetime_secs;
        s.access
            .insert(access.clone(), (email.to_string(), expires_at));
        s.refresh.insert(refresh.clone(), email.to_string());
        json!({
            "access_token": access,
            "refresh_token": refresh,
            "expires_in": s.lifetime_secs,
            "expires_at": expires_at,
            "user": { "email": email },
        })
    }

    fn auth(&self, s: &mut State, request: &HttpRequest, path: &str, body: &Value) -> HttpResponse {
        let text = |key: &str| body[key].as_str().unwrap_or_default().to_string();
        match path {
            "/auth/v1/token?grant_type=password" => {
                let (email, password) = (text("email"), text("password"));
                if s.users.get(&email) == Some(&password) {
                    reply(200, Self::issue(s, &email))
                } else {
                    reply(
                        400,
                        json!({ "error_code": "invalid_credentials", "msg": "Invalid login credentials" }),
                    )
                }
            }
            "/auth/v1/token?grant_type=refresh_token" => {
                let token = text("refresh_token");
                match s.refresh.remove(&token) {
                    Some(email) => {
                        s.spent.insert(token);
                        reply(200, Self::issue(s, &email))
                    }
                    None => {
                        let code = if s.spent.contains(&token) {
                            "refresh_token_already_used"
                        } else {
                            "refresh_token_not_found"
                        };
                        reply(
                            400,
                            json!({ "error_code": code, "msg": "Invalid Refresh Token" }),
                        )
                    }
                }
            }
            "/auth/v1/signup" => {
                let (email, password) = (text("email"), text("password"));
                if s.users.contains_key(&email) {
                    return reply(
                        422,
                        json!({ "error_code": "user_already_exists", "msg": "User already registered" }),
                    );
                }
                if password.len() < 6 {
                    return reply(
                        422,
                        json!({ "error_code": "weak_password", "msg": "Password should be at least 6 characters." }),
                    );
                }
                s.users.insert(email.clone(), password);
                reply(200, Self::issue(s, &email))
            }
            "/auth/v1/logout" => {
                if let Some(token) = Self::bearer(request) {
                    s.access.remove(&token);
                }
                reply(204, json!(null))
            }
            _ => reply(404, json!({ "msg": "not found" })),
        }
    }

    fn bearer(request: &HttpRequest) -> Option<String> {
        request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .and_then(|(_, v)| v.strip_prefix("Bearer "))
            .map(String::from)
    }

    fn account(s: &State, request: &HttpRequest) -> Option<String> {
        let token = Self::bearer(request)?;
        let (email, expires_at) = s.access.get(&token)?;
        (*expires_at > unix_now()).then(|| email.clone())
    }

    fn worker(
        &self,
        s: &mut State,
        request: &HttpRequest,
        path: &str,
        body: &Value,
    ) -> HttpResponse {
        let Some(email) = Self::account(s, request) else {
            return reply(401, json!({ "error": "unauthorized" }));
        };
        match (request.method, path) {
            (Method::Post, "/ask") => {
                let asked = s.asked.entry(email).or_insert(0);
                if *asked >= DAILY_LIMIT {
                    return reply(
                        429,
                        json!({ "error": "limit", "scope": "account", "used": *asked, "limit": DAILY_LIMIT }),
                    );
                }
                *asked += 1;
                let first = body["sources"][0]["id"].as_str();
                reply(
                    200,
                    json!({
                        "answer": format!("Fake answer to: {}", body["question"].as_str().unwrap_or("")),
                        "cited": first.into_iter().collect::<Vec<_>>(),
                        "used": *asked,
                        "limit": DAILY_LIMIT,
                    }),
                )
            }
            (Method::Delete, "/account") => {
                s.users.remove(&email);
                s.asked.remove(&email);
                s.refresh.retain(|_, e| *e != email);
                s.access.retain(|_, (e, _)| *e != email);
                reply(204, json!(null))
            }
            _ => reply(404, json!({ "error": "not_found" })),
        }
    }
}

impl Transport for FakeBackend {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let mut s = self.state();
        let body: Value = request
            .body
            .as_deref()
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or(Value::Null);
        if let Some(path) = request.url.strip_prefix(SUPABASE) {
            s.log.push(format!("{:?} {path}", request.method));
            Ok(self.auth(&mut s, request, path, &body))
        } else if let Some(path) = request.url.strip_prefix(WORKER) {
            s.log.push(format!("{:?} {path}", request.method));
            Ok(self.worker(&mut s, request, path, &body))
        } else {
            Err(format!("fake backend has no route for {}", request.url))
        }
    }
}
