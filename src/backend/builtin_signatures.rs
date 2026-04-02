//! Type signatures for MeTTa built-in operations
//!
//! This module provides a central registry of type signatures for all built-in
//! operations in MeTTa. These signatures are used for:
//! - Arity validation during fuzzy matching
//! - Type compatibility checking for smart suggestions
//! - Return type inference for deep type analysis
//!
//! # Three Pillars of Smart Recommendations
//!
//! Recommendations must satisfy ALL THREE criteria:
//! 1. **Context Compatibility** - Position determines valid recommendations
//! 2. **Type Compatibility** - Use infer_type() and compare against expected types
//! 3. **Arity Compatibility** - Expression arity must fall within min/max range

use std::collections::HashMap;
use std::sync::LazyLock;

/// Type expression for built-in signatures
///
/// Represents the type system used for validating fuzzy match suggestions.
/// Supports concrete types, structural types, and polymorphic type variables.
#[derive(Clone, PartialEq, Debug)]
pub enum TypeExpr {
    // Concrete types
    /// Numeric type (integers and floats)
    Number,
    /// Boolean type (True/False)
    Bool,
    /// String type (double-quoted)
    String,
    /// Atom type (symbols/meta-type: unevaluated)
    Atom,
    /// Expression type (S-expressions, meta-type: unevaluated)
    Expression,
    /// Variable type (meta-type)
    Variable,
    /// Space type (named spaces like &self)
    Space,
    /// State type (mutable state) — legacy, use StateMonad for HE
    State,
    /// StateMonad type with element type: (StateMonad $t)
    StateMonad(Box<TypeExpr>),
    /// Unit type (empty result)
    Unit,
    /// Error type (HE: ErrorType)
    Error,
    /// Type type (type expressions themselves)
    Type,
    /// %Undefined% — universal match type (matches any type)
    Undefined,
    /// Grounded type
    Grounded,

    // Structural types
    /// List type with element type: (List $a)
    List(Box<TypeExpr>),
    /// Arrow/function type: (-> T1 T2 ... Tret)
    Arrow(Vec<TypeExpr>, Box<TypeExpr>),

