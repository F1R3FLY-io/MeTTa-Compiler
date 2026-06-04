//! Arena-allocated MeTTa values.
//!
//! MettaValue provides Copy-semantic MeTTa values allocated from a
//! global slab allocator (`SlabAllocator`) via `GcFactory`.
//!
//! ## Key Benefits
//!
//! - **Copy semantics**: MettaValue is just a pointer (8 bytes), so Clone/Copy is free.
//! - **No reference counting**: Values don't need Arc overhead or atomic operations.
//! - **Lock-free allocation**: Multiple threads can allocate concurrently.
//! - **Background GC**: Dead values are reclaimed by a snapshot-based mark-sweep collector.
//! - **Cache-friendly**: Contiguous 64KB page layout.
//!
//! ## Memory Safety
//!
//! All references within MettaValue are `'static`, tied to the global slab allocator.
//! Values live for the program duration and are reclaimed by the GC when no longer reachable.

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use xxhash_rust::xxh3::Xxh3;

use crate::backend::hash_utils::PtrBuildHasher;
use crate::ir::Span;

use super::metta_value_trait::{MettaValueFactory, MettaValueTrait};
use super::{MemoHandle, SpaceHandle};

use self::serialize_tags::*;

// ============================================================================
// Thread-local hash cache for MettaValue content hashing
// ============================================================================

/// Golden ratio constant for combining child hashes in SExpr.
const HASH_GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;

/// Two-tier hash cache: L1 direct-mapped array + L2 HashMap.
///
/// L1: 1024-entry array indexed by `(ptr >> 4) & 0x3FF`. Each entry is a
/// `(slab_ptr, hash)` pair. On hit (ptr match), returns in ~2ns (array index +
/// compare). On miss, falls through to L2.
///
/// L2: HashMap with Fibonacci pointer hashing. Handles L1 collisions.
/// O(1) amortized lookup.
///
/// Both tiers store the same data — L1 is a subset (last writer wins on collision).
/// This avoids HashMap overhead for ~90%+ of lookups while preserving correctness
/// for the remaining collisions.
struct TieredHashCache {
    l1: Vec<(usize, u64)>,
    l2: HashMap<usize, u64, PtrBuildHasher>,
}

const HASH_L1_SIZE: usize = 1024;
const HASH_L1_MASK: usize = HASH_L1_SIZE - 1;

impl TieredHashCache {
    fn new() -> Self {
        Self {
            l1: vec![(0usize, 0u64); HASH_L1_SIZE],
            l2: HashMap::with_hasher(PtrBuildHasher),
        }
    }

    #[inline(always)]
    fn get(&self, key: usize) -> Option<u64> {
        let idx = (key >> 4) & HASH_L1_MASK;
        // Safety: idx is always < HASH_L1_SIZE due to mask
        let (k, v) = unsafe { *self.l1.get_unchecked(idx) };
        if k == key {
            return Some(v);
        }
        self.l2.get(&key).copied()
    }

    #[inline(always)]
    fn insert(&mut self, key: usize, hash: u64) {
        let idx = (key >> 4) & HASH_L1_MASK;
        // Safety: idx is always < HASH_L1_SIZE due to mask
        unsafe {
            *self.l1.get_unchecked_mut(idx) = (key, hash);
        }
        self.l2.insert(key, hash);
    }

    fn clear(&mut self) {
        for slot in self.l1.iter_mut() {
            *slot = (0, 0);
        }
        self.l2.clear();
    }
}

thread_local! {
    /// Tiered hash cache: L1 direct-mapped (1024 entries, 16 KB) + L2 HashMap.
    ///
    /// Slab pointers are stable (never moved) until GC frees them. This cache
    /// eliminates O(tree_size) recursive hashing for deeply nested S-expressions
    /// (PLN Robot has depth 252). After the first hash, subsequent lookups are O(1).
    ///
    /// Invalidated at GC safepoints via `clear_value_hash_cache()` to prevent
    /// ABA issues when freed slots are reused.
    static VALUE_HASH_CACHE: RefCell<TieredHashCache> =
        RefCell::new(TieredHashCache::new());

    /// GC sweep epoch observed by this thread's value hash cache.
    ///
    /// The cache is keyed by slab pointers. Work-pool threads can miss another
    /// thread's safepoint, so they must lazily clear stale pointer entries after
    /// any GC sweep that may have freed and reused slab slots.
    static VALUE_HASH_CACHE_EPOCH: Cell<u64> = const { Cell::new(0) };
}

/// Clear the thread-local hash value cache.
///
/// Must be called at GC safepoints before slab slots can be reused, to prevent
/// stale cached hashes from being returned for new values at recycled addresses.
pub fn clear_value_hash_cache() {
    VALUE_HASH_CACHE.with(|c| c.borrow_mut().clear());
    let current_epoch = crate::backend::models::gc_allocator::gc_sweep_epoch();
    VALUE_HASH_CACHE_EPOCH.with(|epoch| epoch.set(current_epoch));
}

#[inline]
fn ensure_value_hash_cache_epoch_current() {
    let current_epoch = crate::backend::models::gc_allocator::gc_sweep_epoch();
    VALUE_HASH_CACHE_EPOCH.with(|epoch| {
        if epoch.get() != current_epoch {
            VALUE_HASH_CACHE.with(|c| c.borrow_mut().clear());
            epoch.set(current_epoch);
        }
    });
}

/// Recursive helper: compute hash for a `MettaValue`, using `cache` for memoization.
///
/// For `SExpr`, children hashes are fetched from the cache (if available) and combined
/// with golden-ratio mixing, avoiding full Xxh3 tree traversal. This turns O(tree_size)
/// per call into O(arity) for cached children, and O(1) for fully-cached values.
/// **Stack-safety mandate (2026-05-15)**: refactored to iterative work-list.
/// Audit item T#18. Was recursive on SExpr children (line 229 of pre-refactor),
/// Quoted (line 238), Spanned (line 247). Memoization handles shared
/// substructure; the iterative form handles deeply-nested unique structures.
fn hash_value_cached_inner(value: &MettaValue, cache: &mut TieredHashCache) -> u64 {
    const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
    const LONG_SEED: u64 = 0x517cc1b727220a95;
    const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
    const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;
    const UNIT_HASH: u64 = 0x756e6974_68617368;

    /// Fast-path scalar hash (no recursion). Returns Some(h) for inline /
    /// primitive values, None for composite values that need traversal.
    fn fast_path(value: &MettaValue) -> Option<u64> {
        const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
        const LONG_SEED: u64 = 0x517cc1b727220a95;
        const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
        const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;
        const UNIT_HASH: u64 = 0x756e6974_68617368;
        if value.is_inline() {
            return Some(match value.inline_tag() {
                NB_TAG_UNIT => UNIT_HASH,
                NB_TAG_BOOL => {
                    if (value.tagged as u64 & 1) != 0 {
                        BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
                    } else {
                        BOOL_SEED
                    }
                }
                NB_TAG_LONG => {
                    let n = value.inline_long_value();
                    let x = (n as u64)
                        .wrapping_add(LONG_SEED)
                        .wrapping_mul(GOLDEN_RATIO);
                    x ^ (x >> 32)
                }
                NB_TAG_EMPTY => 9u64.wrapping_mul(GOLDEN_RATIO),
                _ => UNIT_HASH,
            });
        }
        match value.inner_ref() {
            MettaValueInner::Unit => Some(UNIT_HASH),
            MettaValueInner::Bool(b) => Some(if *b {
                BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
            } else {
                BOOL_SEED
            }),
            MettaValueInner::Long(n) => {
                let x = (*n as u64)
                    .wrapping_add(LONG_SEED)
                    .wrapping_mul(GOLDEN_RATIO);
                Some(x ^ (x >> 32))
            }
            MettaValueInner::Float(f) => {
                let bits = f.to_bits();
                let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
                Some(x ^ (x >> 32))
            }
            MettaValueInner::Empty => Some(9u64.wrapping_mul(GOLDEN_RATIO)),
            MettaValueInner::NotReducible => Some(1u64.wrapping_mul(GOLDEN_RATIO)),
            _ => None,
        }
    }

    // Fast path for the input.
    if let Some(h) = fast_path(value) {
        return h;
    }
    // Cache lookup.
    let key = value.inner_ptr() as usize;
    if let Some(h) = cache.get(key) {
        return h;
    }

    // Iterative descent for composite values.
    enum Work {
        Process { val: MettaValue, key: usize },
        // Combine children. `key` is the parent's slab pointer for cache insert.
        // `tag`: 7 = SExpr, 10 = Quoted. `count` is number of child hashes
        // pending on the result stack.
        Combine { key: usize, tag: u64, count: usize },
    }
    let mut work: Vec<Work> = Vec::with_capacity(8);
    let mut hashes: Vec<u64> = Vec::with_capacity(8);
    work.push(Work::Process {
        val: value.clone(),
        key,
    });
    while let Some(w) = work.pop() {
        match w {
            Work::Process { val, key } => {
                if let Some(h) = fast_path(&val) {
                    hashes.push(h);
                    continue;
                }
                if let Some(h) = cache.get(key) {
                    hashes.push(h);
                    continue;
                }
                match val.inner_ref() {
                    MettaValueInner::Atom(s) => {
                        let mut hasher = Xxh3::new();
                        6u8.hash(&mut hasher);
                        s.hash(&mut hasher);
                        let h = hasher.finish();
                        cache.insert(key, h);
                        hashes.push(h);
                    }
                    MettaValueInner::String(s) => {
                        let mut hasher = Xxh3::new();
                        5u8.hash(&mut hasher);
                        s.hash(&mut hasher);
                        let h = hasher.finish();
                        cache.insert(key, h);
                        hashes.push(h);
                    }
                    MettaValueInner::SExpr(items) => {
                        let len = items.len();
                        work.push(Work::Combine {
                            key,
                            tag: 7u64 ^ len as u64,
                            count: len,
                        });
                        for item in items.iter().rev() {
                            let ck = item.inner_ptr() as usize;
                            work.push(Work::Process {
                                val: item.clone(),
                                key: ck,
                            });
                        }
                    }
                    MettaValueInner::Quoted(inner) => {
                        work.push(Work::Combine {
                            key,
                            tag: 10u64,
                            count: 1,
                        });
                        let ck = inner.inner_ptr() as usize;
                        work.push(Work::Process {
                            val: *inner,
                            key: ck,
                        });
                    }
                    MettaValueInner::Spanned(inner, _span) => {
                        // Spanned hash == inner hash, no combine.
                        let ck = inner.inner_ptr() as usize;
                        work.push(Work::Process {
                            val: *inner,
                            key: ck,
                        });
                    }
                    MettaValueInner::Lazy(inner) => {
                        // PT-canonical Lazy is INVISIBLE for hashing
                        // (2026-05-21): mirror Spanned — descend into inner
                        // with no combine, so `hash(Lazy(x)) == hash(x)`.
                        let ck = inner.inner_ptr() as usize;
                        work.push(Work::Process {
                            val: *inner,
                            key: ck,
                        });
                    }
                    MettaValueInner::Error(..) => {
                        let h = 8u64.wrapping_mul(HASH_GOLDEN_RATIO);
                        cache.insert(key, h);
                        hashes.push(h);
                    }
                    other => {
                        let mut hasher = Xxh3::new();
                        hash_value_for_trait_inner(other, &mut hasher);
                        let h = hasher.finish();
                        cache.insert(key, h);
                        hashes.push(h);
                    }
                }
            }
            Work::Combine { key, tag, count } => {
                let start = hashes.len() - count;
                let children: Vec<u64> = hashes.drain(start..).collect();
                let mut combined: u64 = tag;
                for child_hash in children.iter() {
                    combined ^= child_hash
                        .wrapping_add(HASH_GOLDEN_RATIO)
                        .wrapping_add(combined << 6)
                        .wrapping_add(combined >> 2);
                }
                cache.insert(key, combined);
                hashes.push(combined);
            }
        }
    }
    let h = hashes.pop().expect("hash_value_cached_inner: empty result");
    // Silence unused-binding warnings for the original closure-local constants.
    let _ = (GOLDEN_RATIO, LONG_SEED, BOOL_SEED, FLOAT_SEED, UNIT_HASH);
    h
}

/// Low-level Xxh3 hasher for rare MettaValueInner variants (Type, Conjunction, etc.).
/// Only called on cache miss for non-primitive, non-SExpr values.
fn hash_value_for_trait_inner<H: Hasher>(inner: &MettaValueInner, hasher: &mut H) {
    match inner {
        MettaValueInner::Unit => 0u8.hash(hasher),
        MettaValueInner::Bool(b) => {
            2u8.hash(hasher);
            b.hash(hasher);
        }
        MettaValueInner::Long(n) => {
            3u8.hash(hasher);
            n.hash(hasher);
        }
        MettaValueInner::Float(f) => {
            4u8.hash(hasher);
            f.to_bits().hash(hasher);
        }
        MettaValueInner::String(s) => {
            5u8.hash(hasher);
            s.hash(hasher);
        }
        MettaValueInner::Atom(s) => {
            6u8.hash(hasher);
            s.hash(hasher);
        }
        MettaValueInner::SExpr(_) => {
            7u8.hash(hasher);
        } // children not traversed here
        MettaValueInner::Error(..) => 8u8.hash(hasher),
        MettaValueInner::Empty => 9u8.hash(hasher),
        // Plan S0a (2026-05-13) — unique tag 1u8 for NotReducible sentinel.
        MettaValueInner::NotReducible => 1u8.hash(hasher),
        MettaValueInner::Quoted(_) => 10u8.hash(hasher),
        MettaValueInner::Lazy(inner) => {
            // PT-canonical Lazy is INVISIBLE for hashing (2026-05-21):
            // delegate to the inner value so `Lazy(x).hash() == x.hash()`.
            // This branch is hit by the rare-variants fall-through arm in
            // `hash_value_cached_inner` (see "other =>" arm at line ~304).
            // The iterative driver normally handles SExpr/Quoted/Spanned
            // recursively; for Lazy we want the same transparent semantics
            // as Spanned — no tag emission, recurse into the inner.
            hash_value_for_trait_inner(inner.inner_ref(), hasher);
        }
        MettaValueInner::Spanned(_, _) => 11u8.hash(hasher),
        MettaValueInner::Space(handle) => {
            12u8.hash(hasher);
            handle.id.hash(hasher);
        }
        MettaValueInner::State(id) => {
            13u8.hash(hasher);
            id.hash(hasher);
        }
        _ => 14u8.hash(hasher), // Type, Conjunction, Memo
    }
}

/// Arena-allocated MeTTa value with O(1) clone (just copies the pointer).
///
/// This is a thin wrapper around either:
/// 1. A **tagged pointer** to slab-allocated `MettaValueInner` (for compound types), or
/// 2. A **NaN-boxed inline value** (for Bool, Long, Unit, Empty) — no slab allocation.
///
/// ## Encoding Layout
///
/// ```text
/// Slab pointer:  bits [63:48] = 0 (user-space), bits [47:4] = ptr, bits [3:0] = flags
/// NaN-boxed:     bits [63:48] >= 0x7FF8 (quiet NaN tag), bits [47:0] = payload
/// ```
///
/// ### Slab Pointer Flag Bits
///
/// | Bit | Constant              | Meaning                                     |
/// |-----|-----------------------|---------------------------------------------|
/// |  0  | `FLAG_HAS_VARIABLES`  | Value (or any sub-value) contains variables  |
/// |  1  | reserved              | Future use                                   |
/// |  2  | reserved              | Future use                                   |
/// |  3  | reserved              | Future use                                   |
///
/// ### NaN-Boxing Tags (upper 16 bits)
///
/// | Tag    | Hex prefix | Type  | Payload                              |
/// |--------|-----------|-------|--------------------------------------|
/// | 0x7FF8 | TAG_LONG  | Long  | 48-bit signed integer (sign-extended)|
/// | 0x7FF9 | TAG_BOOL  | Bool  | 0 = false, 1 = true                 |
/// | 0x7FFA | TAG_EMPTY | Empty | unused (always 0)                    |
/// | 0x7FFB | TAG_UNIT  | Unit  | unused (always 0)                    |
///
/// Inline values bypass slab allocation entirely: no `alloc_value()`, no GC pressure,
/// no pointer indirection, no TLB misses. The encoding is compatible with the JIT's
/// NaN-boxing scheme, enabling zero-cost JIT↔interpreter value passing.
///
/// Flags are computed once at construction time (bottom-up propagation) and
/// provide O(1) queries vs. the O(depth) recursive tree walk of `contains_variables()`.
#[derive(Clone, Copy)]
pub struct MettaValue {
    /// Either a tagged slab pointer or a NaN-boxed inline value.
    /// - Slab: bits [63:4] = *const MettaValueInner, bits [3:0] = flags
    /// - Inline: bits [63:48] >= 0x7FF8 (NaN tag), bits [47:0] = payload
    pub(crate) tagged: usize,
}

/// Mask to extract the pointer from a tagged MettaValue (clears low 4 flag bits).
/// Only valid for slab-pointer values (not inline NaN-boxed values).
pub(crate) const PTR_MASK: usize = !0xF;

/// Flag bit 0: this value (or any sub-value) contains variables.
pub(crate) const FLAG_HAS_VARIABLES: usize = 0x01;

// ==========================================================================
// NaN-boxing constants for inline value encoding
// ==========================================================================
//
// These mirror the JIT constants in `bytecode/jit/types/constants.rs` but are
// defined here to avoid a circular dependency. The values are identical.

/// NaN-boxing tag for 48-bit signed integers.
/// Payload: sign-extended 48-bit value. Range: -(2^47) to (2^47)-1.
pub(crate) const NB_TAG_LONG: u64 = 0x7FF8_0000_0000_0000;

/// NaN-boxing tag for boolean values. Payload: 0 = false, 1 = true.
pub(crate) const NB_TAG_BOOL: u64 = 0x7FF9_0000_0000_0000;

/// NaN-boxing tag for Empty sentinel (zero-result marker).
pub(crate) const NB_TAG_EMPTY: u64 = 0x7FFA_0000_0000_0000;

/// NaN-boxing tag for Unit value (side-effect marker).
pub(crate) const NB_TAG_UNIT: u64 = 0x7FFB_0000_0000_0000;

/// Mask to extract the tag (upper 16 bits) from a NaN-boxed value.
pub(crate) const NB_TAG_MASK: u64 = 0xFFFF_0000_0000_0000;

/// Mask to extract the 48-bit payload from a NaN-boxed value.
pub(crate) const NB_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Sign bit position within the 48-bit payload (bit 47).
pub(crate) const NB_SIGN_BIT_48: u64 = 0x0000_8000_0000_0000;

/// Minimum value for inline 48-bit NaN-boxed tag detection.
/// Any value with `(tagged >> 48) >= 0x7FF8` is a NaN-boxed inline value.
pub(crate) const NB_MIN_TAG: u64 = 0x7FF8;

/// Maximum i64 value that fits in 48-bit inline encoding.
pub(crate) const NB_LONG_MAX: i64 = (1i64 << 47) - 1;

/// Minimum i64 value that fits in 48-bit inline encoding.
pub(crate) const NB_LONG_MIN: i64 = -(1i64 << 47);

/// Check if a string represents a MeTTa variable (for tagged pointer flag computation).
/// Variables: `$x`, `&name` (but NOT `&`, `&self`, `&kb`, `&stack`), `'x`, `_`.
#[inline]
pub(crate) fn is_variable_str(s: &str) -> bool {
    s == "_"
        || s.starts_with('$')
        || (s.starts_with('&') && s != "&" && s != "&self" && s != "&kb" && s != "&stack")
        || s.starts_with('\'')
}

// ==========================================================================
// Inc 2: value-decode mode (index-arena vs slab). Default Slab — byte-identical.
// ==========================================================================

