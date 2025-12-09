//! Tree-Sitter based parser for MeTTa
//!
//! Converts Tree-Sitter parse trees with decomposed semantic node types
//! into the existing SExpr AST used by MeTTaTron's backend.

use crate::ir::{Position, SExpr, Span};
use tree_sitter::{Node, Parser};

/// Structured syntax error with location and type information
#[derive(Debug, Clone)]
pub struct SyntaxError {
    pub kind: SyntaxErrorKind,
    pub line: usize,
    pub column: usize,
    pub text: String,
}

/// Categorized syntax error kinds for pattern matching
#[derive(Debug, Clone, PartialEq)]
pub enum SyntaxErrorKind {
    /// Unexpected token in input
    UnexpectedToken,
    /// Unclosed opening delimiter (e.g., '(', '[', '{')
    UnclosedDelimiter(char),
    /// Extra closing delimiter without matching open
    ExtraClosingDelimiter(char),
    /// String literal not properly closed
    UnclosedString,
    /// Invalid escape sequence in string
    InvalidEscape(String),
    /// Unknown node kind from parser
    UnknownNodeKind(String),
    /// Parser initialization failed
    ParserInit(String),
    /// Generic/fallback error
    Generic,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Syntax error at line {}, column {}: ",
            self.line, self.column
        )?;
        match &self.kind {
            SyntaxErrorKind::UnexpectedToken => write!(f, "unexpected '{}'", self.text),
            SyntaxErrorKind::UnclosedDelimiter(c) => write!(f, "unclosed '{}'", c),
            SyntaxErrorKind::ExtraClosingDelimiter(c) => write!(f, "unexpected closing '{}'", c),
            SyntaxErrorKind::UnclosedString => write!(f, "unclosed string literal"),
            SyntaxErrorKind::InvalidEscape(s) => write!(f, "invalid escape sequence '{}'", s),
            SyntaxErrorKind::UnknownNodeKind(k) => write!(f, "unknown syntax '{}'", k),
            SyntaxErrorKind::ParserInit(msg) => write!(f, "parser initialization failed: {}", msg),
            SyntaxErrorKind::Generic => write!(f, "invalid syntax"),
        }
    }
}

impl std::error::Error for SyntaxError {}

