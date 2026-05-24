//! Arena-specific execution methods for HybridExecutor.
//!
//! This module provides arena-mode JIT execution support, enabling zero-conversion
//! evaluation where MettaValue expressions are executed via JIT-compiled native code
//! without converting to/from MettaValue.
//!
//! # Architecture
//!
//! The bytecode is identical for arena and heap modes - only the constants differ
//! in type. JIT-compiled code uses `JitContext.value_mode` to dispatch to the
//! appropriate runtime functions for value creation.
//!
//! ```text
//! MettaValue → GenericBytecodeChunk<MettaValue> → BytecodeChunk wrapper
//!     → JIT compile (same bytecode) → JitContext(Arena mode) → MettaValue results
//! ```
//!
//! # Environment Threading
//!
//! JIT execution can modify the environment through operations like rule definition
//! and state changes. Use `execute_jit_arena_with_env()` for expressions that need
//! environment access - it properly threads the environment through execution and
//! returns the updated environment.

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use std::sync::Arc;

use tracing::{debug, trace};

use crate::backend::bytecode::jit::runtime::arithmetic::check_and_clear_jit_type_error;
use crate::backend::bytecode::{GenericBytecodeChunk, MettaEnvironment, VmError, VmResult};
use crate::backend::models::{
    GcFactory, MettaValue, MettaValueFactory, MettaValueInner, SlabAllocator,
};

use super::super::{
    JitBindingFrame, JitChoicePoint, JitContext, JitValue, TypeSignatureRegistry,
    MAX_STACK_SAVE_VALUES, PAYLOAD_MASK, STACK_SAVE_POOL_SIZE, TAG_ATOM, TAG_BOOL, TAG_ERROR,
    TAG_LONG, TAG_MASK, TAG_PTR, TAG_UNIT, TAG_VAR,
};
use super::executor::HybridExecutor;

