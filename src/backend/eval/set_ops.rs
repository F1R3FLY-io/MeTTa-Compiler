//! Generic Set Operations for MeTTa
//!
//! Implements set operations for MeTTa tuples with **multiset semantics**,
//! aligned with the MeTTa HE (hyperon-experimental) reference implementation.
//!
//! ## Semantics Overview
//!
//! | Operation | Semantics | Example |
//! |-----------|-----------|---------|
//! | `unique-atom` | Remove duplicates (alpha-equiv), keep first occurrence | `(a b a c b)` → `(a b c)` |
//! | `union-atom` | Concatenate (preserves all multiplicities) | `(a b)` ∪ `(b c)` → `(a b b c)` |
//! | `intersection-atom` | min(left count, right count) | `(a b c c)` ∩ `(b c c c d)` → `(b c c)` |
//! | `subtraction-atom` | left count − right count (saturating) | `(a b b c)` − `(b c c d)` → `(a b)` |
//!
//! ## Comparison Semantics
//!
//! - `unique-atom`: Uses **alpha-equivalence** for deduplication (matching MeTTa HE's
//!   `atoms_are_equivalent()` — O(n²) worst case, same as HE)
//! - `intersection-atom`, `subtraction-atom`: Uses **structural equality** (`Hash`/`Eq`)
//!   which matches MeTTa HE's `==` for these operations
//! - `union-atom`: Simple concatenation, no comparison needed
//!
//! ## Reference
//!
//! See: `hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs`

use std::collections::HashMap;

use smallvec::smallvec;

use crate::backend::eval::alpha_equiv::atoms_are_alpha_equivalent;
use crate::backend::eval::trampoline::{MettaEnvironment, EvalContext};
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
    let op = items[0].as_atom().expect("set_ops dispatch: head must be atom");
    match op {
        "unique-atom" => eval_unique_atom_generic(items, env, ctx),
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

/// `(unique-atom $list)` — Remove duplicate elements using alpha-equivalence.
///
/// Uses O(n²) alpha-equivalence deduplication matching MeTTa HE's approach:
/// for each element, check against `seen` list using `atoms_are_alpha_equivalent()`.
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
        let err = ctx.factory().error(
            &format!(
                "unique-atom requires exactly 1 argument, got {}. Usage: (unique-atom list)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let list_items = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "unique-atom: argument must be a list",
                items[1].clone(),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // O(n²) alpha-equivalence dedup — matches MeTTa HE's approach
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
        let err = ctx.factory().error(
            &format!(
                "union-atom requires exactly 2 arguments, got {}. Usage: (union-atom left right)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let left = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "union-atom: left argument must be a list",
                items[1].clone(),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let right = match extract_list(&items[2]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "union-atom: right argument must be a list",
                items[2].clone(),
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
        let err = ctx.factory().error(
            &format!(
                "intersection-atom requires exactly 2 arguments, got {}. Usage: (intersection-atom left right)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let left = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "intersection-atom: left argument must be a list",
                items[1].clone(),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let right = match extract_list(&items[2]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "intersection-atom: right argument must be a list",
                items[2].clone(),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // Build count map from right list using hash_value() for keys
    // Maps hash → Vec<(value, remaining_count)> to handle hash collisions correctly
    let mut right_by_hash: HashMap<u64, Vec<(MettaValue, usize)>> = HashMap::with_capacity(right.len());

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
        let err = ctx.factory().error(
            &format!(
                "subtraction-atom requires exactly 2 arguments, got {}. Usage: (subtraction-atom left right)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((smallvec![err], env));
    }

    let left = match extract_list(&items[1]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "subtraction-atom: left argument must be a list",
                items[1].clone(),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    let right = match extract_list(&items[2]) {
        Ok(elems) => elems,
        Err(()) => {
            let err = ctx.factory().error(
                "subtraction-atom: right argument must be a list",
                items[2].clone(),
            );
            return GenericEvalStep::Done((smallvec![err], env));
        }
    };

    // Build count map from right list (same approach as intersection)
    let mut right_by_hash: HashMap<u64, Vec<(MettaValue, usize)>> = HashMap::with_capacity(right.len());
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
        let items = results[0]
            .as_sexpr()
            .expect("expected sexpr result");
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
    fn test_unique_atom_alpha_equiv_dedup() {
        let f = global_factory();
        // (unique-atom (($x $y) ($a $b) ($x $y))) → deduplicates with alpha-equiv
        // ($x $y) and ($a $b) are alpha-equivalent, so only first kept
        // ($x $y) appears again, also alpha-equiv to first, so deduped
        let e1 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let e2 = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        let e3 = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let list = f.sexpr(vec![e1, e2, e3]);
        let items = vec![f.atom("unique-atom"), list];

        let ctx = StaticEvalContext::get();
        let env = MettaEnvironment::default();

        let step = eval_unique_atom_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            let items = results[0].as_sexpr().expect("expected sexpr");
            assert_eq!(items.len(), 1, "all three are alpha-equiv, only first kept");
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
        let left = f.sexpr(vec![
            f.atom("a"),
            f.atom("b"),
            f.atom("c"),
            f.atom("c"),
        ]);
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
        let left = f.sexpr(vec![
            f.atom("a"),
            f.atom("b"),
            f.atom("b"),
            f.atom("c"),
        ]);
        let right = f.sexpr(vec![
            f.atom("b"),
            f.atom("c"),
            f.atom("c"),
            f.atom("d"),
        ]);
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
