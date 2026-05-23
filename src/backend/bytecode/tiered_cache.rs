//! Unified Tiered Compilation Cache
//!
//! This module provides a unified cache for managing all compilation tiers:
//!
//! ```text
//! Tier 0: Tree-Walker Interpreter (cold code, 0-1 executions)
//! Tier 1: Bytecode VM (warm code, 2+ executions)
//! Tier 2: JIT Stage 1 (hot code, 100+ executions)
//! Tier 3: JIT Stage 2 (very hot code, 500+ executions)
//! ```
//!
//! ## Key Features
//!
//! - **Non-blocking compilation**: All tier transitions happen in the background via priority scheduler
//! - **Graceful fallback**: Always executes with the best available tier
//! - **Lock-free access**: Uses DashMap and atomics for thread-safe concurrent access
//! - **Unified state management**: Single cache tracks all tier states per expression
//! - **Priority-based scheduling**: Compilation tasks use BACKGROUND_COMPILE priority to avoid starving eval tasks
//!
//! ## Design
//!
//! Each expression is tracked by its structural hash (u64). On every execution:
//! 1. Execution count is atomically incremented
//! 2. If a threshold is crossed and the previous tier is Ready, spawn background compilation
//! 3. Dispatch to the best available tier (highest Ready tier)
//!
//! Compilation is asynchronous - we spawn priority-scheduled tasks and continue using the current tier
//! until the next tier becomes Ready.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::backend::models::{register_root_provider, RootProvider};

use xxhash_rust::xxh3::Xxh3;

use dashmap::DashMap;

use crate::backend::environment::core::MettaEnvironment;
use crate::backend::hash_utils::IdentityU64BuildHasher;
use crate::backend::models::work_pool::global_compile_pool;
use crate::backend::models::{MettaValue, MettaValueInner, ValueView};
use crate::backend::priority_scheduler::{priority_levels, TaskTypeId};

use super::cache::hash_metta_value;
use super::chunk::BytecodeChunk;
use super::compiler::compile_arc;
use super::jit::compiler::JitCompiler;

/// Cached check for the `METTATRON_JIT_DEBUG` environment variable.
/// When set, logs JIT compilation failures and a summary at program exit.
/// Uses `OnceLock` so the syscall happens at most once per process.
static JIT_DEBUG: OnceLock<bool> = OnceLock::new();

fn is_jit_debug() -> bool {
    *JIT_DEBUG.get_or_init(|| std::env::var("METTATRON_JIT_DEBUG").is_ok())
}

/// Process-persistent JIT compiler.
///
/// `JitCompiler` owns the `cranelift_jit::JITModule` whose mmap'd
/// executable pages back every `NativeCode.ptr` stored in
/// `ExprCompilationState::{jit1_code,jit2_code}`. Because those
/// `NativeCode` entries live in `OnceLock` (write-once, never replaced)
/// for the lifetime of the process, the compiler MUST outlive them — so
/// it is process-static. Dropping it would call `JITModule::drop` →
/// `munmap` the pages → every cached `NativeCode.ptr` would dangle →
/// SIGILL on next dispatch.
///
/// `Mutex` is acceptable here: JIT compilation runs only on the
/// background `global_compile_pool()` (not on the eval hot path) and
/// is infrequent (hot-expression threshold).
fn global_jit_compiler() -> &'static Mutex<Option<JitCompiler>> {
    static COMPILER: OnceLock<Mutex<Option<JitCompiler>>> = OnceLock::new();
    COMPILER.get_or_init(|| Mutex::new(None))
}

/// Threshold to trigger bytecode compilation (after 5 executions).
///
/// V8 equivalent: Ignition → Sparkplug (~1-5, near-immediate).
/// Raised from 1 to 5 to avoid compiling expressions that are only
/// evaluated once or twice (e.g., top-level module imports, initialization).
/// Non-blocking (WorkPool background).
pub const BYTECODE_THRESHOLD: u32 = 5;

/// Threshold to trigger JIT Stage 1 compilation (after 200 executions).
///
/// V8 equivalent: Sparkplug → Maglev (~100-400, adaptive).
/// Set to 200 (mid-range of Maglev threshold) to allow type profile data
/// from bytecode execution before promoting to JIT.
pub const JIT1_THRESHOLD: u32 = 200;

/// Threshold to trigger JIT Stage 2 compilation (after 2000 executions).
///
/// V8 equivalent: Maglev → Turbofan (~1000-6000, adaptive).
/// Set to 2000 (low-range of Turbofan) to ensure JIT1 has collected
/// stable profiles for speculative optimization.
pub const JIT2_THRESHOLD: u32 = 2_000;

/// Interval for flushing thread-local counters to the global DashMap.
///
/// Every FLUSH_INTERVAL sub-expression evals, the thread-local pointer-keyed
/// counter map is flushed to the global DashMap with content hashing. This
/// amortizes the expensive hash+probe cost over many cheap pointer increments.
pub const FLUSH_INTERVAL: u32 = 1024;

// =============================================================================
// Per-Slot Atomic Counter Infrastructure
// =============================================================================
//
// Execution counters stored per-slot in ValuePage::exec_counts (AtomicU32).
// Eval threads increment counters via atomic fetch_add (~10-15 cycles on
// cache hit via thread-local page cache). No HashMap, no DashMap, no lock.
//
// Counters are periodically flushed to the global TieredCache DashMap by
// the GC cron task (200ms interval), and dead-slot counters are flushed
// before GC Phase 3 frees their slots.
//
// Analogous to per-CPU counters in the Linux kernel, but per-slot instead
// of per-thread to avoid pointer-keyed map overhead.

/// Cached page metadata for O(1) hot-path exec_count increment and hash lookup.
#[derive(Clone, Copy)]
struct CachedExecPage {
    /// Page data region start address.
    base: usize,
    /// base + PAGE_SIZE.
    end: usize,
    /// Raw pointer to the page's exec_counts[0].
    counters_ptr: *const AtomicU32,
    /// Raw pointer to the page's compilation_hashes[0].
    hashes_ptr: *const AtomicU64,
    /// Slot size in bytes (for index computation).
    slot_size: usize,
    /// CACHE_GENERATION snapshot at cache time for invalidation.
    generation: u64,
}

// SAFETY: raw pointer is to a 'static slab page that outlives the thread.
// The generation counter invalidates stale entries before dereferencing.
unsafe impl Send for CachedExecPage {}

thread_local! {
    /// Thread-local page cache for fast exec_count increments.
    static EXEC_PAGE_CACHE: Cell<Option<CachedExecPage>> = const { Cell::new(None) };
}

/// Increment the per-slot execution counter for a slab-allocated value.
///
/// Hot path: ~10-15 cycles on cache hit (pointer arithmetic + atomic fetch_add).
/// No hash, no map, no lock.
#[inline]
pub fn increment_exec_count(ptr: *const MettaValueInner) {
    let addr = ptr as usize;
    EXEC_PAGE_CACHE.with(|cell| {
        if let Some(cached) = cell.get() {
            let current_gen =
                crate::backend::models::gc_allocator::global_allocator().page_generation();
            if cached.generation == current_gen && addr >= cached.base && addr < cached.end {
                let slot_idx = (addr - cached.base) / cached.slot_size;
                // SAFETY: slot_idx is within bounds (addr range-checked above),
                // page is still live (generation matches), counter is AtomicU32.
                unsafe { &*cached.counters_ptr.add(slot_idx) }.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Cache miss: find page via SlabAllocator, populate cache
        increment_exec_count_slow(ptr, addr, cell);
    });
}

/// Cold path for increment_exec_count: linear scan of value pages to find
/// the containing page, then populate the thread-local cache.
#[cold]
fn increment_exec_count_slow(
    _ptr: *const MettaValueInner,
    addr: usize,
    cell: &Cell<Option<CachedExecPage>>,
) {
    use crate::backend::models::gc_allocator::global_allocator;

    let allocator = global_allocator();
    let slot_size = allocator.value_slot_size();
    let generation = allocator.page_generation();
    let pages = allocator.value_pages_read();

    for page in pages.iter() {
        let base = page.slot_ptr(0, slot_size) as usize;
        let end = base + crate::backend::models::gc_allocator::PAGE_SIZE;
        if addr >= base && addr < end {
            let slot_idx = (addr - base) / slot_size;
            page.exec_count_fetch_add(slot_idx, 1);
            // Populate cache
            cell.set(Some(CachedExecPage {
                base,
                end,
                counters_ptr: page.exec_counts_ptr(),
                hashes_ptr: page.compilation_hashes_ptr(),
                slot_size,
                generation,
            }));
            return;
        }
    }
    // Pointer not in any page — ignore (shouldn't happen for slab values)
}

/// Read the cached compilation hash for a slab-allocated value.
/// Returns 0 if not yet computed (cron hasn't flushed this slot yet).
/// Hot path: ~3-5 cycles on CachedExecPage hit.
#[inline]
pub fn get_slot_compilation_hash(ptr: *const MettaValueInner) -> u64 {
    let addr = ptr as usize;
    EXEC_PAGE_CACHE.with(|cell| {
        if let Some(cached) = cell.get() {
            let current_gen =
                crate::backend::models::gc_allocator::global_allocator().page_generation();
            if cached.generation == current_gen && addr >= cached.base && addr < cached.end {
                let slot_idx = (addr - cached.base) / cached.slot_size;
                // SAFETY: slot_idx is within bounds (addr range-checked above),
                // page is still live (generation matches), hashes_ptr is AtomicU64.
                return unsafe { &*cached.hashes_ptr.add(slot_idx) }.load(Ordering::Relaxed);
            }
        }
        0 // Cache miss — return 0 (no hash available)
    })
}

/// Increment per-slot execution counter AND return cached compilation hash.
///
/// Single thread-local access + single generation check (vs 2× for separate
/// `increment_exec_count` + `get_slot_compilation_hash` calls).
/// Hot path: ~15-20 cycles on CachedExecPage hit.
///
/// Returns the cached compilation hash (0 if not yet computed by cron).
#[inline]
pub fn increment_and_get_hash(ptr: *const MettaValueInner) -> u64 {
    let addr = ptr as usize;
    EXEC_PAGE_CACHE.with(|cell| {
        if let Some(cached) = cell.get() {
            let current_gen =
                crate::backend::models::gc_allocator::global_allocator().page_generation();
            if cached.generation == current_gen && addr >= cached.base && addr < cached.end {
                let slot_idx = (addr - cached.base) / cached.slot_size;
                // SAFETY: slot_idx is within bounds (addr range-checked above),
                // page is still live (generation matches).
                unsafe { &*cached.counters_ptr.add(slot_idx) }.fetch_add(1, Ordering::Relaxed);
                return unsafe { &*cached.hashes_ptr.add(slot_idx) }.load(Ordering::Relaxed);
            }
        }
        // Cache miss: find page, populate cache, increment counter, return hash
        increment_and_get_hash_slow(ptr, addr, cell)
    })
}

/// Cold path for `increment_and_get_hash`: linear scan of value pages to find
/// the containing page, populate the thread-local cache, increment counter,
/// and return the compilation hash.
#[cold]
fn increment_and_get_hash_slow(
    _ptr: *const MettaValueInner,
    addr: usize,
    cell: &Cell<Option<CachedExecPage>>,
) -> u64 {
    use crate::backend::models::gc_allocator::global_allocator;

    let allocator = global_allocator();
    let slot_size = allocator.value_slot_size();
    let generation = allocator.page_generation();
    let pages = allocator.value_pages_read();

    for page in pages.iter() {
        let base = page.slot_ptr(0, slot_size) as usize;
        let end = base + crate::backend::models::gc_allocator::PAGE_SIZE;
        if addr >= base && addr < end {
            let slot_idx = (addr - base) / slot_size;
            page.exec_count_fetch_add(slot_idx, 1);
            let hash = page.compilation_hash(slot_idx);
            // Populate cache for subsequent fast-path hits
            cell.set(Some(CachedExecPage {
                base,
                end,
                counters_ptr: page.exec_counts_ptr(),
                hashes_ptr: page.compilation_hashes_ptr(),
                slot_size,
                generation,
            }));
            return hash;
        }
    }
    // Pointer not in any page — ignore (shouldn't happen for slab values)
    0
}

/// Compilation status for a tier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TierStatusKind {
    /// Not yet triggered for compilation
    NotStarted = 0,
    /// Background compilation in progress
    Compiling = 1,
    /// Compiled artifact is ready and available
    Ready = 2,
    /// Compilation failed, use fallback tier
    Failed = 3,
}

impl From<u8> for TierStatusKind {
    fn from(v: u8) -> Self {
        match v {
            0 => TierStatusKind::NotStarted,
            1 => TierStatusKind::Compiling,
            2 => TierStatusKind::Ready,
            3 => TierStatusKind::Failed,
            _ => TierStatusKind::NotStarted,
        }
    }
}

/// Native code representation for JIT-compiled functions
///
/// Wraps a function pointer with size information for memory tracking.
///
/// SAFETY INVARIANT: `ptr` is only valid for the lifetime of the
/// `JitCompiler` that produced it. The compiler owns the
/// `cranelift_jit::JITModule` whose mmap'd executable pages `ptr`
/// points into; dropping the compiler calls `JITModule::drop` which
/// `munmap`s those pages, leaving `ptr` dangling. To uphold this
/// invariant, every `NativeCode` stored in the tiered cache is
/// produced by the process-static `global_jit_compiler()`, which is
/// never dropped. Do NOT construct a `NativeCode` from a `JitCompiler`
/// with a shorter lifetime.
#[derive(Clone)]
pub struct NativeCode {
    /// Pointer to JIT-compiled native code
    pub ptr: *const (),
    /// Size of the generated native code in bytes
    pub code_size: usize,
}

// SAFETY: `ptr` points into the mmap owned by the process-static
// `global_jit_compiler()` (see SAFETY INVARIANT on `NativeCode`),
// which never drops; therefore the pointer is valid for the process
// lifetime and is safe to share across threads.
unsafe impl Send for NativeCode {}
unsafe impl Sync for NativeCode {}

impl std::fmt::Debug for NativeCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeCode")
            .field("ptr", &format!("{:p}", self.ptr))
            .field("code_size", &self.code_size)
            .finish()
    }
}

