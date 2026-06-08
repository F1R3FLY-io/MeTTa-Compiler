use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile;
use mettatron::backend::eval::eval;
use mettatron::backend::eval::trampoline::new_env;

macro_rules! eval_bind {
    (($results:pat, $env:pat) = eval($value:expr, $input_env:expr, $state:expr $(,)?)) => {
        #[cfg(feature = "index-gc")]
        let ($results, $env, _eval_root_handle) = eval($value, $input_env, $state);
        #[cfg(not(feature = "index-gc"))]
        let ($results, $env) = eval($value, $input_env, $state);
    };
}

/// Generate nested arithmetic expressions as MeTTa text for benchmarking.
/// Example: (+ (* 1 2) (- 3 4) (* 5 6) (/ 7 8))
fn generate_arithmetic_text(num_operations: usize) -> String {
    let operations = ["+", "-", "*", "/"];
    let mut sub_exprs = Vec::new();

    for i in 0..num_operations {
        let op = operations[i % operations.len()];
        let left = i * 2 + 1;
        let right = i * 2 + 2;
        sub_exprs.push(format!("({} {} {})", op, left, right));
    }

    // Wrap in outer addition
    format!("(+ {})", sub_exprs.join(" "))
}

/// Generate deeply nested expressions as MeTTa text.
/// Example: (+ (+ (+ 1 2) (+ 3 4)) (+ (+ 5 6) (+ 7 8)))
fn generate_nested_text(depth: usize) -> String {
    if depth == 0 {
        return "1".to_string();
    }
    format!(
        "(+ {} {})",
        generate_nested_text(depth - 1),
        generate_nested_text(depth - 1)
    )
}

/// Generate mixed complexity expressions as MeTTa text.
/// Combines arithmetic, comparisons, and nested operations.
fn generate_mixed_text(num_operations: usize) -> String {
    let mut sub_exprs = Vec::new();

    for i in 0..num_operations {
        let inner = if i % 3 == 0 {
            // Arithmetic
            format!("(* {} {})", i * 2, i * 3)
        } else if i % 3 == 1 {
            // Nested arithmetic
            format!("(- (+ {} {}) {})", i * 4, i * 5, i * 2)
        } else {
            // Simple division
            format!("(/ {} 2)", i * 10 + 10)
        };
        sub_exprs.push(inner);
    }

    format!("(+ {})", sub_exprs.join(" "))
}

