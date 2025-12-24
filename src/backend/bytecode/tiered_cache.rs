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
//! - **Non-blocking compilation**: All tier transitions happen in the background via Rayon
//! - **Graceful fallback**: Always executes with the best available tier
//! - **Lock-free access**: Uses DashMap and atomics for thread-safe concurrent access
//! - **Unified state management**: Single cache tracks all tier states per expression
//!
//! ## Design
//!
//! Each expression is tracked by its structural hash (u64). On every execution:
//! 1. Execution count is atomically incremented
//! 2. If a threshold is crossed and the previous tier is Ready, spawn background compilation
//! 3. Dispatch to the best available tier (highest Ready tier)
//!
//! Compilation is asynchronous - we spawn rayon tasks and continue using the current tier
//! until the next tier becomes Ready.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

use crate::backend::models::MettaValue;

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
pub struct ExprCompilationState {
    /// Number of times this expression has been executed
    pub execution_count: AtomicU32,

    // Bytecode tier (Tier 1)
    /// Status of bytecode compilation
    bytecode_status: AtomicU8,
    /// Compiled bytecode chunk (set when status is Ready)
    bytecode_chunk: std::sync::RwLock<Option<Arc<BytecodeChunk>>>,

    // JIT Stage 1 (Tier 2)
    /// Status of JIT Stage 1 compilation
    jit1_status: AtomicU8,
    /// JIT Stage 1 native code (set when status is Ready)
    jit1_code: std::sync::RwLock<Option<Arc<NativeCode>>>,

    // JIT Stage 2 (Tier 3)
    /// Status of JIT Stage 2 compilation
    jit2_status: AtomicU8,
    /// JIT Stage 2 native code (set when status is Ready)
    jit2_code: std::sync::RwLock<Option<Arc<NativeCode>>>,

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
            bytecode_chunk: std::sync::RwLock::new(None),
            jit1_status: AtomicU8::new(TierStatusKind::NotStarted as u8),
            jit1_code: std::sync::RwLock::new(None),
            jit2_status: AtomicU8::new(TierStatusKind::NotStarted as u8),
            jit2_code: std::sync::RwLock::new(None),
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

    /// Get bytecode chunk if ready
    #[inline]
    pub fn bytecode_chunk(&self) -> Option<Arc<BytecodeChunk>> {
        if self.bytecode_status() == TierStatusKind::Ready {
            self.bytecode_chunk
                .read()
                .ok()
                .and_then(|guard| guard.clone())
        } else {
            None
        }
    }

