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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

// Thread-local sampling counter to avoid atomic contention on the global counter
// Each thread tracks its own count and only samples every SAMPLE_RATE evals
thread_local! {
    static THREAD_LOCAL_COUNTER: Cell<u64> = const { Cell::new(0) };
}

// Sequential mode detection - only needed with hybrid-p2-priority-scheduler
#[cfg(feature = "hybrid-p2-priority-scheduler")]
mod sequential_mode {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Global counter for concurrent eval operations
    ///
    /// Tracks how many eval operations are currently in-flight across all threads.
    /// When count is low (< SEQUENTIAL_THRESHOLD), we use Rayon's lightweight spawn
    /// instead of the P2 priority scheduler to avoid scheduler overhead (~500-1000ns per spawn).
    static CONCURRENT_EVALS: AtomicUsize = AtomicUsize::new(0);

    /// Threshold for detecting sequential execution mode
    ///
    /// If fewer than this many evals are in-flight, we're likely running sequentially
    /// and should use Rayon's spawn instead of P2 scheduler for lower overhead.
    const SEQUENTIAL_THRESHOLD: usize = 2;

    /// Check if we're in sequential execution mode
    ///
    /// Returns true if fewer than SEQUENTIAL_THRESHOLD evals are in-flight,
    /// indicating we should use Rayon's lightweight spawn for background work.
    #[inline]
    pub fn is_sequential_mode() -> bool {
        CONCURRENT_EVALS.load(Ordering::Relaxed) < SEQUENTIAL_THRESHOLD
    }

    /// Enter an eval operation (increment concurrent counter)
    ///
    /// Call this at the start of eval() to track concurrent evaluations.
    #[inline]
    pub fn enter_eval() {
        CONCURRENT_EVALS.fetch_add(1, Ordering::Relaxed);
    }

    /// Exit an eval operation (decrement concurrent counter)
    ///
    /// Call this at the end of eval() to track concurrent evaluations.
    #[inline]
    pub fn exit_eval() {
        CONCURRENT_EVALS.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(feature = "hybrid-p2-priority-scheduler")]
pub use sequential_mode::{enter_eval, exit_eval, is_sequential_mode};

use dashmap::DashMap;

use crate::backend::models::MettaValue;

#[cfg(feature = "hybrid-p2-priority-scheduler")]
use crate::backend::priority_scheduler::{global_priority_eval_pool, priority_levels, TaskTypeId};

use super::cache::hash_metta_value;
use super::chunk::BytecodeChunk;
use super::compiler::compile_arc;

/// Threshold to trigger bytecode compilation (eager: after 1st execution)
/// Compilation is non-blocking (rayon background), so eager compilation
/// provides faster VM execution with minimal overhead.
pub const BYTECODE_THRESHOLD: u32 = 1;

/// Threshold to trigger JIT Stage 1 compilation (after 100 executions)
pub const JIT1_THRESHOLD: u32 = 100;

/// Threshold to trigger JIT Stage 2 compilation (after 500 executions)
pub const JIT2_THRESHOLD: u32 = 500;

/// Default warm-up threshold - skip tiered compilation tracking until this many total evaluations
///
/// Set to 1000 to skip overhead for small workloads while still benefiting hot code.
/// This addresses the parallel-4 regression where tiered cache overhead was higher
/// than the benefit for short benchmark runs.
pub const DEFAULT_WARMUP_THRESHOLD: u64 = 1000;

/// Sampling rate for expression tracking - track 1 in N evaluations
///
/// After warm-up, only every Nth eval is tracked to reduce hash computation
/// and DashMap lookup overhead. When an eval is sampled, execution count is
/// incremented by SAMPLE_RATE to maintain correct threshold triggering.
///
/// Set to 32 to reduce tracking overhead by ~97% while still triggering
/// compilation for expressions executed 32+ times (with 32x slower convergence).
pub const SAMPLE_RATE: u64 = 32;

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
#[derive(Clone)]
pub struct NativeCode {
    /// Pointer to JIT-compiled native code
    pub ptr: *const (),
    /// Size of the generated native code in bytes
    pub code_size: usize,
}

// Safety: Native code pointers are safe to send between threads
// as they point to read-only executable memory
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

    /// Original expression for recompilation (if needed)
    /// Stored as hash to avoid cloning large expressions
    expr_hash: u64,
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
        }
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
pub struct TieredCompilationCache {
    /// Map from expression hash to compilation state
    entries: DashMap<u64, Arc<ExprCompilationState>>,

    /// Threshold for bytecode compilation
    pub bytecode_threshold: u32,

    /// Threshold for JIT Stage 1 compilation
    pub jit1_threshold: u32,

    /// Threshold for JIT Stage 2 compilation
    pub jit2_threshold: u32,

    // Warm-up period to skip tracking overhead for small workloads
    /// Threshold for warm-up period - no tracking until this many total evaluations
    warmup_threshold: u64,
    /// Flag indicating warm-up period is complete (set once, never reverts)
    warmup_complete: AtomicBool,

    // Atomic statistics counters (lock-free to avoid contention at 4+ threads)
    expressions_tracked: AtomicU64,
    total_executions: AtomicU64,
    bytecode_compilations_triggered: AtomicU64,
    bytecode_compilations_completed: AtomicU64,
    bytecode_compilations_failed: AtomicU64,
    jit1_compilations_triggered: AtomicU64,
    jit1_compilations_completed: AtomicU64,
    jit1_compilations_failed: AtomicU64,
    jit2_compilations_triggered: AtomicU64,
    jit2_compilations_completed: AtomicU64,
    jit2_compilations_failed: AtomicU64,
    interpreter_executions: AtomicU64,
    bytecode_executions: AtomicU64,
    jit1_executions: AtomicU64,
    jit2_executions: AtomicU64,
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
}

