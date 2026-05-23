//! Grounded Arguments Evaluation
//!
//! This module handles the identification of grounded arguments that need
//! evaluation in a hybrid lazy/eager evaluation strategy.
//!
//! ## MeTTa HE Parity
//!
//! MeTTa HE uses type signatures `(-> ArgType ... RetType)` to decide
//! applicative evaluation: only operators with arrow types trigger pre-evaluation
//! of their arguments. Arguments whose formal type is a meta-type (`Atom`,
//! `Expression`, `Symbol`, `Variable`, `Grounded`, `Pattern`) are passed
//! unevaluated.
//!
//! MeTTaTron adds a bloom filter fallback for untyped operators that may have
//! user-defined rules. The fixpoint detection in `CollectGroundedArg` (in
//! `eval_loop.rs`) prevents infinite loops when the bloom filter
//! produces false positives on data constructors.

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{MettaValueFactory, MettaValueInner, MettaValueTrait};

use super::super::{is_eager_special_form, is_grounded_op};

/// Check if a type value is an arrow type `(-> ...)`.
///
/// Arrow types indicate the symbol is a function, and its arguments
/// should be pre-evaluated (applicative evaluation).
pub fn is_arrow_type<V: MettaValueTrait>(typ: &V) -> bool {
    if let Some(items) = typ.as_sexpr() {
        if let Some(first) = items.first() {
            if let Some(arrow) = first.as_atom() {
                return arrow == "->";
            }
        }
    }
    false
}

/// Check if a type is a MeTTa meta-type (don't pre-evaluate args of these types).
///
/// Meta-types represent syntactic categories — arguments of these types are
/// passed unevaluated per MeTTa HE semantics.
///
/// Meta-types: `Atom`, `Expression`, `Symbol`, `Variable`, `Grounded`, `Pattern`,
/// and `%Undefined%` (unknown parameter type from inferred signatures — HE's
/// `interpret_function` only pre-evaluates args with a **concrete** declared
/// parameter type, falling through to unification otherwise).
pub fn is_meta_type<V: MettaValueTrait>(typ: &V) -> bool {
    if let Some(name) = typ.as_atom() {
        matches!(
            name,
            "Atom" | "Expression" | "Symbol" | "Variable" | "Grounded" | "Pattern" | "%Undefined%"
        )
    } else {
        false
    }
}

/// Extract argument types from an arrow type `(-> T1 T2 ... Tret)`.
///
/// Returns argument types (excluding return type) if this is an arrow type.
/// Returns `None` if the value is not an arrow type.
pub fn extract_arg_types<V: MettaValueTrait + Clone>(typ: &V) -> Option<Vec<V>> {
    if let Some(items) = typ.as_sexpr() {
        if items.len() >= 2 {
            if let Some(arrow) = items.first().and_then(|v| v.as_atom()) {
                if arrow == "->" {
                    // items[0] = "->", items[1..n-1] = arg types, items[n-1] = return type
                    if items.len() >= 3 {
                        return Some(items[1..items.len() - 1].to_vec());
                    }
                    // (-> RetType) with no args
                    return Some(vec![]);
                }
            }
        }
    }
    None
}

/// Extract the return type from an arrow type `(-> T1 T2 ... Tret)`.
///
/// Returns the last element (return type) if this is an arrow type.
/// Returns `None` if the value is not an arrow type.
pub fn extract_return_type<V: MettaValueTrait + Clone>(typ: &V) -> Option<V> {
    if let Some(items) = typ.as_sexpr() {
        if items.len() >= 2 {
            if let Some(arrow) = items.first().and_then(|v| v.as_atom()) {
                if arrow == "->" {
                    return items.last().cloned();
                }
            }
        }
    }
    None
}

/// Check if an operator has a `(-> ...)` type signature indicating it's a function.
///
/// MeTTa HE parity: only operators with arrow types trigger applicative evaluation.
/// If the operator has no type or a non-function type, returns false.
pub fn should_pre_eval_by_type<V, F>(op: &str, env: &GenericEnvironment<V, F>) -> bool
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    env.get_types_generic(op).iter().any(|t| is_arrow_type(t))
}

