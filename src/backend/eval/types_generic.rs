//! Generic Type Operations
//!
//! This module provides generic implementations of type operations that work
//! with any value type implementing `MettaValueTrait`. These are used by the
//! generic evaluation engine to avoid conversions between value types.
//!
//! ## Design
//!
//! Each operation:
//! - Takes generic `&[V]` items and a factory for constructing results
//! - Returns `Vec<V>` results
//! - Uses `MettaValueTrait` methods for type checking
//! - Uses `MettaValueFactory` for constructing new values

use std::collections::HashMap;

use crate::backend::builtin_signatures::{get_return_type, get_signature, TypeExpr};
use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{MettaValueFactory, MettaValueInner, MettaValueTrait};

/// Infer ALL possible types of an expression (nondeterministic, HE parity).
///
/// Returns a Vec of all possible types. For ground types (Bool, Number, etc.)
/// this always returns a single-element Vec. For atoms with multiple type
/// declarations, returns all declared types. For untyped atoms, returns `[%Undefined%]`.
///
/// For function applications `(f x)`, returns the return type from each
/// matching arrow type, plus tuple types from value types.
pub fn infer_types_generic<V, F>(expr: &V, factory: &F, env: &GenericEnvironment<V, F>) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    match expr.inner_raw() {
        MettaValueInner::Bool(_) => vec![factory.atom("Bool")],
        MettaValueInner::Long(_) | MettaValueInner::Float(_) => vec![factory.atom("Number")],
        MettaValueInner::String(_) => vec![factory.atom("String")],
        MettaValueInner::Unit => vec![factory.atom("Expression")],
        MettaValueInner::Type(_) => vec![factory.atom("Type")],
        MettaValueInner::Error(..) => vec![factory.atom("Error")],
        MettaValueInner::Space(_) => vec![factory.atom("Space")],
        MettaValueInner::State(_) => vec![factory.atom("State")],
        MettaValueInner::Memo(_) => vec![factory.atom("Memo")],
        MettaValueInner::Empty => vec![factory.atom("Empty")],
        MettaValueInner::Atom(name) => {
            // Check if it's a variable (starts with $, &, or ')
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                return vec![factory.type_value(factory.atom(name))];
            }

            // Look up ALL types in environment (nondeterministic)
            let types = env.get_types_generic(name);
            if types.is_empty() {
                vec![factory.atom("%Undefined%")]
            } else {
                types
            }
        }
        MettaValueInner::SExpr(_) => {
            let items = expr.as_sexpr().expect("matched SExpr");
            if items.is_empty() {
                return vec![factory.atom("Expression")];
            }

            // Get the operator/function
            if let Some(op) = items.first().and_then(|v| v.as_atom()) {
                // Check the built-in signature registry
                if let Some(sig) = get_signature(op) {
                    if let Some(ret_type) = get_return_type(&sig.type_sig) {
                        return vec![type_expr_to_generic(ret_type, factory)];
                    }
                }

                // Special case for arrow type constructor
                if op == "->" {
                    return vec![factory.atom("Type")];
                }

                // Look up function type in environment (user-defined types)
                // Collect return types from ALL arrow types (nondeterministic)
                let op_types = env.get_types_generic(op);
                let mut result_types = Vec::new();

                for generic_type in &op_types {
                    if let Some(type_items) = generic_type.as_sexpr() {
                        if let Some(arrow) = type_items.first().and_then(|v| v.as_atom()) {
                            if arrow == "->" && type_items.len() > 1 {
                                if let Some(last) = type_items.last() {
                                    if !result_types.contains(last) {
                                        result_types.push(last.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                // Also include non-arrow types as value types
                for generic_type in &op_types {
                    if generic_type.as_sexpr().map_or(true, |items| {
                        items.first().and_then(|v| v.as_atom()) != Some("->")
                    }) {
                        if !result_types.contains(generic_type) {
                            result_types.push(generic_type.clone());
                        }
                    }
                }

                if !result_types.is_empty() {
                    return result_types;
                }
            }

            vec![factory.atom("%Undefined%")]
        }
        MettaValueInner::Conjunction(_) => {
            let goals = expr.as_conjunction().expect("matched Conjunction");
            if goals.is_empty() {
                return vec![factory.atom("Expression")];
            }
            if let Some(last) = goals.last() {
                return infer_types_generic(last, factory, env);
            }
            vec![factory.atom("Expression")]
        }
        MettaValueInner::Quoted(_) => vec![factory.atom("Expression")],
        MettaValueInner::Spanned(..) => {
            let stripped = expr.strip_one_span();
            infer_types_generic(&stripped, factory, env)
        }
    }
}

/// Infer the type of an expression (deterministic convenience wrapper).
///
/// Returns the first inferred type. For callers that need a single deterministic
/// result (e.g., applicative eval tier 2 type checks).
pub fn infer_type_generic<V, F>(expr: &V, factory: &F, env: &GenericEnvironment<V, F>) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    infer_types_generic(expr, factory, env)
        .into_iter()
        .next()
        .unwrap_or_else(|| factory.atom("%Undefined%"))
}

/// Convert a TypeExpr from the signature registry to a generic value
fn type_expr_to_generic<V, F>(type_expr: &TypeExpr, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    match type_expr {
        TypeExpr::Number => factory.atom("Number"),
        TypeExpr::Bool => factory.atom("Bool"),
        TypeExpr::String => factory.atom("String"),
        TypeExpr::Atom => factory.atom("Atom"),
        TypeExpr::Expression => factory.atom("Expression"),
        TypeExpr::Variable => factory.atom("Variable"),
        TypeExpr::Space => factory.atom("Space"),
        TypeExpr::State => factory.atom("State"),
        TypeExpr::Unit => factory.atom("Unit"),
        TypeExpr::Error => factory.atom("Error"),
        TypeExpr::Type => factory.atom("Type"),
        TypeExpr::Undefined => factory.atom("%Undefined%"),
        TypeExpr::Grounded => factory.atom("Grounded"),
        TypeExpr::Any => factory.atom("Any"),
        TypeExpr::Pattern => factory.atom("Pattern"),
        TypeExpr::Bindings => factory.atom("Bindings"),
        TypeExpr::Expr => factory.atom("Expr"),
        TypeExpr::Var(name) => factory.atom(&format!("${}", name)),
        TypeExpr::List(elem) => {
            let elem_type = type_expr_to_generic(elem, factory);
            factory.sexpr(vec![factory.atom("List"), elem_type])
        }
        TypeExpr::StateMonad(elem) => {
            let elem_type = type_expr_to_generic(elem, factory);
            factory.sexpr(vec![factory.atom("StateMonad"), elem_type])
        }
        TypeExpr::Arrow(args, ret) => {
            let mut items = vec![factory.atom("->")];
            for arg in args {
                items.push(type_expr_to_generic(arg, factory));
            }
            items.push(type_expr_to_generic(ret, factory));
            factory.sexpr(items)
        }
    }
}

/// Check if two types match (generic version)
///
/// Handles type variables, `%Undefined%` universal match, and structural equality.
/// HE parity: `%Undefined%` matches any type on either side.
fn types_match_generic<V: MettaValueTrait>(actual: &V, expected: &V) -> bool {
    // %Undefined% matches anything (HE parity)
    if let Some(name) = expected.as_atom() {
        if name == "%Undefined%" {
            return true;
        }
    }
    if let Some(name) = actual.as_atom() {
        if name == "%Undefined%" {
            return true;
        }
    }

    // Type variables match anything
    if let Some(name) = expected.as_atom() {
        if name.starts_with('$') {
            return true;
        }
    }
    if let Some(name) = actual.as_atom() {
        if name.starts_with('$') {
            return true;
        }
    }

    // Type variables in Type wrapper
    if expected.is_type() {
        if let Some(inner) = expected.as_type() {
            if let Some(name) = inner.as_atom() {
                if name.starts_with('$') {
                    return true;
                }
            }
            // Otherwise, unwrap and compare
            if actual.is_type() {
                if let Some(actual_inner) = actual.as_type() {
                    return types_match_generic(actual_inner, inner);
                }
            }
        }
        return false;
    }

    // Exact atom matches
    if let (Some(a), Some(e)) = (actual.as_atom(), expected.as_atom()) {
        return a == e;
    }

    // Bool matches
    if let (Some(a), Some(e)) = (actual.as_bool(), expected.as_bool()) {
        return a == e;
    }

    // Long matches
    if let (Some(a), Some(e)) = (actual.as_long(), expected.as_long()) {
        return a == e;
    }

    // String matches
    if let (Some(a), Some(e)) = (actual.as_string(), expected.as_string()) {
        return a == e;
    }

    // S-expression matches (structural equality)
    if let (Some(a_items), Some(e_items)) = (actual.as_sexpr(), expected.as_sexpr()) {
        if a_items.len() != e_items.len() {
            return false;
        }
        return a_items
            .iter()
            .zip(e_items.iter())
            .all(|(a, e)| types_match_generic(a, e));
    }

    // Unit matches Unit
    if actual.is_unit() && expected.is_unit() {
        return true;
    }

    // Default: no match
    false
}

/// Check if two types match with subtype awareness (generic version).
///
/// First checks direct type match via `types_match_generic`. If that fails,
/// checks if `actual` is a subtype of `expected` using the environment's
/// subtype relations (transitive closure via BFS).
///
/// HE parity: `(:< Sub Super)` declarations make `Sub` match where `Super` is expected.
pub fn types_match_with_subtypes<V, F>(
    actual: &V,
    expected: &V,
    env: &GenericEnvironment<V, F>,
) -> bool
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // Direct match (includes %Undefined% universal match)
    if types_match_generic(actual, expected) {
        return true;
    }

    // Subtype check: if both are atoms, check if actual <: expected
    if let (Some(actual_name), Some(expected_name)) = (actual.as_atom(), expected.as_atom()) {
        return env.is_subtype_of(actual_name, expected_name);
    }

    // For S-expression types (e.g., arrow types), check structural subtyping
    // component-wise. This handles cases like (-> Dog Bool) matching where
    // (-> Animal Bool) is expected (contravariant args, covariant return).
    // For now, we only do simple subtype check on atoms.
    false
}

/// Bidirectional type matching with variable binding.
///
/// Returns `true` if the pattern type unifies with the actual type,
/// recording variable bindings in the `bindings` map.
///
/// This supports polymorphic functions like `(: map (-> (-> $t $u) (List $t) (List $u)))`:
/// - Concrete type equality: `Number == Number`
/// - Type variables: `$t` matches anything, records binding
/// - Structural matching: `(List $t)` matches `(List Number)` with `{$t: Number}`
/// - Arrow types: `(-> $t $u)` matches `(-> Number Bool)` with `{$t: Number, $u: Bool}`
/// - Consistency: if `$t` is already bound to `Number`, it only matches `Number`
pub fn match_types_with_bindings<V: MettaValueTrait + Clone>(
    pattern: &V,
    actual: &V,
    bindings: &mut HashMap<String, V>,
) -> bool {
    // Type variable in pattern — bind or check consistency
    if let Some(name) = pattern.as_atom() {
        if name.starts_with('$') {
            if let Some(existing) = bindings.get(name) {
                return types_match_generic(actual, existing);
            } else {
                bindings.insert(name.to_string(), actual.clone());
                return true;
            }
        }
    }

    // Type variable in actual — symmetric (free variable matches anything)
    if let Some(name) = actual.as_atom() {
        if name.starts_with('$') {
            return true;
        }
    }

    // Atom equality
    if let (Some(p), Some(a)) = (pattern.as_atom(), actual.as_atom()) {
        return p == a;
    }

    // Structural S-expr matching (covers (-> ...), (List ...), etc.)
    if let (Some(p_items), Some(a_items)) = (pattern.as_sexpr(), actual.as_sexpr()) {
        if p_items.len() != a_items.len() {
            return false;
        }
        for (p, a) in p_items.iter().zip(a_items.iter()) {
            if !match_types_with_bindings(p, a, bindings) {
                return false;
            }
        }
        return true;
    }

    // Ground type equality
    pattern == actual
}

/// get-type: Return ALL types of an expression nondeterministically (HE parity).
/// (get-type expr) -> Type (nondeterministic: may return multiple results)
///
/// For atoms with multiple type declarations, returns each type as a separate
/// result. For untyped atoms, returns `%Undefined%`.
pub fn eval_get_type_generic<V, F>(items: &[V], factory: &F, env: &GenericEnvironment<V, F>) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() != 2 {
        return vec![factory.error(
            &format!(
                "get-type requires exactly 1 argument, got {}. Usage: (get-type expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];
    infer_types_generic(expr, factory, env)
}

/// check-type: Check if expression has expected type (generic version, HE parity).
/// (check-type expr expected-type) -> Bool
///
/// Returns True if ANY inferred type matches the expected type.
/// %Undefined% matches any type (universal match per HE semantics).
pub fn eval_check_type_generic<V, F>(items: &[V], factory: &F, env: &GenericEnvironment<V, F>) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() != 3 {
        return vec![factory.error(
            &format!(
                "check-type requires exactly 2 arguments, got {}. Usage: (check-type expr type)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];
    let expected = &items[2];

    // Infer ALL types, check if ANY matches expected (with subtype awareness)
    let actual_types = infer_types_generic(expr, factory, env);
    let matches = actual_types.iter().any(|actual| types_match_with_subtypes(actual, expected, env));

    vec![factory.bool(matches)]
}

/// Check if a value is a type error atom: `(Error expr (BadArgType ...))`
fn is_type_error<V: MettaValueTrait>(v: &V) -> bool {
    if let Some(items) = v.as_sexpr() {
        if items.len() >= 2 {
            if let Some("Error") = items[0].as_atom() {
                // Check for BadArgType in the error detail
                if items.len() >= 3 {
                    if let Some(detail_items) = items[2].as_sexpr() {
                        if !detail_items.is_empty() {
                            if let Some("BadArgType") = detail_items[0].as_atom() {
                                return true;
                            }
                        }
                    }
                }
                return true; // Any Error atom is a type error for validate-atom
            }
        }
    }
    false
}

/// Construct a type error atom: `(Error expr (BadArgType idx expected actual))`
#[allow(dead_code)]
pub fn make_type_error<V, F>(
    factory: &F,
    expr: &V,
    arg_idx: usize,
    expected: &V,
    actual: &V,
) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let detail = factory.sexpr(vec![
        factory.atom("BadArgType"),
        factory.long(arg_idx as i64),
        expected.clone(),
        actual.clone(),
    ]);
    factory.sexpr(vec![
        factory.atom("Error"),
        expr.clone(),
        detail,
    ])
}

/// Evaluate `(validate-atom expr)` — recursive well-typedness checking.
///
/// Returns `True` if the expression is well-typed (all inferred types are
/// non-error), `False` if any inferred type is a type error.
/// Untyped atoms are considered valid (per HE semantics).
///
/// Mirrors HE's `validate_atom` (types.rs:637-639).
pub fn eval_validate_atom_generic<V, F>(
    items: &[V],
    factory: &F,
    env: &GenericEnvironment<V, F>,
) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() != 2 {
        return vec![factory.error(
            &format!(
                "validate-atom requires exactly 1 argument, got {}. Usage: (validate-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];

    // Infer all types for the expression
    let types = infer_types_generic(expr, factory, env);

    // Check if ALL inferred types are non-error
    let valid = types.iter().all(|t| !is_type_error(t));

    vec![factory.bool(valid)]
}

/// Evaluate `(get-type-space space atom)` — query types in a specific space.
///
/// If `space` is `&self`, uses the current environment's type system.
/// Otherwise, queries the named space's atoms for `(: atom $T)` patterns.
///
/// Returns all matching types nondeterministically, or `%Undefined%` if none.
pub fn eval_get_type_space_generic<V, F>(
    items: &[V],
    factory: &F,
    env: &GenericEnvironment<V, F>,
) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() != 3 {
        return vec![factory.error(
            &format!(
                "get-type-space requires exactly 2 arguments, got {}. Usage: (get-type-space space atom)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let space = &items[1];
    let atom = &items[2];

    // If space is &self, use the normal type inference
    if let Some(space_name) = space.as_atom() {
        if space_name == "&self" || space_name == "self" {
            return infer_types_generic(atom, factory, env);
        }
    }

    // Get atom name for type lookup
    let atom_name = match atom.as_atom() {
        Some(name) => name.to_string(),
        None => {
            // Non-atom expressions: infer type structurally via env
            return infer_types_generic(atom, factory, env);
        }
    };

    // Fast path: if space is a resolved SpaceHandle, query its PathMap directly
    if let Some(space_handle) = space.as_space() {
        let types = space_handle.query_types_generic(&atom_name, factory);
        if !types.is_empty() {
            return types;
        }
        return vec![factory.atom("%Undefined%")];
    }

    // Fallback for named spaces by atom name (e.g., `(get-type-space my-space x)`)
    if let Some(space_name) = space.as_atom() {
        let named_spaces = env.shared.named_spaces.read();
        for (_id, (name, atoms)) in named_spaces.iter() {
            if name == space_name {
                let mut types = Vec::new();
                for stored_atom in atoms {
                    if let Some(stored_items) = stored_atom.as_sexpr() {
                        if stored_items.len() == 3 {
                            if let Some(":") = stored_items[0].as_atom() {
                                if let Some(stored_name) = stored_items[1].as_atom() {
                                    if stored_name == atom_name {
                                        let typ = stored_items[2].clone();
                                        if !types.contains(&typ) {
                                            types.push(typ);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if !types.is_empty() {
                    return types;
                }
            }
        }
    }

    // No types found in the specified space
    vec![factory.atom("%Undefined%")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_infer_type_generic_ground_types() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // Bool
        let value = MettaValue::Bool(true);
        let typ = infer_type_generic(&value, &factory, &env);
        assert_eq!(typ.as_atom(), Some("Bool"));

        // Long
        let value = MettaValue::Long(42);
        let typ = infer_type_generic(&value, &factory, &env);
        assert_eq!(typ.as_atom(), Some("Number"));

        // String
        let value = MettaValue::String("hello".to_string());
        let typ = infer_type_generic(&value, &factory, &env);
        assert_eq!(typ.as_atom(), Some("String"));
    }

    #[test]
    fn test_infer_types_generic_ground_types() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        let types = infer_types_generic(&MettaValue::Bool(true), &factory, &env);
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].as_atom(), Some("Bool"));

        let types = infer_types_generic(&MettaValue::Long(42), &factory, &env);
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].as_atom(), Some("Number"));
    }

    #[test]
    fn test_get_type_generic() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        let items = vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Long(42),
        ];
        let result = eval_get_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_atom(), Some("Number"));
    }

    #[test]
    fn test_get_type_nondeterministic() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // Declare multiple types for the same atom
        env.add_type_generic("a", factory.atom("A"));
        env.add_type_generic("a", factory.atom("B"));

        let items = vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Atom("a".to_string()),
        ];
        let result = eval_get_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 2, "get-type should return all types");
        assert!(result.iter().any(|t| t.as_atom() == Some("A")));
        assert!(result.iter().any(|t| t.as_atom() == Some("B")));
    }

    #[test]
    fn test_get_type_undefined_for_untyped() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        let items = vec![
            MettaValue::Atom("get-type".to_string()),
            MettaValue::Atom("untyped-atom".to_string()),
        ];
        let result = eval_get_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_atom(), Some("%Undefined%"));
    }

    #[test]
    fn test_get_type_arrow_return() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: f (-> A B))
        let arrow = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("A"),
            factory.atom("B"),
        ]);
        env.add_type_generic("f", arrow);

        // !(get-type (f x)) → B (return type of arrow)
        let expr = factory.sexpr(vec![
            factory.atom("f"),
            factory.atom("x"),
        ]);
        let types = infer_types_generic(&expr, &factory, &env);
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].as_atom(), Some("B"));
    }

    #[test]
    fn test_get_type_multi_arrow() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: f (-> A B)) and (: f (-> C D))
        let arrow1 = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("A"),
            factory.atom("B"),
        ]);
        let arrow2 = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("C"),
            factory.atom("D"),
        ]);
        env.add_type_generic("f", arrow1);
        env.add_type_generic("f", arrow2);

        // !(get-type (f x)) → both B and D
        let expr = factory.sexpr(vec![
            factory.atom("f"),
            factory.atom("x"),
        ]);
        let types = infer_types_generic(&expr, &factory, &env);
        assert_eq!(types.len(), 2);
        assert!(types.iter().any(|t| t.as_atom() == Some("B")));
        assert!(types.iter().any(|t| t.as_atom() == Some("D")));
    }

    #[test]
    fn test_check_type_generic() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // (check-type 42 Number) -> true
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("Number".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));

        // (check-type 42 String) -> false
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("String".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(false));
    }

    #[test]
    fn test_check_type_with_type_variable() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // (check-type 42 $t) -> true (type variable matches anything)
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$t".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_undefined_universal_match() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // (check-type untyped-atom SomeType) → True (%Undefined% matches anything)
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("untyped-atom".to_string()),
            MettaValue::Atom("SomeType".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true), "%Undefined% should match any type");
    }

    #[test]
    fn test_check_type_multi_type_any_matches() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: a A) + (: a B)
        env.add_type_generic("a", factory.atom("A"));
        env.add_type_generic("a", factory.atom("B"));

        // (check-type a A) → True (A is one of the types)
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("A".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result[0].as_bool(), Some(true));

        // (check-type a B) → True (B is also one of the types)
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("B".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result[0].as_bool(), Some(true));

        // (check-type a C) → False (C is not a declared type)
        let items = vec![
            MettaValue::Atom("check-type".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("C".to_string()),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result[0].as_bool(), Some(false));
    }

    #[test]
    fn test_match_types_with_bindings_concrete() {
        let factory = GcFactory::default();
        let mut bindings = HashMap::new();

        let number = factory.atom("Number");
        assert!(match_types_with_bindings(&number, &number, &mut bindings));
        assert!(bindings.is_empty());

        let string = factory.atom("String");
        assert!(!match_types_with_bindings(&number, &string, &mut bindings));
    }

    #[test]
    fn test_match_types_with_bindings_variable() {
        let factory = GcFactory::default();
        let mut bindings = HashMap::new();

        let pattern = factory.atom("$t");
        let actual = factory.atom("Number");

        assert!(match_types_with_bindings(&pattern, &actual, &mut bindings));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings["$t"].as_atom(), Some("Number"));
    }

    #[test]
    fn test_match_types_with_bindings_consistent() {
        let factory = GcFactory::default();
        let mut bindings = HashMap::new();

        // First binding: $t = Number
        let pattern = factory.atom("$t");
        let number = factory.atom("Number");
        assert!(match_types_with_bindings(&pattern, &number, &mut bindings));

        // Same variable must match same type
        assert!(match_types_with_bindings(&pattern, &number, &mut bindings));

        // Different type fails consistency
        let string = factory.atom("String");
        assert!(!match_types_with_bindings(&pattern, &string, &mut bindings));
    }

    #[test]
    fn test_match_types_with_bindings_structural() {
        let factory = GcFactory::default();
        let mut bindings = HashMap::new();

        // (List $t) vs (List Number) → {$t: Number}
        let pattern = factory.sexpr(vec![factory.atom("List"), factory.atom("$t")]);
        let actual = factory.sexpr(vec![factory.atom("List"), factory.atom("Number")]);

        assert!(match_types_with_bindings(&pattern, &actual, &mut bindings));
        assert_eq!(bindings["$t"].as_atom(), Some("Number"));
    }

    #[test]
    fn test_match_types_with_bindings_arrow() {
        let factory = GcFactory::default();
        let mut bindings = HashMap::new();

        // (-> $t $u) vs (-> Number Bool) → {$t: Number, $u: Bool}
        let pattern = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("$t"),
            factory.atom("$u"),
        ]);
        let actual = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Number"),
            factory.atom("Bool"),
        ]);

        assert!(match_types_with_bindings(&pattern, &actual, &mut bindings));
        assert_eq!(bindings["$t"].as_atom(), Some("Number"));
        assert_eq!(bindings["$u"].as_atom(), Some("Bool"));
    }

    #[test]
    fn test_match_types_with_bindings_length_mismatch() {
        let factory = GcFactory::default();
        let mut bindings = HashMap::new();

        // (-> $t) vs (-> Number Bool) → fail (different lengths)
        let pattern = factory.sexpr(vec![factory.atom("->"), factory.atom("$t")]);
        let actual = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("Number"),
            factory.atom("Bool"),
        ]);

        assert!(!match_types_with_bindings(&pattern, &actual, &mut bindings));
    }

    // ====================================================================
    // Phase 3: Subtype-aware type checking
    // ====================================================================

    #[test]
    fn test_check_type_with_subtypes() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: fido Dog), (:< Dog Animal)
        env.add_type_generic("fido", factory.atom("Dog"));
        env.add_subtype_generic("Dog", "Animal");

        // (check-type fido Animal) → True (via subtype)
        let items = vec![
            factory.atom("check-type"),
            factory.atom("fido"),
            factory.atom("Animal"),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_check_type_subtype_transitive() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: fido Dog), (:< Dog Animal), (:< Animal LivingThing)
        env.add_type_generic("fido", factory.atom("Dog"));
        env.add_subtype_generic("Dog", "Animal");
        env.add_subtype_generic("Animal", "LivingThing");

        // (check-type fido LivingThing) → True (via transitive subtype)
        let items = vec![
            factory.atom("check-type"),
            factory.atom("fido"),
            factory.atom("LivingThing"),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_check_type_not_supertype() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: fido Dog), (:< Dog Animal)
        env.add_type_generic("fido", factory.atom("Dog"));
        env.add_subtype_generic("Dog", "Animal");

        // (check-type fido Cat) → False (Dog is not a subtype of Cat)
        let items = vec![
            factory.atom("check-type"),
            factory.atom("fido"),
            factory.atom("Cat"),
        ];
        let result = eval_check_type_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(false));
    }

    #[test]
    fn test_types_match_with_subtypes_direct() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        env.add_subtype_generic("Dog", "Animal");

        let dog = factory.atom("Dog");
        let animal = factory.atom("Animal");

        assert!(types_match_with_subtypes(&dog, &animal, &env));
        assert!(!types_match_with_subtypes(&animal, &dog, &env)); // not symmetric
    }

    // ====================================================================
    // Phase 4: validate-atom + Type Error Atoms
    // ====================================================================

    #[test]
    fn test_validate_correct() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // (: f (-> A B)), (: a A)
        let arrow = factory.sexpr(vec![
            factory.atom("->"),
            factory.atom("A"),
            factory.atom("B"),
        ]);
        env.add_type_generic("f", arrow);
        env.add_type_generic("a", factory.atom("A"));

        // (validate-atom (f a)) → True
        let expr = factory.sexpr(vec![factory.atom("f"), factory.atom("a")]);
        let items = vec![factory.atom("validate-atom"), expr];
        let result = eval_validate_atom_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_validate_untyped() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // (validate-atom (foo bar)) → True (untyped = always valid, per HE)
        let expr = factory.sexpr(vec![factory.atom("foo"), factory.atom("bar")]);
        let items = vec![factory.atom("validate-atom"), expr];
        let result = eval_validate_atom_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_validate_atom_simple() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // (validate-atom x) → True (untyped atom)
        let items = vec![factory.atom("validate-atom"), factory.atom("x")];
        let result = eval_validate_atom_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_is_type_error() {
        let factory = GcFactory::default();

        // (Error expr (BadArgType 0 A B))
        let error = factory.sexpr(vec![
            factory.atom("Error"),
            factory.atom("expr"),
            factory.sexpr(vec![
                factory.atom("BadArgType"),
                factory.long(0),
                factory.atom("A"),
                factory.atom("B"),
            ]),
        ]);
        assert!(is_type_error(&error));

        // Non-error
        let non_error = factory.atom("A");
        assert!(!is_type_error(&non_error));
    }

    #[test]
    fn test_make_type_error_structure() {
        let factory = GcFactory::default();

        let expr = factory.sexpr(vec![factory.atom("f"), factory.atom("b")]);
        let error = make_type_error(
            &factory,
            &expr,
            0,
            &factory.atom("A"),
            &factory.atom("B"),
        );

        // Verify structure: (Error (f b) (BadArgType 0 A B))
        let items = error.as_sexpr().expect("should be S-expr");
        assert_eq!(items[0].as_atom(), Some("Error"));
        let detail = items[2].as_sexpr().expect("detail should be S-expr");
        assert_eq!(detail[0].as_atom(), Some("BadArgType"));
        assert_eq!(detail[1].as_long(), Some(0));
        assert_eq!(detail[2].as_atom(), Some("A"));
        assert_eq!(detail[3].as_atom(), Some("B"));
    }

    // ====================================================================
    // Phase 5: get-type-space
    // ====================================================================

    #[test]
    fn test_get_type_space_self() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        env.add_type_generic("x", factory.atom("Number"));

        // (get-type-space &self x) → Number
        let items = vec![
            factory.atom("get-type-space"),
            factory.atom("&self"),
            factory.atom("x"),
        ];
        let result = eval_get_type_space_generic(&items, &factory, &env);
        assert!(!result.is_empty());
        assert!(result.iter().any(|t| t.as_atom() == Some("Number")));
    }

    #[test]
    fn test_get_type_space_self_multi() {
        let factory = GcFactory::default();
        let mut env = MettaEnvironment::new(GcFactory::default());

        // Multiple types
        env.add_type_generic("a", factory.atom("A"));
        env.add_type_generic("a", factory.atom("B"));

        let items = vec![
            factory.atom("get-type-space"),
            factory.atom("&self"),
            factory.atom("a"),
        ];
        let result = eval_get_type_space_generic(&items, &factory, &env);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|t| t.as_atom() == Some("A")));
        assert!(result.iter().any(|t| t.as_atom() == Some("B")));
    }

    #[test]
    fn test_get_type_space_unknown_space() {
        let factory = GcFactory::default();
        let env = MettaEnvironment::new(GcFactory::default());

        // (get-type-space unknown-space x) → %Undefined% (no types in that space)
        let items = vec![
            factory.atom("get-type-space"),
            factory.atom("unknown-space"),
            factory.atom("x"),
        ];
        let result = eval_get_type_space_generic(&items, &factory, &env);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_atom(), Some("%Undefined%"));
    }
}
