//! What the search window shows, independent of GPUI: the current query,
//! which reply belongs to it, and the selection.

use magi_core::dto::{ErrorCode, SearchResponse, SearchResult};

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
    selected: usize,
    error: Option<ErrorCode>,
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
            self.error = None;
            self.selected = 0;
            return None;
        }
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
        match reply {
            Ok(response) => {
                self.results = response.results;
                self.error = None;
            }
            Err(error) => {
                self.results.clear();
                self.error = Some(error);
            }
        }
        self.selected = 0;
    }

    /// Moves the selection, stopping at the first and last result.
    pub fn select(&mut self, delta: isize) {
        let last = self.results.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn selected_index(&self) -> Option<usize> {
        (!self.results.is_empty()).then_some(self.selected)
    }

    pub fn selected(&self) -> Option<&SearchResult> {
        self.results.get(self.selected)
    }

    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    pub fn error(&self) -> Option<&ErrorCode> {
        self.error.as_ref()
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
        }
    }

    fn reply(ids: &[i64]) -> Result<SearchResponse, ErrorCode> {
        Ok(SearchResponse {
            results: ids.iter().copied().map(hit).collect(),
            took_ms: 1,
        })
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
}
