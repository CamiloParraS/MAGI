//! What the search window shows, independent of GPUI: the current query,
//! which reply belongs to it, and the selection.

use magi_core::dto::{
    ErrorCode, IndexState, IndexStatus, MatchSource, SearchResponse, SearchResult,
};
use magi_core::search::strength::Strength;

use crate::i18n::Strings;

/// One plain sentence for why a search failed, when the error says. Anything
/// else gets none: the generic line already says it.
pub fn cause(error: &ErrorCode, s: &Strings) -> Option<&'static str> {
    match error {
        ErrorCode::EngineStarting => Some(s.search_cause_starting),
        ErrorCode::Internal { .. } => Some(s.search_cause_internal),
        _ => None,
    }
}

/// What each match source means, "Meaning: Says something close to what you
/// typed", joined by `sep`: the tooltip's lines and the footer's line.
pub fn explain_sources(sources: &[MatchSource], s: &Strings, sep: &str) -> String {
    sources
        .iter()
        .map(|&m| format!("{}: {}", s.source(m), s.source_tip(m)))
        .collect::<Vec<_>>()
        .join(sep)
}

/// The sources a row names. "Related" is the least specific, nearly every
/// row has it, so it is named only when it is the sole reason; beside Words,
/// Looks like and the rest it only repeats them.
pub fn shown_sources(sources: &[MatchSource]) -> Vec<MatchSource> {
    sources
        .iter()
        .copied()
        .filter(|m| *m != MatchSource::Semantic || sources.len() == 1)
        .collect()
}

/// Why a file matched when the row does not show it, for the footer: the most
/// specific source whose reason is not visible (no highlighted word, no file
/// name match). "Related" is the least specific, nearly every row has it, so
/// it speaks only when nothing else does. `None` for a match the row already
/// explains.
pub fn hidden_reason(sources: &[MatchSource], s: &Strings) -> Option<&'static str> {
    let sources = shown_sources(sources);
    [
        MatchSource::Ocr,
        MatchSource::Qr,
        MatchSource::Visual,
        MatchSource::Semantic,
    ]
    .into_iter()
    .find(|m| sources.contains(m))
    .map(|m| s.source_tip(m))
}

/// The status the footer shows: a paused index with nothing waiting to be
/// indexed has nothing to be paused about (results are complete), so it reads
/// as idle. The tray and Settings still say "paused".
pub fn footer_status(status: &IndexStatus) -> IndexStatus {
    let mut shown = status.clone();
    if shown.state == IndexState::Paused && shown.queued == 0 {
        shown.state = IndexState::Idle;
        shown.paused_for = None;
    }
    shown
}

/// A search to run after the debounce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub generation: u64,
    pub text: String,
}

#[derive(Default)]
pub struct SearchState {
    query: String,
    generation: u64,
    results: Vec<SearchResult>,
    /// The weak tail of `results` is showing.
    weak_open: bool,
    selected: usize,
    error: Option<ErrorCode>,
    /// Searches that failed in a row; one reply that works clears it.
    failures: u32,
    loading: bool,
}

impl SearchState {
    /// The search to run after the debounce, or `None` when the trimmed text
    /// is unchanged or empty. Any new text, empty included, makes the
    /// replies still in flight stale; empty also clears the list.
    pub fn set_query(&mut self, text: &str) -> Option<Query> {
        let text = text.trim();
        if text == self.query {
            return None;
        }
        self.query = text.to_owned();
        self.generation += 1;
        if text.is_empty() {
            self.results.clear();
            self.weak_open = false;
            self.error = None;
            self.selected = 0;
            self.loading = false;
            return None;
        }
        // The old message is about the old text.
        self.error = None;
        self.loading = true;
        Some(Query {
            generation: self.generation,
            text: self.query.clone(),
        })
    }

    /// Runs the current query again (after an error), dropping any reply
    /// still in flight.
    pub fn retry(&mut self) -> Option<Query> {
        if self.query.is_empty() {
            return None;
        }
        self.generation += 1;
        self.error = None;
        self.loading = true;
        Some(Query {
            generation: self.generation,
            text: self.query.clone(),
        })
    }

    /// Shows a reply, unless a newer query has started since it was sent.
    pub fn apply(&mut self, generation: u64, reply: Result<SearchResponse, ErrorCode>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match reply {
            Ok(response) => {
                self.results = response.results;
                self.error = None;
                self.failures = 0;
            }
            Err(error) => {
                self.results.clear();
                self.error = Some(error);
                self.failures += 1;
            }
        }
        self.weak_open = false;
        self.selected = 0;
    }

