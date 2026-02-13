//! Pattern matching operations for Environment.
//!
//! Provides methods for matching patterns against atoms in the Space.
//! Includes bloom filter optimization for O(1) rejection.
//!
//! # Deferred Expansion
//!
//! `match_space()` returns `Vec<MultiplicityMatch>` with compressed (value, count) pairs
//! instead of expanding high-multiplicity matches immediately. This:
//!
//! - Reduces memory usage from O(N×M) to O(N) where M = max multiplicity
//! - Enables lazy expansion only when results are actually consumed
//! - Prevents OOM on high-multiplicity atoms (e.g., multiplicity = 1000)
//!
//! Call `.expand()` on each `MultiplicityMatch` to get an iterator of cloned values,
//! or use `.into_iter().flat_map(|m| m.expand()).collect()` to expand all results.

use mork_expr::Expr;
#[allow(unused_imports)]
use pathmap::zipper::ZipperValues;
use tracing::trace;

use super::multiplicity::get_multiplicity;
use super::{MettaEnvironment, MettaValue};
use crate::backend::eval::{apply_bindings, pattern_match};
use crate::backend::models::metta_value_trait::MettaValueTrait;
use crate::backend::mork_convert::{metta_to_mork_query_bytes, mork_bindings_to_metta, ConversionContext};

// Re-export the generic MultiplicityMatch specialized for MettaValue
pub use super::generic::MultiplicityMatch;

impl MettaEnvironment {
    // Note: match_space() is now a generic method on GenericEnvironment<V, F> in generic.rs.
    // The implementation uses MORK PathMap for storage and supports any MettaValueTrait type.

    /// Match pattern against atoms using MORK's native query_multi (O(k) where k = matches).
    ///
    /// This is an optimized version of `match_space()` that uses MORK's trie-based pattern
    /// matching instead of iterating through all atoms. This is O(k) where k = number of
    /// matching atoms, compared to O(n) for the iteration-based approach where n = total atoms.
    ///
    /// # Performance
    ///
    /// For sparse matches (k << n), this can be orders of magnitude faster than iteration.
    /// For dense matches (k ≈ n), performance is similar.
    ///
    /// # Limitations
    ///
    /// - Returns `None` for patterns with arity >= 64 (MORK limitation) - caller should fallback
    /// - May not work correctly with certain pattern structures
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for each match
    ///
    /// # Returns
    /// - `Some(results)` if query_multi was used successfully (even if no matches found)
    /// - `None` if query_multi couldn't be used (caller should fall back to `match_space()`)
    pub fn match_space_query_multi(
        &self,
        pattern: &MettaValue,
        template: &MettaValue,
    ) -> Option<Vec<MultiplicityMatch<MettaValue>>> {
        trace!(target: "mettatron::environment::match_space_query_multi", ?pattern, ?template);

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            // parking_lot::RwLock - no .expect()
            let bloom_result = self
                .shared
                .head_arity_bloom
                .read()
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                return Some(Vec::new()); // Definitely no matches - return empty, not None
            }
        }

        let space = self.create_space();

        // Create conversion context to track variable mappings
        let mut ctx = ConversionContext::new();

        // Convert pattern to MORK bytes
        let pattern_bytes = match metta_to_mork_query_bytes(pattern, &self.shared_mapping, &mut ctx) {
            Ok(bytes) => bytes,
            Err(_) => {
                // Conversion failed (e.g., arity too high) - return None to trigger fallback
                return None;
            }
        };


        let pattern_expr = Expr {
            ptr: pattern_bytes.as_ptr().cast_mut(),
        };

        // Collect matches using MORK's native query_multi
        let mut results: Vec<MultiplicityMatch<MettaValue>> = Vec::new();

        mork::space::Space::query_multi(&space.btm, pattern_expr, |result, matched_expr| {
            if let Err(mork_bindings) = result {
                // Convert MORK bindings to our format
                if let Ok(bindings) = mork_bindings_to_metta(&mork_bindings, &ctx, &space) {
                    // Apply bindings to template
                    let instantiated = apply_bindings(template, &bindings).into_owned();

                    // Extract multiplicity from the matched expression's PathMap path.
                    // matched_expr.span() returns *const [u8] — the serialized MORK bytes
                    // that form the exact PathMap key for this entry. We look up the
                    // multiplicity in the same PathMap that query_multi is traversing.
                    // SAFETY: matched_expr.ptr points to valid MORK bytes within PathMap
                    // memory. The span() traversal is bounded by the expression's length.
                    let mork_bytes = unsafe { &*matched_expr.span() };
                    let multiplicity = get_multiplicity(&space.btm, mork_bytes).max(1) as usize;

                    results.push(MultiplicityMatch::new(instantiated, multiplicity));
                }
            }
            true // Continue searching for ALL matches
        });

        // Also check large expression fallback PathMap
        // parking_lot::RwLock - no .expect()
        let guard = self.shared.large_expr_pathmap.read();
        if let Some(ref fallback) = *guard {
            let btm = self.shared.btm.read();

            for (key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    let multiplicity = get_multiplicity(&btm, &key).max(1) as usize;
                    results.push(MultiplicityMatch::new(instantiated, multiplicity));
                }
            }
        }

        Some(results)
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
            // parking_lot::RwLock - no .expect()
            if !self
                .shared
                .head_arity_bloom
                .read()
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
        // With value-based multiplicity, every entry is an atom (no filtering needed)
        while rz.to_next_val() {
            let path_bytes = rz.path();
            let ptr = path_bytes.as_ptr();

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
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    return Some(instantiated); // EARLY EXIT - found first match!
                }
            }
        }

        drop(space);

        // 2. Check large expression fallback PathMap
        // parking_lot::RwLock - no .expect()
        let guard = self.shared.large_expr_pathmap.read();
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    return Some(instantiated); // EARLY EXIT
                }
            }
        }

        None
    }

    // Note: match_space_exists() is now a generic method on GenericEnvironment<V, F> in generic.rs.
    // The implementation uses MORK PathMap for storage and supports any MettaValueTrait type.
}