impl TieredCompilationCache {
    /// Create a new tiered compilation cache with default thresholds
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
            bytecode_threshold: BYTECODE_THRESHOLD,
            jit1_threshold: JIT1_THRESHOLD,
            jit2_threshold: JIT2_THRESHOLD,
            warmup_threshold: DEFAULT_WARMUP_THRESHOLD,
            warmup_complete: AtomicBool::new(false),
            expressions_tracked: AtomicU64::new(0),
            total_executions: AtomicU64::new(0),
            bytecode_compilations_triggered: AtomicU64::new(0),
            bytecode_compilations_completed: AtomicU64::new(0),
            bytecode_compilations_failed: AtomicU64::new(0),
            jit1_compilations_triggered: AtomicU64::new(0),
            jit1_compilations_completed: AtomicU64::new(0),
            jit1_compilations_failed: AtomicU64::new(0),
            jit2_compilations_triggered: AtomicU64::new(0),
            jit2_compilations_completed: AtomicU64::new(0),
            jit2_compilations_failed: AtomicU64::new(0),
            interpreter_executions: AtomicU64::new(0),
            bytecode_executions: AtomicU64::new(0),
            jit1_executions: AtomicU64::new(0),
            jit2_executions: AtomicU64::new(0),
        }
    }

    /// Create a new cache with custom thresholds
    pub fn with_thresholds(bytecode: u32, jit1: u32, jit2: u32) -> Self {
        Self::with_thresholds_and_warmup(bytecode, jit1, jit2, DEFAULT_WARMUP_THRESHOLD)
    }

    /// Create a new cache with custom thresholds and warm-up threshold
    pub fn with_thresholds_and_warmup(bytecode: u32, jit1: u32, jit2: u32, warmup: u64) -> Self {
        Self {
            entries: DashMap::new(),
            bytecode_threshold: bytecode,
            jit1_threshold: jit1,
            jit2_threshold: jit2,
            warmup_threshold: warmup,
            warmup_complete: AtomicBool::new(warmup == 0),
            expressions_tracked: AtomicU64::new(0),
            total_executions: AtomicU64::new(0),
            bytecode_compilations_triggered: AtomicU64::new(0),
            bytecode_compilations_completed: AtomicU64::new(0),
            bytecode_compilations_failed: AtomicU64::new(0),
            jit1_compilations_triggered: AtomicU64::new(0),
            jit1_compilations_completed: AtomicU64::new(0),
            jit1_compilations_failed: AtomicU64::new(0),
            jit2_compilations_triggered: AtomicU64::new(0),
            jit2_compilations_completed: AtomicU64::new(0),
            jit2_compilations_failed: AtomicU64::new(0),
            interpreter_executions: AtomicU64::new(0),
            bytecode_executions: AtomicU64::new(0),
            jit1_executions: AtomicU64::new(0),
            jit2_executions: AtomicU64::new(0),
        }
    }

    /// Get or create a compilation state for an expression
    fn get_or_create_state(&self, expr: &MettaValue) -> Arc<ExprCompilationState> {
        let hash = hash_metta_value(expr);

        // Fast path: check if already exists
        if let Some(entry) = self.entries.get(&hash) {
            return Arc::clone(entry.value());
        }

        // Slow path: create new entry
        let state = Arc::new(ExprCompilationState::new(hash));
        self.entries.entry(hash).or_insert_with(|| {
            // Update stats atomically (lock-free)
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
        // Get or create state for this expression
        let state = self.get_or_create_state(expr);

        // Atomically increment execution count
        let count = state.execution_count.fetch_add(1, Ordering::Relaxed) + 1;

        // Update total execution stats
        self.total_executions.fetch_add(1, Ordering::Relaxed);

        // Check for tier transitions
        self.maybe_trigger_bytecode(expr, &state, count);
        self.maybe_trigger_jit1(&state, count);
        self.maybe_trigger_jit2(&state, count);

        state
    }

    /// Check if warm-up period is complete
    #[inline]
    pub fn is_warmup_complete(&self) -> bool {
        self.warmup_complete.load(Ordering::Relaxed)
    }

    /// Maybe trigger bytecode compilation
    fn maybe_trigger_bytecode(
        &self,
        expr: &MettaValue,
        state: &Arc<ExprCompilationState>,
        count: u32,
    ) {
        // Check if we've reached the threshold
        if count < self.bytecode_threshold {
            return;
        }

        // Check if already started
        if state.bytecode_status() != TierStatusKind::NotStarted {
            return;
        }

        // Try to win the race to compile
        if !state.try_start_bytecode_compile() {
            return;
        }

        // Update stats atomically (lock-free)
        self.bytecode_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Clone what we need for the background task
        let expr_clone = expr.clone();
        let state_clone = Arc::clone(state);

        // Compilation closure
        let compile_task = move || match compile_arc("tiered", &expr_clone) {
            Ok(chunk) => {
                state_clone.set_bytecode_ready(chunk);
            }
            Err(_) => {
                state_clone.set_bytecode_failed();
            }
        };

        // Choose spawn method based on feature and execution mode
        #[cfg(feature = "hybrid-p2-priority-scheduler")]
        {
            // Hybrid mode: Use Rayon for sequential, P2 scheduler for parallel
            if is_sequential_mode() {
                rayon::spawn(compile_task);
            } else {
                global_priority_eval_pool().spawn_with_priority(
                    compile_task,
                    priority_levels::BACKGROUND_COMPILE,
                    TaskTypeId::BytecodeCompile,
                );
            }
        }

        #[cfg(not(feature = "hybrid-p2-priority-scheduler"))]
        {
            // Default: Always use Rayon (compatible with Rholang shared schedulers)
            rayon::spawn(compile_task);
        }
    }

    /// Maybe trigger JIT Stage 1 compilation
    fn maybe_trigger_jit1(&self, state: &Arc<ExprCompilationState>, count: u32) {
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
        self.jit1_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Get the bytecode chunk
        let chunk = match state.bytecode_chunk() {
            Some(c) => c,
            None => {
                state.set_jit1_failed();
                return;
            }
        };

        // Clone state for background task
        let state_clone = Arc::clone(state);

        // JIT compilation closure
        let jit_compile = move || {
            // JIT compilation using Cranelift
            use super::jit::compiler::JitCompiler;

            // Check if chunk can be JIT compiled
            if !JitCompiler::can_compile_stage1(&chunk) {
                state_clone.set_jit1_failed();
                return;
            }

            // Create JIT compiler and compile
            match JitCompiler::new() {
                Ok(mut compiler) => match compiler.compile(&chunk) {
                    Ok(ptr) => {
                        let code = NativeCode {
                            ptr,
                            code_size: chunk.len() * 8, // Rough estimate
                        };
                        state_clone.set_jit1_ready(Arc::new(code));
                    }
                    Err(_) => {
                        state_clone.set_jit1_failed();
                    }
                },
                Err(_) => {
                    state_clone.set_jit1_failed();
                }
            }
        };

        // Choose spawn method based on feature and execution mode
        #[cfg(feature = "hybrid-p2-priority-scheduler")]
        {
            if is_sequential_mode() {
                rayon::spawn(jit_compile);
            } else {
                global_priority_eval_pool().spawn_with_priority(
                    jit_compile,
                    priority_levels::BACKGROUND_COMPILE,
                    TaskTypeId::JitCompile,
                );
            }
        }

        #[cfg(not(feature = "hybrid-p2-priority-scheduler"))]
        {
            rayon::spawn(jit_compile);
        }
    }

    /// Maybe trigger JIT Stage 2 compilation
    fn maybe_trigger_jit2(&self, state: &Arc<ExprCompilationState>, count: u32) {
        // Check if we've reached the threshold
        if count < self.jit2_threshold {
            return;
        }

        // JIT Stage 2 requires bytecode to be Ready (can skip JIT1)
        if state.bytecode_status() != TierStatusKind::Ready {
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
        self.jit2_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Get the bytecode chunk
        let chunk = match state.bytecode_chunk() {
            Some(c) => c,
            None => {
                state.set_jit2_failed();
                return;
            }
        };

        // Clone state for background task
        let state_clone = Arc::clone(state);

        // JIT Stage 2 compilation closure
        let jit_compile = move || {
            use super::jit::compiler::JitCompiler;

            // Check if chunk can be JIT compiled
            // Stage 2 uses same compilability check as Stage 1 for now
            if !JitCompiler::can_compile_stage1(&chunk) {
                state_clone.set_jit2_failed();
                return;
            }

            // Create JIT compiler and compile
            // TODO: Add Stage 2-specific optimizations (more aggressive inlining, etc.)
            match JitCompiler::new() {
                Ok(mut compiler) => match compiler.compile(&chunk) {
                    Ok(ptr) => {
                        let code = NativeCode {
                            ptr,
                            code_size: chunk.len() * 10, // Stage 2 generates more code
                        };
                        state_clone.set_jit2_ready(Arc::new(code));
                    }
                    Err(_) => {
                        state_clone.set_jit2_failed();
                    }
                },
                Err(_) => {
                    state_clone.set_jit2_failed();
                }
            }
        };

        // Choose spawn method based on feature and execution mode
        #[cfg(feature = "hybrid-p2-priority-scheduler")]
        {
            if is_sequential_mode() {
                rayon::spawn(jit_compile);
            } else {
                global_priority_eval_pool().spawn_with_priority(
                    jit_compile,
                    priority_levels::BACKGROUND_COMPILE,
                    TaskTypeId::JitCompile,
                );
            }
        }

        #[cfg(not(feature = "hybrid-p2-priority-scheduler"))]
        {
            rayon::spawn(jit_compile);
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
        }
    }

    /// Reset statistics (lock-free via atomic stores)
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
    }

    /// Clear the entire cache
    pub fn clear(&self) {
        self.entries.clear();
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

impl Default for TieredCompilationCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global tiered compilation cache
///
/// Shared across all evaluations for optimal reuse of compiled code.
static GLOBAL_TIERED_CACHE: std::sync::LazyLock<TieredCompilationCache> =
    std::sync::LazyLock::new(TieredCompilationCache::new);

/// Get a reference to the global tiered compilation cache
pub fn global_tiered_cache() -> &'static TieredCompilationCache {
    &GLOBAL_TIERED_CACHE
}

// =============================================================================
// Arena Tiered Compilation Cache - Zero-Conversion Support
// =============================================================================

use crate::backend::models::ArenaValue;
use crate::backend::bytecode::chunk::GenericBytecodeChunk;

/// Hash an ArenaValue for cache lookup.
///
/// Uses the same hashing strategy as MettaValue (FxHash-style mixing for primitives,
/// xxHash3 for complex types) to ensure consistent and efficient lookups.
pub fn hash_arena_value(expr: &ArenaValue<'static>) -> u64 {
    use std::hash::{Hash, Hasher};
    use xxhash_rust::xxh3::Xxh3;

    // Golden ratio constant for good hash distribution
    const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
    // Type-specific seeds
    const LONG_SEED: u64 = 0x517cc1b727220a95;
    const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
    const NIL_HASH: u64 = 0x6e696c5f_68617368;
    const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;
    const UNIT_HASH: u64 = 0x756e6974_68617368; // "unit_hash" as bytes

    // Fast path for primitives
    if expr.is_nil() {
        return NIL_HASH;
    }
    if expr.is_unit() {
        return UNIT_HASH;
    }
    if let Some(b) = expr.as_bool() {
        return if b {
            BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
        } else {
            BOOL_SEED
        };
    }
    if let Some(n) = expr.as_long() {
        let x = (n as u64).wrapping_add(LONG_SEED).wrapping_mul(GOLDEN_RATIO);
        return x ^ (x >> 32);
    }
    if let Some(f) = expr.as_float() {
        let bits = f.to_bits();
        let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
        return x ^ (x >> 32);
    }

    // Slow path for complex types - use xxHash3
    let mut hasher = Xxh3::new();
    hash_arena_value_recursive(expr, &mut hasher);
    hasher.finish()
}

/// Recursively hash an ArenaValue for complex types.
fn hash_arena_value_recursive<H: std::hash::Hasher>(expr: &ArenaValue<'static>, hasher: &mut H) {
    use std::hash::Hash;

    // Hash type discriminant first
    let type_tag: u8 = if expr.is_nil() { 0 }
        else if expr.is_unit() { 1 }
        else if expr.is_bool() { 2 }
        else if expr.is_long() { 3 }
        else if expr.is_float() { 4 }
        else if expr.is_string() { 5 }
        else if expr.is_atom() { 6 }
        else if expr.is_sexpr() { 7 }
        else if expr.is_error() { 8 }
        else if expr.is_empty() { 9 }
        else { 10 }; // Other types
    type_tag.hash(hasher);

    // Hash content based on type
    if let Some(b) = expr.as_bool() {
        b.hash(hasher);
    } else if let Some(n) = expr.as_long() {
        n.hash(hasher);
    } else if let Some(f) = expr.as_float() {
        f.to_bits().hash(hasher);
    } else if let Some(s) = expr.as_string() {
        s.hash(hasher);
    } else if let Some(s) = expr.as_atom() {
        s.hash(hasher);
    } else if let Some(items) = expr.as_sexpr() {
        items.len().hash(hasher);
        for item in items {
            hash_arena_value_recursive(item, hasher);
        }
    }
}

/// Per-expression compilation state for arena values.
///
/// Similar to `ExprCompilationState` but stores `GenericBytecodeChunk<ArenaValue<'static>>`.
/// Supports full JIT tiering (Stage 1 and Stage 2) for hot code paths.
pub struct ArenaExprCompilationState {
    /// Number of times this expression has been executed.
    pub execution_count: AtomicU32,

    // Bytecode tier (Tier 1)
    /// Status of bytecode compilation.
    bytecode_status: AtomicU8,

    /// Compiled bytecode chunk (write-once via OnceLock).
    bytecode_chunk: OnceLock<Arc<GenericBytecodeChunk<ArenaValue<'static>>>>,

    // JIT Stage 1 (Tier 2)
    /// Status of JIT Stage 1 compilation.
    jit1_status: AtomicU8,

    /// JIT Stage 1 native code (write-once via OnceLock).
    jit1_code: OnceLock<Arc<NativeCode>>,

    // JIT Stage 2 (Tier 3)
    /// Status of JIT Stage 2 compilation.
    jit2_status: AtomicU8,

    /// JIT Stage 2 native code (write-once via OnceLock).
    jit2_code: OnceLock<Arc<NativeCode>>,

    /// Original expression hash for debugging.
    expr_hash: u64,
}

impl ArenaExprCompilationState {
    /// Create a new cold state with zero executions.
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
        }
    }

    /// Get the current execution count.
    #[inline]
    pub fn count(&self) -> u32 {
        self.execution_count.load(Ordering::Relaxed)
    }

    /// Get bytecode status.
    #[inline]
    pub fn bytecode_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.bytecode_status.load(Ordering::Acquire))
    }

    /// Get bytecode chunk if ready.
    #[inline]
    pub fn bytecode_chunk(&self) -> Option<Arc<GenericBytecodeChunk<ArenaValue<'static>>>> {
        if self.bytecode_status() == TierStatusKind::Ready {
            self.bytecode_chunk.get().cloned()
        } else {
            None
        }
    }

    /// Try to start bytecode compilation (atomic CAS).
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

    /// Set bytecode compilation result.
    pub fn set_bytecode_ready(&self, chunk: Arc<GenericBytecodeChunk<ArenaValue<'static>>>) {
        let _ = self.bytecode_chunk.set(chunk);
        self.bytecode_status
            .store(TierStatusKind::Ready as u8, Ordering::Release);
    }

    /// Mark bytecode compilation as failed.
    pub fn set_bytecode_failed(&self) {
        self.bytecode_status
            .store(TierStatusKind::Failed as u8, Ordering::Release);
    }

    // -------------------------------------------------------------------------
    // JIT Stage 1 Methods
    // -------------------------------------------------------------------------

    /// Get JIT Stage 1 status.
    #[inline]
    pub fn jit1_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.jit1_status.load(Ordering::Acquire))
    }

    /// Get JIT Stage 1 native code if ready (lock-free read via OnceLock).
    #[inline]
    pub fn jit1_code(&self) -> Option<Arc<NativeCode>> {
        if self.jit1_status() == TierStatusKind::Ready {
            self.jit1_code.get().cloned()
        } else {
            None
        }
    }

    /// Try to start JIT Stage 1 compilation (atomic CAS).
    /// Returns true if this thread won the race to compile.
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

    /// Set JIT Stage 1 compilation result (write-once via OnceLock).
    pub fn set_jit1_ready(&self, code: Arc<NativeCode>) {
        let _ = self.jit1_code.set(code);
        self.jit1_status
            .store(TierStatusKind::Ready as u8, Ordering::Release);
    }

    /// Mark JIT Stage 1 compilation as failed.
    pub fn set_jit1_failed(&self) {
        self.jit1_status
            .store(TierStatusKind::Failed as u8, Ordering::Release);
    }

    // -------------------------------------------------------------------------
    // JIT Stage 2 Methods
    // -------------------------------------------------------------------------

    /// Get JIT Stage 2 status.
    #[inline]
    pub fn jit2_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.jit2_status.load(Ordering::Acquire))
    }

    /// Get JIT Stage 2 native code if ready (lock-free read via OnceLock).
    #[inline]
    pub fn jit2_code(&self) -> Option<Arc<NativeCode>> {
        if self.jit2_status() == TierStatusKind::Ready {
            self.jit2_code.get().cloned()
        } else {
            None
        }
    }

    /// Try to start JIT Stage 2 compilation (atomic CAS).
    /// Returns true if this thread won the race to compile.
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

    /// Set JIT Stage 2 compilation result (write-once via OnceLock).
    pub fn set_jit2_ready(&self, code: Arc<NativeCode>) {
        let _ = self.jit2_code.set(code);
        self.jit2_status
            .store(TierStatusKind::Ready as u8, Ordering::Release);
    }

    /// Mark JIT Stage 2 compilation as failed.
    pub fn set_jit2_failed(&self) {
        self.jit2_status
            .store(TierStatusKind::Failed as u8, Ordering::Release);
    }
}

