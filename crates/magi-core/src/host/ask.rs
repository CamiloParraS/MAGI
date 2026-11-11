//! Ask Magi on the host (ADR-0014): the account state in the config and the
//! calls that reach the Magi server through `ask::AskService`.

use std::path::Path;

use rusqlite::{OptionalExtension, params};

use super::{Host, lock};
use crate::ask::{AskAnswer, AskError, AskFile, AskSource, context};
use crate::config;
use crate::discovery::Kind;
use crate::dto::{AccountState, AskFileInfo, WholeSize};
use crate::error::Result;

/// How much of a dropped file's text its chip keeps for the tooltip.
const PREVIEW_CHARS: usize = 1000;

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

    /// Like [`Host::ask`], but the text sent for each ticked file is the part
    /// of it that best matches this question (`ask::context`), not the
    /// snippet the search showed. Keywords alone while the engine restarts.
    pub fn ask_about(&self, question: &str, files: &[AskFile]) -> Result<AskAnswer> {
        let components = self
            .inner
            .running()
            .map(|r| r.components)
            .unwrap_or_default();
        // The model call comes before the reader lock, as in `search`.
        let query = components
            .text
            .as_deref()
            .and_then(|embedder| embedder.embed_query(question).ok());
        let payload =
            context::payload_for(&lock(&self.inner.reader), query.as_deref(), question, files)?;
        let result = self.inner.ask.ask_with(question, &payload);
        self.forget_if_signed_out(&result)?;
        Ok(result?)
    }

    /// What "whole file" would send for `file_id`, from the index alone.
    pub fn ask_whole_size(&self, file_id: i64) -> Result<WholeSize> {
        let (chars, cut) = context::whole_size(&lock(&self.inner.reader), file_id)?;
        Ok(WholeSize { chars, cut })
    }

    /// The searchable indexed file at `path`, for a file dropped on the Ask
    /// window. `None` for a file outside the roots, not indexed, or in a
    /// paused root: Ask Magi only sends what search would show.
    pub fn ask_file_at(&self, path: &Path) -> Result<Option<AskFileInfo>> {
        let Ok(path) = crate::paths::canonicalize(path) else {
            return Ok(None);
        };
        let conn = lock(&self.inner.reader);
        let found = conn
            .query_row(
                concat!(
                    "SELECT f.id, f.file_name, f.kind, f.thumb_key FROM files f
                     JOIN roots r ON r.id = f.root_id
                     WHERE f.path = ?1 AND ",
                    crate::db::roots::searchable_sql!()
                ),
                params![path.to_string_lossy()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Kind>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((file_id, file_name, kind, thumb_key)) = found else {
            return Ok(None);
        };
        let preview = preview(&conn, file_id)?;
        Ok(Some(AskFileInfo {
            file_id,
            file_name,
            kind,
            thumb_path: thumb_key.map(|key| {
                crate::thumbs::thumb_path(&key)
                    .to_string_lossy()
                    .into_owned()
            }),
            preview,
        }))
    }

    /// The start of a searchable file's indexed text, for its chip: what a
    /// search result found by its name alone has no snippet for. Empty when
    /// the file has no text or search would not show it.
    pub fn ask_preview(&self, file_id: i64) -> Result<String> {
        preview(&lock(&self.inner.reader), file_id)
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

fn preview(conn: &rusqlite::Connection, file_id: i64) -> Result<String> {
    Ok(context::first_text(conn, file_id)?
        .map(|text| text.trim().chars().take(PREVIEW_CHARS).collect())
        .unwrap_or_default())
}
