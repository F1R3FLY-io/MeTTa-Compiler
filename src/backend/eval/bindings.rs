//! Generic Binding Operations
//!
//! This module provides generic implementations of binding operations that work
//! with any value type implementing `MettaValueTrait`. These are used by the
//! generic evaluation engine to avoid conversions between value types.
//!
//! ## Operations
//!
//! - `sealed` - Create locally scoped variables
//! - `atom-subst` - Variable substitution through pattern matching

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use smallvec::SmallVec;

use crate::backend::models::{
    GenericBindings, MettaValueFactory, MettaValueInner, MettaValueTrait,
};

/// Global counter for generating unique variable IDs in `sealed`
static SEALED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Walk `val` checking for any atom whose name starts with `prefix`.
///
/// Used by the partial-unification guard in
/// `enumerate_rules_via_unification`: when `bidirectional_unify` returns a
/// binding `(query_var → value)` whose value still contains a freshened
/// rule-side variable (`$__fr_<epoch>_*`), the rule match is "partial" —
/// the rule body's body-local fresh vars cannot be fully resolved on the
/// caller side, and dispatching the rule will produce an instantiated_rhs
/// with free `$__fr_<epoch>_*` atoms that will retrigger non-deterministic
/// rule lookup at the next recursion step. Detecting this lets the caller
/// skip the rule and avoid unbounded recursion (the canonical mmverify
/// `(append $unbound (Cons "x" Nil))` case).
pub fn value_contains_var_with_prefix<V: MettaValueTrait + Clone>(val: &V, prefix: &str) -> bool {
    let mut work_stack: Vec<V> = Vec::with_capacity(8);
    work_stack.push(val.clone());
    while let Some(cur) = work_stack.pop() {
        if let Some(name) = cur.as_atom() {
            if name.starts_with(prefix) {
                return true;
            }
        } else if let Some(items) = cur.as_sexpr() {
            for item in items.iter() {
                work_stack.push(item.clone());
            }
        } else if let Some(goals) = cur.as_conjunction() {
            for goal in goals.iter() {
                work_stack.push(goal.clone());
            }
        } else if let Some((_, details)) = cur.as_error() {
            work_stack.push(details.clone());
        }
    }
    false
}

/// Compute the set of variable names transitively reachable from `value`
/// through the given `bindings`.
///
/// A variable is "live" if it appears free in `value` OR is the free var of
/// some other live variable's resolved value.
///
/// Historical note: this function was originally introduced for the "Fix 4"
/// trim in `ProcessRuleMatches` (mmverify hang plan, defense-in-depth) at
/// `eval_loop.rs:5540-5562`. That trim was REMOVED on 2026-05-06 because it
/// dropped freshened bindings that were needed by sibling iterations of an
/// enclosing foldl-atom (PLN Direct.metta tests 2/3). The proper
/// iteration-boundary liveness gate is `filter_fold_propagating_bindings`
/// at `eval_loop.rs:468-481`. Fix 1 at `engine.rs:649-680` is the actual
/// mmverify-hang fix (partial-bind rejection at rule-match source).
///
/// This helper is retained as a utility (with unit tests) for any future
/// caller that needs reachability semantics with the ProcessRuleMatches
/// awareness limitation in mind.
#[allow(dead_code)]
pub fn transitive_live_vars_generic<V: MettaValueTrait + Clone>(
    value: &V,
    bindings: &GenericBindings<V>,
) -> HashSet<String> {
    let mut live: HashSet<String> = collect_variables_generic(value);
    let mut frontier: Vec<String> = live.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        for (key, val) in bindings.iter() {
            if key == name.as_str() {
                let nested = collect_variables_generic(val);
                for var in nested {
                    if live.insert(var.clone()) {
                        frontier.push(var);
                    }
                }
            }
        }
    }
    live
}

/// Collect all variable names from an expression (generic version)
///
/// Variables are atoms starting with '$'.
pub fn collect_variables_generic<V: MettaValueTrait>(expr: &V) -> HashSet<String> {
    let mut vars = HashSet::new();
    let mut work_stack: Vec<&V> = Vec::with_capacity(16);
    work_stack.push(expr);

    while let Some(val) = work_stack.pop() {
        if let Some(name) = val.as_atom() {
            // BUG-T0-007 (spec §3.4): include &-sigil and '-sigil variables,
            // not only $-prefixed ones. The canonical predicate
            // `is_unification_variable` excludes literal `&` and space refs.
            if is_unification_variable(name) {
                vars.insert(name.to_string());
            }
        } else if let Some(items) = val.as_sexpr() {
            for item in items.iter().rev() {
                work_stack.push(item);
            }
        } else if let Some(goals) = val.as_conjunction() {
            for goal in goals.iter().rev() {
                work_stack.push(goal);
            }
        }
    }

    vars
}

/// Seal variables in an expression (generic version)
///
/// Replaces variables NOT in the ignore set with unique versions.
pub fn seal_variables_generic<V, F>(
    expr: &V,
    ignore: &HashSet<String>,
    unique_id: u64,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path for simple cases.
    // BUG-T0-007 (spec §3.4): freshen `&y` and `'z` sigil variables too,
    // not just `$x`. Uses `is_unification_variable` which excludes literal
    // `&` and space-reference atoms (`&self`, `&kb`, `&stack`).
    if let Some(name) = expr.as_atom() {
        if is_unification_variable(name) && !ignore.contains(name) {
            return factory.atom(&format!("{}_{}", name, unique_id));
        }
        return expr.clone();
    }

    // Ground types pass through unchanged
    if matches!(
        expr.inner_raw(),
        MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Unit
            | MettaValueInner::Space(_)
            | MettaValueInner::State(_)
            | MettaValueInner::Type(_)
            | MettaValueInner::Memo(_)
            | MettaValueInner::Empty
            | MettaValueInner::Error(..)
    ) {
        return expr.clone();
    }

    // Compound types need iterative processing
    seal_variables_iterative_generic(expr, ignore, unique_id, factory)
}

/// Work item for iterative seal_variables
enum SealWork<'a, V> {
    Process(&'a V),
    BuildSExpr(usize),
    BuildConjunction(usize),
}

