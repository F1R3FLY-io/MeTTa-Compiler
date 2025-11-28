use std::sync::LazyLock;

use divan::black_box;
use paste::paste;

use mettatron::config::{configure_eval, EvalConfig};
use mettatron::{compile, run_state, run_state_async, MettaState};

static RT: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

fn main() {
    configure_eval(EvalConfig::cpu_optimized());

    divan::main();
}

const FIB_SRC: &str = include_str!("metta_samples/fib.metta");
const KNOWLEDGE_GRAPH_SRC: &str = include_str!("metta_samples/knowledge_graph.metta");
const CONSTRAINT_SEARCH_SRC: &str = include_str!("metta_samples/contstraint_search_simple.metta");
const MULTI_SPACE_REASONING_SRC: &str = include_str!("metta_samples/multi_space_reasoning.metta");
const PATTERN_MATCHING_STRESS_SRC: &str =
    include_str!("metta_samples/pattern_matching_stress.metta");
const CONCURRENT_SPACE_OPERATIONS_SRC: &str =
    include_str!("metta_samples/concurrent_space_operations.metta");
const METTA_PROGRAMMING_STRESS_SRC: &str =
    include_str!("metta_samples/metta_programming_stress.metta");

fn run_sync(src: &'static str) {
    let state = MettaState::new_empty();
    let program = compile(src).unwrap();
    let result = run_state(state, program).unwrap();
    black_box(result);
}

async fn run_async(src: &'static str) {
    let state = MettaState::new_empty();
    let program = compile(src).unwrap();
    let result = run_state_async(state, program).await.unwrap();
    black_box(result);
}

macro_rules! metta_bench_pair {
    ($name:ident, $src:expr) => {
        paste! {
            #[divan::bench]
            fn $name() {
                run_sync($src);
            }

            #[divan::bench]
            fn [<async_ $name>]() {
                RT.block_on(run_async($src));
            }
        }
    };
}

metta_bench_pair!(fib, FIB_SRC);
metta_bench_pair!(knowledge_graph, KNOWLEDGE_GRAPH_SRC);
metta_bench_pair!(contstraint_search, CONSTRAINT_SEARCH_SRC);
metta_bench_pair!(multi_space_reasoning, MULTI_SPACE_REASONING_SRC);
metta_bench_pair!(pattern_matching_stress, PATTERN_MATCHING_STRESS_SRC);
metta_bench_pair!(concurrent_space_operations, CONCURRENT_SPACE_OPERATIONS_SRC);
metta_bench_pair!(metta_programming_stress, METTA_PROGRAMMING_STRESS_SRC);
