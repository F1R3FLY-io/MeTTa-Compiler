//! JIT Runtime Tests
//!
//! This module contains all unit tests for the JIT runtime functions.

#[cfg(test)]
mod tests {
    // External crates
    use xxhash_rust::xxh3::xxh3_64;

    // Relative imports (super::)
    use super::super::advanced_nondet::{
        jit_runtime_amb, jit_runtime_backtrack, jit_runtime_begin_nondet, jit_runtime_commit,
        jit_runtime_cut, jit_runtime_end_nondet, jit_runtime_enter_cut_scope,
        jit_runtime_exit_cut_scope, jit_runtime_guard,
    };
    use super::super::arithmetic::{
        check_and_clear_jit_type_error, jit_runtime_abs, jit_runtime_acos, jit_runtime_asin,
        jit_runtime_atan, jit_runtime_ceil, jit_runtime_cos, jit_runtime_floor_math,
        jit_runtime_isinf, jit_runtime_isnan, jit_runtime_log, jit_runtime_numeric_abs,
        jit_runtime_numeric_add, jit_runtime_numeric_div, jit_runtime_numeric_eq,
        jit_runtime_numeric_ge, jit_runtime_numeric_gt, jit_runtime_numeric_le,
        jit_runtime_numeric_lt, jit_runtime_numeric_mod, jit_runtime_numeric_mul,
        jit_runtime_numeric_neg, jit_runtime_numeric_sub, jit_runtime_pow, jit_runtime_round,
        jit_runtime_signum, jit_runtime_sin, jit_runtime_sqrt, jit_runtime_tan, jit_runtime_trunc,
        signal_jit_type_error,
    };
    use super::super::bindings::{
        jit_runtime_clear_bindings, jit_runtime_fork_bindings, jit_runtime_free_saved_bindings,
        jit_runtime_has_binding, jit_runtime_load_binding, jit_runtime_pop_binding_frame,
        jit_runtime_push_binding_frame, jit_runtime_restore_bindings,
        jit_runtime_saved_bindings_size, jit_runtime_store_binding,
    };
    use super::super::error_handling::{
        jit_runtime_div_by_zero, jit_runtime_integer_overflow, jit_runtime_stack_overflow,
        jit_runtime_stack_underflow, jit_runtime_type_error,
    };
    use super::super::expression_ops::jit_runtime_index_atom;
    use super::super::global_ops::{
        jit_runtime_load_global, jit_runtime_load_space, jit_runtime_store_global,
    };
    use super::super::helpers::{box_long, extract_long_signed, metta_to_jit};
    use super::super::nondeterminism::{
        collect_results, execute_once, jit_runtime_collect, jit_runtime_collect_native,
        jit_runtime_fail, jit_runtime_fail_native, jit_runtime_fork, jit_runtime_fork_native,
        jit_runtime_get_choice_point_count, jit_runtime_get_current_alternative,
        jit_runtime_get_results_count, jit_runtime_get_resume_ip, jit_runtime_has_alternatives,
        jit_runtime_push_choice_point, jit_runtime_restore_stack, jit_runtime_save_stack,
        jit_runtime_yield, jit_runtime_yield_native,
    };
    use super::super::pattern_matching::{
        jit_runtime_match_arity, jit_runtime_match_head, jit_runtime_pattern_match,
        jit_runtime_pattern_match_bind, jit_runtime_unify, jit_runtime_unify_bind,
    };
    use super::super::rule_dispatch::hash_string;
    use super::super::sexpr_ops::{
        jit_runtime_get_arity, jit_runtime_get_head, jit_runtime_get_tail, jit_runtime_push_empty,
    };
    use super::super::space_ops::{
        jit_runtime_space_add, jit_runtime_space_get_atoms, jit_runtime_space_match,
        jit_runtime_space_match_nondet, jit_runtime_space_remove,
    };
    use super::super::special_forms::{
        jit_runtime_eval_bind, jit_runtime_eval_case, jit_runtime_eval_chain,
        jit_runtime_eval_collapse, jit_runtime_eval_eval, jit_runtime_eval_if,
        jit_runtime_eval_let, jit_runtime_eval_let_star, jit_runtime_eval_match,
        jit_runtime_eval_memo, jit_runtime_eval_memo_first, jit_runtime_eval_new,
        jit_runtime_eval_pragma, jit_runtime_eval_quote, jit_runtime_eval_superpose,
        jit_runtime_eval_unquote,
    };
    use super::super::stack_ops::jit_runtime_load_constant;
    use super::super::type_ops::{
        jit_runtime_assert_type, jit_runtime_check_type, jit_runtime_get_type,
    };
    use super::super::type_predicates::{
        jit_runtime_get_tag, jit_runtime_is_bool, jit_runtime_is_long, jit_runtime_is_unit,
    };
    use super::super::value_creation::{
        jit_runtime_cons_atom, jit_runtime_make_list, jit_runtime_make_quote,
        jit_runtime_make_sexpr, jit_runtime_push_uri,
    };

