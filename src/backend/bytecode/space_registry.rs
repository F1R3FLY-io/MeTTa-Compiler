//! Space Registry for JIT Runtime
//!
//! This module provides a thread-safe registry for named spaces used by the JIT
//! runtime functions `jit_runtime_eval_new` and `jit_runtime_load_space`.
//!
//! # Design
//!
//! - Lock-free via DashMap (concurrent HashMap)
//! - Maps space names to SpaceHandle instances
//! - Supports creating, looking up, and removing spaces by name
//!
//! # Example
//!
//! ```ignore
//! let mut registry = SpaceRegistry::new();
//!
//! // Create or get a named space
//! let handle = registry.get_or_create("my-space");
//!
//! // Look up by name
//! if let Some(space) = registry.get("my-space") {
//!     // Use space
//! }
//! ```

use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

use crate::backend::models::SpaceHandle;

/// Global counter for unique space IDs
static SPACE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique space ID
fn next_space_id() -> u64 {
    SPACE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Registry for named spaces used by JIT runtime
///
/// Uses DashMap for lock-free concurrent access.
pub struct SpaceRegistry {
    /// Spaces stored by name (lock-free concurrent HashMap)
    spaces: DashMap<String, SpaceHandle>,
}

impl std::fmt::Debug for SpaceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<_> = self.spaces.iter().map(|e| e.key().clone()).collect();
        f.debug_struct("SpaceRegistry")
            .field("space_count", &self.spaces.len())
            .field("names", &names)
            .finish()
    }
}

impl Default for SpaceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceRegistry {
    /// Create a new empty space registry
    pub fn new() -> Self {
        Self {
            spaces: DashMap::new(),
        }
    }

    /// Get a space by name
    pub fn get(&self, name: &str) -> Option<SpaceHandle> {
        self.spaces.get(name).map(|e| e.value().clone())
    }

    /// Create a new space with the given name
    ///
    /// Returns the created SpaceHandle. If a space with this name already exists,
    /// returns the existing one.
    pub fn create(&self, name: &str) -> SpaceHandle {
        // Use entry API for atomic get-or-insert
        self.spaces
            .entry(name.to_string())
            .or_insert_with(|| {
                let id = next_space_id();
                SpaceHandle::new(id, name.to_string())
            })
            .value()
            .clone()
    }

    /// Get or create a space by name
    ///
    /// If a space with this name exists, returns it.
    /// Otherwise creates a new space with the given name.
    pub fn get_or_create(&self, name: &str) -> SpaceHandle {
        // Use entry API for atomic get-or-insert
        self.create(name)
    }

    /// Register an existing SpaceHandle by name
    ///
    /// If a space with this name already exists, it will be replaced.
    pub fn register(&self, name: &str, handle: SpaceHandle) {
        self.spaces.insert(name.to_string(), handle);
    }

    /// Remove a space by name
    ///
    /// Returns true if the space was present and removed.
    pub fn remove(&self, name: &str) -> bool {
        self.spaces.remove(name).is_some()
    }

    /// Check if a space with the given name exists
    pub fn contains(&self, name: &str) -> bool {
        self.spaces.contains_key(name)
    }

    /// Get the number of registered spaces
    pub fn len(&self) -> usize {
        self.spaces.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.spaces.is_empty()
    }

    /// Get an iterator over space names
    pub fn names(&self) -> Vec<String> {
        self.spaces.iter().map(|e| e.key().clone()).collect()
    }

    /// Clear all spaces
    pub fn clear(&self) {
        self.spaces.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get() {
        let registry = SpaceRegistry::new();

        // Create a space
        let handle = registry.create("test-space");
        assert_eq!(handle.name, "test-space");

        // Get by name
        let retrieved = registry.get("test-space");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "test-space");
    }

    #[test]
    fn test_get_nonexistent() {
        let registry = SpaceRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_create_returns_existing() {
        let registry = SpaceRegistry::new();

        let first = registry.create("my-space");
        let second = registry.create("my-space");

        // Should be the same space (same ID)
        assert_eq!(first.id, second.id);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_get_or_create() {
        let registry = SpaceRegistry::new();

        // First call creates
        let first = registry.get_or_create("space1");
        assert_eq!(registry.len(), 1);

        // Second call returns existing
        let second = registry.get_or_create("space1");
        assert_eq!(first.id, second.id);
        assert_eq!(registry.len(), 1);

        // Different name creates new
        let third = registry.get_or_create("space2");
        assert_ne!(first.id, third.id);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_register() {
        let registry = SpaceRegistry::new();

        let custom = SpaceHandle::new(12345, "custom".to_string());
        registry.register("custom", custom);

        let retrieved = registry.get("custom").unwrap();
        assert_eq!(retrieved.id, 12345);
    }

    #[test]
    fn test_remove() {
        let registry = SpaceRegistry::new();

        registry.create("to-remove");
        assert!(registry.contains("to-remove"));

        let removed = registry.remove("to-remove");
        assert!(removed);
        assert!(!registry.contains("to-remove"));

        // Removing again returns false
        let removed_again = registry.remove("to-remove");
        assert!(!removed_again);
    }

    #[test]
    fn test_unique_ids() {
        let registry = SpaceRegistry::new();

        let space1 = registry.create("s1");
        let space2 = registry.create("s2");
        let space3 = registry.create("s3");

        // All should have unique IDs
        assert_ne!(space1.id, space2.id);
        assert_ne!(space2.id, space3.id);
        assert_ne!(space1.id, space3.id);
    }

    #[test]
    fn test_names() {
        let registry = SpaceRegistry::new();

        registry.create("alpha");
        registry.create("beta");
        registry.create("gamma");

        let names = registry.names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        assert!(names.contains(&"gamma".to_string()));
    }

    #[test]
    fn test_clear() {
        let registry = SpaceRegistry::new();

        registry.create("a");
        registry.create("b");
        assert_eq!(registry.len(), 2);

        registry.clear();
        assert!(registry.is_empty());
    }
}