/// Process-global value-decode mode, set ONCE at startup before any value is
/// created. `0` (Slab, default) keeps the baseline byte-identical; `1` (Index)
/// reinterprets a heap handle's non-NaN payload as an arena
/// [`Addr`](crate::backend::eval::cesk::index_arena::Addr). Inc 2 leaves this at
/// Slab and only adds the (never-taken-by-default) Index arms; the `--gc=index`
/// flip is Inc 3/4.
// Inc 4: under `--features index-gc` the evaluator's `global_factory()` allocates
// into the index store σ, so the decode must default to Index mode to match (the
// value model is selected at COMPILE time; this static is the runtime-readable
// reflection of that, set once at process start). Default build inits to Slab (0).
static GC_MODE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(if cfg!(feature = "index-gc") { 1 } else { 0 });

/// `true` iff the process is in index-arena value mode. A relaxed load of a
/// write-once, cache-resident static — one perfectly-predicted branch on the hot
/// path (the flag never changes after startup), so the Slab path is effectively
/// unchanged.
#[inline(always)]
pub(crate) fn gc_mode_is_index() -> bool {
    GC_MODE.load(std::sync::atomic::Ordering::Relaxed) != 0
}

/// Switch the process to index-arena value mode. MUST be called at startup
/// before any `MettaValue` is constructed (Inc 3 `--gc=index` startup / tests).
pub fn set_gc_mode_index() {
    GC_MODE.store(1, std::sync::atomic::Ordering::Relaxed);
}

/// Reset to Slab mode (test-only; the mode is a process-global write-once in
/// production, but `nextest` isolates each test in its own process).
#[cfg(test)]
pub(crate) fn reset_gc_mode_slab() {
    GC_MODE.store(0, std::sync::atomic::Ordering::Relaxed);
}

impl MettaValue {
    /// The arena address this heap handle names, or `None` for an inline scalar
    /// (Bool / i48 Long / Unit / Empty) or in Slab mode (where the non-NaN
    /// payload is a real pointer, not an index). The 32-bit `Addr` occupies bits
    /// [35:4]; flags stay in [3:0]; bits [63:48] are zero, so `is_inline()` is
    /// byte-identical to the slab-pointer case.
    #[inline]
    pub(crate) fn as_arena_addr(&self) -> Option<crate::backend::eval::cesk::index_arena::Addr> {
        if self.is_inline() || !gc_mode_is_index() {
            return None;
        }
        Some(crate::backend::eval::cesk::index_arena::Addr::from_raw(
            (self.tagged >> 4) as u32,
        ))
    }

    /// Construct an index-mode heap handle from an arena `Addr` + 4 flag bits
    /// (`FLAG_HAS_VARIABLES` etc.). Inverse of [`as_arena_addr`](Self::as_arena_addr).
    /// Mode-agnostic bit-packing; only meaningful when the process is in Index mode.
    #[allow(dead_code)] // wired into IndexFactory (Inc 2a-4) + mode-aware decode (Inc 2a-5)
    #[inline]
    pub(crate) fn from_addr(
        addr: crate::backend::eval::cesk::index_arena::Addr,
        flags: usize,
    ) -> Self {
        debug_assert!(flags <= 0xF, "flags must fit in the low 4 bits");
        MettaValue {
            tagged: ((addr.raw() as usize) << 4) | (flags & 0xF),
        }
    }
}

#[cfg(test)]
mod inc2_mode_tests {
    use super::*;
    use crate::backend::eval::cesk::index_arena::Addr;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn index_addr_handle_bit_packing_roundtrips() {
        // Bit-level bijection — does NOT flip the global mode (no cross-test
        // pollution): from_addr packs, and the Index-mode decode `(tagged>>4)`
        // recovers the Addr; flags survive in the low 4 bits; the handle is
        // non-inline (bits [63:48] == 0).
        for &(seg, off, flags) in &[
            (0u32, 0u32, 0usize),
            (1, 5, 1),
            (1000, 200, 0),
            (16383, 262143, 1),
        ] {
            let addr = Addr::new(seg, off);
            let h = MettaValue::from_addr(addr, flags);
            assert!(
                !h.is_inline(),
                "an index handle is a non-NaN (non-inline) value"
            );
            assert_eq!(h.tagged & 0xF, flags, "flags preserved in low 4 bits");
            assert_eq!(
                (h.tagged >> 4) as u32,
                addr.raw(),
                "Addr payload roundtrips"
            );
        }
    }

    // (cfg-gate) Asserts the process default decode mode is Slab and that a
    // slab heap value carries no arena Addr. Under `--features index-gc` the
    // process starts in Index mode (`GC_MODE == 1`) and `global_factory()`
    // yields index handles, so this slab-mode invariant is false by design —
    // run it only in the slab build.
    #[cfg(not(feature = "index-gc"))]
    #[test]
    fn default_mode_is_slab_and_has_no_arena_addr() {
        assert!(!gc_mode_is_index(), "default value-decode mode is Slab");
        // A slab-allocated heap value is not an arena Addr in Slab mode.
        let v = crate::backend::models::global_factory().atom("x");
        assert_eq!(
            v.as_arena_addr(),
            None,
            "slab heap value has no Addr in Slab mode"
        );
        // Inline scalars never have an Addr, regardless of mode.
        let n = crate::backend::models::global_factory().long(7);
        assert_eq!(n.as_arena_addr(), None, "inline scalar has no Addr");
    }
}

