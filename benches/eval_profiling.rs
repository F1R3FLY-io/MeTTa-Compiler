//! Comprehensive benchmark suite for eval profiling
//!
//! Targets: Lazy evaluation, Trampolining, TCO, Cartesian products
//!
//! This benchmark suite is designed for profiling with Linux perf to identify
//! performance bottlenecks in the evaluation engine.
//!
//! Run with:
//!   cargo bench --bench eval_profiling
//!
//! Profile with perf:
//!   cargo build --profile=profiling --bench eval_profiling
//!   perf record --call-graph=dwarf -- ./target/profiling/deps/eval_profiling-*
//!   perf report

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;
use mettatron::backend::MettaValue;
use std::time::Duration;

// Include stress test program sources
const TCO_DEEP_RECURSION: &str = include_str!("metta_samples/tco_deep_recursion.metta");
const CARTESIAN_PRODUCT_STRESS: &str = include_str!("metta_samples/cartesian_product_stress.metta");
const TRAMPOLINE_STRESS: &str = include_str!("metta_samples/trampoline_stress.metta");
const LAZY_EAGER_COMPARISON: &str = include_str!("metta_samples/lazy_eager_comparison.metta");
const GROUNDED_TCO_STRESS: &str = include_str!("metta_samples/grounded_tco_stress.metta");

/// Run a complete MeTTa program and return the number of evaluations
fn run_program(src: &str) -> usize {
    let state = compile(src).expect("Failed to compile");
    let mut env = state.environment;
    let mut eval_count = 0;

    for expr in state.source {
        let (_, new_env) = eval(black_box(expr), env);
        env = new_env;
        eval_count += 1;
    }

    eval_count
}

/// Generate a countdown recursion program with specified depth
fn generate_countdown(depth: usize) -> String {
    format!(
        r#"
(= (countdown 0 $acc) $acc)
(= (countdown $n $acc) (countdown (- $n 1) (+ $acc 1)))
!(countdown {} 0)
"#,
        depth
    )
}

/// Generate wide arithmetic expression (many siblings)
fn generate_wide_arithmetic(width: usize) -> MettaValue {
    let mut items = vec![MettaValue::Atom("+".to_string())];
    for i in 1..=width {
        items.push(MettaValue::Long(i as i64));
    }
    MettaValue::SExpr(items)
}

/// Generate deep nested arithmetic (binary tree shape)
fn generate_deep_arithmetic(depth: usize) -> MettaValue {
    if depth == 0 {
        return MettaValue::Long(1);
    }
    MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        generate_deep_arithmetic(depth - 1),
        MettaValue::Long(depth as i64),
    ])
}

/// Generate nondeterministic choice program with specified choice count and depth
fn generate_cartesian_product(choice_count: usize, depth: usize) -> String {
    let mut program = String::new();

    // Generate choice rules
    for i in 0..choice_count {
        program.push_str(&format!("(= (choice) {})\n", i));
    }

    // Generate nested let expression
    let mut expr = String::from("(list");
    for i in 0..depth {
        expr = format!("(let $v{} (choice) {}", i, expr);
    }

    // Add variables to list
    for i in 0..depth {
        expr.push_str(&format!(" $v{}", i));
    }
    expr.push(')');

    // Close all let expressions
    for _ in 0..depth {
        expr.push(')');
    }

    program.push_str(&format!("!{}\n", expr));
    program
}

// =============================================================================
// Benchmark 1: TCO Deep Recursion
// =============================================================================

fn bench_tco_recursion(c: &mut Criterion) {
    let mut group = c.benchmark_group("tco_recursion");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    for depth in [100, 250, 500, 750, 900].iter() {
        let program = generate_countdown(*depth);

        group.throughput(Throughput::Elements(*depth as u64));
        group.bench_with_input(BenchmarkId::new("countdown_depth", depth), depth, |b, _| {
            b.iter(|| run_program(&program));
        });
    }

    group.finish();
}

// =============================================================================
// Benchmark 2: Cartesian Product Scaling
// =============================================================================

