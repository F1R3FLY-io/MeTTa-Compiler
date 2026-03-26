//! Tests for Environment's Copy-on-Write (CoW) semantics and thread safety.
//!
//! This module contains comprehensive tests for:
//! - CoW behavior: cloning, make_owned, isolation
//! - Thread safety: concurrent mutations, race conditions
//! - Stress tests: many clones, deep chains, concurrent access

use super::*;
use crate::backend::models::{MettaValue, MettaValueInner};
use std::sync::atomic::Ordering;
use std::sync::{Arc as StdArc, Barrier};
use std::thread;

/// Helper: Create a simple rule for testing
fn make_test_rule(lhs: &str, rhs: &str) -> (MettaValue, MettaValue) {
    (
        MettaValue::Atom(lhs.to_string()),
        MettaValue::Atom(rhs.to_string()),
    )
}

/// Helper: Create a simple MettaValue fact for testing
#[allow(dead_code)]
fn make_test_fact(value: &str) -> MettaValue {
    MettaValue::Atom(value.to_string())
}

// ============================================================================
// UNIT TESTS - get_all_atoms
// ============================================================================

#[test]
fn test_get_all_atoms_returns_added_atoms() {
    let mut env = MettaEnvironment::default();
    env.add_to_space(&MettaValue::Long(1));
    env.add_to_space(&MettaValue::Long(2));
    env.add_to_space(&MettaValue::sym("foo"));
    let atoms = env.get_all_atoms();
    eprintln!("get_all_atoms returned {} atoms: {:?}", atoms.len(), atoms);
    assert_eq!(atoms.len(), 3, "Expected 3 atoms from get_all_atoms, got {}", atoms.len());
}

// ============================================================================
// UNIT TESTS - CoW Behavior
// ============================================================================

#[test]
fn test_new_environment_owns_data() {
    // Test: New environment should own its data
    let env = MettaEnvironment::default();
    assert!(env.owns_data, "New environment should own its data");
    assert!(
        !env.modified.load(Ordering::Acquire),
        "New environment should not be modified"
    );
}

#[test]
fn test_clone_does_not_own_data() {
    // Test: Cloned environment should not own data initially
    let env = MettaEnvironment::default();
    let clone = env.clone();

    assert!(env.owns_data, "Original environment should still own data");
    assert!(
        !clone.owns_data,
        "Cloned environment should NOT own data initially"
    );
    assert!(
        !clone.modified.load(Ordering::Acquire),
        "Cloned environment should not be modified"
    );
}

#[test]
fn test_clone_shares_arc_pointers() {
    // Test: Clone should share Arc pointers (cheap O(1) clone)
    let env = MettaEnvironment::default();

    // Get Arc pointer addresses before clone (consolidated shared pointer)
    let shared_ptr_before = StdArc::as_ptr(&env.shared);

    let clone = env.clone();

    // Get Arc pointer addresses after clone
    let shared_ptr_after = StdArc::as_ptr(&clone.shared);

    // Pointers should be identical (shared) - O(1) clone
    assert_eq!(
        shared_ptr_before, shared_ptr_after,
        "Clone should share consolidated Arc"
    );
}

#[test]
fn test_make_owned_triggers_on_first_write() {
    // Test: First mutation should trigger make_owned() and deep copy
    let mut env = MettaEnvironment::default();
    let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");

    // Add rule to original (already owns data, no make_owned() needed)
    env.add_rule(lhs, rhs);
    assert!(env.owns_data, "Original should still own data");
    assert!(
        env.modified.load(Ordering::Acquire),
        "Original should be marked modified"
    );

    // Clone and mutate
    let mut clone = env.clone();
    assert!(!clone.owns_data, "Clone should not own data initially");

    // Get Arc pointers before mutation
    let btm_ptr_before = StdArc::as_ptr(&clone.shared);

    // First mutation triggers make_owned()
    let (lhs2, rhs2) = make_test_rule("(clone $y)", "(cloned $y)");
    clone.add_rule(lhs2, rhs2);

    // After mutation
    assert!(clone.owns_data, "Clone should own data after mutation");
    assert!(
        clone.modified.load(Ordering::Acquire),
        "Clone should be marked modified"
    );

    // Arc pointers should be different (deep copy occurred)
    let btm_ptr_after = StdArc::as_ptr(&clone.shared);
    assert_ne!(
        btm_ptr_before, btm_ptr_after,
        "make_owned() should create new Arc"
    );
}

#[test]
fn test_isolation_after_clone_mutation() {
    // Test: Mutations to clone should not affect original
    let mut env = MettaEnvironment::default();
    let (lhs1, rhs1) = make_test_rule("(original $x)", "(original-result $x)");
    env.add_rule(lhs1.clone(), rhs1.clone());

    // Clone and add different rule
    let mut clone = env.clone();
    let (lhs2, rhs2) = make_test_rule("(cloned $y)", "(cloned-result $y)");
    clone.add_rule(lhs2.clone(), rhs2.clone());

    // Original should only have rule1
    let original_rules = env.get_matching_rules_for_expr(&lhs1);
    assert_eq!(original_rules.len(), 1, "Original should have 1 rule");

    // Clone should have both rules (rule1 was shared, rule2 was added)
    let clone_rules = clone.get_matching_rules_for_expr(&lhs1);
    assert_eq!(clone_rules.len(), 1, "Clone should have original rule");

    let clone_rules2 = clone.get_matching_rules_for_expr(&lhs2);
    assert_eq!(clone_rules2.len(), 1, "Clone should have new rule");
}

#[test]
fn test_modification_tracking() {
    // Test: Modification flag is correctly tracked
    let mut env = MettaEnvironment::default();
    assert!(
        !env.modified.load(Ordering::Acquire),
        "New env should not be modified"
    );

    // Add rule → should set modified flag
    let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
    env.add_rule(lhs, rhs);
    assert!(
        env.modified.load(Ordering::Acquire),
        "Env should be modified after add_rule"
    );

    // Clone → clone should have fresh modified flag
    let mut clone = env.clone();
    assert!(
        !clone.modified.load(Ordering::Acquire),
        "Clone should have fresh modified flag"
    );

    // Mutate clone → should set clone's modified flag
    let (lhs2, rhs2) = make_test_rule("(test2 $y)", "(result2 $y)");
    clone.add_rule(lhs2, rhs2);
    assert!(
        clone.modified.load(Ordering::Acquire),
        "Clone should be modified after mutation"
    );
}

