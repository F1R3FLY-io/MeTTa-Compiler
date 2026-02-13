//! Rule management operations for Environment.
//!
//! Provides methods for adding, indexing, and querying rules.
//! Rules are stored as (= lhs rhs) in MORK PathMap as MORK bytes.
//!
//! # Multiplicity Tracking
//!
//! Rules can be defined multiple times, and we track multiplicities efficiently
//! using PathMap<Multiplicity> — each MORK byte key maps to its multiplicity count.
//!
//! # Rule Discovery
//!
//! Rules are discovered by iterating PathMap entries and filtering for (= lhs rhs)
//! s-expressions. Bloom filter provides O(1) rejection for non-matching patterns.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use mork::space::Space;
use mork_expr::Expr;
use pathmap::PathMap;
use tracing::trace;

use super::generic::GenericEnvironment;
use super::mork_encoding::mork_expr_to_generic_value;
use super::multiplicity::{
    decrement_multiplicity, get_multiplicity, increment_multiplicity, set_multiplicity,
    Multiplicity,
};
use super::{MettaEnvironment, MettaValue};
use crate::backend::models::{MettaValueFactory, MettaValueInner, MettaValueTrait};
use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};

/// Extract (lhs, rhs) from a deserialized rule value `(= lhs rhs)`.
///
/// Returns `Some((lhs, rhs))` if the value is an s-expression with 3 elements
/// where the first element is the atom `"="`.
fn extract_rule_parts<V: MettaValueTrait + Clone>(value: &V) -> Option<(V, V)> {
    let children = value.as_sexpr()?;
    if children.len() == 3 {
        if let Some(op) = children[0].as_atom() {
            if op == "=" {
                return Some((children[1].clone(), children[2].clone()));
            }
        }
    }
    None
}

/// Iterator over rule heads with their arities and counts.
///
/// Data is collected into a Vec during creation for iteration.
pub struct RuleHeadsIter {
    inner: std::vec::IntoIter<(String, usize, usize)>,
}

impl RuleHeadsIter {
    /// Create a new iterator from collected rule heads.
    pub fn new(items: Vec<(String, usize, usize)>) -> Self {
        Self {
            inner: items.into_iter(),
        }
    }
}

impl Iterator for RuleHeadsIter {
    type Item = (String, usize, usize);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

// ============================================================================
// Generic Rule Operations (for GenericEnvironment<V, F>)
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Add a rule to the environment.
    ///
    /// The rule is stored as `(= lhs rhs)` in the MORK PathMap.
    /// Bloom filter is updated for O(1) rejection in match_space().
    ///
    /// # Arguments
    /// - `lhs`: The left-hand side pattern
    /// - `rhs`: The right-hand side template
    pub fn add_rule(&mut self, lhs: V, rhs: V) {
        trace!(target: "mettatron::environment::add_rule", "Adding rule");
        self.make_owned(); // CoW: ensure we own data before modifying

        // Get head symbol and arity for bloom filter (clone head string before moving lhs)
        let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
        let arity = lhs.get_arity();

        // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
        if let Some(ref head) = head_owned {
            self.shared.fuzzy_matcher.write().insert(head);
        }

        // Create rule s-expression: (= lhs rhs)
        let rule_sexpr = self.factory.sexpr(vec![
            self.factory.atom("="),
            lhs,
            rhs,
        ]);

        // Add to MORK Space (handles MORK bytes conversion and PathMap insertion)
        self.add_to_space(&rule_sexpr);

        // Update bloom filter with (head, arity) for O(1) match_space() rejection
        if let Some(ref head) = head_owned {
            let arity_u8 = arity as u8;
            self.shared
                .head_arity_bloom
                .write()
                .insert(head.as_bytes(), arity_u8);
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Get matching rules for an expression from PathMap.
    ///
    /// Returns `(lhs, rhs, multiplicity)` tuples for all rules whose LHS
    /// head symbol and arity match the given expression. The caller is
    /// responsible for performing full pattern matching on the returned
    /// candidates.
    ///
    /// # Performance
    /// O(n) where n = number of rules. Bloom filter provides O(1)
    /// rejection before iteration for non-matching head/arity combinations.
    pub fn get_matching_rules_for_expr(&self, expr: &V) -> Vec<(V, V, u64)> {
        let head = expr.get_head_symbol().unwrap_or("");
        let arity = expr.get_arity();

        // Bloom filter O(1) rejection
        if !head.is_empty() {
            if !self
                .shared
                .head_arity_bloom
                .read()
                .may_contain(head.as_bytes(), arity as u8)
            {
                return Vec::new();
            }
        }

        let space = self.create_space();
        let mut rules: Vec<(V, V, u64)> = Vec::new();

        // Iterate PathMap entries, filter for (= lhs rhs) rules
        for (path_bytes, _) in space.btm.iter() {
            let expr_ref = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };

            if let Ok(value) = mork_expr_to_generic_value::<V, F, Multiplicity>(
                &expr_ref, &space, &self.factory,
            ) {
                if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                    // Check if head/arity match before doing full pattern match
                    let rule_head = lhs.get_head_symbol().unwrap_or("");
                    let rule_arity = lhs.get_arity();

                    // Match either same head+arity, or wildcard rule (no head)
                    let head_matches = rule_head.is_empty()
                        || (rule_head == head && rule_arity == arity);

                    if head_matches {
                        let multiplicity = get_multiplicity(&space.btm, &path_bytes).max(1);
                        rules.push((lhs, rhs, multiplicity));
                    }
                }
            }
        }

        rules
    }
}

