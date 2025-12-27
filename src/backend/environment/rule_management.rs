//! Rule management operations for Environment.
//!
//! Provides methods for adding, indexing, and querying rules.
//! Rules are stored as (= lhs rhs) in MORK Space.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use mork::space::Space;
use mork_expr::Expr;
use pathmap::PathMap;
use tracing::trace;

use super::{Environment, MettaValue, Rule};
use crate::backend::symbol::Symbol;

impl Environment {
    /// Get the number of rules in the environment
    /// Counts rules from the rule_index and wildcard_rules (thread-safe, avoids PathMap iteration)
    pub fn rule_count(&self) -> usize {
        // Count rules from the indexed rules
        let index_count: usize = self
            .shared
            .rule_index
            .read()
            .expect("rule_index lock poisoned")
            .values()
            .map(|rules| rules.len())
            .sum();

        // Count wildcard rules
        let wildcard_count = self
            .shared
            .wildcard_rules
            .read()
            .expect("wildcard_rules lock poisoned")
            .len();

        index_count + wildcard_count
    }

    /// Iterator over rule heads with their arities and counts.
    ///
    /// Returns tuples of (head_symbol, arity, rule_count) for each distinct
    /// (head, arity) combination in the rule index.
    ///
    /// # Performance
    /// - O(k) where k = number of distinct (head, arity) pairs
    /// - No PathMap iteration or MORK conversion required
    /// - Much faster than iter_rules() for use cases that only need heads
    ///
    /// # Use Cases
    /// - REPL command completion (showing available rule heads)
    /// - Rule statistics and introspection
    /// - Pattern matching optimization hints
    pub fn iter_rule_heads(&self) -> Vec<(String, usize, usize)> {
        let index = self
            .shared
            .rule_index
            .read()
            .expect("rule_index lock poisoned");
        index
            .iter()
            .map(|((head, arity), rules)| (head.to_string(), *arity, rules.len()))
            .collect()
    }

    /// Iterator over all rules in the Space
    /// Rules are stored as MORK s-expressions: (= lhs rhs)
    ///
    /// Uses PathMap's iter() method with owned copies of MORK bytes.
    /// This avoids raw pointer issues that could cause memory corruption
    /// under concurrent access patterns.
    #[allow(clippy::collapsible_match)]
    pub fn iter_rules(&self) -> impl Iterator<Item = Rule> {
        let space = self.create_space();
        let mut rules = Vec::new();

        // Use PathMap's iter() which returns (Vec<u8>, &V) with owned byte vectors
        // This is safer than raw pointer access via read_zipper().path().as_ptr()
        // because each Vec<u8> is a fully owned copy of the MORK expression bytes
        for (mork_bytes, _) in space.btm.iter() {
            // Create Expr from owned bytes - safe because mork_bytes outlives expr usage
            let expr = Expr {
                ptr: mork_bytes.as_ptr().cast_mut(),
            };

            // Convert MORK expression to MettaValue
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let MettaValue::SExpr(items) = &value {
                    if items.len() == 3 {
                        if let MettaValue::Atom(op) = &items[0] {
                            if op == "=" {
                                rules.push(Rule::new(items[1].clone(), items[2].clone()));
                            }
                        }
                    }
                }
            }
        }

