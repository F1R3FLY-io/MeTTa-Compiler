//! ArenaCompactTree (ACT) out-of-core persistence for the atom space.
//!
//! This module realizes the **Stage 5a** deliverable of the WAM-on-T0 / MM2
//! query-acceleration plan: out-of-core querying of large static fact sets,
//! keeping PathMap/MORK/MM2 as the atom space and adding the "scalable
//! architecture" lever the plan calls for (dataset > RAM).
//!
//! # What an ACT is
//!
//! [`pathmap::arena_compact::ArenaCompactTree`] is a *read-optimized*, contiguous,
//! memory-mappable serialization of a PathMap trie. Writing one (`dump_from_zipper`)
//! flattens a live trie into a single arena file; reading one (`open_mmap` +
//! `read_zipper*`) maps the file and walks it without deserializing into the heap —
//! the OS pages in only the bytes a query actually touches. This is exactly the
//! substrate MORK's own `(exec (I (ACT file pat) …) …)` source uses
//! (`MORK/kernel/src/sources.rs::ACTSource`).
//!
//! # Three operations
//!
//! - [`save_space_to_act`](MettaEnvironment::save_space_to_act): flatten the live
//!   `btm` (the literal-fact trie) to `/dev/shm/<name>.act`. **Multiplicity-faithful**:
//!   the per-leaf [`Multiplicity`]`(u64)` is written into the ACT's `u64` leaf value
//!   via `dump_from_zipper`'s `map_val`. No MORK/PathMap genericization is needed —
//!   `dump_from_zipper` is already `V`-generic.
//! - [`query_act`](MettaEnvironment::query_act): run a MeTTa pattern against the
//!   on-disk `<name>.act` **out-of-core** via MORK's `Space::<()>::query_multi_act` — a
//!   trie-pruned `ProductZipper` join over the mmap'd ACT for head-shaped patterns (with
//!   a mmap leaf-scan fallback for bare-variable / arity-≥64 patterns). Returns the same
//!   instantiated matches `match_space` would, so callers can transparently substitute an
//!   out-of-core fact set for an in-memory one. (The `(I (ACT …))` `query_multi_i` source
//!   form is *not* used — its `ASource::new` byte-matches inline symbol markers, which an
//!   interning build does not produce; see `query_act`'s docs and §3 of the design doc.)
//! - [`load_space_from_act`](MettaEnvironment::load_space_from_act): the reverse of
//!   save — mmap the ACT, decode each `(key, u64-multiplicity)` and re-add it,
//!   restoring a snapshot into the live space (fast big-KB cold-start).
//!
//! # Soundness: the symbol-mapping (sm) invariant — and how cross-run is handled
//!
//! `btm` keys are MORK `Expr` bytes whose symbol atoms reference the *saving* environment's
//! [`SharedMapping`](mork_interning::SharedMapping) (created per `GenericEnvironment::new()`,
//! shared across forks/clones). Decoding such a key therefore needs that same mapping.
//! `save_space_to_act` serializes it to `<name>.sm`; `query_act`/`load_space_from_act`
//! deserialize it (via [`MettaEnvironment::act_sm_for`], cached) to decode AND encode, so a
//! snapshot is fully usable **even in a different process run** (validated by
//! `act_btm_round_trips_cross_environment_via_saved_sm`). The trie-pruned join encodes the
//! query through the snapshot's mapping; this is faithful cross-run after the `mork_interning`
//! deserialize bucket-hash fix (regression-guarded by
//! `act_deserialized_sm_encodes_identically_to_saving_env`), so cross-run head-shaped queries
//! are trie-pruned, not scan-degraded. Wide facts (Wide MORK) are self-describing and need
//! no `sm`.
//!
//! # LSM tiered ACT-backed mutable space (DELIVERED — see [`super::act_tiered`])
//!
//! The "next layer" is now built: an immutable ACT base + a mutable in-memory overlay +
//! per-key tombstone SUPPRESSION COUNTS + `(compact-space!)` compaction, so a *live,
//! mutable* space is transparently backed by an out-of-core ACT base (ACT as the *primary*
//! store, not just an explicit snapshot/query target). `match_space` returns
//! `overlay ++ (base − tombstones)`, gated by a `has_act_base` fast-path flag so the
//! no-base hot path is byte-identical. Surface forms `(attach-act-base! "name")` /
//! `(detach-act-base!)` / `(compact-space! "name")`. See [`super::act_tiered`] and
//! `docs/mm2-integration/act-out-of-core.md` §4. The explicit `save`/`load`/`query` surface
//! in THIS module remains the snapshot/query model and does not touch the hot path.

use std::path::PathBuf;

