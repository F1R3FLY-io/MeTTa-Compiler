//! JIT Profiling and Hotness Tracking
//!
//! This module implements execution profiling to identify hot bytecode chunks
//! that should be JIT compiled. The tiering strategy is:
//!
//! 1. Cold (< 10 executions): Run bytecode only
//! 2. Warming (10-99 executions): Run bytecode, increment counter
//! 3. Hot (>= 100 executions): Trigger JIT compilation
//! 4. Jitted: Run native code
//!
//! The threshold of 100 is aggressive to maximize JIT benefits while
//! amortizing compilation overhead.

use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU8, Ordering};

/// Execution count threshold to trigger JIT compilation
///
/// Set to 100 for aggressive tiering - hot loops will be JIT compiled
/// after approximately 100 iterations. This is balanced against:
/// - Too low: JIT overhead not amortized
/// - Too high: Miss optimization opportunities
pub const HOT_THRESHOLD: u32 = 100;

/// Threshold to start considering for JIT (warming state)
pub const WARM_THRESHOLD: u32 = 10;

/// Maximum executions to track (prevents counter overflow)
pub const MAX_EXECUTION_COUNT: u32 = u32::MAX - 1;

/// JIT compilation state for a bytecode chunk
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JitState {
    /// Chunk has not been executed enough to consider JIT
    Cold = 0,

    /// Chunk is being executed frequently, tracking for JIT
    Warming = 1,

    /// Chunk has reached threshold, JIT compilation queued
    Hot = 2,

    /// Chunk is currently being JIT compiled
    Compiling = 3,

    /// Chunk has been JIT compiled, native code available
    Jitted = 4,

    /// JIT compilation failed, fall back to bytecode permanently
    Failed = 5,
}

impl From<u8> for JitState {
    fn from(v: u8) -> Self {
        match v {
            0 => JitState::Cold,
            1 => JitState::Warming,
            2 => JitState::Hot,
            3 => JitState::Compiling,
            4 => JitState::Jitted,
            5 => JitState::Failed,
            _ => JitState::Cold,
        }
    }
}

/// Profiling data for a bytecode chunk
///
/// This struct tracks execution frequency and manages JIT state transitions.
/// It is designed for concurrent access from multiple threads.
///
/// # Thread Safety
///
/// All fields use atomic operations for thread-safe updates without locks.
/// The state machine transitions are designed to be safe even with races:
/// - Multiple threads may increment the counter simultaneously
/// - Only one thread will successfully transition to Compiling
/// - All threads see Jitted state once compilation completes
#[derive(Debug)]
pub struct JitProfile {
    /// Number of times this chunk has been executed
    execution_count: AtomicU32,

    /// Current JIT state
    state: AtomicU8,

    /// Pointer to native code (null if not yet compiled)
    /// Type is fn(*mut JitContext) -> () but stored as raw pointer for atomics
    native_code: AtomicPtr<()>,

    /// Size of generated native code in bytes (for memory tracking)
    code_size: AtomicU32,
}

impl JitProfile {
    /// Create a new cold profile
    pub const fn new() -> Self {
        JitProfile {
            execution_count: AtomicU32::new(0),
            state: AtomicU8::new(JitState::Cold as u8),
            native_code: AtomicPtr::new(std::ptr::null_mut()),
            code_size: AtomicU32::new(0),
        }
    }

