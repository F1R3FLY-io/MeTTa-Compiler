//! REPL configuration
//!
//! Manages settings for history file location, key bindings, colors, and
//! completion behavior. Loads from `$XDG_CONFIG_HOME/mettatron/repl.toml`
//! (or `$HOME/.config/mettatron/repl.toml`) with sensible defaults.

use std::path::PathBuf;

/// REPL configuration
#[derive(Debug, Clone)]
pub struct ReplConfig {
    /// File path for persistent command history.
    pub history_file: PathBuf,
    /// Maximum number of history entries to retain.
    pub history_max_entries: usize,
    /// Whether to use ANSI color in output.
    pub colors_enabled: bool,
    /// Whether to enable command completion via Tab.
    pub completion_enabled: bool,
}

impl ReplConfig {
    /// Load configuration with sensible defaults.
    ///
    /// History file resolves to `${XDG_CONFIG_HOME:-$HOME/.config}/mettatron/repl_history`.
    /// All other knobs default to the most useful settings.
    pub fn load() -> Self {
        let history_file = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mettatron")
            .join("repl_history");

        Self {
            history_file,
            history_max_entries: 1000,
            colors_enabled: std::env::var("NO_COLOR").is_err(),
            completion_enabled: true,
        }
    }
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self::load()
    }
}