impl std::fmt::Debug for ArenaExprCompilationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaExprCompilationState")
            .field("execution_count", &self.count())
            .field("bytecode_status", &self.bytecode_status())
            .field("jit1_status", &self.jit1_status())
            .field("jit2_status", &self.jit2_status())
            .field("expr_hash", &self.expr_hash)
            .finish()
    }
}

/// Arena tiered compilation cache.
///
/// Manages compilation state for ArenaValue expressions, separate from the
/// MettaValue cache to ensure type safety and zero-conversion execution.
/// Supports full JIT tiering (Stage 1 at 100+ executions, Stage 2 at 500+).
pub struct ArenaTieredCache {
    /// Map from expression hash to compilation state.
    entries: DashMap<u64, Arc<ArenaExprCompilationState>>,

    /// Threshold for bytecode compilation.
    pub bytecode_threshold: u32,

    /// Threshold for JIT Stage 1 compilation.
    pub jit1_threshold: u32,

    /// Threshold for JIT Stage 2 compilation.
    pub jit2_threshold: u32,

    // Statistics
    expressions_tracked: AtomicU64,
    total_executions: AtomicU64,
    bytecode_compilations_triggered: AtomicU64,
    bytecode_compilations_completed: AtomicU64,
    bytecode_compilations_failed: AtomicU64,
    bytecode_executions: AtomicU64,
    jit1_compilations_triggered: AtomicU64,
    jit1_compilations_completed: AtomicU64,
    jit1_compilations_failed: AtomicU64,
    jit1_executions: AtomicU64,
    jit2_compilations_triggered: AtomicU64,
    jit2_compilations_completed: AtomicU64,
    jit2_compilations_failed: AtomicU64,
    jit2_executions: AtomicU64,
    interpreter_executions: AtomicU64,
}

