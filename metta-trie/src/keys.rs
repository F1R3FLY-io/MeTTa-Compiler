//! Trie Key Types for MettaTrie
//!
//! Defines `TrieKey`, the discrimination key used at each level of the trie.
//! Each MeTTa expression is decomposed into a sequence of `TrieKey` values
//! via the `decompose()` function.
//!
//! ## Design
//!
//! `TrieKey` is derived from the `DiscKey` enum in `discrimination_tree.rs`,
//! extended with additional variants for full MettaValue coverage (URI, Unit,
//! Error). The decomposition is a depth-first flattening:
//!
//! ```text
//! (person "Alice" 42) → [Arity(3), Atom("person"), Str("Alice"), Long(42)]
//! (= (fib $n) body)   → [Arity(3), Atom("="), Arity(2), Atom("fib"), Variable, ...]
//! ```
//!
//! ## Key Properties
//!
//! - `Atom` and `Str` use `&'static str` (interned pointers) for O(1) comparison
//! - `Float` uses bitwise representation for exact hashing (no NaN issues)
//! - `Variable` acts as a wildcard during pattern queries
//! - `Arity(n)` indicates an S-expression with `n` children follow in the path

use std::fmt;
use std::hash::{Hash, Hasher};

/// A key in the MettaTrie, representing one level of expression structure.
///
/// During insertion, expressions are decomposed into sequences of `TrieKey`s.
/// During querying, `Variable` positions match any concrete key.
#[derive(Clone, Eq, PartialEq)]
pub enum TrieKey {
    /// An S-expression with the given number of children.
    /// The next `n` segments in the key sequence represent the children.
    Arity(u16),

    /// An atom symbol. Uses interned `&'static str` for O(1) pointer comparison.
    Atom(&'static str),

    /// An integer literal.
    Long(i64),

    /// A boolean literal.
    Bool(bool),

    /// A float literal stored as its bit representation for exact hashing.
    /// Avoids NaN comparison issues.
    Float(u64),

    /// A string literal. Uses interned `&'static str` for O(1) pointer comparison.
    Str(&'static str),

    /// A URI literal. Uses interned `&'static str` for O(1) pointer comparison.
    Uri(&'static str),

    /// A variable or wildcard position.
    /// During insertion of patterns, marks positions where any value matches.
    /// During querying, the traversal follows both concrete and Variable edges.
    Variable,

    /// The unit/empty value.
    Unit,

    /// An error marker in patterns.
    Error,
}

impl Hash for TrieKey {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Discriminant tag first for efficient bucket distribution
        std::mem::discriminant(self).hash(state);
        match self {
            TrieKey::Arity(n) => n.hash(state),
            TrieKey::Atom(s) => {
                // Hash pointer address for interned strings (O(1))
                (*s as *const str).hash(state);
            }
            TrieKey::Long(n) => n.hash(state),
            TrieKey::Bool(b) => b.hash(state),
            TrieKey::Float(bits) => bits.hash(state),
            TrieKey::Str(s) => {
                (*s as *const str).hash(state);
            }
            TrieKey::Uri(s) => {
                (*s as *const str).hash(state);
            }
            TrieKey::Variable | TrieKey::Unit | TrieKey::Error => {}
        }
    }
}

impl fmt::Debug for TrieKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrieKey::Arity(n) => write!(f, "Arity({n})"),
            TrieKey::Atom(s) => write!(f, "Atom({s:?})"),
            TrieKey::Long(n) => write!(f, "Long({n})"),
            TrieKey::Bool(b) => write!(f, "Bool({b})"),
            TrieKey::Float(bits) => write!(f, "Float({:?})", f64::from_bits(*bits)),
            TrieKey::Str(s) => write!(f, "Str({s:?})"),
            TrieKey::Uri(s) => write!(f, "Uri({s:?})"),
            TrieKey::Variable => write!(f, "Var"),
            TrieKey::Unit => write!(f, "Unit"),
            TrieKey::Error => write!(f, "Error"),
        }
    }
}

impl fmt::Display for TrieKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_trie_key_equality() {
        assert_eq!(TrieKey::Arity(3), TrieKey::Arity(3));
        assert_ne!(TrieKey::Arity(3), TrieKey::Arity(2));
        assert_eq!(TrieKey::Long(42), TrieKey::Long(42));
        assert_eq!(TrieKey::Bool(true), TrieKey::Bool(true));
        assert_ne!(TrieKey::Bool(true), TrieKey::Bool(false));
        assert_eq!(TrieKey::Variable, TrieKey::Variable);
        assert_eq!(TrieKey::Unit, TrieKey::Unit);
        assert_ne!(TrieKey::Unit, TrieKey::Error);
    }

    #[test]
    fn test_trie_key_hash_map_usage() {
        let mut map: HashMap<TrieKey, u32> = HashMap::new();
        map.insert(TrieKey::Arity(3), 1);
        map.insert(TrieKey::Long(42), 2);
        map.insert(TrieKey::Variable, 3);

        assert_eq!(map.get(&TrieKey::Arity(3)), Some(&1));
        assert_eq!(map.get(&TrieKey::Long(42)), Some(&2));
        assert_eq!(map.get(&TrieKey::Variable), Some(&3));
        assert_eq!(map.get(&TrieKey::Arity(4)), None);
    }

    #[test]
    fn test_interned_str_equality() {
        // Interned strings with same address should be equal
        let s: &'static str = "hello";
        assert_eq!(TrieKey::Atom(s), TrieKey::Atom(s));
    }
}
