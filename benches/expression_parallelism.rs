use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::environment::Environment;
use mettatron::backend::MettaValue;
use mettatron::eval;

/// Generate nested arithmetic expressions for benchmarking
/// Example: (+ (* 2 3) (/ 10 5) (- 8 4) (* 7 2))
fn generate_arithmetic_expr(num_operations: usize) -> MettaValue {
    let operations = vec!["+", "-", "*", "/"];
    let mut sub_exprs = Vec::new();

    for i in 0..num_operations {
        let op = operations[i % operations.len()];
        let left = (i * 2 + 1) as i64;
        let right = (i * 2 + 2) as i64;

        sub_exprs.push(MettaValue::SExpr(vec![
            MettaValue::Atom(op.to_string()),
            MettaValue::Long(left),
            MettaValue::Long(right),
        ]));
    }

    // Wrap in outer addition
    let mut full_expr = vec![MettaValue::Atom("+".to_string())];
    full_expr.extend(sub_exprs);

    MettaValue::SExpr(full_expr)
}

/// Generate deeply nested expressions
/// Example: (+ (+ (+ 1 2) (+ 3 4)) (+ (+ 5 6) (+ 7 8)))
fn generate_nested_expr(depth: usize) -> MettaValue {
    if depth == 0 {
        return MettaValue::Long(1);
    }

    MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        generate_nested_expr(depth - 1),
        generate_nested_expr(depth - 1),
    ])
}

/// Generate mixed complexity expressions
/// Combines arithmetic, comparisons, and nested operations
fn generate_mixed_expr(num_operations: usize) -> MettaValue {
    let mut sub_exprs = vec![MettaValue::Atom("+".to_string())];

    for i in 0..num_operations {
        let inner = if i % 3 == 0 {
            // Arithmetic
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long((i * 2) as i64),
                MettaValue::Long((i * 3) as i64),
            ])
        } else if i % 3 == 1 {
            // Nested arithmetic
            MettaValue::SExpr(vec![
                MettaValue::Atom("-".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long((i * 4) as i64),
                    MettaValue::Long((i * 5) as i64),
                ]),
                MettaValue::Long((i * 2) as i64),
            ])
        } else {
            // Simple division
            MettaValue::SExpr(vec![
                MettaValue::Atom("/".to_string()),
                MettaValue::Long((i * 10 + 10) as i64),
                MettaValue::Long(2),
            ])
        };

        sub_exprs.push(inner);
    }

    MettaValue::SExpr(sub_exprs)
}

