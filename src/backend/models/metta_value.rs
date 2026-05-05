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

use std::cell::RefCell;
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
        if k == key { return Some(v); }
        self.l2.get(&key).copied()
    }

    #[inline(always)]
    fn insert(&mut self, key: usize, hash: u64) {
        let idx = (key >> 4) & HASH_L1_MASK;
        // Safety: idx is always < HASH_L1_SIZE due to mask
        unsafe { *self.l1.get_unchecked_mut(idx) = (key, hash); }
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
}

/// Clear the thread-local hash value cache.
///
/// Must be called at GC safepoints before slab slots can be reused, to prevent
/// stale cached hashes from being returned for new values at recycled addresses.
pub fn clear_value_hash_cache() {
    VALUE_HASH_CACHE.with(|c| c.borrow_mut().clear());
}

/// Recursive helper: compute hash for a `MettaValue`, using `cache` for memoization.
///
/// For `SExpr`, children hashes are fetched from the cache (if available) and combined
/// with golden-ratio mixing, avoiding full Xxh3 tree traversal. This turns O(tree_size)
/// per call into O(arity) for cached children, and O(1) for fully-cached values.
fn hash_value_cached_inner(value: &MettaValue, cache: &mut TieredHashCache) -> u64 {
    // Golden ratio constants for primitive fast paths
    const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
    const LONG_SEED: u64 = 0x517cc1b727220a95;
    const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
    const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;
    const UNIT_HASH: u64 = 0x756e6974_68617368;

    // Fast path: inline NaN-boxed values — decode directly from tagged bits
    if value.is_inline() {
        return match value.inline_tag() {
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
                let x = (n as u64).wrapping_add(LONG_SEED).wrapping_mul(GOLDEN_RATIO);
                x ^ (x >> 32)
            }
            NB_TAG_EMPTY => 9u64.wrapping_mul(GOLDEN_RATIO),
            _ => UNIT_HASH,
        };
    }

    // Primitives (slab-allocated): compute directly (no cache needed, O(1))
    match value.inner_ref() {
        MettaValueInner::Unit => return UNIT_HASH,
        MettaValueInner::Bool(b) => {
            return if *b { BOOL_SEED.wrapping_mul(GOLDEN_RATIO) } else { BOOL_SEED };
        }
        MettaValueInner::Long(n) => {
            let x = (*n as u64).wrapping_add(LONG_SEED).wrapping_mul(GOLDEN_RATIO);
            return x ^ (x >> 32);
        }
        MettaValueInner::Float(f) => {
            let bits = f.to_bits();
            let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
            return x ^ (x >> 32);
        }
        MettaValueInner::Empty => return 9u64.wrapping_mul(GOLDEN_RATIO),
        _ => {}
    }

    // Cache lookup by slab pointer (L1 direct-mapped → L2 HashMap)
    let key = value.inner_ptr() as usize;
    if let Some(h) = cache.get(key) {
        return h;
    }

    // Cache miss: compute hash
    let h = match value.inner_ref() {
        MettaValueInner::Atom(s) => {
            let mut hasher = Xxh3::new();
            6u8.hash(&mut hasher);
            s.hash(&mut hasher);
            hasher.finish()
        }
        MettaValueInner::String(s) => {
            let mut hasher = Xxh3::new();
            5u8.hash(&mut hasher);
            s.hash(&mut hasher);
            hasher.finish()
        }
        MettaValueInner::SExpr(items) => {
            // Combine children hashes using Boost-style hash_combine.
            // Non-commutative, non-self-cancelling (unlike multiply-XOR which
            // self-cancels for recursive structures like (S (S Z))).
            // O(arity) when children are cached.
            let mut combined: u64 = 7u64 ^ items.len() as u64;
            for item in items.iter() {
                let child_hash = hash_value_cached_inner(item, cache);
                combined ^= child_hash
                    .wrapping_add(HASH_GOLDEN_RATIO)
                    .wrapping_add(combined << 6)
                    .wrapping_add(combined >> 2);
            }
            combined
        }
        MettaValueInner::Quoted(inner) => {
            let inner_hash = hash_value_cached_inner(inner, cache);
            let mut combined = 10u64;
            combined ^= inner_hash
                .wrapping_add(HASH_GOLDEN_RATIO)
                .wrapping_add(combined << 6)
                .wrapping_add(combined >> 2);
            combined
        }
        MettaValueInner::Spanned(inner, _span) => {
            return hash_value_cached_inner(inner, cache);
        }
        MettaValueInner::Error(..) => 8u64.wrapping_mul(HASH_GOLDEN_RATIO),
        // Type, Conjunction, Space, State, Memo — rare, use Xxh3 slow path
        other => {
            let mut hasher = Xxh3::new();
            hash_value_for_trait_inner(other, &mut hasher);
            hasher.finish()
        }
    };

    cache.insert(key, h);
    h
}

