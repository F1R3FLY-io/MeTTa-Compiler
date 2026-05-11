// Type System Benchmark Suite
//
// Two-tier benchmarks:
//   Tier 1 — Micro: Direct function calls measuring isolated function latency/scaling
//   Tier 2 — End-to-end: MeTTa programs via compile() + run_state() exercising real type system paths
//
// 9 groups, ~80 benchmarks covering:
//   1. type_inference     — Core inference engine
//   2. type_matching      — Structural type matching
//   3. type_allocation    — Allocation-heavy operations (freshen, apply)
//   4. subtype_hierarchy  — Subtype graph operations (BFS closure)
//   5. env_type_ops       — Environment type storage (bloom, DashMap)
//   6. applicative_pre_eval — Type-driven eval decisions
//   7. type_special_forms — End-to-end via eval()
//   8. type_fixpoint      — Fixpoint inference
//   9. type_o1_checks     — O(1) lightweight checks

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use std::collections::HashMap;
use std::time::Duration;

use mettatron::backend::compile::compile;
use mettatron::backend::eval::trampoline::{get_static_factory, new_env};
use mettatron::backend::eval::{
    apply_type_bindings, find_grounded_arg_indices_generic, find_typed_arg_indices_generic,
    freshen_type_variables, get_ground_type, infer_types_generic, is_arrow_type,
    is_declared_value_type, is_meta_type, is_pattern_type_compatible, match_types_with_bindings,
    run_type_fixpoint, types_match_generic, types_match_with_subtypes,
};
use mettatron::backend::models::MettaState;
use mettatron::backend::{get_signature, is_builtin, MettaValue, MettaValueFactory};
use mettatron::rholang_integration::run_state;

// =============================================================================
// Helpers
// =============================================================================

/// Build an arrow type: (-> T1 T2 ... Tret)
fn make_arrow(f: &impl MettaValueFactory<MettaValue>, params: &[&str], ret: &str) -> MettaValue {
    let mut items = Vec::with_capacity(params.len() + 2);
    items.push(f.atom("->"));
    for p in params {
        items.push(f.atom(p));
    }
    items.push(f.atom(ret));
    f.sexpr(items)
}

/// Set up a linear subtype chain: T0 <: T1 <: ... <: TN in the given environment
fn setup_subtype_chain(env: &mut mettatron::backend::environment::MettaEnvironment, depth: usize) {
    for i in 0..depth {
        let sub = format!("T{}", i);
        let sup = format!("T{}", i + 1);
        env.add_subtype_generic(&sub, &sup);
    }
}

/// Compile and evaluate a MeTTa source string, returning result strings
fn run_program(src: &str) -> Vec<String> {
    let state = compile(src).expect("Failed to compile");
    let env = new_env();
    let result = run_state(MettaState::from_env(env), &state).expect("Failed to evaluate");
    let strings: Vec<String> = result.output().iter().map(|v| format!("{}", v)).collect();
    strings
}

// =============================================================================
// Group 1: type_inference — Core Inference Engine
// =============================================================================

