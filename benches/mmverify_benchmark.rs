//! mmverify Metamath Verifier Benchmark Suite
//!
//! Benchmarks the mmverify demo which verifies theorem th1 (t = t) from the
//! demo0.mm Metamath database. This exercises:
//! - Space operations (new-space, add-atom, remove-atom, match)
//! - State management (new-state, get-state, change-state!)
//! - List operations (to-list, from-list, append, filter')
//! - Pattern matching (match-atom, unify, chain, decons-atom)
//! - Substitution (apply_subst, check_subst, add-subst)
//! - Proof verification (treat_step, treat_assertion, verify)
//!
//! Run with:
//!   taskset -c 0-17 cargo bench --bench mmverify_benchmark
//!
//! Profile with perf:
//!   ./scripts/profile_mmverify.sh

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mettatron::config::{configure_eval, EvalConfig};
use mettatron::{compile, run_state, MettaState};
use std::time::Duration;

// Include mmverify sources - utils are inlined before the demo body
const MMVERIFY_UTILS: &str = include_str!("../examples/mmverify/mmverify-utils.metta");
const VERIFY_DEMO0_BODY: &str = include_str!("mmverify_samples/verify_demo0_body.metta");

/// Build complete self-contained mmverify program by concatenating utils + demo
fn build_mmverify_program() -> String {
    format!("{}\n\n{}", MMVERIFY_UTILS, VERIFY_DEMO0_BODY)
}

/// Run a complete MeTTa program synchronously
fn run_program(src: &str) {
    let state = MettaState::new_empty();
    let program = compile(src).expect("Failed to compile mmverify program");
    let result = run_state(state, program).expect("Failed to run mmverify program");
    black_box(result);
}

/// End-to-end mmverify verification benchmark
fn bench_mmverify_e2e(c: &mut Criterion) {
    // Configure evaluator for CPU-optimized performance
    configure_eval(EvalConfig::cpu_optimized());

    let mut group = c.benchmark_group("mmverify_e2e");

    // Set measurement parameters for statistical rigor
    // 30 second measurement time for reliable statistics
    group.measurement_time(Duration::from_secs(30));
    // 100 samples for 95% confidence intervals
    group.sample_size(100);
    // 3 second warm-up to reach steady state
    group.warm_up_time(Duration::from_secs(3));

    let program = build_mmverify_program();

    group.bench_function("verify_demo0_complete", |b| {
        b.iter(|| run_program(&program));
    });

    group.finish();
}

/// Benchmark just the compilation phase (parsing + AST construction)
fn bench_mmverify_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("mmverify_compile");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    let program = build_mmverify_program();

    group.bench_function("compile_only", |b| {
        b.iter(|| {
            let compiled = compile(&program).expect("Failed to compile");
            black_box(compiled);
        });
    });

    group.finish();
}

/// Benchmark utilities-only evaluation (without proof verification)
fn bench_mmverify_utils_load(c: &mut Criterion) {
    configure_eval(EvalConfig::cpu_optimized());

    let mut group = c.benchmark_group("mmverify_utils_load");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    // Just the utils with empty state setup
    let utils_only = format!(
        r#"
{}

;; Initialize spaces and state only
!(bind! &stack (new-space))
!(bind! &kb (new-space))
!(bind! &sp (new-state -1))
"#,
        MMVERIFY_UTILS
    );

    group.bench_function("utils_load_only", |b| {
        b.iter(|| run_program(&utils_only));
    });

    group.finish();
}

criterion_group!(
    name = mmverify_benchmarks;
    config = Criterion::default()
        .significance_level(0.05)
        .noise_threshold(0.03)
        .confidence_level(0.95);
    targets =
        bench_mmverify_e2e,
        bench_mmverify_compile,
        bench_mmverify_utils_load
);

criterion_main!(mmverify_benchmarks);
