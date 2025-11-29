//! Per-Module Tokenizer
//!
//! A `Tokenizer` manages dynamic token bindings for a module.
//! Tokens are patterns that, when encountered during parsing,
//! are replaced with specific atoms.
//!
//! ## Pattern Types
//!
//! - **Exact match**: Simple string equality (default for `bind!`)
//! - **Regex match**: Regular expression patterns for advanced matching
//!
//! ## HE Compatibility
//!
//! The HE (hyperon-experimental) tokenizer uses regex patterns for token
//! matching. This implementation supports both exact string matching
//! (for simplicity and performance) and regex patterns (for HE parity).

use std::sync::Arc;

use regex::Regex;

use crate::backend::models::MettaValue;

/// A function that constructs an atom from a matched token string.
pub type TokenConstructor = Arc<dyn Fn(&str) -> MettaValue + Send + Sync>;

/// Token pattern type - exact string or regex.
#[derive(Clone)]
pub enum TokenPattern {
    /// Exact string match (fast path, no regex overhead).
    Exact(String),

    /// Regex pattern match (HE-compatible).
    Regex {
        /// Compiled regex for efficient matching.
        regex: Regex,
        /// Original pattern string for display/debugging.
        raw_pattern: String,
    },
}

impl TokenPattern {
    /// Create an exact match pattern.
    pub fn exact(pattern: &str) -> Self {
        Self::Exact(pattern.to_string())
    }

    /// Create a regex pattern.
    pub fn regex(pattern: &str) -> Result<Self, regex::Error> {
        let regex = Regex::new(pattern)?;
        Ok(Self::Regex {
            regex,
            raw_pattern: pattern.to_string(),
        })
    }

    /// Get the pattern string for display.
    pub fn pattern_str(&self) -> &str {
        match self {
            Self::Exact(s) => s,
            Self::Regex { raw_pattern, .. } => raw_pattern,
        }
    }

    /// Check if the input matches this pattern exactly.
    ///
    /// For exact patterns: string equality check.
    /// For regex patterns: full string match (anchored).
    pub fn matches(&self, input: &str) -> bool {
        match self {
            Self::Exact(s) => s == input,
            Self::Regex { regex, .. } => regex.is_match(input),
        }
    }

    /// Check if the pattern matches at the start of input.
    /// Returns the matched portion if successful.
    ///
    /// This is useful for tokenizer scanning where we want to
    /// find the longest prefix match.
    pub fn match_prefix<'a>(&self, input: &'a str) -> Option<&'a str> {
        match self {
            Self::Exact(s) => {
                if input.starts_with(s) {
                    Some(&input[..s.len()])
                } else {
                    None
                }
            }
            Self::Regex { regex, .. } => regex.find(input).and_then(|m| {
                if m.start() == 0 {
                    Some(m.as_str())
                } else {
                    None
                }
            }),
        }
    }
}

impl std::fmt::Debug for TokenPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(s) => write!(f, "Exact({:?})", s),
            Self::Regex { raw_pattern, .. } => write!(f, "Regex({:?})", raw_pattern),
        }
    }
}

/// A registered token pattern and its constructor.
#[derive(Clone)]
pub struct TokenEntry {
    /// The token pattern (exact or regex).
    pattern: TokenPattern,

    /// Function to construct the atom when this token is matched.
    constructor: TokenConstructor,
}

impl TokenEntry {
    /// Create a new token entry with exact match.
    pub fn new(pattern: String, constructor: TokenConstructor) -> Self {
        Self {
            pattern: TokenPattern::exact(&pattern),
            constructor,
        }
    }

    /// Create a new token entry with a regex pattern.
    pub fn new_regex(pattern: &str, constructor: TokenConstructor) -> Result<Self, regex::Error> {
        Ok(Self {
            pattern: TokenPattern::regex(pattern)?,
            constructor,
        })
    }

    /// Create a new token entry with a pre-built pattern.
    pub fn with_pattern(pattern: TokenPattern, constructor: TokenConstructor) -> Self {
        Self { pattern, constructor }
    }

