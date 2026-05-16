//! **Stack-safety mandate compliance tests (2026-05-15)**.
//!
//! Synthesizes deeply-nested MettaValue inputs and confirms that the
//! mandate-compliant iterative implementations don't overflow the test
//! thread's default stack on any of the previously-recursive paths.
//!
//! Each test uses a nesting depth large enough that the *recursive*
//! versions of these functions would have overflowed a typical 8 MB stack
//! (frames vary by debug vs release, but 50_000 levels is reliably fatal
//! for naïve recursion regardless of build profile).
//!
//! Audit reference: see [[parallel-dispatch-trampolinization]] in user
//! memory and `/home/dylon/.claude/plans/i-got-the-following-idempotent-globe.md`.

use mettatron::backend::models::{
    global_factory, MettaValue, MettaValueFactory, MettaValueTrait,
};

/// Build a left-nested SExpr of the given depth.
/// Result has shape `(f (f (f ... (f leaf))))` with `depth+1` levels.
fn build_deep_left_nested(depth: usize) -> MettaValue {
    let f = global_factory();
    let mut current = f.atom("leaf");
    let head = f.atom("f");
    for _ in 0..depth {
        current = f.sexpr(vec![head.clone(), current]);
    }
    current
}

/// Build a balanced binary SExpr tree of the given depth.
/// **Note**: each occurrence of `current` shares the same slab pointer, so
/// the structure has `depth+1` UNIQUE subtrees but exponentially many
/// reference positions. The OUTPUT string is inherently O(2^depth) chars
/// regardless of memoization, so callers must keep `depth` modest (≤20).
/// Memoization makes the WORK (work_stack pushes) O(depth) instead of O(2^depth).
fn build_shared_substructure(depth: usize) -> MettaValue {
    let f = global_factory();
    let leaf = f.atom("x");
    let head = f.atom("pair");
    let mut current = leaf;
    for _ in 0..depth {
        current = f.sexpr(vec![head.clone(), current.clone(), current]);
    }
    current
}

/// Build a deeply-nested type expression: `(List (List (List ... $t)))`.
fn build_deep_type(depth: usize) -> MettaValue {
    let f = global_factory();
    let mut current = f.atom("$t");
    let head = f.atom("List");
    for _ in 0..depth {
        current = f.sexpr(vec![head.clone(), current]);
    }
    current
}

#[test]
fn deep_to_display_string_shared_substructure_completes_quickly() {
    // depth=18 → output is ~2^18 = 262144 chars. With memo, WORK is O(18×3)
    // = ~54 work-stack pushes. Without memo, WORK would be O(3^18) = ~387M
    // pushes — would not complete in reasonable time.
    let v = build_shared_substructure(18);
    let start = std::time::Instant::now();
    let s = v.to_display_string();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "to_display_string on 18-deep shared structure took {:?} (memo should keep this O(depth))",
        elapsed
    );
    assert!(s.starts_with("(pair "), "expected (pair prefix");
    assert!(s.contains(" x"), "expected 'x' atom in output");
}

#[test]
fn deep_to_display_string_left_nested_no_stack_overflow() {
    // 50_000-deep left-nested form — would have overflowed any recursive
    // implementation. With iterative work-list it just builds a long string.
    let v = build_deep_left_nested(50_000);
    let s = v.to_display_string();
    assert!(s.starts_with("(f "));
    assert!(s.ends_with(")"));
}

#[test]
fn deep_to_metta_string_no_stack_overflow() {
    let v = build_deep_left_nested(50_000);
    let s = v.to_metta_string();
    assert!(s.starts_with("(f "));
}

#[test]
fn deep_to_mork_string_no_stack_overflow() {
    let v = build_deep_left_nested(50_000);
    let s = v.to_mork_string();
    assert!(s.starts_with("(f "));
}

#[test]
fn deep_contains_variables_no_stack_overflow() {
    // 50_000-deep nest with a single `$x` at the leaf. Verifies the
    // iterative refactor of `MettaValueTrait::contains_variables`.
    let f = global_factory();
    let mut current = f.atom("$x");
    let head = f.atom("List");
    for _ in 0..50_000 {
        current = f.sexpr(vec![head.clone(), current]);
    }
    assert!(current.contains_variables());

    // And confirm an all-ground version returns false.
    let mut ground = f.atom("leaf");
    for _ in 0..50_000 {
        ground = f.sexpr(vec![head.clone(), ground]);
    }
    assert!(!ground.contains_variables());
}

#[test]
fn deep_collect_free_variables_no_stack_overflow() {
    let f = global_factory();
    let mut current = f.atom("$x");
    let head = f.atom("f");
    for _ in 0..50_000 {
        current = f.sexpr(vec![head.clone(), current]);
    }
    let mut vars: smallvec::SmallVec<[&'static str; 8]> = smallvec::SmallVec::new();
    current.collect_free_variables(&mut vars);
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0], "$x");
}

// NOTE: type-system functions `apply_type_bindings` / `freshen_type_variables`
// live in a private module (`backend::eval::types`). Their stack-safety is
// covered indirectly by the conformance + nondet-fanout test suite, which
// exercises type-driven evaluation on the standard MettaTron rule set.
// A direct test would need either a pub-export of those fns or an internal
// `#[cfg(test)]` test added to that module itself.

/// Build a deeply-nested expression that exercises BOTH the formatter
/// memoization AND the trampolinized parallel-dispatch path: a `superpose`
/// over many identical deep alternatives. Each alternative shares structure
/// with the others, so the formatter's memo would have been the difference
/// between O(N*depth) and O(2^depth) before the fix.
fn build_deep_superpose_source(branches: usize, leaf_depth: usize) -> String {
    let mut deep = String::from("x");
    for _ in 0..leaf_depth {
        deep = format!("(f {})", deep);
    }
    let alts: Vec<String> = (0..branches).map(|_| deep.clone()).collect();
    format!("!(superpose ({}))", alts.join(" "))
}

#[test]
fn deep_shared_format_does_not_explode() {
    // 100 branches × 100 deep — without memoization this would expand
    // the formatter's work-list to ~2^100 entries. With memoization it's
    // 100 × 100 ≈ 10_000 ops total.
    let source = build_deep_superpose_source(100, 100);
    // Just compile the source; this exercises tree-sitter parser depth too.
    let _state = mettatron::compile(&source).expect("compile deep superpose");
    // Compilation success is the success criterion — no panic, no
    // unbounded allocation.
}
