//! Tests for Environment's Copy-on-Write (CoW) semantics and thread safety.
//!
//! This module contains comprehensive tests for:
//! - CoW behavior: cloning, make_owned, isolation
//! - Thread safety: concurrent mutations, race conditions
//! - Stress tests: many clones, deep chains, concurrent access

use super::*;
use crate::backend::models::MettaValue;
use std::sync::atomic::Ordering;
use std::sync::{Arc as StdArc, Barrier};
use std::thread;

/// Helper: Create a simple rule for testing
fn make_test_rule(lhs: &str, rhs: &str) -> Rule {
    Rule::new(
        MettaValue::Atom(lhs.to_string()),
        MettaValue::Atom(rhs.to_string()),
    )
}

/// Helper: Extract head symbol and arity from a MettaValue (for get_matching_rules)
fn extract_head_arity(value: &MettaValue) -> (&str, usize) {
    match value {
        MettaValue::Atom(s) => (s.as_str(), 0),
        MettaValue::SExpr(vec) if !vec.is_empty() => {
            if let MettaValue::Atom(head) = &vec[0] {
                (head.as_str(), vec.len() - 1)
            } else {
                ("", 0) // Fallback for non-atom head
            }
        }
        _ => ("", 0), // Fallback for other cases
    }
}

/// Helper: Create a simple MettaValue fact for testing
#[allow(dead_code)]
fn make_test_fact(value: &str) -> MettaValue {
    MettaValue::Atom(value.to_string())
}

// ============================================================================
// UNIT TESTS - CoW Behavior
// ============================================================================

#[test]
fn test_new_environment_owns_data() {
    // Test: New environment should own its data
    let env = Environment::new();
    assert!(env.owns_data, "New environment should own its data");
    assert!(
        !env.modified.load(Ordering::Acquire),
        "New environment should not be modified"
    );
}

#[test]
fn test_clone_does_not_own_data() {
    // Test: Cloned environment should not own data initially
    let env = Environment::new();
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
    let env = Environment::new();

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
    let mut env = Environment::new();
    let rule = make_test_rule("(test $x)", "(result $x)");

    // Add rule to original (already owns data, no make_owned() needed)
    env.add_rule(rule.clone());
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
    clone.add_rule(make_test_rule("(clone $y)", "(cloned $y)"));

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
    let mut env = Environment::new();
    let rule1 = make_test_rule("(original $x)", "(original-result $x)");
    env.add_rule(rule1.clone());

    // Clone and add different rule
    let mut clone = env.clone();
    let rule2 = make_test_rule("(cloned $y)", "(cloned-result $y)");
    clone.add_rule(rule2.clone());

    // Original should only have rule1
    let (head1, arity1) = extract_head_arity(&rule1.lhs);
    let original_rules = env.get_matching_rules(head1, arity1);
    assert_eq!(original_rules.len(), 1, "Original should have 1 rule");

    // Clone should have both rules (rule1 was shared, rule2 was added)
    let clone_rules = clone.get_matching_rules(head1, arity1);
    assert_eq!(clone_rules.len(), 1, "Clone should have original rule");

    let (head2, arity2) = extract_head_arity(&rule2.lhs);
    let clone_rules2 = clone.get_matching_rules(head2, arity2);
    assert_eq!(clone_rules2.len(), 1, "Clone should have new rule");
}

#[test]
fn test_modification_tracking() {
    // Test: Modification flag is correctly tracked
    let mut env = Environment::new();
    assert!(
        !env.modified.load(Ordering::Acquire),
        "New env should not be modified"
    );

    // Add rule → should set modified flag
    env.add_rule(make_test_rule("(test $x)", "(result $x)"));
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
    clone.add_rule(make_test_rule("(test2 $y)", "(result2 $y)"));
    assert!(
        clone.modified.load(Ordering::Acquire),
        "Clone should be modified after mutation"
    );
}