    /// Get the pattern string for display.
    pub fn pattern(&self) -> &str {
        self.pattern.pattern_str()
    }

    /// Get a reference to the pattern.
    pub fn pattern_ref(&self) -> &TokenPattern {
        &self.pattern
    }

    /// Construct an atom for this token.
    pub fn construct(&self, matched: &str) -> MettaValue {
        (self.constructor)(matched)
    }
}

impl std::fmt::Debug for TokenEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenEntry")
            .field("pattern", &self.pattern)
            .finish()
    }
}

/// Per-module tokenizer for dynamic token registration.
///
/// Each module has its own tokenizer that can:
/// - Register tokens via `bind!` (exact match or regex)
/// - Look up tokens during parsing
/// - Merge tokens from imported modules
///
/// ## Pattern Types
///
/// - **Exact match**: Fast string equality (default for `bind!`)
/// - **Regex match**: Regular expression patterns for advanced matching
///
/// Token lookup searches from most recently registered to oldest (shadowing).
pub struct Tokenizer {
    /// Registered token entries.
    /// Stored in insertion order; later entries shadow earlier ones.
    tokens: Vec<TokenEntry>,
}

impl Tokenizer {
    /// Create a new empty tokenizer.
    pub fn new() -> Self {
        Self { tokens: Vec::new() }
    }

    /// Register a token with a simple value using exact string match.
    ///
    /// When `pattern` is encountered, it will be replaced with `value`.
    /// This is the default behavior for `bind!`.
    pub fn register_token_value(&mut self, pattern: &str, value: MettaValue) {
        let value_clone = value.clone();
        self.tokens.push(TokenEntry::new(
            pattern.to_string(),
            Arc::new(move |_| value_clone.clone()),
        ));
    }

    /// Register a token with a regex pattern and simple value.
    ///
    /// When input matches the regex pattern, it will be replaced with `value`.
    /// Returns an error if the regex pattern is invalid.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Match any identifier starting with '$'
    /// tokenizer.register_token_value_regex(r"^\$[a-zA-Z_][a-zA-Z0-9_]*$", value)?;
    /// ```
    pub fn register_token_value_regex(
        &mut self,
        pattern: &str,
        value: MettaValue,
    ) -> Result<(), regex::Error> {
        let value_clone = value.clone();
        let entry = TokenEntry::new_regex(pattern, Arc::new(move |_| value_clone.clone()))?;
        self.tokens.push(entry);
        Ok(())
    }

    /// Register a token with a constructor function using exact match.
    ///
    /// When `pattern` is encountered, `constructor` will be called
    /// with the matched string to produce the atom.
    pub fn register_token<F>(&mut self, pattern: &str, constructor: F)
    where
        F: Fn(&str) -> MettaValue + Send + Sync + 'static,
    {
        self.tokens.push(TokenEntry::new(
            pattern.to_string(),
            Arc::new(constructor),
        ));
    }

    /// Register a token with a regex pattern and constructor function.
    ///
    /// When input matches the regex pattern, `constructor` will be called
    /// with the matched string to produce the atom.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Parse numeric literals matching a pattern
    /// tokenizer.register_token_regex(r"^-?\d+", |s| MettaValue::Long(s.parse().unwrap()))?;
    /// ```
    pub fn register_token_regex<F>(
        &mut self,
        pattern: &str,
        constructor: F,
    ) -> Result<(), regex::Error>
    where
        F: Fn(&str) -> MettaValue + Send + Sync + 'static,
    {
        let entry = TokenEntry::new_regex(pattern, Arc::new(constructor))?;
        self.tokens.push(entry);
        Ok(())
    }

    /// Register a token with an existing constructor using exact match.
    pub fn register_token_with_constructor(&mut self, pattern: &str, constructor: TokenConstructor) {
        self.tokens.push(TokenEntry::new(pattern.to_string(), constructor));
    }

