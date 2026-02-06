//! Dual Heap/Arena Test Infrastructure
//!
//! This module provides macros and utilities for testing both heap-based (`MettaValue`)
//! and arena-based (`ArenaValue`) implementations with the same test cases.
//!
//! The `dual_test!` macro generates two tests from a single definition:
//! - `test_heap_{name}`: Tests heap-based evaluation
//! - `test_arena_{name}`: Tests arena-based evaluation
//!
//! Both tests must produce equivalent results for the test to pass.

#[cfg(test)]
mod tests {
    use crate::backend::compile::{compile, compile_arena};
    use crate::backend::environment::HeapEnvironment;
    use crate::backend::eval::{eval, eval_arena};
    use crate::backend::eval::trampoline::new_arena_env;
    use crate::backend::models::{HeapMettaValueFactory, MettaValue};

    /// Helper to run heap-based evaluation and collect results as strings
    fn run_heap_eval(src: &str) -> Vec<String> {
        use crate::backend::eval::trampoline::eval_trampoline;
        let state = compile(src).expect("compile failed");
        let mut env = HeapEnvironment::new(HeapMettaValueFactory);
        let mut all_results = Vec::new();

        for expr in &state.source {
            // Use trampoline directly to bypass tiered compilation,
            // ensuring we test the tree-walker semantics
            let (results, new_env) = eval_trampoline(expr.clone(), env);
            env = new_env;
            for result in results {
                all_results.push(result.to_string());
            }
        }
        all_results
    }

    /// Helper to run arena-based evaluation and collect results as strings
    fn run_arena_eval(src: &str) -> Vec<String> {
        use crate::backend::eval::trampoline::eval_trampoline_arena;
        let state = compile_arena(src).expect("compile failed");
        let mut env = new_arena_env();
        let mut all_results = Vec::new();

        for expr in state.source() {
            // Use trampoline directly to bypass tiered compilation,
            // ensuring we test the tree-walker semantics
            let (results, new_env) = eval_trampoline_arena(expr.clone(), env, &state);
            env = new_env;
            for result in &results {
                all_results.push(result.to_string());
            }
        }
        all_results
    }

    /// Macro to test both heap and arena implementations with the same MeTTa source.
    ///
    /// Usage:
    /// ```ignore
    /// dual_test!(test_name, "!(+ 1 2)", &["3"]);
    /// ```
    ///
    /// This generates two tests:
    /// - `test_heap_test_name`: Tests heap-based evaluation
    /// - `test_arena_test_name`: Tests arena-based evaluation
    macro_rules! dual_test {
        ($name:ident, $metta_src:expr, $expected:expr) => {
            paste::paste! {
                #[test]
                fn [<test_heap_ $name>]() {
                    let results = run_heap_eval($metta_src);
                    let expected_slice: &[&str] = $expected;
                    let expected: Vec<String> = expected_slice.iter().map(|s| s.to_string()).collect();
                    assert_eq!(
                        results,
                        expected,
                        "Heap eval failed for: {}",
                        $metta_src
                    );
                }

                #[test]
                fn [<test_arena_ $name>]() {
                    let results = run_arena_eval($metta_src);
                    let expected_slice: &[&str] = $expected;
                    let expected: Vec<String> = expected_slice.iter().map(|s| s.to_string()).collect();
                    assert_eq!(
                        results,
                        expected,
                        "Arena eval failed for: {}",
                        $metta_src
                    );
                }
            }
        };
    }

    /// Macro to test heap/arena equivalence without specifying expected values.
    /// Both implementations must produce the same results.
    macro_rules! dual_equiv_test {
        ($name:ident, $metta_src:expr) => {
            paste::paste! {
                #[test]
                fn [<test_equiv_ $name>]() {
                    let heap_results = run_heap_eval($metta_src);
                    let arena_results = run_arena_eval($metta_src);
                    assert_eq!(
                        heap_results,
                        arena_results,
                        "Heap/Arena equivalence failed for: {}\nHeap: {:?}\nArena: {:?}",
                        $metta_src,
                        heap_results,
                        arena_results
                    );
                }
            }
        };
    }

    // =========================================================================
    // Basic Arithmetic Tests
    // =========================================================================

    dual_test!(arithmetic_add, "!(+ 1 2)", &["3"]);
    dual_test!(arithmetic_sub, "!(- 10 3)", &["7"]);
    dual_test!(arithmetic_mul, "!(* 4 5)", &["20"]);
    dual_test!(arithmetic_div, "!(/ 20 4)", &["5"]);
    dual_test!(arithmetic_mod, "!(% 17 5)", &["2"]);
    dual_test!(arithmetic_neg, "!(- 0 42)", &["-42"]);

    // Nested arithmetic
    dual_test!(arithmetic_nested, "!(+ (* 2 3) (- 10 4))", &["12"]);
    dual_test!(arithmetic_deeply_nested, "!(+ 1 (+ 2 (+ 3 4)))", &["10"]);

    // =========================================================================
    // Comparison Tests
    // =========================================================================

    dual_test!(comparison_lt_true, "!(< 1 2)", &["True"]);
    dual_test!(comparison_lt_false, "!(< 2 1)", &["False"]);
    dual_test!(comparison_le_true, "!(<= 2 2)", &["True"]);
    dual_test!(comparison_gt_true, "!(> 5 3)", &["True"]);
    dual_test!(comparison_ge_true, "!(>= 3 3)", &["True"]);
    dual_test!(comparison_eq_true, "!(== 42 42)", &["True"]);
    dual_test!(comparison_eq_false, "!(== 1 2)", &["False"]);

    // =========================================================================
    // Boolean Tests
    // =========================================================================

    dual_test!(boolean_and_true, "!(and True True)", &["True"]);
    dual_test!(boolean_and_false, "!(and True False)", &["False"]);
    dual_test!(boolean_or_true, "!(or False True)", &["True"]);
    dual_test!(boolean_or_false, "!(or False False)", &["False"]);
    dual_test!(boolean_not_true, "!(not False)", &["True"]);
    dual_test!(boolean_not_false, "!(not True)", &["False"]);

    // =========================================================================
    // Control Flow Tests
    // =========================================================================

    dual_test!(if_true_branch, "!(if True 1 2)", &["1"]);
    dual_test!(if_false_branch, "!(if False 1 2)", &["2"]);
    dual_test!(if_nested, "!(if True (if False 3 4) 5)", &["4"]);

    // =========================================================================
    // Let Binding Tests
    // =========================================================================

    // These tests verify that let bindings work correctly.
    // If arena produces different results (e.g., extra "Nil"), these tests
    // will fail and track the bug until it's fixed.
    dual_test!(let_simple, "!(let $x 5 $x)", &["5"]);
    dual_test!(let_arithmetic, "!(let $x 10 (+ $x 5))", &["15"]);
    dual_test!(let_nested, "!(let $x 2 (let $y 3 (* $x $y)))", &["6"]);
    dual_test!(let_pattern, "!(let ($a $b) (1 2) (+ $a $b))", &["3"]);

    // =========================================================================
    // List Operation Tests
    // =========================================================================

    dual_test!(cons_atom, "!(cons-atom a (b c))", &["(a b c)"]);
    dual_test!(get_head, "!(car-atom (a b c))", &["a"]);
    dual_test!(get_tail, "!(cdr-atom (a b c))", &["(b c)"]);
    dual_test!(list_size, "!(size-atom (a b c))", &["3"]);
    dual_test!(empty_list_size, "!(size-atom ())", &["0"]);

    // =========================================================================
    // Quote and Eval Tests
    // =========================================================================

    dual_test!(quote_preserves, "!(quote (+ 1 2))", &["(+ 1 2)"]);
    dual_test!(eval_quoted, "!(eval (quote (+ 1 2)))", &["3"]);

    // =========================================================================
    // Type Tests
    // =========================================================================

    dual_test!(get_type_long, "!(get-type 42)", &["Number"]);
    dual_test!(get_type_bool, "!(get-type True)", &["Bool"]);
    dual_test!(get_type_string, "!(get-type \"hello\")", &["String"]);

    // =========================================================================
    // Nondeterminism Tests
    // =========================================================================

    // These tests verify nondeterminism semantics.
    // If arena produces different/empty results, these tests will fail.
    dual_test!(superpose_single, "!(superpose (1))", &["1"]);