use mork::space::{Space, ACT_PATH};
use mork_expr::{maybe_byte_item, Expr};
use pathmap::arena_compact::{ACTMmap, ArenaCompactTree};
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use tracing::trace;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use mork_interning::{SharedMapping, SharedMappingHandle};
use parking_lot::RwLock;

use super::multiplicity::Multiplicity;
use super::{MettaEnvironment, MettaValue};
use crate::backend::eval::{apply_bindings, pattern_match};
use crate::backend::models::metta_value_trait::MettaValueTrait;
use crate::backend::mork_convert::{mork_bindings_to_metta, with_mork_query_bytes};

/// Process-global cache of deserialized `<name>.sm` mappings, so a cross-run
/// `query_act`/`load_space_from_act` deserializes each `.sm` once rather than per call.
/// Keyed by ACT name → (the mapping, a dedicated cache-epoch for `with_mork_query_bytes`).
/// Invalidated for a name by `save_space_to_act` (the only writer of `<name>.sm`), so a
/// re-save is observed by all threads. `parking_lot::RwLock` matches the codebase idiom.
static ACT_SM_CACHE: OnceLock<RwLock<HashMap<String, (SharedMappingHandle, u64)>>> =
    OnceLock::new();

/// Dedicated cache-epoch source for deserialized `.sm` handles. `with_mork_query_bytes`
/// keys its thread-local symbol cache by epoch; a foreign `.sm` must use an epoch distinct
/// from any environment's (which grow from a low base) so the cache rebuilds for it rather
/// than serving env entries. Starting at `1 << 40` cannot collide with env epochs.
static ACT_SM_EPOCH: AtomicU64 = AtomicU64::new(1 << 40);

#[inline]
fn act_sm_cache() -> &'static RwLock<HashMap<String, (SharedMappingHandle, u64)>> {
    ACT_SM_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Drop any cached deserialized `<name>.sm` mapping, forcing the next `act_sm_for(name)` to
/// re-read the file. `save_space_to_act` does this for the name it writes; compaction
/// (`act_tiered::compact_space`) writes a TEMP name then renames over the live `<name>.sm`,
/// so it must separately invalidate the LIVE name's cache entry. `pub(super)` for that use.
pub(super) fn invalidate_act_sm_cache(name: &str) {
    if let Some(cache) = ACT_SM_CACHE.get() {
        cache.write().remove(name);
    }
}

/// The stored multiplicity of an exact fact key in an ACT (or 1 if absent / on a tag
/// error). The trie-pruned join walks the `()` ACT, so it recovers the matched fact's
/// `u64` leaf value via an O(depth) descend of the `u64` read-zipper — giving `query_act`
/// the same bag semantics as in-memory `match_space`.
///
/// `pub(super)` so the LSM-tiered read (`act_tiered.rs`) recovers the base leaf
/// multiplicity for the same bag semantics before applying the tombstone filter.
pub(super) fn act_leaf_multiplicity(tree: &ACTMmap, e: Expr) -> u64 {
    let first = unsafe { *e.ptr };
    if maybe_byte_item(first).is_err() {
        return 1;
    }
    // SAFETY: `e.ptr` points at valid MORK bytes (the matched origin path); `span()`
    // bounds the read to the expression's length.
    let span = unsafe { &*e.span() };
    let mut uz = tree.read_zipper_u64();
    if uz.descend_to_check(span) {
        uz.val().copied().unwrap_or(1).max(1)
    } else {
        1
    }
}

