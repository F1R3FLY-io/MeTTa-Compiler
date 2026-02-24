//! Arena Evaluation Tests
//!
//! Comprehensive test suite for the arena-based evaluation engine.
//! Tests cover arithmetic, control flow, pattern matching, nondeterminism,
//! error handling, higher-order operations, and more.

#[cfg(test)]
mod tests {
    use crate::backend::compile::compile;
    use crate::backend::eval::trampoline::{eval_trampoline, new_env};

    /// Helper to run arena-based evaluation and collect results as strings
    fn run_eval(src: &str) -> Vec<String> {
        let state = compile(src).expect("compile failed");
        let mut env = new_env();
        let mut all_results = Vec::new();

        let source_exprs: Vec<_> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (results, new_env) = eval_trampoline(expr, env, &state);
            env = new_env;
            for result in &results {
                all_results.push(result.to_string());
            }
        }
        all_results
    }

    /// Macro to test evaluation with expected results (order-sensitive).
    macro_rules! eval_test {
        ($name:ident, $metta_src:expr, $expected:expr) => {
            #[test]
            fn $name() {
                let results = run_eval($metta_src);
                let expected_slice: &[&str] = $expected;
                let expected: Vec<String> =
                    expected_slice.iter().map(|s| s.to_string()).collect();
                assert_eq!(
                    results, expected,
                    "Eval failed for: {}",
                    $metta_src
                );
            }
        };
    }

    /// Macro to test evaluation with expected results (order-independent).
    /// Sorts both actual and expected before comparing.
    macro_rules! eval_test_unordered {
        ($name:ident, $metta_src:expr, $expected:expr) => {
            #[test]
            fn $name() {
                let mut results = run_eval($metta_src);
                let expected_slice: &[&str] = $expected;
                let mut expected: Vec<String> =
                    expected_slice.iter().map(|s| s.to_string()).collect();
                results.sort();
                expected.sort();
                assert_eq!(
                    results, expected,
                    "Eval failed (unordered) for: {}",
                    $metta_src
                );
            }
        };
    }

    // =========================================================================
    // Basic Arithmetic
    // =========================================================================

    eval_test!(arithmetic_add, "!(+ 1 2)", &["3"]);
    eval_test!(arithmetic_sub, "!(- 10 3)", &["7"]);
    eval_test!(arithmetic_mul, "!(* 4 5)", &["20"]);
    eval_test!(arithmetic_div, "!(/ 20 4)", &["5"]);
    eval_test!(arithmetic_mod, "!(% 17 5)", &["2"]);
    eval_test!(arithmetic_neg, "!(- 0 42)", &["-42"]);

    // Unary minus (negation)
    eval_test!(unary_minus_int, "!(- 5)", &["-5"]);
    eval_test!(unary_minus_zero, "!(- 0)", &["0"]);
    eval_test!(unary_minus_negative, "!(- -7)", &["7"]);
    eval_test!(unary_minus_float, "!(- 3.14)", &["-3.14"]);
    eval_test!(unary_minus_float_neg, "!(- -2.5)", &["2.5"]);

    eval_test!(arithmetic_nested, "!(+ (* 2 3) (- 10 4))", &["12"]);
    eval_test!(arithmetic_deeply_nested, "!(+ 1 (+ 2 (+ 3 4)))", &["10"]);

    // =========================================================================
    // Eval-before-match: args evaluated before rule matching (MeTTa HE semantics)
    // =========================================================================

    // User-defined function args evaluated before rule matching
    eval_test!(eval_before_match_nested_call,
        "(= (double $x) (+ $x $x)) !(double (+ 1 2))",
        &["6"]);

    eval_test!(eval_before_match_data_constructor,
        "(= (wrap $x) (wrapped $x)) !(wrap (+ 2 3))",
        &["(wrapped 5)"]);

    // =========================================================================
    // Comparisons
    // =========================================================================

    eval_test!(comparison_lt_true, "!(< 1 2)", &["True"]);
    eval_test!(comparison_lt_false, "!(< 2 1)", &["False"]);
    eval_test!(comparison_le_true, "!(<= 2 2)", &["True"]);
    eval_test!(comparison_gt_true, "!(> 5 3)", &["True"]);
    eval_test!(comparison_ge_true, "!(>= 3 3)", &["True"]);
    eval_test!(comparison_eq_true, "!(== 42 42)", &["True"]);
    eval_test!(comparison_eq_false, "!(== 1 2)", &["False"]);

    // =========================================================================
    // Boolean Operations
    // =========================================================================

    eval_test!(boolean_and_true, "!(and True True)", &["True"]);
    eval_test!(boolean_and_false, "!(and True False)", &["False"]);
    eval_test!(boolean_or_true, "!(or False True)", &["True"]);
    eval_test!(boolean_or_false, "!(or False False)", &["False"]);
    eval_test!(boolean_not_true, "!(not False)", &["True"]);
    eval_test!(boolean_not_false, "!(not True)", &["False"]);

    // =========================================================================
    // Control Flow
    // =========================================================================

    eval_test!(if_true_branch, "!(if True 1 2)", &["1"]);
    eval_test!(if_false_branch, "!(if False 1 2)", &["2"]);
    eval_test!(if_nested, "!(if True (if False 3 4) 5)", &["4"]);
    eval_test!(if_true, "!(if True then else)", &["then"]);
    eval_test!(if_false, "!(if False then else)", &["else"]);
    eval_test!(if_computed_condition, "!(if (< 1 2) yes no)", &["yes"]);
    eval_test!(if_nested_then, "!(if True (+ 1 2) 0)", &["3"]);
    eval_test!(if_nested_else, "!(if False 0 (+ 1 2))", &["3"]);
    eval_test!(if_deeply_nested, "!(if True (if True (if True deep outer) outer2) outer3)", &["deep"]);
    eval_test!(if_lazy_eval_true, "!(if True 1 (/ 1 0))", &["1"]);
    // MeTTa HE: non-boolean conditions return unreduced (if cond then else)
    eval_test!(if_non_bool_number, "!(if 1 yes no)", &["(if 1 yes no)"]);
    eval_test!(if_with_atom_condition, "!(if foo then else)", &["(if foo then else)"]);
    // MeTTa HE: Unit is NOT boolean — returns unreduced (if () then else)
    eval_test!(if_unit_condition_unreduced, "!(if () True False)", &["(if () True False)"]);

    // =========================================================================
    // Let Bindings
    // =========================================================================

    eval_test!(let_simple, "!(let $x 5 $x)", &["5"]);
    eval_test!(let_arithmetic, "!(let $x 10 (+ $x 5))", &["15"]);
    eval_test!(let_nested, "!(let $x 2 (let $y 3 (* $x $y)))", &["6"]);
    eval_test!(let_pattern, "!(let ($a $b) (1 2) (+ $a $b))", &["3"]);
    eval_test!(let_with_computation, "!(let $x (+ 2 3) (* $x 2))", &["10"]);
    eval_test!(let_variable_pattern, "!(let $x 42 $x)", &["42"]);
    eval_test!(let_deeply_nested, "!(let $x 1 (let $y 2 (let $z 3 (+ $x (+ $y $z)))))", &["6"]);
    eval_test!(let_star_sequential, "!(let* (($x 1) ($y (+ $x 1))) $y)", &["2"]);
    eval_test!(let_star_multi, "!(let* (($a 1) ($b 2) ($c (+ $a $b))) $c)", &["3"]);
    eval_test!(let_star_multiple, "!(let* (($x 1) ($y (+ $x 1)) ($z (+ $y 1))) $z)", &["3"]);

    // =========================================================================
    // List Operations
    // =========================================================================

    eval_test!(cons_atom, "!(cons-atom a (b c))", &["(a b c)"]);
    eval_test!(get_head, "!(car-atom (a b c))", &["a"]);
    eval_test!(get_tail, "!(cdr-atom (a b c))", &["(b c)"]);
    eval_test!(list_size, "!(size-atom (a b c))", &["3"]);
    eval_test!(empty_list_size, "!(size-atom ())", &["0"]);
    eval_test!(cons_atom_nil_tail, "!(cons-atom a ())", &["(a)"]);
    eval_test!(car_single_element, "!(car-atom (x))", &["x"]);
    eval_test!(cdr_single_element, "!(cdr-atom (x))", &["()"]);
    eval_test!(size_nested, "!(size-atom ((a b) (c d)))", &["2"]);
    eval_test!(decons_basic, "!(decons-atom (a b c))", &["(a (b c))"]);
    eval_test!(index_first, "!(index-atom (a b c) 0)", &["a"]);
    eval_test!(index_last, "!(index-atom (a b c) 2)", &["c"]);
    eval_test!(index_zero, "!(index-atom (a b c d e) 0)", &["a"]);
    eval_test!(index_middle, "!(index-atom (a b c d e) 2)", &["c"]);
    eval_test!(index_last_5elem, "!(index-atom (a b c d e) 4)", &["e"]);

    // =========================================================================
    // Quote and Eval
    // =========================================================================

    // quote is self-evaluating — preserves the (quote ...) wrapper (HE semantics)
    eval_test!(quote_preserves, "!(quote (+ 1 2))", &["(quote (+ 1 2))"]);
    eval_test!(eval_quoted, "!(eval (quote (+ 1 2)))", &["3"]);
    eval_test!(quote_nested, "!(quote (+ (+ 1 2) 3))", &["(quote (+ (+ 1 2) 3))"]);
    eval_test!(eval_force_quoted, "!(eval (quote (* 6 7)))", &["42"]);
    eval_test!(eval_on_value, "!(eval 42)", &["42"]);
    eval_test!(quote_nested_structure, "!(quote ((+ 1 2) (* 3 4)))", &["(quote ((+ 1 2) (* 3 4)))"]);
    eval_test!(eval_nested_quote, "!(eval (quote (+ 1 (+ 2 3))))", &["6"]);

    // unquote: unwraps Quoted variant without evaluating the inner expression
    eval_test!(unquote_quoted, "!(unquote (quote (+ 1 2)))", &["(+ 1 2)"]);
    eval_test!(unquote_non_quoted_identity, "!(unquote 42)", &["42"]);
    eval_test!(unquote_atom, "!(unquote (quote foo))", &["foo"]);

    // quote + introspection transparency
    eval_test!(get_metatype_quoted, "!(get-metatype (quote foo))", &["Expression"]);

    // =========================================================================
    // Type System
    // =========================================================================

    eval_test!(get_type_long, "!(get-type 42)", &["Number"]);
    eval_test!(get_type_bool, "!(get-type True)", &["Bool"]);
    eval_test!(get_type_string, "!(get-type \"hello\")", &["String"]);
    eval_test!(get_type_symbol, "!(get-type foo)", &["Undefined"]);
    eval_test!(get_type_expr, "!(get-type (a b c))", &["Undefined"]);
    eval_test!(get_type_nil, "!(get-type Nil)", &["Undefined"]);
    eval_test!(metatype_expr, "!(get-metatype (a b c))", &["Expression"]);

    #[test]
    fn metatype_variable() {
        let results = run_eval("!(get-metatype $x)");
        assert!(!results.is_empty(), "get-metatype should return a result");
        // Arena treats variables as symbols at the metatype level
        assert_eq!(results[0], "Symbol");
    }

    // =========================================================================
    // Nondeterminism
    // =========================================================================

    eval_test!(superpose_single, "!(superpose (1))", &["1"]);
    eval_test!(superpose_empty, "!(superpose ())", &[]);
    eval_test!(collapse_single, "!(collapse (superpose (1)))", &["(1)"]);
    eval_test!(collapse_single_elem, "!(collapse (superpose (42)))", &["(42)"]);
    eval_test!(collapse_empty_super, "!(collapse (superpose ()))", &["()"]);

    #[test]
    fn superpose_multiple() {
        let results = run_eval("!(superpose (1 2 3))");
        assert!(!results.is_empty(), "superpose should return at least one result");
        assert_eq!(results[0], "1", "First superpose result should be 1");
    }

    // =========================================================================
    // Case Expressions
    // =========================================================================

    eval_test!(case_basic_match, "!(case a ((a yes) (b no)))", &["yes"]);
    eval_test!(case_second_match, "!(case b ((a yes) (b no)))", &["no"]);
    eval_test!(case_default, "!(case c ((a yes) ($x default)))", &["default"]);
    // MeTTa HE: when no case matches, result is Empty (no results / branch pruned)
    eval_test!(case_no_match, "!(case z ((a 1) (b 2)))", &[] as &[&str]);
    eval_test!(case_wildcard, "!(case z ((a 1) (_ default)))", &["default"]);
    eval_test!(case_multi, "!(case b ((a A) (b B) (c C)))", &["B"]);
    eval_test!(complex_case, "!(case (+ 1 1) ((1 one) (2 two) (3 three) ($x other)))", &["two"]);

    // =========================================================================
    // Error Handling
    // =========================================================================

    eval_test!(error_create, "!(Error test-msg details)", &["(Error details test-msg)"]);
    eval_test!(is_error_normal, "!(is-error 42)", &["False"]);
    eval_test!(is_error_string, "!(is-error \"hello\")", &["False"]);
    eval_test!(is_error_bool, "!(is-error True)", &["False"]);
    eval_test!(is_error_nil, "!(is-error Nil)", &["False"]);
    eval_test!(catch_normal, "!(catch 42 default)", &["42"]);
    eval_test!(catch_complex_default, "!(catch 42 (+ 10 20))", &["42"]);
    eval_test!(catch_no_error, "!(catch (+ 1 2) \"default\")", &["3"]);

    #[test]
    fn division_by_zero() {
        let results = run_eval("!(/ 1 0)");
        assert!(!results.is_empty(), "Division by zero should return a result");
        let first = &results[0];
        assert!(
            first.contains("Error") || first.contains("Division") || first.contains("0"),
            "Division by zero should return error, got: {}",
            first
        );
    }

    #[test]
    fn mod_by_zero() {
        let results = run_eval("!(% 10 0)");
        assert!(!results.is_empty(), "Modulo by zero should return a result");
        let first = &results[0];
        assert!(
            first.contains("Error") || first.contains("zero") || first == "0",
            "Modulo by zero should indicate error, got: {}",
            first
        );
    }

    #[test]
    fn mul_type_mismatch_returns_unreduced() {
        // MeTTa HE semantics: type mismatch returns unreduced expression, not error
        let results = run_eval("!(* True 5)");
        assert!(!results.is_empty(), "Type mismatch should return a result");
        assert!(!results[0].contains("Error"),
            "Type mismatch should return unreduced expression, not error. Got: {}", results[0]);
        assert!(results[0].contains("*"), "Should contain the operator");
    }

    #[test]
    fn sub_type_mismatch_returns_unreduced() {
        // MeTTa HE semantics: type mismatch returns unreduced expression, not error
        let results = run_eval("!(- \"hello\" 2)");
        assert!(!results.is_empty(), "Type mismatch should return a result");
        assert!(!results[0].contains("Error"),
            "Type mismatch should return unreduced expression, not error. Got: {}", results[0]);
        assert!(results[0].contains("-"), "Should contain the operator");
    }

    #[test]
    fn div_type_mismatch_returns_unreduced() {
        // MeTTa HE semantics: type mismatch returns unreduced expression, not error
        let results = run_eval("!(/ 10 \"x\")");
        assert!(!results.is_empty(), "Type mismatch should return a result");
        assert!(!results[0].contains("Error"),
            "Type mismatch should return unreduced expression, not error. Got: {}", results[0]);
        assert!(results[0].contains("/"), "Should contain the operator");
    }

    #[test]
    fn error_direct() {
        let results = run_eval("!(Error TestError \"test message\")");
        assert!(!results.is_empty(), "Error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
        assert!(results[0].contains("TestError"), "Should contain error type");
    }

    #[test]
    fn error_if_then_branch() {
        let results = run_eval("!(if True (/ 1 0) ok)");
        assert!(!results.is_empty(), "If with error in then should return result");
        assert!(
            results[0].contains("Error") || results[0].contains("Division"),
            "Then branch error should propagate: {}",
            results[0]
        );
    }

    eval_test!(lazy_if_true_else_not_eval, "!(if True ok (/ 1 0))", &["ok"]);
    eval_test!(lazy_if_false_else_ok, "!(if False error (+ 1 2))", &["3"]);

    // =========================================================================
    // Map/Filter/Fold
    // =========================================================================

    eval_test!(map_empty, "!(map-atom () $x (+ $x 1))", &["()"]);
    eval_test!(map_single, "!(map-atom (1) $x (+ $x 10))", &["(11)"]);
    eval_test!(map_double, "!(map-atom (1 2 3) $x (* $x 2))", &["(2 4 6)"]);
    eval_test!(map_identity, "!(map-atom (1 2 3) $x $x)", &["(1 2 3)"]);
    eval_test!(nested_map, "!(map-atom ((1 2) (3 4)) $lst (car-atom $lst))", &["(1 3)"]);
    eval_test!(filter_all_pass, "!(filter-atom (1 2 3) $x True)", &["(1 2 3)"]);
    eval_test!(filter_all_fail, "!(filter-atom (1 2 3) $x False)", &["()"]);
    eval_test!(filter_some_pass, "!(filter-atom (1 2 3 4) $x (< $x 3))", &["(1 2)"]);
    eval_test!(foldl_sum, "!(foldl-atom (1 2 3 4) 0 $acc $x (+ $acc $x))", &["10"]);
    eval_test!(foldl_empty, "!(foldl-atom () 42 $acc $x (+ $acc $x))", &["42"]);
    eval_test!(foldl_product, "!(foldl-atom (1 2 3 4) 1 $acc $x (* $acc $x))", &["24"]);
    eval_test!(foldl_concat, "!(foldl-atom (a b c) () $acc $x (cons-atom $x $acc))", &["(c b a)"]);

    // =========================================================================
    // Chain Expressions
    // =========================================================================

    eval_test!(chain_basic, "!(chain (superpose (1 2)) $x (+ $x 10))", &["11", "12"]);
    eval_test!(chain_single, "!(chain (+ 1 2) $x (* $x 10))", &["30"]);
    eval_test!(chain_identity, "!(chain 42 $x $x)", &["42"]);
    eval_test!(chain_arithmetic, "!(chain (+ 1 2) $x (* $x 10))", &["30"]);
    eval_test!(chain_superpose, "!(collapse (chain (superpose (1 2 3)) $x (+ $x 100)))", &["(101 102 103)"]);
    eval_test!(chain_with_let, "!(chain 5 $x (let $y 10 (+ $x $y)))", &["15"]);

    #[test]
    fn chain_empty() {
        // MeTTa HE: chain over empty result produces zero results (branch annihilation)
        let results = run_eval("!(chain (superpose ()) $x (+ $x 1))");
        assert!(results.is_empty(), "chain with empty expr should produce zero results, got: {:?}", results);
    }

    // =========================================================================
    // Rules
    // =========================================================================

    eval_test!(
        rule_simple,
        "(= (double $x) (* 2 $x))\n!(double 5)",
        &["10"]
    );

    // Uses if-guard instead of overlapping base-case pattern because without a specificity
    // filter, both `(fact 0)` and `(fact $n)` match at n=0, causing divergence in the
    // recursive branch. MeTTa HE fires all matching rules nondeterministically.
    eval_test!(
        rule_recursive_factorial,
        "(= (fact $n) (if (== $n 0) 1 (* $n (fact (- $n 1)))))\n!(fact 5)",
        &["120"]
    );

    // Without specificity filter, both `(f 0)` and `(f $x)` match input `(f 0)`.
    // MeTTa HE fires all matching rules nondeterministically.
    eval_test_unordered!(
        rule_multiple_patterns,
        "(= (f 0) zero)\n(= (f $x) other)\n!(f 0)",
        &["zero", "other"]
    );

    eval_test!(
        rule_multi_arg,
        "(= (add $a $b) (+ $a $b))\n!(add 3 4)",
        &["7"]
    );

    eval_test!(
        recursive_factorial_10,
        "(= (fact $n) (if (== $n 0) 1 (* $n (fact (- $n 1)))))
         !(fact 10)",
        &["3628800"]
    );

    eval_test!(
        recursive_fib,
        "(= (fib $n) (if (== $n 0) 0 (if (== $n 1) 1 (+ (fib (- $n 1)) (fib (- $n 2))))))
         !(fib 10)",
        &["55"]
    );

    eval_test!(
        mutual_recursion,
        "(= (even $n) (if (== $n 0) True (odd (- $n 1))))
         (= (odd $n) (if (== $n 0) False (even (- $n 1))))
         !(even 4)",
        &["True"]
    );

    eval_test!(
        pattern_guard,
        "(= (safe-div $x 0) (Error \"division by zero\" $x))
         (= (safe-div $x $y) (/ $x $y))
         !(safe-div 10 2)",
        &["5"]
    );

    // Without specificity filter, both `(classify 0)` and `(classify $n)` match at input 0.
    // MeTTa HE fires all matching rules nondeterministically.
    eval_test_unordered!(
        overlapping_patterns,
        "(= (classify 0) zero)
         (= (classify $n) positive)
         !(classify 0)",
        &["zero", "positive"]
    );

    eval_test!(
        nondet_rule,
        "(= (choice) a)
         (= (choice) b)
         (= (choice) c)
         !(collapse (choice))",
        &["(a b c)"]
    );

    // =========================================================================
    // PLN Regression: Overlapping nested patterns (specificity filter removal)
    // =========================================================================

    // Exact PLN reproduction: nested 3-element pattern vs variable-only pattern.
    // Without the specificity filter, both rules fire nondeterministically.
    // The specific rule produces a result; the general rule returns (empty)
    // which produces zero results (branch annihilation per MeTTa HE).
    // Only the specific rule's result survives.
    eval_test!(
        pln_nested_overlap_both_fire,
        "(= (f ((tag $a $b) $tv) $y) (result-specific $a $b))
         (= (f ($c $tv) $y) (empty))
         !(f ((tag hello world) (stv 1)) 2)",
        &["(result-specific hello world)"]
    );

    // Constructor-discriminated rules: no overlap since `Nil` != `(Cons ...)`.
    // Regression test ensuring mmverify-style patterns still work correctly.
    eval_test!(
        constructor_discriminated_no_overlap,
        "(= (len Nil) 0)
         (= (len (Cons $h $t)) (+ 1 (len $t)))
         !(len (Cons a (Cons b Nil)))",
        &["2"]
    );

    // =========================================================================
    // Unify and Switch
    // =========================================================================

    eval_test!(unify_success, "!(unify $x 42 $x fail)", &["42"]);
    eval_test!(unify_failure, "!(unify 1 2 success fail)", &["fail"]);
    eval_test!(unify_pattern, "!(unify ($a $b) (1 2) (+ $a $b) 0)", &["3"]);
    eval_test!(unify_same_values, "!(unify 42 42 success fail)", &["success"]);
    eval_test!(unify_different_values, "!(unify 1 2 success fail)", &["fail"]);
    eval_test!(unify_sexpr_matching, "!(unify (a b c) (a b c) matched not-matched)", &["matched"]);
    eval_test!(unify_sexpr_not_matching, "!(unify (a b c) (a b d) matched not-matched)", &["not-matched"]);
    eval_test!(switch_basic, "!(switch foo ((foo 1) (bar 2)))", &["1"]);
    eval_test!(switch_second, "!(switch bar ((foo 1) (bar 2)))", &["2"]);
    eval_test!(switch_default, "!(switch xyz ((foo 1) ($x 99)))", &["99"]);

    // =========================================================================
    // Boolean Short-Circuit
    // =========================================================================

    eval_test!(and_short_circuit, "!(and False (/ 1 0))", &["False"]);
    eval_test!(or_short_circuit, "!(or True (/ 1 0))", &["True"]);

    // =========================================================================
    // String Equality
    // =========================================================================

    eval_test!(string_eq, "!(== \"a\" \"a\")", &["True"]);
    eval_test!(string_neq, "!(== \"a\" \"b\")", &["False"]);
    eval_test!(string_hello_eq, "!(== \"hello\" \"hello\")", &["True"]);
    eval_test!(string_hello_neq, "!(== \"hello\" \"world\")", &["False"]);

    // =========================================================================
    // Arithmetic Edge Cases
    // =========================================================================

    eval_test!(mul_by_zero, "!(* 1000000 0)", &["0"]);
    eval_test!(add_zero, "!(+ 42 0)", &["42"]);
    eval_test!(mod_positive, "!(% 7 3)", &["1"]);
    eval_test!(mod_negative, "!(% -7 3)", &["-1"]);
    eval_test!(div_exact, "!(/ 10 2)", &["5"]);
    eval_test!(mod_value, "!(% 10 3)", &["1"]);
    eval_test!(math_negative, "!(* -3 4)", &["-12"]);

    // =========================================================================
    // Empty / Nil
    // =========================================================================

    eval_test!(empty_sexpr, "!()", &["()"]);
    eval_test!(nil_comparison, "!(== Nil Nil)", &["True"]);

    // MeTTa HE: (empty) produces zero results (branch annihilation)
    eval_test!(empty_produces_zero_results, "!(empty)", &[]);

    // /safe division with zero divisor → branch annihilation via (empty)
    eval_test!(
        safe_div_zero_annihilation,
        "(= (/safe $A $B) (if (> $B 0.0) (/ $A $B) (empty)))
         !(/safe 1.0 0.0)",
        &[]
    );

    // /safe division with valid divisor → normal result
    eval_test!(
        safe_div_valid,
        "(= (/safe $A $B) (if (> $B 0.0) (/ $A $B) (empty)))
         !(/safe 1.0 0.5)",
        &["2"]
    );

    // Arithmetic with empty-producing arg → zero results (branch annihilation)
    eval_test!(
        add_empty_annihilation,
        "(= (/safe $A $B) (if (> $B 0.0) (/ $A $B) (empty)))
         !(+ (/safe 1.0 0.0) 5.0)",
        &[]
    );

    // Nested arithmetic with empty-producing branch → zero results
    eval_test!(
        nested_arith_empty_annihilation,
        "(= (/safe $A $B) (if (> $B 0.0) (/ $A $B) (empty)))
         !(* 3.0 (+ (/safe 1.0 0.0) 2.0))",
        &[]
    );

    // =========================================================================
    // Equality and Identity
    // =========================================================================

    eval_test!(eq_atoms, "!(== foo foo)", &["True"]);
    eval_test!(eq_different_atoms, "!(== foo bar)", &["False"]);
    eval_test!(eq_sexpr, "!(== (a b) (a b))", &["True"]);
    eval_test!(eq_sexpr_different, "!(== (a b) (a c))", &["False"]);

    // =========================================================================
    // Complex Patterns
    // =========================================================================

    eval_test!(
        pattern_wildcard,
        "(= (first (_ $x)) $x)\n!(first (1 2))",
        &["2"]
    );

    eval_test!(
        pattern_multi_var,
        "(= (swap ($a $b)) ($b $a))\n!(swap (1 2))",
        &["(2 1)"]
    );

    eval_test!(
        pattern_nested_structure,
        "(= (flatten (($x $y) $z)) ($x $y $z))\n!(flatten ((a b) c))",
        &["(a b c)"]
    );

    eval_test!(
        pattern_const_and_var,
        "(= (extract-value (pair $x $y)) $y)\n!(extract-value (pair name John))",
        &["John"]
    );

    // =========================================================================
    // S-Expression Mixed Types
    // =========================================================================

    eval_test!(
        sexpr_mixed_types,
        "!(cons-atom 1 (True \"hello\" 3.14))",
        &["(1 True \"hello\" 3.14)"]
    );

    eval_test!(sexpr_index_atom, "!(index-atom (a b c d e) 2)", &["c"]);
    eval_test!(sexpr_index_first, "!(index-atom (x y z) 0)", &["x"]);
    eval_test!(sexpr_index_last, "!(index-atom (1 2 3 4) 3)", &["4"]);

    // =========================================================================
    // Function Application
    // =========================================================================

    eval_test!(
        lambda_identity,
        "(= (id $x) $x)\n!(id 42)",
        &["42"]
    );

    eval_test!(
        hof_apply_twice,
        "(= (apply-twice $f $x) ($f ($f $x)))\n(= (inc $n) (+ $n 1))\n!(apply-twice inc 0)",
        &["2"]
    );

    eval_test!(
        switch_pattern,
        "(= (handle ok) success)\n(= (handle error) failure)\n!(handle ok)",
        &["success"]
    );

    // =========================================================================
    // Deeply Nested Expressions
    // =========================================================================

    eval_test!(
        deeply_nested_let,
        "!(let $a 1 (let $b 2 (let $c 3 (let $d 4 (+ $a (+ $b (+ $c $d)))))))",
        &["10"]
    );

    // =========================================================================
    // Function/Return
    // =========================================================================

    #[test]
    fn function_no_return() {
        let results = run_eval("!(function (+ 1 2))");
        assert!(!results.is_empty(), "Function should return result");
        assert!(
            results[0] == "3" || results[0].contains("Error"),
            "Should return 3 or error: {}",
            results[0]
        );
    }

    // =========================================================================
    // Property-Based Tests
    // =========================================================================

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(50))]

            #[test]
            fn prop_arithmetic_add(a in -1000i64..1000, b in -1000i64..1000) {
                let src = format!("!(+ {} {})", a, b);
                let results = run_eval(&src);
                let expected = format!("{}", a + b);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_comparison_lt(a in -100i64..100, b in -100i64..100) {
                let src = format!("!(< {} {})", a, b);
                let results = run_eval(&src);
                let expected = if a < b { "True" } else { "False" };
                prop_assert_eq!(results[0].as_str(), expected);
            }

            #[test]
            fn prop_let_binding(val in 0i64..100) {
                let src = format!("!(let $x {} (+ $x 1))", val);
                let results = run_eval(&src);
                let expected = format!("{}", val + 1);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_nested_arithmetic(a in 1i64..50, b in 1i64..50, c in 1i64..50) {
                let src = format!("!(+ {} (+ {} {}))", a, b, c);
                let results = run_eval(&src);
                let expected = format!("{}", a + b + c);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_boolean_and(a: bool, b: bool) {
                let src = format!("!(and {} {})",
                    if a { "True" } else { "False" },
                    if b { "True" } else { "False" });
                let results = run_eval(&src);
                let expected = if a && b { "True" } else { "False" };
                prop_assert_eq!(results[0].as_str(), expected);
            }

            #[test]
            fn prop_mul(a in -50i64..50, b in -50i64..50) {
                let src = format!("!(* {} {})", a, b);
                let results = run_eval(&src);
                let expected = format!("{}", a * b);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_div(a in -100i64..100, b in 1i64..50) {
                let src = format!("!(/ {} {})", a, b);
                let results = run_eval(&src);
                let expected = format!("{}", a / b);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_if_then_else(cond in prop::bool::ANY, then_val in 0i64..100, else_val in 0i64..100) {
                let cond_str = if cond { "True" } else { "False" };
                let src = format!("!(if {} {} {})", cond_str, then_val, else_val);
                let results = run_eval(&src);
                let expected = if cond { format!("{}", then_val) } else { format!("{}", else_val) };
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_nested_let(a in 1i64..50, b in 1i64..50) {
                let src = format!("!(let $x {} (let $y {} (+ $x $y)))", a, b);
                let results = run_eval(&src);
                let expected = format!("{}", a + b);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_if_computed(a in -50i64..50, b in -50i64..50) {
                let src = format!("!(if (< {} {}) less greater-or-equal)", a, b);
                let results = run_eval(&src);
                let expected = if a < b { "less" } else { "greater-or-equal" };
                prop_assert_eq!(results[0].as_str(), expected);
            }

            #[test]
            fn prop_let_star(a in 1i64..20, b in 1i64..20) {
                let src = format!("!(let* (($x {}) ($y (+ $x {}))) (* $x $y))", a, b);
                let results = run_eval(&src);
                let expected = format!("{}", a * (a + b));
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_deep_if(a: bool, b: bool, c: bool) {
                let cond = |x: bool| if x { "True" } else { "False" };
                let src = format!("!(if {} (if {} (if {} 1 2) 3) 4)", cond(a), cond(b), cond(c));
                let results = run_eval(&src);
                let expected = if a { if b { if c { "1" } else { "2" } } else { "3" } } else { "4" };
                prop_assert_eq!(results[0].as_str(), expected);
            }

            #[test]
            fn prop_rule_application(val in 1i64..100) {
                let src = format!("(= (double $x) (* 2 $x))\n!(double {})", val);
                let results = run_eval(&src);
                let expected = format!("{}", val * 2);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_quote_eval_roundtrip(a in 1i64..100, b in 1i64..100) {
                let src = format!("!(eval (quote (+ {} {})))", a, b);
                let results = run_eval(&src);
                let expected = format!("{}", a + b);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_case_atom(val in 0usize..3) {
                let atoms = ["a", "b", "c"];
                let atom = atoms[val];
                let src = format!("!(case {} ((a 1) (b 2) (c 3)))", atom);
                let results = run_eval(&src);
                let expected = format!("{}", val + 1);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }

            #[test]
            fn prop_foldl_sum(init in 0i64..100) {
                let src = format!("!(foldl-atom (1 2 3 4 5) {} $acc $x (+ $acc $x))", init);
                let results = run_eval(&src);
                let expected = format!("{}", init + 15);
                prop_assert_eq!(results[0].as_str(), expected.as_str());
            }
        }
    }

    // ========================================================================
    // Multi-Tier Type-Driven Applicative Evaluation Tests
    //
    // These tests verify that type-driven applicative evaluation (MeTTa HE
    // parity) works correctly across all execution tiers:
    //   Tier 0: Tree-walker interpreter (eval_trampoline)
    //   Tier 1: Bytecode VM (2+ executions)
    //   Tier 2-3: JIT Stages 1/2 (100+/500+ executions)
    //
    // Each test covers a specific scenario and is run through `eval()` which
    // dispatches to the highest available tier based on execution count.
    // ========================================================================

    /// Helper to run evaluation through the tiered `eval()` function
    /// (bytecode VM / JIT promotion), collecting results from the last
    /// force-eval expression.
    fn run_eval_tiered(src: &str) -> Vec<String> {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;
        use crate::backend::eval::trampoline::new_env;

        let state = compile(src).expect("compile failed");
        let mut env = new_env();
        let mut all_results = Vec::new();

        let source_exprs: Vec<_> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (results, new_env) = eval(expr, env, &state);
            env = new_env;
            for result in &results {
                all_results.push(result.to_string());
            }
        }
        all_results
    }

    // --- Data Constructors (NOT pre-evaluated) ---

    eval_test!(
        test_data_constructor_not_preevaluated,
        r#"
            (= (next Z) (S Z))
            (= (next (S $n)) (S (S $n)))
            !(next Z)
            !(next (S Z))
            !(next (S (S Z)))
        "#,
        &["(S Z)", "(S (S Z))", "(S (S (S Z)))"]
    );

    eval_test!(
        test_nested_data_constructors,
        r#"
            (= (add Z $n) $n)
            (= (add (S $m) $n) (S (add $m $n)))
            !(add (S Z) (S Z))
            !(add (S (S Z)) (S Z))
        "#,
        &["(S (S Z))", "(S (S (S Z)))"]
    );

    eval_test!(
        test_data_constructor_in_match_result,
        // (Pair 1 2) is a bare fact — returned as-is (unreduced).
        // The match query then finds it in space and swaps the elements.
        r#"
            (Pair 1 2)
            !(match &self (Pair $a $b) (Pair $b $a))
        "#,
        &["(Pair 1 2)", "(Pair 2 1)"]
    );

    // --- Typed Functions with Arrow Types (SHOULD be pre-evaluated) ---

    eval_test!(
        test_typed_function_preevals_args,
        r#"
            (: double (-> Number Number))
            (= (double $x) (+ $x $x))
            !(double (+ 1 2))
        "#,
        &["6"]
    );

    eval_test!(
        test_typed_function_nested_preevals,
        r#"
            (: square (-> Number Number))
            (= (square $x) (* $x $x))
            !(square (+ 2 3))
        "#,
        &["25"]
    );

    eval_test!(
        test_typed_binary_function,
        r#"
            (: my-add (-> Number Number Number))
            (= (my-add $a $b) (+ $a $b))
            !(my-add (+ 1 2) (+ 3 4))
        "#,
        &["10"]
    );

    // --- Meta-Typed Arguments (NOT pre-evaluated) ---

    eval_test!(
        test_meta_type_expression_not_preevaluated,
        // Type-driven dispatch: my-quote has (-> Expression Expression),
        // so `(+ 1 2)` is NOT pre-evaluated at the my-quote call site.
        // However, the rule body `(quoted $e)` instantiates to `(quoted (+ 1 2))`,
        // and then the tuple path evaluates sub-elements: `(+ 1 2)` → `3`.
        // To preserve unevaluated exprs, use `quote`: `(quoted (quote $e))`.
        // This behavior matches MeTTa HE's interpret_tuple path.
        r#"
            (: my-quote (-> Expression Expression))
            (= (my-quote $e) (quoted $e))
            !(my-quote (+ 1 2))
        "#,
        &["(quoted 3)"]
    );

    eval_test!(
        test_meta_type_atom_not_preevaluated,
        r#"
            (: wrap-atom (-> Atom Atom))
            (= (wrap-atom $a) (wrapped $a))
            !(wrap-atom hello)
        "#,
        &["(wrapped hello)"]
    );

    eval_test!(
        test_meta_type_prevents_bloom_filter_preeval,
        // Verify that when a typed function has Expression-typed args,
        // the bloom filter does NOT pre-evaluate those args even if the
        // arg's head has rules. Without the type system, `(f)` would be
        // pre-evaluated to {1,2,3} by the bloom filter. With the type
        // system marking the arg as Expression, `(f)` is passed unevaluated.
        // The rule body `(head $e)` captures `$e = (f)` and wraps it.
        // Then the tuple path evaluates `(f)` inside `(head (f))` → {1,2,3}.
        r#"
            (= (f) 1)
            (= (f) 2)
            (= (f) 3)
            (: wrap-expr (-> Expression Expression))
            (= (wrap-expr $e) (head $e))
            !(wrap-expr (f))
        "#,
        &["(head 1)", "(head 2)", "(head 3)"]
    );

    eval_test!(
        test_mixed_meta_and_value_types,
        r#"
            (: apply-to (-> (-> Number Number) Number Number))
            (= (apply-to $f $x) ($f $x))
            (: inc (-> Number Number))
            (= (inc $n) (+ $n 1))
            !(apply-to inc (+ 2 3))
        "#,
        &["6"]
    );

    // --- Fixpoint Detection (bloom filter false positives) ---

    eval_test!(
        test_fixpoint_data_constructor_no_infinite_loop,
        // S has facts but NO rewrite rules for arity 1 as a head.
        // Bloom filter may flag it → pre-eval → fixpoint → data.
        // Facts are returned as-is (unreduced), then match finds them.
        // match (number (S $x)) with (number (S Z)) → $x=Z → template (S $x) = (S Z)
        // match (number (S $x)) with (number (S (S Z))) → $x=(S Z) → template (S $x) = (S (S Z))
        r#"
            (number (S Z))
            (number (S (S Z)))
            !(match &self (number (S $x)) (S $x))
        "#,
        &["(number (S Z))", "(number (S (S Z)))", "(S (S Z))", "(S Z)"]
    );

    eval_test!(
        test_fixpoint_peano_to_int,
        // to-int uses (+ 1 (to-int $n)) — + is grounded, but (to-int $n)
        // is a user function. The bloom filter catches (to-int ...) for
        // pre-eval. After one round, rules match and evaluation completes.
        r#"
            (= (to-int Z) 0)
            (= (to-int (S $n)) (+ 1 (to-int $n)))
            !(to-int (S (S (S Z))))
        "#,
        &["3"]
    );

    // --- Nondeterministic Applicative Evaluation ---

    eval_test_unordered!(
        test_nondet_applicative_3_results,
        r#"
            (= (f) 1)
            (= (f) 2)
            (= (f) 3)
            (= (g $x) (* $x $x))
            !(g (f))
        "#,
        &["1", "4", "9"]
    );

    eval_test_unordered!(
        test_nondet_cartesian_product,
        r#"
            (= (a) 1)
            (= (a) 2)
            (= (b) 10)
            (= (b) 20)
            !(+ (a) (b))
        "#,
        &["11", "21", "12", "22"]
    );

    // --- Tiered Execution Tests ---
    // These use `run_eval_tiered` which goes through `eval()` and can
    // trigger bytecode VM / JIT promotion for repeated expressions.

    #[test]
    fn test_tiered_data_constructor() {
        let results = run_eval_tiered(r#"
            (= (next Z) (S Z))
            (= (next (S $n)) (S (S $n)))
            !(next Z)
            !(next (S Z))
        "#);
        assert_eq!(results, vec!["(S Z)", "(S (S Z))"]);
    }

    #[test]
    fn test_tiered_typed_function() {
        let results = run_eval_tiered(r#"
            (: double (-> Number Number))
            (= (double $x) (+ $x $x))
            !(double (+ 1 2))
            !(double (+ 3 4))
        "#);
        assert_eq!(results, vec!["6", "14"]);
    }

    #[test]
    fn test_tiered_meta_type_preservation() {
        // Same as test_meta_type_expression_not_preevaluated but through
        // the tiered eval() path. The tuple path evaluates sub-elements,
        // so `(quoted (+ 1 2))` → `(quoted 3)` per MeTTa HE semantics.
        let results = run_eval_tiered(r#"
            (: my-quote (-> Expression Expression))
            (= (my-quote $e) (quoted $e))
            !(my-quote (+ 1 2))
        "#);
        assert_eq!(results, vec!["(quoted 3)"]);
    }

    #[test]
    fn test_tiered_nondet_applicative() {
        let mut results = run_eval_tiered(r#"
            (= (f) 1)
            (= (f) 2)
            (= (f) 3)
            (= (g $x) (* $x $x))
            !(g (f))
        "#);
        results.sort();
        assert_eq!(results, vec!["1", "4", "9"]);
    }

    #[test]
    fn test_tiered_peano_arithmetic() {
        let results = run_eval_tiered(r#"
            (= (add Z $n) $n)
            (= (add (S $m) $n) (S (add $m $n)))
            !(add (S Z) (S Z))
            !(add (S (S Z)) (S (S Z)))
        "#);
        assert_eq!(results, vec!["(S (S Z))", "(S (S (S (S Z))))"]);
    }

    #[test]
    fn test_tiered_guarded_recursion() {
        // fib with guard to prevent divergence from overlapping rules
        let results = run_eval_tiered(r#"
            (= (fib $n) (if (== $n 0) 1 (if (== $n 1) 1 (+ (fib (- $n 1)) (fib (- $n 2))))))
            !(fib 0)
            !(fib 1)
            !(fib 5)
        "#);
        assert_eq!(results, vec!["1", "1", "8"]);
    }

    #[test]
    fn test_tiered_to_int_conversion() {
        let results = run_eval_tiered(r#"
            (= (to-int Z) 0)
            (= (to-int (S $n)) (+ 1 (to-int $n)))
            !(to-int Z)
            !(to-int (S Z))
            !(to-int (S (S (S Z))))
        "#);
        assert_eq!(results, vec!["0", "1", "3"]);
    }

    // =========================================================================
    // if-reducible — tree-walker (eval_test! uses run_eval → eval_trampoline)
    // =========================================================================

    // Reducible expression: (+ 1 2) evaluates to 3 (differs from original)
    eval_test!(if_reducible_reduces, "!(if-reducible (+ 1 2) True False)", &["True"]);

    // Irreducible atom: foo has no rules, evaluates to itself
    eval_test!(if_reducible_irreducible_atom, "!(if-reducible foo True False)", &["False"]);

    // Irreducible variable: $x has no binding, stays as-is
    eval_test!(if_reducible_irreducible_var, "!(if-reducible $x True False)", &["False"]);

    // Nested reducible: inner reduction makes it reducible
    eval_test!(if_reducible_nested, "!(if-reducible (+ (* 2 3) 1) reduced not-reduced)", &["reduced"]);

    // Then-branch is evaluated (not just returned as data)
    eval_test!(if_reducible_then_eval, "!(if-reducible (+ 1 1) (+ 10 20) fallback)", &["30"]);

    // Else-branch is evaluated when irreducible
    eval_test!(if_reducible_else_eval, "!(if-reducible foo (+ 10 20) (+ 3 4))", &["7"]);

    // User-defined rule makes expression reducible
    eval_test!(if_reducible_user_rule,
        r#"
            (= (double $x) (* $x 2))
            !(if-reducible (double 5) yes no)
        "#,
        &["yes"]
    );

    // Expression with no matching rule is irreducible
    eval_test!(if_reducible_no_rule,
        r#"
            (= (double $x) (* $x 2))
            !(if-reducible (triple 5) yes no)
        "#,
        &["no"]
    );

    // S-expression that is irreducible (no head rule)
    eval_test!(if_reducible_sexpr_irreducible, "!(if-reducible (unknown-fn 1 2) yes no)", &["no"]);

    // Boolean result from comparison is reducible
    eval_test!(if_reducible_comparison, "!(if-reducible (== 1 1) yes no)", &["yes"]);

    // =========================================================================
    // if-reducible arity errors — tree-walker
    // =========================================================================

    #[test]
    fn if_reducible_arity_too_few() {
        let results = run_eval("!(if-reducible foo True)");
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("if-reducible requires exactly 3 arguments"));
    }

    #[test]
    fn if_reducible_arity_too_many() {
        let results = run_eval("!(if-reducible foo True False extra)");
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("if-reducible requires exactly 3 arguments"));
    }

    // =========================================================================
    // match-or — tree-walker
    // =========================================================================

    // Match found in &self space
    // Note: run_eval captures results from ALL expressions; (A B) as a fact returns (A B)
    eval_test!(match_or_found,
        r#"
            (A B)
            !(match-or &self (A $x) default-val $x)
        "#,
        &["(A B)", "B"]
    );

    // No match → default value returned
    eval_test!(match_or_no_match,
        r#"
            (A B)
            !(match-or &self (C $x) default-val $x)
        "#,
        &["(A B)", "default-val"]
    );

    // Default is evaluated (not just returned as data)
    eval_test!(match_or_default_eval,
        r#"
            (A B)
            !(match-or &self (C $x) (+ 1 2) $x)
        "#,
        &["(A B)", "3"]
    );

    // Multiple matches: all returned (match-or is nondeterministic when matches exist)
    eval_test_unordered!(match_or_multiple_matches,
        r#"
            (color red)
            (color blue)
            !(match-or &self (color $x) no-color $x)
        "#,
        &["(color blue)", "(color red)", "blue", "red"]
    );

    // Match with complex template
    eval_test!(match_or_complex_template,
        r#"
            (pair 3 4)
            !(match-or &self (pair $a $b) 0 (+ $a $b))
        "#,
        &["(pair 3 4)", "7"]
    );

    // Named space (owned) — bind! and add-atom each return ()
    eval_test!(match_or_named_space,
        r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (fact $x) unknown $x)
        "#,
        &["()", "()", "42"]
    );

    // Named space with no match → default
    eval_test!(match_or_named_space_default,
        r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (other $x) not-found $x)
        "#,
        &["()", "()", "not-found"]
    );

    // =========================================================================
    // match-or arity errors — tree-walker
    // =========================================================================

    #[test]
    fn match_or_arity_too_few() {
        let results = run_eval("!(match-or &self (A $x) default)");
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("match-or requires exactly 4 arguments"));
    }

    #[test]
    fn match_or_arity_too_many() {
        let results = run_eval("!(match-or &self (A $x) default $x extra)");
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("match-or requires exactly 4 arguments"));
    }

    // =========================================================================
    // if-reducible — tiered (bytecode VM + JIT fallback)
    // =========================================================================

    #[test]
    fn test_tiered_if_reducible_reduces() {
        let results = run_eval_tiered("!(if-reducible (+ 1 2) True False)");
        assert_eq!(results, vec!["True"]);
    }

    #[test]
    fn test_tiered_if_reducible_irreducible() {
        let results = run_eval_tiered("!(if-reducible foo True False)");
        assert_eq!(results, vec!["False"]);
    }

    #[test]
    fn test_tiered_if_reducible_user_rule() {
        let results = run_eval_tiered(r#"
            (= (double $x) (* $x 2))
            !(if-reducible (double 5) yes no)
        "#);
        assert_eq!(results, vec!["yes"]);
    }

    #[test]
    fn test_tiered_if_reducible_no_rule() {
        let results = run_eval_tiered(r#"
            (= (double $x) (* $x 2))
            !(if-reducible (triple 5) yes no)
        "#);
        assert_eq!(results, vec!["no"]);
    }

    #[test]
    fn test_tiered_if_reducible_then_eval() {
        let results = run_eval_tiered("!(if-reducible (+ 1 1) (+ 10 20) fallback)");
        assert_eq!(results, vec!["30"]);
    }

    #[test]
    fn test_tiered_if_reducible_else_eval() {
        let results = run_eval_tiered("!(if-reducible foo (+ 10 20) (+ 3 4))");
        assert_eq!(results, vec!["7"]);
    }

    // =========================================================================
    // match-or — tiered (bytecode VM + JIT fallback)
    // =========================================================================

    #[test]
    fn test_tiered_match_or_found() {
        let results = run_eval_tiered(r#"
            (A B)
            !(match-or &self (A $x) default-val $x)
        "#);
        // (A B) is a fact, returned as data; match-or finds it and returns B
        assert_eq!(results, vec!["(A B)", "B"]);
    }

    #[test]
    fn test_tiered_match_or_no_match() {
        let results = run_eval_tiered(r#"
            (A B)
            !(match-or &self (C $x) default-val $x)
        "#);
        assert_eq!(results, vec!["(A B)", "default-val"]);
    }

    #[test]
    fn test_tiered_match_or_default_eval() {
        let results = run_eval_tiered(r#"
            (A B)
            !(match-or &self (C $x) (+ 1 2) $x)
        "#);
        assert_eq!(results, vec!["(A B)", "3"]);
    }

    #[test]
    fn test_tiered_match_or_multiple() {
        let mut results = run_eval_tiered(r#"
            (color red)
            (color blue)
            !(match-or &self (color $x) no-color $x)
        "#);
        results.sort();
        assert_eq!(results, vec!["(color blue)", "(color red)", "blue", "red"]);
    }

    #[test]
    fn test_tiered_match_or_named_space() {
        let results = run_eval_tiered(r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (fact $x) unknown $x)
        "#);
        // bind! and add-atom each return ()
        assert_eq!(results, vec!["()", "()", "42"]);
    }

    #[test]
    fn test_tiered_match_or_named_space_default() {
        let results = run_eval_tiered(r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (other $x) not-found $x)
        "#);
        assert_eq!(results, vec!["()", "()", "not-found"]);
    }

    // =========================================================================
    // if-reducible + match-or combined usage
    // =========================================================================

    eval_test!(if_reducible_with_match_or,
        r#"
            (color red)
            !(if-reducible (+ 1 2) (match-or &self (color $x) none $x) fallback)
        "#,
        &["(color red)", "red"]
    );

    eval_test!(match_or_with_if_reducible_default,
        r#"
            (A B)
            !(match-or &self (missing $x) (if-reducible (+ 1 1) computed-default raw-default) $x)
        "#,
        &["(A B)", "computed-default"]
    );
}