fn bench_type_inference(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_inference");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let f = get_static_factory();

    // infer_ground_types: Bool, Long, Float, String
    group.bench_function("infer_ground_bool", |b| {
        let env = new_env();
        let val = f.bool(true);
        b.iter(|| black_box(infer_types_generic(&val, &f, &env)));
    });

    group.bench_function("infer_ground_long", |b| {
        let env = new_env();
        let val = f.long(42);
        b.iter(|| black_box(infer_types_generic(&val, &f, &env)));
    });

    group.bench_function("infer_ground_float", |b| {
        let env = new_env();
        let val = f.float(3.14);
        b.iter(|| black_box(infer_types_generic(&val, &f, &env)));
    });

    group.bench_function("infer_ground_string", |b| {
        let env = new_env();
        let val = f.string("hello");
        b.iter(|| black_box(infer_types_generic(&val, &f, &env)));
    });

    // infer_atom_N_types: Atom with N declared types via add_type_generic
    for n in [1, 5, 10, 50] {
        group.bench_with_input(BenchmarkId::new("infer_atom_N_types", n), &n, |b, &n| {
            let mut env = new_env();
            for i in 0..n {
                env.add_type_generic("myAtom", f.atom(&format!("Type{}", i)));
            }
            let val = f.atom("myAtom");
            b.iter(|| black_box(infer_types_generic(&val, &f, &env)));
        });
    }

    // infer_arrow_return: (f x) where (: f (-> A B))
    group.bench_function("infer_arrow_return", |b| {
        let mut env = new_env();
        let arrow = make_arrow(&f, &["A"], "B");
        env.add_type_generic("f", arrow);
        let expr = f.sexpr(vec![f.atom("f"), f.atom("x")]);
        b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
    });

    // infer_multi_arrow: N overloaded arrows for same op
    for n in [1, 2, 5, 10, 20] {
        group.bench_with_input(BenchmarkId::new("infer_multi_arrow", n), &n, |b, &n| {
            let mut env = new_env();
            for i in 0..n {
                let arrow = make_arrow(&f, &[&format!("A{}", i)], &format!("B{}", i));
                env.add_type_generic("g", arrow);
            }
            let expr = f.sexpr(vec![f.atom("g"), f.atom("x")]);
            b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
        });
    }

    // infer_polymorphic_arrow: (-> $t $t) with concrete arg
    group.bench_function("infer_polymorphic_arrow", |b| {
        let mut env = new_env();
        let arrow = make_arrow(&f, &["$t"], "$t");
        env.add_type_generic("id", arrow);
        env.add_type_generic("x", f.atom("Number"));
        let expr = f.sexpr(vec![f.atom("id"), f.atom("x")]);
        b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
    });

    // infer_builtin_sig: (+ 1 2) via builtin registry
    group.bench_function("infer_builtin_sig", |b| {
        let env = new_env();
        let expr = f.sexpr(vec![f.atom("+"), f.long(1), f.long(2)]);
        b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
    });

    // infer_builtin_arg_mismatch: (+ 1 "hello")
    group.bench_function("infer_builtin_arg_mismatch", |b| {
        let env = new_env();
        let expr = f.sexpr(vec![f.atom("+"), f.long(1), f.string("hello")]);
        b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
    });

    // infer_data_constructor: Tuple type for (Pair a b)
    for arity in [2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("infer_data_constructor", arity),
            &arity,
            |b, &arity| {
                let env = new_env();
                let mut items = Vec::with_capacity(arity + 1);
                items.push(f.atom("Tuple"));
                for i in 0..arity {
                    items.push(f.long(i as i64));
                }
                let expr = f.sexpr(items);
                b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
            },
        );
    }

    // infer_control_flow_if: (if cond then else) branch union
    group.bench_function("infer_control_flow_if", |b| {
        let env = new_env();
        let expr = f.sexpr(vec![f.atom("if"), f.bool(true), f.long(1), f.long(2)]);
        b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
    });

    // infer_control_flow_case: N-branch case
    for n in [2, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("infer_control_flow_case", n),
            &n,
            |b, &n| {
                let env = new_env();
                let mut items = vec![f.atom("case"), f.atom("x")];
                for i in 0..n {
                    let branch = f.sexpr(vec![
                        f.sexpr(vec![f.atom(&format!("Pat{}", i))]),
                        f.long(i as i64),
                    ]);
                    items.push(branch);
                }
                let expr = f.sexpr(items);
                b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
            },
        );
    }

    // infer_control_flow_let: (let $x expr body) tracing
    group.bench_function("infer_control_flow_let", |b| {
        let env = new_env();
        let expr = f.sexpr(vec![f.atom("let"), f.atom("$x"), f.long(42), f.atom("$x")]);
        b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
    });

    // infer_nested_let_depth: N-deep nested lets
    for n in [5, 10, 50, 100] {
        group.bench_with_input(
            BenchmarkId::new("infer_nested_let_depth", n),
            &n,
            |b, &n| {
                let env = new_env();
                // Build (let $x0 0 (let $x1 1 ... (let $xN N $xN)))
                let mut expr = f.atom(&format!("$x{}", n - 1));
                for i in (0..n).rev() {
                    expr = f.sexpr(vec![
                        f.atom("let"),
                        f.atom(&format!("$x{}", i)),
                        f.long(i as i64),
                        expr,
                    ]);
                }
                b.iter(|| black_box(infer_types_generic(&expr, &f, &env)));
            },
        );
    }

    group.finish();
}

// =============================================================================
// Group 2: type_matching — Structural Type Matching
// =============================================================================

