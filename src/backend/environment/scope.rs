//! Hierarchical scope tracking for context-aware symbol resolution.
//!
//! Provides scope management for:
//! - Scope-aware "Did you mean?" suggestions (prioritize local symbols)
//! - Tracking variable bindings introduced by `let`, `match`, and functions
//! - Enabling local symbols to shadow global ones in recommendations

use std::collections::HashSet;

/// Hierarchical scope tracker for context-aware symbol resolution.
///
/// Maintains a stack of lexical scopes, where each scope contains the symbols
/// defined within that scope. Used for:
/// - Scope-aware "Did you mean?" suggestions (prioritize local symbols)
/// - Tracking variable bindings introduced by `let`, `match`, and functions
/// - Enabling local symbols to shadow global ones in recommendations
///
/// # Example
/// ```ignore
/// // At global scope, define rule: (= (fib $n) ...)
/// // scope_stack = [{fib}]
///
/// // Inside (let helper (...) body):
/// // scope_stack = [{fib}, {helper}]
///
/// // Inside nested (let x 1 ...):
/// // scope_stack = [{fib}, {helper}, {x}]
/// ```
#[derive(Debug, Clone)]
pub struct ScopeTracker {
    /// Stack of scopes, from global (index 0) to innermost (last)
    scopes: Vec<HashSet<String>>,
}

impl ScopeTracker {
    /// Create a new scope tracker with a single global scope
    pub fn new() -> Self {
        Self {
            scopes: vec![HashSet::new()],
        }
    }

    /// Push a new scope onto the stack (entering a new lexical context)
    pub fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    /// Pop the innermost scope (leaving a lexical context)
    /// Never pops the global scope (index 0)
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Add a symbol to the current (innermost) scope
    pub fn add_symbol(&mut self, name: String) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name);
        }
    }

    /// Add multiple symbols to the current scope
    pub fn add_symbols(&mut self, names: impl IntoIterator<Item = String>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.extend(names);
        }
    }

    /// Check if a symbol is visible from the current scope
    /// Searches from innermost to outermost scope
    pub fn is_visible(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(name))
    }

    /// Get all visible symbols, ordered from local (innermost) to global (outermost)
    /// Local symbols appear first for prioritized recommendations
    pub fn visible_symbols(&self) -> impl Iterator<Item = &String> {
        self.scopes.iter().rev().flat_map(|scope| scope.iter())
    }

    /// Get symbols from the current (innermost) scope only
    pub fn local_symbols(&self) -> impl Iterator<Item = &String> {
        self.scopes
            .last()
            .into_iter()
            .flat_map(|scope| scope.iter())
    }

    /// Get the current scope depth (1 = global only)
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }

    /// Check if we're at global scope (depth 1)
    pub fn at_global_scope(&self) -> bool {
        self.scopes.len() == 1
    }
}