/// Benchmark: Simple arithmetic expressions (threshold boundary testing)
fn bench_simple_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("simple_arithmetic");

    // Test around the threshold boundary (currently 4)
    for num_ops in [2, 3, 4, 5, 6, 8, 10].iter() {
        let text = generate_arithmetic_text(*num_ops);
        let state = compile(&text).expect("Failed to compile");

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        group.bench_with_input(BenchmarkId::new("eval", num_ops), num_ops, |b, _| {
            b.iter(|| {
                let env = new_env();
                eval_bind!((result, _) = eval(black_box(expr), black_box(env), black_box(&state)));
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark: Complex nested expressions
fn bench_nested_expressions(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_expressions");

    // Test various nesting depths
    for depth in [2, 3, 4, 5, 6].iter() {
        let text = generate_nested_text(*depth);
        let state = compile(&text).expect("Failed to compile");

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        group.bench_with_input(BenchmarkId::new("eval_depth", depth), depth, |b, _| {
            b.iter(|| {
                let env = new_env();
                eval_bind!((result, _) = eval(black_box(expr), black_box(env), black_box(&state)));
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark: Mixed complexity expressions
fn bench_mixed_complexity(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_complexity");

    for num_ops in [2, 4, 8, 12, 16, 20].iter() {
        let text = generate_mixed_text(*num_ops);
        let state = compile(&text).expect("Failed to compile");

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        group.bench_with_input(BenchmarkId::new("eval", num_ops), num_ops, |b, _| {
            b.iter(|| {
                let env = new_env();
                eval_bind!((result, _) = eval(black_box(expr), black_box(env), black_box(&state)));
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark: Threshold comparison (sequential vs parallel)
/// This helps tune the PARALLEL_EVAL_THRESHOLD constant
fn bench_threshold_tuning(c: &mut Criterion) {
    let mut group = c.benchmark_group("threshold_tuning");

    // Test critical range around current threshold (low counts)
    // Extended to ultra-high operation counts to find crossover point
    for num_ops in [
        2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384,
        32768,
    ]
    .iter()
    {
        let text = generate_arithmetic_text(*num_ops);
        let state = compile(&text).expect("Failed to compile");

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        group.bench_with_input(BenchmarkId::new("operations", num_ops), num_ops, |b, _| {
            b.iter(|| {
                let env = new_env();
                eval_bind!((result, _) = eval(black_box(expr), black_box(env), black_box(&state)));
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark: Real-world-like expressions
/// Simulates practical MeTTa code patterns
fn bench_realistic_expressions(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic_expressions");

    // Case 1: Financial calculation (4 operations)
    let financial_text = "(+ 10000 (* 10000 (/ 5 100)) (- 100 25))";
    let financial_state = compile(financial_text).expect("Failed to compile");

    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let financial_expr = financial_state.source()[0];
    group.bench_function("financial_calc", |b| {
        b.iter(|| {
            let env = new_env();
            eval_bind!(
                (result, _) = eval(
                    black_box(financial_expr),
                    black_box(env),
                    black_box(&financial_state),
                )
            );
            black_box(result);
        });
    });

    // Case 2: Vector operations (8 operations)
    let vector_parts: Vec<String> = (0..8).map(|i| format!("(* {} {})", i, i + 1)).collect();
    let vector_text = format!("(+ {})", vector_parts.join(" "));
    let vector_state = compile(&vector_text).expect("Failed to compile");

    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let vector_expr = vector_state.source()[0];
    group.bench_function("vector_dot_product", |b| {
        b.iter(|| {
            let env = new_env();
            eval_bind!(
                (result, _) = eval(
                    black_box(vector_expr),
                    black_box(env),
                    black_box(&vector_state),
                )
            );
            black_box(result);
        });
    });

    // Case 3: Complex formula (12 operations)
    let complex_parts: Vec<String> = (0..12)
        .map(|i| format!("(* (+ {} {}) {})", i * 2, i * 3, i + 1))
        .collect();
    let complex_text = format!("(+ {})", complex_parts.join(" "));
    let complex_state = compile(&complex_text).expect("Failed to compile");

    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let complex_expr = complex_state.source()[0];
    group.bench_function("complex_formula", |b| {
        b.iter(|| {
            let env = new_env();
            eval_bind!(
                (result, _) = eval(
                    black_box(complex_expr),
                    black_box(env),
                    black_box(&complex_state),
                )
            );
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
        let items: Vec<String> = (0..*num_ops).map(|i| format!("{}", i)).collect();
        let text = format!("(+ {})", items.join(" "));
        let state = compile(&text).expect("Failed to compile");

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        group.bench_with_input(BenchmarkId::new("trivial_ops", num_ops), num_ops, |b, _| {
            b.iter(|| {
                let env = new_env();
                eval_bind!((result, _) = eval(black_box(expr), black_box(env), black_box(&state)));
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark: Scalability test
/// Tests how performance scales with increasing parallelism
fn bench_scalability(c: &mut Criterion) {
    let mut group = c.benchmark_group("scalability");

    for num_ops in [4, 8, 16, 32, 64].iter() {
        let text = generate_arithmetic_text(*num_ops);
        let state = compile(&text).expect("Failed to compile");

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        group.bench_with_input(BenchmarkId::new("scale", num_ops), num_ops, |b, _| {
            b.iter(|| {
                let env = new_env();
                eval_bind!((result, _) = eval(black_box(expr), black_box(env), black_box(&state)));
                black_box(result);
            });
        });
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