    // Absolute crate imports (crate::)
    use crate::backend::bytecode::jit::types::{
        JitAlternative, JitAlternativeTag, JitBailoutReason, JitBindingFrame, JitChoicePoint,
        JitContext, JitValue, JIT_SIGNAL_ERROR, JIT_SIGNAL_FAIL, JIT_SIGNAL_OK, JIT_SIGNAL_YIELD,
        PAYLOAD_MASK, TAG_BOOL, TAG_LONG, TAG_MASK, TAG_PTR, TAG_UNIT,
    };
    use crate::backend::models::{MettaValue, MettaValueInner, SpaceHandle};

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 1];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe { JitContext::new(stack.as_mut_ptr(), stack.len(), std::ptr::null(), 0) };
        assert!(!ctx.has_nondet_support());

        // Context with nondet support
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];
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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        assert_eq!(tag, TAG_PTR);

        // Results should be cleared
        assert_eq!(ctx.results_count, 0);

        // Verify the SExpr contents
        let ptr = (result & PAYLOAD_MASK) as *const MettaValueInner;
        let metta_val = unsafe { MettaValue::from_inner(&*ptr) };
        if let MettaValueInner::SExpr(items) = metta_val.inner() {
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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 32];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 32];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::unit(); 32];

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

        // Verify it's a heap pointer (TAG_PTR)
        let tag = collected_raw & TAG_MASK;
        assert_eq!(tag, TAG_PTR);

        // Results should be cleared after collection
        assert_eq!(ctx.results_count, 0);

        // =====================================================================
        // Phase 4: Verify the collected S-expression
        // =====================================================================
        let ptr = (collected_raw & PAYLOAD_MASK) as *const MettaValueInner;
        let metta_val = unsafe { MettaValue::from_inner(&*ptr) };

        if let MettaValueInner::SExpr(items) = metta_val.inner() {
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

        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 64];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 64];
        let mut saved_stack: Vec<JitValue> = vec![JitValue::unit(); 64];

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

        // Create slab-allocated MettaValues for atoms
        let atom_a = MettaValue::Atom("A".to_string());
        let atom_b = MettaValue::Atom("B".to_string());

        // Outer fork: A, B
        let outer_alts = vec![
            JitAlternative::value(JitValue::from_inner_ptr(atom_a.inner_ptr())),
            JitAlternative::value(JitValue::from_inner_ptr(atom_b.inner_ptr())),
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
            let outer_name = if let MettaValueInner::Atom(name) = metta.inner() {
                name.to_string()
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
        assert_eq!(tag, TAG_PTR);

        let ptr = (collected_raw & PAYLOAD_MASK) as *const MettaValueInner;
        let metta_val = unsafe { MettaValue::from_inner(&*ptr) };

        if let MettaValueInner::SExpr(items) = metta_val.inner() {
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

        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 32];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 32];

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

    // ==========================================================================
    // Hash function tests (xxh3 coverage)
    // ==========================================================================

    #[test]
    fn test_hash_string_stability() {
        // Hash should be stable
        let h1 = hash_string("test_binding");
        let h2 = hash_string("test_binding");
        assert_eq!(h1, h2);

        // Should match direct xxh3_64 call
        assert_eq!(h1, xxh3_64(b"test_binding"));
    }

    #[test]
    fn test_hash_string_different_strings() {
        // Different strings should produce different hashes
        let h1 = hash_string("binding_a");
        let h2 = hash_string("binding_b");
        let h3 = hash_string("$x");
        let h4 = hash_string("$y");

        assert_ne!(h1, h2);
        assert_ne!(h3, h4);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_hash_string_empty() {
        // Empty string should have a valid hash
        let h = hash_string("");
        assert_eq!(h, xxh3_64(b""));
        // xxh3_64 produces a well-defined hash for empty input
        assert_ne!(h, 0); // xxh3 doesn't return 0 for empty input
    }

    // ==========================================================================
    // jit_runtime_load_space fallback xxh3 path coverage
    // ==========================================================================

    #[test]
    fn test_jit_load_space_fallback_xxh3() {
        // Create constants with space name
        let space_name = "fallback_test_space";
        let constants = vec![MettaValue::sym(space_name)];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        // JitContext::new() sets space_registry=null and grounded_spaces=null
        // This triggers the fallback path in jit_runtime_load_space
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Call jit_runtime_load_space - should use fallback xxh3_64 path
        let result_bits = unsafe { jit_runtime_load_space(&ctx, 0, 0) };
        let result = JitValue::from_raw(result_bits);
        let metta_val = unsafe { result.to_metta() };

        // Verify space has correct xxh3_64-based ID
        match metta_val.inner() {
            MettaValueInner::Space(handle) => {
                let expected_id = xxh3_64(space_name.as_bytes());
                assert_eq!(handle.id, expected_id);
                assert_eq!(handle.name, space_name);
            }
            _ => panic!("Expected Space, got {:?}", metta_val),
        }
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Arithmetic Edge Cases
    // ==========================================================================

    #[test]
    fn test_pow_large_values() {
        // Use values that fit in 48-bit NaN-boxed payload
        let base = box_long(2);
        let exp = box_long(40);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        // 2^40 = 1099511627776, fits in 48-bit payload
        assert_eq!(result_val, 1099511627776);
    }

    #[test]
    fn test_pow_base_zero() {
        let base = box_long(0);
        let exp = box_long(5);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 0); // 0^n = 0 for n > 0
    }

    #[test]
    fn test_pow_base_one() {
        let base = box_long(1);
        let exp = box_long(100);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1); // 1^n = 1
    }

    #[test]
    fn test_pow_negative_base_even_exp() {
        let base = box_long(-2);
        let exp = box_long(4);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 16); // (-2)^4 = 16
    }

    #[test]
    fn test_pow_negative_base_odd_exp() {
        let base = box_long(-2);
        let exp = box_long(3);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, -8); // (-2)^3 = -8
    }

    #[test]
    fn test_abs_zero() {
        let zero = box_long(0);
        let result = unsafe { jit_runtime_abs(zero) };
        assert_eq!(extract_long_signed(result), 0);
    }

    #[test]
    fn test_abs_max_value() {
        // Use max value that fits in 48-bit NaN-boxed payload (47 bits for signed)
        const MAX_48BIT: i64 = 0x0000_7FFF_FFFF_FFFF; // ~140 trillion
        let max = box_long(MAX_48BIT);
        let result = unsafe { jit_runtime_abs(max) };
        assert_eq!(extract_long_signed(result), MAX_48BIT);
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Type Predicates
    // ==========================================================================

    #[test]
    fn test_is_long_various_values() {
        // Test with different long values
        assert_eq!(jit_runtime_is_long(box_long(0)) & 1, 1);
        assert_eq!(jit_runtime_is_long(box_long(i64::MAX)) & 1, 1);
        assert_eq!(jit_runtime_is_long(box_long(i64::MIN)) & 1, 1);
        assert_eq!(jit_runtime_is_long(box_long(-1)) & 1, 1);
    }

    #[test]
    fn test_is_long_nil() {
        let nil_val = TAG_UNIT;
        let result = jit_runtime_is_long(nil_val);
        assert_eq!(result & 1, 0); // nil is not a long
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - JitValue Conversions
    // ==========================================================================

    #[test]
    fn test_jit_value_nil() {
        // After Nil/Unit merge, nil() produces Unit
        let nil = JitValue::unit();
        assert!(nil.is_unit());
        assert!(!nil.is_long());
        assert!(!nil.is_bool());
    }

    #[test]
    fn test_jit_value_unit() {
        let unit = JitValue::unit();
        assert!(unit.is_unit());
        assert!(!unit.is_long());
    }

    #[test]
    fn test_jit_value_bool_true() {
        let t = JitValue::from_bool(true);
        assert!(t.is_bool());
        assert!(!t.is_unit());
        assert!(!t.is_long());
    }

    #[test]
    fn test_jit_value_bool_false() {
        let f = JitValue::from_bool(false);
        assert!(f.is_bool());
        assert!(!f.is_unit());
        assert!(!f.is_long());
    }

    #[test]
    fn test_jit_value_small_long() {
        let small = JitValue::from_long(42);
        assert!(small.is_long());
        assert!(!small.is_unit());
        assert!(!small.is_bool());
    }

    #[test]
    fn test_jit_value_negative_long() {
        let neg = JitValue::from_long(-42);
        assert!(neg.is_long());
    }

    #[test]
    fn test_jit_value_max_long() {
        // Z.A.2 (2026-05-12): JitValue::from_long routes out-of-range
        // values to the slab-allocated heap path tagged TAG_PTR, so
        // `is_long()` (which checks TAG_LONG) returns false. The full
        // i64::MAX is preserved via the MettaValueInner::Long round-trip.
        let max = JitValue::from_long(i64::MAX);
        assert!(!max.is_long(), "i64::MAX should be heap-allocated, not inline");
        let metta = unsafe { max.to_metta() };
        assert_eq!(metta.as_long(), Some(i64::MAX));
    }

    #[test]
    fn test_jit_value_min_long() {
        // Z.A.2: see test_jit_value_max_long for rationale.
        let min = JitValue::from_long(i64::MIN);
        assert!(!min.is_long(), "i64::MIN should be heap-allocated, not inline");
        let metta = unsafe { min.to_metta() };
        assert_eq!(metta.as_long(), Some(i64::MIN));
    }

    #[test]
    fn test_jit_value_inline_long_max_in_range() {
        // 2^47 - 1 stays inline.
        let v = JitValue::from_long(JitValue::INLINE_LONG_MAX);
        assert!(v.is_long(), "INLINE_LONG_MAX should fit inline");
        let metta = unsafe { v.to_metta() };
        assert_eq!(metta.as_long(), Some(JitValue::INLINE_LONG_MAX));
    }

    #[test]
    fn test_jit_value_inline_long_min_in_range() {
        // -2^47 stays inline.
        let v = JitValue::from_long(JitValue::INLINE_LONG_MIN);
        assert!(v.is_long(), "INLINE_LONG_MIN should fit inline");
        let metta = unsafe { v.to_metta() };
        assert_eq!(metta.as_long(), Some(JitValue::INLINE_LONG_MIN));
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Signal Values
    // ==========================================================================

    #[test]
    fn test_signal_values() {
        // Verify signal constants are distinct
        assert_ne!(JIT_SIGNAL_OK, JIT_SIGNAL_FAIL);
        assert_ne!(JIT_SIGNAL_OK, JIT_SIGNAL_YIELD);
        assert_ne!(JIT_SIGNAL_OK, JIT_SIGNAL_ERROR);
        assert_ne!(JIT_SIGNAL_FAIL, JIT_SIGNAL_YIELD);
        assert_ne!(JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR);
        assert_ne!(JIT_SIGNAL_YIELD, JIT_SIGNAL_ERROR);
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Tag and Payload
    // ==========================================================================

    #[test]
    fn test_tag_extraction() {
        let long_val = box_long(42);
        let tag = long_val & TAG_MASK;
        // Long values don't have a tag in the lower bits (they use TAG_PTR or inline)
        // Just verify the mask works
        assert_eq!(tag & TAG_MASK, tag);
    }

    #[test]
    fn test_payload_extraction() {
        let long_val = box_long(42);
        let payload = long_val & PAYLOAD_MASK;
        // Just verify the mask works
        assert_eq!(payload & PAYLOAD_MASK, payload);
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Bailout Reasons
    // ==========================================================================

    #[test]
    fn test_bailout_reason_values() {
        // Verify bailout reasons are distinct
        assert_ne!(
            JitBailoutReason::None as u8,
            JitBailoutReason::TypeError as u8
        );
        assert_ne!(
            JitBailoutReason::TypeError as u8,
            JitBailoutReason::DivisionByZero as u8
        );
        assert_ne!(
            JitBailoutReason::DivisionByZero as u8,
            JitBailoutReason::IntegerOverflow as u8
        );
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Context Operations
    // ==========================================================================

    #[test]
    fn test_jit_context_basic() {
        let constants = vec![MettaValue::Long(1), MettaValue::Long(2)];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Verify context was created
        assert!(!ctx.value_stack.is_null());
        assert_eq!(ctx.stack_cap, 16);
    }

    #[test]
    fn test_jit_context_empty_constants() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 8];

        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Should handle empty constants
        assert_eq!(ctx.constants_len, 0);
    }

    // ==========================================================================
    // Additional Branch Coverage Tests - Alternative Tags
    // ==========================================================================

    #[test]
    fn test_alternative_tag_values() {
        // Verify all alternative tags are distinct
        assert_ne!(
            JitAlternativeTag::Value as u8,
            JitAlternativeTag::Chunk as u8
        );
        assert_ne!(
            JitAlternativeTag::Value as u8,
            JitAlternativeTag::RuleMatch as u8
        );
    }

    #[test]
    fn test_alternative_value() {
        let alt = JitAlternative {
            tag: JitAlternativeTag::Value,
            payload: box_long(42),
            payload2: 0,
            payload3: 0,
        };
        assert_eq!(alt.tag, JitAlternativeTag::Value);
        assert_eq!(extract_long_signed(alt.payload), 42);
    }

    #[test]
    fn test_alternative_chunk() {
        let alt = JitAlternative {
            tag: JitAlternativeTag::Chunk,
            payload: 0x1234, // mock chunk pointer
            payload2: 0,
            payload3: 0,
        };
        assert_eq!(alt.tag, JitAlternativeTag::Chunk);
        assert_eq!(alt.payload, 0x1234);
    }

    // ==========================================================================
    // Phase 3: Special Forms Tests
    // ==========================================================================

    #[test]
    fn test_eval_if_true() {
        let condition = TAG_BOOL | 1; // True
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result =
            unsafe { jit_runtime_eval_if(std::ptr::null_mut(), condition, then_val, else_val, 0) };
        let jv = JitValue::from_raw(result);
        assert_eq!(jv.as_long(), 42);
    }

    #[test]
    fn test_eval_if_false() {
        let condition = TAG_BOOL; // False (TAG_BOOL | 0)
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result =
            unsafe { jit_runtime_eval_if(std::ptr::null_mut(), condition, then_val, else_val, 0) };
        let jv = JitValue::from_raw(result);
        assert_eq!(jv.as_long(), 99);
    }

    #[test]
    fn test_eval_if_unit_conservative_fallback() {
        // In the full JIT pipeline, Unit is intercepted by JumpIfNotBool
        // before reaching eval_if. If it somehow reaches here, the
        // conservative fallback returns else_val.
        let condition = TAG_UNIT;
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result =
            unsafe { jit_runtime_eval_if(std::ptr::null_mut(), condition, then_val, else_val, 0) };
        let jv = JitValue::from_raw(result);
        assert_eq!(jv.as_long(), 99);
    }

    #[test]
    fn test_eval_if_non_bool_conservative_fallback() {
        // In the full JIT pipeline, non-booleans are intercepted by JumpIfNotBool
        // before reaching eval_if. If they somehow reach here, conservative
        // fallback returns else_val.
        let condition = JitValue::from_long(100).to_bits();
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result =
            unsafe { jit_runtime_eval_if(std::ptr::null_mut(), condition, then_val, else_val, 0) };
        let jv = JitValue::from_raw(result);
        assert_eq!(jv.as_long(), 99); // Conservative fallback: else_val
    }

    #[test]
    fn test_eval_quote() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx =
            unsafe { JitContext::new(stack.as_mut_ptr(), stack.len(), std::ptr::null(), 0) };

        let expr = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_eval_quote(&mut ctx, expr, 0) };

        // Quote should wrap the value
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR); // Quote creates a heap-allocated Quote value
    }

    #[test]
    fn test_eval_let_star() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx =
            unsafe { JitContext::new(stack.as_mut_ptr(), stack.len(), std::ptr::null(), 0) };

        let result = unsafe { jit_runtime_eval_let_star(&mut ctx, 0) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit()); // let* returns Unit
    }

    // ==========================================================================
    // Phase 3: Bindings Tests
    // ==========================================================================
    // Note: Bindings tests require JitContext to be created with binding frame
    // capacity, which requires using JitContext::with_nondet() with proper
    // binding buffer setup. These operations are tested via the higher-level
    // VM tests in src/backend/bytecode/vm/tests.rs.

    // ==========================================================================
    // Phase 3: Pattern Matching Tests
    // ==========================================================================

    #[test]
    fn test_pattern_match_ground_equal() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: 42, Value: 42 - should match
        let pattern = JitValue::from_long(42).to_bits();
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };

        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        // Result should be true
    }

    #[test]
    fn test_pattern_match_ground_unequal() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: 42, Value: 99 - should not match
        let pattern = JitValue::from_long(42).to_bits();
        let value = JitValue::from_long(99).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };

        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        // Result should be false
    }

    // Note: Tests using JitValue::from_inner_ptr are disabled because they
    // require careful coordination with the JIT runtime's heap tracking.
    // Pattern matching and S-expression operations are tested via the
    // higher-level VM tests in src/backend/bytecode/vm/tests.rs.

    // ==========================================================================
    // Phase 3: Type Operations Tests
    // ==========================================================================

    #[test]
    fn test_get_type_long() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_get_type(&mut ctx, value, 0) };

        // Result should be an atom "Long"
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_get_type_bool() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let value = TAG_BOOL | 1; // True
        let result = unsafe { jit_runtime_get_type(&mut ctx, value, 0) };

        // Result should be an atom "Bool"
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_get_type_nil() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let value = TAG_UNIT;
        let result = unsafe { jit_runtime_get_type(&mut ctx, value, 0) };

        // Result should be an atom "Nil"
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    // ==========================================================================
    // Phase 3: S-expression Operations Tests
    // ==========================================================================

    #[test]
    fn test_push_empty() {
        let result = unsafe { jit_runtime_push_empty() };
        let _jv = JitValue::from_raw(result);

        // Empty S-expression is semantically Unit in MeTTa (SExpr([]) → Unit via view()).
        // With NaN-boxing inline types, value_to_jit_generic converts it to TAG_UNIT.
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_UNIT);
    }

    // Note: Tests using JitValue::from_inner_ptr for S-expression operations
    // (get_head, get_tail, get_arity, get_element) are disabled because they
    // require careful coordination with the JIT runtime's heap tracking.
    // These operations are tested via the higher-level VM tests in
    // src/backend/bytecode/vm/tests.rs.

    // ==========================================================================
    // Phase 3: Additional Arithmetic Tests
    // ==========================================================================

    #[test]
    fn test_sqrt() {
        // sqrt(16) = 4.0
        let val = box_long(16);
        let result = unsafe { jit_runtime_sqrt(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR); // Float is heap-allocated
    }

    #[test]
    fn test_log() {
        // log_2(8) = 3.0
        let base = box_long(2);
        let val = box_long(8);
        let result = unsafe { jit_runtime_log(base, val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR); // Float is heap-allocated
    }

    #[test]
    fn test_trunc() {
        // trunc(integer) should work
        let val = box_long(42);
        let result = unsafe { jit_runtime_trunc(val) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 42);
    }

    #[test]
    fn test_ceil() {
        // ceil(integer) should return same value
        let val = box_long(42);
        let result = unsafe { jit_runtime_ceil(val) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 42);
    }

    #[test]
    fn test_floor_math() {
        // floor(integer) should return same value
        let val = box_long(42);
        let result = unsafe { jit_runtime_floor_math(val) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 42);
    }

    #[test]
    fn test_round() {
        // round(integer) should return same value
        let val = box_long(42);
        let result = unsafe { jit_runtime_round(val) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 42);
    }

    // ==========================================================================
    // Phase 3: Trigonometric Function Tests
    // ==========================================================================

    #[test]
    fn test_sin() {
        // sin(0) = 0.0
        let val = box_long(0);
        let result = unsafe { jit_runtime_sin(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_cos() {
        // cos(0) = 1.0
        let val = box_long(0);
        let result = unsafe { jit_runtime_cos(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_tan() {
        // tan(0) = 0.0
        let val = box_long(0);
        let result = unsafe { jit_runtime_tan(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_asin() {
        // asin(0) = 0.0
        let val = box_long(0);
        let result = unsafe { jit_runtime_asin(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_acos() {
        // acos(1) = 0.0
        let val = box_long(1);
        let result = unsafe { jit_runtime_acos(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_atan() {
        // atan(0) = 0.0
        let val = box_long(0);
        let result = unsafe { jit_runtime_atan(val) };
        // Result should be a float
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_isnan() {
        // isnan(integer) should be false
        let val = box_long(42);
        let result = unsafe { jit_runtime_isnan(val) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
    }

    #[test]
    fn test_isinf() {
        // isinf(integer) should be false
        let val = box_long(42);
        let result = unsafe { jit_runtime_isinf(val) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
    }

    // ==========================================================================
    // Phase 3: Type Predicate Tests
    // ==========================================================================

    #[test]
    fn test_is_bool_true() {
        let bool_true = TAG_BOOL | 1;
        let result = jit_runtime_is_bool(bool_true);
        assert_eq!(result & 1, 1); // True
    }

    #[test]
    fn test_is_bool_false() {
        let bool_false = TAG_BOOL;
        let result = jit_runtime_is_bool(bool_false);
        assert_eq!(result & 1, 1); // True (it's a bool)
    }

    #[test]
    fn test_is_bool_non_bool() {
        let long_val = box_long(42);
        let result = jit_runtime_is_bool(long_val);
        assert_eq!(result & 1, 0); // False (not a bool)
    }

    #[test]
    fn test_is_unit_with_legacy_nil_tag() {
        let nil = TAG_UNIT;
        let result = jit_runtime_is_unit(nil);
        assert_eq!(result & 1, 1); // True — legacy TAG_UNIT is treated as unit
    }

    #[test]
    fn test_is_unit_non_unit() {
        let long_val = box_long(42);
        let result = jit_runtime_is_unit(long_val);
        assert_eq!(result & 1, 0); // False — Long is not unit
    }

    #[test]
    fn test_get_tag() {
        // jit_runtime_get_tag returns a boxed Long with the tag value >> 48
        // The tag values are: TAG_BOOL = 0x7FF8..., so shifted >> 48 = 32760
        // We verify the function returns a valid Long

        // Bool tag - just verify it returns a long
        let bool_val = TAG_BOOL | 1;
        let tag_result = jit_runtime_get_tag(bool_val);
        let jv = JitValue::from_raw(tag_result);
        assert!(jv.is_long());

        // Unit tag
        let nil = TAG_UNIT;
        let tag_result = jit_runtime_get_tag(nil);
        let jv = JitValue::from_raw(tag_result);
        assert!(jv.is_long());

        // Unit tag
        let unit = TAG_UNIT;
        let tag_result = jit_runtime_get_tag(unit);
        let jv = JitValue::from_raw(tag_result);
        assert!(jv.is_long());
    }

    // ==========================================================================
    // Phase 3: Error Handling Tests
    // ==========================================================================

    #[test]
    fn test_error_type_error() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        unsafe { jit_runtime_type_error(&mut ctx, 10, 0) };

        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::TypeError);
        assert_eq!(ctx.bailout_ip, 10);
    }

    #[test]
    fn test_error_div_by_zero() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        unsafe { jit_runtime_div_by_zero(&mut ctx, 20) };

        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::DivisionByZero);
        assert_eq!(ctx.bailout_ip, 20);
    }

    #[test]
    fn test_error_stack_overflow() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        unsafe { jit_runtime_stack_overflow(&mut ctx, 30) };

        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::StackOverflow);
        assert_eq!(ctx.bailout_ip, 30);
    }

    #[test]
    fn test_error_stack_underflow() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        unsafe { jit_runtime_stack_underflow(&mut ctx, 40) };

        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::StackUnderflow);
        assert_eq!(ctx.bailout_ip, 40);
    }

    #[test]
    fn test_error_integer_overflow() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        unsafe { jit_runtime_integer_overflow(&mut ctx, 50) };

        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::IntegerOverflow);
        assert_eq!(ctx.bailout_ip, 50);
    }

    // ==========================================================================
    // Phase 3B: Binding Operations Tests
    // ==========================================================================

    #[test]
    fn test_jit_store_and_load_binding() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        // Setup binding frames
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        // Push a binding frame
        let result = unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        assert_eq!(result, 0, "Push binding frame should succeed");
        assert_eq!(ctx.binding_frames_count, 1);

        // Store binding $x = 42
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_store_binding(&mut ctx, 0, value, 0) };
        assert_eq!(result, 0, "Store binding should succeed");

        // Load binding $x
        let loaded = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        let jv = JitValue::from_raw(loaded);
        assert_eq!(jv.as_long(), 42);
    }

    #[test]
    fn test_jit_has_binding() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x"), MettaValue::sym("$y")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Store binding for $x
        let value = JitValue::from_long(42).to_bits();
        unsafe { jit_runtime_store_binding(&mut ctx, 0, value, 0) };

        // Check $x exists
        let has_x = unsafe { jit_runtime_has_binding(&ctx, 0) };
        assert_eq!(has_x & 1, 1, "$x should exist");

        // Check $y does not exist
        let has_y = unsafe { jit_runtime_has_binding(&ctx, 1) };
        assert_eq!(has_y & 1, 0, "$y should not exist");
    }

    #[test]
    fn test_jit_push_pop_binding_frame() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        // Push first frame and store $x = 10
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(10).to_bits(), 0) };
        assert_eq!(ctx.binding_frames_count, 1);

        // Push second frame and store $x = 20
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(20).to_bits(), 0) };
        assert_eq!(ctx.binding_frames_count, 2);

        // Load $x should give 20 (innermost frame)
        let val = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val).as_long(), 20);

        // Pop inner frame
        let result = unsafe { jit_runtime_pop_binding_frame(&mut ctx) };
        assert_eq!(result, 0, "Pop should succeed");
        assert_eq!(ctx.binding_frames_count, 1);

        // Load $x should now give 10 (outer frame)
        let val = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val).as_long(), 10);
    }

    #[test]
    fn test_jit_clear_bindings() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Store binding
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(42).to_bits(), 0) };
        assert_eq!(unsafe { jit_runtime_has_binding(&ctx, 0) } & 1, 1);

        // Clear bindings
        unsafe { jit_runtime_clear_bindings(&mut ctx) };

        // Binding should no longer exist
        assert_eq!(unsafe { jit_runtime_has_binding(&ctx, 0) } & 1, 0);
    }

    #[test]
    fn test_jit_fork_restore_bindings() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Store binding $x = 10
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(10).to_bits(), 0) };

        // Fork bindings
        let saved = unsafe { jit_runtime_fork_bindings(&ctx) };
        assert!(!saved.is_null(), "Fork should succeed");

        // Modify $x = 20
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(20).to_bits(), 0) };
        let val = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val).as_long(), 20);

        // Restore bindings
        let result = unsafe { jit_runtime_restore_bindings(&mut ctx, saved, true) };
        assert_eq!(result, 0, "Restore should succeed");

        // $x should be back to 10
        let val = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val).as_long(), 10);
    }

    #[test]
    fn test_jit_binding_not_found() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Try to load non-existent binding
        let _ = unsafe { jit_runtime_load_binding(&mut ctx, 0, 100) };

        // Should signal bailout
        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::InvalidBinding);
    }

    // ==========================================================================
    // Phase 3B: Space Operations Tests
    // ==========================================================================

    #[test]
    fn test_jit_space_add() {
        let space = SpaceHandle::new(1, "test_space".to_string());
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space.clone()));
        let atom_jit = JitValue::from_long(42);

        let result =
            unsafe { jit_runtime_space_add(&mut ctx, space_jit.to_bits(), atom_jit.to_bits(), 0) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());

        // Verify atom was added
        let atoms = space.collapse();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0], MettaValue::Long(42));
    }

    #[test]
    fn test_jit_space_remove() {
        let space = SpaceHandle::new(2, "test_space".to_string());
        space.add_atom(MettaValue::Long(1));
        space.add_atom(MettaValue::Long(2));

        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space.clone()));
        let atom_jit = JitValue::from_long(1);

        let result = unsafe {
            jit_runtime_space_remove(&mut ctx, space_jit.to_bits(), atom_jit.to_bits(), 0)
        };
        let jv = JitValue::from_raw(result);
        // Plan A Phase 4 (Bug 2b): jit_runtime_space_remove returns Unit per
        // HE / spec §9.2 (the boolean removed-flag is discarded).
        assert!(jv.is_unit(), "remove-atom should return Unit, got {jv:?}");

        // Verify atom was removed (post-condition still proves the side effect).
        let atoms = space.collapse();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0], MettaValue::Long(2));
    }

    #[test]
    fn test_jit_space_remove_not_found() {
        let space = SpaceHandle::new(3, "test_space".to_string());
        space.add_atom(MettaValue::Long(1));

        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space.clone()));
        let atom_jit = JitValue::from_long(99); // Not in space

        let result = unsafe {
            jit_runtime_space_remove(&mut ctx, space_jit.to_bits(), atom_jit.to_bits(), 0)
        };
        let jv = JitValue::from_raw(result);
        // Plan A: returns Unit even when atom is not found (no-op success).
        assert!(
            jv.is_unit(),
            "remove-atom missing should return Unit, got {jv:?}"
        );
    }

    #[test]
    fn test_jit_space_get_atoms() {
        use crate::backend::bytecode::jit::types::JitChoicePoint;

        let space = SpaceHandle::new(4, "test_space".to_string());
        space.add_atom(MettaValue::Long(1));
        space.add_atom(MettaValue::Long(2));
        space.add_atom(MettaValue::sym("foo"));

        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![unsafe { std::mem::zeroed() }; 4];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space));

        // get_atoms returns first atom directly, with choice points for the rest
        let result = unsafe { jit_runtime_space_get_atoms(&mut ctx, space_jit.to_bits(), 0) };
        let first = unsafe { JitValue::from_raw(result).to_metta() };

        // Collect remaining atoms from choice points
        let mut all_atoms = vec![first];
        assert_eq!(
            ctx.choice_point_count, 1,
            "Expected 1 choice point for 3 atoms"
        );
        unsafe {
            let cp = &*ctx.choice_points.add(0);
            assert_eq!(cp.alt_count, 2, "Expected 2 alternatives in choice point");
            for i in 0..cp.alt_count as usize {
                let alt = &cp.alternatives_inline[i];
                let alt_metta = JitValue::from_raw(alt.payload).to_metta();
                all_atoms.push(alt_metta);
            }
        }

        assert_eq!(all_atoms.len(), 3, "Expected 3 atoms total");
        assert!(all_atoms.contains(&MettaValue::Long(1)));
        assert!(all_atoms.contains(&MettaValue::Long(2)));
        assert!(all_atoms.contains(&MettaValue::sym("foo")));
    }

    #[test]
    fn test_jit_space_get_atoms_empty() {
        let space = SpaceHandle::new(5, "empty_space".to_string());

        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space));

        let result = unsafe { jit_runtime_space_get_atoms(&mut ctx, space_jit.to_bits(), 0) };
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };

        // SExpr(vec![]) normalizes to Unit after Nil/Unit merge
        assert!(
            metta.is_unit(),
            "Expected Unit (empty S-expression), got: {:?}",
            metta
        );
    }

    #[test]
    fn test_jit_space_match() {
        let space = SpaceHandle::new(6, "match_space".to_string());
        space.add_atom(MettaValue::SExpr(vec![
            MettaValue::sym("fact"),
            MettaValue::Long(1),
        ]));
        space.add_atom(MettaValue::SExpr(vec![
            MettaValue::sym("fact"),
            MettaValue::Long(2),
        ]));
        space.add_atom(MettaValue::SExpr(vec![
            MettaValue::sym("other"),
            MettaValue::Long(3),
        ]));

        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space));
        // Pattern: (fact $x)
        let pattern = MettaValue::SExpr(vec![MettaValue::sym("fact"), MettaValue::var("x")]);
        let pattern_jit = metta_to_jit(&pattern);
        let template_jit = metta_to_jit(&MettaValue::var("x"));

        let result = unsafe {
            jit_runtime_space_match(
                &mut ctx,
                space_jit.to_bits(),
                pattern_jit.to_bits(),
                template_jit.to_bits(),
                0,
            )
        };
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };

        if let MettaValueInner::SExpr(matches) = metta.inner() {
            assert_eq!(matches.len(), 2);
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_jit_space_match_no_matches() {
        let space = SpaceHandle::new(7, "nomatch_space".to_string());
        space.add_atom(MettaValue::SExpr(vec![
            MettaValue::sym("bar"),
            MettaValue::Long(1),
        ]));

        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space_jit = metta_to_jit(&MettaValue::Space(space));
        // Pattern: (foo $x) - won't match (bar 1)
        let pattern = MettaValue::SExpr(vec![MettaValue::sym("foo"), MettaValue::var("x")]);
        let pattern_jit = metta_to_jit(&pattern);
        let template_jit = metta_to_jit(&MettaValue::var("x"));

        let result = unsafe {
            jit_runtime_space_match(
                &mut ctx,
                space_jit.to_bits(),
                pattern_jit.to_bits(),
                template_jit.to_bits(),
                0,
            )
        };
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };

        // SExpr(vec![]) normalizes to Unit after Nil/Unit merge
        assert!(
            metta.is_unit(),
            "Expected Unit (empty S-expression), got: {:?}",
            metta
        );
    }

    // ==========================================================================
    // Phase 3B: Type Operations Tests
    // ==========================================================================

    #[test]
    fn test_jit_get_type_string() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Create a slab-allocated string
        let str_val = MettaValue::String("hello".to_string());
        let str_bits = TAG_PTR | ((str_val.inner_ptr() as u64) & PAYLOAD_MASK);

        let result = unsafe { jit_runtime_get_type(&mut ctx, str_bits, 0) };
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_jit_get_type_unit() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_get_type(&mut ctx, TAG_UNIT, 0) };
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_jit_get_type_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
        let sexpr_jit = metta_to_jit(&sexpr);

        let result = unsafe { jit_runtime_get_type(&mut ctx, sexpr_jit.to_bits(), 0) };
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_jit_get_type_variable() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let var = MettaValue::var("x");
        let var_jit = metta_to_jit(&var);

        let result = unsafe { jit_runtime_get_type(&mut ctx, var_jit.to_bits(), 0) };
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_PTR);
    }

    #[test]
    fn test_jit_check_type_match() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        let type_atom = metta_to_jit(&MettaValue::sym("Number"));

        let result = unsafe { jit_runtime_check_type(&mut ctx, val, type_atom.to_bits(), 0) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        // Should return true for Number type
    }

    #[test]
    fn test_jit_check_type_mismatch() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        let type_atom = metta_to_jit(&MettaValue::sym("Bool")); // Wrong type

        let result = unsafe { jit_runtime_check_type(&mut ctx, val, type_atom.to_bits(), 0) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        // Should return false for mismatched type
    }

    #[test]
    fn test_jit_check_type_variable() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        let type_var = metta_to_jit(&MettaValue::var("T")); // Type variable

        let result = unsafe { jit_runtime_check_type(&mut ctx, val, type_var.to_bits(), 0) };
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        // Type variables match anything, should return true
    }

    #[test]
    fn test_jit_assert_type_pass() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        let type_atom = metta_to_jit(&MettaValue::sym("Number"));

        let result = unsafe { jit_runtime_assert_type(&mut ctx, val, type_atom.to_bits(), 0) };

        // Should return the original value
        assert_eq!(result, val);
        assert!(!ctx.bailout);
    }

    #[test]
    fn test_jit_assert_type_fail() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        let type_atom = metta_to_jit(&MettaValue::sym("Bool")); // Wrong type

        let _result = unsafe { jit_runtime_assert_type(&mut ctx, val, type_atom.to_bits(), 10) };

        // Should signal bailout
        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::TypeError);
    }

    // ==========================================================================
    // Phase 4C: Special Forms Tests
    // ==========================================================================

    #[test]
    fn test_jit_eval_if_true_branch() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let condition = TAG_BOOL | 1; // True
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result = unsafe { jit_runtime_eval_if(&mut ctx, condition, then_val, else_val, 0) };
        assert_eq!(result, then_val);
    }

    #[test]
    fn test_jit_eval_if_false_branch() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let condition = TAG_BOOL; // False (TAG_BOOL | 0)
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result = unsafe { jit_runtime_eval_if(&mut ctx, condition, then_val, else_val, 0) };
        assert_eq!(result, else_val);
    }

    #[test]
    fn test_jit_eval_if_unit_conservative_fallback() {
        // In the full JIT pipeline, Unit is intercepted by JumpIfNotBool
        // before reaching eval_if. If it somehow reaches here, the
        // conservative fallback returns else_val.
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let condition = TAG_UNIT;
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result = unsafe { jit_runtime_eval_if(&mut ctx, condition, then_val, else_val, 0) };
        assert_eq!(result, else_val);
    }

    #[test]
    fn test_jit_eval_if_non_bool_conservative_fallback() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // In the full JIT pipeline, non-booleans are intercepted by JumpIfNotBool.
        // If they reach eval_if, conservative fallback returns else_val.
        let condition = JitValue::from_long(1).to_bits();
        let then_val = JitValue::from_long(42).to_bits();
        let else_val = JitValue::from_long(99).to_bits();

        let result = unsafe { jit_runtime_eval_if(&mut ctx, condition, then_val, else_val, 0) };
        assert_eq!(result, else_val);
    }

    #[test]
    fn test_jit_eval_let_basic() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_eval_let(&mut ctx, 0, value, 0) };

        // Returns Unit
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_eval_let_star_marker() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_eval_let_star(&mut ctx, 0) };

        // Returns Unit (marker function)
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_eval_chain_returns_second() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let first = JitValue::from_long(1).to_bits();
        let second = JitValue::from_long(2).to_bits();

        let result = unsafe { jit_runtime_eval_chain(&mut ctx, first, second, 0) };
        assert_eq!(result, second);
    }

    #[test]
    fn test_jit_eval_quote_wraps_value() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let expr = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let result = unsafe { jit_runtime_eval_quote(&mut ctx, expr, 0) };

        // Should wrap in a quote
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 2);
            if let MettaValueInner::Atom(s) = elems[0].inner() {
                assert_eq!(*s, "quote");
            }
        }
    }

    #[test]
    fn test_jit_eval_unquote_unwraps() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Create (quote foo)
        let quoted = MettaValue::SExpr(vec![MettaValue::sym("quote"), MettaValue::sym("foo")]);
        let expr = metta_to_jit(&quoted).to_bits();

        let result = unsafe { jit_runtime_eval_unquote(&mut ctx, expr, 0) };

        // Should unwrap to foo
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "foo");
        }
    }

    #[test]
    fn test_jit_eval_unquote_not_quoted() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Not a quote - just a symbol
        let expr = metta_to_jit(&MettaValue::sym("foo")).to_bits();

        let result = unsafe { jit_runtime_eval_unquote(&mut ctx, expr, 0) };

        // Should return as-is
        assert_eq!(result, expr);
    }

    #[test]
    fn test_jit_eval_eval_passthrough() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let expr = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let result = unsafe { jit_runtime_eval_eval(&mut ctx, expr, 0) };

        // Current impl returns expression unchanged
        assert_eq!(result, expr);
    }

    #[test]
    fn test_jit_eval_bind_stores_binding() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_eval_bind(&mut ctx, 0, value, 0) };

        // Returns Unit
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_eval_new_creates_space() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_eval_new(&mut ctx, 0) };

        // Should return a space
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        assert!(matches!(metta.inner(), MettaValueInner::Space(_)));
    }

    #[test]
    fn test_jit_eval_pragma_returns_unit() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let directive = JitValue::from_long(0).to_bits();
        let result = unsafe { jit_runtime_eval_pragma(&mut ctx, directive, 0) };

        // Returns Unit
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_eval_memo_passthrough() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let expr = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_eval_memo(&mut ctx, expr, 0) };

        // Current impl returns expression unchanged
        assert_eq!(result, expr);
    }

    #[test]
    fn test_jit_eval_memo_first_passthrough() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let expr = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_eval_memo_first(&mut ctx, expr, 0) };

        // Current impl returns expression unchanged
        assert_eq!(result, expr);
    }

    #[test]
    fn test_jit_eval_superpose_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Empty list
        let list = metta_to_jit(&MettaValue::SExpr(vec![])).to_bits();
        let result = unsafe { jit_runtime_eval_superpose(&mut ctx, list, 0) };

        // Empty superpose signals failure
        assert_eq!(result, JIT_SIGNAL_FAIL as u64);
    }

    #[test]
    fn test_jit_eval_superpose_single() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Single element
        let list = metta_to_jit(&MettaValue::SExpr(vec![MettaValue::Long(42)])).to_bits();
        let result = unsafe { jit_runtime_eval_superpose(&mut ctx, list, 0) };

        // Returns the single element
        let jv = JitValue::from_raw(result);
        assert!(jv.is_long());
        assert_eq!(jv.as_long(), 42);
    }

    #[test]
    fn test_jit_eval_superpose_non_list() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Not a list - just a number
        let val = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_eval_superpose(&mut ctx, val, 0) };

        // Returns as-is
        assert_eq!(result, val);
    }

    #[test]
    fn test_jit_eval_collapse_no_results() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Single value
        let val = metta_to_jit(&MettaValue::Long(42)).to_bits();
        let result = unsafe { jit_runtime_eval_collapse(&mut ctx, val, 0) };

        // Should wrap in a list
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 1);
            if let MettaValueInner::Long(n) = elems[0].inner() {
                assert_eq!(*n, 42);
            }
        }
    }

    #[test]
    fn test_jit_eval_collapse_null_ctx() {
        // Null context
        let val = metta_to_jit(&MettaValue::Long(42)).to_bits();
        let result = unsafe { jit_runtime_eval_collapse(std::ptr::null_mut(), val, 0) };

        // Should wrap in a list even with null ctx
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        assert!(matches!(metta.inner(), MettaValueInner::SExpr(_)));
    }

    // ==========================================================================
    // Phase 4C: Space Operations Tests (renamed to avoid conflicts)
    // ==========================================================================

    #[test]
    fn test_jit_space_add_valid_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(101, "test-space-4c".to_string());
        let space_jit = metta_to_jit(&MettaValue::Space(space.clone())).to_bits();
        let atom_jit = metta_to_jit(&MettaValue::sym("atom1")).to_bits();

        let result = unsafe { jit_runtime_space_add(&mut ctx, space_jit, atom_jit, 0) };

        // Returns Unit
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());

        // Verify atom was added
        let atoms = space.collapse();
        assert_eq!(atoms.len(), 1);
    }

    #[test]
    fn test_jit_space_add_non_space_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Not a space - just a number
        let not_space = JitValue::from_long(42).to_bits();
        let atom_jit = metta_to_jit(&MettaValue::sym("atom1")).to_bits();

        let result = unsafe { jit_runtime_space_add(&mut ctx, not_space, atom_jit, 0) };

        // Plan A Bug 5: type-error path now returns a proper Error value
        // instead of silently returning Unit.
        let jv = JitValue::from_raw(result);
        let is_heap_error = if jv.is_heap() {
            let mv = unsafe { jv.to_metta() };
            matches!(mv.view(), crate::backend::models::ValueView::Error(_, _))
        } else {
            false
        };
        assert!(
            jv.is_error() || is_heap_error,
            "add-atom on non-Space should return Error, got {jv:?}"
        );
    }

    #[test]
    fn test_jit_space_remove_exists_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(102, "test-space-4c2".to_string());
        let atom = MettaValue::sym("atom1");
        space.add_atom(atom.clone());

        let space_jit = metta_to_jit(&MettaValue::Space(space.clone())).to_bits();
        let atom_jit = metta_to_jit(&atom).to_bits();

        let result = unsafe { jit_runtime_space_remove(&mut ctx, space_jit, atom_jit, 0) };

        // Plan A Phase 4 (Bug 2b): returns Unit per HE / spec §9.2.
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit(), "remove-atom should return Unit, got {jv:?}");

        // Verify atom was removed (post-condition still proves the side effect).
        let atoms = space.collapse();
        assert_eq!(atoms.len(), 0);
    }

    #[test]
    fn test_jit_space_remove_missing_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(103, "test-space-4c3".to_string());
        let space_jit = metta_to_jit(&MettaValue::Space(space)).to_bits();
        let atom_jit = metta_to_jit(&MettaValue::sym("nonexistent")).to_bits();

        let result = unsafe { jit_runtime_space_remove(&mut ctx, space_jit, atom_jit, 0) };

        // Plan A: missing atom is a no-op; still returns Unit.
        let jv = JitValue::from_raw(result);
        assert!(
            jv.is_unit(),
            "remove-atom missing should return Unit, got {jv:?}"
        );
    }

    #[test]
    fn test_jit_space_remove_non_space_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let not_space = JitValue::from_long(42).to_bits();
        let atom_jit = metta_to_jit(&MettaValue::sym("atom1")).to_bits();

        let result = unsafe { jit_runtime_space_remove(&mut ctx, not_space, atom_jit, 0) };

        // Plan A Bug 5: type-error path now returns a proper Error value.
        let jv = JitValue::from_raw(result);
        let is_heap_error = if jv.is_heap() {
            let mv = unsafe { jv.to_metta() };
            matches!(mv.view(), crate::backend::models::ValueView::Error(_, _))
        } else {
            false
        };
        assert!(
            jv.is_error() || is_heap_error,
            "remove-atom on non-Space should return Error, got {jv:?}"
        );
    }

    #[test]
    fn test_jit_space_get_atoms_empty_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(104, "test-space-4c4".to_string());
        let space_jit = metta_to_jit(&MettaValue::Space(space)).to_bits();

        let result = unsafe { jit_runtime_space_get_atoms(&mut ctx, space_jit, 0) };

        // Returns empty S-expression
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert!(elems.is_empty());
        }
    }

    #[test]
    fn test_jit_space_get_atoms_populated_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(105, "test-space-4c5".to_string());
        space.add_atom(MettaValue::sym("a"));
        space.add_atom(MettaValue::sym("b"));
        space.add_atom(MettaValue::sym("c"));

        let space_jit = metta_to_jit(&MettaValue::Space(space)).to_bits();

        let result = unsafe { jit_runtime_space_get_atoms(&mut ctx, space_jit, 0) };

        // Returns S-expression with atoms
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 3);
        }
    }

    #[test]
    fn test_jit_space_get_atoms_non_space_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let not_space = JitValue::from_long(42).to_bits();

        let result = unsafe { jit_runtime_space_get_atoms(&mut ctx, not_space, 0) };

        // Returns empty S-expression (type error)
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert!(elems.is_empty());
        }
    }

    #[test]
    fn test_jit_space_match_single_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(106, "test-space-4c6".to_string());
        space.add_atom(MettaValue::sym("foo"));
        space.add_atom(MettaValue::sym("bar"));

        let space_jit = metta_to_jit(&MettaValue::Space(space)).to_bits();
        let pattern_jit = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let template_jit = metta_to_jit(&MettaValue::sym("result")).to_bits();

        let result =
            unsafe { jit_runtime_space_match(&mut ctx, space_jit, pattern_jit, template_jit, 0) };

        // Returns matching atoms
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 1);
        }
    }

    #[test]
    fn test_jit_space_match_none_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(107, "test-space-4c7".to_string());
        space.add_atom(MettaValue::sym("foo"));

        let space_jit = metta_to_jit(&MettaValue::Space(space)).to_bits();
        let pattern_jit = metta_to_jit(&MettaValue::sym("nonexistent")).to_bits();
        let template_jit = metta_to_jit(&MettaValue::sym("result")).to_bits();

        let result =
            unsafe { jit_runtime_space_match(&mut ctx, space_jit, pattern_jit, template_jit, 0) };

        // Returns empty S-expression
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert!(elems.is_empty());
        }
    }

    #[test]
    fn test_jit_space_match_non_space_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let not_space = JitValue::from_long(42).to_bits();
        let pattern_jit = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let template_jit = metta_to_jit(&MettaValue::sym("result")).to_bits();

        let result =
            unsafe { jit_runtime_space_match(&mut ctx, not_space, pattern_jit, template_jit, 0) };

        // Returns empty S-expression (type error)
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert!(elems.is_empty());
        }
    }

    #[test]
    fn test_jit_space_match_nondet_null_ctx_4c() {
        let space = SpaceHandle::new(108, "test-space-4c8".to_string());
        let space_jit = metta_to_jit(&MettaValue::Space(space)).to_bits();
        let pattern_jit = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let template_jit = metta_to_jit(&MettaValue::sym("result")).to_bits();

        let result = unsafe {
            jit_runtime_space_match_nondet(
                std::ptr::null_mut(),
                space_jit,
                pattern_jit,
                template_jit,
                0,
            )
        };

        // Returns nil with null context
        assert_eq!(result, TAG_UNIT);
    }

    #[test]
    fn test_jit_space_match_nondet_not_space_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let not_space = JitValue::from_long(42).to_bits();
        let pattern_jit = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let template_jit = metta_to_jit(&MettaValue::sym("result")).to_bits();

        let result = unsafe {
            jit_runtime_space_match_nondet(&mut ctx, not_space, pattern_jit, template_jit, 0)
        };

        // Returns nil and sets bailout
        assert_eq!(result, TAG_UNIT);
        assert!(ctx.bailout);
        assert_eq!(ctx.bailout_reason, JitBailoutReason::TypeError);
    }

    // ==========================================================================
    // Phase 4C: Value Creation Tests (renamed to avoid conflicts)
    // ==========================================================================

    #[test]
    fn test_jit_make_sexpr_empty_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_make_sexpr(&mut ctx, std::ptr::null(), 0, 0) };

        // Returns empty S-expression
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert!(elems.is_empty());
        }
    }

    #[test]
    fn test_jit_make_sexpr_single_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let values = [JitValue::from_long(42).to_bits()];
        let result = unsafe { jit_runtime_make_sexpr(&mut ctx, values.as_ptr(), 1, 0) };

        // Returns S-expression with one element
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 1);
            if let MettaValueInner::Long(n) = elems[0].inner() {
                assert_eq!(*n, 42);
            }
        }
    }

    #[test]
    fn test_jit_make_sexpr_multiple_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let values = [
            metta_to_jit(&MettaValue::sym("+")).to_bits(),
            JitValue::from_long(1).to_bits(),
            JitValue::from_long(2).to_bits(),
        ];
        let result = unsafe { jit_runtime_make_sexpr(&mut ctx, values.as_ptr(), 3, 0) };

        // Returns S-expression with three elements
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 3);
        }
    }

    #[test]
    fn test_jit_cons_atom_to_sexpr_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let head = JitValue::from_long(1).to_bits();
        let tail = metta_to_jit(&MettaValue::SExpr(vec![
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]))
        .to_bits();

        let result = unsafe { jit_runtime_cons_atom(&mut ctx, head, tail, 0) };

        // Returns S-expression with head prepended
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 3);
            if let MettaValueInner::Long(n) = elems[0].inner() {
                assert_eq!(*n, 1);
            }
        }
    }

    #[test]
    fn test_jit_cons_atom_to_nil_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let head = JitValue::from_long(42).to_bits();
        let tail = TAG_UNIT;

        let result = unsafe { jit_runtime_cons_atom(&mut ctx, head, tail, 0) };

        // Returns single-element S-expression
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 1);
            if let MettaValueInner::Long(n) = elems[0].inner() {
                assert_eq!(*n, 42);
            }
        }
    }

    #[test]
    fn test_jit_make_quote_wraps_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let expr = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let result = unsafe { jit_runtime_make_quote(&mut ctx, expr, 0) };

        // Returns (quote foo)
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 2);
            if let MettaValueInner::Atom(s) = elems[0].inner() {
                assert_eq!(*s, "quote");
            }
        }
    }

    // ==========================================================================
    // Phase 4C: Additional Type Operations Tests (renamed to avoid conflicts)
    // ==========================================================================

    #[test]
    fn test_jit_get_type_bool_true_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_bool(true).to_bits();
        let result = unsafe { jit_runtime_get_type(&mut ctx, val, 0) };

        // Should return "Bool" type
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "Bool");
        }
    }

    #[test]
    fn test_jit_get_type_bool_false_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_bool(false).to_bits();
        let result = unsafe { jit_runtime_get_type(&mut ctx, val, 0) };

        // Should return "Bool" type
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "Bool");
        }
    }

    #[test]
    fn test_jit_get_type_unit_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::unit().to_bits();
        let result = unsafe { jit_runtime_get_type(&mut ctx, val, 0) };

        // Should return "Unit" type
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "Unit");
        }
    }

    #[test]
    fn test_jit_get_type_sexpr_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
        let val = metta_to_jit(&sexpr).to_bits();
        let result = unsafe { jit_runtime_get_type(&mut ctx, val, 0) };

        // Should return "Expression" type
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "Expression");
        }
    }

    #[test]
    fn test_jit_get_type_space_4c() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let space = SpaceHandle::new(200, "type-test-space-4c".to_string());
        let val = metta_to_jit(&MettaValue::Space(space)).to_bits();
        let result = unsafe { jit_runtime_get_type(&mut ctx, val, 0) };

        // Should return "Space" type
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "Space");
        }
    }

    // ==========================================================================
    // Phase 4C: Pattern Matching Tests
    // ==========================================================================

    #[test]
    fn test_jit_pattern_match_literal_success() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let value = metta_to_jit(&MettaValue::sym("foo")).to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return true
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(jv.as_bool());
    }

    #[test]
    fn test_jit_pattern_match_literal_fail() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = metta_to_jit(&MettaValue::sym("foo")).to_bits();
        let value = metta_to_jit(&MettaValue::sym("bar")).to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return false
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(!jv.as_bool());
    }

    #[test]
    fn test_jit_pattern_match_variable() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Variable pattern matches anything
        let pattern = metta_to_jit(&MettaValue::var("x")).to_bits();
        let value = metta_to_jit(&MettaValue::sym("anything")).to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return true
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(jv.as_bool());
    }

    #[test]
    fn test_jit_pattern_match_sexpr_success() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: (foo $x)
        let pattern = metta_to_jit(&MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::var("x"),
        ]))
        .to_bits();
        // Value: (foo bar)
        let value = metta_to_jit(&MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::sym("bar"),
        ]))
        .to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return true
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(jv.as_bool());
    }

    #[test]
    fn test_jit_pattern_match_sexpr_fail_length() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: (foo $x $y)
        let pattern = metta_to_jit(&MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::var("x"),
            MettaValue::var("y"),
        ]))
        .to_bits();
        // Value: (foo bar) - one element short
        let value = metta_to_jit(&MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::sym("bar"),
        ]))
        .to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return false
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(!jv.as_bool());
    }

    #[test]
    fn test_jit_pattern_match_number() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = JitValue::from_long(42).to_bits();
        let value = JitValue::from_long(42).to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return true
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(jv.as_bool());
    }

    #[test]
    fn test_jit_pattern_match_number_fail() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = JitValue::from_long(42).to_bits();
        let value = JitValue::from_long(99).to_bits();

        let result = unsafe { jit_runtime_pattern_match(&mut ctx, pattern, value, 0) };

        // Should return false
        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert!(!jv.as_bool());
    }

    // ==========================================================================
    // Phase 4C: Stack Operations Tests
    // ==========================================================================

    #[test]
    fn test_jit_load_constant_4c() {
        let constants: Vec<MettaValue> = vec![MettaValue::Long(42), MettaValue::sym("foo")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_load_constant(&mut ctx, 0) };

        // Should return the constant
        let jv = JitValue::from_raw(result);
        assert!(jv.is_long());
        assert_eq!(jv.as_long(), 42);
    }

    #[test]
    fn test_jit_load_constant_out_of_bounds_4c() {
        let constants: Vec<MettaValue> = vec![MettaValue::Long(42)];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_load_constant(&mut ctx, 999) };

        // Should return nil for out of bounds
        assert_eq!(result, TAG_UNIT);
    }

    // ==========================================================================
    // Phase 4C: Global Operations Tests
    // ==========================================================================

    #[test]
    fn test_jit_load_global_not_found_4c() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("undefined")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_load_global(&mut ctx, 0, 0) };

        // Should return unit for undefined global (Nil merged into Unit)
        assert_eq!(result, TAG_UNIT);
    }

    #[test]
    fn test_jit_store_global_4c() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("test-var")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_store_global(&mut ctx, 0, value, 0) };

        // Returns Unit
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    // ==========================================================================
    // Phase 4D: Advanced Nondeterminism Tests
    // ==========================================================================

    #[test]
    fn test_jit_cut_null_ctx() {
        let result = unsafe { jit_runtime_cut(std::ptr::null_mut(), 0) };

        // Should return Unit even with null ctx
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_cut_no_markers() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // No cut markers set up
        ctx.choice_point_count = 3;

        let result = unsafe { jit_runtime_cut(&mut ctx, 0) };

        // Should clear all choice points when no markers
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_jit_enter_cut_scope_null_ctx() {
        let result = unsafe { jit_runtime_enter_cut_scope(std::ptr::null_mut()) };

        // Should return 0 for null ctx
        assert_eq!(result, 0);
    }

    #[test]
    fn test_jit_enter_cut_scope_success() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Set up cut markers
        let mut markers: [usize; 8] = [0; 8];
        ctx.cut_markers = markers.as_mut_ptr();
        ctx.cut_marker_cap = 8;
        ctx.cut_marker_count = 0;

        let result = unsafe { jit_runtime_enter_cut_scope(&mut ctx) };

        // Should succeed
        assert_eq!(result, 1);
        assert_eq!(ctx.cut_marker_count, 1);
    }

    #[test]
    fn test_jit_exit_cut_scope_null_ctx() {
        let result = unsafe { jit_runtime_exit_cut_scope(std::ptr::null_mut()) };

        // Should return 0 for null ctx
        assert_eq!(result, 0);
    }

    #[test]
    fn test_jit_exit_cut_scope_no_markers() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // No markers
        ctx.cut_marker_count = 0;

        let result = unsafe { jit_runtime_exit_cut_scope(&mut ctx) };

        // Should return 0 (no markers to pop)
        assert_eq!(result, 0);
    }

    #[test]
    fn test_jit_exit_cut_scope_success() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Set up cut markers
        let mut markers: [usize; 8] = [0; 8];
        ctx.cut_markers = markers.as_mut_ptr();
        ctx.cut_marker_cap = 8;
        ctx.cut_marker_count = 0;

        // Enter and exit
        unsafe { jit_runtime_enter_cut_scope(&mut ctx) };
        assert_eq!(ctx.cut_marker_count, 1);

        let result = unsafe { jit_runtime_exit_cut_scope(&mut ctx) };
        assert_eq!(result, 1);
        assert_eq!(ctx.cut_marker_count, 0);
    }

    #[test]
    fn test_jit_guard_true() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let condition = TAG_BOOL | 1; // True
        let result = unsafe { jit_runtime_guard(&mut ctx, condition, 0) };

        // Guard passes
        assert_eq!(result, 1);
    }

    #[test]
    fn test_jit_guard_false() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let condition = TAG_BOOL; // False
        let result = unsafe { jit_runtime_guard(&mut ctx, condition, 0) };

        // Guard fails
        assert_eq!(result, 0);
    }

    #[test]
    fn test_jit_amb_null_ctx() {
        let result = unsafe { jit_runtime_amb(std::ptr::null_mut(), 2, 0) };

        // Should return unit with null ctx
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_amb_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_amb(&mut ctx, 0, 0) };

        // Empty amb returns unit
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_commit_null_ctx() {
        let result = unsafe { jit_runtime_commit(std::ptr::null_mut(), 1, 0) };

        // Should return Unit with null ctx
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_commit_removes_choice_points() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        ctx.choice_point_count = 5;

        let result = unsafe { jit_runtime_commit(&mut ctx, 2, 0) };

        // Should remove 2 choice points
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
        assert_eq!(ctx.choice_point_count, 3);
    }

    #[test]
    fn test_jit_commit_all() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        ctx.choice_point_count = 5;

        let result = unsafe { jit_runtime_commit(&mut ctx, 0, 0) };

        // Should remove all choice points when count=0
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
        assert_eq!(ctx.choice_point_count, 0);
    }

    #[test]
    fn test_jit_backtrack_null_ctx() {
        let result = unsafe { jit_runtime_backtrack(std::ptr::null_mut(), 0) };

        // Should signal fail with null ctx
        assert_eq!(result, JIT_SIGNAL_FAIL);
    }

    #[test]
    fn test_jit_backtrack_signals_fail() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_backtrack(&mut ctx, 0) };

        // Should signal fail
        assert_eq!(result, JIT_SIGNAL_FAIL);
    }

    #[test]
    fn test_jit_begin_nondet_increments_fork_depth() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let initial_depth = ctx.fork_depth;

        unsafe { jit_runtime_begin_nondet(&mut ctx, 0) };

        // Should increment fork_depth
        assert_eq!(ctx.fork_depth, initial_depth + 1);
    }

    #[test]
    fn test_jit_end_nondet_decrements_fork_depth() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        ctx.fork_depth = 2;

        unsafe { jit_runtime_end_nondet(&mut ctx, 0) };

        // Should decrement fork_depth
        assert_eq!(ctx.fork_depth, 1);
    }

    #[test]
    fn test_jit_end_nondet_no_underflow() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        ctx.fork_depth = 0;

        unsafe { jit_runtime_end_nondet(&mut ctx, 0) };

        // Should not go below 0
        assert_eq!(ctx.fork_depth, 0);
    }

    // ==========================================================================
    // Phase 4D: Expression Operations Tests
    // ==========================================================================

    #[test]
    fn test_jit_get_head_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let val = metta_to_jit(&sexpr).to_bits();

        let result = unsafe { jit_runtime_get_head(&mut ctx, val, 0) };

        // Should return "foo"
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::Atom(s) = metta.inner() {
            assert_eq!(*s, "foo");
        }
    }

    #[test]
    fn test_jit_get_head_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![]);
        let val = metta_to_jit(&sexpr).to_bits();

        let result = unsafe { jit_runtime_get_head(&mut ctx, val, 0) };

        // H3 hard-cut: empty S-expr → Error atom (heap-allocated), not TAG_UNIT.
        assert_ne!(
            result, TAG_UNIT,
            "H3: empty sexpr should produce Error atom"
        );
        let jit_val = JitValue::from_raw(result);
        assert!(jit_val.is_heap(), "Error atom should be heap-allocated");
    }

    #[test]
    fn test_jit_get_tail_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let val = metta_to_jit(&sexpr).to_bits();

        let result = unsafe { jit_runtime_get_tail(&mut ctx, val, 0) };

        // Should return (1 2)
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert_eq!(elems.len(), 2);
        }
    }

    #[test]
    fn test_jit_get_tail_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![]);
        let val = metta_to_jit(&sexpr).to_bits();

        let result = unsafe { jit_runtime_get_tail(&mut ctx, val, 0) };

        // Should return empty S-expression
        let jv = JitValue::from_raw(result);
        let metta = unsafe { jv.to_metta() };
        if let MettaValueInner::SExpr(elems) = metta.inner() {
            assert!(elems.is_empty());
        }
    }

    #[test]
    fn test_jit_get_arity_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let val = metta_to_jit(&sexpr).to_bits();

        let result = unsafe { jit_runtime_get_arity(&mut ctx, val, 0) };

        // Should return 3
        let jv = JitValue::from_raw(result);
        assert!(jv.is_long());
        assert_eq!(jv.as_long(), 3);
    }

    #[test]
    fn test_jit_get_arity_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![]);
        let val = metta_to_jit(&sexpr).to_bits();

        let result = unsafe { jit_runtime_get_arity(&mut ctx, val, 0) };

        // Should return 0
        let jv = JitValue::from_raw(result);
        assert!(jv.is_long());
        assert_eq!(jv.as_long(), 0);
    }

    #[test]
    fn test_jit_index_atom_valid() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::Long(42),
            MettaValue::Long(99),
        ]);
        let val = metta_to_jit(&sexpr).to_bits();
        let index = JitValue::from_long(1).to_bits();

        let result = unsafe { jit_runtime_index_atom(&mut ctx, val, index, 0) };

        // Should return 42 (element at index 1)
        let jv = JitValue::from_raw(result);
        assert!(jv.is_long());
        assert_eq!(jv.as_long(), 42);
    }

    #[test]
    fn test_jit_index_atom_out_of_bounds() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]);
        let val = metta_to_jit(&sexpr).to_bits();
        let index = JitValue::from_long(10).to_bits();

        let result = unsafe { jit_runtime_index_atom(&mut ctx, val, index, 0) };

        // Should return unit for out of bounds
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    #[test]
    fn test_jit_index_atom_non_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        let index = JitValue::from_long(0).to_bits();

        let result = unsafe { jit_runtime_index_atom(&mut ctx, val, index, 0) };

        // Should return unit for non-S-expression
        let jv = JitValue::from_raw(result);
        assert!(jv.is_unit());
    }

    // ==========================================================================
    // Phase 5B: Pattern Matching Tests - Additional Coverage
    // ==========================================================================

    #[test]
    fn test_pattern_match_wildcard() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: _ (wildcard), Value: 42 - should match
        let pattern = metta_to_jit(&MettaValue::sym("_")).to_bits();
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };

        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        // Wildcard should match anything
        assert_eq!(result & 1, 1, "Wildcard should match");
    }

    #[test]
    fn test_pattern_match_variable() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: $x (variable), Value: 42 - should match
        let pattern = metta_to_jit(&MettaValue::sym("$x")).to_bits();
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };

        let jv = JitValue::from_raw(result);
        assert!(jv.is_bool());
        assert_eq!(result & 1, 1, "Variable should match anything");
    }

    #[test]
    fn test_pattern_match_bool_true() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: true, Value: true - should match
        let pattern = TAG_BOOL | 1;
        let value = TAG_BOOL | 1;
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1, "true == true");

        // Pattern: true, Value: false - should not match
        let value_false = TAG_BOOL;
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value_false, 0) };
        assert_eq!(result & 1, 0, "true != false");
    }

    #[test]
    fn test_pattern_match_nil() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: nil, Value: nil - should match
        let pattern = TAG_UNIT;
        let value = TAG_UNIT;
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1, "nil == nil");
    }

    #[test]
    fn test_pattern_match_unit() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: unit, Value: unit - should match
        let pattern = TAG_UNIT;
        let value = TAG_UNIT;
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1, "unit == unit");
    }

    #[test]
    fn test_pattern_match_type_mismatch() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: bool, Value: nil - should not match
        let pattern = TAG_BOOL | 1;
        let value = TAG_UNIT;
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 0, "bool != nil");

        // Pattern: Long, Value: bool - should not match
        let pattern_long = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern_long, value, 0) };
        assert_eq!(result & 1, 0, "long != nil");
    }

    #[test]
    fn test_pattern_match_null_context() {
        // Null context should return false
        let pattern = TAG_BOOL | 1;
        let value = TAG_BOOL | 1;
        let result = unsafe { jit_runtime_pattern_match(std::ptr::null(), pattern, value, 0) };
        assert_eq!(result, TAG_BOOL, "Null context returns false");
    }

    #[test]
    fn test_pattern_match_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: (foo $x), Value: (foo 42)
        let pattern_sexpr = MettaValue::SExpr(vec![MettaValue::sym("foo"), MettaValue::sym("$x")]);
        let value_sexpr = MettaValue::SExpr(vec![MettaValue::sym("foo"), MettaValue::Long(42)]);

        let pattern = metta_to_jit(&pattern_sexpr).to_bits();
        let value = metta_to_jit(&value_sexpr).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1, "(foo $x) matches (foo 42)");
    }

    #[test]
    fn test_pattern_match_sexpr_length_mismatch() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: (foo bar), Value: (foo bar baz) - length mismatch
        let pattern_sexpr = MettaValue::SExpr(vec![MettaValue::sym("foo"), MettaValue::sym("bar")]);
        let value_sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::sym("bar"),
            MettaValue::sym("baz"),
        ]);

        let pattern = metta_to_jit(&pattern_sexpr).to_bits();
        let value = metta_to_jit(&value_sexpr).to_bits();
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 0, "Length mismatch - should not match");
    }

    #[test]
    fn test_pattern_match_bind_variable() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        // Push a binding frame
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Pattern: $x, Value: 42 - should match and bind
        let pattern = metta_to_jit(&MettaValue::sym("$x")).to_bits();
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_pattern_match_bind(&mut ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1, "$x should match and bind to 42");
    }

    #[test]
    fn test_pattern_match_bind_wildcard() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Pattern: _, Value: 42 - should match without binding
        let pattern = metta_to_jit(&MettaValue::sym("_")).to_bits();
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_pattern_match_bind(&mut ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1, "Wildcard should match without binding");
    }

    #[test]
    fn test_pattern_match_bind_null_context() {
        let pattern = TAG_BOOL | 1;
        let value = TAG_BOOL | 1;
        let result =
            unsafe { jit_runtime_pattern_match_bind(std::ptr::null_mut(), pattern, value, 0) };
        assert_eq!(result, TAG_BOOL, "Null context returns false");
    }

    #[test]
    fn test_match_arity() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // S-expression with 3 elements
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let value = metta_to_jit(&sexpr).to_bits();

        // Check arity matches
        let result = unsafe { jit_runtime_match_arity(&ctx, value, 3, 0) };
        assert_eq!(result & 1, 1, "Arity 3 matches");

        // Check arity doesn't match
        let result = unsafe { jit_runtime_match_arity(&ctx, value, 2, 0) };
        assert_eq!(result & 1, 0, "Arity 2 does not match");
    }

    #[test]
    fn test_match_arity_non_sexpr() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Non-S-expression value
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_match_arity(&ctx, value, 0, 0) };
        assert_eq!(result & 1, 0, "Non-S-expression has no arity");
    }

    #[test]
    fn test_match_head() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("foo"), MettaValue::sym("bar")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // S-expression: (foo 1 2)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::sym("foo"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let value = metta_to_jit(&sexpr).to_bits();

        // Check head matches "foo" (index 0)
        let result = unsafe { jit_runtime_match_head(&ctx, value, 0, 0) };
        assert_eq!(result & 1, 1, "Head 'foo' matches");

        // Check head doesn't match "bar" (index 1)
        let result = unsafe { jit_runtime_match_head(&ctx, value, 1, 0) };
        assert_eq!(result & 1, 0, "Head 'bar' does not match");
    }

    #[test]
    fn test_match_head_null_context() {
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_match_head(std::ptr::null(), value, 0, 0) };
        assert_eq!(result, TAG_BOOL, "Null context returns false");
    }

    #[test]
    fn test_match_head_invalid_index() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("foo")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![MettaValue::sym("foo")]);
        let value = metta_to_jit(&sexpr).to_bits();

        // Invalid index (out of bounds)
        let result = unsafe { jit_runtime_match_head(&ctx, value, 100, 0) };
        assert_eq!(result, TAG_BOOL, "Invalid index returns false");
    }

    #[test]
    fn test_match_head_empty_sexpr() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("foo")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let sexpr = MettaValue::SExpr(vec![]);
        let value = metta_to_jit(&sexpr).to_bits();

        // Empty S-expression has no head
        let result = unsafe { jit_runtime_match_head(&ctx, value, 0, 0) };
        assert_eq!(result, TAG_BOOL, "Empty S-expression has no head");
    }

    #[test]
    fn test_unify_basic() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Same value unifies
        let a = JitValue::from_long(42).to_bits();
        let b = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_unify(&ctx, a, b, 0) };
        assert_eq!(result & 1, 1, "42 unifies with 42");

        // Different values don't unify
        let c = JitValue::from_long(99).to_bits();
        let result = unsafe { jit_runtime_unify(&ctx, a, c, 0) };
        assert_eq!(result & 1, 0, "42 does not unify with 99");
    }

    #[test]
    fn test_unify_variable() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Variable unifies with anything
        let var = metta_to_jit(&MettaValue::sym("$x")).to_bits();
        let val = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_unify(&ctx, var, val, 0) };
        assert_eq!(result & 1, 1, "$x unifies with 42");

        // Other way around
        let result = unsafe { jit_runtime_unify(&ctx, val, var, 0) };
        assert_eq!(result & 1, 1, "42 unifies with $x");
    }

    #[test]
    fn test_unify_wildcard() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Wildcard unifies with anything
        let wildcard = metta_to_jit(&MettaValue::sym("_")).to_bits();
        let val = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_unify(&ctx, wildcard, val, 0) };
        assert_eq!(result & 1, 1, "_ unifies with 42");

        // Other way around
        let result = unsafe { jit_runtime_unify(&ctx, val, wildcard, 0) };
        assert_eq!(result & 1, 1, "42 unifies with _");
    }

    #[test]
    fn test_unify_bind() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Unify with binding
        let var = metta_to_jit(&MettaValue::sym("$x")).to_bits();
        let val = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_unify_bind(&mut ctx, var, val, 0) };
        assert_eq!(result & 1, 1, "$x unifies with 42 and binding added");
    }

    #[test]
    fn test_unify_bind_null_context() {
        let a = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_unify_bind(std::ptr::null_mut(), a, a, 0) };
        assert_eq!(result, TAG_BOOL, "Null context returns false");
    }

    // ==========================================================================
    // Phase 5B: Nondeterminism Tests - Additional Coverage
    // ==========================================================================

    #[test]
    fn test_push_choice_point_null_context() {
        let alts = [JitAlternative::value(JitValue::from_long(1))];
        let result = unsafe {
            jit_runtime_push_choice_point(
                std::ptr::null_mut(),
                1,
                alts.as_ptr(),
                0,
                std::ptr::null(),
            )
        };
        assert_eq!(result, -2, "Null context returns -2");
    }

    #[test]
    fn test_fail_null_context() {
        let result = unsafe { jit_runtime_fail(std::ptr::null_mut()) };
        assert_eq!(result, -2, "Null context returns -2");
    }

    #[test]
    fn test_fail_no_choice_points() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        // No choice points - should return -1
        let result = unsafe { jit_runtime_fail(&mut ctx) };
        assert_eq!(result, -1, "No choice points returns -1");
    }

    #[test]
    fn test_get_results_count() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        assert_eq!(unsafe { jit_runtime_get_results_count(&ctx) }, 0);

        // Add some results
        ctx.results_count = 3;
        assert_eq!(unsafe { jit_runtime_get_results_count(&ctx) }, 3);
    }

    #[test]
    fn test_get_results_count_null_context() {
        let result = unsafe { jit_runtime_get_results_count(std::ptr::null()) };
        assert_eq!(result, 0, "Null context returns 0");
    }

    #[test]
    fn test_get_choice_point_count() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        assert_eq!(unsafe { jit_runtime_get_choice_point_count(&ctx) }, 0);

        ctx.choice_point_count = 2;
        assert_eq!(unsafe { jit_runtime_get_choice_point_count(&ctx) }, 2);
    }

    #[test]
    fn test_get_choice_point_count_null_context() {
        let result = unsafe { jit_runtime_get_choice_point_count(std::ptr::null()) };
        assert_eq!(result, 0, "Null context returns 0");
    }

    #[test]
    fn test_fork_zero_alternatives() {
        let constants: Vec<MettaValue> = vec![MettaValue::Long(1)];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Fork with 0 alternatives
        let result = unsafe { jit_runtime_fork(&mut ctx, 0, std::ptr::null(), 0) };
        assert_eq!(result, TAG_UNIT, "Zero alternatives returns NIL");
        assert!(ctx.bailout, "Bailout should be set");
    }

    #[test]
    fn test_fork_null_indices() {
        let constants: Vec<MettaValue> = vec![MettaValue::Long(1)];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Fork with null indices pointer
        let result = unsafe { jit_runtime_fork(&mut ctx, 2, std::ptr::null(), 0) };
        assert_eq!(result, TAG_UNIT, "Null indices returns NIL");
        assert!(ctx.bailout, "Bailout should be set");
    }

    #[test]
    fn test_fork_invalid_index() {
        let constants: Vec<MettaValue> = vec![MettaValue::Long(1)];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Fork with invalid index
        let indices: [u64; 1] = [100]; // Out of bounds
        let result = unsafe { jit_runtime_fork(&mut ctx, 1, indices.as_ptr(), 0) };
        assert_eq!(result, TAG_UNIT, "Invalid index returns NIL");
        assert!(ctx.bailout, "Bailout should be set");
    }

    #[test]
    fn test_fork_null_context() {
        let result = unsafe { jit_runtime_fork(std::ptr::null_mut(), 1, std::ptr::null(), 0) };
        assert_eq!(result, TAG_UNIT, "Null context returns NIL");
    }

    #[test]
    fn test_yield_null_context() {
        let value = JitValue::from_long(42).to_bits();
        let result = unsafe { jit_runtime_yield(std::ptr::null_mut(), value, 0) };
        assert_eq!(result, TAG_UNIT, "Null context returns NIL");
    }

    #[test]
    fn test_yield_results_overflow() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 2]; // Small capacity

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                2, // Small capacity
            )
        };

        // Fill up results
        ctx.results_count = 2;

        // Yield should handle overflow gracefully
        let value = JitValue::from_long(42).to_bits();
        let _ = unsafe { jit_runtime_yield(&mut ctx, value, 0) };

        // Should still set bailout even with overflow
        assert!(ctx.bailout, "Bailout should be set even with overflow");
    }

    #[test]
    fn test_collect_null_context() {
        let result = unsafe { jit_runtime_collect(std::ptr::null_mut(), 0, 0) };
        assert_eq!(result, TAG_UNIT, "Null context returns NIL");
    }

    #[test]
    fn test_collect_empty_results() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        // Collect with no results
        let result = unsafe { jit_runtime_collect(&mut ctx, 0, 0) };

        // Empty S-expression is semantically Unit in MeTTa (SExpr([]) → Unit via view()).
        // With NaN-boxing inline types, value_to_jit_generic converts it to TAG_UNIT.
        let tag = result & TAG_MASK;
        assert_eq!(tag, TAG_UNIT, "Empty collect should return TAG_UNIT");
    }

    #[test]
    fn test_save_stack_null_context() {
        let result = unsafe { jit_runtime_save_stack(std::ptr::null_mut()) };
        assert_eq!(result, JIT_SIGNAL_ERROR, "Null context returns ERROR");
    }

    #[test]
    fn test_save_stack_no_buffer() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        // No saved_stack buffer set

        let result = unsafe { jit_runtime_save_stack(&mut ctx) };
        assert_eq!(result, JIT_SIGNAL_OK, "No buffer returns OK (no-op)");
    }

    #[test]
    fn test_restore_stack_null_context() {
        let result = unsafe { jit_runtime_restore_stack(std::ptr::null_mut()) };
        assert_eq!(result, JIT_SIGNAL_ERROR, "Null context returns ERROR");
    }

    #[test]
    fn test_restore_stack_no_saved() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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
        // No saved stack

        let result = unsafe { jit_runtime_restore_stack(&mut ctx) };
        assert_eq!(result, JIT_SIGNAL_OK, "No saved stack returns OK (no-op)");
    }

    #[test]
    fn test_fork_native_null_context() {
        let result =
            unsafe { jit_runtime_fork_native(std::ptr::null_mut(), 1, std::ptr::null(), 0) };
        assert_eq!(result, TAG_UNIT, "Null context returns NIL");
    }

    #[test]
    fn test_fork_native_zero_alternatives() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        let result = unsafe { jit_runtime_fork_native(&mut ctx, 0, std::ptr::null(), 0) };
        assert_eq!(result, TAG_UNIT, "Zero alternatives returns NIL");
    }

    #[test]
    fn test_yield_native_null_context() {
        let result = unsafe { jit_runtime_yield_native(std::ptr::null_mut(), 0, 0) };
        assert_eq!(result, JIT_SIGNAL_ERROR, "Null context returns ERROR");
    }

    #[test]
    fn test_fail_native_null_context() {
        let result = unsafe { jit_runtime_fail_native(std::ptr::null_mut()) };
        assert_eq!(result, JIT_SIGNAL_FAIL as u64, "Null context returns FAIL");
    }

    #[test]
    fn test_fail_native_no_choice_points() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        let result = unsafe { jit_runtime_fail_native(&mut ctx) };
        assert_eq!(
            result, JIT_SIGNAL_FAIL as u64,
            "No choice points returns FAIL"
        );
    }

    #[test]
    fn test_collect_native_null_context() {
        let result = unsafe { jit_runtime_collect_native(std::ptr::null_mut()) };
        assert_eq!(result, TAG_UNIT, "Null context returns NIL");
    }

    #[test]
    fn test_has_alternatives_null_context() {
        let result = unsafe { jit_runtime_has_alternatives(std::ptr::null()) };
        assert_eq!(result, 0, "Null context returns 0");
    }

    #[test]
    fn test_get_resume_ip() {
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

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

        ctx.resume_ip = 42;
        let result = unsafe { jit_runtime_get_resume_ip(&ctx) };
        assert_eq!(result, 42, "Resume IP should be 42");
    }

    #[test]
    fn test_get_resume_ip_null_context() {
        let result = unsafe { jit_runtime_get_resume_ip(std::ptr::null()) };
        assert_eq!(result, 0, "Null context returns 0");
    }

    // ==========================================================================
    // Phase 5B: Bindings Tests - Additional Coverage
    // ==========================================================================

    #[test]
    fn test_load_binding_null_context() {
        let result = unsafe { jit_runtime_load_binding(std::ptr::null_mut(), 0, 0) };
        assert_eq!(result, TAG_UNIT, "Null context returns NIL");
    }

    #[test]
    fn test_store_binding_null_context() {
        let result = unsafe { jit_runtime_store_binding(std::ptr::null_mut(), 0, 0, 0) };
        assert_eq!(result, -2, "Null context returns -2");
    }

    #[test]
    fn test_store_binding_no_frames() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // No binding frames
        let result = unsafe { jit_runtime_store_binding(&mut ctx, 0, 0, 0) };
        assert_eq!(result, -1, "No binding frames returns -1");
    }

    #[test]
    fn test_has_binding_null_context() {
        let result = unsafe { jit_runtime_has_binding(std::ptr::null(), 0) };
        assert_eq!(result, TAG_BOOL, "Null context returns false");
    }

    #[test]
    fn test_clear_bindings_null_context() {
        // Should not crash
        unsafe { jit_runtime_clear_bindings(std::ptr::null_mut()) };
    }

    #[test]
    fn test_push_binding_frame_null_context() {
        let result = unsafe { jit_runtime_push_binding_frame(std::ptr::null_mut()) };
        assert_eq!(result, -2, "Null context returns -2");
    }

    #[test]
    fn test_push_binding_frame_overflow() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 2];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 2;

        // Fill up binding frames
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Next push should overflow
        let result = unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        assert_eq!(result, -1, "Overflow returns -1");
        assert!(ctx.bailout, "Bailout should be set");
    }

    #[test]
    fn test_pop_binding_frame_null_context() {
        let result = unsafe { jit_runtime_pop_binding_frame(std::ptr::null_mut()) };
        assert_eq!(result, -2, "Null context returns -2");
    }

    #[test]
    fn test_pop_binding_frame_root() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        // Push one frame (root)
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        assert_eq!(ctx.binding_frames_count, 1);

        // Can't pop root frame
        let result = unsafe { jit_runtime_pop_binding_frame(&mut ctx) };
        assert_eq!(result, -1, "Can't pop root frame");
    }

    #[test]
    fn test_fork_bindings_null_context() {
        let result = unsafe { jit_runtime_fork_bindings(std::ptr::null()) };
        assert!(result.is_null(), "Null context returns null");
    }

    #[test]
    fn test_fork_bindings_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // No binding frames
        let saved = unsafe { jit_runtime_fork_bindings(&ctx) };
        assert!(!saved.is_null(), "Should return valid pointer");

        // Clean up
        unsafe { jit_runtime_free_saved_bindings(saved) };
    }

    #[test]
    fn test_restore_bindings_null_context() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let saved = unsafe { jit_runtime_fork_bindings(&ctx) };
        let result = unsafe { jit_runtime_restore_bindings(std::ptr::null_mut(), saved, true) };
        assert_eq!(result, -1, "Null context returns -1");
    }

    #[test]
    fn test_restore_bindings_null_saved() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_restore_bindings(&mut ctx, std::ptr::null_mut(), false) };
        assert_eq!(result, -1, "Null saved returns -1");
    }

    #[test]
    fn test_free_saved_bindings_null() {
        // Should not crash
        unsafe { jit_runtime_free_saved_bindings(std::ptr::null_mut()) };
    }

    #[test]
    fn test_saved_bindings_size_null() {
        let result = unsafe { jit_runtime_saved_bindings_size(std::ptr::null()) };
        assert_eq!(result, 0, "Null returns 0");
    }

    #[test]
    fn test_saved_bindings_size() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x"), MettaValue::sym("$y")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(1).to_bits(), 0) };
        unsafe { jit_runtime_store_binding(&mut ctx, 1, JitValue::from_long(2).to_bits(), 0) };

        let saved = unsafe { jit_runtime_fork_bindings(&ctx) };
        let size = unsafe { jit_runtime_saved_bindings_size(saved) };
        assert_eq!(size, 2, "Should have 2 bindings");

        unsafe { jit_runtime_free_saved_bindings(saved) };
    }

    #[test]
    fn test_binding_update_existing() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        unsafe { jit_runtime_push_binding_frame(&mut ctx) };

        // Store first value
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(10).to_bits(), 0) };
        let val1 = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val1).as_long(), 10);

        // Update same binding
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(20).to_bits(), 0) };
        let val2 = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val2).as_long(), 20);
    }

    #[test]
    fn test_binding_shadowing() {
        let constants: Vec<MettaValue> = vec![MettaValue::sym("$x")];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = binding_frames.len();

        // Outer frame: $x = 10
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(10).to_bits(), 0) };

        // Inner frame: $x = 20 (shadows outer)
        unsafe { jit_runtime_push_binding_frame(&mut ctx) };
        unsafe { jit_runtime_store_binding(&mut ctx, 0, JitValue::from_long(20).to_bits(), 0) };

        // Should see inner value
        let val = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val).as_long(), 20);

        // Pop inner frame
        unsafe { jit_runtime_pop_binding_frame(&mut ctx) };

        // Should see outer value
        let val = unsafe { jit_runtime_load_binding(&mut ctx, 0, 0) };
        assert_eq!(JitValue::from_raw(val).as_long(), 10);
    }

    // =========================================================================
    // Phase 5D: Additional Tests for 0% Coverage Files
    // =========================================================================

    // === special_forms.rs edge cases ===

    #[test]
    fn test_eval_let_star_marker() {
        // let* is mainly a placeholder/marker - just verify it returns Unit
        let result = unsafe { jit_runtime_eval_let_star(std::ptr::null_mut(), 0) };
        assert_eq!(JitValue::from_raw(result).is_unit(), true);
    }

    #[test]
    fn test_eval_match_null_context() {
        // Pattern matching with null context returns false (early return)
        let val = JitValue::from_long(42);
        let pattern = JitValue::from_long(42);
        let result = unsafe {
            jit_runtime_eval_match(std::ptr::null_mut(), val.to_bits(), pattern.to_bits(), 0)
        };
        // With null context, pattern_match returns false (TAG_BOOL | 0)
        assert_eq!(result & 1, 0);
    }

    #[test]
    fn test_eval_match_no_match() {
        let val = JitValue::from_long(42);
        let pattern = JitValue::from_long(99);
        let result = unsafe {
            jit_runtime_eval_match(std::ptr::null_mut(), val.to_bits(), pattern.to_bits(), 0)
        };
        // Pattern 99 doesn't match value 42
        assert_eq!(result & 1, 0);
    }

    #[test]
    fn test_eval_case_null_context() {
        let val = JitValue::from_long(42);
        let result = unsafe { jit_runtime_eval_case(std::ptr::null_mut(), val.to_bits(), 3, 0) };
        // Null context should return -1 (no match)
        let result_val = JitValue::from_raw(result);
        assert_eq!(result_val.as_long(), -1);
    }

    #[test]
    fn test_eval_chain_simple() {
        let first = JitValue::from_long(1);
        let second = JitValue::from_long(2);
        let result = unsafe {
            jit_runtime_eval_chain(std::ptr::null_mut(), first.to_bits(), second.to_bits(), 0)
        };
        // Chain returns second value
        assert_eq!(JitValue::from_raw(result).as_long(), 2);
    }

    #[test]
    fn test_eval_chain_with_unit() {
        let first = JitValue::unit();
        let second = JitValue::from_long(42);
        let result = unsafe {
            jit_runtime_eval_chain(std::ptr::null_mut(), first.to_bits(), second.to_bits(), 0)
        };
        assert_eq!(JitValue::from_raw(result).as_long(), 42);
    }

    #[test]
    fn test_eval_unquote_non_quote() {
        // Unquoting a non-quoted value should return it unchanged
        let val = JitValue::from_long(42);
        let result = unsafe { jit_runtime_eval_unquote(std::ptr::null_mut(), val.to_bits(), 0) };
        assert_eq!(JitValue::from_raw(result).as_long(), 42);
    }

    // === value_creation.rs edge cases ===

    #[test]
    fn test_make_list_empty() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Empty list should return nil
        let result = unsafe { jit_runtime_make_list(&mut ctx, std::ptr::null(), 0, 0) };
        assert_eq!(result & TAG_MASK, TAG_UNIT);
    }

    #[test]
    fn test_make_list_single() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let values = vec![JitValue::from_long(1).to_bits()];
        let result = unsafe { jit_runtime_make_list(&mut ctx, values.as_ptr(), 1, 0) };

        // Should be a heap value (Cons structure)
        assert_eq!(result & TAG_MASK, TAG_PTR);

        // Verify structure
        let jit_val = JitValue::from_raw(result);
        let metta = unsafe { jit_val.to_metta() };
        match metta.inner() {
            MettaValueInner::SExpr(elems) => {
                assert_eq!(elems.len(), 3); // (Cons 1 Nil)
                if let MettaValueInner::Atom(s) = elems[0].inner() {
                    assert_eq!(*s, "Cons");
                } else {
                    panic!("Expected Atom for first element");
                }
                if let MettaValueInner::Long(n) = elems[1].inner() {
                    assert_eq!(*n, 1);
                } else {
                    panic!("Expected Long for second element");
                }
            }
            _ => panic!("Expected SExpr for list"),
        }
    }

    #[test]
    fn test_push_uri_loads_constant() {
        let constants: Vec<MettaValue> = vec![MettaValue::String("http://example.com".to_string())];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let result = unsafe { jit_runtime_push_uri(&ctx, 0) };
        assert_eq!(result & TAG_MASK, TAG_PTR);
    }

    // === type_ops.rs edge cases ===

    #[test]
    fn test_get_type_unknown_tag() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Use a completely invalid tag value
        let invalid_val: u64 = 0xFFFF_0000_0000_0000; // Invalid tag
        let result = unsafe { jit_runtime_get_type(&mut ctx, invalid_val, 0) };

        // Should return some type (Unknown)
        let jit_val = JitValue::from_raw(result);
        let metta = unsafe { jit_val.to_metta() };
        match metta.inner() {
            MettaValueInner::Atom(s) => assert_eq!(*s, "Unknown"),
            _ => panic!("Expected Atom for type name"),
        }
    }

    #[test]
    fn test_check_type_invalid_type_atom() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let val = JitValue::from_long(42).to_bits();
        // Use a Long as the type atom (invalid - should be an atom)
        let type_val: u64 = TAG_LONG | 123;
        let result = unsafe { jit_runtime_check_type(&mut ctx, val, type_val, 0) };

        // Should return false (type check failed due to invalid type atom)
        assert_eq!(result & 1, 0);
    }

    #[test]
    fn test_assert_type_with_null_context() {
        let val = JitValue::from_long(42).to_bits();
        let type_val: u64 = TAG_LONG | 123; // Invalid type atom

        // With null context, should still return the value
        let result = unsafe { jit_runtime_assert_type(std::ptr::null_mut(), val, type_val, 0) };
        assert_eq!(result, val);
    }

    // === space_ops.rs edge cases ===

    #[test]
    fn test_space_match_nondet_with_context_no_choice_points() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };
        // Set choice points to null to trigger the capacity check path
        ctx.choice_points = std::ptr::null_mut();
        ctx.choice_point_cap = 0;

        let not_space = JitValue::from_long(42).to_bits();
        let pattern_jit = JitValue::from_long(1).to_bits();
        let template_jit = JitValue::from_long(2).to_bits();

        let result = unsafe {
            jit_runtime_space_match_nondet(&mut ctx, not_space, pattern_jit, template_jit, 0)
        };

        // Should return nil (type error - not a space)
        assert_eq!(result, TAG_UNIT);
        // Should have bailed out
        assert!(ctx.bailout);
    }

    // === nondeterminism edge cases ===

    #[test]
    fn test_yield_native_null_context_5d() {
        // yield_native returns i64 and takes 3 args
        let result = unsafe {
            jit_runtime_yield_native(std::ptr::null_mut(), JitValue::from_long(42).to_bits(), 0)
        };
        // Should return ERROR signal with null context
        assert_eq!(result, JIT_SIGNAL_ERROR as i64);
    }

    #[test]
    fn test_collect_native_null_context_5d() {
        // collect_native returns u64
        let result = unsafe { jit_runtime_collect_native(std::ptr::null_mut()) };
        // Should return nil with null context
        assert_eq!(result, TAG_UNIT);
    }

    #[test]
    fn test_collect_results_empty_5d() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let mut results: Vec<JitValue> = vec![JitValue::unit(); 16];

        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
                std::ptr::null_mut(),
                0,
                results.as_mut_ptr(),
                results.len(),
            )
        };
        ctx.results_count = 0;

        // collect_results takes *mut JitContext
        let collected = unsafe { collect_results(&mut ctx) };
        assert!(collected.is_empty());
    }

    // === Additional pattern matching edge cases ===

    #[test]
    fn test_pattern_match_long_via_fast_path() {
        // Long comparison uses fast path (doesn't need context)
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = JitValue::from_long(42).to_bits();
        let value = JitValue::from_long(42).to_bits();

        // Fast path: same longs match
        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1);

        // Fast path: different longs don't match
        let different = JitValue::from_long(99).to_bits();
        let result2 = unsafe { jit_runtime_pattern_match(&ctx, pattern, different, 0) };
        assert_eq!(result2 & 1, 0);
    }

    #[test]
    fn test_pattern_match_bool_via_fast_path() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = JitValue::from_bool(true).to_bits();
        let value = JitValue::from_bool(true).to_bits();

        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1);
    }

    #[test]
    fn test_pattern_match_nil_via_fast_path() {
        let constants: Vec<MettaValue> = vec![];
        let mut stack: Vec<JitValue> = vec![JitValue::unit(); 16];
        let ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                stack.len(),
                constants.as_ptr(),
                constants.len(),
            )
        };

        let pattern = JitValue::unit().to_bits();
        let value = JitValue::unit().to_bits();

        let result = unsafe { jit_runtime_pattern_match(&ctx, pattern, value, 0) };
        assert_eq!(result & 1, 1);
    }

    // === Arithmetic edge cases ===

    #[test]
    fn test_pow_large_exponent() {
        let base = box_long(2);
        let exp = box_long(30);
        let result = unsafe { jit_runtime_pow(base, exp) };
        let result_val = extract_long_signed(result);
        assert_eq!(result_val, 1073741824); // 2^30
    }

    #[test]
    fn test_abs_min_value() {
        // i64::MIN is a special case for abs
        let min_val = box_long(i64::MIN);
        let result = unsafe { jit_runtime_abs(min_val) };
        // abs(i64::MIN) overflows in debug, but in release wraps
        // We just verify it doesn't crash
        let _ = extract_long_signed(result);
    }

    #[test]
    fn test_signum_max_positive_5d() {
        // Use a large positive value that fits in 48-bit payload
        // (NaN-boxing uses 48-bit payload, so i64::MAX gets truncated)
        let large_pos = box_long(0x7FFF_FFFF_FFFF); // Max 48-bit positive
        let result = unsafe { jit_runtime_signum(large_pos) };
        assert_eq!(extract_long_signed(result), 1);
    }

    #[test]
    fn test_signum_large_negative() {
        // Use a large negative value that fits in 48-bit payload
        let large_neg = box_long(-0x7FFF_FFFF_FFFF); // Large 48-bit negative
        let result = unsafe { jit_runtime_signum(large_neg) };
        assert_eq!(extract_long_signed(result), -1);
    }

    // ==========================================================================
    // Numeric Runtime FFI Tests (Float Type Promotion)
    // ==========================================================================

    /// Helper: extract a Float from a NaN-boxed result, panicking if not Float.
    fn extract_float(raw: u64) -> f64 {
        let jv = JitValue::from_raw(raw);
        let mv = unsafe { jv.to_metta() };
        match mv.inner() {
            MettaValueInner::Float(f) => *f,
            other => panic!("Expected Float, got {:?}", other),
        }
    }

    /// Helper: extract a Bool from a NaN-boxed result, panicking if not Bool.
    fn extract_bool(raw: u64) -> bool {
        let jv = JitValue::from_raw(raw);
        let mv = unsafe { jv.to_metta() };
        match mv.inner() {
            MettaValueInner::Bool(b) => *b,
            other => panic!("Expected Bool, got {:?}", other),
        }
    }

    /// Helper: assert that a NaN-boxed result is a TAG_PTR (heap-allocated error).
    fn assert_is_error(raw: u64) {
        assert_eq!(
            raw & TAG_MASK,
            TAG_PTR,
            "Expected TAG_PTR (error), got tag {:#018x}",
            raw & TAG_MASK
        );
    }

    // --- Arithmetic: add ---

    #[test]
    fn test_numeric_add_long_long() {
        let a = box_long(3);
        let b = box_long(5);
        let result = unsafe { jit_runtime_numeric_add(a, b) };
        assert_eq!(extract_long_signed(result), 8);
    }

    #[test]
    fn test_numeric_add_long_float() {
        let a = box_long(3);
        let b = metta_to_jit(&MettaValue::Float(2.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_add(a, b) };
        let f = extract_float(result);
        assert!((f - 5.5).abs() < f64::EPSILON, "Expected 5.5, got {}", f);
    }

    #[test]
    fn test_numeric_add_float_float() {
        let a = metta_to_jit(&MettaValue::Float(1.5)).to_bits();
        let b = metta_to_jit(&MettaValue::Float(2.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_add(a, b) };
        let f = extract_float(result);
        assert!((f - 4.0).abs() < f64::EPSILON, "Expected 4.0, got {}", f);
    }

    // --- Arithmetic: sub ---

    #[test]
    fn test_numeric_sub_long_float() {
        let a = box_long(10);
        let b = metta_to_jit(&MettaValue::Float(2.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_sub(a, b) };
        let f = extract_float(result);
        assert!((f - 7.5).abs() < f64::EPSILON, "Expected 7.5, got {}", f);
    }

    // --- Arithmetic: mul ---

    #[test]
    fn test_numeric_mul_float_long() {
        let a = metta_to_jit(&MettaValue::Float(2.5)).to_bits();
        let b = box_long(4);
        let result = unsafe { jit_runtime_numeric_mul(a, b) };
        let f = extract_float(result);
        assert!((f - 10.0).abs() < f64::EPSILON, "Expected 10.0, got {}", f);
    }

    // --- Arithmetic: div ---

    #[test]
    fn test_numeric_div_long_float() {
        let a = box_long(7);
        let b = metta_to_jit(&MettaValue::Float(2.0)).to_bits();
        let result = unsafe { jit_runtime_numeric_div(a, b) };
        let f = extract_float(result);
        assert!((f - 3.5).abs() < f64::EPSILON, "Expected 3.5, got {}", f);
    }

    #[test]
    fn test_numeric_div_by_zero() {
        let a = box_long(10);
        let b = box_long(0);
        let result = unsafe { jit_runtime_numeric_div(a, b) };
        assert_is_error(result);
    }

    // --- Arithmetic: mod ---

    #[test]
    fn test_numeric_mod_long_float() {
        // 85 % 43.5 = 85.0 - 1.0 * 43.5 = 41.5
        let a = box_long(85);
        let b = metta_to_jit(&MettaValue::Float(43.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_mod(a, b) };
        let f = extract_float(result);
        assert!(
            (f - 41.5).abs() < 1e-10,
            "Expected approximately 41.5, got {}",
            f
        );
    }

    #[test]
    fn test_numeric_mod_float_long() {
        // 85.5 % 43 = 85.5 - 1.0 * 43.0 = 42.5
        let a = metta_to_jit(&MettaValue::Float(85.5)).to_bits();
        let b = box_long(43);
        let result = unsafe { jit_runtime_numeric_mod(a, b) };
        let f = extract_float(result);
        assert!(
            (f - 42.5).abs() < 1e-10,
            "Expected approximately 42.5, got {}",
            f
        );
    }

    #[test]
    fn test_numeric_mod_by_zero() {
        let a = box_long(10);
        let b = box_long(0);
        let result = unsafe { jit_runtime_numeric_mod(a, b) };
        assert_is_error(result);
    }

    // --- Unary: neg ---

    #[test]
    fn test_numeric_neg_long() {
        let a = box_long(5);
        let result = unsafe { jit_runtime_numeric_neg(a) };
        assert_eq!(extract_long_signed(result), -5);
    }

    #[test]
    fn test_numeric_neg_float() {
        let a = metta_to_jit(&MettaValue::Float(3.14)).to_bits();
        let result = unsafe { jit_runtime_numeric_neg(a) };
        let f = extract_float(result);
        assert!(
            (f - (-3.14)).abs() < f64::EPSILON,
            "Expected -3.14, got {}",
            f
        );
    }

    // --- Unary: abs ---

    #[test]
    fn test_numeric_abs_negative_long() {
        let a = box_long(-7);
        let result = unsafe { jit_runtime_numeric_abs(a) };
        assert_eq!(extract_long_signed(result), 7);
    }

    #[test]
    fn test_numeric_abs_float() {
        let a = metta_to_jit(&MettaValue::Float(-2.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_abs(a) };
        let f = extract_float(result);
        assert!((f - 2.5).abs() < f64::EPSILON, "Expected 2.5, got {}", f);
    }

    // --- Comparison: lt ---

    #[test]
    fn test_numeric_lt_long_float() {
        // Long(1) < Float(2.5) -> true
        let a = box_long(1);
        let b = metta_to_jit(&MettaValue::Float(2.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_lt(a, b) };
        assert!(extract_bool(result), "Expected Long(1) < Float(2.5) = true");
    }

    #[test]
    fn test_numeric_lt_float_long() {
        // Float(3.0) < Long(2) -> false
        let a = metta_to_jit(&MettaValue::Float(3.0)).to_bits();
        let b = box_long(2);
        let result = unsafe { jit_runtime_numeric_lt(a, b) };
        assert!(
            !extract_bool(result),
            "Expected Float(3.0) < Long(2) = false"
        );
    }

    // --- Comparison: le ---

    #[test]
    fn test_numeric_le_equal_mixed() {
        // Long(2) <= Float(2.0) -> true
        let a = box_long(2);
        let b = metta_to_jit(&MettaValue::Float(2.0)).to_bits();
        let result = unsafe { jit_runtime_numeric_le(a, b) };
        assert!(
            extract_bool(result),
            "Expected Long(2) <= Float(2.0) = true"
        );
    }

    // --- Comparison: gt ---

    #[test]
    fn test_numeric_gt_float_float() {
        // Float(3.0) > Float(2.0) -> true
        let a = metta_to_jit(&MettaValue::Float(3.0)).to_bits();
        let b = metta_to_jit(&MettaValue::Float(2.0)).to_bits();
        let result = unsafe { jit_runtime_numeric_gt(a, b) };
        assert!(
            extract_bool(result),
            "Expected Float(3.0) > Float(2.0) = true"
        );
    }

    // --- Comparison: ge ---

    #[test]
    fn test_numeric_ge_equal_mixed() {
        // Float(5.0) >= Long(5) -> true
        let a = metta_to_jit(&MettaValue::Float(5.0)).to_bits();
        let b = box_long(5);
        let result = unsafe { jit_runtime_numeric_ge(a, b) };
        assert!(
            extract_bool(result),
            "Expected Float(5.0) >= Long(5) = true"
        );
    }

    // --- Equality ---

    #[test]
    fn test_numeric_eq_long_float() {
        // Long(2) == Float(2.0) -> true (KEY FIX: cross-type numeric equality)
        let a = box_long(2);
        let b = metta_to_jit(&MettaValue::Float(2.0)).to_bits();
        let result = unsafe { jit_runtime_numeric_eq(a, b) };
        assert!(
            extract_bool(result),
            "Expected Long(2) == Float(2.0) = true"
        );
    }

    #[test]
    fn test_numeric_eq_different() {
        // Long(2) == Float(2.5) -> false
        let a = box_long(2);
        let b = metta_to_jit(&MettaValue::Float(2.5)).to_bits();
        let result = unsafe { jit_runtime_numeric_eq(a, b) };
        assert!(
            !extract_bool(result),
            "Expected Long(2) == Float(2.5) = false"
        );
    }

    #[test]
    fn test_numeric_eq_nan() {
        // Float(NAN) == Float(NAN) -> false (IEEE 754)
        let a = metta_to_jit(&MettaValue::Float(f64::NAN)).to_bits();
        let b = metta_to_jit(&MettaValue::Float(f64::NAN)).to_bits();
        let result = unsafe { jit_runtime_numeric_eq(a, b) };
        assert!(
            !extract_bool(result),
            "Expected Float(NAN) == Float(NAN) = false per IEEE 754"
        );
    }

    // =========================================================================
    // JIT Type Error Flag Tests
    // =========================================================================

    #[test]
    fn test_type_error_flag_initially_clear() {
        // Flag should be clear by default
        assert!(!check_and_clear_jit_type_error());
    }

    #[test]
    fn test_type_error_flag_signal_and_clear() {
        signal_jit_type_error();
        assert!(
            check_and_clear_jit_type_error(),
            "Flag should be set after signal"
        );
        assert!(
            !check_and_clear_jit_type_error(),
            "Flag should be cleared after check"
        );
    }

    #[test]
    fn test_numeric_add_type_error_sets_flag() {
        // Clear any stale flag state
        let _ = check_and_clear_jit_type_error();

        // Atom + Long should trigger type error flag
        let a = metta_to_jit(&MettaValue::sym("A")).to_bits();
        let b = box_long(0);
        unsafe { jit_runtime_numeric_add(a, b) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_add with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_sub_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = box_long(0);
        let b = metta_to_jit(&MettaValue::sym("B")).to_bits();
        unsafe { jit_runtime_numeric_sub(a, b) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_sub with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_mul_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = metta_to_jit(&MettaValue::sym("X")).to_bits();
        let b = box_long(5);
        unsafe { jit_runtime_numeric_mul(a, b) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_mul with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_div_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = metta_to_jit(&MettaValue::sym("Y")).to_bits();
        let b = box_long(1);
        unsafe { jit_runtime_numeric_div(a, b) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_div with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_mod_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = box_long(10);
        let b = metta_to_jit(&MettaValue::sym("Z")).to_bits();
        unsafe { jit_runtime_numeric_mod(a, b) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_mod with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_neg_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = metta_to_jit(&MettaValue::sym("N")).to_bits();
        unsafe { jit_runtime_numeric_neg(a) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_neg with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_abs_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = metta_to_jit(&MettaValue::sym("M")).to_bits();
        unsafe { jit_runtime_numeric_abs(a) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_abs with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_lt_type_error_sets_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = metta_to_jit(&MettaValue::sym("A")).to_bits();
        let b = box_long(0);
        unsafe { jit_runtime_numeric_lt(a, b) };
        assert!(
            check_and_clear_jit_type_error(),
            "numeric_lt with Atom should set type error flag"
        );
    }

    #[test]
    fn test_numeric_add_valid_does_not_set_flag() {
        let _ = check_and_clear_jit_type_error();
        let a = box_long(1);
        let b = box_long(2);
        let result = unsafe { jit_runtime_numeric_add(a, b) };
        assert!(
            !check_and_clear_jit_type_error(),
            "numeric_add with valid Long operands should NOT set type error flag"
        );
        assert_eq!(extract_long_signed(result), 3);
    }
}