impl<V, F> super::core::GenericEnvironment<V, F>
where
    V: crate::backend::models::MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: crate::backend::models::MettaValueFactory<V> + Clone,
{
    /// Resolve the symbol mapping to use for decoding/encoding the `btm` of `<name>.act`.
    ///
    /// `btm` keys are interned with the *saving* environment's `SharedMapping`. When
    /// `save_space_to_act` ran, it also serialized that mapping to `<name>.sm`. If that file
    /// exists, deserialize it so the snapshot decodes correctly **even in a different process
    /// run**. If it is absent (a legacy `.act`, or a save without sm), fall back to this
    /// environment's own mapping — sound only intra-run. Wide facts need no `sm` (Wide MORK
    /// is self-describing).
    ///
    /// Returns the mapping (and its dedicated cache-epoch) for `<name>.act`'s `btm`. If
    /// `<name>.sm` exists, deserialize it (cached in [`ACT_SM_CACHE`], invalidated on
    /// re-save) so the snapshot decodes/encodes correctly **even in a different process
    /// run**. If absent (legacy `.act` / save without sm), fall back to this environment's
    /// mapping + epoch — sound only intra-run. Wide facts need no `sm` (Wide MORK is
    /// self-describing).
    ///
    /// Both decode (ID→symbol) and encode (symbol→ID, the trie-pruned join) are faithful
    /// through a deserialized mapping after the `mork_interning` deserialize bucket-hash fix.
    ///
    /// Generic (`impl<V,F>`) — its body touches only `self.shared_mapping` /
    /// `self.mork_cache_epoch` (both `V`-agnostic) plus the file/cache, so the LSM-tiered
    /// read (`act_tiered.rs`, also generic) and `query_act` (`MettaEnvironment`) share one
    /// implementation and one `<name>.sm` cache. `pub(super)` for the tiered read.
    pub(super) fn act_sm_for(&self, name: &str) -> (SharedMappingHandle, u64) {
        let sm_path = format!("{ACT_PATH}{name}.sm");
        if !std::path::Path::new(&sm_path).exists() {
            return (self.shared_mapping.clone(), self.mork_cache_epoch);
        }
        let cache = act_sm_cache();
        if let Some((sm, epoch)) = cache.read().get(name) {
            return (sm.clone(), *epoch);
        }
        match SharedMapping::deserialize(&sm_path) {
            Ok(sm) => {
                let epoch = ACT_SM_EPOCH.fetch_add(1, Ordering::Relaxed);
                cache.write().insert(name.to_string(), (sm.clone(), epoch));
                (sm, epoch)
            }
            // Corrupt/unreadable `.sm` → fall back to the env mapping (intra-run only).
            Err(_) => (self.shared_mapping.clone(), self.mork_cache_epoch),
        }
    }
}

impl MettaEnvironment {
    /// Flatten the live literal-fact trie (`btm`) to an out-of-core ACT snapshot at
    /// `/dev/shm/<name>.act`, preserving per-atom multiplicity.
    ///
    /// The destination directory matches MORK's `ACT_PATH` so the resulting file is
    /// directly queryable via [`query_act`](Self::query_act) and MORK's own
    /// `(ACT <name> …)` source. Returns the written path.
    ///
    /// Multiplicity is carried into the ACT's `u64` leaf value through
    /// `dump_from_zipper`'s `map_val` closure (`|m| m.0`); no MORK/PathMap
    /// genericization is required because `dump_from_zipper` is already `V`-generic.
    ///
    /// Both fact tries are snapshotted: `btm` (arity < 64) → `<name>.act`, and — only when
    /// non-empty — `wide_btm` (arity ≥ 64, Wide MORK encoding) → `<name>.wide.act`. Rules
    /// `(= lhs rhs)` and type assertions `(: x T)` live in `btm` and so are captured too
    /// (`load_space_from_act` re-routes them via `add_to_space`). The derived
    /// type/subtype/inferred caches are rebuilt from those assertions on load, so they are
    /// not separately serialized.
    pub fn save_space_to_act(&self, name: &str) -> std::io::Result<PathBuf> {
        let space = self.create_space();
        let path = format!("{ACT_PATH}{name}.act");
        trace!(target: "mettatron::act::save", name, %path);
        // `map_val` carries the Multiplicity(u64) into the ACT's u64 leaf value.
        ArenaCompactTree::dump_from_zipper(space.btm.read_zipper(), |m: &Multiplicity| m.0, &path)?;

        // Wide expressions (arity ≥ 64) → a sibling `<name>.wide.act`, only if any exist.
        let wide = self.shared.atom_space.wide_btm.read();
        if !wide.is_empty() {
            let wide_path = format!("{ACT_PATH}{name}.wide.act");
            trace!(target: "mettatron::act::save", name, %wide_path, "wide");
            ArenaCompactTree::dump_from_zipper(
                wide.read_zipper(),
                |m: &Multiplicity| m.0,
                &wide_path,
            )?;
        }

        // Serialize the symbol mapping → `<name>.sm` so the (interned) `btm` snapshot
        // decodes AND encodes (the cross-run trie-pruned join) in a *different* process run
        // too (see `act_sm_for`). Wide facts need no mapping. This is what makes the
        // persistence genuinely cross-run.
        let sm_path = format!("{ACT_PATH}{name}.sm");
        self.shared_mapping.serialize(&sm_path)?;
        // Invalidate any cached deserialized mapping for this name — the `.sm` just changed.
        if let Some(cache) = ACT_SM_CACHE.get() {
            cache.write().remove(name);
        }

        Ok(PathBuf::from(path))
    }

