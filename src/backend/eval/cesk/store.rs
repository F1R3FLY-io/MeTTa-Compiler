//! Parameterized Store Trait for the SECK Machine
//!
//! The Store component of the SECK machine abstracts over allocation strategies,
//! following Van Horn & Might's parameterized `alloc` function (CACM 2011, §2.4).
//!
//! The `alloc(value, hint)` function enables:
//! - **Context-sensitive allocation**: hints guide short-lived vs long-lived placement
//! - **Region-based allocation**: `let*` scopes can bulk-free on exit
//! - **Generational allocation**: nursery (thread-local bump) vs old-gen (global slab)
//! - **Abstract interpretation**: the AAM methodology swaps `alloc` to produce
//!   abstract addresses for static analysis
//!
//! ## Current Implementation
//!
//! `SlabStore` wraps the existing `GcFactory` / `SlabAllocator`, providing the
//! `Store` trait interface without changing allocation behavior. The `AllocHint`
//! is recorded but not yet acted upon — future phases (Phase 2: GC + Allocation)
//! will use hints to route allocations to different regions.

use std::fmt::Debug;

use crate::backend::models::{MettaValueFactory, MettaValueTrait};

// ============================================================================
// Allocation Hints
// ============================================================================

/// Hints to the Store about the expected lifetime and usage of an allocation.
///
/// These hints enable future optimizations without changing current behavior:
/// - Phase 2.1: Region-based allocation routes `LetScope` to bump regions
/// - Phase 2.2: Incremental GC uses `ShortLived` for nursery placement
/// - Phase 3.2: Per-thread regions use `ThreadLocal` for contention-free alloc
/// - Phase 4: AAM analysis overrides `alloc` entirely for abstract addresses
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AllocHint {
    /// Default allocation — no lifetime hint. Routes to the global slab allocator.
    Default,

    /// Short-lived value (e.g., intermediate results, temporary bindings).
    /// Future: nursery / thread-local bump allocation.
    ShortLived,

    /// Long-lived value (e.g., rules, facts, space atoms).
    /// Future: direct old-gen placement, skip nursery.
    LongLived,

    /// Value scoped to a `let*` binding block.
    /// Future: region-based allocation with bulk-free on scope exit.
    LetScope {
        /// Region ID for the enclosing `let*` scope.
        /// Values with the same region_id are freed together.
        region_id: u32,
    },

    /// Thread-local allocation for parallel branch evaluation.
    /// Future: per-thread bump allocator merged at join point (BEAM-inspired).
    ThreadLocal,

    /// Value is the result of hash-consing (structurally shared).
    /// Future: skip GC tracing for values known to be deduplicated.
    HashConsed,
}

impl Default for AllocHint {
    #[inline]
    fn default() -> Self {
        AllocHint::Default
    }
}

// ============================================================================
// Allocation Region
// ============================================================================

/// A handle to a region for bulk allocation and deallocation.
///
/// Regions enable `let*` scope optimization: all values allocated within a
/// region can be freed in O(1) when the scope exits, rather than waiting
/// for GC to discover they are unreachable.
///
/// Currently a placeholder — region-based allocation is Phase 2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllocRegion {
    /// Unique identifier for this region.
    pub id: u32,
}

// ============================================================================
// Store Trait
// ============================================================================

/// Parameterized allocation interface for the SECK machine.
///
/// The Store trait abstracts over the allocation strategy, enabling:
/// - Production use: `SlabStore` wrapping `GcFactory` (current behavior)
/// - Testing: mock stores for deterministic GC testing
/// - Analysis: abstract stores for AAM-based static analysis (Phase 4)
///
/// ## Lifetime Parameter
///
/// The Store produces values of type `V: MettaValueTrait`. In production,
/// `V = MettaValue` with `'static` lifetime (slab-allocated). Abstract
/// stores may produce `AbstractValue` with abstract addresses.
///
/// ## Thread Safety
///
/// Implementations must be `Send + Sync` to support parallel evaluation.
/// The production `SlabStore` delegates to the lock-free `SlabAllocator`.
pub trait Store<V: MettaValueTrait>: Debug + Send + Sync {
    /// The factory type used to construct values.
    type Factory: MettaValueFactory<V> + Copy + Clone;

    /// Get a reference to the value factory.
    fn factory(&self) -> &Self::Factory;

    /// Allocate a value with the given lifetime hint.
    ///
    /// The hint guides placement strategy but does not affect semantics.
    /// All implementations must return a valid value regardless of hint.
    ///
    /// # Current Behavior
    ///
    /// `SlabStore` ignores the hint and delegates to `GcFactory`. Future
    /// phases will route based on hints (region-based, nursery, etc.).
    #[inline]
    fn alloc_atom(&self, s: &str, _hint: AllocHint) -> V {
        self.factory().atom(s)
    }

    /// Allocate a long integer value.
    #[inline]
    fn alloc_long(&self, n: i64, _hint: AllocHint) -> V {
        self.factory().long(n)
    }

    /// Allocate a boolean value.
    #[inline]
    fn alloc_bool(&self, b: bool, _hint: AllocHint) -> V {
        self.factory().bool(b)
    }

    /// Allocate a float value.
    #[inline]
    fn alloc_float(&self, f: f64, _hint: AllocHint) -> V {
        self.factory().float(f)
    }

    /// Allocate a string value.
    #[inline]
    fn alloc_string(&self, s: &str, _hint: AllocHint) -> V {
        self.factory().string(s)
    }

