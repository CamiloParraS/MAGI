//! The one place Ask makes an HTTP request. A trait so tests and CI run
//! against an in-process server (`MAGI_FAKE_ASK=1`, [`super::FakeBackend`]).

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait Transport: Send + Sync {
    /// An HTTP error status is a response, not an `Err`; `Err` is a failure to
    /// get one (offline, timeout), with a reason fit to show.
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(40)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for UreqTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let method = match request.method {
            Method::Get => ureq::http::Method::GET,
            Method::Post => ureq::http::Method::POST,
            Method::Delete => ureq::http::Method::DELETE,
        };
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let http = builder
            .body(request.body.clone().unwrap_or_default())
            .map_err(|e| e.to_string())?;
        let response = self.agent.run(http).map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok(HttpResponse { status, body })
    }
}
