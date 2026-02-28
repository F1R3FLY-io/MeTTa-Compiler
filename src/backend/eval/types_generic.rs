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
    // Track the source code path for tracing (only allocated when eval-trace is enabled)
    #[cfg(feature = "eval-trace")]
    let mut _trace_source: &str = "";

    let result = match expr.inner_raw() {
        MettaValueInner::Bool(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-bool"; }
            vec![factory.atom("Bool")]
        }
        MettaValueInner::Long(_) | MettaValueInner::Float(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-number"; }
            vec![factory.atom("Number")]
        }
        MettaValueInner::String(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-string"; }
            vec![factory.atom("String")]
        }
        MettaValueInner::Unit => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-unit"; }
            vec![factory.atom("Expression")]
        }
        MettaValueInner::Type(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-type"; }
            vec![factory.atom("Type")]
        }
        MettaValueInner::Error(..) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-error"; }
            vec![factory.atom("Error")]
        }
        MettaValueInner::Space(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-space"; }
            vec![factory.atom("Space")]
        }
        MettaValueInner::State(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-state"; }
            vec![factory.atom("State")]
        }
        MettaValueInner::Memo(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-memo"; }
            vec![factory.atom("Memo")]
        }
        MettaValueInner::Empty => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "literal-empty"; }
            vec![factory.atom("Empty")]
        }
        MettaValueInner::Atom(name) => {
            // Check if it's a variable (starts with $, &, or ')
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                #[cfg(feature = "eval-trace")]
                { _trace_source = "variable"; }
                vec![factory.type_value(factory.atom(name))]
            } else {
                // Look up ALL types in environment (nondeterministic)
                let types = env.get_types_generic(name);
                if types.is_empty() {
                    #[cfg(feature = "eval-trace")]
                    { _trace_source = "fallback-undefined"; }
                    vec![factory.atom("%Undefined%")]
                } else {
                    #[cfg(feature = "eval-trace")]
                    { _trace_source = "env-atom-types"; }
                    types
                }
            }
        }
        MettaValueInner::SExpr(_) => {
            let items = expr.as_sexpr().expect("matched SExpr");
            if items.is_empty() {
                #[cfg(feature = "eval-trace")]
                { _trace_source = "sexpr-empty"; }
                vec![factory.atom("Expression")]
            } else if let Some(op) = items.first().and_then(|v| v.as_atom()) {
                // Check the built-in signature registry
                if let Some(sig) = get_signature(op) {
                    if let Some(ret_type) = get_return_type(&sig.type_sig) {
                        #[cfg(feature = "eval-trace")]
                        { _trace_source = "builtin-signature"; }
                        vec![type_expr_to_generic(ret_type, factory)]
                    } else {
                        // Builtin signature exists but no return type — fall through
                        let (types, source) = infer_types_sexpr_body(op, items, factory, env);
                        #[cfg(feature = "eval-trace")]
                        { _trace_source = source; }
                        let _ = source;
                        types
                    }
                } else if op == "->" {
                    // Special case for arrow type constructor
                    #[cfg(feature = "eval-trace")]
                    { _trace_source = "arrow-constructor"; }
                    vec![factory.atom("Type")]
                } else {
                    let (types, source) = infer_types_sexpr_body(op, items, factory, env);
                    #[cfg(feature = "eval-trace")]
                    { _trace_source = source; }
                    let _ = source;
                    types
                }
            } else {
                // Operator is not an atom
                #[cfg(feature = "eval-trace")]
                { _trace_source = "fallback-undefined"; }
                vec![factory.atom("%Undefined%")]
            }
        }
        MettaValueInner::Conjunction(_) => {
            let goals = expr.as_conjunction().expect("matched Conjunction");
            #[cfg(feature = "eval-trace")]
            { _trace_source = "conjunction"; }
            if goals.is_empty() {
                vec![factory.atom("Expression")]
            } else if let Some(last) = goals.last() {
                infer_types_generic(last, factory, env)
            } else {
                vec![factory.atom("Expression")]
            }
        }
        MettaValueInner::Quoted(_) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "quoted"; }
            vec![factory.atom("Expression")]
        }
        MettaValueInner::Spanned(..) => {
            #[cfg(feature = "eval-trace")]
            { _trace_source = "spanned-strip"; }
            let stripped = expr.strip_one_span();
            infer_types_generic(&stripped, factory, env)
        }
    };

    // Emit TypeInference trace event
    #[cfg(feature = "eval-trace")]
    {
        crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
            let trace_expr = crate::backend::trace::trace_value_generic(expr);
            let trace_types: Vec<trace_format::TraceValue> = result
                .iter()
                .map(crate::backend::trace::trace_value_generic)
                .collect();
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                0,
                trace_expr.clone(),
                vec![],
                None,
                trace_format::TraceEventKind::TypeInference {
                    expression: trace_expr,
                    inferred_types: trace_types,
                    source: _trace_source.to_string(),
                },
            );
        });
    }

    result
}

