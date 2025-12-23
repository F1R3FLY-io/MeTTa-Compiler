//! Hybrid JIT/Bytecode Executor
//!
//! This module provides seamless execution that automatically switches
//! between JIT-compiled native code and bytecode VM interpretation based
//! on execution hotness and code characteristics.
//!
//! # Execution Strategy
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                     HybridExecutor.run()                            │
//! │                                                                     │
//! │  1. Check JitCache for compiled native code                         │
//! │     └─ If found → Execute JIT code                                  │
//! │                                                                     │
//! │  2. Track execution count via TieredCompiler                        │
//! │     └─ If hot threshold reached → Compile to JIT                    │
//! │                                                                     │
//! │  3. Execute:                                                        │
//! │     └─ JIT available → Native execution                             │
//! │     └─ JIT unavailable/bailed → Bytecode VM fallback                │
//! │                                                                     │
//! │  4. Handle bailout:                                                 │
//! │     └─ Transfer JIT stack → VM stack                                │
//! │     └─ Resume execution from bailout IP                             │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use mettatron::backend::bytecode::jit::hybrid::HybridExecutor;
//! use mettatron::backend::bytecode::compile_arc;
//!
//! let chunk = compile_arc("example", &expr)?;
//! let mut executor = HybridExecutor::new();
//! let results = executor.run(&chunk)?;
//! ```

use std::sync::Arc;
use tracing::{debug, trace, warn};

use crate::backend::bytecode::{
    BytecodeChunk, BytecodeVM, VmConfig, VmError, VmResult,
    MorkBridge,
};
use crate::backend::models::MettaValue;

use super::{
    JitCompiler, JitContext, JitValue, JitBailoutReason,
    JitCache, TieredCompiler, TieredStats, ChunkId, Tier,
    HOT_THRESHOLD, STAGE2_THRESHOLD,
    JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL, JIT_SIGNAL_BAILOUT,
    JIT_SIGNAL_ERROR, JIT_SIGNAL_HALT,
};

/// Default stack capacity for JIT execution
const JIT_STACK_CAPACITY: usize = 1024;

/// Default capacity for choice points in JIT nondeterminism
const JIT_CHOICE_POINT_CAPACITY: usize = 64;

/// Default capacity for results buffer in JIT nondeterminism
const JIT_RESULTS_CAPACITY: usize = 256;

/// Default capacity for binding frames in JIT
const JIT_BINDING_FRAMES_CAPACITY: usize = 32;

/// Default capacity for cut markers in JIT
const JIT_CUT_MARKERS_CAPACITY: usize = 16;

/// Hybrid executor configuration
#[derive(Debug, Clone)]
pub struct HybridConfig {
    /// Bytecode VM configuration
    pub vm_config: VmConfig,
    /// JIT value stack capacity
    pub jit_stack_capacity: usize,
    /// JIT choice point capacity
    pub jit_choice_point_capacity: usize,
    /// JIT results buffer capacity
    pub jit_results_capacity: usize,
    /// JIT binding frames capacity
    pub jit_binding_frames_capacity: usize,
    /// JIT cut markers capacity
    pub jit_cut_markers_capacity: usize,
    /// Whether to enable JIT compilation
    pub jit_enabled: bool,
    /// Whether to enable execution tracing
    pub trace: bool,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            vm_config: VmConfig::default(),
            jit_stack_capacity: JIT_STACK_CAPACITY,
            jit_choice_point_capacity: JIT_CHOICE_POINT_CAPACITY,
            jit_results_capacity: JIT_RESULTS_CAPACITY,
            jit_binding_frames_capacity: JIT_BINDING_FRAMES_CAPACITY,
            jit_cut_markers_capacity: JIT_CUT_MARKERS_CAPACITY,
            jit_enabled: super::JIT_ENABLED,
            trace: false,
        }
    }
}

impl HybridConfig {
    /// Create a configuration with tracing enabled
    pub fn with_trace(mut self) -> Self {
        self.trace = true;
        self.vm_config.trace = true;
        self
    }

    /// Create a configuration with JIT disabled (bytecode-only)
    pub fn bytecode_only() -> Self {
        Self {
            jit_enabled: false,
            ..Default::default()
        }
    }
}

/// Statistics for hybrid execution
#[derive(Debug, Clone, Default)]
pub struct HybridStats {
    /// Total number of run() calls
    pub total_runs: u64,
    /// Number of runs that used JIT
    pub jit_runs: u64,
    /// Number of runs that used bytecode VM
    pub vm_runs: u64,
    /// Number of JIT bailouts
    pub jit_bailouts: u64,
    /// Number of successful JIT compilations
    pub jit_compilations: u64,
    /// Number of failed JIT compilations
    pub jit_compile_failures: u64,
    /// Tiered compilation statistics
    pub tiered_stats: TieredStats,
}

impl HybridStats {
    /// Get the JIT hit rate as a percentage
    pub fn jit_hit_rate(&self) -> f64 {
        if self.total_runs == 0 {
            0.0
        } else {
            (self.jit_runs as f64 / self.total_runs as f64) * 100.0
        }
    }

    /// Get the bailout rate as a percentage of JIT runs
    pub fn bailout_rate(&self) -> f64 {
        if self.jit_runs == 0 {
            0.0
        } else {
            (self.jit_bailouts as f64 / self.jit_runs as f64) * 100.0
        }
    }
}

