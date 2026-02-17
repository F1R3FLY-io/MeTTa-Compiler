//! Fact storage operations for Environment.
//!
//! Provides methods for adding, removing, and querying facts in MORK Space.
//! Handles both primary MORK storage and large expression fallback.

use std::sync::atomic::Ordering;

use mork_expr::Expr;
use pathmap::zipper::{ZipperIteration, ZipperMoving};
use pathmap::PathMap;
use tracing::trace;

use super::multiplicity::Multiplicity;
use super::{MettaEnvironment, MettaValue, MettaValueInner};
use crate::backend::models::metta_value_trait::MettaValueTrait;
use crate::backend::mork_convert::with_mork_bytes;
use crate::backend::varint_encoding::metta_to_varint_key;

impl MettaEnvironment {
    /// Check if an atom fact exists (queries MORK Space)
    /// OPTIMIZED: Uses O(p) exact match via descend_to_check() where p = pattern depth
    ///
    /// For atoms (always ground), this provides O(1)-like performance
    /// Expected speedup: 1,000-10,000× for large fact databases
    pub fn has_fact(&self, atom: &str) -> bool {
        trace!(target: "mettatron::environment::has_fact", atom);
        let atom_value = MettaValue::Atom(atom.to_string());

        // Atoms are always ground (no variables), so use fast path
        // This uses descend_to_check() for O(p) trie traversal
        let mork_str = atom_value.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // O(p) exact match navigation through the trie (typically p=1 for atoms)
        // descend_to_check() walks the PathMap trie by following the exact byte sequence
        rz.descend_to_check(mork_bytes)
    }

    /// Check if an s-expression fact exists in the PathMap
    /// Checks directly in the Space using MORK binary format
    /// Uses structural equivalence to handle variable name changes from MORK's De Bruijn indices
    ///
    /// OPTIMIZED: Uses O(p) exact match via descend_to_check() for ground expressions
    /// Falls back to O(n) linear search for patterns with variables
    ///
    /// NOTE: query_multi() cannot be used here because it treats variables in the search pattern
    /// as pattern variables (to be bound), not as atoms to match. This causes false negatives.
    /// For example, searching for `(= (test-rule $x) (processed $x))` with query_multi treats
    /// $x as a pattern variable, which doesn't match the stored rule where $x was normalized to $a.
    pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
        trace!(target: "mettatron::environment::has_sexpr_fact", ?sexpr);
        // Fast path: O(p) exact match for ground (variable-free) expressions
        // This provides 1,000-10,000× speedup for large fact databases
        if !Self::contains_variables(sexpr) {
            // Use descend_to_exact_match for O(p) lookup
            if let Some(matched) = self.descend_to_exact_match(sexpr) {
                // Found exact match - verify structural equivalence
                // (handles any encoding differences)
                return sexpr.structurally_equivalent(&matched);
            }
            // Fast path failed - fall back to linear search
            // This handles cases where MORK encoding differs (e.g., after Par round-trip)
            trace!(target: "mettatron::environment::has_sexpr_fact", "Fast path failed, using linear search");
            return self.has_sexpr_fact_linear(sexpr);
        }

