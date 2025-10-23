/// Tree-Sitter based parser for MeTTa
///
/// Converts Tree-Sitter parse trees with decomposed semantic node types
/// into the existing SExpr AST used by MeTTaTron's backend.

use crate::sexpr::SExpr;
use tree_sitter::{Node, Parser};

/// Parser that uses Tree-Sitter with semantic node type decomposition
pub struct TreeSitterMettaParser {
    parser: Parser,
}

impl TreeSitterMettaParser {
    /// Create a new Tree-Sitter based MeTTa parser
    pub fn new() -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_metta::language())
            .map_err(|e| format!("Failed to set language: {}", e))?;
        Ok(Self { parser })
    }

    /// Parse MeTTa source code into SExpr AST
    pub fn parse(&mut self, source: &str) -> Result<Vec<SExpr>, String> {
        let tree = self
            .parser
            .parse(source, None)
            .ok_or_else(|| "Failed to parse source".to_string())?;

        let root = tree.root_node();

        // Check for syntax errors in the parse tree
        if root.has_error() {
            return Err(self.format_syntax_error(&root, source));
        }

        self.convert_source_file(root, source)
    }

    /// Convert source_file node (contains multiple expressions)
    fn convert_source_file(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut expressions = Vec::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            // Skip comments
            if matches!(child.kind(), "line_comment" | "block_comment") {
                continue;
            }
            if child.is_named() {
                expressions.extend(self.convert_expression(child, source)?);
            }
        }

        Ok(expressions)
    }

    /// Convert a single expression node
    fn convert_expression(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        match node.kind() {
            "expression" => {
                // Unwrap the expression wrapper
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        return self.convert_expression(child, source);
                    }
                }
                Ok(vec![])
            }
            "list" => self.convert_list(node, source),
            "brace_list" => self.convert_brace_list(node, source),
            "prefixed_expression" => self.convert_prefixed_expression(node, source),
            "atom_expression" => self.convert_atom_expression(node, source),
            _ => Err(format!("Unknown expression kind: {}", node.kind())),
        }
    }

    /// Convert list: (expr expr ...)
    fn convert_list(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut items = Vec::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.is_named() {
                items.extend(self.convert_expression(child, source)?);
            }
        }

        Ok(vec![SExpr::List(items)])
    }

    /// Convert brace_list: {expr expr ...}
    /// Matches sexpr.rs behavior: prepend "{}" atom
    fn convert_brace_list(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut items = vec![SExpr::Atom("{}".to_string())];
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.is_named() {
                items.extend(self.convert_expression(child, source)?);
            }
        }

        Ok(vec![SExpr::List(items)])
    }

    /// Convert prefixed_expression: !expr, ?expr, 'expr
    /// Matches sexpr.rs behavior: convert !(expr) to (! expr)
    fn convert_prefixed_expression(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut cursor = node.walk();
        let mut prefix = None;
        let mut argument = None;

        for child in node.children(&mut cursor) {
            match child.kind() {
                "exclaim_prefix" => prefix = Some("!"),
                "question_prefix" => prefix = Some("?"),
                "quote_prefix" => prefix = Some("'"),
                _ if child.is_named() => {
                    argument = Some(self.convert_expression(child, source)?);
                }
                _ => {}
            }
        }

        match (prefix, argument) {
            (Some(p), Some(args)) => {
                let mut items = vec![SExpr::Atom(p.to_string())];
                items.extend(args);
                Ok(vec![SExpr::List(items)])
            }
            _ => Err("Invalid prefixed expression".to_string()),
        }
    }

    /// Convert atom_expression - uses decomposed semantic types
    fn convert_atom_expression(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.is_named() {
                return self.convert_atom(child, source);
            }
        }

        Err("Empty atom expression".to_string())
    }

    /// Convert specific atom types (decomposed for semantics)
    fn convert_atom(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let text = self.node_text(node, source)?;

        match node.kind() {
            // Variables: $var, &var, 'var
            "variable" => Ok(vec![SExpr::Atom(text)]),

            // Wildcard: _
            "wildcard" => Ok(vec![SExpr::Atom(text)]),

            // Identifiers: regular names
            "identifier" => Ok(vec![SExpr::Atom(text)]),

            // Boolean literals
            "boolean_literal" => Ok(vec![SExpr::Atom(text)]),

            // All operator types (already decomposed by grammar)
            "operator" | "arrow_operator" | "comparison_operator" | "assignment_operator"
            | "type_annotation_operator" | "rule_definition_operator"
            | "punctuation_operator" | "arithmetic_operator" | "logic_operator" => {
                Ok(vec![SExpr::Atom(text)])
            }

            // String literal: remove quotes and process escapes
            "string_literal" => {
                let unquoted = self.unescape_string(&text)?;
                Ok(vec![SExpr::String(unquoted)])
            }

            // Float literal: parse to f64
            "float_literal" => {
                let num = text
                    .parse::<f64>()
                    .map_err(|e| format!("Invalid float '{}': {}", text, e))?;
                Ok(vec![SExpr::Float(num)])
            }

            // Integer literal: parse to i64
            "integer_literal" => {
                let num = text
                    .parse::<i64>()
                    .map_err(|e| format!("Invalid integer '{}': {}", text, e))?;
                Ok(vec![SExpr::Integer(num)])
            }

            _ => Err(format!("Unknown atom kind: {}", node.kind())),
        }
    }

    /// Get text for a node
    fn node_text(&self, node: Node, source: &str) -> Result<String, String> {
        let start = node.start_byte();
        let end = node.end_byte();
        Ok(source[start..end].to_string())
    }

    /// Format a syntax error message from the parse tree
    fn format_syntax_error(&self, node: &Node, source: &str) -> String {
        // Find the first ERROR node
        let mut cursor = node.walk();
        if self.find_error_node(&mut cursor) {
            let error_node = cursor.node();
            let start = error_node.start_position();
            let end = error_node.end_position();

            // Extract the problematic text
            let error_text = &source[error_node.start_byte()..error_node.end_byte()];

            return format!(
                "Syntax error at line {}, column {}: unexpected '{}'",
                start.row + 1,
                start.column + 1,
                error_text
            );
        }

        "Syntax error in source code".to_string()
    }

    /// Find the first ERROR node in the tree
    fn find_error_node(&self, cursor: &mut tree_sitter::TreeCursor) -> bool {
        if cursor.node().is_error() || cursor.node().is_missing() {
            return true;
        }

        if cursor.goto_first_child() {
            loop {
                if self.find_error_node(cursor) {
                    return true;
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }

        false
    }

    /// Unescape string literal (remove quotes and process escapes)
    fn unescape_string(&self, s: &str) -> Result<String, String> {
        if !s.starts_with('"') || !s.ends_with('"') {
            return Err(format!("Invalid string literal: {}", s));
        }

        let inner = &s[1..s.len() - 1];
        let mut result = String::new();
        let mut chars = inner.chars();

        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('r') => result.push('\r'),
                    Some('\\') => result.push('\\'),
                    Some('"') => result.push('"'),
                    Some(other) => {
                        result.push('\\');
                        result.push(other);
                    }
                    None => return Err("Unterminated escape sequence".to_string()),
                }
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }
}

