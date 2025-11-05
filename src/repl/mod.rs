//! Enhanced REPL implementation with syntax highlighting, pattern matching, and multi-line support
//!
//! This module provides advanced REPL features:
//! - Tree-Sitter query-based syntax highlighting
//! - State machine for multi-line input handling
//! - Smart indentation using Tree-Sitter indent queries
//! - PathMap-based pattern history search
//! - Interactive history search interface

pub mod config;
pub mod helper;
pub mod history_search;
pub mod indenter;
pub mod pattern_history;
pub mod query_highlighter;
pub mod state_machine;

// Re-exports for convenience
pub use config::ReplConfig;
pub use helper::MettaHelper;
pub use history_search::HistorySearchInterface;
pub use indenter::SmartIndenter;
pub use pattern_history::PatternHistory;
pub use query_highlighter::QueryHighlighter;
pub use state_machine::{ReplEvent, ReplState, ReplStateMachine, StateTransition};