impl ArenaTieredCache {
    /// Create a new arena tiered cache with default thresholds.
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
            bytecode_threshold: BYTECODE_THRESHOLD,
            jit1_threshold: JIT1_THRESHOLD,
            jit2_threshold: JIT2_THRESHOLD,
            expressions_tracked: AtomicU64::new(0),
            total_executions: AtomicU64::new(0),
            bytecode_compilations_triggered: AtomicU64::new(0),
            bytecode_compilations_completed: AtomicU64::new(0),
            bytecode_compilations_failed: AtomicU64::new(0),
            bytecode_executions: AtomicU64::new(0),
            jit1_compilations_triggered: AtomicU64::new(0),
            jit1_compilations_completed: AtomicU64::new(0),
            jit1_compilations_failed: AtomicU64::new(0),
            jit1_executions: AtomicU64::new(0),
            jit2_compilations_triggered: AtomicU64::new(0),
            jit2_compilations_completed: AtomicU64::new(0),
            jit2_compilations_failed: AtomicU64::new(0),
            jit2_executions: AtomicU64::new(0),
            interpreter_executions: AtomicU64::new(0),
        }
    }

    /// Create a new cache with custom thresholds.
    pub fn with_thresholds(bytecode: u32, jit1: u32, jit2: u32) -> Self {
        Self {
            entries: DashMap::new(),
            bytecode_threshold: bytecode,
            jit1_threshold: jit1,
            jit2_threshold: jit2,
            expressions_tracked: AtomicU64::new(0),
            total_executions: AtomicU64::new(0),
            bytecode_compilations_triggered: AtomicU64::new(0),
            bytecode_compilations_completed: AtomicU64::new(0),
            bytecode_compilations_failed: AtomicU64::new(0),
            bytecode_executions: AtomicU64::new(0),
            jit1_compilations_triggered: AtomicU64::new(0),
            jit1_compilations_completed: AtomicU64::new(0),
            jit1_compilations_failed: AtomicU64::new(0),
            jit1_executions: AtomicU64::new(0),
            jit2_compilations_triggered: AtomicU64::new(0),
            jit2_compilations_completed: AtomicU64::new(0),
            jit2_compilations_failed: AtomicU64::new(0),
            jit2_executions: AtomicU64::new(0),
            interpreter_executions: AtomicU64::new(0),
        }
    }

    /// Get or create a compilation state for an expression.
    fn get_or_create_state(&self, expr: &ArenaValue<'static>) -> Arc<ArenaExprCompilationState> {
        let hash = hash_arena_value(expr);

        // Fast path: check if already exists
        if let Some(entry) = self.entries.get(&hash) {
            return Arc::clone(entry.value());
        }

        // Slow path: create new entry
        let state = Arc::new(ArenaExprCompilationState::new(hash));
        self.entries.entry(hash).or_insert_with(|| {
            self.expressions_tracked.fetch_add(1, Ordering::Relaxed);
            Arc::clone(&state)
        });

        self.entries
            .get(&hash)
            .map(|e| Arc::clone(e.value()))
            .unwrap_or(state)
    }

    /// Record an execution and trigger tier compilations if thresholds reached.
    ///
    /// Returns the compilation state for dispatch decisions.
    /// Triggers:
    /// - Bytecode compilation at bytecode_threshold (default: 1)
    /// - JIT Stage 1 compilation at jit1_threshold (default: 100)
    /// - JIT Stage 2 compilation at jit2_threshold (default: 500)
    pub fn record_execution(&self, expr: &ArenaValue<'static>) -> Arc<ArenaExprCompilationState> {
        let state = self.get_or_create_state(expr);

        // Atomically increment execution count
        let count = state.execution_count.fetch_add(1, Ordering::Relaxed) + 1;

        // Update total execution stats
        self.total_executions.fetch_add(1, Ordering::Relaxed);

        // Check for tier compilation triggers (in order of increasing tier)
        self.maybe_trigger_bytecode(expr, &state, count);
        self.maybe_trigger_jit1(&state, count);
        self.maybe_trigger_jit2(&state, count);

        state
    }

    /// Maybe trigger bytecode compilation.
    fn maybe_trigger_bytecode(
        &self,
        expr: &ArenaValue<'static>,
        state: &Arc<ArenaExprCompilationState>,
        count: u32,
    ) {
        // Check threshold
        if count < self.bytecode_threshold {
            return;
        }

        // Check if already started
        if state.bytecode_status() != TierStatusKind::NotStarted {
            return;
        }

        // Try to win the compilation race
        if !state.try_start_bytecode_compile() {
            return;
        }

        // Update stats
        self.bytecode_compilations_triggered.fetch_add(1, Ordering::Relaxed);

        // Clone what we need for the background task
        let expr_clone = expr.clone();
        let state_clone = Arc::clone(state);

        // Compile in background
        rayon::spawn(move || {
            use crate::backend::bytecode::compile_arena_bytecode_arc;

            match compile_arena_bytecode_arc("tiered", &expr_clone) {
                Ok(chunk) => {
                    state_clone.set_bytecode_ready(chunk);
                }
                Err(_) => {
                    state_clone.set_bytecode_failed();
                }
            }
        });
    }

    /// Trigger JIT Stage 1 compilation at jit1_threshold (default: 100 executions).
    ///
    /// JIT Stage 1 requires bytecode to be Ready first (compiles from bytecode).
    /// The generated JIT code uses runtime functions that dispatch based on
    /// `JitContext.value_mode` to handle arena vs heap values correctly.
    fn maybe_trigger_jit1(&self, state: &Arc<ArenaExprCompilationState>, count: u32) {
        // Check threshold
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

        // Try to win the compilation race
        if !state.try_start_jit1_compile() {
            return;
        }

        // Update stats
        self.jit1_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Get the bytecode chunk
        let chunk = match state.bytecode_chunk() {
            Some(c) => c,
            None => {
                state.set_jit1_failed();
                return;
            }
        };

        // Clone state for background task
        let state_clone = Arc::clone(state);

        // JIT compilation in background
        rayon::spawn(move || {
            use super::jit::compiler::JitCompiler;

            // Check if chunk can be JIT compiled
            // We use the bytecode-only compilability check - bytecode structure is the same
            // regardless of value type (ArenaValue or MettaValue)
            if !JitCompiler::can_compile_stage1_bytecode(chunk.code()) {
                state_clone.set_jit1_failed();
                return;
            }

            // Create JIT compiler and compile
            // The bytecode is identical for arena and heap modes - runtime functions
            // dispatch based on JitContext.value_mode to handle value creation correctly
            match JitCompiler::new() {
                Ok(mut compiler) => {
                    // Create a temporary BytecodeChunk wrapper for compilation
                    // The JIT compiler needs BytecodeChunk for full analysis
                    let temp_chunk = super::chunk::BytecodeChunk::from_code_and_constant_count(
                        "arena_jit",
                        chunk.code().to_vec(),
                        chunk.constant_count(),
                    );

                    match compiler.compile(&temp_chunk) {
                        Ok(ptr) => {
                            let code = NativeCode {
                                ptr,
                                code_size: chunk.len() * 8, // Rough estimate
                            };
                            state_clone.set_jit1_ready(Arc::new(code));
                        }
                        Err(_) => {
                            state_clone.set_jit1_failed();
                        }
                    }
                }
                Err(_) => {
                    state_clone.set_jit1_failed();
                }
            }
        });
    }

    /// Trigger JIT Stage 2 compilation at jit2_threshold (default: 500 executions).
    ///
    /// JIT Stage 2 requires bytecode to be Ready (can skip JIT Stage 1).
    /// Stage 2 uses more aggressive optimizations.
    fn maybe_trigger_jit2(&self, state: &Arc<ArenaExprCompilationState>, count: u32) {
        // Check threshold
        if count < self.jit2_threshold {
            return;
        }

        // JIT Stage 2 requires bytecode to be Ready (can skip JIT1)
        if state.bytecode_status() != TierStatusKind::Ready {
            return;
        }

        // Check if already started
        if state.jit2_status() != TierStatusKind::NotStarted {
            return;
        }

        // Try to win the compilation race
        if !state.try_start_jit2_compile() {
            return;
        }

        // Update stats
        self.jit2_compilations_triggered
            .fetch_add(1, Ordering::Relaxed);

        // Get the bytecode chunk
        let chunk = match state.bytecode_chunk() {
            Some(c) => c,
            None => {
                state.set_jit2_failed();
                return;
            }
        };

        // Clone state for background task
        let state_clone = Arc::clone(state);

        // JIT Stage 2 compilation in background
        rayon::spawn(move || {
            use super::jit::compiler::JitCompiler;

            // Check if chunk can be JIT compiled
            if !JitCompiler::can_compile_stage1_bytecode(chunk.code()) {
                state_clone.set_jit2_failed();
                return;
            }

            // Create JIT compiler and compile with Stage 2 optimizations
            // TODO: Add Stage 2-specific optimizations (more aggressive inlining, etc.)
            match JitCompiler::new() {
                Ok(mut compiler) => {
                    // Create a temporary BytecodeChunk wrapper for compilation
                    let temp_chunk = super::chunk::BytecodeChunk::from_code_and_constant_count(
                        "arena_jit_s2",
                        chunk.code().to_vec(),
                        chunk.constant_count(),
                    );

                    match compiler.compile(&temp_chunk) {
                        Ok(ptr) => {
                            let code = NativeCode {
                                ptr,
                                code_size: chunk.len() * 10, // Stage 2 generates more code
                            };
                            state_clone.set_jit2_ready(Arc::new(code));
                        }
                        Err(_) => {
                            state_clone.set_jit2_failed();
                        }
                    }
                }
                Err(_) => {
                    state_clone.set_jit2_failed();
                }
            }
        });
    }

    /// Get the best available execution tier for an expression.
    ///
    /// Returns the highest tier that has Ready status:
    /// - JitStage2 (highest) - very hot code, 500+ executions
    /// - JitStage1 - hot code, 100+ executions
    /// - Bytecode - warm code, 2+ executions
    /// - Interpreter (lowest) - cold code
    pub fn get_best_tier(&self, expr: &ArenaValue<'static>) -> ExecutionTier {
        let hash = hash_arena_value(expr);

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

        ExecutionTier::Interpreter
    }

    /// Get the compilation state for an expression (if it exists).
    pub fn get_state(&self, expr: &ArenaValue<'static>) -> Option<Arc<ArenaExprCompilationState>> {
        let hash = hash_arena_value(expr);
        self.entries.get(&hash).map(|e| Arc::clone(e.value()))
    }

    /// Record an execution at a specific tier (for statistics).
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

    /// Get the number of expressions tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear the cache.
    pub fn clear(&self) {
        self.entries.clear();
    }
}