    // Type variables for polymorphism
    /// Type variable for polymorphic types: $a, $b, etc.
    Var(&'static str),

    // Special markers
    /// Accepts anything (wildcard)
    Any,
    /// Pattern context (may contain $vars)
    Pattern,
    /// Let* binding list: ((var1 val1) (var2 val2) ...)
    Bindings,
    /// Expression that will be evaluated (for quote/eval)
    Expr,
    /// IO monad type with result type: (IO $t)
    /// Marks operations that perform observable side effects (output, tracing).
    /// IO is a compile-time type marker — at runtime, IO-producing operations
    /// return their inner value directly. The IO wrapper tells the caching
    /// system "don't cache expressions that produce this type."
    IO(Box<TypeExpr>),
}

/// Helper to create arrow types more concisely
fn arrow(args: Vec<TypeExpr>, ret: TypeExpr) -> TypeExpr {
    TypeExpr::Arrow(args, Box::new(ret))
}

/// Helper to create list types (used in tests for TypeExpr::List variants)
#[cfg(test)]
fn list(elem: TypeExpr) -> TypeExpr {
    TypeExpr::List(Box::new(elem))
}

/// Helper to create StateMonad types
fn state_monad(elem: TypeExpr) -> TypeExpr {
    TypeExpr::StateMonad(Box::new(elem))
}

/// Helper to create IO monad types
fn io(elem: TypeExpr) -> TypeExpr {
    TypeExpr::IO(Box::new(elem))
}

/// Signature definition for a built-in operation
///
/// Contains the name, arity bounds, and full type signature for type inference.
#[derive(Clone, Debug)]
pub struct BuiltinSignature {
    /// The operation name (e.g., "+", "let", "match")
    pub name: &'static str,
    /// Minimum required arity (number of arguments)
    pub min_arity: usize,
    /// Maximum allowed arity
    pub max_arity: usize,
    /// Full arrow type signature: (-> arg1_type arg2_type ... return_type)
    pub type_sig: TypeExpr,
}

/// Lazy-initialized registry with full type signatures for all MeTTa built-ins
///
/// This contains signatures for:
/// - Arithmetic operators (+, -, *, /)
/// - Comparison operators (<, <=, >, >=, ==, !=)
/// - Control flow (if, case, switch)
/// - Binding forms (let, let*, unify)
/// - Space operations (match, add-atom, get-atoms, etc.)
/// - List operations (car-atom, cdr-atom, cons-atom, etc.)
/// - Type operations (:, get-type, check-type, get-metatype)
/// - Error handling (error, is-error, catch)
/// - Evaluation (!, eval, quote)
/// - State operations (new-state, get-state, change-state!)
/// - I/O and debugging (println!, trace!, repr, format-args)
/// - Module system (include, bind!)
static BUILTIN_SIGNATURES: LazyLock<Vec<BuiltinSignature>> = LazyLock::new(|| {
    use TypeExpr::*;

    vec![
        // ====================================================================
        // Arithmetic operators: (-> Number Number Number)
        // ====================================================================
        BuiltinSignature { name: "+", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "-", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "*", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "/", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "%", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "min", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "max", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "/safe", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "clamp", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Number, Number, Number], Number) },
        BuiltinSignature { name: "floor-div", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        // Unary math
        BuiltinSignature { name: "abs", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "abs-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "floor", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "floor-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "ceil", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "ceil-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "round", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "round-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "sqrt", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "sqrt-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "trunc", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "trunc-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        // Binary math
        BuiltinSignature { name: "pow", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "pow-math", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "log", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        BuiltinSignature { name: "log-math", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number) },
        // ====================================================================
        // Trigonometric functions: (-> Number Number)
        // ====================================================================
        BuiltinSignature { name: "sin-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "cos-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "tan-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "asin-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "acos-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        BuiltinSignature { name: "atan-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Number) },
        // ====================================================================
        // Float classification: (-> Number Bool)
        // ====================================================================
        BuiltinSignature { name: "isnan-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Bool) },
        BuiltinSignature { name: "isinf-math", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Number], Bool) },
        // ====================================================================
        // Comparison operators: (-> Number Number Bool)
        // ====================================================================
        BuiltinSignature { name: "<", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool) },
        BuiltinSignature { name: "<=", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool) },
        BuiltinSignature { name: ">", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool) },
        BuiltinSignature { name: ">=", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool) },
        // ====================================================================
        // Equality operators: polymorphic (-> $a $a Bool)
        // ====================================================================
        BuiltinSignature { name: "==", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Var("a"), Var("a")], Bool) },
        BuiltinSignature { name: "!=", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Var("a"), Var("a")], Bool) },
        // ====================================================================
        // Boolean operators
        // ====================================================================
        BuiltinSignature { name: "and", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Bool, Bool], Bool) },
        BuiltinSignature { name: "or", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Bool, Bool], Bool) },
        BuiltinSignature { name: "not", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Bool], Bool) },
        BuiltinSignature { name: "xor", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Bool, Bool], Bool) },
        // ====================================================================
        // Control flow (HE-aligned: lazy args use Atom meta-type)
        // ====================================================================
        // if: (-> Bool Atom Atom $t) — then/else are Atom (lazy, unevaluated)
        BuiltinSignature { name: "if", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Bool, Atom, Atom], Var("t")) },
        // case: (-> Atom Expression %Undefined%)
        BuiltinSignature { name: "case", min_arity: 2, max_arity: usize::MAX,
            type_sig: arrow(vec![Atom, Expression], Undefined) },
        // switch: (-> %Undefined% Expression %Undefined%)
        BuiltinSignature { name: "switch", min_arity: 2, max_arity: usize::MAX,
            type_sig: arrow(vec![Undefined, Expression], Undefined) },
        // if-equal: (-> Atom Atom Atom Atom %Undefined%)
        BuiltinSignature { name: "if-equal", min_arity: 4, max_arity: 4,
            type_sig: arrow(vec![Atom, Atom, Atom, Atom], Undefined) },
        // if-reducible: (-> Atom Atom Atom %Undefined%)
        BuiltinSignature { name: "if-reducible", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Atom, Atom], Undefined) },
        // ====================================================================
        // Binding forms (HE-aligned)
        // ====================================================================
        // let: (-> Atom %Undefined% Atom %Undefined%)
        BuiltinSignature { name: "let", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Undefined, Atom], Undefined) },
        // let*: (-> Bindings $a $a)
        BuiltinSignature { name: "let*", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Bindings, Var("a")], Var("a")) },
        // unify: (-> Atom Atom Atom Atom %Undefined%)
        BuiltinSignature { name: "unify", min_arity: 4, max_arity: 4,
            type_sig: arrow(vec![Atom, Atom, Atom, Atom], Undefined) },
        // function: (-> Atom Atom) — HE uses Atom meta-type
        BuiltinSignature { name: "function", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        // return: (-> $t $t)
        BuiltinSignature { name: "return", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Var("t")], Var("t")) },
        // chain: (-> Atom Variable Atom %Undefined%)
        BuiltinSignature { name: "chain", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Variable, Atom], Undefined) },
        // ====================================================================
        // Rule & Type definitions (HE-aligned)
        // ====================================================================
        // =: (-> $t $t %Undefined%)
        BuiltinSignature { name: "=", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Var("t"), Var("t")], Undefined) },
        // :: (-> $a Type Unit)
        BuiltinSignature { name: ":", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Var("a"), Type], Unit) },
        // ====================================================================
        // Space operations (HE-aligned)
        // ====================================================================
        // match: (-> SpaceType Atom Atom %Undefined%) — 3 args per HE
        BuiltinSignature { name: "match", min_arity: 3, max_arity: 4,
            type_sig: arrow(vec![Space, Atom, Atom], Undefined) },
        // match-or: MeTTaTron extension
        BuiltinSignature { name: "match-or", min_arity: 4, max_arity: 4,
            type_sig: arrow(vec![Space, Atom, Atom, Atom], Undefined) },
        // new-space: (-> SpaceType)
        BuiltinSignature { name: "new-space", min_arity: 0, max_arity: 0,
            type_sig: arrow(vec![], Space) },
        // add-atom: (-> SpaceType Atom Unit) — atom arg is Atom (unevaluated)
        BuiltinSignature { name: "add-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Space, Atom], Unit) },
        // remove-atom: (-> SpaceType Atom Unit)
        BuiltinSignature { name: "remove-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Space, Atom], Unit) },
        // get-atoms: (-> SpaceType Atom) — HE returns Atom, not List
        BuiltinSignature { name: "get-atoms", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Space], Atom) },
        // collapse: (-> Atom Atom) — HE: Atom → Atom
        BuiltinSignature { name: "collapse", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        // collapse-bind: (-> Atom Expression)
        BuiltinSignature { name: "collapse-bind", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Expression) },
        // ====================================================================
        // List/Expression operations (HE-aligned: Expression, not List)
        // ====================================================================
        // car-atom: (-> Expression %Undefined%)
        BuiltinSignature { name: "car-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Undefined) },
        // cdr-atom: (-> Expression Expression)
        BuiltinSignature { name: "cdr-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Expression) },
        // cons-atom: (-> Atom Expression Atom) — HE signature
        BuiltinSignature { name: "cons-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Expression], Atom) },
        // decons-atom: (-> Expression Atom)
        BuiltinSignature { name: "decons-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Atom) },
        // size-atom: (-> Expression Number)
        BuiltinSignature { name: "size-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Number) },
        // max-atom: (-> Expression Number)
        BuiltinSignature { name: "max-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Number) },
        // min-atom: (-> Expression Number)
        BuiltinSignature { name: "min-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Number) },
        // index-atom: (-> Expression Number Atom)
        BuiltinSignature { name: "index-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Number], Atom) },
        // empty: zero results (not a function returning a list)
        BuiltinSignature { name: "empty", min_arity: 0, max_arity: 0,
            type_sig: arrow(vec![], Undefined) },
        // MeTTaTron extensions
        BuiltinSignature { name: "tuple-concat", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Expression], Expression) },
        BuiltinSignature { name: "tuple-count", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Number) },
        BuiltinSignature { name: "without", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Atom], Expression) },
        BuiltinSignature { name: "element-of", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Expression], Bool) },
        BuiltinSignature { name: "range", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Number, Number], Expression) },
        BuiltinSignature { name: "reverse-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Expression) },
        BuiltinSignature { name: "flatten-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Expression) },
        BuiltinSignature { name: "zip-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Expression], Expression) },
        BuiltinSignature { name: "take-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Number], Expression) },
        BuiltinSignature { name: "drop-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Number], Expression) },
        // ====================================================================
        // Higher-order list operations (HE-aligned: template-based, not arrow)
        // ====================================================================
        // map-atom: (-> Expression Variable Atom Expression)
        BuiltinSignature { name: "map-atom", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Expression, Variable, Atom], Expression) },
        // filter-atom: (-> Expression Variable Atom Expression)
        BuiltinSignature { name: "filter-atom", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Expression, Variable, Atom], Expression) },
        // foldl-atom: (-> Expression Atom Variable Variable Atom %Undefined%)
        BuiltinSignature { name: "foldl-atom", min_arity: 5, max_arity: 5,
            type_sig: arrow(vec![Expression, Atom, Variable, Variable, Atom], Undefined) },
        // MeTTaTron extensions
        BuiltinSignature { name: "sort-tuple", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Atom], Expression) },
        BuiltinSignature { name: "best-candidate", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Atom], Atom) },
        // ====================================================================
        // Set operations: (-> Expression ... Atom)
        // ====================================================================
        BuiltinSignature { name: "unique-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Atom) },
        BuiltinSignature { name: "union-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Expression], Atom) },
        BuiltinSignature { name: "intersection-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Expression], Atom) },
        BuiltinSignature { name: "subtraction-atom", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Expression], Atom) },
        // ====================================================================
        // Nondeterminism
        // ====================================================================
        // superpose: (-> Expression %Undefined%)
        BuiltinSignature { name: "superpose", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Expression], Undefined) },
        // MeTTaTron extensions
        BuiltinSignature { name: "amb", min_arity: 1, max_arity: usize::MAX,
            type_sig: arrow(vec![Atom], Undefined) },
        BuiltinSignature { name: "guard", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Bool, Atom], Atom) },
        BuiltinSignature { name: "commit", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        BuiltinSignature { name: "backtrack", min_arity: 0, max_arity: 0,
            type_sig: arrow(vec![], Undefined) },
        // ====================================================================
        // Quoting & Meta (HE-aligned)
        // ====================================================================
        // quote: (-> Atom Atom)
        BuiltinSignature { name: "quote", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        // unquote: (-> %Undefined% %Undefined%)
        BuiltinSignature { name: "unquote", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Undefined], Undefined) },
        // eval: (-> Atom Atom)
        BuiltinSignature { name: "eval", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        // !: (-> Atom Atom) — same as eval
        BuiltinSignature { name: "!", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        // sealed: (-> Expression Atom Atom)
        BuiltinSignature { name: "sealed", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Expression, Atom], Atom) },
        // atom-subst: (-> Atom Variable Atom Atom)
        BuiltinSignature { name: "atom-subst", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Variable, Atom], Atom) },
        // nop: (-> %Undefined% %Undefined%)
        BuiltinSignature { name: "nop", min_arity: 0, max_arity: usize::MAX,
            type_sig: arrow(vec![Undefined], Undefined) },
        // ====================================================================
        // Type operations
        // ====================================================================
        BuiltinSignature { name: "get-type", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Var("a")], Type) },
        BuiltinSignature { name: "check-type", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Var("a"), Type], Bool) },
        BuiltinSignature { name: "get-metatype", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Var("a")], Type) },
        // validate-atom (Phase 4)
        BuiltinSignature { name: "validate-atom", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Bool) },
        // get-type-space (Phase 5)
        BuiltinSignature { name: "get-type-space", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Space, Atom], Type) },
        // is-function: (-> Type Bool)
        BuiltinSignature { name: "is-function", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Type], Bool) },
        // type-cast: (-> $a Type Atom $a)
        BuiltinSignature { name: "type-cast", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Var("a"), Type, Atom], Var("a")) },
        // metta: (-> Atom Type SpaceType Atom) — interpreter operation (HE parity)
        BuiltinSignature { name: "metta", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Type, Atom], Atom) },
        // match-type-or: (-> Bool Atom Atom Bool) — fold helper for type matching
        BuiltinSignature { name: "match-type-or", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Bool, Atom, Atom], Bool) },
        // first-from-pair: (-> Atom Atom) — extract first element from pair
        BuiltinSignature { name: "first-from-pair", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Atom) },
        // :<: (-> Atom Atom Unit) — subtype declaration
        BuiltinSignature { name: ":<", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Atom) },
        // ====================================================================
        // Error handling (HE-aligned)
        // ====================================================================
        // Error: (-> Atom Atom ErrorType)
        BuiltinSignature { name: "Error", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Error) },
        // error: alias
        BuiltinSignature { name: "error", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Error) },
        // is-error: (-> $a Bool)
        BuiltinSignature { name: "is-error", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Var("a")], Bool) },
        // catch: (-> $a $a $a)
        BuiltinSignature { name: "catch", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Var("a"), Var("a")], Var("a")) },
        // ====================================================================
        // State operations (HE-aligned: StateMonad)
        // ====================================================================
        // new-state: (-> $t (StateMonad $t))
        BuiltinSignature { name: "new-state", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Var("t")], state_monad(Var("t"))) },
        // get-state: (-> (StateMonad $t) $t)
        BuiltinSignature { name: "get-state", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![state_monad(Var("t"))], Var("t")) },
        // change-state!: (-> (StateMonad $t) $t (StateMonad $t))
        BuiltinSignature { name: "change-state!", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![state_monad(Var("t")), Var("t")], state_monad(Var("t"))) },
        // ====================================================================
        // I/O and debugging (HE-aligned)
        // ====================================================================
        // println!: (-> %Undefined% Unit)
        // println!: (-> %Undefined% (IO Unit)) — observable output side effect
        BuiltinSignature { name: "println!", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Undefined], io(Unit)) },
        // trace!: (-> %Undefined% Atom (IO %Undefined%)) — stderr output side effect
        BuiltinSignature { name: "trace!", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Undefined, Atom], io(Undefined)) },
        // repr: (-> $a String)
        BuiltinSignature { name: "repr", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Var("a")], String) },
        // format-args: (-> String Expression String) — HE: second arg is Expression
        BuiltinSignature { name: "format-args", min_arity: 1, max_arity: usize::MAX,
            type_sig: arrow(vec![String, Expression], String) },
        // =alpha: (-> Atom Atom Bool)
        BuiltinSignature { name: "=alpha", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Bool) },
        // ====================================================================
        // Module system
        // ====================================================================
        BuiltinSignature { name: "include", min_arity: 1, max_arity: 2,
            type_sig: arrow(vec![Atom], Unit) },
        BuiltinSignature { name: "import!", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Space, Atom], Unit) },
        BuiltinSignature { name: "bind!", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Var("a")], Unit) },
        BuiltinSignature { name: "mod-space!", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Space) },
        BuiltinSignature { name: "print-mods!", min_arity: 0, max_arity: 0,
            type_sig: arrow(vec![], Unit) },
        BuiltinSignature { name: "pragma!", min_arity: 1, max_arity: usize::MAX,
            type_sig: arrow(vec![Undefined], Unit) },
        // ====================================================================
        // Memoization (MeTTaTron extension)
        // ====================================================================
        BuiltinSignature { name: "new-memo", min_arity: 0, max_arity: 0,
            type_sig: arrow(vec![], Atom) },
        BuiltinSignature { name: "memo", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Expression) },
        BuiltinSignature { name: "memo-first", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Atom) },
        BuiltinSignature { name: "clear-memo!", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Unit) },
        BuiltinSignature { name: "memo-stats", min_arity: 1, max_arity: 1,
            type_sig: arrow(vec![Atom], Expression) },
        // ====================================================================
        // Assertion/Testing
        // ====================================================================
        BuiltinSignature { name: "assertEqual", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Unit) },
        BuiltinSignature { name: "assertEqualMsg", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Atom, Atom], Unit) },
        BuiltinSignature { name: "assertAlphaEqual", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Unit) },
        BuiltinSignature { name: "assertAlphaEqualMsg", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Atom, Atom], Unit) },
        BuiltinSignature { name: "assertEqualToResult", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Unit) },
        BuiltinSignature { name: "assertEqualToResultMsg", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Atom, Atom], Unit) },
        BuiltinSignature { name: "assertAlphaEqualToResult", min_arity: 2, max_arity: 2,
            type_sig: arrow(vec![Atom, Atom], Unit) },
        BuiltinSignature { name: "assertAlphaEqualToResultMsg", min_arity: 3, max_arity: 3,
            type_sig: arrow(vec![Atom, Atom, Atom], Unit) },
    ]
});

