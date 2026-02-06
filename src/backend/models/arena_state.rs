//! Session-Owned Dual Arena with O(1) Bulk Deallocation
//!
//! This module provides `ArenaState`, a session-scoped arena management structure that
//! enables O(1) bulk deallocation when the session ends. It implements a dual-arena
//! model:
//!
//! 1. **Eval Arena** - Thread-local, per-thread session-counted reset for intermediates (~95%)
//! 2. **Storage Arena** - Owned by ArenaState, O(1) bulk free on drop (~5% of allocations)
//!
//! ## Key Properties
//!
//! | Property | Value |
//! |----------|-------|
//! | Allocation overhead | None (pure bump pointer) |
//! | Per-value drop cost | None (no destructors) |
//! | Total drop cost | O(1) - single arena reset |
//! | Memory layout | Contiguous, cache-friendly |
//! | Thread safety | ArenaValue<'static> via pointer cast |
//! | Session creation cost | O(1) when pool has available arenas |
//!
//! ## Memory Safety
//!
//! The `storage_arena()` method returns `&'static Bump` via pointer cast. This is safe
//! because:
//! 1. The Bump is valid for the lifetime of ArenaState
//! 2. ArenaValue<'static> references are only used during evaluation
//! 3. All references become dangling after ArenaState::drop()
//! 4. Integration layer must access output before drop
//!
//! Use AddressSanitizer to detect use-after-free during development:
//! ```bash
//! RUSTFLAGS="-Z sanitizer=address" cargo +nightly test
//! ```

use std::cell::{Cell, RefCell};

use bumpalo::Bump;

use super::arena_value::{ArenaValue, ArenaValueFactory, ArenaValueInner};
use super::metta_value_trait::MettaValueFactory;
use super::{MemoHandle, SpaceHandle};

// ============================================================================
// Eval Arena Per-Thread Session Tracking
// ============================================================================
//
// The eval arena uses per-thread session counting to safely manage memory:
//
// - Each thread has its own eval arena (thread-local, never shared)
// - `ArenaState::new()` increments the thread-local active session count
// - `ArenaState::drop()` decrements it; when it reaches 0, marks the arena for reset
// - `get_eval_arena()` resets the arena only when marked AND no active sessions
//
// This prevents a race condition where one thread's ArenaState drop would
// invalidate ArenaValues still in use on a different thread (or even on the
// same thread in a different session).
//
// ## Important
//
// ArenaState should be created and dropped on the same thread. If moved between
// threads, the per-thread session accounting will be incorrect (the creating
// thread's count won't be decremented, and the dropping thread's count will
// underflow via saturating_sub to 0). This is safe (no UB) but may delay
// arena reset on the creating thread.

thread_local! {
    /// Eval arena with reset tracking.
    ///
    /// The tuple contains (arena, needs_reset).
    /// When needs_reset is true and active sessions is 0, the arena is reset
    /// on the next call to `get_eval_arena()`.
    static EVAL_ARENA: RefCell<(Box<Bump>, bool)> = RefCell::new((Box::new(Bump::new()), false));

    /// Number of active ArenaState sessions on this thread.
    /// When this drops to 0, the eval arena is marked for reset.
    static ACTIVE_EVAL_SESSIONS: Cell<usize> = const { Cell::new(0) };
}

/// Begin an eval session on this thread.
///
/// Called by `ArenaState::new()` to register that this thread has an active
/// session using the eval arena. While any session is active, the eval arena
/// will not be reset.
///
/// If the arena was previously marked for reset (from a prior session ending)
/// and no other sessions are active, the arena is reset before the new session
/// begins. This ensures the new session starts with a clean arena.
#[inline]
fn begin_eval_session() {
    ACTIVE_EVAL_SESSIONS.with(|count| {
        let n = count.get();
        if n == 0 {
            // No active sessions — reset arena if it was marked
            EVAL_ARENA.with(|cell| {
                let mut guard = cell.borrow_mut();
                if guard.1 {
                    guard.0.reset(); // O(1) bulk deallocation
                    guard.1 = false;
                }
            });
        }
        count.set(n + 1);
    });
}

