// PathMap Algebraic Operations Benchmark Suite
//
// Purpose: Comprehensive benchmarks for PathMap algebraic operations
// Reference: PATHMAP_ALGEBRAIC_OPERATIONS.md Section 5
//
// Usage:
//   1. Copy this file to benches/pathmap_algebraic.rs in your project
//   2. Add to Cargo.toml:
//      [[bench]]
//      name = "pathmap_algebraic"
//      harness = false
//   3. Add dependencies to Cargo.toml:
//      [dev-dependencies]
//      criterion = { version = "0.5", features = ["html_reports"] }
//      rand = "0.8"
//      rayon = "1.7"
//   4. Run benchmarks:
//      cargo bench --bench pathmap_algebraic
//
// Benchmark Groups:
//   - join_operations: Union/join benchmarks
//   - meet_operations: Intersection/meet benchmarks
//   - subtract_operations: Difference/subtract benchmarks
//   - restrict_operations: Prefix filtering benchmarks
//   - zipper_operations: Zipper-based operation benchmarks
//   - multi_way_join: Multi-way join performance
//   - value_combining: Value combining overhead
//   - identity_detection: Identity optimization verification
//   - memory_overhead: Memory usage measurements
//
// Example Results Analysis:
//   The benchmarks produce HTML reports in target/criterion/
//   Look for:
//   - O(n + m) complexity confirmation for binary operations
//   - O(k * n) complexity for multi-way joins
//   - Identity detection providing ~2x speedup
//   - Zipper operations showing localized overhead

#![allow(dead_code)]

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};
use pathmap::{PathMap, WriteZipper};
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::collections::HashMap;

// ============================================================================
// Test Data Generation
// ============================================================================

/// Generate a PathMap with sequential integer keys
fn generate_sequential_map(size: usize) -> PathMap<u64> {
    let mut map = PathMap::new();
    for i in 0..size {
        map.insert(&[i as u8], i as u64);
    }
    map
}

/// Generate a PathMap with random keys (fixed seed for reproducibility)
fn generate_random_map(size: usize, max_depth: usize, seed: u64) -> PathMap<u64> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut map = PathMap::new();

    for i in 0..size {
        let depth = rng.gen_range(1..=max_depth);
        let mut path = Vec::with_capacity(depth);
        for _ in 0..depth {
            path.push(rng.gen::<u8>());
        }
        map.insert(&path, i as u64);
    }

    map
}

/// Generate a PathMap with shared prefixes (clustered keys)
fn generate_clustered_map(size: usize, num_clusters: usize, cluster_depth: usize) -> PathMap<u64> {
    let mut rng = StdRng::seed_from_u64(12345);
    let mut map = PathMap::new();

    // Generate cluster prefixes
    let mut clusters = Vec::new();
    for _ in 0..num_clusters {
        let mut prefix = Vec::with_capacity(cluster_depth);
        for _ in 0..cluster_depth {
            prefix.push(rng.gen::<u8>());
        }
        clusters.push(prefix);
    }

    // Insert keys under clusters
    let per_cluster = size / num_clusters;
    for (cluster_idx, prefix) in clusters.iter().enumerate() {
        for i in 0..per_cluster {
            let mut path = prefix.clone();
            path.extend_from_slice(&(i as u32).to_le_bytes());
            map.insert(&path, (cluster_idx * per_cluster + i) as u64);
        }
    }

    map
}

/// Generate two maps with controlled overlap percentage
fn generate_overlapping_maps(size: usize, overlap_pct: usize) -> (PathMap<u64>, PathMap<u64>) {
    let overlap_count = (size * overlap_pct) / 100;
    let unique_count = size - overlap_count;

    let mut map1 = PathMap::new();
    let mut map2 = PathMap::new();

    // Shared keys (0..overlap_count)
    for i in 0..overlap_count {
        let path = &(i as u32).to_le_bytes();
        map1.insert(path, i as u64);
        map2.insert(path, (i + 1000) as u64); // Different values
    }

    // Unique keys for map1 (overlap_count..size)
    for i in overlap_count..size {
        map1.insert(&(i as u32).to_le_bytes(), i as u64);
    }

    // Unique keys for map2 (size..(size + unique_count))
    for i in 0..unique_count {
        map2.insert(&((size + i) as u32).to_le_bytes(), (size + i) as u64);
    }

    (map1, map2)
}