fn bench_cartesian_product(c: &mut Criterion) {
    let mut group = c.benchmark_group("cartesian_product");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    // Test different choice counts with fixed depth
    let binary_3 = generate_cartesian_product(2, 3); // 2^3 = 8 combos
    let ternary_3 = generate_cartesian_product(3, 3); // 3^3 = 27 combos
    let quinary_3 = generate_cartesian_product(5, 3); // 5^3 = 125 combos

    group.throughput(Throughput::Elements(8));
    group.bench_function("binary_3vars_8combos", |b| {
        b.iter(|| run_program(&binary_3));
    });

    group.throughput(Throughput::Elements(27));
    group.bench_function("ternary_3vars_27combos", |b| {
        b.iter(|| run_program(&ternary_3));
    });

    group.throughput(Throughput::Elements(125));
    group.bench_function("quinary_3vars_125combos", |b| {
        b.iter(|| run_program(&quinary_3));
    });

    // Test different depths with fixed choice count
    for depth in [2, 3, 4, 5].iter() {
        let program = generate_cartesian_product(2, *depth);
        let combos = 2_u64.pow(*depth as u32);

        group.throughput(Throughput::Elements(combos));
        group.bench_with_input(BenchmarkId::new("binary_depth", depth), depth, |b, _| {
            b.iter(|| run_program(&program));
        });
    }

    group.finish();
}

// =============================================================================
// Benchmark 3: Trampoline Work Stack
// =============================================================================

fn bench_trampoline_workstack(c: &mut Criterion) {
    let mut group = c.benchmark_group("trampoline_workstack");
    group.measurement_time(Duration::from_secs(10));

    // Wide arithmetic expressions (many siblings)
    for width in [5, 10, 20, 50, 100].iter() {
        let expr = generate_wide_arithmetic(*width);
        let env = Environment::new();

        group.throughput(Throughput::Elements(*width as u64));
        group.bench_with_input(BenchmarkId::new("wide_arithmetic", width), width, |b, _| {
            b.iter(|| eval(black_box(expr.clone()), env.clone()));
        });
    }

    // Deep nested arithmetic (binary tree shape)
    for depth in [5, 10, 15, 20, 25].iter() {
        let expr = generate_deep_arithmetic(*depth);
        let env = Environment::new();

        group.throughput(Throughput::Elements(*depth as u64));
        group.bench_with_input(BenchmarkId::new("deep_arithmetic", depth), depth, |b, _| {
            b.iter(|| eval(black_box(expr.clone()), env.clone()));
        });
    }

    group.finish();
}

// =============================================================================
// Benchmark 4: Lazy Evaluation Benefits
// =============================================================================

fn bench_lazy_evaluation(c: &mut Criterion) {
    let mut group = c.benchmark_group("lazy_evaluation");
    group.measurement_time(Duration::from_secs(10));

    // Lazy if: should skip else branch
    let lazy_if_true = r#"
(= (expensive) (+ (* 100 200) (* 300 400) (* 500 600)))
!(if True 1 (expensive))
"#;

    // Eager pattern: evaluates both branches
    let eager_both = r#"
(= (expensive) (+ (* 100 200) (* 300 400) (* 500 600)))
!(let $then 1 (let $else (expensive) (if True $then $else)))
"#;

    // Short-circuit AND
    let short_circuit_and = r#"
(= (expensive) (+ (* 100 200) (* 300 400) (* 500 600)))
!(and False (expensive))
"#;

    // Short-circuit OR
    let short_circuit_or = r#"
(= (expensive) (+ (* 100 200) (* 300 400) (* 500 600)))
!(or True (expensive))
"#;

    group.bench_function("lazy_if_skip_else", |b| {
        b.iter(|| run_program(lazy_if_true));
    });

    group.bench_function("eager_evaluate_both", |b| {
        b.iter(|| run_program(eager_both));
    });

    group.bench_function("short_circuit_and", |b| {
        b.iter(|| run_program(short_circuit_and));
    });

    group.bench_function("short_circuit_or", |b| {
        b.iter(|| run_program(short_circuit_or));
    });

    group.finish();
}

// =============================================================================
// Benchmark 5: Grounded TCO Operations
// =============================================================================