/// Per-expression compilation state across all tiers
///
/// Tracks the execution count and compilation status for each tier.
/// All fields use atomic operations for lock-free concurrent access.
/// Code storage uses OnceLock for lock-free reads after initialization.
/// Phase 11.B (2026-05-17) — pack a `(rule_epoch, bool)` pair into a
/// single u64 for atomic storage. Encoding:
/// - bit 0:        1 = set, 0 = unset
/// - bit 1:        value (1 = true, 0 = false)
/// - bits 2..=63:  rule_epoch at write time
///
/// 62 bits of epoch is sufficient (rule_epoch is a process-wide counter
/// of rule_index mutations; reaching 2^62 is not realizable on any
/// physical hardware in the program's lifetime).
#[inline]
fn encode_epoch_bool(epoch: u64, value: bool) -> u64 {
    let val_bit = if value { 1u64 << 1 } else { 0 };
    (epoch << 2) | val_bit | 1
}

#[inline]
fn decode_epoch_bool(packed: u64, current_epoch: u64) -> Option<bool> {
    if packed & 1 == 0 {
        return None;
    }
    let stored_epoch = packed >> 2;
    if stored_epoch != current_epoch {
        return None;
    }
    Some((packed >> 1) & 1 != 0)
}

pub struct ExprCompilationState {
    /// Number of times this expression has been executed
    pub execution_count: AtomicU32,

    // Bytecode tier (Tier 1)
    /// Status of bytecode compilation
    bytecode_status: AtomicU8,
    /// Compiled bytecode chunk (write-once via OnceLock, lock-free reads)
    bytecode_chunk: OnceLock<Arc<BytecodeChunk>>,

    // JIT Stage 1 (Tier 2)
    /// Status of JIT Stage 1 compilation
    jit1_status: AtomicU8,
    /// JIT Stage 1 native code (write-once via OnceLock, lock-free reads)
    jit1_code: OnceLock<Arc<NativeCode>>,

    // JIT Stage 2 (Tier 3)
    /// Status of JIT Stage 2 compilation
    jit2_status: AtomicU8,
    /// JIT Stage 2 native code (write-once via OnceLock, lock-free reads)
    jit2_code: OnceLock<Arc<NativeCode>>,

    /// Original expression hash for identification and tracing.
    /// Stored as hash to avoid cloning large expressions.
    pub expr_hash: u64,

    /// Cached `TypeSignatureRegistry` with epoch invalidation.
    ///
    /// Stores `(epoch, registry)` — rebuilt only when `RULE_EPOCH` changes.
    /// Reduces JIT entry overhead from O(types + inferred_types) to O(1) amortized.
    type_registry_cache: parking_lot::Mutex<Option<(u64, Arc<super::jit::TypeSignatureRegistry>)>>,

    /// Phase 8b/8c: Runtime type profile collected during bytecode VM execution.
    ///
    /// Populated incrementally by the bytecode VM when profiling is active
    /// (execution_count >= PROFILING_THRESHOLD). Snapshotted and passed to the
    /// JIT compiler when triggering JIT Stage 1 or Stage 2 compilation.
    ///
    /// V8 equivalent: FeedbackVector (per-function IC slot array).
    /// HotSpot equivalent: MethodData (MDO).
    pub runtime_profile:
        std::sync::Arc<parking_lot::Mutex<super::runtime_profile::RuntimeTypeProfile>>,

    /// Cached result of `can_compile_with_env()` (0=unknown, 1=true, 2=false).
    /// Write-once, lock-free reads. Eliminates ~922K recursive tree walks for
    /// repeated expressions in the PLN benchmark.
    compilable_with_env: AtomicU8,

    /// Phase 11.B (2026-05-17) — bit-packed `(rule_epoch, value)` caches
    /// for the three expression-purity predicates evaluated at the
    /// per-step gate in `eval_loop.rs::should_record_execution_sample`
    /// branch:
    ///
    /// - `expression_involves_impure_rules`
    /// - `expression_has_overridden_grounded_op`
    /// - `expression_has_declared_meta_typed_params`
    ///
    /// Each predicate is O(tree × needles × Phase-11.A bloom) when
    /// recomputed. PLN's hot loop hits each one once per sampled
    /// trampoline step on the same sub-expression; the cache lets the
    /// second-and-later sampled visits return in O(1).
    ///
    /// Encoding (per AtomicU64):
    ///   - bit 0:        1 = cache populated, 0 = unset
    ///   - bit 1:        value (1 = true, 0 = false)
    ///   - bits 2..=63:  rule_epoch at write time
    ///
    /// Read flow: load Acquire; if bit 0 is 0 → `None`; else compare
    /// epoch with `RULE_EPOCH.load(Acquire)`; on mismatch the cached
    /// value is stale → return `None` (caller recomputes and resets).
    ///
    /// Why not `parking_lot::Mutex<Option<(u64, bool)>>` (as
    /// `type_registry_cache` does)? The mutex pays ~10 ns per call
    /// even uncontended; the per-step gate fires thousands of times
    /// per inference. Bit-packing keeps the read at one Acquire load.
    cached_impure_rules: AtomicU64,
    cached_overridden_grounded: AtomicU64,
    cached_meta_typed: AtomicU64,
}

impl ExprCompilationState {
    /// Create a new cold state with zero executions
    pub fn new(expr_hash: u64) -> Self {
        Self {
            execution_count: AtomicU32::new(0),
            bytecode_status: AtomicU8::new(TierStatusKind::NotStarted as u8),
            bytecode_chunk: OnceLock::new(),
            jit1_status: AtomicU8::new(TierStatusKind::NotStarted as u8),
            jit1_code: OnceLock::new(),
            jit2_status: AtomicU8::new(TierStatusKind::NotStarted as u8),
            jit2_code: OnceLock::new(),
            expr_hash,
            type_registry_cache: parking_lot::Mutex::new(None),
            runtime_profile: std::sync::Arc::new(parking_lot::Mutex::new(
                super::runtime_profile::RuntimeTypeProfile::new(),
            )),
            compilable_with_env: AtomicU8::new(0),
            cached_impure_rules: AtomicU64::new(0),
            cached_overridden_grounded: AtomicU64::new(0),
            cached_meta_typed: AtomicU64::new(0),
        }
    }

    /// Phase 11.B — read the cached `expression_involves_impure_rules`
    /// result if its rule_epoch tag matches the current
    /// `RULE_EPOCH`. Returns `None` when unset or stale.
    #[inline]
    pub fn cached_involves_impure_rules(&self, current_epoch: u64) -> Option<bool> {
        decode_epoch_bool(self.cached_impure_rules.load(Ordering::Acquire), current_epoch)
    }

    /// Phase 11.B — write the `expression_involves_impure_rules` cache
    /// for the given `rule_epoch`. Last-writer-wins, lock-free.
    #[inline]
    pub fn set_involves_impure_rules(&self, epoch: u64, value: bool) {
        self.cached_impure_rules
            .store(encode_epoch_bool(epoch, value), Ordering::Release);
    }

    /// Phase 11.B — read the cached
    /// `expression_has_overridden_grounded_op` result.
    #[inline]
    pub fn cached_has_overridden_grounded_op(&self, current_epoch: u64) -> Option<bool> {
        decode_epoch_bool(
            self.cached_overridden_grounded.load(Ordering::Acquire),
            current_epoch,
        )
    }

    /// Phase 11.B — write the `expression_has_overridden_grounded_op` cache.
    #[inline]
    pub fn set_has_overridden_grounded_op(&self, epoch: u64, value: bool) {
        self.cached_overridden_grounded
            .store(encode_epoch_bool(epoch, value), Ordering::Release);
    }

    /// Phase 11.B — read the cached
    /// `expression_has_declared_meta_typed_params` result.
    #[inline]
    pub fn cached_has_declared_meta_typed(&self, current_epoch: u64) -> Option<bool> {
        decode_epoch_bool(self.cached_meta_typed.load(Ordering::Acquire), current_epoch)
    }

    /// Phase 11.B — write the `expression_has_declared_meta_typed_params` cache.
    #[inline]
    pub fn set_has_declared_meta_typed(&self, epoch: u64, value: bool) {
        self.cached_meta_typed
            .store(encode_epoch_bool(epoch, value), Ordering::Release);
    }