    /// Allocate an S-expression value.
    #[inline]
    fn alloc_sexpr(&self, items: Vec<V>, _hint: AllocHint) -> V {
        self.factory().sexpr(items)
    }

    /// Allocate an error value.
    #[inline]
    fn alloc_error(&self, msg: &str, details: V, _hint: AllocHint) -> V {
        self.factory().error(msg, details)
    }

    /// Allocate a unit value.
    #[inline]
    fn alloc_unit(&self, _hint: AllocHint) -> V {
        self.factory().unit()
    }

    /// Allocate an empty value.
    #[inline]
    fn alloc_empty(&self, _hint: AllocHint) -> V {
        self.factory().empty()
    }

    /// Enter a new allocation region (for `let*` scope optimization).
    ///
    /// Returns a region handle. All allocations with `AllocHint::LetScope { region_id }`
    /// matching this region will be bulk-freed when `exit_region()` is called.
    ///
    /// # Current Behavior
    ///
    /// Returns a placeholder region. No-op until Phase 2.1.
    fn enter_region(&self) -> AllocRegion {
        // Placeholder — region IDs are not yet meaningful.
        // Phase 2.1 will implement bump-region allocation.
        AllocRegion { id: 0 }
    }

    /// Exit an allocation region, bulk-freeing all values allocated within it.
    ///
    /// # Current Behavior
    ///
    /// No-op until Phase 2.1.
    fn exit_region(&self, _region: AllocRegion) {
        // Placeholder — Phase 2.1 will implement region deallocation.
    }

    /// Report the approximate number of live bytes managed by this store.
    ///
    /// Used for GC threshold decisions and backpressure.
    fn live_bytes(&self) -> usize {
        0
    }

    /// Report the total number of allocations performed since store creation.
    fn alloc_count(&self) -> u64 {
        0
    }
}

// ============================================================================
// SlabStore — Production Store wrapping GcFactory
// ============================================================================

use crate::backend::models::{GcFactory, MettaValue, global_factory, alloc_count_snapshot, committed_bytes_snapshot};

/// Production Store implementation wrapping the global `GcFactory`.
///
/// This is the concrete Store used by the SECK machine in production.
/// It delegates all allocations to the lock-free `SlabAllocator` via `GcFactory`.
///
/// ## AllocHint Behavior
///
/// Currently all hints are ignored — all allocations go to the global slab.
/// Future phases will route hints to different allocation strategies:
/// - `LetScope`: bump-region allocation (Phase 2.1)
/// - `ThreadLocal`: per-thread nursery (Phase 3.2)
/// - `ShortLived`: nursery with promotion (Phase 2.2)
/// - `LongLived`: direct old-gen placement (Phase 2.2)
#[derive(Debug, Clone, Copy)]
pub struct SlabStore {
    factory: GcFactory,
}

impl SlabStore {
    /// Create a new `SlabStore` backed by the global slab allocator.
    #[inline]
    pub fn new() -> Self {
        Self {
            factory: global_factory(),
        }
    }

    /// Get the inner `GcFactory`.
    #[inline]
    pub fn gc_factory(&self) -> GcFactory {
        self.factory
    }
}

impl Default for SlabStore {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Store<MettaValue> for SlabStore {
    type Factory = GcFactory;

    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }

    #[inline]
    fn live_bytes(&self) -> usize {
        committed_bytes_snapshot()
    }

    #[inline]
    fn alloc_count(&self) -> u64 {
        alloc_count_snapshot()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValueTrait;

    #[test]
    fn test_slab_store_basic_alloc() {
        let store = SlabStore::new();
        let v = store.alloc_atom("hello", AllocHint::Default);
        assert!(v.is_atom());
        assert_eq!(MettaValueTrait::as_atom(&v), Some("hello"));
    }

    #[test]
    fn test_slab_store_alloc_hints_accepted() {
        let store = SlabStore::new();

        // All hints should produce valid values (hints are advisory only)
        let _ = store.alloc_long(42, AllocHint::ShortLived);
        let _ = store.alloc_bool(true, AllocHint::LongLived);
        let _ = store.alloc_float(3.14, AllocHint::LetScope { region_id: 1 });
        let _ = store.alloc_string("test", AllocHint::ThreadLocal);
        let _ = store.alloc_unit(AllocHint::HashConsed);
        let _ = store.alloc_empty(AllocHint::Default);
    }

    #[test]
    fn test_slab_store_region_noop() {
        let store = SlabStore::new();
        let region = store.enter_region();
        assert_eq!(region.id, 0);
        store.exit_region(region); // Should not panic
    }

    #[test]
    fn test_slab_store_sexpr() {
        let store = SlabStore::new();
        let items = vec![
            store.alloc_atom("+", AllocHint::Default),
            store.alloc_long(1, AllocHint::ShortLived),
            store.alloc_long(2, AllocHint::ShortLived),
        ];
        let sexpr = store.alloc_sexpr(items, AllocHint::Default);
        assert!(sexpr.is_sexpr());
        assert_eq!(sexpr.as_sexpr().expect("is sexpr").len(), 3);
    }

    #[test]
    fn test_alloc_hint_default() {
        assert_eq!(AllocHint::default(), AllocHint::Default);
    }

    #[test]
    fn test_slab_store_live_bytes() {
        let store = SlabStore::new();
        // Just verify it doesn't panic — actual value depends on global state
        let _ = store.live_bytes();
    }

    #[test]
    fn test_slab_store_size() {
        // SlabStore should be pointer-sized (holds one GcFactory)
        assert_eq!(
            std::mem::size_of::<SlabStore>(),
            std::mem::size_of::<GcFactory>()
        );
    }
}