/// Helper for S-expression type inference body (env-declared and Phase 10 paths).
///
/// Returns `(inferred_types, trace_source)` where `trace_source` identifies which
/// code path produced the result (for eval-trace instrumentation).
fn infer_types_sexpr_body<V, F>(
    op: &str,
    items: &[V],
    factory: &F,
    env: &GenericEnvironment<V, F>,
) -> (Vec<V>, &'static str)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // Look up function type in environment (user-defined types)
    // Collect return types from ALL arrow types (nondeterministic)
    let op_types = env.get_types_generic(op);
    let mut result_types = Vec::new();

    let actual_args = &items[1..]; // Skip operator

    for generic_type in &op_types {
        if let Some(type_items) = generic_type.as_sexpr() {
            if let Some(arrow) = type_items.first().and_then(|v| v.as_atom()) {
                if arrow == "->" && type_items.len() > 1 {
                    let param_types = &type_items[1..type_items.len() - 1];
                    let return_type = &type_items[type_items.len() - 1];

                    // Phase 10.2: Match actual arg types against declared param
                    // types, collecting type variable bindings for substitution.
                    let mut bindings = HashMap::new();
                    let mut all_match = true;

                    for (i, param_type) in param_types.iter().enumerate() {
                        if i < actual_args.len() {
                            let arg_type = infer_type_generic(&actual_args[i], factory, env);
                            // Skip matching if arg type is %Undefined% (can't constrain)
                            if arg_type.as_atom() != Some("%Undefined%") {
                                if !match_types_with_bindings(param_type, &arg_type, &mut bindings) {
                                    all_match = false;
                                    break;
                                }
                            }
                        }
                    }

                    let resolved = if all_match && !bindings.is_empty() {
                        // Substitute bindings into return type
                        apply_type_bindings(return_type, &bindings, factory)
                    } else {
                        return_type.clone()
                    };

                    if !result_types.contains(&resolved) {
                        result_types.push(resolved);
                    }
                }
            }
        }
    }

    // Also include non-arrow types as value types
    for generic_type in &op_types {
        if generic_type.as_sexpr().map_or(true, |sexpr_items| {
            sexpr_items.first().and_then(|v| v.as_atom()) != Some("->")
        }) {
            if !result_types.contains(generic_type) {
                result_types.push(generic_type.clone());
            }
        }
    }

    if !result_types.is_empty() {
        // Determine trace source: arrow types vs value types
        let has_arrow = op_types.iter().any(|t| {
            t.as_sexpr()
                .and_then(|items| items.first().and_then(|v| v.as_atom()))
                == Some("->")
        });
        let source = if has_arrow {
            "env-declared-arrow"
        } else {
            "env-declared-value"
        };
        return (result_types, source);
    }

    // Phase 10.1: Check inferred function return type index.
    // This catches user-defined functions whose RHS type was inferred
    // at add_rule() time but which lack explicit (: f (-> ...)) declarations.
    if env.has_inferred_type(op) {
        let inferred = env.get_inferred_fn_types(op);
        let mut inferred_results = Vec::new();
        let mut has_inferred_arrow = false;

        for inferred_type in &inferred {
            // Check if this is an arrow type — process it like declared types
            if let Some(type_items) = inferred_type.as_sexpr() {
                if type_items.first().and_then(|v| v.as_atom()) == Some("->")
                    && type_items.len() > 1
                {
                    has_inferred_arrow = true;
                    // Phase 10.2: Type variable substitution on inferred arrow
                    let param_types = &type_items[1..type_items.len() - 1];
                    let return_type = &type_items[type_items.len() - 1];

                    let mut bindings = HashMap::new();
                    let mut all_match = true;
                    for (i, param_type) in param_types.iter().enumerate() {
                        if i < actual_args.len() {
                            let arg_type = infer_type_generic(
                                &actual_args[i], factory, env,
                            );
                            if arg_type.as_atom() != Some("%Undefined%") {
                                if !match_types_with_bindings(
                                    param_type, &arg_type, &mut bindings,
                                ) {
                                    all_match = false;
                                    break;
                                }
                            }
                        }
                    }

                    let resolved = if all_match && !bindings.is_empty() {
                        apply_type_bindings(return_type, &bindings, factory)
                    } else {
                        return_type.clone()
                    };

                    if !inferred_results.contains(&resolved) {
                        inferred_results.push(resolved);
                    }
                    continue;
                }
            }

            // Non-arrow type: direct return type from rhs_type
            if !inferred_results.contains(inferred_type) {
                inferred_results.push(inferred_type.clone());
            }
        }

        if !inferred_results.is_empty() {
            let source = if has_inferred_arrow {
                "phase-10-inferred-arrow"
            } else {
                "phase-10-inferred-value"
            };
            return (inferred_results, source);
        }
    }

    (vec![factory.atom("%Undefined%")], "fallback-undefined")
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
pub fn types_match_generic<V: MettaValueTrait>(actual: &V, expected: &V) -> bool {
    let (result, _reason) = types_match_generic_inner(actual, expected);

    // Emit TypeMatch trace event
    #[cfg(feature = "eval-trace")]
    {
        crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                0,
                crate::backend::trace::trace_value_generic(actual),
                vec![],
                None,
                trace_format::TraceEventKind::TypeMatch {
                    actual: crate::backend::trace::trace_value_generic(actual),
                    expected: crate::backend::trace::trace_value_generic(expected),
                    result,
                    reason: _reason.to_string(),
                },
            );
        });
    }

    result
}