/// Count delimiter balance in source (positive = unclosed, negative = extra close)
fn count_delimiter_balance(source: &str, open: char, close: char) -> i32 {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in source.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            c if c == open && !in_string => depth += 1,
            c if c == close && !in_string => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Check if source has an unclosed string literal
fn has_unclosed_string(source: &str) -> bool {
    let mut in_string = false;
    let mut escape_next = false;

    for ch in source.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            _ => {}
        }
    }
    in_string
}

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
    pub fn parse(&mut self, source: &str) -> Result<Vec<SExpr>, SyntaxError> {
        let tree = self.parser.parse(source, None).ok_or_else(|| SyntaxError {
            kind: SyntaxErrorKind::Generic,
            line: 1,
            column: 1,
            text: "Failed to parse source".into(),
        })?;

        let root = tree.root_node();

        // Check for syntax errors in the parse tree
        if root.has_error() {
            return Err(self.create_syntax_error(&root, source));
        }

        self.convert_source_file(root, source)
            .map_err(|e| SyntaxError {
                kind: SyntaxErrorKind::Generic,
                line: 1,
                column: 1,
                text: e,
            })
    }

    /// Check if a node should be processed (named and not extra)
    ///
    /// This filters out extras like comments and whitespace while preserving
    /// them in the Tree-Sitter CST for future LSP server access.
    #[inline]
    fn should_process_node(&self, node: Node) -> bool {
        node.is_named() && !node.is_extra()
    }

    /// Convert source_file node (contains multiple expressions)
    fn convert_source_file(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut expressions = Vec::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if self.should_process_node(child) {
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
                    if self.should_process_node(child) {
                        return self.convert_expression(child, source);
                    }
                }
                Ok(vec![])
            }
            "list" => self.convert_list(node, source),
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
            if self.should_process_node(child) {
                items.extend(self.convert_expression(child, source)?);
            }
        }

        let span = self.node_span(node);
        Ok(vec![SExpr::List(items, Some(span))])
    }

    /// Convert prefixed_expression: !expr, ?expr, 'expr
    /// Matches sexpr.rs behavior: convert !(expr) to (! expr)
    fn convert_prefixed_expression(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let span = self.node_span(node);
        let mut cursor = node.walk();
        let mut prefix = None;
        let mut prefix_span = None;
        let mut argument = None;

        for child in node.children(&mut cursor) {
            match child.kind() {
                "exclaim_prefix" => {
                    prefix = Some("!");
                    prefix_span = Some(self.node_span(child));
                }
                "question_prefix" => {
                    prefix = Some("?");
                    prefix_span = Some(self.node_span(child));
                }
                "quote_prefix" => {
                    prefix = Some("'");
                    prefix_span = Some(self.node_span(child));
                }
                _ if self.should_process_node(child) => {
                    argument = Some(self.convert_expression(child, source)?);
                }
                _ => {}
            }
        }

        match (prefix, prefix_span, argument) {
            (Some(p), Some(p_span), Some(args)) => {
                let mut items = vec![SExpr::Atom(p.to_string(), Some(p_span))];
                items.extend(args);
                Ok(vec![SExpr::List(items, Some(span))])
            }
            _ => Err("Invalid prefixed expression".to_string()),
        }
    }

    /// Convert atom_expression - uses decomposed semantic types
    fn convert_atom_expression(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if self.should_process_node(child) {
                return self.convert_atom(child, source);
            }
        }

        Err("Empty atom expression".to_string())
    }

    /// Convert specific atom types (decomposed for semantics)
    fn convert_atom(&self, node: Node, source: &str) -> Result<Vec<SExpr>, String> {
        let text = self.node_text(node, source)?;
        let span = self.node_span(node);

        match node.kind() {
            // Variables: $var
            "variable" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // Space references: &name (e.g., &self, &kb, &workspace)
            "space_reference" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // Wildcard: _
            "wildcard" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // Identifiers: regular names
            "identifier" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // Boolean literals
            "boolean_literal" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // Special type symbols: %Undefined%, %Irreducible%, etc.
            "special_type_symbol" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // All operator types (already decomposed by grammar)
            "operator"
            | "arrow_operator"
            | "comparison_operator"
            | "assignment_operator"
            | "type_annotation_operator"
            | "rule_definition_operator"
            | "punctuation_operator"
            | "arithmetic_operator"
            | "logic_operator" => Ok(vec![SExpr::Atom(text, Some(span))]),

            // String literal: remove quotes and process escapes
            "string_literal" => {
                let unquoted = self.unescape_string(&text)?;
                Ok(vec![SExpr::String(unquoted, Some(span))])
            }

            // Float literal: parse to f64
            "float_literal" => {
                let num = text
                    .parse::<f64>()
                    .map_err(|e| format!("Invalid float '{}': {}", text, e))?;
                Ok(vec![SExpr::Float(num, Some(span))])
            }

            // Integer literal: parse to i64
            "integer_literal" => {
                let num = text
                    .parse::<i64>()
                    .map_err(|e| format!("Invalid integer '{}': {}", text, e))?;
                Ok(vec![SExpr::Integer(num, Some(span))])
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

    /// Create a structured syntax error from the parse tree
    fn create_syntax_error(&self, node: &Node, source: &str) -> SyntaxError {
        let mut cursor = node.walk();
        if self.find_error_node(&mut cursor) {
            let error_node = cursor.node();
            let start = error_node.start_position();
            let error_text = source[error_node.start_byte()..error_node.end_byte()].to_string();
            let kind = self.analyze_error_kind(source);

            SyntaxError {
                kind,
                line: start.row + 1,
                column: start.column + 1,
                text: error_text,
            }
        } else {
            SyntaxError {
                kind: SyntaxErrorKind::Generic,
                line: 1,
                column: 1,
                text: String::new(),
            }
        }
    }

    /// Analyze source to determine the specific error kind
    fn analyze_error_kind(&self, source: &str) -> SyntaxErrorKind {
        // Check for unclosed string FIRST - unclosed strings affect delimiter counting
        if has_unclosed_string(source) {
            return SyntaxErrorKind::UnclosedString;
        }

        // Check parenthesis balance
        let paren_balance = count_delimiter_balance(source, '(', ')');
        if paren_balance > 0 {
            return SyntaxErrorKind::UnclosedDelimiter('(');
        } else if paren_balance < 0 {
            return SyntaxErrorKind::ExtraClosingDelimiter(')');
        }

        // Check bracket balance
        let bracket_balance = count_delimiter_balance(source, '[', ']');
        if bracket_balance > 0 {
            return SyntaxErrorKind::UnclosedDelimiter('[');
        } else if bracket_balance < 0 {
            return SyntaxErrorKind::ExtraClosingDelimiter(']');
        }

        // Check brace balance
        let brace_balance = count_delimiter_balance(source, '{', '}');
        if brace_balance > 0 {
            return SyntaxErrorKind::UnclosedDelimiter('{');
        } else if brace_balance < 0 {
            return SyntaxErrorKind::ExtraClosingDelimiter('}');
        }

        SyntaxErrorKind::UnexpectedToken
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
    /// Supports: \n, \t, \r, \\, \", \x##, \u{...}
    fn unescape_string(&self, s: &str) -> Result<String, String> {
        if !s.starts_with('"') || !s.ends_with('"') {
            return Err(format!("Invalid string literal: {}", s));
        }

        let inner = &s[1..s.len() - 1];
        let mut result = String::new();
        let mut chars = inner.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('r') => result.push('\r'),
                    Some('\\') => result.push('\\'),
                    Some('"') => result.push('"'),
                    // Hex escape: \x## (2 hex digits)
                    Some('x') => {
                        let hex1 = chars.next().ok_or("Incomplete hex escape sequence")?;
                        let hex2 = chars.next().ok_or("Incomplete hex escape sequence")?;
                        let hex_str = format!("{}{}", hex1, hex2);
                        let byte = u8::from_str_radix(&hex_str, 16)
                            .map_err(|_| format!("Invalid hex escape: \\x{}", hex_str))?;
                        result.push(byte as char);
                    }
                    // Unicode escape: \u{...} (1-6 hex digits)
                    Some('u') => {
                        if chars.next() != Some('{') {
                            return Err(
                                "Invalid unicode escape: expected '{' after \\u".to_string()
                            );
                        }
                        let mut hex_digits = String::new();
                        loop {
                            match chars.next() {
                                Some('}') => break,
                                Some(ch) if ch.is_ascii_hexdigit() => hex_digits.push(ch),
                                Some(ch) => {
                                    return Err(format!(
                                        "Invalid character in unicode escape: '{}'",
                                        ch
                                    ))
                                }
                                None => {
                                    return Err("Unterminated unicode escape sequence".to_string())
                                }
                            }
                            if hex_digits.len() > 6 {
                                return Err(
                                    "Unicode escape too long (max 6 hex digits)".to_string()
                                );
                            }
                        }
                        if hex_digits.is_empty() {
                            return Err("Empty unicode escape sequence".to_string());
                        }
                        let code_point = u32::from_str_radix(&hex_digits, 16).map_err(|_| {
                            format!("Invalid unicode escape: \\u{{{}}}", hex_digits)
                        })?;
                        let unicode_char = char::from_u32(code_point)
                            .ok_or(format!("Invalid unicode code point: U+{:X}", code_point))?;
                        result.push(unicode_char);
                    }
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

    /// Extract span information from a Tree-Sitter node
    fn node_span(&self, node: Node) -> Span {
        let start_pos = node.start_position();
        let end_pos = node.end_position();

        Span::new(
            Position::new(start_pos.row, start_pos.column),
            Position::new(end_pos.row, end_pos.column),
            node.start_byte(),
            node.end_byte(),
        )
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
    /// Helper function to strip spans from MettaExpr for testing
    /// This allows tests to compare parsed results without worrying about position information
    fn strip_spans(expr: &SExpr) -> SExpr {
        match expr {
            SExpr::Atom(s, _) => SExpr::Atom(s.clone(), None),
            SExpr::String(s, _) => SExpr::String(s.clone(), None),
            SExpr::Integer(n, _) => SExpr::Integer(*n, None),
            SExpr::Float(f, _) => SExpr::Float(*f, None),
            SExpr::List(items, _) => {
                let stripped_items = items.iter().map(strip_spans).collect();
                SExpr::List(stripped_items, None)
            }
            SExpr::Quoted(expr, _) => SExpr::Quoted(Box::new(strip_spans(expr)), None),
        }
    }

    /// Helper to strip spans from a vec of expressions
    fn strip_spans_vec(exprs: &[SExpr]) -> Vec<SExpr> {
        exprs.iter().map(strip_spans).collect()
    }

    #[test]
    fn test_parse_simple_atoms() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Variables
        let result = strip_spans_vec(&parser.parse("$x").unwrap());
        assert_eq!(result, vec![SExpr::Atom("$x".to_string(), None)]);

        // & prefix creates space reference token (like $var for variables)
        let result = strip_spans_vec(&parser.parse("&y").unwrap());
        assert_eq!(
            result,
            vec![SExpr::Atom("&y".to_string(), None)]
        );

        // Wildcard
        let result = strip_spans_vec(&parser.parse("_").unwrap());
        assert_eq!(result, vec![SExpr::Atom("_".to_string(), None)]);

        // Identifier
        let result = strip_spans_vec(&parser.parse("foo").unwrap());
        assert_eq!(result, vec![SExpr::Atom("foo".to_string(), None)]);

        // Operators
        let result = strip_spans_vec(&parser.parse("=").unwrap());
        assert_eq!(result, vec![SExpr::Atom("=".to_string(), None)]);
    }

    #[test]
    fn test_parse_literals() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Integer
        let result = strip_spans_vec(&parser.parse("42").unwrap());
        assert_eq!(result, vec![SExpr::Integer(42, None)]);

        let result = strip_spans_vec(&parser.parse("-17").unwrap());
        assert_eq!(result, vec![SExpr::Integer(-17, None)]);

        // String
        let result = strip_spans_vec(&parser.parse(r#""hello""#).unwrap());
        assert_eq!(result, vec![SExpr::String("hello".to_string(), None)]);

        // String with escapes
        let result = strip_spans_vec(&parser.parse(r#""hello\nworld""#).unwrap());
        assert_eq!(
            result,
            vec![SExpr::String("hello\nworld".to_string(), None)]
        );
    }

    #[test]
    fn test_parse_lists() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Simple list
        let result = strip_spans_vec(&parser.parse("(+ 1 2)").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::Integer(1, None),
                    SExpr::Integer(2, None),
                ],
                None
            )]
        );

        // Nested list
        let result = strip_spans_vec(&parser.parse("(+ (* 2 3) 4)").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::List(
                        vec![
                            SExpr::Atom("*".to_string(), None),
                            SExpr::Integer(2, None),
                            SExpr::Integer(3, None),
                        ],
                        None
                    ),
                    SExpr::Integer(4, None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_prefixed_expressions() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // ! prefix
        let result = strip_spans_vec(&parser.parse("!(+ 1 2)").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("!".to_string(), None),
                    SExpr::List(
                        vec![
                            SExpr::Atom("+".to_string(), None),
                            SExpr::Integer(1, None),
                            SExpr::Integer(2, None),
                        ],
                        None
                    )
                ],
                None
            )]
        );

        // ? prefix
        let result = strip_spans_vec(&parser.parse("?query").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("?".to_string(), None),
                    SExpr::Atom("query".to_string(), None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_multiple_expressions() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        let result = strip_spans_vec(
            &parser
                .parse("(= (double $x) (* $x 2)) !(double 21)")
                .unwrap(),
        );
        assert_eq!(result.len(), 2);

        // First: (= (double $x) (* $x 2))
        match &result[0] {
            SExpr::List(items, _) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("=".to_string(), None));
            }
            _ => panic!("Expected list"),
        }

        // Second: !(double 21)
        match &result[1] {
            SExpr::List(items, _) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], SExpr::Atom("!".to_string(), None));
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parse_with_comments() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Semicolon comments should be ignored
        let result = strip_spans_vec(
            &parser
                .parse(
                    r#"
            ; This is a comment
            (+ 1 2)
            ; Another comment
            "#,
                )
                .unwrap(),
        );

        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::Integer(1, None),
                    SExpr::Integer(2, None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_floats() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Simple float
        let result = strip_spans_vec(&parser.parse("3.15").unwrap());
        assert_eq!(result, vec![SExpr::Float(3.15, None)]);

        // Negative float
        let result = strip_spans_vec(&parser.parse("-2.5").unwrap());
        assert_eq!(result, vec![SExpr::Float(-2.5, None)]);

        // Scientific notation
        let result = strip_spans_vec(&parser.parse("1.0e10").unwrap());
        assert_eq!(result, vec![SExpr::Float(1.0e10, None)]);

        let result = strip_spans_vec(&parser.parse("-1.5e-3").unwrap());
        assert_eq!(result, vec![SExpr::Float(-1.5e-3, None)]);

        // In expressions
        let result = strip_spans_vec(&parser.parse("(+ 3.15 2.71)").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::Float(3.15, None),
                    SExpr::Float(2.71, None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_type_annotation() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Type annotation: (: Socrates Entity)
        let result = strip_spans_vec(&parser.parse("(: Socrates Entity)").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom(":".to_string(), None),
                    SExpr::Atom("Socrates".to_string(), None),
                    SExpr::Atom("Entity".to_string(), None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_rule_definition() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Rule definition: (:= (Add $x Z) $x)
        let result = strip_spans_vec(&parser.parse("(:= (Add $x Z) $x)").unwrap());
        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom(":=".to_string(), None),
                    SExpr::List(
                        vec![
                            SExpr::Atom("Add".to_string(), None),
                            SExpr::Atom("$x".to_string(), None),
                            SExpr::Atom("Z".to_string(), None),
                        ],
                        None
                    ),
                    SExpr::Atom("$x".to_string(), None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_special_type_symbols() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // %Undefined% special type
        let result = strip_spans_vec(&parser.parse("%Undefined%").unwrap());
        assert_eq!(result, vec![SExpr::Atom("%Undefined%".to_string(), None)]);

        // %Irreducible% special type
        let result = strip_spans_vec(&parser.parse("%Irreducible%").unwrap());
        assert_eq!(result, vec![SExpr::Atom("%Irreducible%".to_string(), None)]);

        // In type annotation: (: = (-> $t $t %Undefined%))
        let result = strip_spans_vec(&parser.parse("(: = (-> $t $t %Undefined%))").unwrap());
        match &result[0] {
            SExpr::List(items, _) => {
                assert_eq!(items[0], SExpr::Atom(":".to_string(), None));
                assert_eq!(items[1], SExpr::Atom("=".to_string(), None));
                // Third item should be (-> $t $t %Undefined%)
                match &items[2] {
                    SExpr::List(arrow_items, _) => {
                        assert_eq!(arrow_items[0], SExpr::Atom("->".to_string(), None));
                        assert_eq!(arrow_items[3], SExpr::Atom("%Undefined%".to_string(), None));
                    }
                    _ => panic!("Expected arrow type list"),
                }
            }
            _ => panic!("Expected type annotation list"),
        }
    }

    #[test]
    fn test_parse_hex_escape_sequences() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Hex escape \x1b (ESC character)
        let result = strip_spans_vec(&parser.parse(r#""\x1b[31mRed\x1b[0m""#).unwrap());
        assert_eq!(
            result,
            vec![SExpr::String("\x1b[31mRed\x1b[0m".to_string(), None)]
        );

        // Hex escape \x41 (A)
        let result = strip_spans_vec(&parser.parse(r#""\x41""#).unwrap());
        assert_eq!(result, vec![SExpr::String("A".to_string(), None)]);

        // Multiple hex escapes
        let result = strip_spans_vec(&parser.parse(r#""\x48\x65\x6c\x6c\x6f""#).unwrap());
        assert_eq!(result, vec![SExpr::String("Hello".to_string(), None)]);
    }

    #[test]
    fn test_parse_unicode_escape_sequences() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Unicode escape \u{1F4A1} (💡 light bulb emoji)
        let result = strip_spans_vec(&parser.parse(r#""\u{1F4A1}""#).unwrap());
        assert_eq!(result, vec![SExpr::String("💡".to_string(), None)]);

        // Unicode escape \u{0041} (A)
        let result = strip_spans_vec(&parser.parse(r#""\u{0041}""#).unwrap());
        assert_eq!(result, vec![SExpr::String("A".to_string(), None)]);

        // Unicode escape \u{3B1} (α Greek alpha)
        let result = strip_spans_vec(&parser.parse(r#""\u{3B1}""#).unwrap());
        assert_eq!(result, vec![SExpr::String("α".to_string(), None)]);

        // Mixed regular and unicode
        let result = strip_spans_vec(&parser.parse(r#""Hello \u{1F30D}!""#).unwrap());
        assert_eq!(result, vec![SExpr::String("Hello 🌍!".to_string(), None)]);
    }

    // ========================================================================
    // Comment Parsing Tests (from feature branch)
    // ========================================================================

    #[test]
    fn test_parse_inline_comments() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Comment within list
        let result = strip_spans_vec(&parser.parse("(+ 1 ; comment here\n 2)").unwrap());

        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::Integer(1, None),
                    SExpr::Integer(2, None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_multiple_inline_comments() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Multiple comments in one list
        let result = strip_spans_vec(
            &parser
                .parse("(+ ; first comment\n 1 ; second comment\n 2)")
                .unwrap(),
        );

        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::Integer(1, None),
                    SExpr::Integer(2, None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_comment_in_nested_list() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Comment in nested structure
        let result = strip_spans_vec(&parser.parse("(+ (* 2 ; inner comment\n 3) 4)").unwrap());

        assert_eq!(
            result,
            vec![SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::List(
                        vec![
                            SExpr::Atom("*".to_string(), None),
                            SExpr::Integer(2, None),
                            SExpr::Integer(3, None),
                        ],
                        None
                    ),
                    SExpr::Integer(4, None),
                ],
                None
            )]
        );
    }

    #[test]
    fn test_parse_comment_between_list_items() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Comments between major list items (rule definition)
        let result = strip_spans_vec(
            &parser
                .parse(
                    r#"
            (=
                ; Pattern
                (double $x)
                ; Body
                (* $x 2))
            "#,
                )
                .unwrap(),
        );

        assert_eq!(result.len(), 1);
        match &result[0] {
            SExpr::List(items, _) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("=".to_string(), None));
                // Pattern: (double $x)
                match &items[1] {
                    SExpr::List(pattern_items, _) => {
                        assert_eq!(pattern_items.len(), 2);
                        assert_eq!(pattern_items[0], SExpr::Atom("double".to_string(), None));
                        assert_eq!(pattern_items[1], SExpr::Atom("$x".to_string(), None));
                    }
                    _ => panic!("Expected pattern list"),
                }
                // Body: (* $x 2)
                match &items[2] {
                    SExpr::List(body_items, _) => {
                        assert_eq!(body_items.len(), 3);
                        assert_eq!(body_items[0], SExpr::Atom("*".to_string(), None));
                        assert_eq!(body_items[1], SExpr::Atom("$x".to_string(), None));
                        assert_eq!(body_items[2], SExpr::Integer(2, None));
                    }
                    _ => panic!("Expected body list"),
                }
            }
            _ => panic!("Expected rule definition list"),
        }
    }

    #[test]
    fn test_parse_comment_after_top_level_expression() {
        let mut parser = TreeSitterMettaParser::new().unwrap();

        // Comment after a complete expression
        let result = strip_spans_vec(&parser.parse("(+ 1 2) ; result is 3\n(* 3 4)").unwrap());

        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            SExpr::List(
                vec![
                    SExpr::Atom("+".to_string(), None),
                    SExpr::Integer(1, None),
                    SExpr::Integer(2, None),
                ],
                None
            )
        );
        assert_eq!(
            result[1],
            SExpr::List(
                vec![
                    SExpr::Atom("*".to_string(), None),
                    SExpr::Integer(3, None),
                    SExpr::Integer(4, None),
                ],
                None
            )
        );
    }

    // ========================================================================
    // Structured SyntaxError Tests (from main)
    // ========================================================================

    #[test]
    fn test_syntax_error_unclosed_paren() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse("(+ 1 2");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::UnclosedDelimiter('(')),
            "Expected UnclosedDelimiter('('), got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_extra_close_paren() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse("(+ 1 2))");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::ExtraClosingDelimiter(')')),
            "Expected ExtraClosingDelimiter(')'), got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_unclosed_string() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse(r#"(print "hello)"#);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::UnclosedString),
            "Expected UnclosedString, got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_unclosed_bracket() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse("[1 2 3");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::UnclosedDelimiter('[')),
            "Expected UnclosedDelimiter('['), got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_extra_close_bracket() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse("[1 2 3]]");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::ExtraClosingDelimiter(']')),
            "Expected ExtraClosingDelimiter(']'), got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_unclosed_brace() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse("{a b c");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::UnclosedDelimiter('{')),
            "Expected UnclosedDelimiter('{{'), got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_extra_close_brace() {
        let mut parser = TreeSitterMettaParser::new().unwrap();
        let result = parser.parse("{a b c}}");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            matches!(error.kind, SyntaxErrorKind::ExtraClosingDelimiter('}')),
            "Expected ExtraClosingDelimiter('}}'), got {:?}",
            error.kind
        );
    }

    #[test]
    fn test_syntax_error_display() {
        let error = SyntaxError {
            kind: SyntaxErrorKind::UnclosedDelimiter('('),
            line: 1,
            column: 7,
            text: String::new(),
        };
        let msg = error.to_string();
        assert!(msg.contains("line 1"));
        assert!(msg.contains("column 7"));
        assert!(msg.contains("unclosed '('"));
    }

    #[test]
    fn test_syntax_error_kind_variants() {
        // Test Display impl for all variants
        let variants = vec![
            (
                SyntaxError {
                    kind: SyntaxErrorKind::UnexpectedToken,
                    line: 1,
                    column: 1,
                    text: "foo".to_string(),
                },
                "unexpected 'foo'",
            ),
            (
                SyntaxError {
                    kind: SyntaxErrorKind::UnclosedDelimiter('('),
                    line: 1,
                    column: 1,
                    text: String::new(),
                },
                "unclosed '('",
            ),
            (
                SyntaxError {
                    kind: SyntaxErrorKind::ExtraClosingDelimiter(')'),
                    line: 1,
                    column: 1,
                    text: String::new(),
                },
                "unexpected closing ')'",
            ),
            (
                SyntaxError {
                    kind: SyntaxErrorKind::UnclosedString,
                    line: 1,
                    column: 1,
                    text: String::new(),
                },
                "unclosed string",
            ),
            (
                SyntaxError {
                    kind: SyntaxErrorKind::InvalidEscape("z".to_string()),
                    line: 1,
                    column: 1,
                    text: String::new(),
                },
                "invalid escape sequence 'z'",
            ),
            (
                SyntaxError {
                    kind: SyntaxErrorKind::ParserInit("failed".to_string()),
                    line: 0,
                    column: 0,
                    text: String::new(),
                },
                "parser initialization failed",
            ),
            (
                SyntaxError {
                    kind: SyntaxErrorKind::Generic,
                    line: 1,
                    column: 1,
                    text: String::new(),
                },
                "invalid syntax",
            ),
        ];

        for (error, expected_substring) in variants {
            let msg = error.to_string();
            assert!(
                msg.contains(expected_substring),
                "Error message '{}' should contain '{}'",
                msg,
                expected_substring
            );
        }
    }

    #[test]
    fn test_helper_count_delimiter_balance() {
        // Balanced
        assert_eq!(count_delimiter_balance("(+ 1 2)", '(', ')'), 0);
        assert_eq!(count_delimiter_balance("((a) (b))", '(', ')'), 0);

        // Unclosed
        assert_eq!(count_delimiter_balance("(+ 1 2", '(', ')'), 1);
        assert_eq!(count_delimiter_balance("((a)", '(', ')'), 1);
        assert_eq!(count_delimiter_balance("(((", '(', ')'), 3);

        // Extra closing
        assert_eq!(count_delimiter_balance("(+ 1 2))", '(', ')'), -1);
        assert_eq!(count_delimiter_balance(")))", '(', ')'), -3);

        // Inside strings should be ignored
        assert_eq!(count_delimiter_balance(r#"("(" ")")"#, '(', ')'), 0);
        assert_eq!(count_delimiter_balance(r#"(print "(((")"#, '(', ')'), 0);
    }

    #[test]
    fn test_helper_has_unclosed_string() {
        // Closed strings
        assert!(!has_unclosed_string(r#""hello""#));
        assert!(!has_unclosed_string(r#""hello" "world""#));
        assert!(!has_unclosed_string(r#"(print "test")"#));

        // Unclosed strings
        assert!(has_unclosed_string(r#""hello"#));
        assert!(has_unclosed_string(r#"(print "test)"#));

        // Escaped quotes should be handled
        assert!(!has_unclosed_string(r#""hello \"world\"""#));
        assert!(has_unclosed_string(r#""hello \""#));
    }
}
