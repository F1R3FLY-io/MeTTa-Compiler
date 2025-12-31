//! Set Operations Benchmark: HashMap Implementation
//!
//! Benchmarks the performance of direct HashMap<MettaValue, usize> counting
//! for set operations with multiset semantics.
//!
//! Operations tested:
//! - intersection-atom: min(left count, right count)
//! - subtraction-atom: max(0, left count - right count)
//! - union-atom: concatenation (preserves all multiplicities)
//! - unique-atom: deduplication (preserves first occurrence order)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use mettatron::backend::MettaValue;
use std::collections::{HashMap, HashSet};

const SIZES: &[usize] = &[10, 100, 1_000, 10_000, 100_000];

// ============================================================================
// Data Generators
// ============================================================================

/// Generate atoms with unique suffixes
fn generate_atoms(n: usize, prefix: &str) -> Vec<MettaValue> {
    (0..n)
        .map(|i| MettaValue::Atom(format!("{}{}", prefix, i)))
        .collect()
}

/// Generate two lists with controlled overlap ratio
/// overlap=0.0 means disjoint sets
/// overlap=1.0 means identical sets
fn generate_with_overlap(n: usize, overlap: f64) -> (Vec<MettaValue>, Vec<MettaValue>) {
    let overlap_count = (n as f64 * overlap) as usize;
    let left = generate_atoms(n, "a");
    let mut right = left[n.saturating_sub(overlap_count)..].to_vec();
    right.extend(generate_atoms(n.saturating_sub(overlap_count), "b"));
    (left, right)
}

/// Generate list with duplicates (multiset) for testing multiplicity handling
fn generate_multiset(n: usize, max_multiplicity: usize) -> Vec<MettaValue> {
    let mut result = Vec::with_capacity(n);
    let unique_count = n / max_multiplicity.max(1);
    for i in 0..unique_count {
        let count = 1 + (i % max_multiplicity);
        for _ in 0..count {
            result.push(MettaValue::Atom(format!("elem{}", i)));
        }
    }
    result
}

// ============================================================================
// HashMap Implementation
// ============================================================================

/// HashMap-based intersection preserving left input order
fn intersection_hashmap(left: &[MettaValue], right: &[MettaValue]) -> Vec<MettaValue> {
    // Build count map from right list
    let mut right_counts: HashMap<&MettaValue, usize> = HashMap::with_capacity(right.len());
    for item in right {
        *right_counts.entry(item).or_default() += 1;
    }

    // Filter left list, consuming counts (preserves left input order)
    let mut result = Vec::with_capacity(left.len().min(right_counts.len()));
    for item in left {
        if let Some(count) = right_counts.get_mut(item) {
            if *count > 0 {
                *count -= 1;
                result.push(item.clone());
            }
        }
    }
    result
}

/// HashMap-based subtraction preserving left input order
fn subtraction_hashmap(left: &[MettaValue], right: &[MettaValue]) -> Vec<MettaValue> {
    // Build count map from right list
    let mut right_counts: HashMap<&MettaValue, usize> = HashMap::with_capacity(right.len());
    for item in right {
        *right_counts.entry(item).or_default() += 1;
    }

    // Filter left list, removing items in right (preserves left input order)
    let mut result = Vec::with_capacity(left.len());
    for item in left {
        if let Some(count) = right_counts.get_mut(item) {
            if *count > 0 {
                *count -= 1;
                continue;
            }
        }
        result.push(item.clone());
    }
    result
}

/// HashMap-based union (simple concatenation preserving order)
/// Note: This is multiset union - all elements from both lists are included
fn union_hashmap(left: &[MettaValue], right: &[MettaValue]) -> Vec<MettaValue> {
    let mut result = Vec::with_capacity(left.len() + right.len());
    result.extend(left.iter().cloned());
    result.extend(right.iter().cloned());
    result
}

/// HashMap-based unique (deduplication preserving first occurrence order)
fn unique_hashmap(items: &[MettaValue]) -> Vec<MettaValue> {
    let mut seen = HashSet::with_capacity(items.len());
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        if seen.insert(item.clone()) {
            result.push(item.clone());
        }
    }
    result
}

