use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile_arena;
use mettatron::backend::eval::eval_arena;
use mettatron::backend::eval::trampoline::new_arena_env;

/// Generate N fibonacci rules for benchmarking
fn generate_fibonacci_rules(n: usize) -> String {
    let mut rules = String::new();

    // Base cases
    rules.push_str("(= (fibonacci 0) 0)\n");
    rules.push_str("(= (fibonacci 1) 1)\n");

    // Generate N-2 additional dummy rules that won't match
    for i in 2..n {
        rules.push_str(&format!("(= (dummy-rule-{} $x) $x)\n", i));
    }

    // Real recursive rule at the end (worst case - must scan all rules)
    rules.push_str("(= (fibonacci $n) (+ (fibonacci (- $n 1)) (fibonacci (- $n 2))))\n");

    rules
}

/// Generate N simple pattern matching rules
fn generate_pattern_rules(n: usize) -> String {
    let mut rules = String::new();

    for i in 0..n {
        rules.push_str(&format!("(= (pattern-{} $x) (result-{} $x))\n", i, i));
    }

    rules
}

/// Benchmark rule matching with varying rule counts
fn bench_rule_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_matching");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        let rules_src = generate_fibonacci_rules(*rule_count);
        let query_src = "!(fibonacci 5)";

        // Combine rules + query into a single program
        let full_program = format!("{}\n{}", rules_src, query_src);

        group.bench_with_input(
            BenchmarkId::new("fibonacci_lookup", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let state =
                        compile_arena(&full_program).expect("Failed to compile program");
                    let mut env = new_arena_env();

                    for &expr in state.source() {
                        let (_, new_env) = eval_arena(black_box(expr), env, black_box(&state));
                        env = new_env;
                    }
                    black_box(env)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark pattern matching with different pattern complexities
/// FIXED: Share environment across iterations to measure query performance, not insertion overhead
fn bench_pattern_complexity(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_matching");

    // Simple pattern: (pattern $x)
    let simple_setup = "(= (simple $x) $x)";
    let simple_query = "!(simple 42)";

    // Pre-compile rule and load it into env
    let simple_rule_state = compile_arena(simple_setup).expect("Failed to compile");
    let mut simple_env = new_arena_env();
    for &expr in simple_rule_state.source() {
        let (_, new_env) = eval_arena(expr, simple_env, &simple_rule_state);
        simple_env = new_env;
    }

    // Pre-compile query
    let simple_query_state = compile_arena(simple_query).expect("Failed to compile");

    group.bench_function("simple_variable", |b| {
        b.iter(|| {
            // Only measure query performance, not compilation or rule insertion
            let (result, _) = eval_arena(
                black_box(simple_query_state.source()[0]),
                simple_env.clone(),
                black_box(&simple_query_state),
            );
            black_box(result)
        });
    });

    // Nested pattern: (pattern ($a ($b $c)))
    let nested_setup = "(= (nested ($a ($b $c))) (result $a $b $c))";
    let nested_query = "!(nested (1 (2 3)))";

    let nested_rule_state = compile_arena(nested_setup).expect("Failed to compile");
    let mut nested_env = new_arena_env();
    for &expr in nested_rule_state.source() {
        let (_, new_env) = eval_arena(expr, nested_env, &nested_rule_state);
        nested_env = new_env;
    }

    let nested_query_state = compile_arena(nested_query).expect("Failed to compile");

    group.bench_function("nested_destructuring", |b| {
        b.iter(|| {
            let (result, _) = eval_arena(
                black_box(nested_query_state.source()[0]),
                nested_env.clone(),
                black_box(&nested_query_state),
            );
            black_box(result)
        });
    });

    // Multiple arguments: (pattern $a $b $c $d)
    let multi_arg_setup = "(= (multi $a $b $c $d) (+ (+ $a $b) (+ $c $d)))";
    let multi_arg_query = "!(multi 1 2 3 4)";

    let multi_rule_state = compile_arena(multi_arg_setup).expect("Failed to compile");
    let mut multi_env = new_arena_env();
    for &expr in multi_rule_state.source() {
        let (_, new_env) = eval_arena(expr, multi_env, &multi_rule_state);
        multi_env = new_env;
    }

    let multi_query_state = compile_arena(multi_arg_query).expect("Failed to compile");

    group.bench_function("multi_argument", |b| {
        b.iter(|| {
            let (result, _) = eval_arena(
                black_box(multi_query_state.source()[0]),
                multi_env.clone(),
                black_box(&multi_query_state),
            );
            black_box(result)
        });
    });

    group.finish();
}

/// Benchmark full evaluation of representative programs
fn bench_full_evaluation(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_evaluation");

    // Fibonacci with evaluation
    let fib_program = r#"
        (= (fibonacci 0) 0)
        (= (fibonacci 1) 1)
        (= (fibonacci $n) (+ (fibonacci (- $n 1)) (fibonacci (- $n 2))))
        !(fibonacci 10)
    "#;

    group.bench_function("fibonacci_10", |b| {
        b.iter(|| {
            let state = compile_arena(fib_program).expect("Failed to compile");
            let mut env = new_arena_env();

            for &expr in state.source() {
                let (_, new_env) = eval_arena(black_box(expr), env, &state);
                env = new_env;
            }
            black_box(env)
        });
    });

    // Nested let bindings
    let let_program = r#"
        (let $x 10
            (let $y 20
                (let $z 30
                    (+ (+ $x $y) $z))))
    "#;

    group.bench_function("nested_let", |b| {
        b.iter(|| {
            let state = compile_arena(let_program).expect("Failed to compile");
            let env = new_arena_env();
            let (result, _) = eval_arena(black_box(state.source()[0]), env, &state);
            black_box(result)
        });
    });

    // Type inference
    let type_program = r#"
        (: 42 Long)
        (: "hello" String)
        (: true Bool)
        (get-type 42)
    "#;

    group.bench_function("type_inference", |b| {
        b.iter(|| {
            let state = compile_arena(type_program).expect("Failed to compile");
            let mut env = new_arena_env();

            for &expr in state.source() {
                let (_, new_env) = eval_arena(black_box(expr), env, &state);
                env = new_env;
            }
            black_box(env)
        });
    });

    group.finish();
}

/// Benchmark with many rules to stress-test rule iteration
fn bench_large_rule_sets(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_rule_sets");
    group.sample_size(10); // Reduce sample size for slow benchmarks

    for rule_count in [100, 500, 1000].iter() {
        let rules_src = generate_pattern_rules(*rule_count);
        let query_src = format!("!(pattern-{} 42)", rule_count - 1); // Query last rule (worst case)

        // Combine rules + query into a single program
        let full_program = format!("{}\n{}", rules_src, query_src);

        group.bench_with_input(
            BenchmarkId::new("worst_case_lookup", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let state =
                        compile_arena(&full_program).expect("Failed to compile program");
                    let mut env = new_arena_env();

                    for &expr in state.source() {
                        let (_, new_env) = eval_arena(black_box(expr), env, black_box(&state));
                        env = new_env;
                    }
                    black_box(env)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark has_sexpr_fact() with varying fact counts
/// NOTE: has_sexpr_fact is a HeapEnvironment method, so we still use the heap
/// compile/eval to populate the environment, then benchmark the lookup.
fn bench_has_sexpr_fact(c: &mut Criterion) {
    use mettatron::backend::environment::HeapEnvironment;
    use mettatron::backend::models::MettaValue;

    let mut group = c.benchmark_group("has_sexpr_fact");
    group.sample_size(50); // Increase sample size for more stable results

    for fact_count in [100, 500, 1000, 5000].iter() {
        // Pre-populate environment with facts using HeapEnvironment directly
        let mut env = HeapEnvironment::default();

        // Add facts to the Space via add_to_space
        for i in 0..*fact_count {
            let fact = MettaValue::SExpr(vec![
                MettaValue::Atom(format!("fact-{}", i)),
                MettaValue::Atom(format!("value-{}", i)),
            ]);
            env.add_to_space(&fact);
        }

        // Query for a fact in the middle (typical case)
        let query_idx = fact_count / 2;
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom(format!("fact-{}", query_idx)),
            MettaValue::Atom(format!("value-{}", query_idx)),
        ]);

        group.bench_with_input(
            BenchmarkId::new("query_existing_fact", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let result = env.has_sexpr_fact(black_box(&query));
                    black_box(result)
                });
            },
        );

        // Query for a non-existent fact (worst case for linear search)
        let missing_query = MettaValue::SExpr(vec![
            MettaValue::Atom("nonexistent-fact".to_string()),
            MettaValue::Atom("missing-value".to_string()),
        ]);

        group.bench_with_input(
            BenchmarkId::new("query_missing_fact", fact_count),
            fact_count,
            |b, _| {
                b.iter(|| {
                    let result = env.has_sexpr_fact(black_box(&missing_query));
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_rule_matching,
    bench_pattern_complexity,
    bench_full_evaluation,
    bench_large_rule_sets,
    bench_has_sexpr_fact
);
criterion_main!(benches);
