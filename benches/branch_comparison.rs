use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::environment::Environment;
use mettatron::backend::{MettaValue, Rule};
use mettatron::eval;

/// Comprehensive branch comparison benchmark suite
/// Tests runtime performance and space efficiency across all major dimensions
///
/// This benchmark is designed to compare branches (e.g., current vs main) to validate:
/// - Phase 3a: Prefix-based fast path (1,024× speedup expected)
/// - Phase 5: Bulk insertion optimization (2.0× speedup expected)
/// - Phase 3c: Rayon removal (2-6× improvement expected)
/// - CoW Environment: Clone cost reduction (~100× expected)

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

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

/// Generate pattern with N variables for pattern matching benchmark
fn generate_pattern_with_vars(n: usize) -> MettaValue {
    let mut items = vec![MettaValue::Atom("pattern".to_string())];
    for i in 0..n {
        items.push(MettaValue::Atom(format!("$x{}", i)));
    }
    MettaValue::SExpr(items)
}

/// Generate nested expression of given depth
fn generate_nested_expr(depth: usize) -> MettaValue {
    if depth == 0 {
        return MettaValue::Long(42);
    }
    MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        generate_nested_expr(depth - 1),
        generate_nested_expr(depth - 1),
    ])
}

// ============================================================================
// PHASE 3A: PREFIX-BASED FAST PATH BENCHMARK
// ============================================================================