impl HybridExecutor {
    /// Execute JIT-compiled code directly in arena mode.
    ///
    /// This method is called from `eval()` when JIT code is already compiled
    /// and ready in the TieredCache. It sets up a JitContext in arena mode
    /// and executes the native code.
    ///
    /// # Arguments
    /// * `chunk` - The arena bytecode chunk (used for constants)
    /// * `native_ptr` - Pointer to JIT-compiled function
    /// * `arena` - The static arena allocator
    /// * `factory` - Factory for creating arena values
    ///
    /// # Returns
    /// Vector of MettaValue results
    pub fn execute_jit_arena_direct(
        &mut self,
        chunk: &Arc<GenericBytecodeChunk<MettaValue>>,
        native_ptr: *const (),
        allocator: &'static SlabAllocator,
        factory: &GcFactory,
    ) -> VmResult<Vec<MettaValue>> {
        self.stats.jit_runs += 1;
        self.stats.tiered_stats.jit_stage1_runs += 1;

        // Reset buffers
        for v in &mut self.jit_stack {
            *v = JitValue::unit();
        }
        self.jit_choice_points.clear();
        self.jit_results.clear();
        self.jit_binding_frames.clear();
        self.jit_cut_markers.clear();

        // Ensure capacity
        self.jit_choice_points.resize(
            self.config.jit_choice_point_capacity,
            JitChoicePoint::default(),
        );
        self.jit_results
            .resize(self.config.jit_results_capacity, JitValue::unit());
        self.jit_binding_frames.resize(
            self.config.jit_binding_frames_capacity,
            JitBindingFrame::default(),
        );
        self.jit_cut_markers
            .resize(self.config.jit_cut_markers_capacity, 0);

        let constants = chunk.constants();

        // Create JIT context in arena mode
        // SAFETY: All buffers are valid for the lifetime of this function call
        let mut ctx = unsafe {
            JitContext::for_arena_with_nondet(
                self.jit_stack.as_mut_ptr(),
                self.config.jit_stack_capacity,
                constants.as_ptr() as *const (),
                constants.len(),
                allocator as *const SlabAllocator as *const (),
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

        // Set up space registry if available
        if let Some(registry) = self.space_registry {
            ctx.space_registry = registry;
        }

        // Set up stack save pool (Optimization 5.2)
        let pool_cap = STACK_SAVE_POOL_SIZE * MAX_STACK_SAVE_VALUES;
        ctx.stack_save_pool = self.jit_stack_save_pool.as_mut_ptr();
        ctx.stack_save_pool_cap = pool_cap;
        ctx.stack_save_pool_next = 0;

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::arena", native_ptr = ?native_ptr, "Executing JIT code in arena mode");
        }

        // Cast and call native function
        let native_fn: extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(native_ptr) };

        let jit_result = native_fn(&mut ctx);

        // Check for type error from JIT runtime functions (thread-local flag)
        if check_and_clear_jit_type_error() {
            self.stats.jit_bailouts += 1;
            return Err(VmError::TypeError {
                expected: "number",
                got: "other",
            });
        }

        // Check for bailout
        if ctx.bailout {
            self.stats.jit_bailouts += 1;

            if self.config.trace {
                debug!(target: "mettatron::jit::hybrid::arena", bailout_ip = ctx.bailout_ip, reason = ?ctx.bailout_reason, "JIT bailout in arena mode");
            }

            // Trace: JitBailout event
            #[cfg(feature = "trace")]
            {
                use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
                with_thread_trace_collector(|tc| {
                    tc.emit_converted(
                        trace_format::TraceTier::JitStage1,
                        0,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::JitBailout {
                            bailout_ip: ctx.bailout_ip as u32,
                            reason: format!("{:?}", ctx.bailout_reason),
                            fallback_tier: "tree-walker".to_string(),
                        },
                    );
                });
            }

            return Err(VmError::Runtime(format!(
                "JIT bailout at ip {}: {:?}",
                ctx.bailout_ip, ctx.bailout_reason
            )));
        }

        // Collect results and convert to MettaValue
        let results = self.collect_jit_results_arena(&ctx, jit_result, factory);

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::arena", results_count = results.len(), "JIT arena execution complete");
        }

