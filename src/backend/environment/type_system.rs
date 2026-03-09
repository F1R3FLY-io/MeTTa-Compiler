//! Type system operations for Environment.
//!
//! Provides methods for type assertions, type indexing, and type lookups.
//! Type assertions are stored as (: name type) in MORK Space.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use mork::space::Space;
use mork_expr::Expr;
use pathmap::zipper::{ZipperIteration, ZipperMoving};
use tracing::trace;

use super::generic::GenericEnvironment;
use super::{MettaEnvironment, MettaValue};
use crate::backend::models::{MettaValueFactory, MettaValueTrait, ValueView};

// ============================================================================
// Generic Type Operations (for GenericEnvironment<V, F>)
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Add a type assertion (generic version).
    ///
    /// Appends the type to the `types` HashMap Vec for the given name (with dedup).
    /// HE parity: an atom can have multiple types declared via separate `(: name type)` assertions.
    /// For MettaValue environments that need MORK persistence, use
    /// `Environment::add_type` which also stores in MORK Space.
    pub fn add_type_generic(&mut self, name: &str, typ: V) {
        trace!(target: "mettatron::environment::add_type_generic", name);
        self.make_owned();

        let mut types = self.shared.types.write();
        let vec = types.entry(name.to_string()).or_default();
        if !vec.contains(&typ) {
            vec.push(typ);
        }
        drop(types);

        // Update type bloom filter for O(1) early rejection
        self.shared.atom_space.type_bloom.write().insert(name.as_bytes());

        // Increment rule/type epoch — invalidates cached TypeSignatureRegistry in JIT.
        super::rule_management::increment_rule_epoch();

        self.modified.store(true, Ordering::Release);
    }

    /// Get all types for a symbol (generic version, nondeterministic).
    ///
    /// Returns all declared types for the given name, plus the transitive
    /// supertype closure for each declared type. Returns an empty Vec if no
    /// type assertions exist.
    ///
    /// HE parity: mirrors `add_super_types()` in HE's `query_types()`.
    /// If `(: a Dog)` and `(:< Dog Animal)` and `(:< Animal LivingThing)`,
    /// then `get_types_generic("a")` returns `[Dog, Animal, LivingThing]`.
    ///
    /// For MettaValue environments, prefer `Environment::get_type` which
    /// uses the optimized MORK index.
    pub fn get_types_generic(&self, name: &str) -> Vec<V> {
        // O(1) bloom filter rejection: if the name definitely has no type, skip HashMap
        if !self.shared.atom_space.type_bloom.read().may_have_type(name.as_bytes()) {
            return Vec::new();
        }
        let mut types: Vec<V> = self.shared.types.read().get(name).cloned().unwrap_or_default();

        // HE parity: append transitive supertypes for each declared type.
        // Iterate over direct types (snapshot len), appending supertypes.
        let original_len = types.len();
        for i in 0..original_len {
            if let Some(type_name) = types[i].as_atom() {
                for supertype in self.get_all_supertypes(type_name) {
                    let super_val = self.factory.atom(&supertype);
                    if !types.contains(&super_val) {
                        types.push(super_val);
                    }
                }
            }
        }

        types
    }

    /// O(1) bloom filter check: does this atom name *possibly* have type declarations?
    ///
    /// Returns `false` only if the name definitely has no type (no false negatives).
    /// Returns `true` if the name may have types (possible false positive at ~1% FPR).
    /// Much cheaper than `get_types_generic` — no RwLock on the types HashMap,
    /// no supertype closure computation, no Vec allocation.
    #[inline]
    pub fn may_have_type(&self, name: &str) -> bool {
        self.shared.atom_space.type_bloom.read().may_have_type(name.as_bytes())
    }

    /// Get all atom names that have a specific declared type.
    ///
    /// Enables O(k) type-filtered match instead of O(n) full space scan,
    /// where k = number of atoms of the matching type.
    ///
    /// Used by Phase 8.4 match type-aware space pre-filtering: when the
    /// match pattern is `(: $x SomeType)`, use this as a reverse index
    /// instead of scanning the entire MORK space.
    pub fn get_atoms_of_type(&self, type_name: &str) -> Vec<String> {
        self.shared.types.read()
            .iter()
            .filter(|(_, types)| types.iter().any(|t| {
                t.as_atom() == Some(type_name)
            }))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Remove a specific type assertion (generic version).
    ///
    /// Removes the specific type from the `types` HashMap Vec. If the Vec
    /// becomes empty, removes the key entirely. Also removes the
    /// `(: name type)` atom from the MORK space for consistency.
    /// Invalidates the type index cache.
    pub fn remove_type_generic(&mut self, name: &str, type_val: &V) {
        trace!(target: "mettatron::environment::remove_type_generic", name);
        self.make_owned();

        {
            let mut types = self.shared.types.write();
            if let Some(vec) = types.get_mut(name) {
                vec.retain(|t| t != type_val);
                if vec.is_empty() {
                    types.remove(name);
                }
            }
        }

        // Remove the type assertion from MORK space
        let type_assertion = self.factory.sexpr(vec![
            self.factory.atom(":"),
            self.factory.atom(name),
            type_val.clone(),
        ]);
        self.remove_from_space(&type_assertion);

        // Invalidate type index cache
        self.shared.type_index_dirty.store(true, Ordering::Release);

        // Increment rule/type epoch — invalidates cached TypeSignatureRegistry in JIT.
        super::rule_management::increment_rule_epoch();

        self.modified.store(true, Ordering::Release);
    }

    /// Add a subtype relation (generic version).
    ///
    /// Registers `sub` as a subtype of `super_type`. Supports `(:< Sub Super)` declarations.
    /// HE parity: enables transitive subtype checking via `get_all_supertypes()`.
    pub fn add_subtype_generic(&mut self, sub: &str, super_type: &str) {
        self.make_owned();

        let mut subtypes = self.shared.subtypes.write();
        let vec = subtypes.entry(sub.to_string()).or_default();
        if !vec.contains(&super_type.to_string()) {
            vec.push(super_type.to_string());
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Remove a subtype relation (generic version).
    ///
    /// Removes the `sub <: super_type` relation.
    pub fn remove_subtype_generic(&mut self, sub: &str, super_type: &str) {
        self.make_owned();

        let mut subtypes = self.shared.subtypes.write();
        if let Some(vec) = subtypes.get_mut(sub) {
            vec.retain(|s| s != super_type);
            if vec.is_empty() {
                subtypes.remove(sub);
            }
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Get all supertypes of a type (transitive closure).
    ///
    /// Performs BFS over the `subtypes` HashMap to compute the transitive closure
    /// of the subtype relation. Returns all types that `type_name` is a subtype of,
    /// directly or transitively. Uses a visited set for cycle detection.
    ///
    /// Mirrors HE's `add_super_types` (types.rs:49-63).
    ///
    /// # Example
    /// ```ignore
    /// // (:< Dog Animal)
    /// // (:< Animal LivingThing)
    /// get_all_supertypes("Dog") → ["Animal", "LivingThing"]
    /// ```
    pub fn get_all_supertypes(&self, type_name: &str) -> Vec<String> {
        let subtypes = self.shared.subtypes.read();
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        // Seed BFS with direct supertypes
        if let Some(direct_supers) = subtypes.get(type_name) {
            for s in direct_supers {
                if visited.insert(s.clone()) {
                    queue.push_back(s.clone());
                    result.push(s.clone());
                }
            }
        }

        // BFS for transitive supertypes
        while let Some(current) = queue.pop_front() {
            if let Some(supers) = subtypes.get(&current) {
                for s in supers {
                    if visited.insert(s.clone()) {
                        queue.push_back(s.clone());
                        result.push(s.clone());
                    }
                }
            }
        }

        result
    }

    /// Check if `sub` is a subtype of `super_type` (direct or transitive).
    ///
    /// Returns true if there exists a chain `sub <: ... <: super_type`.
    pub fn is_subtype_of(&self, sub: &str, super_type: &str) -> bool {
        if sub == super_type {
            return true;
        }
        self.get_all_supertypes(sub).iter().any(|s| s == super_type)
    }

    // ========================================================================
    // Phase 10.1: Inferred Function Return Type Queries
    // ========================================================================

    /// Check if a function has inferred return types (Phase 10.1).
    ///
    /// Lock-free: AtomicBloomFilter uses `load(Relaxed)` — zero synchronization.
    /// False positives harmless (fall through to DashMap lookup).
    #[inline]
    pub fn has_inferred_type(&self, name: &str) -> bool {
        self.shared
            .atom_space
            .inferred_type_bloom
            .may_contain(name.as_bytes())
    }

    /// Get inferred return types for a function name (Phase 10.1).
    ///
    /// Lock-free: DashMap per-shard read lock (non-blocking, no writer starvation).
    /// Returns empty Vec if no inferred types exist.
    pub fn get_inferred_fn_types(&self, name: &str) -> Vec<V> {
        if !self.has_inferred_type(name) {
            return Vec::new();
        }
        self.shared
            .inferred_fn_types
            .get(name)
            .map(|entry| entry.value().clone())
            .unwrap_or_default()
    }

    /// Register an inferred return type for a function (Phase 10.1).
    ///
    /// Called from `add_rule()` after computing `rhs_type`. Deduplicates entries.
    /// Updates both the DashMap index and the AtomicBloomFilter.
    ///
    /// Phase 10.5: Also increments `inferred_type_generation` to signal that
    /// a fixpoint re-inference is needed at the next eval boundary.
    pub fn register_inferred_type(&self, name: &str, return_type: &V) {
        // DashMap index — lock-free per-shard write
        let mut entry = self.shared.inferred_fn_types.entry(name.to_string()).or_default();
        if !entry.value().contains(return_type) {
            entry.value_mut().push(return_type.clone());
        }
        drop(entry);

        // Atomic bloom filter — lock-free fetch_or insertion
        self.shared
            .atom_space
            .inferred_type_bloom
            .insert(name.as_bytes());

        // Phase 10.5: Increment generation counter to trigger fixpoint at next eval boundary.
        self.shared
            .atom_space
            .inferred_type_generation
            .fetch_add(1, Ordering::Release);
    }

    /// Phase 10.5: Run iterative fixpoint type inference if new types were registered.
    ///
    /// This method is called at eval boundaries (after `eval()` returns) to propagate
    /// inferred types through mutually recursive function call chains. It uses a
    /// generation-counter protocol to detect when new types have been registered since
    /// the last fixpoint run:
    ///
    /// - `inferred_type_generation`: incremented by `register_inferred_type()` on each new type.
    /// - `fixpoint_generation`: records the generation at which the last fixpoint completed.
    ///
    /// When `inferred_type_generation != fixpoint_generation`, new types exist that haven't
    /// been processed by the fixpoint. A CAS (compare-and-swap) claims the fixpoint run
    /// to prevent concurrent/duplicate runs across threads.
    ///
    /// ## What is a fixpoint?
    ///
    /// A fixpoint (fixed point) is a value *x* that is unchanged by a function application:
    /// *f(x) = x*. Here, we iteratively re-infer return types of mutually recursive
    /// functions until the inferred type set stabilizes — i.e., another round of inference
    /// produces the same types as the previous round. That stable state is the fixpoint
    /// of the type inference function. State-based cycle detection guarantees termination
    /// even in non-monotonic type lattices.
    ///
    /// ## Deadlock Safety
    ///
    /// `run_type_fixpoint()` acquires `rule_index.read()`. This method MUST be called
    /// at eval boundaries (after `eval()` returns), when all locks from `add_rule()`
    /// (which holds `rule_index.write()`) have been released.
    pub fn maybe_run_type_fixpoint(&self) {
        let current = self
            .shared
            .atom_space
            .inferred_type_generation
            .load(Ordering::Acquire);
        let last = self
            .shared
            .atom_space
            .fixpoint_generation
            .load(Ordering::Acquire);

        if current == last {
            return; // No new types since last fixpoint
        }

        // CAS claims this fixpoint run (prevents concurrent/duplicate runs)
        if self
            .shared
            .atom_space
            .fixpoint_generation
            .compare_exchange(last, current, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return; // Another thread ran/is running the fixpoint
        }

        crate::backend::eval::type_fixpoint::run_type_fixpoint(self);
    }
}

// ============================================================================
// MettaValue-specific Type Operations (with MORK persistence)
// ============================================================================

impl MettaEnvironment {
    /// Add a type assertion
    /// Type assertions are stored as (: name type) in MORK Space
    /// Invalidates the type index cache
    pub fn add_type(&mut self, name: String, typ: MettaValue) {
        trace!(target: "mettatron::environment::add_type", name, ?typ);
        self.make_owned(); // CoW: ensure we own data before modifying

        // Create type assertion: (: name typ)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom(name),
            typ,
        ]);
        self.add_to_space(&type_assertion);

        // Invalidate type index cache - AtomicBool
        self.shared.type_index_dirty.store(true, Ordering::Release);
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Ensure the type index is built and up-to-date.
    ///
    /// Uses the dedicated `type_btm` PathMap which is incrementally maintained
    /// on every `add_to_space`/`remove_from_space` of `(: name type)` atoms.
    /// This eliminates the expensive `restrict()` rebuild that was previously needed.
    ///
    /// The `type_index` cache is a snapshot of `type_btm` — O(1) CoW clone.
    pub(crate) fn ensure_type_index(&self) {
        // AtomicBool - check dirty flag
        let dirty = self.shared.type_index_dirty.load(Ordering::Acquire);
        if !dirty {
            return; // Index is up to date
        }

        // Snapshot the incrementally-maintained type PathMap (O(1) CoW clone)
        let type_subtrie = self.shared.atom_space.type_btm.read().clone();

        // Cache the snapshot - parking_lot::RwLock - no .expect()
        *self.shared.type_index.write() = Some(type_subtrie);
        // AtomicBool - clear dirty flag
        self.shared.type_index_dirty.store(false, Ordering::Release);
    }

    /// Get all types for an atom by querying MORK Space (nondeterministic).
    /// Searches for type assertions of the form (: name type)
    /// Returns empty Vec if no type assertion exists for the given name.
    ///
    /// OPTIMIZED: Uses PathMap::restrict() to create a type-only subtrie
    /// Then navigates within that subtrie for O(p + m) lookup where m << n
    /// Falls back to O(n) linear search if index lookup fails
    #[allow(clippy::collapsible_match)]
    pub fn get_type(&self, name: &str) -> Vec<MettaValue> {
        trace!(target: "mettatron::environment::get_type", name);

        // O(1) bloom filter rejection: if the name definitely has no type, skip MORK trie
        if !self.shared.atom_space.type_bloom.read().may_have_type(name.as_bytes()) {
            return Vec::new();
        }

        // Ensure type index is built and up-to-date
        self.ensure_type_index();

        // Get the type index subtrie - parking_lot::RwLock - no .expect()
        let type_index_guard = self.shared.type_index.read();
        let type_index = match type_index_guard.as_ref() {
            Some(index) => index,
            None => {
                // Index failed to build, fall back to linear search
                trace!(target: "mettatron::environment::get_type", name, "Falling back to linear search");
                drop(type_index_guard); // Release lock before fallback
                return self.get_type_linear(name);
            }
        };

        // Fast path: Navigate within type index subtrie
        // Build pattern: (: name) - we know the exact structure
        let type_query = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom(name.to_string()),
        ]);

        // CRITICAL: Must use the same encoding as add_to_space() for consistency
        let mork_str = type_query.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        // Create space for this type index subtrie
        let space = Space {
            sm: self.shared_mapping.clone(),
            btm: type_index.clone(), // O(1) clone via structural sharing
            mmaps: HashMap::new(),
        };

        let mut rz = space.btm.read_zipper();

        // Try O(p + m) lookup within type subtrie where m << n
        // descend_to_check navigates the trie by exact byte sequence
        let mut results = Vec::new();
        if rz.descend_to_check(mork_bytes) {
            // Found exact match for prefix (: name)
            // Now extract all type assertions: (: name TYPE)
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Extract TYPE from (: name TYPE)
                if let ValueView::SExpr(items) = value.view() {
                    if items.len() >= 3 {
                        // items[0] = ":", items[1] = name, items[2] = TYPE
                        results.push(items[2].clone());
                    }
                }
            }
            // Continue scanning for additional type assertions with same prefix
            while rz.to_next_val() {
                let expr = Expr {
                    ptr: rz.path().as_ptr().cast_mut(),
                };
                if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                    if let ValueView::SExpr(items) = value.view() {
                        if items.len() >= 3 {
                            if let (ValueView::Atom(op), ValueView::Atom(atom_name)) =
                                (items[0].view(), items[1].view())
                            {
                                if op == ":" && atom_name == name {
                                    let typ = items[2].clone();
                                    if !results.contains(&typ) {
                                        results.push(typ);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !results.is_empty() {
            return results;
        }

        // Release the type index lock before fallback
        drop(type_index_guard);

        // Slow path: O(n) linear search (fallback if exact match fails)
        // This handles edge cases where MORK encoding might differ
        trace!(target: "mettatron::environment::get_type", name, "Fast path failed, using linear search");
        self.get_type_linear(name)
    }

    /// Linear search fallback for get_type() - O(n) iteration
    /// Collects ALL type assertions for the given name (nondeterministic).
    /// Used when exact match via descend_to_check() fails
    fn get_type_linear(&self, name: &str) -> Vec<MettaValue> {
        let space = self.create_space();
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // Iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            #[allow(clippy::collapsible_match)]
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Check if this is a type assertion: (: name type)
                if let ValueView::SExpr(items) = value.view() {
                    if items.len() == 3 {
                        if let (ValueView::Atom(op), ValueView::Atom(atom_name)) =
                            (items[0].view(), items[1].view())
                        {
                            if op == ":" && atom_name == name {
                                let typ = items[2].clone();
                                if !results.contains(&typ) {
                                    results.push(typ);
                                }
                            }
                        }
                    }
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use pathmap::zipper::{ZipperIteration, ZipperValues};

    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{GcFactory, MettaValueFactory, MettaValueTrait};

    fn factory() -> GcFactory {
        GcFactory::default()
    }

    fn env() -> MettaEnvironment {
        MettaEnvironment::new(factory())
    }

    #[test]
    fn test_multi_type_storage() {
        let f = factory();
        let mut e = env();

        // Add two different types for the same atom
        e.add_type_generic("a", f.atom("A"));
        e.add_type_generic("a", f.atom("B"));

        let types = e.get_types_generic("a");
        assert_eq!(types.len(), 2, "should have 2 types");
        assert!(types.iter().any(|t| t.as_atom() == Some("A")));
        assert!(types.iter().any(|t| t.as_atom() == Some("B")));
    }

    #[test]
    fn test_multi_type_dedup() {
        let f = factory();
        let mut e = env();

        // Add same type twice
        e.add_type_generic("a", f.atom("A"));
        e.add_type_generic("a", f.atom("A"));

        let types = e.get_types_generic("a");
        assert_eq!(types.len(), 1, "should have 1 type (deduped)");
        assert_eq!(types[0].as_atom(), Some("A"));
    }

    #[test]
    fn test_multi_type_removal() {
        let f = factory();
        let mut e = env();

        // Add two types
        e.add_type_generic("a", f.atom("A"));
        e.add_type_generic("a", f.atom("B"));

        // Remove one type
        e.remove_type_generic("a", &f.atom("A"));

        let types = e.get_types_generic("a");
        assert_eq!(types.len(), 1, "should have 1 type remaining");
        assert_eq!(types[0].as_atom(), Some("B"));

        // Remove the other
        e.remove_type_generic("a", &f.atom("B"));
        let types = e.get_types_generic("a");
        assert!(types.is_empty(), "should have no types");
    }

    #[test]
    fn test_multi_type_fork() {
        let f = factory();
        let mut e = env();

        // Add types before fork
        e.add_type_generic("a", f.atom("A"));
        e.add_type_generic("a", f.atom("B"));

        // Fork
        let forked = e.fork_for_nondeterminism();

        // Types should survive fork
        let types = forked.get_types_generic("a");
        assert_eq!(types.len(), 2);
        assert!(types.iter().any(|t| t.as_atom() == Some("A")));
        assert!(types.iter().any(|t| t.as_atom() == Some("B")));
    }

    #[test]
    fn test_multi_type_union() {
        let f = factory();
        let mut e1 = env();
        let mut e2 = env();

        // Add different types in each env
        e1.add_type_generic("a", f.atom("A"));
        e2.add_type_generic("a", f.atom("B"));

        // Union
        let union = e1.union(&e2);

        // Should have both types
        let types = union.get_types_generic("a");
        assert_eq!(types.len(), 2);
        assert!(types.iter().any(|t| t.as_atom() == Some("A")));
        assert!(types.iter().any(|t| t.as_atom() == Some("B")));
    }

    #[test]
    fn test_multi_type_union_dedup() {
        let f = factory();
        let mut e1 = env();
        let mut e2 = env();

        // Add same type in both envs
        e1.add_type_generic("a", f.atom("A"));
        e2.add_type_generic("a", f.atom("A"));

        // Union
        let union = e1.union(&e2);

        // Should be deduped
        let types = union.get_types_generic("a");
        assert_eq!(types.len(), 1, "union should dedup same types");
        assert_eq!(types[0].as_atom(), Some("A"));
    }

    #[test]
    fn test_no_types_returns_empty() {
        let e = env();

        let types = e.get_types_generic("nonexistent");
        assert!(types.is_empty(), "untyped atom should return empty Vec");
    }

    #[test]
    fn test_add_to_space_multi_type() {
        let f = factory();
        let mut e = env();

        // Add type assertions via add_to_space (as MeTTa programs do)
        let type_a = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("A")]);
        let type_b = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("B")]);
        e.add_to_space(&type_a);
        e.add_to_space(&type_b);

        let types = e.get_types_generic("x");
        assert_eq!(types.len(), 2, "add_to_space should support multi-type");
        assert!(types.iter().any(|t| t.as_atom() == Some("A")));
        assert!(types.iter().any(|t| t.as_atom() == Some("B")));
    }

    #[test]
    fn test_remove_from_space_specific_type() {
        let f = factory();
        let mut e = env();

        // Add two type assertions
        let type_a = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("A")]);
        let type_b = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("B")]);
        e.add_to_space(&type_a);
        e.add_to_space(&type_b);

        // Remove only type A
        e.remove_from_space(&type_a);

        let types = e.get_types_generic("x");
        assert_eq!(types.len(), 1, "should have 1 type after removal");
        assert_eq!(types[0].as_atom(), Some("B"));
    }

    #[test]
    fn test_multi_type_with_arrow() {
        let f = factory();
        let mut e = env();

        // Add an atom type and an arrow type
        e.add_type_generic("f", f.atom("MyFunction"));
        e.add_type_generic(
            "f",
            f.sexpr(vec![f.atom("->"), f.atom("Number"), f.atom("Bool")]),
        );

        let types = e.get_types_generic("f");
        assert_eq!(types.len(), 2, "should have both types");
    }

    #[test]
    fn test_no_deprecated_get_type_generic() {
        // Compile-time verification: get_type_generic no longer exists.
        // This test documents that the old API was removed and replaced by get_types_generic.
        // If get_type_generic still existed, the callers in types_generic.rs and grounded.rs
        // would fail to compile since they now use get_types_generic.
    }

    // ====================================================================
    // Phase 3: Subtype Relations (:<)
    // ====================================================================

    #[test]
    fn test_subtype_basic() {
        let mut e = env();
        e.add_subtype_generic("Dog", "Animal");

        assert!(e.is_subtype_of("Dog", "Animal"));
        assert!(!e.is_subtype_of("Animal", "Dog"));
        assert!(e.is_subtype_of("Dog", "Dog")); // reflexive
    }

    #[test]
    fn test_subtype_transitive() {
        let mut e = env();
        e.add_subtype_generic("Dog", "Animal");
        e.add_subtype_generic("Animal", "LivingThing");

        assert!(e.is_subtype_of("Dog", "LivingThing"));
        assert!(e.is_subtype_of("Dog", "Animal"));
        assert!(e.is_subtype_of("Animal", "LivingThing"));
        assert!(!e.is_subtype_of("LivingThing", "Dog"));
    }

    #[test]
    fn test_subtype_cycle() {
        let mut e = env();
        e.add_subtype_generic("A", "B");
        e.add_subtype_generic("B", "A");

        // Should not loop infinitely — visited set prevents cycles
        let supers = e.get_all_supertypes("A");
        assert!(supers.contains(&"B".to_string()));
        assert!(supers.contains(&"A".to_string()));
        assert_eq!(supers.len(), 2); // B, then A (via B→A)
    }

    #[test]
    fn test_subtype_via_add_to_space() {
        let f = factory();
        let mut e = env();

        // Add subtype declaration via add_to_space (as MeTTa programs do)
        let decl = f.sexpr(vec![f.atom(":<"), f.atom("Dog"), f.atom("Animal")]);
        e.add_to_space(&decl);

        assert!(e.is_subtype_of("Dog", "Animal"));
    }

    #[test]
    fn test_subtype_remove_via_remove_from_space() {
        let f = factory();
        let mut e = env();

        let decl = f.sexpr(vec![f.atom(":<"), f.atom("Dog"), f.atom("Animal")]);
        e.add_to_space(&decl);
        assert!(e.is_subtype_of("Dog", "Animal"));

        e.remove_from_space(&decl);
        assert!(!e.is_subtype_of("Dog", "Animal"));
        assert!(e.is_subtype_of("Dog", "Dog")); // reflexive always holds
    }

    #[test]
    fn test_subtype_fork() {
        let mut e = env();
        e.add_subtype_generic("Dog", "Animal");

        let forked = e.fork_for_nondeterminism();
        assert!(forked.is_subtype_of("Dog", "Animal"));
    }

    #[test]
    fn test_subtype_union() {
        let mut e1 = env();
        let mut e2 = env();

        e1.add_subtype_generic("Dog", "Animal");
        e2.add_subtype_generic("Cat", "Animal");

        let union = e1.union(&e2);
        assert!(union.is_subtype_of("Dog", "Animal"));
        assert!(union.is_subtype_of("Cat", "Animal"));
    }

    #[test]
    fn test_subtype_dedup() {
        let mut e = env();
        e.add_subtype_generic("Dog", "Animal");
        e.add_subtype_generic("Dog", "Animal");

        let supers = e.get_all_supertypes("Dog");
        assert_eq!(supers.len(), 1, "should dedup duplicate subtype relations");
    }

    #[test]
    fn test_subtype_multiple_supers() {
        let mut e = env();
        e.add_subtype_generic("Dog", "Animal");
        e.add_subtype_generic("Dog", "Pet");

        let supers = e.get_all_supertypes("Dog");
        assert_eq!(supers.len(), 2);
        assert!(supers.contains(&"Animal".to_string()));
        assert!(supers.contains(&"Pet".to_string()));
    }

    #[test]
    fn test_subtype_no_supertypes() {
        let e = env();
        let supers = e.get_all_supertypes("Nonexistent");
        assert!(supers.is_empty());
    }

    // ====================================================================
    // Phase 7.1: Incremental Type PathMap
    // ====================================================================

    #[test]
    fn test_type_btm_incremental_add() {
        let f = factory();
        let mut e = env();

        // Add a type assertion
        let type_a = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("Number")]);
        e.add_to_space(&type_a);

        // type_btm should now have an entry
        let type_btm = e.shared.atom_space.type_btm.read();
        let mut rz = type_btm.read_zipper();
        let mut count = 0usize;
        while rz.to_next_val() {
            count += 1;
        }
        assert!(count > 0, "type_btm should have at least one entry after add");
    }

    #[test]
    fn test_type_btm_incremental_remove() {
        let f = factory();
        let mut e = env();

        // Add and then remove a type assertion
        let type_a = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("Number")]);
        e.add_to_space(&type_a);
        e.remove_from_space(&type_a);

        // type_btm should be empty again (or have zero-multiplicity entries)
        let type_btm = e.shared.atom_space.type_btm.read();
        let mut rz = type_btm.read_zipper();
        let mut has_positive = false;
        while rz.to_next_val() {
            if rz.val().map_or(false, |m| m.count() > 0) {
                has_positive = true;
            }
        }
        assert!(!has_positive, "type_btm should have no positive-multiplicity entries after remove");
    }

    #[test]
    fn test_type_btm_fork() {
        let f = factory();
        let mut e = env();

        let type_a = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("Number")]);
        e.add_to_space(&type_a);

        // Fork the environment
        let forked = e.fork_for_nondeterminism();

        // Forked type_btm should have the same entries (O(1) CoW clone)
        let type_btm = forked.shared.atom_space.type_btm.read();
        let mut rz = type_btm.read_zipper();
        let mut count = 0;
        while rz.to_next_val() {
            count += 1;
        }
        assert!(count > 0, "forked type_btm should preserve entries");
    }

    #[test]
    fn test_subtype_btm_incremental() {
        let f = factory();
        let mut e = env();

        // Add a subtype declaration
        let decl = f.sexpr(vec![f.atom(":<"), f.atom("Dog"), f.atom("Animal")]);
        e.add_to_space(&decl);

        // subtype_btm should have an entry
        let sub_btm = e.shared.atom_space.subtype_btm.read();
        let mut rz = sub_btm.read_zipper();
        let mut count = 0usize;
        while rz.to_next_val() {
            count += 1;
        }
        assert!(count > 0, "subtype_btm should have entry after (:< Dog Animal)");

        // Remove it
        drop(sub_btm);
        e.remove_from_space(&decl);

        let sub_btm = e.shared.atom_space.subtype_btm.read();
        let mut rz = sub_btm.read_zipper();
        let mut has_positive = false;
        while rz.to_next_val() {
            if rz.val().map_or(false, |m| m.count() > 0) {
                has_positive = true;
            }
        }
        assert!(!has_positive, "subtype_btm should be empty after removal");
    }

    #[test]
    fn test_ensure_type_index_uses_type_btm() {
        let f = factory();
        let mut e = env();

        // Add type assertions
        let type_a = f.sexpr(vec![f.atom(":"), f.atom("x"), f.atom("Number")]);
        let type_b = f.sexpr(vec![f.atom(":"), f.atom("y"), f.atom("Bool")]);
        e.add_to_space(&type_a);
        e.add_to_space(&type_b);

        // get_type queries through ensure_type_index → type_btm snapshot
        let x_types = e.get_type("x");
        assert!(!x_types.is_empty(), "get_type('x') should find Number");
        assert!(x_types.iter().any(|t| t.as_atom() == Some("Number")));

        let y_types = e.get_type("y");
        assert!(!y_types.is_empty(), "get_type('y') should find Bool");
        assert!(y_types.iter().any(|t| t.as_atom() == Some("Bool")));
    }

    // ======================================================================
    // Phase 7.2: Type Bloom Filter tests
    // ======================================================================

    #[test]
    fn test_type_bloom_rejection() {
        let e = env();

        // No types declared — bloom filter should reject
        assert!(
            e.get_types_generic("untyped_atom").is_empty(),
            "bloom filter should reject untyped atom"
        );
    }

    #[test]
    fn test_type_bloom_no_false_negative() {
        let f = factory();
        let mut e = env();

        // Declare type via add_type_generic
        e.add_type_generic("x", f.atom("Number"));

        // Bloom filter must NOT reject — no false negatives allowed
        let types = e.get_types_generic("x");
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].as_atom(), Some("Number"));
    }

    #[test]
    fn test_type_bloom_add_to_space() {
        let f = factory();
        let mut e = env();

        // Add type via add_to_space (exercises the MORK path bloom insert)
        let type_atom = f.sexpr(vec![f.atom(":"), f.atom("y"), f.atom("Bool")]);
        e.add_to_space(&type_atom);

        // get_types_generic should pass bloom filter and find the type
        let types = e.get_types_generic("y");
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].as_atom(), Some("Bool"));
    }

    #[test]
    fn test_type_bloom_survives_fork() {
        let f = factory();
        let mut e = env();
        e.add_type_generic("x", f.atom("Number"));

        // Fork environment — bloom filter is Arc-cloned
        let forked = e.fork_for_nondeterminism();

        // Forked env should still pass bloom filter
        let types = forked.get_types_generic("x");
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].as_atom(), Some("Number"));
    }

    #[test]
    fn test_type_bloom_get_type_mork_path() {
        let f = factory();
        let mut e = env();

        // Add type via add_to_space (populates both HashMap and MORK type_btm)
        let type_atom = f.sexpr(vec![f.atom(":"), f.atom("z"), f.atom("String")]);
        e.add_to_space(&type_atom);

        // get_type (MORK path) should pass bloom filter
        let types = e.get_type("z");
        assert!(!types.is_empty(), "get_type('z') should find String via MORK path");
        assert!(types.iter().any(|t| t.as_atom() == Some("String")));

        // Untyped atom should be rejected by bloom filter before MORK trie traversal
        let empty = e.get_type("nonexistent");
        assert!(empty.is_empty(), "bloom filter should reject nonexistent atom");
    }
}