        // Slow path: O(n) linear search for patterns with variables
        // This is necessary because variables need structural equivalence checking
        trace!(target: "mettatron::environment::has_sexpr_fact", "Using linear search (contains variables)");
        self.has_sexpr_fact_linear(sexpr)
    }

    /// Fallback linear search for has_sexpr_fact (O(n) iteration)
    fn has_sexpr_fact_linear(&self, sexpr: &MettaValue) -> bool {
        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // Directly iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // Use mork_expr_to_metta_value() to avoid "reserved byte" panic
            if let Ok(stored_value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Check structural equivalence (ignores variable names)
                if sexpr.structurally_equivalent(&stored_value) {
                    return true;
                }
            }
        }

        false
    }

    /// Check if a MettaValue contains variables ($x, &y, 'z, or _)
    /// Space references like &self, &kb, &stack are NOT variables
    pub(crate) fn contains_variables(value: &MettaValue) -> bool {
        match value.inner() {
            MettaValueInner::Atom(s) => {
                // Space references are NOT variables
                if *s == "&" || *s == "&self" || *s == "&kb" || *s == "&stack" {
                    return false;
                }
                *s == "_" || s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')
            }
            MettaValueInner::SExpr(items) => (*items).iter().any(Self::contains_variables),
            MettaValueInner::Error(_, details) => Self::contains_variables(details),
            MettaValueInner::Type(t) => Self::contains_variables(t),
            _ => false, // Ground types: Bool, Long, Float, String, Unit
        }
    }

    /// Try exact match lookup using ReadZipper::descend_to_check()
    /// Returns Some(value) if exact match found, None otherwise
    ///
    /// This provides O(p) lookup time where p = pattern depth (typically 3-5)
    /// compared to O(n) for linear iteration where n = total facts in space
    ///
    /// Expected speedup: 1,000-10,000× for large datasets (n=10,000)
    ///
    /// Only works for ground (variable-free) patterns. Patterns with variables
    /// must use query_multi() or linear search.
    pub(crate) fn descend_to_exact_match(&self, pattern: &MettaValue) -> Option<MettaValue> {
        // Only works for ground patterns (no variables)
        if Self::contains_variables(pattern) {
            return None;
        }

        // CRITICAL: Must use the same encoding as add_to_space() for consistency
        // add_to_space() uses to_mork_string().as_bytes(), so we must do the same
        let mork_str = pattern.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // O(p) exact match navigation through the trie
        // descend_to_check() walks the PathMap trie by following the exact byte sequence
        if rz.descend_to_check(mork_bytes) {
            // Found! Extract the value at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };
            return Self::mork_expr_to_metta_value(&expr, &space).ok();
        }

        // No exact match found
        None
    }

    // Note: add_to_space() and remove_from_space() are now in generic.rs as generic methods
    // on GenericEnvironment<V, F>. They use MORK PathMap for storage.

    /// Remove all facts matching a pattern from MORK Space
    ///
    /// Uses lazy matching to avoid creating expanded duplicates in memory.
    /// Returns just the count of removed items (more efficient than returning Vec).
    ///
    /// # Returns
    /// Count of removed facts (total, respecting multiplicity)
    ///
    /// # Performance
    /// - Memory: O(k) where k = unique matching atoms (vs O(n×m) for expanded)
    /// - Calls remove_from_space() once per instance, respecting multiplicity
    ///
    /// # Example
    /// ```ignore
    /// // If (fact 1) exists with multiplicity 3, remove_matching_count returns 3
    /// // but only allocates space for 1 MultiplicityMatch struct
    /// let count = env.remove_matching_count(&pattern);
    /// ```
    pub fn remove_matching_count(&mut self, pattern: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::remove_matching_count", ?pattern);

        // Use match_space to get compressed (value, count) pairs
        // This avoids expanding multiplicity N into N copies in memory
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

    /// Rebuild the bloom filter by iterating through all entries in MORK space.
    ///
    /// This is needed after deserializing the space from PathMap Par format,
    /// since the bloom filter is not serialized and starts empty.
    ///
    /// # Performance
    /// - O(n) where n = number of entries in space
    /// - Converts each MORK path to MettaValue to extract head/arity
    ///
    /// # Thread Safety
    /// - Acquires write lock on bloom filter
    /// - Acquires read lock on PathMap
    pub fn rebuild_bloom_filter_from_space(&mut self) {
        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // Clear existing bloom filter
        self.shared
            .atom_space.head_arity_bloom
            .write()
                        .clear();

        // Iterate through all values in the trie
        while rz.to_next_val() {
            let expr = Expr {
                ptr: rz.path().as_ptr() as *mut u8,
            };

            // Convert MORK bytes to MettaValue
            if let Ok(metta_value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Extract head and arity, insert into bloom filter
                if let Some(head) = MettaValueTrait::get_head_symbol(&metta_value) {
                    let arity = MettaValueTrait::get_arity(&metta_value) as u8;
                    self.shared
                        .atom_space.head_arity_bloom
                        .write()
                                                .insert(head.as_bytes(), arity);
                }
            }
        }
    }

    /// Bulk insert facts into MORK Space using PathMap anamorphism (Strategy 2)
    /// This is significantly faster than individual add_to_space() calls
    /// for large batches (3× speedup) due to:
    /// - Single lock acquisition instead of N locks
    /// - Trie-aware construction (groups by common prefixes)
    /// - Bulk PathMap union operation instead of N individual inserts
    /// - Eliminates redundant trie traversals
    ///
    /// Expected speedup: ~3× for batches of 100+ facts (Strategy 2)
    /// Complexity: O(m) where m = size of fact batch (vs O(n × lock) for individual inserts)
    pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_facts_bulk", ?facts);

        if facts.is_empty() {
            return Ok(());
        }

        self.make_owned(); // CoW: ensure we own data before modifying

        // OPTIMIZATION: Build temporary PathMap directly from callback (zero-copy per fact)
        let sm = &self.shared_mapping;
        let mut fact_trie: PathMap<Multiplicity> = PathMap::new();

        for fact in facts {
            with_mork_bytes(fact, sm, |mork_bytes| {
                fact_trie.insert(mork_bytes, Multiplicity::new(1));
            })
            .map_err(|e| format!("MORK conversion failed for {:?}: {}", fact, e))?;
        }
        trace!(
            target: "mettatron::environment::add_facts_bulk",
            facts_ctr = facts.len(), "Converted all facts to MORK bytes"
        );

        // Single lock acquisition → union → unlock
        // This is the only critical section, minimizing lock contention
        {
            let mut btm = self.shared.atom_space.btm.write();
            *btm = btm.join(&fact_trie);
        }

        // Invalidate type index if any facts were type assertions
        // Conservative: Assume any bulk insert might contain types
        // AtomicBool - store directly
        self.shared.type_index_dirty.store(true, Ordering::Release);

        self.modified.store(true, Ordering::Release); // CoW: mark as modified
        Ok(())
    }

    /// Get read access to the large expression fallback PathMap
    ///
    /// Returns the fallback PathMap that stores expressions with arity >= 64
    /// (which exceed MORK's 63-arity limit). Uses varint encoding for keys.
    /// Returns None if no large expressions have been stored.
    pub fn get_large_expr_pathmap(
        &self,
    ) -> parking_lot::RwLockReadGuard<'_, Option<PathMap<MettaValue>>> {
        // parking_lot::RwLock - no .expect()
        self.shared.atom_space.large_expr_pathmap.read()
    }

    /// Insert a value into the large expressions fallback PathMap
    /// Used during deserialization to restore large expressions (arity >= 64)
    /// that exceed MORK's 63-arity limit
    pub fn insert_large_expr(&self, value: MettaValue) {
        let key = metta_to_varint_key(&value);
        // parking_lot::RwLock - no .expect()
        let mut guard = self.shared.atom_space.large_expr_pathmap.write();
        let fallback = guard.get_or_insert_with(PathMap::new);
        fallback.insert(&key, value);
    }
}