// ============================================================================
// MettaEnvironment-specific Rule Operations
// ============================================================================

impl MettaEnvironment {
    /// Get the number of rules in the environment.
    pub fn rule_count(&self) -> usize {
        let space = self.create_space();
        let mut count = 0;

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if Self::is_rule_sexpr(&value) {
                    count += 1;
                }
            }
        }

        count
    }

    /// Iterator over rule heads with their arities and counts.
    pub fn iter_rule_heads(&self) -> RuleHeadsIter {
        let space = self.create_space();
        let mut head_map: HashMap<(String, usize), usize> = HashMap::new();

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, _rhs)) = extract_rule_parts(&value) {
                    let head = lhs.get_head_symbol().unwrap_or("").to_string();
                    let arity = lhs.get_arity();
                    *head_map.entry((head, arity)).or_insert(0) += 1;
                }
            }
        }

        let items: Vec<(String, usize, usize)> = head_map
            .into_iter()
            .map(|((head, arity), count)| (head, arity, count))
            .collect();

        RuleHeadsIter::new(items)
    }

    /// Collect all rules as (lhs, rhs) pairs.
    pub fn collect_rules(&self) -> Vec<(MettaValue, MettaValue)> {
        let space = self.create_space();
        let mut rules = Vec::new();

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                    rules.push((lhs, rhs));
                }
            }
        }

        rules
    }

    /// Bulk add rules using PathMap::join() for batch efficiency.
    ///
    /// # Arguments
    /// * `rules` - Vec of (lhs, rhs) pairs
    pub fn add_rules_bulk(&mut self, rules: Vec<(MettaValue, MettaValue)>) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_rules_bulk", rule_count = rules.len());
        if rules.is_empty() {
            return Ok(());
        }

        self.make_owned();

        let mut rule_trie: PathMap<Multiplicity> = PathMap::new();

        for (lhs, rhs) in &rules {
            let rule_sexpr = MettaValue::SExpr(vec![
                MettaValue::Atom("=".to_string()),
                lhs.clone(),
                rhs.clone(),
            ]);

            let mut ctx = ConversionContext::new();
            let mork_bytes = metta_to_mork_bytes(&rule_sexpr, &self.shared_mapping, &mut ctx)
                .map_err(|e| format!("MORK conversion failed for rule {:?}: {}", rule_sexpr, e))?;

            rule_trie.insert(&mork_bytes, Multiplicity::new(1));

            // Update bloom filter
            if let Some(head) = lhs.get_head_symbol() {
                let arity = lhs.get_arity() as u8;
                self.shared.fuzzy_matcher.write().insert(head);
                self.shared
                    .head_arity_bloom
                    .write()
                    .insert(head.as_bytes(), arity);
            }

            self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);
        }

        // Single PathMap union (minimal critical section)
        {
            let mut btm = self.shared.btm.write();
            *btm = btm.join(&rule_trie);
        }
        self.modified.store(true, Ordering::Release);
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity).
    ///
    /// Uses PathMap-based multiplicity lookup.
    pub fn get_rule_count(&self, lhs: &MettaValue, rhs: &MettaValue) -> usize {
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            lhs.clone(),
            rhs.clone(),
        ]);

        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(&rule_sexpr, &self.shared_mapping, &mut ctx) {
            Ok(mork_bytes) => {
                let btm = self.shared.btm.read();
                let count = get_multiplicity(&btm, &mork_bytes);
                if count == 0 { 1 } else { count as usize }
            }
            Err(_) => 1,
        }
    }

    /// Get the multiplicities (for serialization).
    /// The keys are hex-encoded MORK bytes for serialization stability.
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        let btm = self.shared.btm.read();
        let mut result = HashMap::new();

        for (path, multiplicity) in btm.iter() {
            let count = multiplicity.count() as usize;
            let count = if count == 0 { 1 } else { count };
            let hex_key = hex::encode(&path);
            result.insert(hex_key, count);
        }

        result
    }

    /// Set the multiplicities (used for deserialization).
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        self.make_owned();

        let mut btm = self.shared.btm.write();

        for (hex_key, count) in counts {
            if let Ok(mork_bytes) = hex::decode(&hex_key) {
                set_multiplicity(&mut btm, &mork_bytes, count as u64);
                self.shared.total_atoms.fetch_add(count, Ordering::Relaxed);
            }
        }

        drop(btm);
        self.modified.store(true, Ordering::Release);
    }

    /// Rebuild bloom filter and fuzzy matcher from PathMap.
    ///
    /// This is needed after deserializing an Environment from PathMap Par,
    /// since the serialization only preserves the PathMap, not the bloom filter.
    pub fn rebuild_bloom_filter(&mut self) {
        trace!(target: "mettatron::environment::rebuild_bloom_filter", "Rebuilding bloom filter and fuzzy matcher from PathMap");
        self.make_owned();

        let space = self.create_space();

        for (path_bytes, _) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, _rhs)) = extract_rule_parts(&value) {
                    if let Some(head) = lhs.get_head_symbol() {
                        let arity = lhs.get_arity();
                        self.shared.fuzzy_matcher.write().insert(head);
                        self.shared
                            .head_arity_bloom
                            .write()
                            .insert(head.as_bytes(), arity as u8);
                    }
                }
            }
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Increment the multiplicity count for a rule.
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after increment.
    pub fn increment_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::increment_rule_multiplicity", ?rule_sexpr);
        self.make_owned();

        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(rule_sexpr, &self.shared_mapping, &mut ctx) {
            Ok(mork_bytes) => {
                let mut btm = self.shared.btm.write();
                let new_count = increment_multiplicity(&mut btm, &mork_bytes);
                drop(btm);

                self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            }
            Err(_) => {
                self.modified.store(true, Ordering::Release);
                1
            }
        }
    }

    /// Decrement the multiplicity count for a rule.
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after decrement, or 0 if the rule wasn't tracked.
    pub fn decrement_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::decrement_rule_multiplicity", ?rule_sexpr);
        self.make_owned();

        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(rule_sexpr, &self.shared_mapping, &mut ctx) {
            Ok(mork_bytes) => {
                let old_count = {
                    let btm = self.shared.btm.read();
                    get_multiplicity(&btm, &mork_bytes)
                };

                if old_count == 0 {
                    self.modified.store(true, Ordering::Release);
                    return 0;
                }

                let mut btm = self.shared.btm.write();
                let new_count = decrement_multiplicity(&mut btm, &mork_bytes);
                drop(btm);

                self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            }
            Err(_) => {
                self.modified.store(true, Ordering::Release);
                0
            }
        }
    }

    /// Check if a MettaValue is a rule s-expression (= lhs rhs)
    pub fn is_rule_sexpr(value: &MettaValue) -> bool {
        if let MettaValueInner::SExpr(items) = value.inner() {
            if items.len() == 3 {
                if let MettaValueInner::Atom(op) = items[0].inner() {
                    return *op == "=";
                }
            }
        }
        false
    }

    // ========================================================================
    // All-Atom Multiplicity Tracking (MeTTa HE Semantics)
    // ========================================================================

    /// Increment multiplicity for ANY atom (not just rules).
    pub fn increment_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &self.shared_mapping, &mut ctx) {
            Ok(mork_bytes) => {
                let mut btm = self.shared.btm.write();
                let new_count = increment_multiplicity(&mut btm, &mork_bytes);
                drop(btm);

                self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            }
            Err(_) => {
                self.modified.store(true, Ordering::Release);
                1
            }
        }
    }

    /// Decrement multiplicity for ANY atom.
    pub fn decrement_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &self.shared_mapping, &mut ctx) {
            Ok(mork_bytes) => {
                let old_count = {
                    let btm = self.shared.btm.read();
                    get_multiplicity(&btm, &mork_bytes)
                };

                if old_count == 0 {
                    return 0;
                }

                let mut btm = self.shared.btm.write();
                let new_count = decrement_multiplicity(&mut btm, &mork_bytes);
                drop(btm);

                self.shared.total_atoms.fetch_sub(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                new_count as usize
            }
            Err(_) => 0,
        }
    }

    /// Get multiplicity for ANY atom.
    pub fn get_atom_multiplicity(&self, value: &MettaValue) -> usize {
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &self.shared_mapping, &mut ctx) {
            Ok(mork_bytes) => {
                let btm = self.shared.btm.read();
                let count = get_multiplicity(&btm, &mork_bytes);
                if count == 0 { 1 } else { count as usize }
            }
            Err(_) => 1,
        }
    }

    /// Get atom multiplicity from raw MORK bytes.
    pub fn get_multiplicity_from_mork_bytes(&self, mork_bytes: &[u8]) -> usize {
        let btm = self.shared.btm.read();
        let count = get_multiplicity(&btm, mork_bytes);
        if count == 0 { 1 } else { count as usize }
    }
}