fn bench_type_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_matching");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let f = get_static_factory();

    // match_identical_atoms
    group.bench_function("match_identical_atoms", |b| {
        let a = f.atom("Number");
        let b_ = f.atom("Number");
        b.iter(|| black_box(types_match_generic(&a, &b_)));
    });

    // match_different_atoms
    group.bench_function("match_different_atoms", |b| {
        let a = f.atom("Number");
        let b_ = f.atom("String");
        b.iter(|| black_box(types_match_generic(&a, &b_)));
    });

    // match_undefined_wildcard: %Undefined% matches anything
    group.bench_function("match_undefined_wildcard", |b| {
        let a = f.atom("%Undefined%");
        let b_ = f.atom("Number");
        b.iter(|| black_box(types_match_generic(&a, &b_)));
    });

    // match_type_variable: $t matches anything
    group.bench_function("match_type_variable", |b| {
        let a = f.atom("$t");
        let b_ = f.atom("Number");
        b.iter(|| black_box(types_match_generic(&a, &b_)));
    });

    // match_meta_type: 5 meta-types
    for meta in &["Atom", "Expression", "Variable", "Grounded", "Type"] {
        group.bench_function(&format!("match_meta_{}", meta), |b| {
            let a = f.atom(meta);
            let b_ = f.atom("Number");
            b.iter(|| black_box(types_match_generic(&a, &b_)));
        });
    }

    // match_sexpr_structural: (-> A B ... Z) identical
    for arity in [2, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("match_sexpr_structural", arity),
            &arity,
            |b, &arity| {
                let mut items = Vec::with_capacity(arity + 1);
                items.push(f.atom("->"));
                for i in 0..arity {
                    items.push(f.atom(&format!("T{}", i)));
                }
                let a = f.sexpr(items.clone());
                let b_ = f.sexpr(items);
                b.iter(|| black_box(types_match_generic(&a, &b_)));
            },
        );
    }

    // match_sexpr_mismatch_last: Mismatch at last element
    for arity in [2, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("match_sexpr_mismatch_last", arity),
            &arity,
            |b, &arity| {
                let mut items_a = Vec::with_capacity(arity + 1);
                let mut items_b = Vec::with_capacity(arity + 1);
                items_a.push(f.atom("->"));
                items_b.push(f.atom("->"));
                for i in 0..arity {
                    items_a.push(f.atom(&format!("T{}", i)));
                    if i == arity - 1 {
                        items_b.push(f.atom("MISMATCH"));
                    } else {
                        items_b.push(f.atom(&format!("T{}", i)));
                    }
                }
                let a = f.sexpr(items_a);
                let b_ = f.sexpr(items_b);
                b.iter(|| black_box(types_match_generic(&a, &b_)));
            },
        );
    }

    // match_subtypes_direct: Dog matches Animal via subtype
    group.bench_function("match_subtypes_direct", |b| {
        let mut env = new_env();
        env.add_subtype_generic("Dog", "Animal");
        let actual = f.atom("Dog");
        let expected = f.atom("Animal");
        b.iter(|| black_box(types_match_with_subtypes(&actual, &expected, &env)));
    });

    // match_subtypes_chain: N-deep transitive subtype BFS
    for depth in [1, 3, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("match_subtypes_chain", depth),
            &depth,
            |b, &depth| {
                let mut env = new_env();
                setup_subtype_chain(&mut env, depth);
                let actual = f.atom("T0");
                let expected = f.atom(&format!("T{}", depth));
                b.iter(|| black_box(types_match_with_subtypes(&actual, &expected, &env)));
            },
        );
    }

    // match_subtypes_miss: Full BFS, no match
    for depth in [1, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("match_subtypes_miss", depth),
            &depth,
            |b, &depth| {
                let mut env = new_env();
                setup_subtype_chain(&mut env, depth);
                let actual = f.atom("T0");
                let expected = f.atom("Unrelated");
                b.iter(|| black_box(types_match_with_subtypes(&actual, &expected, &env)));
            },
        );
    }

    // match_arrow_subtyping: Contravariant params, covariant ret
    group.bench_function("match_arrow_subtyping", |b| {
        let mut env = new_env();
        env.add_subtype_generic("Dog", "Animal");
        env.add_subtype_generic("Cat", "Animal");
        // (-> Animal Bool) should match (-> Dog Bool) with subtyping
        let actual = make_arrow(&f, &["Animal"], "Bool");
        let expected = make_arrow(&f, &["Dog"], "Bool");
        b.iter(|| black_box(types_match_with_subtypes(&actual, &expected, &env)));
    });

    // bindings_single_var: $t → Number
    group.bench_function("bindings_single_var", |b| {
        let pattern = f.atom("$t");
        let actual = f.atom("Number");
        b.iter(|| {
            let mut bindings = HashMap::new();
            black_box(match_types_with_bindings(&pattern, &actual, &mut bindings))
        });
    });

    // bindings_multi_var: (-> $t $u ... ) match
    for vars in [1, 2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("bindings_multi_var", vars),
            &vars,
            |b, &vars| {
                let mut pattern_items = vec![f.atom("->")];
                let mut actual_items = vec![f.atom("->")];
                for i in 0..vars {
                    pattern_items.push(f.atom(&format!("$t{}", i)));
                    actual_items.push(f.atom(&format!("Type{}", i)));
                }
                pattern_items.push(f.atom("$ret"));
                actual_items.push(f.atom("Result"));
                let pattern = f.sexpr(pattern_items);
                let actual = f.sexpr(actual_items);
                b.iter(|| {
                    let mut bindings = HashMap::new();
                    black_box(match_types_with_bindings(&pattern, &actual, &mut bindings))
                });
            },
        );
    }

    // bindings_consistency: Re-match $t with existing binding
    group.bench_function("bindings_consistency", |b| {
        let pattern = f.atom("$t");
        let actual = f.atom("Number");
        b.iter(|| {
            let mut bindings = HashMap::new();
            bindings.insert("$t".to_string(), f.atom("Number"));
            black_box(match_types_with_bindings(&pattern, &actual, &mut bindings))
        });
    });

    group.finish();
}

