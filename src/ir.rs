//! MeTTa Intermediate Representation (IR)
//!
//! This module defines the core IR types used throughout MeTTaTron.
//! The IR is produced by the Tree-Sitter parser and consumed by the backend evaluator.

use std::fmt;

/// Position in source code (line and column, 0-indexed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Line number (0-indexed)
    pub row: usize,
    /// Column number (0-indexed, in bytes)
    pub column: usize,
}

impl Position {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }

    /// Create a zero position (used as default)
    pub fn zero() -> Self {
        Self { row: 0, column: 0 }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.row + 1, self.column + 1)
    }
}

/// Span of source code with absolute position information
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Start position (inclusive)
    pub start: Position,
    /// End position (exclusive)
    pub end: Position,
    /// Start byte offset (absolute position in source)
    pub start_byte: usize,
    /// End byte offset (absolute position in source)
    pub end_byte: usize,
}

impl Span {
    pub fn new(start: Position, end: Position, start_byte: usize, end_byte: usize) -> Self {
        Self {
            start,
            end,
            start_byte,
            end_byte,
        }
    }

    /// Create a zero span (used as default)
    pub fn zero() -> Self {
        Self {
            start: Position::zero(),
            end: Position::zero(),
            start_byte: 0,
            end_byte: 0,
        }
    }

    /// Length of the span in bytes
    pub fn len(&self) -> usize {
        self.end_byte.saturating_sub(self.start_byte)
    }

    /// Check if span is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}-{}", self.start, self.end)
    }
}

/// MeTTa IR - Enhanced intermediate representation for MeTTa expressions
///
/// Represents the abstract syntax of MeTTa code with semantic distinctions
/// for different atom types. This IR is used by the Tree-Sitter parser
/// to provide a unified representation for the backend.
///
/// Each variant includes an optional `Span` for absolute position tracking,
/// which is used by language servers for indexing and navigation.
#[derive(Debug, Clone, PartialEq)]
pub enum MettaExpr {
    /// Symbolic atom (identifiers, operators, variables, etc.)
    Atom(String, Option<Span>),
    /// String literal
    String(String, Option<Span>),
    /// Integer literal
    Integer(i64, Option<Span>),
    /// Floating point literal (supports scientific notation)
    Float(f64, Option<Span>),
    /// List/expression (including special forms like type annotations and rules)
    List(Vec<MettaExpr>, Option<Span>),
}

/// Type alias for backward compatibility
///
/// Originally named `SExpr`, now unified under `MettaExpr` to better
/// represent its role as the IR for MeTTa expressions.
pub type SExpr = MettaExpr;

impl MettaExpr {
    /// Get the span associated with this expression, if any
    pub fn span(&self) -> Option<Span> {
        match self {
            MettaExpr::Atom(_, span) => *span,
            MettaExpr::String(_, span) => *span,
            MettaExpr::Integer(_, span) => *span,
            MettaExpr::Float(_, span) => *span,
            MettaExpr::List(_, span) => *span,
        }
    }

    /// Set the span for this expression
    pub fn with_span(self, new_span: Span) -> Self {
        match self {
            MettaExpr::Atom(s, _) => MettaExpr::Atom(s, Some(new_span)),
            MettaExpr::String(s, _) => MettaExpr::String(s, Some(new_span)),
            MettaExpr::Integer(i, _) => MettaExpr::Integer(i, Some(new_span)),
            MettaExpr::Float(f, _) => MettaExpr::Float(f, Some(new_span)),
            MettaExpr::List(items, _) => MettaExpr::List(items, Some(new_span)),
        }
    }

    /// Create variants without spans (for backward compatibility)
    pub fn atom(s: impl Into<String>) -> Self {
        MettaExpr::Atom(s.into(), None)
    }

    pub fn string(s: impl Into<String>) -> Self {
        MettaExpr::String(s.into(), None)
    }

    pub fn integer(i: i64) -> Self {
        MettaExpr::Integer(i, None)
    }

    pub fn float(f: f64) -> Self {
        MettaExpr::Float(f, None)
    }

    pub fn list(items: Vec<MettaExpr>) -> Self {
        MettaExpr::List(items, None)
    }
}

impl fmt::Display for MettaExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MettaExpr::Atom(s, _) => write!(f, "{}", s),
            MettaExpr::String(s, _) => write!(f, "\"{}\"", s),
            MettaExpr::Integer(i, _) => write!(f, "{}", i),
            MettaExpr::Float(fl, _) => write!(f, "{}", fl),
            MettaExpr::List(items, _) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
        }
    }
}
