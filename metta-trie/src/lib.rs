//! MettaTrie — A trie-map that discriminates on expression components directly.
//!
//! `MettaTrie<E, V>` is a trie-map where keys are sequences of `TrieKey`
//! discriminants (atom names, arities, literals, variables) rather than raw
//! bytes. This eliminates serialization overhead compared to byte-keyed tries
//! (like MORK+PathMap) while preserving trie advantages:
//!
//! - **O(k) insert/query** where k = expression depth
//! - **Prefix-based queries** via trie structure
//! - **Algebraic operations** (join, meet, subtract, restrict) via `Lattice` traits
//! - **CoW structural sharing** via `Arc<MettaTrieNode>` — O(1) clone, O(depth) mutation
//! - **Pattern matching** with variable/wildcard edges
//! - **Zero serialization** — decompose expressions directly into `TrieKey` sequences
//! - **Direct expression retrieval** — stores the original expression at each leaf
//!
//! ## Type Parameters
//!
//! - `E`: The expression type stored at leaves (e.g., `MettaValue`)
//! - `V`: The value type mapped to by the trie (e.g., `Multiplicity`)
//!
//! ## Example
//!
//! ```rust
//! use metta_trie::{MettaTrie, TrieKey};
//!
//! let mut trie: MettaTrie<String, u64> = MettaTrie::new();
//!
//! // Insert: (person "Alice") → multiplicity 1
//! let keys = vec![
//!     TrieKey::Arity(2),
//!     TrieKey::Atom("person"),
//!     TrieKey::Str("Alice"),
//! ];
//! // Note: "person" and "Alice" must be &'static str in production (interned)
//! // This example uses leaked strings for demonstration
//! let person: &'static str = Box::leak("person".to_string().into_boxed_str());
//! let alice: &'static str = Box::leak("Alice".to_string().into_boxed_str());
//! let keys = vec![TrieKey::Arity(2), TrieKey::Atom(person), TrieKey::Str(alice)];
//!
//! trie.insert_at(&keys, "(person Alice)".to_string(), 1);
//! assert_eq!(trie.get_at(&keys), Some(&1));
//! assert_eq!(trie.val_count(), 1);
//!
//! // O(1) clone via Arc
//! let forked = trie.clone();
//! assert_eq!(forked.val_count(), 1);
//! ```

pub mod algebra;
pub mod keys;
pub mod multiplicity;
pub mod node;
pub mod query;
pub mod zipper;

// Re-exports
pub use algebra::{
    AlgebraicResult, AlgebraicStatus, DistributiveLattice, Lattice, COUNTER_IDENT, SELF_IDENT,
};
pub use keys::TrieKey;
pub use node::{MettaTrie, MettaTrieNode, TrieIter};
pub use query::QueryMatch;
pub use multiplicity::Multiplicity;
pub use zipper::ReadZipper;