/// Low-level Xxh3 hasher for rare MettaValueInner variants (Type, Conjunction, etc.).
/// Only called on cache miss for non-primitive, non-SExpr values.
fn hash_value_for_trait_inner<H: Hasher>(inner: &MettaValueInner, hasher: &mut H) {
    match inner {
        MettaValueInner::Unit => 0u8.hash(hasher),
        MettaValueInner::Bool(b) => { 2u8.hash(hasher); b.hash(hasher); }
        MettaValueInner::Long(n) => { 3u8.hash(hasher); n.hash(hasher); }
        MettaValueInner::Float(f) => { 4u8.hash(hasher); f.to_bits().hash(hasher); }
        MettaValueInner::String(s) => { 5u8.hash(hasher); s.hash(hasher); }
        MettaValueInner::Atom(s) => { 6u8.hash(hasher); s.hash(hasher); }
        MettaValueInner::SExpr(_) => { 7u8.hash(hasher); } // children not traversed here
        MettaValueInner::Error(..) => 8u8.hash(hasher),
        MettaValueInner::Empty => 9u8.hash(hasher),
        MettaValueInner::Quoted(_) => 10u8.hash(hasher),
        MettaValueInner::Spanned(_, _) => 11u8.hash(hasher),
        MettaValueInner::Space(handle) => { 12u8.hash(hasher); handle.id.hash(hasher); }
        MettaValueInner::State(id) => { 13u8.hash(hasher); id.hash(hasher); }
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

/// The actual value enum, allocated in the arena.
///
/// This mirrors MettaValueInner but uses arena-allocated collections.
#[derive(Debug)]
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
    /// An error with message and details
    Error(&'static str, MettaValue),
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
    /// Empty sentinel
    Empty,
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
    // Slab-allocated types (Spanned layers are stripped by view())
    Atom(&'static str),
    String(&'static str),
    SExpr(&'static [MettaValue]),
    Error(&'static str, MettaValue),
    Type(MettaValue),
    Conjunction(&'static [MettaValue]),
    Space(&'static SpaceHandle),
    State(u64),
    Memo(&'static MemoHandle),
    Quoted(MettaValue),
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
static INLINE_TRUE_INNER: MettaValueInner = MettaValueInner::Bool(true);
static INLINE_FALSE_INNER: MettaValueInner = MettaValueInner::Bool(false);

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
        Self { tagged: (NB_TAG_BOOL | (b as u64)) as usize }
    }

    /// Create an inline Long value if it fits in 48 bits, otherwise None.
    /// Range: -(2^47) to (2^47)-1 = ±140,737,488,355,328.
    #[inline(always)]
    pub(crate) fn try_inline_long(n: i64) -> Option<Self> {
        if n >= NB_LONG_MIN && n <= NB_LONG_MAX {
            Some(Self { tagged: (NB_TAG_LONG | (n as u64 & NB_PAYLOAD_MASK)) as usize })
        } else {
            None
        }
    }

    /// Create an inline Unit value (no slab allocation).
    #[inline(always)]
    pub(crate) fn inline_unit() -> Self {
        Self { tagged: NB_TAG_UNIT as usize }
    }

    /// Create an inline Empty value (no slab allocation).
    #[inline(always)]
    pub(crate) fn inline_empty() -> Self {
        Self { tagged: NB_TAG_EMPTY as usize }
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
        unsafe { &*((self.tagged & PTR_MASK) as *const MettaValueInner) }
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
                debug_assert!(false, "unknown inline tag: 0x{:04x}", self.inline_tag() >> 48);
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
            MettaValueInner::Error(msg, details) => ValueView::Error(msg, *details),
            MettaValueInner::Type(inner_val) => ValueView::Type(*inner_val),
            MettaValueInner::Conjunction(goals) => ValueView::Conjunction(goals),
            MettaValueInner::Space(handle) => ValueView::Space(handle),
            MettaValueInner::State(id) => ValueView::State(*id),
            MettaValueInner::Memo(handle) => ValueView::Memo(handle),
            MettaValueInner::Quoted(inner_val) => ValueView::Quoted(*inner_val),
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
        if self.is_inline() { return None; } // Inline values are never Spanned
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
        if self.is_inline() { return *self; }
        MettaValue::from_inner(self.inner())
    }

    /// Strip one layer of Spanned, returning the inner value and its span.
    ///
    /// If the value is not Spanned, returns `(self, None)`.
    /// Inline NaN-boxed values are never Spanned.
    #[inline]
    pub fn peel_span(&self) -> (MettaValue, Option<&'static Span>) {
        if self.is_inline() { return (*self, None); }
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
        if self.is_inline() { return std::ptr::null(); }
        (self.tagged & PTR_MASK) as *const MettaValueInner
    }

    // ========================================================================
    // Type checking and inspection methods
    // ========================================================================

    /// Check if this is an Atom variant (transparent through Spanned)
    #[inline]
    pub fn is_atom(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Atom(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_atom(),
            _ => false,
        }
    }

    /// Check if this is a Bool variant (transparent through Spanned)
    #[inline]
    pub fn is_bool(&self) -> bool {
        if self.is_inline() { return self.inline_tag() == NB_TAG_BOOL; }
        match self.inner_ref() {
            MettaValueInner::Bool(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_bool(),
            _ => false,
        }
    }

    /// Check if this is a Long variant (transparent through Spanned)
    #[inline]
    pub fn is_long(&self) -> bool {
        if self.is_inline() { return self.inline_tag() == NB_TAG_LONG; }
        match self.inner_ref() {
            MettaValueInner::Long(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_long(),
            _ => false,
        }
    }

    /// Check if this is a Float variant (transparent through Spanned)
    #[inline]
    pub fn is_float(&self) -> bool {
        if self.is_inline() { return false; } // Floats are always slab-allocated
        match self.inner_ref() {
            MettaValueInner::Float(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_float(),
            _ => false,
        }
    }

    /// Check if this is a String variant (transparent through Spanned)
    #[inline]
    pub fn is_string(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::String(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_string(),
            _ => false,
        }
    }

    /// Check if this is an SExpr variant (transparent through Spanned)
    #[inline]
    pub fn is_sexpr(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::SExpr(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_sexpr(),
            _ => false,
        }
    }

    /// Check if this is an Error variant (transparent through Spanned)
    #[inline]
    pub fn is_error(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Error(_, _) => true,
            MettaValueInner::Spanned(v, _) => v.is_error(),
            _ => false,
        }
    }

    /// Check if this is a Type variant (transparent through Spanned)
    #[inline]
    pub fn is_type(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Type(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_type(),
            _ => false,
        }
    }

    /// Check if this is a Conjunction variant (transparent through Spanned)
    #[inline]
    pub fn is_conjunction(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Conjunction(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_conjunction(),
            _ => false,
        }
    }

    /// Check if this is a Space variant (transparent through Spanned)
    #[inline]
    pub fn is_space(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Space(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_space(),
            _ => false,
        }
    }

    /// Check if this is a State variant (transparent through Spanned)
    #[inline]
    pub fn is_state(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::State(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_state(),
            _ => false,
        }
    }

    /// Check if this is a Unit variant (transparent through Spanned)
    #[inline]
    pub fn is_unit(&self) -> bool {
        if self.is_inline() { return self.inline_tag() == NB_TAG_UNIT; }
        match self.inner_ref() {
            MettaValueInner::Unit => true,
            MettaValueInner::Spanned(v, _) => v.is_unit(),
            _ => false,
        }
    }

    /// Check if this is a Memo variant (transparent through Spanned)
    #[inline]
    pub fn is_memo(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Memo(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_memo(),
            _ => false,
        }
    }

    /// Check if this is a Quoted variant (transparent through Spanned)
    #[inline]
    pub fn is_quoted(&self) -> bool {
        if self.is_inline() { return false; }
        match self.inner_ref() {
            MettaValueInner::Quoted(_) => true,
            MettaValueInner::Spanned(v, _) => v.is_quoted(),
            _ => false,
        }
    }

    /// Check if this is an Empty variant (transparent through Spanned)
    #[inline]
    pub fn is_empty(&self) -> bool {
        if self.is_inline() { return self.inline_tag() == NB_TAG_EMPTY; }
        match self.inner_ref() {
            MettaValueInner::Empty => true,
            MettaValueInner::Spanned(v, _) => v.is_empty(),
            _ => false,
        }
    }

    /// Check if this value is a variable (Atom starting with $) (transparent through Spanned)
    #[inline]
    pub fn is_variable(&self) -> bool {
        if self.is_inline() { return false; } // Inline types are never variables
        match self.inner_ref() {
            MettaValueInner::Atom(s) if s.starts_with('$') => true,
            MettaValueInner::Spanned(v, _) => v.is_variable(),
            _ => false,
        }
    }

    /// Check if this value is a Spanned variant
    #[inline]
    pub fn is_spanned(&self) -> bool {
        if self.is_inline() { return false; }
        matches!(self.inner_ref(), MettaValueInner::Spanned(_, _))
    }

    // ========================================================================
    // Accessor methods for extracting inner values
    // ========================================================================

    /// Try to extract as atom string (transparent through Spanned)
    #[inline]
    pub fn as_atom(&self) -> Option<&'static str> {
        if self.is_inline() { return None; }
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
            _ => None,
        }
    }

    /// Try to extract as f64 (transparent through Spanned)
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        if self.is_inline() { return None; } // Floats are always slab-allocated
        match self.inner_ref() {
            MettaValueInner::Float(f) => Some(*f),
            MettaValueInner::Spanned(v, _) => v.as_float(),
            _ => None,
        }
    }

    /// Try to extract as string (transparent through Spanned)
    #[inline]
    pub fn as_string(&self) -> Option<&'static str> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::String(s) => Some(s),
            MettaValueInner::Spanned(v, _) => v.as_string(),
            _ => None,
        }
    }

    /// Try to extract as sexpr items (transparent through Spanned)
    #[inline]
    pub fn as_sexpr(&self) -> Option<&[MettaValue]> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::SExpr(items) => Some(items),
            MettaValueInner::Spanned(v, _) => v.as_sexpr(),
            _ => None,
        }
    }

    /// Try to extract as error (message, details) (transparent through Spanned)
    #[inline]
    pub fn as_error(&self) -> Option<(&'static str, MettaValue)> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Error(msg, details) => Some((msg, *details)),
            MettaValueInner::Spanned(v, _) => v.as_error(),
            _ => None,
        }
    }

    /// Try to extract as type inner value (transparent through Spanned)
    #[inline]
    pub fn as_type(&self) -> Option<MettaValue> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Type(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_type(),
            _ => None,
        }
    }

    /// Try to extract as conjunction goals (transparent through Spanned)
    #[inline]
    pub fn as_conjunction(&self) -> Option<&[MettaValue]> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Conjunction(goals) => Some(goals),
            MettaValueInner::Spanned(v, _) => v.as_conjunction(),
            _ => None,
        }
    }

    /// Try to extract as space handle (transparent through Spanned)
    #[inline]
    pub fn as_space(&self) -> Option<&SpaceHandle> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Space(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_space(),
            _ => None,
        }
    }

    /// Try to extract as state id (transparent through Spanned)
    #[inline]
    pub fn as_state(&self) -> Option<u64> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::State(id) => Some(*id),
            MettaValueInner::Spanned(v, _) => v.as_state(),
            _ => None,
        }
    }

    /// Try to extract as memo handle (transparent through Spanned)
    #[inline]
    pub fn as_memo(&self) -> Option<&MemoHandle> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Memo(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_memo(),
            _ => None,
        }
    }

    /// Try to extract the inner value of a Quoted variant (owned copy) (transparent through Spanned)
    #[inline]
    pub fn as_quoted(&self) -> Option<MettaValue> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted(),
            _ => None,
        }
    }

    /// Try to extract a reference to the inner value of a Quoted variant (transparent through Spanned)
    #[inline]
    pub fn as_quoted_ref(&self) -> Option<&MettaValue> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted_ref(),
            _ => None,
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
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
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
    #[allow(non_snake_case)]
    #[inline]
    pub fn Error(msg: impl AsRef<str>, details: MettaValue) -> Self {
        super::gc_allocator::global_factory().error(msg.as_ref(), details)
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

    /// Convert to canonical MeTTa string representation.
    /// Produces syntax that can be round-trip parsed by the MeTTa parser.
    /// Guarantees: parse(to_metta_string(value)) == value
    pub fn to_metta_string(&self) -> String {
        match self.inner_ref() {
            MettaValueInner::Atom(s) => s.to_string(),
            MettaValueInner::Bool(true) => "True".to_string(),
            MettaValueInner::Bool(false) => "False".to_string(),
            MettaValueInner::Long(n) => n.to_string(),
            MettaValueInner::Float(f) => {
                let s = f.to_string();
                // Ensure float representation is unambiguous
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            MettaValueInner::String(s) => format!("\"{}\"", escape_metta_string(s)),
            MettaValueInner::SExpr(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.to_metta_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({})", inner)
            }
            MettaValueInner::Unit => "()".to_string(),
            MettaValueInner::Error(msg, details) => {
                format!(
                    "(error \"{}\" {})",
                    escape_metta_string(msg),
                    details.to_metta_string()
                )
            }
            MettaValueInner::Type(t) => t.to_metta_string(),
            MettaValueInner::Conjunction(goals) => {
                if goals.is_empty() {
                    "(,)".to_string()
                } else {
                    let inner = goals
                        .iter()
                        .map(|v| v.to_metta_string())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("(, {})", inner)
                }
            }
            MettaValueInner::Quoted(inner) => format!("(quote {})", inner.to_metta_string()),
            MettaValueInner::Space(h) => format!("(Space {} \"{}\")", h.id, h.name),
            MettaValueInner::State(id) => format!("(State {})", id),
            MettaValueInner::Memo(h) => format!("(Memo {} \"{}\")", h.id, h.name),
            MettaValueInner::Empty => "Empty".to_string(),
            MettaValueInner::Spanned(v, _) => v.to_metta_string(),
        }
    }

    /// Convert to MORK s-expression string format.
    pub fn to_mork_string(&self) -> String {
        match self.inner_ref() {
            MettaValueInner::Atom(s) => {
                if *s == "&" || *s == "&self" || *s == "&kb" || *s == "&stack" {
                    s.to_string()
                } else if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                    format!("${}", &s[1..])
                } else if *s == "_" {
                    "$".to_string()
                } else {
                    s.to_string()
                }
            }
            MettaValueInner::Bool(b) => b.to_string(),
            MettaValueInner::Long(n) => n.to_string(),
            MettaValueInner::Float(f) => f.to_string(),
            MettaValueInner::String(s) => format!("\"{}\"", s),
            MettaValueInner::SExpr(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.to_mork_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({})", inner)
            }
            MettaValueInner::Unit => "()".to_string(),
            MettaValueInner::Error(msg, details) => {
                format!("(error \"{}\" {})", msg, details.to_mork_string())
            }
            MettaValueInner::Type(t) => t.to_mork_string(),
            MettaValueInner::Conjunction(goals) => {
                let inner = goals
                    .iter()
                    .map(|v| v.to_mork_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("(, {})", inner)
            }
            MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
            MettaValueInner::State(id) => format!("(State {})", id),
            MettaValueInner::Quoted(inner) => format!("(quote {})", inner.to_mork_string()),
            MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
            MettaValueInner::Empty => "Empty".to_string(),
            MettaValueInner::Spanned(v, _) => v.to_mork_string(),
        }
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
            MettaValueInner::Error(msg, details) => {
                format!(
                    r#"{{"type":"error","message":"{}","details":{}}}"#,
                    escape_json(msg),
                    details.to_json_string()
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
            MettaValueInner::Empty => r#"{"type":"empty"}"#.to_string(),
            MettaValueInner::Spanned(v, _) => v.to_json_string(),
        }
    }
}

/// Escape special characters in a string for JSON encoding.
pub fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Inline values: format directly without slab deref
        if self.is_inline() {
            return match self.view() {
                ValueView::Bool(b) => write!(f, "{}", if b { "True" } else { "False" }),
                ValueView::Long(n) => write!(f, "{}", n),
                ValueView::Unit => write!(f, "()"),
                ValueView::Empty => write!(f, "Empty"),
                _ => write!(f, "()"),
            };
        }
        // Display is span-transparent: Spanned delegates to inner value
        match self.inner_ref() {
            MettaValueInner::Atom(s) => write!(f, "{}", s),
            MettaValueInner::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            MettaValueInner::Long(n) => write!(f, "{}", n),
            // Spec §02: canonical float form preserves `.0` for whole-number
            // floats (e.g., `1500.0`, not `1500`) so parser round-trip yields
            // Float, not Long.
            MettaValueInner::Float(v) => {
                if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e16 {
                    write!(f, "{}.0", *v as i64)
                } else {
                    write!(f, "{}", v)
                }
            }
            // Spec §01.2: strings canonical-escape `\n`, `\t`, `\r`, `\\`, `\"`.
            MettaValueInner::String(s) => {
                write!(f, "\"")?;
                for c in s.chars() {
                    match c {
                        '\\' => write!(f, "\\\\")?,
                        '"' => write!(f, "\\\"")?,
                        '\n' => write!(f, "\\n")?,
                        '\t' => write!(f, "\\t")?,
                        '\r' => write!(f, "\\r")?,
                        _ => write!(f, "{}", c)?,
                    }
                }
                write!(f, "\"")
            }
            MettaValueInner::SExpr(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
            MettaValueInner::Unit => write!(f, "()"),
            MettaValueInner::Error(msg, details) => write!(f, "(Error {} {})", msg, details),
            MettaValueInner::Type(inner) => write!(f, "(: {})", inner),
            MettaValueInner::Conjunction(goals) => {
                write!(f, "(,")?;
                for goal in goals.iter() {
                    write!(f, " {}", goal)?;
                }
                write!(f, ")")
            }
            MettaValueInner::Space(handle) => write!(f, "<Space:{}>", handle.name),
            MettaValueInner::State(id) => write!(f, "<State:{}>", id),
            MettaValueInner::Quoted(inner) => write!(f, "(quote {})", inner),
            MettaValueInner::Memo(handle) => write!(f, "<Memo:{}>", handle.name),
            MettaValueInner::Empty => write!(f, "Empty"),
            MettaValueInner::Spanned(v, _) => write!(f, "{}", v),
        }
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
        // Strip Spanned wrappers for comparison — spans don't affect structural equality.
        // This ensures Spanned(v, s1) == Spanned(v, s2) and Spanned(v, s) == v.
        let a = strip_spanned(self);
        let b = strip_spanned(other);
        // If both point to the same non-Spanned inner, they're equal
        if std::ptr::eq(a, b) { return true; }
        match (a, b) {
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) => a == b,
            (MettaValueInner::Bool(a), MettaValueInner::Bool(b)) => a == b,
            (MettaValueInner::Long(a), MettaValueInner::Long(b)) => a == b,
            (MettaValueInner::Float(a), MettaValueInner::Float(b)) => a == b,
            (MettaValueInner::String(a), MettaValueInner::String(b)) => a == b,
            (MettaValueInner::SExpr(a), MettaValueInner::SExpr(b)) => a == b,
            (MettaValueInner::Unit, MettaValueInner::Unit) => true,
            (MettaValueInner::Error(ma, da), MettaValueInner::Error(mb, db)) => ma == mb && da == db,
            (MettaValueInner::Type(a), MettaValueInner::Type(b)) => a == b,
            (MettaValueInner::Conjunction(a), MettaValueInner::Conjunction(b)) => a == b,
            (MettaValueInner::Space(a), MettaValueInner::Space(b)) => a.id == b.id,
            (MettaValueInner::State(a), MettaValueInner::State(b)) => a == b,
            (MettaValueInner::Quoted(a), MettaValueInner::Quoted(b)) => a == b,
            (MettaValueInner::Memo(a), MettaValueInner::Memo(b)) => a.id == b.id,
            (MettaValueInner::Empty, MettaValueInner::Empty) => true,
            _ => false,
        }
    }
}

