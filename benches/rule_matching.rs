use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;

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
        let query_src = "(fibonacci 5)";

        group.bench_with_input(
            BenchmarkId::new("fibonacci_lookup", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let env = Environment::new();

                    // Load rules
                    for rule_str in rules_src.lines() {
                        let rule_state = compile(rule_str).expect("Failed to compile rule");
                        for rule_expr in rule_state.source {
                            eval(black_box(rule_expr), env.clone());
                        }
                    }

                    // Execute query
                    let query_state = compile(query_src).expect("Failed to compile query");
                    let query = query_state.source.into_iter().next().expect("No query");
                    let result = eval(black_box(query), env);
                    black_box(result)
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
    let simple_rule = "(= (simple $x) $x)";
    let simple_query = "(simple 42)";

    // Pre-compile rule and query once, share environment
    let simple_env = {
        let env = Environment::new();
        let rule_state = compile(simple_rule).expect("Failed to compile");
        let rule = rule_state.source.into_iter().next().expect("No rule");
        eval(rule, env.clone());
        env
    };
    let simple_query_compiled = {
        let query_state = compile(simple_query).expect("Failed to compile");
        query_state.source.into_iter().next().expect("No query")
    };

    group.bench_function("simple_variable", |b| {
        b.iter(|| {
            // Only measure query performance, not compilation or rule insertion
            let result = eval(black_box(simple_query_compiled.clone()), simple_env.clone());
            black_box(result)
        });
    });

    // Nested pattern: (pattern ($a ($b $c)))
    let nested_rule = "(= (nested ($a ($b $c))) (result $a $b $c))";
    let nested_query = "(nested (1 (2 3)))";

    let nested_env = {
        let env = Environment::new();
        let rule_state = compile(nested_rule).expect("Failed to compile");
        let rule = rule_state.source.into_iter().next().expect("No rule");
        eval(rule, env.clone());
        env
    };
    let nested_query_compiled = {
        let query_state = compile(nested_query).expect("Failed to compile");
        query_state.source.into_iter().next().expect("No query")
    };

    group.bench_function("nested_destructuring", |b| {
        b.iter(|| {
            let result = eval(black_box(nested_query_compiled.clone()), nested_env.clone());
            black_box(result)
        });
    });

    // Multiple arguments: (pattern $a $b $c $d)
    let multi_arg_rule = "(= (multi $a $b $c $d) (+ (+ $a $b) (+ $c $d)))";
    let multi_arg_query = "(multi 1 2 3 4)";

    let multi_env = {
        let env = Environment::new();
        let rule_state = compile(multi_arg_rule).expect("Failed to compile");
        let rule = rule_state.source.into_iter().next().expect("No rule");
        eval(rule, env.clone());
        env
    };
    let multi_query_compiled = {
        let query_state = compile(multi_arg_query).expect("Failed to compile");
        query_state.source.into_iter().next().expect("No query")
    };

    group.bench_function("multi_argument", |b| {
        b.iter(|| {
            let result = eval(black_box(multi_query_compiled.clone()), multi_env.clone());
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
            let env = Environment::new();
            let lines: Vec<&str> = fib_program
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();

            for line in lines {
                let state = compile(line).expect("Failed to compile");
                for expr in state.source {
                    let _ = eval(black_box(expr), env.clone());
                }
            }
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
            let env = Environment::new();
            let state = compile(let_program).expect("Failed to compile");
            let expr = state.source.into_iter().next().expect("No expr");
            let result = eval(black_box(expr), env);
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
            let env = Environment::new();
            let lines: Vec<&str> = type_program
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();

            for line in lines {
                let state = compile(line).expect("Failed to compile");
                for expr in state.source {
                    let _ = eval(black_box(expr), env.clone());
                }
            }
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
        let query_src = format!("(pattern-{} 42)", rule_count - 1); // Query last rule (worst case)

        group.bench_with_input(
            BenchmarkId::new("worst_case_lookup", rule_count),
            rule_count,
            |b, _| {
                b.iter(|| {
                    let env = Environment::new();

                    // Load all rules
                    for rule_str in rules_src.lines() {
                        let rule_state = compile(rule_str).expect("Failed to compile rule");
                        for rule_expr in rule_state.source {
                            eval(black_box(rule_expr), env.clone());
                        }
                    }

                    // Query the last rule (must iterate through all)
                    let query_state = compile(&query_src).expect("Failed to compile query");
                    let query = query_state.source.into_iter().next().expect("No query");
                    let result = eval(black_box(query), env);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark has_sexpr_fact() with varying fact counts
fn bench_has_sexpr_fact(c: &mut Criterion) {
    let mut group = c.benchmark_group("has_sexpr_fact");
    group.sample_size(50); // Increase sample size for more stable results

    for fact_count in [100, 500, 1000, 5000].iter() {
        // Pre-populate environment with facts
        let env = Environment::new();

        // Add facts to the Space
        for i in 0..*fact_count {
            let fact_src = format!("!(add-atom &space (fact-{} value-{}))", i, i);
            let fact_state = compile(&fact_src).expect("Failed to compile fact");
            for fact_expr in fact_state.source {
                eval(fact_expr, env.clone());
            }
        }

        // Query for a fact in the middle (typical case)
        let query_idx = fact_count / 2;
        let query_src = format!("(fact-{} value-{})", query_idx, query_idx);
        let query_state = compile(&query_src).expect("Failed to compile query");
        let query = query_state.source.into_iter().next().expect("No query");

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
        let missing_query_src = "(nonexistent-fact missing-value)";
        let missing_state = compile(missing_query_src).expect("Failed to compile");
        let missing_query = missing_state.source.into_iter().next().expect("No query");

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
