//! Helper functions for MeTTa evaluation.
//!
//! This module contains utility functions used throughout the evaluation process,
//! including pattern specificity calculation, head symbol extraction, grounded
//! operation detection, and token resolution.
//!
//! Note: Functions that depend on `eval()` (like eval_conjunction, evaluate_grounded_args)
//! remain in mod.rs to avoid circular dependencies.

use std::borrow::Cow;

use phf::phf_set;
use tracing::trace;

use crate::backend::environment::{HeapEnvironment, GenericEnvironment};
use crate::backend::fuzzy_match::{FuzzyMatcher, SmartSuggestion, SuggestionContext};
use crate::backend::models::{Bindings, MettaValue, MettaValueFactory, MettaValueInner, MettaValueTrait};

use super::builtin;

/// MeTTa special forms for "did you mean" suggestions during evaluation
#[allow(dead_code)]
pub const SPECIAL_FORMS: &[&str] = &[
    "=",
    "!",
    "quote",
    "if",
    "error",
    "is-error",
    "catch",
    "eval",
    "function",
    "return",
    "chain",
    "match",
    "case",
    "switch",
    "let",
    ":",
    "get-type",
    "check-type",
    "map-atom",
    "filter-atom",
    "foldl-atom",
    "car-atom",
    "cdr-atom",
    "cons-atom",
    "decons-atom",
    "size-atom",
    "max-atom",
    "let*",
    "unify",
    "new-space",
    "add-atom",
    "remove-atom",
    "collapse",
    "superpose",
    "amb",
    "guard",
    "commit",
    "backtrack",
    "get-atoms",
    "new-state",
    "get-state",
    "change-state!",
    "new-memo",
    "memo",
    "memo-first",
    "clear-memo!",
    "memo-stats",
    "bind!",
    "println!",
    "trace!",
    "nop",
    "repr",
    "format-args",
    "empty",
    "get-metatype",
    "include",
];

/// Grounded operations that should be evaluated eagerly (before pattern matching).
/// Uses compile-time perfect hash for O(1) lookup.
static GROUNDED_OPS: phf::Set<&'static str> = phf_set! {
    // Basic arithmetic
    "+", "-", "*", "/", "%",
    // Math functions (short names)
    "pow", "abs", "floor", "ceil", "round", "sqrt",
    // Math functions (full names from try_eval_builtin)
    "floor-div",
    "pow-math", "sqrt-math", "abs-math", "log-math", "trunc-math",
    "ceil-math", "floor-math", "round-math",
    // Trigonometric functions
    "sin-math", "asin-math", "cos-math", "acos-math",
    "tan-math", "atan-math",
    // Float classification
    "isnan-math", "isinf-math",
    // Comparison operations
    "<", "<=", ">", ">=", "==", "!=",
    // Boolean operations
    "not", "and", "or",
    // Type operations that return concrete values
    "get-type", "get-metatype",
    // Atom/expression manipulation operations (all return immediate values)
    "car-atom", "cdr-atom", "cons-atom", "decons-atom", "size-atom",
    "max-atom", "min-atom", "index-atom",
};

/// Set of operations that need re-dispatch through eval_sexpr_step after
/// Cartesian product argument evaluation. Uses compile-time perfect hash
/// for O(1) lookup.
///
/// These are special forms that:
/// 1. Have dedicated dispatch in eval_sexpr_step
/// 2. Are NOT already handled by try_eval_builtin (arithmetic ops)
/// 3. Need special argument handling (lazy args, iteration, etc.)
static SPECIAL_FORMS_REDISPATCH: phf::Set<&'static str> = phf_set! {
    // Higher-order list operations (iterate over elements)
    "map-atom", "filter-atom", "foldl-atom",
    // Control flow (lazy branch evaluation)
    "if", "case", "switch", "switch-minimal", "switch-internal",
    // Binding forms (special scoping)
    "let", "let*", "unify",
    // Sequencing/continuation forms
    "chain", "function", "return",
    // Pattern/substitution forms
    "sealed", "atom-subst", "match",
    // Error handling (special flow)
    "catch", "is-error",
    // Evaluation control
    "eval", "quote",
    // Space operations that need special handling
    "collapse", "collapse-bind", "amb", "guard",
    // State operations
    "new-state", "get-state", "change-state!",
    // I/O operations
    "println!", "trace!",
};