/// Strip all Spanned layers from a MettaValueInner reference.
/// Returns a reference to the innermost non-Spanned variant.
#[inline]
fn strip_spanned(inner: &MettaValueInner) -> &MettaValueInner {
    let mut current = inner;
    loop {
        match current {
            MettaValueInner::Spanned(v, _) => current = v.inner_ref(),
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
        (Some(x), _, Some(y), _) => x == y,             // Long, Long
        (_, Some(x), _, Some(y)) => x == y,             // Float, Float — exact
        (Some(x), _, _, Some(y)) => (x as f64) == y,    // Long → Float exact
        (_, Some(x), Some(y), _) => x == (y as f64),    // Float ↔ Long exact
        _ => a == b,                                     // Structural via PartialEq
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
    fn is_float(&self) -> bool { MettaValue::is_float(self) }

    #[inline]
    fn is_string(&self) -> bool { MettaValue::is_string(self) }

    #[inline]
    fn is_sexpr(&self) -> bool { MettaValue::is_sexpr(self) }

    #[inline]
    fn is_error(&self) -> bool { MettaValue::is_error(self) }

    #[inline]
    fn is_type(&self) -> bool { MettaValue::is_type(self) }

    #[inline]
    fn is_conjunction(&self) -> bool { MettaValue::is_conjunction(self) }

    #[inline]
    fn is_space(&self) -> bool { MettaValue::is_space(self) }

    #[inline]
    fn is_state(&self) -> bool { MettaValue::is_state(self) }

    #[inline]
    fn is_unit(&self) -> bool { MettaValue::is_unit(self) }

    #[inline]
    fn is_memo(&self) -> bool { MettaValue::is_memo(self) }

    #[inline]
    fn is_quoted(&self) -> bool { MettaValue::is_quoted(self) }

    #[inline]
    fn is_empty(&self) -> bool { MettaValue::is_empty(self) }

    #[inline]
    fn is_spanned(&self) -> bool { MettaValue::is_spanned(self) }

    #[inline]
    fn span(&self) -> Option<&'static crate::ir::Span> { MettaValue::span(self) }

    #[inline]
    fn strip_one_span(&self) -> Self {
        if self.is_inline() { return *self; }
        match self.inner_ref() {
            MettaValueInner::Spanned(v, _) => *v,
            _ => *self,
        }
    }

    #[inline]
    fn is_variable(&self) -> bool { MettaValue::is_variable(self) }

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
    fn as_atom(&self) -> Option<&'static str> { MettaValue::as_atom(self) }

    #[inline]
    fn as_bool(&self) -> Option<bool> { MettaValue::as_bool(self) }

    #[inline]
    fn as_long(&self) -> Option<i64> { MettaValue::as_long(self) }

    #[inline]
    fn as_float(&self) -> Option<f64> { MettaValue::as_float(self) }

    #[inline]
    fn as_string(&self) -> Option<&str> { MettaValue::as_string(self) }

    #[inline]
    fn as_sexpr(&self) -> Option<&[Self]> { MettaValue::as_sexpr(self) }

    #[inline]
    fn as_error(&self) -> Option<(&str, &Self)> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Error(msg, details) => Some((msg, details)),
            MettaValueInner::Spanned(v, _) => <MettaValue as MettaValueTrait>::as_error(v),
            _ => None,
        }
    }

    #[inline]
    fn as_type(&self) -> Option<&Self> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Type(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => <MettaValue as MettaValueTrait>::as_type(v),
            _ => None,
        }
    }

    #[inline]
    fn as_conjunction(&self) -> Option<&[Self]> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Conjunction(goals) => Some(goals),
            MettaValueInner::Spanned(v, _) => v.as_conjunction(),
            _ => None,
        }
    }

    #[inline]
    fn as_space(&self) -> Option<&SpaceHandle> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Space(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_space(),
            _ => None,
        }
    }

    #[inline]
    fn as_state(&self) -> Option<u64> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::State(id) => Some(*id),
            MettaValueInner::Spanned(v, _) => v.as_state(),
            _ => None,
        }
    }

    #[inline]
    fn as_memo(&self) -> Option<&MemoHandle> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Memo(handle) => Some(handle),
            MettaValueInner::Spanned(v, _) => v.as_memo(),
            _ => None,
        }
    }

    #[inline]
    fn as_quoted(&self) -> Option<Self> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(*inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted(),
            _ => None,
        }
    }

    #[inline]
    fn as_quoted_ref(&self) -> Option<&Self> {
        if self.is_inline() { return None; }
        match self.inner_ref() {
            MettaValueInner::Quoted(inner) => Some(inner),
            MettaValueInner::Spanned(v, _) => v.as_quoted_ref(),
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
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
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
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
            MettaValueInner::Spanned(v, _) => v.friendly_type_name(),
        }
    }

    fn get_head_symbol(&self) -> Option<&str> {
        if self.is_inline() { return None; }
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
        if self.is_inline() { return 0; }
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
                    let x = (n as u64).wrapping_add(LONG_SEED).wrapping_mul(GOLDEN_RATIO);
                    x ^ (x >> 32)
                }
                NB_TAG_EMPTY => 9u64.wrapping_mul(GOLDEN_RATIO),
                _ => UNIT_HASH,
            };
        }
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
                            NB_TAG_BOOL => if (val.tagged as u64 & 1) != 0 { "True" } else { "False" }.to_string(),
                            NB_TAG_UNIT => "()".to_string(),
                            NB_TAG_EMPTY => "Empty".to_string(),
                            _ => "()".to_string(),
                        });
                        continue;
                    }
                    match val.inner_ref() {
                    MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                    MettaValueInner::Float(f) => result_stack.push(f.to_string()),
                    MettaValueInner::Bool(b) => {
                        result_stack.push(if *b { "True" } else { "False" }.to_string());
                    }
                    MettaValueInner::String(s) => result_stack.push(format!("\"{}\"", s)),
                    MettaValueInner::Atom(a) => result_stack.push(a.to_string()),
                    MettaValueInner::Unit => result_stack.push("()".to_string()),
                    MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                    MettaValueInner::Space(handle) => {
                        result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::State(id) => {
                        result_stack.push(format!("(State {})", id));
                    }
                    MettaValueInner::Memo(handle) => {
                        result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::Error(msg, _) => {
                        result_stack.push(format!("(error \"{}\")", msg));
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
                    MettaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push("()".to_string());
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
                },
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
            }
        }

        result_stack.pop().unwrap_or_default()
    }

    fn to_display_string(&self) -> std::string::String {
        // Stack-based implementation to avoid recursion on deeply nested structures
        // Similar to friendly_repr but strings are printed WITHOUT quotes
        enum ReprWork<'a> {
            Process(&'a MettaValue),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
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
                            NB_TAG_BOOL => if (val.tagged as u64 & 1) != 0 { "True" } else { "False" }.to_string(),
                            NB_TAG_UNIT => "()".to_string(),
                            NB_TAG_EMPTY => "Empty".to_string(),
                            _ => "()".to_string(),
                        });
                        continue;
                    }
                    match val.inner_ref() {
                    MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                    MettaValueInner::Float(f) => result_stack.push(f.to_string()),
                    MettaValueInner::Bool(b) => {
                        result_stack.push(if *b { "True" } else { "False" }.to_string());
                    }
                    // Key difference: strings printed without quotes for display
                    MettaValueInner::String(s) => result_stack.push(s.to_string()),
                    MettaValueInner::Atom(a) => result_stack.push(a.to_string()),
                    MettaValueInner::Unit => result_stack.push("()".to_string()),
                    MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                    MettaValueInner::Space(handle) => {
                        result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::State(id) => {
                        result_stack.push(format!("(State {})", id));
                    }
                    MettaValueInner::Memo(handle) => {
                        result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::Error(msg, _) => {
                        result_stack.push(format!("(Error \"{}\")", msg));
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
                    MettaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push("()".to_string());
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
                },
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

/// Serialize an MettaValue to bytes
fn serialize_value(value: &MettaValue, buf: &mut Vec<u8>) {
    // Inline fast path: avoid slab deref for NaN-boxed types
    if value.is_inline() {
        match value.inline_tag() {
            NB_TAG_BOOL => {
                buf.push(BOOL);
                buf.push(if (value.tagged as u64 & 1) != 0 { 1 } else { 0 });
            }
            NB_TAG_LONG => {
                buf.push(LONG);
                buf.extend_from_slice(&value.inline_long_value().to_le_bytes());
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
        return;
    }
    match value.inner_ref() {
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
            for item in items.iter() {
                serialize_value(item, buf);
            }
        }
        MettaValueInner::Unit => {
            buf.push(UNIT_LEGACY);
        }
        MettaValueInner::Error(msg, details) => {
            buf.push(ERROR);
            write_varint(buf, msg.len());
            buf.extend_from_slice(msg.as_bytes());
            serialize_value(details, buf);
        }
        MettaValueInner::Type(inner) => {
            buf.push(TYPE);
            serialize_value(inner, buf);
        }
        MettaValueInner::Conjunction(goals) => {
            buf.push(CONJUNCTION);
            write_varint(buf, goals.len());
            for goal in goals.iter() {
                serialize_value(goal, buf);
            }
        }
        MettaValueInner::Empty => {
            buf.push(EMPTY);
        }
        MettaValueInner::Space(handle) => {
            buf.push(SPACE);
            buf.extend_from_slice(&handle.id.to_le_bytes());
            // Serialize name length and name bytes
            let name_bytes = handle.name.as_bytes();
            write_varint(buf, name_bytes.len());
            buf.extend_from_slice(name_bytes);
            // Serialize is_module_space flag
            buf.push(if handle.is_module_space() { 1 } else { 0 });
        }
        MettaValueInner::State(id) => {
            buf.push(STATE);
            buf.extend_from_slice(&id.to_le_bytes());
        }
        MettaValueInner::Quoted(inner) => {
            buf.push(QUOTED);
            serialize_value(inner, buf);
        }
        MettaValueInner::Memo(handle) => {
            buf.push(MEMO);
            buf.extend_from_slice(&handle.id.to_le_bytes());
        }
        MettaValueInner::Spanned(v, _) => serialize_value(v, buf),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::gc_allocator::global_factory;
    use super::super::metta_value_trait::MettaValueFactory;

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
        let items = vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ];
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
        let v = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
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
        let factory = global_factory();
        let details = factory.atom("details");
        let v = factory.error("test error", details);
        assert!(v.is_error());
        let (msg, det) = v.as_error().expect("should be error");
        assert_eq!(msg, "test error");
        assert_eq!(det.as_atom(), Some("details"));
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
        let goals = vec![
            factory.atom("goal1"),
            factory.atom("goal2"),
        ];
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
        let v1 = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
        ]);
        let v2 = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
        ]);
        let v3 = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(2),
        ]);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_error_error() {
        let factory = global_factory();
        let d1 = factory.atom("d");
        let d2 = factory.atom("d");
        let v1 = factory.error("err", d1);
        let v2 = factory.error("err", d2);
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
        let v1 = factory.conjunction(vec![
            factory.atom("a"),
        ]);
        let v2 = factory.conjunction(vec![
            factory.atom("a"),
        ]);
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
        let d = factory.unit();
        let v = factory.error("err", d);
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
        assert_eq!(format!("{}", v), "True");
    }

    #[test]
    fn test_display_bool_false() {
        let factory = global_factory();
        let v = factory.bool(false);
        assert_eq!(format!("{}", v), "False");
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
        let factory = global_factory();
        let d = factory.atom("details");
        let v = factory.error("msg", d);
        assert_eq!(format!("{}", v), "(Error msg details)");
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
        let v = factory.conjunction(vec![
            factory.atom("a"),
            factory.atom("b"),
        ]);
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
        let original = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nested_sexpr() {
        let factory = global_factory();
        let inner = factory.sexpr(vec![
            factory.atom("*"),
            factory.long(2),
            factory.long(3),
        ]);
        let original = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            inner,
        ]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_error() {
        let factory = global_factory();
        let details = factory.atom("details");
        let original = factory.error("test error", details);
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
        let original = factory.conjunction(vec![
            factory.atom("a"),
            factory.atom("b"),
        ]);
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
        let v = factory.sexpr(vec![
            factory.atom("foo"),
            factory.long(1),
        ]);
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_head_symbol_variable_head() {
        let factory = global_factory();
        let v = factory.sexpr(vec![
            factory.atom("$x"),
            factory.long(1),
        ]);
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
        let v = factory.sexpr(vec![
            factory.atom("foo"),
            factory.long(1),
            factory.long(2),
        ]);
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
        let v = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
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

        let items = vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ];
        let sexpr = factory.sexpr(items);
        assert!(sexpr.is_sexpr());
        assert_eq!(sexpr.as_sexpr().map(|s| s.len()), Some(3));
    }

    #[test]
    fn test_factory_sexpr_from_slice() {
        let factory = global_factory();

        let items = [
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ];
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
        assert!(numeric_equal(&a, &b), "Float(2.0) == Float(2.0) should be true");
        assert!(!numeric_equal(&a, &c), "Float(2.0) == Float(3.0) should be false");
    }

    #[test]
    fn test_numeric_equal_long_float() {
        // This is the key MeTTa HE fix: cross-type numeric equality
        let a = MettaValue::Long(2);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Float(2.5);
        assert!(numeric_equal(&a, &b), "Long(2) == Float(2.0) should be true (MeTTa HE cross-type fix)");
        assert!(!numeric_equal(&a, &c), "Long(2) == Float(2.5) should be false");
    }

    #[test]
    fn test_numeric_equal_float_long() {
        // Symmetric case: Float on left, Long on right
        let a = MettaValue::Float(2.0);
        let b = MettaValue::Long(2);
        let c = MettaValue::Long(3);
        assert!(numeric_equal(&a, &b), "Float(2.0) == Long(2) should be true (symmetric)");
        assert!(!numeric_equal(&MettaValue::Float(2.5), &MettaValue::Long(2)),
            "Float(2.5) == Long(2) should be false");
        assert!(!numeric_equal(&a, &c), "Float(2.0) == Long(3) should be false");
    }

    #[test]
    fn test_numeric_equal_non_numeric_structural() {
        // Non-numeric types fall through to structural PartialEq
        let foo1 = MettaValue::Atom("foo");
        let foo2 = MettaValue::Atom("foo");
        let bar = MettaValue::Atom("bar");
        let t1 = MettaValue::Bool(true);
        let t2 = MettaValue::Bool(true);

        assert!(numeric_equal(&foo1, &foo2), "Atom(\"foo\") == Atom(\"foo\") should be true (structural)");
        assert!(!numeric_equal(&foo1, &bar), "Atom(\"foo\") == Atom(\"bar\") should be false (structural)");
        assert!(numeric_equal(&t1, &t2), "Bool(true) == Bool(true) should be true (structural)");
    }

    #[test]
    fn test_numeric_equal_cross_type_non_numeric() {
        // Cross-type comparisons between non-numeric types and numeric types
        let foo = MettaValue::Atom("foo");
        let one_long = MettaValue::Long(1);
        let t = MettaValue::Bool(true);

        assert!(!numeric_equal(&foo, &one_long), "Atom(\"foo\") == Long(1) should be false");
        assert!(!numeric_equal(&t, &one_long), "Bool(true) == Long(1) should be false");
    }

    #[test]
    fn test_numeric_not_equal_basic() {
        let a = MettaValue::Long(2);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Long(3);

        assert!(!numeric_not_equal(&a, &b),
            "numeric_not_equal(Long(2), Float(2.0)) should be false (they are equal)");
        assert!(numeric_not_equal(&a, &c),
            "numeric_not_equal(Long(2), Long(3)) should be true (they are not equal)");
    }

    #[test]
    fn test_numeric_equal_ieee754_nan() {
        // IEEE 754: NaN != NaN
        let nan1 = MettaValue::Float(f64::NAN);
        let nan2 = MettaValue::Float(f64::NAN);
        assert!(!numeric_equal(&nan1, &nan2),
            "Float(NaN) == Float(NaN) should be false per IEEE 754");
    }

    #[test]
    fn test_numeric_equal_ieee754_zero() {
        // IEEE 754: +0.0 == -0.0
        let pos_zero = MettaValue::Float(0.0);
        let neg_zero = MettaValue::Float(-0.0);
        let long_zero = MettaValue::Long(0);

        assert!(numeric_equal(&pos_zero, &neg_zero),
            "Float(0.0) == Float(-0.0) should be true per IEEE 754");
        assert!(numeric_equal(&long_zero, &pos_zero),
            "Long(0) == Float(0.0) should be true");
    }

    #[test]
    fn test_numeric_equal_generic_works() {
        // Test the generic version with MettaValue (same trait bound)
        let a = MettaValue::Long(2);
        let b = MettaValue::Float(2.0);
        let c = MettaValue::Long(3);

        assert!(numeric_equal_generic(&a, &b),
            "numeric_equal_generic: Long(2) == Float(2.0) should be true");
        assert!(!numeric_equal_generic(&a, &c),
            "numeric_equal_generic: Long(2) == Long(3) should be false");
    }

    // ================================================================
    // Phase 6: Serialization & trait transparency tests
    // ================================================================

    #[test]
    fn test_spanned_equality_transparent() {
        use crate::ir::{Position, Span};
        use crate::backend::models::global_factory;

        let factory = global_factory();
        let span1 = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 2, byte_offset: 2 },
        };
        let span2 = Span {
            start: Position { row: 5, column: 3, byte_offset: 50 },
            end: Position { row: 5, column: 5, byte_offset: 52 },
        };

        let bare = factory.long(42);
        let spanned1 = factory.spanned(factory.long(42), span1);
        let spanned2 = factory.spanned(factory.long(42), span2);

        // Spanned(v, s) == v
        assert_eq!(bare, spanned1, "Spanned should equal bare value");
        assert_eq!(spanned1, bare, "bare value should equal Spanned");
        // Spanned(v, s1) == Spanned(v, s2) (different spans)
        assert_eq!(spanned1, spanned2, "different spans should not affect equality");
    }

    #[test]
    fn test_spanned_hash_transparent() {
        use crate::ir::{Position, Span};
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use crate::backend::models::global_factory;

        let factory = global_factory();
        let span = Span {
            start: Position { row: 1, column: 0, byte_offset: 10 },
            end: Position { row: 1, column: 5, byte_offset: 15 },
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

        assert_eq!(hash_bare, hash_spanned, "Spanned and bare should hash identically");
    }

    #[test]
    fn test_spanned_display_transparent() {
        use crate::ir::{Position, Span};
        use crate::backend::models::global_factory;

        let factory = global_factory();
        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 5, byte_offset: 5 },
        };

        let bare = factory.atom("hello");
        let spanned = factory.spanned(factory.atom("hello"), span);

        assert_eq!(format!("{}", bare), format!("{}", spanned),
            "Spanned should display identically to bare value");
        assert_eq!(format!("{}", spanned), "hello");
    }

    #[test]
    fn test_spanned_serialize_transparent() {
        use crate::ir::{Position, Span};
        use crate::backend::models::{global_factory, MettaValueTrait};

        let factory = global_factory();
        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 2, byte_offset: 2 },
        };

        let bare = factory.long(42);
        let spanned = factory.spanned(factory.long(42), span);

        let bare_bytes = bare.serialize();
        let spanned_bytes = spanned.serialize();

        assert_eq!(bare_bytes, spanned_bytes,
            "Spanned and bare should serialize identically (span stripped)");
    }

    #[test]
    fn test_spanned_sexpr_serialize_transparent() {
        use crate::ir::{Position, Span};
        use crate::backend::models::{global_factory, MettaValueTrait};

        let factory = global_factory();
        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 7, byte_offset: 7 },
        };

        let bare = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        let spanned = factory.spanned(factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]), span);

        let bare_bytes = bare.serialize();
        let spanned_bytes = spanned.serialize();

        assert_eq!(bare_bytes, spanned_bytes,
            "Spanned S-expression should serialize identically to bare");
    }
}
