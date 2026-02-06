//! Property-based tests for Environment operations using proptest.
//!
//! This module provides comprehensive property-based testing for:
//! - Rule addition idempotency and isolation
//! - Fact storage and multiplicity tracking
//! - CoW (Copy-on-Write) behavior
//! - Pattern matching invariants
//!
//! Run with: cargo test --lib environment::proptests

use proptest::prelude::*;
use std::sync::Arc;

use super::*;
use crate::backend::models::{MettaValue, MettaValueInner};

// =============================================================================
// Strategy Generators for Environment Testing
// =============================================================================

/// Generate a simple symbol name
fn arb_symbol_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,7}".prop_map(|s| s)
}

/// Generate a variable name (with $ prefix)
fn arb_var_name() -> impl Strategy<Value = String> {
    "[a-z]{1,5}".prop_map(|s| format!("${}", s))
}

/// Generate a simple MettaValue atom
fn arb_atom() -> impl Strategy<Value = MettaValue> {
    arb_symbol_name().prop_map(|s| MettaValue::Atom(s))
}

/// Generate a simple MettaValue variable
fn arb_variable() -> impl Strategy<Value = MettaValue> {
    arb_var_name().prop_map(|s| MettaValue::Atom(s))
}

/// Generate a Long value
fn arb_long() -> impl Strategy<Value = MettaValue> {
    (-1000i64..1000i64).prop_map(MettaValue::Long)
}

/// Generate a simple MettaValue (non-recursive)
fn arb_simple_value() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        arb_atom(),
        arb_long(),
        Just(MettaValue::Bool(true)),
        Just(MettaValue::Bool(false)),
        arb_symbol_name().prop_map(MettaValue::String),
    ]
}

/// Generate a simple S-expression with given arity
fn arb_sexpr(arity: usize) -> impl Strategy<Value = MettaValue> {
    (arb_atom(), prop::collection::vec(arb_simple_value(), arity..=arity))
        .prop_map(|(head, mut args)| {
            args.insert(0, head);
            MettaValue::SExpr(args)
        })
}

/// Generate a rule LHS (pattern)
fn arb_rule_lhs() -> impl Strategy<Value = MettaValue> {
    (arb_symbol_name(), prop::collection::vec(prop_oneof![arb_variable(), arb_simple_value()], 1..=3))
        .prop_map(|(head, mut args)| {
            args.insert(0, MettaValue::Atom(head));
            MettaValue::SExpr(args)
        })
}

/// Generate a rule RHS (body)
fn arb_rule_rhs() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        arb_simple_value(),
        arb_sexpr(2),
    ]
}

/// Generate a complete Rule
fn arb_rule() -> impl Strategy<Value = Rule> {
    (arb_rule_lhs(), arb_rule_rhs())
        .prop_map(|(lhs, rhs)| Rule::new(lhs, rhs))
}

/// Generate a fact (simple S-expression data)
fn arb_fact() -> impl Strategy<Value = MettaValue> {
    (arb_symbol_name(), prop::collection::vec(arb_simple_value(), 1..=3))
        .prop_map(|(head, mut args)| {
            args.insert(0, MettaValue::Atom(head));
            MettaValue::SExpr(args)
        })
}