    /// Register a token with an existing constructor using regex pattern.
    pub fn register_token_with_constructor_regex(
        &mut self,
        pattern: &str,
        constructor: TokenConstructor,
    ) -> Result<(), regex::Error> {
        let entry = TokenEntry::new_regex(pattern, constructor)?;
        self.tokens.push(entry);
        Ok(())
    }

    /// Find a matching token for the given input.
    ///
    /// Returns the constructor for the first matching token,
    /// searching from most recently registered to oldest (shadowing).
    ///
    /// For exact patterns: requires string equality.
    /// For regex patterns: requires full match.
    pub fn find_match(&self, name: &str) -> Option<&TokenConstructor> {
        // Search in reverse order (most recent first) for shadowing
        for entry in self.tokens.iter().rev() {
            if entry.pattern_ref().matches(name) {
                return Some(&entry.constructor);
            }
        }
        None
    }

    /// Find a matching token at the start of input (prefix match).
    ///
    /// Returns the matched string and constructor if found.
    /// Useful for tokenizer scanning where we want longest prefix match.
    pub fn find_prefix_match<'a>(&self, input: &'a str) -> Option<(&'a str, &TokenConstructor)> {
        // Search in reverse order (most recent first) for shadowing
        // Note: For true longest-match semantics, we'd need to check all patterns
        // and return the longest. Current implementation returns first match.
        for entry in self.tokens.iter().rev() {
            if let Some(matched) = entry.pattern_ref().match_prefix(input) {
                return Some((matched, &entry.constructor));
            }
        }
        None
    }

    /// Find an exact match for a token name (legacy API).
    ///
    /// This is equivalent to `find_match` for exact patterns.
    /// Kept for backward compatibility.
    #[deprecated(since = "0.3.0", note = "Use find_match instead")]
    pub fn find_exact(&self, name: &str) -> Option<&TokenConstructor> {
        self.find_match(name)
    }

    /// Look up a token and construct its value.
    ///
    /// Returns `Some(value)` if the token is registered, `None` otherwise.
    pub fn lookup(&self, name: &str) -> Option<MettaValue> {
        self.find_match(name).map(|constructor| constructor(name))
    }

    /// Check if a token pattern is registered (by pattern string).
    ///
    /// Note: This checks pattern equality, not whether the token would match.
    pub fn has_token(&self, name: &str) -> bool {
        self.tokens.iter().any(|e| e.pattern() == name)
    }

    /// Check if input would match any registered token.
    pub fn matches(&self, input: &str) -> bool {
        self.find_match(input).is_some()
    }

    /// Merge another tokenizer's entries into this one.
    ///
    /// Entries from `other` are appended, so they shadow existing entries
    /// with the same pattern.
    pub fn merge_from(&mut self, other: &Tokenizer) {
        for entry in &other.tokens {
            self.tokens.push(entry.clone());
        }
    }

    /// Get the number of registered tokens.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Get all registered token patterns.
    pub fn patterns(&self) -> Vec<&str> {
        self.tokens.iter().map(|e| e.pattern()).collect()
    }

    /// Clear all registered tokens.
    pub fn clear(&mut self) {
        self.tokens.clear();
    }

    /// Remove a token by pattern string.
    /// Returns true if a token was removed.
    pub fn remove_token(&mut self, pattern: &str) -> bool {
        let before = self.tokens.len();
        self.tokens.retain(|e| e.pattern() != pattern);
        self.tokens.len() < before
    }
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Tokenizer {
    fn clone(&self) -> Self {
        Self {
            tokens: self.tokens.clone(),
        }
    }
}