/// Inner implementation of `types_match_generic` that returns `(result, reason)`.
///
/// The `reason` string identifies which matching rule decided the outcome,
/// for eval-trace instrumentation.
fn types_match_generic_inner<V: MettaValueTrait>(actual: &V, expected: &V) -> (bool, &'static str) {
    // %Undefined% matches anything (HE parity)
    if let Some(name) = expected.as_atom() {
        if name == "%Undefined%" {
            return (true, "expected-undefined");
        }
    }
    if let Some(name) = actual.as_atom() {
        if name == "%Undefined%" {
            return (true, "actual-undefined");
        }
    }

    // Type variables match anything
    if let Some(name) = expected.as_atom() {
        if name.starts_with('$') {
            return (true, "expected-typevar");
        }
    }
    if let Some(name) = actual.as_atom() {
        if name.starts_with('$') {
            return (true, "actual-typevar");
        }
    }

    // Type variables in Type wrapper
    if expected.is_type() {
        if let Some(inner) = expected.as_type() {
            if let Some(name) = inner.as_atom() {
                if name.starts_with('$') {
                    return (true, "type-wrapper-typevar");
                }
            }
            // Otherwise, unwrap and compare
            if actual.is_type() {
                if let Some(actual_inner) = actual.as_type() {
                    let (r, _) = types_match_generic_inner(actual_inner, inner);
                    return (r, if r { "type-wrapper-structural" } else { "type-wrapper-mismatch" });
                }
            }
        }
        return (false, "type-wrapper-mismatch");
    }

    // Exact atom matches
    if let (Some(a), Some(e)) = (actual.as_atom(), expected.as_atom()) {
        return if a == e {
            (true, "exact-atom-match")
        } else {
            (false, "exact-atom-mismatch")
        };
    }

    // Bool matches
    if let (Some(a), Some(e)) = (actual.as_bool(), expected.as_bool()) {
        return (a == e, "exact-bool");
    }

    // Long matches
    if let (Some(a), Some(e)) = (actual.as_long(), expected.as_long()) {
        return (a == e, "exact-long");
    }

    // String matches
    if let (Some(a), Some(e)) = (actual.as_string(), expected.as_string()) {
        return (a == e, "exact-string");
    }

    // S-expression matches (structural equality)
    if let (Some(a_items), Some(e_items)) = (actual.as_sexpr(), expected.as_sexpr()) {
        if a_items.len() != e_items.len() {
            return (false, "sexpr-length-mismatch");
        }
        let all_match = a_items
            .iter()
            .zip(e_items.iter())
            .all(|(a, e)| types_match_generic(a, e));
        return if all_match {
            (true, "sexpr-structural-match")
        } else {
            (false, "sexpr-structural-mismatch")
        };
    }

    // Unit matches Unit
    if actual.is_unit() && expected.is_unit() {
        return (true, "unit-match");
    }

    // Default: no match
    (false, "no-match")
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

/// Substitute type variable bindings into a type expression (Phase 10.2).
///
/// Given bindings like `{$t: Number, $u: Bool}`, transforms:
/// - `$t` → `Number`
/// - `(List $t)` → `(List Number)`
/// - `(-> $t $u)` → `(-> Number Bool)`
/// - Unbound variables remain as-is
pub fn apply_type_bindings<V, F>(
    typ: &V,
    bindings: &HashMap<String, V>,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Variable atom: substitute if bound
    if let Some(name) = typ.as_atom() {
        if name.starts_with('$') {
            if let Some(bound) = bindings.get(name) {
                return bound.clone();
            }
        }
        return typ.clone();
    }

    // Type wrapper: recurse into inner
    if typ.is_type() {
        if let Some(inner) = typ.as_type() {
            if let Some(name) = inner.as_atom() {
                if name.starts_with('$') {
                    if let Some(bound) = bindings.get(name) {
                        return bound.clone();
                    }
                }
            }
        }
        return typ.clone();
    }

    // S-expression: recurse into all children
    if let Some(items) = typ.as_sexpr() {
        let substituted: Vec<V> = items
            .iter()
            .map(|item| apply_type_bindings(item, bindings, factory))
            .collect();
        return factory.sexpr(substituted);
    }

    // Ground types, errors, etc.: return as-is
    typ.clone()
}

/// Infer an arrow type from a rule definition `(= (f params...) rhs)` (Phase 10.4).
///
/// Uses bidirectional analysis:
///   1. Forward: extract parameter variables from LHS pattern
///   2. Backward: collect type constraints on variables from RHS usage
///   3. Synthesize: build `(-> param_types... return_type)`
///
/// Returns `None` if inference fails or produces only `%Undefined%` types.
///
/// # Examples
///
/// | Rule | Constraints | Result |
/// |------|------------|--------|
/// | `(= (double $x) (+ $x $x))` | `$x: Number` | `(-> Number Number)` |
/// | `(= (neg $x) (- 0 $x))` | `$x: Number` | `(-> Number Number)` |
/// | `(= (is-pos $x) (> $x 0))` | `$x: Number` | `(-> Number Bool)` |
/// | `(= (id $x) $x)` | (none) | `None` (all `%Undefined%`) |
pub fn infer_arrow_type_from_rule<V, F>(
    lhs: &V,
    rhs: &V,
    rhs_type: Option<&V>,
    factory: &F,
    env: &GenericEnvironment<V, F>,
) -> Option<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // 1. Extract LHS parameters: (f $x $y ...) → [$x, $y, ...]
    let lhs_items = lhs.as_sexpr()?;
    if lhs_items.len() < 2 {
        return None; // Need at least (f param)
    }
    let params: Vec<&str> = lhs_items[1..]
        .iter()
        .filter_map(|v| {
            v.as_atom().filter(|name| name.starts_with('$'))
        })
        .collect();

    if params.is_empty() {
        return None; // No variable parameters (e.g., (= (f 0) 1))
    }

    // 2. Collect constraints from RHS: walk the RHS expression tree
    //    looking for subexpressions where params appear as args to typed ops.
    let mut constraints: HashMap<String, V> = HashMap::new();
    collect_param_constraints(rhs, &params, &mut constraints, factory, env);

    // 3. Resolve parameter types
    let mut param_types: Vec<V> = Vec::with_capacity(params.len());
    let mut has_useful_type = false;
    for param in &params {
        if let Some(typ) = constraints.get(*param) {
            param_types.push(typ.clone());
            has_useful_type = true;
        } else {
            param_types.push(factory.atom("%Undefined%"));
        }
    }

    // 4. Compute return type (use pre-computed rhs_type if available)
    let return_type = rhs_type
        .cloned()
        .unwrap_or_else(|| factory.atom("%Undefined%"));

    if return_type.as_atom() != Some("%Undefined%") {
        has_useful_type = true;
    }

    // 5. Filter: if all types are %Undefined%, return None (no useful info)
    if !has_useful_type {
        return None;
    }

    // 6. Synthesize arrow type: (-> param_type1 param_type2 ... return_type)
    let mut arrow_items = Vec::with_capacity(2 + param_types.len());
    arrow_items.push(factory.atom("->"));
    arrow_items.extend(param_types);
    arrow_items.push(return_type);

    Some(factory.sexpr(arrow_items))
}

