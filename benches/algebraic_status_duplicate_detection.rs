// Phase 3b: AlgebraicStatus Optimization - Empirical Validation Benchmarks
//
// This benchmark suite validates the Phase 3b AlgebraicStatus optimization by measuring
// performance across five distinct scenarios:
//
// 1. All New Data (baseline verification)
// 2. All Duplicate Data (maximum benefit)
// 3. Mixed Duplicate Ratios (realistic workloads)
// 4. CoW Clone Impact (downstream effects)
// 5. Type Index Invalidation (facts-specific benefit)
//
// Expected behavior:
// - AlgebraicStatus::Element when data changes → mark as modified
// - AlgebraicStatus::Identity when no changes → skip modification
//
// Hypothesis: Duplicate-heavy workloads should show unbounded savings from:
// - Skipped modified flag updates
// - Skipped type index invalidation (facts)
// - Skipped CoW deep copies on next clone
// - Skipped downstream evaluation work

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::environment::Environment;
use mettatron::backend::{MettaValue, Rule};

// ================================================================================================
// Helper Functions
// ================================================================================================

/// Generate N unique facts for benchmarking
fn create_test_facts(n: usize) -> Vec<MettaValue> {
    let mut facts = Vec::new();
    for i in 0..n {
        facts.push(MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(i as i64),
            MettaValue::Atom(format!("value-{}", i)),
        ]));
    }
    facts
}

/// Generate N unique rules for benchmarking
fn create_test_rules(n: usize) -> Vec<Rule> {
    let mut rules = Vec::new();
    for i in 0..n {
        rules.push(Rule::new(
            MettaValue::SExpr(vec![
                MettaValue::Atom("pattern".to_string()),
                MettaValue::Long(i as i64),
            ]),
            MettaValue::Atom(format!("result-{}", i)),
        ));
    }
    rules
}

/// Generate N unique type assertions for benchmarking
fn create_test_type_facts(n: usize) -> Vec<MettaValue> {
    let mut facts = Vec::new();
    for i in 0..n {
        facts.push(MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom(format!("var{}", i)),
            MettaValue::Atom(format!("Type{}", i % 10)), // 10 different types
        ]));
    }
    facts
}

/// Prepopulate environment with facts
fn prepopulate_with_facts(env: &mut Environment, facts: &[MettaValue]) {
    env.add_facts_bulk(facts).unwrap();
}

/// Prepopulate environment with rules
fn prepopulate_with_rules(env: &mut Environment, rules: Vec<Rule>) {
    env.add_rules_bulk(rules).unwrap();
}

/// Create mixed dataset with specified duplicate ratio
/// Returns (all_items, new_items_only)
fn create_mixed_fact_dataset(
    total: usize,
    duplicate_ratio: f64,
) -> (Vec<MettaValue>, Vec<MettaValue>) {
    let num_duplicates = (total as f64 * duplicate_ratio) as usize;
    let _num_new = total - num_duplicates;

    let all_items = create_test_facts(total);
    let new_items = all_items[num_duplicates..].to_vec();

    (all_items, new_items)
}

/// Create mixed rule dataset with specified duplicate ratio
fn create_mixed_rule_dataset(total: usize, duplicate_ratio: f64) -> (Vec<Rule>, Vec<Rule>) {
    let num_duplicates = (total as f64 * duplicate_ratio) as usize;
    let _num_new = total - num_duplicates;

    let all_items = create_test_rules(total);
    let new_items = all_items[num_duplicates..].to_vec();

    (all_items, new_items)
}

// ================================================================================================
// Group 1: All New Data (Baseline Verification)
// ================================================================================================
// Expected: No regression vs existing bulk_operations.rs
// AlgebraicStatus should always be Element (data changed)

fn bench_add_facts_all_new(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_facts_all_new");

    for size in [10, 100, 500, 1000, 5000].iter() {
        let facts = create_test_facts(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                env.add_facts_bulk(black_box(&facts)).unwrap();
                black_box(env);
            });
        });
    }

    group.finish();
}

fn bench_add_rules_all_new(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_rules_all_new");

    for size in [10, 100, 500, 1000, 5000].iter() {
        let rules = create_test_rules(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                env.add_rules_bulk(black_box(rules.clone())).unwrap();
                black_box(env);
            });
        });
    }

    group.finish();
}

// ================================================================================================
// Group 2: All Duplicate Data (Maximum Benefit)
// ================================================================================================
// Expected: AlgebraicStatus::Identity → skip modified flag
// Hypothesis: Significant speedup from skipped work

fn bench_add_facts_all_duplicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_facts_all_duplicates");

    for size in [10, 100, 500, 1000, 5000].iter() {
        let facts = create_test_facts(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                // First insertion: adds data (Element)
                env.add_facts_bulk(&facts).unwrap();
                // Second insertion: duplicates (Identity) - this is what we measure
                env.add_facts_bulk(black_box(&facts)).unwrap();
                black_box(env);
            });
        });
    }

    group.finish();
}

fn bench_add_rules_all_duplicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_rules_all_duplicates");

    for size in [10, 100, 500, 1000, 5000].iter() {
        let rules = create_test_rules(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                // First insertion: adds data (Element)
                env.add_rules_bulk(rules.clone()).unwrap();
                // Second insertion: duplicates (Identity) - this is what we measure
                env.add_rules_bulk(black_box(rules.clone())).unwrap();
                black_box(env);
            });
        });
    }

    group.finish();
}

// ================================================================================================
// Group 3: Mixed Duplicate Ratios (Realistic Workloads)
// ================================================================================================
// Expected: Savings proportional to duplicate ratio
// Test ratios: 0%, 25%, 50%, 75%, 100%

