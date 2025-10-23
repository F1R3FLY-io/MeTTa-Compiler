/// MeTTa Intermediate Representation (IR)
///
/// This module defines the core IR types used throughout MeTTaTron.
/// The IR is produced by the Tree-Sitter parser and consumed by the backend evaluator.

use std::fmt;

/// MeTTa IR - Enhanced intermediate representation for MeTTa expressions
///
/// Represents the abstract syntax of MeTTa code with semantic distinctions
/// for different atom types. This IR is used by the Tree-Sitter parser
/// to provide a unified representation for the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum MettaExpr {
    /// Symbolic atom (identifiers, operators, variables, etc.)
    Atom(String),
    /// String literal
    String(String),
    /// Integer literal
    Integer(i64),
    /// Floating point literal (supports scientific notation)
    Float(f64),
    /// List/expression (including special forms like type annotations and rules)
    List(Vec<MettaExpr>),
    /// Quoted expression (prevents evaluation)
    Quoted(Box<MettaExpr>),
}

/// Type alias for backward compatibility
///
/// Originally named `SExpr`, now unified under `MettaExpr` to better
/// represent its role as the IR for MeTTa expressions.
pub type SExpr = MettaExpr;

impl fmt::Display for MettaExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MettaExpr::Atom(s) => write!(f, "{}", s),
            MettaExpr::String(s) => write!(f, "\"{}\"", s),
            MettaExpr::Integer(i) => write!(f, "{}", i),
            MettaExpr::Float(fl) => write!(f, "{}", fl),
            MettaExpr::List(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
            MettaExpr::Quoted(expr) => write!(f, "'{}", expr),
        }
    }
}
