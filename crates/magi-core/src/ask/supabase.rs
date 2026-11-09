//! Supabase Auth over REST: password sign-in, sign-up, refresh, sign-out.

use serde::Deserialize;
use serde_json::json;

use super::{AskError, Endpoints, HttpRequest, HttpResponse, Method, Session, Transport, unix_now};

#[derive(Deserialize)]
struct TokenBody {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    expires_at: Option<u64>,
    user: Option<User>,
}

#[derive(Deserialize)]
struct User {
    email: Option<String>,
}

#[derive(Deserialize, Default)]
struct ErrorBody {
    error_code: Option<String>,
    code: Option<serde_json::Value>,
    error: Option<String>,
    msg: Option<String>,
    message: Option<String>,
    error_description: Option<String>,
}

impl ErrorBody {
    fn parse(body: &str) -> Self {
        serde_json::from_str(body).unwrap_or_default()
    }

    fn code(&self) -> String {
        self.error_code
            .clone()
            .or_else(|| {
                self.code
                    .as_ref()
                    .and_then(|c| c.as_str().map(String::from))
            })
            .or_else(|| self.error.clone())
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        self.msg
            .clone()
            .or_else(|| self.message.clone())
            .or_else(|| self.error_description.clone())
            .unwrap_or_else(|| self.code())
    }
}

fn post(ep: &Endpoints, path: &str, bearer: Option<&str>, body: serde_json::Value) -> HttpRequest {
    let mut headers = vec![
        ("apikey".to_string(), ep.anon_key.clone()),
        ("content-type".to_string(), "application/json".to_string()),
    ];
    if let Some(token) = bearer {
        headers.push(("authorization".to_string(), format!("Bearer {token}")));
    }
    HttpRequest {
        method: Method::Post,
        url: format!("{}/auth/v1/{path}", ep.supabase_url),
        headers,
        body: Some(body.to_string()),
    }
}

fn send(t: &dyn Transport, request: &HttpRequest) -> Result<HttpResponse, AskError> {
    t.send(request).map_err(AskError::Network)
}

/// A token response into a session, or `None` when it has no tokens (which is
/// what a sign-up that needs email confirmation returns).
fn session_from(body: &str, fallback_email: &str) -> Option<Session> {
    let parsed: TokenBody = serde_json::from_str(body).ok()?;
    let expires_at = parsed
        .expires_at
        .or_else(|| parsed.expires_in.map(|s| unix_now() + s))?;
    Some(Session {
        access_token: parsed.access_token?,
        refresh_token: parsed.refresh_token?,
        expires_at,
        email: parsed
            .user
            .and_then(|u| u.email)
            .unwrap_or_else(|| fallback_email.to_string()),
    })
}

pub(crate) fn sign_in(
    t: &dyn Transport,
    ep: &Endpoints,
    email: &str,
    password: &str,
) -> Result<Session, AskError> {
    let request = post(
        ep,
        "token?grant_type=password",
        None,
        json!({ "email": email, "password": password }),
    );
    let response = send(t, &request)?;
    if response.status == 200 {
        return session_from(&response.body, email)
            .ok_or_else(|| AskError::Rejected("sign-in returned no session".into()));
    }
    let error = ErrorBody::parse(&response.body);
    match (response.status, error.code().as_str()) {
        (400 | 401, _) => Err(AskError::InvalidCredentials),
        _ => Err(AskError::Rejected(error.text())),
    }
}

pub(crate) fn sign_up(
    t: &dyn Transport,
    ep: &Endpoints,
    email: &str,
    password: &str,
) -> Result<Session, AskError> {
    let request = post(
        ep,
        "signup",
        None,
        json!({ "email": email, "password": password }),
    );
    let response = send(t, &request)?;
    if response.status == 200 {
        return session_from(&response.body, email).ok_or(AskError::ConfirmationRequired);
    }
    let error = ErrorBody::parse(&response.body);
    match error.code().as_str() {
        "user_already_exists" | "email_exists" => Err(AskError::EmailTaken),
        "weak_password" => Err(AskError::WeakPassword(error.text())),
        _ => Err(AskError::Rejected(error.text())),
    }
}

/// The refresh token is single use: the caller saves the new session before
/// using it.
pub(crate) fn refresh(
    t: &dyn Transport,
    ep: &Endpoints,
    refresh_token: &str,
    email: &str,
) -> Result<Session, AskError> {
    let request = post(
        ep,
        "token?grant_type=refresh_token",
        None,
        json!({ "refresh_token": refresh_token }),
    );
    let response = send(t, &request)?;
    match response.status {
        200 => session_from(&response.body, email).ok_or(AskError::SessionExpired),
        400..=499 => Err(AskError::SessionExpired),
        _ => Err(AskError::Network(format!(
            "sign-in service answered {}",
            response.status
        ))),
    }
}

/// Best effort: the local session is gone either way.
pub(crate) fn sign_out(t: &dyn Transport, ep: &Endpoints, session: &Session) {
    let request = post(ep, "logout", Some(&session.access_token), json!({}));
    let _ = t.send(&request);
}