impl Default for ScopeTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Basic Operations
    // ========================================================================

    #[test]
    fn test_scope_tracker_new() {
        // New ScopeTracker should have exactly one scope (global)
        let tracker = ScopeTracker::new();
        assert_eq!(
            tracker.depth(),
            1,
            "New tracker should have depth 1 (global scope)"
        );
        assert!(
            tracker.at_global_scope(),
            "New tracker should be at global scope"
        );
    }

    #[test]
    fn test_scope_tracker_default() {
        // Default implementation should be equivalent to new()
        let tracker = ScopeTracker::default();
        assert_eq!(tracker.depth(), 1);
        assert!(tracker.at_global_scope());
    }

    #[test]
    fn test_scope_tracker_push_scope() {
        let mut tracker = ScopeTracker::new();

        tracker.push_scope();
        assert_eq!(tracker.depth(), 2, "After one push, depth should be 2");
        assert!(
            !tracker.at_global_scope(),
            "Should not be at global scope after push"
        );

        tracker.push_scope();
        assert_eq!(tracker.depth(), 3, "After two pushes, depth should be 3");

        tracker.push_scope();
        assert_eq!(tracker.depth(), 4, "After three pushes, depth should be 4");
    }

    #[test]
    fn test_scope_tracker_pop_scope() {
        let mut tracker = ScopeTracker::new();

        tracker.push_scope();
        tracker.push_scope();
        assert_eq!(tracker.depth(), 3);

        tracker.pop_scope();
        assert_eq!(tracker.depth(), 2, "After one pop, depth should be 2");

        tracker.pop_scope();
        assert_eq!(tracker.depth(), 1, "After two pops, depth should be 1");
        assert!(tracker.at_global_scope(), "Should be back at global scope");
    }

    #[test]
    fn test_scope_tracker_pop_at_global_never_panics() {
        // Popping at global scope should be safe (no panic, stays at depth 1)
        let mut tracker = ScopeTracker::new();
        assert_eq!(tracker.depth(), 1);

        // Pop multiple times at global scope - should never panic
        tracker.pop_scope();
        assert_eq!(tracker.depth(), 1, "Pop at global should stay at depth 1");

        tracker.pop_scope();
        assert_eq!(
            tracker.depth(),
            1,
            "Second pop at global should still be depth 1"
        );

        tracker.pop_scope();
        assert_eq!(
            tracker.depth(),
            1,
            "Third pop at global should still be depth 1"
        );
    }

    // ========================================================================
    // Symbol Addition and Visibility
    // ========================================================================

    #[test]
    fn test_scope_tracker_add_symbol() {
        let mut tracker = ScopeTracker::new();

        tracker.add_symbol("foo".to_string());
        assert!(tracker.is_visible("foo"), "Added symbol should be visible");
        assert!(
            !tracker.is_visible("bar"),
            "Non-added symbol should not be visible"
        );
    }

    #[test]
    fn test_scope_tracker_add_symbols() {
        let mut tracker = ScopeTracker::new();

        tracker.add_symbols(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert!(tracker.is_visible("a"), "First symbol should be visible");
        assert!(tracker.is_visible("b"), "Second symbol should be visible");
        assert!(tracker.is_visible("c"), "Third symbol should be visible");
        assert!(
            !tracker.is_visible("d"),
            "Non-added symbol should not be visible"
        );
    }

    #[test]
    fn test_scope_tracker_visibility_across_scopes() {
        let mut tracker = ScopeTracker::new();

        // Add to global scope
        tracker.add_symbol("global_var".to_string());

        // Push nested scope and add local symbol
        tracker.push_scope();
        tracker.add_symbol("local_var".to_string());

        // Both should be visible from inner scope
        assert!(
            tracker.is_visible("global_var"),
            "Global symbol should be visible from inner scope"
        );
        assert!(
            tracker.is_visible("local_var"),
            "Local symbol should be visible from inner scope"
        );

        // Pop back to global scope
        tracker.pop_scope();

        // Global still visible, local no longer visible
        assert!(
            tracker.is_visible("global_var"),
            "Global symbol should still be visible"
        );
        assert!(
            !tracker.is_visible("local_var"),
            "Local symbol should not be visible after pop"
        );
    }

    #[test]
    fn test_scope_tracker_shadowing() {
        let mut tracker = ScopeTracker::new();

        // Add "x" to global scope
        tracker.add_symbol("x".to_string());
        assert!(tracker.is_visible("x"), "x should be visible in global");

        // Push scope and add "x" again (shadowing)
        tracker.push_scope();
        tracker.add_symbol("x".to_string());
        assert!(
            tracker.is_visible("x"),
            "x should still be visible (shadowed)"
        );

        // Count occurrences - should be 2
        let count = tracker.visible_symbols().filter(|s| *s == "x").count();
        assert_eq!(count, 2, "Should see 'x' twice when shadowed");

        // Pop scope
        tracker.pop_scope();

        // Should still see global "x"
        assert!(tracker.is_visible("x"), "x should be visible after pop");
        let count = tracker.visible_symbols().filter(|s| *s == "x").count();
        assert_eq!(count, 1, "Should only see one 'x' after pop");
    }

    // ========================================================================
    // Symbol Iteration Order
    // ========================================================================

    #[test]
    fn test_scope_tracker_visible_symbols_order() {
        let mut tracker = ScopeTracker::new();

        // Add to global scope
        tracker.add_symbol("global".to_string());

        // Push scope and add local
        tracker.push_scope();
        tracker.add_symbol("local".to_string());

        // Collect visible symbols - local should appear before global
        let symbols: Vec<&String> = tracker.visible_symbols().collect();

        // Find indices
        let local_idx = symbols.iter().position(|s| *s == "local");
        let global_idx = symbols.iter().position(|s| *s == "global");

        assert!(local_idx.is_some(), "local should be in visible symbols");
        assert!(global_idx.is_some(), "global should be in visible symbols");
        assert!(
            local_idx.unwrap() < global_idx.unwrap(),
            "Local symbols should appear before global symbols (innermost first)"
        );
    }

    #[test]
    fn test_scope_tracker_local_symbols() {
        let mut tracker = ScopeTracker::new();

        tracker.add_symbol("global".to_string());
        tracker.push_scope();
        tracker.add_symbol("local1".to_string());
        tracker.add_symbol("local2".to_string());

        // local_symbols should only return symbols from current (innermost) scope
        let local: Vec<&String> = tracker.local_symbols().collect();
        assert_eq!(local.len(), 2, "Should have 2 local symbols");
        assert!(local.contains(&&"local1".to_string()));
        assert!(local.contains(&&"local2".to_string()));
        assert!(
            !local.contains(&&"global".to_string()),
            "Global should not be in local_symbols"
        );
    }

    // ========================================================================
    // Deeply Nested Scopes
    // ========================================================================

    #[test]
    fn test_scope_tracker_deep_nesting() {
        let mut tracker = ScopeTracker::new();

        // Create 10 nested scopes, adding a symbol at each level
        for i in 0..10 {
            tracker.add_symbol(format!("level_{}", i));
            tracker.push_scope();
        }

        assert_eq!(
            tracker.depth(),
            11,
            "Should have 11 scopes (global + 10 nested)"
        );

        // All 10 symbols should be visible
        for i in 0..10 {
            assert!(
                tracker.is_visible(&format!("level_{}", i)),
                "Symbol at level {} should be visible",
                i
            );
        }

        // Pop all scopes back to global
        for _ in 0..10 {
            tracker.pop_scope();
        }

        assert_eq!(tracker.depth(), 1);
        assert!(tracker.at_global_scope());

        // Only level_0 (global scope) should still be visible
        assert!(
            tracker.is_visible("level_0"),
            "Global symbol should still be visible"
        );
        for i in 1..10 {
            assert!(
                !tracker.is_visible(&format!("level_{}", i)),
                "Symbol at level {} should no longer be visible",
                i
            );
        }
    }

    #[test]
    fn test_scope_tracker_real_world_let_nesting() {
        // Simulate: (let x 1 (let y 2 (let z 3 (+ x y z))))
        let mut tracker = ScopeTracker::new();

        // Enter first let scope, bind x
        tracker.push_scope();
        tracker.add_symbol("x".to_string());

        // Enter second let scope, bind y
        tracker.push_scope();
        tracker.add_symbol("y".to_string());

        // Enter third let scope, bind z
        tracker.push_scope();
        tracker.add_symbol("z".to_string());

        // All should be visible
        assert!(tracker.is_visible("x"));
        assert!(tracker.is_visible("y"));
        assert!(tracker.is_visible("z"));
        assert_eq!(tracker.depth(), 4);

        // Pop back through scopes
        tracker.pop_scope(); // exit z scope
        assert!(tracker.is_visible("x"));
        assert!(tracker.is_visible("y"));
        assert!(!tracker.is_visible("z"));

        tracker.pop_scope(); // exit y scope
        assert!(tracker.is_visible("x"));
        assert!(!tracker.is_visible("y"));

        tracker.pop_scope(); // exit x scope
        assert!(!tracker.is_visible("x"));
        assert!(tracker.at_global_scope());
    }

    // ========================================================================
    // Edge Cases
    // ========================================================================

    #[test]
    fn test_scope_tracker_empty_string_symbol() {
        let mut tracker = ScopeTracker::new();

        tracker.add_symbol("".to_string());
        assert!(
            tracker.is_visible(""),
            "Empty string symbol should be visible"
        );
    }

    #[test]
    fn test_scope_tracker_special_characters() {
        let mut tracker = ScopeTracker::new();

        tracker.add_symbol("$var".to_string());
        tracker.add_symbol("&space".to_string());
        tracker.add_symbol("'quoted".to_string());
        tracker.add_symbol("hyphen-name".to_string());
        tracker.add_symbol("underscore_name".to_string());

        assert!(tracker.is_visible("$var"));
        assert!(tracker.is_visible("&space"));
        assert!(tracker.is_visible("'quoted"));
        assert!(tracker.is_visible("hyphen-name"));
        assert!(tracker.is_visible("underscore_name"));
    }

    #[test]
    fn test_scope_tracker_clone() {
        let mut tracker = ScopeTracker::new();
        tracker.add_symbol("a".to_string());
        tracker.push_scope();
        tracker.add_symbol("b".to_string());

        // Clone the tracker
        let mut cloned = tracker.clone();

        // Modifications to clone should not affect original
        cloned.add_symbol("c".to_string());
        cloned.push_scope();
        cloned.add_symbol("d".to_string());

        // Original should still have depth 2
        assert_eq!(tracker.depth(), 2);
        assert!(!tracker.is_visible("c"));
        assert!(!tracker.is_visible("d"));

        // Clone should have depth 3
        assert_eq!(cloned.depth(), 3);
        assert!(cloned.is_visible("c"));
        assert!(cloned.is_visible("d"));
    }

    #[test]
    fn test_scope_tracker_add_duplicate_symbols() {
        let mut tracker = ScopeTracker::new();

        // Adding the same symbol multiple times in same scope - should be deduplicated
        tracker.add_symbol("dup".to_string());
        tracker.add_symbol("dup".to_string());
        tracker.add_symbol("dup".to_string());

        // Should only appear once in visible_symbols (HashSet behavior)
        let count = tracker.visible_symbols().filter(|s| *s == "dup").count();
        assert_eq!(
            count, 1,
            "Duplicate symbols in same scope should be deduplicated"
        );
    }
}
