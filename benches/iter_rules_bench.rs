// Benchmark: iter_rules() and rule_count() Performance
//
// Establishes baseline metrics for rule iteration and counting operations.
// Part of performance recovery analysis after SIGABRT bug fix (commit 49fc002).
//
// Run with CPU affinity (per CLAUDE.md):
// taskset -c 0-17 cargo bench --bench iter_rules_bench

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mettatron::backend::models::MettaValue;
use mettatron::backend::{MettaEnvironment, MettaValueTrait};


// ============================================================================
// Helper Functions
// ============================================================================

/// Create a test rule for benchmarking with varying structure
fn make_test_rule(pattern: &str, body: &str) -> (MettaValue, MettaValue) {
    (
        MettaValue::sym(pattern),
        MettaValue::sym(body),
    )
}

/// Create a rule with S-expression structure (more realistic)
fn make_sexpr_rule(head: &str, idx: usize) -> (MettaValue, MettaValue) {
    (
        MettaValue::SExpr(vec![
            MettaValue::sym(head),
            MettaValue::sym(&format!("arg{}", idx)),
            MettaValue::var(&format!("x{}", idx)),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("result"),
            MettaValue::var(&format!("x{}", idx)),
        ]),
    )
}

/// Populate environment with n simple rules
fn populate_environment(n: usize) -> MettaEnvironment {
    let mut env = MettaEnvironment::default();
    for i in 0..n {
        let (lhs, rhs) = make_test_rule(&format!("(rule{} $x)", i), &format!("(result{} $x)", i));
        env.add_rule(lhs, rhs);
    }
    env
}

/// Populate environment with n S-expression rules (more realistic)
fn populate_environment_sexpr(n: usize, num_heads: usize) -> MettaEnvironment {
    let mut env = MettaEnvironment::default();
    let heads: Vec<String> = (0..num_heads).map(|i| format!("head{}", i)).collect();
    for i in 0..n {
        let head = &heads[i % num_heads];
        let (lhs, rhs) = make_sexpr_rule(head, i);
        env.add_rule(lhs, rhs);
    }
    env
}

// ============================================================================
// Benchmark 1: rule_count() Performance
// ============================================================================