// =============================================================================
// Group 3: type_allocation — Allocation-Heavy Operations
// =============================================================================

fn bench_type_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_allocation");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let f = get_static_factory();

    // freshen_simple: (-> $t $u) → (-> $t__0 $u__0)
    group.bench_function("freshen_simple", |b| {
        let typ = make_arrow(&f, &["$t"], "$u");
        b.iter(|| black_box(freshen_type_variables(&typ, 0, &f)));
    });

    // freshen_wide: Arrow with N type vars
    for n in [2, 5, 10, 20] {
        group.bench_with_input(BenchmarkId::new("freshen_wide", n), &n, |b, &n| {
            let params: Vec<String> = (0..n).map(|i| format!("$t{}", i)).collect();
            let param_refs: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
            let typ = make_arrow(&f, &param_refs, "$ret");
            b.iter(|| black_box(freshen_type_variables(&typ, 0, &f)));
        });
    }

    // freshen_nested: (-> (List $t) (Map $t $u))
    for depth in [1, 2, 3, 5] {
        group.bench_with_input(
            BenchmarkId::new("freshen_nested", depth),
            &depth,
            |b, &depth| {
                // Build a nested type like (List (List (... $t)))
                let mut inner = f.atom("$t");
                for _ in 0..depth {
                    inner = f.sexpr(vec![f.atom("List"), inner]);
                }
                let typ = f.sexpr(vec![f.atom("->"), inner.clone(), inner]);
                b.iter(|| black_box(freshen_type_variables(&typ, 0, &f)));
            },
        );
    }

    // apply_single: Substitute $t → Number
    group.bench_function("apply_single", |b| {
        let typ = f.atom("$t");
        let mut bindings = HashMap::new();
        bindings.insert("$t".to_string(), f.atom("Number"));
        b.iter(|| black_box(apply_type_bindings(&typ, &bindings, &f)));
    });

    // apply_arrow: Substitute N vars in arrow
    for vars in [1, 2, 5, 10] {
        group.bench_with_input(BenchmarkId::new("apply_arrow", vars), &vars, |b, &vars| {
            let params: Vec<String> = (0..vars).map(|i| format!("$t{}", i)).collect();
            let param_refs: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
            let typ = make_arrow(&f, &param_refs, "$ret");
            let mut bindings = HashMap::new();
            for i in 0..vars {
                bindings.insert(format!("$t{}", i), f.atom(&format!("Type{}", i)));
            }
            bindings.insert("$ret".to_string(), f.atom("Result"));
            b.iter(|| black_box(apply_type_bindings(&typ, &bindings, &f)));
        });
    }

    // apply_deep: Deeply nested type substitution
    for depth in [1, 3, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("apply_deep", depth),
            &depth,
            |b, &depth| {
                let mut inner = f.atom("$t");
                for _ in 0..depth {
                    inner = f.sexpr(vec![f.atom("Wrapper"), inner]);
                }
                let mut bindings = HashMap::new();
                bindings.insert("$t".to_string(), f.atom("Number"));
                b.iter(|| black_box(apply_type_bindings(&inner, &bindings, &f)));
            },
        );
    }

    // apply_noop: All concrete, no substitution needed
    for arity in [2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("apply_noop", arity),
            &arity,
            |b, &arity| {
                let typ = make_arrow(
                    &f,
                    &(0..arity)
                        .map(|i| format!("T{}", i))
                        .collect::<Vec<_>>()
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>(),
                    "Result",
                );
                let bindings = HashMap::new();
                b.iter(|| black_box(apply_type_bindings(&typ, &bindings, &f)));
            },
        );
    }

    group.finish();
}

// =============================================================================
// Group 4: subtype_hierarchy — Subtype Graph Operations
// =============================================================================