/// Collect type constraints on parameter variables from RHS usage (Phase 10.4).
///
/// DFS over the RHS expression tree. For each subexpression `(op arg1 arg2 ...)`:
/// - If `op` has a known signature `(-> T1 T2 ... Tret)`, and `arg_i` is a
///   parameter variable, then constrain that variable to `T_i`.
/// - If `op` has inferred types (Phase 10.1), use those too.
///
/// Takes the first/most-specific constraint found for each variable.
fn collect_param_constraints<V, F>(
    expr: &V,
    params: &[&str],
    constraints: &mut HashMap<String, V>,
    factory: &F,
    env: &GenericEnvironment<V, F>,
)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let items = match expr.as_sexpr() {
        Some(items) if !items.is_empty() => items,
        _ => return, // Leaf node or empty — nothing to constrain
    };

    let op = match items.first().and_then(|v| v.as_atom()) {
        Some(op) => op,
        None => {
            // Operator is not an atom — recurse into all children
            for item in items {
                collect_param_constraints(item, params, constraints, factory, env);
            }
            return;
        }
    };

    // Get the operator's type signature(s) as arrow items: ["->" T1 T2 ... Tret]
    let mut op_arrow_types: Vec<Vec<V>> = Vec::new();

    // Check built-in signature registry
    if let Some(sig) = get_signature(op) {
        // Convert TypeExpr::Arrow(args, ret) to ["->" arg_types... ret_type]
        if let TypeExpr::Arrow(ref args, ref ret) = sig.type_sig {
            let mut arrow_items = vec![factory.atom("->")];
            for arg in args {
                arrow_items.push(type_expr_to_generic(arg, factory));
            }
            arrow_items.push(type_expr_to_generic(ret, factory));
            op_arrow_types.push(arrow_items);
        }
    }

    // Check declared types in environment
    for generic_type in env.get_types_generic(op) {
        if let Some(type_items) = generic_type.as_sexpr() {
            if type_items.first().and_then(|v| v.as_atom()) == Some("->") && type_items.len() >= 2 {
                op_arrow_types.push(type_items.to_vec());
            }
        }
    }

    // For each arrow type, match params to arg positions
    for arrow in &op_arrow_types {
        // arrow = ["->" T1 T2 ... Tret]
        let param_types = &arrow[1..arrow.len() - 1]; // Skip "->" and return type
        let actual_args = &items[1..]; // Skip operator

        for (i, param_type) in param_types.iter().enumerate() {
            if i < actual_args.len() {
                if let Some(arg_name) = actual_args[i].as_atom() {
                    if arg_name.starts_with('$') && params.contains(&arg_name) {
                        // This arg is a parameter variable — constrain it
                        let type_name = param_type.as_atom();
                        if type_name != Some("%Undefined%") && type_name != Some("->") {
                            constraints.entry(arg_name.to_string()).or_insert_with(|| param_type.clone());
                        }
                    }
                }
            }
        }
    }

    // Recurse into all subexpressions (including the operator's arguments)
    for item in &items[1..] {
        collect_param_constraints(item, params, constraints, factory, env);
    }
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

