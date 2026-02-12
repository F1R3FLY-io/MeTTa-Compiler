use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile;
use mettatron::backend::eval::eval;
use mettatron::backend::eval::trampoline::new_env;

/// Run a MeTTa program consisting of facts and exec rules through fixed-point evaluation.
///
/// Builds a single MeTTa source string, compiles it, and evaluates each expression.
/// Then repeatedly re-evaluates the exec rules until no new facts are derived or
/// the iteration limit is reached.
///
/// `facts`: list of MeTTa fact expressions (e.g., "(parent Tom Bob)")
/// `rules`: list of MeTTa exec rule expressions (e.g., "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))")
/// `max_iterations`: maximum number of fixed-point iterations
fn eval_mork_to_fixed_point(facts: &[&str], rules: &[&str], max_iterations: usize) {
    // Build full program: facts first, then exec rules with evaluation
    let mut src = String::new();
    for fact in facts {
        src.push_str(fact);
        src.push('\n');
    }
    for rule in rules {
        // Evaluate each exec rule (this fires it against current facts)
        src.push_str(&format!("!{}\n", rule));
    }

    let state = compile(&src).expect("Failed to compile MORK program");
    let mut env = new_env();

    // First pass: evaluate all expressions (facts + exec rules)
    let source_exprs: Vec<_> = state.source().iter().copied().collect();
    for expr in source_exprs {
        let (_, new_env) = eval(expr, env, &state);
        env = new_env;
    }

    // Fixed-point loop: re-evaluate exec rules until convergence
    if !rules.is_empty() {
        // Compile just the exec rule evaluations for repeated application
        let rules_src: String = rules.iter().map(|r| format!("!{}\n", r)).collect();
        let rules_state = compile(&rules_src).expect("Failed to compile rules");

        for _ in 1..max_iterations {
            let prev_env = env.clone();

            let rules_exprs: Vec<_> = rules_state.source().iter().copied().collect();
            for expr in rules_exprs {
                let (_, new_env) = eval(expr, env, &rules_state);
                env = new_env;
            }

            // Check convergence: if environment didn't change, we've reached fixed point
            if format!("{:?}", env) == format!("{:?}", prev_env) {
                break;
            }
        }
    }
}