/// Lazy-initialized hashmap for O(1) signature lookup
static SIGNATURE_MAP: LazyLock<HashMap<&'static str, &'static BuiltinSignature>> =
    LazyLock::new(|| {
        BUILTIN_SIGNATURES
            .iter()
            .map(|sig| (sig.name, sig))
            .collect()
    });

/// Get the signature for a built-in operation by name
///
/// Returns `Some(&BuiltinSignature)` if the operation is a known built-in,
/// `None` otherwise.
///
/// # Example
/// ```ignore
/// if let Some(sig) = get_signature("let") {
///     assert_eq!(sig.min_arity, 3);
///     assert_eq!(sig.max_arity, 3);
/// }
/// ```
pub fn get_signature(name: &str) -> Option<&'static BuiltinSignature> {
    SIGNATURE_MAP.get(name).copied()
}

/// Check if a name is a known built-in operation
pub fn is_builtin(name: &str) -> bool {
    SIGNATURE_MAP.contains_key(name)
}

/// Extract argument types from an arrow signature
///
/// Returns the argument types if the signature is an Arrow type,
/// `None` otherwise.
pub fn get_arg_types(sig: &TypeExpr) -> Option<&[TypeExpr]> {
    match sig {
        TypeExpr::Arrow(args, _) => Some(args),
        _ => None,
    }
}