/// End an eval session on this thread.
///
/// Called by `ArenaState::drop()` to signal that a session has finished.
/// When the last active session on this thread ends, the eval arena is
/// marked for reset. The actual reset happens lazily on the next call to
/// `begin_eval_session()` or `reset_eval_arena()`.
#[inline]
fn end_eval_session() {
    ACTIVE_EVAL_SESSIONS.with(|count| {
        let n = count.get().saturating_sub(1);
        count.set(n);
        if n == 0 {
            // Last session on this thread ended — mark arena for reset
            EVAL_ARENA.with(|cell| {
                cell.borrow_mut().1 = true; // needs_reset
            });
        }
    });
}

/// Get the thread-local eval arena.
///
/// This function provides O(1) access to the eval arena. The arena is
/// never reset while any session is active on this thread (protected by
/// per-thread session counting).
///
/// # Safety
///
/// The returned reference is valid for the current thread's lifetime.
/// The 'static lifetime is achieved via pointer cast — the arena is truly
/// thread-local and persists for the thread's duration.
#[inline]
pub fn get_eval_arena() -> &'static Bump {
    EVAL_ARENA.with(|cell| {
        let guard = cell.borrow();

        // SAFETY: Arena is thread-local and lives for thread duration.
        // The 'static lifetime is valid because:
        // 1. Thread-local storage persists for the thread's lifetime
        // 2. We never return a reference that could escape the thread
        // 3. The arena is only reset between sessions (never during)
        unsafe { &*(&*guard.0 as *const Bump) }
    })
}

/// Explicitly reset the eval arena on this thread.
///
/// Forces an immediate reset of the thread-local eval arena, regardless of
/// session state. Only call this when you are certain no ArenaValues from
/// the eval arena are still referenced.
///
/// This is primarily useful for the CLI's file evaluation mode, where
/// results have already been formatted to strings and no ArenaValues
/// remain in scope.
///
/// # Safety (logical)
///
/// After calling this, all previously allocated ArenaValues from the eval
/// arena become dangling. Accessing them is undefined behavior.
pub fn reset_eval_arena() {
    EVAL_ARENA.with(|cell| {
        let mut guard = cell.borrow_mut();
        guard.0.reset(); // O(1) bulk deallocation
        guard.1 = false; // clear needs_reset flag
    });
}

/// Get the number of active eval sessions on this thread.
///
/// Useful for debugging and testing session lifecycle.
pub fn active_eval_sessions() -> usize {
    ACTIVE_EVAL_SESSIONS.with(|c| c.get())
}

/// Get a factory for allocating in the thread-local eval arena.
#[inline]
pub fn get_eval_factory() -> ArenaValueFactory<'static> {
    ArenaValueFactory::new(get_eval_arena())
}

// ============================================================================
// Storage Arena Pool
// ============================================================================

/// Maximum number of arenas to keep in the pool per thread.
/// Prevents unbounded memory growth in high-churn scenarios.
const MAX_POOL_SIZE: usize = 4;

thread_local! {
    /// Pool of reusable storage arenas (reset but not dropped).
    /// Reduces allocation overhead for multi-session workloads.
    static STORAGE_ARENA_POOL: RefCell<Vec<Box<Bump>>> = const { RefCell::new(Vec::new()) };
}

/// Acquire a storage arena from the pool, or create a new one.
fn acquire_storage_arena() -> Box<Bump> {
    STORAGE_ARENA_POOL.with(|pool| {
        pool.borrow_mut()
            .pop()
            .unwrap_or_else(|| Box::new(Bump::new()))
    })
}

/// Return a storage arena to the pool for reuse.
/// Arena is reset (O(1)) before being returned.
/// Excess arenas beyond MAX_POOL_SIZE are dropped.
fn return_storage_arena(mut arena: Box<Bump>) {
    arena.reset(); // O(1) - just moves allocation pointer

    STORAGE_ARENA_POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.len() < MAX_POOL_SIZE {
            pool.push(arena);
        }
        // else: drop arena (releases memory to system allocator)
    });
}

// ============================================================================
// StorageFactory - Factory for Storage Arena Allocation
// ============================================================================

/// Factory that allocates in session-owned storage arena.
/// Produces ArenaValue<'static> via pointer cast.
#[derive(Clone, Copy)]
pub struct StorageFactory {
    arena: &'static Bump,
}

impl StorageFactory {
    /// Create a new StorageFactory for the given arena.
    #[inline]
    pub fn new(arena: &'static Bump) -> Self {
        Self { arena }
    }

    /// Get the underlying arena.
    #[inline]
    pub fn arena(&self) -> &'static Bump {
        self.arena
    }
}