    /// Record an execution and check if JIT compilation should be triggered
    ///
    /// Returns `true` if this call triggered the transition to Hot state,
    /// meaning the caller should initiate JIT compilation.
    #[inline]
    pub fn record_execution(&self) -> bool {
        let current_state = self.state();

        match current_state {
            JitState::Cold => {
                let count = self.execution_count.fetch_add(1, Ordering::Relaxed) + 1;
                if count >= WARM_THRESHOLD {
                    // Try to transition to Warming
                    let _ = self.state.compare_exchange(
                        JitState::Cold as u8,
                        JitState::Warming as u8,
                        Ordering::Release,
                        Ordering::Relaxed,
                    );
                }
                false
            }
            JitState::Warming => {
                let count = self.execution_count.fetch_add(1, Ordering::Relaxed) + 1;
                if count >= HOT_THRESHOLD {
                    // Try to transition to Hot - return true if we won the race
                    self.state
                        .compare_exchange(
                            JitState::Warming as u8,
                            JitState::Hot as u8,
                            Ordering::Release,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                } else {
                    false
                }
            }
            JitState::Hot | JitState::Compiling | JitState::Jitted | JitState::Failed => {
                // Keep counting for statistics but don't trigger transitions
                if self.execution_count.load(Ordering::Relaxed) < MAX_EXECUTION_COUNT {
                    self.execution_count.fetch_add(1, Ordering::Relaxed);
                }
                false
            }
        }
    }

    /// Get current execution count
    #[inline]
    pub fn execution_count(&self) -> u32 {
        self.execution_count.load(Ordering::Relaxed)
    }

    /// Get current JIT state
    #[inline]
    pub fn state(&self) -> JitState {
        JitState::from(self.state.load(Ordering::Acquire))
    }

    /// Check if chunk should use JIT code
    #[inline]
    pub fn should_use_jit(&self) -> bool {
        self.state() == JitState::Jitted
    }

    /// Check if chunk is hot and needs compilation
    #[inline]
    pub fn is_hot(&self) -> bool {
        self.state() == JitState::Hot
    }

    /// Try to transition from Hot to Compiling
    ///
    /// Returns true if this thread won the race to compile.
    /// The winner should proceed with compilation and call `set_compiled` on success
    /// or `set_failed` on failure.
    pub fn try_start_compiling(&self) -> bool {
        self.state
            .compare_exchange(
                JitState::Hot as u8,
                JitState::Compiling as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Set the compiled native code
    ///
    /// # Safety
    /// The function pointer must be valid for the lifetime of the JIT module
    /// and have the signature `fn(*mut JitContext) -> ()`
    pub unsafe fn set_compiled(&self, code: *const (), size: u32) {
        self.native_code.store(code as *mut (), Ordering::Release);
        self.code_size.store(size, Ordering::Release);
        self.state.store(JitState::Jitted as u8, Ordering::Release);
    }

    /// Mark compilation as failed
    ///
    /// The chunk will permanently use bytecode interpretation
    pub fn set_failed(&self) {
        self.state.store(JitState::Failed as u8, Ordering::Release);
    }

    /// Get the native code pointer
    ///
    /// Returns None if not yet compiled or compilation failed
    #[inline]
    pub fn native_code(&self) -> Option<*const ()> {
        if self.state() == JitState::Jitted {
            let ptr = self.native_code.load(Ordering::Acquire);
            if !ptr.is_null() {
                return Some(ptr as *const ());
            }
        }
        None
    }

    /// Get the native code as a callable function
    ///
    /// # Safety
    /// The caller must ensure JitContext is valid and properly initialized
    #[inline]
    pub unsafe fn get_native_fn(&self) -> Option<unsafe extern "C" fn(*mut super::JitContext)> {
        self.native_code().map(|ptr| {
            std::mem::transmute::<*const (), unsafe extern "C" fn(*mut super::JitContext)>(ptr)
        })
    }

    /// Get the size of generated code
    #[inline]
    pub fn code_size(&self) -> u32 {
        self.code_size.load(Ordering::Relaxed)
    }

    /// Reset the profile (for testing or recompilation)
    pub fn reset(&self) {
        self.execution_count.store(0, Ordering::Release);
        self.state.store(JitState::Cold as u8, Ordering::Release);
        self.native_code
            .store(std::ptr::null_mut(), Ordering::Release);
        self.code_size.store(0, Ordering::Release);
    }

    /// Force transition to Hot state (for testing)
    #[cfg(test)]
    pub fn force_hot(&self) {
        self.execution_count.store(HOT_THRESHOLD, Ordering::Release);
        self.state.store(JitState::Hot as u8, Ordering::Release);
    }
}

impl Default for JitProfile {
    fn default() -> Self {
        JitProfile::new()
    }
}

impl Clone for JitProfile {
    fn clone(&self) -> Self {
        JitProfile {
            execution_count: AtomicU32::new(self.execution_count.load(Ordering::Relaxed)),
            state: AtomicU8::new(self.state.load(Ordering::Relaxed)),
            native_code: AtomicPtr::new(self.native_code.load(Ordering::Relaxed)),
            code_size: AtomicU32::new(self.code_size.load(Ordering::Relaxed)),
        }
    }
}

// =============================================================================
// Statistics Collection
// =============================================================================

/// Aggregate statistics for JIT compilation
#[derive(Debug, Default, Clone)]
pub struct JitStats {
    /// Total number of chunks profiled
    pub total_chunks: usize,

    /// Number of chunks that reached Hot state
    pub hot_chunks: usize,

    /// Number of successfully JIT compiled chunks
    pub jitted_chunks: usize,

    /// Number of chunks where JIT compilation failed
    pub failed_chunks: usize,

    /// Total bytes of generated native code
    pub total_code_bytes: usize,

    /// Total execution count across all chunks
    pub total_executions: u64,
}

impl JitStats {
    /// Create empty statistics
    pub const fn new() -> Self {
        JitStats {
            total_chunks: 0,
            hot_chunks: 0,
            jitted_chunks: 0,
            failed_chunks: 0,
            total_code_bytes: 0,
            total_executions: 0,
        }
    }

    /// Add statistics from a profile
    pub fn add_profile(&mut self, profile: &JitProfile) {
        self.total_chunks += 1;
        self.total_executions += profile.execution_count() as u64;

        match profile.state() {
            JitState::Hot | JitState::Compiling => self.hot_chunks += 1,
            JitState::Jitted => {
                self.jitted_chunks += 1;
                self.total_code_bytes += profile.code_size() as usize;
            }
            JitState::Failed => self.failed_chunks += 1,
            _ => {}
        }
    }

    /// Calculate JIT coverage percentage
    pub fn coverage_percent(&self) -> f64 {
        if self.hot_chunks == 0 {
            0.0
        } else {
            (self.jitted_chunks as f64 / self.hot_chunks as f64) * 100.0
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_cold_to_warming() {
        let profile = JitProfile::new();
        assert_eq!(profile.state(), JitState::Cold);

        // Execute until warming threshold
        for _ in 0..WARM_THRESHOLD {
            assert!(!profile.record_execution());
        }

        assert_eq!(profile.state(), JitState::Warming);
        assert_eq!(profile.execution_count(), WARM_THRESHOLD);
    }

    #[test]
    fn test_profile_warming_to_hot() {
        let profile = JitProfile::new();

        // Execute until hot threshold
        let mut triggered = false;
        for _ in 0..HOT_THRESHOLD {
            if profile.record_execution() {
                triggered = true;
            }
        }

        assert!(triggered, "Should have triggered hot transition");
        assert_eq!(profile.state(), JitState::Hot);
        assert_eq!(profile.execution_count(), HOT_THRESHOLD);
    }

    #[test]
    fn test_profile_compilation_race() {
        let profile = JitProfile::new();
        profile.force_hot();

        // First attempt should win
        assert!(profile.try_start_compiling());
        assert_eq!(profile.state(), JitState::Compiling);

        // Second attempt should fail
        assert!(!profile.try_start_compiling());
    }

    // Dummy extern "C" function for testing
    extern "C" fn dummy_jit_code() {}

    #[test]
    fn test_profile_set_compiled() {
        let profile = JitProfile::new();
        profile.force_hot();
        assert!(profile.try_start_compiling());

        // Simulate compilation with dummy function
        let ptr = dummy_jit_code as *const ();
        unsafe {
            profile.set_compiled(ptr, 128);
        }

        assert_eq!(profile.state(), JitState::Jitted);
        assert!(profile.should_use_jit());
        assert_eq!(profile.code_size(), 128);
        assert!(profile.native_code().is_some());
    }

    #[test]
    fn test_profile_set_failed() {
        let profile = JitProfile::new();
        profile.force_hot();
        assert!(profile.try_start_compiling());

        profile.set_failed();

        assert_eq!(profile.state(), JitState::Failed);
        assert!(!profile.should_use_jit());
        assert!(profile.native_code().is_none());
    }

    #[test]
    fn test_profile_reset() {
        let profile = JitProfile::new();

        // Execute many times
        for _ in 0..200 {
            profile.record_execution();
        }

        profile.reset();

        assert_eq!(profile.state(), JitState::Cold);
        assert_eq!(profile.execution_count(), 0);
        assert!(profile.native_code().is_none());
    }

    #[test]
    fn test_jit_stats() {
        let mut stats = JitStats::new();

        let cold = JitProfile::new();
        stats.add_profile(&cold);

        let hot = JitProfile::new();
        hot.force_hot();
        stats.add_profile(&hot);

        assert_eq!(stats.total_chunks, 2);
        assert_eq!(stats.hot_chunks, 1);
        assert_eq!(stats.jitted_chunks, 0);
    }
}
