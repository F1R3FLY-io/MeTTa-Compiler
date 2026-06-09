//! Store-addressed continuation-spine substrate for Selective CESK*.
//!
//! Re-enterable continuation families should be named by compact store
//! addresses, not embedded as opaque side stacks. The typed stores in this
//! module are intentionally small: ownership and root-walking policy stay with
//! each continuation family, while address allocation/removal is shared.

use std::collections::HashMap;

/// Address of a re-enterable continuation node in the selective continuation
/// spine. This is the E3 capability boundary for state that can be resumed by
/// backtracking, lazy branch production, or a captured/suspended continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContinuationAddr(u32);

impl ContinuationAddr {
    /// Return the compact raw index for serialization/coupling proofs.
    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Typed store for one family of continuation-spine nodes.
#[derive(Debug)]
pub struct SpineStore<N> {
    next_raw: u32,
    nodes: HashMap<ContinuationAddr, N>,
}

impl<N> Default for SpineStore<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N> SpineStore<N> {
    /// Create an empty continuation-spine store.
    pub fn new() -> Self {
        Self {
            next_raw: 1,
            nodes: HashMap::new(),
        }
    }

    /// Allocate a fresh address for `node`.
    pub fn alloc(&mut self, node: N) -> ContinuationAddr {
        let raw = self.next_raw;
        self.next_raw = self
            .next_raw
            .checked_add(1)
            .expect("continuation spine address space exhausted");
        let addr = ContinuationAddr(raw);
        let old = self.nodes.insert(addr, node);
        debug_assert!(
            old.is_none(),
            "fresh continuation address was already occupied"
        );
        addr
    }

    /// Read a node by address.
    #[inline]
    pub fn get(&self, addr: ContinuationAddr) -> Option<&N> {
        self.nodes.get(&addr)
    }

    /// Mutably read a node by address.
    #[inline]
    pub fn get_mut(&mut self, addr: ContinuationAddr) -> Option<&mut N> {
        self.nodes.get_mut(&addr)
    }

    /// Remove a node by address, returning ownership of the payload.
    #[inline]
    pub fn remove(&mut self, addr: ContinuationAddr) -> Option<N> {
        self.nodes.remove(&addr)
    }

    /// True iff a node remains allocated at `addr`.
    #[inline]
    pub fn contains(&self, addr: ContinuationAddr) -> bool {
        self.nodes.contains_key(&addr)
    }

    /// Remove every live node while preserving monotone address allocation.
    #[inline]
    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Number of currently allocated nodes.
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the store has no live nodes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_spine_store_allocates_and_removes_by_address() {
        let mut store = SpineStore::new();
        let first = store.alloc("first");
        let second = store.alloc("second");

        assert_ne!(first, second);
        assert_eq!(store.get(first), Some(&"first"));
        assert_eq!(store.get(second), Some(&"second"));
        assert_eq!(store.remove(first), Some("first"));
        assert!(!store.contains(first));
        assert!(store.contains(second));
    }
}
