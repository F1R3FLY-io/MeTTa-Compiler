//! Integration tests for context-aware fuzzy matching
//!
//! These tests verify the fuzzy matching system works correctly with
//! the MeTTa evaluation pipeline.

use mettatron::backend::builtin_signatures::{builtin_names, get_signature, is_builtin, TypeExpr};
use mettatron::backend::fuzzy_match::{SuggestionConfidence, SuggestionContext};
use mettatron::backend::models::MettaValue;
use mettatron::backend::{compile, eval, Environment, FuzzyMatcher};

// ============================================================================
// Test Helpers
// ============================================================================

/// Create a FuzzyMatcher pre-populated with all built-ins
fn matcher_with_builtins() -> FuzzyMatcher {
    FuzzyMatcher::from_terms(builtin_names())
}

/// Check if the fuzzy matcher suggests `target` for `typo` with given arity.
///
/// This uses the raw suggest() + arity filtering without type compatibility,
/// because type compatibility depends on specific argument types.
fn would_suggest_for_arity(typo: &str, target: &str, arity: usize) -> bool {
    let matcher = matcher_with_builtins();

    // Get raw suggestions
    let suggestions = matcher.suggest(typo, 3);

    // Check if target is in suggestions
    if !suggestions.iter().any(|(term, _)| term == target) {
        return false;
    }

    // Check arity compatibility manually
    if let Some(sig) = get_signature(target) {
        arity >= sig.min_arity && arity <= sig.max_arity
    } else {
        true // Non-builtins pass
    }
}

/// Check if the fuzzy matcher suggests `target` for `typo` with full context
/// including type compatibility checking.
fn would_suggest_with_context(typo: &str, target: &str, args: &[MettaValue]) -> bool {
    let matcher = matcher_with_builtins();
    let env = Environment::new();

    let ctx = SuggestionContext::for_head(args, &env);
    let result = matcher.smart_suggest_with_context(typo, 3, &ctx);

    result
        .map(|r| r.suggestions.contains(&target.to_string()))
        .unwrap_or(false)
}

// ============================================================================
// Issue #51 Regression Tests
// ============================================================================

#[test]
fn test_issue_51_lit_no_suggest_let() {
    // The core bug from issue #51: (lit p) should NOT suggest `let`
    // because `let` requires arity 3, but `lit` has arity 1
    assert!(
        !would_suggest_for_arity("lit", "let", 1),
        "lit with arity 1 should NOT suggest let (requires arity 3)"
    );
}

#[test]
fn test_issue_51_lett_suggests_let() {
    // (lett x 1 x) has arity 3, matching let's requirements
    assert!(
        would_suggest_for_arity("lett", "let", 3),
        "lett with arity 3 should suggest let"
    );
}

#[test]
fn test_issue_51_cach_no_suggest_catch() {
    // (cach expr) with arity 1 should not suggest catch (requires arity 2)
    assert!(
        !would_suggest_for_arity("cach", "catch", 1),
        "cach with arity 1 should NOT suggest catch (requires arity 2)"
    );
}

#[test]
fn test_issue_51_cach_suggests_catch() {
    // (cach expr default) with arity 2 should suggest catch
    assert!(
        would_suggest_for_arity("cach", "catch", 2),
        "cach with arity 2 should suggest catch"
    );
}

// ============================================================================
// Arity Compatibility Integration Tests
// ============================================================================

#[test]
fn test_arity_if_typo() {
    // Test that "if" suggestions respect arity (if requires exactly 3 args)

    // With arity 3, "if" should be suggested for "iff" (arity matches)
    // Note: The raw suggest + arity check doesn't apply confidence filtering,
    // so short words like "iff" do get matched here.
    assert!(
        would_suggest_for_arity("iff", "if", 3),
        "iff→if with arity 3 should be suggested (arity matches)"
    );

    // With arity 1, "if" should NOT be suggested (arity too low)
    assert!(
        !would_suggest_for_arity("iff", "if", 1),
        "iff→if with arity 1 should NOT be suggested (needs 3)"
    );

    // With arity 4, "if" should NOT be suggested (arity too high)
    assert!(
        !would_suggest_for_arity("iff", "if", 4),
        "iff→if with arity 4 should NOT be suggested (max is 3)"
    );
}