/// Hybrid JIT/Bytecode Executor
///
/// This executor seamlessly switches between JIT-compiled native code
/// and bytecode VM interpretation based on execution hotness.
///
/// # Features
///
/// - **Automatic tier progression**: Cold → Bytecode → JIT Stage 1 → JIT Stage 2
/// - **JIT code caching**: Compiled code is cached and reused
/// - **Bailout handling**: Graceful fallback from JIT to VM on unsupported operations
/// - **Nondeterminism support**: Full support for Fork/Yield/Fail/Collect
///
/// # Thread Safety
///
/// The executor itself is not thread-safe, but the underlying JitCache
/// and TieredCompiler use internal locking for safe concurrent access.
pub struct HybridExecutor {
    /// JIT code cache (shared across executors)
    jit_cache: Arc<JitCache>,
    /// Tiered compiler for tier management
    tiered_compiler: Arc<TieredCompiler>,
    /// Configuration
    config: HybridConfig,
    /// Execution statistics
    stats: HybridStats,
    /// Reusable JIT value stack
    jit_stack: Vec<JitValue>,
    /// Reusable JIT choice points buffer
    jit_choice_points: Vec<super::JitChoicePoint>,
    /// Reusable JIT results buffer
    jit_results: Vec<JitValue>,
    /// Reusable JIT binding frames buffer
    jit_binding_frames: Vec<super::JitBindingFrame>,
    /// Reusable JIT cut markers buffer (for proper cut scope tracking)
    jit_cut_markers: Vec<usize>,
    /// Heap allocation tracker for cleanup (prevents memory leaks)
    heap_tracker: Vec<*mut MettaValue>,
    /// Optional MORK bridge for rule dispatch
    bridge: Option<Arc<MorkBridge>>,
    /// Optional external function registry for CallExternal
    external_registry: Option<*const ()>,
    /// Optional memo cache for CallCached
    memo_cache: Option<*const ()>,
    /// Optional space registry for named spaces
    space_registry: Option<*mut ()>,
    /// Optional environment pointer for state operations
    env: Option<*mut ()>,
    /// Pre-resolved grounded space handles [&self, &kb, &stack]
    /// These are resolved once at executor setup and reused for all JIT calls.
    grounded_spaces: [*const (); 3],
    /// Buffer for storing SpaceHandle instances (keeps them alive)
    grounded_space_storage: Vec<crate::backend::models::SpaceHandle>,
    /// Template results buffer for space match instantiation
    template_results: Vec<JitValue>,
    /// Stack save pool for Fork operations (Optimization 5.2)
    /// Pre-allocated ring buffer of stack snapshots, eliminating Box::leak() allocations
    jit_stack_save_pool: Vec<JitValue>,
}

impl HybridExecutor {
    /// Create a new HybridExecutor with default configuration
    pub fn new() -> Self {
        Self::with_config(HybridConfig::default())
    }

    /// Create a new HybridExecutor with custom configuration
    pub fn with_config(config: HybridConfig) -> Self {
        // Optimization 5.2: Pre-allocate stack save pool
        // Pool size = STACK_SAVE_POOL_SIZE slots × MAX_STACK_SAVE_VALUES values per slot
        let pool_capacity = super::STACK_SAVE_POOL_SIZE * super::MAX_STACK_SAVE_VALUES;

        Self {
            jit_cache: Arc::new(JitCache::new()),
            tiered_compiler: Arc::new(TieredCompiler::new()),
            jit_stack: vec![JitValue::nil(); config.jit_stack_capacity],
            jit_choice_points: Vec::with_capacity(config.jit_choice_point_capacity),
            jit_results: Vec::with_capacity(config.jit_results_capacity),
            jit_binding_frames: Vec::with_capacity(config.jit_binding_frames_capacity),
            jit_cut_markers: Vec::with_capacity(config.jit_cut_markers_capacity),
            heap_tracker: Vec::with_capacity(64), // Reasonable default for heap allocations
            stats: HybridStats::default(),
            bridge: None,
            external_registry: None,
            memo_cache: None,
            space_registry: None,
            env: None,
            grounded_spaces: [std::ptr::null(); 3],
            grounded_space_storage: Vec::with_capacity(3),
            template_results: Vec::with_capacity(64),
            jit_stack_save_pool: vec![JitValue::nil(); pool_capacity],
            config,
        }
    }

    /// Create a HybridExecutor that shares cache with another executor
    pub fn with_shared_cache(cache: Arc<JitCache>, compiler: Arc<TieredCompiler>) -> Self {
        let config = HybridConfig::default();
        // Optimization 5.2: Pre-allocate stack save pool
        let pool_capacity = super::STACK_SAVE_POOL_SIZE * super::MAX_STACK_SAVE_VALUES;

        Self {
            jit_cache: cache,
            tiered_compiler: compiler,
            jit_stack: vec![JitValue::nil(); config.jit_stack_capacity],
            jit_choice_points: Vec::with_capacity(config.jit_choice_point_capacity),
            jit_results: Vec::with_capacity(config.jit_results_capacity),
            jit_binding_frames: Vec::with_capacity(config.jit_binding_frames_capacity),
            jit_cut_markers: Vec::with_capacity(config.jit_cut_markers_capacity),
            heap_tracker: Vec::with_capacity(64), // Reasonable default for heap allocations
            stats: HybridStats::default(),
            bridge: None,
            external_registry: None,
            memo_cache: None,
            space_registry: None,
            env: None,
            grounded_spaces: [std::ptr::null(); 3],
            grounded_space_storage: Vec::with_capacity(3),
            template_results: Vec::with_capacity(64),
            jit_stack_save_pool: vec![JitValue::nil(); pool_capacity],
            config,
        }
    }

    /// Set the MORK bridge for rule dispatch
    pub fn set_bridge(&mut self, bridge: Arc<MorkBridge>) {
        self.bridge = Some(bridge);
    }

    /// Set the external function registry for CallExternal opcode
    ///
    /// # Safety
    /// The caller must ensure the pointer remains valid for the lifetime of the executor.
    pub unsafe fn set_external_registry(&mut self, registry: *const ()) {
        self.external_registry = Some(registry);
    }

    /// Set the memo cache for CallCached opcode
    ///
    /// # Safety
    /// The caller must ensure the pointer remains valid for the lifetime of the executor.
    pub unsafe fn set_memo_cache(&mut self, cache: *const ()) {
        self.memo_cache = Some(cache);
    }

    /// Set the space registry for named space operations
    ///
    /// # Safety
    /// The caller must ensure the pointer remains valid for the lifetime of the executor.
    pub unsafe fn set_space_registry(&mut self, registry: *mut ()) {
        self.space_registry = Some(registry);
    }

    /// Set the environment for state operations (new-state, get-state, change-state!)
    ///
    /// # Safety
    /// The caller must ensure the pointer remains valid for the lifetime of the executor.
    pub unsafe fn set_env(&mut self, env: *mut ()) {
        self.env = Some(env);
    }