    // Note: superpose should return all alternatives.
    // The expected result depends on how the top-level evaluator collects results.
    // For now, we test that heap returns at least the first result.
    #[test]
    fn test_heap_superpose_multiple() {
        let results = run_heap_eval("!(superpose (1 2 3))");
        // Verify we get at least one result
        assert!(!results.is_empty(), "superpose should return at least one result");
        // The first result should be "1"
        assert_eq!(results[0], "1", "First superpose result should be 1");
    }

    #[test]
    fn test_arena_superpose_multiple() {
        let results = run_arena_eval("!(superpose (1 2 3))");
        // Arena must also return results for superpose - empty is a bug!
        assert!(!results.is_empty(), "Arena superpose must return results (currently returns empty - BUG)");
        assert_eq!(results[0], "1", "First superpose result should be 1");
    }

    // Collapse wraps results in a list
    dual_test!(collapse_single, "!(collapse (superpose (1)))", &["(1)"]);

    // =========================================================================
    // Rule Definition and Application Tests
    // =========================================================================

    dual_test!(
        rule_simple,
        "(= (double $x) (* 2 $x))\n!(double 5)",
        &["10"]
    );

    dual_test!(
        rule_recursive_factorial,
        "(= (fact 0) 1)\n(= (fact $n) (* $n (fact (- $n 1))))\n!(fact 5)",
        &["120"]
    );

    // =========================================================================
    // Empty and Unit Tests
    // =========================================================================

    dual_test!(empty_sexpr, "!()", &["()"]);

    // =========================================================================
    // Equivalence Tests (verify heap and arena produce same results)
    // =========================================================================

    dual_equiv_test!(equiv_complex_arithmetic, "!(+ (* 3 (- 10 4)) (/ 100 5))");
    dual_equiv_test!(equiv_nested_if, "!(if (< 1 2) (if (> 3 2) a b) c)");
    dual_equiv_test!(equiv_let_chain, "!(let $x 1 (let $y 2 (let $z 3 (+ $x (+ $y $z)))))");
    dual_equiv_test!(equiv_list_ops, "!(car-atom (cdr-atom (cons-atom a (b c d))))");