/// The actual value enum, allocated in the arena.
///
/// This mirrors MettaValueInner but uses arena-allocated collections.
///
/// **CRITICAL**: `#[repr(align(16))]` is REQUIRED, not optional. `MettaValue`
/// stores instances as tagged pointers where the lowest 4 bits carry flags;
/// `MettaValue::inner_ptr()` masks with `PTR_MASK = !0xF` (line 355) and thus
/// REQUIRES every `MettaValueInner` pointer — including the `INLINE_*` static
/// singletons at lines 602-609 — to be 16-byte aligned. Without this attribute
/// the enum is natural-aligned to 8, and the linker may place static
/// singletons at addresses where `addr & 0xF == 0x8`. `inner_ptr()` then
/// returns `addr - 8`, decodes adjacent `.data` memory, and produces
/// arbitrary corrupt values (e.g. `MettaValueInner::Space(garbage)`).
/// Layout is binary-specific (different binaries linking the same library
/// can land the static at different alignments), making the bug appear as
/// non-determinism across binaries / builds.
#[derive(Debug)]
#[repr(align(16))]
pub enum MettaValueInner {
    /// An atom (symbol, variable, or literal) - string allocated in arena
    Atom(&'static str),
    /// A boolean literal
    Bool(bool),
    /// An integer literal
    Long(i64),
    /// A floating point literal
    Float(f64),
    /// A string literal - string content allocated in arena
    String(&'static str),
    /// An s-expression (list of values) - slice allocated in arena
    SExpr(&'static [MettaValue]),
    /// An error: `Error(offending_expr, detail)`.
    ///
    /// HE-bisimilar shape: stores the OFFENDING expression in slot 1 and the
    /// DETAIL atom in slot 2. The detail is typically a `String` value carrying
    /// the human message, or a structured atom like `BadType` /
    /// `IncorrectNumberOfArguments`.
    Error(MettaValue, MettaValue),
    /// A type (first-class types as atoms)
    Type(MettaValue),
    /// A conjunction of goals (MORK-style logical AND)
    Conjunction(&'static [MettaValue]),
    /// A first-class space value - reuses existing SpaceHandle
    Space(SpaceHandle),
    /// A reference to a mutable state cell (id)
    State(u64),
    /// Unit value for side-effecting operations
    Unit,
    /// A memoization table - reuses existing MemoHandle
    Memo(MemoHandle),
    /// Quoted expression — prevents evaluation, preserves the quote wrapper.
    /// Transparent to introspection: car-atom sees "quote", get-metatype sees "Expression".
    Quoted(MettaValue),
    /// PT-canonical lazy-substituted value marker (2026-05-21).
    ///
    /// The inner value is DATA — do not trigger rule lookup on it during
    /// evaluation. This wrapper is INVISIBLE for `Display` (delegates to inner),
    /// `Hash`, and `PartialEq`. It exists only to inhibit the rule-application
    /// path while keeping the substituted value transparent to introspection
    /// (car-atom / get-metatype / formatting) — implementing PeTTa's
    /// "data-in / data-out" semantic for rules whose LHS head is declared with
    /// an all-meta arrow type (e.g. `(: ? (-> Expression Atom))`).
    ///
    /// Emitted by [`apply_bindings_lazy_scoped_generic`] under the
    /// `op_lhs_head_all_meta_typed` gate. Eval treats `Lazy(x)` as already in
    /// normal form: the trampoline `Eval` arm immediately resumes with `x`
    /// (no rule dispatch) and the step dispatcher returns `Done([x])`.
    Lazy(MettaValue),
    /// Empty sentinel
    Empty,
    /// `NotReducible` sentinel — Plan S0a (2026-05-13).
    ///
    /// HE bisimilarity: emitted by `eval` (S4) when the argument is a grounded
    /// scalar at head, a variable-headed expression with no matching equations,
    /// or a `query` with empty result set. See `hyperon-experimental/lib/src/
    /// metta/mod.rs:29` (`NOT_REDUCIBLE_SYMBOL`) and `lib/src/metta/interpreter.rs:546-548, 634`.
    ///
    /// Zero-payload variant (like `Empty`/`Unit`). Single static instance
    /// referenced via `INLINE_NOT_REDUCIBLE_INNER` for pointer-identity
    /// comparison on the hot path.
    NotReducible,
    /// Source-annotated value — transparent to evaluation, preserves location for LSP/diagnostics.
    /// Nesting is allowed: Spanned(Spanned(v, binding_span), template_span) gives full provenance.
    /// `inner()` auto-strips all Spanned layers, so existing pattern matches work unchanged.
    Spanned(MettaValue, &'static Span),
}

// ============================================================================
// Thread Safety for Static Arena Values
// ============================================================================
//
// MettaValue is safe to share across threads because:
// 1. The global SlabAllocator is lock-free and thread-safe
// 2. MettaValue contains only immutable references to slab-allocated data
// 3. Once created, arena values are never mutated
// 4. The 'static lifetime ensures the referenced data lives until GC reclaims it
//
// The unsafe impl is required because:
// - MettaValueInner contains raw references (&'static [MettaValue]) from slab allocation
// - The Rust compiler requires explicit Send/Sync for types with certain reference patterns
// - However, we only use the slab for allocation, never for mutation after creation
//
// SAFETY INVARIANT: Values must only be read, never mutated, after creation.
// This is enforced by MettaValue's API which provides no mutation methods.

// SAFETY: MettaValue can be sent between threads because:
// - It's an immutable reference to 'static slab-allocated data
// - The global SlabAllocator is thread-safe (lock-free Treiber stack + atomic bump)
// - Once created, the data is never mutated
unsafe impl Send for MettaValue {}

// SAFETY: MettaValue can be shared between threads because:
// - It only provides immutable access to the underlying data
// - No mutation methods exist on MettaValue
// - The referenced data is immutable after creation
unsafe impl Sync for MettaValue {}

// SAFETY: MettaValueInner can be sent between threads for the same reasons
unsafe impl Send for MettaValueInner {}

// SAFETY: MettaValueInner can be shared between threads for the same reasons
unsafe impl Sync for MettaValueInner {}

/// Discriminated view of a `MettaValue` that separates frequently-matched
/// inline-representable variants (Bool, Long, Float, Unit, Empty) from
/// slab-allocated variants.
///
/// This replaces `inner()` for pattern matching in hot paths, enabling future
/// NaN-boxing by centralising the decode logic. For slab-backed values the
/// Spanned wrapper is stripped automatically.
///
/// Every `MettaValueInner` variant (except `Spanned`, which is transparent)
/// has a corresponding `ValueView` variant. This enables exhaustive pattern
/// matching: adding a new `MettaValueInner` variant forces updates at every
/// `match value.view()` site.
#[derive(Debug, Clone, Copy)]
pub enum ValueView {
    // Inline-representable types (future NaN-boxing candidates)
    Float(f64),
    Bool(bool),
    Long(i64),
    Unit,
    Empty,
    /// HE `NotReducible` sentinel (Plan S0a, 2026-05-13).
    NotReducible,
    // Slab-allocated types (Spanned layers are stripped by view())
    Atom(&'static str),
    String(&'static str),
    SExpr(&'static [MettaValue]),
    /// HE-bisimilar: `Error(offending, detail)`.
    Error(MettaValue, MettaValue),
    Type(MettaValue),
    Conjunction(&'static [MettaValue]),
    Space(&'static SpaceHandle),
    State(u64),
    Memo(&'static MemoHandle),
    Quoted(MettaValue),
    /// PT-canonical lazy-substituted value marker — INVISIBLE to display/hash/eq.
    /// See [`MettaValueInner::Lazy`] for full semantics.
    Lazy(MettaValue),
}

impl ValueView {
    /// Map a `ValueView` to its MeTTa metatype string (HE-aligned).
    ///
    /// Single source of truth for `get-metatype` across T0 trampoline, T1
    /// bytecode VM, and T2/T3 JIT. Spanned layers are already stripped by
    /// `view()`. Matches HE `lib/src/metta/types.rs::get_meta_type`
    /// (hyperon-experimental commit referenced in Plan S7).
    ///
    /// HE returns exactly 4 categories — Plan S7 (RC-METATYPE-VOCAB,
    /// 2026-05-14) collapsed MTT's fine-grained vocabulary into HE's 4-set:
    ///   - `Grounded`  — all primitive literals, errors, state, memo, space,
    ///                   conjunctions, type-wrappers, units, empty results,
    ///                   and the NotReducible sentinel
    ///   - `Symbol`    — plain non-variable atoms
    ///   - `Variable`  — `$`-prefixed atoms (MeTTa variable sigil)
    ///   - `Expression`— S-expressions and quoted wrappers
    ///
    /// For HE-internal-dispatch sites that need to distinguish numbers from
    /// strings from bools, match on `view()` directly with the typed variants
    /// (`ValueView::Long`, `ValueView::Bool`, `ValueView::String`, etc.) —
    /// do NOT branch on this string.
    pub fn metatype(&self) -> &'static str {
        match self {
            // Grounded: all primitive literals + grounded-object wrappers.
            // Matches HE which classifies anything backed by a Rust-side
            // grounded value (Number, Bool, String, Error, State, Space,
            // Conjunction, Memo, Type, Unit, Empty, NotReducible) as Grounded.
            ValueView::Bool(_)
            | ValueView::Long(_)
            | ValueView::Float(_)
            | ValueView::String(_)
            | ValueView::Unit
            | ValueView::Empty
            | ValueView::NotReducible
            | ValueView::Error(..)
            | ValueView::State(_)
            | ValueView::Type(_)
            | ValueView::Conjunction(_)
            | ValueView::Space(_)
            | ValueView::Memo(_) => "Grounded",
            // Variable: `$`-prefixed atom sigils
            ValueView::Atom(s) if s.starts_with('$') => "Variable",
            // Symbol: plain non-variable atoms
            ValueView::Atom(_) => "Symbol",
            // Expression: S-expressions and Quoted wrappers (Quoted is
            // transparent at the metatype level)
            ValueView::SExpr(_) | ValueView::Quoted(_) => "Expression",
            // Lazy is INVISIBLE — delegate to inner. PT-canonical
            // data-in / data-out marker (2026-05-21).
            ValueView::Lazy(inner) => inner.view().metatype(),
        }
    }
}

// ==========================================================================
// Static singletons for inline values accessed via inner_ref()
// ==========================================================================
//
// When code calls `inner_ref()` or `inner()` on an inline NaN-boxed value,
// we return a reference to one of these static singletons. This provides
// backward compatibility for code that pattern-matches on `MettaValueInner`.
//
// For Long values, `inner_ref()` cannot return a static reference to a
// dynamic i64 value, so it materializes a slab-allocated MettaValueInner.
// This is the slow path — hot-path code should use `view()` or `as_long()`.

static INLINE_UNIT_INNER: MettaValueInner = MettaValueInner::Unit;
static INLINE_EMPTY_INNER: MettaValueInner = MettaValueInner::Empty;
/// Singleton `MettaValueInner::NotReducible` (Plan S0a, 2026-05-13).
/// Stable static address — `factory.not_reducible()` wraps a reference to
/// this for pointer-identity comparison on the hot path.
pub(crate) static INLINE_NOT_REDUCIBLE_INNER: MettaValueInner = MettaValueInner::NotReducible;
static INLINE_TRUE_INNER: MettaValueInner = MettaValueInner::Bool(true);
static INLINE_FALSE_INNER: MettaValueInner = MettaValueInner::Bool(false);

#[inline]
pub(crate) fn is_inline_singleton_inner_ptr(ptr: *const MettaValueInner) -> bool {
    std::ptr::eq(ptr, &INLINE_UNIT_INNER)
        || std::ptr::eq(ptr, &INLINE_EMPTY_INNER)
        || std::ptr::eq(ptr, &INLINE_TRUE_INNER)
        || std::ptr::eq(ptr, &INLINE_FALSE_INNER)
}

thread_local! {
    /// Index-mode `inner_ref()` materialization cache (CRUX Step 2c): an arena
    /// handle's payload is an `Addr`, not a `*const MettaValueInner`, so a slab
    /// deref is invalid in Index mode. `inner_ref()` instead reads the `Node`
    /// from the global index heap and materializes the slab-era `MettaValueInner`
    /// here, returning a `&'static` to the boxed value.
    ///
    /// Keyed by `Addr.raw()` (NOT call count): the arena is **non-moving**, so a
    /// handle's `Addr` is stable, and repeated `inner_ref()` on the same handle
    /// reuses ONE box — bounding the cache by distinct live Addrs and giving a
    /// **stable** materialized pointer per handle (so `from_inner` round-trips and
    /// pointer-identity comparisons behave). The boxes' pointees are
    /// address-stable across `HashMap` growth (growth moves only the 8-byte `Box`,
    /// never its target), so a laundered `&'static` stays valid until
    /// [`clear_inner_shadow`]. For Inc 2–4 (no live Index sweep) it persists,
    /// bounded by the live heap; Inc 6 clears it on the sweep epoch.
    static INNER_SHADOW: std::cell::RefCell<std::collections::HashMap<u32, Box<MettaValueInner>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Clear the index-mode `inner_ref()` materialization cache. Called at the Inc 6
/// sweep epoch (and on a test mode-reset). No-op effect in Slab mode (the cache
/// is only populated when `gc_mode_is_index()`).
#[allow(dead_code)] // wired into the safepoint/sweep epoch in Inc 6; used by tests now
pub(crate) fn clear_inner_shadow() {
    INNER_SHADOW.with(|c| c.borrow_mut().clear());
}

/// Test-only: number of distinct Addrs currently materialized in the shadow cache.
#[cfg(test)]
pub(crate) fn inner_shadow_len() -> usize {
    INNER_SHADOW.with(|c| c.borrow().len())
}

impl MettaValue {
    // ======================================================================
    // NaN-boxing inline discriminant and construction
    // ======================================================================

    /// Check if this value is a NaN-boxed inline value (Bool, Long, Unit, or Empty).
    ///
    /// Inline values bypass slab allocation entirely — they encode the type and
    /// payload directly in the `tagged` field using IEEE 754 quiet NaN tagging.
    #[inline(always)]
    pub(crate) fn is_inline(&self) -> bool {
        (self.tagged as u64 >> 48) >= NB_MIN_TAG
    }

    /// Get the NaN-boxing tag (upper 16 bits shifted to full u64 tag position).
    /// Only valid when `is_inline()` returns true.
    #[inline(always)]
    fn inline_tag(&self) -> u64 {
        (self.tagged as u64) & NB_TAG_MASK
    }

    /// Create an inline Bool value (no slab allocation).
    #[inline(always)]
    pub(crate) fn inline_bool(b: bool) -> Self {
        Self {
            tagged: (NB_TAG_BOOL | (b as u64)) as usize,
        }
    }

    /// Create an inline Long value if it fits in 48 bits, otherwise None.
    /// Range: -(2^47) to (2^47)-1 = ±140,737,488,355,328.
    #[inline(always)]
    pub(crate) fn try_inline_long(n: i64) -> Option<Self> {
        if n >= NB_LONG_MIN && n <= NB_LONG_MAX {
            Some(Self {
                tagged: (NB_TAG_LONG | (n as u64 & NB_PAYLOAD_MASK)) as usize,
            })
        } else {
            None
        }
    }

    /// Create an inline Unit value (no slab allocation).
    #[inline(always)]
    pub(crate) fn inline_unit() -> Self {
        Self {
            tagged: NB_TAG_UNIT as usize,
        }
    }

    /// Create an inline Empty value (no slab allocation).
    #[inline(always)]
    pub(crate) fn inline_empty() -> Self {
        Self {
            tagged: NB_TAG_EMPTY as usize,
        }
    }

    /// Extract the i64 value from an inline Long.
    /// Only valid when `inline_tag() == NB_TAG_LONG`.
    #[inline(always)]
    fn inline_long_value(&self) -> i64 {
        let payload = (self.tagged as u64) & NB_PAYLOAD_MASK;
        // Sign-extend from 48 bits to 64 bits
        if payload & NB_SIGN_BIT_48 != 0 {
            (payload | !NB_PAYLOAD_MASK) as i64
        } else {
            payload as i64
        }
    }

    // ======================================================================
    // Core accessors (inline-aware)
    // ======================================================================

    /// Dereference the tagged pointer to get the inner value.
    ///
    /// For slab-pointer values, masks off flag bits before dereferencing.
    /// For inline NaN-boxed values, returns a static singleton (Bool/Unit/Empty)
    /// or materializes a slab-allocated MettaValueInner (Long — slow path).
    ///
    /// **Prefer `view()` or typed accessors (`as_long()`, `as_bool()`) in hot paths.**
    #[inline]
    pub(crate) fn inner_ref(&self) -> &'static MettaValueInner {
        if self.is_inline() {
            return self.inner_ref_inline();
        }
        // Index-arena mode (CRUX Step 2c): the payload is an `Addr`, not a slab
        // pointer — materialize the `MettaValueInner` from the heap (Addr-keyed
        // shadow cache). Default Slab mode skips this perfectly-predicted branch,
        // so the slab deref below is byte-identical.
        if gc_mode_is_index() {
            return self.inner_ref_index();
        }
        unsafe { &*((self.tagged & PTR_MASK) as *const MettaValueInner) }
    }

    /// Index-mode materialization of `inner_ref()` (cold; see [`INNER_SHADOW`]).
    /// Reads the `Node` at this handle's `Addr` and returns a `&'static`
    /// `MettaValueInner` boxed in the Addr-keyed per-thread shadow cache. Does NOT
    /// strip `Spanned` (matches the slab `inner_ref` contract).
    #[cold]
    #[inline(never)]
    fn inner_ref_index(&self) -> &'static MettaValueInner {
        let raw = (self.tagged >> 4) as u32;
        INNER_SHADOW.with(|c| {
            let mut m = c.borrow_mut();
            // HIT iff the Addr is already cached: then `or_insert_with` runs NO
            // closure (no heap read-lock taken below), so the swept check's lone
            // read-lock is unnested. On a MISS the closure takes+releases the lock
            // before `or_insert_with` returns, and `was_hit` is false ⇒ no check.
            let was_hit = m.contains_key(&raw);
            let boxed = m.entry(raw).or_insert_with(|| {
                let addr = crate::backend::eval::cesk::index_arena::Addr::from_raw(raw);
                Box::new(
                    crate::backend::eval::cesk::index_heap::global_index_heap()
                        .read()
                        .expect("index heap poisoned")
                        .materialize_inner(addr),
                )
            });
            // SAFETY: the box's pointee is address-stable for the cache's life
            // (HashMap growth relocates only the 8-byte Box value, not its target),
            // and the entry is removed only by clear_inner_shadow() at a quiescent
            // point — so the laundered &'static outlives this borrow_mut guard.
            let out = unsafe { &*(&**boxed as *const MettaValueInner) };
            // DEBUG-ONLY swept-slot oracle (INNER_SHADOW-cache-hit extension): a
            // HIT for a swept Addr by a NON-collector thread is a stale ABA read —
            // the slot was swept+reused, but this per-thread shadow cache still
            // holds the OLD occupant's box (the cache is keyed by the raw u32 Addr,
            // not the live node), so the holder reads stale content WITHOUT going
            // through the swept-checked `arena.get()`. Panic so the backtrace names
            // the missed-root holder. Gated on the oracle env flag (no-op when off,
            // so non-oracle/slab runs are byte-identical) and exempts the dedicated
            // collector thread (its re-walks legitimately read swept slots). The
            // read-lock here is safe: this branch only runs on a HIT (the miss path
            // above took NO lock), and no heap lock is held inside `INNER_SHADOW`.
            if was_hit
                && crate::backend::eval::cesk::index_arena::swept_oracle_enabled()
                && !crate::backend::eval::cesk::index_arena::in_collector_read_scope_pub()
            {
                let addr = crate::backend::eval::cesk::index_arena::Addr::from_raw(raw);
                if crate::backend::eval::cesk::index_heap::global_index_heap()
                    .read()
                    .expect("index heap poisoned")
                    .is_addr_swept(addr)
                {
                    panic!(
                        "SWEPT INNER_SHADOW HIT (ABA missed-root): addr={:?} raw={}",
                        addr, raw
                    );
                }
            }
            out
        })
    }

    /// Slow path for `inner_ref()` on inline NaN-boxed values.
    /// Returns static singletons for Bool/Unit/Empty.
    /// For Long, materializes a slab-allocated MettaValueInner.
    #[cold]
    #[inline(never)]
    fn inner_ref_inline(&self) -> &'static MettaValueInner {
        match self.inline_tag() {
            NB_TAG_BOOL => {
                if (self.tagged as u64) & 1 != 0 {
                    &INLINE_TRUE_INNER
                } else {
                    &INLINE_FALSE_INNER
                }
            }
            NB_TAG_UNIT => &INLINE_UNIT_INNER,
            NB_TAG_EMPTY => &INLINE_EMPTY_INNER,
            NB_TAG_LONG => {
                // Slow path: materialize in slab. Callers should use view()/as_long().
                let n = self.inline_long_value();
                let alloc = super::gc_allocator::global_allocator();
                alloc.alloc_value(MettaValueInner::Long(n))
            }
            _ => {
                // Unknown inline tag — should not happen
                debug_assert!(
                    false,
                    "unknown inline tag: 0x{:04x}",
                    self.inline_tag() >> 48
                );
                &INLINE_UNIT_INNER
            }
        }
    }

    /// O(1) flag check: does this value (or any sub-value) contain variables?
    ///
    /// Inline NaN-boxed values (Bool, Long, Unit, Empty) never contain variables.
    /// For slab-pointer values, checks the FLAG_HAS_VARIABLES bit.
    #[inline]
    pub fn has_variables_fast(&self) -> bool {
        let t = self.tagged as u64;
        // Inline values never have variables. Slab pointers have (t >> 48) == 0
        // on x86-64 (user-space canonical addresses). Short-circuit: if bit 0
        // is clear, no variables regardless of encoding.
        (t & FLAG_HAS_VARIABLES as u64 != 0) && (t >> 48) < NB_MIN_TAG
    }

    /// Construct from inner reference with computed flags.
    #[inline]
    pub(crate) fn from_inner_tagged(inner: &'static MettaValueInner, flags: u8) -> Self {
        debug_assert!(flags < 16, "tagged pointer flags must fit in 4 bits");
        debug_assert!(
            (inner as *const MettaValueInner as usize) & 0xF == 0,
            "inner pointer must be 16-byte aligned (SLOT_ALIGN=16)"
        );
        Self {
            tagged: inner as *const MettaValueInner as usize | flags as usize,
        }
    }

    /// Access the inner enum for pattern matching.
    ///
    /// **Automatically strips all `Spanned` layers**, so existing pattern matches
    /// work unchanged. Use `inner_raw()` to access the raw representation including
    /// any Spanned wrapper.
    ///
    /// For inline NaN-boxed values, returns a static singleton or materialized inner.
    /// **Prefer `view()` in hot paths** — it decodes inline values directly without
    /// materializing a MettaValueInner.
    #[inline]
    pub fn inner(&self) -> &'static MettaValueInner {
        // Fast path: inline values are never Spanned
        if self.is_inline() {
            return self.inner_ref_inline();
        }
        let mut current = self.inner_ref();
        loop {
            match current {
                MettaValueInner::Spanned(v, _) => {
                    // Spanned inner is always a slab pointer (or another inline)
                    if v.is_inline() {
                        return v.inner_ref_inline();
                    }
                    current = v.inner_ref();
                }
                _ => return current,
            }
        }
    }

    /// Return a [`ValueView`] that decomposes this value into one variant per
    /// logical type, stripping Spanned layers automatically.
    ///
    /// This centralises the decode logic for pattern matching and is the
    /// preferred entry point for `match` expressions in hot paths.
    /// Inline NaN-boxed values are decoded directly without slab dereference.
    #[inline]
    pub fn view(&self) -> ValueView {
        // Fast path: inline NaN-boxed values — decode from tagged bits directly
        if self.is_inline() {
            return match self.inline_tag() {
                NB_TAG_LONG => ValueView::Long(self.inline_long_value()),
                NB_TAG_BOOL => ValueView::Bool((self.tagged as u64 & 1) != 0),
                NB_TAG_UNIT => ValueView::Unit,
                NB_TAG_EMPTY => ValueView::Empty,
                _ => ValueView::Unit, // unreachable
            };
        }
        // Index-arena mode (Inc 2a-5): the non-NaN payload is an arena `Addr`, not
        // a slab pointer — decode the `Node` directly from the global index heap
        // (Spanned-stripped, `'static`-laundered). Default Slab mode skips this
        // perfectly-predicted branch, so the slab path below is byte-identical.
        if gc_mode_is_index() {
            let addr =
                crate::backend::eval::cesk::index_arena::Addr::from_raw((self.tagged >> 4) as u32);
            return crate::backend::eval::cesk::index_heap::global_index_heap()
                .read()
                .expect("index heap poisoned")
                .view_at(addr);
        }
        let inner = self.inner();
        match inner {
            MettaValueInner::Float(f) => ValueView::Float(*f),
            MettaValueInner::Bool(b) => ValueView::Bool(*b),
            MettaValueInner::Long(n) => ValueView::Long(*n),
            MettaValueInner::Unit => ValueView::Unit,
            MettaValueInner::Empty => ValueView::Empty,
            MettaValueInner::Atom(s) => ValueView::Atom(s),
            MettaValueInner::String(s) => ValueView::String(s),
            MettaValueInner::SExpr(items) => ValueView::SExpr(items),
            MettaValueInner::Error(offending, details) => ValueView::Error(*offending, *details),
            MettaValueInner::Type(inner_val) => ValueView::Type(*inner_val),
            MettaValueInner::Conjunction(goals) => ValueView::Conjunction(goals),
            MettaValueInner::Space(handle) => ValueView::Space(handle),
            MettaValueInner::State(id) => ValueView::State(*id),
            MettaValueInner::Memo(handle) => ValueView::Memo(handle),
            MettaValueInner::Quoted(inner_val) => ValueView::Quoted(*inner_val),
            MettaValueInner::Lazy(inner_val) => ValueView::Lazy(*inner_val),
            MettaValueInner::NotReducible => ValueView::NotReducible,
            // Spanned is stripped by inner() — this is unreachable
            MettaValueInner::Spanned(..) => unreachable!("inner() strips Spanned"),
        }
    }

    /// Access the raw inner representation **without** stripping Spanned layers.
    ///
    /// Use this for span-aware code that needs to inspect the Spanned wrapper.
    /// Most code should use `inner()` instead.
    ///
    /// For inline NaN-boxed values, returns a materialized singleton (no Spanned possible).
    #[inline]
    pub fn inner_raw(&self) -> &'static MettaValueInner {
        self.inner_ref()
    }

    /// Get the outermost source span, if this value is wrapped in `Spanned`.
    ///
    /// Returns the span of the outermost Spanned layer (typically the template/usage site).
    /// Returns `None` for bare values without span annotations.
    #[inline]
    pub fn span(&self) -> Option<&'static Span> {
        if self.is_inline() {
            return None;
        } // Inline values are never Spanned
        match self.inner_ref() {
            MettaValueInner::Spanned(_, span) => Some(span),
            _ => None,
        }
    }

    /// Collect all spans from outermost to innermost (full provenance chain).
    ///
    /// - `[0]` = template/usage site (outermost)
    /// - `[1]` = binding origin
    /// - `[2..]` = deeper origins
    ///
    /// Returns an empty Vec for bare values without span annotations.
    pub fn spans(&self) -> Vec<&'static Span> {
        let mut spans = Vec::with_capacity(2);
        let mut current = self.inner_ref();
        loop {
            match current {
                MettaValueInner::Spanned(v, span) => {
                    spans.push(*span);
                    current = v.inner_ref();
                }
                _ => break,
            }
        }
        spans
    }

    /// Strip ALL Spanned wrappers, returning the bare value.
    ///
    /// The returned MettaValue points directly to the non-Spanned inner value.
    /// Inline NaN-boxed values are returned as-is (never Spanned).
    #[inline]
    pub fn strip_spans(&self) -> MettaValue {
        if self.is_inline() {
            return *self;
        }
        // Index mode (CRUX Step 5): `from_inner(self.inner())` would pack the
        // materialized shadow pointer as if it were a handle — wrong. Instead peel
        // `Spanned` layers via the (mode-aware) `peel_span`, returning the bare
        // handle directly. The slab arm below is unchanged.
        if gc_mode_is_index() {
            let mut current = *self;
            loop {
                let (inner, sp) = current.peel_span();
                if sp.is_none() {
                    return current;
                }
                current = inner;
            }
        }
        MettaValue::from_inner(self.inner())
    }

    /// Strip one layer of Spanned, returning the inner value and its span.
    ///
    /// If the value is not Spanned, returns `(self, None)`.
    /// Inline NaN-boxed values are never Spanned.
    #[inline]
    pub fn peel_span(&self) -> (MettaValue, Option<&'static Span>) {
        if self.is_inline() {
            return (*self, None);
        }
        match self.inner_ref() {
            MettaValueInner::Spanned(v, span) => (*v, Some(span)),
            _ => (*self, None),
        }
    }

    /// Construct an MettaValue from a reference to an MettaValueInner.
    /// Flags default to 0 — use `from_inner_tagged` to set flags.
    #[inline]
    pub fn from_inner(inner: &'static MettaValueInner) -> Self {
        Self {
            tagged: inner as *const MettaValueInner as usize,
        }
    }

    /// Get a raw pointer to the inner value (used by GC for slot identification).
    ///
    /// Returns the pointer to the **outermost** MettaValueInner (which may be Spanned).
    /// This is correct for GC marking, which needs to track the actual slab slot.
    /// For inline NaN-boxed values, returns null (no slab slot to mark).
    #[inline]
    pub fn inner_ptr(&self) -> *const MettaValueInner {
        if self.is_inline() {
            return std::ptr::null();
        }
        // Index-arena mode (CRUX Step 3a): the payload is an `Addr`, not a slab
        // pointer. Return a stable, collision-free KEY derived from the Addr —
        // every shared caller uses `inner_ptr()` only as a hash key / identity
        // compare, never dereferencing it (verified: conformance_common,
        // eval/types cycle-set, alpha_equiv fast-path, eval_loop fixpoint, mork
        // ground-cache). `INDEX_KEY_TAG` (bit 48, above the 32-bit Addr payload)
        // keeps the key non-null even for `Addr(0)` and disjoint from any real
        // low-48-bit slab pointer. Stable because the arena is non-moving.
        if gc_mode_is_index() {
            const INDEX_KEY_TAG: usize = 1 << 48;
            return (INDEX_KEY_TAG | (self.tagged >> 4)) as *const MettaValueInner;
        }
        (self.tagged & PTR_MASK) as *const MettaValueInner
    }

    // ========================================================================
    // Type checking and inspection methods
    // ========================================================================

    /// Check if this is an Atom variant (transparent through Spanned)
    #[inline]
    pub fn is_atom(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Atom(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_atom(),
            _ => false,
        }
    }

    /// Check if this is a Bool variant (transparent through Spanned)
    #[inline]
    pub fn is_bool(&self) -> bool {
        if self.is_inline() {
            return self.inline_tag() == NB_TAG_BOOL;
        }
        match self.inner_ref() {
            MettaValueInner::Bool(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_bool(),
            _ => false,
        }
    }

    /// Check if this is a Long variant (transparent through Spanned)
    #[inline]
    pub fn is_long(&self) -> bool {
        if self.is_inline() {
            return self.inline_tag() == NB_TAG_LONG;
        }
        match self.inner_ref() {
            MettaValueInner::Long(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_long(),
            _ => false,
        }
    }

    /// Check if this is a Float variant (transparent through Spanned)
    #[inline]
    pub fn is_float(&self) -> bool {
        if self.is_inline() {
            return false;
        } // Floats are always slab-allocated
        match self.inner_ref() {
            MettaValueInner::Float(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_float(),
            _ => false,
        }
    }

    /// Check if this is a String variant (transparent through Spanned)
    #[inline]
    pub fn is_string(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::String(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_string(),
            _ => false,
        }
    }

    /// Check if this is an SExpr variant (transparent through Spanned)
    #[inline]
    pub fn is_sexpr(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::SExpr(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_sexpr(),
            _ => false,
        }
    }

    /// Check if this is an Error variant (transparent through Spanned)
    #[inline]
    pub fn is_error(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Error(_, _) => true,
            MettaValueInner::Spanned(v, _) => v.is_error(),
            _ => false,
        }
    }

    /// Check if this value is an error *sentinel* — either the dedicated
    /// `Error` variant OR the user-level surface form `(Error <call> <detail>)`
    /// (an SExpr whose head is `Atom("Error")`). T1 bytecode often produces
    /// the SExpr form when compiling literal Error atoms; using this helper
    /// keeps arithmetic / comparison op-error-arg checks tier-uniform.
    /// Distinct from `is_error()` which matches the narrow variant only.
    #[inline]
    pub fn is_error_sentinel(&self) -> bool {
        if self.is_error() {
            return true;
        }
        if let Some(items) = self.as_sexpr() {
            return items.first().and_then(|h| h.as_atom()) == Some("Error");
        }
        false
    }

    /// Check if this is a Type variant (transparent through Spanned)
    #[inline]
    pub fn is_type(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Type(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_type(),
            _ => false,
        }
    }

    /// Check if this is a Conjunction variant (transparent through Spanned)
    #[inline]
    pub fn is_conjunction(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Conjunction(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_conjunction(),
            _ => false,
        }
    }

    /// Check if this is a Space variant (transparent through Spanned)
    #[inline]
    pub fn is_space(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Space(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_space(),
            _ => false,
        }
    }

    /// Check if this is a State variant (transparent through Spanned)
    #[inline]
    pub fn is_state(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::State(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_state(),
            _ => false,
        }
    }

    /// Check if this is a Unit variant (transparent through Spanned)
    #[inline]
    pub fn is_unit(&self) -> bool {
        if self.is_inline() {
            return self.inline_tag() == NB_TAG_UNIT;
        }
        match self.inner_ref() {
            MettaValueInner::Unit => true,
            MettaValueInner::Spanned(v, _) => v.is_unit(),
            _ => false,
        }
    }

    /// Check if this is a Memo variant (transparent through Spanned)
    #[inline]
    pub fn is_memo(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Memo(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_memo(),
            _ => false,
        }
    }

    /// Check if this is a Quoted variant (transparent through Spanned)
    #[inline]
    pub fn is_quoted(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Quoted(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_quoted(),
            _ => false,
        }
    }

    /// Check if this is an Empty variant (transparent through Spanned)
    #[inline]
    pub fn is_empty(&self) -> bool {
        if self.is_inline() {
            return self.inline_tag() == NB_TAG_EMPTY;
        }
        match self.inner_ref() {
            MettaValueInner::Empty => true,
            MettaValueInner::Spanned(v, _) => v.is_empty(),
            _ => false,
        }
    }

    /// Check if this value is the Empty *sentinel* — i.e. either the
    /// dedicated `ValueView::Empty` variant OR the user-visible symbol
    /// `Atom("Empty")`. Use this in spec §06.4.5 / §10.4 filter sites
    /// (collapse Empty filtering, top-level directive return) where HE
    /// treats both representations as the same sentinel.
    /// Distinct from `is_empty()` which matches the narrow variant only.
    #[inline]
    pub fn is_empty_sentinel(&self) -> bool {
        self.is_empty() || self.as_atom() == Some("Empty")
    }

    /// Check if this value is a variable (Atom starting with $) (transparent through Spanned)
    #[inline]
    pub fn is_variable(&self) -> bool {
        if self.is_inline() {
            return false;
        } // Inline types are never variables
        match self.inner_ref() {
            MettaValueInner::Atom(s) if s.starts_with('$') => true,
            MettaValueInner::Spanned(v, _) => v.is_variable(),
            _ => false,
        }
    }

    /// Check if this value is a Spanned variant
    #[inline]
    pub fn is_spanned(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        matches!(self.inner_ref(), MettaValueInner::Spanned(_, _))
    }

    // ========================================================================
    // Accessor methods for extracting inner values
    // ========================================================================

    /// Try to extract as atom string (transparent through Spanned)
    #[inline]
    pub fn as_atom(&self) -> Option<&'static str> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Atom(s) => Some(s),
            MettaValueInner::Spanned(v, _) => v.as_atom(),
            _ => None,
        }
    }

    /// Try to extract as bool (transparent through Spanned)
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_inline() {
            return if self.inline_tag() == NB_TAG_BOOL {
                Some((self.tagged as u64 & 1) != 0)
            } else {
                None
            };
        }
        match self.inner_ref() {
            MettaValueInner::Bool(b) => Some(*b),
            MettaValueInner::Spanned(v, _) => v.as_bool(),
            _ => None,
        }
    }

    /// Try to extract as i64 (transparent through Spanned)
    #[inline]
    pub fn as_long(&self) -> Option<i64> {
        if self.is_inline() {
            return if self.inline_tag() == NB_TAG_LONG {
                Some(self.inline_long_value())
            } else {
                None
            };
        }
        match self.inner_ref() {
            MettaValueInner::Long(n) => Some(*n),
            MettaValueInner::Spanned(v, _) => v.as_long(),
            // PT-canonical Lazy is a transparent inert/rule-inhibitor marker
            // (see eval_loop.rs:~3235); its numeric value is the inner's, just
            // as Display delegates to the inner. Without this, arithmetic and
            // comparisons on a substituted-then-Lazy-wrapped number (e.g. an
            // stv confidence threaded through PLN's Truth_Revision) saw `None`
            // and silently treated it as non-numeric — collapsing confidences
            // to 0.0 (the Direct.metta D2 conf=0.0 phantom).
            MettaValueInner::Lazy(v) => v.as_long(),
            _ => None,
        }
    }

    /// Try to extract as f64 (transparent through Spanned)
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        if self.is_inline() {
            return None;
        } // Floats are always slab-allocated
        match self.inner_ref() {
            MettaValueInner::Float(f) => Some(*f),
            MettaValueInner::Spanned(v, _) => v.as_float(),
            // Lazy-transparent (see as_long above): a Lazy-wrapped float must
            // coerce to its number so threaded stv confidences arithmetic-
            // correctly (Direct.metta D2 conf=0.0 phantom fix).
            MettaValueInner::Lazy(v) => v.as_float(),
            _ => None,
        }
    }

    /// Try to extract as string (transparent through Spanned)
    #[inline]
    pub fn as_string(&self) -> Option<&'static str> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::String(s) => Some(s),
            MettaValueInner::Spanned(v, _) => v.as_string(),
            _ => None,
        }
    }

