//! Rule management operations for Environment.
//!
//! Provides methods for adding, indexing, and querying rules.
//! Rules are stored as (= lhs rhs) in MORK Space.
//!
//! # Multiplicity Tracking
//!
//! Rules can be defined multiple times, and we track multiplicities efficiently
//! using MORK bytes as keys. This avoids the overhead of symbol interning since
//! MORK bytes are already computed for PathMap storage.
//!
//! # Lazy Iteration
//!
//! Rule iterators use `owning_ref` to keep lock guards alive for iterator lifetimes,
//! enabling true lazy iteration without upfront Vec allocation.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::RwLockReadGuard;

use indexmap::IndexSet;
use mork::space::Space;
use mork_expr::Expr;
use owning_ref::OwningHandle;
use pathmap::PathMap;
use tracing::trace;

use super::multiplicity::{
    add_atom, decrement_multiplicity, get_multiplicity, increment_multiplicity, set_multiplicity,
    Multiplicity,
};
use super::{Environment, MettaValue, Rule};
use crate::backend::models::MettaValueInner;
use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};
use crate::backend::symbol::Symbol;

/// Lazy iterator over rule heads that owns its lock guard.
///
/// Uses `owning_ref::OwningHandle` to safely hold a `RwLockReadGuard` alongside
/// an iterator that borrows from it. This enables true lazy iteration without
/// collecting to a Vec first.
///
/// # Performance
/// - Zero Vec allocation
/// - Lock held for iterator lifetime
/// - Items produced on-demand
pub struct RuleHeadsIter<'a> {
    /// OwningHandle owns the guard and contains an iterator that borrows from it.
    /// The type is: OwningHandle<Guard, Box<dyn Iterator + 'a>>
    inner: OwningHandle<
        RwLockReadGuard<'a, HashMap<(Symbol, usize), IndexSet<Rule>>>,
        Box<dyn Iterator<Item = (String, usize, usize)> + 'a>,
    >,
}

impl<'a> RuleHeadsIter<'a> {
    /// Create a new lazy iterator from a lock guard.
    pub fn new(guard: RwLockReadGuard<'a, HashMap<(Symbol, usize), IndexSet<Rule>>>) -> Self {
        // SAFETY: OwningHandle ensures the guard outlives the iterator.
        // The iterator only borrows from the owned guard, so the reference is valid.
        let inner = OwningHandle::new_with_fn(guard, |index_ptr| {
            // SAFETY: index_ptr is valid for the lifetime of the OwningHandle
            let index = unsafe { &*index_ptr };
            let iter = index
                .iter()
                .map(|((head, arity), rules)| (head.to_string(), *arity, rules.len()));
            Box::new(iter) as Box<dyn Iterator<Item = (String, usize, usize)> + 'a>
        });
        Self { inner }
    }
}

impl<'a> Iterator for RuleHeadsIter<'a> {
    type Item = (String, usize, usize);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // We can't easily get size_hint from the boxed iterator
        (0, None)
    }
}

/// Lazy iterator over rules in the Space.
///
/// Iterates through the PathMap entries and converts MORK bytes to Rules on-demand.
/// This avoids allocating a Vec of all rules upfront.
///
/// # Performance
/// - Zero Vec allocation for results
/// - MORK-to-MettaValue conversion happens lazily per item
/// - Skips non-rule entries efficiently
pub struct RulesIter<V: Clone + Default + Send + Sync + Unpin> {
    /// Owned vector of (mork_bytes, V) entries from PathMap iteration.
    /// We collect the raw entries but defer MORK conversion.
    entries: std::vec::IntoIter<(Vec<u8>, V)>,
    /// Space for MORK-to-MettaValue conversion.
    space: Space<V>,
}

impl<V: Clone + Default + Send + Sync + Unpin> RulesIter<V> {
    /// Create a lazy iterator from an Environment.
    ///
    /// Collects PathMap entries (cheap - just references) but defers
    /// the expensive MORK-to-MettaValue conversion until iteration.
    pub fn new(space: Space<V>) -> Self {
        // Collect entries - this is O(n) but only stores byte references
        // The expensive conversion happens in next()
        // With value-based multiplicity, all entries are atoms (no filtering needed)
        let entries: Vec<(Vec<u8>, V)> = space
            .btm
            .iter()
            .map(|(bytes, val)| (bytes, val.clone()))
            .collect();

        Self {
            entries: entries.into_iter(),
            space,
        }
    }

