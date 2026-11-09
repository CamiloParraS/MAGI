//! Sign-in state and the refresh rules behind `Host`'s Ask methods.
//!
//! The access token lives in memory. The refresh token is saved to the store
//! (the OS keychain) and is single use, so a refresh saves the new one before
//! anything uses it, behind one lock. The store is read only when a question
//! is asked and nothing is in memory yet.

use std::sync::{Arc, Mutex, MutexGuard};

use super::supabase;
use super::{
    AskAnswer, AskError, AskSource, AuthedClient, CredentialStore, Endpoints, FakeBackend,
    MemoryStore, Session, StoredLogin, Transport, UreqTransport, unix_now,
};

/// A session this close to expiry is refreshed first.
const SKEW_SECS: u64 = 30;

#[derive(Clone)]
pub struct AskService {
    inner: Arc<Inner>,
}

struct Inner {
    transport: Box<dyn Transport>,
    store: Box<dyn CredentialStore>,
    endpoints: Result<Endpoints, AskError>,
    memory: Mutex<Option<Session>>,
    /// Held across a whole refresh, so a second question waits and then finds
    /// the new session instead of spending the old refresh token again.
    refresh: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AskService {
    pub fn new(
        transport: Box<dyn Transport>,
        store: Box<dyn CredentialStore>,
        endpoints: Result<Endpoints, AskError>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                transport,
                store,
                endpoints,
                memory: Mutex::new(None),
                refresh: Mutex::new(()),
            }),
        }
    }

    /// The real service, or the in-process fake when `MAGI_FAKE_ASK=1`.
    /// Builds nothing that connects or reads the keychain.
    pub fn from_env() -> Self {
        if std::env::var("MAGI_FAKE_ASK").as_deref() == Ok("1") {
            return Self::new(
                Box::new(FakeBackend::new()),
                Box::new(MemoryStore::new()),
                Ok(FakeBackend::endpoints()),
            );
        }
        Self::new(
            Box::new(UreqTransport::new()),
            Box::new(crate::platform::Keychain::new()),
            Endpoints::compiled(),
        )
    }

    fn endpoints(&self) -> Result<&Endpoints, AskError> {
        self.inner.endpoints.as_ref().map_err(Clone::clone)
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<String, AskError> {
        let ep = self.endpoints()?;
        let session = supabase::sign_in(&*self.inner.transport, ep, email.trim(), password)?;
        Ok(self.adopt(session))
    }

    pub fn sign_up(&self, email: &str, password: &str) -> Result<String, AskError> {
        let ep = self.endpoints()?;
        let session = supabase::sign_up(&*self.inner.transport, ep, email.trim(), password)?;
        Ok(self.adopt(session))
    }

    /// Always ends signed out locally. The server is told only when this run
    /// holds a session; the keychain is cleared, not read.
    pub fn sign_out(&self) {
        let session = lock(&self.inner.memory).take();
        if let (Some(session), Ok(ep)) = (session, self.endpoints()) {
            supabase::sign_out(&*self.inner.transport, ep, &session);
        }
        self.forget_stored();
    }

    pub fn ask(&self, question: &str, sources: &[AskSource]) -> Result<AskAnswer, AskError> {
        let session = self.fresh_session()?;
        let client = AuthedClient::new(&*self.inner.transport, self.endpoints()?, &session);
        let result = client.ask(question, sources);
        self.drop_if_expired(&result);
        result
    }

    /// The server deletes the account and its usage row; then this device
    /// forgets the session too.
    pub fn delete_account(&self) -> Result<(), AskError> {
        let session = self.fresh_session()?;
        let client = AuthedClient::new(&*self.inner.transport, self.endpoints()?, &session);
        let result = client.delete_account();
        match &result {
            Ok(()) => {
                *lock(&self.inner.memory) = None;
                self.forget_stored();
            }
            Err(_) => self.drop_if_expired(&result),
        }
        result
    }

    /// Keeps a new session in memory and saves its refresh token. A keychain
    /// that refuses is not an error: the session lasts until the app closes.
    fn adopt(&self, session: Session) -> String {
        let email = session.email.clone();
        self.persist(&session);
        *lock(&self.inner.memory) = Some(session);
        email
    }

    fn persist(&self, session: &Session) {
        let login = StoredLogin {
            refresh_token: session.refresh_token.clone(),
            email: session.email.clone(),
        };
        if let Err(reason) = self.inner.store.save(&login) {
            tracing::warn!(%reason, "keychain unavailable; staying signed in for this run only");
        }
    }

    fn forget_stored(&self) {
        if let Err(reason) = self.inner.store.clear() {
            tracing::warn!(%reason, "could not clear the keychain entry");
        }
    }

    fn drop_if_expired<T>(&self, result: &Result<T, AskError>) {
        if matches!(result, Err(AskError::SessionExpired)) {
            *lock(&self.inner.memory) = None;
            self.forget_stored();
        }
    }

    /// A usable session, refreshing when needed.
    fn fresh_session(&self) -> Result<Session, AskError> {
        let ep = self.endpoints()?;
        let _one_refresh_at_a_time = lock(&self.inner.refresh);
        let held = lock(&self.inner.memory).clone();
        if let Some(session) = &held
            && session.expires_at > unix_now() + SKEW_SECS
        {
            return Ok(session.clone());
        }
        let (refresh_token, email) = match held {
            Some(s) => (s.refresh_token, s.email),
            None => match self.inner.store.load() {
                Ok(Some(login)) => (login.refresh_token, login.email),
                Ok(None) => return Err(AskError::SignedOut),
                Err(reason) => {
                    tracing::warn!(%reason, "keychain unavailable; treating as signed out");
                    return Err(AskError::SignedOut);
                }
            },
        };
        match supabase::refresh(&*self.inner.transport, ep, &refresh_token, &email) {
            Ok(session) => {
                // Single-use token: save the new one before it is used.
                self.persist(&session);
                *lock(&self.inner.memory) = Some(session.clone());
                Ok(session)
            }
            Err(AskError::SessionExpired) => {
                *lock(&self.inner.memory) = None;
                self.forget_stored();
                Err(AskError::SessionExpired)
            }
            Err(other) => Err(other),
        }
    }
}
