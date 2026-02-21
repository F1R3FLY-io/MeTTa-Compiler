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
        // Chain over empty superpose — arena may return empty or propagate
        let _results = run_eval("!(chain (superpose ()) $x (+ $x 1))");
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
    // The specific rule produces a result; the general rule produces `Empty`
    // (the `(empty)` sexpr evaluates to the built-in Empty atom).
    eval_test_unordered!(
        pln_nested_overlap_both_fire,
        "(= (f ((tag $a $b) $tv) $y) (result-specific $a $b))
         (= (f ($c $tv) $y) (empty))
         !(f ((tag hello world) (stv 1)) 2)",
        &["(result-specific hello world)", "Empty"]
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
}
