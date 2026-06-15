//! LSM-tiered ACT-backed mutable atom space (the "next layer" of Stage 5a).
//!
//! This module turns an out-of-core ArenaCompactTree (ACT) snapshot from
//! [`act_persistence`](super::act_persistence) into the **primary store** of a
//! *live, mutable* atom space, via a classic Log-Structured-Merge tiering:
//!
//! ```text
//!   match_space(pattern)
//!        │
//!        ├── overlay  (the in-memory `btm`/`wide_btm` — fresh adds/removes)
//!        │
//!        └── base − tombstones  (the mmap'd `<name>.act` − per-key suppression)
//!        │
//!        └──> overlay ++ filtered-base   (bag-faithful, overlay-first)
//! ```
//!
//! - **base**: an immutable, memory-mapped ACT (`<name>.act` / `<name>.wide.act`),
//!   attached by `(attach-act-base! "name")`. Never written; the OS page cache keeps
//!   the hot trie pages resident, and a query faults in only the pages it touches.
//! - **overlay**: the existing `btm` / `wide_btm` / `variable_atoms`. Every `add-atom`
//!   writes here; the base is left untouched.
//! - **tombstones**: a per-key SUPPRESSION COUNT over base facts. A `remove-atom` of a
//!   fact that exists in the base records "suppress N base copies" rather than mutating
//!   the immutable base. Stored in a *separate* `PathMap<Multiplicity>` because the
//!   overlay's [`remove_atom`](super::multiplicity::remove_atom) auto-prunes 0-count
//!   entries — a 0-valued `btm` entry cannot represent "suppress 0", and a tombstone
//!   must persist the count of base copies to hide.
//! - **compaction**: `(compact-space! "name")` folds `overlay + (base − tombstones)`
//!   into a fresh ACT, atomically renames it over the live files, and reopens it as the
//!   new base — clearing the overlay and all tombstones (a semantic no-op).
//!
//! # Why `ActBase` does NOT cache the `ACTMmap`
//!
//! [`ACTMmap`](pathmap::arena_compact::ACTMmap) (`= ArenaCompactTree<Mmap>`) carries an
//! interior `Cell<u64>` (the "currently-read value" scratch slot), which makes it `Send`
//! but **`!Sync`**. `AtomSpace<V>` lives behind an `Arc` that is shared across evaluation
//! threads (`SharedEnv = Arc<GenericEnvironmentShared<MettaValue>>`), so it must be
//! `Send + Sync`; embedding a bare `ACTMmap` would poison that. The existing
//! [`query_act`](super::MettaEnvironment::query_act) never caches the mmap either — it
//! re-opens via `ArenaCompactTree::open_mmap` on every call, relying on the OS page cache
//! (a re-`mmap` of already-resident pages is cheap). `ActBase` therefore stores only the
//! **name** (+ the resolved symbol mapping / cache-epoch) and the tiered read opens the
//! mmap fresh, exactly as `query_act` does. This keeps `ActBase` (and hence `AtomSpace`)
//! `Send + Sync` and matches the proven Stage 5a I/O pattern. See the design note in
//! `docs/mm2-integration/act-out-of-core.md` §4.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use mork::space::{Space, ACT_PATH};
use mork_expr::{maybe_byte_item, Expr};
use mork_interning::SharedMappingHandle;
use pathmap::arena_compact::ArenaCompactTree;
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use tracing::trace;

use super::act_persistence::act_leaf_multiplicity;
use super::core::{GenericEnvironment, MettaEnvironment, MultiplicityMatch};
use super::mork_encoding::mork_bytes_to_generic_value;
use super::multiplicity::{get_multiplicity, Multiplicity};
use crate::backend::eval::bindings::{
    apply_bindings_generic, collect_variables_generic, pattern_match_generic,
};
use crate::backend::eval::freshening::freshen_variables_generic;
use crate::backend::eval::space_match::space_match_bidirectional_generic;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};
use crate::backend::mork_convert::{mork_bindings_to_generic, with_mork_query_bytes};