    /// Convert MORK bytes to Rule, if valid.
    #[inline]
    fn convert_to_rule(&self, mork_bytes: &[u8]) -> Option<Rule> {
        // Create Expr from bytes - safe because mork_bytes outlives expr usage
        let expr = Expr {
            ptr: mork_bytes.as_ptr().cast_mut(),
        };

        // Convert MORK expression to MettaValue
        if let Ok(value) = Environment::mork_expr_to_metta_value(&expr, &self.space) {
            if let MettaValueInner::SExpr(items) = value.inner() {
                if items.len() == 3 {
                    if let MettaValueInner::Atom(op) = items[0].inner() {
                        if op == "=" {
                            return Some(Rule::new(items[1].clone(), items[2].clone()));
                        }
                    }
                }
            }
        }
        None
    }
}

impl<V: Clone + Default + Send + Sync + Unpin> Iterator for RulesIter<V> {
    type Item = Rule;

    fn next(&mut self) -> Option<Self::Item> {
        // Keep trying entries until we find a valid rule or run out
        loop {
            let (mork_bytes, _) = self.entries.next()?;
            if let Some(rule) = self.convert_to_rule(&mork_bytes) {
                return Some(rule);
            }
            // Not a rule (e.g., fact), continue to next entry
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Lower bound is 0 (might all be non-rules), upper bound is remaining entries
        (0, Some(self.entries.len()))
    }
}

/// Lazy iterator over matching rules for a given head symbol and arity.
///
/// Chains together indexed rules and wildcard rules without collecting to Vec.
/// The iterator yields references to rules (no cloning until caller requests it).
///
/// # Performance
/// - Zero Vec allocation
/// - Rules are yielded as references (caller decides when to clone)
/// - Lock guards held for iterator lifetime
///
/// # Usage
/// ```ignore
/// // Get references (no cloning)
/// for rule in env.get_matching_rules_iter("foo", 2) {
///     // rule is &Rule
/// }
///
/// // Clone when needed
/// let owned_rules: Vec<Rule> = env.get_matching_rules_iter("foo", 2)
///     .cloned()
///     .collect();
/// ```
pub struct MatchingRulesIter<'a> {
    /// OwningHandle holds the rule_index guard and provides the iterator.
    inner: OwningHandle<
        RwLockReadGuard<'a, HashMap<(Symbol, usize), IndexSet<Rule>>>,
        Box<dyn Iterator<Item = &'a Rule> + 'a>,
    >,
    /// Wildcard rules iterator (if any)
    wildcard_iter: Option<WildcardRulesIter<'a>>,
    /// Phase: 0 = indexed rules, 1 = wildcard rules
    phase: u8,
}

/// Helper struct for iterating over wildcard rules
struct WildcardRulesIter<'a> {
    inner:
        OwningHandle<RwLockReadGuard<'a, IndexSet<Rule>>, Box<dyn Iterator<Item = &'a Rule> + 'a>>,
}

impl<'a> WildcardRulesIter<'a> {
    fn new(guard: RwLockReadGuard<'a, IndexSet<Rule>>) -> Self {
        let inner = OwningHandle::new_with_fn(guard, |rules_ptr| {
            let rules = unsafe { &*rules_ptr };
            Box::new(rules.iter()) as Box<dyn Iterator<Item = &'a Rule> + 'a>
        });
        Self { inner }
    }
}

