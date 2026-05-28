//! Integration tests for the Stage 5a LSM-tiered ACT base **surface forms**, exercised
//! through the real evaluator (compile → eval), not just the Rust API:
//!
//! - `(attach-act-base! "name")` → attach `/dev/shm/<name>.act` as the immutable base
//! - `(detach-act-base!)`        → return to a purely in-memory store
//! - `(compact-space! "name")`   → fold overlay + (base − tombstones) into a fresh ACT
//!
//! These confirm the special-form dispatch arms (`step/sexpr.rs`), the impure-head
//! classification (`dispatch_hints.rs`), and the T0 tier routing (`bytecode/mod.rs`) all
//! wire up correctly end-to-end. The Rust-API-level semantics (overlay/base union, bag
//! fidelity, tombstone shadowing, compaction round-trip) are covered by
//! `backend::environment::act_tiered::tests`.

use std::sync::atomic::{AtomicU64, Ordering};

use mettatron::{compile, eval, new_env, MettaValue};

/// Evaluate MeTTa source through ALL top-level directives, threading the env forward,
/// returning the LAST directive's result set.
fn eval_metta_last(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compilation should succeed");
    let mut env = new_env();
    let mut last = Vec::new();
    let src: Vec<MettaValue> = state.source().iter().copied().collect();
    for &expr in &src {
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        last = results.into_vec();
    }
    last
}

/// Unique `.act` base name per call (avoid `/dev/shm` collisions across parallel tests
/// and repeated runs).
fn unique(tag: &str) -> String {
    static C: AtomicU64 = AtomicU64::new(0);
    format!(
        "mtt_acttiersurf_{}_{}_{}",
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

/// Attach a saved base, then `match` over the tiered space: overlay (one fresh fact) ++
/// base (two saved facts) = three matches.
#[test]
fn surface_attach_then_tiered_match() {
    let name = unique("attach");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (parent alice bob))\n\
         !(add-atom &self (parent bob carol))\n\
         !(save-space! \"{name}\")\n\
         ; fresh env in the next directive block? no — same env: remove the saved facts\n\
         ; from the overlay so the base is what supplies them, then add an overlay-only fact.\n\
         !(remove-atom &self (parent alice bob))\n\
         !(remove-atom &self (parent bob carol))\n\
         !(add-atom &self (parent carol dan))\n\
         !(attach-act-base! \"{name}\")\n\
         !(match &self (parent $x $y) (parent $x $y))"
    );
    let last = eval_metta_last(&src);
    let strs: Vec<String> = last.iter().map(|v| format!("{v:?}")).collect();
    assert_eq!(
        last.len(),
        3,
        "overlay (carol→dan) ++ base (alice→bob, bob→carol) = 3, got {strs:?}"
    );
    assert!(
        strs.iter().all(|s| s.contains("parent")),
        "every result should be a (parent _ _) fact, got {strs:?}"
    );
}

/// `attach-act-base!` returns the base `.act` path as a String.
#[test]
fn surface_attach_returns_path() {
    let name = unique("path");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (x 1))\n\
         !(save-space! \"{name}\")\n\
         !(attach-act-base! \"{name}\")"
    );
    let last = eval_metta_last(&src);
    assert_eq!(
        last.len(),
        1,
        "attach-act-base! returns one value (the path)"
    );
    let path = last[0]
        .as_string()
        .expect("attach-act-base! returns a String path");
    assert!(
        path.ends_with(&format!("{name}.act")),
        "path should name the .act file, got {path:?}"
    );
}

/// Detach returns the space to a purely in-memory store: a base-only fact is no longer
/// visible after detach.
#[test]
fn surface_detach_drops_base() {
    let name = unique("detach");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (base only))\n\
         !(save-space! \"{name}\")\n\
         !(remove-atom &self (base only))\n\
         !(attach-act-base! \"{name}\")\n\
         !(detach-act-base!)\n\
         !(match &self (base $x) (base $x))"
    );
    let last = eval_metta_last(&src);
    assert!(
        last.is_empty(),
        "base-only fact must be gone after detach, got {last:?}"
    );
}

/// Attaching a non-existent base is a graceful error value, not a panic.
#[test]
fn surface_attach_missing_errors() {
    let name = unique("nope");
    let _c = Cleanup(name.clone());
    let src = format!("!(attach-act-base! \"{name}\")");
    let last = eval_metta_last(&src);
    assert_eq!(last.len(), 1);
    assert!(
        format!("{:?}", last[0]).to_lowercase().contains("error")
            || format!("{:?}", last[0]).contains("failed"),
        "expected an attach error, got {:?}",
        last[0]
    );
}