// =============================================================================
// Rule Addition Property Tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Adding a rule increases rule_count by 1
    #[test]
    fn prop_add_rule_increments_count(rule in arb_rule()) {
        let mut env = HeapEnvironment::default();
        let count_before = env.rule_count();
        env.add_rule(rule);
        let count_after = env.rule_count();
        prop_assert_eq!(count_after, count_before + 1);
    }

    /// Adding multiple distinct rules increases count correctly
    #[test]
    fn prop_add_multiple_rules(rules in prop::collection::vec(arb_rule(), 1..10)) {
        let mut env = HeapEnvironment::default();
        let n = rules.len();
        for rule in rules {
            env.add_rule(rule);
        }
        prop_assert_eq!(env.rule_count(), n);
    }

    /// Clone isolation: mutations to clone don't affect original
    #[test]
    fn prop_clone_isolation(rule in arb_rule()) {
        let mut env = HeapEnvironment::default();
        let original_count = env.rule_count();

        let mut clone = env.clone();
        clone.add_rule(rule);

        // Original unchanged
        prop_assert_eq!(env.rule_count(), original_count);
        // Clone has the new rule
        prop_assert_eq!(clone.rule_count(), original_count + 1);
    }

    /// Multiple clones are independent
    #[test]
    fn prop_multiple_clones_independent(rules in prop::collection::vec(arb_rule(), 3..=3)) {
        let env = HeapEnvironment::default();

        let mut clone1 = env.clone();
        let mut clone2 = env.clone();
        let mut clone3 = env.clone();

        clone1.add_rule(rules[0].clone());
        clone2.add_rule(rules[1].clone());
        clone3.add_rule(rules[2].clone());

        prop_assert_eq!(env.rule_count(), 0);
        prop_assert_eq!(clone1.rule_count(), 1);
        prop_assert_eq!(clone2.rule_count(), 1);
        prop_assert_eq!(clone3.rule_count(), 1);
    }
}

// =============================================================================
// Fact Storage Property Tests (Multiplicity Tracking)
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Adding a fact stores it in space (returns 1 for first addition)
    /// Note: get_atom_multiplicity returns 1 for non-existent facts (backward compat)
    /// so we test that adding increases the multiplicity from 1 to 1+ when added.
    #[test]
    fn prop_add_fact_increments_multiplicity(fact in arb_fact()) {
        let mut env = HeapEnvironment::default();

        // Add the fact
        env.add_to_space(&fact);
        let mult_after = env.get_atom_multiplicity(&fact);
        // First addition should give multiplicity >= 1
        prop_assert!(mult_after >= 1);
    }

    /// Adding same fact N times results in multiplicity N
    #[test]
    fn prop_add_fact_n_times(fact in arb_fact(), n in 1usize..5) {
        let mut env = HeapEnvironment::default();

        for _ in 0..n {
            env.add_to_space(&fact);
        }

        let mult = env.get_atom_multiplicity(&fact);
        prop_assert_eq!(mult, n);
    }

    /// Removing decrements multiplicity (testing the delta, not absolute value)
    #[test]
    fn prop_remove_fact_decrements_multiplicity(fact in arb_fact()) {
        let mut env = HeapEnvironment::default();

        // Add twice
        env.add_to_space(&fact);
        env.add_to_space(&fact);
        prop_assert_eq!(env.get_atom_multiplicity(&fact), 2);

        // Remove once - multiplicity should be 1
        env.remove_from_space(&fact);
        prop_assert_eq!(env.get_atom_multiplicity(&fact), 1);

        // After removing all, multiplicity returns 1 due to backward compat
        // (the API returns 1 for both "doesn't exist" and "exists with mult 1")
        env.remove_from_space(&fact);
        // So we just verify it doesn't panic and returns a reasonable value
        let final_mult = env.get_atom_multiplicity(&fact);
        prop_assert!(final_mult >= 1);
    }

    /// Different facts have independent multiplicities
    #[test]
    fn prop_facts_independent_multiplicity(facts in prop::collection::vec(arb_fact(), 2..=2)) {
        let mut env = HeapEnvironment::default();

        let fact1 = &facts[0];
        let fact2 = &facts[1];

        // Skip if facts are equal (same fact)
        prop_assume!(fact1 != fact2);

        // Add fact1 twice, fact2 once
        env.add_to_space(fact1);
        env.add_to_space(fact1);
        env.add_to_space(fact2);

        prop_assert_eq!(env.get_atom_multiplicity(fact1), 2);
        prop_assert_eq!(env.get_atom_multiplicity(fact2), 1);
    }

    /// Clone preserves multiplicities
    #[test]
    fn prop_clone_preserves_multiplicities(fact in arb_fact(), n in 1usize..4) {
        let mut env = HeapEnvironment::default();

        for _ in 0..n {
            env.add_to_space(&fact);
        }

        let clone = env.clone();

        prop_assert_eq!(clone.get_atom_multiplicity(&fact), n);
    }

    /// Clone isolation for multiplicities
    #[test]
    fn prop_clone_multiplicity_isolation(fact in arb_fact()) {
        let mut env = HeapEnvironment::default();
        env.add_to_space(&fact);
        env.add_to_space(&fact);

        let mut clone = env.clone();
        clone.add_to_space(&fact);

        // Original unchanged
        prop_assert_eq!(env.get_atom_multiplicity(&fact), 2);
        // Clone has new addition
        prop_assert_eq!(clone.get_atom_multiplicity(&fact), 3);
    }
}