fn bench_subtype_hierarchy(c: &mut Criterion) {
    let mut group = c.benchmark_group("subtype_hierarchy");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    // supertypes_linear: Linear chain A<:B<:C<:...
    for depth in [1, 5, 10, 20, 50, 100] {
        group.bench_with_input(
            BenchmarkId::new("supertypes_linear", depth),
            &depth,
            |b, &depth| {
                let mut env = new_env();
                setup_subtype_chain(&mut env, depth);
                b.iter(|| black_box(env.get_all_supertypes("T0")));
            },
        );
    }

    // supertypes_diamond: Diamond hierarchy
    // T0 <: T_left_i <: T_top AND T0 <: T_right_i <: T_top
    for width in [2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("supertypes_diamond", width),
            &width,
            |b, &width| {
                let mut env = new_env();
                for i in 0..width {
                    let mid = format!("Mid{}", i);
                    env.add_subtype_generic("Bottom", &mid);
                    env.add_subtype_generic(&mid, "Top");
                }
                b.iter(|| black_box(env.get_all_supertypes("Bottom")));
            },
        );
    }

    // supertypes_wide: N direct supertypes from one type
    for n in [1, 5, 10, 50] {
        group.bench_with_input(BenchmarkId::new("supertypes_wide", n), &n, |b, &n| {
            let mut env = new_env();
            for i in 0..n {
                env.add_subtype_generic("Base", &format!("Super{}", i));
            }
            b.iter(|| black_box(env.get_all_supertypes("Base")));
        });
    }

    // supertypes_empty: No supertypes (immediate return)
    group.bench_function("supertypes_empty", |b| {
        let env = new_env();
        b.iter(|| black_box(env.get_all_supertypes("Nonexistent")));
    });

    // is_subtype_hit: Direct subtype match
    group.bench_function("is_subtype_hit", |b| {
        let mut env = new_env();
        env.add_subtype_generic("Dog", "Animal");
        b.iter(|| black_box(env.is_subtype_of("Dog", "Animal")));
    });

    // is_subtype_miss: No relation, full BFS miss
    for num_types in [10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::new("is_subtype_miss", num_types),
            &num_types,
            |b, &num_types| {
                let mut env = new_env();
                // Build a chain but query unrelated types
                setup_subtype_chain(&mut env, num_types);
                b.iter(|| black_box(env.is_subtype_of("T0", "Unrelated")));
            },
        );
    }

    // is_subtype_transitive: Transitive at depth N
    for depth in [1, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("is_subtype_transitive", depth),
            &depth,
            |b, &depth| {
                let mut env = new_env();
                setup_subtype_chain(&mut env, depth);
                b.iter(|| black_box(env.is_subtype_of("T0", &format!("T{}", depth))));
            },
        );
    }

    group.finish();
}

// =============================================================================
// Group 5: env_type_ops — Environment Type Storage
// =============================================================================

fn bench_env_type_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("env_type_ops");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let f = get_static_factory();

    // get_types_bloom_hit: Bloom pass → HashMap hit
    for n in [10, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::new("get_types_bloom_hit", n), &n, |b, &n| {
            let mut env = new_env();
            for i in 0..n {
                env.add_type_generic(&format!("atom{}", i), f.atom(&format!("Type{}", i)));
            }
            // Query an atom that exists (bloom + hashmap hit)
            let query = format!("atom{}", n / 2);
            b.iter(|| black_box(env.get_types_generic(black_box(&query))));
        });
    }

    // get_types_bloom_reject: Bloom rejects (no type exists)
    for n in [10, 100, 1000, 10000] {
        group.bench_with_input(
            BenchmarkId::new("get_types_bloom_reject", n),
            &n,
            |b, &n| {
                let mut env = new_env();
                for i in 0..n {
                    env.add_type_generic(&format!("atom{}", i), f.atom(&format!("Type{}", i)));
                }
                b.iter(|| black_box(env.get_types_generic(black_box("nonexistent"))));
            },
        );
    }

    // get_types_with_supertypes: get_types + N-deep closure
    for depth in [1, 5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("get_types_with_supertypes", depth),
            &depth,
            |b, &depth| {
                let mut env = new_env();
                setup_subtype_chain(&mut env, depth);
                env.add_type_generic("x", f.atom("T0"));
                // infer_types_generic includes supertype closure
                let val = f.atom("x");
                b.iter(|| black_box(infer_types_generic(&val, &f, &env)));
            },
        );
    }

    // add_type_throughput: Sequential add_type_generic (uses iter_batched for CoW)
    for n in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::new("add_type_throughput", n), &n, |b, &n| {
            b.iter_batched(
                || new_env(),
                |mut env| {
                    for i in 0..n {
                        env.add_type_generic(&format!("atom{}", i), f.atom(&format!("Type{}", i)));
                    }
                    black_box(env);
                },
                BatchSize::SmallInput,
            );
        });
    }

    // inferred_bloom_check: has_inferred_type
    for n in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::new("inferred_bloom_check", n), &n, |b, &n| {
            let env = new_env();
            for i in 0..n {
                env.register_inferred_type(&format!("fn{}", i), &f.atom("Number"));
            }
            let query = format!("fn{}", n / 2);
            b.iter(|| black_box(env.has_inferred_type(black_box(&query))));
        });
    }

    // inferred_dashmap_get: get_inferred_fn_types DashMap
    for n in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::new("inferred_dashmap_get", n), &n, |b, &n| {
            let env = new_env();
            for i in 0..n {
                env.register_inferred_type(&format!("fn{}", i), &f.atom("Number"));
            }
            let query = format!("fn{}", n / 2);
            b.iter(|| black_box(env.get_inferred_fn_types(black_box(&query))));
        });
    }

    group.finish();
}