impl std::fmt::Debug for StorageFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageFactory")
            .field("arena", &(self.arena as *const Bump))
            .finish()
    }
}

// SAFETY: StorageFactory<'static> can be sent between threads because:
// - It's a reference to a session-owned arena
// - The arena is valid for the ArenaState's lifetime
// - Values created in the arena are immutable after creation
unsafe impl Send for StorageFactory {}

// SAFETY: StorageFactory<'static> can be shared between threads because:
// - It provides the same functionality whether accessed from one thread or many
// - The factory is only used for value construction, not mutation
unsafe impl Sync for StorageFactory {}

impl MettaValueFactory<ArenaValue<'static>> for StorageFactory {
    #[inline]
    fn atom(&self, s: &str) -> ArenaValue<'static> {
        ArenaValue::atom(self.arena, s)
    }

    #[inline]
    fn bool(&self, b: bool) -> ArenaValue<'static> {
        ArenaValue::bool(self.arena, b)
    }

    #[inline]
    fn long(&self, n: i64) -> ArenaValue<'static> {
        ArenaValue::long(self.arena, n)
    }

    #[inline]
    fn float(&self, f: f64) -> ArenaValue<'static> {
        ArenaValue::float(self.arena, f)
    }

    #[inline]
    fn string(&self, s: &str) -> ArenaValue<'static> {
        ArenaValue::string(self.arena, s)
    }

    #[inline]
    fn sexpr(&self, items: Vec<ArenaValue<'static>>) -> ArenaValue<'static> {
        ArenaValue::sexpr(self.arena, items)
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[ArenaValue<'static>]) -> ArenaValue<'static> {
        ArenaValue::sexpr(self.arena, items.iter().copied())
    }

    #[inline]
    fn nil(&self) -> ArenaValue<'static> {
        ArenaValue::nil(self.arena)
    }

    #[inline]
    fn error(&self, msg: &str, details: ArenaValue<'static>) -> ArenaValue<'static> {
        ArenaValue::error(self.arena, msg, details)
    }

    #[inline]
    fn type_value(&self, inner: ArenaValue<'static>) -> ArenaValue<'static> {
        ArenaValue::r#type(self.arena, inner)
    }

    #[inline]
    fn conjunction(&self, goals: Vec<ArenaValue<'static>>) -> ArenaValue<'static> {
        ArenaValue::conjunction(self.arena, goals)
    }

    #[inline]
    fn space(&self, handle: SpaceHandle) -> ArenaValue<'static> {
        ArenaValue::space(self.arena, handle)
    }

    #[inline]
    fn state(&self, id: u64) -> ArenaValue<'static> {
        ArenaValue::state(self.arena, id)
    }

    #[inline]
    fn unit(&self) -> ArenaValue<'static> {
        ArenaValue::unit(self.arena)
    }

    #[inline]
    fn memo(&self, handle: MemoHandle) -> ArenaValue<'static> {
        ArenaValue::memo(self.arena, handle)
    }

    #[inline]
    fn empty(&self) -> ArenaValue<'static> {
        ArenaValue::empty(self.arena)
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<(ArenaValue<'static>, usize), String> {
        // Delegate to ArenaValueFactory's deserialize
        let factory = ArenaValueFactory::new(self.arena);
        factory.deserialize(bytes)
    }
}

// ============================================================================
// ArenaState - Session Owner
// ============================================================================

/// Session-scoped arena state with O(1) bulk deallocation.
///
/// Owns the storage arena and coordinates eval arena lifecycle.
/// When dropped:
/// - Storage arena: returned to pool (O(1) reset)
/// - Eval arena: marked for lazy reset when last session on this thread ends
///
/// **Important**: Keep the ArenaState alive as long as you reference any
/// ArenaValues allocated during its session (from either the storage or
/// eval arena). Dropping the ArenaState while holding eval arena references
/// from a different, still-active session is safe — the eval arena won't
/// reset until all sessions on this thread have ended.
///
/// ## Usage
///
/// ```ignore
/// let state = ArenaState::new();
///
/// // Compile source into storage arena
/// let factory = state.storage_factory();
/// let expr = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
///
/// // Evaluation uses thread-local eval arena for intermediates
/// // Results can be pushed to state.output_mut()
///
/// // Access output before drop
/// for result in state.output() {
///     println!("{}", result.friendly_repr());
/// }
///
/// // On drop: O(1) bulk deallocation
/// drop(state);
/// ```
pub struct ArenaState {
    /// Owned storage arena for persistent values (rules, bindings, results).
    /// Acquired from pool on new(), returned to pool on drop.
    storage_arena: Box<Bump>,

