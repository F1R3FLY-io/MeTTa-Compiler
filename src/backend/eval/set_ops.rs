//! Generic Set Operations for MeTTa
//!
//! Implements set operations for MeTTa tuples with **multiset semantics**.
//!
//! ## Semantics Overview
//!
//! | Operation | Semantics | Example |
//! |-----------|-----------|---------|
//! | `unique-atom` | Remove duplicates by **alpha-equivalence** (matches MeTTa HE) | `($x $y)` → `($x)` |
//! | `alpha-unique-atom` | Explicit alias of `unique-atom` (alpha-equivalence) | `($x $y)` → `($x)` |
//! | `struct-unique-atom` | Remove duplicates by **structural equality** (`PartialEq`) | `($x $y)` → `($x $y)` |
//! | `union-atom` | Concatenate (preserves all multiplicities) | `(a b)` ∪ `(b c)` → `(a b b c)` |
//! | `intersection-atom` | min(left count, right count) | `(a b c c)` ∩ `(b c c c d)` → `(b c c)` |
//! | `subtraction-atom` | left count − right count (saturating) | `(a b b c)` − `(b c c d)` → `(a b)` |
//!
//! ## `unique-atom` semantics — MeTTa HE-compatible
//!
//! `unique-atom` uses **alpha-equivalence** dedup (`atoms_are_alpha_equivalent`),
//! matching **MeTTa HE's `UniqueAtomOp`** in `lib/src/metta/runner/stdlib/atom.rs`.
//! Under this semantics, `(unique-atom ($x $y))` returns `($x)` because the two
//! free variables are alpha-equivalent (one can be obtained from the other by
//! consistent variable renaming).
//!
//! `alpha-unique-atom` is an explicit alias of `unique-atom`, kept for callers
//! who want to spell out their intent.
//!
//! `struct-unique-atom` uses **structural equality** (Rust `PartialEq` —
//! variables with different names are NOT considered equal). This matches
//! **PeTTa's `unique-atom`** (`metta.pl:114`, `list_to_set/2`) and is offered
//! as an explicit name for callers who specifically want the byte-identity
//! semantics. For ground (variable-free) lists, all three functions produce
//! identical results.
//!
//! Note: an earlier B10 work in this branch had temporarily flipped
//! `unique-atom` to structural equality (PeTTa-compatible). That divergence
//! has been reverted in favor of MeTTa HE-faithfulness. The `struct-unique-atom`
//! built-in preserves access to the structural semantics under an explicit name.
//!
//! ## Comparison Semantics (other ops)
//!
//! - `intersection-atom`, `subtraction-atom`: Uses **structural equality** (`Hash`/`Eq`)
//!   which matches MeTTa HE's `==` for these operations
//! - `union-atom`: Simple concatenation, no comparison needed
//!
//! ## References
//!
//! - MeTTa HE: `hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs`
//! - PeTTa: `<PeTTa>/src/metta.pl:112-138`

use std::collections::HashMap;

use smallvec::smallvec;

use crate::backend::eval::alpha_equiv::atoms_are_alpha_equivalent;
use crate::backend::eval::trampoline::{EvalContext, MettaEnvironment};
use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueTrait};

use super::step::GenericEvalStep;

/// Dispatch a set operation by name.
///
/// Called from `sexpr.rs` when a set operation is encountered.
/// Extracts the operation name from `items[0]` to avoid borrow conflicts.
/// All set operations return `GenericEvalStep::Done(...)`.
pub fn eval_set_op_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    let op = items[0]
        .as_atom()
        .expect("set_ops dispatch: head must be atom");
    match op {
        "unique-atom" => eval_unique_atom_generic(items, env, ctx),
        "alpha-unique-atom" => eval_alpha_unique_atom_generic(items, env, ctx),
        "struct-unique-atom" => eval_struct_unique_atom_generic(items, env, ctx),
        "union-atom" => eval_union_atom_generic(items, env, ctx),
        "intersection-atom" => eval_intersection_atom_generic(items, env, ctx),
        "subtraction-atom" => eval_subtraction_atom_generic(items, env, ctx),
        _ => unreachable!("eval_set_op_generic called with unknown op: {}", op),
    }
}