        Ok(results)
    }

    /// Execute JIT-compiled code in arena mode with environment threading.
    ///
    /// This method is similar to `execute_jit_arena_direct()` but properly threads
    /// the environment through JIT execution. The environment pointer is passed to
    /// the JIT context, allowing runtime functions to access and modify it.
    ///
    /// # Arguments
    /// * `chunk` - The arena bytecode chunk (used for constants)
    /// * `native_ptr` - Pointer to JIT-compiled function
    /// * `arena` - The static arena allocator
    /// * `factory` - Factory for creating arena values
    /// * `env` - The arena environment to thread through execution
    ///
    /// # Returns
    /// Tuple of (results, updated_environment)
    ///
    /// # Note
    /// This function modifies the environment in-place via the `env_ptr` in JitContext.
    /// The returned environment may have new rules, modified state, etc.
    pub fn execute_jit_arena_with_env(
        &mut self,
        chunk: &Arc<GenericBytecodeChunk<MettaValue>>,
        native_ptr: *const (),
        allocator: &'static SlabAllocator,
        factory: &GcFactory,
        mut env: MettaEnvironment,
    ) -> VmResult<(Vec<MettaValue>, MettaEnvironment)> {
        self.stats.jit_runs += 1;
        self.stats.tiered_stats.jit_stage1_runs += 1;

        // Reset buffers
        for v in &mut self.jit_stack {
            *v = JitValue::unit();
        }
        self.jit_choice_points.clear();
        self.jit_results.clear();
        self.jit_binding_frames.clear();
        self.jit_cut_markers.clear();

        // Ensure capacity
        self.jit_choice_points.resize(
            self.config.jit_choice_point_capacity,
            JitChoicePoint::default(),
        );
        self.jit_results
            .resize(self.config.jit_results_capacity, JitValue::unit());
        self.jit_binding_frames.resize(
            self.config.jit_binding_frames_capacity,
            JitBindingFrame::default(),
        );
        self.jit_cut_markers
            .resize(self.config.jit_cut_markers_capacity, 0);

        let constants = chunk.constants();

        // Create JIT context in arena mode
        // SAFETY: All buffers are valid for the lifetime of this function call
        let mut ctx = unsafe {
            JitContext::for_arena_with_nondet(
                self.jit_stack.as_mut_ptr(),
                self.config.jit_stack_capacity,
                constants.as_ptr() as *const (),
                constants.len(),
                allocator as *const SlabAllocator as *const (),
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

        // Set up space registry if available
        if let Some(registry) = self.space_registry {
            ctx.space_registry = registry;
        }

        // Set up stack save pool (Optimization 5.2)
        let pool_cap = STACK_SAVE_POOL_SIZE * MAX_STACK_SAVE_VALUES;
        ctx.stack_save_pool = self.jit_stack_save_pool.as_mut_ptr();
        ctx.stack_save_pool_cap = pool_cap;
        ctx.stack_save_pool_next = 0;

        // IMPORTANT: Set up environment pointer for environment threading
        // This allows JIT runtime functions to access and modify the environment.
        // SAFETY: The environment reference is valid for the duration of JIT execution.
        ctx.env_ptr = &mut env as *mut MettaEnvironment as *mut ();

        // Build type registry from environment's type assertions (MeTTa HE parity).
        // This pre-computes per-function type classifications (Evaluate vs PassThrough)
        // for O(1) lookup during jit_runtime_call_typed.
        // Stack-allocated; outlives JIT execution since it lives in this function's frame.
        let type_registry = TypeSignatureRegistry::from_env(&env);
        ctx.type_registry_ptr = &type_registry as *const TypeSignatureRegistry;

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::arena", native_ptr = ?native_ptr, "Executing JIT code in arena mode with environment");
        }

        // Cast and call native function
        let native_fn: extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(native_ptr) };

        let jit_result = native_fn(&mut ctx);

        // Check for type error from JIT runtime functions (thread-local flag)
        if check_and_clear_jit_type_error() {
            self.stats.jit_bailouts += 1;
            return Err(VmError::TypeError {
                expected: "number",
                got: "other",
            });
        }

        // Check for bailout
        if ctx.bailout {
            self.stats.jit_bailouts += 1;

            if self.config.trace {
                debug!(target: "mettatron::jit::hybrid::arena", bailout_ip = ctx.bailout_ip, reason = ?ctx.bailout_reason, "JIT bailout in arena mode with env");
            }

            // Trace: JitBailout event
            #[cfg(feature = "trace")]
            {
                use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
                with_thread_trace_collector(|tc| {
                    tc.emit_converted(
                        trace_format::TraceTier::JitStage1,
                        0,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::JitBailout {
                            bailout_ip: ctx.bailout_ip as u32,
                            reason: format!("{:?}", ctx.bailout_reason),
                            fallback_tier: "tree-walker".to_string(),
                        },
                    );
                });
            }

            return Err(VmError::Runtime(format!(
                "JIT bailout at ip {}: {:?}",
                ctx.bailout_ip, ctx.bailout_reason
            )));
        }

        // Collect results and convert to MettaValue
        let results = self.collect_jit_results_arena(&ctx, jit_result, factory);

        if self.config.trace {
            trace!(target: "mettatron::jit::hybrid::arena", results_count = results.len(), "JIT arena execution with env complete");
        }

        // Return results and the (potentially modified) environment
        Ok((results, env))
    }

    /// Collect results from JIT context and convert to MettaValue.
    ///
    /// This method handles the conversion from NaN-boxed JitValue to MettaValue.
    /// In arena mode, TAG_PTR pointers already point to MettaValueInner, so
    /// the conversion is mostly zero-copy.
    fn collect_jit_results_arena(
        &self,
        ctx: &JitContext,
        jit_result: i64,
        factory: &GcFactory,
    ) -> Vec<MettaValue> {
        // If there are collected results (from nondeterminism), use those
        if ctx.results_count > 0 {
            let mut results = Vec::with_capacity(ctx.results_count);
            for i in 0..ctx.results_count {
                let jit_val = unsafe { *ctx.results.add(i) };
                results.push(jit_to_value(jit_val.0, factory));
            }
            return results;
        }

        // Use the function return value if non-zero
        if jit_result != 0 {
            return vec![jit_to_value(jit_result as u64, factory)];
        }

        // Fallback to stack
        if ctx.sp > 0 {
            let mut results = Vec::with_capacity(ctx.sp);
            for i in 0..ctx.sp {
                let jit_val = unsafe { *ctx.value_stack.add(i) };
                results.push(jit_to_value(jit_val.0, factory));
            }
            results
        } else {
            vec![factory.unit()]
        }
    }
}