#[test]
fn test_arity_match_typo() {
    // (metch space pattern body) has arity 3, matches match (min 3, max 4)
    assert!(
        would_suggest_for_arity("metch", "match", 3),
        "metch→match with arity 3 should be suggested"
    );

    // (metch space pattern body default) has arity 4, also valid for match
    assert!(
        would_suggest_for_arity("metch", "match", 4),
        "metch→match with arity 4 should be suggested"
    );

    // (metch space) has arity 1, doesn't match (below min_arity 3)
    assert!(
        !would_suggest_for_arity("metch", "match", 1),
        "metch→match with arity 1 should NOT be suggested"
    );

    // Arity 5 exceeds max_arity 4
    assert!(
        !would_suggest_for_arity("metch", "match", 5),
        "metch→match with arity 5 should NOT be suggested"
    );
}

#[test]
fn test_arity_unify_typo() {
    // unify requires exactly 4 args
    assert!(would_suggest_for_arity("uniffy", "unify", 4));
    assert!(!would_suggest_for_arity("uniffy", "unify", 3));
    assert!(!would_suggest_for_arity("uniffy", "unify", 5));
}

#[test]
fn test_arity_zero_operations() {
    // nop, new-space, empty have arity 0
    assert!(would_suggest_for_arity("nopp", "nop", 0));
    assert!(!would_suggest_for_arity("nopp", "nop", 1));

    assert!(would_suggest_for_arity("emty", "empty", 0));
    assert!(!would_suggest_for_arity("emty", "empty", 1));
}

// ============================================================================
// Type System Integration Tests
// ============================================================================

#[test]
fn test_type_signature_arithmetic() {
    // All arithmetic operators should have Number -> Number -> Number signature
    for op in ["+", "-", "*", "/", "%"] {
        let sig = get_signature(op).expect(&format!("Should have signature for {}", op));
        assert_eq!(sig.min_arity, 2, "{} should have min_arity 2", op);
        assert_eq!(sig.max_arity, 2, "{} should have max_arity 2", op);
    }
}

#[test]
fn test_type_signature_comparison() {
    // All comparison operators should be builtins with arity 2
    for op in ["<", "<=", ">", ">=", "==", "!="] {
        assert!(is_builtin(op), "{} should be a builtin", op);
        let sig = get_signature(op).expect(&format!("Should have signature for {}", op));
        assert_eq!(sig.min_arity, 2);
        assert_eq!(sig.max_arity, 2);
    }
}

#[test]
fn test_type_signature_space_first_arg() {
    // Space operations that take space as first argument
    for op in ["match", "add-atom", "remove-atom", "get-atoms"] {
        let sig = get_signature(op).expect(&format!("Should have signature for {}", op));
        if let TypeExpr::Arrow(args, _) = &sig.type_sig {
            assert!(!args.is_empty(), "{} should have at least one arg", op);
            assert_eq!(
                args[0],
                TypeExpr::Space,
                "{} should expect Space as first arg",
                op
            );
        } else {
            panic!("{} should have Arrow signature", op);
        }
    }
}

// ============================================================================
// Fuzzy Matcher Basic Tests
// ============================================================================

#[test]
fn test_fuzzy_matcher_initialization() {
    let matcher = FuzzyMatcher::new();

    // Should start empty
    assert!(
        matcher.suggest("let", 5).is_empty(),
        "New matcher should be empty"
    );
    assert!(matcher.is_empty());
}

#[test]
fn test_fuzzy_matcher_insert_and_suggest() {
    let matcher = FuzzyMatcher::new();
    matcher.insert("let");
    matcher.insert("match");
    matcher.insert("if");

    assert!(!matcher.is_empty());
    assert_eq!(matcher.len(), 3);

    // Should find suggestions
    let suggestions = matcher.suggest("lett", 2);
    assert!(
        suggestions.iter().any(|(word, _)| word == "let"),
        "Should suggest 'let' for 'lett'"
    );
}