/// Generic version of find_grounded_arg_indices.
///
/// This function identifies S-expression arguments that need pre-evaluation
/// (applicative evaluation). It uses a three-tier strategy:
///
/// 1. **Grounded ops / eager special forms**: Always pre-evaluate (e.g., `+`, `-`)
/// 2. **Type-driven**: If the arg's head has a `(-> ...)` type signature, check
///    per-arg formal types — pre-evaluate value-typed args, skip meta-typed args
/// 3. **Bloom filter fallback**: For untyped heads, check if rules may exist.
///    Fixpoint detection in `CollectGroundedArg` prevents infinite loops.
///
/// ## Type-Driven Per-Arg Selection (MeTTa HE Parity)
///
/// When the arg's head operator has an arrow type, each formal argument type
/// is inspected. Meta-types (`Atom`, `Expression`, etc.) are passed unevaluated.
/// This matches MeTTa HE's `interpret_function` behavior.
pub fn find_grounded_arg_indices_generic<V, F>(
    items: &[V],
    env: &GenericEnvironment<V, F>,
) -> Vec<usize>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let mut indices = Vec::new();

    // Skip the first item (operator) - we only check arguments
    for (i, item) in items.iter().enumerate().skip(1) {
        if let Some(sub_items) = item.as_sexpr() {
            if let Some(first) = sub_items.first() {
                if let Some(op) = first.as_atom() {
                    // Tier 1: Grounded operation or eager special form
                    if is_grounded_op(op) || is_eager_special_form(op) {
                        indices.push(i);
                    }
                    // Tier 2: Type-driven — operator has (-> ...) type signature
                    else if should_pre_eval_by_type(op, env) {
                        indices.push(i);
                    }
                    // Tier 2.5: Phase 10 inferred type feedback (Phase 9.4).
                    // If the operator has inferred arrow types from Phase 10
                    // deep type inference, pre-evaluate it. Uses AtomicBloomFilter
                    // for O(1) rejection before DashMap lookup.
                    else if env.has_inferred_type(op)
                        && env
                            .get_inferred_fn_types(op)
                            .iter()
                            .any(|t| is_arrow_type(t))
                    {
                        indices.push(i);
                    }
                    // Tier 3: Bloom filter fallback for untyped operators with rules.
                    // False positives are handled by fixpoint detection in
                    // CollectGroundedArg (eval_loop.rs).
                    //
                    // Note: the dispatcher's Step-2 gate in `sexpr.rs` checks
                    // the PT-canonical `any_rule_wants_lazy_args` flag BEFORE
                    // this function is called for the parent — when a rule
                    // body's head is lazy (add-atom, quote, if, ...), pre-eval
                    // of THE PARENT'S ARGS is skipped. That's the correct
                    // surgical point. This Tier-3 fallback remains so non-lazy
                    // parents still see applicative argument evaluation.
                    else if env.may_have_rules_for(op, sub_items.len() - 1) {
                        indices.push(i);
                    }
                }
            }
        }
    }

    indices
}