/// Benchmark: Simple parent-child derivation (ancestor.mm2 style)
/// Measures basic fixed-point evaluation with one rule
fn bench_simple_derivation(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_simple_derivation");

    for fact_count in [10, 50, 100, 500].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(fact_count),
            fact_count,
            |b, &count| {
                // Pre-build fact and rule lists
                let facts: Vec<String> = (0..count)
                    .map(|i| format!("(parent person{} person{})", i, i + 1))
                    .collect();
                let fact_refs: Vec<&str> = facts.iter().map(|s| s.as_str()).collect();
                let rules = ["(exec (0 0) (, (parent $p $c)) (, (child $c $p)))"];

                b.iter(|| {
                    eval_mork_to_fixed_point(&fact_refs, &rules, 100);
                    black_box(())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Multi-generation tracking (ancestor.mm2 pattern)
/// Measures meta-programming with dynamic exec generation
fn bench_multi_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_multi_generation");

    for depth in [3, 5, 10].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(depth), depth, |b, &d| {
            // Pre-build facts
            let mut facts: Vec<String> = (0..d)
                .map(|i| format!("(parent person{} person{})", i, i + 1))
                .collect();
            facts.push(format!("(poi person{})", d));
            let fact_refs: Vec<&str> = facts.iter().map(|s| s.as_str()).collect();

            let rules = [
                "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))",
                "(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))",
                "(exec (1 Z) (, (generation Z $c $p) (child $p $gp)) (, (generation (S Z) $c $gp)))",
            ];

            b.iter(|| {
                eval_mork_to_fixed_point(&fact_refs, &rules, 100);
                black_box(())
            });
        });
    }

    group.finish();
}

/// Benchmark: Full ancestor.mm2 family tree
/// Measures complete MORK evaluation with all features
fn bench_full_ancestor_mm2(c: &mut Criterion) {
    c.bench_function("mork_full_ancestor_mm2", |b| {
        let facts: &[&str] = &[
            "(parent Tom Bob)",
            "(parent Pam Bob)",
            "(parent Tom Liz)",
            "(parent Bob Ann)",
            "(parent Bob Pat)",
            "(parent Pat Jim)",
            "(parent Xey Uru)",
            "(parent Yip Uru)",
            "(parent Zac Vic)",
            "(parent Whu Vic)",
            "(parent Uru Ohm)",
            "(parent Vic Ohm)",
            "(female Pam)",
            "(female Liz)",
            "(female Pat)",
            "(female Ann)",
            "(female Vic)",
            "(female Yip)",
            "(female Whu)",
            "(male Tom)",
            "(male Bob)",
            "(male Jim)",
            "(male Uru)",
            "(male Xey)",
            "(male Zac)",
            "(other Ohm)",
            "(poi Ann)",
            "(poi Vic)",
        ];

        let rules: &[&str] = &[
            "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))",
            "(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))",
            "(exec (1 Z) (, (exec (1 $l) $ps $ts) (generation $l $c $p) (child $p $gp)) (, (exec (1 (S $l)) $ps $ts) (generation (S $l) $c $gp)))",
            "(exec (2 0) (, (generation $_ $p $a)) (, (ancestor $p $a)))",
        ];

        b.iter(|| {
            eval_mork_to_fixed_point(facts, rules, 50);
            black_box(())
        });
    });
}

/// Benchmark: Operation forms (fact addition/removal)
/// Measures operation execution performance
fn bench_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_operations");

    for op_count in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(op_count),
            op_count,
            |b, &count| {
                let facts: Vec<String> = (0..count)
                    .map(|i| format!("(temp-fact {})", i))
                    .collect();
                let fact_refs: Vec<&str> = facts.iter().map(|s| s.as_str()).collect();

                let rules: Vec<String> = (0..count)
                    .map(|i| {
                        format!(
                            "(exec ({} 0) (, (temp-fact {})) (O (- (temp-fact {}))))",
                            i, i, i
                        )
                    })
                    .collect();
                let rule_refs: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();

                b.iter(|| {
                    eval_mork_to_fixed_point(&fact_refs, &rule_refs, 100);
                    black_box(())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Priority ordering with mixed types
/// Measures priority comparison overhead
fn bench_priority_ordering(c: &mut Criterion) {
    c.bench_function("mork_priority_ordering", |b| {
        let facts: &[&str] = &["(trigger A)"];

        let rules: &[&str] = &[
            "(exec 0 (, (trigger A)) (, (result-0)))",
            "(exec 1 (, (result-0)) (, (result-1)))",
            "(exec (S Z) (, (result-1)) (, (result-sz)))",
            "(exec (S (S Z)) (, (result-sz)) (, (result-ssz)))",
            "(exec (2 0) (, (result-ssz)) (, (result-20)))",
            "(exec (2 1) (, (result-20)) (, (result-21)))",
            "(exec (3 Z) (, (result-21)) (, (result-3z)))",
            "(exec (3 (S Z)) (, (result-3z)) (, (result-final)))",
        ];

        b.iter(|| {
            eval_mork_to_fixed_point(facts, rules, 50);
            black_box(())
        });
    });
}

/// Benchmark: Conjunction pattern matching with varying goal counts
/// Measures binding threading performance
fn bench_conjunction_goals(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_conjunction_goals");

    for goal_count in [2, 4, 6, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(goal_count),
            goal_count,
            |b, &count| {
                // Pre-build facts
                let facts: Vec<String> = (0..count)
                    .map(|i| format!("(link {} {})", i, i + 1))
                    .collect();
                let fact_refs: Vec<&str> = facts.iter().map(|s| s.as_str()).collect();

                // Build conjunction with N goals
                let mut antecedent = String::from("(,");
                for i in 0..count {
                    antecedent.push_str(&format!(" (link {} $v{})", i, i + 1));
                }
                antecedent.push(')');

                let consequent = format!("(, (final-result $v{}))", count);
                let rule = format!("(exec (0 0) {} {})", antecedent, consequent);
                let rules = [rule.as_str()];

                b.iter(|| {
                    eval_mork_to_fixed_point(&fact_refs, &rules, 50);
                    black_box(())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Fixed-point convergence with varying iteration counts
/// Measures iteration overhead and convergence detection
fn bench_convergence(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_convergence");

    for max_depth in [5, 10, 20, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(max_depth),
            max_depth,
            |b, &depth| {
                let facts = ["(counter 0)"];

                let rules: Vec<String> = (0..depth)
                    .map(|i| {
                        format!(
                            "(exec ({} 0) (, (counter {})) (, (counter {})))",
                            i,
                            i,
                            i + 1
                        )
                    })
                    .collect();
                let rule_refs: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();

                b.iter(|| {
                    eval_mork_to_fixed_point(&facts, &rule_refs, depth + 10);
                    black_box(())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Pattern matching complexity
/// Measures unification and binding overhead
fn bench_pattern_complexity(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_pattern_complexity");

    for nesting in [1, 2, 3, 4].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(nesting), nesting, |b, &n| {
            // Create nested structure
            let mut nested = String::from("value");
            for _ in 0..n {
                nested = format!("(nested {})", nested);
            }
            let facts = [nested.as_str()];

            // Create pattern that matches
            let mut pattern = String::from("$v");
            for _ in 0..n {
                pattern = format!("(nested {})", pattern);
            }
            let rule = format!("(exec (0 0) (, {}) (, (matched $v)))", pattern);
            let rules = [rule.as_str()];

            b.iter(|| {
                eval_mork_to_fixed_point(&facts, &rules, 10);
                black_box(())
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_simple_derivation,
    bench_multi_generation,
    bench_full_ancestor_mm2,
    bench_operations,
    bench_priority_ordering,
    bench_conjunction_goals,
    bench_convergence,
    bench_pattern_complexity,
);
criterion_main!(benches);
