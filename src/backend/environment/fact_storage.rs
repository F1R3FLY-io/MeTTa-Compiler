//! Fact storage operations for Environment.
//!
//! Provides methods for adding, removing, and querying facts via MettaTrie.
//! All atoms stored directly as MettaValue expressions with TrieKey decomposition.

use std::sync::atomic::Ordering;

use tracing::trace;

use metta_trie::Multiplicity;

use super::{MettaEnvironment, MettaValue};
use crate::backend::decompose::decompose_literal;
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValueTrait};

impl MettaEnvironment {
    /// Check if an atom fact exists (queries MettaTrie)
    /// Uses O(k) exact match via decompose_literal + get_at where k = expression depth
    ///
    /// For atoms (always ground), this provides O(1)-like performance
    pub fn has_fact(&self, atom: &str) -> bool {
        trace!(target: "mettatron::environment::has_fact", atom);
        let atom_value = self.factory.atom(atom);
        let keys = decompose_literal(&atom_value);
        let btm = self.shared.atom_space.btm.read();
        super::multiplicity::trie_get_multiplicity(&btm, &keys) > 0
    }

    /// Check if an s-expression fact exists in the MettaTrie
    /// Uses structural equivalence to handle variable name normalization
    ///
    /// OPTIMIZED: Uses O(k) exact match for ground expressions via decompose_literal + get_at
    /// Falls back to O(n) linear search for patterns with variables
    pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
        trace!(target: "mettatron::environment::has_sexpr_fact", ?sexpr);
        // Fast path: O(k) exact match for ground (variable-free) expressions
        if !Self::contains_variables(sexpr) {
            if let Some(matched) = self.descend_to_exact_match(sexpr) {
                return sexpr.structurally_equivalent(&matched);
            }
            // Fast path failed - fall back to linear search
            trace!(target: "mettatron::environment::has_sexpr_fact", "Fast path failed, using linear search");
            return self.has_sexpr_fact_linear(sexpr);
        }

        // Slow path: O(n) linear search for patterns with variables
        trace!(target: "mettatron::environment::has_sexpr_fact", "Using linear search (contains variables)");
        self.has_sexpr_fact_linear(sexpr)
    }

    /// Fallback linear search for has_sexpr_fact (O(n) iteration)
    fn has_sexpr_fact_linear(&self, sexpr: &MettaValue) -> bool {
        let btm = self.shared.atom_space.btm.read();
        for (stored_value, _mult) in btm.iter() {
            if sexpr.structurally_equivalent(stored_value) {
                return true;
            }
        }
        false
    }

    /// Check if a MettaValue contains variables ($x, &y, 'z, or _)
    /// Space references like &self, &kb, &stack are NOT variables
    ///
    /// Delegates to `MettaValueTrait::contains_variables()`.
    pub(crate) fn contains_variables(value: &MettaValue) -> bool {
        value.contains_variables()
    }

    /// Try exact match lookup using decompose_literal + MettaTrie::get_entry_at
    /// Returns Some(value) if exact match found, None otherwise
    ///
    /// This provides O(k) lookup time where k = expression depth (typically 3-5)
    /// compared to O(n) for linear iteration where n = total facts in space
    ///
    /// Only works for ground (variable-free) patterns.
    pub(crate) fn descend_to_exact_match(&self, pattern: &MettaValue) -> Option<MettaValue> {
        if Self::contains_variables(pattern) {
            return None;
        }
        let keys = decompose_literal(pattern);
        let btm = self.shared.atom_space.btm.read();
        btm.get_entry_at(&keys).map(|(expr, _)| expr.clone())
    }

    /// Remove all facts matching a pattern
    ///
    /// Uses lazy matching to avoid creating expanded duplicates in memory.
    /// Returns just the count of removed items (more efficient than returning Vec).
    ///
    /// # Returns
    /// Count of removed facts (total, respecting multiplicity)
    pub fn remove_matching_count(&mut self, pattern: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::remove_matching_count", ?pattern);

        // Use match_space to get compressed (value, count) pairs
        let matches = self.match_space(pattern, pattern);

        // Calculate total count (sum of all multiplicities)
        let count: usize = matches.iter().map(|m| m.count).sum();

        trace!(target: "mettatron::environment::remove_matching_count",
               unique_matches = matches.len(), total_count = count);

        // Remove each match N times based on its multiplicity
        for m in matches {
            for _ in 0..m.count {
                self.remove_from_space(&m.value);
            }
        }

        count
    }

    /// Rebuild the bloom filter by iterating through all entries in the MettaTrie.
    ///
    /// This is needed after deserializing the space, since the bloom filter
    /// is not serialized and starts empty.
    ///
    /// # Performance
    /// - O(n) where n = number of entries in space
    /// - No deserialization needed (MettaTrie stores expressions directly)
    pub fn rebuild_bloom_filter_from_space(&mut self) {
        // Clear existing bloom filter
        self.shared
            .atom_space
            .head_arity_bloom
            .write()
            .clear();

        // Iterate through all values in the MettaTrie — no deserialization needed
        let btm = self.shared.atom_space.btm.read();
        for (metta_value, _mult) in btm.iter() {
            if let Some(head) = MettaValueTrait::get_head_symbol(metta_value) {
                let arity = MettaValueTrait::get_arity(metta_value) as u8;
                self.shared
                    .atom_space
                    .head_arity_bloom
                    .write()
                    .insert(head.as_bytes(), arity);
            }
        }
        // No wide_btm — MettaTrie handles any arity natively
    }

    /// Bulk insert facts into MettaTrie
    /// Builds a temporary trie and joins with the main trie.
    /// Single lock acquisition for the join operation.
    ///
    /// Expected speedup: ~3× for batches of 100+ facts
    pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_facts_bulk", ?facts);

        if facts.is_empty() {
            return Ok(());
        }

        self.make_owned(); // CoW: ensure we own data before modifying

        // Build temporary MettaTrie from facts
        let mut fact_trie = metta_trie::MettaTrie::new();
        for fact in facts {
            let keys = decompose_literal(fact);
            super::multiplicity::trie_add_atom(&mut fact_trie, &keys, fact.clone());
        }
        trace!(
            target: "mettatron::environment::add_facts_bulk",
            facts_ctr = facts.len(), "Decomposed all facts to TrieKeys"
        );

        // Single lock acquisition → join → unlock
        {
            let mut btm = self.shared.atom_space.btm.write();
            *btm = btm.join(&fact_trie);
        }

        // Invalidate type index if any facts were type assertions
        self.shared.type_index_dirty.store(true, Ordering::Release);
        self.modified.store(true, Ordering::Release);
        Ok(())
    }

    /// Insert an atom into the MettaTrie (used during deserialization).
    /// MettaTrie handles any arity natively — no wide expression fallback needed.
    pub fn insert_wide_atom(&self, value: &MettaValue) {
        let keys = decompose_literal(value);
        let mut btm = self.shared.atom_space.btm.write();
        super::multiplicity::trie_add_atom(&mut btm, &keys, value.clone());
        self.shared
            .atom_space
            .total_atoms
            .fetch_add(1, Ordering::Relaxed);
    }
}
