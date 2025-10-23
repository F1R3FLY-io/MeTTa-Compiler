/// S-expression lexer and parser for MeTTa
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    LParen,
    RParen,
    LBrace,
    RBrace,
    Symbol(String),
    String(String),
    Integer(i64),
    Float(f64),
    Dot,
    Exclaim,
    Question,
    Colon,
    Arrow,
    Equals,
    Semicolon,
    Pipe,
    Comma,
    Ampersand,
    Ellipsis,
    At,
    LeftArrow,       // <-
    DoubleLeftArrow, // <=
    TripleLeftArrow, // <<-
    QuestionExclaim, // ?!
    ExclaimQuestion, // !?
    Eof,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::Symbol(s) => write!(f, "{}", s),
            Token::String(s) => write!(f, "\"{}\"", s),
            Token::Integer(i) => write!(f, "{}", i),
            Token::Float(fl) => write!(f, "{}", fl),
            Token::Dot => write!(f, "."),
            Token::Exclaim => write!(f, "!"),
            Token::Question => write!(f, "?"),
            Token::Colon => write!(f, ":"),
            Token::Arrow => write!(f, "->"),
            Token::Equals => write!(f, "="),
            Token::Semicolon => write!(f, ";"),
            Token::Pipe => write!(f, "|"),
            Token::Comma => write!(f, ","),
            Token::Ampersand => write!(f, "&"),
            Token::Ellipsis => write!(f, "..."),
            Token::At => write!(f, "@"),
            Token::LeftArrow => write!(f, "<-"),
            Token::DoubleLeftArrow => write!(f, "<="),
            Token::TripleLeftArrow => write!(f, "<<-"),
            Token::QuestionExclaim => write!(f, "?!"),
            Token::ExclaimQuestion => write!(f, "!?"),
            Token::Eof => write!(f, "EOF"),
        }
    }
}