    /// Pre-resolve grounded space references (&self, &kb, &stack)
    ///
    /// Call this method after setting the space registry to pre-resolve grounded
    /// references for O(1) access during JIT execution. The resolved spaces are
    /// stored internally and wired to the JIT context at execution time.
    ///
    /// # Arguments
    /// - `self_space`: Optional SpaceHandle for &self (index 0)
    /// - `kb_space`: Optional SpaceHandle for &kb (index 1)
    /// - `stack_space`: Optional SpaceHandle for &stack (index 2)
    ///
    /// # Safety
    /// The SpaceHandle instances must remain valid for the lifetime of the executor.
    /// This is guaranteed by storing them in `grounded_space_storage`.
    pub fn set_grounded_spaces(
        &mut self,
        self_space: Option<crate::backend::models::SpaceHandle>,
        kb_space: Option<crate::backend::models::SpaceHandle>,
        stack_space: Option<crate::backend::models::SpaceHandle>,
    ) {
        // Clear existing storage
        self.grounded_space_storage.clear();
        self.grounded_spaces = [std::ptr::null(); 3];

        // Store and get pointers for each space
        if let Some(space) = self_space {
            self.grounded_space_storage.push(space);
            let idx = self.grounded_space_storage.len() - 1;
            self.grounded_spaces[0] = &self.grounded_space_storage[idx] as *const _ as *const ();
        }

        if let Some(space) = kb_space {
            self.grounded_space_storage.push(space);
            let idx = self.grounded_space_storage.len() - 1;
            self.grounded_spaces[1] = &self.grounded_space_storage[idx] as *const _ as *const ();
        }

        if let Some(space) = stack_space {
            self.grounded_space_storage.push(space);
            let idx = self.grounded_space_storage.len() - 1;
            self.grounded_spaces[2] = &self.grounded_space_storage[idx] as *const _ as *const ();
        }
    }

    /// Check if grounded spaces are configured
    pub fn has_grounded_spaces(&self) -> bool {
        self.grounded_spaces.iter().any(|p| !p.is_null())
    }

    /// Get execution statistics
    pub fn stats(&self) -> &HybridStats {
        &self.stats
    }

    /// Get a clone of the JIT cache (for sharing)
    pub fn jit_cache(&self) -> Arc<JitCache> {
        Arc::clone(&self.jit_cache)
    }

    /// Get a clone of the tiered compiler (for sharing)
    pub fn tiered_compiler(&self) -> Arc<TieredCompiler> {
        Arc::clone(&self.tiered_compiler)
    }

    /// Run a bytecode chunk with hybrid execution and native backtracking
    ///
    /// This variant uses the dispatcher loop for native nondeterminism support.
    /// Fork/Yield/Fail operations are handled natively in JIT without bailing
    /// to the bytecode VM.
    ///
    /// Note: This requires the JIT code to support re-entry at resume_ip for
    /// full backtracking support. For simple Fork patterns at the beginning
    /// of execution, this works correctly.
    pub fn run_with_backtracking(&mut self, chunk: &Arc<BytecodeChunk>) -> VmResult<Vec<MettaValue>> {
        self.stats.total_runs += 1;

        if !self.config.jit_enabled {
            // JIT disabled - always use VM
            return self.run_vm(chunk);
        }

        let chunk_id = ChunkId::from_chunk(chunk);

        // Check if we have cached JIT code
        if let Some(native_ptr) = self.jit_cache.get(&chunk_id) {
            // Execute cached JIT code with dispatcher loop
            return self.execute_jit_with_backtracking(chunk, native_ptr);
        }

        // Record execution and check tier
        self.tiered_compiler.record_execution(chunk);
        let tier = self.tiered_compiler.get_tier(chunk);

        match tier {
            Tier::Interpreter | Tier::Bytecode => {
                // Not hot enough for JIT - use VM
                self.run_vm(chunk)
            }
            Tier::JitStage1 | Tier::JitStage2 => {
                // Hot enough for JIT - try to compile
                if let Some(native_ptr) = self.try_compile(chunk, &chunk_id, tier) {
                    self.execute_jit_with_backtracking(chunk, native_ptr)
                } else {
                    // Compilation failed or not possible - use VM
                    self.run_vm(chunk)
                }
            }
        }
    }

    /// Run a bytecode chunk with hybrid execution
    ///
    /// This is the main entry point for execution. It automatically:
    /// 1. Checks for cached JIT code
    /// 2. Tracks execution for tier progression
    /// 3. Compiles to JIT if hot
    /// 4. Executes via JIT or VM as appropriate
    /// 5. Handles bailout transitions
    pub fn run(&mut self, chunk: &Arc<BytecodeChunk>) -> VmResult<Vec<MettaValue>> {
        self.stats.total_runs += 1;

        if !self.config.jit_enabled {
            // JIT disabled - always use VM
            return self.run_vm(chunk);
        }

        let chunk_id = ChunkId::from_chunk(chunk);

        // Check if we have cached JIT code
        if let Some(native_ptr) = self.jit_cache.get(&chunk_id) {
            // Execute cached JIT code
            return self.execute_jit(chunk, native_ptr);
        }

        // Record execution and check tier
        self.tiered_compiler.record_execution(chunk);
        let tier = self.tiered_compiler.get_tier(chunk);

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::execute", ?chunk_id, ?tier, "Executing chunk");
        }