/// Special forms that should be evaluated BEFORE being passed to user-defined rules.
/// These are special forms that produce values and need eager evaluation when used
/// as arguments to other expressions.
///
/// This is critical for MeTTa HE semantic alignment. In MeTTa HE, map-atom is a
/// regular rule that gets evaluated through normal rule application. In MeTTaTron,
/// it's a special form. To maintain semantic equivalence, we need to evaluate these
/// special forms eagerly when they appear as arguments.
///
/// Example: For `(get-expr-size (map-atom (a b) $v ($v x)))`:
/// - MeTTa HE: map-atom is a rule, evaluated as part of normal rule application
/// - MeTTaTron: Without eager evaluation, map-atom would be passed unevaluated
///   to get-expr-size, causing semantic mismatch
static EAGER_SPECIAL_FORMS: phf::Set<&'static str> = phf_set! {
    // Higher-order list operations (produce list values)
    "map-atom", "filter-atom", "foldl-atom",
    // Evaluation control that produces values
    "eval",
    // Space operations that produce values
    "collapse", "collapse-bind", "superpose",
    // State operations that produce values
    "get-state",
    // Error handling that produces values
    "catch",
    // Other value-producing special forms
    "get-metatype",
    // String operations
    "repr", "format-args",
};

/// Check if an operation needs re-dispatch through eval_sexpr_step after
/// Cartesian product argument evaluation.
///
/// Inlined to eliminate function call overhead - compiles to just the phf hash lookup.
#[inline(always)]
pub fn needs_special_form_redispatch(op: &str) -> bool {
    SPECIAL_FORMS_REDISPATCH.contains(op)
}

/// Check if an operation is a special form that should be evaluated eagerly
/// when appearing as an argument to other expressions.
///
/// This ensures MeTTa HE semantic alignment: special forms that produce values
/// (like map-atom) are evaluated before being passed to user-defined rules.
#[inline(always)]
pub fn is_eager_special_form(op: &str) -> bool {
    EAGER_SPECIAL_FORMS.contains(op)
}

/// Convert MettaValue to a friendly type name for error messages
/// This provides user-friendly type names instead of debug format like "Long(5)"
pub fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value.inner() {
        MettaValueInner::Long(_) => "Number (integer)",
        MettaValueInner::Float(_) => "Number (float)",
        MettaValueInner::Bool(_) => "Bool",
        MettaValueInner::String(_) => "String",
        MettaValueInner::Atom(_) => "Atom",
        MettaValueInner::Unit => "Unit",
        MettaValueInner::SExpr(_) => "S-expression",
        MettaValueInner::Error(_, _) => "Error",
        MettaValueInner::Type(_) => "Type",
        MettaValueInner::Conjunction(_) => "Conjunction",
        MettaValueInner::Space(_) => "Space",
        MettaValueInner::State(_) => "State",
        MettaValueInner::Memo(_) => "Memo",
        MettaValueInner::Empty => "Empty",
    }
}

/// Work item for iterative friendly_value_repr
enum ReprWork<'a> {
    /// Process a value
    Process(&'a MettaValue),
    /// Join collected strings with separator and wrap
    Join {
        count: usize,
        prefix: &'static str,
        suffix: &'static str,
        separator: &'static str,
    },
}