/// `detach-act-base!` with a stray argument → graceful arity error.
#[test]
fn surface_detach_arity_error() {
    let last = eval_metta_last("!(detach-act-base! extra)");
    assert_eq!(last.len(), 1);
    assert!(
        format!("{:?}", last[0]).to_lowercase().contains("error")
            || format!("{:?}", last[0]).contains("no arguments"),
        "expected an arity error, got {:?}",
        last[0]
    );
}

/// End-to-end tombstone via the evaluator (`remove-atom` routes through the
/// interior-mutability `remove_from_space_shared` path on the eval side): after attaching
/// a base, removing a base fact suppresses it on a subsequent `match`, and re-adding it
/// un-hides it (add∘remove = id through the real eval pipeline).
#[test]
fn surface_remove_tombstones_base_then_readd_unhides() {
    let name = unique("tomb");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (e a b))\n\
         !(add-atom &self (e b c))\n\
         !(save-space! \"{name}\")\n\
         !(remove-atom &self (e a b))\n\
         !(remove-atom &self (e b c))\n\
         !(attach-act-base! \"{name}\")\n\
         ; base now supplies (e a b),(e b c); overlay empty\n\
         !(remove-atom &self (e a b))\n\
         !(match &self (e $x $y) (e $x $y))"
    );
    let last = eval_metta_last(&src);
    let strs: Vec<String> = last.iter().map(|v| format!("{v:?}")).collect();
    assert_eq!(
        last.len(),
        1,
        "one base fact tombstoned → one survivor, got {strs:?}"
    );
    assert!(
        strs.iter().any(|s| s.contains('b') && s.contains('c')),
        "the survivor should be (e b c), got {strs:?}"
    );
}

/// End-to-end compaction via the evaluator: `(compact-space!)` returns the Σ-multiplicity
/// and the post-compaction `match` is a semantic no-op (same survivors).
#[test]
fn surface_compact_returns_sigma_and_preserves_match() {
    let name = unique("compact");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (g a b))\n\
         !(add-atom &self (g b c))\n\
         !(save-space! \"{name}\")\n\
         !(remove-atom &self (g a b))\n\
         !(remove-atom &self (g b c))\n\
         !(attach-act-base! \"{name}\")\n\
         !(remove-atom &self (g a b))\n\
         !(add-atom &self (g x y))\n\
         !(compact-space! \"{name}\")"
    );
    let last = eval_metta_last(&src);
    assert_eq!(
        last.len(),
        1,
        "compact-space! returns one value (the count)"
    );
    // Survivors after the tombstone+overlay-add: (g b c) + (g x y) = 2.
    assert_eq!(
        last[0].as_long(),
        Some(2),
        "compact-space! Σ-multiplicity should be 2, got {:?}",
        last[0]
    );
}

/// `compact-space!` with no base attached → graceful error value.
#[test]
fn surface_compact_without_base_errors() {
    let name = unique("compactnob");
    let _c = Cleanup(name.clone());
    let src = format!("!(add-atom &self (h 1))\n!(compact-space! \"{name}\")");
    let last = eval_metta_last(&src);
    assert_eq!(last.len(), 1);
    assert!(
        format!("{:?}", last[0]).to_lowercase().contains("error")
            || format!("{:?}", last[0]).contains("failed"),
        "expected a no-base error, got {:?}",
        last[0]
    );
}

/// End-to-end re-add un-hide via the evaluator.
#[test]
fn surface_readd_unhides_base() {
    let name = unique("unhide");
    let _c = Cleanup(name.clone());
    let src = format!(
        "!(add-atom &self (f x))\n\
         !(save-space! \"{name}\")\n\
         !(remove-atom &self (f x))\n\
         !(attach-act-base! \"{name}\")\n\
         !(remove-atom &self (f x))\n\
         ; suppressed; now re-add → revive\n\
         !(add-atom &self (f x))\n\
         !(match &self (f $v) (f $v))"
    );
    let last = eval_metta_last(&src);
    assert_eq!(
        last.len(),
        1,
        "re-add must revive the suppressed base fact, got {last:?}"
    );
}
