//! Scope tracking operations for Environment.
//!
//! Provides methods for managing lexical scopes during evaluation.

use std::sync::atomic::Ordering;

use super::Environment;

impl Environment {
    /// Push a new scope onto the scope tracker.
    /// Called when entering lexical contexts like `let`, `match`, or function bodies.
    pub fn push_scope(&mut self) {
        self.make_owned();
        self.shared
            .scope_tracker
            .write()
            .expect("scope_tracker lock poisoned")
            .push_scope();
        self.modified.store(true, Ordering::Release);
    }

    /// Pop the innermost scope from the scope tracker.
    /// Called when leaving lexical contexts. Never pops the global scope.
    pub fn pop_scope(&mut self) {
        self.make_owned();
        self.shared
            .scope_tracker
            .write()
            .expect("scope_tracker lock poisoned")
            .pop_scope();
        self.modified.store(true, Ordering::Release);
    }

    /// Add a symbol to the current (innermost) scope.
    /// Called when introducing bindings (e.g., pattern variables in `let` or `match`).
    pub fn add_scope_symbol(&mut self, name: String) {
        self.make_owned();
        self.shared
            .scope_tracker
            .write()
            .expect("scope_tracker lock poisoned")
            .add_symbol(name);
        self.modified.store(true, Ordering::Release);
    }

    /// Add multiple symbols to the current scope.
    pub fn add_scope_symbols(&mut self, names: impl IntoIterator<Item = String>) {
        self.make_owned();
        self.shared
            .scope_tracker
            .write()
            .expect("scope_tracker lock poisoned")
            .add_symbols(names);
        self.modified.store(true, Ordering::Release);
    }

    /// Check if a symbol is visible in the current scope hierarchy.
    pub fn is_symbol_visible(&self, name: &str) -> bool {
        self.shared
            .scope_tracker
            .read()
            .expect("scope_tracker lock poisoned")
            .is_visible(name)
    }

    /// Get all visible symbols from the scope tracker, ordered local-first.
    /// Returns symbols from innermost scope first for prioritized recommendations.
    pub fn visible_scope_symbols(&self) -> Vec<String> {
        self.shared
            .scope_tracker
            .read()
            .expect("scope_tracker lock poisoned")
            .visible_symbols()
            .cloned()
            .collect()
    }

    /// Get the current scope depth (1 = global only).
    pub fn scope_depth(&self) -> usize {
        self.shared
            .scope_tracker
            .read()
            .expect("scope_tracker lock poisoned")
            .depth()
    }

    /// Check if currently at global scope.
    pub fn at_global_scope(&self) -> bool {
        self.shared
            .scope_tracker
            .read()
            .expect("scope_tracker lock poisoned")
            .at_global_scope()
    }
}
