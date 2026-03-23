//! Pattern matching operations for Environment.
//!
//! Provides methods for matching patterns against atoms in the MettaTrie.
//! Includes bloom filter optimization for O(1) rejection.
//!
//! # Deferred Expansion
//!
//! `match_space()` returns `Vec<MultiplicityMatch>` with compressed (value, count) pairs
//! instead of expanding high-multiplicity matches immediately. This:
//!
//! - Reduces memory usage from O(N*M) to O(N) where M = max multiplicity
//! - Enables lazy expansion only when results are actually consumed
//! - Prevents OOM on high-multiplicity atoms (e.g., multiplicity = 1000)
//!
//! Call `.expand()` on each `MultiplicityMatch` to get an iterator of cloned values,
//! or use `.into_iter().flat_map(|m| m.expand()).collect()` to expand all results.

use tracing::trace;

use super::{MettaEnvironment, MettaValue};
use crate::backend::decompose::decompose;
use crate::backend::eval::{apply_bindings, pattern_match};
use crate::backend::models::metta_value_trait::MettaValueTrait;

// Re-export the generic MultiplicityMatch specialized for MettaValue
pub use super::generic::MultiplicityMatch;

impl MettaEnvironment {
    // Note: match_space() is now a generic method on GenericEnvironment<V, F> in generic.rs.
    // The implementation uses MettaTrie for storage and supports any MettaValueTrait type.

    /// Match pattern against atoms using MettaTrie::query() (O(k*b) where k = depth, b = branching).
    ///
    /// This is an optimized version of `match_space()` that uses the trie's built-in
    /// pattern matching with Variable edge traversal instead of iterating through all atoms.
    ///
    /// # Performance
    ///
    /// For sparse matches (k << n), this can be orders of magnitude faster than iteration.
    /// For dense matches (k ~ n), performance is similar.
    /// No arity limit — MettaTrie handles any expression size natively.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for each match
    ///
    /// # Returns
    /// - `Some(results)` with matches (even if empty)
    /// - `None` only if the operation could not be performed (caller should fall back)
    pub fn match_space_query_multi(
        &self,
        pattern: &MettaValue,
        template: &MettaValue,
    ) -> Option<Vec<MultiplicityMatch<MettaValue>>> {
        trace!(target: "mettatron::environment::match_space_query_multi", ?pattern, ?template);

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self
                .shared
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                return Some(Vec::new());
            }
        }

        // Decompose pattern to TrieKey sequence with Variable wildcards
        let pattern_keys = decompose(pattern);
        let btm = self.shared.atom_space.btm.read();

        let mut results: Vec<MultiplicityMatch<MettaValue>> = Vec::new();

        // Use MettaTrie::query() for trie-based pattern matching
        // query() follows both concrete and Variable edges at each level
        for qm in btm.query(&pattern_keys) {
            // qm.expr is the stored expression, qm.value is Multiplicity
            // Use pattern_match to extract proper bindings
            if let Some(bindings) = pattern_match(pattern, &qm.expr) {
                let instantiated = apply_bindings(template, &bindings).into_owned();
                let multiplicity = qm.value.count().max(1) as usize;
                results.push(MultiplicityMatch::new(instantiated, multiplicity));
            }
        }

        // No wide_btm — MettaTrie handles any arity natively

        Some(results)
    }

    /// Match pattern against atoms, returning first match only (early exit)
    ///
    /// This is an optimization for cases where only one match is needed (existence checks,
    /// deterministic lookups, etc.). It exits immediately on first match.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for the match
    ///
    /// # Returns
    /// `Some(instantiated_template)` if a match is found, `None` otherwise
    pub fn match_space_first(
        &self,
        pattern: &MettaValue,
        template: &MettaValue,
    ) -> Option<MettaValue> {
        // BLOOM FILTER CHECK: O(1) rejection
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self
                .shared
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(expected_head.as_bytes(), pattern_arity)
            {
                return None;
            }
        }

        // Use MettaTrie::query() with early exit on first match
        let pattern_keys = decompose(pattern);
        let btm = self.shared.atom_space.btm.read();

        for qm in btm.query(&pattern_keys) {
            if let Some(bindings) = pattern_match(pattern, &qm.expr) {
                let instantiated = apply_bindings(template, &bindings).into_owned();
                return Some(instantiated); // EARLY EXIT
            }
        }

        // No wide_btm — MettaTrie handles any arity natively

        None
    }

    // Note: match_space_exists() is now a generic method on GenericEnvironment<V, F> in generic.rs.
}
