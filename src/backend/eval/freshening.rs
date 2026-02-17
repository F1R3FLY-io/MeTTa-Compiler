//! Variable Freshening for Space Operations
//!
//! Provides variable freshening (alpha-renaming) for MeTTa values to prevent
//! variable capture during bidirectional matching and get-atoms operations.
//!
//! ## MeTTa HE Semantics
//!
//! MeTTa HE calls `make_variables_unique()` on stored atoms before matching and
//! when returning atoms via `get-atoms`. This ensures cross-atom variable isolation:
//! two stored atoms `(foo $x)` and `(bar $x)` get independent freshened names so
//! binding `$x` in one doesn't affect the other.
//!
//! ## Implementation
//!
//! Uses the same iterative work-stack pattern as `seal_variables_iterative_generic`
//! from `bindings_generic.rs`. Variables (`$`-prefixed atoms) are renamed to
//! `$__fr_{epoch}_{name}` where epoch is a globally unique counter. Non-variable
//! atoms (`&self`, `&kb`, `&stack`, literals) pass through unchanged.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Global counter for freshening epochs. Each call to `freshen_variables_generic`
/// gets a unique epoch to ensure cross-call variable isolation.
static FRESHEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Work items for the iterative freshening algorithm.
enum FreshenWork<'a, V> {
    /// Process a value (check if variable, recurse into compounds)
    Process(&'a V),
    /// Build an S-expression from the top `count` items on the result stack
    BuildSExpr(usize),
    /// Build a conjunction from the top `count` items on the result stack
    BuildConjunction(usize),
}

/// Freshen (alpha-rename) all variables in a value.
///
/// Renames `$x` → `$__fr_{epoch}_x` for all `$`-prefixed atoms. Non-variable
/// atoms (including `&self`, `&kb`, `&stack`, `_` wildcards) pass through unchanged.
///
/// ## Fast Path
///
/// Returns the value unchanged (no allocation) if it contains no variables.
/// Use `has_variables_generic()` from `mork_forms_generic.rs` to pre-check.
///
/// ## Epoch Isolation
///
/// Each call uses a unique epoch from `FRESHEN_COUNTER`, ensuring that variables
/// freshened in separate calls get distinct names even if the original names match.
pub fn freshen_variables_generic<V, F>(value: &V, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let epoch = FRESHEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    freshen_with_epoch(value, epoch, factory)
}

/// Freshen variables with a specific epoch (used internally and by tests).
fn freshen_with_epoch<V, F>(value: &V, epoch: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let mut work_stack: Vec<FreshenWork<V>> = Vec::with_capacity(32);
    let mut result_stack: Vec<V> = Vec::with_capacity(32);

    work_stack.push(FreshenWork::Process(value));

    while let Some(work) = work_stack.pop() {
        match work {
            FreshenWork::Process(val) => {
                if let Some(name) = val.as_atom() {
                    if name.starts_with('$') && name != "_" {
                        // Rename $varname → $__fr_{epoch}_{varname_without_dollar}
                        let bare_name = &name[1..]; // strip leading '$'
                        result_stack.push(factory.atom(&format!("$__fr_{}_{}", epoch, bare_name)));
                    } else {
                        // Non-variable atom: pass through unchanged
                        // (includes &self, &kb, &stack, _, literals, etc.)
                        result_stack.push(val.clone());
                    }
                } else if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(FreshenWork::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(FreshenWork::Process(item));
                        }
                    }
                } else if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val.clone());
                    } else {
                        work_stack.push(FreshenWork::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(FreshenWork::Process(goal));
                        }
                    }
                } else {
                    // Ground types (Bool, Long, Float, String, Unit, Space, State, etc.)
                    result_stack.push(val.clone());
                }
            }
            FreshenWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.sexpr(children));
            }
            FreshenWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.conjunction(children));
            }
        }
    }

    result_stack.pop().expect("Result stack should not be empty after freshening")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValueInner};

    fn factory() -> GcFactory {
        GcFactory::default()
    }

    #[test]
    fn test_ground_value_unchanged() {
        let f = factory();
        let val = f.long(42);
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result, val, "Ground values should pass through unchanged");
    }

    #[test]
    fn test_unit_unchanged() {
        let f = factory();
        let val = f.unit();
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result, val);
    }

    #[test]
    fn test_literal_atom_unchanged() {
        let f = factory();
        let val = f.atom("foo");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("foo"));
    }

    #[test]
    fn test_space_ref_unchanged() {
        let f = factory();
        let val = f.atom("&self");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("&self"), "&self should not be freshened");
    }

    #[test]
    fn test_wildcard_unchanged() {
        let f = factory();
        let val = f.atom("_");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("_"), "Wildcards should not be freshened");
    }

    #[test]
    fn test_single_variable_freshened() {
        let f = factory();
        let val = f.atom("$x");
        let result = freshen_with_epoch(&val, 42, &f);
        assert_eq!(result.as_atom(), Some("$__fr_42_x"));
    }

    #[test]
    fn test_nested_sexpr_freshened() {
        let f = factory();
        let val = f.sexpr(vec![
            f.atom("foo"),
            f.atom("$x"),
            f.sexpr(vec![f.atom("bar"), f.atom("$y")]),
        ]);
        let result = freshen_with_epoch(&val, 7, &f);

        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items[0].as_atom(), Some("foo"));
        assert_eq!(items[1].as_atom(), Some("$__fr_7_x"));
        let inner = items[2].as_sexpr().expect("should be inner sexpr");
        assert_eq!(inner[0].as_atom(), Some("bar"));
        assert_eq!(inner[1].as_atom(), Some("$__fr_7_y"));
    }

    #[test]
    fn test_repeated_vars_same_epoch() {
        let f = factory();
        let val = f.sexpr(vec![f.atom("$x"), f.atom("$x")]);
        let result = freshen_with_epoch(&val, 5, &f);

        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items[0].as_atom(), Some("$__fr_5_x"));
        assert_eq!(items[1].as_atom(), Some("$__fr_5_x"));
        // Same variable name → same freshened name within the same epoch
        assert_eq!(items[0], items[1]);
    }

    #[test]
    fn test_different_epochs_different_names() {
        let f = factory();
        let val = f.atom("$x");
        let r1 = freshen_with_epoch(&val, 10, &f);
        let r2 = freshen_with_epoch(&val, 11, &f);
        assert_ne!(r1, r2, "Different epochs should produce different names");
        assert_eq!(r1.as_atom(), Some("$__fr_10_x"));
        assert_eq!(r2.as_atom(), Some("$__fr_11_x"));
    }

    #[test]
    fn test_global_counter_increments() {
        let f = factory();
        let val = f.atom("$z");
        let r1 = freshen_variables_generic(&val, &f);
        let r2 = freshen_variables_generic(&val, &f);
        // Each call should use a different epoch
        assert_ne!(r1, r2, "Consecutive calls should use different epochs");
    }

    #[test]
    fn test_ampersand_variable_not_freshened() {
        let f = factory();
        // &kb and &stack are space references, not variables to freshen
        let val = f.atom("&kb");
        let result = freshen_with_epoch(&val, 99, &f);
        assert_eq!(result.as_atom(), Some("&kb"));
    }

    #[test]
    fn test_empty_sexpr_unchanged() {
        let f = factory();
        let val = f.unit();
        let result = freshen_with_epoch(&val, 99, &f);
        assert!(matches!(result.inner(), MettaValueInner::Unit));
    }
}