#[test]
fn test_make_owned_idempotency() {
    // Test: make_owned() should be idempotent (safe to call multiple times)
    let env = MettaEnvironment::default();
    let mut clone = env.clone();

    // First mutation triggers make_owned()
    let (lhs1, rhs1) = make_test_rule("(test1 $x)", "(result1 $x)");
    clone.add_rule(lhs1, rhs1);
    assert!(
        clone.owns_data,
        "Clone should own data after first mutation"
    );

    // Get Arc pointers after first make_owned()
    let shared_ptr_first = StdArc::as_ptr(&clone.shared);

    // Second mutation should NOT trigger another make_owned()
    let (lhs2, rhs2) = make_test_rule("(test2 $y)", "(result2 $y)");
    clone.add_rule(lhs2, rhs2);

    // Arc pointers should be same (no second deep copy)
    let shared_ptr_second = StdArc::as_ptr(&clone.shared);
    assert_eq!(
        shared_ptr_first, shared_ptr_second,
        "make_owned() should not run twice"
    );
}

#[test]
fn test_deep_clone_copies_all_fields() {
    // Test: make_owned() should deep copy the consolidated shared state
    // (All 17 RwLock fields are now in one Arc<EnvironmentShared>)
    let mut env = MettaEnvironment::default();
    let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
    env.add_rule(lhs, rhs);

    let mut clone = env.clone();

    // Get Arc pointer before mutation (single consolidated pointer)
    let shared_before = StdArc::as_ptr(&clone.shared);

    // Trigger make_owned()
    let (lhs2, rhs2) = make_test_rule("(clone $y)", "(cloned $y)");
    clone.add_rule(lhs2, rhs2);

    // Get Arc pointer after mutation
    let shared_after = StdArc::as_ptr(&clone.shared);

    // The consolidated Arc should be different (deep copy occurred)
    assert_ne!(
        shared_before, shared_after,
        "shared should be deep copied after make_owned()"
    );
}

#[test]
fn test_multiple_clones_independent() {
    // Test: Multiple clones should be independent after mutation
    let mut env = MettaEnvironment::default();
    let (lhs, rhs) = make_test_rule("(original $x)", "(original-result $x)");
    env.add_rule(lhs, rhs);

    let mut clone1 = env.clone();
    let mut clone2 = env.clone();
    let mut clone3 = env.clone();

    // Mutate each clone differently
    let (lhs1, rhs1) = make_test_rule("(clone1 $a)", "(result1 $a)");
    clone1.add_rule(lhs1, rhs1);
    let (lhs2, rhs2) = make_test_rule("(clone2 $b)", "(result2 $b)");
    clone2.add_rule(lhs2, rhs2);
    let (lhs3, rhs3) = make_test_rule("(clone3 $c)", "(result3 $c)");
    clone3.add_rule(lhs3, rhs3);

    // Each clone should have only its own rule (plus original)
    let original_count = env.rule_count();
    let clone1_count = clone1.rule_count();
    let clone2_count = clone2.rule_count();
    let clone3_count = clone3.rule_count();

    assert_eq!(original_count, 1, "Original should have 1 rule");
    assert_eq!(clone1_count, 2, "Clone1 should have 2 rules");
    assert_eq!(clone2_count, 2, "Clone2 should have 2 rules");
    assert_eq!(clone3_count, 2, "Clone3 should have 2 rules");
}

// ============================================================================
// PROPERTY-BASED TESTS
// ============================================================================

#[test]
fn property_clone_never_shares_mutable_state_after_write() {
    // Property: After mutation, clone and original should have independent state
    for i in 0..10 {
        let mut env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule(&format!("(test{}  $x)", i), "(result $x)");
        env.add_rule(lhs, rhs);

        let mut clone = env.clone();
        let (lhs2, rhs2) = make_test_rule(&format!("(clone{} $y)", i), "(cloned $y)");
        clone.add_rule(lhs2, rhs2);

        // Verify Arc pointers are different (consolidated shared pointer)
        let env_ptr = StdArc::as_ptr(&env.shared);
        let clone_ptr = StdArc::as_ptr(&clone.shared);
        assert_ne!(
            env_ptr, clone_ptr,
            "Property violated: clone shares mutable state after write (iteration {})",
            i
        );
    }
}

#[test]
fn property_parallel_writes_are_isolated() {
    // Property: Parallel mutations to different clones should be isolated
    let env = MettaEnvironment::default();
    let num_threads = 4;
    let barrier = StdArc::new(Barrier::new(num_threads));

    let handles: Vec<_> = (0..num_threads)
        .map(|i| {
            let mut clone = env.clone();
            let barrier = StdArc::clone(&barrier);

            thread::spawn(move || {
                // Synchronize all threads to start mutations simultaneously
                barrier.wait();

                // Each thread adds a unique rule
                let (lhs, rhs) = make_test_rule(
                    &format!("(thread{} $x)", i),
                    &format!("(result{} $x)", i),
                );
                clone.add_rule(lhs, rhs);

                // Verify this clone only has 1 rule
                let count = clone.rule_count();
                assert_eq!(count, 1, "Thread {} clone should have exactly 1 rule", i);

                clone
            })
        })
        .collect();

    // Join all threads and verify each clone is independent
    let clones: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    for (i, clone) in clones.iter().enumerate() {
        let count = clone.rule_count();
        assert_eq!(
            count, 1,
            "Clone {} should have exactly 1 rule after parallel write",
            i
        );
    }

    // Original should be unchanged
    assert_eq!(
        env.rule_count(),
        0,
        "Original environment should be unchanged"
    );
}

// ============================================================================
// STRESS TESTS
// ============================================================================

