//! Symbol binding and tokenizer operations for Environment.
//!
//! Provides methods for symbol binding (bind!) and token registration.

use std::sync::atomic::Ordering;

use super::{Environment, MettaValue};

impl Environment {
    // ============================================================
    // Symbol Bindings Management (bind!)
    // ============================================================

    /// Bind a symbol to a value
    /// Used by bind! operation
    pub fn bind(&mut self, symbol: &str, value: MettaValue) {
        self.make_owned();

        self.shared
            .bindings
            .write()
            .expect("bindings lock poisoned")
            .insert(symbol.to_string(), value);

        // Also register in fuzzy matcher for suggestions
        self.shared
            .fuzzy_matcher
            .write()
            .expect("fuzzy_matcher lock poisoned")
            .insert(symbol);

        self.modified.store(true, Ordering::Release);
    }

    /// Get the value bound to a symbol
    /// Used for symbol resolution
    pub fn get_binding(&self, symbol: &str) -> Option<MettaValue> {
        self.shared
            .bindings
            .read()
            .expect("bindings lock poisoned")
            .get(symbol)
            .cloned()
    }

    /// Check if a symbol is bound
    pub fn has_binding(&self, symbol: &str) -> bool {
        self.shared
            .bindings
            .read()
            .expect("bindings lock poisoned")
            .contains_key(symbol)
    }

    // ============================================================
    // Tokenizer Operations (bind! support)
    // ============================================================

    /// Register a token with its value in the tokenizer
    /// Used by bind! to register tokens for later resolution
    /// HE-compatible: tokens registered here affect subsequent atom resolution
    pub fn register_token(&mut self, token: &str, value: MettaValue) {
        self.make_owned();
        self.shared
            .tokenizer
            .write()
            .expect("tokenizer lock poisoned")
            .register_token_value(token, value);
        // Also register in fuzzy matcher for suggestions
        self.shared
            .fuzzy_matcher
            .write()
            .expect("fuzzy_matcher lock poisoned")
            .insert(token);
        self.modified.store(true, Ordering::Release);
    }

    /// Look up a token in the tokenizer
    /// Returns the bound value if found
    pub fn lookup_token(&self, token: &str) -> Option<MettaValue> {
        self.shared
            .tokenizer
            .read()
            .expect("tokenizer lock poisoned")
            .lookup(token)
    }

    /// Check if a token is registered in the tokenizer
    pub fn has_token(&self, token: &str) -> bool {
        self.shared
            .tokenizer
            .read()
            .expect("tokenizer lock poisoned")
            .has_token(token)
    }
}
