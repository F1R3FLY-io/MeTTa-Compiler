//! Dedicated benchmarks for pattern_match() function
//!
//! These benchmarks isolate the core pattern matching algorithm from:
//! - Environment overhead (MORK Space, rule indexing)
//! - Serialization overhead (MettaValue → MORK bytes)
//! - Evaluation overhead (rule application, result aggregation)
//!
//! The goal is to profile pattern_match_impl() in isolation to identify
//! bottlenecks in the matching algorithm itself.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::eval::pattern_match;
use mettatron::backend::MettaValue;
use std::time::Duration;

/// Helper to create a variable atom
fn var(name: &str) -> MettaValue {
    MettaValue::Atom(format!("${}", name))
}

/// Helper to create a wildcard
fn wildcard() -> MettaValue {
    MettaValue::Atom("_".to_string())
}

/// Helper to create an atom
fn atom(name: &str) -> MettaValue {
    MettaValue::Atom(name.to_string())
}

/// Helper to create an s-expression
fn sexpr(items: Vec<MettaValue>) -> MettaValue {
    MettaValue::SExpr(items)
}

/// Benchmark 1: Simple Variable Binding
///
/// Pattern: $x
/// Value: 42
///
/// Tests: Baseline performance for single variable binding
fn bench_simple_variable(c: &mut Criterion) {
    let pattern = var("x");
    let value = MettaValue::Long(42);

    c.bench_function("simple_variable", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 2: Multiple Variables
///
/// Pattern: ($a $b $c)
/// Value: (1 2 3)
///
/// Tests: HashMap overhead with 3 variables
fn bench_multiple_variables(c: &mut Criterion) {
    let pattern = sexpr(vec![var("a"), var("b"), var("c")]);
    let value = sexpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);

    c.bench_function("multiple_variables_3", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 3: Variable Count Scaling
///
/// Patterns with 1, 5, 10, 25, 50 variables
///
/// Tests: HashMap performance degradation with increasing variable count
fn bench_variable_count_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("variable_count_scaling");
    group.measurement_time(Duration::from_secs(10));

    for &count in &[1, 5, 10, 25, 50] {
        let pattern_items: Vec<_> = (0..count).map(|i| var(&format!("v{}", i))).collect();
        let value_items: Vec<_> = (0..count).map(|i| MettaValue::Long(i as i64)).collect();

        let pattern = sexpr(pattern_items);
        let value = sexpr(value_items);

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &(pattern, value),
            |b, (p, v)| b.iter(|| black_box(pattern_match(black_box(p), black_box(v)))),
        );
    }

    group.finish();
}

/// Benchmark 4: Nested Patterns (2 levels)
///
/// Pattern: ($a ($b $c))
/// Value: (1 (2 3))
///
/// Tests: Recursion overhead for shallow nesting
fn bench_nested_2_levels(c: &mut Criterion) {
    let pattern = sexpr(vec![var("a"), sexpr(vec![var("b"), var("c")])]);
    let value = sexpr(vec![
        MettaValue::Long(1),
        sexpr(vec![MettaValue::Long(2), MettaValue::Long(3)]),
    ]);

    c.bench_function("nested_2_levels", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 5: Deep Nesting
///
/// Patterns with 1, 3, 5, 10 levels of nesting
///
/// Tests: Recursion depth impact
fn bench_nesting_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("nesting_depth");
    group.measurement_time(Duration::from_secs(10));

    for &depth in &[1, 3, 5, 10] {
        // Build pattern: ($x0 ($x1 ($x2 ...)))
        let mut pattern = var(&format!("x{}", depth - 1));
        for i in (0..depth - 1).rev() {
            pattern = sexpr(vec![var(&format!("x{}", i)), pattern]);
        }

        // Build value: (1 (2 (3 ...)))
        let mut value = MettaValue::Long(depth as i64);
        for i in (1..depth).rev() {
            value = sexpr(vec![MettaValue::Long(i as i64), value]);
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(depth),
            &(pattern, value),
            |b, (p, v)| b.iter(|| black_box(pattern_match(black_box(p), black_box(v)))),
        );
    }

    group.finish();
}

/// Benchmark 6: Existing Binding Check
///
/// Pattern: ($x $x)
/// Value: (42 42)
///
/// Tests: Structural comparison overhead for duplicate variables
fn bench_existing_binding(c: &mut Criterion) {
    let pattern = sexpr(vec![var("x"), var("x")]);
    let value = sexpr(vec![MettaValue::Long(42), MettaValue::Long(42)]);

    c.bench_function("existing_binding_simple", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 7: Existing Binding with Complex Values
///
/// Pattern: ($x $x)
/// Value: ((a (b c)) (a (b c)))
///
/// Tests: Deep structural comparison overhead
fn bench_existing_binding_complex(c: &mut Criterion) {
    let complex_value = sexpr(vec![atom("a"), sexpr(vec![atom("b"), atom("c")])]);

    let pattern = sexpr(vec![var("x"), var("x")]);
    let value = sexpr(vec![complex_value.clone(), complex_value]);

    c.bench_function("existing_binding_complex", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 8: Ground Type Comparisons
///
/// Tests each MettaValue variant for baseline comparison performance
fn bench_ground_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("ground_types");

    // Bool
    let bool_pattern = MettaValue::Bool(true);
    let bool_value = MettaValue::Bool(true);
    group.bench_function("bool", |b| {
        b.iter(|| {
            black_box(pattern_match(
                black_box(&bool_pattern),
                black_box(&bool_value),
            ))
        })
    });

    // Long
    let long_pattern = MettaValue::Long(42);
    let long_value = MettaValue::Long(42);
    group.bench_function("long", |b| {
        b.iter(|| {
            black_box(pattern_match(
                black_box(&long_pattern),
                black_box(&long_value),
            ))
        })
    });

    // Float
    let float_pattern = MettaValue::Float(std::f64::consts::PI);
    let float_value = MettaValue::Float(std::f64::consts::PI);
    group.bench_function("float", |b| {
        b.iter(|| {
            black_box(pattern_match(
                black_box(&float_pattern),
                black_box(&float_value),
            ))
        })
    });

    // String
    let string_pattern = MettaValue::String("hello".to_string());
    let string_value = MettaValue::String("hello".to_string());
    group.bench_function("string", |b| {
        b.iter(|| {
            black_box(pattern_match(
                black_box(&string_pattern),
                black_box(&string_value),
            ))
        })
    });

    // Atom
    let atom_pattern = atom("test");
    let atom_value = atom("test");
    group.bench_function("atom", |b| {
        b.iter(|| {
            black_box(pattern_match(
                black_box(&atom_pattern),
                black_box(&atom_value),
            ))
        })
    });

    group.finish();
}

/// Benchmark 9: Wildcard Performance
///
/// Pattern: (_ $x _)
/// Value: (foo 42 bar)
///
/// Tests: Wildcard matching overhead
fn bench_wildcards(c: &mut Criterion) {
    let pattern = sexpr(vec![wildcard(), var("x"), wildcard()]);
    let value = sexpr(vec![atom("foo"), MettaValue::Long(42), atom("bar")]);

    c.bench_function("wildcards", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 10: Mixed Complexity (Real-world patterns)
///
/// Pattern: ($a ($b $c) ($d ($e $f)))
/// Value: (1 (2 3) (4 (5 6)))
///
/// Tests: Realistic pattern matching scenario
fn bench_mixed_complexity(c: &mut Criterion) {
    let pattern = sexpr(vec![
        var("a"),
        sexpr(vec![var("b"), var("c")]),
        sexpr(vec![var("d"), sexpr(vec![var("e"), var("f")])]),
    ]);

    let value = sexpr(vec![
        MettaValue::Long(1),
        sexpr(vec![MettaValue::Long(2), MettaValue::Long(3)]),
        sexpr(vec![
            MettaValue::Long(4),
            sexpr(vec![MettaValue::Long(5), MettaValue::Long(6)]),
        ]),
    ]);

    c.bench_function("mixed_complexity", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern), black_box(&value))))
    });
}

/// Benchmark 11: Failure Cases (no match)
///
/// Tests: Early exit optimization for mismatched patterns
fn bench_failures(c: &mut Criterion) {
    let mut group = c.benchmark_group("failures");

    // Type mismatch (atom vs long)
    let pattern1 = atom("foo");
    let value1 = MettaValue::Long(42);
    group.bench_function("type_mismatch", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern1), black_box(&value1))))
    });

    // Length mismatch
    let pattern2 = sexpr(vec![var("a"), var("b")]);
    let value2 = sexpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    group.bench_function("length_mismatch", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern2), black_box(&value2))))
    });

    // Binding conflict ($x $x) vs (1 2)
    let pattern3 = sexpr(vec![var("x"), var("x")]);
    let value3 = sexpr(vec![MettaValue::Long(1), MettaValue::Long(2)]);
    group.bench_function("binding_conflict", |b| {
        b.iter(|| black_box(pattern_match(black_box(&pattern3), black_box(&value3))))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_simple_variable,
    bench_multiple_variables,
    bench_variable_count_scaling,
    bench_nested_2_levels,
    bench_nesting_depth,
    bench_existing_binding,
    bench_existing_binding_complex,
    bench_ground_types,
    bench_wildcards,
    bench_mixed_complexity,
    bench_failures,
);

criterion_main!(benches);