        match tier {
            Tier::Interpreter | Tier::Bytecode => {
                // Not hot enough for JIT - use VM
                self.run_vm(chunk)
            }
            Tier::JitStage1 | Tier::JitStage2 => {
                // Hot enough for JIT - try to compile
                if let Some(native_ptr) = self.try_compile(chunk, &chunk_id, tier) {
                    self.execute_jit(chunk, native_ptr)
                } else {
                    // Compilation failed or not possible - use VM
                    self.run_vm(chunk)
                }
            }
        }
    }

    /// Run using the bytecode VM only
    fn run_vm(&mut self, chunk: &Arc<BytecodeChunk>) -> VmResult<Vec<MettaValue>> {
        self.stats.vm_runs += 1;
        self.stats.tiered_stats.bytecode_runs += 1;

        let vm = if let Some(ref bridge) = self.bridge {
            BytecodeVM::with_config_and_bridge(
                Arc::clone(chunk),
                self.config.vm_config.clone(),
                Arc::clone(bridge),
            )
        } else {
            BytecodeVM::with_config(Arc::clone(chunk), self.config.vm_config.clone())
        };
        let mut vm = vm;
        vm.run()
    }

    /// Try to compile a chunk to JIT
    fn try_compile(
        &mut self,
        chunk: &Arc<BytecodeChunk>,
        chunk_id: &ChunkId,
        tier: Tier,
    ) -> Option<*const ()> {
        // Check if chunk can be JIT compiled
        if !chunk.can_jit_compile() {
            if self.config.trace {
                debug!(target: "mettatron::jit::hybrid::compile", "Chunk cannot be JIT compiled");
            }
            return None;
        }

        // Try to start compilation (prevents concurrent compilation)
        if !chunk.jit_profile().try_start_compiling() {
            // Another thread is compiling - check if done
            if chunk.has_jit_code() {
                return self.jit_cache.get(chunk_id);
            }
            return None;
        }

        // Compile the chunk
        match JitCompiler::new() {
            Ok(mut compiler) => {
                match compiler.compile(chunk) {
                    Ok(code_ptr) => {
                        self.stats.jit_compilations += 1;

                        // Store in chunk's profile
                        unsafe {
                            chunk.jit_profile().set_compiled(code_ptr, 0);
                        }

                        // Cache for future use
                        let entry = super::tiered::CacheEntry {
                            native_code: code_ptr,
                            code_size: 0, // Size tracking not implemented yet
                            profile: self.tiered_compiler.get_or_create_profile(chunk),
                            tier,
                            last_access: std::time::Instant::now(),
                        };
                        self.jit_cache.insert(*chunk_id, entry);

                        if self.config.trace {
                            debug!(target: "mettatron::jit::hybrid::compile", ?chunk_id, ?tier, "Compiled chunk");
                        }

                        Some(code_ptr)
                    }
                    Err(e) => {
                        self.stats.jit_compile_failures += 1;
                        chunk.jit_profile().set_failed();

                        if self.config.trace {
                            warn!(target: "mettatron::jit::hybrid::compile", error = ?e, "JIT compilation failed");
                        }
                        None
                    }
                }
            }
            Err(e) => {
                self.stats.jit_compile_failures += 1;
                chunk.jit_profile().set_failed();

                if self.config.trace {
                    warn!(target: "mettatron::jit::hybrid::compile", error = ?e, "Failed to create JIT compiler");
                }
                None
            }
        }
    }

    /// Execute JIT-compiled native code
    fn execute_jit(
        &mut self,
        chunk: &Arc<BytecodeChunk>,
        native_ptr: *const (),
    ) -> VmResult<Vec<MettaValue>> {
        self.stats.jit_runs += 1;
        if chunk.jit_profile().execution_count() >= STAGE2_THRESHOLD {
            self.stats.tiered_stats.jit_stage2_runs += 1;
        } else {
            self.stats.tiered_stats.jit_stage1_runs += 1;
        }

        // Reset buffers
        for v in &mut self.jit_stack {
            *v = JitValue::nil();
        }
        self.jit_choice_points.clear();
        self.jit_results.clear();
        self.jit_binding_frames.clear();
        self.jit_cut_markers.clear();
        self.heap_tracker.clear();

        // Ensure capacity
        self.jit_choice_points.resize(
            self.config.jit_choice_point_capacity,
            super::JitChoicePoint::default(),
        );
        self.jit_results.resize(
            self.config.jit_results_capacity,
            JitValue::nil(),
        );
        self.jit_binding_frames.resize(
            self.config.jit_binding_frames_capacity,
            super::JitBindingFrame::default(),
        );
        self.jit_cut_markers.resize(
            self.config.jit_cut_markers_capacity,
            0,
        );

        let constants = chunk.constants();

        // Create JIT context with full nondeterminism support
        // SAFETY: All buffers are valid for the lifetime of this function call
        let mut ctx = unsafe {
            JitContext::with_nondet(
                self.jit_stack.as_mut_ptr(),
                self.config.jit_stack_capacity,
                constants.as_ptr(),
                constants.len(),
                self.jit_choice_points.as_mut_ptr(),
                self.config.jit_choice_point_capacity,
                self.jit_results.as_mut_ptr(),
                self.config.jit_results_capacity,
            )
        };

        // Set up binding frames
        ctx.binding_frames = self.jit_binding_frames.as_mut_ptr();
        ctx.binding_frames_count = 0;
        ctx.binding_frames_cap = self.config.jit_binding_frames_capacity;

        // Set up cut markers for proper cut scope tracking
        ctx.cut_markers = self.jit_cut_markers.as_mut_ptr();
        ctx.cut_marker_count = 0;
        ctx.cut_marker_cap = self.config.jit_cut_markers_capacity;

        // Set up bridge pointer if available
        if let Some(ref bridge) = self.bridge {
            ctx.bridge_ptr = Arc::as_ptr(bridge) as *const ();
        }

        // Set up external registry if available
        if let Some(registry) = self.external_registry {
            ctx.external_registry = registry;
        }

        // Set up memo cache if available
        if let Some(cache) = self.memo_cache {
            ctx.memo_cache = cache;
        }

        // Set up space registry if available
        if let Some(registry) = self.space_registry {
            ctx.space_registry = registry;
        }

        // Set up environment for state operations (Phase D.1)
        if let Some(env) = self.env {
            unsafe {
                ctx.set_env(env);
            }
        }

        // Set up grounded spaces if configured (Space Ops - Phase 2)
        if self.has_grounded_spaces() {
            unsafe {
                ctx.set_grounded_spaces(self.grounded_spaces.as_ptr(), 3);
            }
        }

        // Set up template results buffer (Space Ops - Phase 2)
        unsafe {
            ctx.set_template_results(self.template_results.as_mut_ptr(), self.template_results.capacity());
        }

        // Set up stack save pool (Optimization 5.2)
        let pool_cap = super::STACK_SAVE_POOL_SIZE * super::MAX_STACK_SAVE_VALUES;
        ctx.stack_save_pool = self.jit_stack_save_pool.as_mut_ptr();
        ctx.stack_save_pool_cap = pool_cap;
        ctx.stack_save_pool_next = 0;

        // Set current chunk pointer
        ctx.current_chunk = Arc::as_ptr(chunk) as *const ();

        // Enable heap tracking for cleanup
        unsafe {
            ctx.enable_heap_tracking(&mut self.heap_tracker as *mut Vec<*mut MettaValue>);
        }

        // Cast and call native function
        // The JIT-compiled function returns the result as i64 (NaN-boxed JitValue)
        let native_fn: extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(native_ptr) };

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::execute", native_ptr = ?native_ptr, "Executing JIT code");
        }

        let jit_result = native_fn(&mut ctx);

        // Check for bailout
        if ctx.bailout {
            self.stats.jit_bailouts += 1;

            if self.config.trace {
                debug!(target: "mettatron::jit::hybrid::execute", bailout_ip = ctx.bailout_ip, reason = ?ctx.bailout_reason, "JIT bailout");
            }

            // Transfer JIT stack to VM and resume
            let mut vm_stack = Vec::with_capacity(ctx.sp);
            for i in 0..ctx.sp {
                let jit_val = unsafe { *ctx.value_stack.add(i) };
                let metta_val = unsafe { jit_val.to_metta() };
                vm_stack.push(metta_val);
            }

            // Cleanup heap allocations before bailout
            unsafe {
                ctx.cleanup_heap_allocations();
            }

            // Resume from bailout point
            let mut vm = if let Some(ref bridge) = self.bridge {
                BytecodeVM::with_config_and_bridge(
                    Arc::clone(chunk),
                    self.config.vm_config.clone(),
                    Arc::clone(bridge),
                )
            } else {
                BytecodeVM::with_config(Arc::clone(chunk), self.config.vm_config.clone())
            };
            return vm.resume_from_bailout(ctx.bailout_ip, vm_stack);
        }

        // Collect results - prioritize yielded results, then return value, then stack
        let results = self.collect_jit_results_with_return(&ctx, jit_result);

        // Cleanup heap allocations
        unsafe {
            ctx.cleanup_heap_allocations();
        }

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::execute", results_count = results.len(), "JIT execution complete");
        }

        Ok(results)
    }

    /// Execute JIT code with native backtracking support
    ///
    /// This is the dispatcher loop that handles Fork/Yield/Fail natively
    /// without bailing out to the bytecode VM.
    ///
    /// # Dispatcher Loop Architecture
    ///
    /// ```text
    /// loop {
    ///     1. Execute JIT code
    ///     2. If bailout → fall back to VM
    ///     3. Collect any yielded results
    ///     4. If choice points exist → backtrack (call fail_native)
    ///     5. If no more alternatives → done
    /// }
    /// ```
    fn execute_jit_with_backtracking(
        &mut self,
        chunk: &Arc<BytecodeChunk>,
        native_ptr: *const (),
    ) -> VmResult<Vec<MettaValue>> {
        self.stats.jit_runs += 1;
        if chunk.jit_profile().execution_count() >= STAGE2_THRESHOLD {
            self.stats.tiered_stats.jit_stage2_runs += 1;
        } else {
            self.stats.tiered_stats.jit_stage1_runs += 1;
        }

        // Reset buffers
        for v in &mut self.jit_stack {
            *v = JitValue::nil();
        }
        self.jit_choice_points.clear();
        self.jit_results.clear();
        self.jit_binding_frames.clear();
        self.jit_cut_markers.clear();
        self.heap_tracker.clear();

        // Ensure capacity
        self.jit_choice_points.resize(
            self.config.jit_choice_point_capacity,
            super::JitChoicePoint::default(),
        );
        self.jit_results.resize(
            self.config.jit_results_capacity,
            JitValue::nil(),
        );
        self.jit_binding_frames.resize(
            self.config.jit_binding_frames_capacity,
            super::JitBindingFrame::default(),
        );
        self.jit_cut_markers.resize(
            self.config.jit_cut_markers_capacity,
            0,
        );

        let constants = chunk.constants();

        // Create JIT context with full nondeterminism support
        let mut ctx = unsafe {
            JitContext::with_nondet(
                self.jit_stack.as_mut_ptr(),
                self.config.jit_stack_capacity,
                constants.as_ptr(),
                constants.len(),
                self.jit_choice_points.as_mut_ptr(),
                self.config.jit_choice_point_capacity,
                self.jit_results.as_mut_ptr(),
                self.config.jit_results_capacity,
            )
        };

        // Set up binding frames
        ctx.binding_frames = self.jit_binding_frames.as_mut_ptr();
        ctx.binding_frames_count = 0;
        ctx.binding_frames_cap = self.config.jit_binding_frames_capacity;

        // Set up cut markers for proper cut scope tracking
        ctx.cut_markers = self.jit_cut_markers.as_mut_ptr();
        ctx.cut_marker_count = 0;
        ctx.cut_marker_cap = self.config.jit_cut_markers_capacity;

        // Set up bridge pointer if available
        if let Some(ref bridge) = self.bridge {
            ctx.bridge_ptr = Arc::as_ptr(bridge) as *const ();
        }

        // Set up external registry if available
        if let Some(registry) = self.external_registry {
            ctx.external_registry = registry;
        }

        // Set up memo cache if available
        if let Some(cache) = self.memo_cache {
            ctx.memo_cache = cache;
        }

        // Set up space registry if available
        if let Some(registry) = self.space_registry {
            ctx.space_registry = registry;
        }

        // Set up environment for state operations (Phase D.1)
        if let Some(env) = self.env {
            unsafe {
                ctx.set_env(env);
            }
        }

        // Set up grounded spaces if configured (Space Ops - Phase 2)
        if self.has_grounded_spaces() {
            unsafe {
                ctx.set_grounded_spaces(self.grounded_spaces.as_ptr(), 3);
            }
        }

        // Set up template results buffer (Space Ops - Phase 2)
        unsafe {
            ctx.set_template_results(self.template_results.as_mut_ptr(), self.template_results.capacity());
        }

        // Set up stack save pool (Optimization 5.2)
        let pool_cap = super::STACK_SAVE_POOL_SIZE * super::MAX_STACK_SAVE_VALUES;
        ctx.stack_save_pool = self.jit_stack_save_pool.as_mut_ptr();
        ctx.stack_save_pool_cap = pool_cap;
        ctx.stack_save_pool_next = 0;

        // Set current chunk pointer
        ctx.current_chunk = Arc::as_ptr(chunk) as *const ();

        // Enable heap tracking for cleanup
        unsafe {
            ctx.enable_heap_tracking(&mut self.heap_tracker as *mut Vec<*mut MettaValue>);
        }

        // Cast native function pointer
        // The JIT-compiled function returns the result as i64 (NaN-boxed JitValue)
        let native_fn: extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(native_ptr) };

        // Collected results from all branches
        let mut all_results: Vec<MettaValue> = Vec::new();

        // Maximum iterations to prevent infinite loops
        const MAX_ITERATIONS: usize = 10000;
        let mut iteration = 0;

        // Dispatcher loop
        loop {
            iteration += 1;
            if iteration > MAX_ITERATIONS {
                // Cleanup heap allocations before error return
                unsafe {
                    ctx.cleanup_heap_allocations();
                }
                return Err(VmError::Runtime(
                    "Maximum backtracking iterations exceeded".to_string()
                ));
            }

            if self.config.trace {
                trace!(target: "mettatron::jit::hybrid::backtrack", iteration, choice_points = ctx.choice_point_count, results = ctx.results_count, "Dispatcher iteration");
            }

            // Execute JIT code and capture return value
            let jit_result = native_fn(&mut ctx);

            // Check for bailout
            if ctx.bailout {
                self.stats.jit_bailouts += 1;

                if self.config.trace {
                    debug!(target: "mettatron::jit::hybrid::backtrack", bailout_ip = ctx.bailout_ip, reason = ?ctx.bailout_reason, "JIT bailout during backtracking");
                }

                // Transfer JIT stack to VM and resume
                let mut vm_stack = Vec::with_capacity(ctx.sp);
                for i in 0..ctx.sp {
                    let jit_val = unsafe { *ctx.value_stack.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    vm_stack.push(metta_val);
                }

                // Resume from bailout point in VM
                let mut vm = if let Some(ref bridge) = self.bridge {
                    BytecodeVM::with_config_and_bridge(
                        Arc::clone(chunk),
                        self.config.vm_config.clone(),
                        Arc::clone(bridge),
                    )
                } else {
                    BytecodeVM::with_config(Arc::clone(chunk), self.config.vm_config.clone())
                };

                // Get VM results and combine with any already collected JIT results
                let vm_results = vm.resume_from_bailout(ctx.bailout_ip, vm_stack)?;
                all_results.extend(vm_results);
                break;
            }

            // Collect any results from the results buffer
            // (populated by Yield operations)
            if ctx.results_count > 0 {
                for i in 0..ctx.results_count {
                    let jit_val = unsafe { *ctx.results.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    all_results.push(metta_val);
                }
                // Reset results count for next iteration
                ctx.results_count = 0;
            }

            // If there are choice points, backtrack to try next alternative
            if ctx.choice_point_count > 0 {
                // Call fail_native to get next alternative
                let next_val = unsafe {
                    super::runtime::jit_runtime_fail_native(&mut ctx)
                };

                // Check if fail_native returned FAIL signal (no more alternatives)
                if next_val == JIT_SIGNAL_FAIL as u64 {
                    // No more alternatives in current choice point
                    // Pop the exhausted choice point
                    if ctx.choice_point_count > 0 {
                        ctx.choice_point_count -= 1;
                    }

                    // If still have choice points, continue backtracking
                    if ctx.choice_point_count > 0 {
                        continue;
                    }
                    // Otherwise, all alternatives exhausted
                    break;
                }

                // Got a valid next alternative - push it and continue
                let next_jit_val = JitValue::from_raw(next_val);
                if ctx.sp < ctx.stack_cap {
                    unsafe {
                        *ctx.value_stack.add(ctx.sp) = next_jit_val;
                    }
                    ctx.sp += 1;
                }

                // Continue to execute with new alternative
                continue;
            }

            // No choice points and no bailout - execution complete
            // Use the return value from the JIT function
            if jit_result != 0 {
                let jit_val = JitValue::from_raw(jit_result as u64);
                let metta_val = unsafe { jit_val.to_metta() };
                all_results.push(metta_val);
            } else if ctx.sp > 0 {
                // Fallback to stack if return value is 0
                for i in 0..ctx.sp {
                    let jit_val = unsafe { *ctx.value_stack.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    all_results.push(metta_val);
                }
            }
            break;
        }

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::backtrack", iterations = iteration, results_count = all_results.len(), "Dispatcher complete");
        }

        // Cleanup heap allocations
        unsafe {
            ctx.cleanup_heap_allocations();
        }

        // Return collected results or Unit if empty
        if all_results.is_empty() {
            Ok(vec![MettaValue::Unit])
        } else {
            Ok(all_results)
        }
    }

    /// Collect results from JIT context
    fn collect_jit_results(&self, ctx: &JitContext) -> Vec<MettaValue> {
        // If there are collected results (from nondeterminism), use those
        if ctx.results_count > 0 {
            let mut results = Vec::with_capacity(ctx.results_count);
            for i in 0..ctx.results_count {
                let jit_val = unsafe { *ctx.results.add(i) };
                let metta_val = unsafe { jit_val.to_metta() };
                results.push(metta_val);
            }
            return results;
        }

        // Otherwise, collect from stack
        if ctx.sp > 0 {
            let mut results = Vec::with_capacity(ctx.sp);
            for i in 0..ctx.sp {
                let jit_val = unsafe { *ctx.value_stack.add(i) };
                let metta_val = unsafe { jit_val.to_metta() };
                results.push(metta_val);
            }
            results
        } else {
            vec![MettaValue::Unit]
        }
    }

    /// Collect results from JIT context, prioritizing the function return value
    ///
    /// This method handles the fact that JIT-compiled functions return the result
    /// directly as the function return value (NaN-boxed i64).
    fn collect_jit_results_with_return(&self, ctx: &JitContext, jit_result: i64) -> Vec<MettaValue> {
        // If there are collected results (from nondeterminism), use those
        if ctx.results_count > 0 {
            let mut results = Vec::with_capacity(ctx.results_count);
            for i in 0..ctx.results_count {
                let jit_val = unsafe { *ctx.results.add(i) };
                let metta_val = unsafe { jit_val.to_metta() };
                results.push(metta_val);
            }
            return results;
        }

        // Use the function return value if non-zero
        // The return value is a NaN-boxed JitValue
        if jit_result != 0 {
            let jit_val = JitValue::from_raw(jit_result as u64);
            let metta_val = unsafe { jit_val.to_metta() };
            return vec![metta_val];
        }

        // Fallback to stack
        if ctx.sp > 0 {
            let mut results = Vec::with_capacity(ctx.sp);
            for i in 0..ctx.sp {
                let jit_val = unsafe { *ctx.value_stack.add(i) };
                let metta_val = unsafe { jit_val.to_metta() };
                results.push(metta_val);
            }
            results
        } else {
            vec![MettaValue::Unit]
        }
    }

    /// Clear all caches and reset statistics
    pub fn reset(&mut self) {
        self.jit_cache.clear();
        self.stats = HybridStats::default();
    }
}

