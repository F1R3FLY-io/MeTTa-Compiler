//! Benchmark for match_space() with multiplicity tracking.
//!
//! This benchmark measures the actual production hot path where:
//! 1. MORK bytes are already available from PathMap iteration
//! 2. get_multiplicity() is called for each matching atom
//!
//! This is distinct from get_rule_count() which requires MORK byte conversion.
//!
//! Usage:
//! ```bash
//! cargo bench --bench match_space_multiplicity
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;
use mettatron::backend::MettaValue;
use std::time::Duration;

/// Helper to create a variable
fn var(name: &str) -> MettaValue {
    MettaValue::Atom(format!("${}", name))
}

/// Benchmark match_space with varying atom counts (multiplicity = 1)
fn bench_match_space_single_multiplicity(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_space_single");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Reduced ranges to avoid OOM: [100, 500, 1000, 5000] -> [50, 100, 200, 500]
    for atom_count in [50, 100, 200, 500].iter() {
        // Pre-populate environment with atoms
        let env = Environment::new();

        // Add unique atoms to the Space
        for i in 0..*atom_count {
            let fact_src = format!("!(add-atom &space (fact {} value-{}))", i, i);
            let fact_state = compile(&fact_src).expect("Failed to compile fact");
            for fact_expr in fact_state.source {
                eval(fact_expr, env.clone());
            }
        }

        // Pattern that matches all facts: (fact $x $y)
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            var("x"),
            var("y"),
        ]);
        let template = MettaValue::SExpr(vec![
            MettaValue::Atom("matched".to_string()),
            var("x"),
            var("y"),
        ]);

        group.bench_with_input(
            BenchmarkId::new("all_matches", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let results = env.match_space(black_box(&pattern), black_box(&template));
                    black_box(results)
                });
            },
        );

        // Pattern that matches single fact: (fact 0 $y)
        let specific_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(0),
            var("y"),
        ]);

        group.bench_with_input(
            BenchmarkId::new("single_match", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let results =
                        env.match_space(black_box(&specific_pattern), black_box(&template));
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark match_space with atoms having multiplicity > 1
/// This is the key benchmark for testing if get_multiplicity() is a bottleneck
fn bench_match_space_high_multiplicity(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_space_multiplicity");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Test with different multiplicities - reduced to avoid OOM: [1, 2, 5, 10] -> [1, 2, 3, 5]
    for multiplicity in [1, 2, 3, 5].iter() {
        let env = Environment::new();

        // Reduced unique atom count to avoid OOM: 100 -> 50
        for i in 0..50 {
            for _ in 0..*multiplicity {
                let fact_src = format!("!(add-atom &space (data {} info-{}))", i, i);
                let fact_state = compile(&fact_src).expect("Failed to compile fact");
                for fact_expr in fact_state.source {
                    eval(fact_expr, env.clone());
                }
            }
        }

        // Pattern that matches all data atoms
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("data".to_string()),
            var("x"),
            var("y"),
        ]);
        let template = MettaValue::SExpr(vec![MettaValue::Atom("result".to_string()), var("x")]);

        group.bench_with_input(
            BenchmarkId::new("100_atoms", multiplicity),
            multiplicity,
            |b, _| {
                b.iter(|| {
                    let results = env.match_space(black_box(&pattern), black_box(&template));
                    // With multiplicity N, we expect 100 * N results
                    black_box(results)
                });
            },
        );
    }

    // Test scaling: fixed multiplicity=3, varying atom count
    // Reduced to avoid OOM: [50, 100, 500, 1000] -> [25, 50, 100, 200]
    // Reduced multiplicity: 5 -> 3
    for atom_count in [25, 50, 100, 200].iter() {
        let env = Environment::new();
        let multiplicity = 3;

        for i in 0..*atom_count {
            for _ in 0..multiplicity {
                let fact_src = format!("!(add-atom &space (item {} val-{}))", i, i);
                let fact_state = compile(&fact_src).expect("Failed to compile fact");
                for fact_expr in fact_state.source {
                    eval(fact_expr, env.clone());
                }
            }
        }

        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("item".to_string()),
            var("x"),
            var("y"),
        ]);
        let template = var("x");

        group.bench_with_input(
            BenchmarkId::new("mult5_atoms", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let results = env.match_space(black_box(&pattern), black_box(&template));
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark match_space_first (early exit optimization)
fn bench_match_space_first(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_space_first");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Reduced ranges to avoid OOM: [100, 500, 1000, 5000] -> [50, 100, 200, 500]
    for atom_count in [50, 100, 200, 500].iter() {
        let env = Environment::new();

        // Add atoms - target is in the middle
        for i in 0..*atom_count {
            let fact_src = format!("!(add-atom &space (entry {} data-{}))", i, i);
            let fact_state = compile(&fact_src).expect("Failed to compile fact");
            for fact_expr in fact_state.source {
                eval(fact_expr, env.clone());
            }
        }

        // Pattern that matches first atom
        let first_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("entry".to_string()),
            MettaValue::Long(0),
            var("y"),
        ]);
        let template = var("y");

        group.bench_with_input(
            BenchmarkId::new("first_atom", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let result =
                        env.match_space_first(black_box(&first_pattern), black_box(&template));
                    black_box(result)
                });
            },
        );

        // Pattern that matches middle atom
        let mid_idx = *atom_count / 2;
        let mid_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("entry".to_string()),
            MettaValue::Long(mid_idx as i64),
            var("y"),
        ]);

        group.bench_with_input(
            BenchmarkId::new("middle_atom", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let result =
                        env.match_space_first(black_box(&mid_pattern), black_box(&template));
                    black_box(result)
                });
            },
        );

        // Pattern that matches no atom (worst case - full scan)
        let miss_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("entry".to_string()),
            MettaValue::Long(-1), // Never exists
            var("y"),
        ]);

        group.bench_with_input(
            BenchmarkId::new("no_match", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let result =
                        env.match_space_first(black_box(&miss_pattern), black_box(&template));
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark match_space_exists (existence check only)
fn bench_match_space_exists(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_space_exists");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // Reduced ranges to avoid OOM: [100, 500, 1000, 5000] -> [50, 100, 200, 500]
    for atom_count in [50, 100, 200, 500].iter() {
        let env = Environment::new();

        for i in 0..*atom_count {
            let fact_src = format!("!(add-atom &space (record {} field-{}))", i, i);
            let fact_state = compile(&fact_src).expect("Failed to compile fact");
            for fact_expr in fact_state.source {
                eval(fact_expr, env.clone());
            }
        }

        // Check existence of first record
        let exists_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("record".to_string()),
            MettaValue::Long(0),
            var("y"),
        ]);

        group.bench_with_input(
            BenchmarkId::new("exists_first", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let exists = env.match_space_exists(black_box(&exists_pattern));
                    black_box(exists)
                });
            },
        );

        // Check existence of non-existent record
        let not_exists_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("record".to_string()),
            MettaValue::Long(-1),
            var("y"),
        ]);

        group.bench_with_input(
            BenchmarkId::new("not_exists", atom_count),
            atom_count,
            |b, _| {
                b.iter(|| {
                    let exists = env.match_space_exists(black_box(&not_exists_pattern));
                    black_box(exists)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_match_space_single_multiplicity,
    bench_match_space_high_multiplicity,
    bench_match_space_first,
    bench_match_space_exists
);

criterion_main!(benches);