        rules.into_iter()
    }

    /// Rebuild the rule index from the MORK Space
    /// This is needed after deserializing an Environment from PathMap Par,
    /// since the serialization only preserves the MORK Space, not the index.
    pub fn rebuild_rule_index(&mut self) {
        trace!(target: "mettatron::environment::rebuild_rule_index", "Rebuilding rule index");
        self.make_owned(); // CoW: ensure we own data before modifying

        // Clear existing indices
        {
            let mut index = self
                .shared
                .rule_index
                .write()
                .expect("rule_index lock poisoned");
            index.clear();
        }
        {
            let mut wildcards = self
                .shared
                .wildcard_rules
                .write()
                .expect("wildcard_rules lock poisoned");
            wildcards.clear();
        }
        // Reset wildcard flag - will be set again if wildcards are added
        self.shared
            .has_wildcard_rules
            .store(false, Ordering::Release);

        // Rebuild from MORK Space
        for rule in self.iter_rules() {
            if let Some(head) = rule.lhs.get_head_symbol() {
                let arity = rule.lhs.get_arity();
                // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
                self.shared
                    .fuzzy_matcher
                    .write()
                    .expect("fuzzy_matcher lock poisoned")
                    .insert(head);
                // Use Symbol for O(1) comparison when symbol-interning is enabled
                let head_sym = Symbol::new(head);
                let mut index = self
                    .shared
                    .rule_index
                    .write()
                    .expect("rule_index lock poisoned");
                index.entry((head_sym, arity)).or_default().push(rule);
            } else {
                // Rules without head symbol (wildcards, variables) go to wildcard list
                let mut wildcards = self
                    .shared
                    .wildcard_rules
                    .write()
                    .expect("wildcard_rules lock poisoned");
                wildcards.push(rule);
                // Mark that we have wildcard rules
                self.shared
                    .has_wildcard_rules
                    .store(true, Ordering::Release);
            }
        }

        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Add a rule to the environment
    /// Rules are stored in MORK Space as s-expressions: (= lhs rhs)
    /// Multiply-defined rules are tracked via multiplicities
    /// Rules are also indexed by (head_symbol, arity) for fast lookup
    pub fn add_rule(&mut self, rule: Rule) {
        trace!(target: "mettatron::environment::add_rule", ?rule);
        self.make_owned(); // CoW: ensure we own data before modifying

        // Create a rule s-expression: (= lhs rhs)
        // Dereference the Arc to get the MettaValue
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            (*rule.lhs).clone(),
            (*rule.rhs).clone(),
        ]);

        // Generate a canonical key for the rule
        // Use MORK string format for readable serialization
        let rule_key = rule_sexpr.to_mork_string();

        // Increment the count for this rule
        {
            let mut counts = self
                .shared
                .multiplicities
                .write()
                .expect("multiplicities lock poisoned");
            let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
            counts.insert(rule_key.clone(), new_count);
        } // Drop the RefMut borrow before add_to_space

        // Add to rule index for O(k) lookup
        // Note: We store the rule only ONCE (in either index or wildcard list)
        // to avoid unnecessary clones. The rule is already in MORK Space.
        if let Some(head) = rule.lhs.get_head_symbol() {
            let arity = rule.lhs.get_arity();
            // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
            self.shared
                .fuzzy_matcher
                .write()
                .expect("fuzzy_matcher lock poisoned")
                .insert(head);
            // Use Symbol for O(1) comparison when symbol-interning is enabled
            let head_sym = Symbol::new(head);
            let mut index = self
                .shared
                .rule_index
                .write()
                .expect("rule_index lock poisoned");
            index.entry((head_sym, arity)).or_default().push(rule); // Move instead of clone
        } else {
            // Rules without head symbol (wildcards, variables) go to wildcard list
            let mut wildcards = self
                .shared
                .wildcard_rules
                .write()
                .expect("wildcard_rules lock poisoned");
            wildcards.push(rule); // Move instead of clone
                                  // Mark that we have wildcard rules (for fast-path in get_matching_rules)
            self.shared
                .has_wildcard_rules
                .store(true, Ordering::Release);
        }

        // Add to MORK Space (only once - PathMap will deduplicate)
        self.add_to_space(&rule_sexpr);
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Bulk add rules using PathMap::join() for batch efficiency
    /// This is significantly faster than individual add_rule() calls
    /// for large batches (20-100× speedup) due to:
    /// - Single lock acquisition for PathMap update
    /// - Bulk union operation instead of N individual inserts
    /// - Reduced overhead for rule index and multiplicity updates
    ///
    /// Expected speedup: 20-100× for batches of 100+ rules
    /// Complexity: O(k) where k = batch size (vs O(n × lock) for individual adds)
    pub fn add_rules_bulk(&mut self, rules: Vec<Rule>) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_rules_bulk", rule_count = rules.len());
        if rules.is_empty() {
            return Ok(());
        }

        self.make_owned(); // CoW: ensure we own data before modifying

        // Build temporary PathMap outside the lock
        let mut rule_trie = PathMap::new();

        // Track rule metadata while building trie
        // Use Symbol for O(1) comparison when symbol-interning is enabled
        let mut rule_index_updates: HashMap<(Symbol, usize), Vec<Rule>> = HashMap::new();
        let mut wildcard_updates: Vec<Rule> = Vec::new();
        let mut multiplicity_updates: HashMap<String, usize> = HashMap::new();

        for rule in rules {
            // Create rule s-expression: (= lhs rhs)
            // Dereference the Arc to get the MettaValue
            let rule_sexpr = MettaValue::SExpr(vec![
                MettaValue::Atom("=".to_string()),
                (*rule.lhs).clone(),
                (*rule.rhs).clone(),
            ]);

            // Track multiplicity
            let rule_key = rule_sexpr.to_mork_string();
            *multiplicity_updates.entry(rule_key).or_insert(0) += 1;

            // Prepare rule index updates
            if let Some(head) = rule.lhs.get_head_symbol() {
                let arity = rule.lhs.get_arity();
                // Track symbol for fuzzy matching
                self.shared
                    .fuzzy_matcher
                    .write()
                    .expect("fuzzy_matcher lock poisoned")
                    .insert(head);
                // Use Symbol for O(1) comparison when symbol-interning is enabled
                let head_sym = Symbol::new(head);
                rule_index_updates
                    .entry((head_sym, arity))
                    .or_default()
                    .push(rule);
            } else {
                wildcard_updates.push(rule);
            }

            // OPTIMIZATION: Always use direct MORK byte conversion
            // This works for both ground terms AND variable-containing terms
            // Variables are encoded using De Bruijn indices
            use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};

            let temp_space = Space {
                sm: self.shared_mapping.clone(),
                btm: PathMap::new(),
                mmaps: HashMap::new(),
            };
            let mut ctx = ConversionContext::new();

            let mork_bytes = metta_to_mork_bytes(&rule_sexpr, &temp_space, &mut ctx)
                .map_err(|e| format!("MORK conversion failed for rule {:?}: {}", rule_sexpr, e))?;

            // Direct insertion without string serialization or parsing
            rule_trie.insert(&mork_bytes, ());
        }

        // Apply all updates in batch (minimize critical sections)

        // Update multiplicities
        {
            let mut counts = self
                .shared
                .multiplicities
                .write()
                .expect("multiplicities lock poisoned");
            for (key, delta) in multiplicity_updates {
                *counts.entry(key).or_insert(0) += delta;
            }
        }

        // Update rule index
        {
            let mut index = self
                .shared
                .rule_index
                .write()
                .expect("rule_index lock poisoned");
            for ((head, arity), mut rules) in rule_index_updates {
                index.entry((head, arity)).or_default().append(&mut rules);
            }
        }

        // Update wildcard rules
        let has_new_wildcards = !wildcard_updates.is_empty();
        {
            let mut wildcards = self
                .shared
                .wildcard_rules
                .write()
                .expect("wildcard_rules lock poisoned");
            wildcards.extend(wildcard_updates);
        }
        if has_new_wildcards {
            self.shared
                .has_wildcard_rules
                .store(true, Ordering::Release);
        }

        // Single PathMap union (minimal critical section)
        {
            let mut btm = self.shared.btm.write().expect("btm lock poisoned");
            *btm = btm.join(&rule_trie);
        }
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity)
    /// Returns 1 if the rule exists but count wasn't tracked (for backward compatibility)
    pub fn get_rule_count(&self, rule: &Rule) -> usize {
        // Dereference the Arc to get the MettaValue
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            (*rule.lhs).clone(),
            (*rule.rhs).clone(),
        ]);
        let rule_key = rule_sexpr.to_mork_string();

        let counts = self
            .shared
            .multiplicities
            .read()
            .expect("multiplicities lock poisoned");
        *counts.get(&rule_key).unwrap_or(&1)
    }

    /// Get the multiplicities (for serialization)
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        self.shared
            .multiplicities
            .read()
            .expect("multiplicities lock poisoned")
            .clone()
    }

    /// Set the multiplicities (used for deserialization)
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self
            .shared
            .multiplicities
            .write()
            .expect("multiplicities lock poisoned") = counts;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Get rules matching a specific head symbol and arity
    /// Returns Vec<Rule> for O(1) lookup instead of O(n) iteration
    /// Also includes wildcard rules that must be checked against all queries
    pub fn get_matching_rules(&self, head: &str, arity: usize) -> Vec<Rule> {
        trace!(target: "mettatron::environment::get_matching_rules", head, arity);

        // Use Symbol for O(1) comparison when symbol-interning is enabled
        let key = (Symbol::new(head), arity);

        // Fast-path: Check if we have any wildcard rules before acquiring the lock
        let has_wildcards = self.shared.has_wildcard_rules.load(Ordering::Acquire);

        // Get indexed rules first
        let index = self
            .shared
            .rule_index
            .read()
            .expect("rule_index lock poisoned");
        let indexed_rules = index.get(&key);
        let indexed_len = indexed_rules.map_or(0, |r| r.len());

        // OPTIMIZATION: Skip wildcard lock acquisition if no wildcard rules exist
        if !has_wildcards {
            // No wildcard rules - just return indexed rules
            let mut matching_rules = Vec::with_capacity(indexed_len);
            if let Some(rules) = indexed_rules {
                matching_rules.extend(rules.iter().cloned());
            }
            return matching_rules;
        }

        // Have wildcard rules - need to acquire lock
        let wildcards = self
            .shared
            .wildcard_rules
            .read()
            .expect("wildcard_rules lock poisoned");
        let wildcard_len = wildcards.len();

        // OPTIMIZATION: Preallocate capacity to avoid reallocation
        let mut matching_rules = Vec::with_capacity(indexed_len + wildcard_len);

        // Get indexed rules with matching head symbol and arity
        if let Some(rules) = indexed_rules {
            matching_rules.extend(rules.iter().cloned());
        }

        // Also include wildcard rules (must always be checked)
        matching_rules.extend(wildcards.iter().cloned());

        trace!(
            target: "mettatron::environment::get_matching_rules",
            match_ctr = matching_rules.len(), "Rules matching"
        );
        matching_rules
    }
}
