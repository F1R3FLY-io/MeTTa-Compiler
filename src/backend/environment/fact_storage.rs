//! Fact storage operations for Environment.
//!
//! Provides methods for adding, removing, and querying facts in MORK Space.
//! Handles both primary MORK storage and large expression fallback.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use mork::space::Space;
use mork_expr::Expr;
use pathmap::PathMap;
use tracing::trace;

use super::{Environment, MettaValue};

impl Environment {
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
        use pathmap::zipper::*;
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

    /// UNUSED: This approach doesn't work because query_multi treats variables as pattern variables
    /// Kept for historical reference - do not use
    #[allow(dead_code)]
    fn has_sexpr_fact_optimized(&self, sexpr: &MettaValue) -> Option<bool> {
        use mork_frontend::bytestring_parser::Parser;

        // Convert MettaValue to MORK pattern for query
        let mork_str = sexpr.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        let space = self.create_space();

        // Parse to MORK Expr (following try_match_all_rules_query_multi pattern)
        let mut parse_buffer = vec![0u8; 4096];
        let mut pdp = mork::space::ParDataParser::new(&space.sm);
        let mut ez = mork_expr::ExprZipper::new(Expr {
            ptr: parse_buffer.as_mut_ptr(),
        });
        let mut context = mork_frontend::bytestring_parser::Context::new(mork_bytes);

        // If parsing fails, return None to trigger fallback
        if pdp.sexpr(&mut context, &mut ez).is_err() {
            return None;
        }

        let pattern_expr = Expr {
            ptr: parse_buffer.as_ptr().cast_mut(),
        };

        // Use query_multi for O(k) prefix-based search
        let mut found = false;
        mork::space::Space::query_multi(&space.btm, pattern_expr, |_bindings, matched_expr| {
            // Convert matched expression back to MettaValue
            if let Ok(stored_value) = Self::mork_expr_to_metta_value(&matched_expr, &space) {
                // Check structural equivalence (handles De Bruijn variable renaming)
                if sexpr.structurally_equivalent(&stored_value) {
                    found = true;
                    return false; // Stop searching, we found it
                }
            }
            true // Continue searching
        });

        Some(found)
    }

    /// Fallback linear search for has_sexpr_fact (O(n) iteration)
    fn has_sexpr_fact_linear(&self, sexpr: &MettaValue) -> bool {
        let space = self.create_space();
        use pathmap::zipper::*;
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

    /// Convert MettaValue to MORK bytes with LRU caching
    /// Checks cache first, only converts if not cached
    /// NOTE: Only caches ground (variable-free) patterns for deterministic results
    /// Variable patterns require fresh ConversionContext for correct De Bruijn encoding
    /// Expected speedup: 3-10x for repeated ground patterns
    #[allow(dead_code)]
    pub(crate) fn metta_to_mork_bytes_cached(&self, value: &MettaValue) -> Result<Vec<u8>, String> {
        use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};

        // Only cache ground (variable-free) patterns
        // Variable patterns need fresh ConversionContext for correct De Bruijn indices
        let is_ground = !Self::contains_variables(value);

        if is_ground {
            // Check cache first for ground patterns (read-only access)
            {
                let mut cache = self
                    .shared
                    .pattern_cache
                    .write()
                    .expect("pattern_cache lock poisoned");
                if let Some(bytes) = cache.get(value) {
                    trace!(target: "mettatron::environment::metta_to_mork_bytes_cached", "Cache hit");
                    return Ok(bytes.clone());
                }
            }
        }

        // Cache miss or variable pattern - perform conversion
        let space = self.create_space();
        let mut ctx = ConversionContext::new();
        let bytes = metta_to_mork_bytes(value, &space, &mut ctx)?;

        if is_ground {
            // Store ground patterns in cache for future use (write access)
            let mut cache = self
                .shared
                .pattern_cache
                .write()
                .expect("pattern_cache lock poisoned");
            cache.put(value.clone(), bytes.clone());
        }

        Ok(bytes)
    }

    /// Check if a MettaValue contains variables ($x, &y, 'z, or _)
    /// Space references like &self, &kb, &stack are NOT variables
    pub(crate) fn contains_variables(value: &MettaValue) -> bool {
        match value {
            MettaValue::Atom(s) => {
                // Space references are NOT variables
                if s == "&" || s == "&self" || s == "&kb" || s == "&stack" {
                    return false;
                }
                s == "_" || s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')
            }
            MettaValue::SExpr(items) => items.iter().any(Self::contains_variables),
            MettaValue::Error(_, details) => Self::contains_variables(details),
            MettaValue::Type(t) => Self::contains_variables(t),
            _ => false, // Ground types: Bool, Long, Float, String, Nil
        }
    }