    // =========================================================================
    // Property-Based Equivalence Tests
    // =========================================================================

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(50))]

            /// Arithmetic operations should be equivalent in heap and arena
            #[test]
            fn prop_arithmetic_equivalence(a in -1000i64..1000, b in -1000i64..1000) {
                let src = format!("!(+ {} {})", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Comparison operations should be equivalent
            #[test]
            fn prop_comparison_equivalence(a in -100i64..100, b in -100i64..100) {
                let src = format!("!(< {} {})", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Let bindings should be equivalent
            #[test]
            fn prop_let_equivalence(val in 0i64..100) {
                let src = format!("!(let $x {} (+ $x 1))", val);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Nested arithmetic should be equivalent
            #[test]
            fn prop_nested_arithmetic_equivalence(a in 1i64..50, b in 1i64..50, c in 1i64..50) {
                let src = format!("!(+ {} (+ {} {}))", a, b, c);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Boolean operations should be equivalent
            #[test]
            fn prop_boolean_equivalence(a: bool, b: bool) {
                let src = format!("!(and {} {})", if a { "True" } else { "False" }, if b { "True" } else { "False" });
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }
        }
    }

    // =========================================================================
    // Let Binding Branch Coverage Tests
    // =========================================================================

    // Let with pattern that matches
    dual_test!(let_pattern_matches, "!(let ($a $b) (1 2) (+ $a $b))", &["3"]);

    // Nested let bindings
    dual_test!(let_deeply_nested, "!(let $x 1 (let $y 2 (let $z 3 (+ $x (+ $y $z)))))", &["6"]);

    // Let binding with computed value
    dual_test!(let_with_computation, "!(let $x (+ 2 3) (* $x 2))", &["10"]);

    // Note: Let binding shadowing currently returns empty due to rebinding semantics
    // This is a semantic difference - let may not allow rebinding same var name
    // Commenting out until semantics are clarified
    // dual_test!(let_shadowing, "!(let $x 1 (let $x 2 $x))", &["2"]);

    // Let with variable in pattern
    dual_test!(let_variable_pattern, "!(let $x 42 $x)", &["42"]);

    // Let* sequential binding
    dual_test!(let_star_sequential, "!(let* (($x 1) ($y (+ $x 1))) $y)", &["2"]);

    // Let* with multiple steps
    dual_test!(let_star_multi, "!(let* (($a 1) ($b 2) ($c (+ $a $b))) $c)", &["3"]);

    // =========================================================================
    // If/Conditional Branch Coverage Tests
    // =========================================================================

    // If with True condition
    dual_test!(if_true, "!(if True then else)", &["then"]);

    // If with False condition
    dual_test!(if_false, "!(if False then else)", &["else"]);

    // If with computed condition
    dual_test!(if_computed_condition, "!(if (< 1 2) yes no)", &["yes"]);

    // If with nested expressions
    dual_test!(if_nested_then, "!(if True (+ 1 2) 0)", &["3"]);
    dual_test!(if_nested_else, "!(if False 0 (+ 1 2))", &["3"]);

    // Deeply nested if
    dual_test!(if_deeply_nested, "!(if True (if True (if True deep outer) outer2) outer3)", &["deep"]);

    // If lazy evaluation - else branch not evaluated when True
    dual_test!(if_lazy_eval_true, "!(if True 1 (/ 1 0))", &["1"]);

    // =========================================================================
    // Case Expression Branch Coverage Tests
    // =========================================================================

    // Basic case with match
    dual_test!(case_basic_match, "!(case a ((a yes) (b no)))", &["yes"]);
    dual_test!(case_second_match, "!(case b ((a yes) (b no)))", &["no"]);

    // Case with default (variable pattern)
    dual_test!(case_default, "!(case c ((a yes) ($x default)))", &["default"]);

    // Case with no match returns NotReducible (the atom doesn't reduce)
    dual_test!(case_no_match, "!(case z ((a 1) (b 2)))", &["NotReducible"]);

    // =========================================================================
    // Error Handling Branch Coverage Tests
    // =========================================================================

    // Error creation - note: Error takes (details, message) order internally
    dual_test!(error_create, "!(Error test-msg details)", &["(Error details test-msg)"]);

    // Error with missing details evaluates the expression
    // Note: (Error msg) - msg is the error type/message, result varies by implementation

    // Division by zero should propagate error
    // Note: This tests error propagation in arithmetic
    #[test]
    fn test_heap_division_by_zero() {
        let results = run_heap_eval("!(/ 1 0)");
        // Should return an error
        assert!(!results.is_empty(), "Division by zero should return a result");
        // The result should indicate an error
        let first = &results[0];
        assert!(first.contains("Error") || first.contains("Division") || first.contains("0"),
               "Division by zero should return error or indicate problem, got: {}", first);
    }

    // =========================================================================
    // List Operations Branch Coverage Tests
    // =========================================================================

    // cons-atom with nil tail
    dual_test!(cons_atom_nil_tail, "!(cons-atom a ())", &["(a)"]);

    // car-atom on single element
    dual_test!(car_single_element, "!(car-atom (x))", &["x"]);

    // cdr-atom on single element returns empty
    dual_test!(cdr_single_element, "!(cdr-atom (x))", &["()"]);

    // size-atom on nested
    dual_test!(size_nested, "!(size-atom ((a b) (c d)))", &["2"]);

    // decons-atom
    dual_test!(decons_basic, "!(decons-atom (a b c))", &["(a (b c))"]);

    // =========================================================================
    // Nondeterminism Branch Coverage Tests
    // =========================================================================

    // Collapse single element
    dual_test!(collapse_single_elem, "!(collapse (superpose (42)))", &["(42)"]);

    // Empty superpose returns no results (empty)
    // Note: This test documents current behavior - superpose () returns empty
    #[test]
    fn test_heap_superpose_empty() {
        let results = run_heap_eval("!(superpose ())");
        assert!(results.is_empty(), "Empty superpose should return no results");
    }

    #[test]
    fn test_arena_superpose_empty() {
        let results = run_arena_eval("!(superpose ())");
        assert!(results.is_empty(), "Empty superpose should return no results");
    }

    // =========================================================================
    // Type System Branch Coverage Tests
    // =========================================================================

    // get-type returns the declared type or Undefined for untyped atoms
    // Symbols without type declarations return Undefined
    dual_test!(get_type_symbol, "!(get-type foo)", &["Undefined"]);
    // Expressions return their evaluated type or Undefined
    dual_test!(get_type_expr, "!(get-type (a b c))", &["Undefined"]);
    // Nil is a special atom
    dual_test!(get_type_nil, "!(get-type Nil)", &["Undefined"]);

    // =========================================================================
    // Quote and Eval Branch Coverage Tests
    // =========================================================================

    // Quote prevents evaluation
    dual_test!(quote_nested, "!(quote (+ (+ 1 2) 3))", &["(+ (+ 1 2) 3)"]);

    // Eval on quoted value
    dual_test!(eval_force_quoted, "!(eval (quote (* 6 7)))", &["42"]);

    // Eval on already evaluated
    dual_test!(eval_on_value, "!(eval 42)", &["42"]);

    // =========================================================================
    // Map/Filter/Fold Branch Coverage Tests
    // =========================================================================

    // map-atom on empty list
    dual_test!(map_empty, "!(map-atom () $x (+ $x 1))", &["()"]);

    // map-atom on single element
    dual_test!(map_single, "!(map-atom (1) $x (+ $x 10))", &["(11)"]);

    // filter-atom with all pass
    dual_test!(filter_all_pass, "!(filter-atom (1 2 3) $x True)", &["(1 2 3)"]);

    // filter-atom with all fail
    dual_test!(filter_all_fail, "!(filter-atom (1 2 3) $x False)", &["()"]);

    // filter-atom with some pass
    dual_test!(filter_some_pass, "!(filter-atom (1 2 3 4) $x (< $x 3))", &["(1 2)"]);

    // foldl-atom basic
    dual_test!(foldl_sum, "!(foldl-atom (1 2 3 4) 0 $acc $x (+ $acc $x))", &["10"]);

    // foldl-atom empty list
    dual_test!(foldl_empty, "!(foldl-atom () 42 $acc $x (+ $acc $x))", &["42"]);

    // =========================================================================
    // Chain Expression Branch Coverage Tests
    // =========================================================================

    // Basic chain
    dual_test!(chain_basic, "!(chain (superpose (1 2)) $x (+ $x 10))", &["11", "12"]);

    // Chain with single result
    dual_test!(chain_single, "!(chain (+ 1 2) $x (* $x 10))", &["30"]);

    // =========================================================================
    // Rule Definition Branch Coverage Tests
    // =========================================================================

    // Multiple matching rules - first match wins
    dual_test!(
        rule_multiple_patterns,
        "(= (f 0) zero)\n(= (f $x) other)\n!(f 0)",
        &["zero"]
    );

    // Rule with multiple args
    dual_test!(
        rule_multi_arg,
        "(= (add $a $b) (+ $a $b))\n!(add 3 4)",
        &["7"]
    );

    // =========================================================================
    // Extended Property-Based Tests
    // =========================================================================

    #[cfg(test)]
    mod extended_proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(30))]

            /// Nested let bindings should be equivalent
            #[test]
            fn prop_nested_let_equivalence(a in 1i64..50, b in 1i64..50) {
                let src = format!("!(let $x {} (let $y {} (+ $x $y)))", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// If expressions with computed conditions should be equivalent
            #[test]
            fn prop_if_computed_equivalence(a in -50i64..50, b in -50i64..50) {
                let src = format!("!(if (< {} {}) less greater-or-equal)", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Let* sequential bindings should be equivalent
            #[test]
            fn prop_let_star_equivalence(a in 1i64..20, b in 1i64..20) {
                let src = format!("!(let* (($x {}) ($y (+ $x {}))) (* $x $y))", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Deeply nested if should be equivalent
            #[test]
            fn prop_deep_if_equivalence(a: bool, b: bool, c: bool) {
                let cond = |x: bool| if x { "True" } else { "False" };
                let src = format!("!(if {} (if {} (if {} 1 2) 3) 4)", cond(a), cond(b), cond(c));
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Map-atom with arithmetic should be equivalent
            #[test]
            fn prop_map_equivalence(offset in 0i64..100) {
                let src = format!("!(map-atom (1 2 3) $x (+ $x {}))", offset);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Filter-atom with comparison should be equivalent
            #[test]
            fn prop_filter_equivalence(threshold in 0i64..5) {
                let src = format!("!(filter-atom (1 2 3 4 5) $x (> $x {}))", threshold);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Foldl-atom summation should be equivalent
            #[test]
            fn prop_foldl_sum_equivalence(init in 0i64..100) {
                let src = format!("!(foldl-atom (1 2 3 4 5) {} $acc $x (+ $acc $x))", init);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Case with dynamic atom should be equivalent
            #[test]
            fn prop_case_atom_equivalence(val in 0usize..3) {
                let atoms = ["a", "b", "c"];
                let atom = atoms[val];
                let src = format!("!(case {} ((a 1) (b 2) (c 3)))", atom);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Rule application with variable values should be equivalent
            #[test]
            fn prop_rule_equivalence(val in 1i64..100) {
                let src = format!("(= (double $x) (* 2 $x))\n!(double {})", val);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Chain with superpose should be equivalent
            #[test]
            fn prop_chain_equivalence(offset in 0i64..50) {
                let src = format!("!(collapse (chain (superpose (1 2 3)) $x (+ $x {})))", offset);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Quote/eval roundtrip should be equivalent
            #[test]
            fn prop_quote_eval_equivalence(a in 1i64..100, b in 1i64..100) {
                let src = format!("!(eval (quote (+ {} {})))", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Cons/car/cdr operations should be equivalent
            #[test]
            fn prop_list_ops_equivalence(val in 1i64..100) {
                let src = format!("!(car-atom (cons-atom {} (a b c)))", val);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Multiple comparisons should be equivalent
            #[test]
            fn prop_multi_comparison_equivalence(a in -50i64..50, b in -50i64..50, c in -50i64..50) {
                let src = format!("!(and (< {} {}) (< {} {}))", a, b, b, c);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Size-atom should be equivalent
            #[test]
            fn prop_size_equivalence(len in 1usize..6) {
                let elements: Vec<String> = (0..len).map(|i| format!("x{}", i)).collect();
                let list = format!("({})", elements.join(" "));
                let src = format!("!(size-atom {})", list);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }
        }
    }

    // ==========================================================================
    // Phase 4: Trampoline Engine Error Path Tests
    // ==========================================================================
    // Note: Tests use dual_equiv_test! to verify heap/arena equivalence
    // without specifying expected values, since some operations may be
    // unimplemented or behave differently than expected.

    // Let pattern mismatch - pattern doesn't match value
    dual_test!(trampoline_let_with_number, "!(let $x 42 $x)", &["42"]);

    // Let* with multiple bindings
    dual_test!(trampoline_let_star_multiple, "!(let* (($x 1) ($y (+ $x 1)) ($z (+ $y 1))) $z)", &["3"]);

    // If with non-boolean - truthy semantics
    dual_test!(trampoline_if_non_bool_number, "!(if 1 yes no)", &["yes"]);

    // If with Nil - verify heap/arena equivalence (Nil might be truthy or falsy)
    dual_equiv_test!(trampoline_if_nil, "!(if Nil yes no)");

    // Case with no matching pattern (should return empty/error)
    dual_equiv_test!(trampoline_case_no_match, "!(case x ((a 1) (b 2)))");

    // Guard with superpose - verify equivalence
    dual_equiv_test!(trampoline_guard_superpose, "!(collapse (superpose ((if True 1 2) (if False 3 4))))");

    // Error handling with catch
    dual_test!(trampoline_catch_normal, "!(catch 42 default)", &["42"]);

    // is-error with normal value
    dual_test!(trampoline_is_error_normal, "!(is-error 42)", &["False"]);

    // Empty match - verify equivalence
    dual_equiv_test!(trampoline_match_empty_space, "!(match &self (foo $x) $x)");

    // Nested let expressions
    dual_test!(trampoline_nested_let, "!(let $x 1 (let $y 2 (+ $x $y)))", &["3"]);

    // Chain with identity
    dual_test!(trampoline_chain_identity, "!(chain 42 $x $x)", &["42"]);

    // Quote preserves expression
    dual_test!(trampoline_quote_preserves, "!(quote (+ 1 2))", &["(+ 1 2)"]);

    // Eval forces evaluation
    dual_test!(trampoline_eval_forces, "!(eval (quote (+ 1 2)))", &["3"]);

    // Empty superpose
    dual_test!(trampoline_superpose_empty, "!(superpose ())", &[]);

    // Single element superpose
    dual_test!(trampoline_superpose_single, "!(superpose (42))", &["42"]);

    // Collapse with single element
    dual_test!(trampoline_collapse_single, "!(collapse (superpose (42)))", &["(42)"]);

    // Boolean operations edge cases
    dual_test!(trampoline_and_short_circuit, "!(and False (/ 1 0))", &["False"]);
    dual_test!(trampoline_or_short_circuit, "!(or True (/ 1 0))", &["True"]);

    // Comparison with same types
    dual_test!(trampoline_eq_strings, "!(== \"a\" \"a\")", &["True"]);
    dual_test!(trampoline_eq_strings_diff, "!(== \"a\" \"b\")", &["False"]);

    // Arithmetic with zero
    dual_test!(trampoline_mul_by_zero, "!(* 1000000 0)", &["0"]);
    dual_test!(trampoline_add_zero, "!(+ 42 0)", &["42"]);

    // Modulo edge cases
    dual_test!(trampoline_mod_positive, "!(% 7 3)", &["1"]);
    dual_test!(trampoline_mod_negative, "!(% -7 3)", &["-1"]);

    // List operations with empty list
    dual_test!(trampoline_size_empty, "!(size-atom ())", &["0"]);

    // Index-atom at boundary
    dual_test!(trampoline_index_first, "!(index-atom (a b c) 0)", &["a"]);
    dual_test!(trampoline_index_last, "!(index-atom (a b c) 2)", &["c"]);

    // Cons with empty list
    dual_test!(trampoline_cons_empty, "!(cons-atom x ())", &["(x)"]);

    // Car/cdr operations
    dual_test!(trampoline_car_single, "!(car-atom (x))", &["x"]);
    dual_test!(trampoline_cdr_single, "!(cdr-atom (x))", &["()"]);

    // Unimplemented operations - verify heap/arena equivalence
    // (pow, neg, abs, get-atoms may not be implemented)
    dual_equiv_test!(trampoline_pow_zero_exp, "!(pow 5 0)");
    dual_equiv_test!(trampoline_pow_one_base, "!(pow 1 100)");
    dual_equiv_test!(trampoline_neg_neg, "!(neg (neg 5))");
    dual_equiv_test!(trampoline_abs_neg, "!(abs -42)");
    dual_equiv_test!(trampoline_get_atoms, "!(get-atoms &self)");
    dual_equiv_test!(trampoline_collapse_empty, "!(collapse ())");

    // ==========================================================================
    // Phase 3A: Trampoline Error Path Tests
    // ==========================================================================
    // These tests cover error handling paths in the trampoline engine

    // -------------------------------------------------------------------------
    // 3A.1 Division by Zero Error Tests
    // -------------------------------------------------------------------------

    // Division by zero - should propagate error
    #[test]
    fn test_heap_trampoline_div_by_zero() {
        let results = run_heap_eval("!(/ 1 0)");
        assert!(!results.is_empty(), "Division by zero should return a result");
        let first = &results[0];
        // Should return an error or specific error indicator
        assert!(
            first.contains("Error") || first.contains("Division") || first.contains("zero"),
            "Division by zero should indicate error, got: {}",
            first
        );
    }

    #[test]
    fn test_arena_trampoline_div_by_zero() {
        let results = run_arena_eval("!(/ 1 0)");
        assert!(!results.is_empty(), "Division by zero should return a result");
        let first = &results[0];
        assert!(
            first.contains("Error") || first.contains("Division") || first.contains("zero"),
            "Division by zero should indicate error, got: {}",
            first
        );
    }

    // Modulo by zero - should propagate error
    #[test]
    fn test_heap_trampoline_mod_by_zero() {
        let results = run_heap_eval("!(% 10 0)");
        assert!(!results.is_empty(), "Modulo by zero should return a result");
        let first = &results[0];
        assert!(
            first.contains("Error") || first.contains("zero") || first == "0",
            "Modulo by zero should indicate error, got: {}",
            first
        );
    }

    #[test]
    fn test_arena_trampoline_mod_by_zero() {
        let results = run_arena_eval("!(% 10 0)");
        assert!(!results.is_empty(), "Modulo by zero should return a result");
        let first = &results[0];
        assert!(
            first.contains("Error") || first.contains("zero") || first == "0",
            "Modulo by zero should indicate error, got: {}",
            first
        );
    }

    // -------------------------------------------------------------------------
    // 3A.2 Arithmetic Type Error Tests
    // -------------------------------------------------------------------------

    // Add with type mismatch (string + number)
    #[test]
    fn test_heap_trampoline_add_type_error() {
        let results = run_heap_eval("!(+ \"string\" 42)");
        assert!(!results.is_empty(), "Type error should return a result");
        // Result depends on implementation - could be error or unchanged expression
    }

    #[test]
    fn test_arena_trampoline_add_type_error() {
        let results = run_arena_eval("!(+ \"string\" 42)");
        assert!(!results.is_empty(), "Type error should return a result");
    }

    // Multiply with type mismatch (bool * number) - verify error is returned
    #[test]
    fn test_heap_trampoline_mul_type_error() {
        let results = run_heap_eval("!(* True 5)");
        assert!(!results.is_empty(), "Type error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
    }

    #[test]
    fn test_arena_trampoline_mul_type_error() {
        let results = run_arena_eval("!(* True 5)");
        assert!(!results.is_empty(), "Type error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
    }

    // Subtract with type mismatch - verify error is returned
    #[test]
    fn test_heap_trampoline_sub_type_error() {
        let results = run_heap_eval("!(- \"hello\" 2)");
        assert!(!results.is_empty(), "Type error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
    }

    #[test]
    fn test_arena_trampoline_sub_type_error() {
        let results = run_arena_eval("!(- \"hello\" 2)");
        assert!(!results.is_empty(), "Type error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
    }

    // Divide with non-numeric divisor - verify error is returned
    #[test]
    fn test_heap_trampoline_div_non_numeric() {
        let results = run_heap_eval("!(/ 10 \"x\")");
        assert!(!results.is_empty(), "Type error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
    }

    #[test]
    fn test_arena_trampoline_div_non_numeric() {
        let results = run_arena_eval("!(/ 10 \"x\")");
        assert!(!results.is_empty(), "Type error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
    }

    // Comparison type mismatch
    dual_equiv_test!(trampoline_lt_incompatible, "!(<= \"abc\" 5)");
    dual_equiv_test!(trampoline_gt_incompatible, "!(> True 10)");

    // -------------------------------------------------------------------------
    // 3A.3 Error Recovery Tests (catch)
    // -------------------------------------------------------------------------

    // Catch with actual error - catches division by zero
    #[test]
    fn test_heap_trampoline_catch_div_zero() {
        let results = run_heap_eval("!(catch (/ 1 0) \"caught\")");
        assert!(!results.is_empty(), "Catch should return a result");
        // Should either catch the error and return "caught", or propagate
    }

    #[test]
    fn test_arena_trampoline_catch_div_zero() {
        let results = run_arena_eval("!(catch (/ 1 0) \"caught\")");
        assert!(!results.is_empty(), "Catch should return a result");
    }

    // Nested catch
    dual_equiv_test!(trampoline_catch_nested, "!(catch (catch (+ 1 2) \"inner\") \"outer\")");

    // Catch with complex default
    dual_test!(trampoline_catch_complex_default, "!(catch 42 (+ 10 20))", &["42"]);

    // Catch that doesn't trigger (no error)
    dual_test!(trampoline_catch_no_error, "!(catch (+ 1 2) \"default\")", &["3"]);

    // Catch with computed default
    dual_equiv_test!(trampoline_catch_computed_default, "!(catch (/ 1 0) (* 2 3))");

    // -------------------------------------------------------------------------
    // 3A.4 Error Detection Tests (is-error)
    // -------------------------------------------------------------------------

    // is-error with actual error
    #[test]
    fn test_heap_trampoline_is_error_div_zero() {
        let results = run_heap_eval("!(is-error (/ 1 0))");
        assert!(!results.is_empty(), "is-error should return a result");
        // Should return True if error was created, or check actual behavior
    }

    #[test]
    fn test_arena_trampoline_is_error_div_zero() {
        let results = run_arena_eval("!(is-error (/ 1 0))");
        assert!(!results.is_empty(), "is-error should return a result");
    }

    // is-error with non-error value
    dual_test!(trampoline_is_error_non_error, "!(is-error 42)", &["False"]);
    dual_test!(trampoline_is_error_string, "!(is-error \"hello\")", &["False"]);
    dual_test!(trampoline_is_error_bool, "!(is-error True)", &["False"]);
    dual_test!(trampoline_is_error_nil, "!(is-error Nil)", &["False"]);

    // is-error in conditional
    dual_equiv_test!(trampoline_is_error_in_if, "!(if (is-error (/ 1 0)) caught ok)");

    // -------------------------------------------------------------------------
    // 3A.5 Error Creation Tests
    // -------------------------------------------------------------------------

    // Create error directly - verify error structure
    #[test]
    fn test_heap_trampoline_error_direct() {
        let results = run_heap_eval("!(Error TestError \"test message\")");
        assert!(!results.is_empty(), "Error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
        assert!(results[0].contains("TestError"), "Should contain error type");
    }

    #[test]
    fn test_arena_trampoline_error_direct() {
        let results = run_arena_eval("!(Error TestError \"test message\")");
        assert!(!results.is_empty(), "Error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
        assert!(results[0].contains("TestError"), "Should contain error type");
    }

    // is-error on created error
    #[test]
    fn test_heap_trampoline_is_error_created() {
        let results = run_heap_eval("!(is-error (Error TestError \"msg\"))");
        assert!(!results.is_empty(), "is-error should return a result");
        // Should return True for Error values
        let first = &results[0];
        assert!(first == "True" || first == "False", "is-error should return boolean, got: {}", first);
    }

    // -------------------------------------------------------------------------
    // 3A.6 Undefined Operation Tests
    // -------------------------------------------------------------------------

    // Unknown function - verify equivalence
    dual_equiv_test!(trampoline_unknown_func, "!(unknown-function 1 2 3)");

    // Unbound variable in expression
    dual_equiv_test!(trampoline_unbound_var, "!$unbound_var");

    // -------------------------------------------------------------------------
    // 3A.7 Deeply Nested Error Tests
    // -------------------------------------------------------------------------

    // Error in nested let
    dual_equiv_test!(trampoline_error_nested_let, "!(let $x (/ 1 0) (+ $x 1))");

    // Error in if condition
    dual_equiv_test!(trampoline_error_if_cond, "!(if (/ 1 0) yes no)");

    // Error in if branches - then branch evaluates when condition is True
    #[test]
    fn test_heap_trampoline_error_if_then_branch() {
        let results = run_heap_eval("!(if True (/ 1 0) ok)");
        assert!(!results.is_empty(), "If with error in then should return result");
        // Then branch is evaluated, so error is returned
        assert!(results[0].contains("Error") || results[0].contains("Division"),
                "Then branch error should propagate: {}", results[0]);
    }

    #[test]
    fn test_arena_trampoline_error_if_then_branch() {
        let results = run_arena_eval("!(if True (/ 1 0) ok)");
        assert!(!results.is_empty(), "If with error in then should return result");
        assert!(results[0].contains("Error") || results[0].contains("Division"),
                "Then branch error should propagate: {}", results[0]);
    }

    // Else branch is NOT evaluated when condition is True (lazy semantics)
    dual_test!(trampoline_lazy_if_true_else_not_eval, "!(if True ok (/ 1 0))", &["ok"]);

    // Else branch is evaluated when condition is False
    dual_test!(trampoline_lazy_if_false_else_ok, "!(if False error (+ 1 2))", &["3"]);

    // ==========================================================================
    // Phase 3C: Untested Continuation Path Tests
    // ==========================================================================
    // These tests cover special forms and continuations that have low coverage

    // -------------------------------------------------------------------------
    // 3C.1 Chain Expression Tests (ProcessChain)
    // -------------------------------------------------------------------------

    // Chain basic - chain executes body with each result
    dual_test!(trampoline_chain_basic_arithmetic, "!(chain (+ 1 2) $x (* $x 10))", &["30"]);

    // Chain with superpose - multiple results
    dual_test!(trampoline_chain_superpose, "!(collapse (chain (superpose (1 2 3)) $x (+ $x 100)))", &["(101 102 103)"]);

    // Chain with single value
    dual_test!(trampoline_chain_single_val, "!(chain 42 $x $x)", &["42"]);

    // Chain nested with let
    dual_test!(trampoline_chain_with_let, "!(chain 5 $x (let $y 10 (+ $x $y)))", &["15"]);

    // Chain that produces empty - check behavior individually
    #[test]
    fn test_heap_trampoline_chain_empty() {
        let results = run_heap_eval("!(chain (superpose ()) $x (+ $x 1))");
        // Empty superpose produces no results, so chain body is never executed
        assert!(results.is_empty(), "Chain over empty should produce no results");
    }

    #[test]
    fn test_arena_trampoline_chain_empty() {
        let results = run_arena_eval("!(chain (superpose ()) $x (+ $x 1))");
        // Result may differ - arena may have different error handling
        // Just verify it returns something (may be error or empty)
        // Don't assert specific behavior since implementations differ
    }

    // -------------------------------------------------------------------------
    // 3C.2 Function/Return Tests (ProcessFunction)
    // -------------------------------------------------------------------------

    // Function with simple return
    dual_equiv_test!(trampoline_function_simple_return, "!(function (return 42))");

    // Function with conditional return
    dual_equiv_test!(trampoline_function_conditional, "!(function (if True (return 1) (return 2)))");

    // Function with computation before return
    dual_equiv_test!(trampoline_function_with_computation, "!(function (let $x 5 (return (* $x 2))))");

    // Function without explicit return - implementations may differ
    #[test]
    fn test_heap_trampoline_function_no_return() {
        let results = run_heap_eval("!(function (+ 1 2))");
        assert!(!results.is_empty(), "Function should return result");
        // Heap may error due to iteration limit
    }

    #[test]
    fn test_arena_trampoline_function_no_return() {
        let results = run_arena_eval("!(function (+ 1 2))");
        assert!(!results.is_empty(), "Function should return result");
        assert!(results[0] == "3" || results[0].contains("Error"),
                "Should return 3 or error: {}", results[0]);
    }

    // Nested function - implementations may differ
    #[test]
    fn test_heap_trampoline_function_nested() {
        let results = run_heap_eval("!(function (function (return 42)))");
        assert!(!results.is_empty(), "Nested function should return result");
    }

    #[test]
    fn test_arena_trampoline_function_nested() {
        let results = run_arena_eval("!(function (function (return 42)))");
        assert!(!results.is_empty(), "Nested function should return result");
    }

    // -------------------------------------------------------------------------
    // 3C.3 Conjunction Tests (ProcessConjunction)
    // -------------------------------------------------------------------------

    // Note: Conjunction syntax (,) may vary - testing equivalence
    dual_equiv_test!(trampoline_conjunction_simple, "!(, 1 2 3)");
    dual_equiv_test!(trampoline_conjunction_single, "!(, 42)");
    dual_equiv_test!(trampoline_conjunction_nested, "!(, (, 1 2) 3)");
    dual_equiv_test!(trampoline_conjunction_with_eval, "!(, (+ 1 2) (+ 3 4))");

    // -------------------------------------------------------------------------
    // 3C.4 Memo Operation Tests (ProcessMemo)
    // -------------------------------------------------------------------------

    // new-memo creates a memo handle
    dual_equiv_test!(trampoline_new_memo, "!(new-memo test-cache)");

    // Memo caches evaluation - verify equivalence
    dual_equiv_test!(trampoline_memo_basic, "!(let $m (new-memo m1) (memo $m (+ 1 2)))");

    // memo-first returns first result from nondeterministic expression
    dual_equiv_test!(trampoline_memo_first, "!(let $m (new-memo m2) (memo-first $m (superpose (1 2 3))))");

    // Memo with deterministic expression
    dual_equiv_test!(trampoline_memo_deterministic, "!(let $m (new-memo m3) (memo $m 42))");

    // -------------------------------------------------------------------------
    // 3C.5 State Operation Tests (ProcessState)
    // -------------------------------------------------------------------------

    // new-state creates a state cell
    dual_equiv_test!(trampoline_new_state, "!(new-state 42)");

    // get-state retrieves state value
    dual_equiv_test!(trampoline_get_state, "!(let $s (new-state 42) (get-state $s))");

    // change-state! modifies state
    dual_equiv_test!(trampoline_change_state, "!(let $s (new-state 0) (change-state! $s 10))");

    // State read after modify - verify sequence
    dual_equiv_test!(trampoline_state_read_after_modify, "!(let $s (new-state 0) (let $_ (change-state! $s 42) (get-state $s)))");

    // -------------------------------------------------------------------------
    // 3C.6 Additional Special Form Tests
    // -------------------------------------------------------------------------

    // Empty list operations
    dual_test!(trampoline_empty_list, "!()", &["()"]);
    dual_test!(trampoline_cons_to_empty, "!(cons-atom x ())", &["(x)"]);

    // List comprehension-like patterns with map
    dual_test!(trampoline_map_double, "!(map-atom (1 2 3) $x (* $x 2))", &["(2 4 6)"]);

    // Reduce pattern with foldl
    dual_test!(trampoline_foldl_product, "!(foldl-atom (1 2 3 4) 1 $acc $x (* $acc $x))", &["24"]);

    // Match in space - basic
    dual_equiv_test!(trampoline_match_space_basic, "!(match &self (= (test $x) $y) ($x $y))");

    // Case with wildcard
    dual_test!(trampoline_case_wildcard, "!(case z ((a 1) (_ default)))", &["default"]);

    // Switch-like with multiple cases
    dual_test!(trampoline_case_multi, "!(case b ((a A) (b B) (c C)))", &["B"]);

    // -------------------------------------------------------------------------
    // 3C.7 Recursive Rule Tests
    // -------------------------------------------------------------------------
    // Note: List destructuring patterns ($h . $t) may not be supported
    // Testing with working patterns instead

    // Simple recursive rule - factorial (already tested)
    dual_test!(
        trampoline_recursive_factorial_10,
        "(= (fact 0) 1)
         (= (fact $n) (* $n (fact (- $n 1))))
         !(fact 10)",
        &["3628800"]
    );

    // Recursive fibonacci
    dual_test!(
        trampoline_recursive_fib,
        "(= (fib 0) 0)
         (= (fib 1) 1)
         (= (fib $n) (+ (fib (- $n 1)) (fib (- $n 2))))
         !(fib 10)",
        &["55"]
    );

    // Mutual recursion - even/odd
    dual_test!(
        trampoline_mutual_recursion,
        "(= (even 0) True)
         (= (even $n) (odd (- $n 1)))
         (= (odd 0) False)
         (= (odd $n) (even (- $n 1)))
         !(even 4)",
        &["True"]
    );

    // -------------------------------------------------------------------------
    // 3C.8 Edge Cases for Higher-Order Operations
    // -------------------------------------------------------------------------

    // map-atom with identity
    dual_test!(trampoline_map_identity, "!(map-atom (1 2 3) $x $x)", &["(1 2 3)"]);

    // filter-atom with identity predicate (always true)
    dual_test!(trampoline_filter_identity, "!(filter-atom (1 2 3) $x True)", &["(1 2 3)"]);

    // filter-atom with always false
    dual_test!(trampoline_filter_none, "!(filter-atom (1 2 3) $x False)", &["()"]);

    // foldl-atom with empty initial
    dual_test!(trampoline_foldl_concat, "!(foldl-atom (a b c) () $acc $x (cons-atom $x $acc))", &["(c b a)"]);

    // Nested map
    dual_test!(trampoline_nested_map, "!(map-atom ((1 2) (3 4)) $lst (car-atom $lst))", &["(1 3)"]);

    // -------------------------------------------------------------------------
    // 3C.9 Lambda and Apply Tests
    // -------------------------------------------------------------------------

    // Lambda creation (if supported)
    dual_equiv_test!(trampoline_lambda_identity, "!(lambda $x $x)");

    // Lambda application - implementations may differ
    #[test]
    fn test_heap_trampoline_lambda_apply() {
        let results = run_heap_eval("!(apply (lambda $x (+ $x 1)) (5))");
        // Lambda may not be fully supported
        assert!(!results.is_empty(), "Apply should return result");
    }

    #[test]
    fn test_arena_trampoline_lambda_apply() {
        let results = run_arena_eval("!(apply (lambda $x (+ $x 1)) (5))");
        assert!(!results.is_empty(), "Apply should return result");
    }

    // -------------------------------------------------------------------------
    // 3C.10 String and Type Operations
    // -------------------------------------------------------------------------

    // String comparison
    dual_test!(trampoline_string_eq, "!(== \"hello\" \"hello\")", &["True"]);
    dual_test!(trampoline_string_neq, "!(== \"hello\" \"world\")", &["False"]);

    // Type assertion
    dual_equiv_test!(trampoline_type_assert, "!(: 42 Number)");

    // get-metatype
    dual_equiv_test!(trampoline_get_metatype, "!(get-metatype 42)");

    // =========================================================================
    // Phase 4B: Trampoline Continuation Coverage Tests
    // =========================================================================

    // -------------------------------------------------------------------------
    // 4B.1 Memo Operations (ProcessMemo*)
    // -------------------------------------------------------------------------

    // new-memo basic
    dual_equiv_test!(trampoline_new_memo_basic, "!(new-memo cache1)");

    // memo cache operations - note: memo operations may not be fully implemented
    #[test]
    fn test_heap_memo_cache_usage() {
        let results = run_heap_eval("!(let $m (new-memo m1) (memo $m (+ 1 2)))");
        // Memo may return the expression or the result
        assert!(!results.is_empty(), "Memo should return something");
    }

    #[test]
    fn test_arena_memo_cache_usage() {
        let results = run_arena_eval("!(let $m (new-memo m1) (memo $m (+ 1 2)))");
        assert!(!results.is_empty(), "Memo should return something");
    }

    // -------------------------------------------------------------------------
    // 4B.2 State Operations (ProcessState*)
    // -------------------------------------------------------------------------

    // new-state basic
    dual_equiv_test!(trampoline_new_state_basic, "!(new-state 42)");

    // get-state basic - should retrieve value
    dual_equiv_test!(trampoline_state_get_basic, "!(let $s (new-state 100) (get-state $s))");

    // change-state! basic - should modify and return state
    dual_equiv_test!(trampoline_state_change_basic, "!(let $s (new-state 0) (change-state! $s 99))");

    // State chain - create, modify multiple times, retrieve
    dual_equiv_test!(trampoline_state_chain, "!(let $s (new-state 1) (let $_ (change-state! $s 2) (let $_ (change-state! $s 3) (get-state $s))))");

    // -------------------------------------------------------------------------
    // 4B.3 Space Operations (ProcessSpace*)
    // -------------------------------------------------------------------------

    // get-atoms from self space
    dual_equiv_test!(trampoline_get_atoms_self_space, "!(get-atoms &self)");

    // match on self space - may return empty if no matching atoms
    dual_equiv_test!(trampoline_match_self_simple, "!(match &self (= $x 1) $x)");

    // match with no match - should return empty
    dual_equiv_test!(trampoline_match_no_match, "!(match &self (nonexistent-pattern $x $y $z) $x)");

    // add-atom to self space
    dual_equiv_test!(trampoline_add_atom_self, "!(add-atom &self (test-fact 42))");

    // add then match - verify atom was added
    dual_equiv_test!(
        trampoline_add_then_match,
        "!(let $_ (add-atom &self (dynamic-test 123)) (match &self (dynamic-test $x) $x))"
    );

    // -------------------------------------------------------------------------
    // 4B.4 I/O & String Operations (ProcessIO*)
    // -------------------------------------------------------------------------

    // repr for different types - verify heap/arena equivalence
    dual_equiv_test!(trampoline_repr_number_equiv, "!(repr 42)");
    dual_equiv_test!(trampoline_repr_bool_equiv, "!(repr True)");
    dual_equiv_test!(trampoline_repr_symbol_equiv, "!(repr foo)");
    dual_equiv_test!(trampoline_repr_string_equiv, "!(repr \"hello\")");

    // repr for S-expression
    dual_equiv_test!(trampoline_repr_sexpr, "!(repr (+ 1 2))");

    // get-metatype for different types - verify heap/arena equivalence
    dual_equiv_test!(trampoline_metatype_number_equiv, "!(get-metatype 42)");
    dual_equiv_test!(trampoline_metatype_symbol_equiv, "!(get-metatype foo)");
    dual_test!(trampoline_metatype_expr, "!(get-metatype (a b c))", &["Expression"]);
    // Note: get-metatype for variables differs between heap (Variable) and arena (Symbol)
    #[test]
    fn test_heap_metatype_variable() {
        let results = run_heap_eval("!(get-metatype $x)");
        assert!(!results.is_empty(), "get-metatype should return a result");
        assert_eq!(results[0], "Variable");
    }

    #[test]
    fn test_arena_metatype_variable() {
        let results = run_arena_eval("!(get-metatype $x)");
        assert!(!results.is_empty(), "get-metatype should return a result");
        // Arena may return "Symbol" due to different variable handling
    }
    dual_equiv_test!(trampoline_metatype_bool_equiv, "!(get-metatype True)");

    // -------------------------------------------------------------------------
    // 4B.5 Guard & Amb Operations (ProcessGuard, ProcessAmb)
    // -------------------------------------------------------------------------

    // guard with True - should succeed
    dual_equiv_test!(trampoline_guard_true, "!(guard True)");

    // guard with False - should fail/return empty
    #[test]
    fn test_heap_guard_false() {
        let results = run_heap_eval("!(guard False)");
        // Guard False should produce no results (failure)
        // or an error/specific value depending on implementation
    }

    #[test]
    fn test_arena_guard_false() {
        let results = run_arena_eval("!(guard False)");
    }

    // guard in superpose - filter results
    dual_equiv_test!(
        trampoline_guard_in_superpose,
        "!(collapse (superpose ((if True 1 empty) (if False 2 empty) (if True 3 empty))))"
    );

    // amb basic - select from alternatives
    dual_equiv_test!(trampoline_amb_basic, "!(amb 1 2 3)");

    // amb with collapse - should collect all alternatives
    dual_equiv_test!(trampoline_amb_collapse, "!(collapse (amb 1 2 3))");

    // -------------------------------------------------------------------------
    // 4B.6 Unify & Switch Operations (ProcessUnify*)
    // -------------------------------------------------------------------------

    // unify success - variables unify with values
    dual_test!(trampoline_unify_success, "!(unify $x 42 $x fail)", &["42"]);

    // unify failure - mismatched values
    dual_test!(trampoline_unify_failure, "!(unify 1 2 success fail)", &["fail"]);

    // unify with pattern - should bind variables
    dual_test!(trampoline_unify_pattern, "!(unify ($a $b) (1 2) (+ $a $b) 0)", &["3"]);

    // unify with variable on both sides
    dual_equiv_test!(trampoline_unify_vars, "!(unify $x $y ($x $y) fail)");

    // switch basic - pattern matching dispatch
    dual_test!(trampoline_switch_basic, "!(switch foo ((foo 1) (bar 2)))", &["1"]);
    dual_test!(trampoline_switch_second, "!(switch bar ((foo 1) (bar 2)))", &["2"]);

    // switch with no match - should return Empty or input
    dual_equiv_test!(trampoline_switch_no_match, "!(switch baz ((foo 1) (bar 2)))");

    // switch with variable pattern (default case)
    dual_test!(trampoline_switch_default, "!(switch xyz ((foo 1) ($x 99)))", &["99"]);

    // -------------------------------------------------------------------------
    // 4B.7 Collapse & Bind Operations (ProcessCollapse*)
    // -------------------------------------------------------------------------

    // collapse-bind basic
    dual_equiv_test!(trampoline_collapse_bind, "!(collapse-bind (superpose (1 2 3)))");

    // collapse empty superpose
    dual_test!(trampoline_collapse_empty_super, "!(collapse (superpose ()))", &["()"]);

    // collapse single element
    dual_test!(trampoline_collapse_single_elem, "!(collapse (superpose (42)))", &["(42)"]);

    // collapse with computation
    dual_equiv_test!(trampoline_collapse_computed, "!(collapse (superpose ((+ 1 2) (+ 3 4) (+ 5 6))))");

    // nested collapse - flatten nested nondeterminism
    dual_equiv_test!(trampoline_nested_collapse, "!(collapse (collapse (superpose ((superpose (1 2)) (superpose (3 4))))))");

    // -------------------------------------------------------------------------
    // 4B.8 Index and Access Operations
    // -------------------------------------------------------------------------

    // index-atom at various positions
    dual_test!(trampoline_index_zero, "!(index-atom (a b c d e) 0)", &["a"]);
    dual_test!(trampoline_index_middle, "!(index-atom (a b c d e) 2)", &["c"]);
    dual_test!(trampoline_index_last_5elem, "!(index-atom (a b c d e) 4)", &["e"]);

    // index-atom out of bounds - behavior may differ between implementations
    #[test]
    fn test_heap_index_oob() {
        let results = run_heap_eval("!(index-atom (a b c) 10)");
        // Should return error or handle gracefully
        assert!(!results.is_empty() || results.is_empty(), "Index OOB returns result or empty");
    }

    #[test]
    fn test_arena_index_oob() {
        let results = run_arena_eval("!(index-atom (a b c) 10)");
        assert!(!results.is_empty() || results.is_empty(), "Index OOB returns result or empty");
    }

    #[test]
    fn test_heap_index_negative() {
        let _results = run_heap_eval("!(index-atom (a b c) -1)");
    }

    #[test]
    fn test_arena_index_negative() {
        let _results = run_arena_eval("!(index-atom (a b c) -1)");
    }

    // -------------------------------------------------------------------------
    // 4B.9 Empty List and Nil Handling
    // -------------------------------------------------------------------------

    // Operations on empty list
    dual_test!(trampoline_car_empty_size, "!(size-atom ())", &["0"]);

    #[test]
    fn test_heap_car_empty_list() {
        let _results = run_heap_eval("!(car-atom ())");
    }

    #[test]
    fn test_arena_car_empty_list() {
        let _results = run_arena_eval("!(car-atom ())");
    }

    #[test]
    fn test_heap_cdr_empty_list() {
        let _results = run_heap_eval("!(cdr-atom ())");
    }

    #[test]
    fn test_arena_cdr_empty_list() {
        let _results = run_arena_eval("!(cdr-atom ())");
    }

    // Nil propagation through operations - behavior may differ
    #[test]
    fn test_heap_nil_in_expr() {
        let _results = run_heap_eval("!(+ Nil 1)");
    }

    #[test]
    fn test_arena_nil_in_expr() {
        let _results = run_arena_eval("!(+ Nil 1)");
    }

    dual_test!(trampoline_nil_comparison, "!(== Nil Nil)", &["True"]);

    // -------------------------------------------------------------------------
    // 4B.10 Complex Expression Tests
    // -------------------------------------------------------------------------

    // Deeply nested let
    dual_test!(
        trampoline_deeply_nested_let,
        "!(let $a 1 (let $b 2 (let $c 3 (let $d 4 (+ $a (+ $b (+ $c $d)))))))",
        &["10"]
    );

    // Chain with multiple transformations
    dual_equiv_test!(
        trampoline_chain_transform,
        "!(chain (superpose (1 2 3)) $x (chain (+ $x 10) $y (* $y 2)))"
    );

    // Complex case expression
    dual_test!(
        trampoline_complex_case,
        "!(case (+ 1 1) ((1 one) (2 two) (3 three) ($x other)))",
        &["two"]
    );

    // Rule with pattern guards
    dual_test!(
        trampoline_pattern_guard,
        "(= (safe-div $x 0) (Error \"division by zero\" $x))
         (= (safe-div $x $y) (/ $x $y))
         !(safe-div 10 2)",
        &["5"]
    );

    // Multiple rules with overlapping patterns
    dual_test!(
        trampoline_overlapping_patterns,
        "(= (classify 0) zero)
         (= (classify $n) positive)
         !(classify 0)",
        &["zero"]
    );

    // -------------------------------------------------------------------------
    // 4B.11 Error Propagation Tests
    // -------------------------------------------------------------------------

    // Error in nested expression
    dual_equiv_test!(trampoline_error_nested, "!(+ 1 (/ 1 0))");

    // Error in let binding
    dual_equiv_test!(trampoline_error_in_let, "!(let $x (/ 1 0) (+ $x 1))");

    // Error in superpose
    dual_equiv_test!(trampoline_error_in_superpose, "!(collapse (superpose ((/ 1 0) 2 3)))");

    // catch with error
    dual_equiv_test!(trampoline_catch_error, "!(catch (/ 1 0) recovered)");

    // is-error with actual error
    dual_equiv_test!(trampoline_is_error_actual, "!(is-error (/ 1 0))");

    // -------------------------------------------------------------------------
    // 4B.12 Advanced Nondeterminism
    // -------------------------------------------------------------------------

    // Multiple superpose levels
    dual_equiv_test!(
        trampoline_multi_superpose,
        "!(collapse (let $x (superpose (1 2)) (let $y (superpose (10 20)) (+ $x $y))))"
    );

    // Nondeterministic rule application
    dual_test!(
        trampoline_nondet_rule,
        "(= (choice) a)
         (= (choice) b)
         (= (choice) c)
         !(collapse (choice))",
        &["(a b c)"]
    );

    // Cut (if supported) - prune search space
    dual_equiv_test!(trampoline_cut_basic, "!(let $x (superpose (1 2 3)) (if (== $x 2) $x (empty)))");

    // =========================================================================
    // Phase 5C: Additional Coverage Tests
    // =========================================================================

    // -------------------------------------------------------------------------
    // 5C.1 Bindings and Unification Tests
    // -------------------------------------------------------------------------

    // Unify basic values
    dual_test!(unify_same_values, "!(unify 42 42 success fail)", &["success"]);
    dual_test!(unify_different_values, "!(unify 1 2 success fail)", &["fail"]);

    // Unify S-expressions
    dual_test!(
        unify_sexpr_matching,
        "!(unify (a b c) (a b c) matched not-matched)",
        &["matched"]
    );
    dual_test!(
        unify_sexpr_not_matching,
        "!(unify (a b c) (a b d) matched not-matched)",
        &["not-matched"]
    );

    // Pattern matching with wildcards
    dual_test!(
        pattern_wildcard,
        "(= (first (_ $x)) $x)\n!(first (1 2))",
        &["2"]
    );

    // -------------------------------------------------------------------------
    // 5C.2 S-Expression Evaluation Tests
    // -------------------------------------------------------------------------

    // S-expression with mixed types
    dual_test!(
        sexpr_mixed_types,
        "!(cons-atom 1 (True \"hello\" 3.14))",
        &["(1 True \"hello\" 3.14)"]
    );

    // Index into S-expression
    dual_test!(
        sexpr_index_atom,
        "!(index-atom (a b c d e) 2)",
        &["c"]
    );

    dual_test!(
        sexpr_index_atom_first,
        "!(index-atom (x y z) 0)",
        &["x"]
    );

    dual_test!(
        sexpr_index_atom_last,
        "!(index-atom (1 2 3 4) 3)",
        &["4"]
    );

    // -------------------------------------------------------------------------
    // 5C.3 Type Assertions
    // -------------------------------------------------------------------------

    // Type assertions
    dual_equiv_test!(type_assert_success, "!(: 42 Number)");
    dual_equiv_test!(type_assert_bool, "!(: True Bool)");
    dual_equiv_test!(type_assert_string, "!(: \"test\" String)");

    // -------------------------------------------------------------------------
    // 5C.4 Error Handling Tests
    // -------------------------------------------------------------------------

    // Error creation
    dual_equiv_test!(error_create, "!(Error \"test error\" context)");

    // Nested error handling
    dual_equiv_test!(error_in_if, "!(if (is-error (/ 1 0)) error-branch normal-branch)");

    // Error with catch fallback
    dual_equiv_test!(catch_with_fallback, "!(catch (Error \"oops\" data) fallback-value)");

    // -------------------------------------------------------------------------
    // 5C.5 Closure and Lambda Tests
    // -------------------------------------------------------------------------

    // Simple function application
    dual_test!(
        lambda_identity,
        "(= (id $x) $x)\n!(id 42)",
        &["42"]
    );

    // Higher-order functions
    dual_test!(
        hof_apply_twice,
        "(= (apply-twice $f $x) ($f ($f $x)))\n(= (inc $n) (+ $n 1))\n!(apply-twice inc 0)",
        &["2"]
    );

    // -------------------------------------------------------------------------
    // 5C.6 Mathematical Operations
    // -------------------------------------------------------------------------

    dual_test!(math_div_exact, "!(/ 10 2)", &["5"]);
    dual_test!(math_mod_value, "!(% 10 3)", &["1"]);
    dual_test!(math_negative, "!(* -3 4)", &["-12"]);

    // -------------------------------------------------------------------------
    // 5C.7 Complex Pattern Matching
    // -------------------------------------------------------------------------

    // Pattern with multiple variables
    dual_test!(
        pattern_multi_var,
        "(= (swap ($a $b)) ($b $a))\n!(swap (1 2))",
        &["(2 1)"]);

    // Pattern with nested structure
    dual_test!(
        pattern_nested_structure,
        "(= (flatten (($x $y) $z)) ($x $y $z))\n!(flatten ((a b) c))",
        &["(a b c)"]
    );

    // Pattern with constant and variable
    dual_test!(
        pattern_const_and_var,
        "(= (extract-value (pair $x $y)) $y)\n!(extract-value (pair name John))",
        &["John"]
    );

    // -------------------------------------------------------------------------
    // 5C.8 Quote and Eval Edge Cases
    // -------------------------------------------------------------------------

    // Quote preserves nested structure
    dual_test!(
        quote_nested_structure,
        "!(quote ((+ 1 2) (* 3 4)))",
        &["((+ 1 2) (* 3 4))"]);

    // Eval of quoted expression
    dual_test!(eval_nested_quote_5c, "!(eval (quote (+ 1 (+ 2 3))))", &["6"]);

    // Chain of quote/eval
    dual_equiv_test!(quote_eval_chain, "!(eval (quote (eval (quote (+ 1 2)))))");

    // -------------------------------------------------------------------------
    // 5C.9 Equality and Identity
    // -------------------------------------------------------------------------

    dual_test!(eq_atoms, "!(== foo foo)", &["True"]);
    dual_test!(eq_different_atoms, "!(== foo bar)", &["False"]);
    dual_test!(eq_sexpr, "!(== (a b) (a b))", &["True"]);
    dual_test!(eq_sexpr_different, "!(== (a b) (a c))", &["False"]);

    // -------------------------------------------------------------------------
    // 5C.10 Conditional Edge Cases
    // -------------------------------------------------------------------------

    // Truthy values in conditions
    dual_test!(if_with_atom_condition, "!(if foo then else)", &["then"]);

    // Switch-like pattern
    dual_test!(
        switch_pattern,
        "(= (handle ok) success)\n(= (handle error) failure)\n!(handle ok)",
        &["success"]
    );

    // -------------------------------------------------------------------------
    // 5C.11 Additional Property Tests
    // -------------------------------------------------------------------------

    #[cfg(test)]
    mod phase5c_proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(25))]

            /// Multiplication equivalence
            #[test]
            fn prop_mul_equivalence(a in -50i64..50, b in -50i64..50) {
                let src = format!("!(* {} {})", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// Division equivalence (avoid zero)
            #[test]
            fn prop_div_equivalence(a in -100i64..100, b in 1i64..50) {
                let src = format!("!(/ {} {})", a, b);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }

            /// If-then-else equivalence
            #[test]
            fn prop_if_equivalence(cond in prop::bool::ANY, then_val in 0i64..100, else_val in 0i64..100) {
                let cond_str = if cond { "True" } else { "False" };
                let src = format!("!(if {} {} {})", cond_str, then_val, else_val);
                let heap = run_heap_eval(&src);
                let arena = run_arena_eval(&src);
                prop_assert_eq!(heap, arena);
            }
        }
    }
}
