//! Benchmark comparing bytecode VM vs tree-walking interpreter
//!
//! This benchmark measures the performance difference between the two evaluation
//! strategies for various MeTTa expression types.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mettatron::backend::bytecode::{compile, BytecodeVM};
use mettatron::backend::{Environment, MettaValue};
use mettatron::backend::eval::eval;
use std::sync::Arc;
use std::time::Duration;

/// Helper to create an atom
fn atom(name: &str) -> MettaValue {
    MettaValue::Atom(name.to_string())
}

/// Helper to create an s-expression
fn sexpr(items: Vec<MettaValue>) -> MettaValue {
    MettaValue::SExpr(items)
}

/// Evaluate expression via bytecode VM
fn eval_bytecode(expr: &MettaValue) -> Vec<MettaValue> {
    let chunk = compile("bench", expr).expect("compilation failed");
    let mut vm = BytecodeVM::new(Arc::new(chunk));
    vm.run().expect("VM execution failed")
}

/// Evaluate expression via tree-walking interpreter
fn eval_tree_walker(expr: &MettaValue) -> Vec<MettaValue> {
    let env = Environment::new();
    let (results, _env) = eval(expr.clone(), env);
    results
}

// ============================================================================
// Benchmark 1: Arithmetic Expressions
// ============================================================================

/// Build nested arithmetic expression: ((((a + b) + c) + d) + e)
fn build_arithmetic_chain(depth: usize) -> MettaValue {
    let mut expr = MettaValue::Long(1);
    for i in 0..depth {
        expr = sexpr(vec![atom("+"), expr, MettaValue::Long(i as i64)]);
    }
    expr
}

