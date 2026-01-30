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
use pathmap::zipper::ZipperValues;
use tracing::trace;

use super::multiplicity::get_multiplicity;
use super::{Environment, MettaValue};
use crate::backend::eval::{apply_bindings, pattern_match};
use crate::backend::mork_convert::{metta_to_mork_bytes, mork_bindings_to_metta, ConversionContext};

/// A lazy match result with deferred multiplicity expansion.
///
/// Instead of cloning the template N times for atoms with multiplicity N,
/// we store the template once with its count. This reduces memory usage
/// from O(N×M) to O(N) where M is the maximum multiplicity.
///
/// # Example
/// ```ignore
/// // Instead of: vec![atom.clone(), atom.clone(), atom.clone()] for multiplicity 3
/// // We store:   MultiplicityMatch { value: atom, count: 3 }
/// ```
#[derive(Debug, Clone)]
pub struct MultiplicityMatch {
    /// The instantiated template value
    pub value: MettaValue,
    /// Number of times this value should appear in results
    pub count: usize,
}

impl MultiplicityMatch {
    /// Create a new multiplicity match.
    #[inline]
    pub fn new(value: MettaValue, count: usize) -> Self {
        Self { value, count }
    }

    /// Expand into an iterator of cloned values.
    ///
    /// This defers the cloning until the iterator is actually consumed,
    /// enabling lazy evaluation of high-multiplicity matches.
    ///
    /// # Example
    /// ```ignore
    /// let m = MultiplicityMatch::new(atom, 1000);
    /// // Only clones when iterated:
    /// for value in m.expand().take(10) {
    ///     // Only 10 clones happen, not 1000
    /// }
    /// ```
    #[inline]
    pub fn expand(self) -> impl Iterator<Item = MettaValue> {
        std::iter::repeat(self.value).take(self.count)
    }

    /// Check if this is a single match (count == 1).
    #[inline]
    pub fn is_single(&self) -> bool {
        self.count == 1
    }
}

impl Environment {
    /// Match pattern against all atoms in the Space (optimized for match operation)
    ///
    /// Returns `MultiplicityMatch` structs containing the instantiated template and its
    /// multiplicity count. This deferred expansion design avoids cloning the template N times
    /// for atoms with multiplicity N.
    ///
    /// This is optimized to work directly with MORK expressions, avoiding
    /// unnecessary string serialization and parsing.
    ///
    /// # Memory Efficiency
    ///
    /// For atoms with high multiplicity (e.g., an atom added 1000 times), this returns a
    /// single `MultiplicityMatch { value: template, count: 1000 }` rather than cloning
    /// the template 1000 times, reducing memory from O(N×M) to O(N) where M = max multiplicity.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for each match
    ///
    /// # Returns
    /// Vector of `MultiplicityMatch` structs, each containing a value and its count.
    /// Call `.expand()` on each to get an iterator of cloned values.
    ///
    /// # Example
    /// ```ignore
    /// // Returns compressed results
    /// let results = env.match_space(&pattern, &template);
    /// let total_count: usize = results.iter().map(|m| m.count).sum();
    ///
    /// // Expand on demand for iteration:
    /// for m in results {
    ///     for value in m.expand().take(10) {
    ///         // Process only first 10 of each match
    ///     }
    /// }
    ///
    /// // Or expand all for Vec<MettaValue>:
    /// let expanded: Vec<MettaValue> = env.match_space(&pattern, &template)
    ///     .into_iter()
    ///     .flat_map(|m| m.expand())
    ///     .collect();
    /// ```
    pub fn match_space(
        &self,
        pattern: &MettaValue,
        template: &MettaValue,
    ) -> Vec<MultiplicityMatch> {
        trace!(target: "mettatron::environment::match_space", ?pattern, ?template);

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                return Vec::new();
            }
        }

        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // 1. Iterate through MORK PathMap (primary storage)
        // With value-based multiplicity, every entry is an atom with its count as the value
        while rz.to_next_val() {
            let path_bytes = rz.path();

            // Get multiplicity directly from the value (no separate lookup needed)
            let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1) as usize;

            let ptr = path_bytes.as_ptr();
            let expr = Expr {
                ptr: ptr.cast_mut(),
            };

            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some(bindings) = pattern_match(pattern, &atom) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();

                    // Store compressed result instead of expanding
                    results.push(MultiplicityMatch::new(instantiated, multiplicity));
                }
            }
        }

        drop(space);

        // 2. Also check large expression fallback PathMap (if allocated)
        let guard = self
            .shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            let btm = self.shared.btm.read().expect("btm lock poisoned");

            for (key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    // Look up multiplicity from main btm using the key
                    let multiplicity = get_multiplicity(&btm, &key).max(1) as usize;
                    results.push(MultiplicityMatch::new(instantiated, multiplicity));
                }
            }
        }

        results
    }

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
    ) -> Option<Vec<MultiplicityMatch>> {
        trace!(target: "mettatron::environment::match_space_query_multi", ?pattern, ?template);

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                return Some(Vec::new()); // Definitely no matches - return empty, not None
            }
        }

        let space = self.create_space();

        // Create conversion context to track variable mappings
        let mut ctx = ConversionContext::new();

        // Convert pattern to MORK bytes
        let pattern_bytes = match metta_to_mork_bytes(pattern, &space, &mut ctx) {
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
        let mut results: Vec<MultiplicityMatch> = Vec::new();

        mork::space::Space::query_multi(&space.btm, pattern_expr, |result, _matched_expr| {
            if let Err(mork_bindings) = result {
                // Convert MORK bindings to our format
                if let Ok(bindings) = mork_bindings_to_metta(&mork_bindings, &ctx, &space) {
                    // Apply bindings to template
                    let instantiated = apply_bindings(template, &bindings).into_owned();

                    // TODO: Get actual multiplicity from matched expression
                    // For now, use count=1 since query_multi doesn't expose multiplicity directly
                    results.push(MultiplicityMatch::new(instantiated, 1));
                }
            }
            true // Continue searching for ALL matches
        });

        // Also check large expression fallback PathMap
        let guard = self
            .shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            let btm = self.shared.btm.read().expect("btm lock poisoned");

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
        let guard = self
            .shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned");
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
        // With value-based multiplicity, every entry is an atom (no filtering needed)
        while rz.to_next_val() {
            let path_bytes = rz.path();
            let ptr = path_bytes.as_ptr();

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