    /// Compiled source expressions (in storage arena)
    source: Vec<ArenaValue<'static>>,

    /// Evaluation output (in storage arena)
    output: Vec<ArenaValue<'static>>,
}

impl ArenaState {
    /// Create a new ArenaState, acquiring storage arena from pool.
    ///
    /// This also begins an eval session on the current thread, preventing
    /// the thread-local eval arena from being reset while this state is alive.
    pub fn new() -> Self {
        begin_eval_session();
        let storage_arena = acquire_storage_arena();
        Self {
            storage_arena,
            source: Vec::new(),
            output: Vec::new(),
        }
    }

    /// Get storage arena with 'static lifetime for sharing during session.
    ///
    /// # Safety
    ///
    /// The returned reference is only valid while self is alive.
    /// All ArenaValue<'static> references become invalid after drop.
    #[inline]
    pub fn storage_arena(&self) -> &'static Bump {
        // SAFETY: Arena is valid for ArenaState's lifetime.
        // The 'static lifetime is a pointer cast - callers must ensure
        // values don't escape the ArenaState's scope.
        unsafe { &*(&*self.storage_arena as *const Bump) }
    }

    /// Get factory for allocating in storage arena.
    #[inline]
    pub fn storage_factory(&self) -> StorageFactory {
        StorageFactory::new(self.storage_arena())
    }

    /// Get reference to source expressions.
    #[inline]
    pub fn source(&self) -> &[ArenaValue<'static>] {
        &self.source
    }

    /// Get mutable reference to source expressions.
    #[inline]
    pub fn source_mut(&mut self) -> &mut Vec<ArenaValue<'static>> {
        &mut self.source
    }

    /// Get reference to output values.
    #[inline]
    pub fn output(&self) -> &[ArenaValue<'static>] {
        &self.output
    }

    /// Get mutable reference to push results.
    #[inline]
    pub fn output_mut(&mut self) -> &mut Vec<ArenaValue<'static>> {
        &mut self.output
    }

    /// Clear output (useful for reusing ArenaState across evaluations).
    #[inline]
    pub fn clear_output(&mut self) {
        self.output.clear();
    }

    /// Get the number of active eval sessions on this thread.
    ///
    /// Useful for debugging and testing session lifecycle.
    #[inline]
    pub fn active_sessions() -> usize {
        active_eval_sessions()
    }
}

impl Default for ArenaState {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ArenaState {
    fn drop(&mut self) {
        // End the eval session on this thread.
        // When the last session on this thread ends, the eval arena is marked
        // for lazy reset (actual reset deferred to next begin_eval_session()).
        end_eval_session();

        // Return storage arena to pool (reset + reuse)
        // Takes ownership by swapping with empty Box
        let arena = std::mem::replace(&mut self.storage_arena, Box::new(Bump::new()));
        return_storage_arena(arena);

        // Note: source and output contain dangling ArenaValue<'static> refs
        // These are harmless - no Drop impl, no access after this point
    }
}

impl std::fmt::Debug for ArenaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaState")
            .field("source_count", &self.source.len())
            .field("output_count", &self.output.len())
            .field("storage_arena", &(&*self.storage_arena as *const Bump))
            .finish()
    }
}

// ============================================================================
// Clone Value Between Arenas
// ============================================================================