impl Default for HybridExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HybridExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HybridExecutor")
            .field("config", &self.config)
            .field("stats", &self.stats)
            .field("jit_cache_size", &self.jit_cache.len())
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::{ChunkBuilder, Opcode, compile_arc};

    #[test]
    fn test_hybrid_executor_new() {
        let executor = HybridExecutor::new();
        assert_eq!(executor.stats().total_runs, 0);
        assert_eq!(executor.stats().jit_runs, 0);
        assert_eq!(executor.stats().vm_runs, 0);
    }

    #[test]
    fn test_hybrid_executor_bytecode_only() {
        let config = HybridConfig::bytecode_only();
        let mut executor = HybridExecutor::with_config(config);

        // Build simple chunk: push 42, return
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();

        let results = executor.run(&chunk).expect("execution should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));

        // Should have used VM, not JIT
        assert_eq!(executor.stats().vm_runs, 1);
        assert_eq!(executor.stats().jit_runs, 0);
    }

    #[test]
    fn test_hybrid_executor_simple_arithmetic() {
        let mut executor = HybridExecutor::new();

        // Build: 10 + 32 = 42
        let mut builder = ChunkBuilder::new("test");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 32);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();

        let results = executor.run(&chunk).expect("execution should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_hybrid_executor_shared_cache() {
        let executor1 = HybridExecutor::new();
        let cache = executor1.jit_cache();
        let compiler = executor1.tiered_compiler();

        let mut executor2 = HybridExecutor::with_shared_cache(cache.clone(), compiler.clone());

        // Both executors should share the same cache
        assert!(Arc::ptr_eq(&executor1.jit_cache(), &executor2.jit_cache()));

        // Build and run a chunk
        let mut builder = ChunkBuilder::new("shared");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();

        let results = executor2.run(&chunk).expect("should succeed");
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_hybrid_stats() {
        let mut stats = HybridStats::default();
        assert_eq!(stats.jit_hit_rate(), 0.0);
        assert_eq!(stats.bailout_rate(), 0.0);

        stats.total_runs = 100;
        stats.jit_runs = 75;
        stats.vm_runs = 25;
        stats.jit_bailouts = 5;

        assert_eq!(stats.jit_hit_rate(), 75.0);
        assert!((stats.bailout_rate() - 6.666).abs() < 0.01);
    }

    #[test]
    fn test_hybrid_config_with_trace() {
        let config = HybridConfig::default().with_trace();
        assert!(config.trace);
        assert!(config.vm_config.trace);
    }

    #[test]
    fn test_hybrid_executor_compile_integration() {
        let mut executor = HybridExecutor::new();

        // Use the compile_arc function for a simple expression
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(20),
            MettaValue::Long(22),
        ]);

        match crate::backend::bytecode::compile_arc("test", &expr) {
            Ok(chunk) => {
                let results = executor.run(&chunk).expect("should succeed");
                assert_eq!(results.len(), 1);
                assert_eq!(results[0], MettaValue::Long(42));
            }
            Err(_) => {
                // Compilation might fail in some test configurations
            }
        }
    }

    #[test]
    fn test_hybrid_executor_run_with_backtracking_simple() {
        // Test that run_with_backtracking works for simple non-forking code
        let mut executor = HybridExecutor::new();

        // Build simple chunk: push 42, return
        let mut builder = ChunkBuilder::new("test_backtrack_simple");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();

        // First verify the bytecode works via VM
        let vm_results = executor.run(&chunk).expect("VM should succeed");
        assert_eq!(vm_results.len(), 1, "VM should return 1 result");
        assert_eq!(vm_results[0], MettaValue::Long(42), "VM should return Long(42)");

        // Now use run_with_backtracking (uses VM since not JIT-compiled yet)
        let results = executor.run_with_backtracking(&chunk).expect("should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_hybrid_executor_run_with_backtracking_arithmetic() {
        // Test dispatcher loop with arithmetic (no nondeterminism)
        let mut executor = HybridExecutor::new();

        // Build: 10 + 32 = 42
        let mut builder = ChunkBuilder::new("test_backtrack_arith");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 32);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build_arc();

        // First verify the bytecode works via VM
        let vm_results = executor.run(&chunk).expect("VM should succeed");
        assert_eq!(vm_results.len(), 1, "VM should return 1 result");
        assert_eq!(vm_results[0], MettaValue::Long(42), "VM should return Long(42)");

        // Now use run_with_backtracking (uses VM since not JIT-compiled yet)
        let results = executor.run_with_backtracking(&chunk).expect("should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    /// Test semantic equivalence: BytecodeVM and HybridExecutor must return identical results
    /// for Fork chunks with proper nondeterminism opcodes.
    ///
    /// This test verifies MeTTa HE semantics: ALL alternatives must be explored.
    #[test]
    fn test_semantic_equivalence_fork_basic() {
        // Create a fork chunk with 3 alternatives: Fork(1, 2, 3)
        // Expected output: [11, 12, 13] (each alternative + 10)
        let num_alternatives = 3;

        let mut builder = ChunkBuilder::new("fork_equivalence_test");

        // Add constants for alternatives 1, 2, 3
        let mut const_indices = Vec::with_capacity(num_alternatives);
        for i in 0..num_alternatives {
            let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
            const_indices.push(idx);
        }

        // Build chunk with proper nondeterminism opcodes:
        // BeginNondet -> Fork(3) [indices] -> +10 -> Yield -> Return
        builder.emit(Opcode::BeginNondet);
        builder.emit_u16(Opcode::Fork, num_alternatives as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Yield);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();

        // 1. Run with BytecodeVM
        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let vm_results = vm.run().expect("VM should succeed");

        // 2. Run with HybridExecutor (uses VM path for first few runs)
        let mut executor = HybridExecutor::new();
        let hybrid_results = executor.run_with_backtracking(&chunk).expect("Hybrid should succeed");

        // 3. Verify semantic equivalence
        assert_eq!(
            vm_results.len(), hybrid_results.len(),
            "VM and Hybrid must return same number of results"
        );

        // Expected: 3 results [11, 12, 13] in some order
        assert_eq!(vm_results.len(), num_alternatives, "Must return ALL {} alternatives", num_alternatives);

        // Verify all expected values are present
        let vm_longs: Vec<i64> = vm_results.iter().filter_map(|v| {
            if let MettaValue::Long(n) = v { Some(*n) } else { None }
        }).collect();

        let hybrid_longs: Vec<i64> = hybrid_results.iter().filter_map(|v| {
            if let MettaValue::Long(n) = v { Some(*n) } else { None }
        }).collect();

        // Both should contain {11, 12, 13}
        let mut vm_sorted = vm_longs.clone();
        let mut hybrid_sorted = hybrid_longs.clone();
        vm_sorted.sort();
        hybrid_sorted.sort();

        assert_eq!(vm_sorted, vec![11, 12, 13], "VM results should be 11, 12, 13");
        assert_eq!(hybrid_sorted, vec![11, 12, 13], "Hybrid results should be 11, 12, 13");
        assert_eq!(vm_sorted, hybrid_sorted, "Results must match exactly");
    }

    /// Test semantic equivalence with more alternatives (5)
    #[test]
    fn test_semantic_equivalence_fork_five_alternatives() {
        let num_alternatives = 5;

        let mut builder = ChunkBuilder::new("fork_5_equivalence");

        let mut const_indices = Vec::with_capacity(num_alternatives);
        for i in 0..num_alternatives {
            let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
            const_indices.push(idx);
        }

        builder.emit(Opcode::BeginNondet);
        builder.emit_u16(Opcode::Fork, num_alternatives as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        // Just yield the value directly, no arithmetic
        builder.emit(Opcode::Yield);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();

        // Run with both implementations
        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let vm_results = vm.run().expect("VM should succeed");

        let mut executor = HybridExecutor::new();
        let hybrid_results = executor.run_with_backtracking(&chunk).expect("Hybrid should succeed");

        // Verify equivalence
        assert_eq!(vm_results.len(), num_alternatives);
        assert_eq!(hybrid_results.len(), num_alternatives);

        let mut vm_longs: Vec<i64> = vm_results.iter().filter_map(|v| {
            if let MettaValue::Long(n) = v { Some(*n) } else { None }
        }).collect();

        let mut hybrid_longs: Vec<i64> = hybrid_results.iter().filter_map(|v| {
            if let MettaValue::Long(n) = v { Some(*n) } else { None }
        }).collect();

        vm_longs.sort();
        hybrid_longs.sort();

        assert_eq!(vm_longs, vec![1, 2, 3, 4, 5]);
        assert_eq!(hybrid_longs, vec![1, 2, 3, 4, 5]);
    }

    /// Test semantic equivalence for single alternative (edge case)
    #[test]
    fn test_semantic_equivalence_fork_single() {
        let mut builder = ChunkBuilder::new("fork_single");

        // Single alternative: Fork(42)
        let idx = builder.add_constant(MettaValue::Long(42));

        builder.emit(Opcode::BeginNondet);
        builder.emit_u16(Opcode::Fork, 1);
        builder.emit_raw(&idx.to_be_bytes());
        builder.emit(Opcode::Yield);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();

        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let vm_results = vm.run().expect("VM should succeed");

        let mut executor = HybridExecutor::new();
        let hybrid_results = executor.run_with_backtracking(&chunk).expect("Hybrid should succeed");

        assert_eq!(vm_results.len(), 1, "Single alternative = 1 result");
        assert_eq!(hybrid_results.len(), 1);
        assert_eq!(vm_results[0], MettaValue::Long(42));
        assert_eq!(hybrid_results[0], MettaValue::Long(42));
    }

    /// Test that without Yield, only first result is returned (documents behavior)
    #[test]
    fn test_fork_without_yield_returns_first_only() {
        let mut builder = ChunkBuilder::new("fork_no_yield");

        let mut const_indices = Vec::new();
        for i in 0..3 {
            let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
            const_indices.push(idx);
        }

        // BeginNondet + Fork but NO Yield - should return first result only
        builder.emit(Opcode::BeginNondet);
        builder.emit_u16(Opcode::Fork, 3);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        // No Yield, just return
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();

        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let vm_results = vm.run().expect("VM should succeed");

        // Without Yield, we only get the first alternative
        assert_eq!(vm_results.len(), 1, "Without Yield, only first result is returned");
        assert_eq!(vm_results[0], MettaValue::Long(1), "First alternative should be 1");
    }
}