/// Convert MettaValue to a user-friendly representation for error messages
/// Unlike debug format, this shows values in MeTTa syntax
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Error messages involving deeply nested data
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
pub fn friendly_value_repr(value: &MettaValue) -> String {
    let mut work_stack: Vec<ReprWork<'_>> = Vec::with_capacity(16);
    let mut result_stack: Vec<String> = Vec::with_capacity(16);

    work_stack.push(ReprWork::Process(value));

    while let Some(work) = work_stack.pop() {
        match work {
            ReprWork::Process(val) => match val.inner() {
                MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                MettaValueInner::Float(f) => result_stack.push(f.to_string()),
                MettaValueInner::Bool(b) => {
                    result_stack.push(if *b { "True" } else { "False" }.to_string());
                }
                MettaValueInner::String(s) => result_stack.push(format!("\"{}\"", s)),
                MettaValueInner::Atom(a) => result_stack.push(a.clone()),
                MettaValueInner::Unit => result_stack.push("()".to_string()),
                MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                MettaValueInner::Space(handle) => {
                    result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                }
                MettaValueInner::State(id) => {
                    result_stack.push(format!("(State {})", id));
                }
                MettaValueInner::Memo(handle) => {
                    result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                }
                MettaValueInner::Error(msg, _) => {
                    result_stack.push(format!("(error \"{}\")", msg));
                }
                MettaValueInner::Type(t) => {
                    // Push join marker, then process inner
                    work_stack.push(ReprWork::Join {
                        count: 1,
                        prefix: "(: ",
                        suffix: ")",
                        separator: "",
                    });
                    work_stack.push(ReprWork::Process(t));
                }
                MettaValueInner::SExpr(items) => {
                    if items.is_empty() {
                        result_stack.push("()".to_string());
                    } else {
                        // Push join marker, then process items in reverse
                        work_stack.push(ReprWork::Join {
                            count: items.len(),
                            prefix: "(",
                            suffix: ")",
                            separator: " ",
                        });
                        for item in items.iter().rev() {
                            work_stack.push(ReprWork::Process(item));
                        }
                    }
                }
                MettaValueInner::Conjunction(goals) => {
                    if goals.is_empty() {
                        result_stack.push("(,)".to_string());
                    } else {
                        work_stack.push(ReprWork::Join {
                            count: goals.len(),
                            prefix: "(, ",
                            suffix: ")",
                            separator: " ",
                        });
                        for goal in goals.iter().rev() {
                            work_stack.push(ReprWork::Process(goal));
                        }
                    }
                }
            },
            ReprWork::Join {
                count,
                prefix,
                suffix,
                separator,
            } => {
                let start = result_stack.len() - count;
                let parts: Vec<String> = result_stack.drain(start..).collect();
                result_stack.push(format!("{}{}{}", prefix, parts.join(separator), suffix));
            }
        }
    }

    debug_assert_eq!(result_stack.len(), 1);
    result_stack.pop().unwrap_or_default()
}

/// Check if an operator is close to a known special form using context-aware heuristics
///
/// Returns a SmartSuggestion with confidence level to determine how to present
/// the suggestion (as a warning/note vs. error vs. not at all).
///
/// Uses the three-pillar context-aware approach to avoid false positives:
/// - **Arity compatibility**: Expression arity must match candidate's min/max arity
/// - **Type compatibility**: Argument types must match expected types from signatures
/// - **Prefix compatibility**: $vars vs &spaces vs plain atoms
///
/// # Arguments
/// - `op`: The operator/head of the expression (potentially misspelled)
/// - `expr`: The full expression (for arity/type checking)
/// - `env`: The environment (for type inference)
#[allow(dead_code)]
pub fn suggest_special_form_with_context(
    op: &str,
    expr: &[MettaValue],
    env: &HeapEnvironment,
) -> Option<SmartSuggestion> {
    use std::sync::OnceLock;

    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    let matcher = MATCHER.get_or_init(|| FuzzyMatcher::from_terms(SPECIAL_FORMS.iter().copied()));

    // Build context for position 0 (head position)
    let ctx = SuggestionContext::for_head(expr, env);

    // Use context-aware suggestion with max distance 2
    // The three-pillar validation filters out structurally incompatible suggestions
    matcher.smart_suggest_with_context(op, 2, &ctx)
}

/// Check if an atom name is a grounded operation that should be eagerly evaluated.
///
/// Inlined to eliminate function call overhead - compiles to just the phf hash lookup.
#[inline(always)]
pub fn is_grounded_op(name: &str) -> bool {
    GROUNDED_OPS.contains(name)
}

/// Resolve registered tokens (like &stack → Space) at the top level only.
/// This is a "shallow resolution" for lazy evaluation:
/// - Atoms that are registered tokens are replaced with their values
/// - Variables ($x) are kept as-is (they're for pattern matching)
/// - S-expressions are kept unevaluated (lazy evaluation)
/// - Special tokens like &self are NOT resolved here (handled in eval_step)
pub fn resolve_tokens_shallow(items: &[MettaValue], env: &HeapEnvironment) -> Vec<MettaValue> {
    items
        .iter()
        .map(|item| {
            match item.inner() {
                MettaValueInner::Atom(name) => {
                    // Skip variables - they're for pattern matching
                    if name.starts_with('$') {
                        return item.clone();
                    }
                    // Skip special atoms like &self, &kb that might be handled elsewhere
                    // or are truly space references
                    if name == "&self" {
                        // Let &self be resolved later in eval_step
                        return item.clone();
                    }
                    // Try to resolve registered tokens (e.g., &stack → Space)
                    if let Some(bound_value) = env.lookup_token(name) {
                        bound_value
                    } else {
                        item.clone()
                    }
                }
                // Keep everything else unchanged (S-expressions, literals, etc.)
                _ => item.clone(),
            }
        })
        .collect()
}

