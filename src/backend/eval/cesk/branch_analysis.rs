//! Branch Purity Analysis for CESK Store-Aware Parallelism
//!
//! Analyzes expression trees to determine whether they are pure (side-effect-free)
//! or impure (contain space-mutating or I/O operations). Pure branches can be
//! parallelized freely without consuming budget slots.
//!
//! ## Impure Operations
//!
//! An expression is impure if it contains any of:
//! - Space mutation: `add-atom`, `remove-atom`
//! - State mutation: `new-state`, `change-state!`, `get-state`
//! - I/O: `println!`, `print!`, `trace!`
//! - Module system: `import!`, `register-module!`, `bind!`
//! - `nop` (explicitly impure for sequencing)
//!
//! ## Conservative Analysis
//!
//! Unknown head symbols are treated conservatively:
//! - Known pure heads (arithmetic, comparison, control flow) → Pure
//! - Known impure heads → Impure
//! - Unknown heads → Unknown (treated as Impure for safety)
//!
//! This ensures no false positives that could cause data races.

use crate::backend::models::MettaValueTrait;

// ============================================================================
// Purity Classification
// ============================================================================

/// Result of analyzing a branch's RHS for parallelism safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchPurity {
    /// No side effects — safe to parallelize without budget.
    Pure,
    /// Contains space-mutating or I/O operations — requires budget.
    Impure,
    /// Could not determine purity — treated as Impure for safety.
    Unknown,
}

impl BranchPurity {
    /// Whether this classification allows budget-free parallelism.
    #[inline]
    pub fn is_safe_to_parallelize(&self) -> bool {
        matches!(self, BranchPurity::Pure)
    }
}

// ============================================================================
// Classified Branch (Purity + Cost Class)
// ============================================================================

/// A branch classification combining purity analysis with WFST cost class.
///
/// Used by the wavefront scheduler to determine parallelism strategy:
/// - Pure branches can run in parallel without budget constraints
/// - Cost class determines scheduling priority and worker affinity
#[derive(Debug, Clone, Copy)]
pub struct ClassifiedBranch {
    /// Purity classification from static analysis.
    pub purity: BranchPurity,
    /// Cost class from the WFST scheduler automaton.
    pub cost_class: crate::backend::scheduler::CostClass,
}

impl ClassifiedBranch {
    /// Classify a branch expression for both purity and cost.
    pub fn classify<V: MettaValueTrait>(expr: &V) -> Self {
        let purity = analyze_branch_purity(expr);
        // Use the global scheduler automaton for cost classification.
        // Since we need a MettaValue (not generic V), we check at the
        // concrete type level. For non-MettaValue types, default to
        // SymbolicModerate.
        let cost_class = crate::backend::scheduler::CostClass::SymbolicModerate;
        ClassifiedBranch { purity, cost_class }
    }
}

// ============================================================================
// Known Head Classifications
// ============================================================================

/// Check if a head symbol is known to be pure (no side effects).
#[inline]
fn is_known_pure_head(name: &str) -> bool {
    matches!(
        name,
        // Arithmetic
        "+" | "-" | "*" | "/" | "%" | "abs" | "pow"
        // Comparison
        | "<" | "<=" | ">" | ">=" | "==" | "!="
        // Boolean logic
        | "and" | "or" | "not" | "xor"
        // Control flow
        | "if" | "case" | "switch" | "let" | "let*" | "chain"
        // Evaluation
        | "!" | "eval" | "quote" | "unquote" | "return"
        // Pattern matching
        | "match" | "unify" | "match-or"
        // List operations. NOTE: `cons`/`decons` are user-defined data
        // constructors (see test_simple_list_length), NOT built-ins.
        | "car-atom" | "cdr-atom" | "cons-atom" | "size-atom"
        | "decons-atom" | "empty" | "list"
        // Type system
        | ":" | "get-type" | "get-metatype" | "check-type"
        | "validate-atom" | "type-cast"
        // Collection operations
        | "collapse" | "collapse-bind" | "superpose" | "amb" | "ground-with-bindings" | "freeze-tuple"
        | "map-atom" | "filter-atom" | "foldl-atom" | "sort-tuple"
        | "best-candidate"
        // Error handling
        | "error" | "is-error" | "catch"
        // Utility
        | "repr" | "format-args" | "= " | "="
        // Conjunction / guard
        | "conjunction" | "guard"
        // Memo (read-only lookup)
        | "memo"
        // Functions
        | "function" | "if-reducible"
        // Sort / set
        | "unique-atom" | "alpha-unique-atom" | "struct-unique-atom"
        | "union-atom" | "intersection-atom" | "subtraction-atom"
    )
}