impl Default for TreeSitterMettaParser {
    fn default() -> Self {
        Self::new().expect("Failed to create TreeSitterMettaParser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_atoms() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Variables
        let result = parser.parse("$x").unwrap();
        assert_eq!(result, vec![SExpr::Atom("$x".to_string())]);

        // & is now an operator (space reference), not a variable prefix
        let result = parser.parse("&y").unwrap();
        assert_eq!(result, vec![SExpr::Atom("&".to_string()), SExpr::Atom("y".to_string())]);

        // Wildcard
        let result = parser.parse("_").unwrap();
        assert_eq!(result, vec![SExpr::Atom("_".to_string())]);

        // Identifier
        let result = parser.parse("foo").unwrap();
        assert_eq!(result, vec![SExpr::Atom("foo".to_string())]);

        // Operators
        let result = parser.parse("=").unwrap();
        assert_eq!(result, vec![SExpr::Atom("=".to_string())]);
    }

    #[test]
    fn test_parse_literals() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Integer
        let result = parser.parse("42").unwrap();
        assert_eq!(result, vec![SExpr::Integer(42)]);

        let result = parser.parse("-17").unwrap();
        assert_eq!(result, vec![SExpr::Integer(-17)]);

        // String
        let result = parser.parse(r#""hello""#).unwrap();
        assert_eq!(result, vec![SExpr::String("hello".to_string())]);

        // String with escapes
        let result = parser.parse(r#""hello\nworld""#).unwrap();
        assert_eq!(result, vec![SExpr::String("hello\nworld".to_string())]);
    }

    #[test]
    fn test_parse_lists() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Simple list
        let result = parser.parse("(+ 1 2)").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("+".to_string()),
                SExpr::Integer(1),
                SExpr::Integer(2),
            ])]
        );

        // Nested list
        let result = parser.parse("(+ (* 2 3) 4)").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("+".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("*".to_string()),
                    SExpr::Integer(2),
                    SExpr::Integer(3),
                ]),
                SExpr::Integer(4),
            ])]
        );
    }

    #[test]
    fn test_parse_prefixed_expressions() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // ! prefix
        let result = parser.parse("!(+ 1 2)").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("!".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("+".to_string()),
                    SExpr::Integer(1),
                    SExpr::Integer(2),
                ])
            ])]
        );

        // ? prefix
        let result = parser.parse("?query").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("?".to_string()),
                SExpr::Atom("query".to_string()),
            ])]
        );
    }

    #[test]
    fn test_parse_brace_list() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Brace list with {} atom prepended
        let result = parser.parse("{a b c}").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("{}".to_string()),
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string()),
            ])]
        );
    }

    #[test]
    fn test_parse_multiple_expressions() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        let result = parser.parse("(= (double $x) (* $x 2)) !(double 21)").unwrap();
        assert_eq!(result.len(), 2);

        // First: (= (double $x) (* $x 2))
        match &result[0] {
            SExpr::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("=".to_string()));
            }
            _ => panic!("Expected list"),
        }

        // Second: !(double 21)
        match &result[1] {
            SExpr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], SExpr::Atom("!".to_string()));
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parse_with_comments() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Line comments should be ignored
        let result = parser
            .parse(
                r#"
            ; This is a comment
            // Another comment style
            (+ 1 2)
            "#,
            )
            .unwrap();

        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("+".to_string()),
                SExpr::Integer(1),
                SExpr::Integer(2),
            ])]
        );

        // Block comments
        let result = parser
            .parse(
                r#"
            /* Block comment */
            (+ 1 2)
            "#,
            )
            .unwrap();

        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("+".to_string()),
                SExpr::Integer(1),
                SExpr::Integer(2),
            ])]
        );
    }

    #[test]
    fn test_parse_floats() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Simple float
        let result = parser.parse("3.14").unwrap();
        assert_eq!(result, vec![SExpr::Float(3.14)]);

        // Negative float
        let result = parser.parse("-2.5").unwrap();
        assert_eq!(result, vec![SExpr::Float(-2.5)]);

        // Scientific notation
        let result = parser.parse("1.0e10").unwrap();
        assert_eq!(result, vec![SExpr::Float(1.0e10)]);

        let result = parser.parse("-1.5e-3").unwrap();
        assert_eq!(result, vec![SExpr::Float(-1.5e-3)]);

        // In expressions
        let result = parser.parse("(+ 3.14 2.71)").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom("+".to_string()),
                SExpr::Float(3.14),
                SExpr::Float(2.71),
            ])]
        );
    }

    #[test]
    fn test_parse_type_annotation() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Type annotation: (: Socrates Entity)
        let result = parser.parse("(: Socrates Entity)").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom(":".to_string()),
                SExpr::Atom("Socrates".to_string()),
                SExpr::Atom("Entity".to_string()),
            ])]
        );
    }

    #[test]
    fn test_parse_rule_definition() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Rule definition: (:= (Add $x Z) $x)
        let result = parser.parse("(:= (Add $x Z) $x)").unwrap();
        assert_eq!(
            result,
            vec![SExpr::List(vec![
                SExpr::Atom(":=".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("Add".to_string()),
                    SExpr::Atom("$x".to_string()),
                    SExpr::Atom("Z".to_string()),
                ]),
                SExpr::Atom("$x".to_string()),
            ])]
        );
    }
}
