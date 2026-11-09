//! Ask Magi client (ADR-0014). All Ask networking lives in this module: sign-in
//! and refresh against Supabase, and the question sent to the Magi server.
//!
//! Signed out, nothing here runs. The authenticated client can only be built
//! from a [`Session`], and a session only comes out of a successful sign-in or
//! refresh, so signed-out code cannot reach the network. The keychain is read
//! only when a question is asked, never at launch.

mod client;
pub mod context;
mod endpoints;
mod fake;
mod service;
mod store;
mod supabase;
mod transport;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use client::AuthedClient;
pub use endpoints::Endpoints;
pub use fake::FakeBackend;
pub use service::AskService;
pub use store::{CredentialStore, MemoryStore, StoredLogin};
pub use transport::{HttpRequest, HttpResponse, Method, Transport, UreqTransport};

/// One result's text, as sent. `id` comes back in `AskAnswer::cited`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskSource {
    pub id: String,
    pub file_name: String,
    pub text: String,
}

/// A file the person ticked. The text sent for it is chosen per question
/// ([`context`]); `fallback` is what goes when the index has no text for it
/// any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskFile {
    pub file_id: i64,
    pub file_name: String,
    pub fallback: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub answer: String,
    /// Only ids that were sent.
    pub cited: Vec<String>,
    pub used: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AskError {
    #[error("Ask Magi is not configured in this build")]
    NotConfigured,
    #[error("not signed in")]
    SignedOut,
    #[error("wrong email or password")]
    InvalidCredentials,
    #[error("that email already has an account")]
    EmailTaken,
    #[error("password too weak: {0}")]
    WeakPassword(String),
    #[error("the account needs email confirmation, which this build does not support")]
    ConfirmationRequired,
    #[error("the session expired; sign in again")]
    SessionExpired,
    #[error("daily limit reached ({used} of {limit})")]
    DailyLimit {
        used: u32,
        limit: u32,
        /// The limit shared by every account, not this one.
        everyone: bool,
    },
    #[error("the question or its sources are too large")]
    TooLarge,
    #[error("the answer service failed; try again")]
    Upstream,
    #[error("network error: {0}")]
    Network(String),
    #[error("the server refused the request: {0}")]
    Rejected(String),
}

/// A signed-in session. Built only by sign-in, sign-up and refresh.
#[derive(Clone)]
pub struct Session {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    /// Unix seconds.
    pub(crate) expires_at: u64,
    pub(crate) email: String,
}

impl Session {
    pub fn email(&self) -> &str {
        &self.email
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("email", &self.email)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