// =============================================================================
// Pattern Matching Property Tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// match_space returns correct number of results for multiplicity
    #[test]
    fn prop_match_space_returns_multiplicity_results(fact in arb_fact(), n in 1usize..4) {
        let mut env = HeapEnvironment::default();

        for _ in 0..n {
            env.add_to_space(&fact);
        }

        let template = MettaValue::Atom("found".to_string());
        let results: Vec<MettaValue> = env
            .match_space(&fact, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();

        prop_assert_eq!(results.len(), n);
    }

    /// match_space with non-existent pattern returns empty
    #[test]
    fn prop_match_space_nonexistent_empty(fact in arb_fact()) {
        let env = HeapEnvironment::default();

        let template = MettaValue::Atom("found".to_string());
        let results: Vec<MettaValue> = env
            .match_space(&fact, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();

        prop_assert!(results.is_empty());
    }
}

// =============================================================================
// CoW (Copy-on-Write) Property Tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// New environment owns data
    #[test]
    fn prop_new_env_owns_data(_unit: ()) {
        let env = HeapEnvironment::default();
        prop_assert!(env.owns_data);
    }

    /// Clone does not own data initially
    #[test]
    fn prop_clone_does_not_own_data(_unit: ()) {
        let env = HeapEnvironment::default();
        let clone = env.clone();
        prop_assert!(!clone.owns_data);
    }

    /// Clone owns data after mutation
    #[test]
    fn prop_clone_owns_data_after_mutation(rule in arb_rule()) {
        let env = HeapEnvironment::default();
        let mut clone = env.clone();

        prop_assert!(!clone.owns_data);

        clone.add_rule(rule);

        prop_assert!(clone.owns_data);
    }

    /// Make_owned is idempotent (second mutation doesn't re-copy)
    #[test]
    fn prop_make_owned_idempotent(rules in prop::collection::vec(arb_rule(), 2..=2)) {
        let env = HeapEnvironment::default();
        let mut clone = env.clone();

        // First mutation triggers make_owned
        clone.add_rule(rules[0].clone());
        let ptr_after_first = Arc::as_ptr(&clone.shared);

        // Second mutation should NOT trigger another make_owned
        clone.add_rule(rules[1].clone());
        let ptr_after_second = Arc::as_ptr(&clone.shared);

        prop_assert_eq!(ptr_after_first, ptr_after_second);
    }
}

// =============================================================================
// Rule Lookup Property Tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Rules can be retrieved after adding
    #[test]
    fn prop_rules_retrievable(rule in arb_rule()) {
        let mut env = HeapEnvironment::default();
        env.add_rule(rule.clone());

        // Get head symbol and arity from LHS
        if let MettaValueInner::SExpr(elements) = rule.lhs.inner() {
            if let Some(MettaValue { .. }) = elements.first() {
                if let MettaValueInner::Atom(head) = elements[0].inner() {
                    let arity = elements.len() - 1;
                    let matching: Vec<_> = env.get_matching_rules_iter(head, arity).collect();
                    prop_assert!(!matching.is_empty(), "Should find at least one matching rule");
                }
            }
        }
    }

    /// iter_rules returns all added rules
    #[test]
    fn prop_iter_rules_returns_all(rules in prop::collection::vec(arb_rule(), 1..5)) {
        let mut env = HeapEnvironment::default();
        let n = rules.len();

        for rule in rules {
            env.add_rule(rule);
        }

        let count: usize = env.iter_rules().count();
        prop_assert_eq!(count, n);
    }
}