/// Extract return type from an arrow signature
///
/// Returns the return type if the signature is an Arrow type,
/// `None` otherwise.
pub fn get_return_type(sig: &TypeExpr) -> Option<&TypeExpr> {
    match sig {
        TypeExpr::Arrow(_, ret) => Some(ret),
        _ => None,
    }
}

/// Get the expected type at a specific argument position
///
/// Returns `None` if:
/// - The signature is not an Arrow type
/// - The position is out of bounds for the signature's argument list
pub fn get_expected_type_at_position(sig: &BuiltinSignature, position: usize) -> Option<&TypeExpr> {
    get_arg_types(&sig.type_sig).and_then(|args| args.get(position))
}

/// Convert a `TypeExpr` to its expected_type atom name for branch pruning.
///
/// Returns `Some("Number")`, `Some("Bool")`, or `Some("String")` for concrete
/// value types. Returns `None` for meta-types (`Atom`, `Expression`, etc.),
/// polymorphic type variables, and structural types — these don't constrain
/// the result type enough to enable branch pruning.
pub fn type_expr_to_expected_type_name(te: &TypeExpr) -> Option<&'static str> {
    match te {
        TypeExpr::Number => Some("Number"),
        TypeExpr::Bool => Some("Bool"),
        TypeExpr::String => Some("String"),
        _ => None, // Meta-types/polymorphic/structural: don't constrain
    }
}

