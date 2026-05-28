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

use mork_expr::{maybe_byte_item, Expr};
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use tracing::trace;

use super::multiplicity::get_multiplicity;
use super::{MettaEnvironment, MettaValue};
use crate::backend::eval::{apply_bindings, pattern_match};
use crate::backend::models::metta_value_trait::MettaValueTrait;
use crate::backend::mork_convert::{mork_bindings_to_metta, with_mork_query_bytes};

// Re-export the generic MultiplicityMatch specialized for MettaValue
pub use super::core::MultiplicityMatch;

impl MettaEnvironment {
    // Note: match_space() is now a generic method on GenericEnvironment<V, F> in core.rs.
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

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist in
        // the OVERLAY. Skip the short-circuit when an ACT base is attached (it may match a
        // head absent from the overlay bloom). The `has_act_base` load is on the bloom-miss
        // branch only, so the hot path is unchanged. See `match_space`.
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            // parking_lot::RwLock - no .expect()
            let bloom_result = self
                .shared
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(expected_head, pattern_arity);
            if !bloom_result
                && !self
                    .shared
                    .atom_space
                    .has_act_base
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                return Some(Vec::new()); // Definitely no matches - return empty, not None
            }
        }

        let space = self.create_space();

        // Convert pattern to MORK query bytes and run query_multi in callback
        let query_result = with_mork_query_bytes(
            pattern,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |pattern_bytes, ctx| {
                let pattern_expr = Expr {
                    ptr: pattern_bytes.as_ptr().cast_mut(),
                };

                // Collect matches using MORK's native query_multi
                let mut results: Vec<MultiplicityMatch<MettaValue>> = Vec::new();

                mork::space::Space::query_multi(
                    &space.btm,
                    pattern_expr,
                    |result, matched_expr| {
                        if let Err(mork_bindings) = result {
                            // Convert MORK bindings to our format
                            if let Ok(bindings) =
                                mork_bindings_to_metta(&mork_bindings, ctx, &space)
                            {
                                // Apply bindings to template
                                let instantiated = apply_bindings(template, &bindings).into_owned();

                                // Extract multiplicity from the matched expression's PathMap path.
                                // matched_expr.span() returns *const [u8] — the serialized MORK bytes
                                // that form the exact PathMap key for this entry. We look up the
                                // multiplicity in the same PathMap that query_multi is traversing.
                                // SAFETY: matched_expr.ptr points to valid MORK bytes within PathMap
                                // memory. The span() traversal is bounded by the expression's length.

                                // Validate matched_expr starts with a valid MORK tag before calling
                                // span() — span() uses ExprZipper::new() which calls byte_item()
                                // and panics on reserved bytes (0x40-0x7F).
                                let first_byte = unsafe { *matched_expr.ptr };
                                if let Err(reserved) = maybe_byte_item(first_byte) {
                                    tracing::warn!(
                                        target: "mettatron::match_space_query_multi",
                                        "Matched expr has reserved first byte 0x{:02x}, skipping",
                                        reserved
                                    );
                                    return true; // Continue searching
                                }

                                let mork_bytes = unsafe { &*matched_expr.span() };
                                let multiplicity =
                                    get_multiplicity(&space.btm, mork_bytes).max(1) as usize;

                                results.push(MultiplicityMatch::new(instantiated, multiplicity));
                            }
                        }
                        true // Continue searching for ALL matches
                    },
                );

                results
            },
        );

        let mut results = match query_result {
            Ok(r) => r,
            Err(_) => {
                // Conversion failed (e.g., arity too high) - return None to trigger fallback
                return None;
            }
        };

        // Also check wide expression PathMap (arity >= 64, Wide MORK encoding)
        // Uses byte-level pre-filter via wide_extract_data() to avoid costly
        // MettaValue reconstruction for non-matches.
        {
            // Encode pattern to Wide MORK De Bruijn bytes (once)
            let mut pattern_ctx = crate::backend::wide_mork::encoding::WideConversionContext::new();
            let mut pattern_debruijn = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_debruijn(
                pattern,
                &mut pattern_ctx,
                &mut pattern_debruijn,
            );

            let wbtm = self.shared.atom_space.wide_btm.read();
            let mut wrz = wbtm.read_zipper();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                let multiplicity = wrz.val().map(|m| m.count()).unwrap_or(1) as usize;

                // Byte-level structural match (same algorithm as MORK's extract_data)
                if crate::backend::wide_mork::extract::wide_extract_data(
                    &pattern_debruijn,
                    path_bytes,
                )
                .is_ok()
                {
                    // Match succeeded — reconstruct value and extract bindings
                    if let Ok(stored_value) =
                        crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                            MettaValue,
                            _,
                        >(
                            path_bytes, &crate::backend::models::global_factory()
                        )
                    {
                        if let Some(bindings) = pattern_match(pattern, &stored_value) {
                            let instantiated = apply_bindings(template, &bindings).into_owned();
                            results.push(MultiplicityMatch::new(instantiated, multiplicity));
                        }
                    }
                }
            }
        }

        // LSM-tiered base: append the attached ACT base matches MINUS tombstones, overlay-
        // first, mirroring `match_space`. (Kept consistent even though this method currently
        // has no hot-path callers, so any future use stays tier-correct.) Gated by the
        // relaxed-acquire `has_act_base` load → no-base path byte-identical.
        if self
            .shared
            .atom_space
            .has_act_base
            .load(std::sync::atomic::Ordering::Acquire)
        {
            results.extend(self.match_space_base(pattern, template));
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
        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist in
        // the OVERLAY. Skip the short-circuit when an ACT base is attached (it may match a
        // head absent from the overlay bloom). The `has_act_base` load is on the bloom-miss
        // branch only, so the hot path is unchanged. See `match_space`.
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            // parking_lot::RwLock - no .expect()
            if !self
                .shared
                .atom_space
                .head_arity_bloom
                .read()
                .may_contain(expected_head, pattern_arity)
                && !self
                    .shared
                    .atom_space
                    .has_act_base
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                // Definitely no matching expressions exist (overlay) and no base attached.
                return None;
            }
        }

        // Stage 2: MM2 trie-pruned first-match early-exit for ground spaces (non-`=`
        // patterns). `query_multi` returns the lex-byte-FIRST match (O(depth)) via the
        // streaming callback's `false` return, instead of the linear early-exit scan
        // below. Both walk byte order, so the first match agrees. Rules (`=`, De-Bruijn,
        // not counted by `variable_fact_count`) and non-ground spaces fall through.
        let ground_space = self
            .shared
            .atom_space
            .variable_fact_count
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
            && self.shared.atom_space.variable_atoms.read().is_empty();
        let mut btm_done = false;
        if ground_space {
            if let Some(head) = pattern.get_head_symbol() {
                let arity = pattern.get_arity();
                if head != "=" && arity > 0 && arity < 64 {
                    let conj = MettaValue::Conjunction(vec![pattern.clone()]);
                    let space = self.create_space();
                    let found = with_mork_query_bytes(
                        &conj,
                        &self.shared_mapping,
                        self.mork_cache_epoch,
                        |bytes, ctx| {
                            let conj_expr = Expr {
                                ptr: bytes.as_ptr().cast_mut(),
                            };
                            let mut hit: Option<MettaValue> = None;
                            mork::space::Space::query_multi(&space.btm, conj_expr, |res, _m| {
                                if let Err(b) = res {
                                    if let Ok(binds) = mork_bindings_to_metta(&b, ctx, &space) {
                                        hit = Some(apply_bindings(template, &binds).into_owned());
                                        return false; // first match — stop the trie walk
                                    }
                                }
                                true
                            });
                            hit
                        },
                    );
                    match found {
                        Ok(Some(v)) => return Some(v),
                        Ok(None) => btm_done = true, // ran; no btm match → skip linear, check wide
                        Err(_) => {}                 // encode failure → linear fallback
                    }
                }
            }
        }

        if !btm_done {
            let space = self.create_space();
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
        }

        // 2. Check wide expression PathMap (arity >= 64, Wide MORK encoding)
        // Uses byte-level pre-filter via wide_extract_data() for early rejection.
        {
            let mut pattern_ctx = crate::backend::wide_mork::encoding::WideConversionContext::new();
            let mut pattern_debruijn = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_debruijn(
                pattern,
                &mut pattern_ctx,
                &mut pattern_debruijn,
            );

            let wbtm = self.shared.atom_space.wide_btm.read();
            let mut wrz = wbtm.read_zipper();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                // Byte-level pre-filter
                if crate::backend::wide_mork::extract::wide_extract_data(
                    &pattern_debruijn,
                    path_bytes,
                )
                .is_ok()
                {
                    if let Ok(stored_value) =
                        crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                            MettaValue,
                            _,
                        >(
                            path_bytes, &crate::backend::models::global_factory()
                        )
                    {
                        if let Some(bindings) = pattern_match(pattern, &stored_value) {
                            let instantiated = apply_bindings(template, &bindings).into_owned();
                            return Some(instantiated); // EARLY EXIT
                        }
                    }
                }
            }
        }

        // LSM-tiered base: overlay (in-memory) had no match — return the FIRST surviving
        // match from the attached ACT base (`base − tombstones`). `match_space_base`
        // preserves overlay-first ordering and bag multiplicity; we take the first
        // instantiated value. Gated by the relaxed-acquire `has_act_base` load so the
        // no-base hot path stays byte-identical.
        if self
            .shared
            .atom_space
            .has_act_base
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if let Some(m) = self.match_space_base(pattern, template).into_iter().next() {
                return Some(m.value);
            }
        }

        None
    }

    // Note: match_space_exists() is now a generic method on GenericEnvironment<V, F> in generic.rs.
    // The implementation uses MORK PathMap for storage and supports any MettaValueTrait type.
}