/// Iterative implementation of seal_variables using explicit work stack.
fn seal_variables_iterative_generic<V, F>(
    expr: &V,
    ignore: &HashSet<String>,
    unique_id: u64,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let mut work_stack: Vec<SealWork<V>> = Vec::with_capacity(32);
    let mut result_stack: Vec<V> = Vec::with_capacity(32);

    work_stack.push(SealWork::Process(expr));

    while let Some(work) = work_stack.pop() {
        match work {
            SealWork::Process(val) => {
                if let Some(name) = val.as_atom() {
                    // BUG-T0-007 (spec §3.4): freshen `&y`/`'z` sigils too.
                    if is_unification_variable(name) && !ignore.contains(name) {
                        result_stack.push(factory.atom(&format!("{}_{}", name, unique_id)));
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(SealWork::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(SealWork::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(SealWork::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(SealWork::Process(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            SealWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.sexpr(children));
            }
            SealWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.conjunction(children));
            }
        }
    }

    result_stack
        .pop()
        .expect("Result stack should not be empty")
}

/// sealed: Create locally scoped variables (generic version)
/// Usage: (sealed ignore-vars expr)
pub fn eval_sealed_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() < 3 {
        let arity_msg = format!(
            "sealed requires 2 arguments, got {}. Usage: (sealed ignore-vars expr)",
            items.len() - 1
        );
        return vec![factory.error(factory.sexpr(items.to_vec()), factory.string(&arity_msg))];
    }

    let ignore_vars = &items[1];
    let expr = &items[2];

    // Collect variables to ignore
    let ignore_set = collect_variables_generic(ignore_vars);

    // Generate unique variable ID
    let unique_id = SEALED_COUNTER.fetch_add(1, Ordering::SeqCst);

    // Seal variables
    let sealed_expr = seal_variables_generic(expr, &ignore_set, unique_id, factory);

    vec![sealed_expr]
}

/// Generic pattern matching for simple variable binding
///
/// This is a simplified version that handles the common case of matching
/// a single variable against a value.
pub fn pattern_match_simple_generic<V: MettaValueTrait + Clone>(
    pattern: &V,
    value: &V,
) -> Option<GenericBindings<V>> {
    // If pattern is a variable, bind it to value
    if let Some(name) = pattern.as_atom() {
        if name.starts_with('$') {
            let mut bindings = GenericBindings::new();
            bindings.insert(name, value.clone());
            return Some(bindings);
        }
    }

    // If pattern equals value exactly, return empty bindings
    if pattern == value {
        return Some(GenericBindings::new());
    }

    // For more complex patterns, we would need full pattern matching
    // For atom-subst, we typically just have simple variable patterns
    None
}

/// Full generic pattern matching for any MettaValueTrait value.
///
/// This function performs pattern matching between a pattern and a value,
/// returning variable bindings if successful. Uses `MettaValueTrait` methods
/// instead of `MettaValueInner` pattern matching, enabling zero-conversion
/// operations for MettaValue.
///
/// ## Supported Pattern Types
///
/// - **Wildcards**: `_` matches any value, no binding created
/// - **Variables**: `$x`, `&y`, `'z` bind to the matched value
/// - **Atoms**: Must match exactly (except space references like `&self`)
/// - **Ground types**: Bool, Long, Float, String must match exactly
/// - **S-expressions**: Structural matching with recursive pattern matching
/// - **Conjunctions**: Structural matching for conjunction goals
/// - **Unit**: Matches empty S-expressions
///
/// ## Performance
///
/// Uses iterative work-stack approach to avoid stack overflow on deeply
/// nested expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks)
/// - Deeply nested data structures common in knowledge graphs
pub fn pattern_match_generic<V: MettaValueTrait + Clone>(
    pattern: &V,
    value: &V,
) -> Option<GenericBindings<V>> {
    let mut bindings = GenericBindings::new();
    if pattern_match_generic_impl::<V, NoFactory<V>>(pattern, value, &mut bindings, None) {
        Some(bindings)
    } else {
        None
    }
}

/// Variant of [`pattern_match_generic`] that supports dotted-pair patterns
/// like `($x . $rest)`. The factory is used solely to construct the SExpr
/// value bound to the rest-var; for non-dotted-pair patterns it's unused.
pub fn pattern_match_generic_with_factory<V, F>(
    pattern: &V,
    value: &V,
    factory: &F,
) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let mut bindings = GenericBindings::new();
    if pattern_match_generic_impl(pattern, value, &mut bindings, Some(factory)) {
        Some(bindings)
    } else {
        None
    }
}

/// Type stand-in used when calling `pattern_match_generic_impl` without a
/// factory. The `None` factory parameter disables dotted-pair pattern
/// support (the only place a factory is needed during matching).
enum NoFactory<V: MettaValueTrait> {
    _Phantom(std::marker::PhantomData<V>),
}
impl<V: MettaValueTrait + Clone> MettaValueFactory<V> for NoFactory<V> {
    fn atom(&self, _s: &str) -> V { unreachable!("NoFactory::atom invoked") }
    fn bool(&self, _b: bool) -> V { unreachable!("NoFactory::bool invoked") }
    fn long(&self, _n: i64) -> V { unreachable!("NoFactory::long invoked") }
    fn float(&self, _f: f64) -> V { unreachable!("NoFactory::float invoked") }
    fn string(&self, _s: &str) -> V { unreachable!("NoFactory::string invoked") }
    fn sexpr(&self, _items: Vec<V>) -> V { unreachable!("NoFactory::sexpr invoked") }
    fn sexpr_from_slice(&self, _items: &[V]) -> V {
        unreachable!("NoFactory::sexpr_from_slice invoked")
    }
    fn error(&self, _offending: V, _detail: V) -> V { unreachable!("NoFactory::error invoked") }
    fn type_value(&self, _t: V) -> V { unreachable!("NoFactory::type_value invoked") }
    fn conjunction(&self, _goals: Vec<V>) -> V { unreachable!("NoFactory::conjunction invoked") }
    fn space(&self, _h: crate::backend::models::SpaceHandle) -> V {
        unreachable!("NoFactory::space invoked")
    }
    fn state(&self, _id: u64) -> V { unreachable!("NoFactory::state invoked") }
    fn memo(&self, _h: crate::backend::models::MemoHandle) -> V {
        unreachable!("NoFactory::memo invoked")
    }
    fn quote(&self, _v: V) -> V { unreachable!("NoFactory::quote invoked") }
    fn unit(&self) -> V { unreachable!("NoFactory::unit invoked") }
    fn empty(&self) -> V { unreachable!("NoFactory::empty invoked") }
    fn conjunction_from_slice(&self, _goals: &[V]) -> V {
        unreachable!("NoFactory::conjunction_from_slice invoked")
    }
    fn spanned(&self, _v: V, _span: crate::ir::Span) -> V {
        unreachable!("NoFactory::spanned invoked")
    }
    fn deserialize(&self, _bytes: &[u8]) -> Result<(V, usize), String> {
        unreachable!("NoFactory::deserialize invoked")
    }
}

/// Internal implementation of generic pattern matching.
///
/// Uses an explicit work stack instead of recursion to handle deeply nested
/// structures without stack overflow. Stack stores **owned** `V` so the
/// BUG-T0-006 repeated-variable path can push the previously-bound value
/// back onto the stack for iterative unification (no recursion). `V: Clone`
/// is cheap (Copy for `MettaValue`, refcount-inc for Arc-backed V).
fn pattern_match_generic_impl<V, F>(
    pattern: &V,
    value: &V,
    bindings: &mut GenericBindings<V>,
    factory: Option<&F>,
) -> bool
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    // Work stack: owned (pattern, value) pairs to match.
    let mut work_stack: Vec<(V, V)> = Vec::with_capacity(16);
    work_stack.push((pattern.clone(), value.clone()));

    while let Some((pat_owned, val_owned)) = work_stack.pop() {
        // Borrow the owned pair for trait-method calls (`as_atom()` etc).
        let pat = &pat_owned;
        let val = &val_owned;
        // Check if pattern is an atom (handles variables, wildcards, and literal atoms)
        if let Some(p_name) = pat.as_atom() {
            // Wildcard matches anything (both `_` and `$_`)
            if is_wildcard_atom(p_name) {
                continue;
            }

            // Check if it's a variable (starts with $, &, or ')
            // EXCEPT: standalone "&" is a literal operator, not a variable
            // EXCEPT: space references like &self, &kb, &stack are NOT variables
            // EXCEPT: $_ is the wildcard (handled above)
            let is_variable =
                (p_name.starts_with('$') || p_name.starts_with('&') || p_name.starts_with('\''))
                    && p_name != "&"
                    && p_name != "&self"
                    && p_name != "&kb"
                    && p_name != "&stack"
                    && p_name != "$_";

            if is_variable {
                // Check if variable is already bound.
                //
                // BUG-T0-006 (spec §04.1): repeated-var consistency uses
                // *unification*, not PartialEq. The previously-bound value may
                // itself contain unbound variables; testing structural equality
                // misses cases where they would still unify (e.g., $a := f($x);
                // later $a vs f(1) must unify $x ↦ 1, not fail). Push the
                // cloned (existing, val) onto the work stack so the outer
                // iterative loop unifies them. Stack-safe (no recursion).
                //
                // Y.1 (2026-05-12): fast-path the trivially-identical case to
                // avoid the work-stack re-push that infinite-loops when both
                // existing and val are the SAME variable atom (e.g. stored
                // fact contains free `$x`, query pattern also contains `$x`
                // repeated — mork_removal_demo.metta hang).
                if let Some(existing) = bindings.get(p_name) {
                    if existing != val {
                        work_stack.push((existing.clone(), val.clone()));
                    }
                } else {
                    // New variable - bind to value
                    bindings.insert(p_name, val.clone());
                }
                continue;
            }

            // Literal atom - must match exactly
            if let Some(v_name) = val.as_atom() {
                if p_name == v_name {
                    continue;
                }
            }
            // Atom pattern "Empty" matches Empty sentinel
            if p_name == "Empty" && val.is_empty() {
                continue;
            }
            return false;
        }

        // Check ground types
        if let Some(p_bool) = pat.as_bool() {
            if let Some(v_bool) = val.as_bool() {
                if p_bool == v_bool {
                    continue;
                }
            }
            return false;
        }

        if let Some(p_long) = pat.as_long() {
            if let Some(v_long) = val.as_long() {
                if p_long == v_long {
                    continue;
                }
            }
            return false;
        }

        if let Some(p_float) = pat.as_float() {
            if let Some(v_float) = val.as_float() {
                if (p_float - v_float).abs() < f64::EPSILON {
                    continue;
                }
            }
            return false;
        }

        if let Some(p_str) = pat.as_string() {
            if let Some(v_str) = val.as_string() {
                if p_str == v_str {
                    continue;
                }
            }
            return false;
        }

        // Unit matches Unit
        if pat.is_unit() {
            if val.is_unit() {
                continue;
            }
            return false;
        }

        // S-expressions: structural matching
        if let Some(p_items) = pat.as_sexpr() {
            // Empty S-expr pattern matches empty values
            if p_items.is_empty() {
                if val.is_unit() {
                    continue;
                }
                if let Some(v_items) = val.as_sexpr() {
                    if v_items.is_empty() {
                        continue;
                    }
                }
                if let Some(name) = val.as_atom() {
                    if name == "Empty" {
                        continue;
                    }
                }
                return false;
            }

            // Non-empty S-expr must match non-empty S-expr
            if let Some(v_items) = val.as_sexpr() {
                // Dotted-pair pattern support (2026-05-11): `($x . $rest)` etc.
                // Requires a factory to build the rest-var binding; if factory
                // is None we fall through to strict length matching.
                if let Some(f) = factory {
                    if p_items.len() >= 2
                        && p_items[p_items.len() - 2].as_atom() == Some(".")
                    {
                        let head_len = p_items.len() - 2;
                        if v_items.len() < head_len {
                            return false;
                        }
                        for i in (0..head_len).rev() {
                            work_stack.push((p_items[i].clone(), v_items[i].clone()));
                        }
                        let rest_pattern = p_items[p_items.len() - 1].clone();
                        let rest_value = f.sexpr_from_slice(&v_items[head_len..]);
                        work_stack.push((rest_pattern, rest_value));
                        continue;
                    }
                }
                if p_items.len() != v_items.len() {
                    return false;
                }
                // Push children in reverse order (LIFO) — clones are cheap
                // (Copy for MettaValue; refcount-inc for Arc-backed V).
                for (p, v) in p_items.iter().zip(v_items.iter()).rev() {
                    work_stack.push((p.clone(), v.clone()));
                }
                continue;
            }
            return false;
        }

        // Conjunctions: structural matching
        if let Some(p_goals) = pat.as_conjunction() {
            if let Some(v_goals) = val.as_conjunction() {
                if p_goals.len() != v_goals.len() {
                    return false;
                }
                // Push children in reverse order (LIFO)
                for (p, v) in p_goals.iter().zip(v_goals.iter()).rev() {
                    work_stack.push((p.clone(), v.clone()));
                }
                continue;
            }
            return false;
        }

        // Errors: HE-bisimilar `Error(offending, detail)`. Recursively match
        // both slots — both are arbitrary atoms that may contain variables.
        if let Some((p_offending, p_detail)) = pat.as_error() {
            if let Some((v_offending, v_detail)) = val.as_error() {
                work_stack.push((p_detail.clone(), v_detail.clone()));
                work_stack.push((p_offending.clone(), v_offending.clone()));
                continue;
            }
            return false;
        }

        // Space handles: must match by id
        if let Some(p_handle) = pat.as_space() {
            if let Some(v_handle) = val.as_space() {
                if p_handle.id == v_handle.id {
                    continue;
                }
            }
            return false;
        }

        // State handles: must match by id
        if let Some(p_id) = pat.as_state() {
            if let Some(v_id) = val.as_state() {
                if p_id == v_id {
                    continue;
                }
            }
            return false;
        }

        // Type wrappers: match inner values
        if let Some(p_inner) = pat.as_type() {
            if let Some(v_inner) = val.as_type() {
                work_stack.push((p_inner.clone(), v_inner.clone()));
                continue;
            }
            return false;
        }

        // Empty sentinel matches empty sentinel
        if pat.is_empty() && val.is_empty() {
            continue;
        }

        // Default: no match
        return false;
    }

    true // All pairs matched successfully
}

/// Apply bindings to a template (generic version)
///
/// Replaces variables in template with their bound values.
/// Preserves Spanned wrappers: if the template has a span, the result
/// will be wrapped in Spanned with the same span.
pub fn apply_bindings_generic<V, F>(template: &V, bindings: &GenericBindings<V>, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Legacy entry point: bare-name lookup (any scope match). Phase P0 guarantees
    // every binding lives at ROOT_SCOPE so this matches the pre-scoping semantics.
    apply_bindings_scoped_generic(template, bindings, &[], factory)
}

/// Substitute bindings into `template`, preferring lookups at the supplied
/// scopes in order before falling back to bare-name (any-scope) lookup.
///
/// `scope_chain` is typically `&[dispatch_scope, ROOT_SCOPE]` when walking
/// a rule's RHS template: rule-LHS-bound atoms hit `dispatch_scope` first;
/// caller-level atoms (embedded into the RHS via bidirectional unify) miss
/// at `dispatch_scope` and fall back to `ROOT_SCOPE`. An empty `scope_chain`
/// uses the legacy any-scope `get(name)` accessor — semantics-preserving for
/// callers that haven't migrated to scoped lookup yet.
pub fn apply_bindings_scoped_generic<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    scope_chain: &[crate::backend::models::generic_bindings::ScopeId],
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path: if no bindings, return as-is
    if bindings.is_empty() {
        return template.clone();
    }

    // Peel Spanned: process inner, re-wrap with same span
    if let Some(span) = template.span() {
        let span = *span; // Copy
        let stripped = template.strip_one_span();
        let result = apply_bindings_scoped_generic(&stripped, bindings, scope_chain, factory);
        // Skip wrapping if result already carries a span
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    apply_bindings_iterative_generic(template, bindings, scope_chain, factory)
}

/// Iterative implementation of apply_bindings.
///
/// Uses an explicit work stack (pushdown automaton) instead of recursion to
/// avoid Rust call-stack overflow on deeply nested expressions or long
/// transitive binding chains.
///
/// **Transitive substitution**: when a variable lookup finds a bound value
/// that itself contains variables, those inner variables are also resolved.
/// This is critical for bidirectional unification cases where one rule
/// variable is bound to an expression containing a free input variable that
/// has been bound by unification of a sibling occurrence:
///
///     bindings: {$B → (Inheritance $1 (IntSet cancerous)), $1 → Anna}
///     template: $B
///     result:   (Inheritance Anna (IntSet cancerous))
///
/// Without transitive substitution, the result would be the unreduced
/// (Inheritance $1 (IntSet cancerous)). The transitive walk happens via the
/// existing work stack (using `Work::ProcessOwned`), so there is NO call-stack
/// growth — this is safe for arbitrarily deep transitive chains.
///
/// Cycle prevention: bidirectional_unify_generic enforces an occurs check
/// at unification time, so cyclic bindings (e.g., $a → (... $a ...)) cannot
/// enter the bindings map. Without cycles, the transitive walk terminates.
///
/// Precondition: `template` is not Spanned (caller peels it).
fn apply_bindings_iterative_generic<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    scope_chain: &[crate::backend::models::generic_bindings::ScopeId],
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Helper: scope-aware lookup. When `scope_chain` is empty (legacy callers),
    // use bare-name any-scope lookup — preserves Phase P0/P1 semantics for
    // callers that haven't migrated to scoped lookup.
    //
    // When `scope_chain` is non-empty, lookup is STRICT to the chain: an
    // out-of-chain binding (e.g. from a sibling dispatch composed into the
    // map at a different scope) is invisible to this template walk. This is
    // the §4.7a / `apply_bindings_to_atom_and_retain` invariant: a rule's
    // RHS resolves only against its own dispatch scope plus the caller's
    // scope chain, never against arbitrary same-name entries that happen
    // to live at unrelated scopes.
    let lookup = |bindings: &GenericBindings<V>, name: &str| -> Option<V> {
        if scope_chain.is_empty() {
            bindings.get(name).cloned()
        } else {
            bindings.get_chain(scope_chain, name).cloned()
        }
    };
    enum Work<'a, V> {
        /// Process a borrowed value from the original template.
        Process(&'a V),
        /// Process an owned value popped from the bindings map. Owned because
        /// the bound value's lifetime is tied to `bindings`, not `template`.
        ProcessOwned(V),
        /// `original` enables identity-equality lazy-allocation: if every
        /// child after substitution is pointer-equal to the original child,
        /// reuse `original` verbatim instead of allocating a new sexpr.
        BuildSExpr {
            count: usize,
            original: V,
        },
        BuildConjunction {
            count: usize,
            original: V,
        },
    }

    // Inline-storage stacks: most calls process small expressions and
    // never spill to the heap. Bytehound profile of Smokes.metta showed
    // 9.5M calls to this function each allocating two 32-cap Vecs (~7 GB
    // cumulative) — pure allocator churn, not retention.
    let mut work_stack: SmallVec<[Work<V>; 16]> = SmallVec::new();
    let mut result_stack: SmallVec<[V; 16]> = SmallVec::new();

    work_stack.push(Work::Process(template));

    while let Some(work) = work_stack.pop() {
        match work {
            Work::Process(val) => {
                // Structural sharing: subtrees with no variables cannot
                // substitute (no `$var` to look up), so the original
                // pointer is the substituted result. Reusing it avoids
                // rebuilding the spine of the template tree at every
                // call. Bytehound profile of Smokes.metta showed 9.5M
                // calls to this function — most of the per-call work
                // was rebuilding ground subtrees that never changed.
                if !val.has_variables_fast() {
                    result_stack.push(val.clone());
                    continue;
                }
                // Handle Spanned children by peeling span, processing, re-wrapping.
                // This calls apply_bindings_generic which peels one Spanned layer,
                // then calls apply_bindings_iterative_generic on the stripped value.
                // Safe because MettaValue has at most one Spanned layer.
                if val.is_spanned() {
                    let result = apply_bindings_scoped_generic(val, bindings, scope_chain, factory);
                    result_stack.push(result);
                    continue;
                }

                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') {
                        if let Some(bound) = lookup(bindings, name) {
                            // Guard: self-referential binding ($a → $a) — emit
                            // directly to prevent infinite transitive loop.
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound.clone());
                                continue;
                            }
                            // Transitive substitution: push the bound value
                            // back to the work stack so any inner variables
                            // also get substituted.
                            //
                            // Note: HE's M-VAR-VAR-DISTINCT (spec §4.3.1) requires
                            // returning the ORIGINAL variable when the chain ends
                            // at an unbound variable. Implementing this without
                            // breaking rule-LHS-var-to-query-var substitution
                            // needs the Equivalence variant — deferred to S11.
                            work_stack.push(Work::ProcessOwned(bound.clone()));
                        } else {
                            // Phase 3.2-B: emit a `VariableLookupFailed`
                            // diagnostic trace when a `$`-prefixed atom
                            // in the template has no matching key in
                            // the supplied bindings.
                            #[cfg(feature = "trace")]
                            {
                                let keys: Vec<String> =
                                    bindings.iter().map(|(k, _)| k.to_string()).collect();
                                crate::backend::trace::thread_local_sink::with_trace_collector_ref(
                                    |tc| {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            0,
                                            crate::backend::trace::trace_value_generic(val),
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::VariableLookupFailed {
                                                context: "apply_bindings/template".to_string(),
                                                var_name: name.to_string(),
                                                available_keys: keys.clone(),
                                                template_excerpt:
                                                    crate::backend::trace::trace_value_generic(val),
                                            },
                                        );
                                    },
                                );
                            }
                            result_stack.push(val.clone());
                        }
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildSExpr {
                            count: items.len(),
                            original: val.clone(),
                        });
                        for item in items.iter().rev() {
                            work_stack.push(Work::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildConjunction {
                            count: goals.len(),
                            original: val.clone(),
                        });
                        for goal in goals.iter().rev() {
                            work_stack.push(Work::Process(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            Work::ProcessOwned(val) => {
                // Structural sharing: same rationale as in `Process`.
                if !val.has_variables_fast() {
                    result_stack.push(val);
                    continue;
                }
                // Same logic as `Process` but operating on an owned value
                // (the value was popped from the bindings map, so its
                // lifetime is no longer tied to the input template).
                if val.is_spanned() {
                    let result =
                        apply_bindings_scoped_generic(&val, bindings, scope_chain, factory);
                    result_stack.push(result);
                    continue;
                }

                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') {
                        if let Some(bound) = lookup(bindings, name) {
                            // Guard: self-referential binding ($a → $a) — emit
                            // directly to prevent infinite transitive loop.
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound.clone());
                                continue;
                            }
                            // Transitive: re-process the (newly) bound value.
                            work_stack.push(Work::ProcessOwned(bound.clone()));
                        } else {
                            result_stack.push(val);
                        }
                    } else {
                        result_stack.push(val);
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val);
                    } else {
                        // Children are borrowed from `val`; clone each before
                        // pushing because `val` is consumed when this branch ends.
                        let len = items.len();
                        let owned_children: Vec<V> = items.iter().cloned().collect();
                        work_stack.push(Work::BuildSExpr {
                            count: len,
                            original: val,
                        });
                        for item in owned_children.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = goals.len();
                        let owned_goals: Vec<V> = goals.iter().cloned().collect();
                        work_stack.push(Work::BuildConjunction {
                            count: len,
                            original: val,
                        });
                        for goal in owned_goals.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(goal));
                        }
                    }
                } else {
                    result_stack.push(val);
                }
            }
            Work::BuildSExpr { count, original } => {
                let start = result_stack.len() - count;
                // Identity-equality lazy-allocation: if every new child is
                // pointer-equal to the corresponding original child, reuse
                // `original` verbatim instead of allocating a new sexpr.
                let items = original
                    .as_sexpr()
                    .expect("BuildSExpr original must be sexpr");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&items[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                } else {
                    let result = factory.sexpr_from_slice(&result_stack[start..]);
                    result_stack.truncate(start);
                    result_stack.push(result);
                }
            }
            Work::BuildConjunction { count, original } => {
                let start = result_stack.len() - count;
                let goals = original
                    .as_conjunction()
                    .expect("BuildConjunction original must be conjunction");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&goals[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                } else {
                    let result = factory.conjunction_from_slice(&result_stack[start..]);
                    result_stack.truncate(start);
                    result_stack.push(result);
                }
            }
        }
    }

    result_stack
        .pop()
        .expect("Result stack should not be empty")
}

// ============================================================================
// S0d.1 — Class-aware apply_bindings
// ============================================================================

/// Apply class-aware bindings to `template`.
///
/// Fast path: if `bindings.is_empty_classes()`, delegates straight to the
/// entries-only [`apply_bindings_generic`]. The class machinery is only
/// invoked when at least one equivalence class has been formed (Unify mode
/// var-var-distinct outcome).
///
/// When a class is consulted:
/// 1. Ordinary entries (`bindings.entries.get(name)`) take precedence
///    (HE invariant: one slot per name).
/// 2. Class lookup: if value-bearing, return the class value; if value-less,
///    return the ORIGINAL lookup-key as an Atom (preserves T03/004
///    strict alpha-distinct output).
/// 3. Unbound: emit `val` as-is.
pub fn apply_bindings_with_classes_generic<V, F>(
    template: &V,
    bindings: &crate::backend::models::BindingsWithClasses<V>,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path: no equivalence classes formed — delegate to existing
    // entries-only path. Bit-identical to pre-S0d behavior.
    if bindings.is_empty_classes() {
        return apply_bindings_generic(template, &bindings.entries, factory);
    }

    // Peel Spanned: process inner, re-wrap with same span
    if let Some(span) = template.span() {
        let span = *span;
        let stripped = template.strip_one_span();
        let result = apply_bindings_with_classes_generic(&stripped, bindings, factory);
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    apply_bindings_iterative_with_classes_generic(template, bindings, factory)
}

/// Iterative class-aware implementation. Mirrors
/// [`apply_bindings_iterative_generic`] but lookups go through the
/// `BindingsWithClasses` resolver.
fn apply_bindings_iterative_with_classes_generic<V, F>(
    template: &V,
    bindings: &crate::backend::models::BindingsWithClasses<V>,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    // Class-aware lookup: ordinary entry → class value → value-less class →
    // None. For a value-less class, the caller emits the original lookup-key
    // verbatim (handled by returning None here so the work-loop falls through
    // to `result_stack.push(val.clone())`).
    let lookup_entry = |name: &str| -> Option<V> { bindings.entries.get(name).cloned() };
    let lookup_class_value = |name: &str| -> Option<V> {
        let table = bindings.classes.as_ref()?;
        let id = table.class_of(name)?;
        table.class_value(id).cloned()
    };
    let is_in_class = |name: &str| -> bool {
        bindings
            .classes
            .as_ref()
            .and_then(|t| t.class_of(name))
            .is_some()
    };

    enum Work<'a, V> {
        Process(&'a V),
        ProcessOwned(V),
        BuildSExpr { count: usize, original: V },
        BuildConjunction { count: usize, original: V },
    }

    let mut work_stack: SmallVec<[Work<V>; 16]> = SmallVec::new();
    let mut result_stack: SmallVec<[V; 16]> = SmallVec::new();

    work_stack.push(Work::Process(template));

    while let Some(work) = work_stack.pop() {
        match work {
            Work::Process(val) => {
                if !val.has_variables_fast() {
                    result_stack.push(val.clone());
                    continue;
                }
                if val.is_spanned() {
                    let result = apply_bindings_with_classes_generic(val, bindings, factory);
                    result_stack.push(result);
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') {
                        // 1. Ordinary binding takes precedence.
                        if let Some(bound) = lookup_entry(name) {
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound);
                                continue;
                            }
                            work_stack.push(Work::ProcessOwned(bound));
                            continue;
                        }
                        // 2. Class lookup.
                        if let Some(v) = lookup_class_value(name) {
                            // Value-bearing class: push for transitive process.
                            if v.as_atom() == Some(name) {
                                result_stack.push(v);
                                continue;
                            }
                            work_stack.push(Work::ProcessOwned(v));
                            continue;
                        }
                        if is_in_class(name) {
                            // Value-less class: emit original lookup-key.
                            // This is the T03/004 strict-alpha invariant —
                            // class members preserve their original name,
                            // they do NOT collapse to a canonical
                            // representative.
                            result_stack.push(val.clone());
                            continue;
                        }
                        // 3. Unbound: emit as-is.
                        result_stack.push(val.clone());
                        continue;
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildSExpr {
                            count: items.len(),
                            original: val.clone(),
                        });
                        for item in items.iter().rev() {
                            work_stack.push(Work::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildConjunction {
                            count: goals.len(),
                            original: val.clone(),
                        });
                        for goal in goals.iter().rev() {
                            work_stack.push(Work::Process(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            Work::ProcessOwned(val) => {
                if !val.has_variables_fast() {
                    result_stack.push(val);
                    continue;
                }
                if val.is_spanned() {
                    let result = apply_bindings_with_classes_generic(&val, bindings, factory);
                    result_stack.push(result);
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') {
                        if let Some(bound) = lookup_entry(name) {
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound);
                                continue;
                            }
                            work_stack.push(Work::ProcessOwned(bound));
                            continue;
                        }
                        if let Some(v) = lookup_class_value(name) {
                            if v.as_atom() == Some(name) {
                                result_stack.push(v);
                                continue;
                            }
                            work_stack.push(Work::ProcessOwned(v));
                            continue;
                        }
                        if is_in_class(name) {
                            // Value-less class: emit ORIGINAL atom.
                            result_stack.push(val);
                            continue;
                        }
                        result_stack.push(val);
                        continue;
                    } else {
                        result_stack.push(val);
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = items.len();
                        let owned_children: Vec<V> = items.iter().cloned().collect();
                        work_stack.push(Work::BuildSExpr {
                            count: len,
                            original: val,
                        });
                        for item in owned_children.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = goals.len();
                        let owned_goals: Vec<V> = goals.iter().cloned().collect();
                        work_stack.push(Work::BuildConjunction {
                            count: len,
                            original: val,
                        });
                        for goal in owned_goals.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(goal));
                        }
                    }
                } else {
                    result_stack.push(val);
                }
            }
            Work::BuildSExpr { count, original } => {
                let start = result_stack.len() - count;
                let items = original
                    .as_sexpr()
                    .expect("BuildSExpr original must be sexpr");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&items[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                } else {
                    let result = factory.sexpr_from_slice(&result_stack[start..]);
                    result_stack.truncate(start);
                    result_stack.push(result);
                }
            }
            Work::BuildConjunction { count, original } => {
                let start = result_stack.len() - count;
                let goals = original
                    .as_conjunction()
                    .expect("BuildConjunction original must be conjunction");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&goals[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                } else {
                    let result = factory.conjunction_from_slice(&result_stack[start..]);
                    result_stack.truncate(start);
                    result_stack.push(result);
                }
            }
        }
    }

    result_stack
        .pop()
        .expect("Result stack should not be empty")
}