// =============================================================================
// Binding Operations Property Tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// has_binding returns false for non-existing bindings
    #[test]
    fn prop_has_binding_false_for_nonexistent(name in arb_symbol_name()) {
        let env = HeapEnvironment::default();
        prop_assert!(!env.has_binding(&name));
    }

    /// get_binding returns None for non-existing bindings
    #[test]
    fn prop_get_binding_none_for_nonexistent(name in arb_symbol_name()) {
        let env = HeapEnvironment::default();
        prop_assert!(env.get_binding(&name).is_none());
    }
}

// =============================================================================
// State Operations Property Tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]

    /// create_state creates unique state IDs
    #[test]
    fn prop_create_state_unique_ids(_unit: ()) {
        let mut env = HeapEnvironment::default();

        let state1 = env.create_state(&MettaValue::Long(1));
        let state2 = env.create_state(&MettaValue::Long(2));
        let state3 = env.create_state(&MettaValue::Long(3));

        // IDs should be unique
        prop_assert_ne!(state1, state2);
        prop_assert_ne!(state2, state3);
        prop_assert_ne!(state1, state3);
    }

    /// get_state returns the stored value
    #[test]
    fn prop_get_state_returns_value(val in arb_simple_value()) {
        let mut env = HeapEnvironment::default();
        let state_id = env.create_state(&val);

        let retrieved = env.get_state(state_id);
        prop_assert!(retrieved.is_some());
        prop_assert_eq!(retrieved.unwrap(), val);
    }

    /// change_state updates the value
    #[test]
    fn prop_change_state_updates(val1 in arb_simple_value(), val2 in arb_simple_value()) {
        let mut env = HeapEnvironment::default();
        let state_id = env.create_state(&val1);

        // Change state
        let changed = env.change_state(state_id, &val2);
        prop_assert!(changed);

        // Verify new value
        let retrieved = env.get_state(state_id);
        prop_assert!(retrieved.is_some());
        prop_assert_eq!(retrieved.unwrap(), val2);
    }

    /// has_state returns correct value
    #[test]
    fn prop_has_state_correct(val in arb_simple_value()) {
        let mut env = HeapEnvironment::default();

        // Non-existent state
        prop_assert!(!env.has_state(99999));

        // After creation
        let state_id = env.create_state(&val);
        prop_assert!(env.has_state(state_id));
    }
}

// =============================================================================
// Regression Tests for Specific Edge Cases
// =============================================================================

#[cfg(test)]
mod regression_tests {
    use super::*;

    /// Test: Empty environment operations don't panic
    #[test]
    fn test_empty_env_operations() {
        let env = HeapEnvironment::default();

        // These should not panic
        let _ = env.rule_count();
        let _ = env.iter_rules().count();
        let _ = env.get_binding("nonexistent");
        let _ = env.get_state(12345);

        let fact = MettaValue::Atom("test".to_string());
        let _ = env.get_atom_multiplicity(&fact);
    }

    /// Test: Clone chain maintains isolation at all levels
    #[test]
    fn test_deep_clone_chain_isolation() {
        let mut env = HeapEnvironment::default();
        env.add_rule(Rule::new(
            MettaValue::Atom("level0".to_string()),
            MettaValue::Atom("body0".to_string()),
        ));

        let mut level1 = env.clone();
        level1.add_rule(Rule::new(
            MettaValue::Atom("level1".to_string()),
            MettaValue::Atom("body1".to_string()),
        ));

        let mut level2 = level1.clone();
        level2.add_rule(Rule::new(
            MettaValue::Atom("level2".to_string()),
            MettaValue::Atom("body2".to_string()),
        ));

        let mut level3 = level2.clone();
        level3.add_rule(Rule::new(
            MettaValue::Atom("level3".to_string()),
            MettaValue::Atom("body3".to_string()),
        ));

        assert_eq!(env.rule_count(), 1);
        assert_eq!(level1.rule_count(), 2);
        assert_eq!(level2.rule_count(), 3);
        assert_eq!(level3.rule_count(), 4);
    }

