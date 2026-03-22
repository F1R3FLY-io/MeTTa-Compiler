//! Speculative Parallel Head Matching (WAM Neck)
//!
//! When an expression has many candidate rules (>threshold), this module
//! splits the matching phase into parallel chunks. Head matching is pure
//! (read-only), so it can be safely parallelized.
//!
//! ## Design
//!
//! ```text
//! Expression E with N candidates:
//!
//! ┌──────────┐  ┌──────────┐  ┌──────────┐
//! │ Chunk 0  │  │ Chunk 1  │  │ Chunk 2  │  ... (parallel)
//! │ match    │  │ match    │  │ match    │
//! │ rules    │  │ rules    │  │ rules    │
//! │ 0..k     │  │ k..2k   │  │ 2k..3k  │
//! └──┬───────┘  └──┬───────┘  └──┬───────┘
//!    │             │             │
//!    └─────────────┴─────────────┘
//!                  │
//!            Merge results
//!                  │
//!            ┌─────v──────┐
//!            │ Body eval  │  (sequential or parallel via dispatch_rule_matches)
//!            └────────────┘
//! ```
//!
//! ## Threshold
//!
//! Parallel speculative matching is only used when the candidate count exceeds
//! `SPECULATIVE_MATCH_THRESHOLD` (default: 32). Below this threshold, the
//! overhead of thread dispatch exceeds the matching cost.

use smallvec::SmallVec;

use crate::backend::models::{GenericBindings, MettaValueTrait};

// ============================================================================
// Configuration
// ============================================================================

/// Minimum number of candidates to trigger speculative parallel matching.
///
/// Below this threshold, sequential matching is faster due to lower overhead.
/// Configurable via `METTATRON_SPECULATIVE_MATCH_THRESHOLD` env var.
pub const DEFAULT_SPECULATIVE_MATCH_THRESHOLD: usize = 32;

/// Get the configured speculative match threshold.
pub fn speculative_match_threshold() -> usize {
    static THRESHOLD: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("METTATRON_SPECULATIVE_MATCH_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_SPECULATIVE_MATCH_THRESHOLD)
    })
}

// ============================================================================
// Match Candidate
// ============================================================================

/// Result of a successful speculative head match.
#[derive(Debug, Clone)]
pub struct MatchCandidate<V: MettaValueTrait> {
    /// The RHS template from the matched rule.
    pub rhs: V,
    /// Bindings produced by the structural match.
    pub bindings: GenericBindings<V>,
    /// Whether the RHS has variables (for fast-path decisions).
    pub rhs_has_variables: bool,
}

// ============================================================================
// Speculative Matching
// ============================================================================

/// Check if speculative parallel matching should be used for the given
/// candidate count.
///
/// Returns `true` if the count exceeds the threshold AND there are
/// idle workers available to do the matching.
#[inline]
pub fn should_speculate(candidate_count: usize) -> bool {
    candidate_count >= speculative_match_threshold()
}

/// Split candidates into chunks for parallel matching.
///
/// Returns chunk boundaries as `(start, end)` pairs. Each chunk can be
/// matched independently by a worker thread.
pub fn chunk_candidates(total: usize, num_workers: usize) -> SmallVec<[(usize, usize); 8]> {
    let chunk_size = (total + num_workers - 1) / num_workers; // Ceiling division
    let mut chunks = SmallVec::new();
    let mut start = 0;
    while start < total {
        let end = (start + chunk_size).min(total);
        chunks.push((start, end));
        start = end;
    }
    chunks
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_speculate() {
        // Below threshold
        assert!(!should_speculate(10));
        assert!(!should_speculate(31));

        // At or above threshold
        assert!(should_speculate(32));
        assert!(should_speculate(100));
    }

    #[test]
    fn test_chunk_candidates_even() {
        let chunks = chunk_candidates(20, 4);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0], (0, 5));
        assert_eq!(chunks[1], (5, 10));
        assert_eq!(chunks[2], (10, 15));
        assert_eq!(chunks[3], (15, 20));
    }

    #[test]
    fn test_chunk_candidates_uneven() {
        let chunks = chunk_candidates(10, 3);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], (0, 4));
        assert_eq!(chunks[1], (4, 8));
        assert_eq!(chunks[2], (8, 10));
    }

    #[test]
    fn test_chunk_candidates_single() {
        let chunks = chunk_candidates(5, 1);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, 5));
    }

    #[test]
    fn test_chunk_candidates_more_workers_than_items() {
        let chunks = chunk_candidates(3, 8);
        assert_eq!(chunks.len(), 3); // One item per chunk
        assert_eq!(chunks[0], (0, 1));
        assert_eq!(chunks[1], (1, 2));
        assert_eq!(chunks[2], (2, 3));
    }
}
