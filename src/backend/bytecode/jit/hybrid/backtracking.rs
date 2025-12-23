//! Backtracking support for hybrid execution.
//!
//! This module implements the dispatcher loop for native nondeterminism,
//! handling Fork/Yield/Fail operations without bailing to the bytecode VM.

use std::sync::Arc;
use tracing::{debug, trace};

use crate::backend::bytecode::{BytecodeChunk, BytecodeVM, VmError, VmResult};
use crate::backend::models::MettaValue;

use super::super::{
    JitContext, JitValue, JitChoicePoint, JitBindingFrame,
    STAGE2_THRESHOLD,
    JIT_SIGNAL_FAIL,
};
use super::executor::HybridExecutor;

impl HybridExecutor {
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
    pub(super) fn execute_jit_with_backtracking(
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
        self.jit_results.resize(
            self.config.jit_results_capacity,
            JitValue::nil(),
        );
        self.jit_binding_frames.resize(
            self.config.jit_binding_frames_capacity,
            JitBindingFrame::default(),
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
                    super::super::runtime::jit_runtime_fail_native(&mut ctx)
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
}