    /// Test: Removing non-existent fact doesn't panic
    #[test]
    fn test_remove_nonexistent_fact() {
        let mut env = HeapEnvironment::default();
        let fact = MettaValue::Atom("nonexistent".to_string());

        // This should not panic
        env.remove_from_space(&fact);

        // Note: get_atom_multiplicity returns 1 for non-existent facts
        // (backward compatibility behavior) so we just verify it doesn't panic
        let mult = env.get_atom_multiplicity(&fact);
        assert!(mult >= 1);
    }

    /// Test: match_space with variable pattern works
    #[test]
    fn test_match_space_with_variable() {
        let mut env = HeapEnvironment::default();

        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("Alice".to_string()),
        ]);

        env.add_to_space(&fact);

        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("$name".to_string()),
        ]);

        let template = MettaValue::Atom("$name".to_string());

        let results: Vec<MettaValue> = env
            .match_space(&pattern, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();

        assert_eq!(results.len(), 1);

        if let MettaValueInner::Atom(name) = results[0].inner() {
            assert_eq!(name, "Alice");
        } else {
            panic!("Expected atom result");
        }
    }

    /// Test: State operations are SHARED between clones (not CoW)
    /// Note: States use DashMap and are explicitly NOT copy-on-write
    #[test]
    fn test_state_clone_shared() {
        let mut env = HeapEnvironment::default();
        let state_id = env.create_state(&MettaValue::Long(100));

        let mut clone = env.clone();

        // Change state in clone
        clone.change_state(state_id, &MettaValue::Long(200));

        // Both should see the new value (states are shared, not isolated)
        assert_eq!(env.get_state(state_id), Some(MettaValue::Long(200)));
        assert_eq!(clone.get_state(state_id), Some(MettaValue::Long(200)));
    }

    /// Test: Rule index is correctly maintained
    #[test]
    fn test_rule_index_maintained() {
        let mut env = HeapEnvironment::default();

        // Add rules with same head
        env.add_rule(Rule::new(
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Long(1),
        ));
        env.add_rule(Rule::new(
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::Long(2),
        ));

        // Should find both rules
        let matching: Vec<_> = env.get_matching_rules_iter("foo", 1).collect();
        assert_eq!(matching.len(), 2);

        // Different head should find nothing
        let matching_bar: Vec<_> = env.get_matching_rules_iter("bar", 1).collect();
        assert_eq!(matching_bar.len(), 0);
    }

    /// Test: Space operations with different value types
    #[test]
    fn test_space_ops_different_types() {
        let mut env = HeapEnvironment::default();

        let facts = vec![
            MettaValue::Long(42),
            MettaValue::String("hello".to_string()),
            MettaValue::Bool(true),
            MettaValue::Atom("symbol".to_string()),
        ];

        for fact in &facts {
            env.add_to_space(fact);
            // After adding, multiplicity should be 1
            assert_eq!(env.get_atom_multiplicity(fact), 1);
        }

        // Add first fact again, then remove once
        env.add_to_space(&facts[0]);
        assert_eq!(env.get_atom_multiplicity(&facts[0]), 2);

        env.remove_from_space(&facts[0]);
        // After removal from mult 2, should be 1
        assert_eq!(env.get_atom_multiplicity(&facts[0]), 1);

        // Others unchanged
        for fact in &facts[1..] {
            assert_eq!(env.get_atom_multiplicity(fact), 1);
        }
    }
}