/// Find grounded arg indices with per-arg type-driven selection.
///
/// When the parent operator has an arrow type `(-> T1 T2 ... Tret)`, this
/// function selects arguments based on their formal type:
/// - Meta-typed args (`Atom`, `Expression`, etc.) are skipped (passed unevaluated)
/// - Value-typed args with S-expr heads are marked for pre-evaluation
///
/// Returns `None` if the parent operator has no arrow type (caller should
/// fall back to `find_grounded_arg_indices_generic`).
///
/// If `precomputed_parent_types` is `Some`, uses it directly instead of
/// querying `env.get_types_generic(parent_op)` — avoids redundant RwLock
/// reads when the caller already fetched types for Phase 9.6 checks.
pub fn find_typed_arg_indices_generic<V, F>(
    items: &[V],
    env: &GenericEnvironment<V, F>,
    precomputed_parent_types: Option<&[V]>,
) -> Option<Vec<usize>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let parent_op = items.first().and_then(|v| v.as_atom())?;
    let parent_types = match precomputed_parent_types {
        Some(types) => types.to_vec(),
        None => env.get_types_generic(parent_op),
    };

    // Collect all arrow types for this operator
    let all_arg_types: Vec<Vec<V>> = parent_types
        .iter()
        .filter_map(|t| extract_arg_types(t))
        .collect();

    // No arrow types found → return None so caller falls back to bloom filter
    if all_arg_types.is_empty() {
        return None;
    }

    let mut indices = Vec::new();

    for (i, item) in items.iter().enumerate().skip(1) {
        let arg_idx = i - 1; // 0-based arg index

        // If formal type is a meta-type in ALL arrow types, skip (don't pre-evaluate).
        // Conservative: if ANY arrow type says value-typed at this position, pre-eval.
        let all_meta = all_arg_types
            .iter()
            .all(|arg_types| arg_idx < arg_types.len() && is_meta_type(&arg_types[arg_idx]));
        if all_meta {
            continue;
        }

        // Only mark S-expression arguments for pre-evaluation
        if item.as_sexpr().is_some() {
            indices.push(i);
        }
    }

    Some(indices)
}

// ============================================================================
// Phase 8.3: Value type dispatch — data constructor shortcut
// ============================================================================

/// Check if an operator has ONLY value types (no arrow types).
/// Returns `true` if the operator has type declarations and NONE are arrow types
/// (i.e., it's a known data constructor). Returns `false` if no type info
/// or if any type is an arrow type.
///
/// Used to skip rule matching entirely for data constructors (Step 2.5 in
/// sexpr.rs), sending them directly to the tuple path.
///
/// If `precomputed_types` is `Some`, uses it directly instead of querying
/// `env.get_types_generic(op)` — avoids redundant RwLock reads when the
/// caller already fetched types for Phase 9.6 checks.
pub fn is_declared_value_type<V, F>(
    op: &str,
    env: &GenericEnvironment<V, F>,
    precomputed_types: Option<&[V]>,
) -> bool
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let types = match precomputed_types {
        Some(t) => std::borrow::Cow::Borrowed(t),
        None => std::borrow::Cow::Owned(env.get_types_generic(op)),
    };
    !types.is_empty() && !types.iter().any(|t| is_arrow_type(t))
}

// ============================================================================
// Phase 8.8: Grounded argument type pre-validation
// ============================================================================

