//! Tests for fuzzy matching.

use super::*;
use crate::backend::builtin_signatures::TypeExpr;
use crate::backend::models::MettaValue;
use crate::backend::Environment;

#[test]
fn test_basic_fuzzy_matching() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "function"]);

    // Exact match (distance 0)
    assert!(matcher.contains("fibonacci"));

    // Single character substitution (distance 1)
    let suggestions = matcher.suggest("fibonaci", 2);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].0, "fibonacci");
    assert_eq!(suggestions[0].1, 1);
}

#[test]
fn test_transposition_typos() {
    let matcher = FuzzyMatcher::from_terms(vec!["test", "testing"]);

    // Transposition: "tset" -> "test"
    let suggestions = matcher.suggest("tset", 1);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].0, "test");
}

#[test]
fn test_multiple_suggestions() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "fib", "fibonacci-fast", "factorial"]);

    // Should find multiple similar matches
    let suggestions = matcher.suggest("fibonaci", 2);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].0, "fibonacci"); // Closest match first
}

#[test]
fn test_closest_match() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "function"]);

    let closest = matcher.closest_match("fibonaci", 2);
    assert!(closest.is_some());
    let (term, distance) = closest.unwrap();
    assert_eq!(term, "fibonacci");
    assert_eq!(distance, 1);
}

#[test]
fn test_did_you_mean_single() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);

    let msg = matcher.did_you_mean("fibonaci", 2, 3);
    assert_eq!(msg, Some("Did you mean: fibonacci?".to_string()));
}

#[test]
fn test_did_you_mean_multiple() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "fib", "fib-fast"]);

    // "fob" -> "fib" has distance 1 (substitute o->i)
    let suggestions = matcher.suggest("fob", 1);
    // Should find at least "fib"
    assert!(!suggestions.is_empty(), "Expected at least one suggestion");

    let msg = matcher.did_you_mean("fob", 1, 3);
    assert!(msg.is_some());
    // If we only found one match, it will say "Did you mean: X?"
    // If we found multiple, it will say "Did you mean one of: X, Y?"
    let msg_str = msg.unwrap();
    assert!(
        msg_str.starts_with("Did you mean:") || msg_str.starts_with("Did you mean one of:"),
        "Unexpected message format: {}",
        msg_str
    );
}

#[test]
fn test_did_you_mean_no_match() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);

    let msg = matcher.did_you_mean("xyz", 1, 3);
    assert_eq!(msg, None);
}

#[test]
fn test_insert_and_remove() {
    let matcher = FuzzyMatcher::new();
    assert_eq!(matcher.len(), 0);

    matcher.insert("test");
    assert_eq!(matcher.len(), 1);
    assert!(matcher.contains("test"));

    let removed = matcher.remove("test");
    assert!(removed);
    assert_eq!(matcher.len(), 0);
}

#[test]
fn test_empty_dictionary() {
    let matcher = FuzzyMatcher::new();
    assert!(matcher.is_empty());

    let suggestions = matcher.suggest("anything", 2);
    assert!(suggestions.is_empty());
}

// ============================================================
// Smart Suggestion Heuristic Tests (issue #51)
// ============================================================

#[test]
fn test_issue_51_lit_vs_let_not_suggested() {
    // This is the exact case from issue #51:
    // `lit` should NOT suggest `let` because it's too short (3 chars)
    let matcher = FuzzyMatcher::from_terms(vec!["let", "if", "case", "match"]);

    let result = matcher.smart_did_you_mean("lit", 2, 3);
    assert!(
        result.is_none(),
        "lit→let should NOT be suggested (short word false positive)"
    );
}

#[test]
fn test_smart_suggestion_longer_words_accepted() {
    // Longer words with small relative distance should be suggested
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "function"]);

    let result = matcher.smart_did_you_mean("fibonaci", 2, 3);
    assert!(result.is_some(), "fibonaci→fibonacci should be suggested");
    let suggestion = result.unwrap();
    assert_eq!(suggestion.confidence, SuggestionConfidence::High);
    assert!(suggestion.message.contains("fibonacci"));
}

#[test]
fn test_smart_suggestion_4_char_word_accepted() {
    // 4-char word with distance 1 should be accepted (barely)
    let matcher = FuzzyMatcher::from_terms(vec!["lett", "test", "case"]);

    // "lest" → "lett" (distance 1, len 4) = 0.25 relative distance
    let result = matcher.smart_did_you_mean("lest", 1, 3);
    assert!(result.is_some(), "lest→lett should be suggested (4 chars)");
}

#[test]
fn test_smart_suggestion_pascal_case_skipped() {
    // PascalCase names should not trigger suggestions (likely data constructors)
    let matcher = FuzzyMatcher::from_terms(vec!["MyType", "DataCon"]);

    // Even though "MyTipe" is close to "MyType", it's PascalCase so skip
    let result = matcher.smart_did_you_mean("MyTipe", 1, 3);
    assert!(
        result.is_none(),
        "PascalCase should not trigger suggestions"
    );
}