    /// Try to extract as sexpr items (transparent through Spanned).
    ///
    /// NOTE: NOT transparent through Lazy — that's intentional. Lazy is a
    /// rule-dispatch inhibitor (a substituted value wrapped in Lazy must not
    /// match any rule LHS even if structurally an SExpr). Callers that want
    /// the structural shape of a Lazy-wrapped value should call
    /// `unwrap_lazy()` first. See PT-canonical Lazy semantics (2026-05-21).
    #[inline]
    pub fn as_sexpr(&self) -> Option<&[MettaValue]> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::SExpr(items) => Some(items),
            MettaValueInner::Spanned(v, _) => v.as_sexpr(),
            _ => None,
        }
    }

    /// Try to extract as error `(type, ctx)` (transparent through Spanned).
    ///
    /// Phase 1.1 PT alignment (2026-05-22): the tuple is `(error_type, ctx)`
    /// per PT canonical `(Error <Type> <Ctx>)` shape (PHE-009, finer #4).
    /// Previously the shape was `(offending, detail)` (offending first); the
    /// argument order has been flipped throughout the codebase. The Error
    /// variant fields hold `(type, ctx)` where `type` is typically an atom
    /// like `BadType` / `IncorrectNumberOfArguments` / `BadArgType` and `ctx`
    /// is the offending expression / context info.
    #[inline]
    pub fn as_error(&self) -> Option<(MettaValue, MettaValue)> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Error(offending, details) => Some((*offending, *details)),
            MettaValueInner::Spanned(v, _) => v.as_error(),
            _ => None,
        }
    }

    /// Try to extract as type inner value (transparent through Spanned)
    #[inline]
    pub fn as_type(&self) -> Option<MettaValue> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Type(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_type(),
            _ => None,
        }
    }

    /// Try to extract as conjunction goals (transparent through Spanned)
    #[inline]
    pub fn as_conjunction(&self) -> Option<&[MettaValue]> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Conjunction(goals) => Some(goals),
            MettaValueInner::Spanned(v, _) => v.as_conjunction(),
            _ => None,
        }
    }

    /// Try to extract as space handle (transparent through Spanned)
    #[inline]
    pub fn as_space(&self) -> Option<&SpaceHandle> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Space(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_space(),
            _ => None,
        }
    }

    /// Try to extract as state id (transparent through Spanned)
    #[inline]
    pub fn as_state(&self) -> Option<u64> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::State(id) => Some(*id),
            MettaValueInner::Spanned(v, _) => v.as_state(),
            _ => None,
        }
    }

    /// Try to extract as memo handle (transparent through Spanned)
    #[inline]
    pub fn as_memo(&self) -> Option<&MemoHandle> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Memo(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_memo(),
            _ => None,
        }
    }

    /// Try to extract the inner value of a Quoted variant (owned copy) (transparent through Spanned)
    #[inline]
    pub fn as_quoted(&self) -> Option<MettaValue> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted(),
            _ => None,
        }
    }

    /// Try to extract a reference to the inner value of a Quoted variant (transparent through Spanned)
    #[inline]
    pub fn as_quoted_ref(&self) -> Option<&MettaValue> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted_ref(),
            _ => None,
        }
    }

    /// Try to extract the inner value of a Lazy variant (owned copy)
    /// (transparent through Spanned). PT-canonical lazy-substitution marker
    /// (2026-05-21).
    #[inline]
    pub fn as_lazy(&self) -> Option<MettaValue> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Lazy(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_lazy(),
            _ => None,
        }
    }

    /// Try to extract a reference to the inner value of a Lazy variant
    /// (transparent through Spanned). PT-canonical lazy-substitution marker
    /// (2026-05-21).
    #[inline]
    pub fn as_lazy_ref(&self) -> Option<&MettaValue> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Lazy(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => v.as_lazy_ref(),
            _ => None,
        }
    }

    /// Check if this is a Lazy variant (transparent through Spanned).
    /// PT-canonical lazy-substitution marker (2026-05-21).
    #[inline]
    pub fn is_lazy(&self) -> bool {
        if self.is_inline() {
            return false;
        }
        match self.inner_ref() {
            MettaValueInner::Lazy(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_lazy(),
            _ => false,
        }
    }

    /// Unwrap any number of Lazy layers, returning the innermost
    /// non-Lazy value. Spanned-transparent. PT-canonical (2026-05-21).
    #[inline]
    pub fn unwrap_lazy(&self) -> MettaValue {
        let mut current = *self;
        loop {
            match current.inner_ref() {
                MettaValueInner::Lazy(inner) => current = *inner,
                _ => return current,
            }
        }
    }

    /// Get the type name of this value as a string slice (transparent through Spanned)
    pub fn type_name(&self) -> &'static str {
        if self.is_inline() {
            return match self.inline_tag() {
                NB_TAG_BOOL => "Bool",
                NB_TAG_LONG => "Number",
                NB_TAG_UNIT => "Unit",
                NB_TAG_EMPTY => "Empty",
                _ => "Unit",
            };
        }
        match self.inner_ref() {
            MettaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            MettaValueInner::Atom(_) => "Symbol",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::Long(_) => "Number",
            MettaValueInner::Float(_) => "Number",
            MettaValueInner::String(_) => "String",
            MettaValueInner::SExpr(_) => "Expression",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Quoted(_) => "Expression",
            // PT-canonical Lazy is INVISIBLE for type_name (2026-05-21):
            // delegate to inner so introspection sees the substituted value's
            // shape, not the lazy wrapper.
            MettaValueInner::Lazy(inner) => inner.type_name(),
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
            MettaValueInner::NotReducible => "NotReducible",
            MettaValueInner::Spanned(v, _) => v.type_name(),
        }
    }
}

// ============================================================================
// Backward-Compat Constructors for MettaValue = MettaValue
// ============================================================================
//
// These associated functions preserve the old `MettaValue::Atom(s)` construction
// syntax now that MettaValue is a type alias for MettaValue.
// They delegate to the global GC slab allocator.

impl MettaValue {
    /// Create an Atom variant via global allocator.
    /// Backward-compat: `MettaValue::Atom("symbol")` or `MettaValue::Atom(owned_string)`
    #[allow(non_snake_case)]
    #[inline]
    pub fn Atom(s: impl AsRef<str>) -> Self {
        super::gc_allocator::global_factory().atom(s.as_ref())
    }

    /// Create a Bool variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Bool(b: bool) -> Self {
        super::gc_allocator::global_factory().bool(b)
    }

    /// Create a Long variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Long(n: i64) -> Self {
        super::gc_allocator::global_factory().long(n)
    }

    /// Create a Float variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Float(f: f64) -> Self {
        super::gc_allocator::global_factory().float(f)
    }

    /// Create a String variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn String(s: impl AsRef<str>) -> Self {
        super::gc_allocator::global_factory().string(s.as_ref())
    }

    /// Create an SExpr variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn SExpr(items: Vec<MettaValue>) -> Self {
        super::gc_allocator::global_factory().sexpr(items)
    }

    /// Create an Error variant via global allocator.
    ///
    /// HE-bisimilar shape: `Error(offending_expr, detail)`. The detail is
    /// typically a `String` value carrying the human message, or a structured
    /// atom like `BadType` / `IncorrectNumberOfArguments`.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Error(error_type: MettaValue, ctx: MettaValue) -> Self {
        // Phase 1.1 PT-canonical: Error(Type, Ctx) — pass through verbatim.
        super::gc_allocator::global_factory().error(error_type, ctx)
    }

    /// Create a Type variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Type(inner: MettaValue) -> Self {
        super::gc_allocator::global_factory().type_value(inner)
    }

    /// Create a Conjunction variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Conjunction(goals: Vec<MettaValue>) -> Self {
        super::gc_allocator::global_factory().conjunction(goals)
    }

    /// Create a Space variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Space(handle: SpaceHandle) -> Self {
        super::gc_allocator::global_factory().space(handle)
    }

    /// Create a State variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn State(id: u64) -> Self {
        super::gc_allocator::global_factory().state(id)
    }

    /// Create a Unit variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Unit() -> Self {
        super::gc_allocator::global_factory().unit()
    }

    /// Create a Memo variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Memo(handle: MemoHandle) -> Self {
        super::gc_allocator::global_factory().memo(handle)
    }

    /// Create an Empty variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Empty() -> Self {
        super::gc_allocator::global_factory().empty()
    }

    /// Create a `NotReducible` sentinel via global allocator — Plan S0a (2026-05-13).
    #[allow(non_snake_case)]
    #[inline]
    pub fn NotReducible() -> Self {
        super::gc_allocator::global_factory().not_reducible()
    }

    // ========================================================================
    // Helper constructors (backward compat)
    // ========================================================================

    /// Create a symbol atom from a string slice.
    #[inline]
    pub fn sym(s: &str) -> Self {
        super::gc_allocator::global_factory().atom(s)
    }

    /// Create a variable atom (prefixed with $).
    #[inline]
    pub fn var(name: &str) -> Self {
        super::gc_allocator::global_factory().atom(&format!("${}", name))
    }

    /// Create a Quoted variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Quoted(inner: Self) -> Self {
        super::gc_allocator::global_factory().quote(inner)
    }

    /// Create a PT-canonical Lazy variant via global allocator (2026-05-21).
    /// The Lazy wrapper is INVISIBLE for display/hash/equality — it exists
    /// only to inhibit rule lookup during evaluation.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Lazy(inner: Self) -> Self {
        super::gc_allocator::global_factory().lazy(inner)
    }

    /// Create a Spanned variant wrapping a value with its source location.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Spanned(value: Self, span: crate::ir::Span) -> Self {
        super::gc_allocator::global_factory().spanned(value, span)
    }

    /// Create a quoted expression.
    pub fn quote(inner: Self) -> Self {
        super::gc_allocator::global_factory().quote(inner)
    }

    /// Check if two values point to the same inner allocation (pointer equality).
    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.inner_ptr() == other.inner_ptr()
    }

    /// Backward-compat no-op: returns self reference.
    /// Previously returned `&Arc<MettaValueInner>`.
    #[inline]
    pub fn arc(&self) -> &Self {
        self
    }

    // ========================================================================
    // Methods formerly only on heap MettaValue
    // ========================================================================

    // (helpers for iterative+memoized formatters; defined below)

    /// Convert to canonical MeTTa string representation.
    /// Produces syntax that can be round-trip parsed by the MeTTa parser.
    /// Guarantees: parse(to_metta_string(value)) == value
    /// **Stack-safety + memory-safety fix (2026-05-15)**: iterative + memoized.
    /// Was recursive and exponentially vulnerable; now uses a heap work-list
    /// and memo keyed by slab pointer. See `to_display_string` for rationale.
    pub fn to_metta_string(&self) -> String {
        format_value_iterative(self, FormatStyle::MettaString)
    }

    /// **Stack-safety + memory-safety fix (2026-05-15)**: iterative + memoized.
    pub fn to_mork_string(&self) -> String {
        format_value_iterative(self, FormatStyle::MorkString)
    }

    /// Convert to a JSON-like string representation.
    pub fn to_json_string(&self) -> String {
        match self.inner_ref() {
            MettaValueInner::Atom(s) => {
                format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s))
            }
            MettaValueInner::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
            MettaValueInner::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
            MettaValueInner::Float(f) => format!(r#"{{"type":"float","value":{}}}"#, f),
            MettaValueInner::String(s) => {
                format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s))
            }
            MettaValueInner::Unit => r#"{"type":"unit"}"#.to_string(),
            MettaValueInner::SExpr(items) => {
                let items_json: Vec<String> =
                    items.iter().map(|value| value.to_json_string()).collect();
                format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
            }
            MettaValueInner::Error(offending, detail) => {
                format!(
                    r#"{{"type":"error","offending":{},"detail":{}}}"#,
                    offending.to_json_string(),
                    detail.to_json_string()
                )
            }
            MettaValueInner::Type(t) => {
                format!(r#"{{"type":"metatype","value":{}}}"#, t.to_json_string())
            }
            MettaValueInner::Conjunction(goals) => {
                let goals_json: Vec<String> =
                    goals.iter().map(|value| value.to_json_string()).collect();
                format!(
                    r#"{{"type":"conjunction","goals":[{}]}}"#,
                    goals_json.join(",")
                )
            }
            MettaValueInner::Space(handle) => {
                format!(
                    r#"{{"type":"space","id":{},"name":"{}"}}"#,
                    handle.id,
                    escape_json(&handle.name)
                )
            }
            MettaValueInner::State(id) => {
                format!(r#"{{"type":"state","id":{}}}"#, id)
            }
            MettaValueInner::Memo(handle) => {
                format!(
                    r#"{{"type":"memo","id":{},"name":"{}"}}"#,
                    handle.id,
                    escape_json(&handle.name)
                )
            }
            MettaValueInner::Quoted(inner) => {
                format!(r#"{{"type":"quoted","value":{}}}"#, inner.to_json_string())
            }
            MettaValueInner::Lazy(inner) => {
                // PT-canonical Lazy is INVISIBLE for JSON output
                // (2026-05-21): delegate to inner.
                inner.to_json_string()
            }
            MettaValueInner::Empty => r#"{"type":"empty"}"#.to_string(),
            MettaValueInner::NotReducible => r#"{"type":"not_reducible"}"#.to_string(),
            MettaValueInner::Spanned(v, _) => v.to_json_string(),
        }
    }
}

/// **Stack-safety + memory-safety helper (2026-05-15)**: format style for
/// `format_value_iterative`. Each style controls how atoms, strings,
/// floats, and the special sentinels are rendered. Composite types
/// (SExpr/Conjunction/Error/Type/Quoted) use the same iterative work-list
/// shape regardless of style.
#[derive(Clone, Copy)]
pub(crate) enum FormatStyle {
    /// `to_metta_string` style: quoted strings, canonical floats,
    /// `True`/`False` for booleans, parser-roundtrip-safe.
    MettaString,
    /// `to_mork_string` style: variable renaming (`$x`, `&x`, `_` → `$`),
    /// unquoted strings (legacy), default Rust float formatting.
    MorkString,
    /// `Display for MettaValue` style: same as MettaString except
    /// State/Memo are rendered as `<State:ID>` / `<Memo:NAME>`
    /// (human-friendly, NOT parser-roundtrip).
    /// Plan Phase C (2026-05-20): Space rendering aligned with
    /// MettaString / HE-canonical form (`ModuleSpace(GroundingSpace-top)`
    /// for `&self`; `&<name>` for named spaces).
    Display,
}