/// Extract a list from a value, returning either the S-expression items or
/// an error message if the value is not a list.
///
/// Unit `()` is treated as an empty list (matching MeTTa HE behavior).
fn extract_list<V: MettaValueTrait>(v: &V) -> Result<&[V], ()> {
    if let Some(items) = v.as_sexpr() {
        Ok(items)
    } else if v.is_unit() {
        // Unit () is treated as empty list
        Ok(&[])
    } else {
        Err(())
    }
}

/// `(unique-atom $list)` — Remove duplicate elements using **alpha-equivalence**.
///
/// **Semantics**: matches **MeTTa HE's `unique-atom`** (in
/// `lib/src/metta/runner/stdlib/atom.rs`, the `UniqueAtomOp` struct).
/// Two atoms are considered duplicates iff they are alpha-equivalent —
/// one can be obtained from the other by consistent variable renaming, and
/// variable-repetition patterns must match. Free variables in the same
/// positions are equivalent.
///
/// Examples:
///   `(unique-atom (a b a c b))` → `(a b c)`  (ground terms — works under
///       any reasonable equality)
///   `(unique-atom ($x $y))` → `($x)`  (alpha-equivalent: both are single
///       free variables)
///   `(unique-atom (($x $x) ($y $y)))` → `(($x $x))`  (same variable-
///       repetition pattern)
///   `(unique-atom (($x $x) ($x $y)))` → both kept (different patterns)
///
/// `alpha-unique-atom` is an explicit alias of this function with identical
/// semantics, kept for callers who want to spell out their intent.
///
/// For PeTTa-compatible **structural** dedup (where variables with
/// different names are NOT considered equal), use `struct-unique-atom`.
///
/// Preserves order of first occurrences.
fn eval_unique_atom_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    if items.len() != 2 {
        let arg_count = items.len() - 1;
        let err = ctx.factory().error(
            ctx.factory().sexpr(items),
            ctx.factory().string(&format!(
                "unique-atom requires exactly 1 argument, got {}. Usage: (unique-atom list)",
                arg_count
            )),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let list_items = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[1].clone(),
                ctx.factory().string("unique-atom: argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // O(n²) alpha-equivalence dedup — matches MeTTa HE's `UniqueAtomOp`.
    // Variables can be renamed consistently; variable-repetition patterns
    // must match.
    let mut seen: Vec<MettaValue> = Vec::with_capacity(list_items.len());
    for item in list_items {
        let already_seen = seen.iter().any(|s| atoms_are_alpha_equivalent(s, item));
        if !already_seen {
            seen.push(item.clone());
        }
    }

    let result = ctx.factory().sexpr(seen);
    GenericEvalStep::Done((smallvec![result], env))
}

/// `(struct-unique-atom $list)` — Remove duplicate elements using
/// **structural equality** (`PartialEq`).
///
/// **Semantics**: PeTTa-compatible byte-identity dedup. Two atoms are
/// considered duplicates iff they are structurally identical — variables
/// with the same name match; variables with different names do NOT.
/// This matches **PeTTa's `unique-atom`** (`metta.pl:114`, `list_to_set/2`).
///
/// MeTTaTron's `unique-atom` uses alpha-equivalence (matching MeTTa HE).
/// `struct-unique-atom` is offered as an explicit name for callers who
/// specifically want the byte-identity semantics.
///
/// Examples:
///   `(struct-unique-atom (a b a c b))` → `(a b c)`  (ground — agrees with
///       `unique-atom` and `alpha-unique-atom`)
///   `(struct-unique-atom ($x $y))` → `($x $y)`  (distinct names — both
///       kept; compare with `unique-atom` which collapses to `($x)`)
///   `(struct-unique-atom ($x $x $y))` → `($x $y)`  (byte-identical $x's
///       are deduped)
///
/// Preserves order of first occurrences.
fn eval_struct_unique_atom_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    if items.len() != 2 {
        let arg_count = items.len() - 1;
        let err = ctx.factory().error(
            ctx.factory().sexpr(items),
            ctx.factory().string(&format!(
                "struct-unique-atom requires exactly 1 argument, got {}. \
                 Usage: (struct-unique-atom list)",
                arg_count
            )),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let list_items = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[1].clone(),
                ctx.factory().string("struct-unique-atom: argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // O(n²) structural-equality dedup — matches PeTTa's `list_to_set/2`.
    // Uses MettaValue's PartialEq, which compares structurally (variables
    // with different names are NOT considered equal — see `unique-atom`
    // for the alpha-equivalence semantics).
    let mut seen: Vec<MettaValue> = Vec::with_capacity(list_items.len());
    for item in list_items {
        let already_seen = seen.iter().any(|s| s == item);
        if !already_seen {
            seen.push(item.clone());
        }
    }

    let result = ctx.factory().sexpr(seen);
    GenericEvalStep::Done((smallvec![result], env))
}

/// `(alpha-unique-atom $list)` — Remove duplicate elements using **alpha-equivalence**.
///
/// **Semantics**: Two atoms are considered duplicates iff they are
/// alpha-equivalent (one can be obtained from the other by consistent
/// variable renaming). Free variables in the same positions are equivalent;
/// variable-repetition patterns must match.
///
/// This **matches MeTTa HE's `unique-atom`** semantics (HE's `UniqueAtomOp`
/// at `lib/src/metta/runner/stdlib/atom.rs:15-43`) and **PeTTa's
/// `alpha-unique-atom`** (`metta.pl:117`, introduced in commit `fe99ffb`).
///
/// **Use this when** you want HE-compatible dedup behavior. For
/// PeTTa-compatible structural dedup (and MeTTaTron's default `unique-atom`),
/// use `unique-atom` instead.
///
/// Examples:
///   `(alpha-unique-atom ($x $y))` → `($x)`  (alpha-equivalent: both are
///       single free variables)
///   `(alpha-unique-atom (a b a))` → `(a b)`  (same as `unique-atom` for
///       ground terms)
///   `(alpha-unique-atom (($x $x) ($y $y)))` → `(($x $x))`  (both have
///       the same variable-repetition pattern)
///   `(alpha-unique-atom (($x $x) ($x $y)))` → both kept (different
///       variable-repetition patterns)
///
/// Preserves order of first occurrences.
fn eval_alpha_unique_atom_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    if items.len() != 2 {
        let arg_count = items.len() - 1;
        let err = ctx.factory().error(
            ctx.factory().sexpr(items),
            ctx.factory().string(&format!(
                "alpha-unique-atom requires exactly 1 argument, got {}. \
                 Usage: (alpha-unique-atom list)",
                arg_count
            )),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let list_items = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[1].clone(),
                ctx.factory().string("alpha-unique-atom: argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // O(n²) alpha-equivalence dedup — matches MeTTa HE's `unique-atom` and
    // PeTTa's `alpha-unique-atom`. Variables can be renamed consistently;
    // variable-repetition patterns must match.
    let mut seen: Vec<MettaValue> = Vec::with_capacity(list_items.len());
    for item in list_items {
        let already_seen = seen.iter().any(|s| atoms_are_alpha_equivalent(s, item));
        if !already_seen {
            seen.push(item.clone());
        }
    }

    let result = ctx.factory().sexpr(seen);
    GenericEvalStep::Done((smallvec![result], env))
}

/// `(union-atom $left $right)` — Multiset union (concatenation).
///
/// All elements from both lists are preserved. Left elements first, then right.
fn eval_union_atom_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    if items.len() != 3 {
        let arg_count = items.len() - 1;
        let err = ctx.factory().error(
            ctx.factory().sexpr(items),
            ctx.factory().atom("IncorrectNumberOfArguments"),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let left = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[1].clone(),
                ctx.factory().string("union-atom: left argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let right = match extract_list(&items[2]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[2].clone(),
                ctx.factory().string("union-atom: right argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let mut combined = Vec::with_capacity(left.len() + right.len());
    combined.extend_from_slice(left);
    combined.extend_from_slice(right);

    let result = ctx.factory().sexpr(combined);
    GenericEvalStep::Done((smallvec![result], env))
}

/// `(intersection-atom $left $right)` — Multiset intersection.
///
/// For each element, result contains min(left count, right count).
/// Uses structural equality (`Hash`/`Eq`) matching MeTTa HE's `==`.
/// Order preserved from left input.
fn eval_intersection_atom_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    if items.len() != 3 {
        let arg_count = items.len() - 1;
        let err = ctx.factory().error(
            ctx.factory().sexpr(items),
            ctx.factory().atom("IncorrectNumberOfArguments"),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let left = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[1].clone(),
                ctx.factory().string("intersection-atom: left argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let right = match extract_list(&items[2]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[2].clone(),
                ctx.factory().string("intersection-atom: right argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // Build count map from right list using hash_value() for keys
    // Maps hash → Vec<(value, remaining_count)> to handle hash collisions correctly
    let mut right_by_hash: HashMap<u64, Vec<(MettaValue, usize)>> =
        HashMap::with_capacity(right.len());

    for item in right {
        let h = item.hash_value();
        let entry = right_by_hash.entry(h).or_default();
        let mut found = false;
        for (existing, count) in entry.iter_mut() {
            if existing == item {
                *count += 1;
                found = true;
                break;
            }
        }
        if !found {
            entry.push((item.clone(), 1));
        }
    }

    let mut result = Vec::with_capacity(left.len());
    for item in left {
        let h = item.hash_value();
        if let Some(entries) = right_by_hash.get_mut(&h) {
            for (existing, count) in entries.iter_mut() {
                if existing == item && *count > 0 {
                    *count -= 1;
                    result.push(item.clone());
                    break;
                }
            }
        }
    }

    let res = ctx.factory().sexpr(result);
    GenericEvalStep::Done((smallvec![res], env))
}

/// `(subtraction-atom $left $right)` — Multiset subtraction.
///
/// For each element, result contains max(0, left count − right count).
/// Uses structural equality (`Hash`/`Eq`) matching MeTTa HE's `==`.
/// Order preserved from left input.
fn eval_subtraction_atom_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    if items.len() != 3 {
        let arg_count = items.len() - 1;
        let err = ctx.factory().error(
            ctx.factory().sexpr(items),
            ctx.factory().atom("IncorrectNumberOfArguments"),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let left = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[1].clone(),
                ctx.factory().string("subtraction-atom: left argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let right = match extract_list(&items[2]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                items[2].clone(),
                ctx.factory().string("subtraction-atom: right argument must be a list"),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // Build count map from right list (same approach as intersection)
    let mut right_by_hash: HashMap<u64, Vec<(MettaValue, usize)>> =
        HashMap::with_capacity(right.len());
    for item in right {
        let h = item.hash_value();
        let entry = right_by_hash.entry(h).or_default();
        let mut found = false;
        for (existing, count) in entry.iter_mut() {
            if existing == item {
                *count += 1;
                found = true;
                break;
            }
        }
        if !found {
            entry.push((item.clone(), 1));
        }
    }

    let mut result = Vec::with_capacity(left.len());
    for item in left {
        let h = item.hash_value();
        let mut should_skip = false;
        if let Some(entries) = right_by_hash.get_mut(&h) {
            for (existing, count) in entries.iter_mut() {
                if existing == item && *count > 0 {
                    *count -= 1;
                    should_skip = true;
                    break;
                }
            }
        }
        if !should_skip {
            result.push(item.clone());
        }
    }

    let res = ctx.factory().sexpr(result);
    GenericEvalStep::Done((smallvec![res], env))
}

#[cfg(test)]
mod tests {
    use crate::backend::eval::trampoline::{MettaEnvironment, StaticEvalContext};
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::MettaValueFactory;

    use super::*;

    // Helper: extract sexpr items from a result
    fn result_items(results: &[crate::backend::models::MettaValue]) -> Vec<String> {
        assert_eq!(results.len(), 1, "expected exactly 1 result");
        let items = results[0].as_sexpr().expect("expected sexpr result");
        items.iter().map(|v| format!("{}", v)).collect()
    }

    // ======================================================================
    // unique-atom tests
    // ======================================================================

    #[test]
    fn test_unique_atom_basic() {
        let f = global_factory();
        // (unique-atom (a b a c b)) → (a b c)
        let list = f.sexpr(vec![
            f.atom("a"),
            f.atom("b"),
            f.atom("a"),
            f.atom("c"),
            f.atom("b"),
        ]);
        let items = vec![f.atom("unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let names = result_items(&results);
            assert_eq!(names, vec!["a", "b", "c"]);
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_unique_atom_empty() {
        let f = global_factory();
        let list = f.sexpr(vec![]);
        let items = vec![f.atom("unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            // Empty list → Unit (empty sexpr normalizes to unit)
            assert!(results[0].is_unit());
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_unique_atom_alpha_equivalent_collapses_renamed_vars() {
        // MeTTa HE-compatible alpha-equivalence: ($x $y) and ($a $b) are
        // alpha-equivalent (both are 2-tuples of distinct free variables),
        // so they collapse to a single representative.
        let f = global_factory();
        let e1 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let e2 = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        let e3 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]); // alpha-equiv of e1 and e2
        let list = f.sexpr(vec![e1, e2, e3]);
        let items = vec![f.atom("unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let items = results[0].as_sexpr().expect("expected sexpr");
            assert_eq!(
                items.len(),
                1,
                "alpha-equivalence: all three 2-tuples of distinct free variables \
                 collapse to a single representative"
            );
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_struct_unique_atom_keeps_distinct_named_vars() {
        // PeTTa-compatible structural equality: ($x $y) and ($a $b) are
        // distinct (different variable names), so neither is deduped.
        // ($x $y) appearing twice IS deduped (byte-identical).
        let f = global_factory();
        let e1 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let e2 = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        let e3 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]); // dup of e1
        let list = f.sexpr(vec![e1, e2, e3]);
        let items = vec![f.atom("struct-unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_struct_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let items = results[0].as_sexpr().expect("expected sexpr");
            assert_eq!(
                items.len(),
                2,
                "structural dedup: ($x $y) and ($a $b) are distinct, the duplicate ($x $y) is removed"
            );
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_unique_atom_pln_robot_compat() {
        // PLN's Robot.metta uses (unique-atom (collapse ...)) on detection
        // tuples — fully ground terms. Structural and alpha equality
        // coincide on ground inputs, so this test verifies PLN compatibility.
        let f = global_factory();
        let d1 = f.sexpr(vec![
            f.atom("detection"),
            f.atom("frisbee"),
            f.atom("coords1"),
        ]);
        let d2 = f.sexpr(vec![
            f.atom("detection"),
            f.atom("orange"),
            f.atom("coords2"),
        ]);
        let d3 = f.sexpr(vec![
            f.atom("detection"),
            f.atom("frisbee"),
            f.atom("coords1"),
        ]); // duplicate of d1
        let list = f.sexpr(vec![d1, d2, d3]);
        let items = vec![f.atom("unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let items = results[0].as_sexpr().expect("expected sexpr");
            assert_eq!(items.len(), 2, "ground duplicates deduped");
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_alpha_unique_atom_alpha_equiv_dedup() {
        // (alpha-unique-atom (($x $y) ($a $b) ($x $y))) — alpha-equivalence dedup.
        // ($x $y) and ($a $b) ARE alpha-equivalent (both are pairs of two
        // distinct free variables), so only the first is kept.
        // The third is byte-identical to the first, also deduped.
        let f = global_factory();
        let e1 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let e2 = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        let e3 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let list = f.sexpr(vec![e1, e2, e3]);
        let items = vec![f.atom("alpha-unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_alpha_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let items = results[0].as_sexpr().expect("expected sexpr");
            assert_eq!(items.len(), 1, "all three are alpha-equiv, only first kept");
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_alpha_unique_atom_distinguishes_repetition_patterns() {
        // ($x $x) and ($x $y) have DIFFERENT variable-repetition patterns
        // (one variable used twice vs two distinct variables), so they are
        // NOT alpha-equivalent and both should be kept.
        let f = global_factory();
        let e1 = f.sexpr(vec![f.atom("$x"), f.atom("$x")]);
        let e2 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let list = f.sexpr(vec![e1, e2]);
        let items = vec![f.atom("alpha-unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_alpha_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let items = results[0].as_sexpr().expect("expected sexpr");
            assert_eq!(items.len(), 2, "different repetition patterns kept");
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_alpha_unique_atom_ground_matches_unique_atom() {
        // For ground (variable-free) inputs, alpha-unique-atom and
        // unique-atom must produce identical results.
        let f = global_factory();
        let list_for_unique = f.sexpr(vec![f.atom("a"), f.atom("b"), f.atom("a"), f.atom("c")]);
        let list_for_alpha = f.sexpr(vec![f.atom("a"), f.atom("b"), f.atom("a"), f.atom("c")]);

        let ctx = StaticEvalContext::get();

        let step_u = eval_unique_atom_generic(
            vec![f.atom("unique-atom"), list_for_unique],
            MettaEnvironment::default(),
            &ctx,
        );
        let step_a = eval_alpha_unique_atom_generic(
            vec![f.atom("alpha-unique-atom"), list_for_alpha],
            MettaEnvironment::default(),
            &ctx,
        );

        let names_u = if let GenericEvalStep::Done((results, _)) = step_u {
            result_items(&results)
        } else {
            panic!("expected Done from unique-atom");
        };
        let names_a = if let GenericEvalStep::Done((results, _)) = step_a {
            result_items(&results)
        } else {
            panic!("expected Done from alpha-unique-atom");
        };
        assert_eq!(names_u, names_a, "ground inputs: unique == alpha-unique");
    }

    #[test]
    fn test_alpha_unique_atom_empty() {
        let f = global_factory();
        let list = f.sexpr(vec![]);
        let items = vec![f.atom("alpha-unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_alpha_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            assert!(results[0].is_unit());
        } else {
            panic!("expected Done");
        }
    }

    // ======================================================================
    // union-atom tests
    // ======================================================================

    #[test]
    fn test_union_atom_basic() {
        let f = global_factory();
        let left = f.sexpr(vec![f.atom("a"), f.atom("b")]);
        let right = f.sexpr(vec![f.atom("c"), f.atom("d")]);
        let items = vec![f.atom("union-atom"), left, right];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_union_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let names = result_items(&results);
            assert_eq!(names, vec!["a", "b", "c", "d"]);
        } else {
            panic!("expected Done");
        }
    }

    // ======================================================================
    // intersection-atom tests
    // ======================================================================

    #[test]
    fn test_intersection_atom_multiset() {
        let f = global_factory();
        // (intersection-atom (a b c c) (b c c c d)) → (b c c)
        let left = f.sexpr(vec![f.atom("a"), f.atom("b"), f.atom("c"), f.atom("c")]);
        let right = f.sexpr(vec![
            f.atom("b"),
            f.atom("c"),
            f.atom("c"),
            f.atom("c"),
            f.atom("d"),
        ]);
        let items = vec![f.atom("intersection-atom"), left, right];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_intersection_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let names = result_items(&results);
            assert_eq!(names, vec!["b", "c", "c"]);
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_intersection_atom_empty_result() {
        let f = global_factory();
        let left = f.sexpr(vec![f.atom("a"), f.atom("b")]);
        let right = f.sexpr(vec![f.atom("c"), f.atom("d")]);
        let items = vec![f.atom("intersection-atom"), left, right];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_intersection_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            // Empty result → Unit
            assert!(results[0].is_unit());
        } else {
            panic!("expected Done");
        }
    }

    // ======================================================================
    // subtraction-atom tests
    // ======================================================================

    #[test]
    fn test_subtraction_atom_multiset() {
        let f = global_factory();
        // (subtraction-atom (a b b c) (b c c d)) → (a b)
        let left = f.sexpr(vec![f.atom("a"), f.atom("b"), f.atom("b"), f.atom("c")]);
        let right = f.sexpr(vec![f.atom("b"), f.atom("c"), f.atom("c"), f.atom("d")]);
        let items = vec![f.atom("subtraction-atom"), left, right];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_subtraction_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let names = result_items(&results);
            assert_eq!(names, vec!["a", "b"]);
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_subtraction_atom_non_list_error() {
        let f = global_factory();
        let items = vec![f.atom("subtraction-atom"), f.long(42), f.long(43)];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_subtraction_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            assert!(results[0].is_error());
        } else {
            panic!("expected Done");
        }
    }
}