/// Benchmark: Simple arithmetic expressions (threshold boundary testing)
fn bench_simple_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("simple_arithmetic");

    // Test around the threshold boundary (currently 4)
    for num_ops in [2, 3, 4, 5, 6, 8, 10].iter() {
        let expr = generate_arithmetic_expr(*num_ops);
        let env = Environment::new();

        group.bench_with_input(
            BenchmarkId::new("eval", num_ops),
            num_ops,
            |b, _| {
                b.iter(|| {
                    let result = eval(black_box(expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Complex nested expressions
fn bench_nested_expressions(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_expressions");

    // Test various nesting depths
    for depth in [2, 3, 4, 5, 6].iter() {
        let expr = generate_nested_expr(*depth);
        let env = Environment::new();

        group.bench_with_input(
            BenchmarkId::new("eval_depth", depth),
            depth,
            |b, _| {
                b.iter(|| {
                    let result = eval(black_box(expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Mixed complexity expressions
fn bench_mixed_complexity(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_complexity");

    for num_ops in [2, 4, 8, 12, 16, 20].iter() {
        let expr = generate_mixed_expr(*num_ops);
        let env = Environment::new();

        group.bench_with_input(
            BenchmarkId::new("eval", num_ops),
            num_ops,
            |b, _| {
                b.iter(|| {
                    let result = eval(black_box(expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Threshold comparison (sequential vs parallel)
/// This helps tune the PARALLEL_EVAL_THRESHOLD constant
fn bench_threshold_tuning(c: &mut Criterion) {
    let mut group = c.benchmark_group("threshold_tuning");

    // Test critical range around current threshold (low counts)
    // Extended to ultra-high operation counts to find crossover point
    for num_ops in [2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768].iter() {
        let expr = generate_arithmetic_expr(*num_ops);
        let env = Environment::new();

        group.bench_with_input(
            BenchmarkId::new("operations", num_ops),
            num_ops,
            |b, _| {
                b.iter(|| {
                    let result = eval(black_box(expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Real-world-like expressions
/// Simulates practical MeTTa code patterns
fn bench_realistic_expressions(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic_expressions");

    // Case 1: Financial calculation (4 operations)
    let financial = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        // Principal
        MettaValue::Long(10000),
        // Interest
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(10000),
            MettaValue::SExpr(vec![
                MettaValue::Atom("/".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(100),
            ]),
        ]),
        // Fees
        MettaValue::SExpr(vec![
            MettaValue::Atom("-".to_string()),
            MettaValue::Long(100),
            MettaValue::Long(25),
        ]),
    ]);

    group.bench_function("financial_calc", |b| {
        let env = Environment::new();
        b.iter(|| {
            let result = eval(black_box(financial.clone()), black_box(env.clone()));
            black_box(result);
        });
    });

    // Case 2: Vector operations (8 operations)
    let mut vector_ops = vec![MettaValue::Atom("+".to_string())];
    for i in 0..8 {
        vector_ops.push(MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(i),
            MettaValue::Long(i + 1),
        ]));
    }
    let vector_expr = MettaValue::SExpr(vector_ops);

    group.bench_function("vector_dot_product", |b| {
        let env = Environment::new();
        b.iter(|| {
            let result = eval(black_box(vector_expr.clone()), black_box(env.clone()));
            black_box(result);
        });
    });

    // Case 3: Complex formula (12 operations)
    let mut complex = vec![MettaValue::Atom("+".to_string())];
    for i in 0..12 {
        complex.push(MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(i * 2),
                MettaValue::Long(i * 3),
            ]),
            MettaValue::Long(i + 1),
        ]));
    }
    let complex_expr = MettaValue::SExpr(complex);

    group.bench_function("complex_formula", |b| {
        let env = Environment::new();
        b.iter(|| {
            let result = eval(black_box(complex_expr.clone()), black_box(env.clone()));
            black_box(result);
        });
    });

    group.finish();
}

/// Benchmark: Parallel overhead measurement
/// Helps understand when parallelization becomes beneficial
fn bench_parallel_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_overhead");
    group.sample_size(100); // More samples for accurate overhead measurement

    // Very simple expressions to measure pure overhead
    for num_ops in [1, 2, 3, 4, 5, 6].iter() {
        let mut expr_vec = vec![MettaValue::Atom("+".to_string())];
        for i in 0..*num_ops {
            expr_vec.push(MettaValue::Long(i as i64));
        }
        let expr = MettaValue::SExpr(expr_vec);
        let env = Environment::new();

        group.bench_with_input(
            BenchmarkId::new("trivial_ops", num_ops),
            num_ops,
            |b, _| {
                b.iter(|| {
                    let result = eval(black_box(expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Scalability test
/// Tests how performance scales with increasing parallelism
fn bench_scalability(c: &mut Criterion) {
    let mut group = c.benchmark_group("scalability");

    for num_ops in [4, 8, 16, 32, 64].iter() {
        let expr = generate_arithmetic_expr(*num_ops);
        let env = Environment::new();

        group.bench_with_input(
            BenchmarkId::new("scale", num_ops),
            num_ops,
            |b, _| {
                b.iter(|| {
                    let result = eval(black_box(expr.clone()), black_box(env.clone()));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_simple_arithmetic,
    bench_nested_expressions,
    bench_mixed_complexity,
    bench_threshold_tuning,
    bench_realistic_expressions,
    bench_parallel_overhead,
    bench_scalability
);

criterion_main!(benches);
