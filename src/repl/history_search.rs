//! Interactive history search interface
//!
//! Provides an interactive search UI for navigating command history

use super::pattern_history::{HistoryEntry, PatternHistory};

/// Search mode for history
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchMode {
    /// Substring search (case-insensitive)
    Substring,
    /// Prefix search
    Prefix,
    /// Function/atom search
    Function,
    /// Regex search
    Regex,
}

/// Search result with match information
#[derive(Debug, Clone)]
pub struct SearchResult<'a> {
    /// The matching history entry
    pub entry: &'a HistoryEntry,
    /// Position in search results (0-based)
    pub position: usize,
    /// Total number of results
    pub total: usize,
}

/// Interactive history search interface
pub struct HistorySearchInterface {
    mode: SearchMode,
    current_query: String,
    current_position: usize,
}

impl HistorySearchInterface {
    /// Create new search interface
    pub fn new() -> Self {
        Self {
            mode: SearchMode::Substring,
            current_query: String::new(),
            current_position: 0,
        }
    }

    /// Set search mode
    pub fn set_mode(&mut self, mode: SearchMode) {
        self.mode = mode;
        self.current_position = 0;
    }

    /// Get current search mode
    pub fn mode(&self) -> &SearchMode {
        &self.mode
    }

    /// Search history with current settings
    pub fn search<'a>(&self, history: &'a PatternHistory, query: &str) -> Vec<&'a HistoryEntry> {
        match self.mode {
            SearchMode::Substring => history.search_substring(query),
            SearchMode::Prefix => history.search_prefix(query),
            SearchMode::Function => history.search_function(query),
            SearchMode::Regex => history.search_regex(query).unwrap_or_default(),
        }
    }

    /// Search and get current result
    pub fn search_with_position<'a>(
        &mut self,
        history: &'a PatternHistory,
        query: &str,
    ) -> Option<SearchResult<'a>> {
        self.current_query = query.to_string();
        let results = self.search(history, query);

        if results.is_empty() {
            return None;
        }

        // Ensure position is in bounds
        if self.current_position >= results.len() {
            self.current_position = 0;
        }

        Some(SearchResult {
            entry: results[self.current_position],
            position: self.current_position,
            total: results.len(),
        })
    }

    /// Move to next search result
    pub fn next<'a>(&mut self, history: &'a PatternHistory) -> Option<SearchResult<'a>> {
        let results = self.search(history, &self.current_query.clone());

        if results.is_empty() {
            return None;
        }

        self.current_position = (self.current_position + 1) % results.len();

        Some(SearchResult {
            entry: results[self.current_position],
            position: self.current_position,
            total: results.len(),
        })
    }

    /// Move to previous search result
    pub fn previous<'a>(&mut self, history: &'a PatternHistory) -> Option<SearchResult<'a>> {
        let results = self.search(history, &self.current_query.clone());

        if results.is_empty() {
            return None;
        }

        if self.current_position == 0 {
            self.current_position = results.len() - 1;
        } else {
            self.current_position -= 1;
        }

        Some(SearchResult {
            entry: results[self.current_position],
            position: self.current_position,
            total: results.len(),
        })
    }

    /// Reset search state
    pub fn reset(&mut self) {
        self.current_query.clear();
        self.current_position = 0;
    }

    /// Get current query
    pub fn current_query(&self) -> &str {
        &self.current_query
    }

    /// Get current position
    pub fn current_position(&self) -> usize {
        self.current_position
    }
}

impl Default for HistorySearchInterface {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_history() -> PatternHistory {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");
        history.add("(+ 10 20)");
        history.add("(* 2 3)");
        history
    }

    #[test]
    fn test_search_interface_creation() {
        let interface = HistorySearchInterface::new();
        assert_eq!(interface.mode(), &SearchMode::Substring);
    }

    #[test]
    fn test_set_mode() {
        let mut interface = HistorySearchInterface::new();
        interface.set_mode(SearchMode::Prefix);
        assert_eq!(interface.mode(), &SearchMode::Prefix);
    }

    #[test]
    fn test_substring_search() {
        let interface = HistorySearchInterface::new();
        let history = setup_history();

        let results = interface.search(&history, "+");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_with_position() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        let result = interface.search_with_position(&history, "+");
        assert!(result.is_some());

        let result = result.unwrap();
        assert_eq!(result.position, 0);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn test_next_result() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        // Initial search
        interface.search_with_position(&history, "+");

        // Move to next
        let result = interface.next(&history);
        assert!(result.is_some());
        assert_eq!(result.unwrap().position, 1);
    }

    #[test]
    fn test_next_wraps_around() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        interface.search_with_position(&history, "+");
        interface.next(&history); // position = 1
        let result = interface.next(&history); // should wrap to 0

        assert_eq!(result.unwrap().position, 0);
    }

    #[test]
    fn test_previous_result() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        interface.search_with_position(&history, "+");
        interface.next(&history); // position = 1

        let result = interface.previous(&history);
        assert_eq!(result.unwrap().position, 0);
    }

    #[test]
    fn test_previous_wraps_around() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        interface.search_with_position(&history, "+");
        // position = 0, previous should wrap to last

        let result = interface.previous(&history);
        assert_eq!(result.unwrap().position, 1);
    }

    #[test]
    fn test_reset() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        interface.search_with_position(&history, "+");
        interface.next(&history);

        interface.reset();
        assert_eq!(interface.current_query(), "");
        assert_eq!(interface.current_position(), 0);
    }

    #[test]
    fn test_prefix_search_mode() {
        let mut interface = HistorySearchInterface::new();
        interface.set_mode(SearchMode::Prefix);

        let history = setup_history();
        let results = interface.search(&history, "(+");

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_function_search_mode() {
        let mut interface = HistorySearchInterface::new();
        interface.set_mode(SearchMode::Function);

        let history = setup_history();
        let results = interface.search(&history, "+");

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_empty_search_results() {
        let mut interface = HistorySearchInterface::new();
        let history = setup_history();

        let result = interface.search_with_position(&history, "nonexistent");
        assert!(result.is_none());
    }
}