    /// Extract concrete prefix from a pattern for efficient trie navigation
    /// Returns (prefix_items, has_variables) where prefix is longest concrete sequence
    ///
    /// Examples:
    /// - (fibonacci 10) → ([fibonacci, 10], false) - fully concrete
    /// - (fibonacci $n) → ([fibonacci], true) - concrete prefix, variable suffix
    /// - ($f 10) → ([], true) - no concrete prefix
    ///
    /// This enables O(p + k) pattern matching instead of O(n):
    /// - p = prefix length (typically 1-3 items)
    /// - k = candidates matching prefix (typically << n)
    /// - n = total entries in space
    #[allow(dead_code)]
    pub(crate) fn extract_pattern_prefix(pattern: &MettaValue) -> (Vec<MettaValue>, bool) {
        match pattern {
            MettaValue::SExpr(items) => {
                let mut prefix = Vec::new();
                let mut has_variables = false;

                for item in items {
                    if Self::contains_variables(item) {
                        has_variables = true;
                        break; // Stop at first variable
                    }
                    prefix.push(item.clone());
                }

                (prefix, has_variables)
            }
            // Non-s-expression patterns are treated as single-item prefix
            _ => {
                if Self::contains_variables(pattern) {
                    (vec![], true)
                } else {
                    (vec![pattern.clone()], false)
                }
            }
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
        use pathmap::zipper::*;
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

    /// Add a fact to the MORK Space for pattern matching
    /// Converts the MettaValue to MORK format and stores it
    /// OPTIMIZATION (Variant C): Uses direct MORK byte conversion for ground values
    ///
    /// IMPORTANT: Official MeTTa semantics - only the top-level expression is stored.
    /// Nested sub-expressions are NOT recursively extracted and stored separately.
    /// To query nested parts, use pattern matching with variables, e.g., (Outer $x)
    pub fn add_to_space(&mut self, value: &MettaValue) {
        trace!(target: "mettatron::environment::add_to_space", ?value);
        use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};
        use crate::backend::varint_encoding::metta_to_varint_key;

        // Always try direct byte conversion first (handles both ground and non-ground values)
        // This skips string serialization + parsing for 10-20× speedup
        // Also properly handles arity limits (returns error instead of panicking)
        let space = self.create_space();
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &space, &mut ctx) {
            Ok(mork_bytes) => {
                // Primary: Store in MORK PathMap (fast O(k) query_multi)
                let mut space_mut = self.create_space();
                space_mut.btm.insert(&mork_bytes, ());
                self.update_pathmap(space_mut);

                // Update bloom filter with (head, arity) for O(1) match_space() rejection
                if let Some(head) = value.get_head_symbol() {
                    let arity = value.get_arity() as u8;
                    self.shared
                        .head_arity_bloom
                        .write()
                        .expect("head_arity_bloom lock poisoned")
                        .insert(head.as_bytes(), arity);
                }
            }
            Err(_e) => {
                // Fallback: Store in PathMap with varint encoding (arity >= 64)
                // Lazy allocation: only create PathMap on first use
                let key = metta_to_varint_key(value);
                self.make_owned(); // CoW: ensure we own data before modifying
                let mut guard = self
                    .shared
                    .large_expr_pathmap
                    .write()
                    .expect("large_expr_pathmap lock poisoned");
                let fallback = guard.get_or_insert_with(PathMap::new);
                fallback.insert(&key, value.clone());

                #[cfg(debug_assertions)]
                eprintln!("Info: large expression stored in fallback PathMap: {}", _e);
            }
        }
    }

    /// Remove a fact from MORK Space by exact match
    ///
    /// This removes the specified value from the PathMap trie if it exists.
    /// The value must match exactly - no pattern matching or wildcards.
    ///
    /// # Performance
    /// - Ground values: O(m) where m = size of MORK encoding
    /// - Uses direct byte conversion for 10-20× speedup (same as add_to_space)
    ///
    /// # Thread Safety
    /// - Acquires write lock on PathMap
    /// - Marks environment as modified (CoW)
    pub fn remove_from_space(&mut self, value: &MettaValue) {
        trace!(target: "mettatron::environment::remove_from_space", ?value);
        use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};
        use crate::backend::varint_encoding::metta_to_varint_key;