/// **Stack-safety + memory-safety fix (2026-05-15)**: shared iterative
/// formatter for `to_metta_string` and `to_mork_string`.
///
/// Replaces the recursive variants (which were both stack-unsafe AND
/// exponentially vulnerable to shared substructure). Uses a heap work-list
/// (`Vec<FmtWork>`) plus a memo (`HashMap<*const MettaValueInner, String>`)
/// keyed by slab pointer to cache each unique subtree's rendered string —
/// so multiple occurrences of a shared subtree are O(string_len) instead
/// of O(full re-expansion).
///
/// See `to_display_string` for the same pattern.
pub(crate) fn format_value_iterative(root: &MettaValue, style: FormatStyle) -> String {
    enum FmtWork<'a> {
        Process(&'a MettaValue),
        Join {
            count: usize,
            prefix: &'static str,
            suffix: &'static str,
            separator: &'static str,
            memo_key: Option<usize>,
        },
        /// Workstream B (Task #6 follow-up, 2026-05-18): HE-style render
        /// for `(Bindings ($x val) …)` — emit as `{ $x <- val, … }` matching
        /// HE's `Display for Bindings` (`hyperon-experimental/hyperon-atom/
        /// src/matcher.rs:762-789`). Only fires when `style ==
        /// FormatStyle::Display`; the `MettaString` and `MorkString` styles
        /// keep the structural `(Bindings (k v) …)` text so the output
        /// round-trips through the MeTTa lexer (which tokenizes `{` and `}`
        /// as separate one-char words).
        JoinBindings {
            var_names: Vec<String>,
            malformed: Vec<bool>,
            memo_key: Option<usize>,
        },
    }

    let mut work_stack: Vec<FmtWork<'_>> = Vec::with_capacity(16);
    let mut result_stack: Vec<String> = Vec::with_capacity(16);
    let mut memo: std::collections::HashMap<usize, String> =
        std::collections::HashMap::with_capacity(64);

    work_stack.push(FmtWork::Process(root));

    while let Some(work) = work_stack.pop() {
        match work {
            FmtWork::Process(val) => {
                if val.is_inline() {
                    result_stack.push(match val.inline_tag() {
                        NB_TAG_LONG => val.inline_long_value().to_string(),
                        // Phase 1.5 PT alignment (2026-05-22): lowercase per
                        // PT translator (PHE-finer #10). Parser accepts both
                        // `True`/`False` and `true`/`false` as input.
                        NB_TAG_BOOL => match style {
                            FormatStyle::MettaString | FormatStyle::Display => {
                                if (val.tagged as u64 & 1) != 0 {
                                    "true"
                                } else {
                                    "false"
                                }
                                .to_string()
                            }
                            FormatStyle::MorkString => ((val.tagged as u64 & 1) != 0).to_string(),
                        },
                        NB_TAG_UNIT => "()".to_string(),
                        NB_TAG_EMPTY => "Empty".to_string(),
                        _ => "()".to_string(),
                    });
                    continue;
                }
                let memo_key = val.inner_ptr() as usize;
                if let Some(cached) = memo.get(&memo_key) {
                    result_stack.push(cached.clone());
                    continue;
                }
                match val.inner_ref() {
                    MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                    MettaValueInner::Float(f) => result_stack.push(match style {
                        FormatStyle::MettaString | FormatStyle::Display => float_canonical(*f),
                        FormatStyle::MorkString => f.to_string(),
                    }),
                    MettaValueInner::Bool(b) => result_stack.push(match style {
                        // Phase 1.5 PT alignment (2026-05-22): lowercase per
                        // PT translator (PHE-finer #10). Parser remains
                        // permissive — `True`/`False` and `true`/`false` are
                        // both accepted as input (see parser/mod.rs).
                        FormatStyle::MettaString | FormatStyle::Display => {
                            if *b { "true" } else { "false" }.to_string()
                        }
                        FormatStyle::MorkString => b.to_string(),
                    }),
                    MettaValueInner::String(s) => result_stack.push(match style {
                        FormatStyle::MettaString | FormatStyle::Display => {
                            // Display impl uses the same canonical escapes
                            // (`\\`, `\"`, `\n`, `\t`, `\r`) as to_metta_string.
                            format!("\"{}\"", escape_metta_string(s))
                        }
                        FormatStyle::MorkString => format!("\"{}\"", s),
                    }),
                    MettaValueInner::Atom(a) => result_stack.push(match style {
                        FormatStyle::MettaString | FormatStyle::Display => a.to_string(),
                        FormatStyle::MorkString => {
                            if *a == "&" || *a == "&self" || *a == "&kb" || *a == "&stack" {
                                a.to_string()
                            } else if a.starts_with('$')
                                || a.starts_with('&')
                                || a.starts_with('\'')
                            {
                                format!("${}", &a[1..])
                            } else if *a == "_" {
                                "$".to_string()
                            } else {
                                a.to_string()
                            }
                        }
                    }),
                    MettaValueInner::Unit => result_stack.push("()".to_string()),
                    MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                    MettaValueInner::NotReducible => {
                        result_stack.push("NotReducible".to_string());
                    }
                    MettaValueInner::Space(handle) => {
                        // Phase C (HE bisim, 2026-05-20): HE-aligned space
                        // print form per fixture T04/028 / §06.15.
                        // `&self` (context-space) → `ModuleSpace(GroundingSpace-top)`
                        // Named space → `&<name>`
                        let canonical = if handle.name == "self" {
                            "ModuleSpace(GroundingSpace-top)".to_string()
                        } else {
                            format!("&{}", handle.name)
                        };
                        result_stack.push(canonical);
                    }
                    MettaValueInner::State(id) => {
                        result_stack.push(match style {
                            FormatStyle::Display => format!("<State:{}>", id),
                            FormatStyle::MettaString | FormatStyle::MorkString => {
                                format!("(State {})", id)
                            }
                        });
                    }
                    MettaValueInner::Memo(handle) => {
                        result_stack.push(match style {
                            FormatStyle::Display => format!("<Memo:{}>", handle.name),
                            FormatStyle::MettaString | FormatStyle::MorkString => {
                                format!("(Memo {} \"{}\")", handle.id, handle.name)
                            }
                        });
                    }
                    MettaValueInner::Error(offending, detail) => {
                        work_stack.push(FmtWork::Join {
                            count: 2,
                            prefix: "(Error ",
                            suffix: ")",
                            separator: " ",
                            memo_key: Some(memo_key),
                        });
                        work_stack.push(FmtWork::Process(detail));
                        work_stack.push(FmtWork::Process(offending));
                    }
                    MettaValueInner::Type(t) => match style {
                        FormatStyle::Display => {
                            // Display wraps as `(: inner)` per the original
                            // `impl fmt::Display for MettaValue`.
                            work_stack.push(FmtWork::Join {
                                count: 1,
                                prefix: "(: ",
                                suffix: ")",
                                separator: "",
                                memo_key: Some(memo_key),
                            });
                            work_stack.push(FmtWork::Process(t));
                        }
                        FormatStyle::MettaString | FormatStyle::MorkString => {
                            // to_metta_string / to_mork_string format Type(t)
                            // as just t (NOT wrapped). Preserve that behavior
                            // — pass through the inner.
                            work_stack.push(FmtWork::Process(t));
                        }
                    },
                    MettaValueInner::Quoted(inner) => {
                        work_stack.push(FmtWork::Join {
                            count: 1,
                            prefix: "(quote ",
                            suffix: ")",
                            separator: "",
                            memo_key: Some(memo_key),
                        });
                        work_stack.push(FmtWork::Process(inner));
                    }
                    MettaValueInner::Lazy(inner) => {
                        // PT-canonical Lazy is INVISIBLE for display
                        // (2026-05-21): delegate to inner so users see
                        // the substituted value verbatim. No wrapping.
                        work_stack.push(FmtWork::Process(inner));
                    }
                    MettaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            let s = "()".to_string();
                            memo.insert(memo_key, s.clone());
                            result_stack.push(s);
                        } else if matches!(style, FormatStyle::Display)
                            && items.first().and_then(|h| h.as_atom()) == Some("Bindings")
                        {
                            // Workstream B: HE-style `{ }` / `{ $x <- val, … }`
                            // render — Display-only (MettaString/MorkString
                            // keep the round-trippable structural shape).
                            let pairs = &items[1..];
                            if pairs.is_empty() {
                                let s = "{ }".to_string();
                                memo.insert(memo_key, s.clone());
                                result_stack.push(s);
                            } else {
                                let mut var_names: Vec<String> = Vec::with_capacity(pairs.len());
                                let mut malformed: Vec<bool> = Vec::with_capacity(pairs.len());
                                for p in pairs {
                                    let (name, ok) = match p.inner_ref() {
                                        MettaValueInner::SExpr(kv) if kv.len() == 2 => {
                                            match kv[0].inner_ref() {
                                                MettaValueInner::Atom(n) => (n.to_string(), true),
                                                _ => (String::new(), false),
                                            }
                                        }
                                        _ => (String::new(), false),
                                    };
                                    var_names.push(name);
                                    malformed.push(!ok);
                                }
                                work_stack.push(FmtWork::JoinBindings {
                                    var_names,
                                    malformed: malformed.clone(),
                                    memo_key: Some(memo_key),
                                });
                                for (i, p) in pairs.iter().enumerate().rev() {
                                    let to_render: &MettaValue = if malformed[i] {
                                        p
                                    } else if let MettaValueInner::SExpr(kv) = p.inner_ref() {
                                        &kv[1]
                                    } else {
                                        p
                                    };
                                    work_stack.push(FmtWork::Process(to_render));
                                }
                            }
                        } else {
                            work_stack.push(FmtWork::Join {
                                count: items.len(),
                                prefix: "(",
                                suffix: ")",
                                separator: " ",
                                memo_key: Some(memo_key),
                            });
                            for item in items.iter().rev() {
                                work_stack.push(FmtWork::Process(item));
                            }
                        }
                    }
                    MettaValueInner::Conjunction(goals) => {
                        if goals.is_empty() {
                            // to_metta_string had `(,)` for empty; to_mork_string
                            // had `(, )` (per its inner=join logic). Use `(,)`
                            // for both — the difference was incidental.
                            let s = "(,)".to_string();
                            memo.insert(memo_key, s.clone());
                            result_stack.push(s);
                        } else {
                            work_stack.push(FmtWork::Join {
                                count: goals.len(),
                                prefix: "(, ",
                                suffix: ")",
                                separator: " ",
                                memo_key: Some(memo_key),
                            });
                            for goal in goals.iter().rev() {
                                work_stack.push(FmtWork::Process(goal));
                            }
                        }
                    }
                    MettaValueInner::Spanned(v, _) => {
                        work_stack.push(FmtWork::Process(v));
                    }
                }
            }
            FmtWork::Join {
                count,
                prefix,
                suffix,
                separator,
                memo_key,
            } => {
                let start = result_stack.len() - count;
                let parts: Vec<String> = result_stack.drain(start..).collect();
                let formatted = format!("{}{}{}", prefix, parts.join(separator), suffix);
                if let Some(key) = memo_key {
                    memo.insert(key, formatted.clone());
                }
                result_stack.push(formatted);
            }
            FmtWork::JoinBindings {
                var_names,
                malformed,
                memo_key,
            } => {
                let count = var_names.len();
                let start = result_stack.len() - count;
                let parts: Vec<String> = result_stack.drain(start..).collect();
                let segs: Vec<String> = parts
                    .into_iter()
                    .enumerate()
                    .map(|(i, rendered)| {
                        if malformed[i] {
                            rendered
                        } else {
                            format!("{} <- {}", var_names[i], rendered)
                        }
                    })
                    .collect();
                let s = format!("{{ {} }}", segs.join(", "));
                if let Some(key) = memo_key {
                    memo.insert(key, s.clone());
                }
                result_stack.push(s);
            }
        }
    }

    result_stack.pop().unwrap_or_default()
}

/// Escape special characters in a string for JSON encoding.
pub fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

/// Spec §02 canonical float formatting: whole-number floats emit `.0`
/// so `parse(format(v))` round-trips to `Float(v)`, not `Long(v as i64)`.
///
/// Single source of truth used by every MeTTa-text float formatter:
/// - `Display::fmt` (`metta_value.rs:1568`)
/// - `to_metta_string` (`metta_value.rs:1334`)
/// - Stack-based stringifiers (`metta_value.rs:2139, 2261`)
/// - REPL `format_result` (`main.rs:221`)
///
/// **Deliberately NOT used by `to_mork_string` (`metta_value.rs:1398`)**
/// because that function produces MORK index keys; changing the format
/// would change the keys and break match outcomes. MORK key formatting
/// is a separate concern with its own backwards-compatibility requirements.
#[inline]
pub fn float_canonical(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{}.0", f as i64)
    } else {
        f.to_string()
    }
}

/// Escape string content for MeTTa string literals.
/// Reverses the logic in the parser's unescape_string().
/// Supports: \n, \t, \r, \\, \", \x##, \u{...}
pub fn escape_metta_string(s: &str) -> String {
    let mut result = String::new();
    for ch in s.chars() {
        match ch {
            '\n' => result.push_str(r"\n"),
            '\t' => result.push_str(r"\t"),
            '\r' => result.push_str(r"\r"),
            '\\' => result.push_str(r"\\"),
            '"' => result.push_str(r#"\""#),
            // ASCII control characters — use hex escape
            c if c.is_control() && (c as u32) < 256 => {
                result.push_str(&format!(r"\x{:02x}", c as u8));
            }
            // Non-ASCII characters — use unicode escape if needed
            c if !c.is_ascii() => {
                result.push_str(&format!(r"\u{{{:x}}}", c as u32));
            }
            // Regular printable ASCII — no escaping needed
            c => result.push(c),
        }
    }
    result
}

// ============================================================================
// Trait implementations
// ============================================================================

impl fmt::Debug for MettaValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Inline values: format directly without slab deref
        if self.is_inline() {
            return match self.view() {
                ValueView::Bool(b) => write!(f, "Bool({})", b),
                ValueView::Long(n) => write!(f, "Long({})", n),
                ValueView::Unit => write!(f, "Unit"),
                ValueView::Empty => write!(f, "Empty"),
                _ => write!(f, "Inline(?)"),
            };
        }
        self.inner_ref().fmt(f)
    }
}

impl fmt::Display for MettaValue {
    /// **Stack-safety + memory-safety fix (2026-05-15)**: delegates to the
    /// iterative + memoized `format_value_iterative` with `FormatStyle::Display`.
    /// Was recursive (Display on children via `write!`) and exponentially
    /// vulnerable to shared substructure — same defect class as the 6.7 PB
    /// `to_display_string` bug.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_value_iterative(self, FormatStyle::Display))
    }
}

impl PartialEq for MettaValue {
    fn eq(&self, other: &Self) -> bool {
        // Fastest path: identical encoding (covers inline equality AND pointer equality)
        if self.tagged == other.tagged {
            return true;
        }
        // For inline values, identical tagged means equal (handled above).
        // If one is inline and the other is slab, they can still be semantically equal
        // (e.g., inline Bool(true) == slab Bool(true)). Compare via view().
        if self.is_inline() || other.is_inline() {
            return match (self.view(), other.view()) {
                (ValueView::Bool(a), ValueView::Bool(b)) => a == b,
                (ValueView::Long(a), ValueView::Long(b)) => a == b,
                (ValueView::Unit, ValueView::Unit) => true,
                (ValueView::Empty, ValueView::Empty) => true,
                // Inline vs slab of different types
                _ => false,
            };
        }
        // Both slab: pointer equality (mask off flags)
        let a = self.tagged & PTR_MASK;
        let b = other.tagged & PTR_MASK;
        if a == b {
            return true;
        }
        // Compare through inner() which strips Spanned — span-transparent equality
        self.inner() == other.inner()
    }
}

impl PartialEq for MettaValueInner {
    fn eq(&self, other: &Self) -> bool {
        // Strip Spanned AND Lazy wrappers for comparison — both are
        // transparent (Spanned: source-position metadata; Lazy: PT-canonical
        // data-in / data-out marker, 2026-05-21). This ensures
        // `Lazy(x) == y` iff `x == y`, mirroring Spanned's behavior.
        let a = strip_spanned(self);
        let b = strip_spanned(other);
        // If both point to the same non-Spanned inner, they're equal
        if std::ptr::eq(a, b) {
            return true;
        }
        match (a, b) {
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) => a == b,
            (MettaValueInner::Bool(a), MettaValueInner::Bool(b)) => a == b,
            (MettaValueInner::Long(a), MettaValueInner::Long(b)) => a == b,
            (MettaValueInner::Float(a), MettaValueInner::Float(b)) => a == b,
            (MettaValueInner::String(a), MettaValueInner::String(b)) => a == b,
            (MettaValueInner::SExpr(a), MettaValueInner::SExpr(b)) => a == b,
            (MettaValueInner::Unit, MettaValueInner::Unit) => true,
            (MettaValueInner::Error(oa, da), MettaValueInner::Error(ob, db)) => {
                oa == ob && da == db
            }
            (MettaValueInner::Type(a), MettaValueInner::Type(b)) => a == b,
            (MettaValueInner::Conjunction(a), MettaValueInner::Conjunction(b)) => a == b,
            (MettaValueInner::Space(a), MettaValueInner::Space(b)) => a.id == b.id,
            (MettaValueInner::State(a), MettaValueInner::State(b)) => a == b,
            (MettaValueInner::Quoted(a), MettaValueInner::Quoted(b)) => a == b,
            (MettaValueInner::Memo(a), MettaValueInner::Memo(b)) => a.id == b.id,
            (MettaValueInner::Empty, MettaValueInner::Empty) => true,
            (MettaValueInner::NotReducible, MettaValueInner::NotReducible) => true,
            _ => false,
        }
    }
}

/// Strip all Spanned (and PT-canonical Lazy) layers from a MettaValueInner reference.
/// Returns a reference to the innermost transparent-wrapper variant.
///
/// Lazy is invisible for equality and hashing per PT-canonical semantics
/// (2026-05-21) — `Lazy(x) == y` iff `x == y`. Eval-side dispatch
/// (`step/sexpr.rs`, `eval_loop.rs`) checks for Lazy BEFORE this strip via
/// `view()`, so the inhibit-rule-lookup behavior is preserved.
#[inline]
fn strip_spanned(inner: &MettaValueInner) -> &MettaValueInner {
    let mut current = inner;
    loop {
        match current {
            MettaValueInner::Spanned(v, _) => current = v.inner_ref(),
            MettaValueInner::Lazy(v) => current = v.inner_ref(),
            _ => return current,
        }
    }
}

impl Eq for MettaValue {}
impl Eq for MettaValueInner {}

/// MeTTa HE-compatible numeric equality with type promotion.
///
/// Long(2) == Float(2.0) -> true. Promotes Long->f64 when comparing
/// mixed numeric types, using epsilon tolerance for float comparison.
/// Non-numeric types fall back to structural PartialEq.
#[inline]
pub fn numeric_equal(a: &MettaValue, b: &MettaValue) -> bool {
    // Delegate to the generic version which handles S-expression recursion
    numeric_equal_generic(a, b)
}

/// MeTTa HE-compatible numeric inequality.
#[inline]
pub fn numeric_not_equal(a: &MettaValue, b: &MettaValue) -> bool {
    !numeric_equal(a, b)
}

/// Generic numeric equality for use with MettaValueTrait.
///
/// Recurses into S-expressions so that structures containing floats
/// (e.g. `(stv 0.519... 0.829...)`) use epsilon comparison on float leaves
/// rather than exact `PartialEq`.
pub fn numeric_equal_generic<V: MettaValueTrait + PartialEq>(a: &V, b: &V) -> bool {
    // H4 (2026-05-05) hard-cut: HE-bisimilar exact equality.
    // Float comparisons use bare f64 `==` (NaN != NaN per IEEE 754,
    // +0.0 == -0.0 per IEEE 754). Long↔Float promotion is preserved
    // via lossless `as f64` cast. S-expr structural recursion is delegated
    // to `MettaValue::PartialEq::eq` (line 1650-1676) — which uses exact
    // child compare via derived `Vec<MettaValue>::PartialEq`.
    match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
        (Some(x), _, Some(y), _) => x == y,          // Long, Long
        (_, Some(x), _, Some(y)) => x == y,          // Float, Float — exact
        (Some(x), _, _, Some(y)) => (x as f64) == y, // Long → Float exact
        (_, Some(x), Some(y), _) => x == (y as f64), // Float ↔ Long exact
        _ => a == b,                                 // Structural via PartialEq
    }
}

impl std::hash::Hash for MettaValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Delegate to the MettaValueTrait::hash_value() method which provides
        // a high-quality xxh3 hash of the value's structure.
        self.hash_value().hash(state);
    }
}

// ============================================================================
// MettaValue trait implementation for MettaValue
// ============================================================================

impl MettaValueTrait for MettaValue {
    type SExprSlice = [MettaValue];

    #[inline]
    fn inner_raw(&self) -> &MettaValueInner {
        self.inner_ref() // Raw field, no Spanned stripping (handles inline via materialization)
    }

    #[inline]
    fn view(&self) -> ValueView {
        MettaValue::view(self) // Delegates to inherent method (inline-aware)
    }

    #[inline]
    fn inner_ptr(&self) -> *const MettaValueInner {
        MettaValue::inner_ptr(self) // Delegates to inherent method (null for inline)
    }

    /// O(1) check — guarded against inline NaN-boxed values.
    #[inline]
    fn has_variables_fast(&self) -> bool {
        MettaValue::has_variables_fast(self) // Delegates to inherent method
    }

    #[inline]
    fn identity_eq(&self, other: &Self) -> bool {
        self.tagged == other.tagged // O(1) pointer/tag comparison
    }

    #[inline]
    unsafe fn from_inner_ptr(ptr: *const MettaValueInner) -> Self {
        // Index mode (Inc 2b): `ptr` is NOT a slab pointer — it carries the bare
        // 32-bit arena `Addr` bits the JIT packed (the INDEX_KEY_TAG was masked off
        // at the 48-bit payload boundary, leaving `addr.raw()`). Reconstruct the
        // handle; do NOT dereference. Slab mode is byte-identical (the deref below).
        if gc_mode_is_index() {
            let addr = crate::backend::eval::cesk::index_arena::Addr::from_raw(ptr as u32);
            return MettaValue::from_addr(addr, 0); // flags=0 matches the slab from_inner path
        }
        // SAFETY: The pointer is slab-allocated with 'static lifetime (managed by GC).
        MettaValue::from_inner(&*ptr)
    }

