// PathMap Copy-On-Write Benchmarks
//
// Comprehensive benchmark suite for measuring PathMap COW performance
// Reference: PATHMAP_COW_ANALYSIS.md Section 10
//
// To integrate into MeTTaTron:
// 1. Copy to benches/pathmap_cow.rs
// 2. Add to Cargo.toml:
//    [[bench]]
//    name = "pathmap_cow"
//    harness = false
// 3. Run with: cargo bench --bench pathmap_cow
//
// Hardware-specific setup (per CLAUDE.md):
// - Pin to single CPU: taskset -c 0 cargo bench
// - Max CPU frequency: sudo cpupower frequency-set -g performance

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use pathmap::PathMap;
use std::sync::Arc;

// ============================================================================
// Helper Functions
// ============================================================================

/// Generate PathMap with n keys of specified depth
fn generate_pathmap(n: usize, depth: usize) -> PathMap<u64> {
    (0..n)
        .map(|i| {
            // Generate key with specified depth (bytes)
            let key = format!("{:0width$}", i, width = depth);
            (key, i as u64)
        })
        .collect()
}

/// Generate deep clone (for comparison with COW)
fn deep_clone<V: Clone>(map: &PathMap<V>) -> PathMap<V> {
    map.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ============================================================================
// Benchmark 1: Clone Operations
// ============================================================================

fn bench_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("clone");

    // Empty PathMap
    let empty: PathMap<u64> = PathMap::new();
    group.bench_function("empty", |b| {
        b.iter(|| {
            let cloned = black_box(&empty).clone();
            black_box(cloned)
        })
    });

    // Small PathMap (100 keys)
    let small = generate_pathmap(100, 20);
    group.bench_function("small_100", |b| {
        b.iter(|| {
            let cloned = black_box(&small).clone();
            black_box(cloned)
        })
    });

    // Medium PathMap (1,000 keys)
    let medium = generate_pathmap(1000, 20);
    group.bench_function("medium_1k", |b| {
        b.iter(|| {
            let cloned = black_box(&medium).clone();
            black_box(cloned)
        })
    });

    // Large PathMap (10,000 keys)
    let large = generate_pathmap(10000, 20);
    group.bench_function("large_10k", |b| {
        b.iter(|| {
            let cloned = black_box(&large).clone();
            black_box(cloned)
        })
    });

    // Very Large PathMap (100,000 keys)
    let very_large = generate_pathmap(100000, 20);
    group.bench_function("very_large_100k", |b| {
        b.iter(|| {
            let cloned = black_box(&very_large).clone();
            black_box(cloned)
        })
    });

    group.finish();
}