        // Always try direct byte conversion (handles both ground and non-ground values)
        // Also properly handles arity limits (returns error instead of panicking)
        let space = self.create_space();
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &space, &mut ctx) {
            Ok(mork_bytes) => {
                // Remove from primary MORK PathMap
                let mut space_mut = self.create_space();
                space_mut.btm.remove(&mork_bytes);
                self.update_pathmap(space_mut);

                // Note deletion for bloom filter lazy rebuild tracking
                // (Standard bloom filters don't support deletion, so we track count
                // for periodic rebuild when false positive rate becomes too high)
                self.shared
                    .head_arity_bloom
                    .write()
                    .expect("head_arity_bloom lock poisoned")
                    .note_deletion();
            }
            Err(_) => {
                // Remove from fallback PathMap (if it exists)
                let key = metta_to_varint_key(value);
                let mut guard = self
                    .shared
                    .large_expr_pathmap
                    .write()
                    .expect("large_expr_pathmap lock poisoned");
                if let Some(ref mut fallback) = *guard {
                    fallback.remove(&key);
                }
            }
        }
    }

    /// Remove all facts matching a pattern from MORK Space
    ///
    /// This finds all facts that match the given pattern (with variables)
    /// and removes each match from the space.
    ///
    /// # Returns
    /// Vector of all removed facts (for logging/undo)
    ///
    /// # Performance
    /// - O(n × m) where n = facts in space, m = pattern complexity
    /// - Optimized by query_all() which uses PathMap prefix search
    ///
    /// # Thread Safety
    /// - Acquires multiple write locks (one per fact removed)
    /// - Consider using bulk removal for large result sets
    pub fn remove_matching(&mut self, pattern: &MettaValue) -> Vec<MettaValue> {
        trace!(target: "mettatron::environment::remove_matching", ?pattern);
        // Query for all matches using match_space with identity template
        let matches = self.match_space(pattern, pattern);

        // Remove each match
        trace!(target: "mettatron::environment::remove_matching", match_count = matches.len());
        for m in &matches {
            self.remove_from_space(m);
        }

        matches
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
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();

        // Clear existing bloom filter
        self.shared
            .head_arity_bloom
            .write()
            .expect("head_arity_bloom lock poisoned")
            .clear();

        // Iterate through all values in the trie
        while rz.to_next_val() {
            let expr = Expr {
                ptr: rz.path().as_ptr() as *mut u8,
            };

            // Convert MORK bytes to MettaValue
            if let Ok(metta_value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Extract head and arity, insert into bloom filter
                if let Some(head) = metta_value.get_head_symbol() {
                    let arity = metta_value.get_arity() as u8;
                    self.shared
                        .head_arity_bloom
                        .write()
                        .expect("head_arity_bloom lock poisoned")
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

        // OPTIMIZATION: Use direct MORK byte conversion
        use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};

        // Create shared temporary space for MORK conversion
        let temp_space = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };

        // Pre-convert all facts to MORK bytes (outside lock)
        // This works for both ground terms AND variable-containing terms
        // Variables are encoded using De Bruijn indices
        let mork_facts: Vec<Vec<u8>> = facts
            .iter()
            .map(|fact| {
                let mut ctx = ConversionContext::new();
                metta_to_mork_bytes(fact, &temp_space, &mut ctx)
                    .map_err(|e| format!("MORK conversion failed for {:?}: {}", fact, e))
            })
            .collect::<Result<Vec<_>, _>>()?;
        trace!(
            target: "mettatron::environment::add_facts_bulk",
            facts_ctr = mork_facts.len(), "Pre-convert all facts to MORK bytes"
        );

        // STRATEGY 1: Simple iterator-based PathMap construction
        // Build temporary PathMap outside the lock using individual inserts
        // This is faster than anamorphism due to avoiding excessive cloning
        let mut fact_trie = PathMap::new();

        for mork_bytes in mork_facts {
            fact_trie.insert(&mork_bytes, ());
        }

        // Single lock acquisition → union → unlock
        // This is the only critical section, minimizing lock contention
        {
            let mut btm = self.shared.btm.write().expect("btm lock poisoned");
            *btm = btm.join(&fact_trie);
        }

        // Invalidate type index if any facts were type assertions
        // Conservative: Assume any bulk insert might contain types
        *self
            .shared
            .type_index_dirty
            .write()
            .expect("type_index_dirty lock poisoned") = true;

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
    ) -> std::sync::RwLockReadGuard<'_, Option<PathMap<MettaValue>>> {
        self.shared
            .large_expr_pathmap
            .read()
            .expect("large_expr_pathmap lock poisoned")
    }

    /// Insert a value into the large expressions fallback PathMap
    /// Used during deserialization to restore large expressions (arity >= 64)
    /// that exceed MORK's 63-arity limit
    pub fn insert_large_expr(&self, value: MettaValue) {
        use crate::backend::varint_encoding::metta_to_varint_key;
        let key = metta_to_varint_key(&value);
        let mut guard = self
            .shared
            .large_expr_pathmap
            .write()
            .expect("large_expr_pathmap lock poisoned");
        let fallback = guard.get_or_insert_with(PathMap::new);
        fallback.insert(&key, value);
    }
}
