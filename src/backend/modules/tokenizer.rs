//! Per-Module Tokenizer
//!
//! A `Tokenizer` manages dynamic token bindings for a module.
//! Tokens are patterns that, when encountered during parsing,
//! are replaced with specific atoms.

use std::sync::Arc;

use crate::backend::models::MettaValue;

/// A function that constructs an atom from a matched token string.
pub type TokenConstructor = Arc<dyn Fn(&str) -> MettaValue + Send + Sync>;

/// A registered token pattern and its constructor.
#[derive(Clone)]
pub struct TokenEntry {
    /// The token pattern (exact string match for now).
    pattern: String,

    /// Function to construct the atom when this token is matched.
    constructor: TokenConstructor,
}

impl TokenEntry {
    /// Create a new token entry.
    pub fn new(pattern: String, constructor: TokenConstructor) -> Self {
        Self { pattern, constructor }
    }

    /// Get the pattern string.
    pub fn pattern(&self) -> &str {
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
/// - Register tokens via `bind!`
/// - Look up tokens during parsing
/// - Merge tokens from imported modules
///
/// Token lookup is done by exact string match (for now).
/// Later versions may support regex patterns.
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

    /// Register a token with a simple value.
    ///
    /// When `pattern` is encountered, it will be replaced with `value`.
    pub fn register_token_value(&mut self, pattern: &str, value: MettaValue) {
        let value_clone = value.clone();
        self.tokens.push(TokenEntry::new(
            pattern.to_string(),
            Arc::new(move |_| value_clone.clone()),
        ));
    }

    /// Register a token with a constructor function.
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

    /// Register a token with an existing constructor.
    pub fn register_token_with_constructor(&mut self, pattern: &str, constructor: TokenConstructor) {
        self.tokens.push(TokenEntry::new(pattern.to_string(), constructor));
    }

    /// Find an exact match for a token name.
    ///
    /// Returns the constructor for the first matching token,
    /// searching from most recently registered to oldest (shadowing).
    pub fn find_exact(&self, name: &str) -> Option<&TokenConstructor> {
        // Search in reverse order (most recent first) for shadowing
        for entry in self.tokens.iter().rev() {
            if entry.pattern() == name {
                return Some(&entry.constructor);
            }
        }
        None
    }

    /// Look up a token and construct its value.
    ///
    /// Returns `Some(value)` if the token is registered, `None` otherwise.
    pub fn lookup(&self, name: &str) -> Option<MettaValue> {
        self.find_exact(name).map(|constructor| constructor(name))
    }

    /// Check if a token is registered.
    pub fn has_token(&self, name: &str) -> bool {
        self.tokens.iter().any(|e| e.pattern() == name)
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

    /// Remove a token by pattern.
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
}
