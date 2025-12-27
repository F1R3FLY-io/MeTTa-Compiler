//! Type system operations for Environment.
//!
//! Provides methods for type assertions, type indexing, and type lookups.
//! Type assertions are stored as (: name type) in MORK Space.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use mork::space::Space;
use mork_expr::Expr;
use pathmap::PathMap;
use tracing::trace;

use super::{Environment, MettaValue};

impl Environment {
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

        // Invalidate type index cache
        *self
            .shared
            .type_index_dirty
            .write()
            .expect("type_index_dirty lock poisoned") = true;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Ensure the type index is built and up-to-date
    /// Uses PathMap's restrict() to extract only type assertions into a subtrie
    /// This enables O(p + m) type lookups where m << n (total facts)
    ///
    /// The type index is lazily initialized and cached until invalidated
    pub(crate) fn ensure_type_index(&self) {
        let dirty = *self
            .shared
            .type_index_dirty
            .read()
            .expect("type_index_dirty lock poisoned");
        if !dirty {
            return; // Index is up to date
        }

        // Build type index using PathMap::restrict()
        // This extracts a subtrie containing only paths that start with ":"
        let btm = self.shared.btm.read().expect("btm lock poisoned");

        // Create a PathMap containing only the ":" prefix
        // restrict() will return all paths in btm that have matching prefixes in this map
        let mut type_prefix_map = PathMap::new();
        let colon_bytes = b":";

        // Insert a single path with just ":" to match all type assertions
        {
            use pathmap::zipper::*;
            let mut wz = type_prefix_map.write_zipper();
            for &byte in colon_bytes {
                wz.descend_to_byte(byte);
            }
            wz.set_val(());
        }

        // Extract type subtrie using restrict()
        let type_subtrie = btm.restrict(&type_prefix_map);

        // Cache the subtrie
        *self
            .shared
            .type_index
            .write()
            .expect("type_index lock poisoned") = Some(type_subtrie);
        *self
            .shared
            .type_index_dirty
            .write()
            .expect("type_index_dirty lock poisoned") = false;
    }

    /// Get type for an atom by querying MORK Space
    /// Searches for type assertions of the form (: name type)
    /// Returns None if no type assertion exists for the given name
    ///
    /// OPTIMIZED: Uses PathMap::restrict() to create a type-only subtrie
    /// Then navigates within that subtrie for O(p + m) lookup where m << n
    /// Falls back to O(n) linear search if index lookup fails
    #[allow(clippy::collapsible_match)]
    pub fn get_type(&self, name: &str) -> Option<MettaValue> {
        trace!(target: "mettatron::environment::get_type", name);

        // Ensure type index is built and up-to-date
        self.ensure_type_index();

        // Get the type index subtrie
        let type_index_opt = self
            .shared
            .type_index
            .read()
            .expect("type_index lock poisoned");
        let type_index = match type_index_opt.as_ref() {
            Some(index) => index,
            None => {
                // Index failed to build, fall back to linear search
                trace!(target: "mettatron::environment::get_type", name, "Falling back to linear search");
                drop(type_index_opt); // Release lock before fallback
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

        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();

        // Try O(p + m) lookup within type subtrie where m << n
        // descend_to_check navigates the trie by exact byte sequence
        if rz.descend_to_check(mork_bytes) {
            // Found exact match for prefix (: name)
            // Now extract the full assertion: (: name TYPE)
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Extract TYPE from (: name TYPE)
                if let MettaValue::SExpr(items) = value {
                    if items.len() >= 3 {
                        // items[0] = ":", items[1] = name, items[2] = TYPE
                        return Some(items[2].clone());
                    }
                }
            }
        }

        // Release the type index lock before fallback
        drop(type_index_opt);

        // Slow path: O(n) linear search (fallback if exact match fails)
        // This handles edge cases where MORK encoding might differ
        trace!(target: "mettatron::environment::get_type", name, "Fast path failed, using linear search");
        self.get_type_linear(name)
    }

    /// Linear search fallback for get_type() - O(n) iteration
    /// Used when exact match via descend_to_check() fails
    fn get_type_linear(&self, name: &str) -> Option<MettaValue> {
        let space = self.create_space();
        use pathmap::zipper::*;
        let mut rz = space.btm.read_zipper();

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
                if let MettaValue::SExpr(items) = &value {
                    if items.len() == 3 {
                        if let (MettaValue::Atom(op), MettaValue::Atom(atom_name), typ) =
                            (&items[0], &items[1], &items[2])
                        {
                            if op == ":" && atom_name == name {
                                return Some(typ.clone());
                            }
                        }
                    }
                }
            }
        }

        None
    }
}