fn bench_clone_vs_deep(c: &mut Criterion) {
    let mut group = c.benchmark_group("clone_vs_deep");

    for size in [100, 1000, 10000].iter() {
        let map = generate_pathmap(*size, 20);

        // COW clone
        group.bench_with_input(BenchmarkId::new("cow", size), &map, |b, m| {
            b.iter(|| {
                let cloned = black_box(m).clone();
                black_box(cloned)
            })
        });

        // Deep clone
        group.bench_with_input(BenchmarkId::new("deep", size), &map, |b, m| {
            b.iter(|| {
                let cloned = deep_clone(black_box(m));
                black_box(cloned)
            })
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 2: Mutation Operations
// ============================================================================

fn bench_insert_exclusive(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_exclusive");

    let base = generate_pathmap(1000, 20);

    group.bench_function("sole_owner", |b| {
        b.iter_batched(
            || base.clone(),  // Setup: clone base
            |mut map| {
                // Mutation: sole owner, no COW overhead
                map.insert("new_key".to_string(), 999);
                black_box(map)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_insert_shared(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_shared");

    let base = generate_pathmap(1000, 20);

    // First insert after clone (triggers COW)
    group.bench_function("first_after_clone", |b| {
        b.iter_batched(
            || {
                let clone1 = base.clone();
                let clone2 = clone1.clone();  // clone2 is shared
                clone2
            },
            |mut map| {
                // First mutation triggers path copy
                map.insert("new_key".to_string(), 999);
                black_box(map)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // Subsequent inserts (path now exclusive)
    group.bench_function("after_100_mutations", |b| {
        b.iter_batched(
            || {
                let clone1 = base.clone();
                let mut clone2 = clone1.clone();
                // Perform 100 mutations first (path now exclusive)
                for i in 0..100 {
                    clone2.insert(format!("key_{}", i), i);
                }
                clone2
            },
            |mut map| {
                // 101st mutation: no COW overhead
                map.insert("new_key".to_string(), 999);
                black_box(map)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

// ============================================================================
// Benchmark 3: Multiple Clones
// ============================================================================

fn bench_multi_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_clone");
    group.throughput(Throughput::Elements(100));

    let base = generate_pathmap(10000, 20);

    group.bench_function("100_clones", |b| {
        b.iter(|| {
            let clones: Vec<_> = (0..100)
                .map(|_| black_box(&base).clone())
                .collect();
            black_box(clones)
        })
    });

    group.finish();
}

fn bench_multi_clone_mutate(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_clone_mutate");

    let base = generate_pathmap(10000, 20);

    // 100 clones, 10 mutations each
    group.bench_function("100_clones_10_mutations", |b| {
        b.iter(|| {
            let clones: Vec<_> = (0..100)
                .map(|i| {
                    let mut clone = black_box(&base).clone();
                    // 10 mutations per clone
                    for j in 0..10 {
                        clone.insert(format!("clone_{}_key_{}", i, j), j as u64);
                    }
                    clone
                })
                .collect();
            black_box(clones)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 4: Read Operations (No COW Overhead)
// ============================================================================

fn bench_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("get");

    let base = generate_pathmap(10000, 20);
    let clone = base.clone();  // Shared

    // Get from original (exclusive)
    group.bench_function("from_original", |b| {
        b.iter(|| {
            let val = black_box(&base).get("5000");
            black_box(val)
        })
    });

    // Get from clone (shared) - should be identical performance
    group.bench_function("from_clone_shared", |b| {
        b.iter(|| {
            let val = black_box(&clone).get("5000");
            black_box(val)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 5: Algebraic Operations
// ============================================================================

fn bench_join(c: &mut Criterion) {
    let mut group = c.benchmark_group("join");

    // Two disjoint maps
    let map1 = generate_pathmap(5000, 20);
    let map2: PathMap<u64> = (5000..10000)
        .map(|i| (format!("{:020}", i), i as u64))
        .collect();

    group.bench_function("disjoint_5k_each", |b| {
        b.iter(|| {
            let joined = black_box(&map1).join(black_box(&map2));
            black_box(joined)
        })
    });

    // Two overlapping maps (50% overlap)
    let map3 = generate_pathmap(7500, 20);
    let map4: PathMap<u64> = (2500..10000)
        .map(|i| (format!("{:020}", i), i as u64))
        .collect();

    group.bench_function("overlap_50_percent", |b| {
        b.iter(|| {
            let joined = black_box(&map3).join(black_box(&map4));
            black_box(joined)
        })
    });

    // Identical maps (100% sharing)
    let map5 = generate_pathmap(10000, 20);
    let map6 = map5.clone();

    group.bench_function("identical_10k", |b| {
        b.iter(|| {
            let joined = black_box(&map5).join(black_box(&map6));
            black_box(joined)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 6: Snapshot Manager Pattern
// ============================================================================

mod snapshot_manager {
    use super::*;
    use std::collections::VecDeque;

    pub struct SnapshotManager<V> {
        current: PathMap<V>,
        history: VecDeque<PathMap<V>>,
        max_history: usize,
    }

    impl<V: Clone> SnapshotManager<V> {
        pub fn new(max_history: usize) -> Self {
            Self {
                current: PathMap::new(),
                history: VecDeque::new(),
                max_history,
            }
        }

        pub fn snapshot(&mut self) {
            self.history.push_back(self.current.clone());
            if self.history.len() > self.max_history {
                self.history.pop_front();
            }
        }

        pub fn undo(&mut self) -> bool {
            if let Some(snapshot) = self.history.pop_back() {
                self.current = snapshot;
                true
            } else {
                false
            }
        }

        pub fn insert(&mut self, key: String, value: V) {
            self.current.insert(key, value);
        }
    }
}

fn bench_snapshot_manager(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_manager");

    group.bench_function("1000_snapshots", |b| {
        b.iter(|| {
            let mut mgr = snapshot_manager::SnapshotManager::new(1000);

            // Perform 1000 operations with snapshots
            for i in 0..1000 {
                mgr.insert(format!("key_{}", i), i as u64);
                if i % 10 == 0 {
                    mgr.snapshot();
                }
            }

            black_box(mgr)
        })
    });

    group.bench_function("1000_snapshots_with_undo", |b| {
        b.iter(|| {
            let mut mgr = snapshot_manager::SnapshotManager::new(1000);

            // Perform 1000 operations with snapshots
            for i in 0..1000 {
                mgr.insert(format!("key_{}", i), i as u64);
                if i % 10 == 0 {
                    mgr.snapshot();
                }
            }

            // Undo 50 times
            for _ in 0..50 {
                mgr.undo();
            }

            black_box(mgr)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 7: Memory Usage (requires jemalloc feature)
// ============================================================================

#[cfg(feature = "jemalloc")]
mod memory_benches {
    use super::*;
    use tikv_jemalloc_ctl::{epoch, stats};

    pub fn measure_memory_usage(c: &mut Criterion) {
        let mut group = c.benchmark_group("memory_usage");

        // Baseline: single PathMap
        group.bench_function("single_10k", |b| {
            b.iter_custom(|iters| {
                let start = std::time::Instant::now();

                for _ in 0..iters {
                    epoch::mib().unwrap().advance().unwrap();
                    let before = stats::allocated::read().unwrap();

                    let map = generate_pathmap(10000, 20);
                    black_box(&map);

                    epoch::mib().unwrap().advance().unwrap();
                    let after = stats::allocated::read().unwrap();

                    eprintln!("Single map allocated: {} MB", (after - before) / 1_048_576);

                    drop(map);
                }

                start.elapsed()
            })
        });

        // 100 COW clones
        group.bench_function("100_clones_10k", |b| {
            b.iter_custom(|iters| {
                let start = std::time::Instant::now();

                for _ in 0..iters {
                    epoch::mib().unwrap().advance().unwrap();
                    let before = stats::allocated::read().unwrap();

                    let base = generate_pathmap(10000, 20);
                    let clones: Vec<_> = (0..100).map(|_| base.clone()).collect();
                    black_box(&clones);

                    epoch::mib().unwrap().advance().unwrap();
                    let after = stats::allocated::read().unwrap();

                    eprintln!("100 clones allocated: {} MB", (after - before) / 1_048_576);

                    drop(clones);
                    drop(base);
                }

                start.elapsed()
            })
        });

        // 100 clones with 10 mutations each
        group.bench_function("100_clones_10_mutations", |b| {
            b.iter_custom(|iters| {
                let start = std::time::Instant::now();

                for _ in 0..iters {
                    epoch::mib().unwrap().advance().unwrap();
                    let before = stats::allocated::read().unwrap();

                    let base = generate_pathmap(10000, 20);
                    let clones: Vec<_> = (0..100)
                        .map(|i| {
                            let mut clone = base.clone();
                            for j in 0..10 {
                                clone.insert(format!("clone_{}_key_{}", i, j), j as u64);
                            }
                            clone
                        })
                        .collect();
                    black_box(&clones);

                    epoch::mib().unwrap().advance().unwrap();
                    let after = stats::allocated::read().unwrap();

                    eprintln!(
                        "100 clones + 10 mutations allocated: {} MB",
                        (after - before) / 1_048_576
                    );

                    drop(clones);
                    drop(base);
                }

                start.elapsed()
            })
        });

        group.finish();
    }
}

// ============================================================================
// Benchmark Groups
// ============================================================================

criterion_group!(
    clone_benches,
    bench_clone,
    bench_clone_vs_deep,
);

criterion_group!(
    mutation_benches,
    bench_insert_exclusive,
    bench_insert_shared,
);

criterion_group!(
    multi_benches,
    bench_multi_clone,
    bench_multi_clone_mutate,
);

criterion_group!(
    read_benches,
    bench_get,
);

criterion_group!(
    algebraic_benches,
    bench_join,
);

criterion_group!(
    pattern_benches,
    bench_snapshot_manager,
);

#[cfg(feature = "jemalloc")]
criterion_group!(
    memory_benches,
    memory_benches::measure_memory_usage,
);

// Main entry point
#[cfg(not(feature = "jemalloc"))]
criterion_main!(
    clone_benches,
    mutation_benches,
    multi_benches,
    read_benches,
    algebraic_benches,
    pattern_benches,
);

#[cfg(feature = "jemalloc")]
criterion_main!(
    clone_benches,
    mutation_benches,
    multi_benches,
    read_benches,
    algebraic_benches,
    pattern_benches,
    memory_benches,
);

// ============================================================================
// Usage Instructions
// ============================================================================

/*
## Running Benchmarks

Basic usage:
```bash
cargo bench --bench pathmap_cow
```

With CPU affinity (recommended):
```bash
taskset -c 0 cargo bench --bench pathmap_cow
```

With performance governor (maximum CPU frequency):
```bash
sudo cpupower frequency-set -g performance
cargo bench --bench pathmap_cow
sudo cpupower frequency-set -g powersave  # Restore after
```

Run specific benchmark group:
```bash
cargo bench --bench pathmap_cow clone
cargo bench --bench pathmap_cow mutation
```

Save baseline for comparison:
```bash
cargo bench --bench pathmap_cow -- --save-baseline before_optimization
```

Compare with baseline:
```bash
cargo bench --bench pathmap_cow -- --baseline before_optimization
```

## Interpreting Results

Example output:
```
clone/large_10k         time:   [4.89 ns 5.02 ns 5.17 ns]
                        change: [-2.3% -0.9% +0.6%] (no significant change)

insert_shared/first_after_clone
                        time:   [1.95 µs 2.01 µs 2.08 µs]
                        change: [+18.2% +21.5% +24.9%] (regression detected!)
```

Key metrics:
- **Time**: Median ± confidence interval (95%)
- **Change**: Percentage difference from baseline
- **Throughput**: Operations per second (for some benchmarks)

## Expected Results

Based on theoretical analysis:

| Benchmark | Expected Time | Notes |
|-----------|---------------|-------|
| clone (any size) | ~5 ns | O(1) refcount increment |
| insert_exclusive | ~100 ns | No COW overhead |
| insert_shared_first | ~2 µs | Path copy (20× slower) |
| 100_clones | ~500 ns | 100 × 5 ns |
| join_identical | ~1 µs | Fast path (ptr equality) |

See PATHMAP_COW_ANALYSIS.md Section 7.3 for detailed analysis.
*/