/// Generic version of resolve_tokens_shallow.
///
/// This function works with any value type implementing `MettaValueTrait`,
/// enabling zero-conversion evaluation for both heap and arena allocation modes.
///
/// Note: Token lookup returns values from the environment. For GenericEnvironment,
/// this is zero-conversion as tokens are stored in the native value type.
pub fn resolve_tokens_shallow_generic<V, F>(items: &[V], env: &GenericEnvironment<V, F>, _factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    items
        .iter()
        .map(|item| {
            if let Some(name) = item.as_atom() {
                // Skip variables - they're for pattern matching
                if name.starts_with('$') {
                    return item.clone();
                }
                // Skip special atoms like &self, &kb that might be handled elsewhere
                // or are truly space references
                if name == "&self" {
                    // Let &self be resolved later in eval_step
                    return item.clone();
                }
                // Try to resolve registered tokens (e.g., &stack → Space)
                // Use lookup_token_generic which converts to V
                if let Some(resolved) = env.lookup_token_generic(name, _factory) {
                    resolved
                } else {
                    item.clone()
                }
            } else {
                // Keep everything else unchanged (S-expressions, literals, etc.)
                item.clone()
            }
        })
        .collect()
}

/// Work item for iterative preprocess_space_refs
///
/// The function transforms a list of MettaValues by:
/// 1. Combining adjacent `& name` atoms into `&name`
/// 2. Recursively processing nested SExprs
///
/// The work stack processes items depth-first, building results bottom-up.
enum PreprocessWork {
    /// Process a list of items at a given nesting level
    /// - `items`: The items to process
    /// - `start_index`: Index in result_stack where this level's results begin
    ProcessItems {
        items: Vec<MettaValue>,
        start_index: usize,
    },
    /// Build an SExpr from collected children
    /// - `start_index`: Index in result_stack where children begin
    BuildSExpr { start_index: usize },
}

/// Preprocess S-expression items to combine `& name` into `&name`.
/// The Tree-Sitter parser treats `&foo` as two tokens (`&` and `foo`), but we need
/// them combined for HE-compatible space reference semantics (e.g., `&self`, `&kb`, `&stack`).
/// Also processes nested S-expressions.
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
/// - Processing before trampoline's MAX_EVAL_DEPTH check runs
pub fn preprocess_space_refs(items: Vec<MettaValue>) -> Vec<MettaValue> {
    // Fast path: empty input
    if items.is_empty() {
        return items;
    }

    let mut work_stack: Vec<PreprocessWork> = Vec::with_capacity(16);
    let mut result_stack: Vec<MettaValue> = Vec::with_capacity(items.len());

    // Start by processing the top-level items
    work_stack.push(PreprocessWork::ProcessItems {
        items,
        start_index: 0,
    });

    while let Some(work) = work_stack.pop() {
        match work {
            PreprocessWork::ProcessItems { items, start_index } => {
                let mut i = 0;
                while i < items.len() {
                    // Check for `& name` pattern - combine any `&` followed by an atom
                    if i + 1 < items.len() {
                        if let (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) =
                            (items[i].inner(), items[i + 1].inner())
                        {
                            if a == "&" {
                                // Combine `& name` into `&name`
                                result_stack.push(MettaValue::Atom(format!("&{}", b)));
                                i += 2;
                                continue;
                            }
                        }
                    }

                    // Handle nested SExpr: push build marker then process children
                    if let MettaValueInner::SExpr(nested) = items[i].inner() {
                        if nested.is_empty() {
                            // Empty SExpr: just push it directly
                            result_stack.push(items[i].clone());
                        } else {
                            // First, push remaining items to be processed after BuildSExpr
                            if i + 1 < items.len() {
                                work_stack.push(PreprocessWork::ProcessItems {
                                    items: items[i + 1..].to_vec(),
                                    start_index, // Continue building at same level
                                });
                            }

                            // Push BuildSExpr marker (will be processed after children)
                            let child_start = result_stack.len();
                            work_stack.push(PreprocessWork::BuildSExpr {
                                start_index: child_start,
                            });

                            // Push nested items for processing
                            work_stack.push(PreprocessWork::ProcessItems {
                                items: nested.clone(), // O(1) - cloning Vec of Arc-wrapped values
                                start_index: child_start,
                            });

                            // Break out of this loop - remaining items are on work stack
                            break;
                        }
                        i += 1;
                        continue;
                    }

                    // Non-SExpr item: clone (O(1) due to Arc) and add to results
                    result_stack.push(items[i].clone());
                    i += 1;
                }
            }
            PreprocessWork::BuildSExpr { start_index } => {
                // Collect children from result stack
                let children: Vec<MettaValue> = result_stack.drain(start_index..).collect();
                result_stack.push(MettaValue::SExpr(children));
            }
        }
    }

    result_stack
}