/// Hand-written lexer for MeTTa
///
/// **DEPRECATED**: Use `TreeSitterMettaParser` instead for better error recovery,
/// incremental parsing, and full MeTTa language support.
#[deprecated(since = "0.1.2", note = "Use TreeSitterMettaParser instead")]
pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Lexer {
            input: input.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    fn current(&self) -> Option<char> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        let pos = self.pos + offset;
        if pos < self.input.len() {
            Some(self.input[pos])
        } else {
            None
        }
    }

    fn advance(&mut self) -> Option<char> {
        if self.pos < self.input.len() {
            let ch = self.input[self.pos];
            self.pos += 1;
            if ch == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            Some(ch)
        } else {
            None
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.current() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(ch) = self.current() {
            self.advance();
            if ch == '\n' {
                break;
            }
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), String> {
        // Already consumed '/'
        while let Some(ch) = self.advance() {
            if ch == '*' && self.current() == Some('/') {
                self.advance(); // consume '/'
                return Ok(());
            }
        }
        Err("Unclosed block comment".to_string())
    }

    fn read_string(&mut self) -> Result<String, String> {
        let mut result = String::new();
        self.advance(); // consume opening quote

        while let Some(ch) = self.current() {
            if ch == '"' {
                self.advance(); // consume closing quote
                return Ok(result);
            } else if ch == '\\' {
                self.advance();
                match self.current() {
                    Some('n') => {
                        result.push('\n');
                        self.advance();
                    }
                    Some('t') => {
                        result.push('\t');
                        self.advance();
                    }
                    Some('\\') => {
                        result.push('\\');
                        self.advance();
                    }
                    Some('"') => {
                        result.push('"');
                        self.advance();
                    }
                    Some(c) => {
                        result.push(c);
                        self.advance();
                    }
                    None => return Err("Unexpected end of string".to_string()),
                }
            } else {
                result.push(ch);
                self.advance();
            }
        }
        Err("Unclosed string literal".to_string())
    }

    fn read_number(&mut self) -> i64 {
        let mut result = String::new();
        while let Some(ch) = self.current() {
            if ch.is_numeric() {
                result.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        result.parse().unwrap_or(0)
    }

    /// Read a number (integer or float) starting from current position
    /// Returns either Token::Integer or Token::Float
    fn read_number_token(&mut self) -> Token {
        let mut result = String::new();

        // Read integer part
        while let Some(ch) = self.current() {
            if ch.is_numeric() {
                result.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        // Check for decimal point
        if self.current() == Some('.') && self.peek(1).map(|c| c.is_numeric()).unwrap_or(false) {
            result.push('.');
            self.advance();

            // Read fractional part
            while let Some(ch) = self.current() {
                if ch.is_numeric() {
                    result.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }

            // Check for scientific notation (e or E)
            if matches!(self.current(), Some('e') | Some('E')) {
                result.push(self.current().unwrap());
                self.advance();

                // Optional sign
                if matches!(self.current(), Some('+') | Some('-')) {
                    result.push(self.current().unwrap());
                    self.advance();
                }

                // Exponent digits
                while let Some(ch) = self.current() {
                    if ch.is_numeric() {
                        result.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
            }

            Token::Float(result.parse().unwrap_or(0.0))
        } else {
            Token::Integer(result.parse().unwrap_or(0))
        }
    }

    fn read_symbol(&mut self) -> String {
        let mut result = String::new();
        while let Some(ch) = self.current() {
            if ch.is_alphanumeric()
                || ch == '_'
                || ch == '-'
                || ch == '+'
                || ch == '*'
                || ch == '/'
                || ch == '$'
                || ch == '&'
                || ch == '\''
                || ch == '>'
            {
                result.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        result
    }

    pub fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace();

        // Handle comments
        while let Some(ch) = self.current() {
            if ch == ';' {
                self.skip_line_comment();
                self.skip_whitespace();
            } else if ch == '/' && self.peek(1) == Some('/') {
                self.advance();
                self.advance();
                self.skip_line_comment();
                self.skip_whitespace();
            } else if ch == '/' && self.peek(1) == Some('*') {
                self.advance();
                self.advance();
                self.skip_block_comment()?;
                self.skip_whitespace();
            } else {
                break;
            }
        }

        match self.current() {
            None => Ok(Token::Eof),
            Some('(') => {
                self.advance();
                Ok(Token::LParen)
            }
            Some(')') => {
                self.advance();
                Ok(Token::RParen)
            }
            Some('{') => {
                self.advance();
                Ok(Token::LBrace)
            }
            Some('}') => {
                self.advance();
                Ok(Token::RBrace)
            }
            Some('"') => Ok(Token::String(self.read_string()?)),
            Some('.') => {
                self.advance();
                if self.current() == Some('.') && self.peek(1) == Some('.') {
                    self.advance();
                    self.advance();
                    Ok(Token::Ellipsis)
                } else {
                    Ok(Token::Dot)
                }
            }
            Some('!') => {
                self.advance();
                if self.current() == Some('?') {
                    self.advance();
                    Ok(Token::ExclaimQuestion)
                } else {
                    Ok(Token::Exclaim)
                }
            }
            Some('?') => {
                self.advance();
                if self.current() == Some('!') {
                    self.advance();
                    Ok(Token::QuestionExclaim)
                } else {
                    Ok(Token::Question)
                }
            }
            Some(':') => {
                self.advance();
                Ok(Token::Colon)
            }
            Some('=') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    let sym = "==".to_string();
                    Ok(Token::Symbol(sym))
                } else {
                    Ok(Token::Equals)
                }
            }
            Some('|') => {
                self.advance();
                Ok(Token::Pipe)
            }
            Some(',') => {
                self.advance();
                Ok(Token::Comma)
            }
            Some('&') => {
                self.advance();
                Ok(Token::Ampersand)
            }
            Some('@') => {
                self.advance();
                Ok(Token::At)
            }
            Some('<') => {
                self.advance();
                if self.current() == Some('-') {
                    self.advance();
                    Ok(Token::LeftArrow)
                } else if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::DoubleLeftArrow)
                } else if self.current() == Some('<') && self.peek(1) == Some('-') {
                    self.advance();
                    self.advance();
                    Ok(Token::TripleLeftArrow)
                } else {
                    // Read as symbol
                    let mut sym = String::from("<");
                    sym.push_str(&self.read_symbol());
                    Ok(Token::Symbol(sym))
                }
            }
            Some('-') => {
                self.advance();
                if self.current() == Some('>') {
                    self.advance();
                    Ok(Token::Arrow)
                } else if self.current().map(|c| c.is_numeric()).unwrap_or(false) {
                    // Read number (could be int or float) and negate it
                    match self.read_number_token() {
                        Token::Integer(n) => Ok(Token::Integer(-n)),
                        Token::Float(f) => Ok(Token::Float(-f)),
                        _ => unreachable!(),
                    }
                } else {
                    let mut sym = String::from("-");
                    while let Some(ch) = self.current() {
                        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                            sym.push(ch);
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    Ok(Token::Symbol(sym))
                }
            }
            Some(ch) if ch.is_numeric() => {
                Ok(self.read_number_token())
            }
            Some(_) => {
                let sym = self.read_symbol();
                if sym.is_empty() {
                    let ch = self.advance().unwrap();
                    Err(format!("Unexpected character: '{}'", ch))
                } else {
                    Ok(Token::Symbol(sym))
                }
            }
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token()?;
            if token == Token::Eof {
                tokens.push(token);
                break;
            }
            tokens.push(token);
        }
        Ok(tokens)
    }
}

/// MeTTa IR - Enhanced intermediate representation for MeTTa expressions
///
/// Represents the abstract syntax of MeTTa code with semantic distinctions
/// for different atom types. This IR is used by both the hand-written parser
/// and the Tree-Sitter parser to provide a unified representation.
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

/// Hand-written parser for MeTTa
///
/// **DEPRECATED**: Use `TreeSitterMettaParser` instead for better error recovery,
/// incremental parsing, and full MeTTa language support.
#[deprecated(since = "0.1.2", note = "Use TreeSitterMettaParser instead")]
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn current(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    #[allow(dead_code)]
    fn expect(&mut self, expected: Token) -> Result<(), String> {
        if self.current() == &expected {
            self.advance();
            Ok(())
        } else {
            Err(format!(
                "Expected {:?}, found {:?}",
                expected,
                self.current()
            ))
        }
    }

    pub fn parse_sexpr(&mut self) -> Result<SExpr, String> {
        match self.current() {
            Token::LParen | Token::LBrace => {
                let is_brace = matches!(self.current(), Token::LBrace);
                self.advance();
                let mut items = Vec::new();
                loop {
                    match self.current() {
                        Token::RParen if !is_brace => {
                            self.advance();
                            break;
                        }
                        Token::RBrace if is_brace => {
                            self.advance();
                            break;
                        }
                        Token::Eof => {
                            return Err(format!(
                                "Unexpected EOF, expected {}",
                                if is_brace { "}" } else { ")" }
                            ))
                        }
                        _ => items.push(self.parse_sexpr()?),
                    }
                }
                // Mark brace lists with a special atom at the start
                if is_brace {
                    let mut brace_list = vec![SExpr::Atom("{}".to_string())];
                    brace_list.extend(items);
                    Ok(SExpr::List(brace_list))
                } else {
                    Ok(SExpr::List(items))
                }
            }
            Token::Symbol(s) => {
                let sym = s.clone();
                self.advance();
                Ok(SExpr::Atom(sym))
            }
            Token::String(s) => {
                let str = s.clone();
                self.advance();
                Ok(SExpr::String(str))
            }
            Token::Integer(i) => {
                let num = *i;
                self.advance();
                Ok(MettaExpr::Integer(num))
            }
            Token::Float(f) => {
                let num = *f;
                self.advance();
                Ok(MettaExpr::Float(num))
            }
            Token::Dot => {
                self.advance();
                Ok(SExpr::Atom(".".to_string()))
            }
            Token::Exclaim => {
                self.advance();
                // Check if this is a prefix operator: !(expr) -> (! expr)
                if matches!(self.current(), Token::LParen | Token::LBrace) {
                    let arg = self.parse_sexpr()?;
                    Ok(SExpr::List(vec![SExpr::Atom("!".to_string()), arg]))
                } else {
                    Ok(SExpr::Atom("!".to_string()))
                }
            }
            Token::Question => {
                self.advance();
                // Check if this is a prefix operator: ?(expr) -> (? expr)
                if matches!(self.current(), Token::LParen | Token::LBrace) {
                    let arg = self.parse_sexpr()?;
                    Ok(SExpr::List(vec![SExpr::Atom("?".to_string()), arg]))
                } else {
                    Ok(SExpr::Atom("?".to_string()))
                }
            }
            Token::Colon => {
                self.advance();
                Ok(SExpr::Atom(":".to_string()))
            }
            Token::Arrow => {
                self.advance();
                Ok(SExpr::Atom("->".to_string()))
            }
            Token::Equals => {
                self.advance();
                Ok(SExpr::Atom("=".to_string()))
            }
            Token::Semicolon => {
                self.advance();
                Ok(SExpr::Atom(";".to_string()))
            }
            Token::Pipe => {
                self.advance();
                Ok(SExpr::Atom("|".to_string()))
            }
            Token::Comma => {
                self.advance();
                Ok(SExpr::Atom(",".to_string()))
            }
            Token::Ampersand => {
                self.advance();
                Ok(SExpr::Atom("&".to_string()))
            }
            Token::Ellipsis => {
                self.advance();
                Ok(SExpr::Atom("...".to_string()))
            }
            Token::At => {
                self.advance();
                Ok(SExpr::Atom("@".to_string()))
            }
            Token::LeftArrow => {
                self.advance();
                Ok(SExpr::Atom("<-".to_string()))
            }
            Token::DoubleLeftArrow => {
                self.advance();
                Ok(SExpr::Atom("<=".to_string()))
            }
            Token::TripleLeftArrow => {
                self.advance();
                Ok(SExpr::Atom("<<-".to_string()))
            }
            Token::QuestionExclaim => {
                self.advance();
                Ok(SExpr::Atom("?!".to_string()))
            }
            Token::ExclaimQuestion => {
                self.advance();
                Ok(SExpr::Atom("!?".to_string()))
            }
            Token::Eof => Err("Unexpected end of input".to_string()),
            _ => Err(format!("Unexpected token: {:?}", self.current())),
        }
    }

    pub fn parse(&mut self) -> Result<Vec<SExpr>, String> {
        let mut exprs = Vec::new();
        while self.current() != &Token::Eof {
            exprs.push(self.parse_sexpr()?);
        }
        Ok(exprs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer_basic() {
        let mut lexer = Lexer::new("(+ 1 2)");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens.len(), 6); // (, +, 1, 2, ), EOF
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Symbol("+".to_string()));
        assert_eq!(tokens[2], Token::Integer(1));
        assert_eq!(tokens[3], Token::Integer(2));
        assert_eq!(tokens[4], Token::RParen);
        assert_eq!(tokens[5], Token::Eof);
    }

    #[test]
    fn test_lexer_parens() {
        let mut lexer = Lexer::new("()");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::RParen);
        assert_eq!(tokens[2], Token::Eof);
    }

    #[test]
    fn test_lexer_braces() {
        let mut lexer = Lexer::new("{}");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LBrace);
        assert_eq!(tokens[1], Token::RBrace);
        assert_eq!(tokens[2], Token::Eof);
    }

    #[test]
    fn test_lexer_symbols() {
        let mut lexer = Lexer::new("foo bar-baz qux_123");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Symbol("foo".to_string()));
        assert_eq!(tokens[1], Token::Symbol("bar-baz".to_string()));
        assert_eq!(tokens[2], Token::Symbol("qux_123".to_string()));
    }

    #[test]
    fn test_lexer_operators() {
        let mut lexer = Lexer::new("+ - * /");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Symbol("+".to_string()));
        assert_eq!(tokens[1], Token::Symbol("-".to_string()));
        assert_eq!(tokens[2], Token::Symbol("*".to_string()));
        assert_eq!(tokens[3], Token::Symbol("/".to_string()));
    }

    #[test]
    fn test_lexer_positive_numbers() {
        let mut lexer = Lexer::new("0 1 42 12345");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(0));
        assert_eq!(tokens[1], Token::Integer(1));
        assert_eq!(tokens[2], Token::Integer(42));
        assert_eq!(tokens[3], Token::Integer(12345));
    }

    #[test]
    fn test_lexer_negative_numbers() {
        let mut lexer = Lexer::new("-1 -42 -999");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(-1));
        assert_eq!(tokens[1], Token::Integer(-42));
        assert_eq!(tokens[2], Token::Integer(-999));
    }

    #[test]
    fn test_lexer_zero() {
        let mut lexer = Lexer::new("0");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(0));
    }

    #[test]
    fn test_lexer_string_basic() {
        let mut lexer = Lexer::new(r#""hello""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String("hello".to_string()));
    }

    #[test]
    fn test_lexer_string_empty() {
        let mut lexer = Lexer::new(r#""""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String("".to_string()));
    }

    #[test]
    fn test_lexer_string_with_spaces() {
        let mut lexer = Lexer::new(r#""hello world""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String("hello world".to_string()));
    }

    #[test]
    fn test_lexer_string_escapes() {
        let mut lexer = Lexer::new(r#""hello\nworld\t!""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String("hello\nworld\t!".to_string()));
    }

    #[test]
    fn test_lexer_string_escaped_quote() {
        let mut lexer = Lexer::new(r#""say \"hello\"""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String(r#"say "hello""#.to_string()));
    }

    #[test]
    fn test_lexer_string_escaped_backslash() {
        let mut lexer = Lexer::new(r#""path\\to\\file""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String(r#"path\to\file"#.to_string()));
    }

    #[test]
    fn test_lexer_string_unclosed() {
        let mut lexer = Lexer::new(r#""hello"#);
        let result = lexer.tokenize();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unclosed string"));
    }

    #[test]
    fn test_lexer_multiple_strings() {
        let mut lexer = Lexer::new(r#""first" "second" "third""#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String("first".to_string()));
        assert_eq!(tokens[1], Token::String("second".to_string()));
        assert_eq!(tokens[2], Token::String("third".to_string()));
    }

    #[test]
    fn test_lexer_exclaim() {
        let mut lexer = Lexer::new("!");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Exclaim);
    }

    #[test]
    fn test_lexer_question() {
        let mut lexer = Lexer::new("?");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Question);
    }

    #[test]
    fn test_lexer_colon() {
        let mut lexer = Lexer::new(":");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Colon);
    }

    #[test]
    fn test_lexer_equals() {
        let mut lexer = Lexer::new("=");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Equals);
    }

    #[test]
    fn test_lexer_double_equals() {
        let mut lexer = Lexer::new("==");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Symbol("==".to_string()));
    }

    #[test]
    fn test_lexer_arrow() {
        let mut lexer = Lexer::new("->");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Arrow);
    }

    #[test]
    fn test_lexer_left_arrow() {
        let mut lexer = Lexer::new("<-");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LeftArrow);
    }

    #[test]
    fn test_lexer_double_left_arrow() {
        let mut lexer = Lexer::new("<=");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::DoubleLeftArrow);
    }

    #[test]
    fn test_lexer_triple_left_arrow() {
        let mut lexer = Lexer::new("<<-");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::TripleLeftArrow);
    }

    #[test]
    fn test_lexer_dot() {
        let mut lexer = Lexer::new(".");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Dot);
    }

    #[test]
    fn test_lexer_ellipsis() {
        let mut lexer = Lexer::new("...");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Ellipsis);
    }

    #[test]
    fn test_lexer_pipe() {
        let mut lexer = Lexer::new("|");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Pipe);
    }

    #[test]
    fn test_lexer_comma() {
        let mut lexer = Lexer::new(",");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Comma);
    }

    #[test]
    fn test_lexer_ampersand() {
        let mut lexer = Lexer::new("&");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Ampersand);
    }

    #[test]
    fn test_lexer_at() {
        let mut lexer = Lexer::new("@");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::At);
    }

    #[test]
    fn test_lexer_question_exclaim() {
        let mut lexer = Lexer::new("?!");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::QuestionExclaim);
    }

    #[test]
    fn test_lexer_exclaim_question() {
        let mut lexer = Lexer::new("!?");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::ExclaimQuestion);
    }

    #[test]
    fn test_lexer_semicolon_comment() {
        let mut lexer = Lexer::new("; this is a comment\n42");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(42));
    }

    #[test]
    fn test_lexer_double_slash_comment() {
        let mut lexer = Lexer::new("// this is a comment\n42");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(42));
    }

    #[test]
    fn test_lexer_block_comment() {
        let mut lexer = Lexer::new("/* this is a\n multi-line comment */ 42");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(42));
    }

    #[test]
    fn test_lexer_block_comment_unclosed() {
        let mut lexer = Lexer::new("/* unclosed comment");
        let result = lexer.tokenize();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unclosed block comment"));
    }

    #[test]
    fn test_lexer_multiple_comments() {
        let mut lexer = Lexer::new("; first\n// second\n/* third */ 42");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Integer(42));
    }

    #[test]
    fn test_lexer_comment_before_expression() {
        let mut lexer = Lexer::new("; comment\n(+ 1 2)");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Symbol("+".to_string()));
    }

    #[test]
    fn test_lexer_whitespace_between_tokens() {
        let mut lexer = Lexer::new("  (  +  1  2  )  ");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Symbol("+".to_string()));
        assert_eq!(tokens[2], Token::Integer(1));
    }

    #[test]
    fn test_lexer_newlines() {
        let mut lexer = Lexer::new("(\n+\n1\n2\n)");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Symbol("+".to_string()));
    }

    #[test]
    fn test_lexer_tabs() {
        let mut lexer = Lexer::new("(\t+\t1\t2\t)");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Symbol("+".to_string()));
    }

    #[test]
    fn test_lexer_dollar_variable() {
        let mut lexer = Lexer::new("$x $var $my_var");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Symbol("$x".to_string()));
        assert_eq!(tokens[1], Token::Symbol("$var".to_string()));
        assert_eq!(tokens[2], Token::Symbol("$my_var".to_string()));
    }

    #[test]
    fn test_lexer_ampersand_variable() {
        let mut lexer = Lexer::new("&rest &args");
        let tokens = lexer.tokenize().unwrap();
        // Note: & is tokenized separately, then the symbol
        assert_eq!(tokens[0], Token::Ampersand);
        assert_eq!(tokens[1], Token::Symbol("rest".to_string()));
    }

    #[test]
    fn test_lexer_quote_variable() {
        let mut lexer = Lexer::new("'quoted");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Symbol("'quoted".to_string()));
    }

    #[test]
    fn test_lexer_wildcard() {
        let mut lexer = Lexer::new("_");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::Symbol("_".to_string()));
    }

    #[test]
    fn test_lexer_nested_expression() {
        let mut lexer = Lexer::new("(+ 1 (+ 2 3))");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Symbol("+".to_string()));
        assert_eq!(tokens[2], Token::Integer(1));
        assert_eq!(tokens[3], Token::LParen);
        assert_eq!(tokens[4], Token::Symbol("+".to_string()));
        assert_eq!(tokens[5], Token::Integer(2));
        assert_eq!(tokens[6], Token::Integer(3));
        assert_eq!(tokens[7], Token::RParen);
        assert_eq!(tokens[8], Token::RParen);
    }

    #[test]
    fn test_lexer_rule_definition() {
        let mut lexer = Lexer::new("(= (double $x) (* $x 2))");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Equals);
        assert_eq!(tokens[2], Token::LParen);
        assert_eq!(tokens[3], Token::Symbol("double".to_string()));
        assert_eq!(tokens[4], Token::Symbol("$x".to_string()));
        assert_eq!(tokens[5], Token::RParen);
    }

    #[test]
    fn test_lexer_type_assertion() {
        let mut lexer = Lexer::new("(: x Number)");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::LParen);
        assert_eq!(tokens[1], Token::Colon);
        assert_eq!(tokens[2], Token::Symbol("x".to_string()));
        assert_eq!(tokens[3], Token::Symbol("Number".to_string()));
    }

    #[test]
    fn test_lexer_empty_input() {
        let mut lexer = Lexer::new("");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], Token::Eof);
    }

    /* -- -- -- Parser tests -- -- -- */

    #[test]
    fn test_parser_basic() {
        let mut lexer = Lexer::new("(+ 1 2)");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);

        match &exprs[0] {
            SExpr::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("+".to_string()));
                assert_eq!(items[1], SExpr::Integer(1));
                assert_eq!(items[2], SExpr::Integer(2));
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parser_empty_list() {
        let mut lexer = Lexer::new("()");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);
        assert_eq!(exprs[0], SExpr::List(vec![]));
    }

    #[test]
    fn test_parser_atom() {
        let mut lexer = Lexer::new("foo");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);
        assert_eq!(exprs[0], SExpr::Atom("foo".to_string()));
    }

    #[test]
    fn test_parser_integer() {
        let mut lexer = Lexer::new("42");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);
        assert_eq!(exprs[0], SExpr::Integer(42));
    }

    #[test]
    fn test_parser_string() {
        let mut lexer = Lexer::new(r#""hello""#);
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);
        assert_eq!(exprs[0], SExpr::String("hello".to_string()));
    }

    #[test]
    fn test_parser_nested_lists() {
        let mut lexer = Lexer::new("(a (b (c)))");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);

        match &exprs[0] {
            SExpr::List(outer) => {
                assert_eq!(outer.len(), 2);
                assert_eq!(outer[0], SExpr::Atom("a".to_string()));

                match &outer[1] {
                    SExpr::List(middle) => {
                        assert_eq!(middle.len(), 2);
                        assert_eq!(middle[0], SExpr::Atom("b".to_string()));

                        match &middle[1] {
                            SExpr::List(inner) => {
                                assert_eq!(inner.len(), 1);
                                assert_eq!(inner[0], SExpr::Atom("c".to_string()));
                            }
                            _ => panic!("Expected inner list"),
                        }
                    }
                    _ => panic!("Expected middle list"),
                }
            }
            _ => panic!("Expected outer list"),
        }
    }

    #[test]
    fn test_parser_multiple_expressions() {
        let mut lexer = Lexer::new("(+ 1 2) (* 3 4)");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 2);
    }

    #[test]
    fn test_parser_exclaim_prefix() {
        let mut lexer = Lexer::new("!(foo)");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);

        // !(foo) should parse as (! (foo))
        match &exprs[0] {
            SExpr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], SExpr::Atom("!".to_string()));
                match &items[1] {
                    SExpr::List(_) => {}
                    _ => panic!("Expected list after !"),
                }
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parser_question_prefix() {
        let mut lexer = Lexer::new("?(foo)");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);

        match &exprs[0] {
            SExpr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], SExpr::Atom("?".to_string()));
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parser_braces() {
        let mut lexer = Lexer::new("{a b c}");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);

        // Braces are marked with {} atom at start
        match &exprs[0] {
            SExpr::List(items) => {
                assert_eq!(items[0], SExpr::Atom("{}".to_string()));
                assert_eq!(items[1], SExpr::Atom("a".to_string()));
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parser_unclosed_paren() {
        let mut lexer = Lexer::new("(+ 1 2");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let result = parser.parse();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("EOF"));
    }

    #[test]
    fn test_parser_extra_close_paren() {
        let mut lexer = Lexer::new("(+ 1 2))");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let result = parser.parse();
        assert!(result.is_err() || result.unwrap().len() == 1);
    }

    #[test]
    fn test_parser_mismatched_braces_parens() {
        let mut lexer = Lexer::new("(+ 1 2}");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_empty_input() {
        let mut lexer = Lexer::new("");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 0);
    }

    #[test]
    fn test_parser_special_operators() {
        let test_cases = vec![
            (":", SExpr::Atom(":".to_string())),
            ("->", SExpr::Atom("->".to_string())),
            ("=", SExpr::Atom("=".to_string())),
            ("<-", SExpr::Atom("<-".to_string())),
            ("<=", SExpr::Atom("<=".to_string())),
            ("...", SExpr::Atom("...".to_string())),
        ];

        for (input, expected) in test_cases {
            let mut lexer = Lexer::new(input);
            let tokens = lexer.tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let exprs = parser.parse().unwrap();
            assert_eq!(exprs.len(), 1);
            assert_eq!(exprs[0], expected, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_display_sexpr() {
        let expr = SExpr::List(vec![
            SExpr::Atom("+".to_string()),
            SExpr::Integer(1),
            SExpr::Integer(2),
        ]);
        assert_eq!(format!("{}", expr), "(+ 1 2)");
    }

    #[test]
    fn test_display_token() {
        assert_eq!(format!("{}", Token::LParen), "(");
        assert_eq!(format!("{}", Token::Symbol("foo".to_string())), "foo");
        assert_eq!(format!("{}", Token::Integer(42)), "42");
        assert_eq!(format!("{}", Token::Arrow), "->");
    }
}
