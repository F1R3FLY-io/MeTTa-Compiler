//! Integration tests for AtomSpace bidirectional matching and variable freshening.
//!
//! These tests exercise the HE-equivalent semantics implemented in the
//! unified AtomSpace:
//!
//! 1. Bidirectional matching — stored atoms with variables match concrete patterns
//! 2. Variable freshening — stored atom variables are renamed to prevent capture
//! 3. get-atoms freshening — each returned variable atom gets independent freshened names
//! 4. Multiplicity preservation through add/remove operations

use mettatron::{compile, eval, new_env, MettaValue, MettaValueInner};

/// Helper: evaluate MeTTa source and return results as a flat Vec
#[allow(dead_code)]
fn eval_metta(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compilation should succeed");
    let mut env = new_env();
    let mut all_results = Vec::new();
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let src: Vec<MettaValue> = state.source().iter().copied().collect();
    for &expr in &src {
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        all_results.extend(results);
    }
    all_results
}

/// Helper: evaluate MeTTa source and return the last result set
fn eval_metta_last(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compilation should succeed");
    let mut env = new_env();
    let mut last_results = Vec::new();
    // SAFE: MutexGuard dropped at semicolon, before eval() runs.
    // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
    let src: Vec<MettaValue> = state.source().iter().copied().collect();
    for &expr in &src {
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        last_results = results;
    }
    last_results
}

// ============================================================
// Basic Space Operations with Ground Atoms
// ============================================================

#[test]
fn test_owned_space_add_and_match_ground() {
    // Ground atoms should work as before: add to space, match by pattern
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (foo 42))
        !(match &kb (foo $x) $x)
    "#);
    assert_eq!(results.len(), 1, "Expected 1 match result");
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(42)),
        "Expected 42, got {:?}", results[0]
    );
}

#[test]
fn test_owned_space_add_and_match_multiple_ground() {
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (pair 1 2))
        !(add-atom &kb (pair 3 4))
        !(match &kb (pair $a $b) (+ $a $b))
    "#);
    // Should get two results: (+ 1 2) and (+ 3 4)
    assert_eq!(results.len(), 2, "Expected 2 match results, got {}", results.len());
}

#[test]
fn test_owned_space_remove_ground() {
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (fact 1))
        !(add-atom &kb (fact 2))
        !(remove-atom &kb (fact 1))
        !(match &kb (fact $x) $x)
    "#);
    // After removing (fact 1), only (fact 2) should match
    assert_eq!(results.len(), 1, "Expected 1 match result after remove");
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(2)),
        "Expected 2, got {:?}", results[0]
    );
}

// ============================================================
// Bidirectional Matching — Stored Variable Atoms
// ============================================================

#[test]
fn test_owned_space_variable_atom_match_concrete() {
    // Store (foo $x) in space, then match with concrete pattern (foo 42).
    // Bidirectional matching should find (foo $x) and bind $x → 42.
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (foo $x))
        !(match &kb (foo 42) found)
    "#);
    assert_eq!(results.len(), 1, "Expected 1 match for stored variable atom");
    assert!(
        matches!(results[0].inner(), MettaValueInner::Atom(ref s) if *s == "found"),
        "Expected 'found', got {:?}", results[0]
    );
}

#[test]
fn test_owned_space_variable_atom_same_var_constraint() {
    // Store (same $x $x) — same variable used twice.
    // Pattern (same $a $a) constrains $a to be equal.
    // The stored atom has $x in both positions (freshened to same name),
    // so this SHOULD match.
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (same $x $x))
        !(match &kb (same $a $a) found)
    "#);
    assert_eq!(results.len(), 1, "Expected 1 match for same-var constraint");
    assert!(
        matches!(results[0].inner(), MettaValueInner::Atom(ref s) if *s == "found"),
        "Expected 'found', got {:?}", results[0]
    );
}

#[test]
fn test_owned_space_variable_atom_different_vars_unification() {
    // Store (pair $x $y) — different variables.
    // Pattern (pair $a $a) constrains both positions to be equal.
    // Under full unification (HE semantics), freshened $x_fr and $y_fr are
    // free variables that CAN be equated: $a → $x_fr, $x_fr → $y_fr.
    // So the match SUCCEEDS — the stored atom includes pairs where both
    // elements are equal (e.g., (pair 1 1)), which satisfies the constraint.
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (pair $x $y))
        !(match &kb (pair $a $a) found)
    "#);
    assert_eq!(results.len(), 1, "Expected 1 match via unification of free stored-side variables");
    assert!(
        matches!(results[0].inner(), MettaValueInner::Atom(ref s) if *s == "found"),
        "Expected 'found', got {:?}", results[0]
    );
}

#[test]
fn test_owned_space_remove_variable_atom() {
    // Variable atoms should be removable
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (foo $x))
        !(remove-atom &kb (foo $x))
        !(match &kb (foo 42) found)
    "#);
    assert!(results.is_empty(), "Expected no matches after removing variable atom, got {:?}", results);
}

// ============================================================
// Chain Resolution — Pattern Constrains Stored Variable
// ============================================================

#[test]
fn test_owned_space_chain_resolution() {
    // Store (chain $x $x) — same variable both positions.
    // Pattern (chain $a 42) — binds second position to 42.
    // Chain resolution: $x_fr → 42 (from pattern match), then $a → $x_fr → 42.
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (chain $x $x))
        !(match &kb (chain $a 42) $a)
    "#);
    assert_eq!(results.len(), 1, "Expected 1 match with chain resolution");
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(42)),
        "Expected 42 from chain resolution, got {:?}", results[0]
    );
}

// ============================================================
// get-atoms Freshening
// ============================================================

#[test]
fn test_get_atoms_includes_variable_atoms() {
    // get-atoms should return both ground and variable atoms
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (ground 42))
        !(add-atom &kb (has-var $x))
        !(get-atoms &kb)
    "#);
    // Should return both atoms (each as a separate superposition element)
    assert_eq!(results.len(), 2, "Expected 2 atoms from get-atoms, got {}", results.len());
}

// ============================================================
// Mixed Ground + Variable Atoms
// ============================================================

#[test]
fn test_mixed_ground_and_variable_match() {
    // Both ground and variable atoms should be found by appropriate patterns
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (data 42))
        !(add-atom &kb (data $y))
        !(match &kb (data 42) found)
    "#);
    // (data 42) matches directly (ground), (data $y) matches via bidirectional ($y → 42)
    assert_eq!(results.len(), 2, "Expected 2 matches (ground + variable), got {}", results.len());
}

#[test]
fn test_variable_only_pattern_matches_all() {
    // Pattern ($a $b) should match all binary atoms in the space
    let results = eval_metta_last(r#"
        !(bind! &kb (new-space))
        !(add-atom &kb (foo 1))
        !(add-atom &kb (bar 2))
        !(add-atom &kb (baz $x))
        !(match &kb ($a $b) ($a $b))
    "#);
    // Should match all 3 atoms
    assert_eq!(results.len(), 3, "Expected 3 matches for wildcard pattern, got {}", results.len());
}
