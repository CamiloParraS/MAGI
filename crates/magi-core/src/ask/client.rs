//! The authenticated calls to the Magi server. Built only from a [`Session`].

use serde::Deserialize;
use serde_json::json;

use super::{
    AskAnswer, AskError, AskSource, Endpoints, HttpRequest, HttpResponse, Method, Session,
    Transport,
};

pub struct AuthedClient<'a> {
    transport: &'a dyn Transport,
    endpoints: &'a Endpoints,
    session: &'a Session,
}

#[derive(Deserialize)]
struct LimitBody {
    scope: Option<String>,
    used: Option<u32>,
    limit: Option<u32>,
}

impl<'a> AuthedClient<'a> {
    /// The only constructor: a signed-out caller has no `Session` to pass.
    pub fn new(
        transport: &'a dyn Transport,
        endpoints: &'a Endpoints,
        session: &'a Session,
    ) -> Self {
        Self {
            transport,
            endpoints,
            session,
        }
    }

    fn request(&self, method: Method, path: &str, body: Option<String>) -> HttpRequest {
        HttpRequest {
            method,
            url: format!("{}{path}", self.endpoints.worker_url),
            headers: vec![
                (
                    "authorization".into(),
                    format!("Bearer {}", self.session.access_token),
                ),
                ("content-type".into(), "application/json".into()),
            ],
            body,
        }
    }

    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, AskError> {
        self.transport.send(request).map_err(AskError::Network)
    }

    pub fn ask(&self, question: &str, sources: &[AskSource]) -> Result<AskAnswer, AskError> {
        let body = json!({ "question": question, "sources": sources }).to_string();
        let response = self.send(&self.request(Method::Post, "/ask", Some(body)))?;
        match response.status {
            200 => {
                let mut answer: AskAnswer =
                    serde_json::from_str(&response.body).map_err(|_| AskError::Upstream)?;
                // The server drops unknown ids too; the client does not trust it.
                answer
                    .cited
                    .retain(|id| sources.iter().any(|s| &s.id == id));
                Ok(answer)
            }
            401 => Err(AskError::SessionExpired),
            413 => Err(AskError::TooLarge),
            429 => {
                let limit: LimitBody = serde_json::from_str(&response.body).unwrap_or(LimitBody {
                    scope: None,
                    used: None,
                    limit: None,
                });
                Err(AskError::DailyLimit {
                    used: limit.used.unwrap_or(0),
                    limit: limit.limit.unwrap_or(0),
                    everyone: limit.scope.as_deref() == Some("global"),
                })
            }
            400 => Err(AskError::Rejected("the question was malformed".into())),
            _ => Err(AskError::Upstream),
        }
    }

    pub fn delete_account(&self) -> Result<(), AskError> {
        let response = self.send(&self.request(Method::Delete, "/account", None))?;
        match response.status {
            200 | 204 => Ok(()),
            401 => Err(AskError::SessionExpired),
            _ => Err(AskError::Upstream),
        }
    }
}
