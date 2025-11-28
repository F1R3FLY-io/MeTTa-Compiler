//! Module Caching Infrastructure
//!
//! Provides hybrid caching with:
//! - Content hash: Deduplication of identical modules at different paths
//! - Path UID: Fast lookup by canonical path
//!
//! This ensures:
//! - Modules are only loaded once per unique content
//! - Symlinks and copies of the same file share a single loaded module
//! - Path-based lookups are O(1) after first load

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Module descriptor for caching and identity.
///
/// Combines path-based identity with content-based deduplication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleDescriptor {
    /// Canonical path to the module file.
    path: PathBuf,

    /// Path-based unique identifier (hash of the canonical path).
    /// Used for fast lookup when the same path is imported multiple times.
    path_uid: u64,

    /// Content hash for deduplication.
    /// Identical content at different paths shares the same ModId.
    content_hash: u64,
}

impl ModuleDescriptor {
    /// Create a new module descriptor from a path and content.
    ///
    /// # Arguments
    /// - `path` - The canonical path to the module file
    /// - `content` - The module's source content (for content-based deduplication)
    pub fn new(path: PathBuf, content: &str) -> Self {
        let path_uid = hash_path(&path);
        let content_hash = hash_content(content);
        Self {
            path,
            path_uid,
            content_hash,
        }
    }

    /// Create a descriptor from just a path (content unknown yet).
    /// Content hash will be 0 until updated.
    pub fn from_path(path: PathBuf) -> Self {
        let path_uid = hash_path(&path);
        Self {
            path,
            path_uid,
            content_hash: 0,
        }
    }

    /// Update the content hash after reading the file.
    pub fn with_content(mut self, content: &str) -> Self {
        self.content_hash = hash_content(content);
        self
    }

    /// Get the canonical path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the path-based unique identifier.
    pub fn path_uid(&self) -> u64 {
        self.path_uid
    }

    /// Get the content hash.
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }
}

impl Hash for ModuleDescriptor {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Use both path_uid and content_hash for the hash
        // This ensures descriptors are unique by both path and content
        self.path_uid.hash(state);
        self.content_hash.hash(state);
    }
}

/// Hash a path to a u64.
///
/// Uses the path's byte representation for consistent hashing.
#[inline]
pub fn hash_path(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.as_os_str().as_encoded_bytes().hash(&mut hasher);
    hasher.finish()
}

/// Hash content string to a u64.
///
/// Used for content-based deduplication.
#[inline]
pub fn hash_content(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Hash arbitrary bytes to a u64.
#[inline]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_descriptor_new() {
        let path = PathBuf::from("/home/user/project/lib/utils.metta");
        let content = "(= (foo) bar)";

        let desc = ModuleDescriptor::new(path.clone(), content);

        assert_eq!(desc.path(), &path);
        assert_ne!(desc.path_uid(), 0);
        assert_ne!(desc.content_hash(), 0);
    }

    #[test]
    fn test_content_deduplication() {
        let path1 = PathBuf::from("/path/a/module.metta");
        let path2 = PathBuf::from("/path/b/module.metta");
        let content = "(= (foo) bar)";

        let desc1 = ModuleDescriptor::new(path1, content);
        let desc2 = ModuleDescriptor::new(path2, content);

        // Different paths
        assert_ne!(desc1.path_uid(), desc2.path_uid());
        // Same content
        assert_eq!(desc1.content_hash(), desc2.content_hash());
    }

    #[test]
    fn test_same_path_same_uid() {
        let path1 = PathBuf::from("/home/user/module.metta");
        let path2 = PathBuf::from("/home/user/module.metta");

        assert_eq!(hash_path(&path1), hash_path(&path2));
    }

    #[test]
    fn test_different_path_different_uid() {
        let path1 = PathBuf::from("/home/user/module1.metta");
        let path2 = PathBuf::from("/home/user/module2.metta");

        assert_ne!(hash_path(&path1), hash_path(&path2));
    }

    #[test]
    fn test_from_path_then_with_content() {
        let path = PathBuf::from("/test/module.metta");
        let content = "(= (test) 42)";

        let desc = ModuleDescriptor::from_path(path.clone());
        assert_eq!(desc.content_hash(), 0);

        let desc = desc.with_content(content);
        assert_ne!(desc.content_hash(), 0);
        assert_eq!(desc.content_hash(), hash_content(content));
    }

    #[test]
    fn test_content_hash_stability() {
        let content = "some metta code";

        // Hash should be deterministic
        assert_eq!(hash_content(content), hash_content(content));
    }

    #[test]
    fn test_empty_content_hash() {
        let hash = hash_content("");
        // Empty string should have a valid hash (not 0)
        // DefaultHasher produces non-zero for empty input due to finalization
        assert_ne!(hash, 0);
    }

    #[test]
    fn test_hash_collision_resistance() {
        // Different content should (almost certainly) produce different hashes
        let hash1 = hash_content("(= (foo) bar)");
        let hash2 = hash_content("(= (foo) baz)");
        let hash3 = hash_content("(= (bar) foo)");

        assert_ne!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_ne!(hash2, hash3);
    }
}
