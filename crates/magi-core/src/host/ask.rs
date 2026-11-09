//! Ask Magi on the host (ADR-0014): the account state in the config and the
//! calls that reach the Magi server through `ask::AskService`.

use super::{Host, lock};
use crate::ask::{AskAnswer, AskError, AskSource};
use crate::config;
use crate::dto::AccountState;
use crate::error::Result;

impl Host {
    /// From the config only: no keychain read and no request.
    pub fn account_state(&self) -> Result<AccountState> {
        let ask = self.inner.load_config()?.ask;
        Ok(AccountState {
            signed_in_as: ask.signed_in_as,
            disclosed: ask.disclosed,
        })
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<AccountState> {
        let email = self.inner.ask.sign_in(email, password)?;
        self.update_ask_config(|ask| ask.signed_in_as = Some(email))
    }

    pub fn sign_up(&self, email: &str, password: &str) -> Result<AccountState> {
        let email = self.inner.ask.sign_up(email, password)?;
        self.update_ask_config(|ask| ask.signed_in_as = Some(email))
    }

    pub fn sign_out(&self) -> Result<AccountState> {
        self.inner.ask.sign_out();
        self.update_ask_config(|ask| ask.signed_in_as = None)
    }

    /// Sends the question and the sources' text to the Magi server. A session
    /// that is gone also clears "signed in as", so the window returns to sign-in.
    pub fn ask(&self, question: &str, sources: &[AskSource]) -> Result<AskAnswer> {
        let result = self.inner.ask.ask(question, sources);
        self.forget_if_signed_out(&result)?;
        Ok(result?)
    }

    /// Deletes the account on the server, then signs out here.
    pub fn delete_account(&self) -> Result<AccountState> {
        let result = self.inner.ask.delete_account();
        self.forget_if_signed_out(&result)?;
        result?;
        self.update_ask_config(|ask| ask.signed_in_as = None)
    }

    /// Records that the first-use disclosure was accepted.
    pub fn accept_ask_disclosure(&self) -> Result<AccountState> {
        self.update_ask_config(|ask| ask.disclosed = true)
    }

    fn forget_if_signed_out<T>(&self, result: &std::result::Result<T, AskError>) -> Result<()> {
        if matches!(result, Err(AskError::SessionExpired | AskError::SignedOut)) {
            self.update_ask_config(|ask| ask.signed_in_as = None)?;
        }
        Ok(())
    }

    fn update_ask_config(
        &self,
        change: impl FnOnce(&mut config::AskConfig),
    ) -> Result<AccountState> {
        let _config = lock(&self.inner.config);
        let mut current = self.inner.load_config()?;
        change(&mut current.ask);
        config::save_to(&self.inner.paths.config, &current)?;
        Ok(AccountState {
            signed_in_as: current.ask.signed_in_as,
            disclosed: current.ask.disclosed,
        })
    }
}