fn bench_grounded_tco(c: &mut Criterion) {
    let mut group = c.benchmark_group("grounded_tco");
    group.measurement_time(Duration::from_secs(10));

    // Generate add chains of different lengths
    for chain_len in [2, 4, 8, 16, 32].iter() {
        let mut expr = MettaValue::Long(1);
        for i in 2..=*chain_len {
            expr = MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                expr,
                MettaValue::Long(i),
            ]);
        }
        let env = Environment::new();

        group.throughput(Throughput::Elements(*chain_len as u64));
        group.bench_with_input(
            BenchmarkId::new("add_chain", chain_len),
            chain_len,
            |b, _| {
                b.iter(|| eval(black_box(expr.clone()), env.clone()));
            },
        );
    }

    // Generate comparison chains
    for chain_len in [2, 4, 8].iter() {
        // Build: (and (< 1 2) (and (< 2 3) ...))
        let mut expr = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(*chain_len as i64),
            MettaValue::Long(*chain_len as i64 + 1),
        ]);

        for i in (1..*chain_len).rev() {
            let cmp = MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::Long(i as i64),
                MettaValue::Long(i as i64 + 1),
            ]);
            expr = MettaValue::SExpr(vec![MettaValue::Atom("and".to_string()), cmp, expr]);
        }

        let env = Environment::new();

        group.throughput(Throughput::Elements(*chain_len as u64));
        group.bench_with_input(
            BenchmarkId::new("comparison_chain", chain_len),
            chain_len,
            |b, _| {
                b.iter(|| eval(black_box(expr.clone()), env.clone()));
            },
        );
    }

    group.finish();
}

// =============================================================================
// Benchmark 6: Full Program Suites
// =============================================================================

fn bench_full_programs(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_programs");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(30);

    group.bench_function("tco_deep_recursion", |b| {
        b.iter(|| run_program(TCO_DEEP_RECURSION));
    });

    group.bench_function("cartesian_product_stress", |b| {
        b.iter(|| run_program(CARTESIAN_PRODUCT_STRESS));
    });

    group.bench_function("trampoline_stress", |b| {
        b.iter(|| run_program(TRAMPOLINE_STRESS));
    });

    group.bench_function("lazy_eager_comparison", |b| {
        b.iter(|| run_program(LAZY_EAGER_COMPARISON));
    });

    group.bench_function("grounded_tco_stress", |b| {
        b.iter(|| run_program(GROUNDED_TCO_STRESS));
    });

    group.finish();
}

// =============================================================================
// Benchmark 7: Continuation Management Overhead
// =============================================================================

fn bench_continuation_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("continuation_overhead");
    group.measurement_time(Duration::from_secs(10));

    // Many let bindings (creates many CollectSExpr continuations)
    for let_count in [4, 8, 12, 16].iter() {
        let mut program = String::new();

        // Build nested let expression
        let mut expr = format!("(+ ");
        for i in 0..*let_count {
            expr = format!("(let $v{} (+ {} {}) {}", i, i * 2, i * 2 + 1, expr);
        }

        // Add all variables
        for i in 0..*let_count {
            expr.push_str(&format!("$v{} ", i));
        }

        // Close all parens
        expr.push(')');
        for _ in 0..*let_count {
            expr.push(')');
        }

        program.push_str(&format!("!{}\n", expr));

        group.throughput(Throughput::Elements(*let_count as u64));
        group.bench_with_input(
            BenchmarkId::new("nested_lets", let_count),
            let_count,
            |b, _| {
                b.iter(|| run_program(&program));
            },
        );
    }

    group.finish();
}

// =============================================================================
// Benchmark 8: Rule Matching with Nondeterminism
// =============================================================================

fn bench_rule_matching_nondet(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_matching_nondet");
    group.measurement_time(Duration::from_secs(15));

    // Multiple rules with same head (nondeterminism)
    for rule_count in [2, 4, 8, 16].iter() {
        let mut program = String::new();

        // Generate multiple rules for same head
        for i in 0..*rule_count {
            program.push_str(&format!("(= (f $x) (+ $x {}))\n", i));
        }

        program.push_str("!(f 100)\n");

        group.throughput(Throughput::Elements(*rule_count as u64));
        group.bench_with_input(
            BenchmarkId::new("rules_same_head", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| run_program(&program));
            },
        );
    }

    group.finish();
}

// =============================================================================
// Criterion Configuration
// =============================================================================

criterion_group!(
    name = eval_profiling;
    config = Criterion::default()
        .significance_level(0.05)
        .noise_threshold(0.03)
        .warm_up_time(Duration::from_secs(3));
    targets =
        bench_tco_recursion,
        bench_cartesian_product,
        bench_trampoline_workstack,
        bench_lazy_evaluation,
        bench_grounded_tco,
        bench_full_programs,
        bench_continuation_overhead,
        bench_rule_matching_nondet
);

criterion_main!(eval_profiling);