    /// Query an on-disk ACT snapshot **out-of-core** with a MeTTa pattern, returning the
    /// `template` instantiated for each match — the **same multiset** in-memory
    /// `match_space(pattern, template)` produces, including per-fact multiplicity (bag
    /// semantics). The stored multiplicity rides in the ACT's `u64` leaf and is recovered
    /// on both paths: read directly at each leaf on the scan; recovered by an O(depth)
    /// `u64`-zipper descend to the matched key on the trie-pruned join.
    ///
    /// The ACT at `/dev/shm/<name>.act` is memory-mapped; the OS faults in only the trie
    /// pages a query touches, and the whole KB is never resident in the heap (it lives in
    /// reclaimable page cache). Two paths, both interning-correct:
    ///
    /// 1. **Trie-pruned ProductZipper join** (`Space::<()>::query_multi_act`) for a
    ///    head-shaped pattern of arity 1..64 — the MM2 out-of-core fast path. The pattern
    ///    is wrapped as the 1-conjunct `(, pattern)` and joined against the mmap'd ACT trie;
    ///    the `ProductZipper` descends only matching byte-prefixes (O(matches), not
    ///    O(|act|)). Bindings arrive at namespace 0 and are recovered by
    ///    `mork_bindings_to_metta` — exactly as the in-memory `match_space` MM2 fast path
    ///    (`match_space_btm_query_multi`) does.
    /// 2. **Leaf scan + MeTTaTron-side unification** as a universal fallback (bare-variable
    ///    or arity-≥64 patterns, or an encoding failure): decode each stored fact via the
    ///    snapshot's `sm` and unify against `pattern`.
    ///
    /// Both paths decode/encode with the snapshot's OWN mapping (`<name>.sm`, via
    /// `act_sm_for`), so this is correct **cross-run** — a fresh process decodes another
    /// run's snapshot, and (after the `mork_interning` deserialize fix) the trie-pruned join
    /// encodes faithfully through the deserialized mapping too. Intra-run falls back to the
    /// env's mapping when no `<name>.sm` exists.
    ///
    /// # Why not `query_multi_i`'s `(I (ACT name pat))` source form
    ///
    /// That form's `ASource::new` byte-matches **inline** symbol markers
    /// (`[SymbolSize(3)]ACT`). MeTTaTron builds MORK with the `interning` feature, so
    /// markers are interned IDs, not inline bytes, and the dispatch hits `unreachable!()`.
    /// `query_multi_act` sidesteps it by taking the (interned) conjunct pattern directly and
    /// matching the (interned) ACT facts — no inline markers. See
    /// `docs/mm2-integration/act-out-of-core.md`.
    ///
    /// Returns an empty vector if the ACT file is absent or unreadable.
    pub fn query_act(
        &self,
        name: &str,
        pattern: &MettaValue,
        template: &MettaValue,
    ) -> Vec<MettaValue> {
        trace!(target: "mettatron::act::query", name);
        // Decode AND encode the `btm` path with the snapshot's OWN mapping (`<name>.sm` if
        // present; else the env's, intra-run). After the `mork_interning` deserialize
        // bucket-hash fix a deserialized mapping encodes faithfully, so the trie-pruned join
        // works cross-run too — no scan-only degradation for cross-run head-shaped queries.
        let (act_sm, act_epoch) = self.act_sm_for(name);
        let mut results: Vec<MettaValue> = Vec::new();

        // ── btm (arity < 64): trie-pruned ProductZipper join for head-shaped, arity-1..64
        //    patterns; else a universal leaf scan. Both decode via the snapshot's mapping. ──
        if let Ok(tree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.act")) {
            // Decode space carries the snapshot's mapping (for the join's binding recovery
            // and the scan's fact decode).
            let mut scan_space = self.create_space();
            scan_space.sm = act_sm.clone();

            let eligible =
                pattern.get_head_symbol().is_some() && (1..64).contains(&pattern.get_arity());
            let mut from_join = false;
            if eligible {
                let conj = MettaValue::Conjunction(vec![pattern.clone()]);
                let joined = with_mork_query_bytes(&conj, &act_sm, act_epoch, |bytes, ctx| {
                    let conj_expr = Expr {
                        ptr: bytes.as_ptr().cast_mut(),
                    };
                    let mut out: Vec<MettaValue> = Vec::new();
                    Space::<()>::query_multi_act(&tree, conj_expr, |res, matched_expr| {
                        if let Err(mork_bindings) = res {
                            if let Ok(bindings) =
                                mork_bindings_to_metta(&mork_bindings, ctx, &scan_space)
                            {
                                let instantiated = apply_bindings(template, &bindings).into_owned();
                                // Bag semantics (matching `match_space`): one copy per unit
                                // of the matched fact's stored multiplicity, recovered by an
                                // O(depth) descend of the u64 read-zipper.
                                let mult = act_leaf_multiplicity(&tree, matched_expr);
                                for _ in 0..mult {
                                    out.push(instantiated.clone());
                                }
                            }
                        }
                        true // collect ALL matches
                    });
                    out
                });
                // The join encodes faithfully (intra- and cross-run), so its result is
                // authoritative — an empty result is a genuine no-match. Only an *encoding
                // failure* (Err) falls through to the universal scan.
                if let Ok(r) = joined {
                    results = r;
                    from_join = true;
                }
            }
            if !from_join {
                // Universal leaf scan (bare-variable / arity-≥64 patterns, or encode
                // failure). u64 zipper → bag semantics; decode via the snapshot's mapping.
                let mut rz = tree.read_zipper_u64();
                while rz.to_next_val() {
                    let path_bytes = rz.path();
                    if path_bytes.is_empty() {
                        continue;
                    }
                    // Guard reserved first bytes (0x40-0x7F) which would panic the decoder.
                    if maybe_byte_item(path_bytes[0]).is_err() {
                        continue;
                    }
                    let mult = rz.val().copied().unwrap_or(1).max(1);
                    let expr = Expr {
                        ptr: path_bytes.as_ptr().cast_mut(),
                    };
                    let Ok(fact) = Self::mork_expr_to_metta_value(&expr, &scan_space) else {
                        continue;
                    };
                    if let Some(bindings) = pattern_match(pattern, &fact) {
                        let instantiated = apply_bindings(template, &bindings).into_owned();
                        for _ in 0..mult {
                            results.push(instantiated.clone());
                        }
                    }
                }
            }
        }

        // ── wide_btm (arity ≥ 64): leaf scan. Wide MORK encoding is sm-independent, so
        //    `wide_bytes_to_generic_value` decodes without the symbol mapping. ──
        if let Ok(wtree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.wide.act")) {
            let factory = crate::backend::models::global_factory();
            let mut wrz = wtree.read_zipper_u64();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                if path_bytes.is_empty() {
                    continue;
                }
                let mult = wrz.val().copied().unwrap_or(1).max(1);
                if let Ok(fact) = crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                    MettaValue,
                    _,
                >(path_bytes, &factory)
                {
                    if let Some(bindings) = pattern_match(pattern, &fact) {
                        let instantiated = apply_bindings(template, &bindings).into_owned();
                        for _ in 0..mult {
                            results.push(instantiated.clone());
                        }
                    }
                }
            }
        }

