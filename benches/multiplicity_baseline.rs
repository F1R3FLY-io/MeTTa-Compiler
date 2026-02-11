//! Baseline benchmark for multiplicity tracking operations.
//!
//! This benchmark measures Environment-level multiplicity operations
//! using only public API, allowing A/B comparison between HashMap and AtomMultiset
//! implementations.
//!
//! Key metrics measured:
//! - Rule insertion with multiplicity tracking
//! - Rule count lookup (hot path in evaluation)
//! - Fork/CoW snapshot operations
//! - Mixed workloads (insert + lookup)
//!
//! Usage:
//! ```bash
//! # Save baseline (with HashMap implementation):
//! git stash push -m "AtomMultiset implementation"
//! cargo bench --bench multiplicity_baseline -- --save-baseline hashmap
//! git stash pop
//!
//! # Compare against baseline (with AtomMultiset implementation):
//! cargo bench --bench multiplicity_baseline -- --baseline hashmap
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::environment::MettaEnvironment;
use mettatron::backend::{MettaValue, MettaValueTrait, Rule};

/// Generate N rules for benchmarking
/// Uses realistic rule structures similar to those in actual MeTTa programs
fn generate_rules(n: usize) -> Vec<Rule> {
    (0..n)
        .map(|i| {
            Rule::new(
                MettaValue::SExpr(vec![
                    MettaValue::Atom(format!("rule{}", i % 100)), // Some overlap for multiplicity
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("result".to_string()),
                    MettaValue::Long(i as i64),
                    MettaValue::Atom("$x".to_string()),
                ]),
            )
        })
        .collect()
}

/// Benchmark: Rule insertion with multiplicity tracking
fn bench_rule_insertion_with_multiplicity(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiplicity_insertion");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        let rules = generate_rules(*rule_count);

        group.bench_with_input(
            BenchmarkId::new("add_rule", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let mut env = MettaEnvironment::default();
                    for rule in &rules {
                        env.add_rule(black_box(rule.clone()));
                    }
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Rule count lookup (hot path in evaluation)
fn bench_rule_count_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiplicity_lookup");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        let rules = generate_rules(*rule_count);

        // Pre-populate environment with rules
        let mut env = MettaEnvironment::default();
        for rule in &rules {
            env.add_rule(rule.clone());
            // Add some rules twice to create multiplicities > 1
            if rule
                .lhs
                .get_head_symbol()
                .unwrap_or("")
                .starts_with("rule0")
            {
                env.add_rule(rule.clone());
            }
        }

        group.bench_with_input(
            BenchmarkId::new("get_rule_count", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    // Look up count for each rule (simulates evaluation hot path)
                    for rule in &rules {
                        black_box(env.get_rule_count(rule));
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Mixed workload (insert + lookup)
/// This simulates realistic evaluation patterns
fn bench_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiplicity_mixed");

    for op_count in [100, 500, 1000].iter() {
        let rules = generate_rules(*op_count);

        group.bench_with_input(
            BenchmarkId::new("insert_then_lookup", op_count),
            op_count,
            |b, _| {
                b.iter(|| {
                    let mut env = MettaEnvironment::default();
                    // Insert phase
                    for rule in &rules {
                        env.add_rule(rule.clone());
                    }
                    // Lookup phase
                    for rule in &rules {
                        black_box(env.get_rule_count(rule));
                    }
                    black_box(env);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("interleaved_insert_lookup", op_count),
            op_count,
            |b, _| {
                b.iter(|| {
                    let mut env = MettaEnvironment::default();
                    // Interleaved pattern
                    for rule in &rules {
                        env.add_rule(rule.clone());
                        black_box(env.get_rule_count(rule));
                    }
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Environment fork with multiplicity data
fn bench_environment_fork(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiplicity_fork");

    for rule_count in [100, 500, 1000].iter() {
        let rules = generate_rules(*rule_count);

        // Pre-populate environment
        let mut env = MettaEnvironment::default();
        for rule in &rules {
            env.add_rule(rule.clone());
        }

        group.bench_with_input(
            BenchmarkId::new("clone_with_multiplicity", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let cloned = env.clone();
                    black_box(cloned);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fork_for_nondeterminism", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let forked = env.fork_for_nondeterminism();
                    black_box(forked);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_rule_insertion_with_multiplicity,
    bench_rule_count_lookup,
    bench_mixed_workload,
    bench_environment_fork
);

criterion_main!(benches);
