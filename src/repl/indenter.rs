//! Smart indentation using Tree-Sitter indent queries
//!
//! Calculates continuation line indentation based on syntax structure

use tree_sitter::{Parser, Query};

/// Smart indenter using Tree-Sitter
pub struct SmartIndenter {
    parser: Parser,
    _indent_query: Query,
    indent_width: usize,
}

impl SmartIndenter {
    /// Create a new smart indenter
    pub fn new() -> Result<Self, String> {
        Self::with_indent_width(2)
    }

    /// Create a new smart indenter with custom indent width
    pub fn with_indent_width(indent_width: usize) -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_metta::language())
            .map_err(|e| format!("Failed to set language: {}", e))?;

        // Load indent queries
        let indent_query_source = include_str!("../../tree-sitter-metta/queries/indents.scm");
        let indent_query = Query::new(&tree_sitter_metta::language(), indent_query_source)
            .map_err(|e| format!("Failed to load indent query: {}", e))?;

        Ok(Self {
            parser,
            _indent_query: indent_query,
            indent_width,
        })
    }

    /// Calculate indentation level for continuation line
    /// Returns number of spaces to indent
    pub fn calculate_indent(&mut self, buffer: &str) -> usize {
        // Parse the buffer
        let tree = match self.parser.parse(buffer, None) {
            Some(tree) => tree,
            None => return 0, // Parse failed, no indentation
        };

        let _root_node = tree.root_node();

        // Simple heuristic: count unclosed delimiters
        let indent_level = self.count_indent_level(buffer);

        indent_level * self.indent_width
    }

    /// Count indent level by counting unclosed delimiters
    fn count_indent_level(&self, buffer: &str) -> usize {
        let mut paren_depth: usize = 0;
        let mut brace_depth: usize = 0;
        let mut in_string = false;
        let mut in_line_comment = false;
        let mut escape_next = false;
        let chars = buffer.chars().peekable();

        for ch in chars {
            if escape_next {
                escape_next = false;
                continue;
            }

            // Line comments (;) end at newline
            if in_line_comment {
                if ch == '\n' {
                    in_line_comment = false;
                }
                continue;
            }

            if in_string {
                if ch == '\\' {
                    escape_next = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            // Start of line comment (MeTTa uses ; for comments)
            if ch == ';' {
                in_line_comment = true;
                continue;
            }

            if ch == '"' {
                in_string = true;
                continue;
            }

            // Count delimiters
            match ch {
                '(' => paren_depth += 1,
                ')' => paren_depth = paren_depth.saturating_sub(1),
                '{' => brace_depth += 1,
                '}' => brace_depth = brace_depth.saturating_sub(1),
                _ => {}
            }
        }

        paren_depth + brace_depth
    }

    /// Get indent width
    pub fn indent_width(&self) -> usize {
        self.indent_width
    }

    /// Set indent width
    pub fn set_indent_width(&mut self, width: usize) {
        self.indent_width = width;
    }

    /// Generate continuation prompt with correct indentation
    pub fn continuation_prompt(&mut self, buffer: &str, base_prompt: &str) -> String {
        let indent_spaces = self.calculate_indent(buffer);
        format!("{}{}", base_prompt, " ".repeat(indent_spaces))
    }
}

impl Default for SmartIndenter {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indenter_creation() {
        let indenter = SmartIndenter::new();
        assert!(indenter.is_ok());
    }

    #[test]
    fn test_indent_simple_incomplete() {
        let mut indenter = SmartIndenter::new().unwrap();
        let indent = indenter.calculate_indent("(+ 1");
        assert_eq!(indent, 2); // 1 unclosed paren * 2 spaces
    }

    #[test]
    fn test_indent_nested() {
        let mut indenter = SmartIndenter::new().unwrap();
        let indent = indenter.calculate_indent("(foo (bar");
        assert_eq!(indent, 4); // 2 unclosed parens * 2 spaces
    }

    #[test]
    fn test_indent_complete() {
        let mut indenter = SmartIndenter::new().unwrap();
        let indent = indenter.calculate_indent("(+ 1 2)");
        assert_eq!(indent, 0); // All closed
    }

    #[test]
    fn test_indent_with_brace() {
        let mut indenter = SmartIndenter::new().unwrap();
        let indent = indenter.calculate_indent("{expr1");
        assert_eq!(indent, 2); // 1 unclosed brace * 2 spaces
    }

    #[test]
    fn test_indent_mixed_delimiters() {
        let mut indenter = SmartIndenter::new().unwrap();
        let indent = indenter.calculate_indent("(foo {bar");
        assert_eq!(indent, 4); // 1 paren + 1 brace * 2 spaces
    }

    #[test]
    fn test_indent_with_string() {
        let mut indenter = SmartIndenter::new().unwrap();
        // (print "(" ) - complete expression, string content ignored
        let indent = indenter.calculate_indent(r#"(print "(")"#);
        assert_eq!(indent, 0); // Complete: 1 open paren, 1 close paren

        // (print "(" - incomplete, missing closing paren
        let indent2 = indenter.calculate_indent(r#"(print "("#);
        assert_eq!(indent2, 2); // 1 unclosed paren
    }

    #[test]
    fn test_indent_with_comment() {
        let mut indenter = SmartIndenter::new().unwrap();
        let indent = indenter.calculate_indent("(foo ; comment with (\nbar");
        assert_eq!(indent, 2); // Comment ignored, 1 unclosed paren
    }

    #[test]
    fn test_continuation_prompt() {
        let mut indenter = SmartIndenter::new().unwrap();
        let prompt = indenter.continuation_prompt("(+ 1", "...> ");
        assert_eq!(prompt, "...>   "); // Base + 2 spaces
    }

    #[test]
    fn test_custom_indent_width() {
        let mut indenter = SmartIndenter::with_indent_width(4).unwrap();
        let indent = indenter.calculate_indent("(foo");
        assert_eq!(indent, 4); // 1 unclosed paren * 4 spaces
    }

    #[test]
    fn test_set_indent_width() {
        let mut indenter = SmartIndenter::new().unwrap();
        indenter.set_indent_width(4);
        let indent = indenter.calculate_indent("(foo");
        assert_eq!(indent, 4);
    }
}