// ============================================================================
// Benchmarks: Scaling with Input Size
// ============================================================================

/// Benchmark intersection-atom scaling from 10 to 100K elements
fn bench_intersection_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("intersection_scaling");

    for &size in SIZES {
        group.throughput(Throughput::Elements(size as u64));

        // Generate test data with 50% overlap (average case)
        let (left_vec, right_vec) = generate_with_overlap(size, 0.5);

        group.bench_with_input(BenchmarkId::new("hashmap", size), &size, |b, _| {
            b.iter(|| black_box(intersection_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark subtraction-atom scaling from 10 to 100K elements
fn bench_subtraction_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("subtraction_scaling");

    for &size in SIZES {
        group.throughput(Throughput::Elements(size as u64));

        // Generate test data with 50% overlap (average case)
        let (left_vec, right_vec) = generate_with_overlap(size, 0.5);

        group.bench_with_input(BenchmarkId::new("hashmap", size), &size, |b, _| {
            b.iter(|| black_box(subtraction_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark union-atom scaling from 10 to 100K elements
fn bench_union_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("union_scaling");

    for &size in SIZES {
        group.throughput(Throughput::Elements(size as u64 * 2)); // Both lists contribute

        // Generate test data with 50% overlap
        let (left_vec, right_vec) = generate_with_overlap(size, 0.5);

        group.bench_with_input(BenchmarkId::new("hashmap", size), &size, |b, _| {
            b.iter(|| black_box(union_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark unique-atom scaling from 10 to 100K elements
fn bench_unique_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("unique_scaling");

    for &size in SIZES {
        group.throughput(Throughput::Elements(size as u64));

        // Generate test data with duplicates (50% unique)
        let items = generate_multiset(size, 2);

        group.bench_with_input(BenchmarkId::new("hashmap", size), &size, |b, _| {
            b.iter(|| black_box(unique_hashmap(&items)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmarks: Overlap Scenarios
// ============================================================================

/// Benchmark intersection with varying overlap percentages
fn bench_intersection_overlap(c: &mut Criterion) {
    let mut group = c.benchmark_group("intersection_overlap");
    let size = 10_000;

    for (name, overlap) in &[
        ("0%", 0.0),
        ("25%", 0.25),
        ("50%", 0.5),
        ("75%", 0.75),
        ("100%", 1.0),
    ] {
        let (left_vec, right_vec) = generate_with_overlap(size, *overlap);

        group.bench_function(BenchmarkId::new("hashmap", name), |b| {
            b.iter(|| black_box(intersection_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark subtraction with varying overlap percentages
fn bench_subtraction_overlap(c: &mut Criterion) {
    let mut group = c.benchmark_group("subtraction_overlap");
    let size = 10_000;

    for (name, overlap) in &[
        ("0%", 0.0),
        ("25%", 0.25),
        ("50%", 0.5),
        ("75%", 0.75),
        ("100%", 1.0),
    ] {
        let (left_vec, right_vec) = generate_with_overlap(size, *overlap);

        group.bench_function(BenchmarkId::new("hashmap", name), |b| {
            b.iter(|| black_box(subtraction_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark union with varying overlap percentages
fn bench_union_overlap(c: &mut Criterion) {
    let mut group = c.benchmark_group("union_overlap");
    let size = 10_000;

    for (name, overlap) in &[
        ("0%", 0.0),
        ("25%", 0.25),
        ("50%", 0.5),
        ("75%", 0.75),
        ("100%", 1.0),
    ] {
        let (left_vec, right_vec) = generate_with_overlap(size, *overlap);

        group.bench_function(BenchmarkId::new("hashmap", name), |b| {
            b.iter(|| black_box(union_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark unique with varying duplicate ratios
fn bench_unique_duplicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("unique_duplicates");
    let size = 10_000;

    for &multiplicity in &[1, 2, 5, 10, 50] {
        let items = generate_multiset(size, multiplicity);
        let unique_ratio = format!("{}x_dup", multiplicity);

        group.bench_function(BenchmarkId::new("hashmap", &unique_ratio), |b| {
            b.iter(|| black_box(unique_hashmap(&items)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmarks: High Multiplicity (Multiset Semantics)
// ============================================================================

/// Benchmark with high element multiplicity to test multiset counting
fn bench_high_multiplicity(c: &mut Criterion) {
    let mut group = c.benchmark_group("high_multiplicity");
    let size = 10_000;

    for &multiplicity in &[2, 5, 10, 50] {
        let left_vec = generate_multiset(size, multiplicity);
        let right_vec = generate_multiset(size / 2, multiplicity);

        group.bench_function(BenchmarkId::new("intersection", multiplicity), |b| {
            b.iter(|| black_box(intersection_hashmap(&left_vec, &right_vec)))
        });

        group.bench_function(BenchmarkId::new("subtraction", multiplicity), |b| {
            b.iter(|| black_box(subtraction_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmarks: Nested S-Expression Data
// ============================================================================

/// Generate nested S-expressions with configurable depth
fn generate_nested_atoms(n: usize, depth: usize, prefix: &str) -> Vec<MettaValue> {
    (0..n)
        .map(|i| {
            let mut value = MettaValue::Atom(format!("{}{}", prefix, i));
            for d in 0..depth {
                value = MettaValue::SExpr(vec![MettaValue::Atom(format!("level{}", d)), value]);
            }
            value
        })
        .collect()
}

/// Generate S-expressions with variable-width children
fn generate_complex_sexprs(n: usize, width: usize) -> Vec<MettaValue> {
    (0..n)
        .map(|i| {
            MettaValue::SExpr(
                (0..width)
                    .map(|j| MettaValue::Atom(format!("e{}_{}", i, j)))
                    .collect(),
            )
        })
        .collect()
}

/// Benchmark intersection with nested S-expression data
fn bench_nested_data_intersection(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_data_intersection");
    let size = 1_000;

    for &depth in &[1, 2, 3, 5] {
        let left_vec = generate_nested_atoms(size, depth, "a");
        let right_vec = generate_nested_atoms(size, depth, "a");

        group.bench_function(BenchmarkId::new("hashmap", depth), |b| {
            b.iter(|| black_box(intersection_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

/// Benchmark with complex S-expressions (variable width)
fn bench_complex_sexprs(c: &mut Criterion) {
    let mut group = c.benchmark_group("complex_sexprs");
    let size = 1_000;

    for &width in &[2, 5, 10, 20] {
        let left_vec = generate_complex_sexprs(size, width);
        let right_vec = generate_complex_sexprs(size, width);

        group.bench_function(BenchmarkId::new("hashmap", width), |b| {
            b.iter(|| black_box(intersection_hashmap(&left_vec, &right_vec)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmarks: Batch Processing (Sets of Sets)
// ============================================================================

/// Benchmark processing multiple set pairs
/// Compares: many small sets vs few large sets (same total elements)
fn bench_batch_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_processing");
    group.sample_size(10);

    // Many small sets vs few large sets (same total elements: 10,000)
    let scenarios = [
        ("100x100", 100, 100), // 100 pairs of 100-element sets
        ("10x1000", 10, 1000), // 10 pairs of 1000-element sets
        ("1x10000", 1, 10000), // 1 pair of 10000-element sets
    ];

    for (name, num_pairs, set_size) in scenarios {
        let pairs: Vec<_> = (0..num_pairs)
            .map(|_| generate_with_overlap(set_size, 0.5))
            .collect();

        let pairs_ref = &pairs;
        group.bench_function(BenchmarkId::new("hashmap", name), |b| {
            b.iter(|| {
                pairs_ref
                    .iter()
                    .map(|(left, right)| intersection_hashmap(left, right))
                    .collect::<Vec<_>>()
            })
        });
    }

    group.finish();
}

// ============================================================================
// Criterion Configuration
// ============================================================================

criterion_group!(
    benches,
    bench_intersection_scaling,
    bench_subtraction_scaling,
    bench_union_scaling,
    bench_unique_scaling,
    bench_intersection_overlap,
    bench_subtraction_overlap,
    bench_union_overlap,
    bench_unique_duplicates,
    bench_high_multiplicity,
    bench_nested_data_intersection,
    bench_complex_sexprs,
    bench_batch_processing,
);
criterion_main!(benches);
