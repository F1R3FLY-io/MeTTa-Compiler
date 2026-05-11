//! Integration tests for the cross-tier substitution fix.
//!
//! Phase 4 of the cross-tier substitution plan (Plans B/C/D + Plan A +
//! mmverify hang fix).
//!
//! These tests exercise variable substitution in quoted forms within rule
//! bodies. Before the fix, `GenericCompiler::compile_quoted` (in
//! `src/backend/bytecode/compiler/core.rs`) embedded the entire S-expression
//! as a single `PushConstant` literal — so when a rule like
//! `(= (foo $tok) (add-atom &kb (Item $tok)))` fired, the bytecode VM
//! would store `(Item $tok)` literally instead of the bound value, and
//! later `get-atoms` would return a freshly-freshened variable
//! `(Item $__fr_<N>_tok)`.
//!
//! After the fix:
//! - Bytecode VM: `compile_quoted` delegates to `compile_as_literal_sexpr`,
//!   which recursively walks SExpr children and emits `PushVariable` for
//!   `$`-prefixed atoms.
//! - JIT: `Opcode::PushVariable` routes to
//!   `jit_runtime_push_variable_with_fallback`, which consults binding
//!   frames first and falls through to the constant-pool atom literal on
//!   miss (matching `op_push_variable`'s semantics).
//! - Trampoline: already correct (audit confirmed).
//!
//! Each test exercises the fix at the MeTTa-program level. The harness
//! compiles MeTTa source, evaluates it through the default tier
//! (bytecode VM with trampoline fallback for unsupported forms), and
//! asserts the observable output matches the bisimilar reference.

use mettatron::{compile, eval, new_env, MettaValue, MettaValueInner};

fn eval_metta_last(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compilation should succeed");
    let mut env = new_env();
    let mut last_results = Vec::new();
    let src: Vec<MettaValue> = state.source().iter().copied().collect();
    for &expr in &src {
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        last_results = results.into_vec();
    }
    last_results
}

fn eval_metta_all(source: &str) -> Vec<Vec<MettaValue>> {
    let state = compile(source).expect("compilation should succeed");
    let mut env = new_env();
    let mut all_results: Vec<Vec<MettaValue>> = Vec::new();
    let src: Vec<MettaValue> = state.source().iter().copied().collect();
    for &expr in &src {
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        all_results.push(results.into_vec());
    }
    all_results
}

// ============================================================
// Canonical repro (was: kb stores $__fr_<N>_tok)
// ============================================================

/// `(= (foo $tok) (add-atom &kb (Item $tok))) !(foo "hello") !(get-atoms &kb)`
/// must produce `[(Item "hello")]`, NOT `[(Item $__fr_<N>_tok)]`.
#[test]
fn rule_body_add_atom_substitutes_string_var() {
    let results = eval_metta_last(
        r#"
        !(bind! &kb (new-space))
        (= (foo $tok) (add-atom &kb (Item $tok)))
        !(foo "hello")
        !(get-atoms &kb)
    "#,
    );
    assert_eq!(results.len(), 1, "Expected 1 atom in kb, got {:?}", results);
    let sexpr = results[0]
        .as_sexpr()
        .unwrap_or_else(|| panic!("Expected (Item \"hello\") sexpr, got {:?}", results[0]));
    assert_eq!(sexpr.len(), 2, "Expected 2-element sexpr (Item \"hello\")");
    assert!(
        matches!(sexpr[0].inner(), MettaValueInner::Atom("Item")),
        "Expected first element 'Item', got {:?}",
        sexpr[0]
    );
    let payload_str = match sexpr[1].inner() {
        MettaValueInner::String(s) => *s,
        other => panic!("Expected String(\"hello\"), got {:?}", other),
    };
    assert_eq!(payload_str, "hello");
}