/// Extract the head symbol from a pattern for indexing
/// Returns None if the pattern doesn't have a clear head symbol
pub fn get_head_symbol(pattern: &MettaValue) -> Option<&str> {
    let hs = match pattern.inner() {
        // For s-expressions like (double $x), extract "double"
        // EXCEPT: standalone "&" is allowed as a head symbol (used in match)
        MettaValueInner::SExpr(items) if !items.is_empty() => match items[0].inner() {
            MettaValueInner::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || head == "&")
                    && !head.starts_with('\'')
                    && head != "_" =>
            {
                Some(head.as_str())
            }
            _ => None,
        },
        // For bare atoms like foo, use the atom itself
        // EXCEPT: standalone "&" is allowed (used in match)
        MettaValueInner::Atom(head)
            if !head.starts_with('$')
                && (!head.starts_with('&') || head == "&")
                && !head.starts_with('\'')
                && head != "_" =>
        {
            Some(head.as_str())
        }
        _ => None,
    };

    trace!(target: "mettatron::backend::eval::get_head_symbol", ?hs);
    hs
}

/// Compute the specificity of a pattern (lower is more specific)
/// More specific patterns have fewer variables
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested patterns. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
/// - Pattern matching on complex nested structures
pub fn pattern_specificity(pattern: &MettaValue) -> usize {
    let mut work_stack: Vec<&MettaValue> = Vec::with_capacity(16);
    work_stack.push(pattern);
    let mut total: usize = 0;

    while let Some(val) = work_stack.pop() {
        match val.inner() {
            // Variables are least specific
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            MettaValueInner::Atom(s)
                if (s.starts_with('$')
                    || s.starts_with('&')
                    || s.starts_with('\'')
                    || s == "_")
                    && s != "&" =>
            {
                total += 1000; // Variables are least specific
            }
            // Literals contribute 0 (most specific, including standalone "&")
            MettaValueInner::Atom(_)
            | MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Unit
            | MettaValueInner::Space(_)
            | MettaValueInner::State(_)
            | MettaValueInner::Memo(_)
            | MettaValueInner::Empty => {}
            // Compound types: push children onto work stack
            MettaValueInner::SExpr(items) => {
                work_stack.extend(items.iter());
            }
            MettaValueInner::Conjunction(goals) => {
                work_stack.extend(goals.iter());
            }
            MettaValueInner::Error(_, details) => {
                work_stack.push(details);
            }
            MettaValueInner::Type(t) => {
                work_stack.push(t);
            }
        }
    }

    total
}

/// Apply variable bindings to a value
///
/// This is made public to support optimized match operations in Environment
///
/// Uses Cow<'a, MettaValue> to avoid cloning when no substitution is needed.
/// Returns Cow::Borrowed(value) when the expression contains no variables bound in `bindings`.
/// Returns Cow::Owned(new_value) only when actual substitution occurred.
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
/// - Processing before trampoline's MAX_EVAL_DEPTH check runs
pub fn apply_bindings<'a>(value: &'a MettaValue, bindings: &Bindings) -> Cow<'a, MettaValue> {
    trace!(target: "mettatron::backend::eval::apply_bindings", ?value, ?bindings);

    // Fast path: empty bindings means no substitutions possible
    if bindings.is_empty() {
        return Cow::Borrowed(value);
    }

    // For simple cases without nesting, use fast path
    match value.inner() {
        // Apply bindings to variables (atoms starting with $, &, or ')
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValueInner::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && s != "&" =>
        {
            match bindings.iter().find(|(name, _)| name.as_str() == s) {
                Some((_name, val)) => return Cow::Owned(val.clone()),
                None => return Cow::Borrowed(value),
            }
        }
        // Non-compound types don't need substitution
        MettaValueInner::Long(_)
        | MettaValueInner::Float(_)
        | MettaValueInner::Bool(_)
        | MettaValueInner::String(_)
        | MettaValueInner::Unit
        | MettaValueInner::Space(_)
        | MettaValueInner::State(_)
        | MettaValueInner::Type(_)
        | MettaValueInner::Memo(_)
        | MettaValueInner::Empty => return Cow::Borrowed(value),
        // Regular atoms (not variables)
        MettaValueInner::Atom(_) => return Cow::Borrowed(value),
        // Compound types need iterative processing
        MettaValueInner::SExpr(_)
        | MettaValueInner::Conjunction(_)
        | MettaValueInner::Error(_, _) => {}
    }

    // Iterative implementation using explicit work stack
    apply_bindings_iterative(value, bindings)
}