    #[inline]
    fn is_atom(&self) -> bool {
        MettaValue::is_atom(self)
    }

    #[inline]
    fn is_bool(&self) -> bool {
        MettaValue::is_bool(self)
    }

    #[inline]
    fn is_long(&self) -> bool {
        MettaValue::is_long(self)
    }

    #[inline]
    fn is_float(&self) -> bool {
        MettaValue::is_float(self)
    }

    #[inline]
    fn is_string(&self) -> bool {
        MettaValue::is_string(self)
    }

    #[inline]
    fn is_sexpr(&self) -> bool {
        MettaValue::is_sexpr(self)
    }

    #[inline]
    fn is_error(&self) -> bool {
        MettaValue::is_error(self)
    }

    #[inline]
    fn is_type(&self) -> bool {
        MettaValue::is_type(self)
    }

    #[inline]
    fn is_conjunction(&self) -> bool {
        MettaValue::is_conjunction(self)
    }

    #[inline]
    fn is_space(&self) -> bool {
        MettaValue::is_space(self)
    }

    #[inline]
    fn is_state(&self) -> bool {
        MettaValue::is_state(self)
    }

    #[inline]
    fn is_unit(&self) -> bool {
        MettaValue::is_unit(self)
    }

    #[inline]
    fn is_memo(&self) -> bool {
        MettaValue::is_memo(self)
    }

    #[inline]
    fn is_quoted(&self) -> bool {
        MettaValue::is_quoted(self)
    }

    #[inline]
    fn is_empty(&self) -> bool {
        MettaValue::is_empty(self)
    }

    #[inline]
    fn is_spanned(&self) -> bool {
        MettaValue::is_spanned(self)
    }

    #[inline]
    fn span(&self) -> Option<&'static crate::ir::Span> {
        MettaValue::span(self)
    }

    #[inline]
    fn strip_one_span(&self) -> Self {
        if self.is_inline() {
            return *self;
        }
        match self.inner_ref() {
            MettaValueInner::Spanned(v, _) => *v,
            _ => *self,
        }
    }

    #[inline]
    fn is_variable(&self) -> bool {
        MettaValue::is_variable(self)
    }

    #[inline]
    fn is_ground_type(&self) -> bool {
        if self.is_inline() {
            return matches!(self.inline_tag(), NB_TAG_BOOL | NB_TAG_LONG);
        }
        match self.inner_ref() {
            MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_ground_type(),
            _ => false,
        }
    }

    #[inline]
    fn as_atom(&self) -> Option<&'static str> {
        MettaValue::as_atom(self)
    }

    #[inline]
    fn as_bool(&self) -> Option<bool> {
        MettaValue::as_bool(self)
    }

    #[inline]
    fn as_long(&self) -> Option<i64> {
        MettaValue::as_long(self)
    }

    #[inline]
    fn as_float(&self) -> Option<f64> {
        MettaValue::as_float(self)
    }

    #[inline]
    fn as_string(&self) -> Option<&str> {
        MettaValue::as_string(self)
    }

    #[inline]
    fn as_sexpr(&self) -> Option<&[Self]> {
        MettaValue::as_sexpr(self)
    }

    #[inline]
    fn as_error(&self) -> Option<(&Self, &Self)> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Error(offending, details) => Some((offending, details)),
            MettaValueInner::Spanned(v, _) => <MettaValue as MettaValueTrait>::as_error(v),
            _ => None,
        }
    }

    #[inline]
    fn as_type(&self) -> Option<&Self> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Type(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => <MettaValue as MettaValueTrait>::as_type(v),
            _ => None,
        }
    }

    #[inline]
    fn as_conjunction(&self) -> Option<&[Self]> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Conjunction(goals) => Some(goals),
            MettaValueInner::Spanned(v, _) => v.as_conjunction(),
            _ => None,
        }
    }

    #[inline]
    fn as_space(&self) -> Option<&SpaceHandle> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Space(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_space(),
            _ => None,
        }
    }

    #[inline]
    fn as_state(&self) -> Option<u64> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::State(id) => Some(*id),
            MettaValueInner::Spanned(v, _) => v.as_state(),
            _ => None,
        }
    }

    #[inline]
    fn as_memo(&self) -> Option<&MemoHandle> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Memo(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_memo(),
            _ => None,
        }
    }

    #[inline]
    fn as_quoted(&self) -> Option<Self> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted(),
            _ => None,
        }
    }

    #[inline]
    fn as_quoted_ref(&self) -> Option<&Self> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted_ref(),
            _ => None,
        }
    }

    #[inline]
    fn as_lazy(&self) -> Option<Self> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Lazy(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_lazy(),
            _ => None,
        }
    }

    #[inline]
    fn as_lazy_ref(&self) -> Option<&Self> {
        if self.is_inline() {
            return None;
        }
        match self.inner_ref() {
            MettaValueInner::Lazy(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => v.as_lazy_ref(),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        if self.is_inline() {
            return match self.inline_tag() {
                NB_TAG_LONG => "Number",
                NB_TAG_BOOL => "Bool",
                NB_TAG_UNIT => "Unit",
                NB_TAG_EMPTY => "Empty",
                _ => "Unit",
            };
        }
        match self.inner_ref() {
            MettaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            MettaValueInner::Atom(_) => "Symbol",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::Long(_) => "Number",
            MettaValueInner::Float(_) => "Number",
            MettaValueInner::String(_) => "String",
            MettaValueInner::SExpr(_) => "Expression",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Quoted(_) => "Expression",
            // PT-canonical Lazy is INVISIBLE (2026-05-21).
            MettaValueInner::Lazy(inner) => inner.type_name(),
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
            MettaValueInner::NotReducible => "NotReducible",
            MettaValueInner::Spanned(v, _) => v.type_name(),
        }
    }

    fn friendly_type_name(&self) -> &'static str {
        if self.is_inline() {
            return match self.inline_tag() {
                NB_TAG_LONG => "Number (integer)",
                NB_TAG_BOOL => "Bool",
                NB_TAG_UNIT => "Unit",
                NB_TAG_EMPTY => "Empty",
                _ => "Unit",
            };
        }
        match self.inner_ref() {
            MettaValueInner::Long(_) => "Number (integer)",
            MettaValueInner::Float(_) => "Number (float)",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::String(_) => "String",
            MettaValueInner::Atom(_) => "Atom",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::SExpr(_) => "S-expression",
            MettaValueInner::Quoted(_) => "Quoted expression",
            // PT-canonical Lazy is INVISIBLE (2026-05-21).
            MettaValueInner::Lazy(inner) => inner.friendly_type_name(),
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
            MettaValueInner::NotReducible => "NotReducible",
            MettaValueInner::Spanned(v, _) => v.friendly_type_name(),
        }
    }

    fn get_head_symbol(&self) -> Option<&str> {
        if self.is_inline() {
            return None;
        }
        // Helper to check if an atom is a space reference (not a variable)
        fn is_space_ref(s: &str) -> bool {
            s == "&" || s == "&self" || s == "&kb" || s == "&stack"
        }

        match self.inner_ref() {
            // For s-expressions like (double $x), extract "double"
            MettaValueInner::SExpr(items) if !items.is_empty() => match items[0].inner_ref() {
                MettaValueInner::Atom(head)
                    if !head.starts_with('$')
                        && (!head.starts_with('&') || is_space_ref(head))
                        && !head.starts_with('\'')
                        && *head != "_" =>
                {
                    Some(head)
                }
                MettaValueInner::Spanned(ref v, _) => v.get_head_symbol(),
                _ => None,
            },
            // For bare atoms like foo, use the atom itself
            MettaValueInner::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || is_space_ref(head))
                    && !head.starts_with('\'')
                    && *head != "_" =>
            {
                Some(head)
            }
            MettaValueInner::Spanned(v, _) => v.get_head_symbol(),
            _ => None,
        }
    }

    fn get_arity(&self) -> usize {
        if self.is_inline() {
            return 0;
        }
        match self.inner_ref() {
            MettaValueInner::SExpr(items) if !items.is_empty() => items.len() - 1, // Exclude head
            MettaValueInner::Spanned(v, _) => v.get_arity(),
            _ => 0,
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        serialize_value(self, &mut buf);
        buf
    }

    #[inline]
    fn hash_value(&self) -> u64 {
        // Inline values: delegate to hash_value_cached_inner's inline fast path
        // to ensure Spanned(Long(42)) and bare Long(42) hash identically.
        if self.is_inline() {
            // This must match exactly what hash_value_cached_inner returns for inline values.
            const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
            const LONG_SEED: u64 = 0x517cc1b727220a95;
            const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
            const UNIT_HASH: u64 = 0x756e6974_68617368;
            return match self.inline_tag() {
                NB_TAG_UNIT => UNIT_HASH,
                NB_TAG_BOOL => {
                    if (self.tagged as u64 & 1) != 0 {
                        BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
                    } else {
                        BOOL_SEED
                    }
                }
                NB_TAG_LONG => {
                    let n = self.inline_long_value();
                    let x = (n as u64)
                        .wrapping_add(LONG_SEED)
                        .wrapping_mul(GOLDEN_RATIO);
                    x ^ (x >> 32)
                }
                NB_TAG_EMPTY => 9u64.wrapping_mul(GOLDEN_RATIO),
                _ => UNIT_HASH,
            };
        }
        ensure_value_hash_cache_epoch_current();
        VALUE_HASH_CACHE.with(|cache_cell| {
            let mut cache = cache_cell.borrow_mut();
            hash_value_cached_inner(self, &mut cache)
        })
    }

    fn friendly_repr(&self) -> std::string::String {
        // Stack-based implementation to avoid recursion on deeply nested structures
        enum ReprWork<'a> {
            Process(&'a MettaValue),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
            },
            /// Workstream B (Task #6 follow-up, 2026-05-18): HE-style render
            /// for `(Bindings ($x val) …)` — emit as `{ $x <- val, … }`
            /// matching HE's `Display for Bindings`.
            JoinBindings {
                var_names: Vec<String>,
                malformed: Vec<bool>,
            },
        }

        let mut work_stack: Vec<ReprWork<'_>> = Vec::with_capacity(16);
        let mut result_stack: Vec<std::string::String> = Vec::with_capacity(16);

        work_stack.push(ReprWork::Process(self));

        while let Some(work) = work_stack.pop() {
            match work {
                ReprWork::Process(val) => {
                    if val.is_inline() {
                        result_stack.push(match val.inline_tag() {
                            NB_TAG_LONG => val.inline_long_value().to_string(),
                            // Phase 1.5 PT alignment: lowercase per PHE-finer #10.
                            NB_TAG_BOOL => if (val.tagged as u64 & 1) != 0 {
                                "true"
                            } else {
                                "false"
                            }
                            .to_string(),
                            NB_TAG_UNIT => "()".to_string(),
                            NB_TAG_EMPTY => "Empty".to_string(),
                            _ => "()".to_string(),
                        });
                        continue;
                    }
                    match val.inner_ref() {
                        MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                        MettaValueInner::Float(f) => result_stack.push(float_canonical(*f)),
                        MettaValueInner::Bool(b) => {
                            result_stack.push(if *b { "true" } else { "false" }.to_string());
                        }
                        MettaValueInner::String(s) => result_stack.push(format!("\"{}\"", s)),
                        MettaValueInner::Atom(a) => result_stack.push(a.to_string()),
                        MettaValueInner::Unit => result_stack.push("()".to_string()),
                        MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                        MettaValueInner::NotReducible => {
                            result_stack.push("NotReducible".to_string())
                        }
                        MettaValueInner::Space(handle) => {
                            // Phase C (HE bisim, 2026-05-20): HE-aligned space
                            // print form per fixture T04/028 / §06.15.
                            let canonical = if handle.name == "self" {
                                "ModuleSpace(GroundingSpace-top)".to_string()
                            } else {
                                format!("&{}", handle.name)
                            };
                            result_stack.push(canonical);
                        }
                        MettaValueInner::State(id) => {
                            result_stack.push(format!("(State {})", id));
                        }
                        MettaValueInner::Memo(handle) => {
                            result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                        }
                        MettaValueInner::Error(offending, detail) => {
                            work_stack.push(ReprWork::Join {
                                count: 2,
                                prefix: "(Error ",
                                suffix: ")",
                                separator: " ",
                            });
                            work_stack.push(ReprWork::Process(detail));
                            work_stack.push(ReprWork::Process(offending));
                        }
                        MettaValueInner::Type(t) => {
                            work_stack.push(ReprWork::Join {
                                count: 1,
                                prefix: "(: ",
                                suffix: ")",
                                separator: "",
                            });
                            work_stack.push(ReprWork::Process(t));
                        }
                        MettaValueInner::Quoted(inner) => {
                            work_stack.push(ReprWork::Join {
                                count: 1,
                                prefix: "(quote ",
                                suffix: ")",
                                separator: "",
                            });
                            work_stack.push(ReprWork::Process(inner));
                        }
                        MettaValueInner::Lazy(inner) => {
                            // PT-canonical Lazy is INVISIBLE (2026-05-21):
                            // delegate to inner so users see substituted
                            // values verbatim, no wrapping in friendly_repr.
                            work_stack.push(ReprWork::Process(inner));
                        }
                        MettaValueInner::SExpr(items) => {
                            if items.is_empty() {
                                result_stack.push("()".to_string());
                            } else if items.first().and_then(|h| h.as_atom()) == Some("Bindings") {
                                // Workstream B: HE-style `{ }` / `{ $x <- val, … }`
                                // render for `(Bindings ($x val) …)` SExpr.
                                let pairs = &items[1..];
                                if pairs.is_empty() {
                                    result_stack.push("{ }".to_string());
                                } else {
                                    let mut var_names: Vec<String> =
                                        Vec::with_capacity(pairs.len());
                                    let mut malformed: Vec<bool> = Vec::with_capacity(pairs.len());
                                    for p in pairs {
                                        let (name, ok) = match p.inner_ref() {
                                            MettaValueInner::SExpr(kv) if kv.len() == 2 => match kv
                                                [0]
                                            .inner_ref()
                                            {
                                                MettaValueInner::Atom(n) => (n.to_string(), true),
                                                _ => (String::new(), false),
                                            },
                                            _ => (String::new(), false),
                                        };
                                        var_names.push(name);
                                        malformed.push(!ok);
                                    }
                                    work_stack.push(ReprWork::JoinBindings {
                                        var_names,
                                        malformed: malformed.clone(),
                                    });
                                    for (i, p) in pairs.iter().enumerate().rev() {
                                        let to_render: &MettaValue = if malformed[i] {
                                            p
                                        } else if let MettaValueInner::SExpr(kv) = p.inner_ref() {
                                            &kv[1]
                                        } else {
                                            p
                                        };
                                        work_stack.push(ReprWork::Process(to_render));
                                    }
                                }
                            } else {
                                work_stack.push(ReprWork::Join {
                                    count: items.len(),
                                    prefix: "(",
                                    suffix: ")",
                                    separator: " ",
                                });
                                for item in items.iter().rev() {
                                    work_stack.push(ReprWork::Process(item));
                                }
                            }
                        }
                        MettaValueInner::Conjunction(goals) => {
                            if goals.is_empty() {
                                result_stack.push("(,)".to_string());
                            } else {
                                work_stack.push(ReprWork::Join {
                                    count: goals.len(),
                                    prefix: "(, ",
                                    suffix: ")",
                                    separator: " ",
                                });
                                for goal in goals.iter().rev() {
                                    work_stack.push(ReprWork::Process(goal));
                                }
                            }
                        }
                        MettaValueInner::Spanned(v, _) => {
                            work_stack.push(ReprWork::Process(v));
                        }
                    }
                }
                ReprWork::Join {
                    count,
                    prefix,
                    suffix,
                    separator,
                } => {
                    let start = result_stack.len() - count;
                    let parts: Vec<std::string::String> = result_stack.drain(start..).collect();
                    result_stack.push(format!("{}{}{}", prefix, parts.join(separator), suffix));
                }
                ReprWork::JoinBindings {
                    var_names,
                    malformed,
                } => {
                    let count = var_names.len();
                    let start = result_stack.len() - count;
                    let parts: Vec<String> = result_stack.drain(start..).collect();
                    let segs: Vec<String> = parts
                        .into_iter()
                        .enumerate()
                        .map(|(i, rendered)| {
                            if malformed[i] {
                                rendered
                            } else {
                                format!("{} <- {}", var_names[i], rendered)
                            }
                        })
                        .collect();
                    result_stack.push(format!("{{ {} }}", segs.join(", ")));
                }
            }
        }

        result_stack.pop().unwrap_or_default()
    }

    fn to_display_string(&self) -> std::string::String {
        // **Memory-safety fix (2026-05-15)**: per-call memoization keyed by
        // slab pointer. PLN produces deeply shared substructure (e.g.,
        // `(Implication X X)` where X is itself deep); without memoization
        // each occurrence of a shared subtree triggers fresh full expansion
        // via the work-list, causing exponential blowup in result_stack /
        // work_stack. Robot.metta repro 2026-05-15 hit `memory allocation
        // of 6738207434563584 bytes failed` (~6.7 PB) in Vec<String>::grow_one
        // from this function (coredump 832730 frame 14).
        //
        // The memo caches each unique slab pointer's formatted string once;
        // subsequent occurrences clone the cached String (O(string_len))
        // instead of re-expanding the subtree (O(2^depth) in worst case).
        //
        // Stack-based implementation to avoid recursion on deeply nested structures.
        // Similar to friendly_repr but strings are printed WITHOUT quotes.
        enum ReprWork<'a> {
            Process(&'a MettaValue),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
                /// Memo key: slab pointer of the value being rendered, or
                /// `None` for synthetic Joins that don't correspond to a
                /// single unique value.
                memo_key: Option<usize>,
            },
            /// Workstream B (Task #6 follow-up, 2026-05-18): HE-style render
            /// for `(Bindings ($x val) …)` — `{ $x <- val, … }` (empty: `{ }`).
            JoinBindings {
                var_names: Vec<String>,
                malformed: Vec<bool>,
                memo_key: Option<usize>,
            },
        }

        let mut work_stack: Vec<ReprWork<'_>> = Vec::with_capacity(16);
        let mut result_stack: Vec<std::string::String> = Vec::with_capacity(16);
        let mut memo: std::collections::HashMap<usize, std::string::String> =
            std::collections::HashMap::with_capacity(64);

        work_stack.push(ReprWork::Process(self));

        while let Some(work) = work_stack.pop() {
            match work {
                ReprWork::Process(val) => {
                    if val.is_inline() {
                        result_stack.push(match val.inline_tag() {
                            NB_TAG_LONG => val.inline_long_value().to_string(),
                            // Phase 1.5 PT alignment: lowercase per PHE-finer #10.
                            NB_TAG_BOOL => if (val.tagged as u64 & 1) != 0 {
                                "true"
                            } else {
                                "false"
                            }
                            .to_string(),
                            NB_TAG_UNIT => "()".to_string(),
                            NB_TAG_EMPTY => "Empty".to_string(),
                            _ => "()".to_string(),
                        });
                        continue;
                    }
                    // Memo lookup: O(1) by slab pointer.
                    let memo_key = val.inner_ptr() as usize;
                    if let Some(cached) = memo.get(&memo_key) {
                        result_stack.push(cached.clone());
                        continue;
                    }
                    match val.inner_ref() {
                        MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                        MettaValueInner::Float(f) => result_stack.push(float_canonical(*f)),
                        MettaValueInner::Bool(b) => {
                            result_stack.push(if *b { "true" } else { "false" }.to_string());
                        }
                        // Key difference: strings printed without quotes for display
                        MettaValueInner::String(s) => result_stack.push(s.to_string()),
                        MettaValueInner::Atom(a) => result_stack.push(a.to_string()),
                        MettaValueInner::Unit => result_stack.push("()".to_string()),
                        MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                        MettaValueInner::NotReducible => {
                            result_stack.push("NotReducible".to_string())
                        }
                        MettaValueInner::Space(handle) => {
                            // Phase C (HE bisim, 2026-05-20): HE-aligned space
                            // print form per fixture T04/028 / §06.15.
                            let canonical = if handle.name == "self" {
                                "ModuleSpace(GroundingSpace-top)".to_string()
                            } else {
                                format!("&{}", handle.name)
                            };
                            result_stack.push(canonical);
                        }
                        MettaValueInner::State(id) => {
                            result_stack.push(format!("(State {})", id));
                        }
                        MettaValueInner::Memo(handle) => {
                            result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                        }
                        MettaValueInner::Error(offending, detail) => {
                            work_stack.push(ReprWork::Join {
                                count: 2,
                                prefix: "(Error ",
                                suffix: ")",
                                separator: " ",
                                memo_key: Some(memo_key),
                            });
                            work_stack.push(ReprWork::Process(detail));
                            work_stack.push(ReprWork::Process(offending));
                        }
                        MettaValueInner::Type(t) => {
                            work_stack.push(ReprWork::Join {
                                count: 1,
                                prefix: "(: ",
                                suffix: ")",
                                separator: "",
                                memo_key: Some(memo_key),
                            });
                            work_stack.push(ReprWork::Process(t));
                        }
                        MettaValueInner::Quoted(inner) => {
                            work_stack.push(ReprWork::Join {
                                count: 1,
                                prefix: "(quote ",
                                suffix: ")",
                                separator: "",
                                memo_key: Some(memo_key),
                            });
                            work_stack.push(ReprWork::Process(inner));
                        }
                        MettaValueInner::Lazy(inner) => {
                            // PT-canonical Lazy is INVISIBLE (2026-05-21):
                            // delegate to inner so users see substituted
                            // values verbatim — no `(quote ...)` wrap.
                            work_stack.push(ReprWork::Process(inner));
                        }
                        MettaValueInner::SExpr(items) => {
                            if items.is_empty() {
                                let s = "()".to_string();
                                memo.insert(memo_key, s.clone());
                                result_stack.push(s);
                            } else if items.first().and_then(|h| h.as_atom()) == Some("Bindings") {
                                // Workstream B: HE-style `{ }` / `{ $x <- val, … }`.
                                let pairs = &items[1..];
                                if pairs.is_empty() {
                                    let s = "{ }".to_string();
                                    memo.insert(memo_key, s.clone());
                                    result_stack.push(s);
                                } else {
                                    let mut var_names: Vec<String> =
                                        Vec::with_capacity(pairs.len());
                                    let mut malformed: Vec<bool> = Vec::with_capacity(pairs.len());
                                    for p in pairs {
                                        let (name, ok) = match p.inner_ref() {
                                            MettaValueInner::SExpr(kv) if kv.len() == 2 => match kv
                                                [0]
                                            .inner_ref()
                                            {
                                                MettaValueInner::Atom(n) => (n.to_string(), true),
                                                _ => (String::new(), false),
                                            },
                                            _ => (String::new(), false),
                                        };
                                        var_names.push(name);
                                        malformed.push(!ok);
                                    }
                                    work_stack.push(ReprWork::JoinBindings {
                                        var_names,
                                        malformed: malformed.clone(),
                                        memo_key: Some(memo_key),
                                    });
                                    for (i, p) in pairs.iter().enumerate().rev() {
                                        let to_render: &MettaValue = if malformed[i] {
                                            p
                                        } else if let MettaValueInner::SExpr(kv) = p.inner_ref() {
                                            &kv[1]
                                        } else {
                                            p
                                        };
                                        work_stack.push(ReprWork::Process(to_render));
                                    }
                                }
                            } else {
                                work_stack.push(ReprWork::Join {
                                    count: items.len(),
                                    prefix: "(",
                                    suffix: ")",
                                    separator: " ",
                                    memo_key: Some(memo_key),
                                });
                                for item in items.iter().rev() {
                                    work_stack.push(ReprWork::Process(item));
                                }
                            }
                        }
                        MettaValueInner::Conjunction(goals) => {
                            if goals.is_empty() {
                                let s = "(,)".to_string();
                                memo.insert(memo_key, s.clone());
                                result_stack.push(s);
                            } else {
                                work_stack.push(ReprWork::Join {
                                    count: goals.len(),
                                    prefix: "(, ",
                                    suffix: ")",
                                    separator: " ",
                                    memo_key: Some(memo_key),
                                });
                                for goal in goals.iter().rev() {
                                    work_stack.push(ReprWork::Process(goal));
                                }
                            }
                        }
                        MettaValueInner::Spanned(v, _) => {
                            // Spanned wrappers don't get their own memo entry;
                            // the inner value is what shares.
                            work_stack.push(ReprWork::Process(v));
                        }
                    }
                }
                ReprWork::Join {
                    count,
                    prefix,
                    suffix,
                    separator,
                    memo_key,
                } => {
                    let start = result_stack.len() - count;
                    let parts: Vec<std::string::String> = result_stack.drain(start..).collect();
                    let formatted = format!("{}{}{}", prefix, parts.join(separator), suffix);
                    if let Some(key) = memo_key {
                        memo.insert(key, formatted.clone());
                    }
                    result_stack.push(formatted);
                }
                ReprWork::JoinBindings {
                    var_names,
                    malformed,
                    memo_key,
                } => {
                    let count = var_names.len();
                    let start = result_stack.len() - count;
                    let parts: Vec<String> = result_stack.drain(start..).collect();
                    let segs: Vec<String> = parts
                        .into_iter()
                        .enumerate()
                        .map(|(i, rendered)| {
                            if malformed[i] {
                                rendered
                            } else {
                                format!("{} <- {}", var_names[i], rendered)
                            }
                        })
                        .collect();
                    let s = format!("{{ {} }}", segs.join(", "));
                    if let Some(key) = memo_key {
                        memo.insert(key, s.clone());
                    }
                    result_stack.push(s);
                }
            }
        }

        result_stack.pop().unwrap_or_default()
    }
}