impl Default for ArenaTieredCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global arena tiered compilation cache.
static GLOBAL_ARENA_TIERED_CACHE: std::sync::LazyLock<ArenaTieredCache> =
    std::sync::LazyLock::new(ArenaTieredCache::new);

/// Get a reference to the global arena tiered compilation cache.
pub fn global_arena_tiered_cache() -> &'static ArenaTieredCache {
    &GLOBAL_ARENA_TIERED_CACHE
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        let cache = TieredCompilationCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.bytecode_threshold, BYTECODE_THRESHOLD);
        assert_eq!(cache.jit1_threshold, JIT1_THRESHOLD);
        assert_eq!(cache.jit2_threshold, JIT2_THRESHOLD);
    }

    #[test]
    fn test_tiered_cache_record_execution() {
        // Create cache with default settings
        let cache = TieredCompilationCache::new();
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
        let cache = TieredCompilationCache::new();
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
        let cache = TieredCompilationCache::new();
        let expr = MettaValue::Long(42);

        // Before any execution, should be interpreter
        assert_eq!(cache.get_best_tier(&expr), ExecutionTier::Interpreter);

        // After some executions (bytecode not ready yet)
        for _ in 0..10 {
            let _ = cache.record_execution(&expr);
        }
        // Still interpreter because compilation is async
        // (In real scenario, bytecode would become ready after rayon task completes)
    }

    #[test]
    fn test_tiered_cache_custom_thresholds() {
        let cache = TieredCompilationCache::with_thresholds(5, 50, 200);
        assert_eq!(cache.bytecode_threshold, 5);
        assert_eq!(cache.jit1_threshold, 50);
        assert_eq!(cache.jit2_threshold, 200);
    }

    #[test]
    fn test_tiered_cache_clear() {
        let cache = TieredCompilationCache::new();
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
    fn test_record_tier_execution() {
        let cache = TieredCompilationCache::new();

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
    fn test_tiered_cache_warmup_threshold() {
        // Test with zero warmup (immediately complete)
        let cache = TieredCompilationCache::with_thresholds_and_warmup(1, 100, 500, 0);
        assert!(cache.is_warmup_complete());

        // Test with non-zero warmup
        let cache2 = TieredCompilationCache::with_thresholds_and_warmup(1, 100, 500, 1000);
        assert!(!cache2.is_warmup_complete());
    }

    #[test]
    fn test_tiered_cache_stats_reset() {
        let cache = TieredCompilationCache::new();
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
        let cache = TieredCompilationCache::new();
        let expr = MettaValue::Long(42);

        // Before recording, state should not exist
        assert!(cache.get_state(&expr).is_none());

        // After recording, state should exist
        let _ = cache.record_execution(&expr);
        assert!(cache.get_state(&expr).is_some());
    }

    #[test]
    fn test_tiered_cache_complex_expression() {
        let cache = TieredCompilationCache::new();

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
        let state = ArenaExprCompilationState::new(99999);
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
        let state = ArenaExprCompilationState::new(12345);

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
        let state = ArenaExprCompilationState::new(12345);

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
    fn test_arena_tiered_cache_new() {
        let cache = ArenaTieredCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.bytecode_threshold, BYTECODE_THRESHOLD);
        assert_eq!(cache.jit1_threshold, JIT1_THRESHOLD);
        assert_eq!(cache.jit2_threshold, JIT2_THRESHOLD);
    }

    #[test]
    fn test_arena_tiered_cache_custom_thresholds() {
        let cache = ArenaTieredCache::with_thresholds(10, 200, 1000);
        assert_eq!(cache.bytecode_threshold, 10);
        assert_eq!(cache.jit1_threshold, 200);
        assert_eq!(cache.jit2_threshold, 1000);
    }

    #[test]
    fn test_arena_tiered_cache_clear() {
        use crate::backend::models::{ArenaValue, ArenaValueFactory, MettaValueFactory};
        use bumpalo::Bump;

        let arena = Box::leak(Box::new(Bump::new()));
        let factory = ArenaValueFactory::new(arena);
        let cache = ArenaTieredCache::new();
        let expr: ArenaValue<'static> = factory.long(42);

        let _ = cache.record_execution(&expr);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_arena_tiered_cache_record_tier_execution() {
        let cache = ArenaTieredCache::new();

        cache.record_tier_execution(ExecutionTier::Interpreter);
        cache.record_tier_execution(ExecutionTier::Bytecode);
        cache.record_tier_execution(ExecutionTier::JitStage1);
        cache.record_tier_execution(ExecutionTier::JitStage2);

        // Verify each tier was recorded (stats aren't directly exposed, but the
        // atomic counters should be incremented)
        // Just verify the method doesn't panic for all tiers
    }

    #[test]
    fn test_arena_tiered_cache_get_state() {
        use crate::backend::models::{ArenaValue, ArenaValueFactory, MettaValueFactory};
        use bumpalo::Bump;

        let arena = Box::leak(Box::new(Bump::new()));
        let factory = ArenaValueFactory::new(arena);
        let cache = ArenaTieredCache::new();
        let expr: ArenaValue<'static> = factory.long(42);

        // Before recording, state should not exist
        assert!(cache.get_state(&expr).is_none());

        // After recording, state should exist
        let _ = cache.record_execution(&expr);
        assert!(cache.get_state(&expr).is_some());
    }

    #[test]
    fn test_arena_tiered_cache_get_best_tier() {
        use crate::backend::models::{ArenaValue, ArenaValueFactory, MettaValueFactory};
        use bumpalo::Bump;

        let arena = Box::leak(Box::new(Bump::new()));
        let factory = ArenaValueFactory::new(arena);
        let cache = ArenaTieredCache::new();
        let expr: ArenaValue<'static> = factory.long(42);

        // Before any execution, should be interpreter
        assert_eq!(cache.get_best_tier(&expr), ExecutionTier::Interpreter);

        // After recording (bytecode compilation is async)
        let _ = cache.record_execution(&expr);
        // Still interpreter because compilation is async
        assert_eq!(cache.get_best_tier(&expr), ExecutionTier::Interpreter);
    }

    #[test]
    fn test_hash_arena_value_primitives() {
        use crate::backend::models::{ArenaValue, ArenaValueFactory, MettaValueFactory};
        use bumpalo::Bump;

        let arena = Box::leak(Box::new(Bump::new()));
        let factory = ArenaValueFactory::new(arena);

        // Test that hashing primitives produces consistent results
        let nil_val: ArenaValue<'static> = factory.nil();
        let unit_val: ArenaValue<'static> = factory.unit();
        let true_val: ArenaValue<'static> = factory.bool(true);
        let false_val: ArenaValue<'static> = factory.bool(false);
        let long_val: ArenaValue<'static> = factory.long(42);
        let float_val: ArenaValue<'static> = factory.float(3.14);

        let nil_hash = hash_arena_value(&nil_val);
        let unit_hash = hash_arena_value(&unit_val);
        let true_hash = hash_arena_value(&true_val);
        let false_hash = hash_arena_value(&false_val);
        let long_hash = hash_arena_value(&long_val);
        let float_hash = hash_arena_value(&float_val);

        // Different types should produce different hashes
        assert_ne!(nil_hash, unit_hash);
        assert_ne!(true_hash, false_hash);
        assert_ne!(long_hash, float_hash);
        assert_ne!(nil_hash, long_hash);
    }

    #[test]
    fn test_hash_arena_value_strings() {
        use crate::backend::models::{ArenaValue, ArenaValueFactory, MettaValueFactory};
        use bumpalo::Bump;

        let arena = Box::leak(Box::new(Bump::new()));
        let factory = ArenaValueFactory::new(arena);

        // Test hashing strings and atoms
        let string_val: ArenaValue<'static> = factory.string("hello");
        let atom_val: ArenaValue<'static> = factory.atom("hello");

        let string_hash = hash_arena_value(&string_val);
        let atom_hash = hash_arena_value(&atom_val);

        // Same content but different types should produce different hashes
        assert_ne!(string_hash, atom_hash);
    }

    #[test]
    fn test_native_code_debug() {
        let code = NativeCode {
            ptr: std::ptr::null(),
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

    #[test]
    fn test_arena_expr_compilation_state_debug() {
        let state = ArenaExprCompilationState::new(99999);
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("ArenaExprCompilationState"));
        assert!(debug_str.contains("execution_count"));
        assert!(debug_str.contains("99999"));
    }
}