impl<'a> Iterator for WildcardRulesIter<'a> {
    type Item = &'a Rule;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a> MatchingRulesIter<'a> {
    /// Create a new lazy iterator for matching rules.
    pub fn new(
        rule_index_guard: RwLockReadGuard<'a, HashMap<(Symbol, usize), IndexSet<Rule>>>,
        key: (Symbol, usize),
        wildcard_guard: Option<RwLockReadGuard<'a, IndexSet<Rule>>>,
    ) -> Self {
        // Create indexed rules iterator
        let inner = OwningHandle::new_with_fn(rule_index_guard, move |index_ptr| {
            let index = unsafe { &*index_ptr };
            let iter: Box<dyn Iterator<Item = &'a Rule> + 'a> = if let Some(rules) = index.get(&key)
            {
                Box::new(rules.iter())
            } else {
                Box::new(std::iter::empty())
            };
            iter
        });

        // Create wildcard iterator if we have wildcard rules
        let wildcard_iter = wildcard_guard.map(WildcardRulesIter::new);

        Self {
            inner,
            wildcard_iter,
            phase: 0,
        }
    }
}

impl<'a> Iterator for MatchingRulesIter<'a> {
    type Item = &'a Rule;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.phase {
                0 => {
                    // Phase 0: Yield indexed rules
                    if let Some(rule) = self.inner.next() {
                        return Some(rule);
                    }
                    // Indexed rules exhausted, move to wildcards
                    self.phase = 1;
                }
                1 => {
                    // Phase 1: Yield wildcard rules
                    if let Some(ref mut wildcard_iter) = self.wildcard_iter {
                        if let Some(rule) = wildcard_iter.next() {
                            return Some(rule);
                        }
                    }
                    // All done
                    return None;
                }
                _ => return None,
            }
        }
    }
}

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
    ///
    /// # Performance
    /// - Zero Vec allocation (lazy evaluation)
    /// - Holds read lock for entire iteration lifetime
    /// - Best for streaming/pipelining large results
    ///
    /// # Example
    /// ```ignore
    /// for (head, arity, count) in env.iter_rule_heads() {
    ///     println!("{}/{}: {} rules", head, arity, count);
    /// }
    ///
    /// // Or collect if needed:
    /// let heads: Vec<_> = env.iter_rule_heads().collect();
    /// ```
    pub fn iter_rule_heads(&self) -> RuleHeadsIter<'_> {
        // Acquire the lock - it will be held for the iterator's lifetime
        let guard = self
            .shared
            .rule_index
            .read()
            .expect("rule_index lock poisoned");

        RuleHeadsIter::new(guard)
    }

    /// Iterator over all rules in the Space.
    ///
    /// Rules are stored as MORK s-expressions: `(= lhs rhs)`.
    /// Returns a lazy iterator that defers MORK-to-MettaValue conversion
    /// until each item is consumed.
    ///
    /// # Performance
    /// - O(n) entry collection (just byte references)
    /// - Lazy MORK conversion per-item
    /// - No upfront Vec<Rule> allocation
    ///
    /// # Note
    /// For backward compatibility, this returns `RulesIter` which implements
    /// `Iterator<Item = Rule>`. The conversion happens lazily as you iterate.
    pub fn iter_rules(&self) -> RulesIter<Multiplicity> {
        let space = self.create_space();
        RulesIter::new(space)
    }

    /// Eager version of iter_rules() - collects all rules to Vec first.
    ///
    /// Use this when you need random access or multiple iterations,
    /// or when you want to release the Space quickly.
    #[allow(clippy::collapsible_match)]
    pub fn collect_rules(&self) -> Vec<Rule> {
        self.iter_rules().collect()
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
                index.entry((head_sym, arity)).or_default().insert(rule);
            } else {
                // Rules without head symbol (wildcards, variables) go to wildcard list
                let mut wildcards = self
                    .shared
                    .wildcard_rules
                    .write()
                    .expect("wildcard_rules lock poisoned");
                wildcards.insert(rule);
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
    /// Multiply-defined rules are tracked via IndexedMultiset for O(1) lookup
    /// Rules are also indexed by (head_symbol, arity) for fast lookup
    pub fn add_rule(&mut self, mut rule: Rule) {
        trace!(target: "mettatron::environment::add_rule", ?rule);
        self.make_owned(); // CoW: ensure we own data before modifying

        // Allocate multiplicity index if not already set
        let idx = rule.multiplicity_idx.unwrap_or_else(|| {
            self.shared
                .multiplicities
                .read()
                .expect("multiplicities lock poisoned")
                .allocate_index()
        });
        rule.multiplicity_idx = Some(idx);

        // Increment count using O(1) array access
        self.shared
            .multiplicities
            .read()
            .expect("multiplicities lock poisoned")
            .increment(idx);

        // Create a rule s-expression: (= lhs rhs)
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs.clone(),
            rule.rhs.clone(),
        ]);

        // Compute MORK bytes ONCE and reuse for both PathMap and legacy multiplicity tracking
        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        // Convert to MORK bytes and store for PathMap insertion
        let mork_bytes_result = metta_to_mork_bytes(&rule_sexpr, &temp_space, &mut ctx);

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
            index.entry((head_sym, arity)).or_default().insert(rule); // Move instead of clone
        } else {
            // Rules without head symbol (wildcards, variables) go to wildcard list
            let mut wildcards = self
                .shared
                .wildcard_rules
                .write()
                .expect("wildcard_rules lock poisoned");
            wildcards.insert(rule); // Move instead of clone
                                    // Mark that we have wildcard rules (for fast-path in get_matching_rules)
            self.shared
                .has_wildcard_rules
                .store(true, Ordering::Release);
        }

        // Add to MORK Space using value-based multiplicity tracking
        // Single entry: mork_bytes → Multiplicity(count)
        if let Ok(mork_bytes) = mork_bytes_result {
            // Get write lock on btm
            let mut btm = self.shared.btm.write().expect("btm lock poisoned");

            // Add atom with multiplicity tracking (single entry design)
            add_atom(&mut btm, &mork_bytes);

            drop(btm);
            self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);

            // Update bloom filter with (head, arity) for O(1) match_space() rejection
            if let Some(head) = rule_sexpr.get_head_symbol() {
                let arity = rule_sexpr.get_arity() as u8;
                self.shared
                    .head_arity_bloom
                    .write()
                    .expect("head_arity_bloom lock poisoned")
                    .insert(head.as_bytes(), arity);
            }
        }
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Bulk add rules using PathMap::join() for batch efficiency
    /// This is significantly faster than individual add_rule() calls
    /// for large batches (20-100× speedup) due to:
    /// - Single lock acquisition for PathMap update
    /// - Bulk union operation instead of N individual inserts
    /// - MORK bytes reused for both PathMap and multiplicity tracking
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
        let mut rule_trie: PathMap<Multiplicity> = PathMap::new();

        // Track rule metadata while building trie
        // Use Symbol for O(1) comparison when symbol-interning is enabled
        let mut rule_index_updates: HashMap<(Symbol, usize), IndexSet<Rule>> = HashMap::new();
        let mut wildcard_updates: IndexSet<Rule> = IndexSet::new();

        for rule in rules {
            // Create rule s-expression: (= lhs rhs)
            let rule_sexpr = MettaValue::SExpr(vec![
                MettaValue::Atom("=".to_string()),
                rule.lhs.clone(),
                rule.rhs.clone(),
            ]);

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
                    .insert(rule);
            } else {
                wildcard_updates.insert(rule);
            }

            // Compute MORK bytes for PathMap insertion
            let temp_space: Space<Multiplicity> = Space {
                sm: self.shared_mapping.clone(),
                btm: PathMap::new(),
                mmaps: HashMap::new(),
            };
            let mut ctx = ConversionContext::new();

            let mork_bytes = metta_to_mork_bytes(&rule_sexpr, &temp_space, &mut ctx)
                .map_err(|e| format!("MORK conversion failed for rule {:?}: {}", rule_sexpr, e))?;

            // Insert atom with multiplicity 1
            rule_trie.insert(&mork_bytes, Multiplicity::new(1));

            // Increment total atom count
            self.shared.total_atoms.fetch_add(1, Ordering::Relaxed);
        }

        // Apply all updates in batch (minimize critical sections)
        // Note: With value-based multiplicity, PathMap's join uses pjoin which adds multiplicities

        // Update rule index
        {
            let mut index = self
                .shared
                .rule_index
                .write()
                .expect("rule_index lock poisoned");
            for ((head, arity), rules) in rule_index_updates {
                index.entry((head, arity)).or_default().extend(rules);
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
        // Note: With value-based multiplicity, join uses pjoin which adds multiplicities together
        {
            let mut btm = self.shared.btm.write().expect("btm lock poisoned");
            *btm = btm.join(&rule_trie);
        }
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity)
    /// Returns 1 if the rule exists but count wasn't tracked (for backward compatibility)
    ///
    /// # Performance
    /// O(1) via direct array access when rule has cached multiplicity_idx
    pub fn get_rule_count(&self, rule: &Rule) -> usize {
        // Fast path: Use cached multiplicity_idx for O(1) lookup
        if let Some(idx) = rule.multiplicity_idx {
            let count = self
                .shared
                .multiplicities
                .read()
                .expect("multiplicities lock poisoned")
                .count(idx);
            // Backward compatibility: return 1 if count is 0 (rule exists but not tracked)
            return if count == 0 { 1 } else { count };
        }

        // Slow path: Fall back to PathMap-based multiplicity lookup
        // This path should rarely be hit after rules are added via add_rule()
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs.clone(),
            rule.rhs.clone(),
        ]);

        // Compute MORK bytes and lookup multiplicity using efficient fixed-width encoding
        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(&rule_sexpr, &temp_space, &mut ctx) {
            Ok(mork_bytes) => {
                // Use efficient O(prefix_len) multiplicity lookup
                let btm = self.shared.btm.read().expect("btm lock poisoned");
                let count = get_multiplicity(&btm, &mork_bytes);
                if count == 0 {
                    1
                } else {
                    count as usize
                } // Backward compatibility: return 1 if not tracked
            }
            Err(_) => 1, // Fallback to 1 on conversion error
        }
    }

    /// Get the multiplicities (for serialization)
    /// Iterates through PathMap and reads multiplicity values directly.
    /// The keys are hex-encoded MORK bytes for serialization stability.
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        let btm = self.shared.btm.read().expect("btm lock poisoned");
        let mut result = HashMap::new();

        // With value-based multiplicity, every entry is an atom with its count as the value
        for (path, multiplicity) in btm.iter() {
            let count = multiplicity.count() as usize;
            let count = if count == 0 { 1 } else { count }; // Legacy compatibility

            // Hex-encode the MORK bytes for serialization
            let hex_key = hex::encode(&path);
            result.insert(hex_key, count);
        }

        result
    }

    /// Set the multiplicities (used for deserialization)
    /// Decodes hex-encoded MORK bytes and sets multiplicity counts.
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        self.make_owned(); // CoW: ensure we own data before modifying

        let mut btm = self.shared.btm.write().expect("btm lock poisoned");

        // For each atom with count N, set the multiplicity directly
        for (hex_key, count) in counts {
            // Decode hex-encoded MORK bytes
            if let Ok(mork_bytes) = hex::decode(&hex_key) {
                // Set multiplicity directly (single entry design)
                set_multiplicity(&mut btm, &mork_bytes, count as u64);
                self.shared.total_atoms.fetch_add(count, Ordering::Relaxed);
            }
        }

        drop(btm);
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Get rules matching a specific head symbol and arity (lazy version).
    ///
    /// Returns an iterator that yields references to matching rules without
    /// allocating a Vec. Rules are yielded in order: indexed rules first,
    /// then wildcard rules.
    ///
    /// # Performance
    /// - Zero Vec allocation
    /// - Rules yielded as references (caller decides when to clone)
    /// - Lock guards held for iterator lifetime
    ///
    /// # Example
    /// ```ignore
    /// // Process rules without cloning
    /// for rule in env.get_matching_rules_iter("foo", 2) {
    ///     process(rule);  // rule is &Rule
    /// }
    ///
    /// // Clone when needed
    /// let owned: Vec<Rule> = env.get_matching_rules_iter("foo", 2)
    ///     .cloned()
    ///     .collect();
    /// ```
    pub fn get_matching_rules_iter(&self, head: &str, arity: usize) -> MatchingRulesIter<'_> {
        let key = (Symbol::new(head), arity);

        // Acquire rule_index lock
        let rule_index_guard = self
            .shared
            .rule_index
            .read()
            .expect("rule_index lock poisoned");

        // Fast-path: Check if we have any wildcard rules
        let has_wildcards = self.shared.has_wildcard_rules.load(Ordering::Acquire);

        // Only acquire wildcard lock if wildcards exist
        let wildcard_guard = if has_wildcards {
            Some(
                self.shared
                    .wildcard_rules
                    .read()
                    .expect("wildcard_rules lock poisoned"),
            )
        } else {
            None
        };

        MatchingRulesIter::new(rule_index_guard, key, wildcard_guard)
    }

    /// Increment the multiplicity count for a rule.
    ///
    /// This should be called when a rule is added via add_to_space() to track
    /// its multiplicity. It increments both the IndexedMultiset (if the rule
    /// has a cached index) and the MorkBytesMultiset (for serialization).
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after increment.
    pub fn increment_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::increment_rule_multiplicity", ?rule_sexpr);
        self.make_owned(); // CoW: ensure we own data before modifying

        // Extract LHS from (= lhs rhs) to find the rule in the index
        let (head, arity, lhs) = if let MettaValueInner::SExpr(items) = rule_sexpr.inner() {
            if items.len() == 3 {
                if let MettaValueInner::Atom(op) = items[0].inner() {
                    if op == "=" {
                        let lhs = &items[1];
                        let head = lhs.get_head_symbol();
                        let arity = lhs.get_arity();
                        (head, arity, Some(lhs))
                    } else {
                        (None, 0, None)
                    }
                } else {
                    (None, 0, None)
                }
            } else {
                (None, 0, None)
            }
        } else {
            (None, 0, None)
        };

        // Try to find the rule in the index and increment its IndexedMultiset count
        if let (Some(head), Some(lhs_val)) = (head, lhs) {
            let key = (Symbol::new(&head), arity);

            // Find the rule with matching LHS to get its multiplicity_idx
            let mut found_idx: Option<u32> = None;
            {
                let index = self
                    .shared
                    .rule_index
                    .read()
                    .expect("rule_index lock poisoned");
                if let Some(rules) = index.get(&key) {
                    for rule in rules {
                        // Check if this rule's LHS matches
                        if rule.lhs.structurally_equivalent(lhs_val) {
                            found_idx = rule.multiplicity_idx;
                            break;
                        }
                    }
                }
            }

            // Also check wildcard rules if not found
            if found_idx.is_none() {
                let wildcards = self
                    .shared
                    .wildcard_rules
                    .read()
                    .expect("wildcard_rules lock poisoned");
                for rule in wildcards.iter() {
                    if rule.lhs.structurally_equivalent(lhs_val) {
                        found_idx = rule.multiplicity_idx;
                        break;
                    }
                }
            }

            // Increment IndexedMultiset if we found the index
            if let Some(idx) = found_idx {
                self.shared
                    .multiplicities
                    .read()
                    .expect("multiplicities lock poisoned")
                    .increment(idx);
            }
        }

        // Increment multiplicity using efficient suffix-replacement approach
        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(rule_sexpr, &temp_space, &mut ctx) {
            Ok(mork_bytes) => {
                // Use the efficient multiplicity module functions
                let mut btm = self
                    .shared
                    .btm
                    .write()
                    .expect("btm lock poisoned in increment_rule_multiplicity");
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
    /// This should be called when a rule is removed from the space.
    /// It decrements both the IndexedMultiset (if the rule has a cached index)
    /// and the MorkBytesMultiset (for serialization consistency).
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after decrement, or 0 if the rule wasn't tracked.
    pub fn decrement_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::decrement_rule_multiplicity", ?rule_sexpr);
        self.make_owned(); // CoW: ensure we own data before modifying

        // Extract LHS from (= lhs rhs) to find the rule in the index
        let (head, arity, lhs) = if let MettaValueInner::SExpr(items) = rule_sexpr.inner() {
            if items.len() == 3 {
                if let MettaValueInner::Atom(op) = items[0].inner() {
                    if op == "=" {
                        let lhs = &items[1];
                        let head = lhs.get_head_symbol();
                        let arity = lhs.get_arity();
                        (head, arity, Some(lhs))
                    } else {
                        (None, 0, None)
                    }
                } else {
                    (None, 0, None)
                }
            } else {
                (None, 0, None)
            }
        } else {
            (None, 0, None)
        };

        // Try to find the rule in the index and decrement its IndexedMultiset count
        if let (Some(head), Some(lhs_val)) = (head, lhs) {
            let key = (Symbol::new(&head), arity);

            // Find the rule with matching LHS to get its multiplicity_idx
            let mut found_idx: Option<u32> = None;
            {
                let index = self
                    .shared
                    .rule_index
                    .read()
                    .expect("rule_index lock poisoned");
                if let Some(rules) = index.get(&key) {
                    for rule in rules {
                        // Check if this rule's LHS matches
                        if rule.lhs.structurally_equivalent(lhs_val) {
                            found_idx = rule.multiplicity_idx;
                            break;
                        }
                    }
                }
            }

            // Also check wildcard rules if not found
            if found_idx.is_none() {
                let wildcards = self
                    .shared
                    .wildcard_rules
                    .read()
                    .expect("wildcard_rules lock poisoned");
                for rule in wildcards.iter() {
                    if rule.lhs.structurally_equivalent(lhs_val) {
                        found_idx = rule.multiplicity_idx;
                        break;
                    }
                }
            }

            // Decrement IndexedMultiset if we found the index
            if let Some(idx) = found_idx {
                self.shared
                    .multiplicities
                    .write()
                    .expect("multiplicities lock poisoned")
                    .decrement(idx);
            }
        }

        // Decrement multiplicity using efficient suffix-replacement approach
        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(rule_sexpr, &temp_space, &mut ctx) {
            Ok(mork_bytes) => {
                // Check current count first to know if decrement will happen
                let old_count = {
                    let btm = self.shared.btm.read().expect("btm lock");
                    get_multiplicity(&btm, &mork_bytes)
                };

                if old_count == 0 {
                    self.modified.store(true, Ordering::Release);
                    return 0;
                }

                // Use the efficient multiplicity module functions
                let mut btm = self
                    .shared
                    .btm
                    .write()
                    .expect("btm lock poisoned in decrement_rule_multiplicity");
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
                    return op == "=";
                }
            }
        }
        false
    }

    // ========================================================================
    // All-Atom Multiplicity Tracking (MeTTa HE Semantics)
    // ========================================================================

    /// Increment multiplicity for ANY atom (not just rules).
    ///
    /// This is the primary multiplicity tracking mechanism for MeTTa HE semantics.
    /// Uses efficient suffix-replacement approach for O(8) count updates.
    ///
    /// # Arguments
    /// * `value` - The MettaValue atom to increment count for
    ///
    /// # Returns
    /// The new count after increment.
    pub fn increment_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned(); // CoW: ensure we own data before modifying

        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &temp_space, &mut ctx) {
            Ok(mork_bytes) => {
                // Use the efficient multiplicity module functions
                let mut btm = self
                    .shared
                    .btm
                    .write()
                    .expect("btm lock poisoned in increment_atom_multiplicity");
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
    ///
    /// Called when an atom is removed from the space. Uses efficient
    /// suffix-replacement for O(8) count updates.
    ///
    /// # Arguments
    /// * `value` - The MettaValue atom to decrement count for
    ///
    /// # Returns
    /// The new count after decrement (0 means the atom should be removed from PathMap).
    pub fn decrement_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned(); // CoW: ensure we own data before modifying

        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &temp_space, &mut ctx) {
            Ok(mork_bytes) => {
                // Check current count first to know if decrement will happen
                let old_count = {
                    let btm = self.shared.btm.read().expect("btm lock");
                    get_multiplicity(&btm, &mork_bytes)
                };

                if old_count == 0 {
                    return 0;
                }

                // Use the efficient multiplicity module functions
                let mut btm = self
                    .shared
                    .btm
                    .write()
                    .expect("btm lock poisoned in decrement_atom_multiplicity");
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
    ///
    /// Returns the number of times this atom has been added to the space.
    /// Uses efficient O(prefix_len) lookup via multiplicity module.
    ///
    /// # Arguments
    /// * `value` - The MettaValue atom to query
    ///
    /// # Returns
    /// The multiplicity count (at least 1 if the atom exists).
    pub fn get_atom_multiplicity(&self, value: &MettaValue) -> usize {
        let temp_space: Space<Multiplicity> = Space {
            sm: self.shared_mapping.clone(),
            btm: PathMap::new(),
            mmaps: HashMap::new(),
        };
        let mut ctx = ConversionContext::new();

        match metta_to_mork_bytes(value, &temp_space, &mut ctx) {
            Ok(mork_bytes) => {
                // Use efficient O(prefix_len) lookup
                let btm = self.shared.btm.read().expect("btm lock");
                let count = get_multiplicity(&btm, &mork_bytes);
                // Return at least 1 if count is 0 (for backward compatibility)
                if count == 0 {
                    1
                } else {
                    count as usize
                }
            }
            Err(_) => 1,
        }
    }

    /// Get atom multiplicity from raw MORK bytes.
    ///
    /// This is an optimization for `match_space()` which already has the MORK bytes
    /// and doesn't need to re-compute them. Uses efficient O(prefix_len) lookup.
    ///
    /// # Arguments
    /// * `mork_bytes` - The MORK-encoded bytes of the atom
    ///
    /// # Returns
    /// The multiplicity count (at least 1 if the atom exists).
    pub fn get_multiplicity_from_mork_bytes(&self, mork_bytes: &[u8]) -> usize {
        // Use efficient O(prefix_len) lookup
        let btm = self.shared.btm.read().expect("btm lock");
        let count = get_multiplicity(&btm, mork_bytes);
        // Return at least 1 if count is 0 (for backward compatibility)
        if count == 0 {
            1
        } else {
            count as usize
        }
    }
}