/// Validate ground-type arguments against a grounded op's arrow signature.
/// Returns `Some(error_value)` if a ground-type arg has incompatible type.
/// Returns `None` if all args are compatible or can't be validated yet
/// (S-expr args aren't validated since they need evaluation first).
///
/// This provides clear type error messages instead of `NoReduce` → reconstructed
/// unreduced expression trees when ground-typed args don't match the signature.
///
/// Phase 2 (2026-05-19): emits HE-canonical 3-tuple
/// `(Error <call-form> (BadArgType <1-indexed-pos> <expected-type> <actual-type>))`
/// in place of the prior `(Error TypeError "msg")` shape. Mirrors HE's
/// `BadArgType` (see `hyperon-experimental/lib/src/metta/mod.rs:26-28`) so
/// downstream `case ((Error _ (BadArgType _ _ _)) caught)` patterns unify.
///
/// `call_items` is the full call expression items including `op` at index 0
/// (so `factory.sexpr(call_items.to_vec())` rebuilds the original call form).
pub fn validate_grounded_arg_types<V, F>(
    op: &str,
    call_items: &[V],
    args: &[V],
    factory: &F,
) -> Option<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    use crate::backend::builtin_signatures::{get_signature, TypeExpr};

    let sig = get_signature(op)?;

    // Extract argument types from the arrow signature (-> T1 T2 ... Tret)
    let arg_types = match &sig.type_sig {
        TypeExpr::Arrow(args_te, _ret) => args_te,
        _ => return None, // Not an arrow type — can't validate
    };

    for (i, arg) in args.iter().enumerate() {
        if i >= arg_types.len() {
            break;
        }

        // Pattern match on (actual_inner, expected_type) pairs.
        // Only validate ground-type args in final form; skip S-exprs/atoms/variables.
        let mismatch = match (arg.inner_raw(), &arg_types[i]) {
            // Number args
            (MettaValueInner::Long(_) | MettaValueInner::Float(_), TypeExpr::Number) => false,
            (MettaValueInner::Long(_) | MettaValueInner::Float(_), TypeExpr::Bool) => true,
            (MettaValueInner::Long(_) | MettaValueInner::Float(_), TypeExpr::String) => true,
            // Bool args
            (MettaValueInner::Bool(_), TypeExpr::Bool) => false,
            (MettaValueInner::Bool(_), TypeExpr::Number) => true,
            (MettaValueInner::Bool(_), TypeExpr::String) => true,
            // String args
            (MettaValueInner::String(_), TypeExpr::String) => false,
            (MettaValueInner::String(_), TypeExpr::Number) => true,
            (MettaValueInner::String(_), TypeExpr::Bool) => true,
            // Polymorphic expected types — always compatible
            (
                _,
                TypeExpr::Var(_)
                | TypeExpr::Any
                | TypeExpr::Atom
                | TypeExpr::Undefined
                | TypeExpr::Expression,
            ) => false,
            // Non-ground args (S-expr, atom, variable) — can't validate yet
            _ => false,
        };

        if mismatch {
            let actual_type = match arg.inner_raw() {
                MettaValueInner::Long(_) | MettaValueInner::Float(_) => "Number",
                MettaValueInner::Bool(_) => "Bool",
                MettaValueInner::String(_) => "String",
                _ => unreachable!("only ground types reach mismatch=true"),
            };
            let expected_name = match &arg_types[i] {
                TypeExpr::Number => "Number",
                TypeExpr::Bool => "Bool",
                TypeExpr::String => "String",
                _ => unreachable!("only concrete types reach mismatch=true"),
            };
            // Phase 2 (2026-05-19): emit HE-canonical 3-tuple form
            // `(Error <call> (BadArgType <pos> <expected> <got>))`. Replaces
            // the prior `(Error TypeError "msg")` shape which never matched
            // HE's `case ((Error _ (BadArgType _ _ _)) caught)` patterns.
            let call_form = factory.sexpr(call_items.to_vec());
            let bad_arg_type = factory.sexpr(vec![
                factory.atom("BadArgType"),
                factory.long((i + 1) as i64),
                factory.atom(expected_name),
                factory.atom(actual_type),
            ]);
            return Some(factory.error( bad_arg_type,call_form));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{GcFactory, MettaValue, MettaValueFactory};

    fn factory() -> GcFactory {
        GcFactory::default()
    }

    fn env() -> MettaEnvironment {
        MettaEnvironment::new(factory())
    }

    #[test]
    fn test_is_arrow_type() {
        let f = factory();

        // (-> Number Number) is an arrow type
        let arrow = f.sexpr(vec![f.atom("->"), f.atom("Number"), f.atom("Number")]);
        assert!(is_arrow_type(&arrow));

        // "Number" is not an arrow type
        let not_arrow = f.atom("Number");
        assert!(!is_arrow_type(&not_arrow));

        // (List Number) is not an arrow type
        let not_arrow2 = f.sexpr(vec![f.atom("List"), f.atom("Number")]);
        assert!(!is_arrow_type(&not_arrow2));
    }

    #[test]
    fn test_is_meta_type() {
        let f = factory();

        assert!(is_meta_type(&f.atom("Atom")));
        assert!(is_meta_type(&f.atom("Expression")));
        assert!(is_meta_type(&f.atom("Symbol")));
        assert!(is_meta_type(&f.atom("Variable")));
        assert!(is_meta_type(&f.atom("Grounded")));
        assert!(is_meta_type(&f.atom("Pattern")));

        assert!(!is_meta_type(&f.atom("Number")));
        assert!(!is_meta_type(&f.atom("Bool")));
        assert!(!is_meta_type(&f.atom("String")));
        assert!(!is_meta_type(&f.atom("$t")));
    }

    #[test]
    fn test_extract_arg_types() {
        let f = factory();

        // (-> Number Bool String) → arg types = [Number, Bool], return = String
        let arrow = f.sexpr(vec![
            f.atom("->"),
            f.atom("Number"),
            f.atom("Bool"),
            f.atom("String"),
        ]);
        let args = extract_arg_types(&arrow).expect("should be arrow type");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].as_atom(), Some("Number"));
        assert_eq!(args[1].as_atom(), Some("Bool"));

        // (-> Number) → no args, return = Number
        let nullary = f.sexpr(vec![f.atom("->"), f.atom("Number")]);
        let args = extract_arg_types(&nullary).expect("should be arrow type");
        assert_eq!(args.len(), 0);

        // "Number" → not an arrow type
        let not_arrow = f.atom("Number");
        assert!(extract_arg_types(&not_arrow).is_none());
    }

    #[test]
    fn test_should_pre_eval_by_type() {
        let f = factory();
        let mut e = env();

        // No type defined → should not pre-eval
        assert!(!should_pre_eval_by_type("my-fn", &e));

        // Add arrow type → should pre-eval
        let arrow = f.sexpr(vec![f.atom("->"), f.atom("Number"), f.atom("Number")]);
        e.add_type_generic("my-fn", arrow);
        assert!(should_pre_eval_by_type("my-fn", &e));

        // Add non-arrow type → should not pre-eval
        e.add_type_generic("MyType", f.atom("Type"));
        assert!(!should_pre_eval_by_type("MyType", &e));
    }

    #[test]
    fn test_find_typed_arg_indices_meta_type_skipped() {
        let f = factory();
        let mut e = env();

        // (: lazy-fn (-> Expression Number Number))
        // First arg is Expression (meta-type) → skip
        // Second arg is Number (value-type) → pre-eval if S-expr
        let arrow = f.sexpr(vec![
            f.atom("->"),
            f.atom("Expression"),
            f.atom("Number"),
            f.atom("Number"),
        ]);
        e.add_type_generic("lazy-fn", arrow);

        // (lazy-fn (+ 1 2) (+ 3 4))
        let items: Vec<MettaValue> = vec![
            f.atom("lazy-fn"),
            f.sexpr(vec![f.atom("+"), f.long(1), f.long(2)]),
            f.sexpr(vec![f.atom("+"), f.long(3), f.long(4)]),
        ];

        let indices =
            find_typed_arg_indices_generic(&items, &e, None).expect("should find typed indices");
        // Only index 2 (second arg, Number type) should be selected
        assert_eq!(indices, vec![2]);
    }

    #[test]
    fn test_find_typed_arg_indices_no_type() {
        let f = factory();
        let e = env();

        // Untyped operator → returns None
        let items: Vec<MettaValue> = vec![
            f.atom("unknown-fn"),
            f.sexpr(vec![f.atom("+"), f.long(1), f.long(2)]),
        ];

        assert!(find_typed_arg_indices_generic(&items, &e, None).is_none());
    }

    #[test]
    fn test_extract_return_type() {
        let f = factory();

        // (-> Number Bool String) → return type = String
        let arrow = f.sexpr(vec![
            f.atom("->"),
            f.atom("Number"),
            f.atom("Bool"),
            f.atom("String"),
        ]);
        let ret = extract_return_type(&arrow).expect("should extract return type");
        assert_eq!(ret.as_atom(), Some("String"));

        // (-> Number) → return type = Number (nullary function)
        let nullary = f.sexpr(vec![f.atom("->"), f.atom("Number")]);
        let ret = extract_return_type(&nullary).expect("should extract return type");
        assert_eq!(ret.as_atom(), Some("Number"));

        // "Number" → not an arrow type
        let not_arrow = f.atom("Number");
        assert!(extract_return_type(&not_arrow).is_none());
    }
}
