//! Tree-sitter grammar for MeTTa language
//!
//! This crate provides Tree-sitter bindings for the MeTTa language,
//! with semantic decomposition of atom types for precise LSP support.

use tree_sitter::Language;

extern "C" {
    fn tree_sitter_metta() -> Language;
}

/// Returns the Tree-sitter Language for MeTTa
pub fn language() -> Language {
    unsafe { tree_sitter_metta() }
}

/// Node type names exposed by the grammar
pub mod node_types {
    pub const EXPRESSION: &str = "expression";
    pub const LIST: &str = "list";
    pub const BRACE_LIST: &str = "brace_list";
    pub const PREFIXED_EXPRESSION: &str = "prefixed_expression";
    pub const ATOM_EXPRESSION: &str = "atom_expression";

    // Semantic atom types
    pub const VARIABLE: &str = "variable";
    pub const WILDCARD: &str = "wildcard";
    pub const IDENTIFIER: &str = "identifier";
    pub const STRING_LITERAL: &str = "string_literal";
    pub const INTEGER_LITERAL: &str = "integer_literal";
    pub const BOOLEAN_LITERAL: &str = "boolean_literal";

    // Operator types
    pub const OPERATOR: &str = "operator";
    pub const ARROW_OPERATOR: &str = "arrow_operator";
    pub const COMPARISON_OPERATOR: &str = "comparison_operator";
    pub const ASSIGNMENT_OPERATOR: &str = "assignment_operator";
    pub const PUNCTUATION_OPERATOR: &str = "punctuation_operator";
    pub const ARITHMETIC_OPERATOR: &str = "arithmetic_operator";
    pub const LOGIC_OPERATOR: &str = "logic_operator";

    // Prefix types
    pub const EXCLAIM_PREFIX: &str = "exclaim_prefix";
    pub const QUESTION_PREFIX: &str = "question_prefix";
    pub const QUOTE_PREFIX: &str = "quote_prefix";

    // Comments
    pub const LINE_COMMENT: &str = "line_comment";
    pub const BLOCK_COMMENT: &str = "block_comment";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_loads() {
        let lang = language();
        assert!(lang.node_kind_count() > 0);
    }
}