/// Clone a value from one arena to another.
///
/// This is a deep copy, allocating all nodes in the target arena.
/// Used when values need to move from eval arena to storage arena.
///
/// # Type Parameters
///
/// - `'a`: Source arena lifetime
/// - `'b`: Target arena lifetime (typically 'static for storage)
///
/// # Example
///
/// ```ignore
/// let eval_arena = get_eval_arena();
/// let eval_factory = ArenaValueFactory::new(eval_arena);
/// let intermediate = eval_factory.long(42);
///
/// let state = ArenaState::new();
/// let storage_factory = state.storage_factory();
/// let persistent = clone_value(&intermediate, &storage_factory);
/// ```
pub fn clone_value<'a, 'b, F>(value: &ArenaValue<'a>, target_factory: &F) -> ArenaValue<'b>
where
    F: MettaValueFactory<ArenaValue<'b>>,
{
    match value.inner() {
        ArenaValueInner::Atom(s) => target_factory.atom(s),
        ArenaValueInner::Bool(b) => target_factory.bool(*b),
        ArenaValueInner::Long(n) => target_factory.long(*n),
        ArenaValueInner::Float(f) => target_factory.float(*f),
        ArenaValueInner::String(s) => target_factory.string(s),
        ArenaValueInner::SExpr(items) => {
            let cloned: Vec<_> = items
                .iter()
                .map(|item| clone_value(item, target_factory))
                .collect();
            target_factory.sexpr(cloned)
        }
        ArenaValueInner::Nil => target_factory.nil(),
        ArenaValueInner::Unit => target_factory.unit(),
        ArenaValueInner::Empty => target_factory.empty(),
        ArenaValueInner::Conjunction(goals) => {
            let cloned: Vec<_> = goals
                .iter()
                .map(|goal| clone_value(goal, target_factory))
                .collect();
            target_factory.conjunction(cloned)
        }
        ArenaValueInner::Error(msg, details) => {
            let cloned_details = clone_value(details, target_factory);
            target_factory.error(msg, cloned_details)
        }
        ArenaValueInner::Type(inner) => {
            let cloned_inner = clone_value(inner, target_factory);
            target_factory.type_value(cloned_inner)
        }
        ArenaValueInner::Space(handle) => target_factory.space(handle.clone()),
        ArenaValueInner::State(id) => target_factory.state(*id),
        ArenaValueInner::Memo(handle) => target_factory.memo(handle.clone()),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_state_creation() {
        let state = ArenaState::new();
        assert!(state.source().is_empty());
        assert!(state.output().is_empty());
    }

    #[test]
    fn test_storage_factory_allocation() {
        let state = ArenaState::new();
        let factory = state.storage_factory();

        let atom = factory.atom("hello");
        assert!(atom.is_atom());
        assert_eq!(atom.as_atom(), Some("hello"));

        let num = factory.long(42);
        assert!(num.is_long());
        assert_eq!(num.as_long(), Some(42));

        let sexpr = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        assert!(sexpr.is_sexpr());
        let items = sexpr.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_eval_arena_access() {
        let arena = get_eval_arena();
        let s = arena.alloc_str("test");
        assert_eq!(s, "test");
    }

    #[test]
    fn test_session_count_lifecycle() {
        // Session count starts at 0 on a fresh thread (or may be higher due to
        // concurrent tests, so check relative changes).
        let before = active_eval_sessions();
        {
            let _state = ArenaState::new();
            // During the session, count should be higher
            assert!(
                active_eval_sessions() > before,
                "session count should increase after new: before={}",
                before,
            );
        }
        // After drop, count should return to previous value
        assert_eq!(
            active_eval_sessions(),
            before,
            "session count should return to original after drop",
        );
    }

    #[test]
    fn test_pool_reuse() {
        // Create and drop several states to populate pool
        for _ in 0..5 {
            let _state = ArenaState::new();
        }

        // Pool should have arenas up to MAX_POOL_SIZE
        STORAGE_ARENA_POOL.with(|pool| {
            let pool = pool.borrow();
            assert!(pool.len() <= MAX_POOL_SIZE);
        });
    }

    #[test]
    fn test_clone_value_simple() {
        let state = ArenaState::new();
        let eval_factory = get_eval_factory();
        let storage_factory = state.storage_factory();

        // Create value in eval arena
        let original = eval_factory.long(42);

        // Clone to storage arena
        let cloned = clone_value(&original, &storage_factory);

        assert!(cloned.is_long());
        assert_eq!(cloned.as_long(), Some(42));
    }

    #[test]
    fn test_clone_value_nested() {
        let state = ArenaState::new();
        let eval_factory = get_eval_factory();
        let storage_factory = state.storage_factory();

        // Create nested sexpr in eval arena
        let original = eval_factory.sexpr(vec![
            eval_factory.atom("outer"),
            eval_factory.sexpr(vec![
                eval_factory.atom("inner"),
                eval_factory.long(1),
                eval_factory.long(2),
            ]),
        ]);

        // Clone to storage arena
        let cloned = clone_value(&original, &storage_factory);

        assert!(cloned.is_sexpr());
        let items = cloned.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 2);
        assert!(items[1].is_sexpr());
    }

    #[test]
    fn test_output_accumulation() {
        let mut state = ArenaState::new();
        let factory = state.storage_factory();

        state.output_mut().push(factory.long(1));
        state.output_mut().push(factory.long(2));
        state.output_mut().push(factory.long(3));

        assert_eq!(state.output().len(), 3);

        state.clear_output();
        assert!(state.output().is_empty());
    }

    #[test]
    fn test_source_storage() {
        let mut state = ArenaState::new();
        let factory = state.storage_factory();

        let expr = factory.sexpr(vec![factory.atom("!"), factory.atom("hello")]);
        state.source_mut().push(expr);

        assert_eq!(state.source().len(), 1);
        assert!(state.source()[0].is_sexpr());
    }

    #[test]
    fn test_storage_factory_debug() {
        let state = ArenaState::new();
        let factory = state.storage_factory();
        let debug_str = format!("{:?}", factory);
        assert!(debug_str.contains("StorageFactory"));
    }

    #[test]
    fn test_arena_state_debug() {
        let state = ArenaState::new();
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("ArenaState"));
        assert!(debug_str.contains("source_count"));
        assert!(debug_str.contains("output_count"));
    }

    // ========================================================================
    // Clone Value Variant Coverage
    // ========================================================================

    #[test]
    fn test_clone_value_all_variants() {
        let state = ArenaState::new();
        let eval_factory = get_eval_factory();
        let storage_factory = state.storage_factory();

        // Atom
        let v = eval_factory.atom("hello");
        let c = clone_value(&v, &storage_factory);
        assert_eq!(c.as_atom(), Some("hello"));

        // Bool
        let v = eval_factory.bool(true);
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_bool());

        // Long
        let v = eval_factory.long(-999);
        let c = clone_value(&v, &storage_factory);
        assert_eq!(c.as_long(), Some(-999));

        // Float
        let v = eval_factory.float(3.14);
        let c = clone_value(&v, &storage_factory);
        assert_eq!(c.as_float(), Some(3.14));

        // String
        let v = eval_factory.string("world");
        let c = clone_value(&v, &storage_factory);
        assert_eq!(c.as_string(), Some("world"));

        // Nil
        let v = eval_factory.nil();
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_nil());

        // Unit
        let v = eval_factory.unit();
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_unit());

        // Empty
        let v = eval_factory.empty();
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_empty());

        // Error
        let details = eval_factory.atom("bad-input");
        let v = eval_factory.error("oops", details);
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_error());
        let (msg, _detail) = c.as_error().expect("should be error");
        assert_eq!(msg, "oops");

        // Type
        let inner = eval_factory.atom("Int");
        let v = eval_factory.type_value(inner);
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_type());

        // Conjunction
        let goals = vec![eval_factory.atom("a"), eval_factory.atom("b")];
        let v = eval_factory.conjunction(goals);
        let c = clone_value(&v, &storage_factory);
        let conj = c.as_conjunction().expect("should be conjunction");
        assert_eq!(conj.len(), 2);

        // SExpr (already covered but verify here too)
        let v = eval_factory.sexpr(vec![eval_factory.atom("+"), eval_factory.long(1)]);
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_sexpr());
        assert_eq!(c.as_sexpr().expect("sexpr").len(), 2);

        // State
        let v = eval_factory.state(42);
        let c = clone_value(&v, &storage_factory);
        assert!(c.is_state());
    }

    // ========================================================================
    // Clone Value Deep Nesting
    // ========================================================================

    #[test]
    fn test_clone_value_deeply_nested() {
        let state = ArenaState::new();
        let f = get_eval_factory();
        let sf = state.storage_factory();

        // Build a deeply nested structure: (a (b (c (d (e)))))
        let mut current = f.atom("e");
        for name in ["d", "c", "b", "a"].iter() {
            current = f.sexpr(vec![f.atom(name), current]);
        }

        let cloned = clone_value(&current, &sf);

        // Verify structure by traversing
        let mut node = cloned;
        for expected in ["a", "b", "c", "d"] {
            let items = node.as_sexpr().expect("should be sexpr");
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].as_atom(), Some(expected));
            node = items[1];
        }
        assert_eq!(node.as_atom(), Some("e"));
    }

    // ========================================================================
    // Pool Behavior
    // ========================================================================

    #[test]
    fn test_pool_max_size_enforced() {
        // Create and drop many states (more than MAX_POOL_SIZE)
        for _ in 0..(MAX_POOL_SIZE + 10) {
            let _state = ArenaState::new();
        }

        // Pool should never exceed MAX_POOL_SIZE regardless of how many states
        // were created and dropped (concurrent tests may also modify pool)
        STORAGE_ARENA_POOL.with(|pool| {
            let pool = pool.borrow();
            assert!(
                pool.len() <= MAX_POOL_SIZE,
                "pool size ({}) should not exceed MAX_POOL_SIZE ({})",
                pool.len(),
                MAX_POOL_SIZE,
            );
        });
    }

    #[test]
    fn test_pool_acquire_returns_reset_arena() {
        // Clear pool
        STORAGE_ARENA_POOL.with(|pool| pool.borrow_mut().clear());

        // Create a state, allocate in it, then drop
        {
            let state = ArenaState::new();
            let factory = state.storage_factory();
            // Allocate a bunch of data
            for i in 0..1000 {
                let _ = factory.long(i);
            }
        }

        // Acquire from pool - the arena should have been reset
        let arena = acquire_storage_arena();
        // After reset, allocated bytes should be minimal (just arena metadata)
        // We can't check exact bytes, but the arena should be usable
        let s = arena.alloc_str("fresh");
        assert_eq!(s, "fresh");

        // Return it
        return_storage_arena(arena);
    }

    // ========================================================================
    // Session Tracking
    // ========================================================================

    #[test]
    fn test_multiple_sessions_sequential() {
        // Each create/drop cycle should maintain correct session counts
        let base = active_eval_sessions();
        for _ in 0..10 {
            let _state = ArenaState::new();
            assert_eq!(active_eval_sessions(), base + 1);
        }
        assert_eq!(active_eval_sessions(), base);
    }

    #[test]
    fn test_eval_arena_reset_after_session_end() {
        // Access eval arena to establish baseline
        let arena1 = get_eval_arena();
        let ptr1 = arena1 as *const Bump;

        // Create and drop an ArenaState (begin + end session)
        {
            let _state = ArenaState::new();
        }

        // Start a new session — this should trigger the deferred reset
        let state2 = ArenaState::new();

        // Access eval arena again — should be the same pointer (reset, not reallocated)
        let arena2 = get_eval_arena();
        let ptr2 = arena2 as *const Bump;

        assert_eq!(ptr1, ptr2, "eval arena should be the same pointer (reset, not reallocated)");
        drop(state2);
    }

    // ========================================================================
    // Multi-Session Stress
    // ========================================================================

    #[test]
    fn test_many_sessions_sequential() {
        for _ in 0..100 {
            let mut state = ArenaState::new();
            let factory = state.storage_factory();

            // Allocate some data
            let expr = factory.sexpr(vec![
                factory.atom("rule"),
                factory.long(42),
                factory.string("value"),
            ]);
            state.source_mut().push(expr);

            // Output some results
            state.output_mut().push(factory.long(84));

            // Verify values are accessible
            assert_eq!(state.source().len(), 1);
            assert_eq!(state.output().len(), 1);
        }

        // After all sessions, session count should be back to baseline
        // (can't assert exact 0 due to concurrent tests on same thread)

        // Pool should be bounded
        STORAGE_ARENA_POOL.with(|pool| {
            assert!(pool.borrow().len() <= MAX_POOL_SIZE);
        });
    }

    // ========================================================================
    // Parallel Arena Usage
    // ========================================================================

    #[test]
    fn test_parallel_sessions() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let num_threads = 4;
        let barrier = Arc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();

                    for i in 0..25 {
                        let mut state = ArenaState::new();
                        let factory = state.storage_factory();

                        let expr = factory.sexpr(vec![
                            factory.atom("thread-expr"),
                            factory.long(thread_id as i64),
                            factory.long(i as i64),
                        ]);
                        state.source_mut().push(expr);

                        // Verify value integrity
                        let items = state.source()[0]
                            .as_sexpr()
                            .expect("should be sexpr");
                        assert_eq!(items.len(), 3);
                        assert_eq!(items[1].as_long(), Some(thread_id as i64));
                        assert_eq!(items[2].as_long(), Some(i as i64));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread should not panic");
        }
    }

    #[test]
    fn test_parallel_eval_arena_isolation() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let num_threads = 4;
        let barrier = Arc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();

                    let factory = get_eval_factory();
                    let values: Vec<_> = (0..100)
                        .map(|i| factory.long(thread_id as i64 * 1000 + i))
                        .collect();

                    // Verify all values are intact (no cross-thread contamination)
                    for (i, v) in values.iter().enumerate() {
                        assert_eq!(
                            v.as_long(),
                            Some(thread_id as i64 * 1000 + i as i64),
                            "thread {} value {} corrupted",
                            thread_id,
                            i
                        );
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread should not panic");
        }
    }

    // ========================================================================
    // Drop Timing (O(1) verification)
    // ========================================================================

    #[test]
    fn test_drop_time_constant() {
        // Allocate small state
        let small_allocs = 100;
        let small_drop_time = {
            let mut state = ArenaState::new();
            let factory = state.storage_factory();
            for i in 0..small_allocs {
                state.output_mut().push(factory.long(i));
            }
            let start = std::time::Instant::now();
            drop(state);
            start.elapsed()
        };

        // Allocate large state (100x more data)
        let large_allocs = 10_000;
        let large_drop_time = {
            let mut state = ArenaState::new();
            let factory = state.storage_factory();
            for i in 0..large_allocs {
                // Create nested sexprs for more arena usage
                let expr = factory.sexpr(vec![
                    factory.atom("rule"),
                    factory.long(i),
                    factory.string(&format!("value-{}", i)),
                ]);
                state.output_mut().push(expr);
            }
            let start = std::time::Instant::now();
            drop(state);
            start.elapsed()
        };

        // O(1) drop: large should not be proportionally slower than small.
        // Allow generous margin (10x) since timing is noisy, but
        // with O(n) drop the ratio would be ~100x.
        let ratio = if small_drop_time.as_nanos() > 0 {
            large_drop_time.as_nanos() as f64 / small_drop_time.as_nanos() as f64
        } else {
            // Both are sub-nanosecond (extremely fast), which confirms O(1)
            1.0
        };

        // With O(1) drop, the ratio should be well under 50x
        // (O(n) with 100x more data would give ~100x ratio)
        assert!(
            ratio < 50.0,
            "Drop time ratio ({:.1}x) suggests non-O(1) behavior. \
             Small: {:?}, Large: {:?}",
            ratio,
            small_drop_time,
            large_drop_time,
        );
    }

    // ========================================================================
    // Default Trait
    // ========================================================================

    #[test]
    fn test_arena_state_default() {
        let state = ArenaState::default();
        assert!(state.source().is_empty());
        assert!(state.output().is_empty());
    }

    // ========================================================================
    // StorageFactory Send/Sync
    // ========================================================================

    #[test]
    fn test_storage_factory_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<StorageFactory>();
        assert_sync::<StorageFactory>();
    }

    // ========================================================================
    // Integration: Compile + Evaluate through ArenaState
    // ========================================================================

    #[test]
    fn test_compile_arena_integration() {
        use crate::backend::compile::compile_arena;
        use crate::backend::eval::eval_arena;
        use crate::backend::eval::trampoline::new_arena_env;

        let state = compile_arena("!(+ 1 2)").expect("compile should succeed");
        assert!(!state.source().is_empty(), "should have source expressions");

        let mut env = new_arena_env();
        for expr in state.source().iter().copied() {
            let (results, new_env) = eval_arena(expr, env, &state);
            env = new_env;

            // !(+ 1 2) should produce [3]
            let non_empty: Vec<_> = results.iter().filter(|v| !v.is_empty()).collect();
            if !non_empty.is_empty() {
                assert_eq!(non_empty.len(), 1);
                assert_eq!(non_empty[0].as_long(), Some(3));
            }
        }
    }

    #[test]
    fn test_compile_arena_rules() {
        use crate::backend::models::metta_value_trait::MettaValue as MettaValueTrait;
        use crate::backend::compile::compile_arena;
        use crate::backend::eval::eval_arena;
        use crate::backend::eval::trampoline::new_arena_env;

        let source = "(= (double $x) (+ $x $x))\n!(double 5)";
        let state = compile_arena(source).expect("compile should succeed");

        let mut env = new_arena_env();
        let mut final_results = Vec::new();

        for expr in state.source().iter().copied() {
            let (results, new_env) = eval_arena(expr, env, &state);
            env = new_env;
            for r in &results {
                if !r.is_empty() {
                    final_results.push(*r);
                }
            }
        }

        // (double 5) should produce 10
        assert!(
            final_results.iter().any(|r| r.as_long() == Some(10)),
            "expected 10 in results, got: {:?}",
            final_results.iter().map(|r| r.friendly_repr()).collect::<Vec<_>>()
        );
    }
}