fn bench_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("arithmetic");
    group.measurement_time(Duration::from_secs(5));

    for depth in [5, 10, 20, 50].iter() {
        let expr = build_arithmetic_chain(*depth);

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 2: Boolean Logic
// ============================================================================

/// Build nested boolean expression: (and (or (not a) b) (and c d))
fn build_boolean_expr(depth: usize) -> MettaValue {
    let mut expr = MettaValue::Bool(true);
    for i in 0..depth {
        let inner = if i % 3 == 0 {
            sexpr(vec![atom("not"), expr])
        } else if i % 3 == 1 {
            sexpr(vec![atom("or"), expr, MettaValue::Bool(false)])
        } else {
            sexpr(vec![atom("and"), expr, MettaValue::Bool(true)])
        };
        expr = inner;
    }
    expr
}

fn bench_boolean(c: &mut Criterion) {
    let mut group = c.benchmark_group("boolean");
    group.measurement_time(Duration::from_secs(5));

    for depth in [5, 10, 20, 50].iter() {
        let expr = build_boolean_expr(*depth);

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 3: Conditionals
// ============================================================================

/// Build nested if expressions: (if cond (if cond ... ) else)
fn build_if_chain(depth: usize) -> MettaValue {
    let mut expr = MettaValue::Long(0);
    for i in (0..depth).rev() {
        expr = sexpr(vec![
            atom("if"),
            sexpr(vec![atom("<"), MettaValue::Long(i as i64), MettaValue::Long(5)]),
            MettaValue::Long(i as i64),
            expr,
        ]);
    }
    expr
}

fn bench_conditionals(c: &mut Criterion) {
    let mut group = c.benchmark_group("conditionals");
    group.measurement_time(Duration::from_secs(5));

    for depth in [5, 10, 20].iter() {
        let expr = build_if_chain(*depth);

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 4: Nondeterminism (Superpose)
// ============================================================================

/// Build superpose with N alternatives: (superpose (1 2 3 ... N))
fn build_superpose(count: usize) -> MettaValue {
    let alternatives: Vec<MettaValue> = (0..count).map(|i| MettaValue::Long(i as i64)).collect();
    sexpr(vec![atom("superpose"), MettaValue::SExpr(alternatives)])
}

fn bench_superpose(c: &mut Criterion) {
    let mut group = c.benchmark_group("superpose");
    group.measurement_time(Duration::from_secs(5));

    for count in [5, 10, 50, 100].iter() {
        let expr = build_superpose(*count);

        group.throughput(Throughput::Elements(*count as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", count), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode", count), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 5: Quote/Unquote
// ============================================================================

fn bench_quote(c: &mut Criterion) {
    let mut group = c.benchmark_group("quote");
    group.measurement_time(Duration::from_secs(5));

    // Simple quoted expression
    let simple = sexpr(vec![
        atom("quote"),
        sexpr(vec![atom("+"), MettaValue::Long(1), MettaValue::Long(2)]),
    ]);

    // Deeply nested quoted expression
    let mut deep = MettaValue::Long(42);
    for _ in 0..10 {
        let d = deep.clone();
        deep = sexpr(vec![atom("foo"), d, deep]);
    }
    let deep_quoted = sexpr(vec![atom("quote"), deep]);

    group.bench_function("simple/tree-walker", |b| {
        b.iter(|| eval_tree_walker(black_box(&simple)))
    });

    group.bench_function("simple/bytecode", |b| {
        b.iter(|| eval_bytecode(black_box(&simple)))
    });

    group.bench_function("deep/tree-walker", |b| {
        b.iter(|| eval_tree_walker(black_box(&deep_quoted)))
    });

    group.bench_function("deep/bytecode", |b| {
        b.iter(|| eval_bytecode(black_box(&deep_quoted)))
    });

    group.finish();
}

// ============================================================================
// Benchmark 6: Compilation Overhead
// ============================================================================

fn bench_compilation(c: &mut Criterion) {
    let mut group = c.benchmark_group("compilation");
    group.measurement_time(Duration::from_secs(5));

    // Various expression sizes
    let small = sexpr(vec![atom("+"), MettaValue::Long(1), MettaValue::Long(2)]);
    let medium = build_arithmetic_chain(20);
    let large = build_arithmetic_chain(100);

    group.bench_function("small/compile", |b| {
        b.iter(|| compile("bench", black_box(&small)))
    });

    group.bench_function("medium/compile", |b| {
        b.iter(|| compile("bench", black_box(&medium)))
    });

    group.bench_function("large/compile", |b| {
        b.iter(|| compile("bench", black_box(&large)))
    });

    // Measure compile + execute combined
    group.bench_function("small/compile+execute", |b| {
        b.iter(|| eval_bytecode(black_box(&small)))
    });

    group.bench_function("medium/compile+execute", |b| {
        b.iter(|| eval_bytecode(black_box(&medium)))
    });

    group.bench_function("large/compile+execute", |b| {
        b.iter(|| eval_bytecode(black_box(&large)))
    });

    // Measure execute-only (pre-compiled chunk reused)
    let small_chunk = Arc::new(compile("bench", &small).unwrap());
    let medium_chunk = Arc::new(compile("bench", &medium).unwrap());
    let large_chunk = Arc::new(compile("bench", &large).unwrap());

    group.bench_function("small/execute-only", |b| {
        b.iter(|| {
            let mut vm = BytecodeVM::new(Arc::clone(&small_chunk));
            vm.run()
        })
    });

    group.bench_function("medium/execute-only", |b| {
        b.iter(|| {
            let mut vm = BytecodeVM::new(Arc::clone(&medium_chunk));
            vm.run()
        })
    });

    group.bench_function("large/execute-only", |b| {
        b.iter(|| {
            let mut vm = BytecodeVM::new(Arc::clone(&large_chunk));
            vm.run()
        })
    });

    group.finish();
}

// ============================================================================
// Main
// ============================================================================

criterion_group!(
    benches,
    bench_arithmetic,
    bench_boolean,
    bench_conditionals,
    bench_superpose,
    bench_quote,
    bench_compilation,
);

criterion_main!(benches);