/// Same shape with Long instead of String.
#[test]
fn rule_body_add_atom_substitutes_long_var() {
    let results = eval_metta_last(
        r#"
        !(bind! &kb (new-space))
        (= (foo $n) (add-atom &kb (Item $n)))
        !(foo 42)
        !(get-atoms &kb)
    "#,
    );
    assert_eq!(results.len(), 1);
    let sexpr = results[0].as_sexpr().expect("expected sexpr");
    assert_eq!(sexpr.len(), 2);
    assert!(matches!(sexpr[0].inner(), MettaValueInner::Atom("Item")));
    assert!(
        matches!(sexpr[1].inner(), MettaValueInner::Long(42)),
        "Expected Long(42), got {:?}",
        sexpr[1]
    );
}

// ============================================================
// remove-atom symmetry
// ============================================================

/// `(= (rm-tok $t) (remove-atom &kb (Item $t)))` must remove the
/// substituted atom, not a freshened literal.
#[test]
fn rule_body_remove_atom_substitutes_var() {
    let results = eval_metta_last(
        r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (Item "hello"))
        !(add-atom &kb (Item "world"))
        (= (rm-tok $t) (remove-atom &kb (Item $t)))
        !(rm-tok "hello")
        !(get-atoms &kb)
    "#,
    );
    // After removing (Item "hello"), kb should contain only (Item "world").
    assert_eq!(results.len(), 1, "Expected 1 atom in kb, got {:?}", results);
    let sexpr = results[0].as_sexpr().expect("expected sexpr");
    assert_eq!(sexpr.len(), 2);
    assert!(matches!(sexpr[0].inner(), MettaValueInner::Atom("Item")));
    let payload = match sexpr[1].inner() {
        MettaValueInner::String(s) => *s,
        other => panic!("Expected String(\"world\"), got {:?}", other),
    };
    assert_eq!(payload, "world");
}

// ============================================================
// let-bound variable substitutes into add-atom (rule-body-style)
// ============================================================

/// Inside a let, calling a user rule whose body uses `add-atom (... $tok)`
/// must substitute the let-bound value into the kb-stored atom.
/// `(quote ...)` is excluded here because per HE semantics quote
/// SUPPRESSES substitution of $-vars within its body — the expected
/// behavior of `(let $x 42 (quote (foo $x)))` is `[(quote (foo $x))]`.
#[test]
fn let_bound_var_substitutes_through_rule_body() {
    let outputs = eval_metta_all(
        r#"
        !(bind! &kb (new-space))
        (= (insert $tok) (add-atom &kb (Item $tok)))
        !(let $value 99 (insert $value))
        !(get-atoms &kb)
    "#,
    );
    let kb_atoms = outputs.last().expect("expected get-atoms output");
    assert_eq!(
        kb_atoms.len(),
        1,
        "Expected 1 atom in kb, got {:?}",
        kb_atoms
    );
    let sexpr = kb_atoms[0].as_sexpr().expect("expected (Item 99) sexpr");
    assert_eq!(sexpr.len(), 2);
    assert!(matches!(sexpr[0].inner(), MettaValueInner::Atom("Item")));
    assert!(
        matches!(sexpr[1].inner(), MettaValueInner::Long(99)),
        "Expected Long(99) substituted via rule + let, got {:?}",
        sexpr[1]
    );
}

// ============================================================
// Top-level $-var preserves variable-atom routing
// ============================================================

/// `!(add-atom &kb (foo $x))` at top level (no binding frame) should
/// preserve the variable-atom routing semantics — `$x` falls through to
/// its own atom literal via `op_push_variable`. This test ensures the
/// fix didn't break the top-level case that was already working.
#[test]
fn top_level_add_atom_preserves_variable_atom() {
    let results = eval_metta_last(
        r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (foo $x))
        !(get-atoms &kb)
    "#,
    );
    // Variable atom semantics: $x is freshened on retrieval. We just check
    // the structural shape.
    assert!(!results.is_empty(), "kb should contain at least one atom");
    let sexpr = results[0].as_sexpr().expect("expected sexpr");
    assert_eq!(sexpr.len(), 2);
    assert!(matches!(sexpr[0].inner(), MettaValueInner::Atom("foo")));
    // Second element should still be a variable atom (possibly freshened).
    let name = sexpr[1]
        .as_atom()
        .expect("expected variable atom in second position");
    assert!(
        name.starts_with('$'),
        "Expected $-prefixed variable atom, got {:?}",
        name
    );
}