// ====================================================================
// Phase 8.5: Type constraint extraction for let/let* type validation
// ====================================================================

/// Extract a type constraint from a typed pattern `(: $var Type)`.
/// Returns `Some(type_value)` if the pattern is a type assertion, `None` otherwise.
///
/// Used by `ProcessLet` to short-circuit structural pattern matching when
/// the value's type is incompatible with the pattern's type constraint.
pub fn extract_type_constraint<V: MettaValueTrait + Clone>(pattern: &V) -> Option<V> {
    let items = pattern.as_sexpr()?;
    if items.len() == 3 {
        match (items[0].as_atom(), items[1].as_atom()) {
            (Some(":"), Some(name)) if name.starts_with('$') => Some(items[2].clone()),
            _ => None,
        }
    } else {
        None
    }
}

// ====================================================================
// Phase 8.6: Ground type identification for case type-driven skipping
// ====================================================================

/// Get the ground type name of a value (Number, Bool, String) if it's a literal.
/// Returns `None` for atoms, S-exprs, variables, and other non-ground types.
/// Uses pattern matching on `MettaValueInner` for O(1) dispatch.
pub fn get_ground_type<V: MettaValueTrait>(val: &V) -> Option<&'static str> {
    match val.inner_raw() {
        MettaValueInner::Long(_) | MettaValueInner::Float(_) => Some("Number"),
        MettaValueInner::Bool(_) => Some("Bool"),
        MettaValueInner::String(_) => Some("String"),
        _ => None,
    }
}

