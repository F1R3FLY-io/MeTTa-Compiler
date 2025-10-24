//! Enhanced REPL implementation with syntax highlighting, pattern matching, and multi-line support
//!
//! This module provides advanced REPL features:
//! - Tree-Sitter query-based syntax highlighting
//! - State machine for multi-line input handling
//! - Smart indentation using Tree-Sitter indent queries
//! - PathMap-based pattern history search
//! - Interactive history search interface

pub mod query_highlighter;
pub mod state_machine;
pub mod indenter;
pub mod pattern_history;
pub mod history_search;
pub mod helper;
pub mod config;

// Re-exports for convenience
pub use query_highlighter::QueryHighlighter;
pub use state_machine::{ReplStateMachine, ReplState, ReplEvent, StateTransition};
pub use indenter::SmartIndenter;
pub use pattern_history::PatternHistory;
pub use history_search::HistorySearchInterface;
pub use helper::MettaHelper;
pub use config::ReplConfig;