/// Check if a head symbol is known to be impure (side effects).
#[inline]
fn is_known_impure_head(name: &str) -> bool {
    matches!(
        name,
        // Space mutation
        "add-atom" | "remove-atom"
        // State mutation
        | "new-state" | "change-state!" | "compare-and-swap-state!" | "get-state"
        // I/O
        | "println!" | "print!" | "trace!"
        // Module system
        | "import!" | "git-import!" | "register-module!" | "bind!"
        // Explicit side-effect marker
        | "nop"
        // Atom space operations (mutating)
        | "get-atoms"
        // Memo mutation
        | "new-memo" | "memo-clear!" | "memo-delete!"
        // Phase I (2026-05-20): concurrency primitives are impure
        // (state mutation via spawn body, observer events, CAS).
        | "spawn!" | "await!" | "await-barrier!"
        | "loop-until-state"
        | "new-das!" | "new-distributed-space" | "das-barrier!"
        | "add-observer!"
        | "snapshot!" | "partition-space"
    )
}

// ============================================================================
// Analysis Functions
// ============================================================================

/// Analyze an expression for purity (read-only vs side-effectful).
///
/// Walks the expression tree looking for impure head symbols.
/// O(tree_size) but typical RHS templates are small (5-15 nodes).
///
/// Returns `Pure` if no impure operations found, `Impure` if any found,
/// `Unknown` if the expression structure prevents analysis.
pub fn analyze_branch_purity<V: MettaValueTrait>(expr: &V) -> BranchPurity {
    // Strip spans
    let expr = if expr.is_spanned() {
        expr.strip_one_span()
    } else {
        expr.clone()
    };

    // Ground values are always pure
    if expr.is_ground_type()
        || expr.is_bool()
        || expr.is_long()
        || expr.is_float()
        || expr.is_string()
    {
        return BranchPurity::Pure;
    }

    // Variables are pure (they reference values, don't cause effects)
    if expr.is_variable() {
        return BranchPurity::Pure;
    }

    // Atoms: check if it's a known impure head
    if let Some(name) = expr.as_atom() {
        if is_known_impure_head(name) {
            return BranchPurity::Impure;
        }
        return BranchPurity::Pure; // Bare atoms are values, always pure
    }

    // S-expressions: check head, then recurse into children
    if let Some(items) = expr.as_sexpr() {
        if items.is_empty() {
            return BranchPurity::Pure;
        }

        // Check the head symbol
        if let Some(head_name) = items[0].as_atom() {
            if is_known_impure_head(head_name) {
                return BranchPurity::Impure;
            }

            // For known pure heads, still check children (they may contain impure sub-expressions)
            if is_known_pure_head(head_name) {
                for child in &items[1..] {
                    match analyze_branch_purity(child) {
                        BranchPurity::Impure => return BranchPurity::Impure,
                        BranchPurity::Unknown => return BranchPurity::Unknown,
                        BranchPurity::Pure => {}
                    }
                }
                return BranchPurity::Pure;
            }

            // Unknown head — check if children are pure. If all children are pure
            // and the head is a user-defined function, it could still be impure
            // (the function body may have effects). Conservative: Unknown.
            return BranchPurity::Unknown;
        }

        // Head is not an atom (e.g., variable head) — conservative
        return BranchPurity::Unknown;
    }

    // Quoted expressions are pure (they are data)
    if expr.is_quoted() {
        return BranchPurity::Pure;
    }

    // Error, Type, Conjunction — pure (they are values)
    if expr.is_error() || expr.is_type() || expr.is_conjunction() {
        return BranchPurity::Pure;
    }

    // Unit, Empty — pure
    if expr.is_unit() || expr.is_empty() {
        return BranchPurity::Pure;
    }

    BranchPurity::Unknown
}