    /// Get or build a cached `TypeSignatureRegistry` for JIT execution.
    ///
    /// Returns the cached registry if the current `RULE_EPOCH` matches the
    /// cached epoch. Otherwise rebuilds from the environment and caches it.
    /// O(1) amortized — only rebuilds when rules/types change.
    pub fn get_or_build_type_registry(
        &self,
        env: &crate::backend::eval::trampoline::MettaEnvironment,
    ) -> Arc<super::jit::TypeSignatureRegistry> {
        let current_epoch =
            crate::backend::environment::rule_management::RULE_EPOCH.load(Ordering::Acquire);
        let mut guard = self.type_registry_cache.lock();
        if let Some((cached_epoch, ref registry)) = *guard {
            if cached_epoch == current_epoch {
                return Arc::clone(registry);
            }
        }
        let registry = Arc::new(super::jit::TypeSignatureRegistry::from_env(env));
        *guard = Some((current_epoch, Arc::clone(&registry)));
        registry
    }

    /// Get the current execution count
    #[inline]
    pub fn count(&self) -> u32 {
        self.execution_count.load(Ordering::Relaxed)
    }

    /// Get bytecode status
    #[inline]
    pub fn bytecode_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.bytecode_status.load(Ordering::Acquire))
    }

    /// Check cached `can_compile_with_env` result. Returns `None` if not yet computed.
    #[inline]
    pub fn cached_compilable_with_env(&self) -> Option<bool> {
        match self.compilable_with_env.load(Ordering::Relaxed) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    /// Store `can_compile_with_env` result (idempotent, first write wins).
    #[inline]
    pub fn set_compilable_with_env(&self, compilable: bool) {
        let val = if compilable { 1u8 } else { 2u8 };
        let _ =
            self.compilable_with_env
                .compare_exchange(0, val, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Get bytecode chunk if ready (lock-free read via OnceLock)
    #[inline]
    pub fn bytecode_chunk(&self) -> Option<Arc<BytecodeChunk>> {
        if self.bytecode_status() == TierStatusKind::Ready {
            self.bytecode_chunk.get().cloned()
        } else {
            None
        }
    }

    /// Get JIT Stage 1 status
    #[inline]
    pub fn jit1_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.jit1_status.load(Ordering::Acquire))
    }

    /// Get JIT Stage 1 native code if ready (lock-free read via OnceLock)
    #[inline]
    pub fn jit1_code(&self) -> Option<Arc<NativeCode>> {
        if self.jit1_status() == TierStatusKind::Ready {
            self.jit1_code.get().cloned()
        } else {
            None
        }
    }

    /// Get JIT Stage 2 status
    #[inline]
    pub fn jit2_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.jit2_status.load(Ordering::Acquire))
    }

    /// Get JIT Stage 2 native code if ready (lock-free read via OnceLock)
    #[inline]
    pub fn jit2_code(&self) -> Option<Arc<NativeCode>> {
        if self.jit2_status() == TierStatusKind::Ready {
            self.jit2_code.get().cloned()
        } else {
            None
        }
    }

    /// Try to start bytecode compilation (atomic CAS)
    /// Returns true if this thread won the race to compile
    #[inline]
    pub fn try_start_bytecode_compile(&self) -> bool {
        self.bytecode_status
            .compare_exchange(
                TierStatusKind::NotStarted as u8,
                TierStatusKind::Compiling as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Set bytecode compilation result (write-once via OnceLock)
    pub fn set_bytecode_ready(&self, chunk: Arc<BytecodeChunk>) {
        // OnceLock::set ignores the value if already set (write-once semantics)
        let _ = self.bytecode_chunk.set(chunk);
        self.bytecode_status
            .store(TierStatusKind::Ready as u8, Ordering::Release);
    }

    /// Mark bytecode compilation as failed
    pub fn set_bytecode_failed(&self) {
        self.bytecode_status
            .store(TierStatusKind::Failed as u8, Ordering::Release);
    }

    /// Try to start JIT Stage 1 compilation (atomic CAS)
    /// Returns true if this thread won the race to compile
    #[inline]
    pub fn try_start_jit1_compile(&self) -> bool {
        self.jit1_status
            .compare_exchange(
                TierStatusKind::NotStarted as u8,
                TierStatusKind::Compiling as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Set JIT Stage 1 compilation result (write-once via OnceLock)
    pub fn set_jit1_ready(&self, code: Arc<NativeCode>) {
        // OnceLock::set ignores the value if already set (write-once semantics)
        let _ = self.jit1_code.set(code);
        self.jit1_status
            .store(TierStatusKind::Ready as u8, Ordering::Release);
    }

    /// Mark JIT Stage 1 compilation as failed
    pub fn set_jit1_failed(&self) {
        self.jit1_status
            .store(TierStatusKind::Failed as u8, Ordering::Release);
    }

    /// Try to start JIT Stage 2 compilation (atomic CAS)
    /// Returns true if this thread won the race to compile
    #[inline]
    pub fn try_start_jit2_compile(&self) -> bool {
        self.jit2_status
            .compare_exchange(
                TierStatusKind::NotStarted as u8,
                TierStatusKind::Compiling as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Set JIT Stage 2 compilation result (write-once via OnceLock)
    pub fn set_jit2_ready(&self, code: Arc<NativeCode>) {
        // OnceLock::set ignores the value if already set (write-once semantics)
        let _ = self.jit2_code.set(code);
        self.jit2_status
            .store(TierStatusKind::Ready as u8, Ordering::Release);
    }

    /// Mark JIT Stage 2 compilation as failed
    pub fn set_jit2_failed(&self) {
        self.jit2_status
            .store(TierStatusKind::Failed as u8, Ordering::Release);
    }

    /// Revert bytecode compilation status from Compiling back to NotStarted.
    ///
    /// Used when the compilation task is dropped due to backpressure,
    /// allowing a future execution to re-trigger compilation.
    pub fn revert_bytecode_to_not_started(&self) {
        self.bytecode_status
            .compare_exchange(
                TierStatusKind::Compiling as u8,
                TierStatusKind::NotStarted as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .ok(); // Ignore if already transitioned (race with concurrent set_*_ready/failed)
    }

    /// Revert JIT Stage 1 compilation status from Compiling back to NotStarted.
    pub fn revert_jit1_to_not_started(&self) {
        self.jit1_status
            .compare_exchange(
                TierStatusKind::Compiling as u8,
                TierStatusKind::NotStarted as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .ok();
    }

    /// Revert JIT Stage 2 compilation status from Compiling back to NotStarted.
    pub fn revert_jit2_to_not_started(&self) {
        self.jit2_status
            .compare_exchange(
                TierStatusKind::Compiling as u8,
                TierStatusKind::NotStarted as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .ok();
    }
}

impl std::fmt::Debug for ExprCompilationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExprCompilationState")
            .field("execution_count", &self.count())
            .field("bytecode_status", &self.bytecode_status())
            .field("jit1_status", &self.jit1_status())
            .field("jit2_status", &self.jit2_status())
            .field("expr_hash", &self.expr_hash)
            .finish()
    }
}

/// Execution tier for dispatch decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ExecutionTier {
    /// Use tree-walking interpreter
    Interpreter = 0,
    /// Use bytecode VM
    Bytecode = 1,
    /// Use JIT Stage 1 native code
    JitStage1 = 2,
    /// Use JIT Stage 2 native code
    JitStage2 = 3,
}

/// Unified tiered compilation cache
///
/// Manages compilation state for all expressions across all tiers.
/// Uses DashMap for lock-free concurrent access.
pub struct TieredCache {
    /// Map from expression hash to compilation state
    pub(crate) entries: DashMap<u64, Arc<ExprCompilationState>, IdentityU64BuildHasher>,

    /// Source expressions currently captured by queued/running bytecode
    /// compilation tasks. These are shallow `MettaValue` roots, not owned
    /// copies; `TieredCacheRoots` traces them while async compilation can
    /// still dereference the source expression.
    pending_bytecode_roots: Arc<DashMap<u64, MettaValue, IdentityU64BuildHasher>>,

    /// Threshold for bytecode compilation
    pub bytecode_threshold: u32,

    /// Threshold for JIT Stage 1 compilation
    pub jit1_threshold: u32,

    /// Threshold for JIT Stage 2 compilation
    pub jit2_threshold: u32,

    // Atomic statistics counters (lock-free to avoid contention at 4+ threads)
    // Gated behind track-stats feature — zero overhead when disabled.
    #[cfg(feature = "track-stats")]
    expressions_tracked: AtomicU64,
    #[cfg(feature = "track-stats")]
    pub(crate) total_executions: AtomicU64,
    #[cfg(feature = "track-stats")]
    bytecode_compilations_triggered: AtomicU64,
    #[cfg(feature = "track-stats")]
    bytecode_compilations_completed: AtomicU64,
    #[cfg(feature = "track-stats")]
    bytecode_compilations_failed: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_compilations_triggered: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_compilations_completed: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_compilations_failed: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_compilations_triggered: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_compilations_completed: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_compilations_failed: AtomicU64,
    #[cfg(feature = "track-stats")]
    interpreter_executions: AtomicU64,
    #[cfg(feature = "track-stats")]
    bytecode_executions: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_executions: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_executions: AtomicU64,

    // JIT failure reason breakdown counters
    #[cfg(feature = "track-stats")]
    jit1_failures_nondeterminism: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_failures_unsupported_opcode: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_failures_compiler_init: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit1_failures_codegen: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_failures_nondeterminism: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_failures_unsupported_opcode: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_failures_compiler_init: AtomicU64,
    #[cfg(feature = "track-stats")]
    jit2_failures_codegen: AtomicU64,
}

/// RAII handle for a source expression held by an async bytecode compile task.
///
/// The handle unregisters the root when the queued task is dropped or when the
/// compile closure finishes. This keeps rooting tied to the actual async
/// lifetime instead of to execution counts or tier status.
struct PendingBytecodeRootGuard {
    expr_hash: u64,
    roots: Arc<DashMap<u64, MettaValue, IdentityU64BuildHasher>>,
}

impl Drop for PendingBytecodeRootGuard {
    fn drop(&mut self) {
        self.roots.remove(&self.expr_hash);
    }
}

/// Statistics for the tiered compilation cache
#[derive(Debug, Clone, Default)]
pub struct TieredCacheStats {
    /// Number of expressions tracked
    pub expressions_tracked: u64,

    /// Total execution count across all expressions
    pub total_executions: u64,

    /// Number of bytecode compilations triggered
    pub bytecode_compilations_triggered: u64,

    /// Number of bytecode compilations completed
    pub bytecode_compilations_completed: u64,

    /// Number of bytecode compilations failed
    pub bytecode_compilations_failed: u64,

    /// Number of JIT Stage 1 compilations triggered
    pub jit1_compilations_triggered: u64,

    /// Number of JIT Stage 1 compilations completed
    pub jit1_compilations_completed: u64,

    /// Number of JIT Stage 1 compilations failed
    pub jit1_compilations_failed: u64,

    /// Number of JIT Stage 2 compilations triggered
    pub jit2_compilations_triggered: u64,

    /// Number of JIT Stage 2 compilations completed
    pub jit2_compilations_completed: u64,

    /// Number of JIT Stage 2 compilations failed
    pub jit2_compilations_failed: u64,

    /// Executions at interpreter tier
    pub interpreter_executions: u64,

    /// Executions at bytecode tier
    pub bytecode_executions: u64,

    /// Executions at JIT Stage 1 tier
    pub jit1_executions: u64,

    /// Executions at JIT Stage 2 tier
    pub jit2_executions: u64,

    // JIT failure reason breakdown
    /// JIT Stage 1 failures due to nondeterminism in bytecode chunk
    pub jit1_failures_nondeterminism: u64,
    /// JIT Stage 1 failures due to unsupported opcodes
    pub jit1_failures_unsupported_opcode: u64,
    /// JIT Stage 1 failures due to compiler initialization error
    pub jit1_failures_compiler_init: u64,
    /// JIT Stage 1 failures due to code generation error
    pub jit1_failures_codegen: u64,
    /// JIT Stage 2 failures due to nondeterminism in bytecode chunk
    pub jit2_failures_nondeterminism: u64,
    /// JIT Stage 2 failures due to unsupported opcodes
    pub jit2_failures_unsupported_opcode: u64,
    /// JIT Stage 2 failures due to compiler initialization error
    pub jit2_failures_compiler_init: u64,
    /// JIT Stage 2 failures due to code generation error
    pub jit2_failures_codegen: u64,
}

/// Per-expression stats snapshot for diagnostics display.
/// Zero-cost at runtime — reads from existing DashMap entries at print time only.
#[derive(Debug, Clone)]
pub struct PerExpressionStats {
    pub expr_hash: u64,
    pub execution_count: u32,
    pub bytecode_status: TierStatusKind,
    pub jit1_status: TierStatusKind,
    pub jit2_status: TierStatusKind,
}

impl std::fmt::Display for TierStatusKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TierStatusKind::NotStarted => write!(f, "NotStarted"),
            TierStatusKind::Compiling => write!(f, "Compiling"),
            TierStatusKind::Ready => write!(f, "Ready"),
            TierStatusKind::Failed => write!(f, "Failed"),
        }
    }
}

impl TieredCache {
    /// Create a new tiered compilation cache with default thresholds.
    ///
    /// Per V8 best practice: no global warm-up period. Each expression is
    /// tracked from first invocation — cold code naturally never reaches
    /// compilation thresholds.
    pub fn new() -> Self {
        Self {
            entries: DashMap::with_hasher(IdentityU64BuildHasher),
            pending_bytecode_roots: Arc::new(DashMap::with_hasher(IdentityU64BuildHasher)),
            bytecode_threshold: BYTECODE_THRESHOLD,
            jit1_threshold: JIT1_THRESHOLD,
            jit2_threshold: JIT2_THRESHOLD,
            #[cfg(feature = "track-stats")]
            expressions_tracked: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            total_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_compilations_triggered: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_compilations_completed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_compilations_failed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_compilations_triggered: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_compilations_completed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_compilations_failed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_compilations_triggered: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_compilations_completed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_compilations_failed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            interpreter_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_nondeterminism: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_unsupported_opcode: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_compiler_init: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_codegen: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_nondeterminism: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_unsupported_opcode: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_compiler_init: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_codegen: AtomicU64::new(0),
        }
    }

    /// Create a new cache with custom thresholds
    pub fn with_thresholds(bytecode: u32, jit1: u32, jit2: u32) -> Self {
        Self {
            entries: DashMap::with_hasher(IdentityU64BuildHasher),
            pending_bytecode_roots: Arc::new(DashMap::with_hasher(IdentityU64BuildHasher)),
            bytecode_threshold: bytecode,
            jit1_threshold: jit1,
            jit2_threshold: jit2,
            #[cfg(feature = "track-stats")]
            expressions_tracked: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            total_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_compilations_triggered: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_compilations_completed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_compilations_failed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_compilations_triggered: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_compilations_completed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_compilations_failed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_compilations_triggered: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_compilations_completed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_compilations_failed: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            interpreter_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            bytecode_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_executions: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_nondeterminism: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_unsupported_opcode: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_compiler_init: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit1_failures_codegen: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_nondeterminism: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_unsupported_opcode: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_compiler_init: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            jit2_failures_codegen: AtomicU64::new(0),
        }
    }

    fn register_pending_bytecode_root(
        &self,
        expr_hash: u64,
        expr: MettaValue,
    ) -> PendingBytecodeRootGuard {
        self.pending_bytecode_roots.insert(expr_hash, expr);
        PendingBytecodeRootGuard {
            expr_hash,
            roots: Arc::clone(&self.pending_bytecode_roots),
        }
    }

    fn collect_roots_into(&self, roots: &mut Vec<MettaValue>) {
        roots.extend(
            self.pending_bytecode_roots
                .iter()
                .map(|entry| *entry.value()),
        );
        for entry in self.entries.iter() {
            if let Some(chunk) = entry.value().bytecode_chunk() {
                super::cache::collect_chunk_constants(&chunk, roots);
            }
        }
    }

    /// Get or create a compilation state for an expression
    pub(crate) fn get_or_create_state(&self, expr: &MettaValue) -> Arc<ExprCompilationState> {
        let hash = hash_metta_value(expr);

        // Fast path: check if already exists
        if let Some(entry) = self.entries.get(&hash) {
            return Arc::clone(entry.value());
        }

        // Slow path: create new entry
        let state = Arc::new(ExprCompilationState::new(hash));
        self.entries.entry(hash).or_insert_with(|| {
            // Update stats atomically (lock-free)
            #[cfg(feature = "track-stats")]
            self.expressions_tracked.fetch_add(1, Ordering::Relaxed);
            Arc::clone(&state)
        });

        // Return the entry (could be ours or another thread's)
        self.entries
            .get(&hash)
            .map(|e| Arc::clone(e.value()))
            .unwrap_or(state)
    }

    /// Record an execution and trigger appropriate tier compilations
    ///
    /// Returns the compilation state for dispatch decisions.
    /// Every execution is tracked and triggers bytecode compilation at threshold.
    pub fn record_execution(&self, expr: &MettaValue) -> Arc<ExprCompilationState> {
        // Ensure tiered cache roots are registered with GC (idempotent, OnceLock-guarded)
        ensure_tiered_cache_roots_registered();

        // Get or create state for this expression
        let state = self.get_or_create_state(expr);

        // Atomically increment execution count
        let count = state.execution_count.fetch_add(1, Ordering::Relaxed) + 1;

        // Update total execution stats
        #[cfg(feature = "track-stats")]
        self.total_executions.fetch_add(1, Ordering::Relaxed);

        // Check for tier transitions
        self.maybe_trigger_bytecode(expr, &state, count);
        self.maybe_trigger_jit1(&state, count);
        self.maybe_trigger_jit2(&state, count);

        state
    }

    /// Maybe trigger bytecode compilation
    pub(crate) fn maybe_trigger_bytecode(
        &self,
        expr: &MettaValue,
        state: &Arc<ExprCompilationState>,
        count: u32,
    ) {
        // This method is reached from eval, periodic counter sync, and GC
        // count flushing. Register here, not only from record_execution().
        ensure_tiered_cache_roots_registered();

        // Check if we've reached the threshold
        if count < self.bytecode_threshold {
            return;
        }

        // Check if already started
        if state.bytecode_status() != TierStatusKind::NotStarted {
            return;
        }

        // Don't compile expressions that eval_inner would never route to bytecode.
        // Unknown heads (user-defined functions, `_ => false` at mod.rs:528) waste
        // CPU on allocation + compilation the bytecode VM can't execute.
        if !super::can_compile(expr) && !super::can_compile_with_env(expr) {
            state.set_bytecode_failed();
            return;
        }

        // Try to win the race to compile
        if !state.try_start_bytecode_compile() {
            return;
        }

        // Update stats atomically (lock-free)
        #[cfg(feature = "track-stats")]
        self.bytecode_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Capture a shallow source pointer and register it as a root for the
        // queued/running compile task. MettaValue is pointer-like; the root
        // provider must see this source until compile_arc has consumed it.
        let expr_clone = expr.clone();
        let root_guard = self.register_pending_bytecode_root(state.expr_hash, expr_clone);
        let state_clone = Arc::clone(state);

        // Compilation closure
        let compile_task = move || {
            let _root_guard = root_guard;
            match compile_arc("tiered", &expr_clone) {
                Ok(chunk) => {
                    state_clone.set_bytecode_ready(chunk);
                    #[cfg(feature = "track-stats")]
                    global_tiered_cache()
                        .bytecode_compilations_completed
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    state_clone.set_bytecode_failed();
                    #[cfg(feature = "track-stats")]
                    global_tiered_cache()
                        .bytecode_compilations_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        };

        // Dispatch to unified work pool at BACKGROUND_COMPILE priority
        let enqueued = global_compile_pool().spawn_compile(
            compile_task,
            TaskTypeId::BytecodeCompile,
            priority_levels::BACKGROUND_COMPILE,
        );
        if !enqueued {
            // Task dropped due to backpressure — revert state so future
            // executions can re-trigger compilation
            self.pending_bytecode_roots.remove(&state.expr_hash);
            state.revert_bytecode_to_not_started();
            #[cfg(feature = "track-stats")]
            self.bytecode_compilations_triggered
                .fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Pre-seed the tiered cache for an expression at rule insertion time (Phase 4a).
    ///
    /// Seeds the execution counter to just below the bytecode threshold so the
    /// very first real evaluation triggers immediate bytecode compilation. This
    /// eliminates the warmup period for rule RHS templates.
    ///
    /// Takes a content hash rather than the expression itself to work with the
    /// generic `V: MettaValueTrait` types in rule management (the concrete
    /// `MettaValue` is only available at evaluation time). The compilability
    /// check and actual compilation happen in `maybe_trigger_bytecode` when
    /// `record_execution` fires on the first real evaluation.
    pub fn preseed_for_immediate_compile(&self, expr_hash: u64) {
        // Create or get state for this hash
        let state = {
            if let Some(entry) = self.entries.get(&expr_hash) {
                Arc::clone(entry.value())
            } else {
                let new_state = Arc::new(ExprCompilationState::new(expr_hash));
                self.entries.entry(expr_hash).or_insert_with(|| {
                    #[cfg(feature = "track-stats")]
                    self.expressions_tracked.fetch_add(1, Ordering::Relaxed);
                    Arc::clone(&new_state)
                });
                self.entries
                    .get(&expr_hash)
                    .map(|e| Arc::clone(e.value()))
                    .unwrap_or(new_state)
            }
        };

        // Set count to threshold - 1 so next record_execution triggers compilation.
        // CAS loop to avoid overwriting a higher count (e.g., if expression was
        // already evaluated and promoted).
        let target = self.bytecode_threshold.saturating_sub(1);
        let _ = state.execution_count.fetch_max(target, Ordering::Relaxed);
    }

    /// Maybe trigger JIT Stage 1 compilation
    pub(crate) fn maybe_trigger_jit1(&self, state: &Arc<ExprCompilationState>, count: u32) {
        // Check if we've reached the threshold
        if count < self.jit1_threshold {
            return;
        }

        // JIT Stage 1 requires bytecode to be Ready
        if state.bytecode_status() != TierStatusKind::Ready {
            return;
        }

        // Check if already started
        if state.jit1_status() != TierStatusKind::NotStarted {
            return;
        }

        // Try to win the race to compile
        if !state.try_start_jit1_compile() {
            return;
        }

        // Update stats atomically (lock-free)
        #[cfg(feature = "track-stats")]
        self.jit1_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Get the bytecode chunk
        let chunk = match state.bytecode_chunk() {
            Some(c) => c,
            None => {
                state.set_jit1_failed();
                #[cfg(feature = "track-stats")]
                global_tiered_cache()
                    .jit1_compilations_failed
                    .fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        // Phase 8c: Snapshot runtime type profile for the JIT compiler.
        // The profile is collected during bytecode VM execution and captures
        // branch frequencies, type feedback, rule match stats, and guard outcomes.
        let profile_snapshot = {
            let profile = state.runtime_profile.lock();
            profile.snapshot()
        };

        // Clone state for background task
        let state_clone = Arc::clone(state);

        // JIT compilation closure
        let jit_compile = move || {
            // Split nondeterminism check from opcode check for failure reason tracking
            if chunk.has_nondeterminism() {
                if is_jit_debug() {
                    eprintln!(
                        "[JIT1] Rejected (nondeterminism): chunk '{}' len={}",
                        chunk.name(),
                        chunk.len()
                    );
                }
                state_clone.set_jit1_failed();
                #[cfg(feature = "track-stats")]
                {
                    let cache = global_tiered_cache();
                    cache
                        .jit1_failures_nondeterminism
                        .fetch_add(1, Ordering::Relaxed);
                    cache
                        .jit1_compilations_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
                return;
            }

            if !JitCompiler::can_compile_stage1(&chunk) {
                if is_jit_debug() {
                    eprintln!(
                        "[JIT1] Rejected (unsupported opcode): chunk '{}' len={}",
                        chunk.name(),
                        chunk.len()
                    );
                }
                state_clone.set_jit1_failed();
                #[cfg(feature = "track-stats")]
                {
                    let cache = global_tiered_cache();
                    cache
                        .jit1_failures_unsupported_opcode
                        .fetch_add(1, Ordering::Relaxed);
                    cache
                        .jit1_compilations_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
                return;
            }

            // Phase 8c: Profile is available for the JIT compiler to use.
            // Currently passed to compile_with_profile if the profile is mature;
            // otherwise falls back to non-profiled compilation.
            let _profile = profile_snapshot; // Available for future JIT optimizations

            // Compile using the process-persistent JIT compiler. The
            // compiler outlives every cached NativeCode.ptr; dropping
            // it would munmap the executable pages and cause SIGILL on
            // the next dispatch (see global_jit_compiler() doc).
            let compile_result = {
                let mut guard = global_jit_compiler()
                    .lock()
                    .expect("JIT compiler mutex poisoned");
                let compiler = match guard.as_mut() {
                    Some(c) => c,
                    None => match JitCompiler::new() {
                        Ok(c) => {
                            *guard = Some(c);
                            guard.as_mut().expect("just inserted")
                        }
                        Err(e) => {
                            if is_jit_debug() {
                                eprintln!("[JIT1] Compiler init failed: {:?}", e);
                            }
                            state_clone.set_jit1_failed();
                            #[cfg(feature = "track-stats")]
                            {
                                let cache = global_tiered_cache();
                                cache
                                    .jit1_failures_compiler_init
                                    .fetch_add(1, Ordering::Relaxed);
                                cache
                                    .jit1_compilations_failed
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                            return;
                        }
                    },
                };
                compiler.compile(&chunk)
            };

            match compile_result {
                Ok(ptr) => {
                    let code = NativeCode {
                        ptr,
                        code_size: chunk.len() * 8, // Rough estimate
                    };
                    state_clone.set_jit1_ready(Arc::new(code));
                    #[cfg(feature = "track-stats")]
                    global_tiered_cache()
                        .jit1_compilations_completed
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    if is_jit_debug() {
                        eprintln!("[JIT1] Compile failed for '{}': {:?}", chunk.name(), e);
                    }
                    state_clone.set_jit1_failed();
                    #[cfg(feature = "track-stats")]
                    {
                        let cache = global_tiered_cache();
                        cache.jit1_failures_codegen.fetch_add(1, Ordering::Relaxed);
                        cache
                            .jit1_compilations_failed
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        };

        // Choose spawn method based on feature and execution mode
        // Dispatch to unified work pool at BACKGROUND_COMPILE priority
        let enqueued = global_compile_pool().spawn_compile(
            jit_compile,
            TaskTypeId::JitCompile,
            priority_levels::BACKGROUND_COMPILE,
        );
        if !enqueued {
            state.revert_jit1_to_not_started();
            #[cfg(feature = "track-stats")]
            self.jit1_compilations_triggered
                .fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Maybe trigger JIT Stage 2 compilation
    ///
    /// JIT Stage 2 requires BOTH bytecode AND JIT Stage 1 to be Ready.
    /// This ensures JIT1 profile data (type feedback, branch frequencies)
    /// is available before the aggressive optimizing compiler runs.
    /// (V8 equivalent: Turbofan requires Maglev to have run and collected ICs.)
    pub(crate) fn maybe_trigger_jit2(&self, state: &Arc<ExprCompilationState>, count: u32) {
        // Check if we've reached the threshold
        if count < self.jit2_threshold {
            return;
        }

        // JIT Stage 2 requires bytecode to be Ready
        if state.bytecode_status() != TierStatusKind::Ready {
            return;
        }

        // JIT Stage 2 also requires JIT Stage 1 to be Ready (not skippable).
        // This ensures JIT1 has executed and collected runtime profile data
        // that JIT2 can use for speculative optimizations.
        if state.jit1_status() != TierStatusKind::Ready {
            return;
        }

        // Check if already started
        if state.jit2_status() != TierStatusKind::NotStarted {
            return;
        }

        // Try to win the race to compile
        if !state.try_start_jit2_compile() {
            return;
        }

        // Update stats atomically (lock-free)
        #[cfg(feature = "track-stats")]
        self.jit2_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Get the bytecode chunk
        let chunk = match state.bytecode_chunk() {
            Some(c) => c,
            None => {
                state.set_jit2_failed();
                #[cfg(feature = "track-stats")]
                global_tiered_cache()
                    .jit2_compilations_failed
                    .fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        // Phase 8c: Snapshot runtime type profile for JIT Stage 2.
        // By this point, the profile has been refined by JIT1 execution and is
        // more representative than the bytecode-only profile used for JIT1.
        let profile_snapshot = {
            let profile = state.runtime_profile.lock();
            profile.snapshot()
        };

        // Clone state for background task
        let state_clone = Arc::clone(state);

        // JIT Stage 2 compilation closure
        let jit_compile = move || {
            // Phase 8c: Mature profile available for aggressive JIT2 optimizations:
            // - Guard elimination for guards that never fail
            // - Dead branch elimination for never-taken branches
            // - Rule inlining for high-frequency rules
            let _profile = profile_snapshot;

            // Split nondeterminism check from opcode check for failure reason tracking
            if chunk.has_nondeterminism() {
                if is_jit_debug() {
                    eprintln!(
                        "[JIT2] Rejected (nondeterminism): chunk '{}' len={}",
                        chunk.name(),
                        chunk.len()
                    );
                }
                state_clone.set_jit2_failed();
                #[cfg(feature = "track-stats")]
                {
                    let cache = global_tiered_cache();
                    cache
                        .jit2_failures_nondeterminism
                        .fetch_add(1, Ordering::Relaxed);
                    cache
                        .jit2_compilations_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
                return;
            }

            if !JitCompiler::can_compile_stage1(&chunk) {
                if is_jit_debug() {
                    eprintln!(
                        "[JIT2] Rejected (unsupported opcode): chunk '{}' len={}",
                        chunk.name(),
                        chunk.len()
                    );
                }
                state_clone.set_jit2_failed();
                #[cfg(feature = "track-stats")]
                {
                    let cache = global_tiered_cache();
                    cache
                        .jit2_failures_unsupported_opcode
                        .fetch_add(1, Ordering::Relaxed);
                    cache
                        .jit2_compilations_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
                return;
            }

            // Compile using the process-persistent JIT compiler. The
            // compiler outlives every cached NativeCode.ptr; dropping
            // it would munmap the executable pages and cause SIGILL on
            // the next dispatch (see global_jit_compiler() doc). Stage
            // 2 currently reuses the same Cranelift codegen path as
            // Stage 1; tier promotion is driven by `TieredCache`'s
            // hot-expression hit-count threshold, not by distinct
            // codegen passes. Stage-2-specific aggressive inlining is
            // future work tracked by the bytecode/jit perf roadmap.
            let compile_result = {
                let mut guard = global_jit_compiler()
                    .lock()
                    .expect("JIT compiler mutex poisoned");
                let compiler = match guard.as_mut() {
                    Some(c) => c,
                    None => match JitCompiler::new() {
                        Ok(c) => {
                            *guard = Some(c);
                            guard.as_mut().expect("just inserted")
                        }
                        Err(e) => {
                            if is_jit_debug() {
                                eprintln!("[JIT2] Compiler init failed: {:?}", e);
                            }
                            state_clone.set_jit2_failed();
                            #[cfg(feature = "track-stats")]
                            {
                                let cache = global_tiered_cache();
                                cache
                                    .jit2_failures_compiler_init
                                    .fetch_add(1, Ordering::Relaxed);
                                cache
                                    .jit2_compilations_failed
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                            return;
                        }
                    },
                };
                compiler.compile(&chunk)
            };

            match compile_result {
                Ok(ptr) => {
                    let code = NativeCode {
                        ptr,
                        code_size: chunk.len() * 10, // Stage 2 generates more code
                    };
                    state_clone.set_jit2_ready(Arc::new(code));
                    #[cfg(feature = "track-stats")]
                    global_tiered_cache()
                        .jit2_compilations_completed
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    if is_jit_debug() {
                        eprintln!("[JIT2] Compile failed for '{}': {:?}", chunk.name(), e);
                    }
                    state_clone.set_jit2_failed();
                    #[cfg(feature = "track-stats")]
                    {
                        let cache = global_tiered_cache();
                        cache.jit2_failures_codegen.fetch_add(1, Ordering::Relaxed);
                        cache
                            .jit2_compilations_failed
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        };

        // Dispatch to unified work pool at BACKGROUND_COMPILE priority
        let enqueued = global_compile_pool().spawn_compile(
            jit_compile,
            TaskTypeId::JitCompile,
            priority_levels::BACKGROUND_COMPILE,
        );
        if !enqueued {
            state.revert_jit2_to_not_started();
            #[cfg(feature = "track-stats")]
            self.jit2_compilations_triggered
                .fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Get the best available execution tier for an expression
    ///
    /// Returns the highest tier that has Ready status.
    pub fn get_best_tier(&self, expr: &MettaValue) -> ExecutionTier {
        let hash = hash_metta_value(expr);

        if let Some(entry) = self.entries.get(&hash) {
            let state = entry.value();

            // Check from highest to lowest tier
            if state.jit2_status() == TierStatusKind::Ready {
                return ExecutionTier::JitStage2;
            }
            if state.jit1_status() == TierStatusKind::Ready {
                return ExecutionTier::JitStage1;
            }
            if state.bytecode_status() == TierStatusKind::Ready {
                return ExecutionTier::Bytecode;
            }
        }

        // Default to interpreter
        ExecutionTier::Interpreter
    }

    /// Get the compilation state for an expression (if it exists)
    pub fn get_state(&self, expr: &MettaValue) -> Option<Arc<ExprCompilationState>> {
        let hash = hash_metta_value(expr);
        self.entries.get(&hash).map(|e| Arc::clone(e.value()))
    }

    /// Get current cache statistics (builds from atomics, lock-free)
    #[cfg(feature = "track-stats")]
    pub fn stats(&self) -> TieredCacheStats {
        TieredCacheStats {
            expressions_tracked: self.expressions_tracked.load(Ordering::Relaxed),
            total_executions: self.total_executions.load(Ordering::Relaxed),
            bytecode_compilations_triggered: self
                .bytecode_compilations_triggered
                .load(Ordering::Relaxed),
            bytecode_compilations_completed: self
                .bytecode_compilations_completed
                .load(Ordering::Relaxed),
            bytecode_compilations_failed: self.bytecode_compilations_failed.load(Ordering::Relaxed),
            jit1_compilations_triggered: self.jit1_compilations_triggered.load(Ordering::Relaxed),
            jit1_compilations_completed: self.jit1_compilations_completed.load(Ordering::Relaxed),
            jit1_compilations_failed: self.jit1_compilations_failed.load(Ordering::Relaxed),
            jit2_compilations_triggered: self.jit2_compilations_triggered.load(Ordering::Relaxed),
            jit2_compilations_completed: self.jit2_compilations_completed.load(Ordering::Relaxed),
            jit2_compilations_failed: self.jit2_compilations_failed.load(Ordering::Relaxed),
            interpreter_executions: self.interpreter_executions.load(Ordering::Relaxed),
            bytecode_executions: self.bytecode_executions.load(Ordering::Relaxed),
            jit1_executions: self.jit1_executions.load(Ordering::Relaxed),
            jit2_executions: self.jit2_executions.load(Ordering::Relaxed),
            jit1_failures_nondeterminism: self.jit1_failures_nondeterminism.load(Ordering::Relaxed),
            jit1_failures_unsupported_opcode: self
                .jit1_failures_unsupported_opcode
                .load(Ordering::Relaxed),
            jit1_failures_compiler_init: self.jit1_failures_compiler_init.load(Ordering::Relaxed),
            jit1_failures_codegen: self.jit1_failures_codegen.load(Ordering::Relaxed),
            jit2_failures_nondeterminism: self.jit2_failures_nondeterminism.load(Ordering::Relaxed),
            jit2_failures_unsupported_opcode: self
                .jit2_failures_unsupported_opcode
                .load(Ordering::Relaxed),
            jit2_failures_compiler_init: self.jit2_failures_compiler_init.load(Ordering::Relaxed),
            jit2_failures_codegen: self.jit2_failures_codegen.load(Ordering::Relaxed),
        }
    }

    /// Snapshot per-expression stats for diagnostics (read-only, called only at `--tier-stats` print time).
    ///
    /// Iterates the existing `DashMap` entries — no new per-expression tracking overhead.
    /// Returns entries sorted by execution count descending.
    #[cfg(feature = "track-stats")]
    pub fn per_expression_stats(&self) -> Vec<PerExpressionStats> {
        let mut stats: Vec<PerExpressionStats> = self
            .entries
            .iter()
            .map(|entry| {
                let state = entry.value();
                PerExpressionStats {
                    expr_hash: state.expr_hash,
                    execution_count: state.execution_count.load(Ordering::Relaxed),
                    bytecode_status: state.bytecode_status(),
                    jit1_status: state.jit1_status(),
                    jit2_status: state.jit2_status(),
                }
            })
            .collect();
        stats.sort_by(|a, b| b.execution_count.cmp(&a.execution_count));
        stats
    }

    /// Reset statistics (lock-free via atomic stores)
    #[cfg(feature = "track-stats")]
    pub fn reset_stats(&self) {
        self.expressions_tracked.store(0, Ordering::Relaxed);
        self.total_executions.store(0, Ordering::Relaxed);
        self.bytecode_compilations_triggered
            .store(0, Ordering::Relaxed);
        self.bytecode_compilations_completed
            .store(0, Ordering::Relaxed);
        self.bytecode_compilations_failed
            .store(0, Ordering::Relaxed);
        self.jit1_compilations_triggered.store(0, Ordering::Relaxed);
        self.jit1_compilations_completed.store(0, Ordering::Relaxed);
        self.jit1_compilations_failed.store(0, Ordering::Relaxed);
        self.jit2_compilations_triggered.store(0, Ordering::Relaxed);
        self.jit2_compilations_completed.store(0, Ordering::Relaxed);
        self.jit2_compilations_failed.store(0, Ordering::Relaxed);
        self.interpreter_executions.store(0, Ordering::Relaxed);
        self.bytecode_executions.store(0, Ordering::Relaxed);
        self.jit1_executions.store(0, Ordering::Relaxed);
        self.jit2_executions.store(0, Ordering::Relaxed);
        self.jit1_failures_nondeterminism
            .store(0, Ordering::Relaxed);
        self.jit1_failures_unsupported_opcode
            .store(0, Ordering::Relaxed);
        self.jit1_failures_compiler_init.store(0, Ordering::Relaxed);
        self.jit1_failures_codegen.store(0, Ordering::Relaxed);
        self.jit2_failures_nondeterminism
            .store(0, Ordering::Relaxed);
        self.jit2_failures_unsupported_opcode
            .store(0, Ordering::Relaxed);
        self.jit2_failures_compiler_init.store(0, Ordering::Relaxed);
        self.jit2_failures_codegen.store(0, Ordering::Relaxed);
    }

    /// Clear the entire cache
    pub fn clear(&self) {
        self.entries.clear();
        self.pending_bytecode_roots.clear();
        #[cfg(feature = "track-stats")]
        self.reset_stats();
    }

    /// Get the number of expressions being tracked
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record an execution at a specific tier (for statistics, lock-free)
    #[cfg(feature = "track-stats")]
    pub fn record_tier_execution(&self, tier: ExecutionTier) {
        match tier {
            ExecutionTier::Interpreter => {
                self.interpreter_executions.fetch_add(1, Ordering::Relaxed);
            }
            ExecutionTier::Bytecode => {
                self.bytecode_executions.fetch_add(1, Ordering::Relaxed);
            }
            ExecutionTier::JitStage1 => {
                self.jit1_executions.fetch_add(1, Ordering::Relaxed);
            }
            ExecutionTier::JitStage2 => {
                self.jit2_executions.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Default for TieredCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global tiered compilation cache
///
/// Shared across all evaluations for optimal reuse of compiled code.
static GLOBAL_TIERED_CACHE: std::sync::LazyLock<TieredCache> = std::sync::LazyLock::new(|| {
    #[cfg(feature = "track-stats")]
    maybe_register_jit_summary();
    TieredCache::new()
});

/// Get a reference to the global tiered compilation cache.
pub fn global_tiered_cache() -> &'static TieredCache {
    &GLOBAL_TIERED_CACHE
}

/// Register an atexit hook that prints a JIT summary when `METTATRON_JIT_DEBUG` is set.
/// Called once on first access to the global tiered cache (idempotent via OnceLock).
#[cfg(feature = "track-stats")]
static JIT_SUMMARY_REGISTERED: OnceLock<()> = OnceLock::new();

#[cfg(feature = "track-stats")]
fn maybe_register_jit_summary() {
    if !is_jit_debug() {
        return;
    }
    JIT_SUMMARY_REGISTERED.get_or_init(|| {
        extern "C" fn jit_summary_atexit() {
            let stats = global_tiered_cache().stats();
            eprintln!(
                "[JIT Summary] tracked={} bytecode={}/{}/{} jit1={}/{}/{} jit2={}/{}/{} (ok/fail/pending)",
                stats.expressions_tracked,
                stats.bytecode_compilations_completed,
                stats.bytecode_compilations_failed,
                stats.bytecode_compilations_triggered.saturating_sub(
                    stats.bytecode_compilations_completed + stats.bytecode_compilations_failed
                ),
                stats.jit1_compilations_completed,
                stats.jit1_compilations_failed,
                stats.jit1_compilations_triggered.saturating_sub(
                    stats.jit1_compilations_completed + stats.jit1_compilations_failed
                ),
                stats.jit2_compilations_completed,
                stats.jit2_compilations_failed,
                stats.jit2_compilations_triggered.saturating_sub(
                    stats.jit2_compilations_completed + stats.jit2_compilations_failed
                ),
            );
        }
        // SAFETY: jit_summary_atexit is an extern "C" fn with no parameters,
        // which is the required signature for libc::atexit.
        unsafe {
            libc::atexit(jit_summary_atexit);
        }
    });
}

// =============================================================================
// GC Root Provider for GLOBAL_TIERED_CACHE
// =============================================================================

/// GC root provider that exposes all MettaValue constants stored in the
/// tiered compilation cache's bytecode chunks to the garbage collector.
///
/// Without this, constants in cached bytecode chunks are invisible to GC and
/// may be freed while still reachable from the cache, causing use-after-free.
struct TieredCacheRoots;

impl RootProvider for TieredCacheRoots {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        global_tiered_cache().collect_roots_into(roots);
    }
}

/// Keeps the Arc<dyn RootProvider> alive for the lifetime of the process so
/// the Weak reference in ROOT_REGISTRY remains valid.
static TIERED_CACHE_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();

/// Ensure the tiered cache is registered as a GC root provider.
///
/// Called lazily on first `record_execution`. Idempotent — OnceLock
/// guarantees single initialization.
pub fn ensure_tiered_cache_roots_registered() {
    TIERED_CACHE_ROOT_PROVIDER.get_or_init(|| {
        let provider = Arc::new(TieredCacheRoots) as Arc<dyn RootProvider>;
        register_root_provider(&provider);
        provider
    });
}

// =============================================================================
// Arena Value Hashing
// =============================================================================

/// Hash an MettaValue for cache lookup.
///
/// Uses the same hashing strategy as MettaValue (FxHash-style mixing for primitives,
/// xxHash3 for complex types) to ensure consistent and efficient lookups.
pub fn hash_value(expr: &MettaValue) -> u64 {
    // Golden ratio constant for good hash distribution
    const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
    // Type-specific seeds
    const LONG_SEED: u64 = 0x517cc1b727220a95;
    const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
    const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;
    const UNIT_HASH: u64 = 0x756e6974_68617368; // "unit_hash" as bytes

    // Fast path for primitives, slow path for complex types
    match expr.view() {
        ValueView::Unit => UNIT_HASH,
        ValueView::Bool(b) => {
            if b {
                BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
            } else {
                BOOL_SEED
            }
        }
        ValueView::Long(n) => {
            let x = (n as u64)
                .wrapping_add(LONG_SEED)
                .wrapping_mul(GOLDEN_RATIO);
            x ^ (x >> 32)
        }
        ValueView::Float(f) => {
            let bits = f.to_bits();
            let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
            x ^ (x >> 32)
        }
        ValueView::Empty => {
            // Use a distinct seed for Empty
            0x656d7074_79686173 // "empty_has" as bytes
        }
        ValueView::NotReducible => {
            // Plan S0a (2026-05-13) — distinct seed for NotReducible sentinel.
            0x6e72_6564_7563_6962 // "nreducib" as bytes
        }
        ValueView::Atom(_)
        | ValueView::String(_)
        | ValueView::SExpr(_)
        | ValueView::Error(..)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_)
        | ValueView::Lazy(_) => {
            let mut hasher = Xxh3::new();
            hash_value_recursive(expr, &mut hasher);
            hasher.finish()
        }
    }
}

/// **Stack-safety mandate (2026-05-15)**: refactored to iterative work-list
/// (audit item T#19). Was recursive on SExpr / Quoted children — deeply-nested
/// values would overflow. No memoization needed: this is hash-streaming, not
/// string-building, so shared substructure doesn't cause exponential output —
/// just re-hashes the same bytes which is fine.
fn hash_value_recursive<H: std::hash::Hasher>(expr: &MettaValue, hasher: &mut H) {
    let mut work: Vec<MettaValue> = Vec::with_capacity(8);
    work.push(expr.clone());
    while let Some(val) = work.pop() {
        match val.view() {
            ValueView::Unit => 0u8.hash(hasher),
            ValueView::Bool(b) => {
                2u8.hash(hasher);
                b.hash(hasher);
            }
            ValueView::Long(n) => {
                3u8.hash(hasher);
                n.hash(hasher);
            }
            ValueView::Float(f) => {
                4u8.hash(hasher);
                f.to_bits().hash(hasher);
            }
            ValueView::Empty => 9u8.hash(hasher),
            ValueView::NotReducible => 1u8.hash(hasher),
            ValueView::String(s) => {
                5u8.hash(hasher);
                s.hash(hasher);
            }
            ValueView::Atom(s) => {
                6u8.hash(hasher);
                s.hash(hasher);
            }
            ValueView::SExpr(items) => {
                7u8.hash(hasher);
                items.len().hash(hasher);
                // Push children in reverse so the first child is hashed first.
                for item in items.iter().rev() {
                    work.push(item.clone());
                }
            }
            ValueView::Error(..) => 8u8.hash(hasher),
            ValueView::Quoted(inner) => {
                10u8.hash(hasher);
                "quote".hash(hasher);
                work.push(inner);
            }
            // PT-canonical Lazy is INVISIBLE for hashing — recurse into the
            // wrapped value WITHOUT emitting a discriminator tag, so
            // hash(Lazy(x)) == hash(x).
            ValueView::Lazy(inner) => {
                work.push(inner);
            }
            ValueView::Type(_)
            | ValueView::Conjunction(_)
            | ValueView::Space(_)
            | ValueView::State(_)
            | ValueView::Memo(_) => 10u8.hash(hasher),
        }
    }
}

// =============================================================================
// Sub-Expression Dispatch
// =============================================================================

/// Attempt to dispatch a sub-expression to its highest compiled tier.
///
/// Reads the cached compilation hash from the slab slot, looks up the
/// `ExprCompilationState` in the global `TieredCache`, and dispatches to
/// JIT Stage 2 > JIT Stage 1 > Bytecode VM if ready.
///
/// Returns `Some((results, new_env))` on successful dispatch, `None` if
/// no compiled code is available or execution fails (falls back to tree-walker).
///
/// # Safety
/// `ptr` must point to a live, slab-allocated `MettaValueInner`.
pub fn try_sub_expr_dispatch(
    ptr: *const MettaValueInner,
    _value: &MettaValue,
    env: &MettaEnvironment,
) -> Option<(Vec<MettaValue>, MettaEnvironment)> {
    let hash = get_slot_compilation_hash(ptr);
    if hash == 0 {
        return None;
    }

    let cache = global_tiered_cache();
    let state_ref = cache.entries.get(&hash)?;
    let state = std::sync::Arc::clone(state_ref.value());
    drop(state_ref); // release DashMap read guard

    // Cascade: JIT Stage 2 > JIT Stage 1 > Bytecode VM
    // env.clone() is deferred to here — only when a tier actually dispatches.
    if state.jit2_status() == TierStatusKind::Ready {
        if let Some(code) = state.jit2_code() {
            if let Ok((results, new_env)) = dispatch_jit(&state, code.ptr, env.clone()) {
                #[cfg(feature = "track-stats")]
                cache.record_tier_execution(ExecutionTier::JitStage2);
                return Some((results, new_env));
            }
        }
    }
    if state.jit1_status() == TierStatusKind::Ready {
        if let Some(code) = state.jit1_code() {
            if let Ok((results, new_env)) = dispatch_jit(&state, code.ptr, env.clone()) {
                #[cfg(feature = "track-stats")]
                cache.record_tier_execution(ExecutionTier::JitStage1);
                return Some((results, new_env));
            }
        }
    }
    if state.bytecode_status() == TierStatusKind::Ready {
        if let Some(chunk) = state.bytecode_chunk() {
            match super::execute_arena(chunk, env.clone()) {
                Ok((results, new_env, unreduced)) if !unreduced => {
                    #[cfg(feature = "track-stats")]
                    cache.record_tier_execution(ExecutionTier::Bytecode);
                    return Some((results, new_env));
                }
                _ => {}
            }
        }
    }
    None
}

/// Attempt to dispatch a sub-expression using a pre-computed compilation hash.
///
/// Like `try_sub_expr_dispatch`, but skips the `get_slot_compilation_hash` call
/// since the hash was already obtained by `increment_and_get_hash`. The caller
/// guarantees `hash != 0`.
///
/// Returns `Some((results, new_env))` on successful dispatch, `None` if
/// no compiled code is available or execution fails (falls back to tree-walker).
pub fn try_sub_expr_dispatch_with_hash(
    hash: u64,
    _value: &MettaValue,
    env: &MettaEnvironment,
) -> Option<(Vec<MettaValue>, MettaEnvironment)> {
    // hash is already known non-zero (checked by caller)
    let cache = global_tiered_cache();
    let state_ref = cache.entries.get(&hash)?;
    let state = std::sync::Arc::clone(state_ref.value());
    drop(state_ref); // release DashMap read guard

    // Cascade: JIT Stage 2 > JIT Stage 1 > Bytecode VM
    // env.clone() deferred to dispatch site — only when a tier is actually invoked.
    if state.jit2_status() == TierStatusKind::Ready {
        if let Some(code) = state.jit2_code() {
            if let Ok((results, new_env)) = dispatch_jit(&state, code.ptr, env.clone()) {
                #[cfg(feature = "track-stats")]
                cache.record_tier_execution(ExecutionTier::JitStage2);
                return Some((results, new_env));
            }
        }
    }
    if state.jit1_status() == TierStatusKind::Ready {
        if let Some(code) = state.jit1_code() {
            if let Ok((results, new_env)) = dispatch_jit(&state, code.ptr, env.clone()) {
                #[cfg(feature = "track-stats")]
                cache.record_tier_execution(ExecutionTier::JitStage1);
                return Some((results, new_env));
            }
        }
    }
    if state.bytecode_status() == TierStatusKind::Ready {
        if let Some(chunk) = state.bytecode_chunk() {
            match super::execute_arena(chunk, env.clone()) {
                Ok((results, new_env, unreduced)) if !unreduced => {
                    #[cfg(feature = "track-stats")]
                    cache.record_tier_execution(ExecutionTier::Bytecode);
                    return Some((results, new_env));
                }
                _ => {}
            }
        }
    }
    None
}

/// Attempt to dispatch a closed, pure user-defined sub-expression through the
/// cached environment-aware bytecode tier.
///
/// This path is intentionally narrower than `try_sub_expr_dispatch_with_hash`:
/// it only uses bytecode, exhausts the VM's nondeterministic top-level results,
/// and rejects executions that leave unreduced values or pending choices. That
/// keeps recursive helper calls eligible for warmup without changing observable
/// branch behavior.
pub fn try_sub_expr_env_dispatch_with_hash(
    hash: u64,
    _value: &MettaValue,
    env: &MettaEnvironment,
) -> Option<(Vec<MettaValue>, MettaEnvironment)> {
    let cache = global_tiered_cache();
    let state_ref = cache.entries.get(&hash)?;
    let state = std::sync::Arc::clone(state_ref.value());
    drop(state_ref);

    if state.bytecode_status() != TierStatusKind::Ready {
        return None;
    }

    let chunk = state.bytecode_chunk()?;
    let factory = env.factory().clone();
    let mut vm =
        super::GenericBytecodeVM::with_env_and_factory(chunk, env.clone(), factory.clone());
    vm.yield_on_top_return = true;

    let results = vm.run().ok()?;
    if vm.unreduced || vm.had_unreduced_result || vm.choice_points_len() > 0 {
        return None;
    }

    let final_env = vm
        .env
        .take()
        .unwrap_or_else(|| MettaEnvironment::new(factory));
    Some((results, final_env))
}

/// Helper: dispatch to JIT-compiled native code with environment.
fn dispatch_jit(
    state: &std::sync::Arc<ExprCompilationState>,
    native_ptr: *const (),
    env: MettaEnvironment,
) -> Result<(Vec<MettaValue>, MettaEnvironment), ()> {
    use super::jit::HybridExecutor;
    use crate::backend::models::{global_allocator, global_factory};
    let allocator = global_allocator();
    let factory = global_factory();
    let chunk = state.bytecode_chunk().ok_or(())?;
    let mut executor = HybridExecutor::new();
    executor
        .execute_jit_arena_with_env(&chunk, native_ptr, allocator, &factory, env)
        .map_err(|_| ())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;

    use crate::backend::models::{global_factory, MettaValueFactory};

    #[test]
    fn test_tier_status_kind_conversion() {
        assert_eq!(TierStatusKind::from(0), TierStatusKind::NotStarted);
        assert_eq!(TierStatusKind::from(1), TierStatusKind::Compiling);
        assert_eq!(TierStatusKind::from(2), TierStatusKind::Ready);
        assert_eq!(TierStatusKind::from(3), TierStatusKind::Failed);
        assert_eq!(TierStatusKind::from(255), TierStatusKind::NotStarted);
    }

    #[test]
    fn test_execution_tier_ordering() {
        assert!(ExecutionTier::Interpreter < ExecutionTier::Bytecode);
        assert!(ExecutionTier::Bytecode < ExecutionTier::JitStage1);
        assert!(ExecutionTier::JitStage1 < ExecutionTier::JitStage2);
    }

    #[test]
    fn test_expr_compilation_state_new() {
        let state = ExprCompilationState::new(12345);
        assert_eq!(state.count(), 0);
        assert_eq!(state.bytecode_status(), TierStatusKind::NotStarted);
        assert_eq!(state.jit1_status(), TierStatusKind::NotStarted);
        assert_eq!(state.jit2_status(), TierStatusKind::NotStarted);
        assert!(state.bytecode_chunk().is_none());
        assert!(state.jit1_code().is_none());
        assert!(state.jit2_code().is_none());
    }

    #[test]
    fn test_expr_compilation_state_try_start_compile() {
        let state = ExprCompilationState::new(12345);

        // First attempt should succeed
        assert!(state.try_start_bytecode_compile());
        assert_eq!(state.bytecode_status(), TierStatusKind::Compiling);

        // Second attempt should fail
        assert!(!state.try_start_bytecode_compile());
        assert_eq!(state.bytecode_status(), TierStatusKind::Compiling);
    }

    #[test]
    fn test_tiered_cache_new() {
        let cache = TieredCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.bytecode_threshold, BYTECODE_THRESHOLD);
        assert_eq!(cache.jit1_threshold, JIT1_THRESHOLD);
        assert_eq!(cache.jit2_threshold, JIT2_THRESHOLD);
    }

    #[test]
    fn test_pending_bytecode_root_is_collected_and_removed() {
        let cache = TieredCache::new();
        let factory = global_factory();
        let expr = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        let hash = hash_metta_value(&expr);

        let guard = cache.register_pending_bytecode_root(hash, expr);
        let mut roots = Vec::new();
        cache.collect_roots_into(&mut roots);
        assert!(
            roots.contains(&expr),
            "pending bytecode source must be visible to GC root collection"
        );

        drop(guard);
        roots.clear();
        cache.collect_roots_into(&mut roots);
        assert!(
            !roots.contains(&expr),
            "pending bytecode source root must be removed when the task lifetime ends"
        );
    }

    #[test]
    fn test_tiered_cache_clear_removes_pending_bytecode_roots() {
        let cache = TieredCache::new();
        let factory = global_factory();
        let expr = factory.sexpr(vec![factory.atom("*"), factory.long(3), factory.long(4)]);
        let hash = hash_metta_value(&expr);

        let _guard = cache.register_pending_bytecode_root(hash, expr);
        cache.clear();

        let mut roots = Vec::new();
        cache.collect_roots_into(&mut roots);
        assert!(
            !roots.contains(&expr),
            "clearing the tiered cache must also clear pending bytecode roots"
        );
    }

    #[test]
    fn test_tiered_cache_record_execution() {
        // Create cache with default settings
        let cache = TieredCache::new();
        let expr = MettaValue::Long(42);

        // record_execution always returns state directly (simplified API)
        let state = cache.record_execution(&expr);

        // After 1 call, count should be 1
        assert_eq!(state.count(), 1);

        // Call again to verify count increases
        let _ = cache.record_execution(&expr);
        assert_eq!(state.count(), 2);

        // Call more times
        for _ in 0..8 {
            let _ = cache.record_execution(&expr);
        }
        assert_eq!(state.count(), 10);

        // Cache should have one entry
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_tiered_cache_multiple_expressions() {
        // Test that different expressions get different states
        let cache = TieredCache::new();
        let expr1 = MettaValue::Long(42);
        let expr2 = MettaValue::Long(43);

        // Record first expression
        let state1 = cache.record_execution(&expr1);
        assert_eq!(state1.count(), 1);

        // Record second expression
        let state2 = cache.record_execution(&expr2);
        assert_eq!(state2.count(), 1);

        // Record first expression again
        let state1_again = cache.record_execution(&expr1);
        assert_eq!(state1_again.count(), 2);

        // Second expression should still have count 1
        let state2_check = cache.record_execution(&expr2);
        assert_eq!(state2_check.count(), 2);

        // Cache should have two entries
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_tiered_cache_get_best_tier() {
        let cache = TieredCache::new();
        let expr = MettaValue::Long(42);

        // Before any execution, should be interpreter
        assert_eq!(cache.get_best_tier(&expr), ExecutionTier::Interpreter);

        // After executions, compilation is spawned in the background.
        // The best tier is either Interpreter (if async compile hasn't finished)
        // or Bytecode (if the background compile task completed before we check).
        for _ in 0..10 {
            let _ = cache.record_execution(&expr);
        }
        let tier = cache.get_best_tier(&expr);
        assert!(
            tier == ExecutionTier::Interpreter || tier == ExecutionTier::Bytecode,
            "expected Interpreter or Bytecode, got {:?}",
            tier
        );
    }

    #[test]
    fn test_tiered_cache_custom_thresholds() {
        let cache = TieredCache::with_thresholds(5, 50, 200);
        assert_eq!(cache.bytecode_threshold, 5);
        assert_eq!(cache.jit1_threshold, 50);
        assert_eq!(cache.jit2_threshold, 200);
    }

    #[test]
    fn test_tiered_cache_clear() {
        let cache = TieredCache::new();
        let expr = MettaValue::Long(42);

        let _ = cache.record_execution(&expr);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_native_code_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        assert_send::<NativeCode>();
        assert_sync::<NativeCode>();
    }

    #[test]
    #[cfg(feature = "track-stats")]
    fn test_record_tier_execution() {
        let cache = TieredCache::new();

        cache.record_tier_execution(ExecutionTier::Interpreter);
        cache.record_tier_execution(ExecutionTier::Bytecode);
        cache.record_tier_execution(ExecutionTier::JitStage1);
        cache.record_tier_execution(ExecutionTier::JitStage2);

        let stats = cache.stats();
        assert_eq!(stats.interpreter_executions, 1);
        assert_eq!(stats.bytecode_executions, 1);
        assert_eq!(stats.jit1_executions, 1);
        assert_eq!(stats.jit2_executions, 1);
    }

    // ========================================================================
    // Branch Coverage Tests
    // ========================================================================

    #[test]
    fn test_tier_status_kind_invalid_value() {
        // Test that invalid u8 values default to NotStarted
        assert_eq!(TierStatusKind::from(4), TierStatusKind::NotStarted);
        assert_eq!(TierStatusKind::from(100), TierStatusKind::NotStarted);
        assert_eq!(TierStatusKind::from(254), TierStatusKind::NotStarted);
    }

    #[test]
    fn test_expr_compilation_state_set_failed() {
        let state = ExprCompilationState::new(12345);

        // Mark bytecode as failed
        state.set_bytecode_failed();
        assert_eq!(state.bytecode_status(), TierStatusKind::Failed);
        assert!(state.bytecode_chunk().is_none());

        // Try to compile again should fail (already Failed, not NotStarted)
        assert!(!state.try_start_bytecode_compile());
    }

    #[test]
    fn test_expr_compilation_state_jit1_transitions() {
        let state = ExprCompilationState::new(12345);

        // Initially NotStarted
        assert_eq!(state.jit1_status(), TierStatusKind::NotStarted);
        assert!(state.jit1_code().is_none());

        // Start JIT1 compilation
        assert!(state.try_start_jit1_compile());
        assert_eq!(state.jit1_status(), TierStatusKind::Compiling);

        // Second attempt should fail
        assert!(!state.try_start_jit1_compile());

        // Set failed
        state.set_jit1_failed();
        assert_eq!(state.jit1_status(), TierStatusKind::Failed);
    }

    #[test]
    fn test_expr_compilation_state_jit2_transitions() {
        let state = ExprCompilationState::new(12345);

        // Initially NotStarted
        assert_eq!(state.jit2_status(), TierStatusKind::NotStarted);
        assert!(state.jit2_code().is_none());

        // Start JIT2 compilation
        assert!(state.try_start_jit2_compile());
        assert_eq!(state.jit2_status(), TierStatusKind::Compiling);

        // Second attempt should fail
        assert!(!state.try_start_jit2_compile());

        // Set failed
        state.set_jit2_failed();
        assert_eq!(state.jit2_status(), TierStatusKind::Failed);
    }

    #[test]
    fn test_tiered_cache_v8_aligned_default_thresholds() {
        // No global warm-up period — per V8 best practice.
        // Each expression is tracked from first invocation.
        let cache = TieredCache::new();
        assert_eq!(cache.bytecode_threshold, 5); // Ignition→Sparkplug (raised from 1)
        assert_eq!(cache.jit1_threshold, 200); // Sparkplug→Maglev
        assert_eq!(cache.jit2_threshold, 2_000); // Maglev→Turbofan
    }

    #[test]
    #[cfg(feature = "track-stats")]
    fn test_tiered_cache_stats_reset() {
        let cache = TieredCache::new();
        let expr = MettaValue::Long(42);

        // Record some executions
        for _ in 0..5 {
            let _ = cache.record_execution(&expr);
        }
        cache.record_tier_execution(ExecutionTier::Interpreter);

        let stats = cache.stats();
        assert!(stats.total_executions >= 5);
        assert!(stats.interpreter_executions >= 1);

        // Reset stats
        cache.reset_stats();
        let stats2 = cache.stats();
        assert_eq!(stats2.total_executions, 0);
        assert_eq!(stats2.interpreter_executions, 0);
    }

    #[test]
    fn test_tiered_cache_get_state_nonexistent() {
        let cache = TieredCache::new();
        let expr = MettaValue::Long(42);

        // Before recording, state should not exist
        assert!(cache.get_state(&expr).is_none());

        // After recording, state should exist
        let _ = cache.record_execution(&expr);
        assert!(cache.get_state(&expr).is_some());
    }

    #[test]
    fn test_tiered_cache_complex_expression() {
        let cache = TieredCache::new();

        // Test with an S-expression
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let state = cache.record_execution(&expr);
        assert_eq!(state.count(), 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_arena_expr_compilation_state_new() {
        let state = ExprCompilationState::new(99999);
        assert_eq!(state.count(), 0);
        assert_eq!(state.bytecode_status(), TierStatusKind::NotStarted);
        assert_eq!(state.jit1_status(), TierStatusKind::NotStarted);
        assert_eq!(state.jit2_status(), TierStatusKind::NotStarted);
        assert!(state.bytecode_chunk().is_none());
        assert!(state.jit1_code().is_none());
        assert!(state.jit2_code().is_none());
    }

    #[test]
    fn test_arena_expr_compilation_state_bytecode_transitions() {
        let state = ExprCompilationState::new(12345);

        // Start compilation
        assert!(state.try_start_bytecode_compile());
        assert_eq!(state.bytecode_status(), TierStatusKind::Compiling);

        // Second attempt fails
        assert!(!state.try_start_bytecode_compile());

        // Set failed
        state.set_bytecode_failed();
        assert_eq!(state.bytecode_status(), TierStatusKind::Failed);
    }

    #[test]
    fn test_arena_expr_compilation_state_jit_transitions() {
        let state = ExprCompilationState::new(12345);

        // JIT1 transitions
        assert!(state.try_start_jit1_compile());
        assert_eq!(state.jit1_status(), TierStatusKind::Compiling);
        assert!(!state.try_start_jit1_compile());
        state.set_jit1_failed();
        assert_eq!(state.jit1_status(), TierStatusKind::Failed);

        // JIT2 transitions
        assert!(state.try_start_jit2_compile());
        assert_eq!(state.jit2_status(), TierStatusKind::Compiling);
        assert!(!state.try_start_jit2_compile());
        state.set_jit2_failed();
        assert_eq!(state.jit2_status(), TierStatusKind::Failed);
    }

    #[test]
    fn test_hash_value_primitives() {
        let factory = global_factory();

        // Test that hashing primitives produces consistent results
        let nil_val: MettaValue = factory.unit();
        let unit_val: MettaValue = factory.unit();
        let true_val: MettaValue = factory.bool(true);
        let false_val: MettaValue = factory.bool(false);
        let long_val: MettaValue = factory.long(42);
        let float_val: MettaValue = factory.float(3.14);

        let nil_hash = hash_value(&nil_val);
        let unit_hash = hash_value(&unit_val);
        let true_hash = hash_value(&true_val);
        let false_hash = hash_value(&false_val);
        let long_hash = hash_value(&long_val);
        let float_hash = hash_value(&float_val);

        // After Nil/Unit merge, nil and unit produce the same hash
        assert_eq!(nil_hash, unit_hash);
        // Different types should produce different hashes
        assert_ne!(true_hash, false_hash);
        assert_ne!(long_hash, float_hash);
        assert_ne!(unit_hash, long_hash);
    }

    #[test]
    fn test_hash_value_strings() {
        let factory = global_factory();

        // Test hashing strings and atoms
        let string_val: MettaValue = factory.string("hello");
        let atom_val: MettaValue = factory.atom("hello");

        let string_hash = hash_value(&string_val);
        let atom_hash = hash_value(&atom_val);

        // Same content but different types should produce different hashes
        assert_ne!(string_hash, atom_hash);
    }

    #[test]
    fn test_native_code_debug() {
        let code = NativeCode {
            ptr: ptr::null(),
            code_size: 100,
        };
        let debug_str = format!("{:?}", code);
        assert!(debug_str.contains("NativeCode"));
        assert!(debug_str.contains("100"));
    }

    #[test]
    fn test_expr_compilation_state_debug() {
        let state = ExprCompilationState::new(12345);
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("ExprCompilationState"));
        assert!(debug_str.contains("execution_count"));
        assert!(debug_str.contains("12345"));
    }
}