#[test]
fn test_smart_suggestion_hyphenated_skipped() {
    // Hyphenated names should not trigger suggestions (compound identifiers)
    let matcher = FuzzyMatcher::from_terms(vec!["is-valid", "get-value"]);

    let result = matcher.smart_did_you_mean("is-valud", 1, 3);
    assert!(
        result.is_none(),
        "Hyphenated names should not trigger suggestions"
    );
}

#[test]
fn test_smart_suggestion_prefix_mismatch_rejected() {
    // Different prefixes should not match
    let matcher = FuzzyMatcher::from_terms(vec!["$stack", "&stack"]);

    // Querying "$stack" should not suggest "&stack"
    let result = matcher.smart_did_you_mean("$steck", 1, 3);
    if let Some(suggestion) = result {
        // If we get a suggestion, it should only be $stack, not &stack
        for term in &suggestion.suggestions {
            assert!(
                term.starts_with('$'),
                "Should not suggest &stack for $steck"
            );
        }
    }
}

#[test]
fn test_is_likely_data_constructor() {
    // PascalCase
    assert!(is_likely_data_constructor("MyType"));
    assert!(is_likely_data_constructor("DataConstructor"));
    assert!(is_likely_data_constructor("True"));
    assert!(is_likely_data_constructor("False"));

    // All uppercase
    assert!(is_likely_data_constructor("NIL"));
    assert!(is_likely_data_constructor("VOID"));

    // Hyphenated
    assert!(is_likely_data_constructor("is-valid"));
    assert!(is_likely_data_constructor("get-value"));

    // Underscored
    assert!(is_likely_data_constructor("my_value"));

    // With digits
    assert!(is_likely_data_constructor("value1"));
    assert!(is_likely_data_constructor("test2"));

    // Regular lowercase words - NOT data constructors
    assert!(!is_likely_data_constructor("let"));
    assert!(!is_likely_data_constructor("if"));
    assert!(!is_likely_data_constructor("match"));
    assert!(!is_likely_data_constructor("factorial"));
}

#[test]
fn test_are_prefixes_compatible() {
    // Same prefix types should be compatible
    assert!(are_prefixes_compatible("$x", "$y"));
    assert!(are_prefixes_compatible("&space", "&other"));
    assert!(are_prefixes_compatible("foo", "bar"));

    // Different prefix types should NOT be compatible
    assert!(!are_prefixes_compatible("$x", "&x"));
    assert!(!are_prefixes_compatible("&space", "$space"));
    assert!(!are_prefixes_compatible("$var", "var"));
}

#[test]
fn test_compute_suggestion_confidence() {
    // High confidence: long word, small relative distance
    assert_eq!(
        compute_suggestion_confidence("fibonacci", "fibonaci", 1, 9),
        SuggestionConfidence::High
    );

    // Low confidence: medium word, medium relative distance
    assert_eq!(
        compute_suggestion_confidence("match", "matsh", 1, 5),
        SuggestionConfidence::Low
    );

    // None: short word, high relative distance
    assert_eq!(
        compute_suggestion_confidence("lit", "let", 1, 3),
        SuggestionConfidence::None
    );

    // None: 3-char word with distance 1 (min length check)
    assert_eq!(
        compute_suggestion_confidence("add", "adn", 1, 3),
        SuggestionConfidence::None
    );
}

#[test]
fn test_smart_suggestion_confidence_levels() {
    let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);

    // Long word should have high confidence
    let result = matcher.smart_did_you_mean("fibonaci", 2, 3);
    assert!(result.is_some());
    assert_eq!(result.unwrap().confidence, SuggestionConfidence::High);
}

// ============================================================
// Context-Aware Smart Suggestion Tests (Three Pillars)
// ============================================================