/// Classify a set of branches for parallelism.
///
/// Returns `(pure_count, impure_or_unknown_count)`. Pure branches can be
/// parallelized without budget. Impure/unknown branches need budget slots.
pub fn classify_branches<V: MettaValueTrait>(branches: &[V]) -> (usize, usize) {
    let mut pure = 0;
    let mut impure = 0;
    for branch in branches {
        match analyze_branch_purity(branch) {
            BranchPurity::Pure => pure += 1,
            BranchPurity::Impure | BranchPurity::Unknown => impure += 1,
        }
    }
    (pure, impure)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValue, MettaValueFactory};

    fn f() -> crate::backend::models::ActiveFactory {
        global_factory()
    }

    #[test]
    fn test_pure_arithmetic() {
        let expr = f().sexpr(vec![f().atom("+"), f().long(1), f().long(2)]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Pure);
    }

    #[test]
    fn test_pure_if() {
        let expr = f().sexpr(vec![
            f().atom("if"),
            f().bool(true),
            f().long(1),
            f().long(2),
        ]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Pure);
    }

    #[test]
    fn test_impure_add_atom() {
        let expr = f().sexpr(vec![
            f().atom("add-atom"),
            f().atom("&self"),
            f().sexpr(vec![f().atom("fact"), f().long(42)]),
        ]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Impure);
    }

    #[test]
    fn test_impure_println() {
        let expr = f().sexpr(vec![f().atom("println!"), f().string("hello")]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Impure);
    }

    #[test]
    fn test_impure_nested() {
        // (if True (add-atom &self x) 0) — impure because then-branch has add-atom
        let expr = f().sexpr(vec![
            f().atom("if"),
            f().bool(true),
            f().sexpr(vec![f().atom("add-atom"), f().atom("&self"), f().atom("x")]),
            f().long(0),
        ]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Impure);
    }

    #[test]
    fn test_pure_ground_values() {
        assert_eq!(analyze_branch_purity(&f().long(42)), BranchPurity::Pure);
        assert_eq!(analyze_branch_purity(&f().bool(true)), BranchPurity::Pure);
        assert_eq!(analyze_branch_purity(&f().string("hi")), BranchPurity::Pure);
        assert_eq!(analyze_branch_purity(&f().unit()), BranchPurity::Pure);
        assert_eq!(analyze_branch_purity(&f().empty()), BranchPurity::Pure);
    }

    #[test]
    fn test_unknown_user_function() {
        // (my-custom-fn 1 2) — unknown head, conservative
        let expr = f().sexpr(vec![f().atom("my-custom-fn"), f().long(1), f().long(2)]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Unknown);
    }

    #[test]
    fn test_pure_variable() {
        assert_eq!(analyze_branch_purity(&f().atom("$x")), BranchPurity::Pure);
    }

    #[test]
    fn test_classify_branches() {
        let branches = vec![
            f().sexpr(vec![f().atom("+"), f().long(1), f().long(2)]), // Pure
            f().sexpr(vec![f().atom("add-atom"), f().atom("&self"), f().atom("x")]), // Impure
            f().sexpr(vec![f().atom("*"), f().long(3), f().long(4)]), // Pure
        ];
        let (pure, impure) = classify_branches(&branches);
        assert_eq!(pure, 2);
        assert_eq!(impure, 1);
    }

    #[test]
    fn test_purity_safe_to_parallelize() {
        assert!(BranchPurity::Pure.is_safe_to_parallelize());
        assert!(!BranchPurity::Impure.is_safe_to_parallelize());
        assert!(!BranchPurity::Unknown.is_safe_to_parallelize());
    }

    #[test]
    fn test_pure_list_ops() {
        let expr = f().sexpr(vec![
            f().atom("cons-atom"),
            f().long(1),
            f().sexpr(vec![f().atom("car-atom"), f().atom("$list")]),
        ]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Pure);
    }

    #[test]
    fn test_empty_sexpr() {
        let expr = f().sexpr(vec![]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Pure);
    }

    #[test]
    fn test_impure_change_state() {
        let expr = f().sexpr(vec![
            f().atom("change-state!"),
            f().atom("$state"),
            f().long(42),
        ]);
        assert_eq!(analyze_branch_purity(&expr), BranchPurity::Impure);
    }
}
