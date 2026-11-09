//! The Ask Magi window (ADR-0014): what the search window hands over, and
//! the states the window moves through. Plain functions and data, so
//! `cargo test` covers it; [`view`] wires it to GPUI and `Host`.

pub mod view;

use magi_core::Error;
use magi_core::ask::{AskAnswer, AskError, AskFile};
use magi_core::discovery::Kind;
use magi_core::dto::{AccountState, SearchResult};

use crate::i18n::Strings;
use crate::search::detail::snippet_is_the_path;

/// The server's limits. It counts as JavaScript does, in UTF-16 units.
pub const MAX_SOURCES: usize = 5;
pub const MAX_QUESTION_LEN: usize = 500;
pub const MAX_SOURCE_LEN: usize = 1000;
pub const MAX_FILE_NAME_LEN: usize = 255;

pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text` cut to at most `max` UTF-16 units, never inside a character.
pub fn clip(text: &str, max: usize) -> String {
    let mut used = 0;
    let end = text
        .char_indices()
        .find(|(_, c)| {
            used += c.len_utf16();
            used > max
        })
        .map_or(text.len(), |(at, _)| at);
    text[..end].to_owned()
}

/// A result's excerpt, as it would be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub file_id: i64,
    pub file_name: String,
    pub kind: Kind,
    pub text: String,
    pub ticked: bool,
}

/// What the search window hands over: the window keeps its own copy, because
/// the search window closes as soon as this one takes the focus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Request {
    pub question: String,
    pub sources: Vec<Source>,
}

/// The excerpt worth sending, if any. A snippet that only repeats the file's
/// path is left out with the photos: it holds nothing from inside the file,
/// and sending it would send the folder names (ADR-0014: never full paths).
fn excerpt(result: &SearchResult) -> Option<&str> {
    let text = result.snippet.as_ref()?.text.trim();
    (!text.is_empty() && !snippet_is_the_path(text, &result.path)).then_some(text)
}

/// A copy of the top results that have text, ticked, with the search text as
/// the question.
pub fn request(query: &str, results: &[SearchResult]) -> Request {
    let sources = results
        .iter()
        .filter_map(|result| {
            excerpt(result).map(|text| Source {
                file_id: result.file_id,
                file_name: clip(&result.file_name, MAX_FILE_NAME_LEN),
                kind: result.kind,
                text: clip(text, MAX_SOURCE_LEN),
                ticked: true,
            })
        })
        .take(MAX_SOURCES)
        .collect();
    Request {
        question: clip(query.trim(), MAX_QUESTION_LEN),
        sources,
    }
}

/// There is something to ask about. With only photos on screen the window
/// still opens, and says there is no text to send.
pub fn can_ask(results: &[SearchResult]) -> bool {
    !results.is_empty()
}

pub fn credentials_ok(email: &str, password: &str) -> bool {
    email.trim().contains('@') && !password.is_empty()
}

/// Why something failed, as far as the person can act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    WrongLogin,
    EmailTaken,
    WeakPassword,
    NeedsConfirmation,
    Network,
    Service,
    TooLarge,
    /// The session ended; signing in again brings the chat back.
    SessionEnded,
}

pub fn problem_of(error: &AskError) -> Problem {
    match error {
        AskError::InvalidCredentials => Problem::WrongLogin,
        AskError::EmailTaken => Problem::EmailTaken,
        AskError::WeakPassword(_) => Problem::WeakPassword,
        AskError::ConfirmationRequired => Problem::NeedsConfirmation,
        AskError::TooLarge => Problem::TooLarge,
        AskError::Network(_) => Problem::Network,
        AskError::SignedOut | AskError::SessionExpired => Problem::SessionEnded,
        AskError::NotConfigured
        | AskError::DailyLimit { .. }
        | AskError::Upstream
        | AskError::Rejected(_) => Problem::Service,
    }
}

pub fn problem_text(problem: Problem, s: &Strings) -> &'static str {
    match problem {
        Problem::WrongLogin => s.ask_err_wrong_login,
        Problem::EmailTaken => s.ask_err_email_taken,
        Problem::WeakPassword => s.ask_err_weak_password,
        Problem::NeedsConfirmation => s.ask_err_confirmation,
        Problem::Network => s.ask_err_network,
        Problem::Service => s.ask_err_service,
        Problem::TooLarge => s.ask_err_too_large,
        Problem::SessionEnded => s.ask_session_ended,
    }
}

/// A `Host` failure as the Ask error it carries.
pub fn ask_error(error: Error) -> AskError {
    match error {
        Error::Ask(error) => error,
        other => {
            tracing::warn!(error = %other, "an Ask Magi call failed outside the client");
            AskError::Upstream
        }
    }
}

/// Why Send is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    NoQuestion,
    TooLong,
    NoSources,
    /// An answer is on its way.
    Busy,
    /// Today's questions are used up.
    Limited,
}

/// What to say beside a Send that is off, when nothing else on screen says
/// it already: an empty box has its placeholder, asking has its spinner,
/// the limit its message, and no sources at all their own line.
pub fn blocked_text(blocked: Blocked, has_sources: bool, s: &Strings) -> Option<&'static str> {
    match blocked {
        Blocked::TooLong if has_sources => Some(s.ask_too_long),
        Blocked::NoSources if has_sources => Some(s.ask_tick_one),
        _ => None,
    }
}

/// The note under an answer: it points at the cited files only when there
/// are some.
pub fn ai_note(answer: &Answer, s: &Strings) -> &'static str {
    match answer.cited.is_empty() {
        true => s.ask_written_by_ai_uncited,
        false => s.ask_written_by_ai,
    }
}

/// An answer, numbered against what its turn sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    /// Indexes into the turn's `sent`, in order; number `n` on screen is
    /// `sent[n - 1]`.
    pub cited: Vec<usize>,
    pub used: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnState {
    Asking,
    Answered(Answer),
    Failed(Problem),
    /// The daily limit: this account's, or with `everyone` the server's.
    Limit {
        limit: u32,
        everyone: bool,
    },
}

/// One question in the chat, exactly what went with it, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub question: String,
    pub sent: Vec<Source>,
    pub state: TurnState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Signed out; with `expired` because the session ended mid-chat.
    SignIn {
        expired: bool,
    },
    Chat,
}

/// A question on its way. Only `generation`'s reply will be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub generation: u64,
    pub question: String,
    /// The ticked files, in chip order. What is sent for each is chosen for
    /// this question by `Host::ask_about`, so the numbers match the chips.
    pub files: Vec<AskFile>,
    /// The first question: save that the disclosure above the box was seen.
    pub accept_disclosure: bool,
}

#[derive(Debug)]
pub struct AskModel {
    account: AccountState,
    request: Request,
    turns: Vec<Turn>,
    /// The session ended under a question, which is why sign-in is back.
    expired: bool,
    /// A new search or a sign-out makes the reply in flight stale.
    generation: u64,
}

impl AskModel {
    pub fn new(account: AccountState, request: Request) -> Self {
        Self {
            account,
            request,
            turns: Vec::new(),
            expired: false,
            generation: 0,
        }
    }

    pub fn phase(&self) -> Phase {
        match self.account.signed_in_as {
            None => Phase::SignIn {
                expired: self.expired,
            },
            Some(_) => Phase::Chat,
        }
    }

    pub fn account(&self) -> &AccountState {
        &self.account
    }

    /// The first-use disclosure still has to be shown.
    pub fn needs_disclosure(&self) -> bool {
        !self.account.disclosed
    }

    /// The search text the chat opened with.
    pub fn question(&self) -> &str {
        &self.request.question
    }

    /// The chips: what the next question would send.
    pub fn sources(&self) -> &[Source] {
        &self.request.sources
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    /// The last question is on its way.
    pub fn is_asking(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| turn.state == TurnState::Asking)
    }

    /// Today's count, from the latest answer.
    pub fn used(&self) -> Option<(u32, u32)> {
        self.turns.iter().rev().find_map(|turn| match &turn.state {
            TurnState::Answered(answer) => Some((answer.used, answer.limit)),
            _ => None,
        })
    }

    /// Asking from a new search: a new chat about its results.
    pub fn replace(&mut self, request: Request) {
        self.request = request;
        self.turns.clear();
        self.generation += 1;
    }

    /// Ticks or unticks a chip, except while its question is on its way.
    pub fn toggle(&mut self, ix: usize) {
        if self.is_asking() {
            return;
        }
        if let Some(source) = self.request.sources.get_mut(ix) {
            source.ticked = !source.ticked;
        }
    }

    pub fn accepted_disclosure(&mut self, account: AccountState) {
        self.account = account;
    }

    /// Who is signed in now, wherever that changed (the sign-in form here
    /// or in Settings). Signing out drops the question on its way.
    pub fn sync_account(&mut self, account: AccountState) {
        let signed_out = self.account.signed_in_as.is_some() && account.signed_in_as.is_none();
        if account.signed_in_as.is_some() {
            self.expired = false;
        }
        self.account = account;
        if signed_out {
            self.generation += 1;
            if self.is_asking() {
                self.turns.pop();
            }
        }
    }

    pub fn blocked(&self, question: &str) -> Option<Blocked> {
        let question = question.trim();
        if self.is_asking() {
            Some(Blocked::Busy)
        } else if self
            .turns
            .last()
            .is_some_and(|turn| matches!(turn.state, TurnState::Limit { .. }))
        {
            Some(Blocked::Limited)
        } else if question.is_empty() {
            Some(Blocked::NoQuestion)
        } else if utf16_len(question) > MAX_QUESTION_LEN {
            Some(Blocked::TooLong)
        } else if !self.request.sources.iter().any(|s| s.ticked) {
            Some(Blocked::NoSources)
        } else {
            None
        }
    }

    fn outgoing(&mut self) -> Option<Outgoing> {
        let turn = self.turns.last()?;
        self.generation += 1;
        // The snippet is only the fallback for a file the index lost.
        let files = turn
            .sent
            .iter()
            .map(|s| AskFile {
                file_id: s.file_id,
                file_name: s.file_name.clone(),
                fallback: s.text.clone(),
            })
            .collect();
        let accept_disclosure = !self.account.disclosed;
        self.account.disclosed = true;
        Some(Outgoing {
            generation: self.generation,
            question: turn.question.clone(),
            files,
            accept_disclosure,
        })
    }

    /// Asks `question` about the ticked chips, as a new turn.
    pub fn start(&mut self, question: &str) -> Option<Outgoing> {
        if self.phase() != Phase::Chat || self.blocked(question).is_some() {
            return None;
        }
        self.turns.push(Turn {
            question: question.trim().to_owned(),
            sent: self
                .request
                .sources
                .iter()
                .filter(|s| s.ticked)
                .cloned()
                .collect(),
            state: TurnState::Asking,
        });
        self.outgoing()
    }

    /// Sends the last question again, as it went, when it failed.
    pub fn retry(&mut self) -> Option<Outgoing> {
        if self.phase() != Phase::Chat {
            return None;
        }
        let turn = self.turns.last_mut()?;
        if !matches!(turn.state, TurnState::Failed(_)) {
            return None;
        }
        turn.state = TurnState::Asking;
        self.outgoing()
    }

    /// Shows how a question ended, unless a newer search has replaced it.
    pub fn finished(&mut self, generation: u64, result: Result<AskAnswer, AskError>) {
        if generation != self.generation || !self.is_asking() {
            return;
        }
        let state = match result {
            Ok(answer) => {
                let sent = self.turns.last().map_or(0, |turn| turn.sent.len());
                TurnState::Answered(Answer {
                    cited: cited_indexes(&answer.cited, sent),
                    text: answer.answer,
                    used: answer.used,
                    limit: answer.limit,
                })
            }
            Err(AskError::DailyLimit {
                limit, everyone, ..
            }) => TurnState::Limit { limit, everyone },
            Err(AskError::SessionExpired | AskError::SignedOut) => {
                self.account.signed_in_as = None;
                self.expired = true;
                TurnState::Failed(Problem::SessionEnded)
            }
            Err(error) => TurnState::Failed(problem_of(&error)),
        };
        if let Some(turn) = self.turns.last_mut() {
            turn.state = state;
        }
    }
}

/// The cited numbers as indexes into what was sent, each once and in order.
/// Anything else the model returned is dropped.
fn cited_indexes(cited: &[String], sent: usize) -> Vec<usize> {
    let mut indexes: Vec<usize> = cited
        .iter()
        .filter_map(|id| id.parse::<usize>().ok())
        .filter(|&n| (1..=sent).contains(&n))
        .map(|n| n - 1)
        .collect();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

#[cfg(test)]
mod tests;