    /// Moves the selection, stopping at the first and last result shown.
    /// Going down past the last strong result opens the fold, so the
    /// keyboard reaches everything the mouse can.
    pub fn select(&mut self, delta: isize) {
        let next = self.selected.saturating_add_signed(delta);
        if next >= self.strong_count() && delta > 0 && self.weak_count() > 0 {
            self.weak_open = true;
        }
        self.selected = next.min(self.shown().len().saturating_sub(1));
    }

    /// Selects the result at `ix`, clamped to the last one shown.
    pub fn select_at(&mut self, ix: usize) {
        self.selected = ix.min(self.shown().len().saturating_sub(1));
    }

    /// Opens or closes the fold; closing takes the selection out of it.
    pub fn toggle_weak(&mut self) {
        self.weak_open = !self.weak_open;
        self.selected = self.selected.min(self.shown().len().saturating_sub(1));
    }

    pub fn selected_index(&self) -> Option<usize> {
        (!self.shown().is_empty()).then_some(self.selected)
    }

    pub fn selected(&self) -> Option<&SearchResult> {
        self.shown().get(self.selected)
    }

    /// Every result of the reply, shown or folded.
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// What Ask Magi may ask about: the results on screen for the text in
    /// the box. Folded weak results ("Nothing close") and a list left over
    /// from older text while a new search runs are not.
    pub fn askable(&self) -> &[SearchResult] {
        match self.loading {
            true => &[],
            false => self.shown(),
        }
    }

    /// The results on screen: the strong ones, then the weak ones when open.
    pub fn shown(&self) -> &[SearchResult] {
        let end = match self.weak_open {
            true => self.results.len(),
            false => self.strong_count(),
        };
        &self.results[..end]
    }

    /// The results ahead of the fold (the core sorts weak ones last).
    pub fn strong_count(&self) -> usize {
        self.results
            .iter()
            .take_while(|r| r.strength == Strength::Strong)
            .count()
    }

    pub fn weak_count(&self) -> usize {
        self.results.len() - self.strong_count()
    }

    pub fn weak_open(&self) -> bool {
        self.weak_open
    }

    /// Result `ix`'s position among the list's children: the fold's own row
    /// sits between the strong and the weak results.
    pub fn list_item(&self, ix: usize) -> usize {
        ix + usize::from(ix >= self.strong_count())
    }

    /// Searches that failed in a row.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// The first failure is tried again without being asked (the engine is
    /// often just restarting); a second one is the user's to act on.
    pub fn should_auto_retry(&self) -> bool {
        self.error.is_some() && self.failures == 1
    }

    pub fn error(&self) -> Option<&ErrorCode> {
        self.error.as_ref()
    }

