//! Helper functions for space operations.
//!
//! This module provides utility functions for space name validation
//! and suggestion generation.

use std::sync::OnceLock;

use crate::backend::fuzzy_match::FuzzyMatcher;

/// Valid space names for "Did you mean?" suggestions
pub(super) const VALID_SPACE_NAMES: &[&str] = &["self"];

/// Get fuzzy matcher for space names (lazily initialized)
pub(super) fn space_name_matcher() -> &'static FuzzyMatcher {
    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    MATCHER.get_or_init(|| FuzzyMatcher::from_terms(VALID_SPACE_NAMES.iter().copied()))
}

/// Suggest a valid space name if the given name is close to one
pub(super) fn suggest_space_name(name: &str) -> Option<String> {
    // First check for common case errors
    let lower = name.to_lowercase();
    if lower == "self" && name != "self" {
        return Some("Did you mean: self? (space names are case-sensitive)".to_string());
    }

    // Use fuzzy matcher for other typos (e.g., "slef" -> "self")
    space_name_matcher().did_you_mean(name, 2, 1)
}