#[test]
fn test_make_owned_idempotency() {
    // Test: make_owned() should be idempotent (safe to call multiple times)
    let env = Environment::new();
    let mut clone = env.clone();

    // First mutation triggers make_owned()
    clone.add_rule(make_test_rule("(test1 $x)", "(result1 $x)"));
    assert!(
        clone.owns_data,
        "Clone should own data after first mutation"
    );

    // Get Arc pointers after first make_owned()
    let shared_ptr_first = StdArc::as_ptr(&clone.shared);

    // Second mutation should NOT trigger another make_owned()
    clone.add_rule(make_test_rule("(test2 $y)", "(result2 $y)"));

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
    let mut env = Environment::new();
    env.add_rule(make_test_rule("(test $x)", "(result $x)"));

    let mut clone = env.clone();

    // Get Arc pointer before mutation (single consolidated pointer)
    let shared_before = StdArc::as_ptr(&clone.shared);

    // Trigger make_owned()
    clone.add_rule(make_test_rule("(clone $y)", "(cloned $y)"));

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
    let mut env = Environment::new();
    env.add_rule(make_test_rule("(original $x)", "(original-result $x)"));

    let mut clone1 = env.clone();
    let mut clone2 = env.clone();
    let mut clone3 = env.clone();

    // Mutate each clone differently
    clone1.add_rule(make_test_rule("(clone1 $a)", "(result1 $a)"));
    clone2.add_rule(make_test_rule("(clone2 $b)", "(result2 $b)"));
    clone3.add_rule(make_test_rule("(clone3 $c)", "(result3 $c)"));

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
        let mut env = Environment::new();
        env.add_rule(make_test_rule(&format!("(test{}  $x)", i), "(result $x)"));

        let mut clone = env.clone();
        clone.add_rule(make_test_rule(&format!("(clone{} $y)", i), "(cloned $y)"));

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
    let env = Environment::new();
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
                clone.add_rule(make_test_rule(
                    &format!("(thread{} $x)", i),
                    &format!("(result{} $x)", i),
                ));

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
    let env = Environment::new();

    for i in 0..1000 {
        let mut clone = env.clone();
        clone.add_rule(make_test_rule(&format!("(stress{} $x)", i), "(result $x)"));

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
    let mut env = Environment::new();
    env.add_rule(make_test_rule("(original $x)", "(result $x)"));

    let mut current = env.clone();
    for i in 0..10 {
        current.add_rule(make_test_rule(&format!("(depth{} $x)", i), "(result $x)"));
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
    let env = StdArc::new(Environment::new());
    let num_threads = 8;

    let handles: Vec<_> = (0..num_threads)
        .map(|i| {
            let env = StdArc::clone(&env);

            thread::spawn(move || {
                for j in 0..100 {
                    let mut clone = env.as_ref().clone();
                    clone.add_rule(make_test_rule(&format!("(t{}_{} $x)", i, j), "(result $x)"));
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
    use std::sync::Mutex as StdMutex;

    let base_env = Environment::new();
    let results = StdArc::new(StdMutex::new(Vec::new()));
    let num_threads = 4;

    let handles: Vec<_> = (0..num_threads)
        .map(|i| {
            let mut env = base_env.clone();
            let results = StdArc::clone(&results);

            thread::spawn(move || {
                // Each thread adds rules dynamically during "evaluation"
                for j in 0..10 {
                    let rule = make_test_rule(&format!("(eval{}_{}  $x)", i, j), "(result $x)");
                    env.add_rule(rule);
                }

                let count = env.rule_count();
                results.lock().unwrap().push(count);
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // Each thread should have 10 rules
    let results = results.lock().unwrap();
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
    let mut env = Environment::new();
    for i in 0..100 {
        env.add_rule(make_test_rule(&format!("(rule{} $x)", i), "(result $x)"));
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
    let mut env = Environment::new();

    // Add various rules
    let rules = vec![
        make_test_rule("(color car red)", "(assert color car red)"),
        make_test_rule("(color truck blue)", "(assert color truck blue)"),
        make_test_rule("(size car small)", "(assert size car small)"),
    ];

    for rule in &rules {
        env.add_rule(rule.clone());
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
    for rule in &rules {
        let (head, arity) = extract_head_arity(&rule.lhs);
        let original_matches = env.get_matching_rules(head, arity);
        let clone_matches = clone.get_matching_rules(head, arity);

        assert!(!original_matches.is_empty(), "Original should have rule");
        assert!(!clone_matches.is_empty(), "Clone should have rule");
    }
}

// ============================================================================
// Thread Safety Tests - Concurrent Mutation
// ============================================================================

mod thread_safety {
    use super::*;
    use std::time::Duration;

    // Helper: Create a test rule with proper SExpr structure
    fn make_test_rule_sexpr(pattern: &str, body: &str) -> Rule {
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

        Rule::new(lhs, rhs)
    }

    #[test]
    fn test_concurrent_clone_and_mutate_2_threads() {
        let mut base = Environment::new();

        // Add some base rules
        for i in 0..10 {
            base.add_rule(make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            ));
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
                        clone.add_rule(make_test_rule_sexpr(
                            &format!("(thread{}_rule{} $x)", thread_id, i),
                            &format!("(result{} $x)", i),
                        ));
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
        let results: Vec<Environment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

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

        let mut base = Environment::new();

        // Add base rules
        for i in 0..20 {
            base.add_rule(make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            ));
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
                        clone.add_rule(make_test_rule_sexpr(
                            &format!("(t{}_r{} $x)", thread_id, i),
                            &format!("(res{} $x)", i),
                        ));
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
        let results: Vec<(usize, Environment)> =
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

        let env = StdArc::new(Environment::new());
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
                        clone.add_rule(make_test_rule_sexpr(
                            &format!("(rule_{}_{} $x)", thread_id, i),
                            &format!("(body_{}_{} $x)", thread_id, i),
                        ));
                    }

                    clone
                })
            })
            .collect();

        // Collect all clones
        let clones: Vec<Environment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

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

        let mut base = Environment::new();
        for i in 0..50 {
            base.add_rule(make_test_rule_sexpr(
                &format!("(rule{} $x)", i),
                "(result $x)",
            ));
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

        let mut base = Environment::new();
        for i in 0..20 {
            base.add_rule(make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            ));
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
                        clone.add_rule(make_test_rule_sexpr(
                            &format!("(mut{}_{} $x)", id, i),
                            "(result $x)",
                        ));
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

        let mut base = Environment::new();
        for i in 0..10 {
            base.add_rule(make_test_rule_sexpr(
                &format!("(base{} $x)", i),
                "(result $x)",
            ));
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
                    my_clone.add_rule(make_test_rule_sexpr(
                        &format!("(first_mutation_{} $x)", thread_id),
                        "(result $x)",
                    ));

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
        let results: Vec<Environment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

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

        let mut base = Environment::new();
        for i in 0..30 {
            base.add_rule(make_test_rule_sexpr(
                &format!("(rule{} $x)", i),
                "(result $x)",
            ));
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
                        clone.add_rule(make_test_rule_sexpr(
                            &format!("(writer{}_{} $x)", id, i),
                            "(result $x)",
                        ));
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
}