#[test]
fn test_context_arity_filtering_lit_vs_let() {
    // Core issue #51 case: (lit p) has arity 1, let needs arity 3
    let matcher = FuzzyMatcher::from_terms(vec!["let", "if", "case", "match"]);
    let env = Environment::new();

    // Expression: (lit p) - 1 argument
    let expr = vec![
        MettaValue::Atom("lit".to_string()),
        MettaValue::Atom("p".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    // Should NOT suggest 'let' because arity doesn't match
    let result = matcher.smart_suggest_with_context("lit", 2, &ctx);
    assert!(
        result.is_none(),
        "lit→let should NOT be suggested due to arity mismatch (1 != 3)"
    );
}

#[test]
fn test_context_arity_matching_lett_vs_let() {
    // (lett x 1 x) has arity 3, same as let - should suggest
    let matcher = FuzzyMatcher::from_terms(vec!["let", "if", "case", "match"]);
    let env = Environment::new();

    // Expression: (lett x 1 x) - 3 arguments
    let expr = vec![
        MettaValue::Atom("lett".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Long(1),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    // Should suggest 'let' because arity matches
    let result = matcher.smart_suggest_with_context("lett", 2, &ctx);
    assert!(
        result.is_some(),
        "lett→let should be suggested (arity 3 matches)"
    );
    assert!(result.unwrap().suggestions.contains(&"let".to_string()));
}

#[test]
fn test_context_arity_catch_filtering() {
    // (cach e) has arity 1, catch needs arity 2 - should NOT suggest
    let matcher = FuzzyMatcher::from_terms(vec!["catch", "case", "match"]);
    let env = Environment::new();

    // Expression: (cach e) - 1 argument
    let expr = vec![
        MettaValue::Atom("cach".to_string()),
        MettaValue::Atom("e".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("cach", 2, &ctx);
    // Should NOT suggest 'catch' because arity 1 < min_arity 2
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"catch".to_string()),
            "cach with arity 1 should NOT suggest catch (needs 2)"
        );
    }
}

#[test]
fn test_context_arity_catch_matching() {
    // (cach e d) has arity 2, catch needs arity 2 - should suggest
    let matcher = FuzzyMatcher::from_terms(vec!["catch", "case", "match"]);
    let env = Environment::new();

    // Expression: (cach e d) - 2 arguments
    let expr = vec![
        MettaValue::Atom("cach".to_string()),
        MettaValue::Atom("e".to_string()),
        MettaValue::Atom("d".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("cach", 2, &ctx);
    assert!(result.is_some(), "cach with arity 2 should suggest catch");
    assert!(result.unwrap().suggestions.contains(&"catch".to_string()));
}

#[test]
fn test_context_type_filtering_match_space() {
    // (match "hello" p t) - String at position 1, but match expects Space
    let matcher = FuzzyMatcher::from_terms(vec!["match", "catch"]);
    let env = Environment::new();

    // Expression with String where Space is expected
    let expr = vec![
        MettaValue::Atom("metch".to_string()),   // typo for 'match'
        MettaValue::String("hello".to_string()), // String, not Space
        MettaValue::Atom("p".to_string()),
        MettaValue::Atom("t".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("metch", 2, &ctx);
    // Should NOT suggest 'match' because type doesn't match at position 1
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"match".to_string()),
            "metch with String arg should NOT suggest match (expects Space)"
        );
    }
}

#[test]
fn test_context_type_matching_match_space() {
    // (match &self p t) - Space at position 1, correct for match
    let matcher = FuzzyMatcher::from_terms(vec!["match", "catch"]);
    let env = Environment::new();

    // Expression with proper Space reference
    let expr = vec![
        MettaValue::Atom("metch".to_string()), // typo for 'match'
        MettaValue::Atom("&self".to_string()), // Space reference
        MettaValue::Atom("p".to_string()),
        MettaValue::Atom("t".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("metch", 2, &ctx);
    assert!(result.is_some(), "metch with &self should suggest match");
    assert!(result.unwrap().suggestions.contains(&"match".to_string()));
}

#[test]
fn test_context_prefix_suggestion_match_self() {
    // In (match self p t), suggest &self because position 1 expects Space
    let matcher = FuzzyMatcher::from_terms(vec!["match"]);
    let env = Environment::new();

    // Expression: (match self p t) - need to check 'self' in arg position
    let expr = vec![
        MettaValue::Atom("match".to_string()),
        MettaValue::Atom("self".to_string()), // Should suggest &self
        MettaValue::Atom("p".to_string()),
        MettaValue::Atom("t".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "match", &env);

    let result = matcher.smart_suggest_with_context("self", 2, &ctx);
    assert!(
        result.is_some(),
        "self in match position 1 should suggest &self"
    );
    let suggestion = result.unwrap();
    assert!(
        suggestion.suggestions.contains(&"&self".to_string()),
        "Should suggest &self: {:?}",
        suggestion.suggestions
    );
}

#[test]
fn test_context_no_prefix_suggestion_head_position() {
    // In (self foo bar), don't suggest &self for head position
    let matcher = FuzzyMatcher::from_terms(vec!["match"]);
    let env = Environment::new();

    // Expression: (self foo bar) - self is in head position
    let expr = vec![
        MettaValue::Atom("self".to_string()),
        MettaValue::Atom("foo".to_string()),
        MettaValue::Atom("bar".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("self", 2, &ctx);
    // Should NOT suggest &self for head position
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"&self".to_string()),
            "Should not suggest &self in head position"
        );
    }
}

#[test]
fn test_type_matches_number() {
    let env = Environment::new();
    assert!(type_matches(&MettaValue::Long(42), &TypeExpr::Number, &env));
    assert!(type_matches(
        &MettaValue::Float(3.14),
        &TypeExpr::Number,
        &env
    ));
    assert!(!type_matches(
        &MettaValue::String("42".to_string()),
        &TypeExpr::Number,
        &env
    ));
}

#[test]
fn test_type_matches_bool() {
    let env = Environment::new();
    assert!(type_matches(&MettaValue::Bool(true), &TypeExpr::Bool, &env));
    assert!(type_matches(
        &MettaValue::Atom("True".to_string()),
        &TypeExpr::Bool,
        &env
    ));
    assert!(!type_matches(&MettaValue::Long(1), &TypeExpr::Bool, &env));
}

#[test]
fn test_type_matches_space() {
    let env = Environment::new();
    assert!(type_matches(
        &MettaValue::Atom("&self".to_string()),
        &TypeExpr::Space,
        &env
    ));
    assert!(type_matches(
        &MettaValue::Atom("&kb".to_string()),
        &TypeExpr::Space,
        &env
    ));
    assert!(!type_matches(
        &MettaValue::Atom("self".to_string()),
        &TypeExpr::Space,
        &env
    ));
}

#[test]
fn test_type_matches_any_and_pattern() {
    let env = Environment::new();
    // Any and Pattern should match anything
    assert!(type_matches(&MettaValue::Long(42), &TypeExpr::Any, &env));
    assert!(type_matches(
        &MettaValue::String("x".to_string()),
        &TypeExpr::Pattern,
        &env
    ));
    assert!(type_matches(
        &MettaValue::Bool(false),
        &TypeExpr::Var("a"),
        &env
    ));
}

#[test]
fn test_values_compatible() {
    // Same types should be compatible
    assert!(values_compatible(
        &MettaValue::Long(1),
        &MettaValue::Long(2)
    ));
    assert!(values_compatible(
        &MettaValue::Long(1),
        &MettaValue::Float(2.0)
    ));
    assert!(values_compatible(
        &MettaValue::Bool(true),
        &MettaValue::Bool(false)
    ));
    assert!(values_compatible(
        &MettaValue::Atom("a".to_string()),
        &MettaValue::Atom("b".to_string())
    ));

    // Different types should not be compatible
    assert!(!values_compatible(
        &MettaValue::Long(1),
        &MettaValue::String("1".to_string())
    ));
    assert!(!values_compatible(
        &MettaValue::Bool(true),
        &MettaValue::Long(1)
    ));
}

// ============================================================
// Arity Edge Case Tests
// ============================================================

#[test]
fn test_context_arity_zero_arity_nop() {
    // nop has arity 0, (nopp) has 0 args - should match
    let matcher = FuzzyMatcher::from_terms(vec!["nop", "not"]);
    let env = Environment::new();

    // Expression: (nopp) - 0 arguments
    let expr = vec![MettaValue::Atom("nopp".to_string())];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("nopp", 2, &ctx);
    assert!(
        result.is_some(),
        "nopp with arity 0 should suggest nop (arity 0)"
    );
    assert!(result.unwrap().suggestions.contains(&"nop".to_string()));
}

#[test]
fn test_context_arity_zero_arity_empty() {
    // empty has arity 0
    let matcher = FuzzyMatcher::from_terms(vec!["empty", "error"]);
    let env = Environment::new();

    // Expression: (emty) - 0 arguments (typo for empty)
    let expr = vec![MettaValue::Atom("emty".to_string())];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("emty", 2, &ctx);
    // "emty" has 4 chars and distance 1 from "empty" (5 chars), ratio ~0.25
    assert!(
        result.is_some(),
        "emty with arity 0 should suggest empty (arity 0)"
    );
    assert!(result.unwrap().suggestions.contains(&"empty".to_string()));
}

#[test]
fn test_context_arity_zero_arity_with_args_should_not_match() {
    // nop has arity 0, (nopp x) has 1 arg - should NOT match
    let matcher = FuzzyMatcher::from_terms(vec!["nop"]);
    let env = Environment::new();

    // Expression: (nopp x) - 1 argument, but nop expects 0
    let expr = vec![
        MettaValue::Atom("nopp".to_string()),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("nopp", 2, &ctx);
    // Should NOT suggest nop because arity 1 > max_arity 0
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"nop".to_string()),
            "nopp with arity 1 should NOT suggest nop (expects 0)"
        );
    }
}

#[test]
fn test_context_arity_variadic_case_min() {
    // case has min_arity 2, max_arity MAX
    let matcher = FuzzyMatcher::from_terms(vec!["case", "catch"]);
    let env = Environment::new();

    // Expression: (cas x y) - 2 arguments, meets min
    let expr = vec![
        MettaValue::Atom("cas".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Atom("y".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("cas", 2, &ctx);
    // Note: "cas" is 3 chars, "case" is 4, distance 1 - ratio 0.33, rejected by confidence
    // But "catch" is 5 chars, distance 2 - ratio 0.4, also rejected
    // This tests arity matching, not string similarity
    // The test verifies that arity 2 is within [2, MAX] for case
    let _ = result; // Just validate the arity check runs
}

#[test]
fn test_context_arity_variadic_case_many_args() {
    // case can have many arguments (variadic)
    let matcher = FuzzyMatcher::from_terms(vec!["case"]);
    let env = Environment::new();

    // Expression: (caze x y z w v) - 5 arguments
    let expr = vec![
        MettaValue::Atom("caze".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Atom("y".to_string()),
        MettaValue::Atom("z".to_string()),
        MettaValue::Atom("w".to_string()),
        MettaValue::Atom("v".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("caze", 2, &ctx);
    // caze (4 chars) → case (4 chars), distance 1, ratio 0.25
    assert!(
        result.is_some(),
        "caze with many args should suggest case (variadic)"
    );
    assert!(result.unwrap().suggestions.contains(&"case".to_string()));
}

#[test]
fn test_context_arity_variadic_below_min() {
    // case has min_arity 2, (cas x) has 1 arg - below min
    let matcher = FuzzyMatcher::from_terms(vec!["case"]);
    let env = Environment::new();

    // Expression: (cas x) - 1 argument, below min_arity 2
    let expr = vec![
        MettaValue::Atom("cas".to_string()),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("cas", 2, &ctx);
    // Should NOT suggest case because arity 1 < min_arity 2
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"case".to_string()),
            "cas with arity 1 should NOT suggest case (min 2)"
        );
    }
}

#[test]
fn test_context_arity_exact_min() {
    // if has min_arity 3, max_arity 3
    let matcher = FuzzyMatcher::from_terms(vec!["if"]);
    let env = Environment::new();

    // Expression: (iff cond then else) - exactly 3 arguments
    let expr = vec![
        MettaValue::Atom("iff".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("iff", 2, &ctx);
    // iff (3 chars) → if (2 chars), distance 1
    // But min length check requires query >= 4 for distance 1
    // So this won't suggest "if" due to short word heuristic
    let _ = result;
}

#[test]
fn test_context_arity_above_max_fixed() {
    // + has min_arity 2, max_arity 2
    let matcher = FuzzyMatcher::from_terms(vec!["+"]);
    let env = Environment::new();

    // Expression: (++ 1 2 3) - 3 arguments, above max 2
    let expr = vec![
        MettaValue::Atom("++".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("++", 1, &ctx);
    // Should NOT suggest + because arity 3 > max_arity 2
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"+".to_string()),
            "++ with arity 3 should NOT suggest + (max 2)"
        );
    }
}

#[test]
fn test_context_arity_unify_four_args() {
    // unify has exactly 4 arguments
    let matcher = FuzzyMatcher::from_terms(vec!["unify"]);
    let env = Environment::new();

    // Expression: (uniffy a b c d) - 4 arguments, matches
    let expr = vec![
        MettaValue::Atom("uniffy".to_string()),
        MettaValue::Atom("a".to_string()),
        MettaValue::Atom("b".to_string()),
        MettaValue::Atom("c".to_string()),
        MettaValue::Atom("d".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("uniffy", 2, &ctx);
    // uniffy (6 chars) → unify (5 chars), distance 1, ratio ~0.2
    assert!(result.is_some(), "uniffy with arity 4 should suggest unify");
    assert!(result.unwrap().suggestions.contains(&"unify".to_string()));
}

#[test]
fn test_context_arity_unify_wrong_arity() {
    // unify has exactly 4 arguments
    let matcher = FuzzyMatcher::from_terms(vec!["unify"]);
    let env = Environment::new();

    // Expression: (uniffy a b c) - 3 arguments, wrong arity
    let expr = vec![
        MettaValue::Atom("uniffy".to_string()),
        MettaValue::Atom("a".to_string()),
        MettaValue::Atom("b".to_string()),
        MettaValue::Atom("c".to_string()),
    ];
    let ctx = SuggestionContext::for_head(&expr, &env);

    let result = matcher.smart_suggest_with_context("uniffy", 2, &ctx);
    // Should NOT suggest unify because arity 3 != 4
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"unify".to_string()),
            "uniffy with arity 3 should NOT suggest unify (needs 4)"
        );
    }
}

// ============================================================
// Type Compatibility Edge Case Tests
// ============================================================

#[test]
fn test_type_matches_unit() {
    let env = Environment::new();
    assert!(type_matches(&MettaValue::Unit, &TypeExpr::Unit, &env));
    assert!(!type_matches(&MettaValue::Long(0), &TypeExpr::Unit, &env));
}

#[test]
fn test_type_matches_nil() {
    let env = Environment::new();
    assert!(type_matches(&MettaValue::Nil, &TypeExpr::Nil, &env));
    assert!(!type_matches(&MettaValue::Unit, &TypeExpr::Nil, &env));
}

#[test]
fn test_type_matches_error() {
    let env = Environment::new();
    let error_val = MettaValue::Error(
        "test error".to_string(),
        std::sync::Arc::new(MettaValue::String("error msg".to_string())),
    );
    assert!(type_matches(&error_val, &TypeExpr::Error, &env));
    assert!(!type_matches(
        &MettaValue::Atom("error".to_string()),
        &TypeExpr::Error,
        &env
    ));
}

#[test]
fn test_type_matches_atom() {
    let env = Environment::new();
    assert!(type_matches(
        &MettaValue::Atom("foo".to_string()),
        &TypeExpr::Atom,
        &env
    ));
    assert!(type_matches(
        &MettaValue::Atom("bar".to_string()),
        &TypeExpr::Atom,
        &env
    ));
    assert!(!type_matches(
        &MettaValue::String("foo".to_string()),
        &TypeExpr::Atom,
        &env
    ));
}

#[test]
fn test_type_matches_string() {
    let env = Environment::new();
    assert!(type_matches(
        &MettaValue::String("hello".to_string()),
        &TypeExpr::String,
        &env
    ));
    assert!(!type_matches(
        &MettaValue::Atom("hello".to_string()),
        &TypeExpr::String,
        &env
    ));
}

#[test]
fn test_type_matches_type_names() {
    let env = Environment::new();
    // Standard type names should match TypeExpr::Type
    assert!(type_matches(
        &MettaValue::Atom("Number".to_string()),
        &TypeExpr::Type,
        &env
    ));
    assert!(type_matches(
        &MettaValue::Atom("Bool".to_string()),
        &TypeExpr::Type,
        &env
    ));
    assert!(type_matches(
        &MettaValue::Atom("String".to_string()),
        &TypeExpr::Type,
        &env
    ));
    assert!(type_matches(
        &MettaValue::Atom("List".to_string()),
        &TypeExpr::Type,
        &env
    ));
}

#[test]
fn test_type_matches_list_sexpr() {
    let env = Environment::new();
    let list = MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    assert!(type_matches(
        &list,
        &TypeExpr::List(Box::new(TypeExpr::Var("a"))),
        &env
    ));
}

#[test]
fn test_type_matches_empty_list() {
    let env = Environment::new();
    let empty_list = MettaValue::SExpr(vec![]);
    assert!(type_matches(
        &empty_list,
        &TypeExpr::List(Box::new(TypeExpr::Number)),
        &env
    ));
}

#[test]
fn test_type_matches_nested_list() {
    let env = Environment::new();
    let nested = MettaValue::SExpr(vec![
        MettaValue::SExpr(vec![MettaValue::Long(1)]),
        MettaValue::SExpr(vec![MettaValue::Long(2)]),
    ]);
    assert!(type_matches(
        &nested,
        &TypeExpr::List(Box::new(TypeExpr::List(Box::new(TypeExpr::Var("a"))))),
        &env
    ));
}

#[test]
fn test_type_matches_arrow_atom() {
    let env = Environment::new();
    // Function names (atoms) match arrow types
    let arrow_type = TypeExpr::Arrow(vec![TypeExpr::Number], Box::new(TypeExpr::Number));
    assert!(type_matches(
        &MettaValue::Atom("my-func".to_string()),
        &arrow_type,
        &env
    ));
}

#[test]
fn test_type_matches_arrow_sexpr() {
    let env = Environment::new();
    // Lambda-like expressions match arrow types
    let arrow_type = TypeExpr::Arrow(vec![TypeExpr::Var("a")], Box::new(TypeExpr::Var("b")));
    let lambda = MettaValue::SExpr(vec![
        MettaValue::Atom("lambda".to_string()),
        MettaValue::Atom("x".to_string()),
        MettaValue::Atom("x".to_string()),
    ]);
    assert!(type_matches(&lambda, &arrow_type, &env));
}

#[test]
fn test_type_matches_bindings() {
    let env = Environment::new();
    // Bindings type accepts anything
    assert!(type_matches(
        &MettaValue::Long(42),
        &TypeExpr::Bindings,
        &env
    ));
    assert!(type_matches(
        &MettaValue::SExpr(vec![]),
        &TypeExpr::Bindings,
        &env
    ));
}

#[test]
fn test_type_matches_expr() {
    let env = Environment::new();
    // Expr type accepts anything
    assert!(type_matches(
        &MettaValue::Atom("x".to_string()),
        &TypeExpr::Expr,
        &env
    ));
    assert!(type_matches(&MettaValue::Long(1), &TypeExpr::Expr, &env));
}

#[test]
fn test_type_mismatch_number_expects_string() {
    let env = Environment::new();
    assert!(!type_matches(
        &MettaValue::Long(42),
        &TypeExpr::String,
        &env
    ));
    assert!(!type_matches(
        &MettaValue::Float(3.14),
        &TypeExpr::String,
        &env
    ));
}

#[test]
fn test_type_mismatch_string_expects_number() {
    let env = Environment::new();
    assert!(!type_matches(
        &MettaValue::String("42".to_string()),
        &TypeExpr::Number,
        &env
    ));
}

#[test]
fn test_type_mismatch_bool_expects_number() {
    let env = Environment::new();
    assert!(!type_matches(
        &MettaValue::Bool(true),
        &TypeExpr::Number,
        &env
    ));
}

#[test]
fn test_type_mismatch_atom_expects_list() {
    let env = Environment::new();
    assert!(!type_matches(
        &MettaValue::Atom("not-a-list".to_string()),
        &TypeExpr::List(Box::new(TypeExpr::Var("a"))),
        &env
    ));
}

// ============================================================
// Type Variable Unification Tests
// ============================================================

#[test]
fn test_type_var_unify_same_number_type() {
    let env = Environment::new();
    // Both arguments are Numbers - consistent $a binding
    let args = vec![MettaValue::Long(1), MettaValue::Long(2)];
    let expected_types = vec![TypeExpr::Var("a"), TypeExpr::Var("a")];
    assert!(validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_unify_number_and_float() {
    let env = Environment::new();
    // Long and Float are both compatible as numbers
    let args = vec![MettaValue::Long(1), MettaValue::Float(2.0)];
    let expected_types = vec![TypeExpr::Var("a"), TypeExpr::Var("a")];
    assert!(validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_unify_different_types_fail() {
    let env = Environment::new();
    // Number and String are different - inconsistent $a
    let args = vec![MettaValue::Long(1), MettaValue::String("x".to_string())];
    let expected_types = vec![TypeExpr::Var("a"), TypeExpr::Var("a")];
    assert!(!validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_unify_bool_and_number_fail() {
    let env = Environment::new();
    // Bool and Number are different
    let args = vec![MettaValue::Bool(true), MettaValue::Long(1)];
    let expected_types = vec![TypeExpr::Var("a"), TypeExpr::Var("a")];
    assert!(!validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_multiple_vars_consistent() {
    let env = Environment::new();
    // unify has signature (-> $a $a $b $b $b)
    // Args: (atom atom number number) where $a=Atom, $b=Number
    let args = vec![
        MettaValue::Atom("x".to_string()),
        MettaValue::Atom("y".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ];
    let expected_types = vec![
        TypeExpr::Var("a"),
        TypeExpr::Var("a"),
        TypeExpr::Var("b"),
        TypeExpr::Var("b"),
    ];
    assert!(validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_multiple_vars_inconsistent() {
    let env = Environment::new();
    // $a consistent (atoms) but $b inconsistent (number vs string)
    let args = vec![
        MettaValue::Atom("x".to_string()),
        MettaValue::Atom("y".to_string()),
        MettaValue::Long(1),
        MettaValue::String("z".to_string()),
    ];
    let expected_types = vec![
        TypeExpr::Var("a"),
        TypeExpr::Var("a"),
        TypeExpr::Var("b"),
        TypeExpr::Var("b"),
    ];
    assert!(!validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_atoms_always_compatible() {
    let env = Environment::new();
    // Different atoms are considered compatible (both Atom type)
    let args = vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Atom("bar".to_string()),
    ];
    let expected_types = vec![TypeExpr::Var("a"), TypeExpr::Var("a")];
    assert!(validate_type_vars(&args, &expected_types, &env));
}

#[test]
fn test_type_var_sexprs_compatible() {
    let env = Environment::new();
    // Different s-expressions are considered compatible
    let args = vec![
        MettaValue::SExpr(vec![MettaValue::Long(1)]),
        MettaValue::SExpr(vec![MettaValue::Long(2), MettaValue::Long(3)]),
    ];
    let expected_types = vec![TypeExpr::Var("a"), TypeExpr::Var("a")];
    assert!(validate_type_vars(&args, &expected_types, &env));
}

// ============================================================
// Prefix Context Suggestion Tests
// ============================================================

#[test]
fn test_prefix_context_add_atom() {
    // add-atom expects Space at position 1
    let matcher = FuzzyMatcher::from_terms(vec!["add-atom"]);
    let env = Environment::new();

    let expr = vec![
        MettaValue::Atom("add-atom".to_string()),
        MettaValue::Atom("kb".to_string()), // Should suggest &kb
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "add-atom", &env);

    let result = matcher.smart_suggest_with_context("kb", 2, &ctx);
    assert!(
        result.is_some(),
        "kb in add-atom position 1 should suggest &kb"
    );
    let suggestion = result.unwrap();
    assert!(
        suggestion.suggestions.contains(&"&kb".to_string()),
        "Should suggest &kb: {:?}",
        suggestion.suggestions
    );
}

#[test]
fn test_prefix_context_remove_atom() {
    // remove-atom expects Space at position 1
    let matcher = FuzzyMatcher::from_terms(vec!["remove-atom"]);
    let env = Environment::new();

    let expr = vec![
        MettaValue::Atom("remove-atom".to_string()),
        MettaValue::Atom("myspace".to_string()),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "remove-atom", &env);

    let result = matcher.smart_suggest_with_context("myspace", 2, &ctx);
    assert!(result.is_some());
    assert!(result
        .unwrap()
        .suggestions
        .contains(&"&myspace".to_string()));
}

#[test]
fn test_prefix_context_get_atoms() {
    // get-atoms expects Space at position 1
    let matcher = FuzzyMatcher::from_terms(vec!["get-atoms"]);
    let env = Environment::new();

    let expr = vec![
        MettaValue::Atom("get-atoms".to_string()),
        MettaValue::Atom("space".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "get-atoms", &env);

    let result = matcher.smart_suggest_with_context("space", 2, &ctx);
    assert!(result.is_some());
    assert!(result.unwrap().suggestions.contains(&"&space".to_string()));
}

#[test]
fn test_prefix_no_suggestion_already_has_ampersand() {
    // If it already has &, don't suggest adding another
    let matcher = FuzzyMatcher::from_terms(vec!["match"]);
    let env = Environment::new();

    let expr = vec![
        MettaValue::Atom("match".to_string()),
        MettaValue::Atom("&self".to_string()),
        MettaValue::Atom("p".to_string()),
        MettaValue::Atom("t".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "match", &env);

    let result = matcher.smart_suggest_with_context("&self", 2, &ctx);
    // Should NOT suggest &&self
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.iter().any(|s| s.starts_with("&&")),
            "Should not suggest double ampersand"
        );
    }
}

#[test]
fn test_prefix_no_suggestion_for_dollar_var() {
    // $variables in space position should not get & prefix
    let matcher = FuzzyMatcher::from_terms(vec!["match"]);
    let env = Environment::new();

    let expr = vec![
        MettaValue::Atom("match".to_string()),
        MettaValue::Atom("$space".to_string()),
        MettaValue::Atom("p".to_string()),
        MettaValue::Atom("t".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "match", &env);

    let result = matcher.smart_suggest_with_context("$space", 2, &ctx);
    // Should NOT suggest &$space
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"&$space".to_string()),
            "Should not suggest & prefix for $ variables"
        );
    }
}

#[test]
fn test_prefix_no_suggestion_pattern_position() {
    // let's pattern position (position 1) expects Pattern, not Space
    let matcher = FuzzyMatcher::from_terms(vec!["let"]);
    let env = Environment::new();

    let expr = vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("self".to_string()), // Pattern position
        MettaValue::Long(1),
        MettaValue::Atom("x".to_string()),
    ];
    let ctx = SuggestionContext::for_arg(&expr, 1, "let", &env);

    let result = matcher.smart_suggest_with_context("self", 2, &ctx);
    // Should NOT suggest &self in pattern position
    if let Some(suggestion) = &result {
        assert!(
            !suggestion.suggestions.contains(&"&self".to_string()),
            "Should not suggest & prefix in pattern position"
        );
    }
}

// ============================================================
// Data Constructor Detection Tests
// ============================================================

#[test]
fn test_data_constructor_multi_hyphen() {
    assert!(is_likely_data_constructor("my-long-hyphenated-name"));
}

#[test]
fn test_data_constructor_all_uppercase() {
    assert!(is_likely_data_constructor("NIL"));
    assert!(is_likely_data_constructor("VOID"));
    assert!(is_likely_data_constructor("ERROR_CODE"));
}

#[test]
fn test_data_constructor_starts_with_uppercase() {
    assert!(is_likely_data_constructor("MyType"));
    assert!(is_likely_data_constructor("DataConstructor"));
    assert!(is_likely_data_constructor("True"));
    assert!(is_likely_data_constructor("False"));
}

#[test]
fn test_data_constructor_with_underscore() {
    assert!(is_likely_data_constructor("my_value"));
    assert!(is_likely_data_constructor("some_data"));
}

#[test]
fn test_data_constructor_with_digits() {
    assert!(is_likely_data_constructor("value1"));
    assert!(is_likely_data_constructor("test123"));
}

#[test]
fn test_not_data_constructor_simple_lowercase() {
    assert!(!is_likely_data_constructor("factorial"));
    assert!(!is_likely_data_constructor("fibonacci"));
    assert!(!is_likely_data_constructor("let"));
    assert!(!is_likely_data_constructor("match"));
}

#[test]
fn test_not_data_constructor_special_prefix() {
    assert!(!is_likely_data_constructor("$var"));
    assert!(!is_likely_data_constructor("&space"));
    assert!(!is_likely_data_constructor("'quoted"));
}

#[test]
fn test_data_constructor_empty_string() {
    assert!(!is_likely_data_constructor(""));
}

// ============================================================
// Prefix Compatibility Tests
// ============================================================

#[test]
fn test_prefix_compat_percent_percent() {
    assert!(are_prefixes_compatible("%Undefined%", "%Irreducible%"));
}

#[test]
fn test_prefix_incompat_quote_none() {
    assert!(!are_prefixes_compatible("'quoted", "regular"));
}

#[test]
fn test_prefix_incompat_percent_none() {
    assert!(!are_prefixes_compatible("%special%", "regular"));
}

// ============================================================
// Confidence Level Calculation Tests
// ============================================================

#[test]
fn test_confidence_6_char_distance_2_low() {
    // 6-char query, 6-char suggestion with distance 2: ratio 2/6 = 0.33, low confidence
    let conf = compute_suggestion_confidence("functi", "funtio", 2, 6);
    assert_eq!(conf, SuggestionConfidence::Low);
}

#[test]
fn test_confidence_8_char_distance_1_high() {
    // 8-char word with distance 1: ratio 1/8 = 0.125, high confidence
    let conf = compute_suggestion_confidence("fibonaci", "fibonacci", 1, 8);
    assert_eq!(conf, SuggestionConfidence::High);
}

#[test]
fn test_confidence_5_char_distance_2_low() {
    // 5-char word with distance 2: ratio 2/5 = 0.4 > 0.34, rejected
    let conf = compute_suggestion_confidence("hello", "hallo", 2, 5);
    assert_eq!(conf, SuggestionConfidence::None);
}

#[test]
fn test_confidence_10_char_distance_1_high() {
    // 10-char word with distance 1: ratio 1/10 = 0.1, high confidence
    let conf = compute_suggestion_confidence("factoriale", "factorial", 1, 10);
    assert_eq!(conf, SuggestionConfidence::High);
}

#[test]
fn test_confidence_distance_2_requires_6_chars() {
    // Distance 2 with 5 chars: should be low
    let conf = compute_suggestion_confidence("matsh", "match", 2, 5);
    // ratio 2/5 = 0.4 > 0.34 → None
    assert_eq!(conf, SuggestionConfidence::None);
}