/// Get all built-in names (useful for fuzzy matching initialization)
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_SIGNATURES.iter().map(|sig| sig.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_signature_known() {
        let sig = get_signature("let").unwrap();
        assert_eq!(sig.name, "let");
        assert_eq!(sig.min_arity, 3);
        assert_eq!(sig.max_arity, 3);
    }

    #[test]
    fn test_get_signature_unknown() {
        assert!(get_signature("unknown_form").is_none());
        assert!(get_signature("lit").is_none()); // The problem case from issue #51
    }

    #[test]
    fn test_is_builtin() {
        assert!(is_builtin("+"));
        assert!(is_builtin("let"));
        assert!(is_builtin("match"));
        assert!(!is_builtin("lit"));
        assert!(!is_builtin("MyDataType"));
    }

    #[test]
    fn test_arity_let_vs_lit() {
        // This is the core issue #51 case: lit has 1 arg, let needs 3
        let sig = get_signature("let").unwrap();
        let lit_arity = 1; // (lit p) has 1 argument

        // lit's arity doesn't match let's requirements
        assert!(lit_arity < sig.min_arity);
    }

    #[test]
    fn test_arity_catch() {
        let sig = get_signature("catch").unwrap();
        assert_eq!(sig.min_arity, 2);
        assert_eq!(sig.max_arity, 2);

        // (cach e) has 1 arg, doesn't match catch's 2
        let cach_arity = 1;
        assert!(cach_arity < sig.min_arity);

        // (cach e d) has 2 args, matches catch
        let cach_arity_2 = 2;
        assert!(cach_arity_2 >= sig.min_arity && cach_arity_2 <= sig.max_arity);
    }

    #[test]
    fn test_get_arg_types() {
        let sig = get_signature("if").unwrap();
        let arg_types = get_arg_types(&sig.type_sig).unwrap();

        assert_eq!(arg_types.len(), 3);
        assert_eq!(arg_types[0], TypeExpr::Bool);
        // HE-aligned: then/else are Atom (lazy, unevaluated)
        assert_eq!(arg_types[1], TypeExpr::Atom);
        assert_eq!(arg_types[2], TypeExpr::Atom);
    }

    #[test]
    fn test_get_return_type() {
        let sig = get_signature("+").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Number);

        let sig = get_signature("==").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Bool);
    }

    #[test]
    fn test_space_operations() {
        let sig = get_signature("match").unwrap();
        let arg_types = get_arg_types(&sig.type_sig).unwrap();

        // First argument of match should be Space
        assert_eq!(arg_types[0], TypeExpr::Space);
    }

    #[test]
    fn test_variadic_operations() {
        let sig = get_signature("case").unwrap();
        assert_eq!(sig.min_arity, 2);
        assert_eq!(sig.max_arity, usize::MAX);

        let sig = get_signature("format-args").unwrap();
        assert_eq!(sig.min_arity, 1);
        assert_eq!(sig.max_arity, usize::MAX);
    }

    #[test]
    fn test_get_expected_type_at_position() {
        let sig = get_signature("match").unwrap();

        // Position 0 (first arg) should be Space
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Space);

        // Position 1 should be Atom (HE-aligned: pattern is Atom meta-type)
        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Atom);

        // Out of bounds
        assert!(get_expected_type_at_position(sig, 100).is_none());
    }

    #[test]
    fn test_all_special_forms_have_signatures() {
        // All forms and operations should have signatures
        let all_ops = [
            // Core
            "=", ":", "!", "quote", "unquote", "eval", "nop",
            // Control flow
            "if", "case", "switch", "if-equal", "if-reducible",
            // Binding
            "let", "let*", "unify", "function", "return", "chain",
            // Error handling
            "error", "Error", "is-error", "catch",
            // Space ops
            "match", "match-or", "new-space", "add-atom", "remove-atom",
            "collapse", "collapse-bind", "get-atoms",
            // Type ops
            "get-type", "check-type", "get-metatype", "validate-atom", "get-type-space",
            "is-function", "type-cast", "metta", "match-type-or", "first-from-pair", ":<",
            // List/Expression ops
            "car-atom", "cdr-atom", "cons-atom", "decons-atom",
            "size-atom", "max-atom", "min-atom", "index-atom", "empty",
            "tuple-concat", "tuple-count", "without", "element-of",
            "range", "reverse-atom", "flatten-atom", "zip-atom",
            "take-atom", "drop-atom",
            // Higher-order
            "map-atom", "filter-atom", "foldl-atom",
            "sort-tuple", "best-candidate",
            // Set ops
            "unique-atom", "union-atom", "intersection-atom", "subtraction-atom",
            // Nondeterminism
            "superpose", "amb", "guard", "commit", "backtrack",
            // Quoting
            "sealed", "atom-subst",
            // State
            "new-state", "get-state", "change-state!",
            // I/O
            "println!", "trace!", "repr", "format-args", "=alpha",
            // Modules
            "include", "import!", "bind!", "mod-space!", "print-mods!", "pragma!",
            // Memoization
            "new-memo", "memo", "memo-first", "clear-memo!", "memo-stats",
            // Assertions
            "assertEqual", "assertEqualMsg",
            "assertAlphaEqual", "assertAlphaEqualMsg",
            "assertEqualToResult", "assertEqualToResultMsg",
            "assertAlphaEqualToResult", "assertAlphaEqualToResultMsg",
            // Arithmetic
            "+", "-", "*", "/", "%", "min", "max",
            "/safe", "clamp", "floor-div",
            "abs", "abs-math", "floor", "floor-math",
            "ceil", "ceil-math", "round", "round-math",
            "sqrt", "sqrt-math", "trunc", "trunc-math",
            "pow", "pow-math", "log", "log-math",
            // Trig
            "sin-math", "cos-math", "tan-math",
            "asin-math", "acos-math", "atan-math",
            // Float classification
            "isnan-math", "isinf-math",
            // Boolean
            "and", "or", "not", "xor",
            // Comparison
            "<", "<=", ">", ">=", "==", "!=",
        ];

        for form in all_ops {
            assert!(
                is_builtin(form),
                "Operation '{}' should have a signature",
                form
            );
        }
    }

    #[test]
    fn test_arithmetic_operators_have_signatures() {
        for op in ["+", "-", "*", "/", "%"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Operator '{}' should have a signature", op);
            let sig = sig.unwrap();
            assert_eq!(sig.min_arity, 2);
            assert_eq!(sig.max_arity, 2);
        }
    }

    #[test]
    fn test_comparison_operators_have_signatures() {
        for op in ["<", "<=", ">", ">=", "==", "!="] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Operator '{}' should have a signature", op);
            let sig = sig.unwrap();
            assert_eq!(sig.min_arity, 2);
            assert_eq!(sig.max_arity, 2);
        }
    }

    // ========================================================================
    // Additional Signature Completeness Tests
    // ========================================================================

    #[test]
    fn test_list_operations_have_signatures() {
        for op in [
            "car-atom", "cdr-atom", "cons-atom", "decons-atom",
            "size-atom", "max-atom", "min-atom", "index-atom", "empty",
            "tuple-concat", "tuple-count", "without", "element-of",
            "range", "reverse-atom", "flatten-atom", "zip-atom",
            "take-atom", "drop-atom",
        ] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "List operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_state_operations_have_signatures() {
        for op in ["new-state", "get-state", "change-state!"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "State operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_io_operations_have_signatures() {
        for op in ["println!", "trace!", "repr", "format-args"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "I/O operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_module_operations_have_signatures() {
        for op in ["bind!", "include", "import!", "mod-space!", "print-mods!", "pragma!"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "Module operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_type_operations_have_signatures() {
        for op in [":", "get-type", "check-type", "get-metatype", "validate-atom", "get-type-space", "is-function", "type-cast", "metta", "match-type-or", "first-from-pair", ":<"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "Type operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_error_operations_have_signatures() {
        for op in ["error", "Error", "is-error", "catch"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "Error operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_evaluation_operations_have_signatures() {
        for op in ["!", "eval", "quote", "unquote", "nop", "sealed", "atom-subst"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "Evaluation operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_binding_operations_have_signatures() {
        for op in ["let", "let*", "unify", "function", "return", "chain"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "Binding operation '{}' should have a signature",
                op
            );
        }
    }

    #[test]
    fn test_higher_order_list_operations_have_signatures() {
        for op in ["map-atom", "filter-atom", "foldl-atom", "sort-tuple", "best-candidate"] {
            let sig = get_signature(op);
            assert!(
                sig.is_some(),
                "Higher-order list operation '{}' should have a signature",
                op
            );
        }
    }

    // ========================================================================
    // Return Type Tests
    // ========================================================================

    #[test]
    fn test_return_type_arithmetic() {
        for op in ["+", "-", "*", "/", "%"] {
            let sig = get_signature(op).unwrap();
            let ret = get_return_type(&sig.type_sig).unwrap();
            assert_eq!(
                *ret,
                TypeExpr::Number,
                "Arithmetic op '{}' should return Number",
                op
            );
        }
    }

    #[test]
    fn test_return_type_comparison() {
        for op in ["<", "<=", ">", ">=", "==", "!="] {
            let sig = get_signature(op).unwrap();
            let ret = get_return_type(&sig.type_sig).unwrap();
            assert_eq!(
                *ret,
                TypeExpr::Bool,
                "Comparison op '{}' should return Bool",
                op
            );
        }
    }

    #[test]
    fn test_return_type_space_operations() {
        let sig = get_signature("new-space").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Space);

        let sig = get_signature("add-atom").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Unit);

        // HE-aligned: get-atoms returns Atom, not List
        let sig = get_signature("get-atoms").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Atom);
    }

    #[test]
    fn test_return_type_state_operations() {
        // HE-aligned: new-state returns StateMonad($t)
        let sig = get_signature("new-state").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert!(matches!(ret, TypeExpr::StateMonad(_)));

        let sig = get_signature("get-state").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Var("t"));
    }

    #[test]
    fn test_return_type_error() {
        let sig = get_signature("error").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Error);
    }

    #[test]
    fn test_return_type_type_operations() {
        let sig = get_signature("get-type").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Type);

        let sig = get_signature("check-type").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Bool);

        let sig = get_signature(":").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Unit);
    }

    #[test]
    fn test_return_type_io() {
        let sig = get_signature("println!").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::IO(Box::new(TypeExpr::Unit)));

        let sig = get_signature("trace!").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::IO(Box::new(TypeExpr::Undefined)));

        let sig = get_signature("repr").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::String);
    }

    // ========================================================================
    // TypeExpr Tests
    // ========================================================================

    #[test]
    fn test_type_expr_equality() {
        assert_eq!(TypeExpr::Number, TypeExpr::Number);
        assert_eq!(TypeExpr::Bool, TypeExpr::Bool);
        assert_eq!(TypeExpr::Var("a"), TypeExpr::Var("a"));
        assert_ne!(TypeExpr::Var("a"), TypeExpr::Var("b"));
        assert_ne!(TypeExpr::Number, TypeExpr::String);
    }

    #[test]
    fn test_type_expr_clone() {
        let original = TypeExpr::Arrow(
            vec![TypeExpr::Number, TypeExpr::Number],
            Box::new(TypeExpr::Number),
        );
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_type_expr_list_equality() {
        let list1 = TypeExpr::List(Box::new(TypeExpr::Var("a")));
        let list2 = TypeExpr::List(Box::new(TypeExpr::Var("a")));
        let list3 = TypeExpr::List(Box::new(TypeExpr::Var("b")));

        assert_eq!(list1, list2);
        assert_ne!(list1, list3);
    }

    #[test]
    fn test_type_expr_arrow_equality() {
        let arrow1 = TypeExpr::Arrow(
            vec![TypeExpr::Bool, TypeExpr::Var("a"), TypeExpr::Var("a")],
            Box::new(TypeExpr::Var("a")),
        );
        let arrow2 = TypeExpr::Arrow(
            vec![TypeExpr::Bool, TypeExpr::Var("a"), TypeExpr::Var("a")],
            Box::new(TypeExpr::Var("a")),
        );
        let arrow3 = TypeExpr::Arrow(vec![TypeExpr::Bool], Box::new(TypeExpr::Bool));

        assert_eq!(arrow1, arrow2);
        assert_ne!(arrow1, arrow3);
    }

    // ========================================================================
    // Helper Function Tests
    // ========================================================================

    #[test]
    fn test_arrow_helper() {
        let t = arrow(vec![TypeExpr::Number, TypeExpr::Number], TypeExpr::Number);
        assert!(matches!(t, TypeExpr::Arrow(args, _) if args.len() == 2));

        // Empty arrow
        let t = arrow(vec![], TypeExpr::Unit);
        if let TypeExpr::Arrow(args, ret) = t {
            assert!(args.is_empty());
            assert_eq!(*ret, TypeExpr::Unit);
        } else {
            panic!("Expected Arrow");
        }
    }

    #[test]
    fn test_list_helper() {
        let t = list(TypeExpr::Number);
        if let TypeExpr::List(inner) = t {
            assert_eq!(*inner, TypeExpr::Number);
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_nested_list() {
        // List of Lists
        let t = list(list(TypeExpr::Number));
        if let TypeExpr::List(outer) = t {
            if let TypeExpr::List(inner) = *outer {
                assert_eq!(*inner, TypeExpr::Number);
            } else {
                panic!("Expected inner List");
            }
        } else {
            panic!("Expected outer List");
        }
    }

    // ========================================================================
    // builtin_names Iterator Tests
    // ========================================================================

    #[test]
    fn test_builtin_names_not_empty() {
        let names: Vec<_> = builtin_names().collect();
        assert!(!names.is_empty(), "builtin_names should not be empty");
        assert!(names.len() > 100, "Should have at least 100 built-ins, got {}", names.len());
    }

    #[test]
    fn test_builtin_names_contains_expected() {
        let names: Vec<_> = builtin_names().collect();
        assert!(names.contains(&"+"), "Should contain +");
        assert!(names.contains(&"let"), "Should contain let");
        assert!(names.contains(&"match"), "Should contain match");
        assert!(names.contains(&"if"), "Should contain if");
    }

    #[test]
    fn test_builtin_names_consistency() {
        // Every name from builtin_names should be found via get_signature
        for name in builtin_names() {
            assert!(
                get_signature(name).is_some(),
                "builtin_names returned '{}' but get_signature can't find it",
                name
            );
        }
    }

    // ========================================================================
    // Arity Edge Case Tests
    // ========================================================================

    #[test]
    fn test_zero_arity_operations() {
        // new-space and empty have fixed arity 0
        for op in ["new-space", "empty"] {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 0, "Op '{}' should have min_arity 0", op);
            assert_eq!(sig.max_arity, 0, "Op '{}' should have max_arity 0", op);
        }
        // nop accepts 0+ args per HE
        let sig = get_signature("nop").unwrap();
        assert_eq!(sig.min_arity, 0);
    }

    #[test]
    fn test_single_arity_operations() {
        let single_arity_ops = [
            "!",
            "quote",
            "eval",
            "get-atoms",
            "get-type",
            "is-error",
            "new-state",
            "get-state",
            "println!",
            "repr",
        ];
        for op in single_arity_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 1, "Op '{}' should have min_arity 1", op);
            assert_eq!(sig.max_arity, 1, "Op '{}' should have max_arity 1", op);
        }
    }

    #[test]
    fn test_binary_operations() {
        let binary_ops = [
            "+",
            "-",
            "*",
            "/",
            "%",
            "<",
            "<=",
            ">",
            ">=",
            "==",
            "!=",
            "add-atom",
            "remove-atom",
            "cons-atom",
            "bind!",
            "catch",
            "let*",
            ":",
            "check-type",
            "error",
            "change-state!",
        ];
        for op in binary_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 2, "Op '{}' should have min_arity 2", op);
            assert_eq!(sig.max_arity, 2, "Op '{}' should have max_arity 2", op);
        }
    }

    #[test]
    fn test_ternary_operations() {
        // foldl-atom is now 5-ary per HE
        let ternary_ops = ["if", "let", "chain"];
        for op in ternary_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 3, "Op '{}' should have min_arity 3", op);
            assert_eq!(sig.max_arity, 3, "Op '{}' should have max_arity 3", op);
        }
        let sig = get_signature("foldl-atom").unwrap();
        assert_eq!(sig.min_arity, 5);
        assert_eq!(sig.max_arity, 5);
    }

    #[test]
    fn test_quaternary_operations() {
        let quad_ops = ["unify"];
        for op in quad_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 4, "Op '{}' should have min_arity 4", op);
            assert_eq!(sig.max_arity, 4, "Op '{}' should have max_arity 4", op);
        }
    }

    #[test]
    fn test_match_arity_with_optional_default() {
        let sig = get_signature("match").unwrap();
        assert_eq!(sig.min_arity, 3, "match should have min_arity 3");
        assert_eq!(
            sig.max_arity, 4,
            "match should have max_arity 4 (optional default)"
        );
    }

    // ========================================================================
    // Expected Type Position Tests
    // ========================================================================

    #[test]
    fn test_expected_type_if() {
        let sig = get_signature("if").unwrap();

        // (if cond then else) — HE: then/else are Atom (lazy)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Bool, "if position 0 should be Bool");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Atom, "if position 1 should be Atom (lazy)");

        let t2 = get_expected_type_at_position(sig, 2).unwrap();
        assert_eq!(*t2, TypeExpr::Atom, "if position 2 should be Atom (lazy)");
    }

    #[test]
    fn test_expected_type_let() {
        let sig = get_signature("let").unwrap();

        // HE-aligned: (let Atom %Undefined% Atom %Undefined%)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Atom, "let position 0 should be Atom (pattern)");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Undefined, "let position 1 should be %Undefined% (value)");

        let t2 = get_expected_type_at_position(sig, 2).unwrap();
        assert_eq!(*t2, TypeExpr::Atom, "let position 2 should be Atom (body)");
    }

    #[test]
    fn test_expected_type_add_atom() {
        let sig = get_signature("add-atom").unwrap();

        // HE-aligned: (add-atom SpaceType Atom Unit) — atom arg is unevaluated
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Space, "add-atom position 0 should be Space");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Atom, "add-atom position 1 should be Atom (unevaluated)");
    }

    #[test]
    fn test_expected_type_cons_atom() {
        let sig = get_signature("cons-atom").unwrap();

        // HE-aligned: (cons-atom Atom Expression Atom)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Atom, "cons-atom position 0 should be Atom");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Expression, "cons-atom position 1 should be Expression");
    }

    #[test]
    fn test_expected_type_out_of_bounds() {
        let sig = get_signature("+").unwrap();

        assert!(get_expected_type_at_position(sig, 0).is_some());
        assert!(get_expected_type_at_position(sig, 1).is_some());
        assert!(
            get_expected_type_at_position(sig, 2).is_none(),
            "Position 2 should be out of bounds for +"
        );
        assert!(
            get_expected_type_at_position(sig, 100).is_none(),
            "Position 100 should be out of bounds"
        );
    }

    // ========================================================================
    // TypeExpr Debug/Display Tests
    // ========================================================================

    #[test]
    fn test_type_expr_debug_format() {
        // Ensure Debug is implemented and produces reasonable output
        let t = TypeExpr::Arrow(
            vec![TypeExpr::Number, TypeExpr::Number],
            Box::new(TypeExpr::Number),
        );
        let debug_str = format!("{:?}", t);
        assert!(debug_str.contains("Arrow"));
        assert!(debug_str.contains("Number"));
    }

    #[test]
    fn test_builtin_signature_debug_format() {
        let sig = get_signature("+").unwrap();
        let debug_str = format!("{:?}", sig);
        assert!(debug_str.contains("+"));
        assert!(debug_str.contains("min_arity"));
    }

    // ========================================================================
    // Phase 9: New Type Variant Tests
    // ========================================================================

    #[test]
    fn test_type_expr_new_variants() {
        assert_eq!(TypeExpr::Undefined, TypeExpr::Undefined);
        assert_eq!(TypeExpr::Expression, TypeExpr::Expression);
        assert_eq!(TypeExpr::Variable, TypeExpr::Variable);
        assert_eq!(TypeExpr::Grounded, TypeExpr::Grounded);
        assert_ne!(TypeExpr::Undefined, TypeExpr::Atom);
        assert_ne!(TypeExpr::Expression, TypeExpr::Atom);
    }

    #[test]
    fn test_state_monad_type() {
        let sm1 = TypeExpr::StateMonad(Box::new(TypeExpr::Var("t")));
        let sm2 = TypeExpr::StateMonad(Box::new(TypeExpr::Var("t")));
        let sm3 = TypeExpr::StateMonad(Box::new(TypeExpr::Number));
        assert_eq!(sm1, sm2);
        assert_ne!(sm1, sm3);
    }

    #[test]
    fn test_state_monad_helper() {
        let t = state_monad(TypeExpr::Var("t"));
        if let TypeExpr::StateMonad(inner) = t {
            assert_eq!(*inner, TypeExpr::Var("t"));
        } else {
            panic!("Expected StateMonad");
        }
    }

    #[test]
    fn test_boolean_operations_have_signatures() {
        for op in ["and", "or", "not", "xor"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Boolean op '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_set_operations_have_signatures() {
        for op in ["unique-atom", "union-atom", "intersection-atom", "subtraction-atom"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Set op '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_nondeterminism_operations_have_signatures() {
        for op in ["superpose", "amb", "guard", "commit", "backtrack"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Nondeterminism op '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_memoization_operations_have_signatures() {
        for op in ["new-memo", "memo", "memo-first", "clear-memo!", "memo-stats"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Memoization op '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_assertion_operations_have_signatures() {
        for op in [
            "assertEqual", "assertEqualMsg",
            "assertAlphaEqual", "assertAlphaEqualMsg",
            "assertEqualToResult", "assertEqualToResultMsg",
            "assertAlphaEqualToResult", "assertAlphaEqualToResultMsg",
        ] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Assertion op '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_math_operations_have_signatures() {
        // Unary math
        for op in ["abs", "abs-math", "floor", "floor-math", "ceil", "ceil-math",
                    "round", "round-math", "sqrt", "sqrt-math", "trunc", "trunc-math"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Math op '{}' should have a signature", op);
            let sig = sig.unwrap();
            assert_eq!(sig.min_arity, 1, "Unary math op '{}' should have min_arity 1", op);
        }
        // Binary math
        for op in ["pow", "pow-math", "log", "log-math"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Math op '{}' should have a signature", op);
            let sig = sig.unwrap();
            assert_eq!(sig.min_arity, 2, "Binary math op '{}' should have min_arity 2", op);
        }
    }

    #[test]
    fn test_trig_operations_have_signatures() {
        for op in ["sin-math", "cos-math", "tan-math", "asin-math", "acos-math", "atan-math"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Trig op '{}' should have a signature", op);
            let sig = sig.unwrap();
            assert_eq!(sig.min_arity, 1);
            let ret = get_return_type(&sig.type_sig).unwrap();
            assert_eq!(*ret, TypeExpr::Number);
        }
    }

    #[test]
    fn test_float_classification_signatures() {
        for op in ["isnan-math", "isinf-math"] {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 1);
            let ret = get_return_type(&sig.type_sig).unwrap();
            assert_eq!(*ret, TypeExpr::Bool);
        }
    }

    #[test]
    fn test_total_signature_count() {
        let names: Vec<_> = builtin_names().collect();
        // We have 130+ signatures now
        assert!(names.len() >= 130, "Should have at least 130 built-in signatures, got {}", names.len());
    }
}
