//! Tree-Sitter query-based syntax highlighter
//!
//! This module provides syntax highlighting using Tree-Sitter queries
//! from tree-sitter-metta/queries/highlights.scm

use rustyline::highlight::Highlighter;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

/// ANSI color codes for terminal output
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const COMMENT: &str = "\x1b[90m";        // Bright black (gray)
    pub const STRING: &str = "\x1b[32m";         // Green
    pub const NUMBER: &str = "\x1b[33m";         // Yellow
    pub const BOOLEAN: &str = "\x1b[35m";        // Magenta
    pub const VARIABLE: &str = "\x1b[36m";       // Cyan
    pub const VARIABLE_SPECIAL: &str = "\x1b[96m"; // Bright cyan
    pub const FUNCTION: &str = "\x1b[34m";       // Blue
    pub const OPERATOR: &str = "\x1b[91m";       // Bright red
    pub const KEYWORD: &str = "\x1b[95m";        // Bright magenta
    pub const PUNCTUATION: &str = "\x1b[37m";    // White
}

/// Query-based syntax highlighter using Tree-Sitter
pub struct QueryHighlighter {
    parser: Parser,
    query: Query,
}

impl QueryHighlighter {
    /// Create a new query highlighter
    pub fn new() -> Result<Self, String> {
        // Initialize Tree-Sitter parser
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_metta::language())
            .map_err(|e| format!("Failed to set language: {}", e))?;

        // Load highlight queries from tree-sitter-metta
        let query_source = include_str!("../../tree-sitter-metta/queries/highlights.scm");
        let query = Query::new(&tree_sitter_metta::language(), query_source)
            .map_err(|e| format!("Failed to load highlight query: {}", e))?;

        Ok(Self { parser, query })
    }

    /// Map capture name to ANSI color code
    fn capture_to_color(&self, capture_name: &str) -> &'static str {
        match capture_name {
            "comment" => colors::COMMENT,
            "string" => colors::STRING,
            "number" | "number.float" => colors::NUMBER,
            "boolean" => colors::BOOLEAN,
            "variable" => colors::VARIABLE,
            "variable.special" => colors::VARIABLE_SPECIAL,
            "function" => colors::FUNCTION,
            "operator" | "operator.type" => colors::OPERATOR,
            "keyword" | "keyword.operator" => colors::KEYWORD,
            "punctuation.bracket" | "punctuation.delimiter" => colors::PUNCTUATION,
            _ => colors::RESET,
        }
    }

    /// Highlight source code with ANSI color codes
    pub fn highlight_code(&mut self, source: &str) -> String {
        // Parse the source code
        let tree = match self.parser.parse(source, None) {
            Some(tree) => tree,
            None => return source.to_string(), // Return unhighlighted on parse failure
        };

        let root_node = tree.root_node();
        let mut cursor = QueryCursor::new();

        // Collect all captures with their ranges and colors
        let mut highlights: Vec<(usize, usize, &str)> = Vec::new();

        // Manually collect matches (tree-sitter 0.25 doesn't implement Iterator)
        let mut mat_iter = cursor.matches(&self.query, root_node, source.as_bytes());
        while let Some(mat) = mat_iter.next() {
            for capture in mat.captures {
                let capture_name = &self.query.capture_names()[capture.index as usize];
                let color = self.capture_to_color(capture_name);
                let start = capture.node.start_byte();
                let end = capture.node.end_byte();
                highlights.push((start, end, color));
            }
        }

        // Sort by start position (stable sort maintains precedence order)
        highlights.sort_by_key(|&(start, _, _)| start);

        // Apply colors to source text
        if highlights.is_empty() {
            return source.to_string();
        }

        let mut result = String::with_capacity(source.len() + highlights.len() * 10);
        let mut pos = 0;

        for (start, end, color) in highlights {
            // Skip overlapping highlights (keep first match)
            if start < pos {
                continue;
            }

            // Add text before highlight
            if start > pos {
                result.push_str(&source[pos..start]);
            }

            // Add highlighted text
            result.push_str(color);
            result.push_str(&source[start..end]);
            result.push_str(colors::RESET);

            pos = end;
        }

        // Add remaining text
        if pos < source.len() {
            result.push_str(&source[pos..]);
        }

        result
    }
}

impl Highlighter for QueryHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        // Create a temporary parser and query for each line
        // (rustyline's Highlighter trait requires &self, not &mut self)
        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_metta::language()).is_err() {
            return std::borrow::Cow::Borrowed(line);
        }

        let tree = match parser.parse(line, None) {
            Some(tree) => tree,
            None => return std::borrow::Cow::Borrowed(line),
        };

        let root_node = tree.root_node();
        let mut cursor = QueryCursor::new();

        let mut highlights: Vec<(usize, usize, &str)> = Vec::new();

        // Manually collect matches (tree-sitter 0.25 doesn't implement Iterator)
        let mut mat_iter = cursor.matches(&self.query, root_node, line.as_bytes());
        while let Some(mat) = mat_iter.next() {
            for capture in mat.captures {
                let capture_name = &self.query.capture_names()[capture.index as usize];
                let color = self.capture_to_color(capture_name);
                let start = capture.node.start_byte();
                let end = capture.node.end_byte();
                highlights.push((start, end, color));
            }
        }

        if highlights.is_empty() {
            return std::borrow::Cow::Borrowed(line);
        }

        highlights.sort_by_key(|&(start, _, _)| start);

        let mut result = String::with_capacity(line.len() + highlights.len() * 10);
        let mut pos = 0;

        for (start, end, color) in highlights {
            if start < pos {
                continue;
            }

            if start > pos {
                result.push_str(&line[pos..start]);
            }

            result.push_str(color);
            result.push_str(&line[start..end]);
            result.push_str(colors::RESET);

            pos = end;
        }

        if pos < line.len() {
            result.push_str(&line[pos..]);
        }

        std::borrow::Cow::Owned(result)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        // Enable character-level highlighting for better responsiveness
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlighter_creation() {
        let highlighter = QueryHighlighter::new();
        assert!(highlighter.is_ok(), "Failed to create highlighter");
    }

    #[test]
    fn test_highlight_simple_expression() {
        let mut highlighter = QueryHighlighter::new().unwrap();
        let source = "(+ 1 2)";
        let highlighted = highlighter.highlight_code(source);

        // Should contain ANSI codes
        assert!(highlighted.contains("\x1b["), "Expected ANSI color codes");
        assert!(highlighted.len() > source.len(), "Expected colored output to be longer");
    }

    #[test]
    fn test_highlight_comment() {
        let mut highlighter = QueryHighlighter::new().unwrap();
        let source = "; This is a comment";
        let highlighted = highlighter.highlight_code(source);

        // Should contain comment color
        assert!(highlighted.contains(colors::COMMENT), "Expected comment highlighting");
    }

    #[test]
    fn test_highlight_string() {
        let mut highlighter = QueryHighlighter::new().unwrap();
        let source = r#""hello world""#;
        let highlighted = highlighter.highlight_code(source);

        // Should contain string color
        assert!(highlighted.contains(colors::STRING), "Expected string highlighting");
    }

    #[test]
    fn test_highlight_variable() {
        let mut highlighter = QueryHighlighter::new().unwrap();
        let source = "(= $x 42)";
        let highlighted = highlighter.highlight_code(source);

        // Should contain variable color
        assert!(highlighted.contains(colors::VARIABLE), "Expected variable highlighting");
    }
}