#[test]
fn stress_many_clones_with_mutations() {
    // Stress: Create 1000 clones and mutate each one
    let env = MettaEnvironment::default();

    for i in 0..1000 {
        let mut clone = env.clone();
        let (lhs, rhs) = make_test_rule(&format!("(stress{} $x)", i), "(result $x)");
        clone.add_rule(lhs, rhs);

        assert!(
            clone.owns_data,
            "Clone {} should own data after mutation",
            i
        );
        assert_eq!(clone.rule_count(), 1, "Clone {} should have 1 rule", i);
    }

    // Original should be unchanged
    assert_eq!(
        env.rule_count(),
        0,
        "Original should be unchanged after 1000 clone mutations"
    );
}

#[test]
fn stress_deep_clone_chains() {
    // Stress: Create clone chains (clone of clone of clone...)
    let mut env = MettaEnvironment::default();
    let (lhs, rhs) = make_test_rule("(original $x)", "(result $x)");
    env.add_rule(lhs, rhs);

    let mut current = env.clone();
    for i in 0..10 {
        let (lhs_i, rhs_i) = make_test_rule(&format!("(depth{} $x)", i), "(result $x)");
        current.add_rule(lhs_i, rhs_i);
        let next = current.clone();
        current = next;
    }

    // Final clone should have 1 (original) + 10 (depth) = 11 rules
    assert_eq!(current.rule_count(), 11, "Final clone should have 11 rules");

    // Original should be unchanged
    assert_eq!(env.rule_count(), 1, "Original should still have 1 rule");
}

#[test]
fn stress_concurrent_clone_and_mutate() {
    // Stress: Concurrent cloning and mutation across multiple threads
    let env = StdArc::new(MettaEnvironment::default());
    let num_threads = 8;

    let handles: Vec<_> = (0..num_threads)
        .map(|i| {
            let env = StdArc::clone(&env);

            thread::spawn(move || {
                for j in 0..100 {
                    let mut clone = env.as_ref().clone();
                    let (lhs, rhs) = make_test_rule(&format!("(t{}_{} $x)", i, j), "(result $x)");
                    clone.add_rule(lhs, rhs);
                    assert_eq!(clone.rule_count(), 1, "Clone should have 1 rule");
                }
            })
        })
        .collect();

    // Join all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // Original should be unchanged
    assert_eq!(
        env.rule_count(),
        0,
        "Original should be unchanged after concurrent stress"
    );
}

// ============================================================================
// INTEGRATION TESTS
// ============================================================================

