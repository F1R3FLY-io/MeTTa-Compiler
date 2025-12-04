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
    /// Atom type (symbols)
    Atom,
    /// Space type (named spaces like &self)
    Space,
    /// State type (mutable state)
    State,
    /// Unit type (empty result)
    Unit,
    /// Nil type
    Nil,
    /// Error type
    Error,
    /// Type type (type expressions themselves)
    Type,

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
}

/// Helper to create arrow types more concisely
fn arrow(args: Vec<TypeExpr>, ret: TypeExpr) -> TypeExpr {
    TypeExpr::Arrow(args, Box::new(ret))
}

/// Helper to create list types
fn list(elem: TypeExpr) -> TypeExpr {
    TypeExpr::List(Box::new(elem))
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
        BuiltinSignature {
            name: "+",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number),
        },
        BuiltinSignature {
            name: "-",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number),
        },
        BuiltinSignature {
            name: "*",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number),
        },
        BuiltinSignature {
            name: "/",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number),
        },
        BuiltinSignature {
            name: "%",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Number),
        },
        // ====================================================================
        // Comparison operators: (-> Number Number Bool)
        // ====================================================================
        BuiltinSignature {
            name: "<",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool),
        },
        BuiltinSignature {
            name: "<=",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool),
        },
        BuiltinSignature {
            name: ">",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool),
        },
        BuiltinSignature {
            name: ">=",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Number, Number], Bool),
        },
        // ====================================================================
        // Equality operators: polymorphic (-> $a $a Bool)
        // ====================================================================
        BuiltinSignature {
            name: "==",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Var("a"), Var("a")], Bool),
        },
        BuiltinSignature {
            name: "!=",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Var("a"), Var("a")], Bool),
        },
        // ====================================================================
        // Control flow
        // ====================================================================
        // if: (-> Bool $a $a $a)
        BuiltinSignature {
            name: "if",
            min_arity: 3,
            max_arity: 3,
            type_sig: arrow(vec![Bool, Var("a"), Var("a")], Var("a")),
        },
        // case: (-> $a (Pattern $b)... $b)
        BuiltinSignature {
            name: "case",
            min_arity: 2,
            max_arity: usize::MAX,
            type_sig: arrow(vec![Var("a"), Pattern], Var("b")),
        },
        // switch: (-> $a (Pattern $b)... $b)
        BuiltinSignature {
            name: "switch",
            min_arity: 2,
            max_arity: usize::MAX,
            type_sig: arrow(vec![Var("a"), Pattern], Var("b")),
        },
        // ====================================================================
        // Binding forms
        // ====================================================================
        // let: (-> Pattern $a $b $b)
        BuiltinSignature {
            name: "let",
            min_arity: 3,
            max_arity: 3,
            type_sig: arrow(vec![Pattern, Var("a"), Var("b")], Var("b")),
        },
        // let*: (-> Bindings $a $a)
        BuiltinSignature {
            name: "let*",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Bindings, Var("a")], Var("a")),
        },
        // unify: (-> $a $a $b $b $b)
        BuiltinSignature {
            name: "unify",
            min_arity: 4,
            max_arity: 4,
            type_sig: arrow(vec![Var("a"), Var("a"), Var("b"), Var("b")], Var("b")),
        },
        // function: (-> $a $a)
        BuiltinSignature {
            name: "function",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Var("a")),
        },
        // return: (-> $a $a)
        BuiltinSignature {
            name: "return",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Var("a")),
        },
        // chain: (-> $a Pattern $b $b)
        BuiltinSignature {
            name: "chain",
            min_arity: 3,
            max_arity: 3,
            type_sig: arrow(vec![Var("a"), Pattern, Var("b")], Var("b")),
        },
        // ====================================================================
        // Space operations
        // ====================================================================
        // match: (-> Space Pattern $a [$a]) - optional default
        BuiltinSignature {
            name: "match",
            min_arity: 3,
            max_arity: 4,
            type_sig: arrow(vec![Space, Pattern, Var("a"), Var("a")], Var("a")),
        },
        // new-space: (-> Space)
        BuiltinSignature {
            name: "new-space",
            min_arity: 0,
            max_arity: 0,
            type_sig: arrow(vec![], Space),
        },
        // add-atom: (-> Space $a Unit)
        BuiltinSignature {
            name: "add-atom",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Space, Var("a")], Unit),
        },
        // remove-atom: (-> Space $a Unit)
        BuiltinSignature {
            name: "remove-atom",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Space, Var("a")], Unit),
        },
        // get-atoms: (-> Space (List $a))
        BuiltinSignature {
            name: "get-atoms",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Space], list(Var("a"))),
        },
        // collapse: (-> $a (List $a))
        BuiltinSignature {
            name: "collapse",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], list(Var("a"))),
        },
        // ====================================================================
        // List operations
        // ====================================================================
        // car-atom: (-> (List $a) $a)
        BuiltinSignature {
            name: "car-atom",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![list(Var("a"))], Var("a")),
        },
        // cdr-atom: (-> (List $a) (List $a))
        BuiltinSignature {
            name: "cdr-atom",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![list(Var("a"))], list(Var("a"))),
        },
        // cons-atom: (-> $a (List $a) (List $a))
        BuiltinSignature {
            name: "cons-atom",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Var("a"), list(Var("a"))], list(Var("a"))),
        },
        // decons-atom: (-> (List $a) (List $a))
        BuiltinSignature {
            name: "decons-atom",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![list(Var("a"))], list(Var("a"))),
        },
        // size-atom: (-> (List $a) Number)
        BuiltinSignature {
            name: "size-atom",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![list(Var("a"))], Number),
        },
        // max-atom: (-> (List Number) Number)
        BuiltinSignature {
            name: "max-atom",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![list(Number)], Number),
        },
        // empty: (-> (List $a))
        BuiltinSignature {
            name: "empty",
            min_arity: 0,
            max_arity: 0,
            type_sig: arrow(vec![], list(Var("a"))),
        },
        // ====================================================================
        // Higher-order list operations
        // ====================================================================
        // map-atom: (-> (-> $a $b) (List $a) (List $b))
        BuiltinSignature {
            name: "map-atom",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(
                vec![arrow(vec![Var("a")], Var("b")), list(Var("a"))],
                list(Var("b")),
            ),
        },
        // filter-atom: (-> (-> $a Bool) (List $a) (List $a))
        BuiltinSignature {
            name: "filter-atom",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(
                vec![arrow(vec![Var("a")], Bool), list(Var("a"))],
                list(Var("a")),
            ),
        },
        // foldl-atom: (-> (-> $acc $a $acc) $acc (List $a) $acc)
        BuiltinSignature {
            name: "foldl-atom",
            min_arity: 3,
            max_arity: 3,
            type_sig: arrow(
                vec![
                    arrow(vec![Var("acc"), Var("a")], Var("acc")),
                    Var("acc"),
                    list(Var("a")),
                ],
                Var("acc"),
            ),
        },
        // ====================================================================
        // Type operations
        // ====================================================================
        // :: (-> $a Type Unit)
        BuiltinSignature {
            name: ":",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Var("a"), Type], Unit),
        },
        // get-type: (-> $a Type)
        BuiltinSignature {
            name: "get-type",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Type),
        },
        // check-type: (-> $a Type Bool)
        BuiltinSignature {
            name: "check-type",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Var("a"), Type], Bool),
        },
        // get-metatype: (-> $a Type)
        BuiltinSignature {
            name: "get-metatype",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Type),
        },
        // ====================================================================
        // Error handling
        // ====================================================================
        // error: (-> Any Any Error)
        BuiltinSignature {
            name: "error",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Any, Any], Error),
        },
        // is-error: (-> $a Bool)
        BuiltinSignature {
            name: "is-error",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Bool),
        },
        // catch: (-> $a $a $a)
        BuiltinSignature {
            name: "catch",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Var("a"), Var("a")], Var("a")),
        },
        // ====================================================================
        // Evaluation control
        // ====================================================================
        // !: (-> $a $a)
        BuiltinSignature {
            name: "!",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Var("a")),
        },
        // eval: (-> Expr $a)
        BuiltinSignature {
            name: "eval",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Expr], Var("a")),
        },
        // quote: (-> $a Expr)
        BuiltinSignature {
            name: "quote",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Expr),
        },
        // nop: (-> Unit)
        BuiltinSignature {
            name: "nop",
            min_arity: 0,
            max_arity: 0,
            type_sig: arrow(vec![], Unit),
        },
        // ====================================================================
        // Rule definition
        // ====================================================================
        // =: (-> Pattern $a Unit)
        BuiltinSignature {
            name: "=",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Pattern, Var("a")], Unit),
        },
        // ====================================================================
        // State operations
        // ====================================================================
        // new-state: (-> $a State)
        BuiltinSignature {
            name: "new-state",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], State),
        },
        // get-state: (-> State $a)
        BuiltinSignature {
            name: "get-state",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![State], Var("a")),
        },
        // change-state!: (-> State $a $a)
        BuiltinSignature {
            name: "change-state!",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![State, Var("a")], Var("a")),
        },
        // ====================================================================
        // I/O and debugging
        // ====================================================================
        // println!: (-> $a Unit)
        BuiltinSignature {
            name: "println!",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Unit),
        },
        // trace!: (-> $a $a)
        BuiltinSignature {
            name: "trace!",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], Var("a")),
        },
        // repr: (-> $a String)
        BuiltinSignature {
            name: "repr",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![Var("a")], String),
        },
        // format-args: (-> String $a... String)
        BuiltinSignature {
            name: "format-args",
            min_arity: 1,
            max_arity: usize::MAX,
            type_sig: arrow(vec![String, Any], String),
        },
        // ====================================================================
        // Module system
        // ====================================================================
        // bind!: (-> Atom $a Unit)
        BuiltinSignature {
            name: "bind!",
            min_arity: 2,
            max_arity: 2,
            type_sig: arrow(vec![Atom, Var("a")], Unit),
        },
        // include: (-> String Unit)
        BuiltinSignature {
            name: "include",
            min_arity: 1,
            max_arity: 1,
            type_sig: arrow(vec![String], Unit),
        },
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
        assert_eq!(arg_types[1], TypeExpr::Var("a"));
        assert_eq!(arg_types[2], TypeExpr::Var("a"));
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

        // Position 1 should be Pattern
        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Pattern);

        // Out of bounds
        assert!(get_expected_type_at_position(sig, 100).is_none());
    }

    #[test]
    fn test_all_special_forms_have_signatures() {
        // All forms from SPECIAL_FORMS in eval/mod.rs should have signatures
        let special_forms = [
            "=", "!", "quote", "if", "error", "is-error", "catch", "eval", "function", "return",
            "chain", "match", "case", "switch", "let", ":", "get-type", "check-type", "map-atom",
            "filter-atom", "foldl-atom", "car-atom", "cdr-atom", "cons-atom", "decons-atom",
            "size-atom", "max-atom", "let*", "unify", "new-space", "add-atom", "remove-atom",
            "collapse", "get-atoms", "new-state", "get-state", "change-state!", "bind!",
            "println!", "trace!", "nop", "repr", "format-args", "empty", "get-metatype", "include",
        ];

        for form in special_forms {
            assert!(
                is_builtin(form),
                "Special form '{}' should have a signature",
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
        for op in ["car-atom", "cdr-atom", "cons-atom", "decons-atom", "size-atom", "max-atom", "empty"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "List operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_state_operations_have_signatures() {
        for op in ["new-state", "get-state", "change-state!"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "State operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_io_operations_have_signatures() {
        for op in ["println!", "trace!", "repr", "format-args"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "I/O operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_module_operations_have_signatures() {
        for op in ["bind!", "include"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Module operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_type_operations_have_signatures() {
        for op in [":", "get-type", "check-type", "get-metatype"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Type operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_error_operations_have_signatures() {
        for op in ["error", "is-error", "catch"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Error operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_evaluation_operations_have_signatures() {
        for op in ["!", "eval", "quote", "nop"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Evaluation operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_binding_operations_have_signatures() {
        for op in ["let", "let*", "unify", "function", "return", "chain"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Binding operation '{}' should have a signature", op);
        }
    }

    #[test]
    fn test_higher_order_list_operations_have_signatures() {
        for op in ["map-atom", "filter-atom", "foldl-atom"] {
            let sig = get_signature(op);
            assert!(sig.is_some(), "Higher-order list operation '{}' should have a signature", op);
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
            assert_eq!(*ret, TypeExpr::Number, "Arithmetic op '{}' should return Number", op);
        }
    }

    #[test]
    fn test_return_type_comparison() {
        for op in ["<", "<=", ">", ">=", "==", "!="] {
            let sig = get_signature(op).unwrap();
            let ret = get_return_type(&sig.type_sig).unwrap();
            assert_eq!(*ret, TypeExpr::Bool, "Comparison op '{}' should return Bool", op);
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

        let sig = get_signature("get-atoms").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert!(matches!(ret, TypeExpr::List(_)));
    }

    #[test]
    fn test_return_type_state_operations() {
        let sig = get_signature("new-state").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::State);

        let sig = get_signature("get-state").unwrap();
        let ret = get_return_type(&sig.type_sig).unwrap();
        assert_eq!(*ret, TypeExpr::Var("a"));
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
        assert_eq!(*ret, TypeExpr::Unit);

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
        let original = TypeExpr::Arrow(vec![TypeExpr::Number, TypeExpr::Number], Box::new(TypeExpr::Number));
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
        let arrow1 = TypeExpr::Arrow(vec![TypeExpr::Bool, TypeExpr::Var("a"), TypeExpr::Var("a")], Box::new(TypeExpr::Var("a")));
        let arrow2 = TypeExpr::Arrow(vec![TypeExpr::Bool, TypeExpr::Var("a"), TypeExpr::Var("a")], Box::new(TypeExpr::Var("a")));
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
        assert!(names.len() > 40, "Should have at least 40 built-ins");
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
        let zero_arity_ops = ["nop", "new-space", "empty"];
        for op in zero_arity_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 0, "Op '{}' should have min_arity 0", op);
            assert_eq!(sig.max_arity, 0, "Op '{}' should have max_arity 0", op);
        }
    }

    #[test]
    fn test_single_arity_operations() {
        let single_arity_ops = ["!", "quote", "eval", "get-atoms", "get-type", "is-error", "new-state", "get-state", "println!", "repr"];
        for op in single_arity_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 1, "Op '{}' should have min_arity 1", op);
            assert_eq!(sig.max_arity, 1, "Op '{}' should have max_arity 1", op);
        }
    }

    #[test]
    fn test_binary_operations() {
        let binary_ops = ["+", "-", "*", "/", "%", "<", "<=", ">", ">=", "==", "!=", "add-atom", "remove-atom", "cons-atom", "bind!", "catch", "let*", ":", "check-type", "error", "change-state!"];
        for op in binary_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 2, "Op '{}' should have min_arity 2", op);
            assert_eq!(sig.max_arity, 2, "Op '{}' should have max_arity 2", op);
        }
    }

    #[test]
    fn test_ternary_operations() {
        let ternary_ops = ["if", "let", "chain", "foldl-atom"];
        for op in ternary_ops {
            let sig = get_signature(op).unwrap();
            assert_eq!(sig.min_arity, 3, "Op '{}' should have min_arity 3", op);
            assert_eq!(sig.max_arity, 3, "Op '{}' should have max_arity 3", op);
        }
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
        assert_eq!(sig.max_arity, 4, "match should have max_arity 4 (optional default)");
    }

    // ========================================================================
    // Expected Type Position Tests
    // ========================================================================

    #[test]
    fn test_expected_type_if() {
        let sig = get_signature("if").unwrap();

        // (if cond then else)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Bool, "if position 0 should be Bool");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Var("a"), "if position 1 should be Var(a)");

        let t2 = get_expected_type_at_position(sig, 2).unwrap();
        assert_eq!(*t2, TypeExpr::Var("a"), "if position 2 should be Var(a)");
    }

    #[test]
    fn test_expected_type_let() {
        let sig = get_signature("let").unwrap();

        // (let pattern value body)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Pattern, "let position 0 should be Pattern");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Var("a"), "let position 1 should be Var(a)");

        let t2 = get_expected_type_at_position(sig, 2).unwrap();
        assert_eq!(*t2, TypeExpr::Var("b"), "let position 2 should be Var(b)");
    }

    #[test]
    fn test_expected_type_add_atom() {
        let sig = get_signature("add-atom").unwrap();

        // (add-atom space atom)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Space, "add-atom position 0 should be Space");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert_eq!(*t1, TypeExpr::Var("a"), "add-atom position 1 should be Var(a)");
    }

    #[test]
    fn test_expected_type_cons_atom() {
        let sig = get_signature("cons-atom").unwrap();

        // (cons-atom elem list)
        let t0 = get_expected_type_at_position(sig, 0).unwrap();
        assert_eq!(*t0, TypeExpr::Var("a"), "cons-atom position 0 should be Var(a)");

        let t1 = get_expected_type_at_position(sig, 1).unwrap();
        assert!(matches!(t1, TypeExpr::List(_)), "cons-atom position 1 should be List");
    }

    #[test]
    fn test_expected_type_out_of_bounds() {
        let sig = get_signature("+").unwrap();

        assert!(get_expected_type_at_position(sig, 0).is_some());
        assert!(get_expected_type_at_position(sig, 1).is_some());
        assert!(get_expected_type_at_position(sig, 2).is_none(), "Position 2 should be out of bounds for +");
        assert!(get_expected_type_at_position(sig, 100).is_none(), "Position 100 should be out of bounds");
    }

    // ========================================================================
    // TypeExpr Debug/Display Tests
    // ========================================================================

    #[test]
    fn test_type_expr_debug_format() {
        // Ensure Debug is implemented and produces reasonable output
        let t = TypeExpr::Arrow(vec![TypeExpr::Number, TypeExpr::Number], Box::new(TypeExpr::Number));
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
}
