//! The OS keychain behind Ask Magi's sign-in: Credential Manager on Windows,
//! Keychain on macOS, the Secret Service (over `zbus`, no libdbus) on Linux.
//! Nothing here runs until Ask reads or writes the stored login.

use std::sync::OnceLock;

use keyring_core::{Entry, Error as KeyringError};

use crate::ask::{CredentialStore, StoredLogin};

const SERVICE: &str = "magi";
const ACCOUNT: &str = "ask-session";

/// Installs this OS's store once. The failure is kept, so a missing Secret
/// Service daemon reads as "no keychain" on every call.
fn install() -> &'static Result<(), String> {
    static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        #[cfg(target_os = "windows")]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(target_os = "linux")]
        let store = zbus_secret_service_keyring_store::Store::new();
        store
            .map(|store| keyring_core::set_default_store(store))
            .map_err(|e| e.to_string())
    })
}

pub struct Keychain;

impl Keychain {
    pub fn new() -> Self {
        Self
    }

    fn entry() -> Result<Entry, String> {
        install().clone()?;
        Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
    }
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for Keychain {
    fn load(&self) -> Result<Option<StoredLogin>, String> {
        match Self::entry()?.get_password() {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, login: &StoredLogin) -> Result<(), String> {
        let raw = serde_json::to_string(login).map_err(|e| e.to_string())?;
        Self::entry()?.set_password(&raw).map_err(|e| e.to_string())
    }

    fn clear(&self) -> Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
