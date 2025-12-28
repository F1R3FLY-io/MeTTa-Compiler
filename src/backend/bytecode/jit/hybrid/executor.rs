//! HybridExecutor implementation.
//!
//! This module contains the core HybridExecutor struct and its methods for
//! seamless JIT/VM execution switching.

use std::sync::Arc;
use tracing::{debug, trace, warn};

use crate::backend::bytecode::{BytecodeChunk, BytecodeVM, MorkBridge, VmResult};
use crate::backend::models::MettaValue;

use super::super::{
    ChunkId, JitBindingFrame, JitCache, JitChoicePoint, JitCompiler, JitContext, JitValue, Tier,
    TieredCompiler, STAGE2_THRESHOLD,
};
use super::config::{HybridConfig, HybridStats};

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
    pub(super) jit_cache: Arc<JitCache>,
    /// Tiered compiler for tier management
    pub(super) tiered_compiler: Arc<TieredCompiler>,
    /// Configuration
    pub(super) config: HybridConfig,
    /// Execution statistics
    pub(super) stats: HybridStats,
    /// Reusable JIT value stack
    pub(super) jit_stack: Vec<JitValue>,
    /// Reusable JIT choice points buffer
    pub(super) jit_choice_points: Vec<JitChoicePoint>,
    /// Reusable JIT results buffer
    pub(super) jit_results: Vec<JitValue>,
    /// Reusable JIT binding frames buffer
    pub(super) jit_binding_frames: Vec<JitBindingFrame>,
    /// Reusable JIT cut markers buffer (for proper cut scope tracking)
    pub(super) jit_cut_markers: Vec<usize>,
    /// Heap allocation tracker for cleanup (prevents memory leaks)
    pub(super) heap_tracker: Vec<*mut MettaValue>,
    /// Optional MORK bridge for rule dispatch
    pub(super) bridge: Option<Arc<MorkBridge>>,
    /// Optional external function registry for CallExternal
    pub(super) external_registry: Option<*const ()>,
    /// Optional memo cache for CallCached
    pub(super) memo_cache: Option<*const ()>,
    /// Optional space registry for named spaces
    pub(super) space_registry: Option<*mut ()>,
    /// Optional environment pointer for state operations
    pub(super) env: Option<*mut ()>,
    /// Pre-resolved grounded space handles [&self, &kb, &stack]
    /// These are resolved once at executor setup and reused for all JIT calls.
    pub(super) grounded_spaces: [*const (); 3],
    /// Buffer for storing SpaceHandle instances (keeps them alive)
    pub(super) grounded_space_storage: Vec<crate::backend::models::SpaceHandle>,
    /// Template results buffer for space match instantiation
    pub(super) template_results: Vec<JitValue>,
    /// Stack save pool for Fork operations (Optimization 5.2)
    /// Pre-allocated ring buffer of stack snapshots, eliminating Box::leak() allocations
    pub(super) jit_stack_save_pool: Vec<JitValue>,
}

impl HybridExecutor {
    /// Create a new HybridExecutor with default configuration
    pub fn new() -> Self {
        Self::with_config(HybridConfig::default())
    }

    /// Create a new HybridExecutor with custom configuration
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn with_config(config: HybridConfig) -> Self {
        // Optimization 5.2: Pre-allocate stack save pool
        // Pool size = STACK_SAVE_POOL_SIZE slots × MAX_STACK_SAVE_VALUES values per slot
        let pool_capacity =
            super::super::STACK_SAVE_POOL_SIZE * super::super::MAX_STACK_SAVE_VALUES;

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
        let pool_capacity =
            super::super::STACK_SAVE_POOL_SIZE * super::super::MAX_STACK_SAVE_VALUES;

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
    pub fn run_with_backtracking(
        &mut self,
        chunk: &Arc<BytecodeChunk>,
    ) -> VmResult<Vec<MettaValue>> {
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
    pub(super) fn run_vm(&mut self, chunk: &Arc<BytecodeChunk>) -> VmResult<Vec<MettaValue>> {
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
    pub(super) fn try_compile(
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
                        let entry = super::super::tiered::CacheEntry {
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
    pub(super) fn execute_jit(
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
            JitChoicePoint::default(),
        );
        self.jit_results
            .resize(self.config.jit_results_capacity, JitValue::nil());
        self.jit_binding_frames.resize(
            self.config.jit_binding_frames_capacity,
            JitBindingFrame::default(),
        );
        self.jit_cut_markers
            .resize(self.config.jit_cut_markers_capacity, 0);

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
            ctx.set_template_results(
                self.template_results.as_mut_ptr(),
                self.template_results.capacity(),
            );
        }

        // Set up stack save pool (Optimization 5.2)
        let pool_cap = super::super::STACK_SAVE_POOL_SIZE * super::super::MAX_STACK_SAVE_VALUES;
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

    /// Collect results from JIT context
    #[allow(dead_code)]
    pub(super) fn collect_jit_results(&self, ctx: &JitContext) -> Vec<MettaValue> {
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
    pub(super) fn collect_jit_results_with_return(
        &self,
        ctx: &JitContext,
        jit_result: i64,
    ) -> Vec<MettaValue> {
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