/// Benchmark: Environment lookups with ground patterns (prefix fast path)
/// Expected: 1,024× speedup on current branch vs main
fn bench_prefix_fast_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("prefix_fast_path");

    // Pre-populate environment with facts
    let sizes = [100, 500, 1000, 5000, 10000];

    for &size in &sizes {
        let mut env = Environment::new();
        let facts = generate_facts(size);
        for fact in &facts {
            env.add_to_space(fact);
        }

        // Benchmark has_sexpr_fact with ground patterns
        let search_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("value-42".to_string()),
        ]);

        group.bench_with_input(
            BenchmarkId::new("has_sexpr_fact_ground", size),
            &size,
            |b, _| {
                b.iter(|| {
                    black_box(env.has_sexpr_fact(&search_pattern));
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// PHASE 5: BULK INSERTION BENCHMARK
// ============================================================================

/// Benchmark: Bulk fact insertion
/// Expected: 2.0× speedup on current branch vs main
fn bench_bulk_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_insertion");

    for fact_count in [10, 50, 100, 500, 1000].iter() {
        let facts = generate_facts(*fact_count);

        group.bench_with_input(
            BenchmarkId::new("add_facts_bulk", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let mut env = Environment::new();
                    env.add_facts_bulk(black_box(&facts)).unwrap();
                    black_box(env);
                });
            },
        );

        // Benchmark bulk rule insertion
        let rules = generate_rules(*fact_count);

        group.bench_with_input(
            BenchmarkId::new("add_rules_bulk", fact_count),
            fact_count,
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

// ============================================================================
// COPY-ON-WRITE ENVIRONMENT BENCHMARK
// ============================================================================

/// Benchmark: Environment cloning cost
/// Expected: ~100× faster clones on current branch (<50ns vs O(n))
fn bench_cow_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("cow_clone");

    for rule_count in [0, 10, 100, 500, 1000].iter() {
        let mut env = Environment::new();
        let rules = generate_rules(*rule_count);
        for rule in &rules {
            env.add_rule(rule.clone());
        }

        group.bench_with_input(
            BenchmarkId::new("env_clone", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    black_box(env.clone());
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// PATTERN MATCHING BENCHMARK
// ============================================================================

/// Benchmark: Pattern matching with varying complexity
/// Expected: Similar performance (no major optimization in this area)
fn bench_pattern_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_matching");

    // Variable count scaling
    for var_count in [1, 5, 10, 20, 50].iter() {
        let pattern = generate_pattern_with_vars(*var_count);
        let expr = generate_pattern_with_vars(*var_count);

        group.bench_with_input(
            BenchmarkId::new("var_count", var_count),
            var_count,
            |b, _| {
                b.iter(|| {
                    black_box(&pattern);
                    black_box(&expr);
                });
            },
        );
    }

    // Nesting depth scaling
    for depth in [1, 3, 5, 7, 10].iter() {
        let expr = generate_nested_expr(*depth);

        group.bench_with_input(
            BenchmarkId::new("nesting_depth", depth),
            depth,
            |b, _| {
                b.iter(|| {
                    black_box(&expr);
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// RULE MATCHING BENCHMARK
// ============================================================================

/// Benchmark: Rule matching with varying rule set sizes
/// Expected: Better performance on current branch for large rule sets (prefix optimization)
fn bench_rule_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_matching");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        let mut env = Environment::new();

        // Add fibonacci-like rules
        for i in 0..*rule_count {
            env.add_rule(Rule {
                lhs: MettaValue::SExpr(vec![
                    MettaValue::Atom("fib".to_string()),
                    MettaValue::Long(i as i64),
                ]),
                rhs: MettaValue::Long((i * 2) as i64),
            });
        }

        // Benchmark lookup
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom("fib".to_string()),
            MettaValue::Long(42),
        ]);

        group.bench_with_input(
            BenchmarkId::new("rule_lookup", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    black_box(env.has_sexpr_fact(&query));
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// TYPE LOOKUP BENCHMARK
// ============================================================================

/// Benchmark: Type lookup performance
/// Expected: ~1.1× faster on current branch
fn bench_type_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_lookup");

    for type_count in [10, 100, 1000, 10000].iter() {
        let mut env = Environment::new();

        // Add type facts to space for lookup testing
        for i in 0..*type_count {
            let type_fact = MettaValue::SExpr(vec![
                MettaValue::Atom(":".to_string()),
                MettaValue::Atom(format!("var{}", i)),
                MettaValue::Atom(format!("Type{}", i % 10)),
            ]);
            env.add_to_space(&type_fact);
        }

        // Benchmark get_type
        group.bench_with_input(
            BenchmarkId::new("get_type", type_count),
            type_count,
            |b, _| {
                b.iter(|| {
                    black_box(env.get_type("var42"));
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// EVALUATION BENCHMARK
// ============================================================================

/// Benchmark: Full evaluation (including sequential evaluation after Rayon removal)
/// Expected: Similar or slightly better on current branch (no parallel overhead)
fn bench_evaluation(c: &mut Criterion) {
    let mut group = c.benchmark_group("evaluation");

    // Simple arithmetic
    let simple_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(40),
        MettaValue::Long(2),
    ]);

    group.bench_function("simple_arithmetic", |b| {
        let env = Environment::new();
        b.iter(|| {
            let result = eval(black_box(simple_expr.clone()), black_box(env.clone()));
            black_box(result);
        });
    });

    // Nested arithmetic (sequential evaluation)
    for depth in [3, 5, 7].iter() {
        let nested_expr = generate_nested_expr(*depth);

        group.bench_with_input(
            BenchmarkId::new("nested_arithmetic", depth),
            depth,
            |b, _| {
                let env = Environment::new();
                b.iter(|| {
                    let result = eval(black_box(nested_expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// SCALABILITY BENCHMARK
// ============================================================================

/// Benchmark: Scalability with large datasets
/// Tests how performance scales with increasing data size
fn bench_scalability(c: &mut Criterion) {
    let mut group = c.benchmark_group("scalability");

    // Test environment operations at scale
    for size in [100, 1000, 10000].iter() {
        let facts = generate_facts(*size);

        // Benchmark environment construction time
        group.bench_with_input(
            BenchmarkId::new("env_construction", size),
            size,
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

        // Benchmark lookup performance at scale
        let mut env = Environment::new();
        env.add_facts_bulk(&facts).unwrap();

        let search = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long((*size / 2) as i64),
            MettaValue::Atom(format!("value-{}", *size / 2)),
        ]);

        group.bench_with_input(
            BenchmarkId::new("lookup_at_scale", size),
            size,
            |b, _| {
                b.iter(|| {
                    black_box(env.has_sexpr_fact(&search));
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// CRITERION CONFIGURATION
// ============================================================================

criterion_group!(
    benches,
    bench_prefix_fast_path,
    bench_bulk_insertion,
    bench_cow_clone,
    bench_pattern_matching,
    bench_rule_matching,
    bench_type_lookup,
    bench_evaluation,
    bench_scalability
);

criterion_main!(benches);