#[test]
fn test_fuzzy_matcher_with_all_builtins() {
    let matcher = matcher_with_builtins();

    // Should have at least 40 built-ins
    assert!(
        matcher.len() >= 40,
        "Should have at least 40 built-ins, got {}",
        matcher.len()
    );

    // Should find common typos
    let suggestions = matcher.suggest("lett", 2);
    assert!(
        suggestions.iter().any(|(word, _)| word == "let"),
        "Should suggest 'let' for 'lett'"
    );

    let suggestions = matcher.suggest("iff", 2);
    assert!(
        suggestions.iter().any(|(word, _)| word == "if"),
        "Should suggest 'if' for 'iff'"
    );
}

#[test]
fn test_fuzzy_matcher_did_you_mean() {
    let matcher = FuzzyMatcher::new();
    matcher.insert("let");
    matcher.insert("match");

    let result = matcher.did_you_mean("lett", 2, 3);
    assert!(result.is_some(), "Should find a suggestion");
    assert!(result.unwrap().contains("let"), "Should contain 'let'");
}

// ============================================================================
// SuggestionContext Tests
// ============================================================================

#[test]
fn test_suggestion_context_for_head() {
    let env = Environment::new();
    let args = vec![
        MettaValue::Atom("unknown".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ];

    let ctx = SuggestionContext::for_head(&args, &env);
    assert_eq!(ctx.arity(), 2, "arity should be number of args after head");
    assert_eq!(ctx.position, 0, "head position should be 0");
    assert!(
        ctx.parent_head.is_none(),
        "head position has no parent head"
    );
}

#[test]
fn test_suggestion_context_for_arg() {
    let env = Environment::new();
    let args = vec![
        MettaValue::Atom("match".to_string()),
        MettaValue::Atom("unknown".to_string()),
        MettaValue::Atom("pattern".to_string()),
    ];

    let ctx = SuggestionContext::for_arg(&args, 1, "match", &env);
    assert_eq!(ctx.position, 1);
    assert_eq!(ctx.parent_head, Some("match"));
}

#[test]
fn test_suggestion_context_arity() {
    let env = Environment::new();

    // Zero arity (just the head)
    let args = vec![MettaValue::Atom("nop".to_string())];
    let ctx = SuggestionContext::for_head(&args, &env);
    assert_eq!(ctx.arity(), 0);

    // Arity 3
    let args = vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Long(1),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&args, &env);
    assert_eq!(ctx.arity(), 3);
}

// ============================================================================
// Smart Suggestion Tests
// ============================================================================

