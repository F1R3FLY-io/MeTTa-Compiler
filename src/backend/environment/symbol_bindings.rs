//! Symbol binding and tokenizer operations for Environment.
//!
//! Provides methods for symbol binding (bind!) and token registration.

use std::sync::atomic::Ordering;

use super::core::GenericEnvironment;
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValueTrait};

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

        super::core::with_env_satb_deletion_barrier(|satb_active| {
            let old = self
                .shared
                .bindings
                .write()
                .insert(symbol.to_string(), value);
            if satb_active {
                if let Some(old) = old {
                    super::core::shade_generic_values_for_satb(std::iter::once(old));
                }
            }
        });
        #[cfg(not(feature = "index-gc"))]
        {
            self.shared
                .bindings
                .write()
                .insert(symbol.to_string(), value);
        }

        // Also register in fuzzy matcher for suggestions
        self.shared.fuzzy_matcher.write().insert(symbol);

        self.modified.store(true, Ordering::Release);
    }

    /// Get the value bound to a symbol
    /// Used for symbol resolution
    pub fn get_binding(&self, symbol: &str) -> Option<V> {
        self.shared.bindings.read().get(symbol).cloned()
    }

    /// Check if a symbol is bound
    pub fn has_binding(&self, symbol: &str) -> bool {
        self.shared.bindings.read().contains_key(symbol)
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
