//! Where the refresh token lives between runs: the OS keychain
//! (`platform::Keychain`), or memory only. Never a plaintext file.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

/// What is saved. Small on purpose: the access token stays in memory, and
/// Windows caps a credential at 2.5 KB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLogin {
    pub refresh_token: String,
    pub email: String,
}

/// `Err` carries a reason for the log only; callers fall back to memory.
pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredLogin>, String>;
    fn save(&self, login: &StoredLogin) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}

/// A store for tests and `MAGI_FAKE_ASK`: dev instances share the real OS
/// keychain, so they must not touch it.
#[derive(Default)]
pub struct MemoryStore {
    value: Mutex<Option<StoredLogin>>,
    loads: AtomicUsize,
    saves: AtomicUsize,
    failing: AtomicBool,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every operation fails, like a denied keychain prompt.
    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    pub fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    pub fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }

    pub fn peek(&self) -> Option<StoredLogin> {
        self.value.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn check(&self) -> Result<(), String> {
        if self.failing.load(Ordering::SeqCst) {
            Err("keychain unavailable".into())
        } else {
            Ok(())
        }
    }
}

impl CredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<StoredLogin>, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.check()?;
        Ok(self.peek())
    }

    fn save(&self, login: &StoredLogin) -> Result<(), String> {
        self.check()?;
        self.saves.fetch_add(1, Ordering::SeqCst);
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some(login.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), String> {
        self.check()?;
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }
}

impl<T: CredentialStore + ?Sized> CredentialStore for std::sync::Arc<T> {
    fn load(&self) -> Result<Option<StoredLogin>, String> {
        (**self).load()
    }
    fn save(&self, login: &StoredLogin) -> Result<(), String> {
        (**self).save(login)
    }
    fn clear(&self) -> Result<(), String> {
        (**self).clear()
    }
}