// ============================================================================
// Serialization helpers for MettaValue
// ============================================================================

/// Tag bytes for serialization format (same as MettaValue)
pub mod serialize_tags {
    pub const ATOM: u8 = 0x01;
    pub const BOOL: u8 = 0x02;
    pub const LONG: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const STRING: u8 = 0x05;
    pub const SEXPR: u8 = 0x06;
    pub const UNIT_LEGACY: u8 = 0x07;
    pub const ERROR: u8 = 0x08;
    pub const TYPE: u8 = 0x09;
    pub const CONJUNCTION: u8 = 0x0A;
    pub const UNIT: u8 = 0x0B;
    pub const EMPTY: u8 = 0x0C;
    pub const SPACE: u8 = 0x0D;
    pub const STATE: u8 = 0x0E;
    pub const MEMO: u8 = 0x0F;
    pub const QUOTED: u8 = 0x10;
    /// Plan S0a (2026-05-13) — HE `NotReducible` sentinel tag.
    pub const NOT_REDUCIBLE: u8 = 0x11;
}

/// Write a varint to buffer
fn write_varint(buf: &mut Vec<u8>, mut n: usize) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
}

/// Read a varint from bytes
pub(crate) fn read_varint(bytes: &[u8]) -> Result<(usize, usize), std::string::String> {
    let mut result: usize = 0;
    let mut shift = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        result |= ((byte & 0x7F) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, i + 1));
        }
        shift += 7;
        if shift > 63 {
            return Err("varint overflow".to_string());
        }
    }
    Err("unexpected end of varint".to_string())
}

// hash_value_for_trait removed — replaced by hash_value_cached_inner + hash_value_for_trait_inner
// which use pointer-keyed thread-local caching + Boost hash_combine for O(1) amortized hashing.