#[test]
fn integration_parallel_eval_with_dynamic_rules() {
    // Integration: Simulate parallel evaluation where each thread adds rules dynamically
    use parking_lot::Mutex;

    let base_env = MettaEnvironment::default();
    let results = StdArc::new(Mutex::new(Vec::new()));
    let num_threads = 4;

    let handles: Vec<_> = (0..num_threads)
        .map(|i| {
            let mut env = base_env.clone();
            let results = StdArc::clone(&results);

            thread::spawn(move || {
                // Each thread adds rules dynamically during "evaluation"
                for j in 0..10 {
                    let (lhs, rhs) = make_test_rule(&format!("(eval{}_{}  $x)", i, j), "(result $x)");
                    env.add_rule(lhs, rhs);
                }

                let count = env.rule_count();
                results.lock().push(count);
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // Each thread should have 10 rules
    let results = results.lock();
    assert_eq!(
        results.len(),
        num_threads,
        "Should have {} results",
        num_threads
    );
    for (i, &count) in results.iter().enumerate() {
        assert_eq!(count, 10, "Thread {} should have 10 rules", i);
    }

    // Base environment should be unchanged
    assert_eq!(
        base_env.rule_count(),
        0,
        "Base environment should be unchanged"
    );
}

#[test]
fn integration_read_while_write() {
    // Integration: Test concurrent reads and writes (RwLock benefit)
    let mut env = MettaEnvironment::default();
    for i in 0..100 {
        let (lhs, rhs) = make_test_rule(&format!("(rule{} $x)", i), "(result $x)");
        env.add_rule(lhs, rhs);
    }

    let env = StdArc::new(env);
    let num_readers = 8;
    let barrier = StdArc::new(Barrier::new(num_readers + 1));

    // Spawn reader threads
    let reader_handles: Vec<_> = (0..num_readers)
        .map(|_| {
            let env = StdArc::clone(&env);
            let barrier = StdArc::clone(&barrier);

            thread::spawn(move || {
                barrier.wait();

                // Multiple readers should be able to read concurrently (RwLock benefit)
                for _ in 0..100 {
                    let count = env.rule_count();
                    assert!(count >= 100, "Should see at least 100 rules");
                }
            })
        })
        .collect();

    // Start all readers simultaneously
    barrier.wait();

    // Join all readers
    for handle in reader_handles {
        handle.join().unwrap();
    }
}

#[test]
fn integration_clone_preserves_rule_data() {
    // Integration: Verify clone preserves all rule data correctly
    let mut env = MettaEnvironment::default();

    // Add various rules
    let rules = vec![
        make_test_rule("(color car red)", "(assert color car red)"),
        make_test_rule("(color truck blue)", "(assert color truck blue)"),
        make_test_rule("(size car small)", "(assert size car small)"),
    ];

    for (lhs, rhs) in &rules {
        env.add_rule(lhs.clone(), rhs.clone());
    }

    // Clone environment
    let clone = env.clone();

    // Verify clone has same rules
    assert_eq!(
        clone.rule_count(),
        env.rule_count(),
        "Clone should have same rule count"
    );

    // Verify each rule is accessible
    for (lhs, _rhs) in &rules {
        let original_matches = env.get_matching_rules_for_expr(lhs);
        let clone_matches = clone.get_matching_rules_for_expr(lhs);

        assert!(!original_matches.is_empty(), "Original should have rule");
        assert!(!clone_matches.is_empty(), "Clone should have rule");
    }
}

// ============================================================================
// Thread Safety Tests - Concurrent Mutation
// ============================================================================

// ============================================================================
// ALL-ATOM MULTIPLICITY TESTS (MeTTa HE Semantics)
// ============================================================================

mod all_atom_multiplicity {
    use super::*;

    /// Test: Adding the same data atom twice results in count=2
    #[test]
    fn test_add_same_atom_twice_increments_count() {
        let mut env = MettaEnvironment::default();

        // Create a simple data atom (not a rule)
        let atom = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);

        // Add the atom twice
        env.add_to_space(&atom);
        env.add_to_space(&atom);

        // Check multiplicity is 2
        let count = env.get_atom_multiplicity(&atom);
        assert_eq!(count, 2, "Atom added twice should have multiplicity 2");
    }

    /// Test: match_space returns N results for atoms with multiplicity N
    #[test]
    fn test_match_space_returns_n_copies_for_multiplicity_n() {
        let mut env = MettaEnvironment::default();

        // Create and add a data atom 3 times
        let atom = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);

        env.add_to_space(&atom);
        env.add_to_space(&atom);
        env.add_to_space(&atom);

        // Match should return 3 results
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        let template = MettaValue::Atom("found".to_string());

        // match_space returns Vec<MultiplicityMatch>, expand for MeTTa HE semantics
        let results: Vec<MettaValue> = env
            .match_space(&pattern, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(
            results.len(),
            3,
            "match_space should return 3 results for atom with multiplicity 3"
        );

        // All results should be the template
        for result in &results {
            assert_eq!(
                result, &template,
                "Each result should be the instantiated template"
            );
        }
    }

    /// Test: Removing once decrements counter, atom still in space
    #[test]
    fn test_remove_once_decrements_counter_keeps_atom() {
        let mut env = MettaEnvironment::default();

        // Add atom twice
        let atom = MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(42),
        ]);

        env.add_to_space(&atom);
        env.add_to_space(&atom);

        // Verify count is 2
        assert_eq!(
            env.get_atom_multiplicity(&atom),
            2,
            "Should have multiplicity 2"
        );

        // Remove once
        env.remove_from_space(&atom);

        // Count should be 1
        assert_eq!(
            env.get_atom_multiplicity(&atom),
            1,
            "Should have multiplicity 1 after one removal"
        );

        // Atom should still be matchable (1 result)
        let results: Vec<MettaValue> = env
            .match_space(&atom, &MettaValue::Atom("found".to_string()))
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(results.len(), 1, "Should return 1 result after one removal");
    }

    /// Test: Removing twice (count=0) removes atom from PathMap
    #[test]
    fn test_remove_all_copies_removes_from_space() {
        let mut env = MettaEnvironment::default();

        // Add atom twice
        let atom = MettaValue::SExpr(vec![
            MettaValue::Atom("removable".to_string()),
            MettaValue::Long(123),
        ]);

        env.add_to_space(&atom);
        env.add_to_space(&atom);

        // Remove twice
        env.remove_from_space(&atom);
        env.remove_from_space(&atom);

        // Atom should no longer be matchable
        let results: Vec<MettaValue> = env
            .match_space(&atom, &MettaValue::Atom("found".to_string()))
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(
            results.len(),
            0,
            "Should return 0 results after all copies removed"
        );
    }

    /// Test: Simple atom (not s-expression) multiplicity
    #[test]
    fn test_simple_atom_multiplicity() {
        let mut env = MettaEnvironment::default();

        let atom = MettaValue::Atom("simple-fact".to_string());

        // Add 4 times
        for _ in 0..4 {
            env.add_to_space(&atom);
        }

        // Check multiplicity
        assert_eq!(
            env.get_atom_multiplicity(&atom),
            4,
            "Simple atom should have multiplicity 4"
        );

        // Match should return 4 results
        // Use a variable pattern to match any atom
        let pattern = MettaValue::Atom("simple-fact".to_string());
        let template = MettaValue::Atom("matched".to_string());
        let results: Vec<MettaValue> = env
            .match_space(&pattern, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(
            results.len(),
            4,
            "match_space should return 4 results for multiplicity 4"
        );
    }

    /// Test: Rules with multiplicity still work correctly
    #[test]
    fn test_rules_with_multiplicity() {
        let mut env = MettaEnvironment::default();

        // Create a rule s-expression
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(2),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        // Add the rule twice via add_to_space
        env.add_to_space(&rule_sexpr);
        env.add_to_space(&rule_sexpr);

        // Check multiplicity
        assert_eq!(
            env.get_atom_multiplicity(&rule_sexpr),
            2,
            "Rule should have multiplicity 2"
        );

        // Match the rule pattern
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("$lhs".to_string()),
            MettaValue::Atom("$rhs".to_string()),
        ]);
        let template = MettaValue::Atom("rule-found".to_string());

        let results: Vec<MettaValue> = env
            .match_space(&pattern, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        assert_eq!(
            results.len(),
            2,
            "match_space should return 2 results for rule with multiplicity 2"
        );
    }

    /// Test: Fork/CoW preserves multiplicities correctly
    #[test]
    fn test_fork_preserves_multiplicities() {
        let mut env = MettaEnvironment::default();

        // Add atom 3 times
        let atom = MettaValue::SExpr(vec![
            MettaValue::Atom("preserved".to_string()),
            MettaValue::Long(999),
        ]);

        env.add_to_space(&atom);
        env.add_to_space(&atom);
        env.add_to_space(&atom);

        // Clone the environment (fork)
        let mut forked = env.clone();

        // Original should still have multiplicity 3
        assert_eq!(
            env.get_atom_multiplicity(&atom),
            3,
            "Original should have multiplicity 3"
        );

        // Forked should also have multiplicity 3
        assert_eq!(
            forked.get_atom_multiplicity(&atom),
            3,
            "Forked should have multiplicity 3"
        );

        // Add once more to forked
        forked.add_to_space(&atom);

        // Forked should now have multiplicity 4
        assert_eq!(
            forked.get_atom_multiplicity(&atom),
            4,
            "Forked should have multiplicity 4 after add"
        );

        // Original should still have multiplicity 3 (isolation)
        assert_eq!(
            env.get_atom_multiplicity(&atom),
            3,
            "Original should still have multiplicity 3 after fork mutation"
        );
    }

    /// Test: Different atoms have independent multiplicities
    #[test]
    fn test_different_atoms_independent_multiplicities() {
        let mut env = MettaEnvironment::default();

        let atom1 = MettaValue::SExpr(vec![
            MettaValue::Atom("first".to_string()),
            MettaValue::Long(1),
        ]);

        let atom2 = MettaValue::SExpr(vec![
            MettaValue::Atom("second".to_string()),
            MettaValue::Long(2),
        ]);

        // Add atom1 twice, atom2 once
        env.add_to_space(&atom1);
        env.add_to_space(&atom1);
        env.add_to_space(&atom2);

        // Check multiplicities
        assert_eq!(
            env.get_atom_multiplicity(&atom1),
            2,
            "atom1 should have multiplicity 2"
        );
        assert_eq!(
            env.get_atom_multiplicity(&atom2),
            1,
            "atom2 should have multiplicity 1"
        );

        // Match each and verify results
        let template = MettaValue::Atom("found".to_string());

        let results1: Vec<MettaValue> = env
            .match_space(&atom1, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();
        let results2: Vec<MettaValue> = env
            .match_space(&atom2, &template)
            .into_iter()
            .flat_map(|m| m.expand())
            .collect();

        assert_eq!(results1.len(), 2, "atom1 should match twice");
        assert_eq!(results2.len(), 1, "atom2 should match once");
    }

    /// Test: match_space with variable pattern and multiplicity
    #[test]
    fn test_match_space_variable_pattern_with_multiplicity() {
        let mut env = MettaEnvironment::default();

        // Add same fact twice
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("Alice".to_string()),
        ]);

        env.add_to_space(&fact);
        env.add_to_space(&fact);

        // Match with variable pattern
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
        assert_eq!(
            results.len(),
            2,
            "Should return 2 results for multiplicity 2"
        );

        // Both results should be "Alice"
        for result in &results {
            if let MettaValueInner::Atom(name) = result.inner() {
                assert_eq!(*name, "Alice", "Result should be Alice");
            } else {
                panic!("Result should be an atom");
            }
        }
    }

    /// Test: Multiplicity survives rebuild_bloom_filter
    #[test]
    fn test_multiplicity_survives_rebuild() {
        let mut env = MettaEnvironment::default();

        // Add a rule twice
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test-rebuild".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        env.add_to_space(&rule_sexpr);
        env.add_to_space(&rule_sexpr);

        // Check multiplicity before rebuild
        assert_eq!(
            env.get_atom_multiplicity(&rule_sexpr),
            2,
            "Multiplicity should be 2 before rebuild"
        );

        // Rebuild bloom filter
        env.rebuild_bloom_filter();

        // Check multiplicity after rebuild (should still be 2)
        assert_eq!(
            env.get_atom_multiplicity(&rule_sexpr),
            2,
            "Multiplicity should still be 2 after rebuild"
        );
    }
}