// =============================================================================
// Group 6: applicative_pre_eval — Type-Driven Eval Decisions
// =============================================================================

fn bench_applicative_pre_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("applicative_pre_eval");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let f = get_static_factory();

    // typed_indices_meta: Arrow with meta-typed params (skip pre-eval)
    for arity in [2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("typed_indices_meta", arity),
            &arity,
            |b, &arity| {
                let mut env = new_env();
                // (: op (-> Atom Atom ... Atom Result))
                let params: Vec<&str> = (0..arity).map(|_| "Atom").collect();
                let arrow = make_arrow(&f, &params, "Result");
                env.add_type_generic("op", arrow);
                let mut items = vec![f.atom("op")];
                for i in 0..arity {
                    items.push(f.atom(&format!("arg{}", i)));
                }
                b.iter(|| {
                    black_box(find_typed_arg_indices_generic(
                        black_box(&items),
                        &env,
                        None,
                    ))
                });
            },
        );
    }

    // typed_indices_value: Arrow with all value-typed params
    for arity in [2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("typed_indices_value", arity),
            &arity,
            |b, &arity| {
                let mut env = new_env();
                let params: Vec<&str> = (0..arity).map(|_| "Number").collect();
                let arrow = make_arrow(&f, &params, "Result");
                env.add_type_generic("op", arrow);
                let mut items = vec![f.atom("op")];
                for i in 0..arity {
                    items.push(f.sexpr(vec![f.atom("f"), f.long(i as i64)]));
                }
                b.iter(|| {
                    black_box(find_typed_arg_indices_generic(
                        black_box(&items),
                        &env,
                        None,
                    ))
                });
            },
        );
    }

    // typed_indices_no_arrow: No arrow type → returns None
    group.bench_function("typed_indices_no_arrow", |b| {
        let env = new_env();
        let items = vec![f.atom("unknown"), f.long(1), f.long(2)];
        b.iter(|| {
            black_box(find_typed_arg_indices_generic(
                black_box(&items),
                &env,
                None,
            ))
        });
    });

    // grounded_indices_bloom_hit: Bloom says "maybe rules"
    for arity in [2, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("grounded_indices_bloom_hit", arity),
            &arity,
            |b, &arity| {
                let mut env = new_env();
                // Add a rule so bloom filter has something
                let mut lhs_items = vec![f.atom("op")];
                for i in 0..arity {
                    lhs_items.push(f.atom(&format!("$x{}", i)));
                }
                let lhs = f.sexpr(lhs_items.clone());
                let rhs = f.atom("result");
                env.add_to_space(&f.sexpr(vec![f.atom("="), lhs, rhs]));
                let mut items = vec![f.atom("op")];
                for i in 0..arity {
                    items.push(f.sexpr(vec![f.atom("inner"), f.long(i as i64)]));
                }
                b.iter(|| black_box(find_grounded_arg_indices_generic(black_box(&items), &env)));
            },
        );
    }

    // grounded_indices_bloom_miss: Bloom rejects (no rules for this op)
    group.bench_function("grounded_indices_bloom_miss", |b| {
        let env = new_env();
        let items = vec![
            f.atom("no_rules_op"),
            f.sexpr(vec![f.atom("inner"), f.long(1)]),
        ];
        b.iter(|| black_box(find_grounded_arg_indices_generic(black_box(&items), &env)));
    });

    // is_value_type_hit: Atom with only value types
    group.bench_function("is_value_type_hit", |b| {
        let mut env = new_env();
        env.add_type_generic("x", f.atom("Number"));
        b.iter(|| black_box(is_declared_value_type(black_box("x"), &env, None)));
    });

    // is_value_type_miss: Atom with arrow type
    group.bench_function("is_value_type_miss", |b| {
        let mut env = new_env();
        env.add_type_generic("f", make_arrow(&f, &["Number"], "Bool"));
        b.iter(|| black_box(is_declared_value_type(black_box("f"), &env, None)));
    });

    group.finish();
}

// =============================================================================
// Group 7: type_special_forms — End-to-End via eval()
// =============================================================================