/// Serialize an MettaValue to bytes.
///
/// **Stack-safety mandate (2026-05-15)**: refactored to iterative work-list.
/// Audit item T#21. Was recursive on SExpr / Error / Type / Conjunction /
/// Quoted / Spanned children — deeply-nested values would overflow. No
/// memoization needed: serialization is sequential byte-writing, and shared
/// substructure must still produce identical byte sequences for each
/// occurrence (the parser-side reconstructs separate values for each).
fn serialize_value(value: &MettaValue, buf: &mut Vec<u8>) {
    let mut work: Vec<MettaValue> = Vec::with_capacity(8);
    work.push(value.clone());
    while let Some(val) = work.pop() {
        if val.is_inline() {
            match val.inline_tag() {
                NB_TAG_BOOL => {
                    buf.push(BOOL);
                    buf.push(if (val.tagged as u64 & 1) != 0 { 1 } else { 0 });
                }
                NB_TAG_LONG => {
                    buf.push(LONG);
                    buf.extend_from_slice(&val.inline_long_value().to_le_bytes());
                }
                NB_TAG_UNIT => {
                    buf.push(UNIT);
                }
                NB_TAG_EMPTY => {
                    buf.push(EMPTY);
                }
                _ => {
                    buf.push(UNIT);
                }
            }
            continue;
        }
        match val.inner_ref() {
            MettaValueInner::Atom(s) => {
                buf.push(ATOM);
                write_varint(buf, s.len());
                buf.extend_from_slice(s.as_bytes());
            }
            MettaValueInner::Bool(b) => {
                buf.push(BOOL);
                buf.push(if *b { 1 } else { 0 });
            }
            MettaValueInner::Long(n) => {
                buf.push(LONG);
                buf.extend_from_slice(&n.to_le_bytes());
            }
            MettaValueInner::Float(f) => {
                buf.push(FLOAT);
                buf.extend_from_slice(&f.to_le_bytes());
            }
            MettaValueInner::String(s) => {
                buf.push(STRING);
                write_varint(buf, s.len());
                buf.extend_from_slice(s.as_bytes());
            }
            MettaValueInner::SExpr(items) => {
                buf.push(SEXPR);
                write_varint(buf, items.len());
                // Push children in reverse so first child is serialized first.
                for item in items.iter().rev() {
                    work.push(item.clone());
                }
            }
            MettaValueInner::Unit => {
                buf.push(UNIT_LEGACY);
            }
            MettaValueInner::Error(offending, detail) => {
                buf.push(ERROR);
                // Push detail then offending so offending is serialized first
                // (reverse stack order).
                work.push(detail.clone());
                work.push(offending.clone());
            }
            MettaValueInner::Type(inner) => {
                buf.push(TYPE);
                work.push(inner.clone());
            }
            MettaValueInner::Conjunction(goals) => {
                buf.push(CONJUNCTION);
                write_varint(buf, goals.len());
                for goal in goals.iter().rev() {
                    work.push(goal.clone());
                }
            }
            MettaValueInner::Empty => {
                buf.push(EMPTY);
            }
            MettaValueInner::Space(handle) => {
                buf.push(SPACE);
                buf.extend_from_slice(&handle.id.to_le_bytes());
                let name_bytes = handle.name.as_bytes();
                write_varint(buf, name_bytes.len());
                buf.extend_from_slice(name_bytes);
                buf.push(if handle.is_module_space() { 1 } else { 0 });
            }
            MettaValueInner::State(id) => {
                buf.push(STATE);
                buf.extend_from_slice(&id.to_le_bytes());
            }
            MettaValueInner::Quoted(inner) => {
                buf.push(QUOTED);
                work.push(inner.clone());
            }
            MettaValueInner::Lazy(inner) => {
                // PT-canonical Lazy is INVISIBLE for serialization
                // (2026-05-21): mirror Spanned and serialize the inner
                // value verbatim. The Lazy marker is purely a runtime
                // eval-inhibitor for substituted values and has no
                // persisted shape — its inner round-trips through the
                // byte stream as itself.
                work.push(inner.clone());
            }
            MettaValueInner::Memo(handle) => {
                buf.push(MEMO);
                buf.extend_from_slice(&handle.id.to_le_bytes());
            }
            MettaValueInner::NotReducible => {
                buf.push(NOT_REDUCIBLE);
            }
            MettaValueInner::Spanned(v, _) => {
                // Spanned is span-transparent for serialization.
                work.push(v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::gc_allocator::global_factory;
    use super::super::metta_value_trait::MettaValueFactory;
    use super::*;

    // ========================================================================
    // Basic Constructor and Accessor Tests
    // ========================================================================

    #[test]
    fn test_arena_atom() {
        let factory = global_factory();
        let v = factory.atom("hello");
        assert!(v.is_atom());
        assert_eq!(v.as_atom(), Some("hello"));
    }

    #[test]
    fn test_arena_long() {
        let factory = global_factory();
        let v = factory.long(42);
        assert!(v.is_long());
        assert_eq!(v.as_long(), Some(42));
    }

    #[test]
    fn test_arena_bool() {
        let factory = global_factory();
        let v = factory.bool(true);
        assert!(v.is_bool());
        assert_eq!(v.as_bool(), Some(true));
    }

    #[test]
    fn test_arena_sexpr() {
        let factory = global_factory();
        let items = vec![factory.atom("+"), factory.long(1), factory.long(2)];
        let v = factory.sexpr(items);
        assert!(v.is_sexpr());
        let items = v.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_copy_semantics() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = v1; // Copy, not move
        let v3 = v1; // Can copy again
        assert_eq!(v1.as_long(), Some(42));
        assert_eq!(v2.as_long(), Some(42));
        assert_eq!(v3.as_long(), Some(42));
    }

    #[test]
    fn test_display() {
        let factory = global_factory();
        let v = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        assert_eq!(format!("{}", v), "(+ 1 2)");
    }

    // ========================================================================
    // All Type Variant Constructor Tests (Phase 1)
    // ========================================================================

    #[test]
    fn test_arena_float() {
        let factory = global_factory();
        let v = factory.float(3.14);
        assert!(v.is_float());
        assert!(!v.is_long());
        assert_eq!(v.as_float(), Some(3.14));
        assert_eq!(v.as_long(), None);
    }

    #[test]
    fn test_arena_string() {
        let factory = global_factory();
        let v = factory.string("hello world");
        assert!(v.is_string());
        assert!(!v.is_atom());
        assert_eq!(v.as_string(), Some("hello world"));
        assert_eq!(v.as_atom(), None);
    }

    #[test]
    fn test_arena_nil() {
        let factory = global_factory();
        let v = factory.unit();
        // After Nil/Unit merge, nil() returns Unit
        assert!(v.is_unit());
        assert!(!v.is_empty());
    }

    #[test]
    fn test_arena_unit() {
        let factory = global_factory();
        let v = factory.unit();
        assert!(v.is_unit());
        assert!(!v.is_empty());
    }

    #[test]
    fn test_arena_empty() {
        let factory = global_factory();
        let v = factory.empty();
        assert!(v.is_empty());
        assert!(!v.is_unit());
    }

    #[test]
    fn test_arena_error() {
        // Phase 1.1 PT-canonical: Error(Type, Ctx).
        let factory = global_factory();
        let error_type = factory.atom("BadType");
        let ctx_val = factory.string("test error context");
        let v = factory.error(error_type, ctx_val);
        assert!(v.is_error());
        let (type_v, ctx_v) = v.as_error().expect("should be error");
        assert_eq!(type_v.as_atom(), Some("BadType"));
        assert_eq!(ctx_v.as_string(), Some("test error context"));
    }

    #[test]
    fn test_arena_type() {
        let factory = global_factory();
        let inner = factory.atom("Number");
        let v = factory.type_value(inner);
        assert!(v.is_type());
        let t = v.as_type().expect("should be type");
        assert_eq!(t.as_atom(), Some("Number"));
    }

    #[test]
    fn test_arena_conjunction() {
        let factory = global_factory();
        let goals = vec![factory.atom("goal1"), factory.atom("goal2")];
        let v = factory.conjunction(goals);
        assert!(v.is_conjunction());
        let conj = v.as_conjunction().expect("should be conjunction");
        assert_eq!(conj.len(), 2);
    }

    #[test]
    fn test_state() {
        let factory = global_factory();
        let v = factory.state(12345);
        assert!(v.is_state());
        assert_eq!(v.as_state(), Some(12345));
    }

    #[test]
    fn test_arena_sexpr_empty() {
        let factory = global_factory();
        // Empty sexpr via factory.sexpr(vec![]) normalizes to Unit
        let v = factory.sexpr(vec![]);
        // After Nil/Unit merge + BumpVec->slice: empty sexpr normalizes to Unit
        assert!(v.is_unit());
    }

    // ========================================================================
    // Type Check Method Coverage
    // ========================================================================

    #[test]
    fn test_is_variable_true() {
        let factory = global_factory();
        let v = factory.atom("$x");
        assert!(v.is_variable());
    }

    #[test]
    fn test_is_variable_false() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert!(!v.is_variable());
    }

    #[test]
    fn test_is_ground_type() {
        let factory = global_factory();

        // Ground types
        assert!(factory.bool(true).is_ground_type());
        assert!(factory.long(42).is_ground_type());
        assert!(factory.float(3.14).is_ground_type());
        assert!(factory.string("hello").is_ground_type());

        // Non-ground types
        assert!(!factory.atom("foo").is_ground_type());
        assert!(!factory.sexpr(vec![]).is_ground_type());
        // After Nil/Unit merge, Unit/() is NOT a ground type (it's an expression in MeTTa HE)
        assert!(!factory.unit().is_ground_type());
        assert!(!factory.unit().is_ground_type());
    }

    // ========================================================================
    // PartialEq Tests for All Variant Combinations
    // ========================================================================

    #[test]
    fn test_eq_long_long() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = factory.long(42);
        let v3 = factory.long(99);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_float_float() {
        let factory = global_factory();
        let v1 = factory.float(3.14);
        let v2 = factory.float(3.14);
        let v3 = factory.float(2.71);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_bool_bool() {
        let factory = global_factory();
        let t1 = factory.bool(true);
        let t2 = factory.bool(true);
        let f = factory.bool(false);
        assert_eq!(t1, t2);
        assert_ne!(t1, f);
    }

    #[test]
    fn test_eq_atom_atom() {
        let factory = global_factory();
        let v1 = factory.atom("foo");
        let v2 = factory.atom("foo");
        let v3 = factory.atom("bar");
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_string_string() {
        let factory = global_factory();
        let v1 = factory.string("hello");
        let v2 = factory.string("hello");
        let v3 = factory.string("world");
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_nil_nil() {
        let factory = global_factory();
        let v1 = factory.unit();
        let v2 = factory.unit();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_unit_unit() {
        let factory = global_factory();
        let v1 = factory.unit();
        let v2 = factory.unit();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_empty_empty() {
        let factory = global_factory();
        let v1 = factory.empty();
        let v2 = factory.empty();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_sexpr_sexpr() {
        let factory = global_factory();
        let v1 = factory.sexpr(vec![factory.atom("+"), factory.long(1)]);
        let v2 = factory.sexpr(vec![factory.atom("+"), factory.long(1)]);
        let v3 = factory.sexpr(vec![factory.atom("+"), factory.long(2)]);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_error_error() {
        let factory = global_factory();
        let off1 = factory.atom("d");
        let off2 = factory.atom("d");
        let det1 = factory.string("err");
        let det2 = factory.string("err");
        let v1 = factory.error(det1, off1);
        let v2 = factory.error(det2, off2);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_type_type() {
        let factory = global_factory();
        let i1 = factory.atom("Number");
        let i2 = factory.atom("Number");
        let v1 = factory.type_value(i1);
        let v2 = factory.type_value(i2);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_conjunction_conjunction() {
        let factory = global_factory();
        let v1 = factory.conjunction(vec![factory.atom("a")]);
        let v2 = factory.conjunction(vec![factory.atom("a")]);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_state_state() {
        let factory = global_factory();
        let v1 = factory.state(100);
        let v2 = factory.state(100);
        let v3 = factory.state(200);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_ne_different_types() {
        let factory = global_factory();
        let long = factory.long(42);
        let float = factory.float(42.0);
        let bool_val = factory.bool(true);
        let atom = factory.atom("42");
        let string = factory.string("42");

        // Different types should never be equal
        assert_ne!(long, float);
        assert_ne!(long, bool_val);
        assert_ne!(long, atom);
        assert_ne!(float, string);
        assert_ne!(atom, string);
    }

    // ========================================================================
    // Accessor Method Edge Cases
    // ========================================================================

    #[test]
    fn test_accessor_wrong_type_returns_none() {
        let factory = global_factory();
        let v = factory.long(42);
        assert_eq!(v.as_atom(), None);
        assert_eq!(v.as_bool(), None);
        assert_eq!(v.as_float(), None);
        assert_eq!(v.as_string(), None);
        assert_eq!(v.as_sexpr(), None);
        assert_eq!(v.as_error(), None);
        assert_eq!(v.as_type(), None);
        assert_eq!(v.as_conjunction(), None);
        assert_eq!(v.as_space(), None);
        assert_eq!(v.as_state(), None);
        assert_eq!(v.as_memo(), None);
    }

    // ========================================================================
    // Type Name Tests
    // ========================================================================

    #[test]
    fn test_type_name_variable() {
        let factory = global_factory();
        let v = factory.atom("$x");
        assert_eq!(v.type_name(), "Variable");
    }

    #[test]
    fn test_type_name_symbol() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert_eq!(v.type_name(), "Symbol");
    }

    #[test]
    fn test_type_name_bool() {
        let factory = global_factory();
        let v = factory.bool(true);
        assert_eq!(v.type_name(), "Bool");
    }

    #[test]
    fn test_type_name_long() {
        let factory = global_factory();
        let v = factory.long(42);
        assert_eq!(v.type_name(), "Number");
    }

    #[test]
    fn test_type_name_float() {
        let factory = global_factory();
        let v = factory.float(3.14);
        assert_eq!(v.type_name(), "Number");
    }

    #[test]
    fn test_type_name_string() {
        let factory = global_factory();
        let v = factory.string("hello");
        assert_eq!(v.type_name(), "String");
    }

    #[test]
    fn test_type_name_sexpr() {
        let factory = global_factory();
        // Use a non-empty sexpr for the Expression type name test
        let v = factory.sexpr(vec![factory.long(1)]);
        assert_eq!(v.type_name(), "Expression");
    }

    #[test]
    fn test_type_name_nil() {
        // After Nil/Unit merge, nil() returns Unit which has type_name "Unit"
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(v.type_name(), "Unit");
    }

    #[test]
    fn test_type_name_error() {
        let factory = global_factory();
        let offending = factory.unit();
        let detail = factory.string("err");
        let v = factory.error(detail, offending);
        assert_eq!(v.type_name(), "Error");
    }

    #[test]
    fn test_type_name_type() {
        let factory = global_factory();
        let i = factory.atom("Int");
        let v = factory.type_value(i);
        assert_eq!(v.type_name(), "Type");
    }

    #[test]
    fn test_type_name_conjunction() {
        let factory = global_factory();
        let v = factory.conjunction(vec![]);
        assert_eq!(v.type_name(), "Conjunction");
    }

    #[test]
    fn test_type_name_state() {
        let factory = global_factory();
        let v = factory.state(1);
        assert_eq!(v.type_name(), "State");
    }

    #[test]
    fn test_type_name_unit() {
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(v.type_name(), "Unit");
    }

    #[test]
    fn test_type_name_empty() {
        let factory = global_factory();
        let v = factory.empty();
        assert_eq!(v.type_name(), "Empty");
    }

    // ========================================================================
    // Display Formatting Tests
    // ========================================================================

    #[test]
    fn test_display_bool_true() {
        let factory = global_factory();
        let v = factory.bool(true);
        // Phase 1.5 PT alignment: lowercase per PHE-finer #10.
        assert_eq!(format!("{}", v), "true");
    }

    #[test]
    fn test_display_bool_false() {
        let factory = global_factory();
        let v = factory.bool(false);
        assert_eq!(format!("{}", v), "false");
    }

    #[test]
    fn test_display_long() {
        let factory = global_factory();
        let v = factory.long(-42);
        assert_eq!(format!("{}", v), "-42");
    }

    #[test]
    fn test_display_float() {
        let factory = global_factory();
        let v = factory.float(3.14);
        assert_eq!(format!("{}", v), "3.14");
    }

    #[test]
    fn test_display_string() {
        let factory = global_factory();
        let v = factory.string("hello");
        assert_eq!(format!("{}", v), "\"hello\"");
    }

    #[test]
    fn test_display_nil() {
        // After Nil/Unit merge, nil() returns Unit which displays as "()"
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_unit() {
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_empty() {
        let factory = global_factory();
        let v = factory.empty();
        assert_eq!(format!("{}", v), "Empty");
    }

    #[test]
    fn test_display_empty_sexpr() {
        let factory = global_factory();
        // Empty sexpr via factory normalizes to Unit
        let v = factory.sexpr(vec![]);
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_error() {
        // Phase 1.1 PT-canonical: Error(Type, Ctx). Display emits `(Error <Type> <Ctx>)`.
        let factory = global_factory();
        let error_type = factory.atom("BadType");
        let ctx_val = factory.string("msg");
        let v = factory.error(error_type, ctx_val);
        assert_eq!(format!("{}", v), "(Error BadType \"msg\")");
    }

    #[test]
    fn test_display_type() {
        let factory = global_factory();
        let i = factory.atom("Int");
        let v = factory.type_value(i);
        assert_eq!(format!("{}", v), "(: Int)");
    }

    #[test]
    fn test_display_conjunction() {
        let factory = global_factory();
        let v = factory.conjunction(vec![factory.atom("a"), factory.atom("b")]);
        assert_eq!(format!("{}", v), "(, a b)");
    }

    #[test]
    fn test_display_state() {
        let factory = global_factory();
        let v = factory.state(123);
        assert_eq!(format!("{}", v), "<State:123>");
    }

    // ========================================================================
    // Serialization Round-Trip Tests
    // ========================================================================

    #[test]
    fn test_serialize_roundtrip_long() {
        let factory = global_factory();
        let original = factory.long(12345);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_float() {
        let factory = global_factory();
        let original = factory.float(3.14159);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_bool() {
        let factory = global_factory();
        let original = factory.bool(true);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_atom() {
        let factory = global_factory();
        let original = factory.atom("hello-world");
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_string() {
        let factory = global_factory();
        let original = factory.string("test string");
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nil() {
        let factory = global_factory();
        let original = factory.unit();
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_unit() {
        let factory = global_factory();
        let original = factory.unit();
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_empty() {
        let factory = global_factory();
        let original = factory.empty();
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_sexpr() {
        let factory = global_factory();
        let original = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nested_sexpr() {
        let factory = global_factory();
        let inner = factory.sexpr(vec![factory.atom("*"), factory.long(2), factory.long(3)]);
        let original = factory.sexpr(vec![factory.atom("+"), factory.long(1), inner]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_error() {
        // Phase 1.1 PT-canonical: Error(Type, Ctx) — roundtrip preserves field order.
        let factory = global_factory();
        let error_type = factory.atom("BadType");
        let ctx_val = factory.string("test error");
        let original = factory.error(error_type, ctx_val);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_type() {
        let factory = global_factory();
        let inner = factory.atom("Number");
        let original = factory.type_value(inner);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_conjunction() {
        let factory = global_factory();
        let original = factory.conjunction(vec![factory.atom("a"), factory.atom("b")]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_state() {
        let factory = global_factory();
        let original = factory.state(999);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    // ========================================================================
    // Hash Value Tests
    // ========================================================================

    #[test]
    fn test_hash_equal_values() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = factory.long(42);
        assert_eq!(v1.hash_value(), v2.hash_value());
    }

    #[test]
    fn test_hash_different_values() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = factory.long(43);
        assert_ne!(v1.hash_value(), v2.hash_value());
    }

    #[test]
    fn test_hash_different_types() {
        let factory = global_factory();
        let long = factory.long(42);
        let float = factory.float(42.0);
        // Different types should (almost certainly) have different hashes
        assert_ne!(long.hash_value(), float.hash_value());
    }

    #[test]
    fn test_hash_nil_unit_empty() {
        let factory = global_factory();
        let nil = factory.unit();
        let unit = factory.unit();
        let empty = factory.empty();
        // After Nil/Unit merge, nil and unit are the same value
        assert_eq!(nil.hash_value(), unit.hash_value());
        // Empty is still distinct from unit
        assert_ne!(unit.hash_value(), empty.hash_value());
    }

    // ========================================================================
    // MettaValueTrait Method Tests
    // ========================================================================

    #[test]
    fn test_get_head_symbol_sexpr() {
        let factory = global_factory();
        let v = factory.sexpr(vec![factory.atom("foo"), factory.long(1)]);
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_head_symbol_variable_head() {
        let factory = global_factory();
        let v = factory.sexpr(vec![factory.atom("$x"), factory.long(1)]);
        // Variable as head returns None
        assert_eq!(v.get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_bare_atom() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_arity_sexpr() {
        let factory = global_factory();
        let v = factory.sexpr(vec![factory.atom("foo"), factory.long(1), factory.long(2)]);
        // Arity is len - 1 (excluding head)
        assert_eq!(v.get_arity(), 2);
    }

    #[test]
    fn test_get_arity_atom() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert_eq!(v.get_arity(), 0);
    }

    #[test]
    fn test_friendly_repr() {
        let factory = global_factory();
        let v = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        assert_eq!(v.friendly_repr(), "(+ 1 2)");
    }

    #[test]
    fn test_friendly_type_name() {
        let factory = global_factory();
        assert_eq!(factory.long(1).friendly_type_name(), "Number (integer)");
        assert_eq!(factory.float(1.0).friendly_type_name(), "Number (float)");
        assert_eq!(factory.bool(true).friendly_type_name(), "Bool");
    }

    // ========================================================================
    // Factory Tests
    // ========================================================================

    #[test]
    fn test_factory_creates_values() {
        let factory = global_factory();

        let atom = factory.atom("test");
        assert!(atom.is_atom());

        let long = factory.long(42);
        assert!(long.is_long());

        let bool_val = factory.bool(true);
        assert!(bool_val.is_bool());

        let nil = factory.unit();
        assert!(nil.is_unit()); // nil() returns Unit after Nil/Unit merge

        let unit = factory.unit();
        assert!(unit.is_unit());
    }

    #[test]
    fn test_factory_sexpr_from_vec() {
        let factory = global_factory();

        let items = vec![factory.atom("+"), factory.long(1), factory.long(2)];
        let sexpr = factory.sexpr(items);
        assert!(sexpr.is_sexpr());
        assert_eq!(sexpr.as_sexpr().map(|s| s.len()), Some(3));
    }

    #[test]
    fn test_factory_sexpr_from_slice() {
        let factory = global_factory();

        let items = [factory.atom("+"), factory.long(1), factory.long(2)];
        let sexpr = factory.sexpr_from_slice(&items);
        assert!(sexpr.is_sexpr());
        assert_eq!(sexpr.as_sexpr().map(|s| s.len()), Some(3));
    }

    // ========================================================================
    // Deserialization Error Handling
    // ========================================================================

    #[test]
    fn test_deserialize_empty_input() {
        let factory = global_factory();
        let result = factory.deserialize(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_unknown_tag() {
        let factory = global_factory();
        let result = factory.deserialize(&[0xFF]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tag"));
    }

    #[test]
    fn test_deserialize_truncated_long() {
        let factory = global_factory();
        // LONG tag but only 4 bytes (needs 8)
        let result = factory.deserialize(&[0x03, 0x00, 0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_truncated_float() {
        let factory = global_factory();
        // FLOAT tag but only 4 bytes (needs 8)
        let result = factory.deserialize(&[0x04, 0x00, 0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    // ========================================================================
    // Pointer Equality Fast Path
    // ========================================================================

    #[test]
    fn test_pointer_equality_fast_path() {
        let factory = global_factory();
        let v = factory.long(42);
        let v_copy = v; // Copy, same pointer
                        // Both should be equal via pointer comparison fast path
        assert_eq!(v, v_copy);
    }

    // ========================================================================
    // Numeric Equality Tests (numeric_equal, numeric_not_equal,
    //                         numeric_equal_generic, float_equal)
    // ========================================================================

    #[test]
    fn test_numeric_equal_long_long() {
        let a = MettaValue::Long(2);
        let b = MettaValue::Long(2);
        let c = MettaValue::Long(3);
        assert!(numeric_equal(&a, &b), "Long(2) == Long(2) should be true");
        assert!(!numeric_equal(&a, &c), "Long(2) == Long(3) should be false");
    }

    #[test]
    fn test_numeric_equal_float_float() {
        let a = MettaValue::Float(2.0);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Float(3.0);
        assert!(
            numeric_equal(&a, &b),
            "Float(2.0) == Float(2.0) should be true"
        );
        assert!(
            !numeric_equal(&a, &c),
            "Float(2.0) == Float(3.0) should be false"
        );
    }

    #[test]
    fn test_numeric_equal_long_float() {
        // This is the key MeTTa HE fix: cross-type numeric equality
        let a = MettaValue::Long(2);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Float(2.5);
        assert!(
            numeric_equal(&a, &b),
            "Long(2) == Float(2.0) should be true (MeTTa HE cross-type fix)"
        );
        assert!(
            !numeric_equal(&a, &c),
            "Long(2) == Float(2.5) should be false"
        );
    }

    #[test]
    fn test_numeric_equal_float_long() {
        // Symmetric case: Float on left, Long on right
        let a = MettaValue::Float(2.0);
        let b = MettaValue::Long(2);
        let c = MettaValue::Long(3);
        assert!(
            numeric_equal(&a, &b),
            "Float(2.0) == Long(2) should be true (symmetric)"
        );
        assert!(
            !numeric_equal(&MettaValue::Float(2.5), &MettaValue::Long(2)),
            "Float(2.5) == Long(2) should be false"
        );
        assert!(
            !numeric_equal(&a, &c),
            "Float(2.0) == Long(3) should be false"
        );
    }

    #[test]
    fn test_numeric_equal_non_numeric_structural() {
        // Non-numeric types fall through to structural PartialEq
        let foo1 = MettaValue::Atom("foo");
        let foo2 = MettaValue::Atom("foo");
        let bar = MettaValue::Atom("bar");
        let t1 = MettaValue::Bool(true);
        let t2 = MettaValue::Bool(true);

        assert!(
            numeric_equal(&foo1, &foo2),
            "Atom(\"foo\") == Atom(\"foo\") should be true (structural)"
        );
        assert!(
            !numeric_equal(&foo1, &bar),
            "Atom(\"foo\") == Atom(\"bar\") should be false (structural)"
        );
        assert!(
            numeric_equal(&t1, &t2),
            "Bool(true) == Bool(true) should be true (structural)"
        );
    }

    #[test]
    fn test_numeric_equal_cross_type_non_numeric() {
        // Cross-type comparisons between non-numeric types and numeric types
        let foo = MettaValue::Atom("foo");
        let one_long = MettaValue::Long(1);
        let t = MettaValue::Bool(true);

        assert!(
            !numeric_equal(&foo, &one_long),
            "Atom(\"foo\") == Long(1) should be false"
        );
        assert!(
            !numeric_equal(&t, &one_long),
            "Bool(true) == Long(1) should be false"
        );
    }

    #[test]
    fn test_numeric_not_equal_basic() {
        let a = MettaValue::Long(2);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Long(3);

        assert!(
            !numeric_not_equal(&a, &b),
            "numeric_not_equal(Long(2), Float(2.0)) should be false (they are equal)"
        );
        assert!(
            numeric_not_equal(&a, &c),
            "numeric_not_equal(Long(2), Long(3)) should be true (they are not equal)"
        );
    }

    #[test]
    fn test_numeric_equal_ieee754_nan() {
        // IEEE 754: NaN != NaN
        let nan1 = MettaValue::Float(f64::NAN);
        let nan2 = MettaValue::Float(f64::NAN);
        assert!(
            !numeric_equal(&nan1, &nan2),
            "Float(NaN) == Float(NaN) should be false per IEEE 754"
        );
    }

    #[test]
    fn test_numeric_equal_ieee754_zero() {
        // IEEE 754: +0.0 == -0.0
        let pos_zero = MettaValue::Float(0.0);
        let neg_zero = MettaValue::Float(-0.0);
        let long_zero = MettaValue::Long(0);

        assert!(
            numeric_equal(&pos_zero, &neg_zero),
            "Float(0.0) == Float(-0.0) should be true per IEEE 754"
        );
        assert!(
            numeric_equal(&long_zero, &pos_zero),
            "Long(0) == Float(0.0) should be true"
        );
    }

    #[test]
    fn test_numeric_equal_generic_works() {
        // Test the generic version with MettaValue (same trait bound)
        let a = MettaValue::Long(2);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Long(3);

        assert!(
            numeric_equal_generic(&a, &b),
            "numeric_equal_generic: Long(2) == Float(2.0) should be true"
        );
        assert!(
            !numeric_equal_generic(&a, &c),
            "numeric_equal_generic: Long(2) == Long(3) should be false"
        );
    }

    // ================================================================
    // Phase 6: Serialization & trait transparency tests
    // ================================================================

    #[test]
    fn test_spanned_equality_transparent() {
        use crate::backend::models::global_factory;
        use crate::ir::{Position, Span};

        let factory = global_factory();
        let span1 = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 2,
                byte_offset: 2,
            },
        };
        let span2 = Span {
            start: Position {
                row: 5,
                column: 3,
                byte_offset: 50,
            },
            end: Position {
                row: 5,
                column: 5,
                byte_offset: 52,
            },
        };

        let bare = factory.long(42);
        let spanned1 = factory.spanned(factory.long(42), span1);
        let spanned2 = factory.spanned(factory.long(42), span2);

        // Spanned(v, s) == v
        assert_eq!(bare, spanned1, "Spanned should equal bare value");
        assert_eq!(spanned1, bare, "bare value should equal Spanned");
        // Spanned(v, s1) == Spanned(v, s2) (different spans)
        assert_eq!(
            spanned1, spanned2,
            "different spans should not affect equality"
        );
    }

    #[test]
    fn test_spanned_hash_transparent() {
        use crate::backend::models::global_factory;
        use crate::ir::{Position, Span};
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let factory = global_factory();
        let span = Span {
            start: Position {
                row: 1,
                column: 0,
                byte_offset: 10,
            },
            end: Position {
                row: 1,
                column: 5,
                byte_offset: 15,
            },
        };

        let bare = factory.long(42);
        let spanned = factory.spanned(factory.long(42), span);

        let hash_bare = {
            let mut h = DefaultHasher::new();
            bare.hash(&mut h);
            h.finish()
        };
        let hash_spanned = {
            let mut h = DefaultHasher::new();
            spanned.hash(&mut h);
            h.finish()
        };

        assert_eq!(
            hash_bare, hash_spanned,
            "Spanned and bare should hash identically"
        );
    }

    #[test]
    fn test_spanned_display_transparent() {
        use crate::backend::models::global_factory;
        use crate::ir::{Position, Span};

        let factory = global_factory();
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 5,
                byte_offset: 5,
            },
        };

        let bare = factory.atom("hello");
        let spanned = factory.spanned(factory.atom("hello"), span);

        assert_eq!(
            format!("{}", bare),
            format!("{}", spanned),
            "Spanned should display identically to bare value"
        );
        assert_eq!(format!("{}", spanned), "hello");
    }

    #[test]
    fn test_spanned_serialize_transparent() {
        use crate::backend::models::{global_factory, MettaValueTrait};
        use crate::ir::{Position, Span};

        let factory = global_factory();
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 2,
                byte_offset: 2,
            },
        };

        let bare = factory.long(42);
        let spanned = factory.spanned(factory.long(42), span);

        let bare_bytes = bare.serialize();
        let spanned_bytes = spanned.serialize();

        assert_eq!(
            bare_bytes, spanned_bytes,
            "Spanned and bare should serialize identically (span stripped)"
        );
    }

    #[test]
    fn test_spanned_sexpr_serialize_transparent() {
        use crate::backend::models::{global_factory, MettaValueTrait};
        use crate::ir::{Position, Span};

        let factory = global_factory();
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 7,
                byte_offset: 7,
            },
        };

        let bare = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        let spanned = factory.spanned(
            factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]),
            span,
        );

        let bare_bytes = bare.serialize();
        let spanned_bytes = spanned.serialize();

        assert_eq!(
            bare_bytes, spanned_bytes,
            "Spanned S-expression should serialize identically to bare"
        );
    }
}