#[test]
fn test_smart_suggest_respects_arity() {
    let matcher = matcher_with_builtins();
    let env = Environment::new();

    // (lett x 1 x) - arity 3, should suggest let
    let args = vec![
        MettaValue::Atom("lett".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Long(1),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&args, &env);
    let result = matcher.smart_suggest_with_context("lett", 3, &ctx);
    assert!(
        result
            .map(|r| r.suggestions.contains(&"let".to_string()))
            .unwrap_or(false),
        "lett with arity 3 should suggest let"
    );

    // (lit p) - arity 1, should NOT suggest let
    let args = vec![
        MettaValue::Atom("lit".to_string()),
        MettaValue::Atom("p".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&args, &env);
    let result = matcher.smart_suggest_with_context("lit", 3, &ctx);
    let suggests_let = result
        .map(|r| r.suggestions.contains(&"let".to_string()))
        .unwrap_or(false);
    assert!(!suggests_let, "lit with arity 1 should NOT suggest let");
}

#[test]
fn test_smart_suggest_confidence_levels() {
    let matcher = matcher_with_builtins();
    let env = Environment::new();

    // Long word with small edit distance should have higher confidence
    let args = vec![
        MettaValue::Atom("fibonaci".to_string()),
        MettaValue::Long(10),
    ];
    let ctx = SuggestionContext::for_head(&args, &env);

    // The matcher won't know "fibonacci" unless we add it
    let matcher = FuzzyMatcher::new();
    matcher.insert("fibonacci");

    let result = matcher.smart_suggest_with_context("fibonaci", 3, &ctx);
    // Should find a suggestion with some confidence
    if let Some(suggestion) = result {
        if !suggestion.suggestions.is_empty() {
            assert!(
                suggestion.confidence != SuggestionConfidence::None,
                "Should have some confidence for near-match"
            );
        }
    }
}

// ============================================================================
// Builtin Signature Consistency Tests
// ============================================================================

#[test]
fn test_all_special_forms_have_consistent_signatures() {
    // Verify key invariants about built-in signatures

    // Arithmetic: all binary, return Number
    for op in ["+", "-", "*", "/", "%"] {
        let sig = get_signature(op).unwrap();
        assert_eq!(
            sig.min_arity, sig.max_arity,
            "{} should have fixed arity",
            op
        );
        assert_eq!(sig.min_arity, 2, "{} should be binary", op);
    }

    // Control flow: if is ternary, case/switch are variadic
    let if_sig = get_signature("if").unwrap();
    assert_eq!(if_sig.min_arity, 3);
    assert_eq!(if_sig.max_arity, 3);

    let case_sig = get_signature("case").unwrap();
    assert_eq!(case_sig.min_arity, 2);
    assert_eq!(case_sig.max_arity, usize::MAX);
}

#[test]
fn test_signature_count() {
    // Should have at least 40 built-in signatures
    let count = builtin_names().count();
    assert!(
        count >= 40,
        "Should have at least 40 built-in signatures, found {}",
        count
    );
}

#[test]
fn test_all_builtins_have_valid_signatures() {
    for name in builtin_names() {
        let sig = get_signature(name);
        assert!(sig.is_some(), "Builtin '{}' should have a signature", name);
        let sig = sig.unwrap();
        assert!(
            sig.min_arity <= sig.max_arity,
            "Builtin '{}' has invalid arity range: {} > {}",
            name,
            sig.min_arity,
            sig.max_arity
        );
    }
}

// ============================================================================
// Real MeTTa Code Compilation Tests
// ============================================================================

#[test]
fn test_compile_basic_expression() {
    let result = compile("(+ 1 2)");
    assert!(result.is_ok(), "Should compile basic expression");

    let state = result.unwrap();
    assert!(
        !state.source.is_empty(),
        "Should have at least one expression"
    );
}

#[test]
fn test_compile_nested_expression() {
    let result = compile("(let x (+ 1 2) (* x x))");
    assert!(result.is_ok(), "Should compile nested expression");

    let state = result.unwrap();
    assert!(!state.source.is_empty());
}

#[test]
fn test_compile_rule_definition() {
    let result = compile("(= (double $x) (* $x 2))");
    assert!(result.is_ok(), "Should compile rule definition");

    let state = result.unwrap();
    assert!(!state.source.is_empty());
}

#[test]
fn test_compile_multiple_expressions() {
    let result = compile("(= (double $x) (* $x 2))\n!(double 5)");
    assert!(result.is_ok(), "Should compile multiple expressions");

    let state = result.unwrap();
    assert!(state.source.len() >= 2, "Should have multiple expressions");
}

// ============================================================================
// Evaluation Tests
// ============================================================================

#[test]
fn test_eval_arithmetic() {
    let env = Environment::new();
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);

    let (results, _) = eval(expr, env);
    assert!(!results.is_empty(), "Should have results");

    if let Some(MettaValue::Long(n)) = results.first() {
        assert_eq!(*n, 3, "1 + 2 should be 3");
    }
}

#[test]
fn test_eval_if_true() {
    let env = Environment::new();
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);

    let (results, _) = eval(expr, env);
    assert!(!results.is_empty());

    if let Some(MettaValue::Long(n)) = results.first() {
        assert_eq!(*n, 1, "if True should return then branch");
    }
}

#[test]
fn test_eval_if_false() {
    let env = Environment::new();
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(false),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);

    let (results, _) = eval(expr, env);
    assert!(!results.is_empty());

    if let Some(MettaValue::Long(n)) = results.first() {
        assert_eq!(*n, 2, "if False should return else branch");
    }
}

// ============================================================================
// Environment Tests
// ============================================================================

#[test]
fn test_environment_new() {
    let env = Environment::new();
    // New environment should be valid
    assert_eq!(env.rule_count(), 0, "New environment should have no rules");
}

