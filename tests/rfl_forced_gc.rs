//! Bounded forced-GC gate for the R-FL duplicate free-list regression.
//!
//! The production detector is `METTATRON_INDEX_GC_FREELIST_CHECK=1`.  This
//! test drives the public compile/eval path through enough allocation churn to
//! force real index-GC cycles while that detector is enabled.


use std::fmt::Write;

use mettatron::backend::eval::cesk::index_heap::index_gc;
use mettatron::{compile, eval, new_env, MettaValue};

const ROUNDS: usize = 4;
const FORMS_PER_ROUND: usize = 640;
const ATOMS_PER_FORM: usize = 96;

#[test]
fn forced_evaluator_churn_runs_gc_with_freelist_checker() {
    std::env::set_var("METTATRON_INDEX_GC_FREELIST_CHECK", "1");
    std::env::set_var("METTATRON_INDEX_GC_REPORT", "1");
    std::env::set_var("METTATRON_INDEX_GC_MIN_BYTES", "131072");
    std::env::set_var("METTATRON_INDEX_GC_MAX_BYTES", "268435456");
    std::env::set_var("METTATRON_PARALLEL_FANOUT_DEPTH", "0");

    let cycles_before = index_gc::cycles_run();
    let minors_before = index_gc::minor_cycles_run();
    let majors_before = index_gc::major_cycles_run();

    for round in 0..ROUNDS {
        run_generated_round(round);
    }

    let cycles_after = index_gc::cycles_run();
    let minors_after = index_gc::minor_cycles_run();
    let majors_after = index_gc::major_cycles_run();
    assert!(
        cycles_after > cycles_before,
        "forced churn must run at least one index-GC cycle: before={cycles_before}, after={cycles_after}"
    );
    assert!(
        minors_after > minors_before,
        "forced churn must run at least one minor cycle: before={minors_before}, after={minors_after}"
    );
    assert!(
        majors_after > majors_before,
        "forced churn must run at least one major cycle: before={majors_before}, after={majors_after}"
    );
}

fn run_generated_round(round: usize) {
    let source = generated_source(round);
    let state = compile(&source).expect("compile generated forced-GC source");
    let mut env = new_env();
    let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();

    assert_eq!(
        source_exprs.len(),
        FORMS_PER_ROUND + 1,
        "generated round should have all quoted churn forms plus sentinel"
    );

    for expr in source_exprs {
        let (results, next_env, ..) = eval(expr, env, &state);
        env = next_env;
        assert!(
            !results.is_empty(),
            "generated forced-GC expression should remain reducible"
        );
    }
}

fn generated_source(round: usize) -> String {
    let mut source =
        String::with_capacity(FORMS_PER_ROUND * ATOMS_PER_FORM * 12 + FORMS_PER_ROUND * 32);

    for form in 0..FORMS_PER_ROUND {
        write!(&mut source, "!(quote (chunk{round}x{form}").expect("format generated form header");
        for atom in 0..ATOMS_PER_FORM {
            write!(&mut source, " r{round}x{form}x{atom}").expect("format generated atom");
        }
        source.push_str("))\n");
    }

    write!(&mut source, "!(quote rfl-gc-round-{round})\n").expect("format generated sentinel");
    source
}
