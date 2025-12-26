//! Pattern matching operations for Environment.
//!
//! Provides methods for matching patterns against atoms in the Space.
//! Includes bloom filter optimization for O(1) rejection.

use mork_expr::Expr;
use tracing::trace;

use super::{Environment, MettaValue};
use crate::backend::eval::{apply_bindings, pattern_match};

impl Environment {
    /// Match pattern against all atoms in the Space (optimized for match operation)
    /// Returns all instantiated templates for atoms matching the pattern
    ///
    /// This is optimized to work directly with MORK expressions, avoiding
    /// unnecessary string serialization and parsing.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for each match
    ///
    /// # Returns
    /// Vector of instantiated templates (MettaValue) for all matches
    pub fn match_space(&self, pattern: &MettaValue, template: &MettaValue) -> Vec<MettaValue> {
        trace!(target: "mettatron::environment::match_space", ?pattern, ?template);

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        // This is "Tier 0" optimization - skips entire iteration if bloom filter says no match
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                // Definitely no matching expressions exist
                return Vec::new();
            }
        }

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // 1. Iterate through MORK PathMap (primary storage)
        while rz.to_next_val() {
            let ptr = rz.path().as_ptr();

            // Get the s-expression at this position
            let expr = Expr {
                ptr: ptr.cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Try to match the pattern against this atom
                if let Some(bindings) = pattern_match(pattern, &atom) {
                    // Apply bindings to the template
                    let instantiated = apply_bindings(template, &bindings);
                    results.push(instantiated);
                }
            }
        }

        drop(space);

        // 2. Also check large expression fallback PathMap (if allocated)
        // These are expressions with arity >= 64 that couldn't fit in MORK
        let guard = self
            .shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings);
                    results.push(instantiated);
                }
            }
        }

        results
    }

    /// Match pattern against atoms in the Space, returning first match only (early exit)
    ///
    /// This is an optimization for cases where only one match is needed (existence checks,
    /// deterministic lookups, etc.). It exits immediately on first match, avoiding the
    /// O(N) iteration through all facts when only one is needed.
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
        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity)
            {
                // Definitely no matching expressions exist
                return None;
            }
        }

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();

        // OPTIMIZATION: Extract pattern's head symbol and arity for lazy pre-filtering
        let pattern_head_bytes: Option<&[u8]> = pattern.get_head_symbol().map(|s| s.as_bytes());
        // Note: mork_head_info() already adjusts MORK arity to match MettaValue convention
        let pattern_arity = pattern.get_arity() as u8;

        // 1. Iterate through MORK PathMap (primary storage) - EARLY EXIT on first match
        while rz.to_next_val() {
            let ptr = rz.path().as_ptr();

            // DISABLED: pre-filter extracts wrong data from rz.path()
            /*
            if let Some(expected_head) = pattern_head_bytes {
                if let Some((mork_head, mork_arity)) = unsafe { Self::mork_head_info(ptr) } {
                    if mork_head != expected_head || mork_arity != pattern_arity {
                        continue; // Skip this expression entirely
                    }
                }
            }
            */
            let _ = (pattern_head_bytes, pattern_arity); // suppress unused warnings

            let expr = Expr {
                ptr: ptr.cast_mut(),
            };

            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some(bindings) = pattern_match(pattern, &atom) {
                    let instantiated = apply_bindings(template, &bindings);
                    return Some(instantiated); // EARLY EXIT - found first match!
                }
            }
        }

        drop(space);

        // 2. Check large expression fallback PathMap
        let guard = self
            .shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings);
                    return Some(instantiated); // EARLY EXIT
                }
            }
        }

        None
    }

    /// Check if any atom in the Space matches the pattern (existence check only)
    ///
    /// This is the fastest query when you only need to know IF a match exists,
    /// not what the match is. It avoids template instantiation overhead.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    ///
    /// # Returns
    /// `true` if at least one match exists, `false` otherwise
    pub fn match_space_exists(&self, pattern: &MettaValue) -> bool {
        // BLOOM FILTER CHECK: O(1) rejection
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity)
            {
                return false;
            }
        }

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();

        let pattern_head_bytes: Option<&[u8]> = pattern.get_head_symbol().map(|s| s.as_bytes());
        // Note: mork_head_info() already adjusts MORK arity to match MettaValue convention
        let pattern_arity = pattern.get_arity() as u8;

        // Iterate through MORK PathMap - EARLY EXIT on first match
        while rz.to_next_val() {
            let ptr = rz.path().as_ptr();

            // DISABLED: pre-filter extracts wrong data from rz.path()
            // TODO: Investigate why mork_head_info returns garbage bytes
            /*
            if let Some(expected_head) = pattern_head_bytes {
                if let Some((mork_head, mork_arity)) = unsafe { Self::mork_head_info(ptr) } {
                    if mork_head != expected_head || mork_arity != pattern_arity {
                        continue;
                    }
                }
            }
            */
            let _ = (pattern_head_bytes, pattern_arity); // suppress unused warnings

            let expr = Expr {
                ptr: ptr.cast_mut(),
            };

            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                if pattern_match(pattern, &atom).is_some() {
                    return true; // EARLY EXIT - match exists!
                }
            }
        }

        drop(space);

        // Check large expression fallback PathMap
        let guard = self
            .shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if pattern_match(pattern, stored_value).is_some() {
                    return true; // EARLY EXIT
                }
            }
        }

        false
    }
}