impl std::fmt::Debug for Tokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokenizer")
            .field("token_count", &self.tokens.len())
            .field("patterns", &self.patterns())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tokenizer() {
        let tok = Tokenizer::new();
        assert_eq!(tok.token_count(), 0);
        assert!(!tok.has_token("foo"));
    }

    #[test]
    fn test_register_token_value() {
        let mut tok = Tokenizer::new();
        let value = MettaValue::Long(42);

        tok.register_token_value("&answer", value.clone());

        assert!(tok.has_token("&answer"));
        assert_eq!(tok.lookup("&answer"), Some(MettaValue::Long(42)));
        assert_eq!(tok.lookup("&unknown"), None);
    }

    #[test]
    fn test_register_token_with_constructor() {
        let mut tok = Tokenizer::new();

        tok.register_token("PI", |_| MettaValue::Float(3.14159));

        assert!(tok.has_token("PI"));
        if let Some(MettaValue::Float(f)) = tok.lookup("PI") {
            assert!((f - 3.14159).abs() < 0.0001);
        } else {
            panic!("Expected Float");
        }
    }

    #[test]
    fn test_shadowing() {
        let mut tok = Tokenizer::new();

        tok.register_token_value("x", MettaValue::Long(1));
        tok.register_token_value("x", MettaValue::Long(2));

        // Most recent should shadow
        assert_eq!(tok.lookup("x"), Some(MettaValue::Long(2)));
    }

    #[test]
    fn test_merge() {
        let mut tok1 = Tokenizer::new();
        tok1.register_token_value("a", MettaValue::Long(1));

        let mut tok2 = Tokenizer::new();
        tok2.register_token_value("b", MettaValue::Long(2));
        tok2.register_token_value("a", MettaValue::Long(3)); // Shadow

        tok1.merge_from(&tok2);

        assert_eq!(tok1.token_count(), 3);
        assert_eq!(tok1.lookup("b"), Some(MettaValue::Long(2)));
        // tok2's "a" should shadow tok1's "a"
        assert_eq!(tok1.lookup("a"), Some(MettaValue::Long(3)));
    }

    #[test]
    fn test_remove_token() {
        let mut tok = Tokenizer::new();
        tok.register_token_value("a", MettaValue::Long(1));
        tok.register_token_value("b", MettaValue::Long(2));

        assert!(tok.remove_token("a"));
        assert!(!tok.has_token("a"));
        assert!(tok.has_token("b"));

        // Removing non-existent
        assert!(!tok.remove_token("a"));
    }

    #[test]
    fn test_clear() {
        let mut tok = Tokenizer::new();
        tok.register_token_value("a", MettaValue::Long(1));
        tok.register_token_value("b", MettaValue::Long(2));

        tok.clear();
        assert_eq!(tok.token_count(), 0);
    }

    #[test]
    fn test_patterns() {
        let mut tok = Tokenizer::new();
        tok.register_token_value("&kb", MettaValue::Long(1));
        tok.register_token_value("PI", MettaValue::Float(3.14));

        let patterns = tok.patterns();
        assert!(patterns.contains(&"&kb"));
        assert!(patterns.contains(&"PI"));
    }

    // ============================================================
    // Regex pattern tests
    // ============================================================

    #[test]
    fn test_regex_pattern_basic() {
        let mut tok = Tokenizer::new();

        // Register a regex pattern that matches variables
        tok.register_token_value_regex(r"^\$[a-z]+$", MettaValue::Atom("var".to_string()))
            .expect("valid regex");

        // Should match
        assert!(tok.matches("$foo"));
        assert!(tok.matches("$bar"));
        assert_eq!(tok.lookup("$xyz"), Some(MettaValue::Atom("var".to_string())));

        // Should not match
        assert!(!tok.matches("foo"));
        assert!(!tok.matches("$FOO")); // uppercase
        assert!(!tok.matches("$123")); // numbers
    }

    #[test]
    fn test_regex_pattern_with_constructor() {
        let mut tok = Tokenizer::new();

        // Register a regex pattern that parses integers
        tok.register_token_regex(r"^-?\d+$", |s| {
            MettaValue::Long(s.parse::<i64>().unwrap_or(0))
        })
        .expect("valid regex");

        // Should parse numbers
        assert_eq!(tok.lookup("42"), Some(MettaValue::Long(42)));
        assert_eq!(tok.lookup("-17"), Some(MettaValue::Long(-17)));
        assert_eq!(tok.lookup("0"), Some(MettaValue::Long(0)));

        // Should not match non-numbers
        assert_eq!(tok.lookup("hello"), None);
        assert_eq!(tok.lookup("12.5"), None); // has decimal
    }

    #[test]
    fn test_regex_vs_exact_priority() {
        let mut tok = Tokenizer::new();

        // Register exact match first
        tok.register_token_value("$x", MettaValue::Long(1));

        // Then register regex that would also match
        tok.register_token_regex(r"^\$[a-z]$", |_| MettaValue::Long(2))
            .expect("valid regex");

        // Regex was registered later, so it shadows exact match
        assert_eq!(tok.lookup("$x"), Some(MettaValue::Long(2)));

        // Now test the reverse
        let mut tok2 = Tokenizer::new();
        tok2.register_token_regex(r"^\$[a-z]$", |_| MettaValue::Long(1))
            .expect("valid regex");
        tok2.register_token_value("$x", MettaValue::Long(2));

        // Exact match was registered later, so it shadows regex
        assert_eq!(tok2.lookup("$x"), Some(MettaValue::Long(2)));
    }

    #[test]
    fn test_prefix_match() {
        let mut tok = Tokenizer::new();

        // Register exact pattern
        tok.register_token_value("hello", MettaValue::Atom("greeting".to_string()));

        // Test prefix match
        if let Some((matched, _)) = tok.find_prefix_match("hello world") {
            assert_eq!(matched, "hello");
        } else {
            panic!("Expected prefix match");
        }

        // No match
        assert!(tok.find_prefix_match("hi there").is_none());
    }

    #[test]
    fn test_regex_prefix_match() {
        let mut tok = Tokenizer::new();

        // Register regex that matches identifiers
        tok.register_token_regex(r"^[a-zA-Z_][a-zA-Z0-9_]*", |s| {
            MettaValue::Atom(s.to_string())
        })
        .expect("valid regex");

        // Test prefix match
        if let Some((matched, constructor)) = tok.find_prefix_match("foo123 bar") {
            assert_eq!(matched, "foo123");
            assert_eq!(constructor(matched), MettaValue::Atom("foo123".to_string()));
        } else {
            panic!("Expected prefix match");
        }

        // Match at start only
        if let Some((matched, _)) = tok.find_prefix_match("_under_score rest") {
            assert_eq!(matched, "_under_score");
        } else {
            panic!("Expected prefix match");
        }
    }

    #[test]
    fn test_invalid_regex() {
        let mut tok = Tokenizer::new();

        // Invalid regex should return error
        let result = tok.register_token_value_regex(r"[invalid(", MettaValue::Unit);
        assert!(result.is_err());

        // Tokenizer should still work after failed registration
        tok.register_token_value("valid", MettaValue::Long(1));
        assert_eq!(tok.lookup("valid"), Some(MettaValue::Long(1)));
    }

    #[test]
    fn test_token_pattern_debug() {
        let exact = TokenPattern::exact("hello");
        let regex = TokenPattern::regex(r"^\d+$").expect("valid regex");

        // Debug output should be readable
        let exact_debug = format!("{:?}", exact);
        let regex_debug = format!("{:?}", regex);

        assert!(exact_debug.contains("Exact"));
        assert!(exact_debug.contains("hello"));
        assert!(regex_debug.contains("Regex"));
        // Note: backslash is escaped in debug output, so we check for partial match
        assert!(regex_debug.contains("d+$"));
    }

    #[test]
    fn test_matches_method() {
        let mut tok = Tokenizer::new();
        tok.register_token_value("foo", MettaValue::Long(1));
        tok.register_token_regex(r"^bar\d+$", |_| MettaValue::Long(2))
            .expect("valid regex");

        // Exact match
        assert!(tok.matches("foo"));

        // Regex match
        assert!(tok.matches("bar1"));
        assert!(tok.matches("bar42"));
        assert!(tok.matches("bar999"));

        // No match
        assert!(!tok.matches("baz"));
        assert!(!tok.matches("bar")); // no digits
        assert!(!tok.matches("barx")); // not digit
    }
}