fn bench_rule_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_count");

    for size in [100, 1000, 10000].iter() {
        let env = populate_environment(*size);

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("simple_rules", size), size, |b, _| {
            b.iter(|| {
                let count = black_box(&env).rule_count();
                black_box(count)
            })
        });
    }

    // Also test with varying head counts (affects rule_index size)
    for (rules, heads) in [(1000, 10), (1000, 100), (1000, 500)].iter() {
        let env = populate_environment_sexpr(*rules, *heads);

        group.bench_with_input(
            BenchmarkId::new(format!("{}_heads", heads), rules),
            rules,
            |b, _| {
                b.iter(|| {
                    let count = black_box(&env).rule_count();
                    black_box(count)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Benchmark 2: iter_rules() Performance
// ============================================================================

fn bench_collect_rules(c: &mut Criterion) {
    let mut group = c.benchmark_group("collect_rules");

    for size in [100, 1000, 10000].iter() {
        let env = populate_environment(*size);

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("simple_rules", size), size, |b, _| {
            b.iter(|| {
                let rules = black_box(&env).collect_rules();
                black_box(rules)
            })
        });
    }

    // Test with S-expression rules (more realistic MORK conversion)
    for size in [100, 1000].iter() {
        let env = populate_environment_sexpr(*size, 50);

        group.bench_with_input(BenchmarkId::new("sexpr_rules", size), size, |b, _| {
            b.iter(|| {
                let rules = black_box(&env).collect_rules();
                black_box(rules)
            })
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 3: collect_rules().len() vs rule_count() Comparison
// ============================================================================

fn bench_count_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("count_comparison");

    let env = populate_environment(1000);

    // rule_count() - O(k) via rule_index
    group.bench_function("rule_count_direct", |b| {
        b.iter(|| {
            let count = black_box(&env).rule_count();
            black_box(count)
        })
    });

    // collect_rules().len() - O(n) via PathMap iteration
    group.bench_function("collect_rules_len", |b| {
        b.iter(|| {
            let count = black_box(&env).collect_rules().len();
            black_box(count)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 4: collect_rules() Allocation Pressure
// ============================================================================

fn bench_collect_rules_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("collect_rules_allocation");

    // Test full collection (measures allocation overhead)
    let env = populate_environment(1000);

    group.bench_function("collect_all", |b| {
        b.iter(|| {
            let rules = black_box(&env).collect_rules();
            let count = rules.len();
            black_box(count)
        })
    });

    // Test with truncation after collection (measures lazy evaluation benefits)
    group.bench_function("collect_first_10", |b| {
        b.iter(|| {
            let rules: Vec<_> = black_box(&env).collect_rules().into_iter().take(10).collect();
            black_box(rules)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 5: iter_rule_heads() Performance (Lazy Iterator)
// ============================================================================

fn bench_iter_rule_heads(c: &mut Criterion) {
    let mut group = c.benchmark_group("iter_rule_heads");

    // Compare iter_rule_heads() (lazy) vs iter_rules() for getting head info
    for size in [100, 1000, 10000].iter() {
        let env = populate_environment(*size);

        group.throughput(Throughput::Elements(*size as u64));

        // New O(k) lazy method - collects to Vec for fair comparison
        group.bench_with_input(BenchmarkId::new("heads_only", size), size, |b, _| {
            b.iter(|| {
                let heads: Vec<_> = black_box(&env).iter_rule_heads().collect();
                black_box(heads)
            })
        });

        // Old O(n) method (for comparison)
        group.bench_with_input(BenchmarkId::new("via_collect_rules", size), size, |b, _| {
            b.iter(|| {
                let heads: Vec<_> = black_box(&env)
                    .collect_rules()
                    .iter()
                    .map(|(lhs, _rhs)| lhs.get_head_symbol().map(|s: &str| s.to_string()))
                    .collect();
                black_box(heads)
            })
        });
    }

    // Test with varying head counts
    for (rules, heads) in [(1000, 10), (1000, 100), (1000, 500)].iter() {
        let env = populate_environment_sexpr(*rules, *heads);

        group.bench_with_input(
            BenchmarkId::new(format!("{}_distinct_heads", heads), rules),
            rules,
            |b, _| {
                b.iter(|| {
                    let heads: Vec<_> = black_box(&env).iter_rule_heads().collect();
                    black_box(heads)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Benchmark 6: Concurrent iter_rules() Access
// ============================================================================

fn bench_concurrent_collect(c: &mut Criterion) {
    use std::sync::Arc;
    use std::thread;

    let mut group = c.benchmark_group("concurrent_collect_rules");

    let env = Arc::new(populate_environment(1000));

    // Sequential collection
    group.bench_function("sequential", |b| {
        b.iter(|| {
            let rules = env.collect_rules();
            black_box(rules)
        })
    });

    // 4 threads collecting concurrently
    group.bench_function("4_threads", |b| {
        b.iter(|| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let env = Arc::clone(&env);
                    thread::spawn(move || {
                        let rules = env.collect_rules();
                        black_box(rules.len())
                    })
                })
                .collect();

            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            black_box(results)
        })
    });

    group.finish();
}

// ============================================================================
// Criterion Groups
// ============================================================================

criterion_group!(
    name = rule_count_benches;
    config = Criterion::default().sample_size(100);
    targets = bench_rule_count
);

criterion_group!(
    name = collect_rules_benches;
    config = Criterion::default().sample_size(50);
    targets = bench_collect_rules
);

criterion_group!(
    name = comparison_benches;
    config = Criterion::default().sample_size(100);
    targets = bench_count_comparison
);

criterion_group!(
    name = allocation_benches;
    config = Criterion::default().sample_size(50);
    targets = bench_collect_rules_allocation
);

criterion_group!(
    name = concurrent_benches;
    config = Criterion::default().sample_size(30);
    targets = bench_concurrent_collect
);

criterion_group!(
    name = iter_rule_heads_benches;
    config = Criterion::default().sample_size(50);
    targets = bench_iter_rule_heads
);

criterion_main!(
    rule_count_benches,
    collect_rules_benches,
    iter_rule_heads_benches,
    comparison_benches,
    allocation_benches,
    concurrent_benches,
);

// ============================================================================
// Usage Instructions
// ============================================================================

/*
## Running Benchmarks

Basic usage:
```bash
cargo bench --bench iter_rules_bench
```

With CPU affinity (recommended per CLAUDE.md):
```bash
taskset -c 0-17 cargo bench --bench iter_rules_bench
```

Run specific benchmark group:
```bash
cargo bench --bench iter_rules_bench rule_count
cargo bench --bench iter_rules_bench iter_rules
cargo bench --bench iter_rules_bench comparison
```

Save baseline for comparison:
```bash
cargo bench --bench iter_rules_bench -- --save-baseline before_optimization
```

Compare with baseline:
```bash
cargo bench --bench iter_rules_bench -- --baseline before_optimization
```

## Expected Metrics

After SIGABRT fix (commit 49fc002):
- rule_count(): O(k) where k = distinct (head, arity) pairs
- iter_rules(): O(n) with 1 Vec<u8> allocation per rule

Key comparisons:
- rule_count_direct vs iter_rules_count: Shows speedup from avoiding PathMap iteration
- iterate_only vs iterate_first_10: Shows lazy evaluation potential
*/