    /// The trimmed query the results belong to.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// A search for the current query is pending or running.
    pub fn is_loading(&self) -> bool {
        self.loading
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_core::discovery::Kind;

    fn hit(file_id: i64) -> SearchResult {
        SearchResult {
            file_id,
            path: format!("C:\\docs\\{file_id}.txt"),
            file_name: format!("{file_id}.txt"),
            kind: Kind::Text,
            score: 1.0,
            snippet: None,
            page: None,
            thumb_path: None,
            modified_at: 0,
            match_sources: vec![],
            strength: Strength::Strong,
            copies: vec![],
        }
    }

    fn reply(ids: &[i64]) -> Result<SearchResponse, ErrorCode> {
        Ok(SearchResponse {
            results: ids.iter().copied().map(hit).collect(),
            took_ms: 1,
        })
    }

    /// `strong` strong results, then `weak` weak ones.
    fn folded(strong: i64, weak: i64) -> SearchState {
        let mut s = SearchState::default();
        let q = s.set_query("rent").unwrap();
        let results = (0..strong + weak)
            .map(|id| SearchResult {
                strength: if id < strong {
                    Strength::Strong
                } else {
                    Strength::Weak
                },
                ..hit(id)
            })
            .collect();
        s.apply(
            q.generation,
            Ok(SearchResponse {
                results,
                took_ms: 1,
            }),
        );
        s
    }

    #[test]
    fn only_results_on_screen_can_be_asked_about() {
        // "Nothing close": every result is folded away.
        let mut s = folded(0, 3);
        assert!(s.askable().is_empty());
        s.toggle_weak();
        assert_eq!(s.askable().len(), 3);
        // Folded weak results are not sent with the strong ones.
        assert_eq!(folded(2, 3).askable().len(), 2);
        // While a new query is out, the old list is not what was asked.
        let mut s = folded(2, 0);
        s.set_query("rent car");
        assert!(s.askable().is_empty());
    }

    #[test]
    fn weak_results_stay_folded_until_opened() {
        let mut s = folded(2, 3);
        assert_eq!((s.shown().len(), s.weak_count()), (2, 3));
        s.toggle_weak();
        assert_eq!(s.shown().len(), 5);
    }

    #[test]
    fn going_down_past_the_strong_results_opens_the_fold() {
        let mut s = folded(2, 1);
        s.select(1);
        assert!(!s.weak_open());
        s.select(1);
        assert!(s.weak_open());
        assert_eq!(s.selected().map(|r| r.file_id), Some(2));
    }

    #[test]
    fn going_down_with_nothing_folded_stays_on_the_last_result() {
        let mut s = folded(2, 0);
        s.select(5);
        assert_eq!(s.selected().map(|r| r.file_id), Some(1));
        assert!(!s.weak_open());
    }

    #[test]
    fn closing_the_fold_moves_the_selection_out_of_it() {
        let mut s = folded(2, 2);
        s.toggle_weak();
        s.select_at(3);
        s.toggle_weak();
        assert_eq!(s.selected().map(|r| r.file_id), Some(1));
    }

    #[test]
    fn with_no_strong_result_nothing_is_selected_until_opened() {
        let mut s = folded(0, 2);
        assert_eq!(s.selected_index(), None);
        assert!(s.selected().is_none());
        s.toggle_weak();
        assert_eq!(s.selected().map(|r| r.file_id), Some(0));
    }

    #[test]
    fn a_new_reply_closes_the_fold() {
        let mut s = folded(1, 1);
        s.toggle_weak();
        let q = s.set_query("rent payment").unwrap();
        s.apply(q.generation, reply(&[1]));
        assert!(!s.weak_open());
    }

    #[test]
    fn the_folds_row_shifts_only_the_weak_results() {
        let s = folded(2, 2);
        assert_eq!((s.list_item(1), s.list_item(2)), (1, 3));
    }

    #[test]
    fn failures_count_in_a_row_and_one_success_clears_them() {
        let mut s = SearchState::default();
        let q = s.set_query("rent").unwrap();
        s.apply(q.generation, Err(ErrorCode::EngineStarting));
        assert_eq!(s.failures(), 1);
        assert!(s.should_auto_retry());
        let q = s.retry().unwrap();
        s.apply(q.generation, Err(ErrorCode::EngineStarting));
        assert_eq!(s.failures(), 2);
        assert!(
            !s.should_auto_retry(),
            "only the first failure retries alone"
        );
        let q = s.retry().unwrap();
        s.apply(q.generation, reply(&[1]));
        assert_eq!(s.failures(), 0);
    }

    #[test]
    fn each_source_is_explained_on_its_own_line() {
        let s = crate::i18n::Lang::En.strings();
        assert_eq!(
            explain_sources(&[MatchSource::Keyword, MatchSource::Semantic], s, "\n"),
            "Words: Has your exact words\nRelated: Says something close to your search"
        );
        assert_eq!(explain_sources(&[], s, "\n"), "");
    }

    #[test]
    fn a_pause_with_nothing_waiting_is_not_shown_in_the_footer() {
        use magi_core::dto::PauseReason;
        let status = |state, queued| IndexStatus {
            state,
            paused_for: (state == IndexState::Paused).then_some(PauseReason::LowMemory),
            scanned: 0,
            queued,
            indexed: 10,
            skipped: 0,
            errors: 0,
            current_file: None,
            roots: vec![],
        };
        let idle = footer_status(&status(IndexState::Paused, 0));
        assert_eq!((idle.state, idle.paused_for), (IndexState::Idle, None));
        let waiting = footer_status(&status(IndexState::Paused, 4));
        assert_eq!(waiting.state, IndexState::Paused);
        assert_eq!(
            footer_status(&status(IndexState::Indexing, 0)).state,
            IndexState::Indexing
        );
    }

    #[test]
    fn only_a_match_you_cannot_see_is_explained_in_the_footer() {
        let s = crate::i18n::Lang::En.strings();
        assert_eq!(hidden_reason(&[MatchSource::Keyword], s), None);
        assert_eq!(
            hidden_reason(&[MatchSource::Keyword, MatchSource::Semantic], s),
            None
        );
        assert_eq!(
            hidden_reason(&[MatchSource::Semantic], s),
            Some("Says something close to your search")
        );
        assert_eq!(
            shown_sources(&[MatchSource::Semantic, MatchSource::Visual]),
            vec![MatchSource::Visual]
        );
        assert_eq!(
            hidden_reason(&[MatchSource::Semantic, MatchSource::Visual], s),
            Some("Looks like what you described")
        );
        assert_eq!(
            hidden_reason(&[MatchSource::Semantic, MatchSource::Ocr], s),
            Some("Has your words inside the image")
        );
        assert_eq!(hidden_reason(&[], s), None);
    }

    #[test]
    fn only_errors_that_say_why_get_a_cause() {
        let s = crate::i18n::Lang::En.strings();
        assert_eq!(
            cause(&ErrorCode::EngineStarting, s),
            Some("The search engine is still starting.")
        );
        assert!(cause(&ErrorCode::Internal { detail: "x".into() }, s).is_some());
        assert_eq!(cause(&ErrorCode::FileIdNotFound { file_id: 1 }, s), None);
    }

    #[test]
    fn typing_after_an_error_drops_the_message_at_once() {
        let mut s = SearchState::default();
        let q = s.set_query("ren").unwrap();
        s.apply(q.generation, Err(ErrorCode::EngineStarting));
        s.set_query("rent").unwrap();
        assert!(s.error().is_none());
        assert!(!s.should_auto_retry());
    }

    #[test]
    fn a_new_query_gets_a_new_generation_and_trimmed_text() {
        let mut s = SearchState::default();
        let q1 = s.set_query(" rent ").unwrap();
        assert_eq!(q1.text, "rent");
        let q2 = s.set_query("rent payment").unwrap();
        assert!(q2.generation > q1.generation);
    }

    #[test]
    fn the_same_query_after_trimming_runs_nothing() {
        let mut s = SearchState::default();
        s.set_query("rent").unwrap();
        assert_eq!(s.set_query("rent  "), None);
    }

    #[test]
    fn a_stale_reply_is_dropped() {
        let mut s = SearchState::default();
        let old = s.set_query("ren").unwrap();
        let new = s.set_query("rent").unwrap();
        s.apply(new.generation, reply(&[2]));
        s.apply(old.generation, reply(&[1]));
        assert_eq!(
            s.results().iter().map(|r| r.file_id).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn clearing_the_query_drops_the_search_in_flight() {
        let mut s = SearchState::default();
        let q = s.set_query("rent").unwrap();
        assert_eq!(s.set_query(""), None);
        s.apply(q.generation, reply(&[1]));
        assert!(s.results().is_empty());
        assert_eq!(s.selected_index(), None);
    }

    #[test]
    fn selection_clamps_and_resets_on_new_results() {
        let mut s = SearchState::default();
        let q = s.set_query("rent").unwrap();
        s.apply(q.generation, reply(&[1, 2, 3]));
        assert_eq!(s.selected_index(), Some(0));
        s.select(-1);
        assert_eq!(s.selected_index(), Some(0));
        s.select(5);
        assert_eq!(s.selected().map(|r| r.file_id), Some(3));
        s.select_at(1);
        assert_eq!(s.selected().map(|r| r.file_id), Some(2));
        s.select_at(9);
        assert_eq!(s.selected().map(|r| r.file_id), Some(3));
        let q = s.set_query("rent payment").unwrap();
        s.apply(q.generation, reply(&[7, 8]));
        assert_eq!(s.selected().map(|r| r.file_id), Some(7));
    }

    #[test]
    fn an_error_reply_clears_results_and_is_kept() {
        let mut s = SearchState::default();
        let q = s.set_query("rent").unwrap();
        s.apply(q.generation, reply(&[1]));
        let q = s.set_query("rent payment").unwrap();
        let err = ErrorCode::Internal {
            detail: "db".into(),
        };
        s.apply(q.generation, Err(err.clone()));
        assert!(s.results().is_empty());
        assert_eq!(s.error(), Some(&err));
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn loading_lasts_from_a_new_query_until_its_reply() {
        let mut s = SearchState::default();
        assert!(!s.is_loading());
        let old = s.set_query("ren").unwrap();
        assert!(s.is_loading());
        let new = s.set_query("rent").unwrap();
        s.apply(old.generation, reply(&[1]));
        assert!(s.is_loading(), "a stale reply does not end loading");
        s.apply(new.generation, reply(&[2]));
        assert!(!s.is_loading());
        s.set_query("");
        assert!(!s.is_loading(), "an empty query runs nothing");
    }

    #[test]
    fn retry_reruns_the_query_and_drops_the_failed_reply_in_flight() {
        let mut s = SearchState::default();
        assert_eq!(s.retry(), None, "nothing to retry without a query");
        let q = s.set_query("rent").unwrap();
        s.apply(
            q.generation,
            Err(ErrorCode::Internal {
                detail: "db".into(),
            }),
        );
        let again = s.retry().unwrap();
        assert_eq!(again.text, "rent");
        assert!(again.generation > q.generation);
        assert_eq!(s.error(), None);
        assert!(s.is_loading());
        assert_eq!(s.query(), "rent");
    }
}