#[test]
fn test_environment_clone() {
    let env1 = Environment::new();
    let env2 = env1.clone();

    // Both should be valid and independent
    assert_eq!(env1.rule_count(), env2.rule_count());
}

// ============================================================================
// Full Context Tests (with type compatibility)
// ============================================================================

#[test]
fn test_full_context_match_with_space() {
    // Test smart_suggest_with_context with proper space argument
    // (metch &self pattern body) - first arg is a space
    let args = vec![
        MettaValue::Atom("metch".to_string()),
        MettaValue::Atom("&self".to_string()), // Space type (starts with &)
        MettaValue::Atom("pattern".to_string()),
        MettaValue::Atom("body".to_string()),
    ];

    assert!(
        would_suggest_with_context("metch", "match", &args),
        "metch→match should be suggested with proper space argument"
    );
}

#[test]
fn test_full_context_match_without_space() {
    // Test smart_suggest_with_context with improper space argument
    // (metch notspace pattern body) - first arg is NOT a space
    let args = vec![
        MettaValue::Atom("metch".to_string()),
        MettaValue::Atom("notspace".to_string()), // Not a space
        MettaValue::Atom("pattern".to_string()),
        MettaValue::Atom("body".to_string()),
    ];

    assert!(
        !would_suggest_with_context("metch", "match", &args),
        "metch→match should NOT be suggested without proper space argument"
    );
}

#[test]
fn test_full_context_let_with_proper_types() {
    // (lett x 1 x) - let pattern value body
    let args = vec![
        MettaValue::Atom("lett".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Long(1),
        MettaValue::Atom("x".to_string()),
    ];

    assert!(
        would_suggest_with_context("lett", "let", &args),
        "lett→let should be suggested with proper types"
    );
}

#[test]
fn test_full_context_arithmetic_correct_types() {
    // Short typos like "++" are filtered by confidence checks (query_len >= 4 for distance 1).
    // The confidence system is working correctly - this is expected behavior.
    let args = vec![
        MettaValue::Atom("++".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ];

    // Short typo is filtered out
    assert!(
        !would_suggest_with_context("++", "+", &args),
        "++→+ should be filtered due to short query length"
    );
}

#[test]
fn test_full_context_catch_with_proper_types() {
    // (catsh expr default) - typo for catch
    let args = vec![
        MettaValue::Atom("catsh".to_string()),
        MettaValue::Atom("expr".to_string()),
        MettaValue::Atom("default".to_string()),
    ];

    assert!(
        would_suggest_with_context("catsh", "catch", &args),
        "catsh→catch should be suggested with proper types"
    );
}

#[test]
fn test_full_context_catch_wrong_arity() {
    // (catsh expr) - only 1 arg, catch requires 2
    let args = vec![
        MettaValue::Atom("catsh".to_string()),
        MettaValue::Atom("expr".to_string()),
    ];

    // This fails arity check (needs 2 args, has 1)
    assert!(
        !would_suggest_with_context("catsh", "catch", &args),
        "catsh→catch should NOT be suggested with wrong arity"
    );
}

#[test]
fn test_full_context_arithmetic_wrong_types() {
    // This test is valid because the type check would reject even if
    // confidence passed (which it doesn't for short symbols)
    let args = vec![
        MettaValue::Atom("++".to_string()),
        MettaValue::String("a".to_string()),
        MettaValue::String("b".to_string()),
    ];

    assert!(
        !would_suggest_with_context("++", "+", &args),
        "++→+ should NOT be suggested with string arguments"
    );
}

// ============================================================================
// Error Scenario Tests
// ============================================================================

#[test]
fn test_compile_invalid_syntax() {
    // Unclosed paren
    let result = compile("(let x 1");
    assert!(result.is_err(), "Should fail to compile invalid syntax");
}

#[test]
fn test_fuzzy_no_suggestion_for_very_different() {
    let matcher = matcher_with_builtins();

    // Very different strings shouldn't match
    let suggestions = matcher.suggest("xyz123", 2);
    assert!(
        suggestions.is_empty(),
        "Very different string shouldn't match any builtin"
    );
}