fn bench_add_facts_mixed_ratios(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_facts_mixed_ratios");
    let size = 1000;

    for ratio_percent in [0, 25, 50, 75, 100].iter() {
        let ratio = *ratio_percent as f64 / 100.0;
        let (all_items, _new_items) = create_mixed_fact_dataset(size, ratio);

        group.bench_with_input(
            BenchmarkId::new("ratio", ratio_percent),
            ratio_percent,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    // Pre-populate with items that will be duplicated
                    let num_duplicates = (size as f64 * ratio) as usize;
                    if num_duplicates > 0 {
                        prepopulate_with_facts(&mut env, &all_items[..num_duplicates]);
                    }
                    // Now add mixed batch (some duplicates, some new)
                    env.add_facts_bulk(black_box(&all_items)).unwrap();
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

fn bench_add_rules_mixed_ratios(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_rules_mixed_ratios");
    let size = 1000;

    for ratio_percent in [0, 25, 50, 75, 100].iter() {
        let ratio = *ratio_percent as f64 / 100.0;
        let (all_items, _new_items) = create_mixed_rule_dataset(size, ratio);

        group.bench_with_input(
            BenchmarkId::new("ratio", ratio_percent),
            ratio_percent,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    // Pre-populate with items that will be duplicated
                    let num_duplicates = (size as f64 * ratio) as usize;
                    if num_duplicates > 0 {
                        prepopulate_with_rules(&mut env, all_items[..num_duplicates].to_vec());
                    }
                    // Now add mixed batch (some duplicates, some new)
                    env.add_rules_bulk(black_box(all_items.clone())).unwrap();
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

// ================================================================================================
// Group 4: CoW Clone Impact (Downstream Effects)
// ================================================================================================
// Expected: O(1) Arc increment vs O(n) deep copy
// Hypothesis: Unmodified environment clones faster

fn bench_cow_clone_after_duplicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_cow_clone_after_duplicates");

    for size in [100, 500, 1000].iter() {
        let facts = create_test_facts(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                env.add_facts_bulk(&facts).unwrap();
                // Add duplicates (Identity status → no modified flag)
                env.add_facts_bulk(&facts).unwrap();
                // Clone should be O(1) Arc increment
                let cloned = black_box(env.clone());
                black_box(cloned);
            });
        });
    }

    group.finish();
}

fn bench_cow_clone_after_new_data(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_cow_clone_after_new_data");

    for size in [100, 500, 1000].iter() {
        let facts1 = create_test_facts(*size);
        let mut facts2 = create_test_facts(*size);
        // Make facts2 different
        for fact in &mut facts2 {
            if let MettaValue::SExpr(ref mut items) = fact {
                if items.len() >= 2 {
                    items[1] = MettaValue::Long(999999);
                }
            }
        }

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                env.add_facts_bulk(&facts1).unwrap();
                // Add new data (Element status → modified flag set)
                env.add_facts_bulk(&facts2).unwrap();
                // Clone requires deep copy due to modified flag
                let cloned = black_box(env.clone());
                black_box(cloned);
            });
        });
    }

    group.finish();
}

// ================================================================================================
// Group 5: Type Index Invalidation (Facts-Specific Benefit)
// ================================================================================================
// Expected: Hot cache preserved for duplicates
// Hypothesis: No index rebuild when adding duplicate type assertions

fn bench_type_lookup_after_duplicate_facts(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_type_lookup_after_duplicates");

    for size in [100, 500, 1000].iter() {
        let type_facts = create_test_type_facts(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                env.add_facts_bulk(&type_facts).unwrap();
                // Add duplicates (Identity → type index NOT invalidated)
                env.add_facts_bulk(&type_facts).unwrap();
                // Lookup should use hot cache
                let result = black_box(env.get_type("var0"));
                black_box(result);
            });
        });
    }

    group.finish();
}

fn bench_type_lookup_after_new_facts(c: &mut Criterion) {
    let mut group = c.benchmark_group("algebraic_status_type_lookup_after_new_facts");

    for size in [100, 500, 1000].iter() {
        let type_facts1 = create_test_type_facts(*size);
        let mut type_facts2 = create_test_type_facts(*size);
        // Make type_facts2 different
        for fact in &mut type_facts2 {
            if let MettaValue::SExpr(ref mut items) = fact {
                if items.len() >= 2 {
                    if let MettaValue::Atom(ref mut name) = items[1] {
                        *name = format!("{}_new", name);
                    }
                }
            }
        }

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut env = Environment::new();
                env.add_facts_bulk(&type_facts1).unwrap();
                // Add new data (Element → type index invalidated)
                env.add_facts_bulk(&type_facts2).unwrap();
                // Lookup requires index rebuild
                let result = black_box(env.get_type("var0"));
                black_box(result);
            });
        });
    }

    group.finish();
}

// ================================================================================================
// Criterion Configuration
// ================================================================================================

criterion_group!(
    benches,
    // Group 1: All New Data
    bench_add_facts_all_new,
    bench_add_rules_all_new,
    // Group 2: All Duplicates
    bench_add_facts_all_duplicates,
    bench_add_rules_all_duplicates,
    // Group 3: Mixed Ratios
    bench_add_facts_mixed_ratios,
    bench_add_rules_mixed_ratios,
    // Group 4: CoW Clone Impact
    bench_cow_clone_after_duplicates,
    bench_cow_clone_after_new_data,
    // Group 5: Type Index Invalidation
    bench_type_lookup_after_duplicate_facts,
    bench_type_lookup_after_new_facts
);

criterion_main!(benches);
