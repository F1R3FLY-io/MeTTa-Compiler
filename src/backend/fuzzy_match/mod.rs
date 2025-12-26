//! Fuzzy string matching for "Did you mean?" suggestions.
//!
//! This module provides fuzzy matching capabilities using liblevenshtein's
//! Levenshtein automata for efficient approximate string matching.
//!
//! **Performance Optimizations**:
//! - **Lazy Initialization**: The matcher defers expensive DynamicDawgChar
//!   construction until the first query. During normal successful evaluation,
//!   no Levenshtein automaton is built. This saves ~4% CPU time.
//! - **SIMD Acceleration**: liblevenshtein is compiled with SIMD support enabled
//!   for faster distance calculations on modern CPUs.
//! - **Bloom Filter**: Fast negative lookup rejection using a bloom filter.
//!   For `contains()` checks, this provides ~91-93% faster rejection of
//!   non-existent terms (~20-30ns vs ~25-40µs for full traversal).
//!   Memory cost: ~1.2 bytes per term.
//!
//! **Unicode Support**: Uses DynamicDawgChar for character-level Levenshtein
//! distances, providing correct Unicode semantics for multi-byte UTF-8 sequences.
//! Example: "ñ" → "n" = distance 1 (character-level), not distance 2 (byte-level).
//!
//! **Sophisticated Recommendation Heuristics** (issue #51):
//! To distinguish typos from intentional data constructors, we apply:
//! - **Relative distance threshold**: distance/min_len must be < 0.33 (avoid `lit`→`let`)
//! - **Minimum length**: Require query length >= 4 for distance-1 suggestions
//! - **Data constructor detection**: Skip suggestions for PascalCase, hyphenated names
//! - **Prefix type detection**: Don't suggest across prefix boundaries (`$x` vs `&x`)

mod context;
mod helpers;
mod matcher;
mod suggestions;
mod types;

#[cfg(test)]
mod tests;

// Re-export main types
pub use matcher::FuzzyMatcher;
pub use types::{SmartSuggestion, SuggestionConfidence, SuggestionContext};

// Re-export helper functions for testing and external use
pub use helpers::{
    are_prefixes_compatible, compute_suggestion_confidence, is_likely_data_constructor,
    type_matches, validate_type_vars, values_compatible,
};
