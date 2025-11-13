// Copy-on-Write Environment Benchmarks
//
// Measures performance impact of CoW implementation on Environment operations
// Reference: docs/design/COW_PHASE1A_COMPLETE.md Section "Performance Validation Plan"
//
// Run with CPU affinity (per CLAUDE.md):
// taskset -c 0-17 cargo bench --bench cow_environment

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use mettatron::backend::Environment;
use mettatron::backend::models::{Rule, MettaValue};
use std::sync::Arc as StdArc;

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a test rule for benchmarking
fn make_test_rule(pattern: &str, body: &str) -> Rule {
    Rule {
        lhs: MettaValue::Atom(pattern.to_string()),
        rhs: MettaValue::Atom(body.to_string()),
    }
}

/// Populate environment with n rules
fn populate_environment(n: usize) -> Environment {
    let mut env = Environment::new();
    for i in 0..n {
        let rule = make_test_rule(&format!("(rule{} $x)", i), &format!("(result{} $x)", i));
        env.add_rule(rule);
    }
    env
}

// ============================================================================
// Benchmark 1: Clone Performance (Target: < 50ns)
// ============================================================================

fn bench_clone_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("clone_cost");

    // Empty environment
    let empty = Environment::new();
    group.bench_function("empty", |b| {
        b.iter(|| {
            let clone = black_box(&empty).clone();
            black_box(clone)
        })
    });

    // Small environment (10 rules)
    let small = populate_environment(10);
    group.bench_function("small_10_rules", |b| {
        b.iter(|| {
            let clone = black_box(&small).clone();
            black_box(clone)
        })
    });

    // Medium environment (100 rules)
    let medium = populate_environment(100);
    group.bench_function("medium_100_rules", |b| {
        b.iter(|| {
            let clone = black_box(&medium).clone();
            black_box(clone)
        })
    });

    // Large environment (1000 rules)
    let large = populate_environment(1000);
    group.bench_function("large_1000_rules", |b| {
        b.iter(|| {
            let clone = black_box(&large).clone();
            black_box(clone)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 2: make_owned() Cost (Target: < 100µs for 1000 rules)
// ============================================================================

fn bench_make_owned_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("make_owned_cost");

    for size in [10, 100, 1000].iter() {
        let base = populate_environment(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter_batched(
                || base.clone(),  // Setup: create shared clone
                |mut clone| {
                    // First mutation triggers make_owned()
                    let rule = make_test_rule("(trigger $x)", "(owned $x)");
                    clone.add_rule(rule);
                    black_box(clone)
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 3: Concurrent Reads (Target: 4× improvement vs Mutex)
// ============================================================================

fn bench_concurrent_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_reads");

    let env = StdArc::new(populate_environment(1000));

    // Sequential reads (baseline)
    group.bench_function("sequential_1000_reads", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let count = black_box(&env).rule_count();
                black_box(count);
            }
        })
    });

    // Parallel reads (4 threads)
    group.bench_function("parallel_4_threads_250_reads_each", |b| {
        use std::thread;

        b.iter(|| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let env = StdArc::clone(&env);
                    thread::spawn(move || {
                        for _ in 0..250 {
                            let count = env.rule_count();
                            black_box(count);
                        }
                    })
                })
                .collect();

            for handle in handles {
                handle.join().unwrap();
            }
        })
    });

    // Parallel reads (8 threads)
    group.bench_function("parallel_8_threads_125_reads_each", |b| {
        use std::thread;

        b.iter(|| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let env = StdArc::clone(&env);
                    thread::spawn(move || {
                        for _ in 0..125 {
                            let count = env.rule_count();
                            black_box(count);
                        }
                    })
                })
                .collect();

            for handle in handles {
                handle.join().unwrap();
            }
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 4: Multiple Clones with Mutations
// ============================================================================

fn bench_multi_clone_mutate(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_clone_mutate");

    let base = populate_environment(100);

    // 10 clones, 10 mutations each
    group.bench_function("10_clones_10_mutations", |b| {
        b.iter(|| {
            let clones: Vec<_> = (0..10)
                .map(|i| {
                    let mut clone = base.clone();
                    for j in 0..10 {
                        let rule = make_test_rule(
                            &format!("(clone{}_rule{} $x)", i, j),
                            &format!("(result{} $x)", j),
                        );
                        clone.add_rule(rule);
                    }
                    clone
                })
                .collect();
            black_box(clones)
        })
    });

    // 100 clones, 1 mutation each
    group.bench_function("100_clones_1_mutation", |b| {
        b.iter(|| {
            let clones: Vec<_> = (0..100)
                .map(|i| {
                    let mut clone = base.clone();
                    let rule = make_test_rule(&format!("(clone{} $x)", i), "(result $x)");
                    clone.add_rule(rule);
                    clone
                })
                .collect();
            black_box(clones)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 5: Read Performance (No COW Overhead)
// ============================================================================

fn bench_read_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_operations");

    let env = populate_environment(1000);
    let clone = env.clone();  // Shared clone

    // Read from original (exclusive)
    group.bench_function("rule_count_original", |b| {
        b.iter(|| {
            let count = black_box(&env).rule_count();
            black_box(count)
        })
    });

    // Read from clone (shared) - should be identical
    group.bench_function("rule_count_shared_clone", |b| {
        b.iter(|| {
            let count = black_box(&clone).rule_count();
            black_box(count)
        })
    });

    // Read after make_owned (exclusive again)
    let mut mutated_clone = env.clone();
    mutated_clone.add_rule(make_test_rule("(trigger $x)", "(owned $x)"));

    group.bench_function("rule_count_after_make_owned", |b| {
        b.iter(|| {
            let count = black_box(&mutated_clone).rule_count();
            black_box(count)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 6: Overall Regression Test (Target: < 1% vs pre-CoW)
// ============================================================================

fn bench_typical_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("typical_workload");

    // Typical pattern: create env, add rules, clone, mutate clone
    group.bench_function("create_add_clone_mutate", |b| {
        b.iter(|| {
            // Create and populate
            let mut env = Environment::new();
            for i in 0..50 {
                let rule = make_test_rule(&format!("(rule{} $x)", i), "(result $x)");
                env.add_rule(rule);
            }

            // Clone for parallel evaluation
            let mut clone = env.clone();

            // Mutate clone
            for i in 0..10 {
                let rule = make_test_rule(&format!("(dynamic{} $x)", i), "(result $x)");
                clone.add_rule(rule);
            }

            // Read from both
            let env_count = env.rule_count();
            let clone_count = clone.rule_count();

            black_box((env, clone, env_count, clone_count))
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark Groups
// ============================================================================

criterion_group!(
    clone_benches,
    bench_clone_cost,
);

criterion_group!(
    make_owned_benches,
    bench_make_owned_cost,
);

criterion_group!(
    concurrent_benches,
    bench_concurrent_reads,
);

criterion_group!(
    multi_clone_benches,
    bench_multi_clone_mutate,
);

criterion_group!(
    read_benches,
    bench_read_operations,
);

criterion_group!(
    workload_benches,
    bench_typical_workload,
);

criterion_main!(
    clone_benches,
    make_owned_benches,
    concurrent_benches,
    multi_clone_benches,
    read_benches,
    workload_benches,
);

// ============================================================================
// Usage Instructions
// ============================================================================

/*
## Running Benchmarks

Basic usage:
```bash
cargo bench --bench cow_environment
```

With CPU affinity (recommended per CLAUDE.md):
```bash
taskset -c 0-17 cargo bench --bench cow_environment
```

With performance governor (maximum CPU frequency):
```bash
sudo cpupower frequency-set -g performance
taskset -c 0-17 cargo bench --bench cow_environment
sudo cpupower frequency-set -g powersave  # Restore after
```

Run specific benchmark group:
```bash
cargo bench --bench cow_environment clone
cargo bench --bench cow_environment make_owned
cargo bench --bench cow_environment concurrent
```

Save baseline for comparison:
```bash
cargo bench --bench cow_environment -- --save-baseline before_cow
```

Compare with baseline:
```bash
cargo bench --bench cow_environment -- --baseline before_cow
```

## Expected Results (from COW_PHASE1A_COMPLETE.md)

| Benchmark | Target | Notes |
|-----------|--------|-------|
| Clone cost | < 50 ns | O(1) Arc increment |
| make_owned() | < 100 µs | One-time deep copy (1000 rules) |
| Concurrent reads | 4× improvement | RwLock vs Mutex |
| Overall regression | < 1% | Read-heavy workload |

## Interpreting Results

Example output:
```
clone_cost/large_1000_rules
                        time:   [42.3 ns 43.1 ns 44.0 ns]
                        change: [-1.2% +0.3% +1.9%] (no change)

make_owned_cost/1000    time:   [87.4 µs 89.2 µs 91.3 µs]
                        change: [+2.1% +3.8% +5.6%] (within target)

concurrent_reads/parallel_4_threads_250_reads_each
                        time:   [1.23 ms 1.25 ms 1.27 ms]
                        change: [-76.2% -75.8% -75.3%] (4.1× speedup!)
```

Key metrics:
- **Time**: Median ± confidence interval (95%)
- **Change**: Percentage difference from baseline
- **Speedup**: Calculated from time ratio

See docs/design/COW_PHASE1A_COMPLETE.md for detailed performance targets.
*/
