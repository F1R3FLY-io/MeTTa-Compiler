use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::compile::compile;
use mettatron::backend::environment::Environment;

/// Generate N type assertions for benchmarking
/// Creates type assertions like: (: atom-0 Int), (: atom-1 String), etc.
fn generate_type_assertions(n: usize) -> String {
    let mut types = String::new();
    let type_names = ["Int", "String", "Bool", "Float", "Atom"];

    for i in 0..n {
        let type_name = type_names[i % type_names.len()];
        types.push_str(&format!("(: atom-{} {})\n", i, type_name));
    }

    types
}

/// Benchmark type lookup performance with varying type assertion counts
/// This tests the PathMap::restrict() optimization for type index
fn bench_type_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_lookup");

    // Test with increasing numbers of type assertions
    for type_count in [10, 100, 1000, 5000, 10000].iter() {
        let types_src = generate_type_assertions(*type_count);

        group.bench_with_input(
            BenchmarkId::new("get_type_first", type_count),
            type_count,
            |b, _| {
                // Setup: Create environment with N type assertions
                let mut env = Environment::new();
                let state = compile(&types_src).expect("Failed to compile types");
                for typ in state.source {
                    env.add_to_space(&typ);
                }

                // Benchmark: Look up the FIRST type assertion (best case)
                // This tests how well the type index works
                b.iter(|| {
                    let result = env.get_type(black_box("atom-0"));
                    black_box(result);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_type_middle", type_count),
            type_count,
            |b, _| {
                // Setup: Create environment with N type assertions
                let mut env = Environment::new();
                let state = compile(&types_src).expect("Failed to compile types");
                for typ in state.source {
                    env.add_to_space(&typ);
                }

                // Benchmark: Look up a MIDDLE type assertion
                let middle_atom = format!("atom-{}", type_count / 2);
                b.iter(|| {
                    let result = env.get_type(black_box(&middle_atom));
                    black_box(result);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_type_last", type_count),
            type_count,
            |b, _| {
                // Setup: Create environment with N type assertions
                let mut env = Environment::new();
                let state = compile(&types_src).expect("Failed to compile types");
                for typ in state.source {
                    env.add_to_space(&typ);
                }

                // Benchmark: Look up the LAST type assertion (worst case for linear search)
                let last_atom = format!("atom-{}", type_count - 1);
                b.iter(|| {
                    let result = env.get_type(black_box(&last_atom));
                    black_box(result);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_type_missing", type_count),
            type_count,
            |b, _| {
                // Setup: Create environment with N type assertions
                let mut env = Environment::new();
                let state = compile(&types_src).expect("Failed to compile types");
                for typ in state.source {
                    env.add_to_space(&typ);
                }

                // Benchmark: Look up a MISSING type (full search required)
                b.iter(|| {
                    let result = env.get_type(black_box("nonexistent-atom"));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark type index initialization cost
/// Tests the one-time cost of building the type index via PathMap::restrict()
fn bench_type_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_index_build");

    for type_count in [100, 1000, 5000, 10000].iter() {
        let types_src = generate_type_assertions(*type_count);

        group.bench_with_input(
            BenchmarkId::new("first_lookup_cold_cache", type_count),
            type_count,
            |b, _| {
                b.iter_batched(
                    || {
                        // Setup: Create fresh environment for each iteration (cold cache)
                        let mut env = Environment::new();
                        let state = compile(&types_src).expect("Failed to compile types");
                        for typ in state.source {
                            env.add_to_space(&typ);
                        }
                        env
                    },
                    |env| {
                        // Measure: First lookup builds the index
                        let result = env.get_type(black_box("atom-0"));
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("subsequent_lookup_hot_cache", type_count),
            type_count,
            |b, _| {
                // Setup: Create environment and warm up cache
                let mut env = Environment::new();
                let state = compile(&types_src).expect("Failed to compile types");
                for typ in state.source {
                    env.add_to_space(&typ);
                }
                // Warm up: Build the type index
                env.get_type("atom-0");

                // Measure: Subsequent lookups use cached index
                b.iter(|| {
                    let result = env.get_type(black_box("atom-0"));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark mixed workload: type lookups with other operations
fn bench_type_lookup_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("type_lookup_mixed");

    for type_count in [1000, 5000, 10000].iter() {
        let types_src = generate_type_assertions(*type_count);

        group.bench_with_input(
            BenchmarkId::new("lookup_after_insert", type_count),
            type_count,
            |b, _| {
                b.iter_batched(
                    || {
                        // Setup: Create environment with N-1 type assertions
                        let mut env = Environment::new();
                        let mut types_partial_src = generate_type_assertions(type_count - 1);
                        types_partial_src.push_str(&format!("(: new-atom Int)\n"));
                        let state = compile(&types_partial_src).expect("Failed to compile types");
                        for typ in state.source {
                            env.add_to_space(&typ);
                        }
                        env
                    },
                    |mut env| {
                        // Measure: Insert invalidates cache, then lookup rebuilds it
                        env.add_type("another-atom".to_string(), mettatron::backend::MettaValue::Atom("String".to_string()));
                        let result = env.get_type(black_box("atom-0"));
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_type_lookup,
    bench_type_index_build,
    bench_type_lookup_mixed
);
criterion_main!(benches);