fn bench_type_special_forms(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_special_forms");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(3));

    // get_type_ground
    group.bench_function("get_type_ground", |b| {
        b.iter(|| black_box(run_program("!(get-type 42)")));
    });

    // get_type_typed_atom
    group.bench_function("get_type_typed_atom", |b| {
        b.iter(|| black_box(run_program("(: x Number)\n!(get-type x)")));
    });

    // get_type_multi_typed: N type decls + get-type
    for n in [1, 5, 10] {
        group.bench_with_input(BenchmarkId::new("get_type_multi_typed", n), &n, |b, &n| {
            let mut src = String::new();
            for i in 0..n {
                src.push_str(&format!("(: x Type{})\n", i));
            }
            src.push_str("!(get-type x)");
            b.iter(|| black_box(run_program(&src)));
        });
    }

    // get_type_arrow_app
    group.bench_function("get_type_arrow_app", |b| {
        b.iter(|| black_box(run_program("(: f (-> Number Bool))\n!(get-type (f 1))")));
    });

    // check_type_match
    group.bench_function("check_type_match", |b| {
        b.iter(|| black_box(run_program("!(check-type 42 Number)")));
    });

    // check_type_mismatch
    group.bench_function("check_type_mismatch", |b| {
        b.iter(|| black_box(run_program("!(check-type 42 String)")));
    });

    // check_type_subtype at varying depth
    for depth in [1, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("check_type_subtype", depth),
            &depth,
            |b, &depth| {
                let mut src = String::new();
                for i in 0..depth {
                    src.push_str(&format!("(:< T{} T{})\n", i, i + 1));
                }
                src.push_str(&format!("(: x T0)\n!(check-type x T{})", depth));
                b.iter(|| black_box(run_program(&src)));
            },
        );
    }

    // type_cast_pass
    group.bench_function("type_cast_pass", |b| {
        b.iter(|| black_box(run_program("(: x Number)\n!(type-cast x Number &self)")));
    });

    // type_cast_fail
    group.bench_function("type_cast_fail", |b| {
        b.iter(|| black_box(run_program("(: x Number)\n!(type-cast x String &self)")));
    });

    // type_cast_meta
    group.bench_function("type_cast_meta", |b| {
        b.iter(|| black_box(run_program("(: x Number)\n!(type-cast x Atom &self)")));
    });

    // is_function_true
    group.bench_function("is_function_true", |b| {
        b.iter(|| black_box(run_program("!(is-function (-> A B))")));
    });

    // is_function_false
    group.bench_function("is_function_false", |b| {
        b.iter(|| black_box(run_program("!(is-function Number)")));
    });

    // Full type-heavy program
    group.bench_function("type_heavy_program", |b| {
        let src = include_str!("metta_samples/type_heavy_program.metta");
        b.iter(|| black_box(run_program(src)));
    });

    group.finish();
}

// =============================================================================
// Group 8: type_fixpoint — Fixpoint Inference
// =============================================================================

