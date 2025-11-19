use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::environment::Environment;
use mettatron::backend::{MettaValue, Rule};

/// Generate N facts for benchmarking
fn generate_facts(n: usize) -> Vec<MettaValue> {
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

/// Generate N rules for benchmarking
fn generate_rules(n: usize) -> Vec<Rule> {
    let mut rules = Vec::new();
    for i in 0..n {
        rules.push(Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("rule".to_string()),
                MettaValue::Long(i as i64),
            ]),
            rhs: MettaValue::Atom(format!("result-{}", i)),
        });
    }
    rules
}

/// Benchmark BASELINE: Individual fact insertion
fn bench_individual_facts(c: &mut Criterion) {
    let mut group = c.benchmark_group("fact_insertion_baseline");

    for fact_count in [10, 50, 100, 500, 1000].iter() {
        let facts = generate_facts(*fact_count);

        group.bench_with_input(
            BenchmarkId::new("individual_add_to_space", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    for fact in &facts {
                        env.add_to_space(black_box(fact));
                    }
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark OPTIMIZED: Bulk fact insertion
fn bench_bulk_facts(c: &mut Criterion) {
    let mut group = c.benchmark_group("fact_insertion_optimized");

    for fact_count in [10, 50, 100, 500, 1000].iter() {
        let facts = generate_facts(*fact_count);

        group.bench_with_input(
            BenchmarkId::new("bulk_add_facts_bulk", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    env.add_facts_bulk(black_box(&facts)).unwrap();
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark BASELINE: Individual rule insertion
fn bench_individual_rules(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_insertion_baseline");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        let rules = generate_rules(*rule_count);

        group.bench_with_input(
            BenchmarkId::new("individual_add_rule", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
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

/// Benchmark OPTIMIZED: Bulk rule insertion
fn bench_bulk_rules(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_insertion_optimized");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        let rules = generate_rules(*rule_count);

        group.bench_with_input(
            BenchmarkId::new("bulk_add_rules_bulk", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    env.add_rules_bulk(black_box(rules.clone())).unwrap();
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

/// Direct speedup comparison: Individual vs Bulk facts
fn bench_fact_speedup_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("fact_insertion_comparison");

    for fact_count in [100, 500, 1000].iter() {
        let facts = generate_facts(*fact_count);

        // Baseline
        group.bench_with_input(
            BenchmarkId::new("baseline", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    for fact in &facts {
                        env.add_to_space(black_box(fact));
                    }
                    black_box(env);
                });
            },
        );

        // Optimized
        group.bench_with_input(
            BenchmarkId::new("optimized", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    env.add_facts_bulk(black_box(&facts)).unwrap();
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

/// Direct speedup comparison: Individual vs Bulk rules
fn bench_rule_speedup_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_insertion_comparison");

    for rule_count in [100, 500, 1000].iter() {
        let rules = generate_rules(*rule_count);

        // Baseline
        group.bench_with_input(
            BenchmarkId::new("baseline", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    for rule in &rules {
                        env.add_rule(black_box(rule.clone()));
                    }
                    black_box(env);
                });
            },
        );

        // Optimized
        group.bench_with_input(
            BenchmarkId::new("optimized", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    env.add_rules_bulk(black_box(rules.clone())).unwrap();
                    black_box(env);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_individual_facts,
    bench_bulk_facts,
    bench_individual_rules,
    bench_bulk_rules,
    bench_fact_speedup_comparison,
    bench_rule_speedup_comparison
);

criterion_main!(benches);