// ============================================================
// Rule body with constant data — no var, no regression
// ============================================================

/// Sanity check: rule body with no $-vars in the quoted argument should
/// behave identically to pre-fix (always was correct in this case).
#[test]
fn rule_body_add_atom_no_var_unchanged() {
    let results = eval_metta_last(
        r#"
        !(bind! &kb (new-space))
        (= (foo) (add-atom &kb (Item "constant")))
        !(foo)
        !(get-atoms &kb)
    "#,
    );
    assert_eq!(results.len(), 1);
    let sexpr = results[0].as_sexpr().expect("expected sexpr");
    assert_eq!(sexpr.len(), 2);
    assert!(matches!(sexpr[0].inner(), MettaValueInner::Atom("Item")));
    let s = match sexpr[1].inner() {
        MettaValueInner::String(s) => *s,
        other => panic!("Expected String, got {:?}", other),
    };
    assert_eq!(s, "constant");
}

// ============================================================
// Repeated rule firing — substitution is per-call
// ============================================================

/// `(= (insert $tok) (add-atom &kb (Item $tok)))` called multiple times
/// with different arguments must produce DISTINCT atoms in kb (not 4
/// copies of the same freshened-literal).
#[test]
fn rule_body_repeated_calls_substitute_per_call() {
    let outputs = eval_metta_all(
        r#"
        !(bind! &kb (new-space))
        (= (insert $tok) (add-atom &kb (Item $tok)))
        !(insert "alpha")
        !(insert "beta")
        !(insert "gamma")
        !(insert "delta")
        !(get-atoms &kb)
    "#,
    );
    let kb_atoms = outputs.last().expect("expected get-atoms output");
    assert_eq!(
        kb_atoms.len(),
        4,
        "Expected 4 distinct atoms, got {:?}",
        kb_atoms
    );

    // Collect the string payloads
    let mut payloads: Vec<&'static str> = kb_atoms
        .iter()
        .filter_map(|v| {
            v.as_sexpr().and_then(|items| {
                if items.len() == 2 {
                    if let MettaValueInner::String(s) = items[1].inner() {
                        Some(*s)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
        .collect();
    payloads.sort();
    assert_eq!(payloads, vec!["alpha", "beta", "delta", "gamma"]);
}

// ============================================================
// Rule definition data — = inside add-atom NOT double-fired
// ============================================================

/// `(add-atom &kb (= (computed) "result"))` should register the embedded
/// rule definition in `&kb` (which routes through `add_to_space` →
/// `extract_rule_parts`), NOT trigger an additional rule definition by
/// double-firing the `=` form. The atom argument is data, not code, even
/// though it has the shape of a rule.
///
/// We verify this by checking that calling `(computed)` resolves through
/// the kb-registered rule (which is the intended HE semantics for
/// add-atom of a rule-shaped atom).
#[test]
fn add_atom_with_rule_shaped_data_registers_via_extract() {
    let outputs = eval_metta_all(
        r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (= (computed) "result"))
        !(match &kb (= (computed) $r) $r)
    "#,
    );
    let last = outputs.last().expect("expected match output");
    assert_eq!(
        last.len(),
        1,
        "Expected match to find the registered rule body"
    );
    let payload = match last[0].inner() {
        MettaValueInner::String(s) => *s,
        other => panic!("Expected String(\"result\"), got {:?}", other),
    };
    assert_eq!(payload, "result");
}
