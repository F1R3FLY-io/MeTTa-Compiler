//! Fast HashMap/HashSet type aliases with optional AES-NI acceleration
//!
//! When the `ahash-hasher` feature is enabled, this module provides
//! `FastHashMap` and `FastHashSet` types that use ahash's AES-NI accelerated
//! hasher instead of SipHash. This can provide 30-50% improvement on
//! hash-heavy operations on Intel Haswell+ and AMD Zen+ processors.
//!
//! When the feature is disabled, these types are simple aliases to
//! the standard library HashMap/HashSet.
//!
//! # Example
//! ```ignore
//! use crate::backend::hash_utils::{FastHashMap, FastHashSet};
//!
//! let mut map: FastHashMap<String, i64> = FastHashMap::default();
//! map.insert("key".to_string(), 42);
//!
//! let mut set: FastHashSet<String> = FastHashSet::default();
//! set.insert("value".to_string());
//! ```
//!
//! # Performance
//! - ahash uses AES-NI instructions (AESENC, AESDEC) on x86_64
//! - Provides ~2-3x faster hashing than SipHash for small keys
//! - Still provides DOS resistance via per-process random state

#[cfg(feature = "ahash-hasher")]
pub use ahash::{AHashMap as FastHashMap, AHashSet as FastHashSet};

#[cfg(not(feature = "ahash-hasher"))]
pub use std::collections::{HashMap as FastHashMap, HashSet as FastHashSet};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_hash_map() {
        let mut map: FastHashMap<String, i64> = FastHashMap::default();
        map.insert("key".to_string(), 42);
        assert_eq!(map.get("key"), Some(&42));
    }

    #[test]
    fn test_fast_hash_set() {
        let mut set: FastHashSet<String> = FastHashSet::default();
        set.insert("value".to_string());
        assert!(set.contains("value"));
    }
}