    /// Get JIT Stage 1 status
    #[inline]
    pub fn jit1_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.jit1_status.load(Ordering::Acquire))
    }

    /// Get JIT Stage 1 native code if ready
    #[inline]
    pub fn jit1_code(&self) -> Option<Arc<NativeCode>> {
        if self.jit1_status() == TierStatusKind::Ready {
            self.jit1_code.read().ok().and_then(|guard| guard.clone())
        } else {
            None
        }
    }

    /// Get JIT Stage 2 status
    #[inline]
    pub fn jit2_status(&self) -> TierStatusKind {
        TierStatusKind::from(self.jit2_status.load(Ordering::Acquire))
    }

    /// Get JIT Stage 2 native code if ready
    #[inline]
    pub fn jit2_code(&self) -> Option<Arc<NativeCode>> {
        if self.jit2_status() == TierStatusKind::Ready {
            self.jit2_code.read().ok().and_then(|guard| guard.clone())
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

    /// Set bytecode compilation result
    pub fn set_bytecode_ready(&self, chunk: Arc<BytecodeChunk>) {
        if let Ok(mut guard) = self.bytecode_chunk.write() {
            *guard = Some(chunk);
        }
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

    /// Set JIT Stage 1 compilation result
    pub fn set_jit1_ready(&self, code: Arc<NativeCode>) {
        if let Ok(mut guard) = self.jit1_code.write() {
            *guard = Some(code);
        }
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

    /// Set JIT Stage 2 compilation result
    pub fn set_jit2_ready(&self, code: Arc<NativeCode>) {
        if let Ok(mut guard) = self.jit2_code.write() {
            *guard = Some(code);
        }
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

    /// Statistics tracking
    stats: std::sync::RwLock<TieredCacheStats>,
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
            stats: std::sync::RwLock::new(TieredCacheStats::default()),
        }
    }

    /// Create a new cache with custom thresholds
    pub fn with_thresholds(bytecode: u32, jit1: u32, jit2: u32) -> Self {
        Self {
            entries: DashMap::new(),
            bytecode_threshold: bytecode,
            jit1_threshold: jit1,
            jit2_threshold: jit2,
            stats: std::sync::RwLock::new(TieredCacheStats::default()),
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
        self.entries
            .entry(hash)
            .or_insert_with(|| {
                // Update stats
                if let Ok(mut stats) = self.stats.write() {
                    stats.expressions_tracked += 1;
                }
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
    pub fn record_execution(&self, expr: &MettaValue) -> Arc<ExprCompilationState> {
        let state = self.get_or_create_state(expr);

        // Atomically increment execution count
        let count = state.execution_count.fetch_add(1, Ordering::Relaxed) + 1;

        // Update total execution stats
        if let Ok(mut stats) = self.stats.write() {
            stats.total_executions += 1;
        }

        // Check for tier transitions
        self.maybe_trigger_bytecode(expr, &state, count);
        self.maybe_trigger_jit1(&state, count);
        self.maybe_trigger_jit2(&state, count);

        state
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

        // Update stats
        if let Ok(mut stats) = self.stats.write() {
            stats.bytecode_compilations_triggered += 1;
        }

        // Clone what we need for the background task
        let expr_clone = expr.clone();
        let state_clone = Arc::clone(state);
        let stats_ref = &self.stats;

        // Spawn background compilation on rayon
        rayon::spawn(move || {
            match compile_arc("tiered", &expr_clone) {
                Ok(chunk) => {
                    state_clone.set_bytecode_ready(chunk);
                    // Note: We can't easily update stats here since rayon::spawn
                    // requires 'static lifetime. Stats updates would require
                    // Arc<RwLock<Stats>> passed to the closure.
                }
                Err(_) => {
                    state_clone.set_bytecode_failed();
                }
            }
        });

        // For now, just mark triggered (completion tracking would need Arc)
        let _ = stats_ref;
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

        // Update stats
        if let Ok(mut stats) = self.stats.write() {
            stats.jit1_compilations_triggered += 1;
        }

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

        // Spawn background JIT compilation on rayon
        rayon::spawn(move || {
            // JIT compilation using Cranelift
            
            {
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
            }
        });
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

        // Update stats
        if let Ok(mut stats) = self.stats.write() {
            stats.jit2_compilations_triggered += 1;
        }

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

        // Spawn background JIT Stage 2 compilation on rayon
        rayon::spawn(move || {
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
        });
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

    /// Get current cache statistics
    pub fn stats(&self) -> TieredCacheStats {
        self.stats
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        if let Ok(mut stats) = self.stats.write() {
            *stats = TieredCacheStats::default();
        }
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

    /// Record an execution at a specific tier (for statistics)
    pub fn record_tier_execution(&self, tier: ExecutionTier) {
        if let Ok(mut stats) = self.stats.write() {
            match tier {
                ExecutionTier::Interpreter => stats.interpreter_executions += 1,
                ExecutionTier::Bytecode => stats.bytecode_executions += 1,
                ExecutionTier::JitStage1 => stats.jit1_executions += 1,
                ExecutionTier::JitStage2 => stats.jit2_executions += 1,
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
        let cache = TieredCompilationCache::new();
        let expr = MettaValue::Long(42);

        // Record first execution
        let state = cache.record_execution(&expr);
        assert_eq!(state.count(), 1);

        // Record more executions
        let state = cache.record_execution(&expr);
        assert_eq!(state.count(), 2);

        // Cache should have one entry
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_tiered_cache_get_best_tier() {
        let cache = TieredCompilationCache::new();
        let expr = MettaValue::Long(42);

        // Before any execution, should be interpreter
        assert_eq!(cache.get_best_tier(&expr), ExecutionTier::Interpreter);

        // After some executions (bytecode not ready yet)
        for _ in 0..10 {
            cache.record_execution(&expr);
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

        cache.record_execution(&expr);
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
}
