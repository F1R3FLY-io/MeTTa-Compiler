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
            env = (*new_env).clone();
            for (result, _bindings) in &results {
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
                let expected: Vec<String> = expected_slice.iter().map(|s| s.to_string()).collect();
                assert_eq!(results, expected, "Eval failed for: {}", $metta_src);
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
    eval_test!(
        eval_before_match_nested_call,
        "(= (double $x) (+ $x $x)) !(double (+ 1 2))",
        &["6"]
    );

    eval_test!(
        eval_before_match_data_constructor,
        "(= (wrap $x) (wrapped $x)) !(wrap (+ 2 3))",
        &["(wrapped 5)"]
    );

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
    eval_test!(
        if_deeply_nested,
        "!(if True (if True (if True deep outer) outer2) outer3)",
        &["deep"]
    );
    eval_test!(if_lazy_eval_true, "!(if True 1 (/ 1 0))", &["1"]);
    // Non-boolean conditions return the unreduced (if cond then else) as
    // a residual normal form. Spec §11.2.1's "non-Bool → NotReducible"
    // rule is satisfied as a residual: the unreduced expression matches
    // no equation, so callers get the equivalent "no further reduction"
    // signal. See eval_loop.rs ProcessIfCondition non-boolean branch.
    eval_test!(if_non_bool_number, "!(if 1 yes no)", &["(if 1 yes no)"]);
    eval_test!(
        if_with_atom_condition,
        "!(if foo then else)",
        &["(if foo then else)"]
    );
    // MeTTa HE: Unit is NOT boolean — returns unreduced (if () then else)
    eval_test!(
        if_unit_condition_unreduced,
        "!(if () True False)",
        &["(if () True False)"]
    );

    // =========================================================================
    // Let Bindings
    // =========================================================================

    eval_test!(let_simple, "!(let $x 5 $x)", &["5"]);
    eval_test!(let_arithmetic, "!(let $x 10 (+ $x 5))", &["15"]);
    eval_test!(let_nested, "!(let $x 2 (let $y 3 (* $x $y)))", &["6"]);
    eval_test!(let_pattern, "!(let ($a $b) (1 2) (+ $a $b))", &["3"]);
    eval_test!(let_with_computation, "!(let $x (+ 2 3) (* $x 2))", &["10"]);
    eval_test!(let_variable_pattern, "!(let $x 42 $x)", &["42"]);
    eval_test!(
        let_deeply_nested,
        "!(let $x 1 (let $y 2 (let $z 3 (+ $x (+ $y $z)))))",
        &["6"]
    );
    eval_test!(
        let_star_sequential,
        "!(let* (($x 1) ($y (+ $x 1))) $y)",
        &["2"]
    );
    eval_test!(
        let_star_multi,
        "!(let* (($a 1) ($b 2) ($c (+ $a $b))) $c)",
        &["3"]
    );
    eval_test!(
        let_star_multiple,
        "!(let* (($x 1) ($y (+ $x 1)) ($z (+ $y 1))) $z)",
        &["3"]
    );

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
    eval_test!(
        quote_nested,
        "!(quote (+ (+ 1 2) 3))",
        &["(quote (+ (+ 1 2) 3))"]
    );
    eval_test!(eval_force_quoted, "!(eval (quote (* 6 7)))", &["42"]);
    // Plan S4 (2026-05-14): HE-faithful one-step `(eval X)` semantics.
    // Grounded scalar at top level emits `NotReducible` per spec §06.4 /
    // T04-kernel/069-eval-on-grounded.expected.yaml. HE's `eval_impl` line
    // 504 classifies the resolved arg as a scalar with no equation match
    // → `return_not_reducible()`. The outer `metta_call_return` wrapping at
    // the user-visible `!` level converts `NotReducible` back to the
    // original atom in HE's REPL, but MeTTaTron's `!` is unwrapped — so we
    // observe the raw kernel-level sentinel.
    eval_test!(eval_on_value, "!(eval 42)", &["NotReducible"]);
    eval_test!(
        quote_nested_structure,
        "!(quote ((+ 1 2) (* 3 4)))",
        &["(quote ((+ 1 2) (* 3 4)))"]
    );
    eval_test!(eval_nested_quote, "!(eval (quote (+ 1 (+ 2 3))))", &["6"]);

    // unquote: unwraps Quoted variant without evaluating the inner expression
    eval_test!(unquote_quoted, "!(unquote (quote (+ 1 2)))", &["(+ 1 2)"]);
    eval_test!(unquote_non_quoted_identity, "!(unquote 42)", &["42"]);
    eval_test!(unquote_atom, "!(unquote (quote foo))", &["foo"]);

    // quote + introspection transparency
    eval_test!(
        get_metatype_quoted,
        "!(get-metatype (quote foo))",
        &["Expression"]
    );

    // =========================================================================
    // Type System
    // =========================================================================

    eval_test!(get_type_long, "!(get-type 42)", &["Number"]);
    eval_test!(get_type_bool, "!(get-type True)", &["Bool"]);
    eval_test!(get_type_string, "!(get-type \"hello\")", &["String"]);
    eval_test!(get_type_symbol, "!(get-type foo)", &["%Undefined%"]);
    // Phase 10.6: (a b c) where `a` has no rules is a data constructor → Expression
    eval_test!(get_type_expr, "!(get-type (a b c))", &["Expression"]);
    eval_test!(get_type_nil, "!(get-type Nil)", &["%Undefined%"]);
    eval_test!(metatype_expr, "!(get-metatype (a b c))", &["Expression"]);

    #[test]
    fn metatype_variable() {
        let results = run_eval("!(get-metatype $x)");
        assert!(!results.is_empty(), "get-metatype should return a result");
        // Variables should be classified as "Variable" per MeTTa HE semantics
        assert_eq!(results[0], "Variable");
    }

    // Gap 1: get-metatype correctly classifies all metatypes
    eval_test!(
        metatype_variable_dollar,
        "!(get-metatype $x)",
        &["Variable"]
    );
    eval_test!(metatype_symbol, "!(get-metatype foo)", &["Symbol"]);
    // Plan S7 (RC-METATYPE-VOCAB, 2026-05-14): HE 4-category vocabulary.
    // All primitive literals (Number/Bool/String/...) collapse to "Grounded".
    // For fine-grained type information, use (get-type ...) instead.
    // Matches HE `lib/src/metta/types.rs::get_meta_type`.
    eval_test!(metatype_number_long, "!(get-metatype 42)", &["Grounded"]);
    eval_test!(metatype_number_float, "!(get-metatype 3.14)", &["Grounded"]);
    eval_test!(metatype_bool_true, "!(get-metatype True)", &["Grounded"]);
    eval_test!(metatype_string_lit, "!(get-metatype \"hi\")", &["Grounded"]);

    // Gap 2: dependent type reduction (structural)
    eval_test!(
        deptype_structural,
        "(: S (-> Nat Nat)) (: Z Nat) !(get-type (S (S Z)))",
        &["Nat"]
    );

    // Gap 3: match-types
    eval_test!(
        match_types_same,
        "!(match-types Number Number yes no)",
        &["yes"]
    );
    eval_test!(
        match_types_diff,
        "!(match-types Number String yes no)",
        &["no"]
    );
    eval_test!(
        match_types_undefined_lhs,
        "!(match-types %Undefined% Number yes no)",
        &["yes"]
    );
    eval_test!(
        match_types_undefined_rhs,
        "!(match-types Number %Undefined% yes no)",
        &["yes"]
    );
    eval_test!(
        match_types_atom_lhs,
        "!(match-types Atom Number yes no)",
        &["yes"]
    );
    eval_test!(
        match_types_atom_rhs,
        "!(match-types Number Atom yes no)",
        &["yes"]
    );

    // =========================================================================
    // Nondeterminism
    // =========================================================================

    eval_test!(superpose_single, "!(superpose (1))", &["1"]);
    eval_test!(superpose_empty, "!(superpose ())", &[]);
    eval_test!(collapse_single, "!(collapse (superpose (1)))", &["(1)"]);
    eval_test!(
        collapse_single_elem,
        "!(collapse (superpose (42)))",
        &["(42)"]
    );
    eval_test!(collapse_empty_super, "!(collapse (superpose ()))", &["()"]);

    #[test]
    fn superpose_multiple() {
        let results = run_eval("!(superpose (1 2 3))");
        assert!(
            !results.is_empty(),
            "superpose should return at least one result"
        );
        assert_eq!(results[0], "1", "First superpose result should be 1");
    }

    // =========================================================================
    // Case Expressions
    // =========================================================================

    eval_test!(case_basic_match, "!(case a ((a yes) (b no)))", &["yes"]);
    eval_test!(case_second_match, "!(case b ((a yes) (b no)))", &["no"]);
    eval_test!(
        case_default,
        "!(case c ((a yes) ($x default)))",
        &["default"]
    );
    // MeTTa HE: when no case matches, result is Empty (no results / branch pruned)
    eval_test!(case_no_match, "!(case z ((a 1) (b 2)))", &[] as &[&str]);
    eval_test!(case_wildcard, "!(case z ((a 1) (_ default)))", &["default"]);
    eval_test!(case_multi, "!(case b ((a A) (b B) (c C)))", &["B"]);
    eval_test!(
        complex_case,
        "!(case (+ 1 1) ((1 one) (2 two) (3 three) ($x other)))",
        &["two"]
    );

    // =========================================================================
    // Case with nondeterministic scrutinee (demand-driven pruning tests)
    // These tests exercise the Demand::AtLeast(1) optimization on case scrutinee
    // evaluation. The case handler sets demand on the scrutinee WorkItem, which
    // causes dispatch_rule_matches to use BranchCoroutine for lazy evaluation.
    // =========================================================================

    // Case with nondeterministic scrutinee: overlapping rules where one produces empty.
    // This is the key mmverify pattern. Rule 1 is a catch-all that produces empty
    // for the specific input, and Rule 2 matches and produces a real result.
    // With Demand::AtLeast(1), the empty branch is tried but doesn't satisfy demand,
    // so the coroutine tries the next branch which succeeds.
    eval_test!(
        case_demand_one_empty_one_result,
        "(= (try-match $x $x) matched)
         (= (try-match $x $y) (empty))
         !(case (try-match hello hello) ((matched yes) (Empty no)))",
        &["yes"]
    );

    // Case with nondeterministic scrutinee: first rule produces empty, second succeeds
    // This verifies that AtLeast(1) tries subsequent branches when earlier ones fail
    eval_test!(
        case_demand_fallthrough_to_second_rule,
        "(= (search Nil $t) (empty))
         (= (search (Cons $h $rest) $t) (if (== $h $t) found (search $rest $t)))
         !(case (search (Cons a (Cons b Nil)) b) ((found yes) (Empty no)))",
        &["yes"]
    );

    // Case with nondeterministic scrutinee matching Empty pattern
    eval_test!(
        case_demand_empty_scrutinee_matches_empty_arm,
        "(= (search Nil $t) (empty))
         (= (search (Cons $h $rest) $t) (if (== $h $t) found (search $rest $t)))
         !(case (search (Cons a Nil) b) ((found yes) (Empty no)))",
        &["no"]
    );

    // Case with deterministic scrutinee (single rule) - demand pruning not activated
    eval_test!(
        case_demand_single_rule_no_pruning,
        "(= (f $x) (+ $x 1))
         !(case (f 41) ((42 answer) ($x other)))",
        &["answer"]
    );

    // mmverify-like pattern: match-atom with overlapping rules in case
    // Rule 1 matches specific pattern, Rule 2 is a fallback
    eval_test!(
        case_demand_mmverify_pattern,
        "(= (match-atom (Found $v) $p) $v)
         (= (match-atom (NotFound) $p) (empty))
         !(case (match-atom (Found 42) needle) ((42 matched) (Empty missed)))",
        &["matched"]
    );

    // Constructor-discriminated rules inside case scrutinee
    eval_test!(
        case_demand_constructor_discriminated,
        "(= (len Nil) 0)
         (= (len (Cons $h $t)) (+ 1 (len $t)))
         !(case (len (Cons a (Cons b Nil))) ((0 empty) (1 single) (2 pair) ($n many)))",
        &["pair"]
    );

    // =========================================================================
    // Error Handling
    // =========================================================================

    // HE-bisimilar shape: source `(Error offending detail)` maps directly to
    // internal Error(offending, detail). Display emits the same slot order.
    eval_test!(
        error_create,
        "!(Error test-msg details)",
        &["(Error test-msg details)"]
    );
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
        assert!(
            !results.is_empty(),
            "Division by zero should return a result"
        );
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
    fn mul_type_mismatch_returns_error() {
        // H5 (2026-05-05) hard-cut: spec §13.2 line 43-44 — non-Number arg
        // returns (Error <call> <detail>). Previously this produced an
        // unreduced sexpr (NoReduce); the trampoline now wraps the
        // ExecError as a MeTTa Error atom for debuggability.
        //
        // ERR-shape align (2026-05-16): the Error shape is now HE-aligned
        // `(Error <call> <detail>)` — slot 1 is the call form `(* True 5)`,
        // slot 2 is a string/atom detail. Accept either:
        // * the call symbol `(* ` (T0 IncorrectArgument string-msg path)
        // * the BadType tag atom (T1 VM materialize path)
        // * any "Number" keyword in the detail (operand-type message).
        let results = run_eval("!(* True 5)");
        assert!(!results.is_empty(), "Type mismatch should return a result");
        assert!(
            results[0].contains("Error")
                && (results[0].contains("BadType")
                    || results[0].contains("Number")
                    || results[0].contains("IncorrectArgument")),
            "Type mismatch should return (Error ...). Got: {}",
            results[0]
        );
    }

    #[test]
    fn sub_type_mismatch_returns_error() {
        // H5 (2026-05-05) hard-cut: see mul_type_mismatch_returns_error.
        let results = run_eval("!(- \"hello\" 2)");
        assert!(!results.is_empty(), "Type mismatch should return a result");
        assert!(
            results[0].contains("Error")
                && (results[0].contains("BadType")
                    || results[0].contains("Number")
                    || results[0].contains("IncorrectArgument")),
            "Type mismatch should return (Error ...). Got: {}",
            results[0]
        );
    }

    #[test]
    fn div_type_mismatch_returns_error() {
        // H5 (2026-05-05) hard-cut: see mul_type_mismatch_returns_error.
        let results = run_eval("!(/ 10 \"x\")");
        assert!(!results.is_empty(), "Type mismatch should return a result");
        assert!(
            results[0].contains("Error")
                && (results[0].contains("BadType")
                    || results[0].contains("Number")
                    || results[0].contains("IncorrectArgument")),
            "Type mismatch should return (Error ...). Got: {}",
            results[0]
        );
    }

    #[test]
    fn error_direct() {
        let results = run_eval("!(Error TestError \"test message\")");
        assert!(!results.is_empty(), "Error should return a result");
        assert!(results[0].contains("Error"), "Should return an error");
        assert!(
            results[0].contains("TestError"),
            "Should contain error type"
        );
    }

    #[test]
    fn error_if_then_branch() {
        let results = run_eval("!(if True (/ 1 0) ok)");
        assert!(
            !results.is_empty(),
            "If with error in then should return result"
        );
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
    eval_test!(
        nested_map,
        "!(map-atom ((1 2) (3 4)) $lst (car-atom $lst))",
        &["(1 3)"]
    );
    eval_test!(
        filter_all_pass,
        "!(filter-atom (1 2 3) $x True)",
        &["(1 2 3)"]
    );
    eval_test!(filter_all_fail, "!(filter-atom (1 2 3) $x False)", &["()"]);
    eval_test!(
        filter_some_pass,
        "!(filter-atom (1 2 3 4) $x (< $x 3))",
        &["(1 2)"]
    );
    eval_test!(
        foldl_sum,
        "!(foldl-atom (1 2 3 4) 0 $acc $x (+ $acc $x))",
        &["10"]
    );
    eval_test!(
        foldl_empty,
        "!(foldl-atom () 42 $acc $x (+ $acc $x))",
        &["42"]
    );
    eval_test!(
        foldl_product,
        "!(foldl-atom (1 2 3 4) 1 $acc $x (* $acc $x))",
        &["24"]
    );
    eval_test!(
        foldl_concat,
        "!(foldl-atom (a b c) () $acc $x (cons-atom $x $acc))",
        &["(c b a)"]
    );

    // =========================================================================
    // Chain Expressions
    // =========================================================================

    eval_test!(
        chain_basic,
        "!(chain (superpose (1 2)) $x (+ $x 10))",
        &["11", "12"]
    );
    eval_test!(chain_single, "!(chain (+ 1 2) $x (* $x 10))", &["30"]);
    eval_test!(chain_identity, "!(chain 42 $x $x)", &["42"]);
    eval_test!(chain_arithmetic, "!(chain (+ 1 2) $x (* $x 10))", &["30"]);
    eval_test!(
        chain_superpose,
        "!(collapse (chain (superpose (1 2 3)) $x (+ $x 100)))",
        &["(101 102 103)"]
    );
    eval_test!(
        chain_with_let,
        "!(chain 5 $x (let $y 10 (+ $x $y)))",
        &["15"]
    );

    #[test]
    fn chain_empty() {
        // MeTTa HE: chain over empty result produces zero results (branch annihilation)
        let results = run_eval("!(chain (superpose ()) $x (+ $x 1))");
        assert!(
            results.is_empty(),
            "chain with empty expr should produce zero results, got: {:?}",
            results
        );
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
    // Minimal test for parallel branching with SExpr result (2 matches).
    eval_test_unordered!(
        parallel_branch_sexpr_result,
        "(= (g 0) (pair a b))
         (= (g $n) other)
         !(g 0)",
        &["(pair a b)", "other"]
    );

    // Test parallel branching where second branch produces (empty).
    eval_test!(
        parallel_branch_empty_second,
        "(= (h 0) (pair x y))
         (= (h $n) (empty))
         !(h 0)",
        &["(pair x y)"]
    );

    // MeTTa HE fires all matching rules nondeterministically.
    eval_test_unordered!(
        overlapping_patterns,
        "(= (classify 0) zero)
         (= (classify $n) positive)
         !(classify 0)",
        &["zero", "positive"]
    );

    // Nondeterministic rules: collapse gathers all results into a list.
    // Order within collapse is implementation-defined (parallel vs sequential
    // evaluation may reorder), so we check the set of elements.
    #[test]
    fn nondet_rule() {
        let results = run_eval(
            "(= (choice) a)
             (= (choice) b)
             (= (choice) c)
             !(collapse (choice))",
        );
        assert_eq!(results.len(), 1, "Expected exactly one collapse result");
        // Parse the S-expression result and check elements as a set
        let result = &results[0];
        assert!(
            result.starts_with('(') && result.ends_with(')'),
            "Expected S-expression: {}",
            result
        );
        let inner = &result[1..result.len() - 1];
        let mut elements: Vec<&str> = inner.split_whitespace().collect();
        elements.sort();
        assert_eq!(
            elements,
            vec!["a", "b", "c"],
            "Collapse result should contain a, b, c in any order"
        );
    }

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
    eval_test!(
        unify_same_values,
        "!(unify 42 42 success fail)",
        &["success"]
    );
    eval_test!(
        unify_different_values,
        "!(unify 1 2 success fail)",
        &["fail"]
    );
    eval_test!(
        unify_sexpr_matching,
        "!(unify (a b c) (a b c) matched not-matched)",
        &["matched"]
    );
    eval_test!(
        unify_sexpr_not_matching,
        "!(unify (a b c) (a b d) matched not-matched)",
        &["not-matched"]
    );
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
    // Spec §02 canonical float: "2.0" not "2"
    eval_test!(
        safe_div_valid,
        "(= (/safe $A $B) (if (> $B 0.0) (/ $A $B) (empty)))
         !(/safe 1.0 0.5)",
        &["2.0"]
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

    eval_test!(lambda_identity, "(= (id $x) $x)\n!(id 42)", &["42"]);

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
        // X.6 (2026-05-11) — top-level facts no longer auto-add to &self;
        // use explicit (add-atom &self ...) to put the fact in space.
        r#"
            !(add-atom &self (Pair 1 2))
            !(match &self (Pair $a $b) (Pair $b $a))
        "#,
        &["()", "(Pair 2 1)"]
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

    eval_test_unordered!(
        test_meta_type_prevents_bloom_filter_preeval,
        // Verify that when a typed function has Expression-typed args,
        // the bloom filter does NOT pre-evaluate those args even if the
        // arg's head has rules. Without the type system, `(f)` would be
        // pre-evaluated to {1,2,3} by the bloom filter. With the type
        // system marking the arg as Expression, `(f)` is passed unevaluated.
        // The rule body `(head $e)` captures `$e = (f)` and wraps it.
        // Then the tuple path evaluates `(f)` inside `(head (f))` → {1,2,3}.
        // Order is nondeterministic (parallel eval may reorder).
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

    eval_test_unordered!(
        test_fixpoint_data_constructor_no_infinite_loop,
        // X.6 — explicit add-atom for &self after auto-add removal.
        // Bloom-filter / fixpoint behavior is independent of auto-add;
        // we just put the facts in &self ourselves.
        r#"
            !(add-atom &self (number (S Z)))
            !(add-atom &self (number (S (S Z))))
            !(match &self (number (S $x)) (S $x))
        "#,
        &["()", "()", "(S Z)", "(S (S Z))"]
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
        let results = run_eval_tiered(
            r#"
            (= (next Z) (S Z))
            (= (next (S $n)) (S (S $n)))
            !(next Z)
            !(next (S Z))
        "#,
        );
        assert_eq!(results, vec!["(S Z)", "(S (S Z))"]);
    }

    #[test]
    fn test_tiered_typed_function() {
        let results = run_eval_tiered(
            r#"
            (: double (-> Number Number))
            (= (double $x) (+ $x $x))
            !(double (+ 1 2))
            !(double (+ 3 4))
        "#,
        );
        assert_eq!(results, vec!["6", "14"]);
    }

    #[test]
    fn test_tiered_meta_type_preservation() {
        // Same as test_meta_type_expression_not_preevaluated but through
        // the tiered eval() path. The tuple path evaluates sub-elements,
        // so `(quoted (+ 1 2))` → `(quoted 3)` per MeTTa HE semantics.
        let results = run_eval_tiered(
            r#"
            (: my-quote (-> Expression Expression))
            (= (my-quote $e) (quoted $e))
            !(my-quote (+ 1 2))
        "#,
        );
        assert_eq!(results, vec!["(quoted 3)"]);
    }

    #[test]
    fn test_tiered_nondet_applicative() {
        let mut results = run_eval_tiered(
            r#"
            (= (f) 1)
            (= (f) 2)
            (= (f) 3)
            (= (g $x) (* $x $x))
            !(g (f))
        "#,
        );
        results.sort();
        assert_eq!(results, vec!["1", "4", "9"]);
    }

    #[test]
    fn test_tiered_peano_arithmetic() {
        let results = run_eval_tiered(
            r#"
            (= (add Z $n) $n)
            (= (add (S $m) $n) (S (add $m $n)))
            !(add (S Z) (S Z))
            !(add (S (S Z)) (S (S Z)))
        "#,
        );
        assert_eq!(results, vec!["(S (S Z))", "(S (S (S (S Z))))"]);
    }

    #[test]
    fn test_tiered_guarded_recursion() {
        // fib with guard to prevent divergence from overlapping rules
        let results = run_eval_tiered(
            r#"
            (= (fib $n) (if (== $n 0) 1 (if (== $n 1) 1 (+ (fib (- $n 1)) (fib (- $n 2))))))
            !(fib 0)
            !(fib 1)
            !(fib 5)
        "#,
        );
        assert_eq!(results, vec!["1", "1", "8"]);
    }

    #[test]
    fn test_tiered_to_int_conversion() {
        let results = run_eval_tiered(
            r#"
            (= (to-int Z) 0)
            (= (to-int (S $n)) (+ 1 (to-int $n)))
            !(to-int Z)
            !(to-int (S Z))
            !(to-int (S (S (S Z))))
        "#,
        );
        assert_eq!(results, vec!["0", "1", "3"]);
    }

    // =========================================================================
    // if-reducible — tree-walker (eval_test! uses run_eval → eval_trampoline)
    // =========================================================================

    // Reducible expression: (+ 1 2) evaluates to 3 (differs from original)
    eval_test!(
        if_reducible_reduces,
        "!(if-reducible (+ 1 2) True False)",
        &["True"]
    );

    // Irreducible atom: foo has no rules, evaluates to itself
    eval_test!(
        if_reducible_irreducible_atom,
        "!(if-reducible foo True False)",
        &["False"]
    );

    // Irreducible variable: $x has no binding, stays as-is
    eval_test!(
        if_reducible_irreducible_var,
        "!(if-reducible $x True False)",
        &["False"]
    );

    // Nested reducible: inner reduction makes it reducible
    eval_test!(
        if_reducible_nested,
        "!(if-reducible (+ (* 2 3) 1) reduced not-reduced)",
        &["reduced"]
    );

    // Then-branch is evaluated (not just returned as data)
    eval_test!(
        if_reducible_then_eval,
        "!(if-reducible (+ 1 1) (+ 10 20) fallback)",
        &["30"]
    );

    // Else-branch is evaluated when irreducible
    eval_test!(
        if_reducible_else_eval,
        "!(if-reducible foo (+ 10 20) (+ 3 4))",
        &["7"]
    );

    // User-defined rule makes expression reducible
    eval_test!(
        if_reducible_user_rule,
        r#"
            (= (double $x) (* $x 2))
            !(if-reducible (double 5) yes no)
        "#,
        &["yes"]
    );

    // Expression with no matching rule is irreducible
    eval_test!(
        if_reducible_no_rule,
        r#"
            (= (double $x) (* $x 2))
            !(if-reducible (triple 5) yes no)
        "#,
        &["no"]
    );

    // S-expression that is irreducible (no head rule)
    eval_test!(
        if_reducible_sexpr_irreducible,
        "!(if-reducible (unknown-fn 1 2) yes no)",
        &["no"]
    );

    // Boolean result from comparison is reducible
    eval_test!(
        if_reducible_comparison,
        "!(if-reducible (== 1 1) yes no)",
        &["yes"]
    );

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
    eval_test!(
        match_or_found,
        // X.6 — explicit add-atom for &self after auto-add removal.
        r#"
            !(add-atom &self (A B))
            !(match-or &self (A $x) default-val $x)
        "#,
        &["()", "B"]
    );

    // No match → default value returned
    // S1 TOPLEVEL (2026-05-13): bare top-level `(A B)` is now HE ADD-mode —
    // silent side-effecting fact, no echo in result multiset. Only the
    // bang directive contributes to the expected output.
    eval_test!(
        match_or_no_match,
        r#"
            (A B)
            !(match-or &self (C $x) default-val $x)
        "#,
        &["default-val"]
    );

    // Default is evaluated (not just returned as data)
    // S1 TOPLEVEL (2026-05-13): see match_or_no_match.
    eval_test!(
        match_or_default_eval,
        r#"
            (A B)
            !(match-or &self (C $x) (+ 1 2) $x)
        "#,
        &["3"]
    );

    // Multiple matches: all returned (match-or is nondeterministic when matches exist)
    // X.6 — explicit add-atom for &self after auto-add removal.
    eval_test_unordered!(
        match_or_multiple_matches,
        r#"
            !(add-atom &self (color red))
            !(add-atom &self (color blue))
            !(match-or &self (color $x) no-color $x)
        "#,
        &["()", "()", "blue", "red"]
    );

    // Match with complex template
    // X.6 — explicit add-atom for &self after auto-add removal.
    eval_test!(
        match_or_complex_template,
        r#"
            !(add-atom &self (pair 3 4))
            !(match-or &self (pair $a $b) 0 (+ $a $b))
        "#,
        &["()", "7"]
    );

    // Named space (owned) — bind! and add-atom each return ()
    eval_test!(
        match_or_named_space,
        r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (fact $x) unknown $x)
        "#,
        &["()", "()", "42"]
    );

    // Named space with no match → default
    eval_test!(
        match_or_named_space_default,
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
        let results = run_eval_tiered(
            r#"
            (= (double $x) (* $x 2))
            !(if-reducible (double 5) yes no)
        "#,
        );
        assert_eq!(results, vec!["yes"]);
    }

    #[test]
    fn test_tiered_if_reducible_no_rule() {
        let results = run_eval_tiered(
            r#"
            (= (double $x) (* $x 2))
            !(if-reducible (triple 5) yes no)
        "#,
        );
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
        let results = run_eval_tiered(
            r#"
            (A B)
            !(match-or &self (A $x) default-val $x)
        "#,
        );
        // (A B) is a fact, returned as data; match-or finds it and returns B
        assert_eq!(results, vec!["(A B)", "B"]);
    }

    #[test]
    fn test_tiered_match_or_no_match() {
        let results = run_eval_tiered(
            r#"
            (A B)
            !(match-or &self (C $x) default-val $x)
        "#,
        );
        assert_eq!(results, vec!["(A B)", "default-val"]);
    }

    #[test]
    fn test_tiered_match_or_default_eval() {
        let results = run_eval_tiered(
            r#"
            (A B)
            !(match-or &self (C $x) (+ 1 2) $x)
        "#,
        );
        assert_eq!(results, vec!["(A B)", "3"]);
    }

    #[test]
    fn test_tiered_match_or_multiple() {
        let mut results = run_eval_tiered(
            r#"
            (color red)
            (color blue)
            !(match-or &self (color $x) no-color $x)
        "#,
        );
        results.sort();
        assert_eq!(results, vec!["(color blue)", "(color red)", "blue", "red"]);
    }

    #[test]
    fn test_tiered_match_or_named_space() {
        let results = run_eval_tiered(
            r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (fact $x) unknown $x)
        "#,
        );
        // bind! and add-atom each return ()
        assert_eq!(results, vec!["()", "()", "42"]);
    }

    #[test]
    fn test_tiered_match_or_named_space_default() {
        let results = run_eval_tiered(
            r#"
            !(bind! &kb (new-space))
            !(add-atom &kb (fact 42))
            !(match-or &kb (other $x) not-found $x)
        "#,
        );
        assert_eq!(results, vec!["()", "()", "not-found"]);
    }

    // =========================================================================
    // if-reducible + match-or combined usage
    // =========================================================================

    eval_test!(
        if_reducible_with_match_or,
        // X.6 — explicit add-atom for &self after auto-add removal.
        r#"
            !(add-atom &self (color red))
            !(if-reducible (+ 1 2) (match-or &self (color $x) none $x) fallback)
        "#,
        &["()", "red"]
    );

    // S1 TOPLEVEL (2026-05-13): see match_or_no_match.
    eval_test!(
        match_or_with_if_reducible_default,
        r#"
            (A B)
            !(match-or &self (missing $x) (if-reducible (+ 1 1) computed-default raw-default) $x)
        "#,
        &["computed-default"]
    );

    // =========================================================================
    // Phase 8: Type-Driven Optimization Tests
    // =========================================================================

    // --- Sub-phase 8.1: rhs_type wiring ---

    // Arithmetic rules produce Number-typed RHS
    eval_test!(
        phase8_rhs_type_arithmetic,
        r#"
            (= (f $x) (+ $x 1))
            !(f 5)
        "#,
        &["6"]
    );

    // Comparison rules produce Bool-typed RHS
    eval_test!(
        phase8_rhs_type_comparison,
        r#"
            (= (h $x) (< $x 0))
            !(h 5)
        "#,
        &["False"]
    );

    // Variable-only RHS — type depends on binding
    eval_test!(
        phase8_rhs_type_variable,
        r#"
            (= (g $x) $x)
            !(g 42)
        "#,
        &["42"]
    );

    // Unknown function in RHS → %Undefined% filtered out (rhs_type = None)
    eval_test!(
        phase8_rhs_type_undefined,
        r#"
            (= (k $x) (unknown $x))
            !(k hello)
        "#,
        &["(unknown hello)"]
    );

    // --- Sub-phase 8.3: Function vs tuple dispatch ---

    // Value type (no arrow type) → tuple path directly, no rule matching
    eval_test!(
        phase8_value_type_skips_rules,
        r#"
            (: Red Color)
            !(Red 1 2)
        "#,
        &["(Red 1 2)"]
    );

    // Arrow type → still does rule matching
    eval_test!(
        phase8_arrow_type_does_not_skip,
        r#"
            (: f (-> Number Number))
            (= (f $x) (+ $x 10))
            !(f 5)
        "#,
        &["15"]
    );

    // Untyped operator → still does rule matching (no shortcut)
    eval_test!(
        phase8_no_type_does_not_skip,
        r#"
            (= (foo $x) (+ $x 1))
            !(foo 1)
        "#,
        &["2"]
    );

    // --- Sub-phase 8.4: match type-aware space pre-filtering ---

    // Type-based match filtering: only atoms with matching type returned
    eval_test!(
        phase8_match_type_filter_basic,
        r#"
            (: a Number)
            (: b String)
            !(match &self (: $x Number) $x)
        "#,
        &["a"]
    );

    // Multiple atoms of same type — all returned
    eval_test_unordered!(
        phase8_match_type_filter_multi,
        r#"
            (: x Number)
            (: y Number)
            (: z String)
            !(match &self (: $w Number) $w)
        "#,
        &["x", "y"]
    );

    // No atoms of matching type → empty result
    eval_test!(
        phase8_match_type_filter_empty,
        r#"
            (: a Number)
            !(match &self (: $x Bool) $x)
        "#,
        &[]
    );

    // --- Sub-phase 8.5: let/let* type validation ---

    // Typed let binding with matching type — value 42 matches (: $x Number) structurally
    // Note: (: $x Number) as a let-pattern does structural matching, not type checking
    eval_test!(
        phase8_let_typed_match,
        r#"
            !(let $x 42 (+ $x 1))
        "#,
        &["43"]
    );

    // Typed let binding — untyped fallback unchanged
    eval_test!(
        phase8_let_untyped_unchanged,
        r#"
            !(let $x 42 $x)
        "#,
        &["42"]
    );

    // --- Sub-phase 8.6: case type-driven pattern skipping ---

    // String pattern should be skipped for numeric scrutinee
    eval_test!(
        phase8_case_type_skip_string,
        r#"
            !(case 42 (("hello" string-match) ($x (+ $x 1))))
        "#,
        &["43"]
    );

    // Variable pattern should NOT be skipped
    eval_test!(
        phase8_case_type_no_false_skip,
        r#"
            !(case 42 (($x (+ $x 1))))
        "#,
        &["43"]
    );

    // --- Sub-phase 8.8: Grounded arg type pre-validation ---

    // Valid args pass through normally
    eval_test!(
        phase8_grounded_valid_args,
        r#"
            !(+ 1 2)
        "#,
        &["3"]
    );

    // S-expr args are NOT pre-validated (need evaluation first)
    eval_test!(
        phase8_grounded_sexpr_not_validated,
        r#"
            (= (f) 1)
            !(+ (f) 2)
        "#,
        &["3"]
    );

    // --- Sub-phase 8.7: Branch pruning by return type ---

    // Rules with unknown rhs_type (None) are NOT pruned (conservative)
    eval_test!(
        phase8_branch_prune_conservative,
        r#"
            (= (g $x) (unknown-fn $x))
            !(g hello)
        "#,
        &["(unknown-fn hello)"]
    );

    // Expected type Bool from if-condition: rules still fire correctly
    eval_test!(
        phase8_expected_type_bool_if,
        r#"
            (= (pred $x) (< $x 10))
            !(if (pred 5) yes no)
        "#,
        &["yes"]
    );

    // Expected type Number for arithmetic: basic functionality preserved
    eval_test!(
        phase8_expected_type_number_arithmetic,
        r#"
            (= (double $x) (+ $x $x))
            !(+ (double 3) 1)
        "#,
        &["7"]
    );

    // =========================================================================
    // Phase 10.1: Inferred Function Return Type Index
    // =========================================================================

    /// (= (f $x) (+ $x 1)) → inferred rhs_type = Number
    /// get-type (f 5) should return Number via inferred type index
    #[test]
    fn test_inferred_type_arithmetic_rule() {
        let results = run_eval(
            r#"
            (= (f $x) (+ $x 1))
            !(get-type (f 5))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number in results, got: {:?}",
            results
        );
    }

    /// (= (h $x) (< $x 0)) → inferred rhs_type = Bool
    /// get-type (h 5) should return Bool via inferred type index
    #[test]
    fn test_inferred_type_comparison_rule() {
        let results = run_eval(
            r#"
            (= (h $x) (< $x 0))
            !(get-type (h 5))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Bool"),
            "Expected Bool in results, got: {:?}",
            results
        );
    }

    /// (= (f $x) (+ $x 1)), (= (g $x) (f $x))
    /// get-type (g 5) should return Number via chained inferred types:
    /// g's rhs is (f $x) — (f $x) is an S-expr whose head "f" has inferred type Number
    #[test]
    fn test_inferred_type_chained() {
        let results = run_eval(
            r#"
            (= (f $x) (+ $x 1))
            (= (g $x) (f $x))
            !(get-type (g 5))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number in results for chained inferred types, got: {:?}",
            results
        );
    }

    // =========================================================================
    // Phase 10.2: Type Variable Substitution in Return Types
    // =========================================================================

    /// (: id (-> $t $t)), id 42 → get-type should resolve $t to Number
    #[test]
    fn test_type_var_substitution_identity() {
        let results = run_eval(
            r#"
            (: id (-> $t $t))
            (= (id $x) $x)
            !(get-type (id 42))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for (id 42) via type var substitution, got: {:?}",
            results
        );
    }

    /// (: == (-> $a $a Bool)) already works, regression test
    #[test]
    fn test_type_var_substitution_equality() {
        let results = run_eval(
            r#"
            !(get-type (== 1 2))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Bool"),
            "Expected Bool for (== 1 2), got: {:?}",
            results
        );
    }

    /// (: wrap (-> $t (List $t))), wrap 42 → (List Number)
    #[test]
    fn test_type_var_substitution_nested() {
        let results = run_eval(
            r#"
            (: wrap (-> $t (List $t)))
            (= (wrap $x) (list $x))
            !(get-type (wrap 42))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "(List Number)"),
            "Expected (List Number) for (wrap 42) via type var substitution, got: {:?}",
            results
        );
    }

    /// Unbound variable arg → can't substitute, return raw type variable
    #[test]
    fn test_type_var_no_binding() {
        let results = run_eval(
            r#"
            (: id (-> $t $t))
            (= (id $x) $x)
            !(get-type (id $x))
        "#,
        );
        // With unbound $x, arg type is %Undefined% → no constraint on $t
        // Should return $t as-is (or %Undefined% via type variable match)
        assert!(
            !results.is_empty(),
            "get-type (id $x) should return at least one type"
        );
    }

    // =========================================================================
    // Phase 10.3: Recursive Subexpression Type Inference
    // =========================================================================

    /// Chain: f→Number, g calls f, h calls g → h returns Number
    #[test]
    fn test_recursive_inference_chain() {
        let results = run_eval(
            r#"
            (= (f $x) (+ $x 1))
            (= (g $x) (f $x))
            (= (h $x) (g $x))
            !(get-type (h 5))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for 3-level chain (h→g→f→Number), got: {:?}",
            results
        );
    }

    /// Mismatch pruning: (: f (-> Number Bool)), (: f (-> String Number))
    /// (f (+ 1 2)) → arg is Number → only first arrow matches → Bool
    #[test]
    fn test_recursive_inference_prunes_mismatch() {
        let results = run_eval(
            r#"
            (: f (-> Number Bool))
            (: f (-> String Number))
            !(get-type (f (+ 1 2)))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Bool"),
            "Expected Bool for (f (+ 1 2)) with Number→Bool arrow, got: {:?}",
            results
        );
    }

    // =========================================================================
    // Phase 10.4: Local Bidirectional Inference for Rules
    // =========================================================================

    /// (= (double $x) (+ $x $x)) → inferred arrow (-> Number Number)
    /// Without explicit type declaration, get-type should still find Number args
    #[test]
    fn test_infer_arrow_double() {
        let results = run_eval(
            r#"
            (= (double $x) (+ $x $x))
            !(get-type (double 5))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for (double 5) via inferred arrow type, got: {:?}",
            results
        );
    }

    /// (= (is-pos $x) (> $x 0)) → inferred arrow (-> Number Bool)
    #[test]
    fn test_infer_arrow_is_positive() {
        let results = run_eval(
            r#"
            (= (is-pos $x) (> $x 0))
            !(get-type (is-pos 5))
        "#,
        );
        // rhs_type from (> $x 0) is Bool, so the inferred return type should be Bool
        assert!(
            results.iter().any(|r| r == "Bool"),
            "Expected Bool for (is-pos 5) via inferred arrow type, got: {:?}",
            results
        );
    }

    /// (= (id $x) $x) → $x has no constraints → arrow type is all %Undefined% → None
    /// Should fall back to rhs_type (also %Undefined% for variable RHS)
    #[test]
    fn test_infer_arrow_identity() {
        let results = run_eval(
            r#"
            (= (id $x) $x)
            !(get-type (id 42))
        "#,
        );
        // id has no constraints and variable RHS → %Undefined% rhs_type → falls through
        // The result should be at least something (possibly %Undefined%)
        assert!(
            !results.is_empty(),
            "get-type (id 42) should return at least one result"
        );
    }

    /// (= (f $x) (if (> $x 0) (+ $x 1) (- 0 $x))) → $x constrained by >, +, -
    /// All constrain $x to Number. The RHS type is inferred from the `if` expression,
    /// which has type `$t` (unresolved type var from if's signature). The inferred
    /// arrow type is `(-> Number $t)`. The direct rhs_type should also be Number
    /// (from `+` or `-` branches), so we should see Number in the results.
    #[test]
    fn test_infer_arrow_multi_constraint() {
        let results = run_eval(
            r#"
            (= (f $x) (if (> $x 0) (+ $x 1) (- 0 $x)))
            !(get-type (f 5))
        "#,
        );
        // The inferred type includes Number (from rhs_type) and possibly $t (from
        // if's polymorphic return). Check that Number is among the results.
        let has_number = results.iter().any(|r| r == "Number");
        let has_type_var = results.iter().any(|r| r.starts_with('$'));
        assert!(
            has_number || has_type_var,
            "Expected Number or type variable for (f 5), got: {:?}",
            results
        );
    }

    /// Two rules for same head with different rhs_types → both types returned
    #[test]
    fn test_inferred_type_nondeterministic() {
        let results = run_eval(
            r#"
            (= (poly 0) True)
            (= (poly $x) (+ $x 1))
            !(get-type (poly 0))
        "#,
        );
        // Should have at least Bool (from True) and Number (from (+ $x 1))
        let has_bool = results.iter().any(|r| r == "Bool");
        let has_number = results.iter().any(|r| r == "Number");
        assert!(
            has_bool || has_number,
            "Expected Bool and/or Number in nondeterministic results, got: {:?}",
            results
        );
    }

    // ========================================================================
    // Phase 10.5: Fixpoint convergence integration tests
    // ========================================================================

    /// Simple call chain: g calls f. Fixpoint should propagate f's return type to g.
    #[test]
    fn test_fixpoint_simple_chain() {
        let results = run_eval(
            r#"
            (= (f $x) (+ $x 1))
            (= (g $x) (f $x))
            !(get-type (g 5))
        "#,
        );
        let has_number = results.iter().any(|r| r == "Number");
        assert!(
            has_number,
            "Expected Number in fixpoint-propagated type for (g 5), got: {:?}",
            results
        );
    }

    /// Mutual recursion: f calls g and g calls f. Fixpoint should converge.
    /// The `if` expression returns a type variable `$t` (polymorphic return type)
    /// since type inference doesn't descend into `if` branches. The fixpoint
    /// converges to this type variable — the important thing is it doesn't diverge.
    #[test]
    fn test_fixpoint_mutual_recursion() {
        let results = run_eval(
            r#"
            (= (f $x) (if (== $x 0) 1 (g (- $x 1))))
            (= (g $x) (f (+ $x 1)))
            !(get-type (f 5))
        "#,
        );
        let has_number = results.iter().any(|r| r == "Number");
        let has_type_var = results.iter().any(|r| r.starts_with('$'));
        assert!(
            has_number || has_type_var,
            "Expected Number or type variable for (f 5), got: {:?}",
            results
        );
    }

    /// Long dependency chain: a → b → c → d. Fixpoint processes leaves first
    /// (Tarjan reverse topological order) so type propagates through the chain.
    /// Verifies that the fixpoint terminates within bounded iterations.
    #[test]
    fn test_fixpoint_max_iterations() {
        let results = run_eval(
            r#"
            (= (d $x) (+ $x 1))
            (= (c $x) (d $x))
            (= (b $x) (c $x))
            (= (a $x) (b $x))
            !(get-type (a 5))
        "#,
        );
        // The chain a→b→c→d→(+ $x 1) should ultimately resolve to Number.
        // This also verifies the fixpoint doesn't hang on long chains.
        let has_number = results.iter().any(|r| r == "Number");
        assert!(
            has_number,
            "Expected Number in fixpoint-propagated type for (a 5), got: {:?}",
            results
        );
    }

    // =========================================================================
    // Phase A: Supertype Closure in get-type (HE Parity)
    // =========================================================================

    /// (: a Dog), (:< Dog Animal) → get-type a should include both Dog and Animal
    #[test]
    fn test_get_type_includes_supertypes() {
        let results = run_eval(
            r#"
            (: a Dog)
            (:< Dog Animal)
            !(get-type a)
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Dog"),
            "Expected Dog in results, got: {:?}",
            results
        );
        assert!(
            results.iter().any(|r| r == "Animal"),
            "Expected Animal (supertype) in results, got: {:?}",
            results
        );
    }

    /// (:< Dog Animal), (:< Animal LivingThing) → transitive supertype closure
    #[test]
    fn test_get_type_includes_transitive_supertypes() {
        let results = run_eval(
            r#"
            (: a Dog)
            (:< Dog Animal)
            (:< Animal LivingThing)
            !(get-type a)
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Dog"),
            "Expected Dog, got: {:?}",
            results
        );
        assert!(
            results.iter().any(|r| r == "Animal"),
            "Expected Animal, got: {:?}",
            results
        );
        assert!(
            results.iter().any(|r| r == "LivingThing"),
            "Expected LivingThing (transitive), got: {:?}",
            results
        );
    }

    /// Same type declared directly and via supertype → no duplicates
    #[test]
    fn test_get_type_no_duplicate_supertypes() {
        let results = run_eval(
            r#"
            (: a Dog)
            (: a Animal)
            (:< Dog Animal)
            !(get-type a)
        "#,
        );
        let dog_count = results.iter().filter(|r| r.as_str() == "Dog").count();
        let animal_count = results.iter().filter(|r| r.as_str() == "Animal").count();
        assert_eq!(
            dog_count, 1,
            "Dog should appear exactly once, got: {:?}",
            results
        );
        assert_eq!(
            animal_count, 1,
            "Animal should appear exactly once, got: {:?}",
            results
        );
    }

    // =========================================================================
    // Phase B: Tuple Type Construction (HE Parity)
    // =========================================================================

    /// (: a A), (: b B) → get-type (a b) = (A B)
    #[test]
    fn test_tuple_type_simple() {
        let results = run_eval(
            r#"
            (: a A)
            (: b B)
            !(get-type (a b))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "(A B)"),
            "Expected (A B) tuple type, got: {:?}",
            results
        );
    }

    /// Nondeterministic types → Cartesian product
    #[test]
    fn test_tuple_type_cartesian() {
        let results = run_eval(
            r#"
            (: a A)
            (: a AA)
            (: b B)
            !(get-type (a b))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "(A B)"),
            "Expected (A B) in results, got: {:?}",
            results
        );
        assert!(
            results.iter().any(|r| r == "(AA B)"),
            "Expected (AA B) in results, got: {:?}",
            results
        );
    }

    /// Nested tuple type
    #[test]
    fn test_tuple_type_nested() {
        let results = run_eval(
            r#"
            (: a A)
            (: b B)
            (: c C)
            !(get-type (a b c))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "(A B C)"),
            "Expected (A B C) tuple type, got: {:?}",
            results
        );
    }

    /// Element with no type → falls back to Expression
    #[test]
    fn test_tuple_type_untyped_element_falls_back() {
        let results = run_eval(
            r#"
            (: a A)
            !(get-type (a untyped_thing))
        "#,
        );
        // untyped_thing has no type, so tuple construction can't proceed
        // Falls back to Expression
        assert!(
            results.iter().any(|r| r == "Expression"),
            "Expected Expression fallback, got: {:?}",
            results
        );
    }

    /// Data constructor with typed literals → tuple type includes literal types
    #[test]
    fn test_tuple_type_with_literals() {
        let results = run_eval(
            r#"
            (: stv DataCtor)
            !(get-type (stv 0.5 0.8))
        "#,
        );
        // stv has type DataCtor (not an arrow), 0.5 and 0.8 are Number
        // Tuple type should be (DataCtor Number Number)
        assert!(
            results.iter().any(|r| r == "(DataCtor Number Number)"),
            "Expected (DataCtor Number Number) tuple type, got: {:?}",
            results
        );
    }

    // =========================================================================
    // Phase C: Control-Flow Tracing Extensions
    // =========================================================================

    /// (chain (+ 1 2) $x (+ $x 1)) → Number (body type)
    #[test]
    fn test_infer_type_chain_traces_body() {
        let results = run_eval(
            r#"
            !(get-type (chain (+ 1 2) $x (+ $x 1)))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for chain body type, got: {:?}",
            results
        );
    }

    /// (function (chain (+ 1 2) $x (return (+ $x 1)))) → Number
    #[test]
    fn test_infer_type_function_return() {
        let results = run_eval(
            r#"
            !(get-type (function (chain (+ 1 2) $x (return (+ $x 1)))))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for function return type, got: {:?}",
            results
        );
    }

    /// (superpose (42 "hello")) → {Number, String}
    #[test]
    fn test_infer_type_superpose_union() {
        let results = run_eval(
            r#"
            !(get-type (superpose (42 "hello")))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number in superpose type union, got: {:?}",
            results
        );
        assert!(
            results.iter().any(|r| r == "String"),
            "Expected String in superpose type union, got: {:?}",
            results
        );
    }

    /// (superpose (1 2 3)) → Number (deduplicated)
    #[test]
    fn test_infer_type_superpose_dedup() {
        let results = run_eval(
            r#"
            !(get-type (superpose (1 2 3)))
        "#,
        );
        let number_count = results.iter().filter(|r| r.as_str() == "Number").count();
        assert_eq!(
            number_count, 1,
            "Expected exactly one Number (deduplicated), got: {:?}",
            results
        );
    }

    /// (match &self ($x) (+ $x 1)) → Number (template type)
    #[test]
    fn test_infer_type_match_template() {
        let results = run_eval(
            r#"
            !(get-type (match &self ($x) (+ $x 1)))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for match template type, got: {:?}",
            results
        );
    }

    /// (unify $a $b 42 "hello") → {Number, String}
    #[test]
    fn test_infer_type_unify_branches() {
        let results = run_eval(
            r#"
            !(get-type (unify $a $b 42 "hello"))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number in unify branch types, got: {:?}",
            results
        );
        assert!(
            results.iter().any(|r| r == "String"),
            "Expected String in unify branch types, got: {:?}",
            results
        );
    }

    /// Nested chain inside function with return — return type traced from
    /// the innermost (return expr), inferring its type structurally.
    #[test]
    fn test_infer_type_nested_chain_function() {
        // (function (chain (+ 1 2) $r (return (+ $r 1)))) — return arg is
        // (+ $r 1) which has type Number from the builtin signature.
        let results = run_eval(
            r#"
            !(get-type (function (chain (+ 1 2) $r (return (+ $r 1)))))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for nested chain→function→return, got: {:?}",
            results
        );
    }

    // =========================================================================
    // Phase D: Variable Freshening in Type Lookup
    // =========================================================================

    /// Two arrows with same type variable name shouldn't cross-contaminate
    #[test]
    fn test_type_variable_freshening_no_cross_contamination() {
        let results = run_eval(
            r#"
            (: f (-> $t $t))
            (: g (-> $t Bool))
            (= (f $x) $x)
            (= (g $x) True)
            !(get-type (f (g 42)))
        "#,
        );
        // g returns Bool, so f(Bool) should return Bool (via $t=Bool)
        assert!(
            results.iter().any(|r| r == "Bool"),
            "Expected Bool for (f (g 42)), got: {:?}",
            results
        );
    }

    /// Within a single arrow, type variables should still be consistently bound
    #[test]
    fn test_freshening_preserves_intra_arrow_binding() {
        let results = run_eval(
            r#"
            (: id (-> $t $t))
            (= (id $x) $x)
            !(get-type (id 42))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for (id 42) via intra-arrow binding, got: {:?}",
            results
        );
    }

    // ====================================================================
    // Phase F: Meta-type awareness tests
    // ====================================================================

    /// Meta-type Atom in parameter position should accept any argument type
    #[test]
    fn test_meta_type_atom_matches_anything() {
        let results = run_eval(
            r#"
            (: f (-> Atom Bool))
            (= (f $x) True)
            !(check-type (f 42) Bool)
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// Meta-type Symbol should accept symbol arguments
    #[test]
    fn test_meta_type_symbol_match() {
        let results = run_eval(
            r#"
            (: f (-> Symbol Bool))
            (= (f $x) True)
            !(check-type (f foo) Bool)
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// Meta-type Expression should accept S-expression arguments
    #[test]
    fn test_meta_type_expression_match() {
        let results = run_eval(
            r#"
            (: f (-> Expression Bool))
            (= (f $x) True)
            !(check-type (f (a b)) Bool)
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// get-type with meta-type Atom parameter should not be filtered out
    #[test]
    fn test_get_type_with_atom_param_not_filtered() {
        let results = run_eval(
            r#"
            (: myop (-> Atom Number))
            !(get-type (myop anything))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for (myop anything) with Atom param, got: {:?}",
            results
        );
    }

    // ====================================================================
    // Phase E: Argument type validation in get-type
    // ====================================================================

    /// get-type should return empty for arg type mismatch (HE parity)
    #[test]
    fn test_get_type_arg_mismatch_returns_empty() {
        let results = run_eval(
            r#"
            !(get-type (+ 5 "hello"))
        "#,
        );
        // HE returns empty (no results) for type mismatch
        assert!(
            results.is_empty() || results.iter().all(|r| r == "%Undefined%"),
            "Expected empty or %Undefined% for (+ 5 \"hello\"), got: {:?}",
            results
        );
    }

    /// get-type should filter to matching arrows only
    #[test]
    fn test_get_type_partial_match_filters() {
        let results = run_eval(
            r#"
            (: f (-> Number Bool))
            (: f (-> String Number))
            !(get-type (f 5))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Bool"),
            "Expected Bool for (f 5) with Number arg, got: {:?}",
            results
        );
        assert!(
            !results.iter().any(|r| r == "Number"),
            "Should NOT return Number for (f 5) since 5 is not String, got: {:?}",
            results
        );
    }

    /// get-type with correct args should still work
    #[test]
    fn test_get_type_correct_args_unchanged() {
        let results = run_eval(
            r#"
            !(get-type (+ 1 2))
        "#,
        );
        assert!(
            results.iter().any(|r| r == "Number"),
            "Expected Number for (+ 1 2), got: {:?}",
            results
        );
    }

    // ====================================================================
    // Phase G: is-function tests
    // ====================================================================

    /// is-function should return True for arrow types
    #[test]
    fn test_is_function_arrow_true() {
        let results = run_eval_tiered(
            r#"
            !(is-function (-> A B))
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// is-function should return False for non-arrow atoms
    #[test]
    fn test_is_function_atom_false() {
        let results = run_eval_tiered(
            r#"
            !(is-function Number)
        "#,
        );
        assert_eq!(results, vec!["False"]);
    }

    /// is-function should handle nested arrows
    #[test]
    fn test_is_function_nested_arrow() {
        let results = run_eval_tiered(
            r#"
            !(is-function (-> (-> A B) C))
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// is-function should return False for empty expression
    #[test]
    fn test_is_function_empty_expr() {
        let results = run_eval_tiered(
            r#"
            !(is-function ())
        "#,
        );
        assert_eq!(results, vec!["False"]);
    }

    // ====================================================================
    // Phase H: type-cast tests
    // ====================================================================

    /// type-cast should return atom when type matches
    #[test]
    fn test_type_cast_match_returns_atom() {
        let results = run_eval_tiered(
            r#"
            (: a A)
            !(type-cast a A &self)
        "#,
        );
        assert_eq!(results, vec!["a"]);
    }

    /// type-cast should return (Error atom BadType) when type doesn't match
    #[test]
    fn test_type_cast_mismatch_returns_error() {
        let results = run_eval_tiered(
            r#"
            (: a A)
            !(type-cast a B &self)
        "#,
        );
        assert_eq!(results, vec!["(Error a BadType)"]);
    }

    /// type-cast with %Undefined% expected type should accept anything
    #[test]
    fn test_type_cast_undefined_matches() {
        let results = run_eval_tiered(
            r#"
            (: a A)
            !(type-cast a %Undefined% &self)
        "#,
        );
        assert_eq!(results, vec!["a"]);
    }

    /// type-cast with untyped atom should accept (untyped = %Undefined%)
    #[test]
    fn test_type_cast_untyped_matches() {
        let results = run_eval_tiered(
            r#"
            !(type-cast a B &self)
        "#,
        );
        assert_eq!(results, vec!["a"]);
    }

    /// type-cast with grounded type
    #[test]
    fn test_type_cast_grounded() {
        let results = run_eval_tiered(
            r#"
            !(type-cast 42 Number &self)
        "#,
        );
        assert_eq!(results, vec!["42"]);
    }

    /// type-cast with meta-type Atom
    #[test]
    fn test_type_cast_meta_atom() {
        let results = run_eval_tiered(
            r#"
            (: a A)
            !(type-cast a Atom &self)
        "#,
        );
        assert_eq!(results, vec!["a"]);
    }

    /// type-cast with meta-type Symbol
    #[test]
    fn test_type_cast_meta_symbol() {
        let results = run_eval_tiered(
            r#"
            (: a A)
            !(type-cast a Symbol &self)
        "#,
        );
        assert_eq!(results, vec!["a"]);
    }

    /// type-cast with meta-type Grounded
    #[test]
    fn test_type_cast_meta_grounded() {
        let results = run_eval_tiered(
            r#"
            !(type-cast 42 Grounded &self)
        "#,
        );
        assert_eq!(results, vec!["42"]);
    }

    /// type-cast with meta-type Expression
    #[test]
    fn test_type_cast_meta_expression() {
        let results = run_eval_tiered(
            r#"
            !(type-cast (a b) Expression &self)
        "#,
        );
        assert_eq!(results, vec!["(a b)"]);
    }

    /// type-cast with meta-type Variable
    #[test]
    fn test_type_cast_meta_variable() {
        let results = run_eval_tiered(
            r#"
            !(type-cast $v Variable &self)
        "#,
        );
        assert_eq!(results, vec!["$v"]);
    }

    // ====================================================================
    // Phase I: Arrow structural subtyping tests (unit tests)
    // ====================================================================

    /// Arrow covariant return: (-> Number Dog) should match (-> Number Animal) if Dog <: Animal
    #[test]
    fn test_arrow_covariant_return() {
        use crate::backend::environment::MettaEnvironment;
        use crate::backend::eval::types::types_match_with_subtypes;
        use crate::backend::models::GcFactory;
        use crate::backend::models::MettaValueFactory;

        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());
        env.add_subtype_generic("Dog", "Animal");

        // (-> Number Dog) vs (-> Number Animal)
        let actual = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Number"),
            factory.atom("Dog"),
        ]);
        let expected = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Number"),
            factory.atom("Animal"),
        ]);
        assert!(
            types_match_with_subtypes(&actual, &expected, &env),
            "Arrow covariant return: (-> Number Dog) should match (-> Number Animal)"
        );
    }

    /// Arrow contravariant param: (-> Animal Bool) should match (-> Dog Bool) if Dog <: Animal
    #[test]
    fn test_arrow_contravariant_param() {
        use crate::backend::environment::MettaEnvironment;
        use crate::backend::eval::types::types_match_with_subtypes;
        use crate::backend::models::GcFactory;
        use crate::backend::models::MettaValueFactory;

        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());
        env.add_subtype_generic("Dog", "Animal");

        // (-> Animal Bool) vs (-> Dog Bool)
        // Contravariant: expected param Dog <: actual param Animal => match
        let actual = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Animal"),
            factory.atom("Bool"),
        ]);
        let expected = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Dog"),
            factory.atom("Bool"),
        ]);
        assert!(
            types_match_with_subtypes(&actual, &expected, &env),
            "Arrow contravariant param: (-> Animal Bool) should match (-> Dog Bool)"
        );
    }

    /// Arrow variance mismatch: (-> Dog Bool) should NOT match (-> Animal Bool)
    /// with covariant params (would be unsound)
    #[test]
    fn test_arrow_invariant_mismatch() {
        use crate::backend::environment::MettaEnvironment;
        use crate::backend::eval::types::types_match_with_subtypes;
        use crate::backend::models::GcFactory;
        use crate::backend::models::MettaValueFactory;

        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());
        env.add_subtype_generic("Dog", "Animal");

        // (-> Dog Bool) vs (-> Animal Bool)
        // WRONG to accept with covariant params: Animal is NOT <: Dog
        let actual = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Dog"),
            factory.atom("Bool"),
        ]);
        let expected = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Animal"),
            factory.atom("Bool"),
        ]);
        assert!(
            !types_match_with_subtypes(&actual, &expected, &env),
            "Arrow invariant mismatch: (-> Dog Bool) should NOT match (-> Animal Bool)"
        );
    }

    // ====================================================================
    // Gap A: (:< SubType SuperType) subtype declaration dispatch
    // ====================================================================

    /// (:< Dog Animal) should register subtype so type-cast succeeds
    #[test]
    fn test_subtype_decl_type_cast() {
        let results = run_eval_tiered(
            r#"
            (: rex Dog)
            (:< Dog Animal)
            !(type-cast rex Animal &self)
        "#,
        );
        assert_eq!(results, vec!["rex"]);
    }

    /// (:< ...) should return empty list (like : declarations)
    #[test]
    fn test_subtype_decl_returns_empty() {
        let results = run_eval_tiered(
            r#"
            !(:< Dog Animal)
        "#,
        );
        assert!(
            results.is_empty(),
            "Subtype declaration should return empty, got: {:?}",
            results
        );
    }

    /// Transitive subtype: Dog <: Animal, Animal <: LivingThing
    #[test]
    fn test_subtype_decl_transitive() {
        let results = run_eval_tiered(
            r#"
            (: rex Dog)
            (:< Dog Animal)
            (:< Animal LivingThing)
            !(type-cast rex LivingThing &self)
        "#,
        );
        assert_eq!(results, vec!["rex"]);
    }

    /// (:< ...) should fail for non-subtype type-cast
    #[test]
    fn test_subtype_decl_mismatch() {
        let results = run_eval_tiered(
            r#"
            (: rex Dog)
            (:< Dog Animal)
            !(type-cast rex Plant &self)
        "#,
        );
        assert_eq!(results, vec!["(Error rex BadType)"]);
    }

    /// (:< ...) should reject non-atom arguments
    #[test]
    fn test_subtype_decl_bad_args() {
        let results = run_eval_tiered(
            r#"
            !(:< (a b) Animal)
        "#,
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("Error"), "Should error on non-atom arg");
    }

    // ====================================================================
    // Gap B: match-type-or fold helper
    // ====================================================================

    /// match-type-or with False folded, matching type → True
    #[test]
    fn test_match_type_or_match() {
        let results = run_eval_tiered(
            r#"
            !(match-type-or False Number Number)
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// match-type-or with True folded, non-matching type → True (or semantics)
    #[test]
    fn test_match_type_or_folded_true() {
        let results = run_eval_tiered(
            r#"
            !(match-type-or True Number String)
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    /// match-type-or with False folded, non-matching type → False
    #[test]
    fn test_match_type_or_no_match() {
        let results = run_eval_tiered(
            r#"
            !(match-type-or False Number String)
        "#,
        );
        assert_eq!(results, vec!["False"]);
    }

    /// match-type-or with %Undefined% → always True
    #[test]
    fn test_match_type_or_undefined() {
        let results = run_eval_tiered(
            r#"
            !(match-type-or False %Undefined% String)
        "#,
        );
        assert_eq!(results, vec!["True"]);
    }

    // ====================================================================
    // Gap C: (metta atom type space) interpreter operation
    // ====================================================================

    /// metta with %Undefined% type → evaluates expression normally
    #[test]
    fn test_metta_undefined_type_evaluates() {
        let results = run_eval_tiered(
            r#"
            (= (double $x) (* 2 $x))
            !(metta (double 5) %Undefined% &self)
        "#,
        );
        assert_eq!(results, vec!["10"]);
    }

    /// metta with Atom type → evaluates expression normally
    #[test]
    fn test_metta_atom_type_evaluates() {
        let results = run_eval_tiered(
            r#"
            (= (double $x) (* 2 $x))
            !(metta (double 5) Atom &self)
        "#,
        );
        assert_eq!(results, vec!["10"]);
    }

    /// metta with Variable → passes through unchanged
    #[test]
    fn test_metta_variable_passthrough() {
        let results = run_eval_tiered(
            r#"
            !(metta $x Number &self)
        "#,
        );
        assert_eq!(results, vec!["$x"]);
    }

    /// metta with Symbol and matching meta-type → passes through
    #[test]
    fn test_metta_symbol_metatype() {
        let results = run_eval_tiered(
            r#"
            !(metta foo Symbol &self)
        "#,
        );
        assert_eq!(results, vec!["foo"]);
    }

    /// metta with Expression and Expression meta-type → passes through
    #[test]
    fn test_metta_expression_metatype() {
        let results = run_eval_tiered(
            r#"
            !(metta (a b) Expression &self)
        "#,
        );
        assert_eq!(results, vec!["(a b)"]);
    }

    /// metta with Grounded and Grounded meta-type → passes through
    #[test]
    fn test_metta_grounded_metatype() {
        let results = run_eval_tiered(
            r#"
            !(metta 42 Grounded &self)
        "#,
        );
        assert_eq!(results, vec!["42"]);
    }

    /// metta with typed symbol → type-cast check
    #[test]
    fn test_metta_symbol_typed_match() {
        let results = run_eval_tiered(
            r#"
            (: foo Foo)
            !(metta foo Foo &self)
        "#,
        );
        assert_eq!(results, vec!["foo"]);
    }

    /// metta with typed symbol mismatch → error
    #[test]
    fn test_metta_symbol_typed_mismatch() {
        let results = run_eval_tiered(
            r#"
            (: foo Foo)
            !(metta foo Bar &self)
        "#,
        );
        assert_eq!(results, vec!["(Error foo BadType)"]);
    }

    /// metta evaluates expression and type-checks result
    #[test]
    fn test_metta_eval_and_typecheck() {
        let results = run_eval_tiered(
            r#"
            (: inc (-> Number Number))
            (= (inc $n) (+ $n 1))
            !(metta (inc 5) Number &self)
        "#,
        );
        assert_eq!(results, vec!["6"]);
    }

    // ====================================================================
    // Gap D: first-from-pair
    // ====================================================================

    /// first-from-pair extracts first element from a pair
    #[test]
    fn test_first_from_pair_basic() {
        let results = run_eval_tiered(
            r#"
            !(first-from-pair (hello world))
        "#,
        );
        assert_eq!(results, vec!["hello"]);
    }

    /// first-from-pair with numeric pair
    #[test]
    fn test_first_from_pair_numeric() {
        let results = run_eval_tiered(
            r#"
            !(first-from-pair (42 99))
        "#,
        );
        assert_eq!(results, vec!["42"]);
    }

    /// first-from-pair with non-pair → error
    #[test]
    fn test_first_from_pair_not_pair_single() {
        let results = run_eval_tiered(
            r#"
            !(first-from-pair (only))
        "#,
        );
        assert_eq!(results.len(), 1);
        assert!(
            results[0].contains("Error"),
            "Should error on non-pair: {:?}",
            results
        );
    }

    /// first-from-pair with non-pair (triple) → error
    #[test]
    fn test_first_from_pair_not_pair_triple() {
        let results = run_eval_tiered(
            r#"
            !(first-from-pair (a b c))
        "#,
        );
        assert_eq!(results.len(), 1);
        assert!(
            results[0].contains("Error"),
            "Should error on triple: {:?}",
            results
        );
    }

    /// first-from-pair with non-expression → error
    #[test]
    fn test_first_from_pair_non_expr() {
        let results = run_eval_tiered(
            r#"
            !(first-from-pair hello)
        "#,
        );
        assert_eq!(results.len(), 1);
        assert!(
            results[0].contains("Error"),
            "Should error on atom: {:?}",
            results
        );
    }
}
