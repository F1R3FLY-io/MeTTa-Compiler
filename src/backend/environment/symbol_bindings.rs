//! Symbol binding and tokenizer operations for Environment.
//!
//! Provides methods for symbol binding (bind!) and token registration.

use std::sync::atomic::Ordering;

use super::generic::GenericEnvironment;
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValue as MettaValueTrait};
use crate::backend::MettaValue;

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // ============================================================
    // Symbol Bindings Management (bind!)
    // ============================================================

    /// Bind a symbol to a value
    /// Used by bind! operation
    pub fn bind(&mut self, symbol: &str, value: V) {
        self.make_owned();

        // DashMap - use .insert() directly
        self.shared.bindings.insert(symbol.to_string(), value);

        // Also register in fuzzy matcher for suggestions
        // parking_lot::RwLock - no .expect()
        self.shared.fuzzy_matcher.write().insert(symbol);

        self.modified.store(true, Ordering::Release);
    }

    /// Get the value bound to a symbol
    /// Used for symbol resolution
    pub fn get_binding(&self, symbol: &str) -> Option<V> {
        // DashMap - use .get() directly, returns Ref which needs .value()
        self.shared.bindings.get(symbol).map(|r| r.value().clone())
    }

    /// Check if a symbol is bound
    pub fn has_binding(&self, symbol: &str) -> bool {
        // DashMap - use .contains_key() directly
        self.shared.bindings.contains_key(symbol)
    }

    // ============================================================
    // Tokenizer Operations (bind! support)
    // ============================================================

    /// Look up a token in the tokenizer
    /// Returns the bound value if found
    ///
    /// NOTE: The tokenizer stores values of type V.
    pub fn lookup_token(&self, token: &str) -> Option<V> {
        // parking_lot::RwLock - no .expect()
        self.shared.tokenizer.read().lookup(token)
    }

    /// Look up a token in the tokenizer (generic version).
    ///
    /// Returns the bound value if found. This is the zero-conversion version
    /// that works with any value type V.
    pub fn lookup_token_generic(&self, token: &str, _factory: &F) -> Option<V> {
        self.shared.tokenizer.read().lookup(token)
    }

    /// Check if a token is registered in the tokenizer
    pub fn has_token(&self, token: &str) -> bool {
        // parking_lot::RwLock - no .expect()
        self.shared.tokenizer.read().has_token(token)
    }
}

