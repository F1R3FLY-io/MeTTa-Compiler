//! Integration tests for the Stage 5a ACT out-of-core MeTTa **surface forms**,
//! exercised through the real evaluator (compile → eval), not just the Rust API:
//!
//! - `(save-space! "name")`  → dump `&self` to `/dev/shm/<name>.act`
//! - `(query-act "name" pat tmpl)` → out-of-core query (trie-pruned `query_multi_act`)
//! - `(load-space! "name")`  → restore the snapshot into `&self`
//!
//! These confirm the special-form dispatch arms (`step/sexpr.rs`), the impure-head
//! classification (`dispatch_hints.rs`), and the T0 tier routing (`bytecode/mod.rs`)
//! all wire up correctly end-to-end. The Rust-API-level semantics (multiplicity
//! fidelity, in-memory equivalence) are covered by
//! `backend::environment::act_persistence::tests`.

use std::sync::atomic::{AtomicU64, Ordering};

use mettatron::{compile, eval, new_env, MettaValue};

/// Evaluate MeTTa source, returning the LAST top-level directive's result set.
fn eval_metta_last(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compilation should succeed");
    let mut env = new_env();
    let mut last = Vec::new();
    let src: Vec<MettaValue> = state.source().iter().copied().collect();
    for &expr in &src {
        let (results, new_env, ..) = eval(expr, env, &state);
        env = new_env;
        last = results.into_vec();
    }
    last
}

/// Unique `.act` base name per call (avoid `/dev/shm` collisions across parallel
/// tests and repeated runs).
fn unique(tag: &str) -> String {
    static C: AtomicU64 = AtomicU64::new(0);
    format!(
        "mtt_actsurf_{}_{}_{}",
        tag,
        std::process::id(),
        C.fetch_add(1, Ordering::Relaxed)
    )
}

/// RAII removal of all `/dev/shm/<name>.*` artifacts (btm, wide, sm).
struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for suffix in [".act", ".wide.act", ".sm"] {
            let _ = std::fs::remove_file(format!("/dev/shm/{}{suffix}", self.0));
        }
    }
}

#[test]
fn surface_save_then_out_of_core_query() {
    let name = unique("query");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (parent alice bob))\n\
         !(add-atom &self (parent bob carol))\n\
         !(add-atom &self (parent carol dan))\n\
         !(add-atom &self (color sky blue))\n\
         !(save-space! \"{name}\")\n\
         !(query-act \"{name}\" (parent $x $y) (parent $x $y))"
    );
    let last = eval_metta_last(&src);
    let strs: Vec<String> = last.iter().map(|v| format!("{v:?}")).collect();
    assert_eq!(
        last.len(),
        3,
        "expected 3 out-of-core (parent _ _) matches, got {strs:?}"
    );
    assert!(
        strs.iter().all(|s| s.contains("parent")),
        "every result should be a (parent _ _) fact, got {strs:?}"
    );
}

#[test]
fn surface_query_projects_template() {
    let name = unique("project");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (edge a b))\n\
         !(add-atom &self (edge b c))\n\
         !(save-space! \"{name}\")\n\
         !(query-act \"{name}\" (edge $u $v) (reachable $v $u))"
    );
    let last = eval_metta_last(&src);
    let strs: Vec<String> = last.iter().map(|v| format!("{v:?}")).collect();
    assert_eq!(last.len(), 2, "expected 2 projected matches, got {strs:?}");
    assert!(
        strs.iter().all(|s| s.contains("reachable")),
        "every result should be a (reachable _ _) projection, got {strs:?}"
    );
}

#[test]
fn surface_query_no_match_is_empty() {
    let name = unique("nomatch");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (parent alice bob))\n\
         !(save-space! \"{name}\")\n\
         !(query-act \"{name}\" (sibling $x $y) (sibling $x $y))"
    );
    let last = eval_metta_last(&src);
    assert!(last.is_empty(), "no (sibling _ _) facts; got {last:?}");
}

#[test]
fn surface_save_returns_path() {
    let name = unique("path");
    let _c = Cleanup(name.clone());
    let src = format!("!(add-atom &self (x 1))\n!(save-space! \"{name}\")");
    let last = eval_metta_last(&src);
    assert_eq!(last.len(), 1, "save-space! returns one value (the path)");
    let path = last[0]
        .as_string()
        .expect("save-space! returns a String path");
    assert!(
        path.ends_with(&format!("{name}.act")),
        "path should name the .act file, got {path:?}"
    );
    assert!(
        std::path::Path::new(path).exists(),
        "the .act file should exist at {path:?}"
    );
}

#[test]
fn surface_load_returns_insertion_count() {
    let name = unique("load");
    let _c = Cleanup(name.clone());
    // Save two distinct facts, then restore them into the same space.
    let src = format!(
        "!(add-atom &self (item a))\n\
         !(add-atom &self (item b))\n\
         !(save-space! \"{name}\")\n\
         !(load-space! \"{name}\")"
    );
    let last = eval_metta_last(&src);
    assert_eq!(last.len(), 1, "load-space! returns one value (the count)");
    assert_eq!(
        last[0].as_long(),
        Some(2),
        "load-space! should report Σ-multiplicity = 2 insertions, got {:?}",
        last[0]
    );
}

#[test]
fn surface_save_arity_error() {
    // Wrong arity → graceful error value, not a panic.
    let last = eval_metta_last("!(save-space!)");
    assert_eq!(last.len(), 1);
    assert!(
        format!("{:?}", last[0]).to_lowercase().contains("error")
            || format!("{:?}", last[0]).contains("requires"),
        "expected an arity error, got {:?}",
        last[0]
    );
}