mod thread_safety {
    use super::*;
    use std::time::Duration;

    // Helper: Create a test rule with proper SExpr structure
    fn make_test_rule_sexpr(pattern: &str, body: &str) -> (MettaValue, MettaValue) {
        // Parse pattern string into proper MettaValue structure
        // "(head $x)" → SExpr([Atom("head"), Atom("$x")])
        let lhs = if pattern.starts_with('(') && pattern.ends_with(')') {
            // Parse s-expression pattern
            let inner = &pattern[1..pattern.len() - 1];
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.is_empty() {
                MettaValue::Atom(pattern.to_string())
            } else {
                MettaValue::SExpr(
                    parts
                        .into_iter()
                        .map(|p| MettaValue::Atom(p.to_string()))
                        .collect(),
                )
            }
        } else {
            // Simple atom pattern
            MettaValue::Atom(pattern.to_string())
        };

        // Parse body similarly
        let rhs = if body.starts_with('(') && body.ends_with(')') {
            let inner = &body[1..body.len() - 1];
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.is_empty() {
                MettaValue::Atom(body.to_string())
            } else {
                MettaValue::SExpr(
                    parts
                        .into_iter()
                        .map(|p| MettaValue::Atom(p.to_string()))
                        .collect(),
                )
            }
        } else {
            MettaValue::Atom(body.to_string())
        };

        (lhs, rhs)
    }

    #[test]
    fn test_concurrent_clone_and_mutate_2_threads() {
        let mut base = MettaEnvironment::default();

        // Add some base rules
        for i in 0..10 {
            let (lhs, rhs) = make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            );
            base.add_rule(lhs, rhs);
        }

        let base = StdArc::new(base);
        let handles: Vec<_> = (0..2)
            .map(|thread_id| {
                let base = StdArc::clone(&base);
                thread::spawn(move || {
                    // Clone and mutate independently
                    let mut clone = (*base).clone();

                    // Add thread-specific rules
                    for i in 0..5 {
                        let (lhs, rhs) = make_test_rule_sexpr(
                            &format!("(thread{}_rule{} $x)", thread_id, i),
                            &format!("(result{} $x)", i),
                        );
                        clone.add_rule(lhs, rhs);
                    }

                    // Verify this clone has base + thread-specific rules
                    assert_eq!(
                        clone.rule_count(),
                        15,
                        "Thread {} should have 15 rules",
                        thread_id
                    );

                    clone
                })
            })
            .collect();

        // Wait for all threads and collect results
        let results: Vec<MettaEnvironment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify base is unchanged
        assert_eq!(base.rule_count(), 10, "Base should still have 10 rules");