fn bench_type_fixpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_fixpoint");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(3));

    // fixpoint_no_rules: No rules, immediate return
    group.bench_function("fixpoint_no_rules", |b| {
        let env = new_env();
        b.iter(|| {
            run_type_fixpoint(&env);
            black_box(&env);
        });
    });

    // fixpoint_N_independent: N non-recursive rules
    for n in [5, 10, 50, 100] {
        group.bench_with_input(
            BenchmarkId::new("fixpoint_N_independent", n),
            &n,
            |b, &n| {
                b.iter_batched(
                    || {
                        let mut src = String::new();
                        for i in 0..n {
                            src.push_str(&format!("(= (f{} $x) (+ $x {}))\n", i, i));
                        }
                        let state = compile(&src).expect("Failed to compile");
                        let env_state = new_env();
                        let result =
                            run_state(MettaState::from_env(env_state), &state).expect("eval");
                        result.environment
                    },
                    |env| {
                        run_type_fixpoint(&env);
                        black_box(&env);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    // fixpoint_mutual_pair: Two mutually recursive functions
    group.bench_function("fixpoint_mutual_pair", |b| {
        b.iter_batched(
            || {
                let src = r#"
                    (: is-even (-> Number Bool))
                    (= (is-even $n) (if (== $n 0) True (is-odd (- $n 1))))
                    (= (is-odd $n) (if (== $n 0) False (is-even (- $n 1))))
                "#;
                let state = compile(src).expect("Failed to compile");
                let env_state = new_env();
                let result = run_state(MettaState::from_env(env_state), &state).expect("eval");
                result.environment
            },
            |env| {
                run_type_fixpoint(&env);
                black_box(&env);
            },
            BatchSize::SmallInput,
        );
    });

    // fixpoint_with_explicit: N rules with explicit (: f (-> ...))
    for n in [5, 10, 50] {
        group.bench_with_input(
            BenchmarkId::new("fixpoint_with_explicit", n),
            &n,
            |b, &n| {
                b.iter_batched(
                    || {
                        let mut src = String::new();
                        for i in 0..n {
                            src.push_str(&format!(
                                "(: f{} (-> Number Number))\n(= (f{} $x) (+ $x {}))\n",
                                i, i, i
                            ));
                        }
                        let state = compile(&src).expect("Failed to compile");
                        let env_state = new_env();
                        let result =
                            run_state(MettaState::from_env(env_state), &state).expect("eval");
                        result.environment
                    },
                    |env| {
                        run_type_fixpoint(&env);
                        black_box(&env);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// =============================================================================
// Group 9: type_o1_checks — O(1) Lightweight Checks
// =============================================================================

fn bench_type_o1_checks(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_o1_checks");

    let f = get_static_factory();

    // is_meta_type_positive: 5 meta-type names
    for name in &["Atom", "Expression", "Variable", "Grounded", "Type"] {
        group.bench_function(&format!("is_meta_type_pos_{}", name), |b| {
            b.iter(|| black_box(is_meta_type(black_box(name))));
        });
    }

    // is_meta_type_negative: 5 non-meta-type names
    for name in &["Number", "String", "Bool", "MyType", "foo"] {
        group.bench_function(&format!("is_meta_type_neg_{}", name), |b| {
            b.iter(|| black_box(is_meta_type(black_box(name))));
        });
    }

    // get_ground_type_long: Long → Some("Number")
    group.bench_function("get_ground_type_long", |b| {
        let val = f.long(42);
        b.iter(|| black_box(get_ground_type::<MettaValue>(black_box(&val))));
    });

    // get_ground_type_atom: Atom → None
    group.bench_function("get_ground_type_atom", |b| {
        let val = f.atom("x");
        b.iter(|| black_box(get_ground_type::<MettaValue>(black_box(&val))));
    });

    // is_pattern_compat_var: Variable matches any ground
    group.bench_function("is_pattern_compat_var", |b| {
        let val = f.atom("$x");
        b.iter(|| black_box(is_pattern_type_compatible(black_box(&val), "Number")));
    });

    // is_pattern_compat_miss: String pattern vs Number
    group.bench_function("is_pattern_compat_miss", |b| {
        let val = f.string("hello");
        b.iter(|| black_box(is_pattern_type_compatible(black_box(&val), "Number")));
    });

    // is_arrow_type_true: (-> A B)
    group.bench_function("is_arrow_type_true", |b| {
        let val = make_arrow(&f, &["A"], "B");
        b.iter(|| black_box(is_arrow_type(black_box(&val))));
    });

    // is_arrow_type_false: (A B) — not an arrow
    group.bench_function("is_arrow_type_false", |b| {
        let val = f.sexpr(vec![f.atom("A"), f.atom("B")]);
        b.iter(|| black_box(is_arrow_type(black_box(&val))));
    });

    // get_signature_hit: "+"
    group.bench_function("get_signature_hit", |b| {
        b.iter(|| black_box(get_signature(black_box("+"))));
    });

    // get_signature_miss: "nonexistent"
    group.bench_function("get_signature_miss", |b| {
        b.iter(|| black_box(get_signature(black_box("nonexistent"))));
    });

    // is_builtin_hit: "+"
    group.bench_function("is_builtin_hit", |b| {
        b.iter(|| black_box(is_builtin(black_box("+"))));
    });

    // is_builtin_miss: "foo"
    group.bench_function("is_builtin_miss", |b| {
        b.iter(|| black_box(is_builtin(black_box("foo"))));
    });

    group.finish();
}

// =============================================================================
// Criterion Configuration & Main
// =============================================================================

criterion_group!(
    name = micro_benchmarks;
    config = Criterion::default()
        .significance_level(0.05)
        .noise_threshold(0.03)
        .measurement_time(Duration::from_secs(10))
        .sample_size(50)
        .warm_up_time(Duration::from_secs(3));
    targets =
        bench_type_inference,
        bench_type_matching,
        bench_type_allocation,
        bench_subtype_hierarchy,
        bench_env_type_ops,
        bench_applicative_pre_eval
);

criterion_group!(
    name = e2e_benchmarks;
    config = Criterion::default()
        .significance_level(0.05)
        .noise_threshold(0.03)
        .measurement_time(Duration::from_secs(15))
        .sample_size(30)
        .warm_up_time(Duration::from_secs(3));
    targets =
        bench_type_special_forms,
        bench_type_fixpoint
);

criterion_group!(
    name = o1_benchmarks;
    config = Criterion::default()
        .significance_level(0.05)
        .noise_threshold(0.03);
    targets =
        bench_type_o1_checks
);

criterion_main!(micro_benchmarks, e2e_benchmarks, o1_benchmarks);
