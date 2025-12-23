//! Types for fuzzy matching and smart suggestions.

use crate::backend::models::MettaValue;
use crate::backend::Environment;

/// Result of a smart suggestion query with confidence level
#[derive(Debug, Clone)]
pub struct SmartSuggestion {
    /// The formatted "Did you mean: X?" message
    pub message: String,
    /// How confident we are this is a typo vs intentional
    pub confidence: SuggestionConfidence,
    /// The suggested terms
    pub suggestions: Vec<String>,
}

/// Confidence level for typo suggestions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SuggestionConfidence {
    /// No suggestion should be made
    None,
    /// Low confidence - only show as a note, don't affect evaluation
    Low,
    /// High confidence - likely a typo, show as warning
    High,
}

/// Context for making context-aware suggestions.
///
/// This struct captures information about where an unknown symbol appears,
/// enabling the three-pillar validation (arity, type, context).
#[derive(Debug, Clone)]
pub struct SuggestionContext<'a> {
    /// The full expression containing the unknown symbol (head + args)
    pub expr: &'a [MettaValue],
    /// Position of the unknown symbol in parent (0 = head position)
    pub position: usize,
    /// Head of the parent expression (if symbol is not in head position)
    pub parent_head: Option<&'a str>,
    /// Environment for type inference
    pub env: &'a Environment,
}

impl<'a> SuggestionContext<'a> {
    /// Create a new context for head position
    pub fn for_head(expr: &'a [MettaValue], env: &'a Environment) -> Self {
        Self {
            expr,
            position: 0,
            parent_head: None,
            env,
        }
    }

    /// Create a new context for a specific argument position
    pub fn for_arg(
        expr: &'a [MettaValue],
        position: usize,
        parent_head: &'a str,
        env: &'a Environment,
    ) -> Self {
        Self {
            expr,
            position,
            parent_head: Some(parent_head),
            env,
        }
    }

    /// Get the arity (number of arguments, excluding head)
    pub fn arity(&self) -> usize {
        self.expr.len().saturating_sub(1)
    }
}