/// Convert a NaN-boxed JIT value to MettaValue.
///
/// In arena mode, TAG_PTR pointers point to MettaValueInner, so the conversion
/// is zero-copy for complex values. Primitives (Long, Bool, Nil, Unit) are
/// created fresh in the arena.
///
/// # Arguments
/// * `jit_val` - Raw NaN-boxed 64-bit value
/// * `factory` - Factory for creating arena values
fn jit_to_value(jit_val: u64, factory: &GcFactory) -> MettaValue {
    let tag = jit_val & TAG_MASK;

    match tag {
        TAG_LONG => {
            // Extract 48-bit signed integer
            let payload = jit_val & PAYLOAD_MASK;
            // Sign extend from 48 bits to 64 bits
            let value = if payload & (1 << 47) != 0 {
                // Negative number - sign extend
                (payload | !PAYLOAD_MASK) as i64
            } else {
                payload as i64
            };
            factory.long(value)
        }
        TAG_BOOL => factory.bool((jit_val & PAYLOAD_MASK) != 0),
        TAG_UNIT => factory.unit(),
        TAG_ATOM => {
            // In arena mode, atom pointer points to a thin pointer (*const String or similar)
            // We need to read the string and create a new atom
            let ptr = (jit_val & PAYLOAD_MASK) as *const String;
            if !ptr.is_null() {
                // SAFETY: The pointer was created during JIT execution and should be valid
                let s = unsafe { &*ptr };
                factory.atom(s.as_str())
            } else {
                factory.unit()
            }
        }
        TAG_VAR => {
            // In arena mode, variable pointer points to a thin pointer (*const String)
            // Variables are represented as atoms with $ prefix
            let ptr = (jit_val & PAYLOAD_MASK) as *const String;
            if !ptr.is_null() {
                let s = unsafe { &*ptr };
                // Create variable as an atom with $ prefix (same as MeTTa convention)
                factory.atom(s.as_str())
            } else {
                factory.unit()
            }
        }
        TAG_PTR => {
            // Pointer to slab-allocated MettaValueInner
            let ptr = (jit_val & PAYLOAD_MASK) as *const MettaValueInner;
            if !ptr.is_null() {
                unsafe { MettaValue::from_inner(&*ptr) }
            } else {
                factory.unit()
            }
        }
        TAG_ERROR => {
            // Error values — also point to slab-allocated MettaValueInner
            let ptr = (jit_val & PAYLOAD_MASK) as *const MettaValueInner;
            if !ptr.is_null() {
                unsafe { MettaValue::from_inner(&*ptr) }
            } else {
                factory.error(factory.string("unknown error"), factory.unit())
            }
        }
        _ => {
            // Unknown tag - return nil
            factory.unit()
        }
    }
}
