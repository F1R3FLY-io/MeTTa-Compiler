//! JIT Runtime Tests
//!
//! This module contains all unit tests for the JIT runtime functions.

#[cfg(test)]
mod tests {
    use super::super::advanced_nondet::jit_runtime_cut;
    use super::super::arithmetic::{jit_runtime_abs, jit_runtime_pow, jit_runtime_signum};
    use super::super::helpers::{box_long, extract_long_signed};
    use super::super::nondeterminism::{
        collect_results, execute_once, jit_runtime_collect_native, jit_runtime_fail,
        jit_runtime_fail_native, jit_runtime_get_current_alternative, jit_runtime_has_alternatives,
        jit_runtime_push_choice_point, jit_runtime_restore_stack, jit_runtime_save_stack,
        jit_runtime_yield, jit_runtime_yield_native,
    };
    use super::super::type_predicates::jit_runtime_is_long;
    use crate::backend::bytecode::jit::types::{
        JitAlternative, JitAlternativeTag, JitBailoutReason, JitChoicePoint, JitContext, JitValue,
        JIT_SIGNAL_ERROR, JIT_SIGNAL_FAIL, JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, PAYLOAD_MASK, TAG_HEAP,
        TAG_MASK,
    };
    use crate::backend::models::MettaValue;

    #[test]
    fn test_pow_positive() {
        let base = box_long(2);
        let exp = box_long(10);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1024);
    }

    #[test]
    fn test_pow_zero_exp() {
        let base = box_long(5);
        let exp = box_long(0);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1);
    }

    #[test]
    fn test_pow_negative_exp() {
        let base = box_long(2);
        let exp = box_long(-1);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 0); // Integer division truncates
    }

    #[test]
    fn test_pow_one_negative_exp() {
        let base = box_long(1);
        let exp = box_long(-5);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1); // 1^anything = 1
    }

    #[test]
    fn test_abs() {
        let neg = box_long(-42);
        let result = unsafe { jit_runtime_abs(neg) };
        assert_eq!(extract_long_signed(result), 42);

        let pos = box_long(42);
        let result = unsafe { jit_runtime_abs(pos) };
        assert_eq!(extract_long_signed(result), 42);
    }

    #[test]
    fn test_signum() {
        assert_eq!(
            extract_long_signed(unsafe { jit_runtime_signum(box_long(-42)) }),
            -1
        );
        assert_eq!(
            extract_long_signed(unsafe { jit_runtime_signum(box_long(0)) }),
            0
        );
        assert_eq!(
            extract_long_signed(unsafe { jit_runtime_signum(box_long(42)) }),
            1
        );
    }

    #[test]
    fn test_is_long() {
        let long = box_long(42);
        let result = jit_runtime_is_long(long);
        assert_eq!(result & 1, 1); // true

        use crate::backend::bytecode::jit::types::TAG_BOOL;
        let bool_val = TAG_BOOL | 1;
        let result = jit_runtime_is_long(bool_val);
        assert_eq!(result & 1, 0); // false
    }

    #[test]
    fn test_extract_long_signed() {
        // Positive value
        let pos = box_long(12345);
        assert_eq!(extract_long_signed(pos), 12345);

        // Negative value
        let neg = box_long(-12345);
        assert_eq!(extract_long_signed(neg), -12345);

        // Zero
        let zero = box_long(0);
        assert_eq!(extract_long_signed(zero), 0);

        // Max 48-bit positive
        let max = box_long((1i64 << 47) - 1);
        assert_eq!(extract_long_signed(max), (1i64 << 47) - 1);

        // Min 48-bit negative
        let min = box_long(-(1i64 << 47));
        assert_eq!(extract_long_signed(min), -(1i64 << 47));
    }

    // =========================================================================
    // Choice Point Tests
    // =========================================================================

    #[test]
    fn test_push_choice_point_success() {
        // Create context with choice point support
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Create some alternatives
        let alts = [
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
            JitAlternative::value(JitValue::from_long(3)),
        ];

        // Push a choice point
        let result = unsafe {
            jit_runtime_push_choice_point(&mut ctx, 3, alts.as_ptr(), 100, std::ptr::null())
        };

        assert_eq!(result, 0); // Success
        assert_eq!(ctx.choice_point_count, 1);
    }

    #[test]
    fn test_push_choice_point_overflow() {
        // Create context with only 1 choice point slot
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 1];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                1, // Only 1 slot
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let alts = [JitAlternative::value(JitValue::from_long(1))];

        // First push succeeds
        let result = unsafe {
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(result, 0);

        // Second push should fail (overflow)
        let result = unsafe {
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(result, -1); // Overflow
        assert!(ctx.bailout);
    }

    #[test]
    fn test_fail_with_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let alts = [
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
        ];

        // Push choice point
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 2, alts.as_ptr(), 0, std::ptr::null());
        }

        // First fail should return first alternative
        let tag = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(tag, JitAlternativeTag::Value as i64);

        // Get the alternative
        let alt = unsafe { jit_runtime_get_current_alternative(&ctx) };
        assert_eq!(alt.tag, JitAlternativeTag::Value);
        let val = JitValue::from_raw(alt.payload);
        assert_eq!(val.as_long(), 1);

        // Second fail should return second alternative
        let tag = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(tag, JitAlternativeTag::Value as i64);

        let alt = unsafe { jit_runtime_get_current_alternative(&ctx) };
        let val = JitValue::from_raw(alt.payload);
        assert_eq!(val.as_long(), 2);

        // Third fail should return -1 (no more alternatives)
        let tag = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(tag, -1);
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_yield_stores_result_and_signals_bailout() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Create the value to yield (as NaN-boxed u64)
        let yield_value = JitValue::from_long(42);

        // Yield the value (Phase 4: value is passed as argument, not popped from stack)
        let _result = unsafe { jit_runtime_yield(&mut ctx, yield_value.to_bits(), 0) };

        // Should have stored the result
        assert_eq!(ctx.results_count, 1);
        let stored = unsafe { *ctx.results.add(0) };
        assert_eq!(stored.as_long(), 42);

        // Should have signaled bailout with Yield reason
        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::Yield);
    }

    #[test]
    fn test_cut_clears_choice_points() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let alts = [JitAlternative::value(JitValue::from_long(1))];

        // Push multiple choice points
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null());
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null());
            jit_runtime_push_choice_point(&mut ctx, 1, alts.as_ptr(), 0, std::ptr::null());
        }
        assert_eq!(ctx.choice_point_count, 3);

        // Cut should clear all
        unsafe { jit_runtime_cut(&mut ctx, 0) };
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_context_has_nondet_support() {
        // Context without nondet support
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let ctx = unsafe { JitContext::new(stack.as_mut_ptr(), stack.len(), std::ptr::null(), 0) };
        assert!(!ctx.has_nondet_support());

        // Context with nondet support
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];
        let ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        assert!(ctx.has_nondet_support());
    }

    // =========================================================================
    // Stage 2: Native Nondeterminism Tests
    // =========================================================================

    #[test]
    fn test_yield_native_stores_result() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Yield a value using native function
        let value = JitValue::from_long(42);
        let signal = unsafe { jit_runtime_yield_native(&mut ctx, value.to_bits(), 10) };

        // Should return YIELD signal
        assert_eq!(signal, JIT_SIGNAL_YIELD);

        // Should have stored the result
        assert_eq!(ctx.results_count, 1);
        let stored = unsafe { *ctx.results.add(0) };
        assert_eq!(stored.as_long(), 42);

        // Should have set resume_ip
        assert_eq!(ctx.resume_ip, 10);
    }

    #[test]
    fn test_collect_native_gathers_results() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Store some results manually
        unsafe {
            *ctx.results.add(0) = JitValue::from_long(1);
            *ctx.results.add(1) = JitValue::from_long(2);
            *ctx.results.add(2) = JitValue::from_long(3);
        }
        ctx.results_count = 3;

        // Collect results
        let result = unsafe { jit_runtime_collect_native(&mut ctx) };

        // Should return a heap pointer
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_HEAP);

        // Results should be cleared
        assert_eq!(ctx.results_count, 0);

        // Verify the SExpr contents
        let ptr = (result & PAYLOAD_MASK) as *const MettaValue;
        let metta_val = unsafe { &*ptr };
        if let MettaValue::SExpr(items) = metta_val {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Long(1));
            assert_eq!(items[1], MettaValue::Long(2));
            assert_eq!(items[2], MettaValue::Long(3));
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_has_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // No choice points = no alternatives
        let has_alts = unsafe { jit_runtime_has_alternatives(&ctx) };
        assert_eq!(has_alts, 0);

        // Add a choice point with alternatives
        let alts = [
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
        ];
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 2, alts.as_ptr(), 0, std::ptr::null());
        }

        // Should now have alternatives
        let has_alts = unsafe { jit_runtime_has_alternatives(&ctx) };
        assert_eq!(has_alts, 1);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn test_fail_native_exhausts_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.sp = 5; // Set some stack pointer

        // Add a choice point with 2 alternatives (using inline alternatives)
        let mut cp = JitChoicePoint::default();
        cp.saved_sp = 2; // Save sp at 2
        cp.alt_count = 2;
        cp.current_index = 0;
        cp.alternatives_inline[0] = JitAlternative::value(JitValue::from_long(10));
        cp.alternatives_inline[1] = JitAlternative::value(JitValue::from_long(20));
        cp.saved_ip = 100;
        cp.saved_chunk = std::ptr::null();
        cp.saved_stack_pool_idx = -1; // No saved stack
        cp.saved_stack_count = 0;
        cp.fork_depth = 0;
        cp.saved_binding_frames_count = 0;
        cp.is_collect_boundary = false;
        unsafe {
            *ctx.choice_points.add(0) = cp;
        }
        ctx.choice_point_count = 1;

        // First fail should return first alternative and restore sp
        let result1 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let jv1 = JitValue::from_raw(result1);
        assert_eq!(jv1.as_long(), 10);
        assert_eq!(ctx.sp, 2); // sp restored

        // Second fail should return second alternative
        let result2 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let jv2 = JitValue::from_raw(result2);
        assert_eq!(jv2.as_long(), 20);

        // Third fail should exhaust and return FAIL signal
        let result3 = unsafe { jit_runtime_fail_native(&mut ctx) };
        assert_eq!(result3, JIT_SIGNAL_FAIL as u64);
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_save_restore_stack() {
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.saved_stack = saved_stack.as_mut_ptr();
        ctx.saved_stack_cap = saved_stack.len();

        // Set up some stack values
        unsafe {
            *ctx.value_stack.add(0) = JitValue::from_long(100);
            *ctx.value_stack.add(1) = JitValue::from_long(200);
            *ctx.value_stack.add(2) = JitValue::from_long(300);
        }
        ctx.sp = 3;

        // Save stack
        let signal = unsafe { jit_runtime_save_stack(&mut ctx) };
        assert_eq!(signal, JIT_SIGNAL_OK);
        assert_eq!(ctx.saved_stack_count, 3);

        // Modify stack
        unsafe {
            *ctx.value_stack.add(0) = JitValue::from_long(999);
            *ctx.value_stack.add(1) = JitValue::from_long(888);
        }
        ctx.sp = 2;

        // Restore stack
        let signal = unsafe { jit_runtime_restore_stack(&mut ctx) };
        assert_eq!(signal, JIT_SIGNAL_OK);
        assert_eq!(ctx.sp, 3);

        // Verify restored values
        let v0 = unsafe { *ctx.value_stack.add(0) };
        let v1 = unsafe { *ctx.value_stack.add(1) };
        let v2 = unsafe { *ctx.value_stack.add(2) };
        assert_eq!(v0.as_long(), 100);
        assert_eq!(v1.as_long(), 200);
        assert_eq!(v2.as_long(), 300);
    }

    #[test]
    fn test_signal_constants() {
        // Verify signal constants are distinct and sensible
        assert_eq!(JIT_SIGNAL_OK, 0);
        assert_eq!(JIT_SIGNAL_YIELD, 2);
        assert_eq!(JIT_SIGNAL_FAIL, 3);
        assert_eq!(JIT_SIGNAL_ERROR, -1);

        // Verify they're all different
        assert_ne!(JIT_SIGNAL_OK, JIT_SIGNAL_YIELD);
        assert_ne!(JIT_SIGNAL_OK, JIT_SIGNAL_FAIL);
        assert_ne!(JIT_SIGNAL_OK, JIT_SIGNAL_ERROR);
        assert_ne!(JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL);
        assert_ne!(JIT_SIGNAL_YIELD, JIT_SIGNAL_ERROR);
        assert_ne!(JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR);
    }

    #[test]
    fn test_collect_results() {
        // Test the collect_results helper function
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Store some results
        unsafe {
            *ctx.results.add(0) = JitValue::from_long(10);
            *ctx.results.add(1) = JitValue::from_long(20);
            *ctx.results.add(2) = JitValue::from_long(30);
        }
        ctx.results_count = 3;

        // Collect results
        let collected = unsafe { collect_results(&mut ctx) };

        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0], MettaValue::Long(10));
        assert_eq!(collected[1], MettaValue::Long(20));
        assert_eq!(collected[2], MettaValue::Long(30));
    }

    #[test]
    fn test_execute_once() {
        // Test the execute_once helper function with a simple JIT function
        // that just returns a constant
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Simulate a JIT function that pushes 42 and returns OK
        unsafe extern "C" fn mock_jit_fn(ctx: *mut JitContext) -> i64 {
            let ctx_ref = ctx.as_mut().unwrap();
            *ctx_ref.value_stack.add(ctx_ref.sp) = JitValue::from_long(42);
            ctx_ref.sp += 1;
            JIT_SIGNAL_OK
        }

        let result = unsafe { execute_once(&mut ctx, mock_jit_fn) };

        assert!(result.is_some());
        assert_eq!(result.unwrap(), MettaValue::Long(42));
    }

    // =========================================================================
    // Phase 2.2: Fork/Yield/Collect Full Cycle Integration Test
    // =========================================================================
    // Tests the complete nondeterminism workflow:
    // 1. Fork creates choice points with multiple alternatives
    // 2. Yield stores results for each alternative
    // 3. Collect gathers all results into an S-expression
    // =========================================================================

    #[test]
    fn test_fork_yield_collect_full_cycle() {
        // Create context with nondeterminism support
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 32];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 32];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::nil(); 32];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.saved_stack = saved_stack.as_mut_ptr();
        ctx.saved_stack_cap = saved_stack.len();

        // =====================================================================
        // Phase 1: Fork - Create choice point with 3 alternatives (1, 2, 3)
        // =====================================================================
        let alternatives = vec![
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
            JitAlternative::value(JitValue::from_long(3)),
        ];
        let alts_ptr = Box::leak(alternatives.into_boxed_slice()).as_ptr();

        // Push the fork choice point
        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 3, alts_ptr, 0, std::ptr::null());
        }

        // Verify choice point was created
        assert_eq!(ctx.choice_point_count, 1);
        let has_alts = unsafe { jit_runtime_has_alternatives(&ctx) };
        assert_eq!(has_alts, 1);

        // =====================================================================
        // Phase 2: Process each alternative and Yield results
        // =====================================================================
        // Simulate the evaluation loop:
        // - Get next alternative via fail_native
        // - Yield the result
        // - Repeat until no more alternatives

        // Process alternative 1
        let alt1 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let val1 = JitValue::from_raw(alt1);
        assert_eq!(val1.as_long(), 1);

        // Yield alternative 1
        let signal1 = unsafe { jit_runtime_yield_native(&mut ctx, val1.to_bits(), 0) };
        assert_eq!(signal1, JIT_SIGNAL_YIELD);
        assert_eq!(ctx.results_count, 1);

        // Process alternative 2
        let alt2 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let val2 = JitValue::from_raw(alt2);
        assert_eq!(val2.as_long(), 2);

        // Yield alternative 2
        let signal2 = unsafe { jit_runtime_yield_native(&mut ctx, val2.to_bits(), 0) };
        assert_eq!(signal2, JIT_SIGNAL_YIELD);
        assert_eq!(ctx.results_count, 2);

        // Process alternative 3
        let alt3 = unsafe { jit_runtime_fail_native(&mut ctx) };
        let val3 = JitValue::from_raw(alt3);
        assert_eq!(val3.as_long(), 3);

        // Yield alternative 3
        let signal3 = unsafe { jit_runtime_yield_native(&mut ctx, val3.to_bits(), 0) };
        assert_eq!(signal3, JIT_SIGNAL_YIELD);
        assert_eq!(ctx.results_count, 3);

        // No more alternatives - fail_native returns FAIL signal
        let alt4 = unsafe { jit_runtime_fail_native(&mut ctx) };
        assert_eq!(alt4, JIT_SIGNAL_FAIL as u64);
        assert_eq!(ctx.choice_point_count, 0);

        // =====================================================================
        // Phase 3: Collect all yielded results
        // =====================================================================
        let collected_raw = unsafe { jit_runtime_collect_native(&mut ctx) };

        // Verify it's a heap pointer (TAG_HEAP)
        let tag = collected_raw & TAG_MASK;
        assert_eq!(tag, TAG_HEAP);

        // Results should be cleared after collection
        assert_eq!(ctx.results_count, 0);

        // =====================================================================
        // Phase 4: Verify the collected S-expression
        // =====================================================================
        let ptr = (collected_raw & PAYLOAD_MASK) as *const MettaValue;
        let metta_val = unsafe { &*ptr };

        if let MettaValue::SExpr(items) = metta_val {
            assert_eq!(items.len(), 3, "Expected 3 collected results");
            assert_eq!(items[0], MettaValue::Long(1), "First result should be 1");
            assert_eq!(items[1], MettaValue::Long(2), "Second result should be 2");
            assert_eq!(items[2], MettaValue::Long(3), "Third result should be 3");
        } else {
            panic!("Expected SExpr, got {:?}", metta_val);
        }
    }

    #[test]
    fn test_nested_fork_yield_collect() {
        // Test nested Fork/Yield/Collect with two levels of nondeterminism
        // Outer fork: alternatives A, B
        // For each outer, inner fork: alternatives 1, 2
        // Expected results: (A 1), (A 2), (B 1), (B 2)

        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.saved_stack = saved_stack.as_mut_ptr();
        ctx.saved_stack_cap = saved_stack.len();

        // Create heap-allocated MettaValues for atoms
        let atom_a = Box::leak(Box::new(MettaValue::Atom("A".to_string())));
        let atom_b = Box::leak(Box::new(MettaValue::Atom("B".to_string())));

        // Outer fork: A, B
        let outer_alts = vec![
            JitAlternative::value(JitValue::from_heap_ptr(atom_a)),
            JitAlternative::value(JitValue::from_heap_ptr(atom_b)),
        ];
        let outer_ptr = Box::leak(outer_alts.into_boxed_slice()).as_ptr();

        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 2, outer_ptr, 0, std::ptr::null());
        }
        assert_eq!(ctx.choice_point_count, 1);

        let mut collected_pairs: Vec<(String, i64)> = Vec::new();

        // Process outer alternatives
        for outer_idx in 0..2 {
            // Get outer alternative
            let outer_val_raw = unsafe { jit_runtime_fail_native(&mut ctx) };
            if outer_val_raw == JIT_SIGNAL_FAIL as u64 {
                break;
            }
            let outer_val = JitValue::from_raw(outer_val_raw);

            // Extract atom name (to_metta() returns MettaValue directly)
            let metta = unsafe { outer_val.to_metta() };
            let outer_name = if let MettaValue::Atom(name) = metta {
                name
            } else {
                panic!("Expected Atom for outer, got {:?}", metta);
            };

            // Inner fork: 1, 2
            let inner_alts = vec![
                JitAlternative::value(JitValue::from_long(1)),
                JitAlternative::value(JitValue::from_long(2)),
            ];
            let inner_ptr = Box::leak(inner_alts.into_boxed_slice()).as_ptr();

            unsafe {
                jit_runtime_push_choice_point(&mut ctx, 2, inner_ptr, 0, std::ptr::null());
            }

            // Process inner alternatives
            for _inner_idx in 0..2 {
                let inner_val_raw = unsafe { jit_runtime_fail_native(&mut ctx) };
                if inner_val_raw == JIT_SIGNAL_FAIL as u64 {
                    break;
                }
                let inner_val = JitValue::from_raw(inner_val_raw);
                let inner_num = inner_val.as_long();

                // Record the pair
                collected_pairs.push((outer_name.clone(), inner_num));

                // Yield combined result (as a simple encoding: outer_idx * 10 + inner_num)
                let combined = JitValue::from_long(outer_idx as i64 * 10 + inner_num);
                unsafe {
                    jit_runtime_yield_native(&mut ctx, combined.to_bits(), 0);
                }
            }
        }

        // Verify we collected all 4 combinations
        assert_eq!(collected_pairs.len(), 4);
        assert!(collected_pairs.contains(&("A".to_string(), 1)));
        assert!(collected_pairs.contains(&("A".to_string(), 2)));
        assert!(collected_pairs.contains(&("B".to_string(), 1)));
        assert!(collected_pairs.contains(&("B".to_string(), 2)));

        // Verify results were yielded
        assert_eq!(ctx.results_count, 4);

        // Collect all results
        let collected_raw = unsafe { jit_runtime_collect_native(&mut ctx) };
        let tag = collected_raw & TAG_MASK;
        assert_eq!(tag, TAG_HEAP);

        let ptr = (collected_raw & PAYLOAD_MASK) as *const MettaValue;
        let metta_val = unsafe { &*ptr };

        if let MettaValue::SExpr(items) = metta_val {
            assert_eq!(items.len(), 4, "Expected 4 collected results");
            // Results should be: 1 (A,1), 2 (A,2), 11 (B,1), 12 (B,2)
            assert_eq!(items[0], MettaValue::Long(1)); // A*10 + 1 = 0*10 + 1 = 1
            assert_eq!(items[1], MettaValue::Long(2)); // A*10 + 2 = 0*10 + 2 = 2
            assert_eq!(items[2], MettaValue::Long(11)); // B*10 + 1 = 1*10 + 1 = 11
            assert_eq!(items[3], MettaValue::Long(12)); // B*10 + 2 = 1*10 + 2 = 12
        } else {
            panic!("Expected SExpr, got {:?}", metta_val);
        }
    }

    #[test]
    fn test_fork_with_early_cut() {
        // Test that cut properly terminates nondeterministic search
        // Fork with 5 alternatives, but cut after finding the first even number

        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 32];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 32];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Fork with 5 alternatives: 1, 2, 3, 4, 5
        let alternatives = vec![
            JitAlternative::value(JitValue::from_long(1)),
            JitAlternative::value(JitValue::from_long(2)),
            JitAlternative::value(JitValue::from_long(3)),
            JitAlternative::value(JitValue::from_long(4)),
            JitAlternative::value(JitValue::from_long(5)),
        ];
        let alts_ptr = Box::leak(alternatives.into_boxed_slice()).as_ptr();

        unsafe {
            jit_runtime_push_choice_point(&mut ctx, 5, alts_ptr, 0, std::ptr::null());
        }
        assert_eq!(ctx.choice_point_count, 1);

        let mut found_even = false;
        let mut iterations = 0;

        while !found_even {
            iterations += 1;
            let val_raw = unsafe { jit_runtime_fail_native(&mut ctx) };
            if val_raw == JIT_SIGNAL_FAIL as u64 {
                break;
            }

            let val = JitValue::from_raw(val_raw);
            let num = val.as_long();

            if num % 2 == 0 {
                // Found even number, yield it and cut
                unsafe {
                    jit_runtime_yield_native(&mut ctx, val.to_bits(), 0);
                    jit_runtime_cut(&mut ctx, 0);
                }
                found_even = true;
            }
        }

        // Should have found even number (2) after 2 iterations (1, 2)
        assert!(found_even);
        assert_eq!(iterations, 2);

        // Cut should have cleared all choice points
        assert_eq!(ctx.choice_point_count, 0);

        // Should have only one result (the first even number found: 2)
        assert_eq!(ctx.results_count, 1);
        let result = unsafe { *ctx.results.add(0) };
        assert_eq!(result.as_long(), 2);
    }
}