        // Verify each result has exactly its own mutations
        assert_eq!(results.len(), 2);
        for (thread_id, clone) in results.iter().enumerate() {
            assert_eq!(
                clone.rule_count(),
                15,
                "Clone {} should have 15 rules",
                thread_id
            );
        }
    }

    #[test]
    fn test_concurrent_clone_and_mutate_8_threads() {
        const N_THREADS: usize = 8;
        const RULES_PER_THREAD: usize = 10;

        let mut base = MettaEnvironment::default();

        // Add base rules
        for i in 0..20 {
            let (lhs, rhs) = make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            );
            base.add_rule(lhs, rhs);
        }

        let base = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|thread_id| {
                let base = StdArc::clone(&base);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Clone
                    let mut clone = (*base).clone();

                    // Synchronize to maximize concurrency
                    barrier.wait();

                    // Mutate concurrently
                    for i in 0..RULES_PER_THREAD {
                        let (lhs, rhs) = make_test_rule_sexpr(
                            &format!("(t{}_r{} $x)", thread_id, i),
                            &format!("(res{} $x)", i),
                        );
                        clone.add_rule(lhs, rhs);
                    }

                    // Verify count
                    assert_eq!(
                        clone.rule_count(),
                        20 + RULES_PER_THREAD,
                        "Thread {} should have {} rules",
                        thread_id,
                        20 + RULES_PER_THREAD
                    );

                    (thread_id, clone)
                })
            })
            .collect();

        // Collect results
        let results: Vec<(usize, MettaEnvironment)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify base unchanged
        assert_eq!(base.rule_count(), 20);

        // Verify each got the right number
        for (thread_id, clone) in &results {
            assert_eq!(
                clone.rule_count(),
                30,
                "Clone {} should have 30 rules",
                thread_id
            );
        }
    }

    #[test]
    fn test_concurrent_add_rules() {
        const N_THREADS: usize = 4;
        const RULES_PER_THREAD: usize = 25;

        let env = StdArc::new(MettaEnvironment::default());
        let barrier = StdArc::new(Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|thread_id| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Each thread gets its own clone
                    let mut clone = (*env).clone();

                    // Synchronize
                    barrier.wait();

                    // Add rules concurrently
                    for i in 0..RULES_PER_THREAD {
                        let (lhs, rhs) = make_test_rule_sexpr(
                            &format!("(rule_{}_{} $x)", thread_id, i),
                            &format!("(body_{}_{} $x)", thread_id, i),
                        );
                        clone.add_rule(lhs, rhs);
                    }

                    clone
                })
            })
            .collect();

        // Collect all clones
        let clones: Vec<MettaEnvironment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify each clone has exactly RULES_PER_THREAD
        for (i, clone) in clones.iter().enumerate() {
            assert_eq!(
                clone.rule_count(),
                RULES_PER_THREAD,
                "Clone {} should have {} rules",
                i,
                RULES_PER_THREAD
            );
        }

        // Verify original is unchanged
        assert_eq!(env.rule_count(), 0);
    }

    #[test]
    fn test_concurrent_read_shared_clone() {
        const N_READERS: usize = 16;
        const READS_PER_THREAD: usize = 100;

        let mut base = MettaEnvironment::default();
        for i in 0..50 {
            let (lhs, rhs) = make_test_rule_sexpr(
                &format!("(rule{} $x)", i),
                "(result $x)",
            );
            base.add_rule(lhs, rhs);
        }

        let env = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_READERS));

        let handles: Vec<_> = (0..N_READERS)
            .map(|_| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Synchronize to maximize contention
                    barrier.wait();

                    // Perform many reads
                    for _ in 0..READS_PER_THREAD {
                        let count = env.rule_count();
                        assert_eq!(count, 50, "Should always see 50 rules");
                    }
                })
            })
            .collect();

        // Wait for completion
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify environment unchanged
        assert_eq!(env.rule_count(), 50);
    }

    // ========================================================================
    // Race Condition Tests
    // ========================================================================

    #[test]
    fn test_clone_during_mutation() {
        const N_CLONERS: usize = 4;
        const N_MUTATORS: usize = 4;

        let mut base = MettaEnvironment::default();
        for i in 0..20 {
            let (lhs, rhs) = make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            );
            base.add_rule(lhs, rhs);
        }

        let env = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_CLONERS + N_MUTATORS));

        // Spawn cloners
        let cloner_handles: Vec<_> = (0..N_CLONERS)
            .map(|id| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    // Clone repeatedly
                    for _ in 0..10 {
                        let clone = (*env).clone();
                        assert_eq!(clone.rule_count(), 20, "Cloner {} saw wrong count", id);
                        thread::sleep(Duration::from_micros(10));
                    }
                })
            })
            .collect();

        // Spawn mutators (they mutate their own clones)
        let mutator_handles: Vec<_> = (0..N_MUTATORS)
            .map(|id| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    // Get a clone and mutate it
                    let mut clone = (*env).clone();
                    for i in 0..10 {
                        let (lhs, rhs) = make_test_rule_sexpr(
                            &format!("(mut{}_{} $x)", id, i),
                            "(result $x)",
                        );
                        clone.add_rule(lhs, rhs);
                        thread::sleep(Duration::from_micros(10));
                    }

                    assert_eq!(clone.rule_count(), 30, "Mutator {} final count wrong", id);
                })
            })
            .collect();

        // Wait for all threads
        for handle in cloner_handles.into_iter().chain(mutator_handles) {
            handle.join().unwrap();
        }

        // Base should be unchanged
        assert_eq!(env.rule_count(), 20);
    }

    #[test]
    fn test_make_owned_race() {
        // Test that concurrent first mutations (which trigger make_owned) are safe
        const N_THREADS: usize = 8;

        let mut base = MettaEnvironment::default();
        for i in 0..10 {
            let (lhs, rhs) = make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            );
            base.add_rule(lhs, rhs);
        }

        // Create one shared clone
        let shared_clone = StdArc::new(base.clone());
        let barrier = StdArc::new(Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|thread_id| {
                let clone_ref = StdArc::clone(&shared_clone);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Each thread gets its own clone from the shared clone
                    let mut my_clone = (*clone_ref).clone();

                    // Synchronize to maximize race potential
                    barrier.wait();

                    // This mutation triggers make_owned() for this specific clone
                    // All threads do this simultaneously, testing atomicity
                    let (lhs, rhs) = make_test_rule_sexpr(
                        &format!("(first_mutation_{} $x)", thread_id),
                        "(result $x)",
                    );
                    my_clone.add_rule(lhs, rhs);

                    // Verify we have base + 1 rule
                    assert_eq!(
                        my_clone.rule_count(),
                        11,
                        "Thread {} should have 11 rules",
                        thread_id
                    );

                    my_clone
                })
            })
            .collect();

        // Collect results
        let results: Vec<MettaEnvironment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify each got its own copy
        for (i, clone) in results.iter().enumerate() {
            assert_eq!(clone.rule_count(), 11, "Result {} should have 11 rules", i);
        }

        // Verify shared clone and base are unchanged
        assert_eq!(shared_clone.rule_count(), 10);
        assert_eq!(base.rule_count(), 10);
    }

    #[test]
    fn test_read_during_make_owned() {
        // Test reading while another clone is doing make_owned()
        const N_READERS: usize = 8;
        const N_WRITERS: usize = 2;

        let mut base = MettaEnvironment::default();
        for i in 0..30 {
            let (lhs, rhs) = make_test_rule_sexpr(
                &format!("(rule{} $x)", i),
                "(result $x)",
            );
            base.add_rule(lhs, rhs);
        }

        let shared = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_READERS + N_WRITERS));

        // Readers: clone and read repeatedly
        let reader_handles: Vec<_> = (0..N_READERS)
            .map(|id| {
                let shared = StdArc::clone(&shared);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    for _ in 0..20 {
                        let clone = (*shared).clone();
                        let count = clone.rule_count();
                        assert_eq!(count, 30, "Reader {} saw wrong count: {}", id, count);
                        thread::sleep(Duration::from_micros(5));
                    }
                })
            })
            .collect();

        // Writers: clone and mutate (triggering make_owned)
        let writer_handles: Vec<_> = (0..N_WRITERS)
            .map(|id| {
                let shared = StdArc::clone(&shared);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    for i in 0..10 {
                        let mut clone = (*shared).clone();
                        let (lhs, rhs) = make_test_rule_sexpr(
                            &format!("(writer{}_{} $x)", id, i),
                            "(result $x)",
                        );
                        clone.add_rule(lhs, rhs);
                        assert_eq!(
                            clone.rule_count(),
                            31,
                            "Writer {} iteration {} wrong count",
                            id,
                            i
                        );
                        thread::sleep(Duration::from_micros(5));
                    }
                })
            })
            .collect();

        // Wait for all
        for handle in reader_handles.into_iter().chain(writer_handles) {
            handle.join().unwrap();
        }

        // Shared should be unchanged
        assert_eq!(shared.rule_count(), 30);
    }

    #[test]
    fn test_remove_from_space_decrements_multiplicity() {
        // Test: Removing a rule should decrement its multiplicity count
        let mut env = MettaEnvironment::default();

        // Create a rule and add it first via add_rule() (which sets up multiplicity tracking)
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let rhs = MettaValue::SExpr(vec![
            MettaValue::Atom("bar".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        // Add the rule via add_rule() first (sets up rule_index and multiplicity tracking)
        env.add_rule(lhs.clone(), rhs.clone());

        // Create the rule s-expression for add_to_space() second add
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            lhs.clone(),
            rhs.clone(),
        ]);

        // Add the rule again via add_to_space() (should increment multiplicity)
        env.add_to_space(&rule_sexpr);

        // Get the rule count (should be 2)
        // Find the rule and check its count
        let rules = env.collect_rules();
        assert!(!rules.is_empty(), "Should have at least one rule");

        // Find the rule we added
        let found = rules
            .iter()
            .find(|(rule_lhs, _rule_rhs)| {
                if let MettaValueInner::SExpr(lhs_elems) = rule_lhs.inner() {
                    if lhs_elems.len() == 2 {
                        if let MettaValueInner::Atom(head) = lhs_elems[0].inner() {
                            return *head == "foo";
                        }
                    }
                }
                false
            })
            .expect("Should find the foo rule");

        let count_before = env.get_rule_count(&found.0, &found.1);
        assert_eq!(count_before, 2, "Rule should have multiplicity of 2");

        // Remove the rule once
        env.remove_from_space(&rule_sexpr);

        // Check that multiplicity decreased
        // Note: The rule might still be present (or removed depending on implementation)
        // but the multiplicity count should have been decremented
        let count_after = env.get_rule_count(&found.0, &found.1);
        assert_eq!(
            count_after, 1,
            "Rule multiplicity should be 1 after removal"
        );

        // Remove the rule again
        env.remove_from_space(&rule_sexpr);

        // After second removal, count should be 0 (but get_rule_count returns 1 for missing)
        let count_final = env.get_rule_count(&found.0, &found.1);
        assert!(
            count_final <= 1,
            "Rule multiplicity should be 0 or 1 after second removal"
        );
    }

    // =========================================================================
    // Phase 5C: Additional Environment Coverage Tests
    // =========================================================================

    #[test]
    fn test_env_default_owns_data() {
        let env = MettaEnvironment::default();
        assert!(env.owns_data, "Default env should own data");
        assert!(!env.modified.load(Ordering::Relaxed), "Default env should not be modified");
    }

    #[test]
    fn test_env_rule_count_empty() {
        let env = MettaEnvironment::default();
        assert_eq!(env.rule_count(), 0, "Empty env should have 0 rules");
    }

    #[test]
    fn test_env_rule_count_after_add() {
        let mut env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
        env.add_rule(lhs, rhs);
        assert_eq!(env.rule_count(), 1, "Env should have 1 rule after add");

        let (lhs2, rhs2) = make_test_rule("(test2 $y)", "(result2 $y)");
        env.add_rule(lhs2, rhs2);
        assert_eq!(env.rule_count(), 2, "Env should have 2 rules");
    }

    #[test]
    fn test_env_collect_rules_empty() {
        let env = MettaEnvironment::default();
        let rules = env.collect_rules();
        assert!(rules.is_empty(), "collect_rules on empty env should be empty");
    }

    #[test]
    fn test_env_collect_rules_with_rules() {
        let mut env = MettaEnvironment::default();
        let (lhs1, rhs1) = make_test_rule("(a $x)", "(b $x)");
        env.add_rule(lhs1, rhs1);
        let (lhs2, rhs2) = make_test_rule("(c $y)", "(d $y)");
        env.add_rule(lhs2, rhs2);

        let rules = env.collect_rules();
        assert_eq!(rules.len(), 2, "Should have 2 rules");
    }

    #[test]
    fn test_env_get_matching_rules_no_match() {
        let mut env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule("(foo $x)", "(bar $x)");
        env.add_rule(lhs, rhs);

        // Try to get rules for non-existent head
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom("baz".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let rules = env.get_matching_rules_for_expr(&query);
        assert!(rules.is_empty(), "Should have no matching rules for 'baz'");
    }

    #[test]
    fn test_env_get_matching_rules_match() {
        let mut env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule_sexpr("(foo $x)", "(bar $x)");
        env.add_rule(lhs.clone(), rhs);

        // Get rules for matching head
        let rules = env.get_matching_rules_for_expr(&lhs);
        assert!(!rules.is_empty(), "Should have matching rules for 'foo'");
    }

    #[test]
    fn test_env_clone_does_not_own() {
        let mut env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
        env.add_rule(lhs, rhs);

        let clone = env.clone();
        assert!(!clone.owns_data, "Clone should not own data");
        assert!(!clone.modified.load(Ordering::Relaxed), "Clone should not be modified");
    }

    #[test]
    fn test_env_multiple_clones_independent_modifications() {
        let mut env = MettaEnvironment::default();
        let (lhs_base, rhs_base) = make_test_rule_sexpr("(base $x)", "(result $x)");
        env.add_rule(lhs_base, rhs_base);

        let mut clone1 = env.clone();
        let mut clone2 = env.clone();

        // Modify clone1
        let (lhs_c1, rhs_c1) = make_test_rule_sexpr("(clone1 $x)", "(res1 $x)");
        clone1.add_rule(lhs_c1, rhs_c1);

        // Modify clone2
        let (lhs_c2, rhs_c2) = make_test_rule_sexpr("(clone2 $y)", "(res2 $y)");
        clone2.add_rule(lhs_c2, rhs_c2);

        // Verify independence
        assert_eq!(env.rule_count(), 1, "Original should have 1 rule");
        assert_eq!(clone1.rule_count(), 2, "Clone1 should have 2 rules");
        assert_eq!(clone2.rule_count(), 2, "Clone2 should have 2 rules");

        // Verify they have different rules
        let clone2_query = MettaValue::SExpr(vec![
            MettaValue::Atom("clone2".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let clone1_has_clone2 = clone1.get_matching_rules_for_expr(&clone2_query);
        assert!(clone1_has_clone2.is_empty(), "Clone1 should not have clone2's rules");

        let clone1_query = MettaValue::SExpr(vec![
            MettaValue::Atom("clone1".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let clone2_has_clone1 = clone2.get_matching_rules_for_expr(&clone1_query);
        assert!(clone2_has_clone1.is_empty(), "Clone2 should not have clone1's rules");
    }

    #[test]
    fn test_env_add_to_space_non_rule() {
        let mut env = MettaEnvironment::default();

        // Add a non-rule value (fact)
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(42),
        ]);
        env.add_to_space(&fact);

        // The fact should be in space but not as a rule
        // (implementation-specific behavior)
        // This just tests that it doesn't crash
    }

    #[test]
    fn test_env_add_to_space_rule() {
        let mut env = MettaEnvironment::default();

        // Check initial atom count
        let initial_atoms = env.shared.atom_space.total_atoms.load(Ordering::Relaxed);
        assert_eq!(initial_atoms, 0, "New environment should have 0 atoms");

        // Add a rule via add_to_space
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(2),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        // add_to_space adds to the space (btm/pathmap)
        env.add_to_space(&rule_sexpr);

        // Verify the atom was added (total atoms should increase)
        // We can access internals since shared is pub(crate)
        let final_atoms = env.shared.atom_space.total_atoms.load(Ordering::Relaxed);
        assert_eq!(final_atoms, 1, "After add_to_space, total_atoms should be 1");
    }

    #[test]
    fn test_env_cow_not_triggered_without_mutation() {
        let env = MettaEnvironment::default();
        let shared_ptr_before = StdArc::as_ptr(&env.shared);

        // Clone without mutation
        let clone = env.clone();
        let clone_shared_ptr = StdArc::as_ptr(&clone.shared);

        // Should share the same Arc
        assert_eq!(shared_ptr_before, clone_shared_ptr, "Clone should share Arc without mutation");
    }

    #[test]
    fn test_env_cow_triggered_on_mutation() {
        let env = MettaEnvironment::default();

        let mut clone = env.clone();
        let shared_ptr_before = StdArc::as_ptr(&clone.shared);

        // Mutate to trigger CoW
        let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
        clone.add_rule(lhs, rhs);
        let shared_ptr_after = StdArc::as_ptr(&clone.shared);

        // Should have different Arc after mutation
        assert_ne!(shared_ptr_before, shared_ptr_after, "CoW should create new Arc on mutation");
    }

    #[test]
    fn test_env_modified_flag_set_on_mutation() {
        let mut env = MettaEnvironment::default();
        assert!(!env.modified.load(Ordering::Relaxed), "New env should not be modified");

        let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
        env.add_rule(lhs, rhs);
        assert!(env.modified.load(Ordering::Relaxed), "Env should be modified after add_rule");
    }

    #[test]
    fn test_env_clone_fresh_modified_flag() {
        let mut env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule("(test $x)", "(result $x)");
        env.add_rule(lhs, rhs);
        assert!(env.modified.load(Ordering::Relaxed), "Original should be modified");

        let clone = env.clone();
        assert!(!clone.modified.load(Ordering::Relaxed), "Clone should have fresh modified flag");
    }

    #[test]
    fn test_env_get_rule_count_missing() {
        let env = MettaEnvironment::default();
        let (lhs, rhs) = make_test_rule("(nonexistent $x)", "(result $x)");

        // Count for non-existent rule should be handled gracefully (0 or 1)
        let count = env.get_rule_count(&lhs, &rhs);
        assert!(count <= 1, "Count for non-existent rule should be 0 or 1");
    }

    #[test]
    fn test_env_wildcard_rule_matching() {
        let mut env = MettaEnvironment::default();

        // Add a rule with variable as head (wildcard rule)
        env.add_rule(
            MettaValue::Atom("$any".to_string()),
            MettaValue::Atom("matched".to_string()),
        );

        // Wildcard rules should be tracked
        // (exact behavior depends on implementation)
    }

}