/// An attached immutable ACT base for the tiered atom space.
///
/// Stores only the base **name** and the symbol mapping needed to decode/encode its
/// `btm` keys — NOT the `ACTMmap` itself (which is `!Sync`; the read path re-opens the
/// mmap per query, mirroring [`query_act`](super::MettaEnvironment::query_act)). The
/// wide sibling (`<name>.wide.act`), if any, needs no mapping (Wide MORK is
/// self-describing), so it is opened by name on demand too.
///
/// Held behind `Arc<ActBase>` inside `AtomSpace::act_base` so that:
/// - `fork` / `make_owned` clone it with a cheap `Arc` bump (read-only sharing),
/// - `fork_for_nondeterminism` shares it for free (the whole `atom_space` is `Arc`-shared),
/// - compaction can RCU-swap a fresh `Arc<ActBase>` in (dropping the old only after
///   installing the new).
#[derive(Clone)]
pub struct ActBase {
    /// The base name (file stem under `/dev/shm/`, i.e. MORK's `ACT_PATH`). The tiered
    /// read opens `<name>.act` / `<name>.wide.act` fresh per query.
    pub name: String,
    /// The symbol mapping for decoding/encoding `<name>.act`'s `btm` keys. Resolved once
    /// at attach time from the snapshot's `<name>.sm` (cross-run faithful) — or the
    /// attaching env's mapping if no sidecar exists. Used by the trie-pruned join (encode)
    /// and the leaf-scan decode, exactly as `query_act` resolves it via `act_sm_for`.
    pub base_sm: SharedMappingHandle,
    /// The dedicated cache-epoch for `base_sm` (paired with it by `act_sm_for_generic`),
    /// used to key `with_mork_query_bytes`'s thread-local symbol cache for the join encode.
    pub base_epoch: u64,
}

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// Compile-time guarantee: `ActBase` is `Send + Sync` (so `AtomSpace<V>` — which
// embeds it — can remain `Send + Sync` for `Arc<GenericEnvironmentShared>` cross-
// thread sharing). `String`/`SharedMappingHandle`/`u64` are all `Send + Sync`;
// crucially we do NOT embed the `!Sync` `ACTMmap` here.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ActBase>();
};

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=
    // Attach / detach
    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=

    /// Attach the on-disk ACT snapshot `<name>.act` (+ optional `<name>.wide.act`) as the
    /// immutable BASE of this space, turning the in-memory `btm`/`wide_btm` into an LSM
    /// OVERLAY on top of it. After attach, `match_space` returns
    /// `overlay ++ (base − tombstones)` (overlay-first, bag-faithful). Returns the base
    /// `.act` path.
    ///
    /// CoW: callers run this on a `clone()`d env so the attachment threads forward like any
    /// other space mutation. The base `ACTMmap` is NOT cached (it is `!Sync`); only the
    /// resolved name + symbol mapping are stored, and the read re-opens the mmap per query
    /// (the proven `query_act` I/O pattern — the OS page cache keeps it warm). The
    /// `has_act_base` gate and `act_base` slot are set under the same write critical section
    /// so a concurrent reader never observes the gate `true` with an empty slot.
    pub fn attach_act_base(&mut self, name: &str) -> std::io::Result<PathBuf> {
        self.make_owned();
        let path = format!("{ACT_PATH}{name}.act");
        trace!(target: "mettatron::act::attach", name, %path);
        // Validate the base exists & is a well-formed ACT before committing the attachment
        // (a failed attach must leave the space untiered, not half-attached).
        let _probe = ArenaCompactTree::open_mmap(&path)?;
        drop(_probe);
        // Resolve the snapshot's own symbol mapping (cross-run faithful via `<name>.sm`;
        // else this env's mapping intra-run), exactly as `query_act` does.
        let (base_sm, base_epoch) = self.act_sm_for(name);
        let base = Arc::new(ActBase {
            name: name.to_string(),
            base_sm,
            base_epoch,
        });
        // Set the slot first, then the gate, both under exclusive access (we own the env).
        *self.shared.atom_space.act_base.write() = Some(base);
        // Fresh attachment starts with NO suppression (the overlay is whatever the live
        // space already holds; the base is purely additive until something is removed).
        // PathMap has no `clear`; replace with a fresh empty map (O(1) drop of the CoW root).
        *self.shared.atom_space.tombstones.write() = pathmap::PathMap::new();
        self.shared
            .atom_space
            .has_act_base
            .store(true, Ordering::Release);
        self.mark_modified();
        Ok(PathBuf::from(path))
    }

    /// Detach the ACT base, returning the space to a purely in-memory store. The OVERLAY
    /// (`btm`/`wide_btm`) is kept as-is; the base and all tombstones are dropped and the
    /// fast-path gate is cleared. (Facts that lived ONLY in the base are no longer visible
    /// after detach — detach is "stop tiering", not "materialize". Use `compact-space!`
    /// first to fold the base into the overlay if you want to keep its facts.)
    ///
    /// CoW: run on a `clone()`d env. RCU: the old `Arc<ActBase>` is dropped only after the
    /// slot is cleared.
    pub fn detach_act_base(&mut self) {
        self.make_owned();
        trace!(target: "mettatron::act::detach", "detaching ACT base");
        self.shared
            .atom_space
            .has_act_base
            .store(false, Ordering::Release);
        let old = self.shared.atom_space.act_base.write().take();
        *self.shared.atom_space.tombstones.write() = pathmap::PathMap::new();
        drop(old);
        self.mark_modified();
    }

    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=
    // Tombstone bookkeeping
    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=

    /// The number of base copies currently suppressed for `key_bytes` (0 if none). The key
    /// is a base-`sm`-encoded fact key (so it byte-matches base keys on the read path).
    #[inline]
    pub(super) fn tombstone_count(&self, key_bytes: &[u8]) -> u64 {
        get_multiplicity(&self.shared.atom_space.tombstones.read(), key_bytes)
    }

    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=
    // Tiered read: base − tombstones
    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=

    /// Match `pattern` against the BASE (`<name>.act` + `<name>.wide.act`) MINUS the
    /// per-key tombstone suppression, instantiating `template` per match with bag
    /// multiplicity. This is the base half of the tiered `match_space`; the caller
    /// concatenates the in-memory overlay matches (overlay-first) on top.
    ///
    /// Reuses `query_act`'s two-path strategy, but applies the tombstone filter to each
    /// matched fact's stored multiplicity:
    /// 1. **Trie-pruned `query_multi_act` join** for a head-shaped, arity-1..64 pattern —
    ///    O(matches). The matched fact's literal MORK key bytes (`matched_expr.span()`)
    ///    index the tombstone count; `effective = max(0, base_leaf_mult − tombstone)`.
    /// 2. **Leaf scan + generic unification** fallback (bare-variable / arity-≥64 /
    ///    encode-failure), with the same tombstone filter on each leaf.
    /// Both decode/encode via the base's OWN `sm` (cross-run faithful). Variable-containing
    /// base facts are bidirectionally unified (freshened), matching `match_space`.
    ///
    /// Returns an empty vec if no base is attached or the file is unreadable.
    pub(super) fn match_space_base(&self, pattern: &V, template: &V) -> Vec<MultiplicityMatch<V>> {
        let Some(base) = self.shared.atom_space.act_base.read().clone() else {
            return Vec::new();
        };
        let name = &base.name;
        let act_sm = base.base_sm.clone();
        let act_epoch = base.base_epoch;
        let mut results: Vec<MultiplicityMatch<V>> = Vec::new();

        let pattern_vars = collect_variables_generic(pattern);

        // ── btm (arity < 64): trie-pruned join for head-shaped arity-1..64 patterns;
        //    else a universal leaf scan. Both decode via the snapshot's mapping. ──
        if let Ok(tree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.act")) {
            // Decode space carries the snapshot's mapping (for the join's binding recovery
            // and the scan's fact decode).
            let mut scan_space = self.create_space();
            scan_space.sm = act_sm.clone();

            let eligible =
                pattern.get_head_symbol().is_some() && (1..64).contains(&pattern.get_arity());
            let mut from_join = false;
            if eligible {
                let conj = self.factory.conjunction(vec![pattern.clone()]);
                let joined = with_mork_query_bytes(&conj, &act_sm, act_epoch, |bytes, ctx| {
                    let conj_expr = Expr {
                        ptr: bytes.as_ptr().cast_mut(),
                    };
                    let mut out: Vec<MultiplicityMatch<V>> = Vec::new();
                    Space::<()>::query_multi_act(&tree, conj_expr, |res, matched_expr| {
                        if let Err(mork_bindings) = res {
                            if let Ok(bindings) = mork_bindings_to_generic::<V, F, Multiplicity>(
                                &mork_bindings,
                                ctx,
                                &scan_space,
                                &self.factory,
                            ) {
                                // Tombstone filter on the matched fact's stored multiplicity.
                                let base_mult = act_leaf_multiplicity(&tree, matched_expr);
                                // The matched fact's literal MORK key bytes — index the
                                // tombstone count (tombstone keys are in the base sm space).
                                let key = unsafe { &*matched_expr.span() };
                                let suppressed = self.tombstone_count(key);
                                let effective = base_mult.saturating_sub(suppressed);
                                if effective > 0 {
                                    let instantiated =
                                        apply_bindings_generic(template, &bindings, &self.factory);
                                    out.push(MultiplicityMatch::new(
                                        instantiated,
                                        effective as usize,
                                    ));
                                }
                            }
                        }
                        true // collect ALL matches
                    });
                    out
                });
                if let Ok(r) = joined {
                    results = r;
                    from_join = true;
                }
            }
            if !from_join {
                // Universal leaf scan (bare-variable / arity-≥64 patterns, or encode
                // failure). u64 zipper → bag semantics; decode via the snapshot's mapping;
                // tombstone-filtered. Variable-containing base facts use bidirectional
                // unification (freshened), matching `match_space`.
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
                    let base_mult = rz.val().copied().unwrap_or(1).max(1);
                    let suppressed = self.tombstone_count(path_bytes);
                    let effective = base_mult.saturating_sub(suppressed);
                    if effective == 0 {
                        continue;
                    }
                    let Ok(fact) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                        path_bytes,
                        &scan_space,
                        &self.factory,
                    ) else {
                        continue;
                    };
                    if fact.has_variables_fast() {
                        let freshened = freshen_variables_generic(&fact, &self.factory);
                        if let Some(bindings) =
                            space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                        {
                            let instantiated =
                                apply_bindings_generic(template, &bindings, &self.factory);
                            results.push(MultiplicityMatch::new(instantiated, effective as usize));
                        }
                    } else if let Some(bindings) = pattern_match_generic(pattern, &fact) {
                        let instantiated =
                            apply_bindings_generic(template, &bindings, &self.factory);
                        results.push(MultiplicityMatch::new(instantiated, effective as usize));
                    }
                }
            }
        }

        // ── wide_btm (arity ≥ 64): leaf scan. Wide MORK encoding is sm-independent. ──
        if let Ok(wtree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.wide.act")) {
            let mut wrz = wtree.read_zipper_u64();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                if path_bytes.is_empty() {
                    continue;
                }
                let base_mult = wrz.val().copied().unwrap_or(1).max(1);
                let suppressed = self.tombstone_count(path_bytes);
                let effective = base_mult.saturating_sub(suppressed);
                if effective == 0 {
                    continue;
                }
                if let Ok(fact) = crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                    V,
                    F,
                >(path_bytes, &self.factory)
                {
                    if fact.has_variables_fast() {
                        let freshened = freshen_variables_generic(&fact, &self.factory);
                        if let Some(bindings) =
                            space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                        {
                            let instantiated =
                                apply_bindings_generic(template, &bindings, &self.factory);
                            results.push(MultiplicityMatch::new(instantiated, effective as usize));
                        }
                    } else if let Some(bindings) = pattern_match_generic(pattern, &fact) {
                        let instantiated =
                            apply_bindings_generic(template, &bindings, &self.factory);
                        results.push(MultiplicityMatch::new(instantiated, effective as usize));
                    }
                }
            }
        }

        results
    }

    /// Existence-only tiered base read: `true` iff some base fact (after tombstone
    /// suppression) matches `pattern`. Mirrors `match_space_base` but early-exits on the
    /// first surviving match. Used by `match_space_exists`/`match_space_first` after the
    /// overlay misses.
    pub(super) fn match_space_base_exists(&self, pattern: &V) -> bool {
        let Some(base) = self.shared.atom_space.act_base.read().clone() else {
            return false;
        };
        let name = &base.name;
        let act_sm = base.base_sm.clone();
        let act_epoch = base.base_epoch;
        let pattern_vars = collect_variables_generic(pattern);

        if let Ok(tree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.act")) {
            let mut scan_space = self.create_space();
            scan_space.sm = act_sm.clone();

            let eligible =
                pattern.get_head_symbol().is_some() && (1..64).contains(&pattern.get_arity());
            let mut found = false;
            let mut ran_join = false;
            if eligible {
                let conj = self.factory.conjunction(vec![pattern.clone()]);
                let probed = with_mork_query_bytes(&conj, &act_sm, act_epoch, |bytes, _ctx| {
                    let conj_expr = Expr {
                        ptr: bytes.as_ptr().cast_mut(),
                    };
                    let mut hit = false;
                    Space::<()>::query_multi_act(&tree, conj_expr, |res, matched_expr| {
                        if res.is_err() {
                            // A match exists in the base only if NOT fully tombstoned.
                            let base_mult = act_leaf_multiplicity(&tree, matched_expr);
                            let key = unsafe { &*matched_expr.span() };
                            if base_mult.saturating_sub(self.tombstone_count(key)) > 0 {
                                hit = true;
                                return false; // first surviving match — stop the walk
                            }
                        }
                        true
                    });
                    hit
                });
                if let Ok(h) = probed {
                    found = h;
                    ran_join = true;
                }
            }
            if found {
                return true;
            }
            if !ran_join {
                let mut rz = tree.read_zipper_u64();
                while rz.to_next_val() {
                    let path_bytes = rz.path();
                    if path_bytes.is_empty() || maybe_byte_item(path_bytes[0]).is_err() {
                        continue;
                    }
                    let base_mult = rz.val().copied().unwrap_or(1).max(1);
                    if base_mult.saturating_sub(self.tombstone_count(path_bytes)) == 0 {
                        continue;
                    }
                    let Ok(fact) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                        path_bytes,
                        &scan_space,
                        &self.factory,
                    ) else {
                        continue;
                    };
                    if fact.has_variables_fast() {
                        let freshened = freshen_variables_generic(&fact, &self.factory);
                        if space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                            .is_some()
                        {
                            return true;
                        }
                    } else if pattern_match_generic(pattern, &fact).is_some() {
                        return true;
                    }
                }
            }
        }

        if let Ok(wtree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.wide.act")) {
            let mut wrz = wtree.read_zipper_u64();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                if path_bytes.is_empty() {
                    continue;
                }
                let base_mult = wrz.val().copied().unwrap_or(1).max(1);
                if base_mult.saturating_sub(self.tombstone_count(path_bytes)) == 0 {
                    continue;
                }
                if let Ok(fact) = crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                    V,
                    F,
                >(path_bytes, &self.factory)
                {
                    if fact.has_variables_fast() {
                        let freshened = freshen_variables_generic(&fact, &self.factory);
                        if space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
                            .is_some()
                        {
                            return true;
                        }
                    } else if pattern_match_generic(pattern, &fact).is_some() {
                        return true;
                    }
                }
            }
        }

        false
    }

    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=
    // Tombstone WRITES (add un-tombstone / remove tombstone)
    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=
    //
    // Bag invariant for a tiered key K: `visible(K) = max(0, base_mult(K) − tombstone(K))
    // + overlay_mult(K)`, with `tombstone(K) ∈ [0, base_mult(K)]` and `overlay_mult(K) ≥ 0`.
    // To make `add-atom`/`remove-atom` EXACT bag inverses (each ±1 on `visible`):
    //   • add:    if `tombstone(K) > 0` → `tombstone(K) -= 1` (revive a suppressed base
    //             copy, +1) and DO NOT also write the overlay; else fall through to the
    //             normal overlay `+1`.
    //   • remove: if the overlay held a copy → normal overlay `−1`; else if
    //             `tombstone(K) < base_mult(K)` → `tombstone(K) += 1` (suppress a base
    //             copy, −1); else no-op (already 0).
    // This is the only model under which add∘remove = id on the bag (proof: each branch
    // moves `visible` by exactly ±1 and never drives a layer out of its valid range).
    //
    // NOTE / deviation from the design's prose ("unchanged overlay write AND decrement the
    // tombstone"): doing BOTH on add double-counts — e.g. base=2, then `rm,rm,add` would
    // give 2 instead of 1. The XOR model above (un-tombstone OR overlay-add, not both) is
    // the corrected, bag-exact semantics. See `docs/mm2-integration/act-out-of-core.md` §4.

    /// Compute the BASE key bytes for `value` (narrow facts → base-`sm`-encoded bytes;
    /// arity-≥64 facts → Wide MORK bytes, which are sm-independent) and the base's stored
    /// multiplicity at that key, then call `f(key_bytes, base_mult)`. The key bytes match
    /// exactly what the tiered read's leaf zipper yields (`.act` narrow keys / `.wide.act`
    /// wide keys), so tombstones written through here byte-match base keys on the read path.
    /// Returns `None` when no base is attached.
    ///
    /// The narrow path materializes the key into an owned `Vec<u8>` (rather than passing
    /// `f` into the encoder closure) so `f: FnOnce` is consumed EXACTLY once even when the
    /// narrow encode fails (arity ≥ 64) and we fall through to the wide key.
    fn with_base_key<R>(&self, value: &V, f: impl FnOnce(&[u8], u64) -> R) -> Option<R> {
        let base = self.shared.atom_space.act_base.read().clone()?;
        let name = &base.name;
        // Narrow path: encode via the base `sm`; materialize the key bytes (Err for arity
        // ≥ 64 → None, then the wide fallback runs).
        let narrow_key: Option<Vec<u8>> =
            with_mork_query_bytes(value, &base.base_sm, base.base_epoch, |bytes, _ctx| {
                bytes.to_vec()
            })
            .ok();
        if let Some(key) = narrow_key {
            let base_mult = match ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.act")) {
                Ok(tree) => {
                    let mut uz = tree.read_zipper_u64();
                    if uz.descend_to_check(&key) {
                        uz.val().copied().unwrap_or(0)
                    } else {
                        0
                    }
                }
                Err(_) => 0,
            };
            return Some(f(&key, base_mult));
        }
        // Wide path (arity ≥ 64): Wide MORK key (sm-independent), looked up in
        // `<name>.wide.act`.
        let mut wide_key = Vec::new();
        crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
        let wide_base_mult = match ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.wide.act"))
        {
            Ok(wtree) => {
                let mut uz = wtree.read_zipper_u64();
                if uz.descend_to_check(&wide_key) {
                    uz.val().copied().unwrap_or(0)
                } else {
                    0
                }
            }
            Err(_) => 0,
        };
        Some(f(&wide_key, wide_base_mult))
    }

    /// On `add-atom`: if a tombstone is suppressing a base copy of `value`, REVIVE one base
    /// copy by decrementing the tombstone, and return `true` so the caller skips the overlay
    /// write (the +1 is realized by the revived base copy). Returns `false` (caller does the
    /// normal overlay `+1`) when no base is attached or there is nothing to un-suppress.
    /// `set_multiplicity(_, 0)` prunes a now-zero tombstone entry.
    pub(super) fn untombstone_on_add(&self, value: &V) -> bool {
        if !self.shared.atom_space.has_act_base.load(Ordering::Acquire) {
            return false;
        }
        self.with_base_key(value, |key, _base_mult| {
            let mut ts = self.shared.atom_space.tombstones.write();
            let cur = get_multiplicity(&ts, key);
            if cur > 0 {
                super::multiplicity::set_multiplicity(&mut ts, key, cur - 1);
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
    }

    /// On `remove-atom` when the OVERLAY had no copy to decrement: if `value` exists in the
    /// base and is not yet fully suppressed (`tombstone < base_mult`), SUPPRESS one more base
    /// copy by incrementing the tombstone, and return `true` (the −1 is realized). Returns
    /// `false` (the remove is a genuine no-op — fact absent from both overlay and live base)
    /// otherwise. The tombstone is capped at `base_mult` so it can never over-suppress.
    pub(super) fn tombstone_on_remove(&self, value: &V) -> bool {
        if !self.shared.atom_space.has_act_base.load(Ordering::Acquire) {
            return false;
        }
        self.with_base_key(value, |key, base_mult| {
            if base_mult == 0 {
                return false; // not a base fact → nothing to suppress
            }
            let mut ts = self.shared.atom_space.tombstones.write();
            let cur = get_multiplicity(&ts, key);
            if cur < base_mult {
                super::multiplicity::set_multiplicity(&mut ts, key, cur + 1);
                true
            } else {
                false // already fully suppressed
            }
        })
        .unwrap_or(false)
    }

    /// `remove-atom` interposition: returns `true` when the removal was satisfied by
    /// SUPPRESSING a base copy (a tombstone), so the caller should NOT run the normal
    /// overlay decrement. This happens only when (a) a base is attached, AND (b) the
    /// OVERLAY holds no copy of `value` (overlay-first removal: overlay copies are decremented
    /// by the normal path), AND (c) `tombstone_on_remove` could suppress another base copy.
    /// Returns `false` otherwise — the caller proceeds with the normal overlay decrement
    /// (which itself no-ops if the fact is absent from both overlay and live base).
    ///
    /// The overlay-presence probe encodes via THIS env's mapping (overlay `btm` keys), while
    /// `tombstone_on_remove` encodes via the BASE mapping — the two layers have independent
    /// key spaces, so each is checked with its own encoding.
    pub(super) fn remove_overlay_miss_tombstone(&self, value: &V) -> bool {
        if !self.shared.atom_space.has_act_base.load(Ordering::Acquire) {
            return false;
        }
        // Overlay-first: if the overlay (this env's `btm`, env-sm-encoded) holds a copy, let
        // the normal decrement handle it (return false). Only an overlay miss tombstones.
        let overlay_has = with_mork_query_bytes(
            value,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |bytes, _ctx| get_multiplicity(&self.shared.atom_space.btm.read(), bytes) > 0,
        )
        .unwrap_or(false);
        if overlay_has {
            return false;
        }
        self.tombstone_on_remove(value)
    }

    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=
    // Compaction support
    // =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-=

    /// Decode the entire attached base MINUS tombstone suppression into owned
    /// `(value, effective_mult)` pairs — the logical "base − tombstones" content. Used by
    /// compaction to fold the immutable base into a fresh ACT alongside the overlay. Decodes
    /// every `<name>.act` narrow leaf (via the base `sm`) and every `<name>.wide.act` wide
    /// leaf (sm-independent), emitting `(fact, max(0, base_mult − tombstone))` for each
    /// surviving fact. Returns an empty vec when no base is attached.
    pub(super) fn collect_base_minus_tombstones(&self) -> Vec<(V, u64)> {
        let Some(base) = self.shared.atom_space.act_base.read().clone() else {
            return Vec::new();
        };
        let name = &base.name;
        let mut out: Vec<(V, u64)> = Vec::new();

        // Narrow facts (`<name>.act`), decoded via the base `sm`.
        if let Ok(tree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.act")) {
            let mut scan_space = self.create_space();
            scan_space.sm = base.base_sm.clone();
            let mut rz = tree.read_zipper_u64();
            while rz.to_next_val() {
                let path_bytes = rz.path();
                if path_bytes.is_empty() || maybe_byte_item(path_bytes[0]).is_err() {
                    continue;
                }
                let base_mult = rz.val().copied().unwrap_or(1).max(1);
                let effective = base_mult.saturating_sub(self.tombstone_count(path_bytes));
                if effective == 0 {
                    continue;
                }
                if let Ok(fact) = mork_bytes_to_generic_value::<V, F, Multiplicity>(
                    path_bytes,
                    &scan_space,
                    &self.factory,
                ) {
                    out.push((fact, effective));
                }
            }
        }

        // Wide facts (`<name>.wide.act`), sm-independent decode.
        if let Ok(wtree) = ArenaCompactTree::open_mmap(format!("{ACT_PATH}{name}.wide.act")) {
            let mut wrz = wtree.read_zipper_u64();
            while wrz.to_next_val() {
                let path_bytes = wrz.path();
                if path_bytes.is_empty() {
                    continue;
                }
                let base_mult = wrz.val().copied().unwrap_or(1).max(1);
                let effective = base_mult.saturating_sub(self.tombstone_count(path_bytes));
                if effective == 0 {
                    continue;
                }
                if let Ok(fact) = crate::backend::wide_mork::decode::wide_bytes_to_generic_value::<
                    V,
                    F,
                >(path_bytes, &self.factory)
                {
                    out.push((fact, effective));
                }
            }
        }

        out
    }
}

impl MettaEnvironment {
    /// **Compact** the tiered space: fold `overlay + (base − tombstones)` into a FRESH ACT
    /// at `<name>.{act,wide.act,sm}`, atomically replacing the old base, then re-attach it
    /// with an empty overlay and no tombstones. Returns the Σ-multiplicity (total fact count)
    /// of the compacted space as recorded in `total_atoms`.
    ///
    /// This is the LSM "merge" step: it discards tombstone bookkeeping and overlay churn by
    /// rewriting the on-disk base to exactly the current logical content, so the live state
    /// after compaction is byte-for-byte equivalent to before (a SEMANTIC NO-OP on the
    /// visible multiset) but with a clean overlay/tombstone slate and a single materialized
    /// base. Errors (with the space left attached to the OLD base) if there is no base, or on
    /// any I/O failure.
    ///
    /// Algorithm (atomic-rename, so in-flight readers of the old base are never corrupted):
    /// 1. Decode `(base − tombstones)` into owned facts (reads the OLD files, intact).
    /// 2. `detach` the base, then re-`add` those facts into the OVERLAY (no tombstone
    ///    interference) — now `self`'s in-memory store == the full compacted content, all
    ///    interned with THIS env's mapping. (`add_to_space` re-routes rules / type
    ///    assertions, rebuilding their registries — same as `load_space_from_act`.)
    /// 3. `save_space_to_act` to a TEMP name (writes `<tmp>.{act,wide.act,sm}` from THIS
    ///    env's full btm + mapping).
    /// 4. Atomically `rename` the temp files over `<name>.*` (the `.wide.act` is renamed if
    ///    the compacted content has wide facts, else the stale `<name>.wide.act` is removed).
    /// 5. Invalidate the deserialized-`sm` cache for `name` (its `.sm` just changed).
    /// 6. Clear the overlay (btm/wide/variable + reset `total_atoms`).
    /// 7. RCU re-attach `name` (installs a fresh `Arc<ActBase>` resolving the new `<name>.sm`;
    ///    the old `Arc<ActBase>` was already dropped at detach in step 2).
    pub fn compact_space(&mut self, name: &str) -> std::io::Result<usize> {
        if !self.shared.atom_space.has_act_base.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "compact-space!: no ACT base attached",
            ));
        }
        trace!(target: "mettatron::act::compact", name);

        // (1) Decode base − tombstones (reads the OLD, still-intact base files).
        let decoded = self.collect_base_minus_tombstones();

        // (2) Detach the base (drops the old Arc<ActBase>, clears tombstones, gate off), then
        //     re-add the decoded facts into the OVERLAY so subsequent `add_to_space` calls do
        //     NOT trigger the un-tombstone path (the base is gone). `self`'s in-memory store
        //     now holds overlay ++ revived-base = the full compacted content.
        self.detach_act_base();
        for (value, mult) in decoded {
            for _ in 0..mult {
                self.add_to_space(&value);
            }
        }
        let sigma = self.total_atoms();

        // (3) Dump THIS env's full btm/wide_btm to a temp name (with this env's mapping).
        let tmp = format!("{name}.compact.{}.tmp", std::process::id());
        self.save_space_to_act(&tmp)?;

        // (4) Atomically replace the live files. Renames within `/dev/shm` are atomic.
        let tmp_act = format!("{ACT_PATH}{tmp}.act");
        let live_act = format!("{ACT_PATH}{name}.act");
        std::fs::rename(&tmp_act, &live_act)?;

        let tmp_sm = format!("{ACT_PATH}{tmp}.sm");
        let live_sm = format!("{ACT_PATH}{name}.sm");
        std::fs::rename(&tmp_sm, &live_sm)?;

        // The wide sibling is written by save_space_to_act ONLY when wide facts exist. Rename
        // it over the live wide file if present; otherwise REMOVE any stale live wide file so
        // the re-attached base doesn't resurrect old wide facts.
        let tmp_wide = format!("{ACT_PATH}{tmp}.wide.act");
        let live_wide = format!("{ACT_PATH}{name}.wide.act");
        if std::path::Path::new(&tmp_wide).exists() {
            std::fs::rename(&tmp_wide, &live_wide)?;
        } else {
            let _ = std::fs::remove_file(&live_wide);
        }

        // (5) The `<name>.sm` just changed → invalidate its cached deserialized mapping so a
        //     re-attach (and any cross-env query) re-reads it. Also drop the temp-name entry
        //     `save_space_to_act` created (we won't reuse `tmp`).
        super::act_persistence::invalidate_act_sm_cache(name);
        super::act_persistence::invalidate_act_sm_cache(&tmp);

        // (6) Clear the overlay — the compacted content now lives in the (about-to-be-re-
        //     attached) base, so the overlay must start empty for the SEMANTIC NO-OP invariant
        //     (overlay ++ base would otherwise double the facts).
        *self.shared.atom_space.btm.write() = pathmap::PathMap::new();
        *self.shared.atom_space.wide_btm.write() = pathmap::PathMap::new();
        super::core::with_env_satb_deletion_barrier(|satb_active| {
            if satb_active {
                let removed: Vec<_> = self
                    .shared
                    .atom_space
                    .variable_atoms
                    .read()
                    .iter()
                    .map(|(value, _)| value.clone())
                    .collect();
                super::core::shade_generic_values_for_satb(removed);
            }
            self.shared.atom_space.variable_atoms.write().clear();
        });
        self.shared
            .atom_space
            .total_atoms
            .store(0, Ordering::Release);

        // (7) RCU re-attach: install a fresh Arc<ActBase> resolving the new `<name>.sm`.
        self.attach_act_base(name)?;

        Ok(sigma)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::MettaValue;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Build a ground S-expression `(p0 p1 …)` from symbols.
    fn sym_sexpr(parts: &[&str]) -> MettaValue {
        MettaValue::SExpr(parts.iter().map(|p| MettaValue::Atom(*p)).collect())
    }

    /// Unique ACT base name per call — avoids `/dev/shm` collisions across the
    /// (parallel) test runner and repeated runs.
    fn unique_name(tag: &str) -> String {
        static CTR: AtomicU64 = AtomicU64::new(0);
        format!(
            "mettatron_acttier_{}_{}_{}",
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

    /// Fully expand a `match_space` result into a flat multiset (bag semantics).
    fn match_all(env: &MettaEnvironment, pattern: &MettaValue) -> Vec<MettaValue> {
        env.match_space(pattern, pattern)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect()
    }

    /// Hard Constraint: with NO base attached, `match_space` is unchanged — the
    /// `has_act_base` gate is the only added cost (a predictable-false branch).
    #[test]
    fn no_base_fast_path_unchanged() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        env.add_to_space(&sym_sexpr(&["parent", "bob", "carol"]));
        assert!(
            !env.shared.atom_space.has_act_base.load(Ordering::Acquire),
            "fresh env must have no ACT base"
        );
        let pattern = sym_sexpr(&["parent", "$x", "$y"]);
        let got = match_all(&env, &pattern);
        assert_eq!(got.len(), 2, "two in-memory matches, got {got:?}");
        // match_space_base on an unattached space yields nothing.
        assert!(env.match_space_base(&pattern, &pattern).is_empty());
        assert!(!env.match_space_base_exists(&pattern));
    }

    /// The headline tiered-read guarantee: after attaching a base, `match_space`
    /// returns OVERLAY ++ (BASE − tombstones). Facts that live ONLY in the base are
    /// found, and fresh overlay adds are found too — their union is the result.
    #[test]
    fn base_plus_overlay_union() {
        // Build a base snapshot with two facts, then attach into a fresh env that
        // adds a third (overlay-only) fact.
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        producer.add_to_space(&sym_sexpr(&["parent", "bob", "carol"]));
        let name = unique_name("union");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        // overlay-only fact (NOT in the base).
        env.add_to_space(&sym_sexpr(&["parent", "carol", "dan"]));
        env.attach_act_base(&name).expect("attach");
        assert!(env.shared.atom_space.has_act_base.load(Ordering::Acquire));

        let pattern = sym_sexpr(&["parent", "$x", "$y"]);
        let got = match_all(&env, &pattern);
        // overlay {carol→dan} ++ base {alice→bob, bob→carol} = 3 facts.
        assert_eq!(
            got.len(),
            3,
            "overlay ++ base union should be 3, got {got:?}"
        );
        let expected = normalized(&[
            sym_sexpr(&["parent", "carol", "dan"]),
            sym_sexpr(&["parent", "alice", "bob"]),
            sym_sexpr(&["parent", "bob", "carol"]),
        ]);
        assert_eq!(normalized(&got), expected, "got {got:?}");

        // Existence + first-match also see the base.
        assert!(env.match_space_exists(&sym_sexpr(&["parent", "alice", "bob"])));
        assert!(env
            .match_space_first(
                &sym_sexpr(&["parent", "alice", "$y"]),
                &sym_sexpr(&["parent", "alice", "$y"])
            )
            .is_some());
    }

    /// A base attached into a FRESH environment (independent symbol mapping, simulating a
    /// different process run) is queryable via the saved `<name>.sm` — the cross-run
    /// trie-pruned join encodes faithfully through the deserialized mapping resolved at
    /// attach time. Give the consumer a divergent symbol history first so its OWN mapping
    /// cannot encode the producer's facts.
    #[test]
    fn cross_env_attach_and_query() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["edge", "a", "b"]));
        producer.add_to_space(&sym_sexpr(&["edge", "b", "c"]));
        let name = unique_name("crossenv");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut consumer = MettaEnvironment::default();
        consumer.add_to_space(&sym_sexpr(&["decoy", "zzz", "qqq", "www"]));
        consumer.attach_act_base(&name).expect("attach");

        let pattern = sym_sexpr(&["edge", "$u", "$v"]);
        let template = sym_sexpr(&["reachable", "$v", "$u"]); // swapped projection
        let got: Vec<MettaValue> = consumer
            .match_space(&pattern, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        let expected = normalized(&[
            sym_sexpr(&["reachable", "b", "a"]),
            sym_sexpr(&["reachable", "c", "b"]),
        ]);
        assert_eq!(
            normalized(&got),
            expected,
            "cross-env tiered query, got {got:?}"
        );
    }

    /// Bag faithfulness through the tiered read: a base fact stored with multiplicity 3
    /// is matched 3 times (the `u64` leaf rides through the join's O(depth) descend),
    /// and an overlay copy of the same fact ADDS to that count.
    #[test]
    fn base_multiplicity_is_bag_faithful_and_overlay_adds() {
        let mut producer = MettaEnvironment::default();
        let fact = sym_sexpr(&["dup", "x"]);
        producer.add_to_space(&fact);
        producer.add_to_space(&fact);
        producer.add_to_space(&fact); // base multiplicity 3
        let name = unique_name("bagtier");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        let pattern = sym_sexpr(&["dup", "$v"]);
        assert_eq!(
            match_all(&env, &pattern).len(),
            3,
            "base multiplicity 3 must be bag-faithful through the tiered read"
        );

        // One overlay copy → 1 (overlay) + 3 (base) = 4.
        env.add_to_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            4,
            "overlay copy adds to base multiplicity (1 + 3)"
        );
    }

    /// Clone-isolation: attaching a base to a `clone()`d env must NOT leak the attachment
    /// back into the source env (the env clone-isolation invariant). The clone is tiered;
    /// the original stays purely in-memory.
    #[test]
    fn clone_isolation_of_attach() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["base", "fact"]));
        let name = unique_name("cloneiso");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let original = MettaEnvironment::default();
        let mut cloned = original.clone();
        cloned.attach_act_base(&name).expect("attach");

        // The clone sees the base fact; the original does not.
        assert!(
            cloned.match_space_exists(&sym_sexpr(&["base", "fact"])),
            "clone with attached base must see base facts"
        );
        assert!(
            !original
                .shared
                .atom_space
                .has_act_base
                .load(Ordering::Acquire),
            "attaching to a clone must NOT tier the source env"
        );
        assert!(
            !original.match_space_exists(&sym_sexpr(&["base", "fact"])),
            "original (no base) must not see the clone's base facts"
        );
    }

    /// `fork_for_nondeterminism` Arc-shares the whole atom_space, so a forked branch
    /// observes the SAME attached base (and tombstone state) for free — matching the
    /// PeTTa global-atomspace semantics the fork was built for.
    #[test]
    fn fork_for_nondeterminism_shares_base() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["shared", "base", "fact"]));
        let name = unique_name("forkshare");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");

        let branch = env.fork_for_nondeterminism();
        assert!(
            branch
                .shared
                .atom_space
                .has_act_base
                .load(Ordering::Acquire),
            "nondeterministic fork must share the attached base gate"
        );
        assert!(
            branch.match_space_exists(&sym_sexpr(&["shared", "base", "fact"])),
            "forked branch must see the shared base facts"
        );
    }

    /// Detach returns the space to purely-in-memory: base facts disappear, overlay stays.
    #[test]
    fn detach_drops_base_keeps_overlay() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["only", "in", "base"]));
        let name = unique_name("detach");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["only", "in", "overlay"]));
        env.attach_act_base(&name).expect("attach");
        assert!(env.match_space_exists(&sym_sexpr(&["only", "in", "base"])));

        env.detach_act_base();
        assert!(
            !env.shared.atom_space.has_act_base.load(Ordering::Acquire),
            "detach clears the base gate"
        );
        assert!(
            !env.match_space_exists(&sym_sexpr(&["only", "in", "base"])),
            "base-only fact gone after detach"
        );
        assert!(
            env.match_space_exists(&sym_sexpr(&["only", "in", "overlay"])),
            "overlay fact survives detach"
        );
    }

    /// Attaching a non-existent base name is a graceful error (no panic), and leaves the
    /// space untiered.
    #[test]
    fn attach_missing_base_errors_untiered() {
        let mut env = MettaEnvironment::default();
        let name = unique_name("missing"); // never saved
        let res = env.attach_act_base(&name);
        assert!(res.is_err(), "attaching an absent base must error");
        assert!(
            !env.shared.atom_space.has_act_base.load(Ordering::Acquire),
            "a failed attach must leave the space untiered"
        );
    }

    // ── Phase 2: tombstones (remove/add over a base) ────────────────────────

    /// Tombstone SHADOWING: removing a base fact suppresses it on the tiered read, even
    /// though the immutable base file is untouched.
    #[test]
    fn tombstone_shadows_base_fact() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["parent", "alice", "bob"]));
        producer.add_to_space(&sym_sexpr(&["parent", "bob", "carol"]));
        let name = unique_name("shadow");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        let pattern = sym_sexpr(&["parent", "$x", "$y"]);
        assert_eq!(
            match_all(&env, &pattern).len(),
            2,
            "both base facts visible"
        );

        // Remove one base fact → tombstone it (overlay had no copy).
        env.remove_from_space(&sym_sexpr(&["parent", "alice", "bob"]));
        let got = match_all(&env, &pattern);
        assert_eq!(got.len(), 1, "one base fact suppressed, got {got:?}");
        assert_eq!(
            normalized(&got),
            normalized(&[sym_sexpr(&["parent", "bob", "carol"])]),
            "the surviving fact is the un-removed one, got {got:?}"
        );
        // Existence agrees.
        assert!(!env.match_space_exists(&sym_sexpr(&["parent", "alice", "bob"])));
        assert!(env.match_space_exists(&sym_sexpr(&["parent", "bob", "carol"])));
    }

    /// Remove-then-readd UN-HIDES: removing a base fact then adding it back returns the
    /// space to its original multiset (add∘remove = id), via the un-tombstone-on-add path.
    #[test]
    fn remove_then_readd_unhides_base_fact() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["fact", "x"]));
        let name = unique_name("readd");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        let pattern = sym_sexpr(&["fact", "$v"]);
        assert_eq!(match_all(&env, &pattern).len(), 1);

        env.remove_from_space(&sym_sexpr(&["fact", "x"]));
        assert_eq!(
            match_all(&env, &pattern).len(),
            0,
            "suppressed after remove"
        );

        env.add_to_space(&sym_sexpr(&["fact", "x"]));
        assert_eq!(
            match_all(&env, &pattern).len(),
            1,
            "re-add revives the suppressed base copy (add∘remove = id)"
        );
        // The tombstone PathMap is empty again (the entry was pruned at count 0).
        assert!(
            env.shared.atom_space.tombstones.read().val_count() == 0,
            "tombstone entry pruned after un-tombstone"
        );
    }

    /// Tombstone MULTIPLICITY (count form): a base fact stored with multiplicity 3 must
    /// take THREE removes to fully suppress, decrementing the visible count by one each
    /// time; a fourth remove is a no-op (capped at base_mult).
    #[test]
    fn tombstone_is_count_based_over_multiplicity() {
        let mut producer = MettaEnvironment::default();
        let fact = sym_sexpr(&["dup", "y"]);
        producer.add_to_space(&fact);
        producer.add_to_space(&fact);
        producer.add_to_space(&fact); // base multiplicity 3
        let name = unique_name("tscount");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        let pattern = sym_sexpr(&["dup", "$v"]);
        assert_eq!(match_all(&env, &pattern).len(), 3, "base mult 3");

        env.remove_from_space(&fact);
        assert_eq!(match_all(&env, &pattern).len(), 2, "after 1 remove");
        env.remove_from_space(&fact);
        assert_eq!(match_all(&env, &pattern).len(), 1, "after 2 removes");
        env.remove_from_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            0,
            "after 3 removes (fully suppressed)"
        );
        // 4th remove is a no-op (tombstone capped at base_mult = 3).
        env.remove_from_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            0,
            "4th remove is a no-op (capped)"
        );

        // Re-add brings it back to 1, 2, 3 …
        env.add_to_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            1,
            "1 re-add revives one base copy"
        );
        env.add_to_space(&fact);
        env.add_to_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            3,
            "all 3 base copies revived"
        );
    }

    /// Overlay-re-add ADDITIVE multiplicity: an OVERLAY copy stacks ON TOP of the base
    /// multiplicity (no tombstone involved — the base is fully visible), and removing the
    /// overlay copy peels back to the base count (overlay-first removal).
    #[test]
    fn overlay_readd_is_additive_over_base() {
        let mut producer = MettaEnvironment::default();
        let fact = sym_sexpr(&["item", "z"]);
        producer.add_to_space(&fact);
        producer.add_to_space(&fact); // base multiplicity 2
        let name = unique_name("additive");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        let pattern = sym_sexpr(&["item", "$v"]);
        assert_eq!(match_all(&env, &pattern).len(), 2, "base mult 2");

        // Two overlay copies → 2 (base) + 2 (overlay) = 4.
        env.add_to_space(&fact);
        env.add_to_space(&fact);
        assert_eq!(match_all(&env, &pattern).len(), 4, "overlay 2 + base 2");

        // Remove one → overlay-first decrement → 2 (base) + 1 (overlay) = 3.
        env.remove_from_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            3,
            "overlay-first removal peels overlay"
        );
        // Remove the other overlay copy → back to base 2.
        env.remove_from_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            2,
            "overlay exhausted → base remains"
        );
        // Now removing again tombstones the base → 1.
        env.remove_from_space(&fact);
        assert_eq!(
            match_all(&env, &pattern).len(),
            1,
            "next remove suppresses a base copy"
        );
    }

    /// Removing a fact that is in NEITHER overlay nor base is a clean no-op (no spurious
    /// tombstone, no panic).
    #[test]
    fn remove_absent_fact_is_noop() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["present", "a"]));
        let name = unique_name("absent");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        // Remove a fact that exists nowhere.
        env.remove_from_space(&sym_sexpr(&["ghost", "b"]));
        assert!(
            env.shared.atom_space.tombstones.read().val_count() == 0,
            "removing an absent fact must not create a tombstone"
        );
        assert_eq!(
            match_all(&env, &sym_sexpr(&["present", "$x"])).len(),
            1,
            "the real base fact is unaffected"
        );
    }

    /// Tombstones thread through `clone()` with isolation: a tombstone written on a clone
    /// does not affect the source env's tiered view.
    #[test]
    fn tombstone_clone_isolation() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["shared", "fact"]));
        let name = unique_name("tsclone");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut base_env = MettaEnvironment::default();
        base_env.attach_act_base(&name).expect("attach");

        // Clone, then suppress the base fact on the clone only.
        let mut cloned = base_env.clone();
        cloned.remove_from_space(&sym_sexpr(&["shared", "fact"]));

        assert_eq!(
            match_all(&cloned, &sym_sexpr(&["shared", "$x"])).len(),
            0,
            "clone suppressed the base fact"
        );
        assert_eq!(
            match_all(&base_env, &sym_sexpr(&["shared", "$x"])).len(),
            1,
            "source env's tiered view is unaffected by the clone's tombstone"
        );
    }

    // ── Phase 3: compaction ─────────────────────────────────────────────────

    /// Compaction SEMANTIC NO-OP invariant: after `(compact-space!)`, the visible multiset
    /// for every pattern is byte-for-byte identical to before, BUT the overlay and tombstones
    /// are clean and the base file now materializes the full content. Exercises all three
    /// layers: a surviving base fact, a tombstoned (removed) base fact, an overlay-only fact,
    /// and an overlay copy stacked on a base fact.
    #[test]
    fn compaction_is_semantic_no_op() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["p", "keep"]));
        producer.add_to_space(&sym_sexpr(&["p", "drop"]));
        let stacked = sym_sexpr(&["p", "stack"]);
        producer.add_to_space(&stacked); // base mult 1 for (p stack)
        let name = unique_name("compactnoop");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        // Mutate across all layers:
        env.remove_from_space(&sym_sexpr(&["p", "drop"])); // tombstone a base fact
        env.add_to_space(&sym_sexpr(&["p", "fresh"])); // overlay-only
        env.add_to_space(&stacked); // overlay copy on top of base (p stack) → mult 2

        let pattern = sym_sexpr(&["p", "$v"]);
        let before = normalized(&match_all(&env, &pattern));
        // Expect: keep(1) + stack(2) + fresh(1) = 4 ; drop suppressed.
        assert_eq!(before.len(), 4, "pre-compaction multiset, got {before:?}");

        // Compact.
        let sigma = env.compact_space(&name).expect("compact");
        assert_eq!(sigma, 4, "Σ-multiplicity of compacted content");

        // SEMANTIC NO-OP: identical visible multiset after compaction.
        let after = normalized(&match_all(&env, &pattern));
        assert_eq!(
            after, before,
            "compaction must not change the visible multiset"
        );

        // The slate is clean: overlay btm empty, tombstones empty, still tiered.
        assert!(
            env.shared.atom_space.has_act_base.load(Ordering::Acquire),
            "compaction keeps the space tiered"
        );
        assert_eq!(
            env.shared.atom_space.tombstones.read().val_count(),
            0,
            "compaction clears tombstones"
        );
        assert_eq!(
            env.shared.atom_space.btm.read().val_count(),
            0,
            "compaction clears the overlay btm (content now lives in the fresh base)"
        );
    }

    /// Compaction ROUND-TRIP: the compacted base, attached into a FRESH environment, yields
    /// exactly the compacted multiset (the new `<name>.{act,sm}` is a self-contained, cross-
    /// env-usable snapshot — proving the base was genuinely rewritten with the live mapping).
    #[test]
    fn compaction_round_trip_into_fresh_env() {
        let mut producer = MettaEnvironment::default();
        producer.add_to_space(&sym_sexpr(&["q", "a"]));
        producer.add_to_space(&sym_sexpr(&["q", "b"]));
        let name = unique_name("compactrt");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        env.remove_from_space(&sym_sexpr(&["q", "a"])); // tombstone
        env.add_to_space(&sym_sexpr(&["q", "c"])); // overlay
        env.compact_space(&name).expect("compact");
        // Live env sees {q b, q c}.
        let pattern = sym_sexpr(&["q", "$v"]);
        assert_eq!(
            normalized(&match_all(&env, &pattern)),
            normalized(&[sym_sexpr(&["q", "b"]), sym_sexpr(&["q", "c"])]),
            "live env post-compaction"
        );

        // A FRESH env (divergent symbol history) attaches the compacted base and sees the
        // same multiset — the rewritten `<name>.sm` decodes faithfully cross-env.
        let mut fresh = MettaEnvironment::default();
        fresh.add_to_space(&sym_sexpr(&["decoy", "z"]));
        fresh.attach_act_base(&name).expect("attach");
        assert_eq!(
            normalized(&match_all(&fresh, &pattern)),
            normalized(&[sym_sexpr(&["q", "b"]), sym_sexpr(&["q", "c"])]),
            "fresh-env attach of the compacted base"
        );
    }

    /// Compaction preserves MULTIPLICITY: a base fact at mult 3 with one tombstone + one
    /// overlay copy compacts to the correct net count, and the count survives the rewrite.
    #[test]
    fn compaction_preserves_multiplicity() {
        let mut producer = MettaEnvironment::default();
        let f = sym_sexpr(&["m", "v"]);
        producer.add_to_space(&f);
        producer.add_to_space(&f);
        producer.add_to_space(&f); // base mult 3
        let name = unique_name("compactmult");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        env.remove_from_space(&f); // tombstone 1 → visible base 2
        env.add_to_space(&f); // overlay-revive? No: tombstone>0 so un-tombstone → visible 3
                              // After un-tombstone the tombstone is back to 0 → visible = base 3.
        env.add_to_space(&f); // now tombstone 0 → real overlay copy → visible 4

        let pattern = sym_sexpr(&["m", "$x"]);
        assert_eq!(
            match_all(&env, &pattern).len(),
            4,
            "pre-compaction net mult"
        );

        let sigma = env.compact_space(&name).expect("compact");
        assert_eq!(sigma, 4, "compacted Σ-multiplicity");
        assert_eq!(
            match_all(&env, &pattern).len(),
            4,
            "multiplicity 4 survives compaction"
        );

        // And a second compaction (idempotent) keeps it at 4.
        let sigma2 = env.compact_space(&name).expect("recompact");
        assert_eq!(sigma2, 4, "re-compaction is idempotent");
        assert_eq!(match_all(&env, &pattern).len(), 4);
    }

    /// Compacting an UNATTACHED space is a graceful error (no base to compact).
    #[test]
    fn compact_without_base_errors() {
        let mut env = MettaEnvironment::default();
        env.add_to_space(&sym_sexpr(&["x", "1"]));
        let name = unique_name("compactnobase");
        let res = env.compact_space(&name);
        assert!(res.is_err(), "compacting an untiered space must error");
    }

    /// Compaction with WIDE facts (arity ≥ 64) round-trips: the `<name>.wide.act` sibling is
    /// rewritten alongside the narrow base.
    #[test]
    fn compaction_handles_wide_facts() {
        let mut producer = MettaEnvironment::default();
        let mut parts: Vec<MettaValue> = Vec::with_capacity(70);
        parts.push(MettaValue::Atom("wide"));
        for i in 0..69 {
            parts.push(MettaValue::Long(i));
        }
        let wide_fact = MettaValue::SExpr(parts);
        producer.add_to_space(&wide_fact);
        producer.add_to_space(&sym_sexpr(&["narrow", "n"]));
        let name = unique_name("compactwide");
        let _c = ActCleanup(name.clone());
        producer.save_space_to_act(&name).expect("save");

        let mut env = MettaEnvironment::default();
        env.attach_act_base(&name).expect("attach");
        // Both visible before compaction.
        assert_eq!(env.match_space(&wide_fact, &wide_fact).len(), 1);
        assert!(env.match_space_exists(&sym_sexpr(&["narrow", "n"])));

        let sigma = env.compact_space(&name).expect("compact");
        assert_eq!(sigma, 2, "one wide + one narrow fact");

        // Both still visible after compaction (wide sibling rewritten).
        assert_eq!(
            env.match_space(&wide_fact, &wide_fact).len(),
            1,
            "wide fact survives compaction"
        );
        assert!(
            env.match_space_exists(&sym_sexpr(&["narrow", "n"])),
            "narrow fact survives compaction"
        );
    }
}