/// Check if a case pattern could match a value of the given ground type.
/// Conservative: returns `true` (compatible) if uncertain.
/// Uses pattern matching on `MettaValueInner` for efficient dispatch.
///
/// Only rejects patterns that are ground-typed literals of a different type
/// (e.g., a String literal pattern cannot match a Number scrutinee).
/// Variable patterns, S-expression patterns, and atom patterns are always
/// considered compatible since they might structurally match.
pub fn is_pattern_type_compatible<V: MettaValueTrait>(pattern: &V, ground_type: &str) -> bool {
    match pattern.inner_raw() {
        // Variables match anything
        MettaValueInner::Atom(name) if name.starts_with('$') => true,
        // Ground values: compatible only if same ground type category
        MettaValueInner::Long(_) | MettaValueInner::Float(_) => ground_type == "Number",
        MettaValueInner::Bool(_) => ground_type == "Bool",
        MettaValueInner::String(_) => ground_type == "String",
        // S-expressions, non-variable atoms, etc.: conservatively compatible
        _ => true,
    }
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

    // =========================================================================
    // Phase 8.5: extract_type_constraint tests
    // =========================================================================

    #[test]
    fn test_extract_type_constraint_typed_pattern() {
        let factory = GcFactory::default();
        // (: $x Number)
        let pattern = factory.sexpr(vec![
            factory.atom(":"),
            factory.atom("$x"),
            factory.atom("Number"),
        ]);
        let constraint = extract_type_constraint(&pattern);
        assert!(constraint.is_some());
        assert_eq!(constraint.unwrap().as_atom(), Some("Number"));
    }

    #[test]
    fn test_extract_type_constraint_non_variable() {
        let factory = GcFactory::default();
        // (: foo Number) — foo is not a variable
        let pattern = factory.sexpr(vec![
            factory.atom(":"),
            factory.atom("foo"),
            factory.atom("Number"),
        ]);
        let constraint = extract_type_constraint(&pattern);
        assert!(constraint.is_none());
    }

    #[test]
    fn test_extract_type_constraint_wrong_length() {
        let factory = GcFactory::default();
        // (: $x) — only 2 elements
        let pattern = factory.sexpr(vec![
            factory.atom(":"),
            factory.atom("$x"),
        ]);
        let constraint = extract_type_constraint(&pattern);
        assert!(constraint.is_none());
    }

    #[test]
    fn test_extract_type_constraint_non_sexpr() {
        let factory = GcFactory::default();
        // Just an atom — not an S-expression
        let pattern = factory.atom("$x");
        let constraint = extract_type_constraint(&pattern);
        assert!(constraint.is_none());
    }

    // =========================================================================
    // Phase 8.6: get_ground_type / is_pattern_type_compatible tests
    // =========================================================================

    #[test]
    fn test_get_ground_type_number() {
        assert_eq!(get_ground_type(&MettaValue::Long(42)), Some("Number"));
        assert_eq!(get_ground_type(&MettaValue::Float(3.14)), Some("Number"));
    }

    #[test]
    fn test_get_ground_type_bool() {
        assert_eq!(get_ground_type(&MettaValue::Bool(true)), Some("Bool"));
        assert_eq!(get_ground_type(&MettaValue::Bool(false)), Some("Bool"));
    }

    #[test]
    fn test_get_ground_type_string() {
        assert_eq!(get_ground_type(&MettaValue::String("hi".to_string())), Some("String"));
    }

    #[test]
    fn test_get_ground_type_atom_returns_none() {
        assert_eq!(get_ground_type(&MettaValue::Atom("foo".to_string())), None);
    }

    #[test]
    fn test_is_pattern_type_compatible_variable() {
        let factory = GcFactory::default();
        // Variables match any ground type
        assert!(is_pattern_type_compatible(&factory.atom("$x"), "Number"));
        assert!(is_pattern_type_compatible(&factory.atom("$y"), "Bool"));
    }

    #[test]
    fn test_is_pattern_type_compatible_same_type() {
        // Number literal compatible with Number
        assert!(is_pattern_type_compatible(&MettaValue::Long(42), "Number"));
        // Bool literal compatible with Bool
        assert!(is_pattern_type_compatible(&MettaValue::Bool(true), "Bool"));
        // String literal compatible with String
        assert!(is_pattern_type_compatible(&MettaValue::String("hi".to_string()), "String"));
    }

    #[test]
    fn test_is_pattern_type_compatible_different_type() {
        // Number literal NOT compatible with Bool
        assert!(!is_pattern_type_compatible(&MettaValue::Long(42), "Bool"));
        // Bool literal NOT compatible with Number
        assert!(!is_pattern_type_compatible(&MettaValue::Bool(true), "Number"));
        // String literal NOT compatible with Number
        assert!(!is_pattern_type_compatible(&MettaValue::String("hi".to_string()), "Number"));
    }

    #[test]
    fn test_is_pattern_type_compatible_non_ground_conservative() {
        let factory = GcFactory::default();
        // Non-variable atoms are conservatively compatible
        assert!(is_pattern_type_compatible(&factory.atom("foo"), "Number"));
        // S-expressions are conservatively compatible
        let sexpr = factory.sexpr(vec![factory.atom("a"), factory.atom("b")]);
        assert!(is_pattern_type_compatible(&sexpr, "Number"));
    }

    // =========================================================================
    // Phase 8.7: types_match_generic tests
    // =========================================================================

    #[test]
    fn test_types_match_generic_same() {
        let factory = GcFactory::default();
        let number = factory.atom("Number");
        assert!(types_match_generic(&number, &number));
    }

    #[test]
    fn test_types_match_generic_different() {
        let factory = GcFactory::default();
        let number = factory.atom("Number");
        let bool_t = factory.atom("Bool");
        assert!(!types_match_generic(&number, &bool_t));
    }

    #[test]
    fn test_types_match_generic_undefined_matches_any() {
        let factory = GcFactory::default();
        let number = factory.atom("Number");
        let undefined = factory.atom("%Undefined%");
        // %Undefined% on either side matches anything
        assert!(types_match_generic(&number, &undefined));
        assert!(types_match_generic(&undefined, &number));
    }

    #[test]
    fn test_types_match_generic_variable_matches_any() {
        let factory = GcFactory::default();
        let number = factory.atom("Number");
        let var = factory.atom("$t");
        // Type variables match anything
        assert!(types_match_generic(&number, &var));
        assert!(types_match_generic(&var, &number));
    }
}
