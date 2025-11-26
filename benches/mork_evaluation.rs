use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;
use mettatron::backend::eval::fixed_point::eval_env_to_fixed_point;

/// Benchmark: Simple parent-child derivation (ancestor.mm2 style)
/// Measures basic fixed-point evaluation with one rule
fn bench_simple_derivation(c: &mut Criterion) {
    let mut group = c.benchmark_group("mork_simple_derivation");

    for fact_count in [10, 50, 100, 500].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(fact_count),
            fact_count,
            |b, &count| {
                b.iter(|| {
                    let mut env = Environment::new();

                    // Add parent facts
                    for i in 0..count {
                        let fact = format!("(parent person{} person{})", i, i + 1);
                        env.add_to_space(&compile(&fact).unwrap().source[0]);
                    }

                    // Add derivation rule: parent → child
                    let rule = compile(
                        "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))"
                    ).unwrap();
                    env.add_to_space(&rule.source[0]);

                    // Run to fixed point
                    let (_, result) = eval_env_to_fixed_point(env, 100);
                    black_box(result)
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
        group.bench_with_input(
            BenchmarkId::from_parameter(depth),
            depth,
            |b, &d| {
                b.iter(|| {
                    let mut env = Environment::new();

                    // Create linear family tree
                    for i in 0..d {
                        let fact = format!("(parent person{} person{})", i, i + 1);
                        env.add_to_space(&compile(&fact).unwrap().source[0]);
                    }

                    // Point of interest (bottom of tree)
                    let poi_fact = format!("(poi person{})", d);
                    env.add_to_space(&compile(&poi_fact).unwrap().source[0]);

                    // Rule 1: parent → child
                    let rule1 = compile(
                        "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))"
                    ).unwrap();
                    env.add_to_space(&rule1.source[0]);

                    // Rule 2: poi + child → generation Z
                    let rule2 = compile(
                        "(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))"
                    ).unwrap();
                    env.add_to_space(&rule2.source[0]);

                    // Rule 3: Meta-rule for generation tracking
                    let rule3 = compile(
                        "(exec (1 Z) (, (generation Z $c $p) (child $p $gp))
                                    (, (generation (S Z) $c $gp)))"
                    ).unwrap();
                    env.add_to_space(&rule3.source[0]);

                    // Run to fixed point
                    let (_, result) = eval_env_to_fixed_point(env, 100);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Full ancestor.mm2 family tree
/// Measures complete MORK evaluation with all features
fn bench_full_ancestor_mm2(c: &mut Criterion) {
    c.bench_function("mork_full_ancestor_mm2", |b| {
        b.iter(|| {
            let mut env = Environment::new();

            // Family tree (12 parent relationships)
            let parents = [
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
            ];

            for parent in &parents {
                env.add_to_space(&compile(parent).unwrap().source[0]);
            }

            // Gender facts (15 facts)
            let genders = [
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
            ];

            for gender in &genders {
                env.add_to_space(&compile(gender).unwrap().source[0]);
            }

            // Points of interest
            env.add_to_space(&compile("(poi Ann)").unwrap().source[0]);
            env.add_to_space(&compile("(poi Vic)").unwrap().source[0]);

            // All 4 exec rules from ancestor.mm2
            let rule1 = compile(
                "(exec (0 0) (, (parent $p $c)) (, (child $c $p)))"
            ).unwrap();
            env.add_to_space(&rule1.source[0]);

            let rule2 = compile(
                "(exec (0 1) (, (poi $c) (child $c $p)) (, (generation Z $c $p)))"
            ).unwrap();
            env.add_to_space(&rule2.source[0]);

            let rule3 = compile(
                "(exec (1 Z) (, (exec (1 $l) $ps $ts)
                               (generation $l $c $p)
                               (child $p $gp))
                            (, (exec (1 (S $l)) $ps $ts)
                               (generation (S $l) $c $gp)))"
            ).unwrap();
            env.add_to_space(&rule3.source[0]);

            let rule4 = compile(
                "(exec (2 0) (, (generation $_ $p $a)) (, (ancestor $p $a)))"
            ).unwrap();
            env.add_to_space(&rule4.source[0]);

            // Run to fixed point
            let (_, result) = eval_env_to_fixed_point(env, 50);
            black_box(result)
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
                b.iter(|| {
                    let mut env = Environment::new();

                    // Add initial facts
                    for i in 0..count {
                        let fact = format!("(temp-fact {})", i);
                        env.add_to_space(&compile(&fact).unwrap().source[0]);
                    }

                    // Add rule that removes facts
                    for i in 0..count {
                        let rule = format!(
                            "(exec ({} 0) (, (temp-fact {})) (O (- (temp-fact {}))))",
                            i, i, i
                        );
                        env.add_to_space(&compile(&rule).unwrap().source[0]);
                    }

                    // Run to fixed point (should remove all temp facts)
                    let (_, result) = eval_env_to_fixed_point(env, 100);
                    black_box(result)
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
        b.iter(|| {
            let mut env = Environment::new();

            // Add facts
            env.add_to_space(&compile("(trigger A)").unwrap().source[0]);

            // Add rules with different priority types (should execute in order)
            // Integer priorities
            let rule1 = compile("(exec 0 (, (trigger A)) (, (result-0)))").unwrap();
            env.add_to_space(&rule1.source[0]);

            let rule2 = compile("(exec 1 (, (result-0)) (, (result-1)))").unwrap();
            env.add_to_space(&rule2.source[0]);

            // Peano priorities
            let rule3 = compile("(exec (S Z) (, (result-1)) (, (result-sz)))").unwrap();
            env.add_to_space(&rule3.source[0]);

            let rule4 = compile("(exec (S (S Z)) (, (result-sz)) (, (result-ssz)))").unwrap();
            env.add_to_space(&rule4.source[0]);

            // Tuple priorities
            let rule5 = compile("(exec (2 0) (, (result-ssz)) (, (result-20)))").unwrap();
            env.add_to_space(&rule5.source[0]);

            let rule6 = compile("(exec (2 1) (, (result-20)) (, (result-21)))").unwrap();
            env.add_to_space(&rule6.source[0]);

            // Mixed tuple/Peano
            let rule7 = compile("(exec (3 Z) (, (result-21)) (, (result-3z)))").unwrap();
            env.add_to_space(&rule7.source[0]);

            let rule8 = compile("(exec (3 (S Z)) (, (result-3z)) (, (result-final)))").unwrap();
            env.add_to_space(&rule8.source[0]);

            // Run to fixed point
            let (_, result) = eval_env_to_fixed_point(env, 50);
            black_box(result)
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
                b.iter(|| {
                    let mut env = Environment::new();

                    // Add chain of facts
                    for i in 0..count {
                        let fact = format!("(link {} {})", i, i + 1);
                        env.add_to_space(&compile(&fact).unwrap().source[0]);
                    }

                    // Build conjunction with N goals
                    let mut antecedent = String::from("(,");
                    for i in 0..count {
                        antecedent.push_str(&format!(" (link {} $v{})", i, i + 1));
                    }
                    antecedent.push(')');

                    // Build consequent using last variable
                    let consequent = format!("(, (final-result $v{}))", count);

                    let rule = format!("(exec (0 0) {} {})", antecedent, consequent);
                    env.add_to_space(&compile(&rule).unwrap().source[0]);

                    // Run to fixed point
                    let (_, result) = eval_env_to_fixed_point(env, 50);
                    black_box(result)
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
                b.iter(|| {
                    let mut env = Environment::new();

                    // Add base fact
                    env.add_to_space(&compile("(counter 0)").unwrap().source[0]);

                    // Add rule that increments counter (will run 'depth' times)
                    for i in 0..depth {
                        let rule = format!(
                            "(exec ({} 0) (, (counter {})) (, (counter {})))",
                            i,
                            i,
                            i + 1
                        );
                        env.add_to_space(&compile(&rule).unwrap().source[0]);
                    }

                    // Run to fixed point
                    let (_, result) = eval_env_to_fixed_point(env, depth + 10);
                    black_box(result)
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
        group.bench_with_input(
            BenchmarkId::from_parameter(nesting),
            nesting,
            |b, &n| {
                b.iter(|| {
                    let mut env = Environment::new();

                    // Create nested structure
                    let mut nested = String::from("value");
                    for _ in 0..n {
                        nested = format!("(nested {})", nested);
                    }

                    // Add fact
                    env.add_to_space(&compile(&nested).unwrap().source[0]);

                    // Create pattern that matches
                    let mut pattern = String::from("$v");
                    for _ in 0..n {
                        pattern = format!("(nested {})", pattern);
                    }

                    let rule = format!(
                        "(exec (0 0) (, {}) (, (matched $v)))",
                        pattern
                    );
                    env.add_to_space(&compile(&rule).unwrap().source[0]);

                    // Run to fixed point
                    let (_, result) = eval_env_to_fixed_point(env, 10);
                    black_box(result)
                });
            },
        );
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
