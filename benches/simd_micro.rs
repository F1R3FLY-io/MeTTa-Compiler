//! Microbenchmarks for SIMD optimization investigation
//!
//! These benchmarks isolate specific operations to determine
//! whether SIMD optimization would provide measurable benefit.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::hint::black_box as bb;

// Test strings of various lengths
const SHORT_VAR: &str = "$x";
const MEDIUM_VAR: &str = "$variable";
const LONG_VAR: &str = "$very_long_variable_name_for_testing";
const GROUND_ATOM: &str = "ground_atom";

// ============================================================================
// Hypothesis 1: Variable Prefix Detection
// ============================================================================

/// Current implementation using starts_with
#[inline(never)]
fn is_variable_starts_with(s: &str) -> bool {
    s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')
}

/// Alternative: Direct byte access (manual optimization)
#[inline(never)]
fn is_variable_byte_check(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.as_bytes()[0];
    first == b'$' || first == b'&' || first == b'\''
}

/// Alternative: Lookup table approach
static VAR_PREFIX_TABLE: [bool; 256] = {
    let mut table = [false; 256];
    table[b'$' as usize] = true;
    table[b'&' as usize] = true;
    table[b'\'' as usize] = true;
    table
};

#[inline(never)]
fn is_variable_lookup_table(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    VAR_PREFIX_TABLE[s.as_bytes()[0] as usize]
}

fn bench_variable_prefix_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("variable_prefix_detection");

    // Test with variable strings
    for (name, test_str) in [
        ("short_var", SHORT_VAR),
        ("medium_var", MEDIUM_VAR),
        ("long_var", LONG_VAR),
        ("ground_atom", GROUND_ATOM),
    ] {
        group.bench_with_input(
            BenchmarkId::new("starts_with", name),
            &test_str,
            |b, s| b.iter(|| black_box(is_variable_starts_with(black_box(s)))),
        );

        group.bench_with_input(
            BenchmarkId::new("byte_check", name),
            &test_str,
            |b, s| b.iter(|| black_box(is_variable_byte_check(black_box(s)))),
        );

        group.bench_with_input(
            BenchmarkId::new("lookup_table", name),
            &test_str,
            |b, s| b.iter(|| black_box(is_variable_lookup_table(black_box(s)))),
        );
    }

    group.finish();
}

// ============================================================================
// Hypothesis 2: String Equality Comparison
// ============================================================================

/// Generate test strings of various lengths
fn generate_test_strings() -> Vec<(String, String)> {
    vec![
        ("short".to_string(), "short".to_string()),
        ("medium_length_string".to_string(), "medium_length_string".to_string()),
        ("this_is_a_longer_string_for_testing_purposes".to_string(),
         "this_is_a_longer_string_for_testing_purposes".to_string()),
        // Also test mismatches
        ("mismatch_early".to_string(), "xismatch_early".to_string()),
        ("mismatch_late_in_string".to_string(), "mismatch_late_in_striXg".to_string()),
    ]
}

fn bench_string_equality(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_equality");

    let test_cases = generate_test_strings();

    for (i, (a, b)) in test_cases.iter().enumerate() {
        let name = format!("len_{}_match_{}", a.len(), a == b);

        // Standard equality
        group.bench_with_input(
            BenchmarkId::new("std_eq", &name),
            &(a, b),
            |bench, (a, b)| bench.iter(|| black_box(black_box(a.as_str()) == black_box(b.as_str()))),
        );

        // Direct bytes comparison
        group.bench_with_input(
            BenchmarkId::new("bytes_eq", &name),
            &(a, b),
            |bench, (a, b)| {
                bench.iter(|| {
                    let a = black_box(a.as_bytes());
                    let b = black_box(b.as_bytes());
                    black_box(a.len() == b.len() && a == b)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Hypothesis 3: Batch Arithmetic Operations
// ============================================================================

/// Test batch addition - scalar version
#[inline(never)]
fn batch_add_scalar(a: &[i64], b: &[i64], result: &mut [i64]) {
    for i in 0..a.len() {
        result[i] = a[i].wrapping_add(b[i]);
    }
}

/// Test batch addition - iterator version
#[inline(never)]
fn batch_add_iter(a: &[i64], b: &[i64], result: &mut [i64]) {
    for ((r, x), y) in result.iter_mut().zip(a.iter()).zip(b.iter()) {
        *r = x.wrapping_add(*y);
    }
}

fn bench_batch_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_arithmetic");

    for size in [4usize, 8, 16, 32, 64] {
        let a: Vec<i64> = (0..size as i64).collect();
        let b: Vec<i64> = (0..size as i64).map(|x| x * 2).collect();

        group.bench_with_input(
            BenchmarkId::new("scalar", size),
            &(&a, &b),
            |bench, (a, b)| {
                let mut result = vec![0i64; a.len()];
                bench.iter(|| {
                    batch_add_scalar(black_box(a), black_box(b), &mut result);
                    black_box(result[0])
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("iter", size),
            &(&a, &b),
            |bench, (a, b)| {
                let mut result = vec![0i64; a.len()];
                bench.iter(|| {
                    batch_add_iter(black_box(a), black_box(b), &mut result);
                    black_box(result[0])
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Additional: Measure overhead of operations in pattern matching
// ============================================================================

fn bench_hashmap_operations(c: &mut Criterion) {
    use std::collections::HashMap;

    let mut group = c.benchmark_group("hashmap_overhead");

    // Measure HashMap insert overhead
    group.bench_function("insert_single", |b| {
        b.iter(|| {
            let mut map: HashMap<String, i64> = HashMap::new();
            map.insert(black_box("$x".to_string()), black_box(42));
            black_box(map)
        })
    });

    group.bench_function("insert_3", |b| {
        b.iter(|| {
            let mut map: HashMap<String, i64> = HashMap::new();
            map.insert(black_box("$a".to_string()), black_box(1));
            map.insert(black_box("$b".to_string()), black_box(2));
            map.insert(black_box("$c".to_string()), black_box(3));
            black_box(map)
        })
    });

    // Measure String clone overhead
    group.bench_function("string_clone_short", |b| {
        let s = "$x".to_string();
        b.iter(|| black_box(black_box(&s).clone()))
    });

    group.bench_function("string_clone_medium", |b| {
        let s = "$variable_name".to_string();
        b.iter(|| black_box(black_box(&s).clone()))
    });

    group.bench_function("string_clone_long", |b| {
        let s = "$very_long_variable_name_for_testing".to_string();
        b.iter(|| black_box(black_box(&s).clone()))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_variable_prefix_detection,
    bench_string_equality,
    bench_batch_arithmetic,
    bench_hashmap_operations,
);

criterion_main!(benches);
