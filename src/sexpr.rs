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
                    let num = self.read_number();
                    Ok(Token::Integer(-num))
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
                let num = self.read_number();
                Ok(Token::Integer(num))
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

/// S-expression AST
#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    Atom(String),
    String(String),
    Integer(i64),
    List(Vec<SExpr>),
    Quoted(Box<SExpr>),
}

impl fmt::Display for SExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SExpr::Atom(s) => write!(f, "{}", s),
            SExpr::String(s) => write!(f, "\"{}\"", s),
            SExpr::Integer(i) => write!(f, "{}", i),
            SExpr::List(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
            SExpr::Quoted(expr) => write!(f, "'{}", expr),
        }
    }
}

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
                Ok(SExpr::Integer(num))
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
    }

    #[test]
    fn test_parser_basic() {
        let mut lexer = Lexer::new("(+ 1 2)");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let exprs = parser.parse().unwrap();
        assert_eq!(exprs.len(), 1);
    }
}