        results
    }

    /// Restore an ACT snapshot into the live space (fast big-KB cold-start), preserving
    /// each atom's multiplicity. Returns the number of `add_to_space` insertions performed
    /// (Σ multiplicities).
    ///
    /// Decodes the `btm` path via the snapshot's own `sm` (`<name>.sm` if present), so it
    /// restores correctly **cross-run** (a fresh environment); re-adding via `add_to_space`
    /// re-interns into this environment's mapping. Wide facts decode without any `sm`.
    pub fn load_space_from_act(&mut self, name: &str) -> std::io::Result<usize> {
        let path = format!("{ACT_PATH}{name}.act");
        trace!(target: "mettatron::act::load", name, %path);
        let tree = ArenaCompactTree::open_mmap(&path)?;
        // Decode `btm` with the snapshot's own mapping (cross-run capable); `add_to_space`
        // below re-interns into this environment's mapping. (Decode-only — epoch unused.)
        let (act_sm, _) = self.act_sm_for(name);
        let mut space = self.create_space();
        space.sm = act_sm;

        // Phase 1: decode (immutable borrow of `tree`/`space`); collect owned values.
        let mut decoded: Vec<(MettaValue, u64)> = Vec::new();
        {
            let mut rz = tree.read_zipper_u64();
            while rz.to_next_val() {
                let path_bytes = rz.path();
                if path_bytes.is_empty() {
                    continue;
                }
                // Guard against reserved first bytes (0x40-0x7F) which would panic
                // ExprZipper::new inside the decoder.
                let first_byte = path_bytes[0];
                if maybe_byte_item(first_byte).is_err() {
                    continue;
                }
                let expr = Expr {
                    ptr: path_bytes.as_ptr().cast_mut(),
                };
                let mult = rz.val().copied().unwrap_or(1).max(1);
                if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                    decoded.push((value, mult));
                }
            }
        }

        // Phase 2: re-add btm (mutable borrow of `self`); multiplicity = repeated add.
        // `add_to_space` re-routes rules `(= …)` and type assertions `(: …)` to their
        // registries, rebuilding the derived type caches.
        let mut count = 0usize;
        for (value, mult) in decoded {
            for _ in 0..mult {
                self.add_to_space(&value);
                count += 1;
            }
        }

        // Phase 3: wide facts (arity ≥ 64) from `<name>.wide.act`, if present. Wide MORK
        // encoding is sm-independent, so decode needs only the factory. `add_to_space`
        // routes arity-≥64 values back to `wide_btm`.
        if let Ok(wtree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.wide.act")) {
            let factory = crate::backend::models::global_factory();
            let mut wdecoded: Vec<(MettaValue, u64)> = Vec::new();
            {
                let mut wrz = wtree.read_zipper_u64();
                while wrz.to_next_val() {
                    let path_bytes = wrz.path();
                    if path_bytes.is_empty() {
                        continue;
                    }
                    let mult = wrz.val().copied().unwrap_or(1).max(1);
                    if let Ok(value) =
                        crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                            MettaValue,
                            _,
                        >(path_bytes, &factory)
                    {
                        wdecoded.push((value, mult));
                    }
                }
            }
            for (value, mult) in wdecoded {
                for _ in 0..mult {
                    self.add_to_space(&value);
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Build a ground S-expression `(p0 p1 …)` from symbols.
    fn sym_sexpr(parts: &[&str]) -> MettaValue {
        MettaValue::SExpr(parts.iter().map(|p| MettaValue::Atom(*p)).collect())
    }

    /// Regression for the `mork_interning` deserialize bucket-hash fix: a deserialized
    /// `<name>.sm` must ENCODE a pattern to the same bytes the saving environment produced
    /// (not merely decode) — the property the cross-run trie-pruned join depends on. Before
    /// the fix, deserialize bucketed symbols by the wrong slice, so encode re-interned every
    /// symbol at a fresh id and the bytes diverged.
    #[test]
    fn act_deserialized_sm_encodes_identically_to_saving_env() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        let name = unique_name("smencode");
        let _c = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");
        let act_sm = SharedMapping::deserialize(format!("{ACT_PATH}{name}.sm")).expect("deser");

        let pattern = sym_sexpr(&["parent", "alice", "bob"]);
        let env_bytes = with_mork_query_bytes(
            &pattern,
            &env.shared_mapping,
            env.mork_cache_epoch,
            |b, _| b.to_vec(),
        )
        .unwrap();
        let act_bytes =
            with_mork_query_bytes(&pattern, &act_sm, 1 << 40, |b, _| b.to_vec()).unwrap();
        assert_eq!(
            env_bytes, act_bytes,
            "deserialized sm must encode identically to the saving env's sm"
        );
    }

    /// Unique ACT base name per call — avoids `/dev/shm` collisions across the
    /// (parallel) test runner and repeated runs.
    fn unique_name(tag: &str) -> String {
        static CTR: AtomicU64 = AtomicU64::new(0);
        format!(
            "mettatron_act_{}_{}_{}",
            tag,
            std::process::id(),
            CTR.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// RAII removal of all `/dev/shm/<name>.*` artifacts (btm, wide, sm).
    struct ActCleanup(String);
    impl Drop for ActCleanup {
        fn drop(&mut self) {
            for suffix in [".act", ".wide.act", ".sm"] {
                let _ = std::fs::remove_file(format!("{ACT_PATH}{}{suffix}", self.0));
            }
        }
    }

    /// Order-insensitive comparison key for a result multiset.
    fn normalized(values: &[MettaValue]) -> Vec<String> {
        let mut v: Vec<String> = values.iter().map(|x| format!("{x:?}")).collect();
        v.sort();
        v
    }

    /// The headline guarantee: an out-of-core ACT query returns exactly what the
    /// in-memory `match_space` returns, with the in-memory `btm` empty during the
    /// query (the match set comes entirely from the mmap'd `.act`).
    #[test]
    fn act_out_of_core_query_matches_in_memory_match_space() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        env.add_to_space(&sym_sexpr(&["parent", "bob", "carol"]));
        env.add_to_space(&sym_sexpr(&["parent", "carol", "dan"]));
        env.add_to_space(&sym_sexpr(&["color", "sky", "blue"]));

        let name = unique_name("query");
        let _cleanup = ActCleanup(name.clone());

        let path = env
            .save_space_to_act(&name)
            .expect("save_space_to_act failed");
        assert!(path.exists(), "ACT file should exist at {path:?}");

        let pattern = sym_sexpr(&["parent", "$x", "$y"]);

        // In-memory reference (queries the live btm).
        let in_memory: Vec<MettaValue> = env
            .match_space(&pattern, &pattern)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();

        // Out-of-core: mmap the .act, query with an empty in-memory tier.
        let out_of_core = env.query_act(&name, &pattern, &pattern);

        assert_eq!(
            normalized(&out_of_core),
            normalized(&in_memory),
            "out-of-core ACT query must equal in-memory match_space\n ooc={out_of_core:?}\n mem={in_memory:?}"
        );
        assert_eq!(
            out_of_core.len(),
            3,
            "expected 3 (parent _ _) matches, got {out_of_core:?}"
        );
    }

    /// A non-identity template still projects correctly out-of-core (bindings are
    /// recovered by MeTTaTron-side re-unification, independent of MORK source
    /// binding namespaces).
    #[test]
    fn act_out_of_core_query_projects_template() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["edge", "a", "b"]));
        env.add_to_space(&sym_sexpr(&["edge", "b", "c"]));

        let name = unique_name("project");
        let _cleanup = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");

        let pattern = sym_sexpr(&["edge", "$from", "$to"]);
        let template = sym_sexpr(&["reachable", "$to", "$from"]); // swapped projection
        let got = env.query_act(&name, &pattern, &template);

        let expected = vec![
            sym_sexpr(&["reachable", "b", "a"]),
            sym_sexpr(&["reachable", "c", "b"]),
        ];
        assert_eq!(normalized(&got), normalized(&expected), "got {got:?}");
    }

    /// Contract: `query_act` is **bag-faithful** — it emits one copy per unit of the
    /// matched fact's stored multiplicity, exactly matching in-memory `match_space`'s
    /// multiset (the multiplicity rides in the ACT's `u64` leaf and is recovered on the
    /// query path: directly on the scan, via an O(depth) descend on the join).
    #[test]
    fn act_query_is_bag_faithful_for_high_multiplicity() {
        let mut env = MettaEnvironment::default();
        let fact = sym_sexpr(&["dup", "x"]);
        env.add_to_space(&fact);
        env.add_to_space(&fact);
        env.add_to_space(&fact); // multiplicity 3

        let name = unique_name("bagsem");
        let _cleanup = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");

        let pattern = sym_sexpr(&["dup", "$v"]);

        // in-memory match_space expands by multiplicity → 3 copies.
        let in_memory: Vec<MettaValue> = env
            .match_space(&pattern, &pattern)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(
            in_memory.len(),
            3,
            "match_space is bag (multiplicity-expanded)"
        );

        // out-of-core query is now bag-faithful → 3 copies, equal to match_space.
        let out_of_core = env.query_act(&name, &pattern, &pattern);
        assert_eq!(
            out_of_core.len(),
            3,
            "query_act must be bag-faithful (== match_space multiset), got {out_of_core:?}"
        );
    }

    /// Save preserves per-atom multiplicity in the ACT's u64 leaf, and load restores
    /// it (Σ multiplicity insertions). Round-trips within the same environment (sm).
    #[test]
    fn act_save_load_round_trip_preserves_multiplicity() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["fact", "a"]));
        env.add_to_space(&sym_sexpr(&["fact", "b"]));
        let dup = sym_sexpr(&["fact", "dup"]);
        env.add_to_space(&dup);
        env.add_to_space(&dup);
        env.add_to_space(&dup); // multiplicity 3

        let name = unique_name("roundtrip");
        let _cleanup = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");

        // load re-adds the snapshot into the same env; the count returned is the
        // Σ multiplicity over the snapshot = 1 + 1 + 3 = 5.
        let total_added = env.load_space_from_act(&name).expect("load");
        assert_eq!(total_added, 5, "Σ multiplicity over snapshot should be 5");

        // dup multiplicity is now 3 (original) + 3 (restored) = 6 — proving the ACT
        // leaf carried the count 3, not a flattened 1.
        assert_eq!(
            env.get_atom_multiplicity(&dup),
            6,
            "multiplicity 3 must survive the ACT round-trip"
        );
    }

    /// A query that matches nothing returns empty (no panic on an absent subpattern).
    #[test]
    fn act_out_of_core_query_no_match_is_empty() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        let name = unique_name("nomatch");
        let _cleanup = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");

        let pattern = sym_sexpr(&["sibling", "$x", "$y"]);
        let got = env.query_act(&name, &pattern, &pattern);
        assert!(got.is_empty(), "no (sibling _ _) facts, got {got:?}");
    }

    /// Wide expressions (arity ≥ 64) are snapshotted to the `<name>.wide.act` sibling
    /// (Wide MORK encoding) and round-trip through both query and load. Wide decode is
    /// sm-independent, so a wide-only snapshot even loads into a fresh environment.
    #[test]
    fn act_wide_arity_fact_round_trips_and_queries() {
        let mut env = MettaEnvironment::default();
        // arity 70 (> 63) → stored in wide_btm via Wide MORK encoding.
        let mut parts: Vec<MettaValue> = Vec::with_capacity(70);
        parts.push(MettaValue::Atom("wide"));
        for i in 0..69 {
            parts.push(MettaValue::Long(i));
        }
        let wide_fact = MettaValue::SExpr(parts);
        env.add_to_space(&wide_fact);

        let name = unique_name("wide");
        let _cleanup = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");

        // the `.wide.act` sibling must be written.
        assert!(
            std::path::Path::new(&format!("{ACT_PATH}{name}.wide.act")).exists(),
            "wide ACT sibling should be written"
        );

        // out-of-core query finds the wide fact (arity ≥ 64 → scan path covers wide_btm).
        let got = env.query_act(&name, &wide_fact, &wide_fact);
        assert_eq!(
            got.len(),
            1,
            "wide fact should match out-of-core, got {got:?}"
        );

        // wide decode is sm-independent → a fresh environment restores it.
        let mut env2 = MettaEnvironment::default();
        let n = env2.load_space_from_act(&name).expect("load");
        assert_eq!(n, 1, "exactly one wide fact restored");
        assert_eq!(
            env2.query_act(&name, &wide_fact, &wide_fact).len(),
            1,
            "wide fact queryable after cross-env restore"
        );
    }

    /// Cross-run persistence: `btm` facts are interned with the saving environment's
    /// `SharedMapping`, which `save_space_to_act` serializes to `<name>.sm`. A FRESH
    /// environment (independent mapping, simulating a different process run) must decode
    /// them via that sidecar — both on `load_space_from_act` (pure decode) and on
    /// `query_act` (the sm-faithful scan fallback). This is what makes the persistence
    /// genuinely cross-run rather than intra-run-only.
    #[test]
    fn act_btm_round_trips_cross_environment_via_saved_sm() {
        let mut env1 = MettaEnvironment::default();
        env1.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        env1.add_to_space(&sym_sexpr(&["parent", "bob", "carol"]));
        let name = unique_name("crossrun");
        let _cleanup = ActCleanup(name.clone());
        env1.save_space_to_act(&name).expect("save");
        assert!(
            std::path::Path::new(&format!("{ACT_PATH}{name}.sm")).exists(),
            "the .sm sidecar must be written for cross-run decode"
        );

        let pattern = sym_sexpr(&["parent", "$x", "$y"]);

        // (a) A fresh environment LOADS the snapshot — pure decode via the serialized sm.
        let mut env2 = MettaEnvironment::default();
        let n = env2.load_space_from_act(&name).expect("cross-env load");
        assert_eq!(n, 2, "two btm facts restored cross-env, got {n}");
        let in_mem: Vec<MettaValue> = env2
            .match_space(&pattern, &pattern)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(
            in_mem.len(),
            2,
            "restored facts must be queryable in the fresh env, got {in_mem:?}"
        );

        // (b) A fresh environment QUERIES the snapshot out-of-core via the trie-pruned join.
        //     Give env3 a divergent symbol history first, so its OWN mapping cannot encode
        //     env1's facts — proving the join encodes through the saved `<name>.sm` (not
        //     env3's mapping) and so works cross-run.
        let mut env3 = MettaEnvironment::default();
        env3.add_to_space(&sym_sexpr(&["decoy", "zzz", "qqq", "www"]));
        let got = env3.query_act(&name, &pattern, &pattern);
        let strs: Vec<String> = got.iter().map(|v| format!("{v:?}")).collect();
        assert_eq!(
            got.len(),
            2,
            "cross-env out-of-core query must resolve via the saved sm, got {strs:?}"
        );
        assert!(strs.iter().all(|s| s.contains("parent")));
    }

    /// `btm` holds not just plain facts but also rules `(= lhs rhs)` (De-Bruijn-encoded) and
    /// type assertions `(: x T)`. This verifies the save-doc claim that all three round-trip:
    /// a fresh environment that loads the snapshot has the rule fire and the type resolve.
    #[test]
    fn act_save_load_round_trips_rules_and_type_assertions() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["fact", "a"]));
        let rule = MettaValue::SExpr(vec![
            MettaValue::Atom("="),
            sym_sexpr(&["double", "$x"]),
            sym_sexpr(&["pair", "$x", "$x"]),
        ]);
        env.add_to_space(&rule);
        let type_decl = MettaValue::SExpr(vec![
            MettaValue::Atom(":"),
            MettaValue::Atom("foo"),
            MettaValue::Atom("Bar"),
        ]);
        env.add_to_space(&type_decl);

        let name = unique_name("rulety");
        let _cleanup = ActCleanup(name.clone());
        env.save_space_to_act(&name).expect("save");

        // Restore into a FRESH environment (independent sm → exercises the .sm decode path).
        let mut env2 = MettaEnvironment::default();
        env2.load_space_from_act(&name).expect("load");

        // The plain fact, the rule, and the type assertion are all present in btm.
        assert!(
            env2.has_sexpr_fact(&sym_sexpr(&["fact", "a"])),
            "plain fact must round-trip"
        );
        assert!(
            env2.has_sexpr_fact(&rule),
            "rule (= …) must round-trip (De-Bruijn decode + re-route through add_rule)"
        );
        assert!(
            env2.has_sexpr_fact(&type_decl),
            "type assertion (: x T) must round-trip"
        );
    }
}
