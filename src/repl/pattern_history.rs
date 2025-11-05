//! Pattern-based history search
//!
//! Stores command history with support for pattern matching and structural search

use crate::backend::compile::compile;
use crate::backend::models::MettaValue;
use std::collections::VecDeque;

/// History entry with source and parsed representation
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Original source text
    pub source: String,
    /// Parsed MettaValue (if parsing succeeded)
    pub parsed: Option<Vec<MettaValue>>,
    /// Entry index
    pub index: usize,
}

/// Pattern history with structural search support
pub struct PatternHistory {
    entries: VecDeque<HistoryEntry>,
    max_size: usize,
    next_index: usize,
}

impl PatternHistory {
    /// Create new pattern history
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    /// Create new pattern history with custom capacity
    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_size),
            max_size,
            next_index: 0,
        }
    }

    /// Add a command to history
    pub fn add(&mut self, command: &str) {
        // Skip empty commands
        if command.trim().is_empty() {
            return;
        }

        // Parse the command
        let parsed = compile(command).ok().map(|state| state.source);

        let entry = HistoryEntry {
            source: command.to_string(),
            parsed,
            index: self.next_index,
        };

        self.next_index += 1;

        // Add to history
        self.entries.push_back(entry);

        // Maintain max size
        if self.entries.len() > self.max_size {
            self.entries.pop_front();
        }
    }

    /// Get total number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get entry by index (0 = oldest)
    pub fn get(&self, index: usize) -> Option<&HistoryEntry> {
        self.entries.get(index)
    }

    /// Get most recent entry
    pub fn last(&self) -> Option<&HistoryEntry> {
        self.entries.back()
    }

    /// Search history by substring (case-insensitive)
    pub fn search_substring(&self, pattern: &str) -> Vec<&HistoryEntry> {
        let pattern_lower = pattern.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| entry.source.to_lowercase().contains(&pattern_lower))
            .collect()
    }

    /// Search history by regex pattern
    pub fn search_regex(&self, pattern: &str) -> Result<Vec<&HistoryEntry>, String> {
        let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
        Ok(self
            .entries
            .iter()
            .filter(|entry| re.is_match(&entry.source))
            .collect())
    }

    /// Search for entries that start with a specific prefix
    pub fn search_prefix(&self, prefix: &str) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.source.starts_with(prefix))
            .collect()
    }

    /// Search for entries containing a specific function/atom
    pub fn search_function(&self, function_name: &str) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                if let Some(parsed) = &entry.parsed {
                    Self::contains_atom(parsed, function_name)
                } else {
                    false
                }
            })
            .collect()
    }

    /// Check if parsed expressions contain a specific atom
    fn contains_atom(values: &[MettaValue], atom: &str) -> bool {
        for value in values {
            if Self::value_contains_atom(value, atom) {
                return true;
            }
        }
        false
    }

    /// Check if a MettaValue contains a specific atom
    fn value_contains_atom(value: &MettaValue, atom: &str) -> bool {
        match value {
            MettaValue::Atom(s) => s == atom,
            MettaValue::SExpr(items) => items.iter().any(|v| Self::value_contains_atom(v, atom)),
            MettaValue::Error(_, details) => Self::value_contains_atom(details, atom),
            MettaValue::Type(inner) => Self::value_contains_atom(inner, atom),
            _ => false,
        }
    }

    /// Get all entries in reverse chronological order (most recent first)
    pub fn iter_reverse(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter().rev()
    }

    /// Get all entries in chronological order (oldest first)
    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    /// Clear all history
    pub fn clear(&mut self) {
        self.entries.clear();
        self.next_index = 0;
    }

    /// Get entries that match a structural pattern
    /// For example, "(= $x $y)" matches all rule definitions
    pub fn search_structural_pattern(&self, _pattern: &str) -> Vec<&HistoryEntry> {
        // TODO: Implement full pattern matching with variables
        // For now, return empty vector
        Vec::new()
    }
}

impl Default for PatternHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_history_creation() {
        let history = PatternHistory::new();
        assert_eq!(history.len(), 0);
        assert!(history.is_empty());
    }

    #[test]
    fn test_add_entry() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        assert_eq!(history.len(), 1);
        assert!(!history.is_empty());
    }

    #[test]
    fn test_add_multiple_entries() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");
        history.add("(* 4 4)");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_get_entry() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");

        let entry = history.get(0).unwrap();
        assert_eq!(entry.source, "(+ 1 2)");

        let entry = history.get(1).unwrap();
        assert_eq!(entry.source, "(- 5 3)");
    }

    #[test]
    fn test_last_entry() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");

        let last = history.last().unwrap();
        assert_eq!(last.source, "(- 5 3)");
    }

    #[test]
    fn test_search_substring() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");
        history.add("(+ 10 20)");

        let results = history.search_substring("+");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_prefix() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");
        history.add("(+ 10 20)");

        let results = history.search_prefix("(+");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_function() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");
        history.add("(* (+ 1 1) 2)");

        let results = history.search_function("+");
        assert_eq!(results.len(), 2); // Both direct and nested uses
    }

    #[test]
    fn test_max_capacity() {
        let mut history = PatternHistory::with_capacity(3);
        history.add("cmd1");
        history.add("cmd2");
        history.add("cmd3");
        history.add("cmd4"); // Should push out cmd1

        assert_eq!(history.len(), 3);
        assert_eq!(history.get(0).unwrap().source, "cmd2");
    }

    #[test]
    fn test_skip_empty_commands() {
        let mut history = PatternHistory::new();
        history.add("");
        history.add("   ");
        history.add("\t\n");

        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_iter_reverse() {
        let mut history = PatternHistory::new();
        history.add("cmd1");
        history.add("cmd2");
        history.add("cmd3");

        let sources: Vec<String> = history.iter_reverse().map(|e| e.source.clone()).collect();

        assert_eq!(sources, vec!["cmd3", "cmd2", "cmd1"]);
    }

    #[test]
    fn test_clear() {
        let mut history = PatternHistory::new();
        history.add("(+ 1 2)");
        history.add("(- 5 3)");

        history.clear();
        assert_eq!(history.len(), 0);
        assert!(history.is_empty());
    }
}