/// Generate PathMap with HashMap values for testing collection combining
fn generate_map_with_hashmap_values(size: usize) -> PathMap<HashMap<String, u64>> {
    let mut map = PathMap::new();

    for i in 0..size {
        let mut inner = HashMap::new();
        inner.insert(format!("key_{}", i), i as u64);
        inner.insert(format!("count_{}", i), (i * 2) as u64);
        map.insert(&(i as u32).to_le_bytes(), inner);
    }

    map
}

// ============================================================================
// Join Operation Benchmarks
// ============================================================================

fn bench_join_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("join_operations");

    // Test various sizes
    for size in [100, 1_000, 10_000, 100_000] {
        let map1 = generate_sequential_map(size);
        let map2 = generate_sequential_map(size);

        group.throughput(Throughput::Elements((size * 2) as u64));
        group.bench_with_input(
            BenchmarkId::new("join_disjoint", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let result = black_box(&map1).join(black_box(&map2));
                    black_box(result);
                });
            },
        );
    }

    // Test various overlap percentages
    for overlap_pct in [0, 25, 50, 75, 100] {
        let (map1, map2) = generate_overlapping_maps(10_000, overlap_pct);

        group.bench_with_input(
            BenchmarkId::new("join_overlap", overlap_pct),
            &overlap_pct,
            |b, _| {
                b.iter(|| {
                    let result = black_box(&map1).join(black_box(&map2));
                    black_box(result);
                });
            },
        );
    }

    // Test identity detection optimization
    let map1 = generate_sequential_map(10_000);
    let map2 = map1.clone();

    group.bench_function("join_identity_same_instance", |b| {
        b.iter(|| {
            let result = black_box(&map1).join(black_box(&map1));
            black_box(result);
        });
    });

    group.bench_function("join_identity_different_instance", |b| {
        b.iter(|| {
            let result = black_box(&map1).join(black_box(&map2));
            black_box(result);
        });
    });

    // Test clustered vs random structure
    let clustered = generate_clustered_map(10_000, 10, 4);
    let random = generate_random_map(10_000, 8, 54321);

    group.bench_function("join_clustered", |b| {
        b.iter(|| {
            let result = black_box(&clustered).join(black_box(&clustered));
            black_box(result);
        });
    });

    group.bench_function("join_random", |b| {
        b.iter(|| {
            let result = black_box(&random).join(black_box(&random));
            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Meet Operation Benchmarks
// ============================================================================

fn bench_meet_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("meet_operations");

    // Test various overlap percentages (meet only includes shared keys)
    for overlap_pct in [0, 25, 50, 75, 100] {
        let (map1, map2) = generate_overlapping_maps(10_000, overlap_pct);

        group.bench_with_input(
            BenchmarkId::new("meet_overlap", overlap_pct),
            &overlap_pct,
            |b, _| {
                b.iter(|| {
                    let result = black_box(&map1).meet(black_box(&map2));
                    black_box(result);
                });
            },
        );
    }

    // Test empty result case (disjoint maps)
    let map1 = generate_sequential_map(10_000);
    let mut map2 = PathMap::new();
    for i in 10_000..20_000 {
        map2.insert(&(i as u32).to_le_bytes(), i as u64);
    }

    group.bench_function("meet_disjoint_empty_result", |b| {
        b.iter(|| {
            let result = black_box(&map1).meet(black_box(&map2));
            black_box(result);
        });
    });

    // Test complete overlap (identity case)
    let map1 = generate_sequential_map(10_000);
    let map2 = map1.clone();

    group.bench_function("meet_complete_overlap", |b| {
        b.iter(|| {
            let result = black_box(&map1).meet(black_box(&map2));
            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Subtract Operation Benchmarks
// ============================================================================

fn bench_subtract_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("subtract_operations");

    // Test various overlap percentages
    for overlap_pct in [0, 25, 50, 75, 100] {
        let (map1, map2) = generate_overlapping_maps(10_000, overlap_pct);

        group.bench_with_input(
            BenchmarkId::new("subtract_overlap", overlap_pct),
            &overlap_pct,
            |b, _| {
                b.iter(|| {
                    let result = black_box(&map1).subtract(black_box(&map2));
                    black_box(result);
                });
            },
        );
    }

    // Test empty result case (complete overlap)
    let map1 = generate_sequential_map(10_000);
    let map2 = map1.clone();

    group.bench_function("subtract_complete_overlap_empty", |b| {
        b.iter(|| {
            let result = black_box(&map1).subtract(black_box(&map2));
            black_box(result);
        });
    });

    // Test no overlap (identity result)
    let map1 = generate_sequential_map(10_000);
    let mut map2 = PathMap::new();
    for i in 10_000..20_000 {
        map2.insert(&(i as u32).to_le_bytes(), i as u64);
    }

    group.bench_function("subtract_no_overlap_identity", |b| {
        b.iter(|| {
            let result = black_box(&map1).subtract(black_box(&map2));
            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Restrict Operation Benchmarks
// ============================================================================

fn bench_restrict_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("restrict_operations");

    let map = generate_clustered_map(100_000, 100, 4);

    // Test various prefix depths
    for depth in [1, 2, 4, 8] {
        let mut rng = StdRng::seed_from_u64(99999);
        let mut prefix = Vec::with_capacity(depth);
        for _ in 0..depth {
            prefix.push(rng.gen::<u8>());
        }

        group.bench_with_input(
            BenchmarkId::new("restrict_depth", depth),
            &depth,
            |b, _| {
                b.iter(|| {
                    let result = black_box(&map).restrict(black_box(&prefix));
                    black_box(result);
                });
            },
        );
    }

    // Test empty result (non-existent prefix)
    let nonexistent_prefix = vec![255, 255, 255, 255];
    group.bench_function("restrict_empty_result", |b| {
        b.iter(|| {
            let result = black_box(&map).restrict(black_box(&nonexistent_prefix));
            black_box(result);
        });
    });

    // Test root prefix (identity result)
    let empty_prefix: Vec<u8> = vec![];
    group.bench_function("restrict_root_identity", |b| {
        b.iter(|| {
            let result = black_box(&map).restrict(black_box(&empty_prefix));
            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Zipper Operation Benchmarks
// ============================================================================

fn bench_zipper_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("zipper_operations");

    // Compare whole-map vs zipper-based join
    let map1 = generate_sequential_map(10_000);
    let map2 = generate_sequential_map(10_000);

    group.bench_function("join_whole_map", |b| {
        b.iter(|| {
            let result = black_box(&map1).join(black_box(&map2));
            black_box(result);
        });
    });

    group.bench_function("join_zipper_root", |b| {
        b.iter(|| {
            let mut map1_clone = black_box(&map1).clone();
            let mut zipper = WriteZipper::from_root(&mut map1_clone);
            zipper.join_mut(black_box(&map2));
            black_box(map1_clone);
        });
    });

    // Test localized zipper operations
    let map = generate_clustered_map(100_000, 100, 4);
    let small_update = generate_sequential_map(100);

    group.bench_function("join_zipper_localized", |b| {
        b.iter(|| {
            let mut map_clone = black_box(&map).clone();
            let prefix = vec![0, 1, 2];
            if let Ok(mut zipper) = WriteZipper::from_root(&mut map_clone).descend(&prefix) {
                zipper.join_mut(black_box(&small_update));
            }
            black_box(map_clone);
        });
    });

    group.finish();
}

// ============================================================================
// Multi-Way Join Benchmarks
// ============================================================================

fn bench_multi_way_join(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_way_join");

    // Test various numbers of maps to join
    for num_maps in [2, 4, 8, 16, 32] {
        let maps: Vec<PathMap<u64>> = (0..num_maps)
            .map(|i| generate_random_map(1_000, 6, i as u64))
            .collect();

        group.throughput(Throughput::Elements((num_maps * 1_000) as u64));
        group.bench_with_input(
            BenchmarkId::new("multi_join", num_maps),
            &num_maps,
            |b, _| {
                b.iter(|| {
                    let mut result = black_box(&maps[0]).clone();
                    for map in &maps[1..] {
                        result = result.join(black_box(map));
                    }
                    black_box(result);
                });
            },
        );
    }

    // Compare sequential vs parallel multi-way join
    let maps: Vec<PathMap<u64>> = (0..16)
        .map(|i| generate_random_map(5_000, 6, i as u64))
        .collect();

    group.bench_function("multi_join_sequential", |b| {
        b.iter(|| {
            let mut result = black_box(&maps[0]).clone();
            for map in &maps[1..] {
                result = result.join(black_box(map));
            }
            black_box(result);
        });
    });

    group.bench_function("multi_join_parallel_reduce", |b| {
        b.iter(|| {
            use rayon::prelude::*;

            let result = black_box(&maps)
                .par_iter()
                .cloned()
                .reduce(|| PathMap::new(), |acc, map| acc.join(&map));

            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Value Combining Benchmarks
// ============================================================================

fn bench_value_combining(c: &mut Criterion) {
    let mut group = c.benchmark_group("value_combining");

    // Primitive values (no combining overhead)
    let map1: PathMap<u64> = generate_sequential_map(10_000);
    let map2: PathMap<u64> = generate_sequential_map(10_000);

    group.bench_function("join_u64_values", |b| {
        b.iter(|| {
            let result = black_box(&map1).join(black_box(&map2));
            black_box(result);
        });
    });

    // HashMap values (combining overhead)
    let map1_hm = generate_map_with_hashmap_values(10_000);
    let map2_hm = generate_map_with_hashmap_values(10_000);

    group.bench_function("join_hashmap_values", |b| {
        b.iter(|| {
            let result = black_box(&map1_hm).join(black_box(&map2_hm));
            black_box(result);
        });
    });

    // Option values with Some/None mixing
    let mut map1_opt: PathMap<Option<u64>> = PathMap::new();
    let mut map2_opt: PathMap<Option<u64>> = PathMap::new();
    for i in 0..10_000 {
        let val = if i % 2 == 0 { Some(i as u64) } else { None };
        map1_opt.insert(&(i as u32).to_le_bytes(), val);
        map2_opt.insert(&(i as u32).to_le_bytes(), val);
    }

    group.bench_function("join_option_values", |b| {
        b.iter(|| {
            let result = black_box(&map1_opt).join(black_box(&map2_opt));
            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Identity Detection Benchmarks
// ============================================================================

fn bench_identity_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("identity_detection");

    let map = generate_sequential_map(10_000);
    let clone = map.clone();

    // Self-join (same instance, ptr_eq succeeds immediately)
    group.bench_function("identity_self_join", |b| {
        b.iter(|| {
            let result = black_box(&map).join(black_box(&map));
            black_box(result);
        });
    });

    // Clone join (different instance, structural sharing allows ptr_eq on subtries)
    group.bench_function("identity_clone_join", |b| {
        b.iter(|| {
            let result = black_box(&map).join(black_box(&clone));
            black_box(result);
        });
    });

    // Modified clone (no identity detection possible)
    let mut modified_clone = map.clone();
    modified_clone.insert(&[255], 99999);

    group.bench_function("no_identity_modified_join", |b| {
        b.iter(|| {
            let result = black_box(&map).join(black_box(&modified_clone));
            black_box(result);
        });
    });

    group.finish();
}

// ============================================================================
// Memory Overhead Benchmarks
// ============================================================================

#[cfg(feature = "jemalloc")]
fn bench_memory_overhead(c: &mut Criterion) {
    use tikv_jemalloc_ctl::{epoch, stats};

    let mut group = c.benchmark_group("memory_overhead");

    // Measure memory for join operation
    group.bench_function("memory_join_overhead", |b| {
        b.iter_custom(|iters| {
            let map1 = generate_sequential_map(10_000);
            let map2 = generate_sequential_map(10_000);

            // Advance epoch and get baseline
            let _ = epoch::mib().map(|e| e.advance());
            let allocated_before = stats::allocated::read().unwrap_or(0);

            let start = std::time::Instant::now();
            for _ in 0..iters {
                let result = black_box(&map1).join(black_box(&map2));
                black_box(result);
            }
            let elapsed = start.elapsed();

            // Measure allocation delta
            let _ = epoch::mib().map(|e| e.advance());
            let allocated_after = stats::allocated::read().unwrap_or(0);
            let delta = allocated_after.saturating_sub(allocated_before);

            println!("Join allocated: {} bytes per iteration", delta / iters as usize);

            elapsed
        });
    });

    // Measure memory for structural sharing
    group.bench_function("memory_clone_sharing", |b| {
        b.iter_custom(|iters| {
            let map = generate_sequential_map(10_000);

            let _ = epoch::mib().map(|e| e.advance());
            let allocated_before = stats::allocated::read().unwrap_or(0);

            let start = std::time::Instant::now();
            for _ in 0..iters {
                let clone = black_box(&map).clone();
                black_box(clone);
            }
            let elapsed = start.elapsed();

            let _ = epoch::mib().map(|e| e.advance());
            let allocated_after = stats::allocated::read().unwrap_or(0);
            let delta = allocated_after.saturating_sub(allocated_before);

            println!("Clone allocated: {} bytes per iteration", delta / iters as usize);

            elapsed
        });
    });

    group.finish();
}

#[cfg(not(feature = "jemalloc"))]
fn bench_memory_overhead(_c: &mut Criterion) {
    println!("Memory overhead benchmarks require jemalloc feature");
}

// ============================================================================
// Criterion Configuration
// ============================================================================

criterion_group!(
    benches,
    bench_join_operations,
    bench_meet_operations,
    bench_subtract_operations,
    bench_restrict_operations,
    bench_zipper_operations,
    bench_multi_way_join,
    bench_value_combining,
    bench_identity_detection,
    bench_memory_overhead,
);

criterion_main!(benches);

// ============================================================================
// Example Analysis Commands
// ============================================================================
//
// After running benchmarks, analyze results with:
//
// 1. View HTML reports:
//    open target/criterion/report/index.html
//
// 2. Compare runs:
//    cargo bench --bench pathmap_algebraic -- --save-baseline baseline1
//    # Make changes...
//    cargo bench --bench pathmap_algebraic -- --baseline baseline1
//
// 3. Focus on specific group:
//    cargo bench --bench pathmap_algebraic join_operations
//
// 4. Export results:
//    cargo bench --bench pathmap_algebraic -- --output-format bencher | tee results.txt
//
// 5. Profile with flamegraph:
//    cargo flamegraph --bench pathmap_algebraic -- --bench
//
// Expected Results:
//   - Join/Meet/Subtract: ~O(n + m) scaling confirmed
//   - Restrict: ~O(|prefix|) localized overhead
//   - Multi-way join: ~O(k * n) for k maps
//   - Identity detection: ~2x speedup for clone joins
//   - Zipper operations: Lower overhead for localized updates
//   - Value combining: HashMap > Option > primitive overhead
