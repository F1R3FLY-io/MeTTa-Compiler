//! Symbol interning for fast symbol comparison and reduced memory usage
//!
//! When the `symbol-interning` feature is enabled, this module provides a `Symbol` type
//! that uses lasso's ThreadedRodeo for O(1) symbol comparison via interned Spur keys.
//!
//! When the feature is disabled, `Symbol` is a simple wrapper around `String` for compatibility.
//!
//! # Performance Characteristics
//!
//! With `symbol-interning`:
//! - Symbol creation: O(1) amortized (hash table lookup/insert)
//! - Symbol comparison: O(1) (integer comparison)
//! - Memory: One copy per unique string + 4 bytes per Symbol instance
//! - Thread-safe: Uses ThreadedRodeo for concurrent interning
//!
//! Without `symbol-interning`:
//! - Symbol creation: O(n) where n = string length (clone)
//! - Symbol comparison: O(n) (string comparison)
//! - Memory: Full string per Symbol instance
//!
//! # Example
//! ```ignore
//! use crate::backend::symbol::{Symbol, intern};
//!
//! let s1 = intern("hello");
//! let s2 = intern("hello");
//! assert_eq!(s1, s2);  // O(1) comparison with feature, O(n) without
//!
//! let as_str: &str = s1.as_str();
//! assert_eq!(as_str, "hello");
//! ```

use std::fmt;
use std::hash::{Hash, Hasher};

#[cfg(feature = "symbol-interning")]
mod interned {
    use lasso::{Spur, ThreadedRodeo};
    use std::sync::OnceLock;

    /// Global interner for symbols - lazily initialized, thread-safe
    static INTERNER: OnceLock<ThreadedRodeo> = OnceLock::new();

    /// Get or initialize the global interner
    #[inline]
    fn interner() -> &'static ThreadedRodeo {
        INTERNER.get_or_init(ThreadedRodeo::new)
    }

    /// Interned symbol - 4 bytes, O(1) comparison
    #[derive(Copy, Clone, Eq, PartialEq, Hash)]
    pub struct Symbol(Spur);

    impl Symbol {
        /// Create a new symbol from a string (interns if new)
        #[inline]
        pub fn new(s: &str) -> Self {
            Symbol(interner().get_or_intern(s))
        }

        /// Create a new symbol from an owned string
        #[inline]
        pub fn from_string(s: String) -> Self {
            Symbol(interner().get_or_intern(s))
        }

        /// Get the string representation of this symbol
        #[inline]
        pub fn as_str(&self) -> &'static str {
            interner().resolve(&self.0)
        }

        /// Convert to owned String
        #[inline]
        pub fn to_string(&self) -> String {
            self.as_str().to_string()
        }
    }

    impl std::fmt::Debug for Symbol {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Symbol({:?})", self.as_str())
        }
    }

    impl std::fmt::Display for Symbol {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.as_str())
        }
    }

    impl From<&str> for Symbol {
        #[inline]
        fn from(s: &str) -> Self {
            Symbol::new(s)
        }
    }

    impl From<String> for Symbol {
        #[inline]
        fn from(s: String) -> Self {
            Symbol::from_string(s)
        }
    }

    impl From<&String> for Symbol {
        #[inline]
        fn from(s: &String) -> Self {
            Symbol::new(s.as_str())
        }
    }

    impl AsRef<str> for Symbol {
        #[inline]
        fn as_ref(&self) -> &str {
            self.as_str()
        }
    }

    impl PartialEq<str> for Symbol {
        fn eq(&self, other: &str) -> bool {
            self.as_str() == other
        }
    }

    impl PartialEq<&str> for Symbol {
        fn eq(&self, other: &&str) -> bool {
            self.as_str() == *other
        }
    }

    impl PartialEq<String> for Symbol {
        fn eq(&self, other: &String) -> bool {
            self.as_str() == other.as_str()
        }
    }

    /// Intern a string and return a Symbol
    #[inline]
    pub fn intern(s: &str) -> Symbol {
        Symbol::new(s)
    }

    /// Intern an owned string and return a Symbol
    #[inline]
    pub fn intern_string(s: String) -> Symbol {
        Symbol::from_string(s)
    }
}

#[cfg(not(feature = "symbol-interning"))]
mod string_based {
    /// Non-interned symbol - just a String wrapper for API compatibility
    #[derive(Clone, Eq, PartialEq, Hash, Debug)]
    pub struct Symbol(String);

    impl Symbol {
        /// Create a new symbol from a string
        #[inline]
        pub fn new(s: &str) -> Self {
            Symbol(s.to_string())
        }

        /// Create a new symbol from an owned string (no copy)
        #[inline]
        pub fn from_string(s: String) -> Self {
            Symbol(s)
        }

        /// Get the string representation of this symbol
        #[inline]
        pub fn as_str(&self) -> &str {
            &self.0
        }

        /// Convert to owned String
        #[inline]
        pub fn to_string(&self) -> String {
            self.0.clone()
        }
    }

    impl std::fmt::Display for Symbol {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl From<&str> for Symbol {
        #[inline]
        fn from(s: &str) -> Self {
            Symbol::new(s)
        }
    }

    impl From<String> for Symbol {
        #[inline]
        fn from(s: String) -> Self {
            Symbol::from_string(s)
        }
    }

    impl From<&String> for Symbol {
        #[inline]
        fn from(s: &String) -> Self {
            Symbol::new(s.as_str())
        }
    }

    impl AsRef<str> for Symbol {
        #[inline]
        fn as_ref(&self) -> &str {
            &self.0
        }
    }

    impl PartialEq<str> for Symbol {
        fn eq(&self, other: &str) -> bool {
            self.0 == other
        }
    }

    impl PartialEq<&str> for Symbol {
        fn eq(&self, other: &&str) -> bool {
            self.0 == *other
        }
    }

    impl PartialEq<String> for Symbol {
        fn eq(&self, other: &String) -> bool {
            &self.0 == other
        }
    }

    /// Intern a string and return a Symbol (no actual interning without feature)
    #[inline]
    pub fn intern(s: &str) -> Symbol {
        Symbol::new(s)
    }

    /// Intern an owned string and return a Symbol (no actual interning without feature)
    #[inline]
    pub fn intern_string(s: String) -> Symbol {
        Symbol::from_string(s)
    }
}

// Re-export the appropriate implementation
#[cfg(feature = "symbol-interning")]
pub use interned::{intern, intern_string, Symbol};

#[cfg(not(feature = "symbol-interning"))]
pub use string_based::{intern, intern_string, Symbol};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_creation() {
        let s1 = Symbol::new("hello");
        let s2 = intern("hello");
        assert_eq!(s1, s2);
        assert_eq!(s1.as_str(), "hello");
    }

    #[test]
    fn test_symbol_from_string() {
        let s1 = Symbol::from_string("world".to_string());
        let s2 = intern_string("world".to_string());
        assert_eq!(s1, s2);
        assert_eq!(s1.as_str(), "world");
    }

    #[test]
    fn test_symbol_equality() {
        let s1 = intern("test");
        let s2 = intern("test");
        let s3 = intern("other");
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_symbol_partial_eq_str() {
        let s = intern("hello");
        assert!(s == "hello");
        assert!(s != "world");
    }

    #[test]
    fn test_symbol_hash() {
        use std::collections::HashMap;
        let mut map: HashMap<Symbol, i32> = HashMap::new();
        map.insert(intern("key"), 42);
        assert_eq!(map.get(&intern("key")), Some(&42));
    }

    #[test]
    fn test_symbol_display() {
        let s = intern("display_test");
        assert_eq!(format!("{}", s), "display_test");
    }
}