/// Apply bindings to `template` while freshening template variables per-invocation.
///
/// Fuses three operations that `match_rules_native` used to perform as three
/// separate tree walks:
///   1. `freshen_variables_with_epoch(template)` — rename every `$x` → `$__fr_{epoch}_x`.
///   2. `freshen_bindings_keys_with_epoch(bindings, rule_var_names)` — rename LHS keys.
///   3. `apply_bindings_generic(rhs_freshened, bindings_for_apply)` — substitute.
///
/// The fused version performs ONE walk over `template`. For each `$var` encountered:
/// - Look up `var` (original name) in `bindings`. If found, transitively process the
///   bound value (WITHOUT rename — bound values come from caller scope and retain
///   their original variable names per spec §04.3).
/// - If not found in `bindings`, emit the **renamed** form `$__fr_{epoch}_{bare}` so
///   that body-local variables (not present in bindings) still get per-invocation
///   freshening, preserving scope isolation across recursive rule invocations.
///
/// This is spec §03.3 CachingMapper behavior at O(|rule_var_names| + |template|) cost,
/// replacing the former O(3·|template|) three-walk.
///
/// Preserves Spanned wrappers identically to `apply_bindings_generic`.
/// Handles self-referential bindings (`$a → $a`) and transitive chains via the
/// same iterative work-stack pattern.
pub fn apply_bindings_with_rename_generic<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    rename: &crate::backend::eval::freshening::CachingRename,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Peel Spanned: process inner, re-wrap with same span
    if let Some(span) = template.span() {
        let span = *span;
        let stripped = template.strip_one_span();
        let result = apply_bindings_with_rename_generic(&stripped, bindings, rename, factory);
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    // Function-entry structural-sharing guard: a template with no variables
    // cannot rename or substitute, so we return the original verbatim.
    // This avoids the SmallVec setup and a pop+match round-trip for the
    // common ground-template case (every call to this function pays this
    // check; the inner-loop check at line ~821 handles deeper subtrees).
    if !template.has_variables_fast() {
        return template.clone();
    }

    // Two work-stack modes:
    //   ProcessTemplate — still descending through the template; rename on miss.
    //   ProcessOwned — descending through a bound VALUE (retains caller-scope
    //                  variable names; no rename on miss).
    //
    // BuildSExpr/BuildConjunction carry the `original` value so that when every
    // produced child is identity-equal to the original child, we reuse `original`
    // verbatim instead of allocating a fresh slab slot. Mirrors
    // `apply_bindings_iterative_generic`'s identity-equality lazy-allocation
    // and `apply_bindings_inner`'s `identity_eq` check at engine.rs:212.
    enum Work<'a, V> {
        ProcessTemplate(&'a V),
        ProcessOwned(V),
        BuildSExpr { count: usize, original: V },
        BuildConjunction { count: usize, original: V },
    }

    // Inline-storage stacks (see apply_bindings_iterative_generic for rationale).
    let mut work_stack: SmallVec<[Work<V>; 16]> = SmallVec::new();
    let mut result_stack: SmallVec<[V; 16]> = SmallVec::new();

    work_stack.push(Work::ProcessTemplate(template));

    while let Some(work) = work_stack.pop() {
        match work {
            Work::ProcessTemplate(val) => {
                // Structural sharing: subtrees with no variables cannot
                // substitute (no var to look up in bindings) or rename
                // (no `$` atom to rewrite), so the original pointer can
                // be reused. This collapses the per-match cost from
                // O(RHS tree size) to O(variable occurrences).
                if !val.has_variables_fast() {
                    result_stack.push(val.clone());
                    continue;
                }
                if val.is_spanned() {
                    let result = apply_bindings_with_rename_generic(val, bindings, rename, factory);
                    result_stack.push(result);
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') && name != "$_" {
                        if let Some(bound) = bindings.get(name) {
                            // Self-referential guard
                            if bound.as_atom() == Some(name) {
                                // Emit the renamed form directly (cached per dispatch)
                                result_stack.push(rename.fresh_atom(name, factory));
                                continue;
                            }
                            // Transitive: descend into bound value (owned mode — no rename)
                            work_stack.push(Work::ProcessOwned(bound.clone()));
                        } else {
                            // Miss → emit renamed form (body-local or non-bound rule var)
                            result_stack.push(rename.fresh_atom(name, factory));
                        }
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildSExpr {
                            count: items.len(),
                            original: val.clone(),
                        });
                        for item in items.iter().rev() {
                            work_stack.push(Work::ProcessTemplate(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildConjunction {
                            count: goals.len(),
                            original: val.clone(),
                        });
                        for goal in goals.iter().rev() {
                            work_stack.push(Work::ProcessTemplate(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            Work::ProcessOwned(val) => {
                // Structural sharing in owned mode: no rename happens at
                // all here (miss → emit as-is), so a no-variable subtree
                // is identical to itself.
                if !val.has_variables_fast() {
                    result_stack.push(val);
                    continue;
                }
                if val.is_spanned() {
                    // Bug fix (2026-05-06): preserve owned-mode invariant
                    // across Spanned peeling. Pre-fix, this arm recursed
                    // into `apply_bindings_with_rename_generic` (the
                    // top-level entry), which restarts in
                    // `Work::ProcessTemplate` mode at the equivalent of
                    // line 2172 — silently flipping owned (no-rename) to
                    // template (rename-on-miss). The recursion would then
                    // freshen any user-scope variable inside an "owned"
                    // (caller-scope) substituted value, producing names
                    // like `$__fr_E_who` that downstream binding lookups
                    // can never match.
                    //
                    // Fix: peel the Spanned wrapper LOCALLY and re-push
                    // as `Work::ProcessOwned` so the "owned, no rename"
                    // semantic survives span boundaries. Span loss on
                    // interior substituted values is safe (audited every
                    // `is_spanned()` consumer; outer-template span on the
                    // parent `BuildSExpr.original` survives, which is
                    // the only span LSP/diagnostic consumers care about).
                    work_stack.push(Work::ProcessOwned(val.strip_one_span()));
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') && name != "$_" {
                        if let Some(bound) = bindings.get(name) {
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound.clone());
                                continue;
                            }
                            work_stack.push(Work::ProcessOwned(bound.clone()));
                        } else {
                            // Miss in owned mode → emit as-is (caller-scope name)
                            result_stack.push(val);
                        }
                    } else {
                        result_stack.push(val);
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = items.len();
                        let owned_children: Vec<V> = items.iter().cloned().collect();
                        work_stack.push(Work::BuildSExpr {
                            count: len,
                            original: val,
                        });
                        for item in owned_children.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = goals.len();
                        let owned_goals: Vec<V> = goals.iter().cloned().collect();
                        work_stack.push(Work::BuildConjunction {
                            count: len,
                            original: val,
                        });
                        for goal in owned_goals.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(goal));
                        }
                    }
                } else {
                    result_stack.push(val);
                }
            }
            Work::BuildSExpr { count, original } => {
                let start = result_stack.len() - count;
                // Identity-equality lazy-allocation: if every produced child
                // is pointer-equal to the corresponding original child,
                // reuse `original` instead of allocating a new sexpr.
                let items = original
                    .as_sexpr()
                    .expect("BuildSExpr original must be sexpr");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&items[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                    continue;
                }
                let result = factory.sexpr_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
            Work::BuildConjunction { count, original } => {
                let start = result_stack.len() - count;
                let goals = original
                    .as_conjunction()
                    .expect("BuildConjunction original must be conjunction");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&goals[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                    continue;
                }
                let result = factory.conjunction_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
        }
    }

    result_stack
        .pop()
        .expect("Result stack should not be empty")
}

// =============================================================================
// Bidirectional Unification (Martelli-Montanari)
// =============================================================================

/// Bidirectional unification using the Martelli-Montanari algorithm.
///
/// Unlike one-directional `pattern_match_generic` (which only binds variables
/// on the pattern side), this handles variables on **both sides** simultaneously.
///
/// ## Algorithm
///
/// Maintains a work stack of `(V, V)` equation pairs. For each pair:
/// 1. **Deref** both sides through existing bindings (transitive)
/// 2. **Variable vs anything**: Occurs check, then bind (or verify consistency)
/// 3. **S-expression decomposition**: Arity check, then push child pairs
/// 4. **Ground term comparison**: Structural equality
///
/// Based on: Martelli & Montanari (1982), "An Efficient Unification Algorithm"
/// Reference implementation with Rocq proofs: mettail-rust/prattail/src/unification.rs
///
/// ## Complexity
///
/// O(n * k) where n = total term size and k = number of variables (for consistency
/// checks). Occurs check adds O(|term|) per binding. For typical MeTTa patterns
/// with 3-10 variables, this is effectively linear.
///
/// ## Examples
///
/// ```text
/// unify((a $x), ($y b))         => Some({$x → b, $y → a})
/// unify(($x $x), (a a))         => Some({$x → a})
/// unify(($x $x), (a b))         => None  (variable consistency)
/// unify($x, (f $x))             => None  (occurs check)
/// unify((f _ $y), (f anything z)) => Some({$y → z})
/// ```
pub fn bidirectional_unify_generic<V: MettaValueTrait + Clone>(
    a: &V,
    b: &V,
) -> Option<GenericBindings<V>> {
    let mut bindings = GenericBindings::new();
    if bidirectional_unify_generic_impl(a, b, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

/// Check if `name` is a wildcard atom that matches anything without binding.
///
/// Both bare `_` and `$_` are wildcards in MeTTaTron: they match any value
/// without recording a binding. Each occurrence is independent (no unification
/// across occurrences of the same wildcard, unlike a normal variable).
///
/// This differs from HE, which treats `$_` as a regular variable named `_`
/// with per-query CachingMapper uniqueness; our wildcard treatment gives the
/// same practical effect (independent occurrences, no binding) with simpler
/// semantics that match MeTTa code like mmverify's `(DVar ($x $y) $_ ...)`.
///
/// Named wildcard-like variables (`$_x`, `$_foo`) are NOT wildcards — they
/// are regular variables with underscore-prefixed names.
#[inline]
pub fn is_wildcard_atom(name: &str) -> bool {
    name == "_" || name == "$_"
}

/// Check if `name` refers to a MeTTa variable (starts with `$`, `&`, or `'`).
///
/// Excludes standalone `&` (literal operator), space references (`&self`,
/// `&kb`, `&stack`), and the `$_` wildcard.
#[inline]
fn is_unification_variable(name: &str) -> bool {
    (name.starts_with('$') || name.starts_with('&') || name.starts_with('\''))
        && name != "&"
        && name != "&self"
        && name != "&kb"
        && name != "&stack"
        && name != "$_"
}

/// Iterative occurs check: does variable `var_name` appear anywhere in `term`?
///
/// Uses an explicit work stack to avoid stack overflow on deeply nested terms.
/// Returns `true` if `var_name` occurs in `term` (unification would create an
/// infinite term).
/// Public wrapper for occurs check (used by VM opcodes).
pub fn occurs_in_generic_pub<V: MettaValueTrait + Clone>(
    var_name: &str,
    term: &V,
    bindings: &GenericBindings<V>,
) -> bool {
    occurs_in_generic(var_name, term, bindings)
}

fn occurs_in_generic<V: MettaValueTrait + Clone>(
    var_name: &str,
    term: &V,
    bindings: &GenericBindings<V>,
) -> bool {
    // Owned work stack (V is Copy for MettaValue, so cloning is zero-cost)
    let mut work_stack: Vec<V> = Vec::with_capacity(16);
    work_stack.push(term.clone());

    while let Some(raw) = work_stack.pop() {
        // Deref through bindings
        let current = deref_value_owned(&raw, bindings);

        if let Some(name) = current.as_atom() {
            if name == var_name {
                return true;
            }
            // Don't descend into atoms further
            continue;
        }

        if let Some(items) = current.as_sexpr() {
            for item in items.iter() {
                work_stack.push(item.clone());
            }
            continue;
        }

        if let Some(goals) = current.as_conjunction() {
            for goal in goals.iter() {
                work_stack.push(goal.clone());
            }
            continue;
        }

        // HE-bisimilar Error(offending, detail): both slots are full values
        // that may contain variables. Push both for traversal.
        if let Some((offending, detail)) = current.as_error() {
            work_stack.push(offending.clone());
            work_stack.push(detail.clone());
            continue;
        }

        if let Some(inner) = current.as_type() {
            work_stack.push(inner.clone());
        }

        // Ground types (Long, Bool, Float, String, Unit, etc.) cannot contain variables
    }

    false
}

/// Internal implementation of Martelli-Montanari bidirectional unification.
///
/// Uses an owned work stack of equation pairs `(lhs, rhs)`. Since `V` is
/// typically Copy (MettaValue is 8 bytes), cloning is zero-cost.
/// Bindings are accumulated in the `bindings` parameter.
fn bidirectional_unify_generic_impl<V: MettaValueTrait + Clone>(
    a: &V,
    b: &V,
    bindings: &mut GenericBindings<V>,
) -> bool {
    // Owned work stack — V is Copy for MettaValue so cloning is zero-cost.
    let mut work_stack: Vec<(V, V)> = Vec::with_capacity(16);
    work_stack.push((a.clone(), b.clone()));

    while let Some((lhs_raw, rhs_raw)) = work_stack.pop() {
        // Step 1: Dereference both sides through existing bindings (transitive)
        let lhs = deref_value_owned(&lhs_raw, bindings);
        let rhs = deref_value_owned(&rhs_raw, bindings);

        // Step 2: Trivial identity
        if lhs == rhs {
            continue;
        }

        // Step 3: Wildcards (match anything without binding). Both `_` and
        // `$_` are wildcards — each occurrence is independent.
        if let Some(name) = lhs.as_atom() {
            if is_wildcard_atom(name) {
                continue;
            }
        }
        if let Some(name) = rhs.as_atom() {
            if is_wildcard_atom(name) {
                continue;
            }
        }

        // Step 4: Variable on LHS
        if let Some(l_name) = lhs.as_atom() {
            if is_unification_variable(l_name) {
                // Already bound? Push (existing_value, rhs) for consistency check
                if let Some(existing) = bindings.get(l_name) {
                    let existing = existing.clone();
                    work_stack.push((existing, rhs));
                    continue;
                }

                // Occurs check: prevent $x = f($x) → infinite terms
                if occurs_in_generic(l_name, &rhs, bindings) {
                    return false;
                }

                // Bind: l_name → rhs
                bindings.insert(l_name, rhs);
                continue;
            }
        }

        // Step 5: Variable on RHS
        if let Some(r_name) = rhs.as_atom() {
            if is_unification_variable(r_name) {
                // Already bound? Push (existing_value, lhs) for consistency check
                if let Some(existing) = bindings.get(r_name) {
                    let existing = existing.clone();
                    work_stack.push((existing, lhs));
                    continue;
                }

                // Occurs check
                if occurs_in_generic(r_name, &lhs, bindings) {
                    return false;
                }

                // Bind: r_name → lhs
                bindings.insert(r_name, lhs);
                continue;
            }
        }

        // Step 6: Both are non-variable — structural comparison

        // Atoms: must match exactly
        if let Some(l_name) = lhs.as_atom() {
            if let Some(r_name) = rhs.as_atom() {
                if l_name == r_name {
                    continue;
                }
            }
            // Atom "Empty" matches Empty sentinel
            if l_name == "Empty" && rhs.is_empty() {
                continue;
            }
            return false;
        }
        // RHS atom "Empty" matching LHS Empty sentinel
        if let Some(r_name) = rhs.as_atom() {
            if r_name == "Empty" && lhs.is_empty() {
                continue;
            }
            return false;
        }

        // Ground types
        if let Some(l_bool) = lhs.as_bool() {
            if let Some(r_bool) = rhs.as_bool() {
                if l_bool == r_bool {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_long) = lhs.as_long() {
            if let Some(r_long) = rhs.as_long() {
                if l_long == r_long {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_float) = lhs.as_float() {
            if let Some(r_float) = rhs.as_float() {
                if l_float.to_bits() == r_float.to_bits() {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_str) = lhs.as_string() {
            if let Some(r_str) = rhs.as_string() {
                if l_str == r_str {
                    continue;
                }
            }
            return false;
        }

        // Unit matches Unit
        if lhs.is_unit() {
            if rhs.is_unit() {
                continue;
            }
            // Unit matches empty S-expr
            if let Some(r_items) = rhs.as_sexpr() {
                if r_items.is_empty() {
                    continue;
                }
            }
            return false;
        }
        if rhs.is_unit() {
            // Empty S-expr matches Unit (symmetric)
            if let Some(l_items) = lhs.as_sexpr() {
                if l_items.is_empty() {
                    continue;
                }
            }
            return false;
        }

        // S-expressions: decompose element-wise
        if let Some(l_items) = lhs.as_sexpr() {
            if let Some(r_items) = rhs.as_sexpr() {
                if l_items.len() != r_items.len() {
                    return false;
                }
                if l_items.is_empty() {
                    continue;
                }
                // Push child pairs in reverse order (LIFO → left-to-right processing)
                for (l, r) in l_items.iter().zip(r_items.iter()).rev() {
                    work_stack.push((l.clone(), r.clone()));
                }
                continue;
            }
            return false;
        }

        // Conjunctions: structural matching
        if let Some(l_goals) = lhs.as_conjunction() {
            if let Some(r_goals) = rhs.as_conjunction() {
                if l_goals.len() != r_goals.len() {
                    return false;
                }
                for (l, r) in l_goals.iter().zip(r_goals.iter()).rev() {
                    work_stack.push((l.clone(), r.clone()));
                }
                continue;
            }
            return false;
        }

        // Errors: HE-bisimilar `Error(offending, detail)`. Recurse into both
        // slots so unification handles variables in either position.
        if let Some((l_offending, l_detail)) = lhs.as_error() {
            if let Some((r_offending, r_detail)) = rhs.as_error() {
                work_stack.push((l_detail.clone(), r_detail.clone()));
                work_stack.push((l_offending.clone(), r_offending.clone()));
                continue;
            }
            return false;
        }

        // Space handles
        if let Some(l_handle) = lhs.as_space() {
            if let Some(r_handle) = rhs.as_space() {
                if l_handle.id == r_handle.id {
                    continue;
                }
            }
            return false;
        }

        // State handles
        if let Some(l_id) = lhs.as_state() {
            if let Some(r_id) = rhs.as_state() {
                if l_id == r_id {
                    continue;
                }
            }
            return false;
        }

        // Type wrappers
        if let Some(l_inner) = lhs.as_type() {
            if let Some(r_inner) = rhs.as_type() {
                work_stack.push((l_inner.clone(), r_inner.clone()));
                continue;
            }
            return false;
        }

        // Empty sentinel
        if lhs.is_empty() && rhs.is_empty() {
            continue;
        }

        // Default: no match
        return false;
    }

    true // All equation pairs unified successfully
}

// ============================================================================
// S0d.1 — Class-aware bidirectional unification (UnifyMode dispatch)
// ============================================================================

/// Bidirectional unification with explicit [`UnifyMode`].
///
/// - [`UnifyMode::Match`]: behaves identically to
///   [`bidirectional_unify_generic`] — ordinary chain-terminus binding for
///   var-var-distinct pairs. The returned [`BindingsWithClasses`] will have
///   no class table (the `classes` field is `None`).
///
/// - [`UnifyMode::Unify`]: under var-var-distinct, creates an equivalence
///   class via [`BindingsWithClasses::insert_equivalence`]. Honors HE's
///   M-VAR-VAR-DISTINCT (spec §4.3.1).
pub fn bidirectional_unify_generic_with_mode<V>(
    a: &V,
    b: &V,
    mode: crate::backend::models::UnifyMode,
) -> Option<crate::backend::models::BindingsWithClasses<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
{
    let mut bindings = crate::backend::models::BindingsWithClasses::<V>::new();
    if bidirectional_unify_with_classes_impl(a, b, &mut bindings, mode) {
        Some(bindings)
    } else {
        None
    }
}

/// Class-aware internal unifier. Mirrors [`bidirectional_unify_generic_impl`]
/// but routes value installation through [`BindingsWithClasses::insert_value`]
/// (which respects existing equivalence classes) and creates equivalence
/// classes for var-var-distinct pairs under [`UnifyMode::Unify`].
fn bidirectional_unify_with_classes_impl<V>(
    a: &V,
    b: &V,
    bindings: &mut crate::backend::models::BindingsWithClasses<V>,
    mode: crate::backend::models::UnifyMode,
) -> bool
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
{
    use crate::backend::models::{MergeConflict, UnifyMode};

    // Owned work stack — V is Copy for MettaValue so cloning is zero-cost.
    let mut work_stack: Vec<(V, V)> = Vec::with_capacity(16);
    work_stack.push((a.clone(), b.clone()));

    while let Some((lhs_raw, rhs_raw)) = work_stack.pop() {
        // Step 1: Dereference both sides through existing entries (transitive).
        // We still deref through `entries` only; class lookup is checked when
        // we encounter a fresh variable (handled below).
        let lhs = deref_value_owned(&lhs_raw, &bindings.entries);
        let rhs = deref_value_owned(&rhs_raw, &bindings.entries);

        // Step 2: Trivial identity
        if lhs == rhs {
            continue;
        }

        // Step 3: Wildcards
        if let Some(name) = lhs.as_atom() {
            if is_wildcard_atom(name) {
                continue;
            }
        }
        if let Some(name) = rhs.as_atom() {
            if is_wildcard_atom(name) {
                continue;
            }
        }

        // Step 4: Variable on LHS
        if let Some(l_name) = lhs.as_atom() {
            if is_unification_variable(l_name) {
                // Already bound via ordinary entry? Push consistency check.
                if let Some(existing) = bindings.entries.get(l_name) {
                    let existing = existing.clone();
                    work_stack.push((existing, rhs));
                    continue;
                }
                // Already a class member with value? Push consistency check.
                if let Some(table) = bindings.classes.as_ref() {
                    if let Some(id) = table.class_of(l_name) {
                        if let Some(v) = table.class_value(id) {
                            let v = v.clone();
                            work_stack.push((v, rhs));
                            continue;
                        }
                        // Value-less class on LHS: under Unify mode, if rhs is
                        // also a value-less variable, extend the class with it.
                        if mode == UnifyMode::Unify {
                            if let Some(r_name) = rhs.as_atom() {
                                if is_unification_variable(r_name)
                                    && bindings.entries.get(r_name).is_none()
                                {
                                    if bindings
                                        .insert_equivalence(l_name.into(), r_name.into())
                                        .is_err()
                                    {
                                        return false;
                                    }
                                    continue;
                                }
                            }
                        }
                        // Install value into the class.
                        if occurs_in_generic(l_name, &rhs, &bindings.entries) {
                            return false;
                        }
                        match bindings.insert_value(l_name.into(), rhs.clone()) {
                            Ok(()) => continue,
                            Err(MergeConflict::Incompatible) => return false,
                            Err(MergeConflict::NeedsUnify(va, vb)) => {
                                work_stack.push((va, vb));
                                continue;
                            }
                        }
                    }
                }
                // Occurs check
                if occurs_in_generic(l_name, &rhs, &bindings.entries) {
                    return false;
                }
                // Unify mode: var-var-distinct → equivalence class.
                if mode == UnifyMode::Unify {
                    if let Some(r_name) = rhs.as_atom() {
                        if is_unification_variable(r_name)
                            && bindings.entries.get(r_name).is_none()
                        {
                            // Class membership check on RHS (might be in a class
                            // already; insert_equivalence handles all 4 cases).
                            if bindings
                                .insert_equivalence(l_name.into(), r_name.into())
                                .is_err()
                            {
                                return false;
                            }
                            continue;
                        }
                    }
                }
                // Default: ordinary bind (Match mode OR Unify mode with non-var rhs).
                match bindings.insert_value(l_name.into(), rhs) {
                    Ok(()) => continue,
                    Err(MergeConflict::Incompatible) => return false,
                    Err(MergeConflict::NeedsUnify(va, vb)) => {
                        work_stack.push((va, vb));
                        continue;
                    }
                }
            }
        }

        // Step 5: Variable on RHS (mirror)
        if let Some(r_name) = rhs.as_atom() {
            if is_unification_variable(r_name) {
                if let Some(existing) = bindings.entries.get(r_name) {
                    let existing = existing.clone();
                    work_stack.push((existing, lhs));
                    continue;
                }
                if let Some(table) = bindings.classes.as_ref() {
                    if let Some(id) = table.class_of(r_name) {
                        if let Some(v) = table.class_value(id) {
                            let v = v.clone();
                            work_stack.push((v, lhs));
                            continue;
                        }
                        // Value-less class on RHS — LHS is already non-var
                        // (we'd have caught LHS-var case above), so install
                        // value into the class.
                        if occurs_in_generic(r_name, &lhs, &bindings.entries) {
                            return false;
                        }
                        match bindings.insert_value(r_name.into(), lhs.clone()) {
                            Ok(()) => continue,
                            Err(MergeConflict::Incompatible) => return false,
                            Err(MergeConflict::NeedsUnify(va, vb)) => {
                                work_stack.push((va, vb));
                                continue;
                            }
                        }
                    }
                }
                if occurs_in_generic(r_name, &lhs, &bindings.entries) {
                    return false;
                }
                // Default: ordinary bind. (Var-var-distinct cases were
                // handled in Step 4 when both sides were vars; reaching
                // here means LHS is non-variable.)
                match bindings.insert_value(r_name.into(), lhs) {
                    Ok(()) => continue,
                    Err(MergeConflict::Incompatible) => return false,
                    Err(MergeConflict::NeedsUnify(va, vb)) => {
                        work_stack.push((va, vb));
                        continue;
                    }
                }
            }
        }

        // Step 6: both non-variable — structural comparison (same logic as
        // bidirectional_unify_generic_impl).

        if let Some(l_name) = lhs.as_atom() {
            if let Some(r_name) = rhs.as_atom() {
                if l_name == r_name {
                    continue;
                }
            }
            if l_name == "Empty" && rhs.is_empty() {
                continue;
            }
            return false;
        }
        if let Some(r_name) = rhs.as_atom() {
            if r_name == "Empty" && lhs.is_empty() {
                continue;
            }
            return false;
        }

        if let Some(l_bool) = lhs.as_bool() {
            if let Some(r_bool) = rhs.as_bool() {
                if l_bool == r_bool {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_long) = lhs.as_long() {
            if let Some(r_long) = rhs.as_long() {
                if l_long == r_long {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_float) = lhs.as_float() {
            if let Some(r_float) = rhs.as_float() {
                if l_float.to_bits() == r_float.to_bits() {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_str) = lhs.as_string() {
            if let Some(r_str) = rhs.as_string() {
                if l_str == r_str {
                    continue;
                }
            }
            return false;
        }

        if lhs.is_unit() {
            if rhs.is_unit() {
                continue;
            }
            if let Some(r_items) = rhs.as_sexpr() {
                if r_items.is_empty() {
                    continue;
                }
            }
            return false;
        }
        if rhs.is_unit() {
            if let Some(l_items) = lhs.as_sexpr() {
                if l_items.is_empty() {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_items) = lhs.as_sexpr() {
            if let Some(r_items) = rhs.as_sexpr() {
                if l_items.len() != r_items.len() {
                    return false;
                }
                if l_items.is_empty() {
                    continue;
                }
                for (l, r) in l_items.iter().zip(r_items.iter()).rev() {
                    work_stack.push((l.clone(), r.clone()));
                }
                continue;
            }
            return false;
        }

        if let Some(l_goals) = lhs.as_conjunction() {
            if let Some(r_goals) = rhs.as_conjunction() {
                if l_goals.len() != r_goals.len() {
                    return false;
                }
                for (l, r) in l_goals.iter().zip(r_goals.iter()).rev() {
                    work_stack.push((l.clone(), r.clone()));
                }
                continue;
            }
            return false;
        }

        if let Some((l_offending, l_detail)) = lhs.as_error() {
            if let Some((r_offending, r_detail)) = rhs.as_error() {
                work_stack.push((l_detail.clone(), r_detail.clone()));
                work_stack.push((l_offending.clone(), r_offending.clone()));
                continue;
            }
            return false;
        }

        if let Some(l_handle) = lhs.as_space() {
            if let Some(r_handle) = rhs.as_space() {
                if l_handle.id == r_handle.id {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_id) = lhs.as_state() {
            if let Some(r_id) = rhs.as_state() {
                if l_id == r_id {
                    continue;
                }
            }
            return false;
        }

        if let Some(l_inner) = lhs.as_type() {
            if let Some(r_inner) = rhs.as_type() {
                work_stack.push((l_inner.clone(), r_inner.clone()));
                continue;
            }
            return false;
        }

        if lhs.is_empty() && rhs.is_empty() {
            continue;
        }

        return false;
    }

    true
}

/// Dereference a value through existing bindings transitively (owned version).
///
/// Follows binding chains until reaching a non-variable or unbound variable.
/// Returns an owned value (Clone is zero-cost for Copy types like MettaValue).
fn deref_value_owned<V: MettaValueTrait + Clone>(val: &V, bindings: &GenericBindings<V>) -> V {
    let mut current = val.clone();
    // Limit chain length to prevent infinite loops from buggy bindings
    for _ in 0..64 {
        if let Some(name) = current.as_atom() {
            if is_unification_variable(name) {
                if let Some(bound) = bindings.get(name) {
                    current = bound.clone();
                    continue;
                }
            }
        }
        break;
    }
    current
}

/// atom-subst: Variable substitution (generic version)
/// Usage: (atom-subst value $var template)
pub fn eval_atom_subst_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() < 4 {
        let arity_msg = format!(
            "atom-subst requires 3 arguments, got {}. Usage: (atom-subst value $var template)",
            items.len() - 1
        );
        return vec![factory.error(factory.sexpr(items.to_vec()), factory.string(&arity_msg))];
    }

    let value = &items[1];
    let var = &items[2];
    let template = &items[3];

    // Use pattern matching to bind value to var
    if let Some(bindings) = pattern_match_simple_generic(var, value) {
        let instantiated = apply_bindings_generic(template, &bindings, factory);
        vec![instantiated]
    } else {
        // Pattern didn't match - return empty
        vec![]
    }
}

// ============================================================================
// Per-branch binding propagation helpers (Stage 1c+)
// ============================================================================
//
// These helpers support MeTTa-HE-faithful per-branch binding propagation
// through the trampoline evaluator. They operate on `GenericBindings<V>`
// and are used at rule-match composition sites and at `collapse-bind` scope
// boundaries. Bindings here encode the variable-substitution context of
// a specific nondeterministic branch — each `BoundValue`'s `.1` field.

/// Detect whether a `MettaValueTrait` value is a variable atom (starts with
/// `$`). Variables that appear as VALUES in a binding indicate a chain —
/// e.g., `{$a: $who}` means $a is aliased to $who (set up by bidirectional
/// unification when a rule variable met a template variable).
#[inline]
pub fn is_variable_value<V: MettaValueTrait>(val: &V) -> bool {
    val.as_atom()
        .map_or(false, |s| s.starts_with('$') && s != "$_")
}

/// Unification-style composition of an outer binding set with an inner one.
///
/// When a rule-match produces `outer = {$a_ruleX: $template_var}` (the rule
/// variable is bound to the caller's template variable via bidirectional
/// unification) and a sub-evaluation of the RHS produces `inner = {$a_ruleX:
/// ground_value}` (the rule variable gets concretized deep inside the RHS),
/// the chain `$template_var ↔ $a_ruleX ↔ ground_value` must be resolved so
/// the final binding set contains `$template_var → ground_value`.
///
/// Algorithm (per shared key `k`):
/// - If only `outer` has `k`: result[k] = outer[k]
/// - If only `inner` has `k`: result[k] = inner[k]
/// - If both have `k`:
///   - If outer[k] is a variable atom `$X`: `$X` is an alias for the value
///     inner binds to `k`, so add `$X → inner[k]` AND keep `k → inner[k]`.
///     (Symmetric case for `inner[k]` being a variable alias.)
///   - If both are equal: use that value.
///   - Otherwise: ground-value conflict — this branch is inconsistent.
///     We emit an `Empty` bindings set (the caller treats this as the
///     branch's bindings being degenerate; the branch's RESULT is still
///     retained, but with no per-branch bindings recorded). This matches
///     the pragmatic "preserve the fact that a result was produced"
///     semantics since MeTTaTron's rule evaluation already performs the
///     actual substitution up-front via apply_bindings.
///
/// After composition, call `apply_chain_generic` to transitively resolve
/// any newly-introduced chain entries.
/// Locate the scope at which an alias target name `x_name` already has a
/// binding in either `outer` or `inner`. Returns `ROOT_SCOPE` if neither has
/// one (caller-side default — a fresh user-typed query variable).
///
/// Used by `compose_outer_inner_generic`'s alias-resolution arm. Without
/// this lookup, an alias entry like `(dispatch_scope, $rule_var) → $caller`
/// would emit `(dispatch_scope, $caller) → val` — putting the caller-side
/// variable's binding under a rule-internal scope where downstream
/// `apply_bindings` chain `[ROOT_SCOPE, ...]` can't find it. With the
/// lookup, the alias correctly emits `(ROOT_SCOPE, $caller) → val`.
#[inline]
fn find_alias_target_scope<V: MettaValueTrait + Clone>(
    outer: &GenericBindings<V>,
    inner: &GenericBindings<V>,
    x_name: &str,
) -> crate::backend::models::generic_bindings::ScopeId {
    use crate::backend::models::generic_bindings::ROOT_SCOPE;
    outer
        .iter_full()
        .find(|(_, n, _)| *n == x_name)
        .map(|(s, _, _)| s)
        .or_else(|| {
            inner
                .iter_full()
                .find(|(_, n, _)| *n == x_name)
                .map(|(s, _, _)| s)
        })
        .unwrap_or(ROOT_SCOPE)
}

pub fn compose_outer_inner_generic<V, F>(
    outer: &GenericBindings<V>,
    inner: &GenericBindings<V>,
    factory: &F,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if outer.is_empty() {
        return inner.clone();
    }
    if inner.is_empty() {
        return outer.clone();
    }
    let _ = factory;
    let mut result = GenericBindings::new();
    // Phase 2 (Bug 2): track inner entries consumed by cross-scope
    // unification so the second pass doesn't re-emit them at their
    // original (scope, name). Pre-allocate to inner.len() — bounded
    // upper limit on the number of consumable entries.
    let mut consumed_inner: smallvec::SmallVec<
        [(
            crate::backend::models::generic_bindings::ScopeId,
            crate::backend::models::BindingName,
        ); 8],
    > = smallvec::SmallVec::with_capacity(inner.len());
    // First pass: for full ScopedKeys (scope, name) present in both maps,
    // unify their values. Same-name entries at *different* scopes were
    // previously independent — but rule-side aliases like
    // `(dispatch_scope, $rule_var) → $caller_var` co-existing with a
    // caller-side `(ROOT_SCOPE, $caller_var) → val` left the alias
    // unresolved, breaking subsequent template instantiation.  The
    // cross-scope probe below merges these conservatively.
    for (scope, name, outer_val) in outer.iter_full() {
        if let Some(inner_val) = inner.get_scoped(scope, name) {
            // Both bind `(scope, name)`. Unify.
            if outer_val == inner_val {
                result.insert_or_replace_scoped(scope, name, outer_val.clone());
            } else if is_variable_value(outer_val) {
                // outer: (scope, name) → $X, inner: (scope, name) → inner_val.
                // The alias target $X may live at a *different* scope than
                // the binding being composed: e.g. a rule-side binding
                // `(dispatch_scope, $rule_var) → $caller_var` aliases to the
                // caller-side `$caller_var` at ROOT_SCOPE. The compose must
                // emit `(target_scope, $X) → inner_val`, where target_scope
                // is whichever scope already holds an entry for `$X` in
                // either side, falling back to ROOT_SCOPE (caller default).
                if let Some(x_name) = outer_val.as_atom() {
                    let target_scope = find_alias_target_scope(outer, inner, x_name);
                    if let Some(existing) = result.get_scoped(target_scope, x_name) {
                        if existing != inner_val {
                            // Conflict on the alias — inconsistent branch,
                            // drop to empty.
                            return GenericBindings::new();
                        }
                    } else {
                        result.insert_or_replace_scoped(target_scope, x_name, inner_val.clone());
                    }
                }
                result.insert_or_replace_scoped(scope, name, inner_val.clone());
            } else if is_variable_value(inner_val) {
                // Symmetric: inner: (scope, name) → $Y, outer: (scope, name) → outer_val.
                if let Some(y_name) = inner_val.as_atom() {
                    let target_scope = find_alias_target_scope(outer, inner, y_name);
                    if let Some(existing) = result.get_scoped(target_scope, y_name) {
                        if existing != outer_val {
                            return GenericBindings::new();
                        }
                    } else {
                        result.insert_or_replace_scoped(target_scope, y_name, outer_val.clone());
                    }
                }
                result.insert_or_replace_scoped(scope, name, outer_val.clone());
            } else {
                // Phase 2B (revised): distinguish two sub-cases:
                //
                // A. "Rewrite-stage" variants: outer holds the POST-eval
                //    form (more reduced / more bound), inner holds the
                //    PRE-eval syntactic form (retaining free variables).
                //    OR vice versa. These are not inconsistent — they
                //    are the same term at different reduction points in
                //    the evaluation pipeline. Prefer the more-resolved
                //    side (fewer free variables).
                //
                // B. Genuinely inconsistent: both sides are fully ground
                //    (no free variables) but name DIFFERENT concrete
                //    values. HE's `Bindings::merge` rejects this → branch
                //    dies. We return empty.
                //
                // The `outer.has_variables_fast()` test is a cheap
                // structural proxy for "more/less resolved". Two terms
                // that both still have variables are indeterminate and
                // get treated as inconsistent (conservatively empty).
                let outer_has_vars = outer_val.has_variables_fast();
                let inner_has_vars = inner_val.has_variables_fast();
                match (outer_has_vars, inner_has_vars) {
                    (true, false) => {
                        // Inner is more resolved — prefer inner.
                        result.insert_or_replace_scoped(scope, name, inner_val.clone());
                    }
                    (false, true) => {
                        // Outer is more resolved — prefer outer.
                        result.insert_or_replace_scoped(scope, name, outer_val.clone());
                    }
                    (false, false) => {
                        // Both ground and unequal — genuinely
                        // inconsistent branch. HE-faithful: return empty.
                        #[cfg(feature = "trace")]
                        {
                            crate::backend::trace::with_trace_collector_ref(|tc| {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    0,
                                    trace_format::TraceValue::Unit,
                                    vec![],
                                    None,
                                    trace_format::TraceEventKind::BindingsDropped {
                                        cont_kind: "compose_outer_inner_generic"
                                            .to_string(),
                                        flow_id: 0,
                                        site: format!(
                                            "bindings.rs:compose-conflict-ground-ground name={} outer={:?} inner={:?}",
                                            name,
                                            crate::backend::trace::convert::trace_value_generic(outer_val),
                                            crate::backend::trace::convert::trace_value_generic(inner_val),
                                        ),
                                        dropped_keys: vec![name.to_string()],
                                        sample: vec![(
                                            name.to_string(),
                                            crate::backend::trace::convert::trace_value_generic(outer_val),
                                        )],
                                    },
                                );
                            });
                        }
                        return GenericBindings::new();
                    }
                    (true, true) => {
                        // Both still have free variables — unresolvable
                        // without more information. Conservatively treat
                        // as inconsistent.
                        #[cfg(feature = "trace")]
                        {
                            crate::backend::trace::with_trace_collector_ref(|tc| {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    0,
                                    trace_format::TraceValue::Unit,
                                    vec![],
                                    None,
                                    trace_format::TraceEventKind::BindingsDropped {
                                        cont_kind: "compose_outer_inner_generic".to_string(),
                                        flow_id: 0,
                                        site: format!(
                                            "bindings.rs:compose-conflict-nonground name={}",
                                            name,
                                        ),
                                        dropped_keys: vec![name.to_string()],
                                        sample: vec![(
                                            name.to_string(),
                                            crate::backend::trace::convert::trace_value_generic(
                                                outer_val,
                                            ),
                                        )],
                                    },
                                );
                            });
                        }
                        return GenericBindings::new();
                    }
                }
            }
        } else {
            // Phase 2 (Bug 2): exact (scope, name) miss; probe across scopes
            // for the SAME name in inner. If found, treat conservatively:
            //   - One side is a variable atom (alias case): route through
            //     the alias arm via find_alias_target_scope (preserves
            //     bisimilarity with HE's coherent-theta requirement).
            //   - Both ground and equal: emit at min-priority scope
            //     (ROOT_SCOPE wins; else outer_scope).
            //   - Both ground and unequal: SIBLING-BRANCH INDEPENDENCE,
            //     not conflict — emit outer at outer_scope, leave inner
            //     for the second pass to emit at its original scope.
            //     (Sibling branches at different dispatch_scopes have
            //     independent bindings; merging unequal grounds across
            //     scopes would break `within_query_cache_isolation_contract`.)
            //   - One or both have free variables: independent (sibling).
            let mut cross_match: Option<(crate::backend::models::generic_bindings::ScopeId, &V)> =
                None;
            for (s, n, v) in inner.iter_full() {
                if n == name && s != scope {
                    cross_match = Some((s, v));
                    break;
                }
            }
            match cross_match {
                Some((inner_scope, inner_val)) => {
                    let outer_is_var = is_variable_value(outer_val);
                    let inner_is_var = is_variable_value(inner_val);
                    if outer_is_var || inner_is_var {
                        // Alias arm: route same as same-scope alias case.
                        if outer_is_var {
                            if let Some(x_name) = outer_val.as_atom() {
                                let target_scope = find_alias_target_scope(outer, inner, x_name);
                                match result.get_scoped(target_scope, x_name) {
                                    Some(existing) if existing != inner_val => {
                                        return GenericBindings::new();
                                    }
                                    Some(_) => {}
                                    None => {
                                        result.insert_or_replace_scoped(
                                            target_scope,
                                            x_name,
                                            inner_val.clone(),
                                        );
                                    }
                                }
                            }
                            result.insert_or_replace_scoped(scope, name, inner_val.clone());
                        } else {
                            // inner_is_var
                            if let Some(y_name) = inner_val.as_atom() {
                                let target_scope = find_alias_target_scope(outer, inner, y_name);
                                match result.get_scoped(target_scope, y_name) {
                                    Some(existing) if existing != outer_val => {
                                        return GenericBindings::new();
                                    }
                                    Some(_) => {}
                                    None => {
                                        result.insert_or_replace_scoped(
                                            target_scope,
                                            y_name,
                                            outer_val.clone(),
                                        );
                                    }
                                }
                            }
                            result.insert_or_replace_scoped(scope, name, outer_val.clone());
                        }
                        consumed_inner
                            .push((inner_scope, crate::backend::models::BindingName::from(name)));
                    } else if outer_val == inner_val {
                        // Both ground and equal: emit single entry at
                        // ROOT_SCOPE if either is ROOT_SCOPE, else outer_scope.
                        let target_scope = if scope
                            == crate::backend::models::generic_bindings::ROOT_SCOPE
                            || inner_scope == crate::backend::models::generic_bindings::ROOT_SCOPE
                        {
                            crate::backend::models::generic_bindings::ROOT_SCOPE
                        } else {
                            scope
                        };
                        result.insert_or_replace_scoped(target_scope, name, outer_val.clone());
                        consumed_inner
                            .push((inner_scope, crate::backend::models::BindingName::from(name)));
                    } else {
                        // Sibling-branch independence: emit outer here;
                        // second pass emits inner at its scope.
                        result.insert_or_replace_scoped(scope, name, outer_val.clone());
                    }
                }
                None => {
                    // Genuinely outer-only — emit at (scope, name).
                    result.insert_or_replace_scoped(scope, name, outer_val.clone());
                }
            }
        }
    }
    // Second pass: add inner-only (scope, name) entries, skipping any
    // consumed by cross-scope unification in the first pass.
    for (scope, name, inner_val) in inner.iter_full() {
        if consumed_inner
            .iter()
            .any(|(s, n)| *s == scope && n.matches(name))
        {
            continue;
        }
        if result.get_scoped(scope, name).is_none() {
            result.insert_or_replace_scoped(scope, name, inner_val.clone());
        }
    }
    result
}

/// Strict variant of `compose_outer_inner_generic`: returns `None` on
/// binding conflict, distinguishing "conflict → drop branch" from
/// "empty inputs → empty output".
///
/// `compose_outer_inner_generic` returns empty `GenericBindings` both when
/// all inputs are empty (a normal flow) and when two non-empty inputs
/// have an unresolvable conflict (a branch-dies signal). Callers that need
/// HE-bisimilar silent pruning (drop the combination on conflict) can't
/// distinguish these cases. This wrapper does:
///
///   - `Some(inner.clone())` when `outer` is empty.
///   - `Some(outer.clone())` when `inner` is empty.
///   - `None` when both are non-empty AND the compose result is empty
///     (genuine conflict — inconsistent branch).
///   - `Some(result)` otherwise.
///
/// HE semantics: use `None` as the trigger to drop the combination (skip
/// the alternative, `continue`, emit zero results, etc.), matching
/// `BindingsSet::empty()` filtering in hyperon-experimental.
pub fn compose_outer_inner_strict_generic<V, F>(
    outer: &GenericBindings<V>,
    inner: &GenericBindings<V>,
    factory: &F,
) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if outer.is_empty() {
        return Some(inner.clone());
    }
    if inner.is_empty() {
        return Some(outer.clone());
    }
    let result = compose_outer_inner_generic(outer, inner, factory);
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Transitive chain resolution: for every (v, val) in the bindings, rewrite
/// `val` via `apply_bindings(val, self)` until a fixed point.
///
/// Handles chains like `{$a: $who, $who: a}` → `{$a: a, $who: a}`.
/// Uses iteration with a max-depth guard (16) to handle pathological cycles
/// without infinite looping. `apply_bindings_generic` itself handles
/// transitive substitution, so typically one pass suffices; the loop is a
/// safeguard for value-level rewrites that don't settle on the first pass.
pub fn apply_chain_generic<V, F>(bindings: &mut GenericBindings<V>, factory: &F)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if bindings.is_empty() || bindings.len() == 1 {
        // Single entry can't have a chain within this binding set.
        return;
    }
    const MAX_PASSES: usize = 16;
    // P2 follow-up: snapshot full (scope, name) keys — same name at
    // different scopes are independent bindings, so collapsing them via
    // bare-name `iter()` would corrupt rule-side / caller-side entries
    // that happen to share `$who`. Each entry's value is chain-resolved
    // STRICTLY at its own scope (with ROOT_SCOPE fallback) so a rule-side
    // binding's value chains to other rule-side / caller-side bindings,
    // not to unrelated same-name entries at other dispatch scopes.
    let snapshot_keys: Vec<(
        crate::backend::models::generic_bindings::ScopeId,
        crate::backend::models::BindingName,
    )> = bindings
        .iter_full()
        .map(|(s, n, _)| (s, crate::backend::models::BindingName::from(n)))
        .collect();
    let mut pass = 0;
    loop {
        let mut changed = false;
        for (scope, name) in &snapshot_keys {
            let val = match bindings.get_scoped(*scope, name.as_str()) {
                Some(v) => v.clone(),
                None => continue,
            };
            let resolved = apply_bindings_scoped_generic(
                &val,
                bindings,
                &[*scope, crate::backend::models::generic_bindings::ROOT_SCOPE],
                factory,
            );
            if !val.identity_eq(&resolved) && val != resolved {
                bindings.insert_or_replace_scoped(*scope, name.clone(), resolved);
                changed = true;
            }
        }
        pass += 1;
        if !changed || pass >= MAX_PASSES {
            break;
        }
    }
}

/// Project a binding set to just the tracked-var keys. For each tracked
/// var, look it up in `self` and copy the resolved value. Tracked vars
/// that don't appear in `self` are omitted.
///
/// Assumes `apply_chain_generic` has already been called on `bindings`,
/// so values in `bindings` are already transitively resolved. This is
/// the projection step used at `ProcessCollapseBind` output to emit only
/// the variables the caller cares about in the `(Bindings …)` sidecar.
pub fn project_bindings_generic<V: MettaValueTrait + Clone>(
    bindings: &GenericBindings<V>,
    keep: &[&'static str],
) -> GenericBindings<V> {
    if bindings.is_empty() || keep.is_empty() {
        return GenericBindings::new();
    }
    let mut result = GenericBindings::new();
    for &v in keep {
        if let Some(val) = bindings.get(v) {
            result.insert_or_replace(v, val.clone());
        }
    }
    result
}

/// Project bindings to the variables that can still be observed by the next
/// consumer expression or by an active `collapse-bind` capture frame.
///
/// `GenericBindings` is used both as a lexical substitution environment and
/// as per-branch provenance. This helper is the scope/export barrier between
/// those roles: freshened internal variables (`$__fr_*`) are retained only
/// while the next consumer still mentions them, or while they are needed to
/// resolve a retained caller-visible binding.
///
/// Returns `None` when a retained caller-visible binding would still contain
/// a freshened variable that is not retained by the projection.
pub fn project_bindings_for_consumer_generic<V, F>(
    bindings: &GenericBindings<V>,
    consumers: &[&V],
    tracked_vars: Option<&[&'static str]>,
    factory: &F,
) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if bindings.is_empty() {
        return Some(GenericBindings::new());
    }

    let mut resolved = bindings.clone();
    apply_chain_generic(&mut resolved, factory);

    let mut live: HashSet<String> = HashSet::new();
    for consumer in consumers {
        live.extend(collect_variables_generic(*consumer));
    }
    if let Some(vars) = tracked_vars {
        live.extend(vars.iter().map(|v| v.to_string()));
    }

    if live.is_empty() {
        return Some(GenericBindings::new());
    }

    loop {
        let before = live.len();
        for (_scope, name, val) in resolved.iter_full() {
            if live.contains(name) && val.has_variables_fast() {
                live.extend(collect_variables_generic(val));
            }
        }
        if live.len() == before {
            break;
        }
    }

    let mut projected = GenericBindings::new();
    for (scope, name, val) in resolved.iter_full() {
        if live.contains(name) {
            projected.insert_scoped(
                scope,
                crate::backend::models::BindingName::from(name),
                val.clone(),
            );
        }
    }

    for (_scope, name, val) in projected.iter_full() {
        let caller_visible = !name.starts_with("$__fr_")
            || tracked_vars
                .map(|vars| vars.iter().any(|tracked| *tracked == name))
                .unwrap_or(false);
        if caller_visible && val.has_variables_fast() {
            for var in collect_variables_generic(val) {
                if var.starts_with("$__fr_")
                    && !projected
                        .iter_full()
                        .any(|(_, projected_name, _)| projected_name == var)
                {
                    return None;
                }
            }
        }
    }

    Some(projected)
}

/// Export the caller-visible portion of a rule-match scratch binding map.
///
/// Rule dispatch builds an internal binding map that contains both rule-local
/// keys (`$__fr_<epoch>_*` in a per-dispatch scope) and caller/query keys
/// (`ROOT_SCOPE`). That scratch map is valid for instantiating the RHS, but it
/// must not escape as branch provenance: doing so leaks recursive rule-frame
/// variables into later evaluation and trace/root walks.
///
/// This helper projects only variables that actually appear in `query`. Values
/// are resolved through `scratch` before export. If a projected caller value
/// still references the current rule's freshened variable prefix, the match is
/// partial and is rejected by returning `None`.
pub fn export_query_bindings_generic<V, F>(
    scratch: &GenericBindings<V>,
    query: &V,
    current_rule_prefix: &str,
    scope_chain: &[crate::backend::models::generic_bindings::ScopeId],
    factory: &F,
) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if scratch.is_empty() {
        return Some(GenericBindings::new());
    }

    let query_vars = collect_variables_generic(query);
    if query_vars.is_empty() {
        return Some(GenericBindings::new());
    }

    let mut exported = GenericBindings::new();
    for var in query_vars {
        if var.starts_with(current_rule_prefix) {
            continue;
        }
        let Some(value) = scratch.get_chain(scope_chain, var.as_str()) else {
            continue;
        };
        let resolved = apply_bindings_scoped_generic(value, scratch, scope_chain, factory);
        if value_contains_var_with_prefix(&resolved, current_rule_prefix) {
            return None;
        }
        exported.insert_or_replace(var, resolved);
    }
    Some(exported)
}

/// Strip freshened-var bindings from a binding set.
///
/// Keys starting with `$__fr_` are per-invocation rule-match artifacts produced
/// by [`freshen_variables_generic`](crate::backend::eval::freshening). They
/// are valid within a single rule dispatch but MUST NOT cross user-level
/// iteration boundaries (e.g., between `let*` pairs) — otherwise a recursive
/// rule invocation at epoch N and a sibling invocation at epoch M can both
/// end up with `$__fr_N_tail` alive in the outer scope when they represent
/// independent recursion frames, producing spurious ground-ground conflicts
/// at `compose_outer_inner_*_generic`.
///
/// Filtering at the `let*`-pair handoff restores the observational
/// bisimilarity MeTTaTron targets with MeTTa HE, which achieves the same
/// scope isolation naturally: HE doesn't freshen per-invocation — it gives
/// each rule query a fresh `Bindings` object merged into the caller's
/// context, so cross-frame variable-name collisions cannot occur. Our
/// per-invocation freshening is a performance optimization; filtering at
/// boundaries is the complementary scope barrier.
///
/// Returns a new `GenericBindings` containing only user-level keys (keys
/// that do NOT start with `$__fr_`).
///
/// Chain resolution must be performed BEFORE calling this: if a user-level
/// binding `$user_x → $__fr_M_alias` points at a freshened key that this
/// filter is about to drop, the user-level binding becomes a dangling
/// reference. Call [`apply_chain_generic`] first to materialize the values.
pub fn strip_freshened_bindings<V: MettaValueTrait + Clone>(
    bindings: &GenericBindings<V>,
) -> GenericBindings<V> {
    let mut result = GenericBindings::new();
    for (name, val) in bindings.iter() {
        if !name.starts_with("$__fr_") {
            result.insert_or_replace(name, val.clone());
        }
    }
    result
}

/// Re-tag entries whose name appears in `rule_var_names` from `from_scope`
/// to `to_scope`. Caller-side keys (names absent from `rule_var_names`) pass
/// through verbatim.
///
/// **Rules are stored with original variable names** (see comment at
/// `rule_management.rs:2071-2076`); per-match freshening produces atoms
/// like `$__fr_{epoch}_*` only inside the substituted RHS, never as
/// matcher binding keys. The matchers (StructuralMatcher, EnhancedMatcher)
/// emit bindings keyed on the rule's *original* LHS variable names — the
/// same names that populate `entry.var_names`. Caller-side keys
/// (introduced by `bidirectional_unify_generic` fallback at
/// `EnhancedMatcher::try_match_with_bindings:398-407`) are NOT in
/// `entry.var_names` and stay at their original scope.
///
/// `apply_bindings_scoped_generic` then walks the rule's RHS template
/// with chain `[dispatch_scope, ROOT_SCOPE]`: rule-LHS atoms hit
/// `dispatch_scope`; caller-level atoms embedded into the RHS by
/// bidirectional unify miss at `dispatch_scope` and fall back to
/// `ROOT_SCOPE`.
///
/// Mirrors `freshening::freshen_bindings_keys_with_epoch` semantically but
/// uses scope tags instead of name rewrites — preserving HE bisimilarity
/// per `metta-specification` §19.2 and §6.3.1.
pub fn retag_rule_keys_at_scope<V: MettaValueTrait + Clone>(
    bindings: GenericBindings<V>,
    rule_var_names: &[&'static str],
    from_scope: crate::backend::models::generic_bindings::ScopeId,
    to_scope: crate::backend::models::generic_bindings::ScopeId,
) -> GenericBindings<V> {
    if from_scope == to_scope || bindings.is_empty() || rule_var_names.is_empty() {
        return bindings;
    }
    let mut result = GenericBindings::new();
    for (scope, name, val) in bindings.iter_full() {
        let target_scope = if scope == from_scope && rule_var_names.contains(&name) {
            to_scope
        } else {
            scope
        };
        result.insert_scoped(target_scope, name, val.clone());
    }
    result
}

/// Substitute scope-tagged bindings into `template` AND rename body-local
/// `$x` atoms (those not in any of the provided scopes) using `body_local_epoch`.
///
/// This is the hybrid replacement for `apply_bindings_with_rename_generic`:
/// LHS-bound atoms are looked up via the scope chain (typically
/// `[dispatch_scope, ROOT_SCOPE]`), while *unbound* `$`-prefixed atoms
/// (body-local vars introduced by `let*` patterns inside the RHS, e.g.
/// `$head` and `$tail` in `BestCandidate`'s `let* (($head ...) ($tail ...))`)
/// are renamed to `$__fr_{body_local_epoch}_x` so recursive invocations
/// don't collide on the same bare name at `ROOT_SCOPE` once the `let*`
/// pattern binds them.
///
/// Set `body_local_epoch == 0` to disable renaming (un-bound atoms pass
/// through verbatim — for callers that explicitly want bare-name vars,
/// e.g. tests or post-P3 callers).
///
/// Preserves the structural-sharing identity-equality optimization from
/// `apply_bindings_iterative_generic` for ground subtrees.
pub fn apply_bindings_with_rename_scoped<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    scope_chain: &[crate::backend::models::generic_bindings::ScopeId],
    body_local_epoch: u64,
    outer_carrying: &GenericBindings<V>,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    use crate::backend::eval::freshening::CachingRename;

    // Fast path: nothing to substitute, nothing to rename, nothing to fall back to.
    if bindings.is_empty() && outer_carrying.is_empty() && body_local_epoch == 0 {
        return template.clone();
    }

    // Peel Spanned: process inner, re-wrap with same span
    if let Some(span) = template.span() {
        let span = *span;
        let stripped = template.strip_one_span();
        let result = apply_bindings_with_rename_scoped(
            &stripped,
            bindings,
            scope_chain,
            body_local_epoch,
            outer_carrying,
            factory,
        );
        if result.span().is_some() {
            return result;
        }
        return factory.spanned(result, span);
    }

    let rename = if body_local_epoch != 0 {
        Some(CachingRename::new(body_local_epoch))
    } else {
        None
    };

    apply_bindings_with_rename_scoped_iterative(
        template,
        bindings,
        scope_chain,
        rename.as_ref(),
        outer_carrying,
        factory,
    )
}

fn apply_bindings_with_rename_scoped_iterative<V, F>(
    template: &V,
    bindings: &GenericBindings<V>,
    scope_chain: &[crate::backend::models::generic_bindings::ScopeId],
    rename: Option<&crate::backend::eval::freshening::CachingRename>,
    outer_carrying: &GenericBindings<V>,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    use crate::backend::models::generic_bindings::ROOT_SCOPE;

    enum Work<'a, V> {
        ProcessTemplate(&'a V),
        ProcessOwned(V),
        BuildSExpr { count: usize, original: V },
        BuildConjunction { count: usize, original: V },
    }

    // Lookup probes rule-side first (scope-aware), then falls back to
    // caller-side ROOT_SCOPE. Caller-side fallback resolves caller variables
    // that appear inside a captured value (e.g. `$C`'s value `(uncle $a $b)`
    // where `$a, $b` are caller-side vars). Without it, the body-local rename
    // freshens these to `$__fr_E_a/b`, producing a wildcard-LHS rule when
    // materialized via `add-atom` — see Phase 1, Bug 1 of the scope-tag
    // plumbing fix plan.
    let lookup = |bindings: &GenericBindings<V>, name: &str| -> Option<V> {
        let primary = if scope_chain.is_empty() {
            bindings.get(name).cloned()
        } else {
            bindings.get_chain(scope_chain, name).cloned()
        };
        match primary {
            Some(v) => Some(v),
            None => outer_carrying.get_scoped(ROOT_SCOPE, name).cloned(),
        }
    };

    let mut work_stack: SmallVec<[Work<V>; 16]> = SmallVec::new();
    let mut result_stack: SmallVec<[V; 16]> = SmallVec::new();

    work_stack.push(Work::ProcessTemplate(template));

    while let Some(work) = work_stack.pop() {
        match work {
            Work::ProcessTemplate(val) => {
                if !val.has_variables_fast() {
                    result_stack.push(val.clone());
                    continue;
                }
                if val.is_spanned() {
                    let result = apply_bindings_with_rename_scoped(
                        val,
                        bindings,
                        scope_chain,
                        rename.map_or(0, |r| r.epoch()),
                        outer_carrying,
                        factory,
                    );
                    result_stack.push(result);
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') && name != "$_" {
                        if let Some(bound) = lookup(bindings, name) {
                            // Self-referential guard
                            if bound.as_atom() == Some(name) {
                                if let Some(r) = rename {
                                    result_stack.push(r.fresh_atom(name, factory));
                                } else {
                                    result_stack.push(bound);
                                }
                                continue;
                            }
                            // Transitive: descend into bound value (owned mode — no rename)
                            work_stack.push(Work::ProcessOwned(bound));
                        } else if let Some(r) = rename {
                            // Body-local var (not bound, not in scope chain) →
                            // rename per dispatch to avoid recursive collision.
                            result_stack.push(r.fresh_atom(name, factory));
                        } else {
                            result_stack.push(val.clone());
                        }
                    } else {
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildSExpr {
                            count: items.len(),
                            original: val.clone(),
                        });
                        for item in items.iter().rev() {
                            work_stack.push(Work::ProcessTemplate(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(Work::BuildConjunction {
                            count: goals.len(),
                            original: val.clone(),
                        });
                        for goal in goals.iter().rev() {
                            work_stack.push(Work::ProcessTemplate(goal));
                        }
                    }
                } else {
                    result_stack.push(val.clone());
                }
            }
            Work::ProcessOwned(val) => {
                if !val.has_variables_fast() {
                    result_stack.push(val);
                    continue;
                }
                if val.is_spanned() {
                    // Bug fix (2026-05-06): preserve owned-mode invariant
                    // across Spanned peeling. Pre-fix, this arm recursed
                    // into `apply_bindings_with_rename_scoped` (the
                    // top-level entry), which restarts in
                    // `Work::ProcessTemplate` mode at line 2172 — silently
                    // flipping owned (no-rename) to template
                    // (rename-on-miss). PLN's `?` macro
                    // (`Direct.metta:32-59`) failed because
                    // `(? (grandfather $who c))` matched rule
                    // `(? $term)` → `{$term → spanned((grandfather $who c))}`,
                    // and during instantiation the spanned value entered
                    // this arm, was peeled via the leaky top-level call
                    // in ProcessTemplate mode, and `$who` got renamed to
                    // `$__fr_E_who` — a name the downstream
                    // `{$who → a}` collapse-bind binding could never
                    // match.
                    //
                    // Fix: peel the Spanned wrapper LOCALLY and re-push
                    // as `Work::ProcessOwned` so the "owned, no rename"
                    // semantic survives span boundaries. HE-faithful:
                    // HE's `make_variables_unique` is only ever applied
                    // to the stored side BEFORE matching, never to
                    // caller-side substituted contents
                    // (hyperon-experimental/hyperon-space/src/index/trie.rs:262).
                    // Span loss on interior substituted values is safe
                    // (audited every `is_spanned()` consumer; outer
                    // template span survives via the parent
                    // `BuildSExpr.original`).
                    work_stack.push(Work::ProcessOwned(val.strip_one_span()));
                    continue;
                }
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') && name != "$_" {
                        if let Some(bound) = lookup(bindings, name) {
                            if bound.as_atom() == Some(name) {
                                result_stack.push(bound);
                                continue;
                            }
                            work_stack.push(Work::ProcessOwned(bound));
                        } else {
                            // Owned mode: a bound value's free var passes
                            // through verbatim — no rename here (the value
                            // came from caller scope).
                            result_stack.push(val);
                        }
                    } else {
                        result_stack.push(val);
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = items.len();
                        let owned_children: Vec<V> = items.iter().cloned().collect();
                        work_stack.push(Work::BuildSExpr {
                            count: len,
                            original: val,
                        });
                        for item in owned_children.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val);
                    } else {
                        let len = goals.len();
                        let owned_goals: Vec<V> = goals.iter().cloned().collect();
                        work_stack.push(Work::BuildConjunction {
                            count: len,
                            original: val,
                        });
                        for goal in owned_goals.into_iter().rev() {
                            work_stack.push(Work::ProcessOwned(goal));
                        }
                    }
                } else {
                    result_stack.push(val);
                }
            }
            Work::BuildSExpr { count, original } => {
                let start = result_stack.len() - count;
                let items = original
                    .as_sexpr()
                    .expect("BuildSExpr original must be sexpr");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&items[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                    continue;
                }
                let result = factory.sexpr_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
            Work::BuildConjunction { count, original } => {
                let start = result_stack.len() - count;
                let goals = original
                    .as_conjunction()
                    .expect("BuildConjunction original must be conjunction");
                let changed = (0..count).any(|i| !result_stack[start + i].identity_eq(&goals[i]));
                if !changed {
                    result_stack.truncate(start);
                    result_stack.push(original);
                    continue;
                }
                let result = factory.conjunction_from_slice(&result_stack[start..]);
                result_stack.truncate(start);
                result_stack.push(result);
            }
        }
    }

    result_stack
        .pop()
        .expect("Result stack should not be empty")
}

/// Prepare `accumulated_bindings` for composition with the current `let*`
/// pair's pattern-match result. Applies **pattern-keyed shadow** only:
/// strip any key that the current pair's `pattern` is about to bind so the
/// new pm binding "wins" while preserving strict-prune semantics for any
/// genuine conflict (rule-match bindings on variables NOT in this pattern
/// that contradict accumulated still fire the conflict trail).
///
/// MeTTa's `let*` is sequential-shadow: `(let* (($x 1) ($x 2)) $x)` returns
/// `2` in HE. Stripping the pattern's shadow keys from `accumulated` before
/// strict-compose achieves that while keeping ghost-branch pruning intact
/// for non-shadow conflicts.
///
/// **2026-04-23**: the earlier variant of this helper ALSO stripped
/// `$__fr_*` (per-invocation freshened) keys as a "scope barrier" against
/// stale iteration leaks. That was wrong — it dropped freshened bindings
/// legitimately produced by the CURRENT rule invocation (e.g., PLN's
/// `BestCandidate` let*-body referring to rule-match `$__fr_N_f`), causing
/// downstream `if` conditions to see unbound freshened vars. The strip is
/// removed; `strip_freshened_bindings` remains available for explicit use
/// elsewhere if a real scope-barrier need emerges.
pub fn prepare_letstar_accumulated<V, F>(
    accumulated: &GenericBindings<V>,
    pattern: &V,
    factory: &F,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone,
    F: crate::backend::models::MettaValueFactory<V>,
{
    let pattern_vars: std::collections::HashSet<String> = collect_variables_generic(pattern);
    let _ = factory; // factory kept for future use / symmetry with other helpers
    if pattern_vars.is_empty() {
        return accumulated.clone();
    }
    let mut result = GenericBindings::new();
    for (name, val) in accumulated.iter() {
        if !pattern_vars.contains(name) {
            result.insert_or_replace(name, val.clone());
        }
    }
    result
}

/// Encode bindings as a `(Bindings ($var val) …)` S-expression.
///
/// Generic factory variant shared across trampoline, bytecode-VM (Phase C),
/// and JIT (Phase D) tiers. The trampoline tier keeps a concrete-typed
/// wrapper (`encode_bindings_as_sexpr` in `trampoline/eval_loop.rs`) but
/// that wrapper now forwards to this function.
pub fn encode_bindings_as_sexpr_generic<V, F>(bindings: &GenericBindings<V>, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: crate::backend::models::MettaValueFactory<V>,
{
    let mut items: Vec<V> = Vec::with_capacity(bindings.len() + 1);
    items.push(factory.atom("Bindings"));
    // Y.6 (2026-05-12): collect & sort by variable name for deterministic
    // output across tiers. The underlying SmallVec insertion order depends
    // on the matcher's traversal sequence, which is not guaranteed to be
    // stable across runs (e.g., parallel sub-evals in T1). T0 happened to
    // emit alphabetical order due to source-code traversal order — sorting
    // here makes every tier match deterministically.
    let mut pairs: Vec<(&str, &V)> = bindings.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in pairs {
        items.push(factory.sexpr(vec![factory.atom(name), value.clone()]));
    }
    factory.sexpr(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_collect_variables_generic() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let vars = collect_variables_generic(&expr);
        assert!(vars.contains("$x"));
        assert!(vars.contains("$y"));
        assert!(!vars.contains("foo"));
    }

    #[test]
    fn test_export_query_bindings_resolves_query_var_without_rule_keys() {
        use crate::backend::models::generic_bindings::ROOT_SCOPE;

        let factory = GcFactory::default();
        let dispatch_scope = 42;
        let prefix = "$__fr_7_";
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom("Toothbrush".to_string()),
            MettaValue::Atom("$who".to_string()),
        ]);

        let mut scratch = GenericBindings::new();
        scratch.insert_scoped(
            ROOT_SCOPE,
            "$who",
            MettaValue::Atom("$__fr_7_owner".to_string()),
        );
        scratch.insert_scoped(
            dispatch_scope,
            "$__fr_7_owner",
            MettaValue::Atom("Alice".to_string()),
        );

        let exported = export_query_bindings_generic(
            &scratch,
            &query,
            prefix,
            &[dispatch_scope, ROOT_SCOPE],
            &factory,
        )
        .expect("resolved caller binding should export");

        assert_eq!(
            exported.get("$who").and_then(|v| v.as_atom()),
            Some("Alice")
        );
        assert!(exported.get("$__fr_7_owner").is_none());
    }

    #[test]
    fn test_export_query_bindings_rejects_current_rule_fresh_vars_in_values() {
        use crate::backend::models::generic_bindings::ROOT_SCOPE;

        let factory = GcFactory::default();
        let dispatch_scope = 43;
        let prefix = "$__fr_8_";
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom("Toothbrush".to_string()),
            MettaValue::Atom("$who".to_string()),
        ]);

        let mut scratch = GenericBindings::new();
        scratch.insert_scoped(
            ROOT_SCOPE,
            "$who",
            MettaValue::SExpr(vec![
                MettaValue::Atom("Needs".to_string()),
                MettaValue::Atom("$__fr_8_unresolved".to_string()),
            ]),
        );

        let exported = export_query_bindings_generic(
            &scratch,
            &query,
            prefix,
            &[dispatch_scope, ROOT_SCOPE],
            &factory,
        );

        assert!(exported.is_none());
    }

    #[test]
    fn test_export_query_bindings_keeps_other_frame_fresh_vars() {
        use crate::backend::models::generic_bindings::ROOT_SCOPE;

        let factory = GcFactory::default();
        let dispatch_scope = 44;
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom("Uses".to_string()),
            MettaValue::Atom("$__fr_9_outer".to_string()),
        ]);

        let mut scratch = GenericBindings::new();
        scratch.insert_scoped(
            ROOT_SCOPE,
            "$__fr_9_outer",
            MettaValue::Atom("Brush".to_string()),
        );

        let exported = export_query_bindings_generic(
            &scratch,
            &query,
            "$__fr_10_",
            &[dispatch_scope, ROOT_SCOPE],
            &factory,
        )
        .expect("fresh vars from an outer frame are caller-visible here");

        assert_eq!(
            exported.get("$__fr_9_outer").and_then(|v| v.as_atom()),
            Some("Brush")
        );
    }

    #[test]
    fn test_project_bindings_for_consumer_resolves_tracked_alias_and_drops_fresh_key() {
        let factory = GcFactory::default();
        let mut bindings = GenericBindings::new();
        bindings.insert("$__fr_a", MettaValue::Atom("$who".to_string()));
        bindings.insert("$who", MettaValue::Atom("Alice".to_string()));

        let consumer = MettaValue::Atom("done".to_string());
        let projected = project_bindings_for_consumer_generic(
            &bindings,
            &[&consumer],
            Some(&["$who"]),
            &factory,
        )
        .expect("tracked caller alias should resolve");

        assert_eq!(
            projected.get("$who").and_then(|v| v.as_atom()),
            Some("Alice")
        );
        assert!(projected.get("$__fr_a").is_none());
    }

    #[test]
    fn test_project_bindings_for_consumer_keeps_live_fresh_var_only() {
        let factory = GcFactory::default();
        let mut bindings = GenericBindings::new();
        bindings.insert(
            "$__fr_tail",
            MettaValue::SExpr(vec![
                MettaValue::Atom("Cons".to_string()),
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("Nil".to_string()),
            ]),
        );
        bindings.insert("$__fr_head", MettaValue::Atom("unused".to_string()));

        let consumer = MettaValue::SExpr(vec![
            MettaValue::Atom("next".to_string()),
            MettaValue::Atom("$__fr_tail".to_string()),
        ]);
        let projected =
            project_bindings_for_consumer_generic(&bindings, &[&consumer], None, &factory)
                .expect("live fresh consumer var should be retained");

        assert!(projected.get("$__fr_tail").is_some());
        assert!(projected.get("$__fr_head").is_none());
    }

    #[test]
    fn test_project_bindings_for_consumer_rejects_dangling_visible_fresh_ref() {
        let factory = GcFactory::default();
        let mut bindings = GenericBindings::new();
        bindings.insert("$who", MettaValue::Atom("$__fr_missing".to_string()));

        let consumer = MettaValue::Atom("done".to_string());
        let projected = project_bindings_for_consumer_generic(
            &bindings,
            &[&consumer],
            Some(&["$who"]),
            &factory,
        );

        assert!(projected.is_none());
    }

    /// BUG-T0-007 regression: `collect_variables_generic` must include
    /// `&y` and `'z` sigil variables, not just `$x`.
    #[test]
    fn test_collect_variables_generic_includes_amp_and_apos_sigils() {
        let factory = GcFactory::default();
        let expr = factory.sexpr(vec![
            factory.atom("foo"),
            factory.atom("$x"),
            factory.atom("&y"),
            factory.atom("'z"),
            factory.atom("&self"), // excluded — space reference, not a variable
        ]);
        let vars = collect_variables_generic(&expr);
        assert!(vars.contains("$x"), "$x should be collected");
        assert!(vars.contains("&y"), "&y should be collected (BUG-T0-007)");
        assert!(vars.contains("'z"), "'z should be collected (BUG-T0-007)");
        assert!(!vars.contains("&self"), "&self is a space reference, not a variable");
        assert!(!vars.contains("foo"), "foo is a literal atom");
    }

    /// BUG-T0-007 regression: `seal_variables_generic` must freshen
    /// `&y` and `'z` sigil variables, not just `$x`.
    #[test]
    fn test_seal_variables_generic_freshens_amp_and_apos_sigils() {
        let factory = GcFactory::default();
        let expr = factory.sexpr(vec![
            factory.atom("foo"),
            factory.atom("$x"),
            factory.atom("&y"),
            factory.atom("'z"),
        ]);
        let ignore = HashSet::new();
        let sealed = seal_variables_generic(&expr, &ignore, 42, &factory);

        let items = sealed.as_sexpr().expect("sealed expr is sexpr");
        assert_eq!(items[0].as_atom(), Some("foo"));
        assert_eq!(items[1].as_atom(), Some("$x_42"), "$x should be freshened");
        assert_eq!(items[2].as_atom(), Some("&y_42"), "&y should be freshened (BUG-T0-007)");
        assert_eq!(items[3].as_atom(), Some("'z_42"), "'z should be freshened (BUG-T0-007)");
    }

    #[test]
    fn test_seal_variables_generic() {
        let factory = GcFactory::default();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let mut ignore = HashSet::new();
        ignore.insert("$x".to_string());

        let sealed = seal_variables_generic(&expr, &ignore, 42, &factory);

        if let Some(items) = sealed.as_sexpr() {
            assert_eq!(items[0].as_atom(), Some("foo"));
            assert_eq!(items[1].as_atom(), Some("$x")); // preserved
            assert_eq!(items[2].as_atom(), Some("$y_42")); // sealed
        } else {
            panic!("Expected sexpr");
        }
    }

    #[test]
    fn test_eval_sealed_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("sealed".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("$x".to_string())]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ];
        let result = eval_sealed_generic(&items, &factory);
        assert_eq!(result.len(), 1);

        if let Some(items) = result[0].as_sexpr() {
            assert_eq!(items[0].as_atom(), Some("foo"));
            assert_eq!(items[1].as_atom(), Some("$x")); // preserved
                                                        // $y should be sealed with some unique ID
            assert!(items[2]
                .as_atom()
                .map(|s| s.starts_with("$y_"))
                .unwrap_or(false));
        }
    }

    #[test]
    fn test_eval_atom_subst_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("atom-subst".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        ];
        let result = eval_atom_subst_generic(&items, &factory);
        assert_eq!(result.len(), 1);

        if let Some(items) = result[0].as_sexpr() {
            assert_eq!(items[0].as_atom(), Some("+"));
            assert_eq!(items[1].as_long(), Some(42));
            assert_eq!(items[2].as_long(), Some(1));
        }
    }

    // =========================================================================
    // Bidirectional Unification (Martelli-Montanari) Tests
    // =========================================================================

    /// Helper: create an S-expression from a slice of MettaValues
    fn sexpr(items: Vec<MettaValue>) -> MettaValue {
        MettaValue::SExpr(items)
    }

    #[test]
    fn test_unify_identical_atoms() {
        let a = MettaValue::sym("foo");
        let b = MettaValue::sym("foo");
        let result = bidirectional_unify_generic(&a, &b);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_different_atoms() {
        let a = MettaValue::sym("foo");
        let b = MettaValue::sym("bar");
        let result = bidirectional_unify_generic(&a, &b);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_variable_lhs() {
        let var = MettaValue::var("x");
        let val = MettaValue::sym("hello");
        let result = bidirectional_unify_generic(&var, &val);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("hello"));
    }

    #[test]
    fn test_unify_variable_rhs() {
        let val = MettaValue::sym("hello");
        let var = MettaValue::var("x");
        let result = bidirectional_unify_generic(&val, &var);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("hello"));
    }

    #[test]
    fn test_unify_bidirectional() {
        // (a $x) unified with ($y b) => {$x -> b, $y -> a}
        let lhs = sexpr(vec![MettaValue::sym("a"), MettaValue::var("x")]);
        let rhs = sexpr(vec![MettaValue::var("y"), MettaValue::sym("b")]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("b"));
        assert_eq!(bindings.get("$y").unwrap().as_atom(), Some("a"));
    }

    #[test]
    fn test_unify_variable_consistency_success() {
        // ($x $x) unified with (a a) => Some({$x -> a})
        let pattern = sexpr(vec![MettaValue::var("x"), MettaValue::var("x")]);
        let value = sexpr(vec![MettaValue::sym("a"), MettaValue::sym("a")]);
        let result = bidirectional_unify_generic(&pattern, &value);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("a"));
    }

    #[test]
    fn test_unify_variable_consistency_failure() {
        // ($x $x) unified with (a b) => None
        let pattern = sexpr(vec![MettaValue::var("x"), MettaValue::var("x")]);
        let value = sexpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
        let result = bidirectional_unify_generic(&pattern, &value);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_occurs_check_simple() {
        // $x unified with (f $x) => None (infinite term)
        let var = MettaValue::var("x");
        let term = sexpr(vec![MettaValue::sym("f"), MettaValue::var("x")]);
        let result = bidirectional_unify_generic(&var, &term);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_occurs_check_nested() {
        // $x unified with (f (g $x)) => None
        let var = MettaValue::var("x");
        let term = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("x")]),
        ]);
        let result = bidirectional_unify_generic(&var, &term);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_occurs_check_success() {
        // $x unified with (f $y) => Some({$x -> (f $y)})
        let var = MettaValue::var("x");
        let term = sexpr(vec![MettaValue::sym("f"), MettaValue::var("y")]);
        let result = bidirectional_unify_generic(&var, &term);
        assert!(result.is_some());
    }

    #[test]
    fn test_unify_all_variable_prefixes() {
        // (&x 'y $z) vs (a b c) => Some({&x->a, 'y->b, $z->c})
        let factory = GcFactory::default();
        let lhs = sexpr(vec![
            factory.atom("&x"),
            factory.atom("'y"),
            MettaValue::var("z"),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("a"),
            MettaValue::sym("b"),
            MettaValue::sym("c"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("&x").unwrap().as_atom(), Some("a"));
        assert_eq!(bindings.get("'y").unwrap().as_atom(), Some("b"));
        assert_eq!(bindings.get("$z").unwrap().as_atom(), Some("c"));
    }

    #[test]
    fn test_unify_standalone_ampersand_not_variable() {
        // (& $x) vs (& 5) => Some({$x -> 5})
        let factory = GcFactory::default();
        let lhs = sexpr(vec![factory.atom("&"), MettaValue::var("x")]);
        let rhs = sexpr(vec![factory.atom("&"), MettaValue::Long(5)]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_long(), Some(5));
    }

    #[test]
    fn test_unify_wildcard_lhs() {
        let wildcard = MettaValue::sym("_");
        let val = MettaValue::Long(42);
        let result = bidirectional_unify_generic(&wildcard, &val);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_wildcard_rhs() {
        let val = MettaValue::Long(42);
        let wildcard = MettaValue::sym("_");
        let result = bidirectional_unify_generic(&val, &wildcard);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_wildcard_both_sides() {
        // (_ $x) vs ($y _) => Some({$x -> _, $y -> _})
        let lhs = sexpr(vec![MettaValue::sym("_"), MettaValue::var("x")]);
        let rhs = sexpr(vec![MettaValue::var("y"), MettaValue::sym("_")]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
    }

    #[test]
    fn test_unify_nested_sexpr() {
        // (f (g $x) $y) vs (f (g a) b) => Some({$x -> a, $y -> b})
        let lhs = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("x")]),
            MettaValue::var("y"),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::sym("a")]),
            MettaValue::sym("b"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("a"));
        assert_eq!(bindings.get("$y").unwrap().as_atom(), Some("b"));
    }

    #[test]
    fn test_unify_arity_mismatch() {
        let lhs = sexpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
        let rhs = sexpr(vec![
            MettaValue::sym("a"),
            MettaValue::sym("b"),
            MettaValue::sym("c"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_none());
    }

    #[test]
    fn test_unify_empty_sexpr() {
        let lhs = sexpr(vec![]);
        let rhs = sexpr(vec![]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_ground_types() {
        // Long
        assert!(
            bidirectional_unify_generic(&MettaValue::Long(42), &MettaValue::Long(42)).is_some()
        );
        assert!(
            bidirectional_unify_generic(&MettaValue::Long(42), &MettaValue::Long(43)).is_none()
        );

        // Bool
        assert!(
            bidirectional_unify_generic(&MettaValue::Bool(true), &MettaValue::Bool(true)).is_some()
        );
        assert!(
            bidirectional_unify_generic(&MettaValue::Bool(true), &MettaValue::Bool(false))
                .is_none()
        );

        // String
        assert!(bidirectional_unify_generic(
            &MettaValue::String("hi".to_string()),
            &MettaValue::String("hi".to_string()),
        )
        .is_some());
        assert!(bidirectional_unify_generic(
            &MettaValue::String("hi".to_string()),
            &MettaValue::String("bye".to_string()),
        )
        .is_none());
    }

    #[test]
    fn test_unify_type_mismatch() {
        // Long vs Bool
        assert!(
            bidirectional_unify_generic(&MettaValue::Long(1), &MettaValue::Bool(true)).is_none()
        );
        // Atom vs Long
        assert!(bidirectional_unify_generic(&MettaValue::sym("a"), &MettaValue::Long(1)).is_none());
    }

    #[test]
    fn test_unify_transitive_deref() {
        // ($x $y $z) vs (a $x $y) => {$x->a, $y->a, $z->a}
        let lhs = sexpr(vec![
            MettaValue::var("x"),
            MettaValue::var("y"),
            MettaValue::var("z"),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("a"),
            MettaValue::var("x"),
            MettaValue::var("y"),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(
            result.is_some(),
            "Transitive deref unification should succeed"
        );
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$x").unwrap().as_atom(), Some("a"));
        // $y is bound to $x, which deref's to a — consistency check should pass
    }

    #[test]
    fn test_unify_deep_nesting() {
        // Build a deeply nested term: (f (f (f ... (f $x) ...)))
        let mut lhs = MettaValue::var("x");
        let mut rhs = MettaValue::sym("leaf");
        for _ in 0..100 {
            lhs = sexpr(vec![MettaValue::sym("f"), lhs]);
            rhs = sexpr(vec![MettaValue::sym("f"), rhs]);
        }
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some(), "Deep nesting should not overflow stack");
        assert_eq!(result.unwrap().get("$x").unwrap().as_atom(), Some("leaf"));
    }

    #[test]
    fn test_unify_unit_empty_sexpr_cross_type() {
        let unit = MettaValue::Unit();
        let empty = sexpr(vec![]);
        assert!(bidirectional_unify_generic(&unit, &empty).is_some());
        assert!(bidirectional_unify_generic(&empty, &unit).is_some());
    }

    #[test]
    fn test_unify_variable_to_variable() {
        // $x vs $y => Some({$x -> $y}) or Some({$y -> $x})
        let x = MettaValue::var("x");
        let y = MettaValue::var("y");
        let result = bidirectional_unify_generic(&x, &y);
        assert!(result.is_some());
        let bindings = result.unwrap();
        // One of them should be bound
        assert_eq!(bindings.len(), 1);
    }

    #[test]
    fn test_unify_same_variable_both_sides() {
        // $x vs $x => Some({}) (trivially equal after deref)
        let x = MettaValue::var("x");
        let result = bidirectional_unify_generic(&x, &x);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_unify_space_ref_not_variable() {
        // &self is a space reference, not a variable
        let factory = GcFactory::default();
        let lhs = factory.atom("&self");
        let rhs = MettaValue::sym("foo");
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(
            result.is_none(),
            "&self should not be treated as a variable"
        );
    }

    #[test]
    fn test_unify_complex_bidirectional() {
        // (f $x (g $y)) vs (f (h $y) (g a))
        // => {$x -> (h a), $y -> a}  (with transitive deref)
        let lhs = sexpr(vec![
            MettaValue::sym("f"),
            MettaValue::var("x"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("y")]),
        ]);
        let rhs = sexpr(vec![
            MettaValue::sym("f"),
            sexpr(vec![MettaValue::sym("h"), MettaValue::var("y")]),
            sexpr(vec![MettaValue::sym("g"), MettaValue::sym("a")]),
        ]);
        let result = bidirectional_unify_generic(&lhs, &rhs);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert_eq!(bindings.get("$y").unwrap().as_atom(), Some("a"));
        // $x should be bound to (h $y) where $y -> a
        // The binding stores the value at bind-time: (h $y)
        // Deref'd, it represents (h a)
        assert!(bindings.get("$x").is_some());
    }

    #[test]
    fn test_is_unification_variable() {
        assert!(is_unification_variable("$x"));
        assert!(is_unification_variable("$foo"));
        assert!(is_unification_variable("&x"));
        assert!(is_unification_variable("'y"));
        assert!(!is_unification_variable("&"));
        assert!(!is_unification_variable("&self"));
        assert!(!is_unification_variable("&kb"));
        assert!(!is_unification_variable("&stack"));
        assert!(!is_unification_variable("foo"));
        assert!(!is_unification_variable("_"));
        assert!(!is_unification_variable("42"));
    }

    #[test]
    fn test_occurs_in_simple() {
        let term = sexpr(vec![MettaValue::sym("f"), MettaValue::var("x")]);
        let bindings = GenericBindings::new();
        assert!(occurs_in_generic("$x", &term, &bindings));
        assert!(!occurs_in_generic("$y", &term, &bindings));
    }

    #[test]
    fn test_occurs_in_through_bindings() {
        // $y is bound to (g $x). Does $x occur in $y?
        let mut bindings = GenericBindings::new();
        let factory = GcFactory::default();
        bindings.insert(
            factory.atom("$y").as_atom().expect("atom"),
            sexpr(vec![MettaValue::sym("g"), MettaValue::var("x")]),
        );
        let term = MettaValue::var("y");
        assert!(occurs_in_generic("$x", &term, &bindings));
    }

    #[test]
    fn test_deref_value_owned_chain() {
        // $x -> $y -> $z -> 42
        let factory = GcFactory::default();
        let mut bindings = GenericBindings::new();
        bindings.insert(
            factory.atom("$x").as_atom().expect("atom"),
            MettaValue::var("y"),
        );
        bindings.insert(
            factory.atom("$y").as_atom().expect("atom"),
            MettaValue::var("z"),
        );
        bindings.insert(
            factory.atom("$z").as_atom().expect("atom"),
            MettaValue::Long(42),
        );

        let result = deref_value_owned(&MettaValue::var("x"), &bindings);
        assert_eq!(result.as_long(), Some(42));
    }

    // =========================================================================
    // Scoped-bindings compose tests (Phase P1)
    //
    // These verify scope-aware compose semantics in isolation, independent
    // of any caller currently producing non-ROOT_SCOPE bindings. Once
    // Phase P2 starts producing non-root-scope bindings, the existing
    // ghost-branch / let* / let-scope regressions exercise the same logic
    // at the integration level — but having unit tests here lets us land
    // P1 with confidence before P2 changes the wire format.
    // =========================================================================

    use crate::backend::models::generic_bindings::{allocate_scope_id, ROOT_SCOPE};

    #[test]
    fn scoped_compose_same_name_different_scopes_independent() {
        // outer at ROOT_SCOPE, inner at a fresh dispatch scope: same bare
        // name but different (scope, name) keys → both survive.
        let factory = GcFactory::default();
        let dispatch_scope = allocate_scope_id();
        let mut outer: GenericBindings<MettaValue> = GenericBindings::new();
        outer.insert_scoped(ROOT_SCOPE, "$x", factory.long(1));
        let mut inner: GenericBindings<MettaValue> = GenericBindings::new();
        inner.insert_scoped(dispatch_scope, "$x", factory.long(2));

        let result = compose_outer_inner_generic(&outer, &inner, &factory);

        assert_eq!(result.len(), 2);
        assert_eq!(result.get_scoped(ROOT_SCOPE, "$x"), Some(&factory.long(1)));
        assert_eq!(
            result.get_scoped(dispatch_scope, "$x"),
            Some(&factory.long(2))
        );
    }

    #[test]
    fn scoped_compose_same_name_same_scope_idempotent() {
        // Both maps bind `(ROOT_SCOPE, $x) → 1`. Compose retains exactly
        // one entry, no conflict.
        let factory = GcFactory::default();
        let mut outer: GenericBindings<MettaValue> = GenericBindings::new();
        outer.insert_scoped(ROOT_SCOPE, "$x", factory.long(1));
        let mut inner: GenericBindings<MettaValue> = GenericBindings::new();
        inner.insert_scoped(ROOT_SCOPE, "$x", factory.long(1));

        let result = compose_outer_inner_strict_generic(&outer, &inner, &factory)
            .expect("idempotent compose must succeed");

        assert_eq!(result.len(), 1);
        assert_eq!(result.get_scoped(ROOT_SCOPE, "$x"), Some(&factory.long(1)));
    }

    #[test]
    fn scoped_compose_same_name_same_scope_ground_conflict_drops() {
        // outer says (ROOT_SCOPE, $x) → 1, inner says (ROOT_SCOPE, $x) → 2.
        // Both ground, both at the SAME scope ⇒ genuine conflict ⇒ strict
        // returns None. This must keep working post-P2 for caller-level
        // (`$user_var`) ground-ground conflicts.
        let factory = GcFactory::default();
        let mut outer: GenericBindings<MettaValue> = GenericBindings::new();
        outer.insert_scoped(ROOT_SCOPE, "$x", factory.long(1));
        let mut inner: GenericBindings<MettaValue> = GenericBindings::new();
        inner.insert_scoped(ROOT_SCOPE, "$x", factory.long(2));

        assert!(
            compose_outer_inner_strict_generic(&outer, &inner, &factory).is_none(),
            "ground-ground same-scope conflict must drop the branch"
        );
    }

    #[test]
    fn scoped_compose_alias_within_scope_resolves() {
        // outer: (ROOT_SCOPE, $a) → atom("$X")  (alias)
        // inner: (ROOT_SCOPE, $a) → 7
        // Result must include (ROOT_SCOPE, $X) → 7 because the alias
        // target lives at the same scope as the binding (§3.3).
        let factory = GcFactory::default();
        let mut outer: GenericBindings<MettaValue> = GenericBindings::new();
        outer.insert_scoped(ROOT_SCOPE, "$a", factory.atom("$X"));
        let mut inner: GenericBindings<MettaValue> = GenericBindings::new();
        inner.insert_scoped(ROOT_SCOPE, "$a", factory.long(7));

        let result = compose_outer_inner_generic(&outer, &inner, &factory);

        assert_eq!(result.get_scoped(ROOT_SCOPE, "$X"), Some(&factory.long(7)));
    }

    #[test]
    fn scoped_compose_cross_scope_aliases_do_not_collide() {
        // outer: (ROOT_SCOPE, $a) → atom("$X"), with $X bound to 5 at root.
        // inner produced by a rule dispatch at scope_d binds (scope_d, $X) → 99.
        // The two $X bindings have different ScopedKeys, so both survive.
        // Without scope tags the inner $X binding would shadow the outer.
        let factory = GcFactory::default();
        let scope_d = allocate_scope_id();
        let mut outer: GenericBindings<MettaValue> = GenericBindings::new();
        outer.insert_scoped(ROOT_SCOPE, "$a", factory.atom("$X"));
        outer.insert_scoped(ROOT_SCOPE, "$X", factory.long(5));
        let mut inner: GenericBindings<MettaValue> = GenericBindings::new();
        inner.insert_scoped(scope_d, "$X", factory.long(99));

        let result = compose_outer_inner_generic(&outer, &inner, &factory);

        assert_eq!(result.get_scoped(ROOT_SCOPE, "$X"), Some(&factory.long(5)));
        assert_eq!(result.get_scoped(scope_d, "$X"), Some(&factory.long(99)));
    }

    #[test]
    fn scoped_compose_only_outer_only_inner_passthrough() {
        // outer: (ROOT_SCOPE, $a) → 1, no inner entry for $a.
        // inner: (s1, $b) → 2, no outer entry for $b.
        // Both pass through verbatim.
        let factory = GcFactory::default();
        let s1 = allocate_scope_id();
        let mut outer: GenericBindings<MettaValue> = GenericBindings::new();
        outer.insert_scoped(ROOT_SCOPE, "$a", factory.long(1));
        let mut inner: GenericBindings<MettaValue> = GenericBindings::new();
        inner.insert_scoped(s1, "$b", factory.long(2));

        let result = compose_outer_inner_generic(&outer, &inner, &factory);

        assert_eq!(result.len(), 2);
        assert_eq!(result.get_scoped(ROOT_SCOPE, "$a"), Some(&factory.long(1)));
        assert_eq!(result.get_scoped(s1, "$b"), Some(&factory.long(2)));
    }

    // =========================================================================
    // mmverify hang fix tests (Plan agent's plan, Phase 3.5).
    // =========================================================================

    #[test]
    fn value_contains_var_with_prefix_finds_atom() {
        let factory = GcFactory::default();
        let v = factory.atom("$__fr_42_tail");
        assert!(value_contains_var_with_prefix(&v, "$__fr_42_"));
        assert!(!value_contains_var_with_prefix(&v, "$__fr_43_"));
    }

    #[test]
    fn value_contains_var_with_prefix_walks_sexpr() {
        // (Cons $__fr_42_head $__fr_42_tail)
        let factory = GcFactory::default();
        let v = factory.sexpr(vec![
            factory.atom("Cons"),
            factory.atom("$__fr_42_head"),
            factory.atom("$__fr_42_tail"),
        ]);
        assert!(value_contains_var_with_prefix(&v, "$__fr_42_"));
        assert!(!value_contains_var_with_prefix(&v, "$__fr_99_"));
    }

    #[test]
    fn value_contains_var_with_prefix_misses_unrelated() {
        // Concrete value with no freshened vars.
        let factory = GcFactory::default();
        let v = factory.sexpr(vec![
            factory.atom("Cons"),
            factory.atom("a"),
            factory.atom("Nil"),
        ]);
        assert!(!value_contains_var_with_prefix(&v, "$__fr_"));
        assert!(!value_contains_var_with_prefix(&v, "$x"));
    }

    #[test]
    fn value_contains_var_with_prefix_walks_nested() {
        // ((foo $__fr_7_inner) bar)
        let factory = GcFactory::default();
        let v = factory.sexpr(vec![
            factory.sexpr(vec![factory.atom("foo"), factory.atom("$__fr_7_inner")]),
            factory.atom("bar"),
        ]);
        assert!(value_contains_var_with_prefix(&v, "$__fr_7_"));
    }

    /// Verifies the canonical mmverify-hang shape: the partial-unification guard
    /// in `enumerate_rules_via_unification` rejects rule matches where a query-side
    /// variable binds to a value containing the rule's freshened LHS variables.
    #[test]
    fn partial_binding_shape_detected() {
        let factory = GcFactory::default();
        // Simulate the unify result for `(append $unbound (Cons "x" Nil))` against
        // rule LHS `(append (Cons $__fr_E_head $__fr_E_tail) $__fr_E_list)`.
        let prefix = "$__fr_42_";
        // $unbound (caller-side, not freshened) → (Cons $__fr_42_head $__fr_42_tail)
        let partial_value = factory.sexpr(vec![
            factory.atom("Cons"),
            factory.atom("$__fr_42_head"),
            factory.atom("$__fr_42_tail"),
        ]);
        // $__fr_42_list (rule-side) → (Cons "x" Nil) — concrete; ignored by guard.
        let concrete_value = factory.sexpr(vec![
            factory.atom("Cons"),
            factory.atom("x"),
            factory.atom("Nil"),
        ]);

        // The guard's logic: a binding is "partial" iff name is NOT freshened
        // AND value contains a freshened var with the same epoch.
        let is_partial_caller_to_rule_value = !"$unbound".starts_with(prefix)
            && value_contains_var_with_prefix(&partial_value, prefix);
        assert!(is_partial_caller_to_rule_value, "must detect partial bind");

        let is_partial_rule_side = !"$__fr_42_list".starts_with(prefix)  // false — IS freshened
                && value_contains_var_with_prefix(&concrete_value, prefix);
        assert!(
            !is_partial_rule_side,
            "rule-side keys with concrete values are fine"
        );

        let is_partial_caller_to_concrete = !"$unbound".starts_with(prefix)
            && value_contains_var_with_prefix(&concrete_value, prefix);
        assert!(
            !is_partial_caller_to_concrete,
            "caller key + concrete value is fine"
        );
    }

    #[test]
    fn transitive_live_vars_includes_value_vars() {
        // (foo $a $b) with empty bindings — live = {$a, $b}.
        let factory = GcFactory::default();
        let value = factory.sexpr(vec![
            factory.atom("foo"),
            factory.atom("$a"),
            factory.atom("$b"),
        ]);
        let bindings: GenericBindings<MettaValue> = GenericBindings::new();

        let live = transitive_live_vars_generic(&value, &bindings);
        assert!(live.contains("$a"));
        assert!(live.contains("$b"));
        assert!(!live.contains("$c"));
    }

    #[test]
    fn transitive_live_vars_chains_through_bindings() {
        // value: (foo $a)
        // bindings: $a → (Cons $b Nil), $b → (bar $c)
        // Live: {$a, $b, $c}
        let factory = GcFactory::default();
        let value = factory.sexpr(vec![factory.atom("foo"), factory.atom("$a")]);
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert_scoped(
            ROOT_SCOPE,
            "$a",
            factory.sexpr(vec![
                factory.atom("Cons"),
                factory.atom("$b"),
                factory.atom("Nil"),
            ]),
        );
        bindings.insert_scoped(
            ROOT_SCOPE,
            "$b",
            factory.sexpr(vec![factory.atom("bar"), factory.atom("$c")]),
        );

        let live = transitive_live_vars_generic(&value, &bindings);
        assert!(live.contains("$a"));
        assert!(live.contains("$b"));
        assert!(live.contains("$c"));
    }

    #[test]
    fn transitive_live_vars_excludes_unreachable_freshened_var() {
        // value: (foo $a)
        // bindings: $a → 7 (concrete), $__fr_99_dead → (Cons $other Nil)
        // Live: {$a} only — the freshened binding is unreachable from value.
        // The Fix 4 trim filter would drop $__fr_99_dead because it's freshened
        // AND not in live; $a stays because it's in live.
        let factory = GcFactory::default();
        let value = factory.sexpr(vec![factory.atom("foo"), factory.atom("$a")]);
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert_scoped(ROOT_SCOPE, "$a", factory.long(7));
        bindings.insert_scoped(
            ROOT_SCOPE,
            "$__fr_99_dead",
            factory.sexpr(vec![
                factory.atom("Cons"),
                factory.atom("$other"),
                factory.atom("Nil"),
            ]),
        );

        let live = transitive_live_vars_generic(&value, &bindings);
        assert!(live.contains("$a"));
        assert!(!live.contains("$__fr_99_dead"));
        assert!(!live.contains("$other"));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::backend::models::MettaValue;
    use proptest::prelude::*;

    // =========================================================================
    // Strategy Generators
    // =========================================================================

    /// Generate arbitrary Long values
    fn arb_long() -> impl Strategy<Value = MettaValue> {
        (-10000i64..10000i64).prop_map(MettaValue::Long)
    }

    /// Generate arbitrary Bool values
    fn arb_bool() -> impl Strategy<Value = MettaValue> {
        prop::bool::ANY.prop_map(MettaValue::Bool)
    }

    /// Generate arbitrary Symbol values (non-variable atoms)
    fn arb_symbol() -> impl Strategy<Value = MettaValue> {
        "[a-z]{1,8}".prop_map(|s| MettaValue::sym(&s))
    }

    /// Generate arbitrary String values
    fn arb_string() -> impl Strategy<Value = MettaValue> {
        ".{0,20}".prop_map(MettaValue::String)
    }

    /// Generate simple ground values (no variables, no S-expressions)
    fn arb_simple_ground() -> impl Strategy<Value = MettaValue> {
        prop_oneof![
            arb_long(),
            arb_bool(),
            arb_symbol(),
            arb_string(),
            Just(MettaValue::Unit()),
        ]
    }

    /// Generate ground MettaValue (no variables) with bounded depth
    fn arb_ground_value(depth: usize) -> BoxedStrategy<MettaValue> {
        if depth == 0 {
            arb_simple_ground().boxed()
        } else {
            prop_oneof![
                arb_simple_ground(),
                prop::collection::vec(arb_ground_value(depth - 1), 0..4)
                    .prop_map(MettaValue::SExpr),
            ]
            .boxed()
        }
    }

    /// Generate arbitrary MettaValue including variables, with bounded depth
    fn arb_metta_value(depth: usize) -> BoxedStrategy<MettaValue> {
        if depth == 0 {
            prop_oneof![
                arb_simple_ground(),
                "[a-e]{1,3}".prop_map(|s| MettaValue::var(&s)),
            ]
            .boxed()
        } else {
            prop_oneof![
                arb_simple_ground(),
                "[a-e]{1,3}".prop_map(|s| MettaValue::var(&s)),
                prop::collection::vec(arb_metta_value(depth - 1), 0..4).prop_map(MettaValue::SExpr),
            ]
            .boxed()
        }
    }

    /// Check if a MettaValue contains any variables
    fn is_ground(v: &MettaValue) -> bool {
        let mut stack: Vec<&MettaValue> = vec![v];
        while let Some(current) = stack.pop() {
            if let Some(name) = current.as_atom() {
                if is_unification_variable(name) {
                    return false;
                }
            }
            if let Some(items) = current.as_sexpr() {
                for item in items.iter() {
                    stack.push(item);
                }
            }
        }
        true
    }

    // =========================================================================
    // Property Tests
    // =========================================================================

    proptest! {
        /// Reflexivity: any term unifies with itself
        #[test]
        fn unify_reflexive(v in arb_metta_value(3)) {
            let result = bidirectional_unify_generic(&v, &v);
            prop_assert!(result.is_some(), "Any term should unify with itself: {:?}", v);
        }

        /// Symmetry: unify(a,b) succeeds iff unify(b,a) succeeds
        #[test]
        fn unify_symmetric(a in arb_metta_value(2), b in arb_metta_value(2)) {
            let ab = bidirectional_unify_generic(&a, &b);
            let ba = bidirectional_unify_generic(&b, &a);
            prop_assert_eq!(
                ab.is_some(), ba.is_some(),
                "Unification should be symmetric: a={:?}, b={:?}", a, b
            );
        }

        /// Ground terms: unify(ground_a, ground_b) = Some({}) iff a == b structurally
        #[test]
        fn unify_ground_terms(a in arb_ground_value(2), b in arb_ground_value(2)) {
            // Only test values we know are ground
            prop_assume!(is_ground(&a) && is_ground(&b));
            let result = bidirectional_unify_generic(&a, &b);
            if a == b {
                prop_assert!(result.is_some(), "Equal ground terms should unify");
                prop_assert!(result.unwrap().is_empty(), "Ground unification should produce no bindings");
            } else {
                prop_assert!(result.is_none(), "Unequal ground terms should not unify: {:?} vs {:?}", a, b);
            }
        }

        /// Variable consistency: ($x $x) only unifies with (a b) when a == b
        #[test]
        fn variable_consistency(a in arb_ground_value(2), b in arb_ground_value(2)) {
            prop_assume!(is_ground(&a) && is_ground(&b));
            let pattern = MettaValue::SExpr(vec![MettaValue::var("x"), MettaValue::var("x")]);
            let value = MettaValue::SExpr(vec![a.clone(), b.clone()]);
            let result = bidirectional_unify_generic(&pattern, &value);
            if a == b {
                prop_assert!(result.is_some(), "($x $x) should unify with ({:?} {:?})", a, b);
            } else {
                prop_assert!(result.is_none(), "($x $x) should NOT unify with ({:?} {:?})", a, b);
            }
        }

        /// Wildcard universality: _ unifies with any term (from either side)
        #[test]
        fn wildcard_matches_anything(v in arb_metta_value(3)) {
            let wildcard = MettaValue::sym("_");
            prop_assert!(
                bidirectional_unify_generic(&wildcard, &v).is_some(),
                "Wildcard should match anything on LHS"
            );
            prop_assert!(
                bidirectional_unify_generic(&v, &wildcard).is_some(),
                "Wildcard should match anything on RHS"
            );
        }

        /// Idempotence: unifying a term with itself produces empty or identity bindings
        #[test]
        fn unify_self_produces_no_new_info(v in arb_ground_value(3)) {
            prop_assume!(is_ground(&v));
            let result = bidirectional_unify_generic(&v, &v);
            prop_assert!(result.is_some());
            prop_assert!(result.unwrap().is_empty(), "Self-unification of ground term should produce no bindings");
        }

        /// Occurs check: $x never unifies with S-expr containing $x
        #[test]
        fn occurs_check_prevents_infinite_terms(
            wrapper_head in "[a-z]{1,3}".prop_map(|s| MettaValue::sym(&s)),
            extra_args in prop::collection::vec(arb_ground_value(1), 0..3),
        ) {
            let var = MettaValue::var("occ");
            let mut children = vec![wrapper_head];
            children.extend(extra_args);
            children.push(MettaValue::var("occ"));
            let term = MettaValue::SExpr(children);

            let result = bidirectional_unify_generic(&var, &term);
            prop_assert!(result.is_none(), "Occurs check should prevent $occ = {:?}", term);
        }

        /// Single variable binding: $x vs ground_value always succeeds
        #[test]
        fn single_var_binds_to_ground(v in arb_ground_value(2)) {
            prop_assume!(is_ground(&v));
            let var = MettaValue::var("single");
            let result = bidirectional_unify_generic(&var, &v);
            prop_assert!(result.is_some());
            let bindings = result.unwrap();
            prop_assert_eq!(bindings.len(), 1);
            prop_assert_eq!(bindings.get("$single").unwrap(), &v);
        }
    }
}