/// Work item for iterative apply_bindings
#[derive(Clone)]
enum ApplyBindingsWork<'a> {
    /// Process a value - may push more work
    Process(&'a MettaValue),
    /// Build an SExpr from the last N results
    BuildSExpr(usize, &'a MettaValue),
    /// Build a Conjunction from the last N results
    BuildConjunction(usize, &'a MettaValue),
    /// Build an Error from the last result
    BuildError(String, &'a MettaValue),
}

/// Iterative implementation of apply_bindings using explicit work stack.
///
/// This avoids recursion to prevent stack overflow on deeply nested structures.
fn apply_bindings_iterative<'a>(value: &'a MettaValue, bindings: &Bindings) -> Cow<'a, MettaValue> {
    // Work stack: items to process
    let mut work_stack: Vec<ApplyBindingsWork<'a>> = Vec::with_capacity(32);
    // Result stack: processed results (MettaValue, was_modified)
    let mut result_stack: Vec<(MettaValue, bool)> = Vec::with_capacity(32);

    work_stack.push(ApplyBindingsWork::Process(value));

    while let Some(work) = work_stack.pop() {
        match work {
            ApplyBindingsWork::Process(val) => {
                match val.inner() {
                    // Variable substitution
                    MettaValueInner::Atom(s)
                        if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\''))
                            && s != "&" =>
                    {
                        match bindings.iter().find(|(name, _)| name.as_str() == s) {
                            Some((_name, bound_val)) => {
                                result_stack.push((bound_val.clone(), true));
                            }
                            None => {
                                result_stack.push((val.clone(), false));
                            }
                        }
                    }
                    // S-expression: push build marker, then push children in reverse order
                    MettaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push((val.clone(), false));
                        } else {
                            // Push build marker first (processed last)
                            work_stack.push(ApplyBindingsWork::BuildSExpr(items.len(), val));
                            // Push children in reverse order so first child is processed first
                            for item in items.iter().rev() {
                                work_stack.push(ApplyBindingsWork::Process(item));
                            }
                        }
                    }
                    // Conjunction: similar to SExpr
                    MettaValueInner::Conjunction(goals) => {
                        if goals.is_empty() {
                            result_stack.push((val.clone(), false));
                        } else {
                            work_stack.push(ApplyBindingsWork::BuildConjunction(goals.len(), val));
                            for goal in goals.iter().rev() {
                                work_stack.push(ApplyBindingsWork::Process(goal));
                            }
                        }
                    }
                    // Error: push build marker, then push details
                    MettaValueInner::Error(msg, details) => {
                        work_stack.push(ApplyBindingsWork::BuildError(msg.clone(), val));
                        work_stack.push(ApplyBindingsWork::Process(details));
                    }
                    // All other types: no substitution needed
                    _ => {
                        result_stack.push((val.clone(), false));
                    }
                }
            }
            ApplyBindingsWork::BuildSExpr(count, original) => {
                // Pop `count` results and build SExpr
                let start = result_stack.len() - count;
                let children: Vec<(MettaValue, bool)> = result_stack.drain(start..).collect();

                let any_modified = children.iter().any(|(_, modified)| *modified);
                if any_modified {
                    let new_items: Vec<MettaValue> = children.into_iter().map(|(v, _)| v).collect();
                    result_stack.push((MettaValue::SExpr(new_items), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
            ApplyBindingsWork::BuildConjunction(count, original) => {
                let start = result_stack.len() - count;
                let children: Vec<(MettaValue, bool)> = result_stack.drain(start..).collect();

                let any_modified = children.iter().any(|(_, modified)| *modified);
                if any_modified {
                    let new_goals: Vec<MettaValue> = children.into_iter().map(|(v, _)| v).collect();
                    result_stack.push((MettaValue::Conjunction(new_goals), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
            ApplyBindingsWork::BuildError(msg, original) => {
                // Pop the details result
                let (details, modified) = result_stack
                    .pop()
                    .expect("BuildError should have details on result stack");

                if modified {
                    result_stack.push((MettaValue::Error(msg, details), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
        }
    }

    // Final result should be on the stack
    debug_assert_eq!(
        result_stack.len(),
        1,
        "apply_bindings should produce exactly one result"
    );
    let (result, modified) = result_stack
        .pop()
        .expect("Result stack should not be empty");

    if modified {
        Cow::Owned(result)
    } else {
        Cow::Borrowed(value)
    }
}

/// Delegate to builtin module for built-in operations
pub fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    builtin::try_eval_builtin(op, args)
}

/// Check structural equality between two MettaValues
/// HE-compatible: Unit and empty SExpr are considered equal
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
#[allow(dead_code)]
pub fn values_equal(a: &MettaValue, b: &MettaValue) -> bool {
    // Work stack: pairs of values to compare
    let mut work_stack: Vec<(&MettaValue, &MettaValue)> = Vec::with_capacity(16);
    work_stack.push((a, b));

    while let Some((val_a, val_b)) = work_stack.pop() {
        let equal = match (val_a.inner(), val_b.inner()) {
            // Same-type comparisons
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) => a == b,
            (MettaValueInner::Bool(a), MettaValueInner::Bool(b)) => a == b,
            (MettaValueInner::Long(a), MettaValueInner::Long(b)) => a == b,
            (MettaValueInner::Float(a), MettaValueInner::Float(b)) => a == b,
            (MettaValueInner::String(a), MettaValueInner::String(b)) => a == b,
            (MettaValueInner::Unit, MettaValueInner::Unit) => true,
            (MettaValueInner::Empty, MettaValueInner::Empty) => true,

            // HE-compatible: Unit equals empty SExpr
            (MettaValueInner::Unit, MettaValueInner::SExpr(items))
            | (MettaValueInner::SExpr(items), MettaValueInner::Unit) => items.is_empty(),

            // S-expression structural equality: push children onto work stack
            (MettaValueInner::SExpr(a_items), MettaValueInner::SExpr(b_items)) => {
                if a_items.len() != b_items.len() {
                    return false; // Early exit on length mismatch
                }
                // Push children in reverse order for LIFO processing
                for (a, b) in a_items.iter().zip(b_items.iter()).rev() {
                    work_stack.push((a, b));
                }
                true // Continue processing work stack
            }

            // Conjunction structural equality: push children onto work stack
            (MettaValueInner::Conjunction(a_goals), MettaValueInner::Conjunction(b_goals)) => {
                if a_goals.len() != b_goals.len() {
                    return false; // Early exit on length mismatch
                }
                for (a, b) in a_goals.iter().zip(b_goals.iter()).rev() {
                    work_stack.push((a, b));
                }
                true // Continue processing work stack
            }

            // Error equality: check message and push details onto work stack
            (
                MettaValueInner::Error(a_msg, a_details),
                MettaValueInner::Error(b_msg, b_details),
            ) => {
                if a_msg != b_msg {
                    return false; // Message mismatch
                }
                work_stack.push((a_details, b_details));
                true // Continue processing work stack
            }

            // Space and State equality by identity
            (MettaValueInner::Space(a), MettaValueInner::Space(b)) => a.id == b.id,
            (MettaValueInner::State(a), MettaValueInner::State(b)) => a == b,

            // Type equality
            (MettaValueInner::Type(a), MettaValueInner::Type(b)) => a == b,

            // Memo equality by identity
            (MettaValueInner::Memo(a), MettaValueInner::Memo(b)) => a.id == b.id,

            // Different types are not equal
            _ => false,
        };

        if !equal {
            return false; // Early exit on any mismatch
        }
    }

    true // All pairs matched successfully
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that deeply nested structures don't cause stack overflow.
    /// This verifies that all three converted functions use iterative implementations.
    ///
    /// 10,000 levels of nesting would cause stack overflow with recursive implementations
    /// (typical stack is ~8MB, each frame ~100-200 bytes, so ~40,000-80,000 max depth).
    /// Tokio worker stacks are even smaller (~2MB).
    #[test]
    fn test_deeply_nested_no_stack_overflow() {
        const DEPTH: usize = 10_000;

        // Build deeply nested structure: (a (a (a ... (a ())...)))
        let mut value = MettaValue::Unit();
        for _ in 0..DEPTH {
            value = MettaValue::SExpr(vec![MettaValue::Atom("a".to_string()), value]);
        }

        // preprocess_space_refs should complete without stack overflow
        let result = preprocess_space_refs(vec![value.clone()]);
        assert_eq!(result.len(), 1);

        // pattern_specificity should complete without stack overflow
        let specificity = pattern_specificity(&value);
        assert_eq!(specificity, 0); // No variables, all atoms

        // friendly_value_repr should complete without stack overflow
        let repr = friendly_value_repr(&value);
        assert!(repr.starts_with("(a (a"));
        // The deepest nesting contains "()" with closing parens
        assert!(repr.contains("()"));
        assert!(repr.ends_with(')'));
    }

    /// Test preprocess_space_refs combines `& name` into `&name`
    #[test]
    fn test_preprocess_space_refs_combines_ampersand() {
        let items = vec![
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
        ];
        let result = preprocess_space_refs(items);
        assert_eq!(result.len(), 1);
        if let MettaValueInner::Atom(s) = result[0].inner() {
            assert_eq!(s, "&self");
        } else {
            panic!("Expected Atom");
        }
    }

    /// Test preprocess_space_refs handles nested SExprs
    #[test]
    fn test_preprocess_space_refs_nested() {
        let items = vec![MettaValue::SExpr(vec![
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("kb".to_string()),
        ])];
        let result = preprocess_space_refs(items);
        assert_eq!(result.len(), 1);
        if let MettaValueInner::SExpr(inner) = result[0].inner() {
            assert_eq!(inner.len(), 1);
            if let MettaValueInner::Atom(s) = inner[0].inner() {
                assert_eq!(s, "&kb");
            } else {
                panic!("Expected Atom");
            }
        } else {
            panic!("Expected SExpr");
        }
    }

    /// Test pattern_specificity counts variables correctly
    #[test]
    fn test_pattern_specificity_variables() {
        // Variable patterns should have high specificity (1000 per variable)
        let var = MettaValue::Atom("$x".to_string());
        assert_eq!(pattern_specificity(&var), 1000);

        // Wildcard should have high specificity
        let wildcard = MettaValue::Atom("_".to_string());
        assert_eq!(pattern_specificity(&wildcard), 1000);

        // Literal should have 0 specificity
        let literal = MettaValue::Atom("foo".to_string());
        assert_eq!(pattern_specificity(&literal), 0);

        // Standalone "&" should have 0 specificity (it's an operator, not a variable)
        let ampersand = MettaValue::Atom("&".to_string());
        assert_eq!(pattern_specificity(&ampersand), 0);

        // Space reference variable should have high specificity
        let space_var = MettaValue::Atom("&x".to_string());
        assert_eq!(pattern_specificity(&space_var), 1000);

        // Nested expression with variables
        let nested = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        assert_eq!(pattern_specificity(&nested), 2000);
    }

    /// Test friendly_value_repr produces correct output
    #[test]
    fn test_friendly_value_repr() {
        // Atoms
        assert_eq!(friendly_value_repr(&MettaValue::Atom("foo".to_string())), "foo");

        // Numbers
        assert_eq!(friendly_value_repr(&MettaValue::Long(42)), "42");
        assert_eq!(friendly_value_repr(&MettaValue::Float(3.14)), "3.14");

        // Booleans
        assert_eq!(friendly_value_repr(&MettaValue::Bool(true)), "True");
        assert_eq!(friendly_value_repr(&MettaValue::Bool(false)), "False");

        // Strings
        assert_eq!(
            friendly_value_repr(&MettaValue::String("hello".to_string())),
            "\"hello\""
        );

        // Special values
        assert_eq!(friendly_value_repr(&MettaValue::Unit()), "()");
        assert_eq!(friendly_value_repr(&MettaValue::Empty()), "Empty");

        // S-expressions
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert_eq!(friendly_value_repr(&sexpr), "(foo 1 2)");

        // Empty S-expression
        let empty_sexpr = MettaValue::SExpr(vec![]);
        assert_eq!(friendly_value_repr(&empty_sexpr), "()");

        // Conjunction
        let conj = MettaValue::Conjunction(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        assert_eq!(friendly_value_repr(&conj), "(, a b)");

        // Empty conjunction
        let empty_conj = MettaValue::Conjunction(vec![]);
        assert_eq!(friendly_value_repr(&empty_conj), "(,)");
    }
}
