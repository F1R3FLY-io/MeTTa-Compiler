//! Memoization Benchmark Suite
//!
//! Benchmarks the memoization feature with classic dynamic programming problems:
//! - Fibonacci (exponential → linear with memoization)
//! - Levenshtein edit distance (exponential → polynomial with memoization)
//!
//! These benchmarks verify that memoization provides the expected speedup
//! for recursive functions with overlapping subproblems.
//!
//! Run with:
//!   cargo bench --bench memoization_benchmark
//!
//! Expected results:
//!   - Fibonacci with memo should be ~1000x faster than without
//!   - Levenshtein with memo should be ~100-200x faster than without

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mettatron::config::{configure_eval, EvalConfig};
use mettatron::{compile, run_state, MettaState};
use std::sync::Once;
use std::time::Duration;

// Fibonacci without memoization - exponential time
const FIB_NO_MEMO: &str = r#"
(= (fib 0) 0)
(= (fib 1) 1)
(= (fib $n)
   (if (> $n 1)
       (+ (fib (- $n 1)) (fib (- $n 2)))
       $n))
!(fib 20)
"#;

// Fibonacci with memoization - linear time
const FIB_MEMO: &str = r#"
!(bind! &fib-cache (new-memo "fib-cache"))
(= (fib 0) 0)
(= (fib 1) 1)
(= (fib $n)
   (if (> $n 1)
       (memo-first &fib-cache
         (+ (fib (- $n 1)) (fib (- $n 2))))
       $n))
!(fib 20)
!(fib 30)
!(fib 40)
!(fib 50)
"#;

// Levenshtein without memoization - exponential time
const LEV_NO_MEMO: &str = r#"
(= (min3 $a $b $c)
   (if (< $a $b)
       (if (< $a $c) $a $c)
       (if (< $b $c) $b $c)))

(= (lev Nil Nil) 0)
(= (lev Nil (Cons $_ $ys)) (+ 1 (lev Nil $ys)))
(= (lev (Cons $_ $xs) Nil) (+ 1 (lev $xs Nil)))
(= (lev (Cons $x $xs) (Cons $y $ys))
   (if (== $x $y)
       (lev $xs $ys)
       (+ 1 (min3
              (lev $xs (Cons $y $ys))
              (lev (Cons $x $xs) $ys)
              (lev $xs $ys)))))

; kitten -> sitting (distance 3)
!(lev (Cons k (Cons i (Cons t (Cons t (Cons e (Cons n Nil))))))
      (Cons s (Cons i (Cons t (Cons t (Cons i (Cons n (Cons g Nil))))))))
"#;

// Levenshtein with memoization - polynomial time
const LEV_MEMO: &str = r#"
!(bind! &lev-cache (new-memo "lev-cache"))

(= (min3 $a $b $c)
   (if (< $a $b)
       (if (< $a $c) $a $c)
       (if (< $b $c) $b $c)))

(= (lev Nil Nil) 0)
(= (lev Nil (Cons $_ $ys)) (+ 1 (lev Nil $ys)))
(= (lev (Cons $_ $xs) Nil) (+ 1 (lev $xs Nil)))
(= (lev (Cons $x $xs) (Cons $y $ys))
   (memo-first &lev-cache
     (if (== $x $y)
         (lev $xs $ys)
         (+ 1 (min3
                (lev $xs (Cons $y $ys))
                (lev (Cons $x $xs) $ys)
                (lev $xs $ys))))))

; kitten -> sitting (distance 3)
!(lev (Cons k (Cons i (Cons t (Cons t (Cons e (Cons n Nil))))))
      (Cons s (Cons i (Cons t (Cons t (Cons i (Cons n (Cons g Nil))))))))

; saturday -> sunday (distance 3)
!(lev (Cons s (Cons a (Cons t (Cons u (Cons r (Cons d (Cons a (Cons y Nil))))))))
      (Cons s (Cons u (Cons n (Cons d (Cons a (Cons y Nil)))))))
"#;

// Ensure EvalConfig is only configured once
static INIT: Once = Once::new();

fn init_config() {
    INIT.call_once(|| {
        configure_eval(EvalConfig::cpu_optimized());
    });
}

/// Run a complete MeTTa program
fn run_program(src: &str) {
    let state = MettaState::new_empty();
    let program = compile(src).expect("Failed to compile program");
    let result = run_state(state, program).expect("Failed to run program");
    black_box(result);
}

/// Benchmark Fibonacci without memoization
fn bench_fib_no_memo(c: &mut Criterion) {
    init_config();

    let mut group = c.benchmark_group("fibonacci");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20); // Fewer samples since it's slow

    group.bench_function("fib_20_no_memo", |b| {
        b.iter(|| run_program(FIB_NO_MEMO));
    });

    group.finish();
}

/// Benchmark Fibonacci with memoization
fn bench_fib_memo(c: &mut Criterion) {
    init_config();

    let mut group = c.benchmark_group("fibonacci");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    group.bench_function("fib_50_with_memo", |b| {
        b.iter(|| run_program(FIB_MEMO));
    });

    group.finish();
}

/// Benchmark Levenshtein without memoization
fn bench_lev_no_memo(c: &mut Criterion) {
    init_config();

    let mut group = c.benchmark_group("levenshtein");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20); // Fewer samples since it's slow

    group.bench_function("lev_kitten_sitting_no_memo", |b| {
        b.iter(|| run_program(LEV_NO_MEMO));
    });

    group.finish();
}

/// Benchmark Levenshtein with memoization
fn bench_lev_memo(c: &mut Criterion) {
    init_config();

    let mut group = c.benchmark_group("levenshtein");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    group.bench_function("lev_multiple_with_memo", |b| {
        b.iter(|| run_program(LEV_MEMO));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fib_no_memo,
    bench_fib_memo,
    bench_lev_no_memo,
    bench_lev_memo,
);
criterion_main!(benches);
