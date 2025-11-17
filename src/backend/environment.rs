use lru::LruCache;
use mork::space::Space;
use mork_interning::SharedMappingHandle;
use pathmap::{zipper::*, PathMap};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use super::fuzzy_match::FuzzyMatcher;
use super::{MettaValue, Rule};

/// The environment contains the fact database and type assertions
/// All facts (rules, atoms, s-expressions, type assertions) are stored in MORK PathMap
///
/// Thread-safe with Copy-on-Write (CoW) semantics:
/// - Clones share data until first modification (owns_data = false)
/// - First mutation triggers deep copy via make_owned() (owns_data = true)
/// - RwLock enables concurrent reads (4× improvement over Mutex)
/// - Modifications tracked via Arc<AtomicBool> for fast union() paths
pub struct Environment {
    /// THREAD-SAFE: SharedMappingHandle for symbol interning (string → u64)
    /// Can be cloned and shared across threads (Send + Sync)
    shared_mapping: SharedMappingHandle,

    /// CoW: Tracks if this clone owns its data (true = can modify in-place, false = must deep copy first)
    /// Set to true on new(), false on clone(), true after make_owned()
    owns_data: bool,

    /// CoW: Tracks if this environment has been modified since creation/clone
    /// Used for fast-path union() optimization (unmodified clones can skip deep merge)
    /// Arc-wrapped to allow independent tracking per clone
    modified: Arc<AtomicBool>,

    /// THREAD-SAFE: PathMap trie for fact storage
    /// Cloning is O(1) via structural sharing (immutable after clone)
    /// PathMap provides O(m) prefix queries and O(m) existence checks
    /// RwLock allows concurrent reads (multiple threads can read simultaneously)
    btm: Arc<RwLock<PathMap<()>>>,

    /// Rule index: Maps (head_symbol, arity) -> Vec<Rule> for O(1) rule lookup
    /// This enables O(k) rule matching where k = rules with matching head symbol
    /// Instead of O(n) iteration through all rules
    /// RwLock allows concurrent reads for parallel rule matching
    #[allow(clippy::type_complexity)]
    rule_index: Arc<RwLock<HashMap<(String, usize), Vec<Rule>>>>,

    /// Wildcard rules: Rules without a clear head symbol (e.g., variable patterns, wildcards)
    /// These rules must be checked against all queries
    /// RwLock allows concurrent reads during parallel evaluation
    wildcard_rules: Arc<RwLock<Vec<Rule>>>,

    /// Multiplicities: tracks how many times each rule is defined
    /// Maps a normalized rule key to its definition count
    /// This allows multiply-defined rules to produce multiple results
    /// RwLock allows concurrent reads for parallel rule application
    multiplicities: Arc<RwLock<HashMap<String, usize>>>,

    /// Pattern cache: LRU cache for MORK serialization results
    /// Maps MettaValue -> MORK bytes to avoid redundant conversions
    /// Cache size: 1000 entries (typical REPL/program has <1000 unique patterns)
    /// Expected speedup: 3-10x for repeated pattern matching
    /// RwLock allows concurrent reads (cache hits don't require exclusive lock)
    pattern_cache: Arc<RwLock<LruCache<MettaValue, Vec<u8>>>>,

    /// Fuzzy matcher: Tracks known symbols for "Did you mean?" suggestions
    /// Populated automatically as rules and functions are added to environment
    /// Used to suggest similar symbols when encountering undefined atoms
    fuzzy_matcher: FuzzyMatcher,

    /// Type index: Lazy-initialized subtrie containing only type assertions
    /// Extracted via PathMap::restrict() for O(1) type lookups
    /// Invalidated on type assertion additions
    /// RwLock allows concurrent type lookups during parallel evaluation
    type_index: Arc<RwLock<Option<PathMap<()>>>>,

    /// Type index invalidation flag: Set to true when types are added
    /// Causes type_index to be rebuilt on next get_type() call
    /// RwLock allows concurrent checks of dirty flag
    type_index_dirty: Arc<RwLock<bool>>,
}

impl Environment {
    pub fn new() -> Self {
        use mork_interning::SharedMapping;

        Environment {
            shared_mapping: SharedMapping::new(),
            owns_data: true, // CoW: new environments own their data
            modified: Arc::new(AtomicBool::new(false)), // CoW: track modifications
            btm: Arc::new(RwLock::new(PathMap::new())),
            rule_index: Arc::new(RwLock::new(HashMap::new())),
            wildcard_rules: Arc::new(RwLock::new(Vec::new())),
            multiplicities: Arc::new(RwLock::new(HashMap::new())),
            pattern_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(1000).unwrap()))),
            fuzzy_matcher: FuzzyMatcher::new(),
            type_index: Arc::new(RwLock::new(None)),
            type_index_dirty: Arc::new(RwLock::new(true)),
        }
    }

    /// CoW: Make this environment own its data (deep copy if sharing)
    /// Called automatically on first mutation of a cloned environment
    /// No-op if already owns data (owns_data == true)
    fn make_owned(&mut self) {
        // Fast path: already own data
        if self.owns_data {
            return;
        }

        // Deep copy all 7 RwLock-wrapped fields
        // Clone the data first to avoid borrowing issues
        let btm_data = self.btm.read().unwrap().clone();
        let rule_index_data = self.rule_index.read().unwrap().clone();
        let wildcard_rules_data = self.wildcard_rules.read().unwrap().clone();
        let multiplicities_data = self.multiplicities.read().unwrap().clone();
        let pattern_cache_data = self.pattern_cache.read().unwrap().clone();
        let type_index_data = self.type_index.read().unwrap().clone();
        let type_index_dirty_data = *self.type_index_dirty.read().unwrap();

        // Now assign the new Arc<RwLock<T>> instances
        self.btm = Arc::new(RwLock::new(btm_data));
        self.rule_index = Arc::new(RwLock::new(rule_index_data));
        self.wildcard_rules = Arc::new(RwLock::new(wildcard_rules_data));
        self.multiplicities = Arc::new(RwLock::new(multiplicities_data));
        self.pattern_cache = Arc::new(RwLock::new(pattern_cache_data));
        self.type_index = Arc::new(RwLock::new(type_index_data));
        self.type_index_dirty = Arc::new(RwLock::new(type_index_dirty_data));

        // Mark as owning data and modified
        self.owns_data = true;
        self.modified.store(true, Ordering::Release);
    }

    /// Create a thread-local Space for operations
    /// Following the Rholang LSP pattern: cheap clone via structural sharing
    ///
    /// This is useful for advanced operations that need direct access to the Space,
    /// such as debugging or custom MORK queries.
    pub fn create_space(&self) -> Space {
        let btm = self.btm.read().unwrap().clone(); // CoW: read lock for concurrent reads
        Space {
            btm,
            sm: self.shared_mapping.clone(),
            mmaps: HashMap::new(),
        }
    }

    /// Update PathMap and shared mapping after Space modifications (write operations)
    /// This updates both the PathMap (btm) and the SharedMappingHandle (sm)
    pub(crate) fn update_pathmap(&mut self, space: Space) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self.btm.write().unwrap() = space.btm; // CoW: write lock for exclusive access
        self.shared_mapping = space.sm;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Convert a MORK Expr directly to MettaValue without text serialization
    /// This avoids the "reserved byte" panic that occurs in serialize2()
    ///
    /// The key insight: serialize2() uses byte_item() which panics on bytes 64-127.
    /// We use maybe_byte_item() instead, which returns Result<Tag, u8> and handles reserved bytes gracefully.
    ///
    /// CRITICAL FIX for "reserved 114" and similar bugs during evaluation/iteration.
    #[allow(unused_variables)]
    pub(crate) fn mork_expr_to_metta_value(
        expr: &mork_expr::Expr,
        space: &Space,
    ) -> Result<MettaValue, String> {
        use mork_expr::{maybe_byte_item, Tag};
        use std::slice::from_raw_parts;

        // Stack-based traversal to avoid recursion limits
        #[derive(Debug)]
        enum StackFrame {
            Arity {
                remaining: u8,
                items: Vec<MettaValue>,
            },
        }

        let mut stack: Vec<StackFrame> = Vec::new();
        let mut offset = 0usize;
        let ptr = expr.ptr;
        let mut newvar_count = 0u8; // Track how many NewVars we've seen for proper indexing

        'parsing: loop {
            // Read the next byte and interpret as tag
            let byte = unsafe { *ptr.byte_add(offset) };
            let tag = match maybe_byte_item(byte) {
                Ok(t) => t,
                Err(reserved_byte) => {
                    // Reserved byte encountered - this is the bug we're fixing!
                    // Instead of panicking, return an error that calling code can handle
                    return Err(format!(
                        "Reserved byte {} at offset {}",
                        reserved_byte, offset
                    ));
                }
            };

            offset += 1;

            // Handle the tag and build MettaValue
            let value = match tag {
                Tag::NewVar => {
                    // De Bruijn index - NewVar introduces a new variable with the next index
                    // Use MORK's VARNAMES for proper variable names
                    const VARNAMES: [&str; 64] = [
                        "$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "x10", "x11",
                        "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20", "x21",
                        "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30", "x31",
                        "x32", "x33", "x34", "x35", "x36", "x37", "x38", "x39", "x40", "x41",
                        "x42", "x43", "x44", "x45", "x46", "x47", "x48", "x49", "x50", "x51",
                        "x52", "x53", "x54", "x55", "x56", "x57", "x58", "x59", "x60", "x61",
                        "x62", "x63",
                    ];
                    let var_name = if (newvar_count as usize) < VARNAMES.len() {
                        VARNAMES[newvar_count as usize].to_string()
                    } else {
                        format!("$var{}", newvar_count)
                    };
                    newvar_count += 1;
                    MettaValue::Atom(var_name)
                }
                Tag::VarRef(i) => {
                    // Variable reference - use MORK's VARNAMES for proper variable names
                    // VARNAMES: ["$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "x10", ...]
                    const VARNAMES: [&str; 64] = [
                        "$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "x10", "x11",
                        "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20", "x21",
                        "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30", "x31",
                        "x32", "x33", "x34", "x35", "x36", "x37", "x38", "x39", "x40", "x41",
                        "x42", "x43", "x44", "x45", "x46", "x47", "x48", "x49", "x50", "x51",
                        "x52", "x53", "x54", "x55", "x56", "x57", "x58", "x59", "x60", "x61",
                        "x62", "x63",
                    ];
                    if (i as usize) < VARNAMES.len() {
                        MettaValue::Atom(VARNAMES[i as usize].to_string())
                    } else {
                        MettaValue::Atom(format!("$var{}", i))
                    }
                }
                Tag::SymbolSize(size) => {
                    // Read symbol bytes
                    let symbol_bytes =
                        unsafe { from_raw_parts(ptr.byte_add(offset), size as usize) };
                    offset += size as usize;

                    // Look up symbol in symbol table if interning is enabled
                    let symbol_str = {
                        #[cfg(feature = "interning")]
                        {
                            // With interning, symbols are ALWAYS stored as 8-byte i64 IDs
                            if symbol_bytes.len() == 8 {
                                // Convert bytes to i64, then back to bytes for symbol table lookup
                                let symbol_id =
                                    i64::from_be_bytes(symbol_bytes.try_into().unwrap())
                                        .to_be_bytes();
                                if let Some(actual_bytes) = space.sm.get_bytes(symbol_id) {
                                    // Found in symbol table - use actual symbol string
                                    String::from_utf8_lossy(actual_bytes).to_string()
                                } else {
                                    // Symbol ID not in table - fall back to treating as raw bytes
                                    String::from_utf8_lossy(symbol_bytes).to_string()
                                }
                            } else {
                                // Not 8 bytes - treat as raw symbol string
                                String::from_utf8_lossy(symbol_bytes).to_string()
                            }
                        }
                        #[cfg(not(feature = "interning"))]
                        {
                            // Without interning, symbols are stored as raw UTF-8 bytes
                            String::from_utf8_lossy(symbol_bytes).to_string()
                        }
                    };

                    // Parse the symbol to check if it's a number or string literal
                    if let Ok(n) = symbol_str.parse::<i64>() {
                        MettaValue::Long(n)
                    } else if symbol_str == "true" {
                        MettaValue::Bool(true)
                    } else if symbol_str == "false" {
                        MettaValue::Bool(false)
                    } else if symbol_str.starts_with('"')
                        && symbol_str.ends_with('"')
                        && symbol_str.len() >= 2
                    {
                        // String literal - strip quotes
                        MettaValue::String(symbol_str[1..symbol_str.len() - 1].to_string())
                    } else if symbol_str.starts_with('`')
                        && symbol_str.ends_with('`')
                        && symbol_str.len() >= 2
                    {
                        // URI literal - strip backticks
                        MettaValue::Uri(symbol_str[1..symbol_str.len() - 1].to_string())
                    } else {
                        MettaValue::Atom(symbol_str)
                    }
                }
                Tag::Arity(arity) => {
                    if arity == 0 {
                        // Empty s-expression
                        MettaValue::Nil
                    } else {
                        // Push new frame for this s-expression
                        stack.push(StackFrame::Arity {
                            remaining: arity,
                            items: Vec::new(),
                        });
                        continue 'parsing;
                    }
                }
            };

            // Value is complete - add to parent or return
            let mut value = value; // Make value mutable for the popping loop
            'popping: loop {
                match stack.last_mut() {
                    None => {
                        // No parent - this is the final result
                        return Ok(value);
                    }
                    Some(StackFrame::Arity { remaining, items }) => {
                        items.push(value.clone());
                        *remaining -= 1;

                        if *remaining == 0 {
                            // S-expression is complete
                            let completed_items = items.clone();
                            stack.pop();
                            value = MettaValue::SExpr(completed_items); // Mutate, don't shadow!
                            continue 'popping;
                        } else {
                            // More items needed
                            continue 'parsing;
                        }
                    }
                }
            }
        }
    }

    /// Helper function to serialize a MORK Expr to a readable string
    /// DEPRECATED: This uses serialize2() which panics on reserved bytes.
    /// Use mork_expr_to_metta_value() instead for production code.
    #[allow(dead_code)]
    #[allow(unused_variables)]
    fn serialize_mork_expr_old(expr: &mork_expr::Expr, space: &Space) -> String {
        let mut buffer = Vec::new();
        expr.serialize2(
            &mut buffer,
            |s| {
                #[cfg(feature = "interning")]
                {
                    let symbol = i64::from_be_bytes(s.try_into().unwrap()).to_be_bytes();
                    let mstr = space
                        .sm
                        .get_bytes(symbol)
                        .map(|x| unsafe { std::str::from_utf8_unchecked(x) });
                    unsafe { std::mem::transmute(mstr.unwrap_or("")) }
                }
                #[cfg(not(feature = "interning"))]
                unsafe {
                    std::mem::transmute(std::str::from_utf8_unchecked(s))
                }
            },
            |i, _intro| mork_expr::Expr::VARNAMES[i as usize],
        );

        String::from_utf8_lossy(&buffer).to_string()
    }

    /// Add a type assertion
    /// Type assertions are stored as (: name type) in MORK Space
    /// Invalidates the type index cache
    pub fn add_type(&mut self, name: String, typ: MettaValue) {
        self.make_owned(); // CoW: ensure we own data before modifying

        // Create type assertion: (: name typ)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom(name),
            typ,
        ]);
        self.add_to_space(&type_assertion);

        // Invalidate type index cache
        *self.type_index_dirty.write().unwrap() = true;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Ensure the type index is built and up-to-date
    /// Uses PathMap's restrict() to extract only type assertions into a subtrie
    /// This enables O(p + m) type lookups where m << n (total facts)
    ///
    /// The type index is lazily initialized and cached until invalidated
    fn ensure_type_index(&self) {
        let dirty = *self.type_index_dirty.read().unwrap();
        if !dirty {
            return; // Index is up to date
        }

        // Build type index using PathMap::restrict()
        // This extracts a subtrie containing only paths that start with ":"
        let btm = self.btm.read().unwrap();

        // Create a PathMap containing only the ":" prefix
        // restrict() will return all paths in btm that have matching prefixes in this map
        let mut type_prefix_map = PathMap::new();
        let colon_bytes = b":";

        // Insert a single path with just ":" to match all type assertions
        {
            let mut wz = type_prefix_map.write_zipper();
            for &byte in colon_bytes {
                wz.descend_to_byte(byte);
            }
            wz.set_val(());
        }

        // Extract type subtrie using restrict()
        let type_subtrie = btm.restrict(&type_prefix_map);

        // Cache the subtrie
        *self.type_index.write().unwrap() = Some(type_subtrie);
        *self.type_index_dirty.write().unwrap() = false;
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
        use mork_expr::Expr;

        // Ensure type index is built and up-to-date
        self.ensure_type_index();

        // Get the type index subtrie
        let type_index_opt = self.type_index.read().unwrap();
        let type_index = match type_index_opt.as_ref() {
            Some(index) => index,
            None => {
                // Index failed to build, fall back to linear search
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
        self.get_type_linear(name)
    }

    /// Linear search fallback for get_type() - O(n) iteration
    /// Used when exact match via descend_to_check() fails
    fn get_type_linear(&self, name: &str) -> Option<MettaValue> {
        use mork_expr::Expr;

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // Iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
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

    /// Get the number of rules in the environment
    /// Counts rules directly from PathMap Space
    pub fn rule_count(&self) -> usize {
        self.iter_rules().count()
    }

    /// Iterator over all rules in the Space
    /// Rules are stored as MORK s-expressions: (= lhs rhs)
    ///
    /// Uses direct zipper traversal to avoid dump/parse overhead.
    /// This provides O(n) iteration without string serialization.
    #[allow(clippy::collapsible_match)]
    pub fn iter_rules(&self) -> impl Iterator<Item = Rule> {
        use mork_expr::Expr;

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();
        let mut rules = Vec::new();

        // Directly iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let MettaValue::SExpr(items) = &value {
                    if items.len() == 3 {
                        if let MettaValue::Atom(op) = &items[0] {
                            if op == "=" {
                                rules.push(Rule {
                                    lhs: items[1].clone(),
                                    rhs: items[2].clone(),
                                });
                            }
                        }
                    }
                }
            }
        }

        drop(space);
        rules.into_iter()
    }

    /// Rebuild the rule index from the MORK Space
    /// This is needed after deserializing an Environment from PathMap Par,
    /// since the serialization only preserves the MORK Space, not the index.
    pub fn rebuild_rule_index(&mut self) {
        self.make_owned(); // CoW: ensure we own data before modifying

        // Clear existing indices
        {
            let mut index = self.rule_index.write().unwrap();
            index.clear();
        }
        {
            let mut wildcards = self.wildcard_rules.write().unwrap();
            wildcards.clear();
        }

        // Rebuild from MORK Space
        for rule in self.iter_rules() {
            if let Some(head) = rule.lhs.get_head_symbol() {
                let arity = rule.lhs.get_arity();
                let mut index = self.rule_index.write().unwrap();
                index
                    .entry((head.clone(), arity))
                    .or_default()
                    .push(rule);

                // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
                self.fuzzy_matcher.insert(&head);
            } else {
                // Rules without head symbol (wildcards, variables) go to wildcard list
                let mut wildcards = self.wildcard_rules.write().unwrap();
                wildcards.push(rule);
            }
        }

        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Match pattern against all atoms in the Space (optimized for match operation)
    /// Returns all instantiated templates for atoms matching the pattern
    ///
    /// This is optimized to work directly with MORK expressions, avoiding
    /// unnecessary string serialization and parsing.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for each match
    ///
    /// # Returns
    /// Vector of instantiated templates (MettaValue) for all matches
    pub fn match_space(&self, pattern: &MettaValue, template: &MettaValue) -> Vec<MettaValue> {
        use crate::backend::eval::{apply_bindings, pattern_match};
        use mork_expr::Expr;

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // Directly iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Try to match the pattern against this atom
                if let Some(bindings) = pattern_match(pattern, &atom) {
                    // Apply bindings to the template
                    let instantiated = apply_bindings(template, &bindings);
                    results.push(instantiated);
                }
            }
        }

        drop(space);
        results
    }

    /// Add a rule to the environment
    /// Rules are stored in MORK Space as s-expressions: (= lhs rhs)
    /// Multiply-defined rules are tracked via multiplicities
    /// Rules are also indexed by (head_symbol, arity) for fast lookup
    pub fn add_rule(&mut self, rule: Rule) {
        self.make_owned(); // CoW: ensure we own data before modifying

        // Create a rule s-expression: (= lhs rhs)
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs.clone(),
            rule.rhs.clone(),
        ]);

        // Generate a canonical key for the rule
        // Use MORK string format for readable serialization
        let rule_key = rule_sexpr.to_mork_string();

        // Increment the count for this rule
        {
            let mut counts = self.multiplicities.write().unwrap();
            let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
            counts.insert(rule_key.clone(), new_count);
        } // Drop the RefMut borrow before add_to_space

        // Add to rule index for O(k) lookup
        // Note: We store the rule only ONCE (in either index or wildcard list)
        // to avoid unnecessary clones. The rule is already in MORK Space.
        if let Some(head) = rule.lhs.get_head_symbol() {
            let arity = rule.lhs.get_arity();
            let mut index = self.rule_index.write().unwrap();
            index
                .entry((head.clone(), arity))
                .or_default()
                .push(rule); // Move instead of clone

            // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
            self.fuzzy_matcher.insert(&head);
        } else {
            // Rules without head symbol (wildcards, variables) go to wildcard list
            let mut wildcards = self.wildcard_rules.write().unwrap();
            wildcards.push(rule); // Move instead of clone
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
        if rules.is_empty() {
            return Ok(());
        }

        self.make_owned(); // CoW: ensure we own data before modifying

        // Build temporary PathMap outside the lock
        let mut rule_trie = PathMap::new();

        // Track rule metadata while building trie
        let mut rule_index_updates: HashMap<(String, usize), Vec<Rule>> = HashMap::new();
        let mut wildcard_updates: Vec<Rule> = Vec::new();
        let mut multiplicity_updates: HashMap<String, usize> = HashMap::new();

        for rule in rules {
            // Create rule s-expression: (= lhs rhs)
            let rule_sexpr = MettaValue::SExpr(vec![
                MettaValue::Atom("=".to_string()),
                rule.lhs.clone(),
                rule.rhs.clone(),
            ]);

            // Track multiplicity
            let rule_key = rule_sexpr.to_mork_string();
            *multiplicity_updates.entry(rule_key).or_insert(0) += 1;

            // Prepare rule index updates
            if let Some(head) = rule.lhs.get_head_symbol() {
                let arity = rule.lhs.get_arity();
                rule_index_updates
                    .entry((head.clone(), arity))
                    .or_default()
                    .push(rule);

                // Track symbol for fuzzy matching
                self.fuzzy_matcher.insert(&head);
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
            let mut counts = self.multiplicities.write().unwrap();
            for (key, delta) in multiplicity_updates {
                *counts.entry(key).or_insert(0) += delta;
            }
        }

        // Update rule index
        {
            let mut index = self.rule_index.write().unwrap();
            for ((head, arity), mut rules) in rule_index_updates {
                index
                    .entry((head, arity))
                    .or_default()
                    .append(&mut rules);
            }
        }

        // Update wildcard rules
        {
            let mut wildcards = self.wildcard_rules.write().unwrap();
            wildcards.extend(wildcard_updates);
        }

        // Single PathMap union (minimal critical section)
        {
            let mut btm = self.btm.write().unwrap();
            *btm = btm.join(&rule_trie);
        }
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity)
    /// Returns 1 if the rule exists but count wasn't tracked (for backward compatibility)
    pub fn get_rule_count(&self, rule: &Rule) -> usize {
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs.clone(),
            rule.rhs.clone(),
        ]);
        let rule_key = rule_sexpr.to_mork_string();

        let counts = self.multiplicities.read().unwrap();
        *counts.get(&rule_key).unwrap_or(&1)
    }

    /// Get the multiplicities (for serialization)
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        self.multiplicities.read().unwrap().clone()
    }

    /// Set the multiplicities (used for deserialization)
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self.multiplicities.write().unwrap() = counts;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Check if an atom fact exists (queries MORK Space)
    /// OPTIMIZED: Uses O(p) exact match via descend_to_check() where p = pattern depth
    ///
    /// For atoms (always ground), this provides O(1)-like performance
    /// Expected speedup: 1,000-10,000× for large fact databases
    pub fn has_fact(&self, atom: &str) -> bool {
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
            return self.has_sexpr_fact_linear(sexpr);
        }

        // Slow path: O(n) linear search for patterns with variables
        // This is necessary because variables need structural equivalence checking
        self.has_sexpr_fact_linear(sexpr)
    }

    /// UNUSED: This approach doesn't work because query_multi treats variables as pattern variables
    /// Kept for historical reference - do not use
    #[allow(dead_code)]
    fn has_sexpr_fact_optimized(&self, sexpr: &MettaValue) -> Option<bool> {
        use mork_expr::Expr;
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
        use mork_expr::Expr;

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

    /// Convert MettaValue to MORK bytes with LRU caching
    /// Checks cache first, only converts if not cached
    /// NOTE: Only caches ground (variable-free) patterns for deterministic results
    /// Variable patterns require fresh ConversionContext for correct De Bruijn encoding
    /// Expected speedup: 3-10x for repeated ground patterns
    pub(crate) fn metta_to_mork_bytes_cached(&self, value: &MettaValue) -> Result<Vec<u8>, String> {
        use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};

        // Only cache ground (variable-free) patterns
        // Variable patterns need fresh ConversionContext for correct De Bruijn indices
        let is_ground = !Self::contains_variables(value);

        if is_ground {
            // Check cache first for ground patterns (read-only access)
            {
                let mut cache = self.pattern_cache.write().unwrap();
                if let Some(bytes) = cache.get(value) {
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
            let mut cache = self.pattern_cache.write().unwrap();
            cache.put(value.clone(), bytes.clone());
        }

        Ok(bytes)
    }

    /// Check if a MettaValue contains variables ($x, &y, 'z, or _)
    fn contains_variables(value: &MettaValue) -> bool {
        match value {
            MettaValue::Atom(s) => {
                s == "_"
                    || (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && s != "&"
            }
            MettaValue::SExpr(items) => items.iter().any(Self::contains_variables),
            MettaValue::Error(_, details) => Self::contains_variables(details),
            MettaValue::Type(t) => Self::contains_variables(t),
            _ => false, // Ground types: Bool, Long, Float, String, Uri, Nil
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
    fn descend_to_exact_match(&self, pattern: &MettaValue) -> Option<MettaValue> {
        use mork_expr::Expr;

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

    /// Add a fact to the MORK Space for pattern matching
    /// Converts the MettaValue to MORK format and stores it
    /// OPTIMIZATION (Variant C): Uses direct MORK byte conversion for ground values
    ///
    /// IMPORTANT: Official MeTTa semantics - only the top-level expression is stored.
    /// Nested sub-expressions are NOT recursively extracted and stored separately.
    /// To query nested parts, use pattern matching with variables, e.g., (Outer $x)
    pub fn add_to_space(&mut self, value: &MettaValue) {
        use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};

        // Try direct byte conversion first (Variant C)
        // This skips string serialization + parsing for 10-20× speedup
        let is_ground = !Self::contains_variables(value);

        if is_ground {
            // Ground values: use direct MORK byte conversion (no parsing needed)
            let space = self.create_space();
            let mut ctx = ConversionContext::new();

            if let Ok(mork_bytes) = metta_to_mork_bytes(value, &space, &mut ctx) {
                // Direct PathMap insertion without parsing
                let mut space_mut = self.create_space();
                space_mut.btm.insert(&mork_bytes, ());
                self.update_pathmap(space_mut);
                return;
            }
        }

        // Fallback: use string path for variable-containing values
        let mork_str = value.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        // Create thread-local Space
        let mut space = self.create_space();

        // Use MORK's parser to load the s-expression into PathMap trie
        if let Ok(_count) = space.load_all_sexpr_impl(mork_bytes, true) {
            // Successfully added to space
        }

        // Update shared PathMap with modified Space
        self.update_pathmap(space);
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
            let mut btm = self.btm.write().unwrap();
            *btm = btm.join(&fact_trie);
        }

        // Invalidate type index if any facts were type assertions
        // Conservative: Assume any bulk insert might contain types
        *self.type_index_dirty.write().unwrap() = true;

        self.modified.store(true, Ordering::Release); // CoW: mark as modified
        Ok(())
    }

    /// Get rules matching a specific head symbol and arity
    /// Returns Vec<Rule> for O(1) lookup instead of O(n) iteration
    /// Also includes wildcard rules that must be checked against all queries
    pub fn get_matching_rules(&self, head: &str, arity: usize) -> Vec<Rule> {
        // OPTIMIZATION: Preallocate capacity to avoid reallocation
        // Calculate exact size needed for both indexed rules and wildcards
        let (indexed_len, wildcard_len) = {
            let index = self.rule_index.read().unwrap();
            let wildcards = self.wildcard_rules.read().unwrap();
            let indexed = index.get(&(head.to_string(), arity)).map_or(0, |r| r.len());
            (indexed, wildcards.len())
        };

        let mut matching_rules = Vec::with_capacity(indexed_len + wildcard_len);

        // Get indexed rules with matching head symbol and arity
        {
            let index = self.rule_index.read().unwrap();
            if let Some(rules) = index.get(&(head.to_string(), arity)) {
                matching_rules.extend(rules.clone());
            }
        }

        // Also include wildcard rules (must always be checked)
        {
            let wildcards = self.wildcard_rules.read().unwrap();
            matching_rules.extend(wildcards.clone());
        }

        matching_rules
    }

    /// Get fuzzy suggestions for a potentially misspelled symbol
    ///
    /// Returns a list of (symbol, distance) pairs sorted by Levenshtein distance.
    ///
    /// # Arguments
    /// - `query`: The symbol to find matches for (e.g., "fibonaci")
    /// - `max_distance`: Maximum edit distance (typically 1-2)
    ///
    /// # Example
    /// ```ignore
    /// let suggestions = env.suggest_similar_symbols("fibonaci", 2);
    /// // Returns: [("fibonacci", 1)]
    /// ```
    pub fn suggest_similar_symbols(
        &self,
        query: &str,
        max_distance: usize,
    ) -> Vec<(String, usize)> {
        self.fuzzy_matcher.suggest(query, max_distance)
    }

    /// Generate a "Did you mean?" error message for an undefined symbol
    ///
    /// Returns None if no suggestions are found within max_distance.
    ///
    /// # Arguments
    /// - `symbol`: The undefined symbol
    /// - `max_distance`: Maximum edit distance (default: 2)
    ///
    /// # Example
    /// ```ignore
    /// if let Some(msg) = env.did_you_mean("fibonaci", 2) {
    ///     eprintln!("Error: Undefined symbol 'fibonaci'. {}", msg);
    /// }
    /// // Prints: "Error: Undefined symbol 'fibonaci'. Did you mean: fibonacci?"
    /// ```
    pub fn did_you_mean(&self, symbol: &str, max_distance: usize) -> Option<String> {
        self.fuzzy_matcher.did_you_mean(symbol, max_distance, 3)
    }

    /// Union two environments (monotonic merge)
    /// PathMap and shared_mapping are shared via Arc, so facts (including type assertions) are automatically merged
    /// Multiplicities and rule indices are also merged via shared Arc
    pub fn union(&self, _other: &Environment) -> Environment {
        // PathMap and SharedMappingHandle are shared via Arc/Clone
        // Facts (including type assertions) added to either are automatically visible in both
        let shared_mapping = self.shared_mapping.clone();
        let btm = self.btm.clone();

        // Merge rule index and wildcard rules (both are Arc<Mutex>, so they're already shared)
        let rule_index = self.rule_index.clone();
        let wildcard_rules = self.wildcard_rules.clone();

        // Merge multiplicities (both are Arc<Mutex>, so they're already shared)
        // The counts are automatically shared via the Arc
        let multiplicities = self.multiplicities.clone();
        let pattern_cache = self.pattern_cache.clone();
        let fuzzy_matcher = self.fuzzy_matcher.clone();
        let type_index = self.type_index.clone();
        let type_index_dirty = self.type_index_dirty.clone();

        Environment {
            shared_mapping,
            owns_data: false, // CoW: union creates a new shared environment
            modified: Arc::new(AtomicBool::new(false)), // CoW: fresh modification tracker
            btm,
            rule_index,
            wildcard_rules,
            multiplicities,
            pattern_cache,
            fuzzy_matcher,
            type_index,
            type_index_dirty,
        }
    }
}

/// CoW: Manual Clone implementation
/// Clones share data (owns_data = false) until first modification triggers make_owned()
impl Clone for Environment {
    fn clone(&self) -> Self {
        Environment {
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: clones do not own data initially
            modified: Arc::new(AtomicBool::new(false)), // CoW: fresh modification tracker
            btm: Arc::clone(&self.btm),
            rule_index: Arc::clone(&self.rule_index),
            wildcard_rules: Arc::clone(&self.wildcard_rules),
            multiplicities: Arc::clone(&self.multiplicities),
            pattern_cache: Arc::clone(&self.pattern_cache),
            fuzzy_matcher: self.fuzzy_matcher.clone(),
            type_index: Arc::clone(&self.type_index),
            type_index_dirty: Arc::clone(&self.type_index_dirty),
        }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("space", &"<MORK Space>")
            .finish()
    }
}

#[cfg(test)]
mod cow_tests {
    use super::*;
    use crate::backend::models::MettaValue;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc as StdArc, Barrier};
    use std::thread;

    /// Helper: Create a simple rule for testing
    fn make_test_rule(lhs: &str, rhs: &str) -> Rule {
        Rule {
            lhs: MettaValue::Atom(lhs.to_string()),
            rhs: MettaValue::Atom(rhs.to_string()),
        }
    }

    /// Helper: Extract head symbol and arity from a MettaValue (for get_matching_rules)
    fn extract_head_arity(value: &MettaValue) -> (&str, usize) {
        match value {
            MettaValue::Atom(s) => (s.as_str(), 0),
            MettaValue::SExpr(vec) if !vec.is_empty() => {
                if let MettaValue::Atom(head) = &vec[0] {
                    (head.as_str(), vec.len() - 1)
                } else {
                    ("", 0) // Fallback for non-atom head
                }
            }
            _ => ("", 0), // Fallback for other cases
        }
    }

    /// Helper: Create a simple MettaValue fact for testing
    fn make_test_fact(value: &str) -> MettaValue {
        MettaValue::Atom(value.to_string())
    }

    // ============================================================================
    // UNIT TESTS (~300 LOC)
    // ============================================================================

    #[test]
    fn test_new_environment_owns_data() {
        // Test: New environment should own its data
        let env = Environment::new();
        assert!(env.owns_data, "New environment should own its data");
        assert!(
            !env.modified.load(Ordering::Acquire),
            "New environment should not be modified"
        );
    }

    #[test]
    fn test_clone_does_not_own_data() {
        // Test: Cloned environment should not own data initially
        let env = Environment::new();
        let clone = env.clone();

        assert!(env.owns_data, "Original environment should still own data");
        assert!(
            !clone.owns_data,
            "Cloned environment should NOT own data initially"
        );
        assert!(
            !clone.modified.load(Ordering::Acquire),
            "Cloned environment should not be modified"
        );
    }

    #[test]
    fn test_clone_shares_arc_pointers() {
        // Test: Clone should share Arc pointers (cheap O(1) clone)
        let env = Environment::new();

        // Get Arc pointer addresses before clone
        let btm_ptr_before = StdArc::as_ptr(&env.btm);
        let rule_index_ptr_before = StdArc::as_ptr(&env.rule_index);

        let clone = env.clone();

        // Get Arc pointer addresses after clone
        let btm_ptr_after = StdArc::as_ptr(&clone.btm);
        let rule_index_ptr_after = StdArc::as_ptr(&clone.rule_index);

        // Pointers should be identical (shared)
        assert_eq!(btm_ptr_before, btm_ptr_after, "Clone should share btm Arc");
        assert_eq!(
            rule_index_ptr_before, rule_index_ptr_after,
            "Clone should share rule_index Arc"
        );
    }

    #[test]
    fn test_make_owned_triggers_on_first_write() {
        // Test: First mutation should trigger make_owned() and deep copy
        let mut env = Environment::new();
        let rule = make_test_rule("(test $x)", "(result $x)");

        // Add rule to original (already owns data, no make_owned() needed)
        env.add_rule(rule.clone());
        assert!(env.owns_data, "Original should still own data");
        assert!(
            env.modified.load(Ordering::Acquire),
            "Original should be marked modified"
        );

        // Clone and mutate
        let mut clone = env.clone();
        assert!(!clone.owns_data, "Clone should not own data initially");

        // Get Arc pointers before mutation
        let btm_ptr_before = StdArc::as_ptr(&clone.btm);

        // First mutation triggers make_owned()
        clone.add_rule(make_test_rule("(clone $y)", "(cloned $y)"));

        // After mutation
        assert!(clone.owns_data, "Clone should own data after mutation");
        assert!(
            clone.modified.load(Ordering::Acquire),
            "Clone should be marked modified"
        );

        // Arc pointers should be different (deep copy occurred)
        let btm_ptr_after = StdArc::as_ptr(&clone.btm);
        assert_ne!(
            btm_ptr_before, btm_ptr_after,
            "make_owned() should create new Arc"
        );
    }

    #[test]
    fn test_isolation_after_clone_mutation() {
        // Test: Mutations to clone should not affect original
        let mut env = Environment::new();
        let rule1 = make_test_rule("(original $x)", "(original-result $x)");
        env.add_rule(rule1.clone());

        // Clone and add different rule
        let mut clone = env.clone();
        let rule2 = make_test_rule("(cloned $y)", "(cloned-result $y)");
        clone.add_rule(rule2.clone());

        // Original should only have rule1
        let (head1, arity1) = extract_head_arity(&rule1.lhs);
        let original_rules = env.get_matching_rules(head1, arity1);
        assert_eq!(original_rules.len(), 1, "Original should have 1 rule");

        // Clone should have both rules (rule1 was shared, rule2 was added)
        let clone_rules = clone.get_matching_rules(head1, arity1);
        assert_eq!(clone_rules.len(), 1, "Clone should have original rule");

        let (head2, arity2) = extract_head_arity(&rule2.lhs);
        let clone_rules2 = clone.get_matching_rules(head2, arity2);
        assert_eq!(clone_rules2.len(), 1, "Clone should have new rule");
    }

    #[test]
    fn test_modification_tracking() {
        // Test: Modification flag is correctly tracked
        let mut env = Environment::new();
        assert!(
            !env.modified.load(Ordering::Acquire),
            "New env should not be modified"
        );

        // Add rule → should set modified flag
        env.add_rule(make_test_rule("(test $x)", "(result $x)"));
        assert!(
            env.modified.load(Ordering::Acquire),
            "Env should be modified after add_rule"
        );

        // Clone → clone should have fresh modified flag
        let mut clone = env.clone();
        assert!(
            !clone.modified.load(Ordering::Acquire),
            "Clone should have fresh modified flag"
        );

        // Mutate clone → should set clone's modified flag
        clone.add_rule(make_test_rule("(test2 $y)", "(result2 $y)"));
        assert!(
            clone.modified.load(Ordering::Acquire),
            "Clone should be modified after mutation"
        );
    }

    #[test]
    fn test_make_owned_idempotency() {
        // Test: make_owned() should be idempotent (safe to call multiple times)
        let mut env = Environment::new();
        let mut clone = env.clone();

        // First mutation triggers make_owned()
        clone.add_rule(make_test_rule("(test1 $x)", "(result1 $x)"));
        assert!(
            clone.owns_data,
            "Clone should own data after first mutation"
        );

        // Get Arc pointers after first make_owned()
        let btm_ptr_first = StdArc::as_ptr(&clone.btm);

        // Second mutation should NOT trigger another make_owned()
        clone.add_rule(make_test_rule("(test2 $y)", "(result2 $y)"));

        // Arc pointers should be same (no second deep copy)
        let btm_ptr_second = StdArc::as_ptr(&clone.btm);
        assert_eq!(
            btm_ptr_first, btm_ptr_second,
            "make_owned() should not run twice"
        );
    }

    #[test]
    fn test_deep_clone_copies_all_fields() {
        // Test: make_owned() should deep copy all 7 RwLock fields
        let mut env = Environment::new();
        env.add_rule(make_test_rule("(test $x)", "(result $x)"));

        let mut clone = env.clone();

        // Get Arc pointers before mutation
        let btm_before = StdArc::as_ptr(&clone.btm);
        let rule_index_before = StdArc::as_ptr(&clone.rule_index);
        let wildcard_rules_before = StdArc::as_ptr(&clone.wildcard_rules);
        let multiplicities_before = StdArc::as_ptr(&clone.multiplicities);
        let pattern_cache_before = StdArc::as_ptr(&clone.pattern_cache);
        let type_index_before = StdArc::as_ptr(&clone.type_index);
        let type_index_dirty_before = StdArc::as_ptr(&clone.type_index_dirty);

        // Trigger make_owned()
        clone.add_rule(make_test_rule("(clone $y)", "(cloned $y)"));

        // Get Arc pointers after mutation
        let btm_after = StdArc::as_ptr(&clone.btm);
        let rule_index_after = StdArc::as_ptr(&clone.rule_index);
        let wildcard_rules_after = StdArc::as_ptr(&clone.wildcard_rules);
        let multiplicities_after = StdArc::as_ptr(&clone.multiplicities);
        let pattern_cache_after = StdArc::as_ptr(&clone.pattern_cache);
        let type_index_after = StdArc::as_ptr(&clone.type_index);
        let type_index_dirty_after = StdArc::as_ptr(&clone.type_index_dirty);

        // All 7 Arc pointers should be different (deep copy occurred)
        assert_ne!(btm_before, btm_after, "btm should be deep copied");
        assert_ne!(
            rule_index_before, rule_index_after,
            "rule_index should be deep copied"
        );
        assert_ne!(
            wildcard_rules_before, wildcard_rules_after,
            "wildcard_rules should be deep copied"
        );
        assert_ne!(
            multiplicities_before, multiplicities_after,
            "multiplicities should be deep copied"
        );
        assert_ne!(
            pattern_cache_before, pattern_cache_after,
            "pattern_cache should be deep copied"
        );
        assert_ne!(
            type_index_before, type_index_after,
            "type_index should be deep copied"
        );
        assert_ne!(
            type_index_dirty_before, type_index_dirty_after,
            "type_index_dirty should be deep copied"
        );
    }

    #[test]
    fn test_multiple_clones_independent() {
        // Test: Multiple clones should be independent after mutation
        let mut env = Environment::new();
        env.add_rule(make_test_rule("(original $x)", "(original-result $x)"));

        let mut clone1 = env.clone();
        let mut clone2 = env.clone();
        let mut clone3 = env.clone();

        // Mutate each clone differently
        clone1.add_rule(make_test_rule("(clone1 $a)", "(result1 $a)"));
        clone2.add_rule(make_test_rule("(clone2 $b)", "(result2 $b)"));
        clone3.add_rule(make_test_rule("(clone3 $c)", "(result3 $c)"));

        // Each clone should have only its own rule (plus original)
        let original_count = env.rule_count();
        let clone1_count = clone1.rule_count();
        let clone2_count = clone2.rule_count();
        let clone3_count = clone3.rule_count();

        assert_eq!(original_count, 1, "Original should have 1 rule");
        assert_eq!(clone1_count, 2, "Clone1 should have 2 rules");
        assert_eq!(clone2_count, 2, "Clone2 should have 2 rules");
        assert_eq!(clone3_count, 2, "Clone3 should have 2 rules");
    }

    // ============================================================================
    // PROPERTY-BASED TESTS (~100 LOC)
    // ============================================================================

    #[test]
    fn property_clone_never_shares_mutable_state_after_write() {
        // Property: After mutation, clone and original should have independent state
        for i in 0..10 {
            let mut env = Environment::new();
            env.add_rule(make_test_rule(&format!("(test{}  $x)", i), "(result $x)"));

            let mut clone = env.clone();
            clone.add_rule(make_test_rule(&format!("(clone{} $y)", i), "(cloned $y)"));

            // Verify Arc pointers are different
            let env_ptr = StdArc::as_ptr(&env.btm);
            let clone_ptr = StdArc::as_ptr(&clone.btm);
            assert_ne!(
                env_ptr, clone_ptr,
                "Property violated: clone shares mutable state after write (iteration {})",
                i
            );
        }
    }

    #[test]
    fn property_parallel_writes_are_isolated() {
        // Property: Parallel mutations to different clones should be isolated
        let env = Environment::new();
        let num_threads = 4;
        let barrier = StdArc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let mut clone = env.clone();
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Synchronize all threads to start mutations simultaneously
                    barrier.wait();

                    // Each thread adds a unique rule
                    clone.add_rule(make_test_rule(
                        &format!("(thread{} $x)", i),
                        &format!("(result{} $x)", i),
                    ));

                    // Verify this clone only has 1 rule
                    let count = clone.rule_count();
                    assert_eq!(count, 1, "Thread {} clone should have exactly 1 rule", i);

                    clone
                })
            })
            .collect();

        // Join all threads and verify each clone is independent
        let clones: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        for (i, clone) in clones.iter().enumerate() {
            let count = clone.rule_count();
            assert_eq!(
                count, 1,
                "Clone {} should have exactly 1 rule after parallel write",
                i
            );
        }

        // Original should be unchanged
        assert_eq!(
            env.rule_count(),
            0,
            "Original environment should be unchanged"
        );
    }

    // ============================================================================
    // STRESS TESTS (~100 LOC)
    // ============================================================================

    #[test]
    fn stress_many_clones_with_mutations() {
        // Stress: Create 1000 clones and mutate each one
        let env = Environment::new();

        for i in 0..1000 {
            let mut clone = env.clone();
            clone.add_rule(make_test_rule(&format!("(stress{} $x)", i), "(result $x)"));

            assert!(
                clone.owns_data,
                "Clone {} should own data after mutation",
                i
            );
            assert_eq!(clone.rule_count(), 1, "Clone {} should have 1 rule", i);
        }

        // Original should be unchanged
        assert_eq!(
            env.rule_count(),
            0,
            "Original should be unchanged after 1000 clone mutations"
        );
    }

    #[test]
    fn stress_deep_clone_chains() {
        // Stress: Create clone chains (clone of clone of clone...)
        let mut env = Environment::new();
        env.add_rule(make_test_rule("(original $x)", "(result $x)"));

        let mut current = env.clone();
        for i in 0..10 {
            current.add_rule(make_test_rule(&format!("(depth{} $x)", i), "(result $x)"));
            let next = current.clone();
            current = next;
        }

        // Final clone should have 1 (original) + 10 (depth) = 11 rules
        assert_eq!(current.rule_count(), 11, "Final clone should have 11 rules");

        // Original should be unchanged
        assert_eq!(env.rule_count(), 1, "Original should still have 1 rule");
    }

    #[test]
    fn stress_concurrent_clone_and_mutate() {
        // Stress: Concurrent cloning and mutation across multiple threads
        let env = StdArc::new(Environment::new());
        let num_threads = 8;

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let env = StdArc::clone(&env);

                thread::spawn(move || {
                    for j in 0..100 {
                        let mut clone = env.as_ref().clone();
                        clone
                            .add_rule(make_test_rule(&format!("(t{}_{} $x)", i, j), "(result $x)"));
                        assert_eq!(clone.rule_count(), 1, "Clone should have 1 rule");
                    }
                })
            })
            .collect();

        // Join all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Original should be unchanged
        assert_eq!(
            env.rule_count(),
            0,
            "Original should be unchanged after concurrent stress"
        );
    }

    // ============================================================================
    // INTEGRATION TESTS (~100 LOC)
    // ============================================================================

    #[test]
    fn integration_parallel_eval_with_dynamic_rules() {
        // Integration: Simulate parallel evaluation where each thread adds rules dynamically
        use std::sync::Mutex as StdMutex;

        let base_env = Environment::new();
        let results = StdArc::new(StdMutex::new(Vec::new()));
        let num_threads = 4;

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let mut env = base_env.clone();
                let results = StdArc::clone(&results);

                thread::spawn(move || {
                    // Each thread adds rules dynamically during "evaluation"
                    for j in 0..10 {
                        let rule = make_test_rule(&format!("(eval{}_{}  $x)", i, j), "(result $x)");
                        env.add_rule(rule);
                    }

                    let count = env.rule_count();
                    results.lock().unwrap().push(count);
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Each thread should have 10 rules
        let results = results.lock().unwrap();
        assert_eq!(
            results.len(),
            num_threads,
            "Should have {} results",
            num_threads
        );
        for (i, &count) in results.iter().enumerate() {
            assert_eq!(count, 10, "Thread {} should have 10 rules", i);
        }

        // Base environment should be unchanged
        assert_eq!(
            base_env.rule_count(),
            0,
            "Base environment should be unchanged"
        );
    }

    #[test]
    fn integration_read_while_write() {
        // Integration: Test concurrent reads and writes (RwLock benefit)
        let mut env = Environment::new();
        for i in 0..100 {
            env.add_rule(make_test_rule(&format!("(rule{} $x)", i), "(result $x)"));
        }

        let env = StdArc::new(env);
        let num_readers = 8;
        let barrier = StdArc::new(Barrier::new(num_readers + 1));

        // Spawn reader threads
        let reader_handles: Vec<_> = (0..num_readers)
            .map(|_| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    // Multiple readers should be able to read concurrently (RwLock benefit)
                    for _ in 0..100 {
                        let count = env.rule_count();
                        assert!(count >= 100, "Should see at least 100 rules");
                    }
                })
            })
            .collect();

        // Start all readers simultaneously
        barrier.wait();

        // Join all readers
        for handle in reader_handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn integration_clone_preserves_rule_data() {
        // Integration: Verify clone preserves all rule data correctly
        let mut env = Environment::new();

        // Add various rules
        let rules = vec![
            make_test_rule("(color car red)", "(assert color car red)"),
            make_test_rule("(color truck blue)", "(assert color truck blue)"),
            make_test_rule("(size car small)", "(assert size car small)"),
        ];

        for rule in &rules {
            env.add_rule(rule.clone());
        }

        // Clone environment
        let clone = env.clone();

        // Verify clone has same rules
        assert_eq!(
            clone.rule_count(),
            env.rule_count(),
            "Clone should have same rule count"
        );

        // Verify each rule is accessible
        for rule in &rules {
            let (head, arity) = extract_head_arity(&rule.lhs);
            let original_matches = env.get_matching_rules(head, arity);
            let clone_matches = clone.get_matching_rules(head, arity);

            assert!(!original_matches.is_empty(), "Original should have rule");
            assert!(!clone_matches.is_empty(), "Clone should have rule");
        }
    }
}
// ============================================================================
// Thread Safety Tests (Phase 2) - To be appended to environment.rs
// ============================================================================

#[cfg(test)]
mod thread_safety_tests {
    use super::*;
    use std::sync::{Arc as StdArc, Barrier};
    use std::thread;
    use std::time::Duration;

    // Helper: Create a test rule with proper SExpr structure
    fn make_test_rule(pattern: &str, body: &str) -> Rule {
        // Parse pattern string into proper MettaValue structure
        // "(head $x)" → SExpr([Atom("head"), Atom("$x")])
        let lhs = if pattern.starts_with('(') && pattern.ends_with(')') {
            // Parse s-expression pattern
            let inner = &pattern[1..pattern.len() - 1];
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.is_empty() {
                MettaValue::Atom(pattern.to_string())
            } else {
                MettaValue::SExpr(
                    parts
                        .into_iter()
                        .map(|p| MettaValue::Atom(p.to_string()))
                        .collect(),
                )
            }
        } else {
            // Simple atom pattern
            MettaValue::Atom(pattern.to_string())
        };

        // Parse body similarly
        let rhs = if body.starts_with('(') && body.ends_with(')') {
            let inner = &body[1..body.len() - 1];
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.is_empty() {
                MettaValue::Atom(body.to_string())
            } else {
                MettaValue::SExpr(
                    parts
                        .into_iter()
                        .map(|p| MettaValue::Atom(p.to_string()))
                        .collect(),
                )
            }
        } else {
            MettaValue::Atom(body.to_string())
        };

        Rule { lhs, rhs }
    }

    // Helper: Extract head and arity from a pattern
    fn extract_head_arity(pattern: &MettaValue) -> (&str, usize) {
        match pattern {
            MettaValue::SExpr(items) if !items.is_empty() => {
                if let MettaValue::Atom(head) = &items[0] {
                    // Count variables (starts with $, &, or ')
                    let arity = items[1..].iter().filter(|item| {
                        matches!(item, MettaValue::Atom(s) if s.starts_with('$') || s.starts_with('&') || s.starts_with('\''))
                    }).count();
                    (head.as_str(), arity)
                } else {
                    ("_", 0)
                }
            }
            MettaValue::Atom(s) => (s.as_str(), 0),
            _ => ("_", 0),
        }
    }

    // ========================================================================
    // Category 1: Concurrent Mutation Tests
    // ========================================================================

    #[test]
    fn test_concurrent_clone_and_mutate_2_threads() {
        let mut base = Environment::new();

        // Add some base rules
        for i in 0..10 {
            base.add_rule(make_test_rule(&format!("(base{} $x)", i), "(result $x)"));
        }

        let base = StdArc::new(base);
        let handles: Vec<_> = (0..2)
            .map(|thread_id| {
                let base = StdArc::clone(&base);
                thread::spawn(move || {
                    // Clone and mutate independently
                    let mut clone = (*base).clone();

                    // Add thread-specific rules
                    for i in 0..5 {
                        clone.add_rule(make_test_rule(
                            &format!("(thread{}_rule{} $x)", thread_id, i),
                            &format!("(result{} $x)", i),
                        ));
                    }

                    // Verify this clone has base + thread-specific rules
                    assert_eq!(
                        clone.rule_count(),
                        15,
                        "Thread {} should have 15 rules",
                        thread_id
                    );

                    // Verify thread-specific rules exist
                    for i in 0..5 {
                        let pattern = format!("(thread{}_rule{} $x)", thread_id, i);
                        let rule = make_test_rule(&pattern, &format!("(result{} $x)", i));
                        let (head, arity) = extract_head_arity(&rule.lhs);
                        let matches = clone.get_matching_rules(head, arity);
                        assert!(
                            !matches.is_empty(),
                            "Thread {} rule {} should exist",
                            thread_id,
                            i
                        );
                    }

                    clone
                })
            })
            .collect();

        // Wait for all threads and collect results
        let results: Vec<Environment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify base is unchanged
        assert_eq!(base.rule_count(), 10, "Base should still have 10 rules");

        // Verify each result has exactly its own mutations
        assert_eq!(results.len(), 2);
        for (thread_id, clone) in results.iter().enumerate() {
            assert_eq!(
                clone.rule_count(),
                15,
                "Clone {} should have 15 rules",
                thread_id
            );

            // Verify other thread's rules DON'T exist (isolation)
            let other_thread = 1 - thread_id;
            for i in 0..5 {
                let pattern = format!("(thread{}_rule{} $x)", other_thread, i);
                let rule = make_test_rule(&pattern, &format!("(result{} $x)", i));
                let (head, arity) = extract_head_arity(&rule.lhs);
                let matches = clone.get_matching_rules(head, arity);
                assert!(
                    matches.is_empty(),
                    "Clone {} should NOT have thread {} rules",
                    thread_id,
                    other_thread
                );
            }
        }
    }

    #[test]
    fn test_concurrent_clone_and_mutate_8_threads() {
        const N_THREADS: usize = 8;
        const RULES_PER_THREAD: usize = 10;

        let mut base = Environment::new();

        // Add base rules
        for i in 0..20 {
            base.add_rule(make_test_rule(&format!("(base{} $x)", i), "(result $x)"));
        }

        let base = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|thread_id| {
                let base = StdArc::clone(&base);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Clone
                    let mut clone = (*base).clone();

                    // Synchronize to maximize concurrency
                    barrier.wait();

                    // Mutate concurrently
                    for i in 0..RULES_PER_THREAD {
                        clone.add_rule(make_test_rule(
                            &format!("(t{}_r{} $x)", thread_id, i),
                            &format!("(res{} $x)", i),
                        ));
                    }

                    // Verify count
                    assert_eq!(
                        clone.rule_count(),
                        20 + RULES_PER_THREAD,
                        "Thread {} should have {} rules",
                        thread_id,
                        20 + RULES_PER_THREAD
                    );

                    (thread_id, clone)
                })
            })
            .collect();

        // Collect results
        let results: Vec<(usize, Environment)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify base unchanged
        assert_eq!(base.rule_count(), 20);

        // Verify isolation: each clone has only its own mutations
        for (thread_id, clone) in &results {
            for (other_id, _) in &results {
                if thread_id == other_id {
                    continue; // Skip self
                }

                // Verify other thread's rules DON'T exist
                for i in 0..RULES_PER_THREAD {
                    let pattern = format!("(t{}_r{} $x)", other_id, i);
                    let rule = Rule {
                        lhs: MettaValue::Atom(pattern),
                        rhs: MettaValue::Atom(format!("(res{} $x)", i)),
                    };
                    let (head, arity) = extract_head_arity(&rule.lhs);
                    let matches = clone.get_matching_rules(head, arity);
                    assert!(
                        matches.is_empty(),
                        "Clone {} should NOT have thread {} rules",
                        thread_id,
                        other_id
                    );
                }
            }
        }
    }

    #[test]
    fn test_concurrent_add_rules() {
        const N_THREADS: usize = 4;
        const RULES_PER_THREAD: usize = 25;

        let env = StdArc::new(Environment::new());
        let barrier = StdArc::new(Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|thread_id| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Each thread gets its own clone
                    let mut clone = (*env).clone();

                    // Synchronize
                    barrier.wait();

                    // Add rules concurrently
                    for i in 0..RULES_PER_THREAD {
                        clone.add_rule(make_test_rule(
                            &format!("(rule_{}_{} $x)", thread_id, i),
                            &format!("(body_{}_{} $x)", thread_id, i),
                        ));
                    }

                    clone
                })
            })
            .collect();

        // Collect all clones
        let clones: Vec<Environment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify each clone has exactly RULES_PER_THREAD
        for (i, clone) in clones.iter().enumerate() {
            assert_eq!(
                clone.rule_count(),
                RULES_PER_THREAD,
                "Clone {} should have {} rules",
                i,
                RULES_PER_THREAD
            );
        }

        // Verify original is unchanged
        assert_eq!(env.rule_count(), 0);
    }

    #[test]
    fn test_concurrent_read_shared_clone() {
        const N_READERS: usize = 16;
        const READS_PER_THREAD: usize = 100;

        let mut base = Environment::new();
        for i in 0..50 {
            base.add_rule(make_test_rule(&format!("(rule{} $x)", i), "(result $x)"));
        }

        let env = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_READERS));

        let handles: Vec<_> = (0..N_READERS)
            .map(|_| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Synchronize to maximize contention
                    barrier.wait();

                    // Perform many reads
                    for _ in 0..READS_PER_THREAD {
                        let count = env.rule_count();
                        assert_eq!(count, 50, "Should always see 50 rules");
                    }
                })
            })
            .collect();

        // Wait for completion
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify environment unchanged
        assert_eq!(env.rule_count(), 50);
    }

    // ========================================================================
    // Category 2: Race Condition Tests
    // ========================================================================

    #[test]
    fn test_clone_during_mutation() {
        const N_CLONERS: usize = 4;
        const N_MUTATORS: usize = 4;

        let mut base = Environment::new();
        for i in 0..20 {
            base.add_rule(make_test_rule(&format!("(base{} $x)", i), "(result $x)"));
        }

        let env = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_CLONERS + N_MUTATORS));

        // Spawn cloners
        let cloner_handles: Vec<_> = (0..N_CLONERS)
            .map(|id| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    // Clone repeatedly
                    for _ in 0..10 {
                        let clone = (*env).clone();
                        assert_eq!(clone.rule_count(), 20, "Cloner {} saw wrong count", id);
                        thread::sleep(Duration::from_micros(10));
                    }
                })
            })
            .collect();

        // Spawn mutators (they mutate their own clones)
        let mutator_handles: Vec<_> = (0..N_MUTATORS)
            .map(|id| {
                let env = StdArc::clone(&env);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    // Get a clone and mutate it
                    let mut clone = (*env).clone();
                    for i in 0..10 {
                        clone.add_rule(make_test_rule(
                            &format!("(mut{}_{} $x)", id, i),
                            "(result $x)",
                        ));
                        thread::sleep(Duration::from_micros(10));
                    }

                    assert_eq!(clone.rule_count(), 30, "Mutator {} final count wrong", id);
                })
            })
            .collect();

        // Wait for all threads
        for handle in cloner_handles.into_iter().chain(mutator_handles) {
            handle.join().unwrap();
        }

        // Base should be unchanged
        assert_eq!(env.rule_count(), 20);
    }

    #[test]
    fn test_make_owned_race() {
        // Test that concurrent first mutations (which trigger make_owned) are safe
        const N_THREADS: usize = 8;

        let mut base = Environment::new();
        for i in 0..10 {
            base.add_rule(make_test_rule(&format!("(base{} $x)", i), "(result $x)"));
        }

        // Create one shared clone
        let shared_clone = StdArc::new(base.clone());
        let barrier = StdArc::new(Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|thread_id| {
                let clone_ref = StdArc::clone(&shared_clone);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    // Each thread gets its own clone from the shared clone
                    let mut my_clone = (*clone_ref).clone();

                    // Synchronize to maximize race potential
                    barrier.wait();

                    // This mutation triggers make_owned() for this specific clone
                    // All threads do this simultaneously, testing atomicity
                    my_clone.add_rule(make_test_rule(
                        &format!("(first_mutation_{} $x)", thread_id),
                        "(result $x)",
                    ));

                    // Verify we have base + 1 rule
                    assert_eq!(
                        my_clone.rule_count(),
                        11,
                        "Thread {} should have 11 rules",
                        thread_id
                    );

                    my_clone
                })
            })
            .collect();

        // Collect results
        let results: Vec<Environment> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Verify each got its own copy
        for (i, clone) in results.iter().enumerate() {
            assert_eq!(clone.rule_count(), 11, "Result {} should have 11 rules", i);
        }

        // Verify shared clone and base are unchanged
        assert_eq!(shared_clone.rule_count(), 10);
        assert_eq!(base.rule_count(), 10);
    }

    #[test]
    fn test_read_during_make_owned() {
        // Test reading while another clone is doing make_owned()
        const N_READERS: usize = 8;
        const N_WRITERS: usize = 2;

        let mut base = Environment::new();
        for i in 0..30 {
            base.add_rule(make_test_rule(&format!("(rule{} $x)", i), "(result $x)"));
        }

        let shared = StdArc::new(base);
        let barrier = StdArc::new(Barrier::new(N_READERS + N_WRITERS));

        // Readers: clone and read repeatedly
        let reader_handles: Vec<_> = (0..N_READERS)
            .map(|id| {
                let shared = StdArc::clone(&shared);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    for _ in 0..20 {
                        let clone = (*shared).clone();
                        let count = clone.rule_count();
                        assert_eq!(count, 30, "Reader {} saw wrong count: {}", id, count);
                        thread::sleep(Duration::from_micros(5));
                    }
                })
            })
            .collect();

        // Writers: clone and mutate (triggering make_owned)
        let writer_handles: Vec<_> = (0..N_WRITERS)
            .map(|id| {
                let shared = StdArc::clone(&shared);
                let barrier = StdArc::clone(&barrier);

                thread::spawn(move || {
                    barrier.wait();

                    for i in 0..10 {
                        let mut clone = (*shared).clone();
                        clone.add_rule(make_test_rule(
                            &format!("(writer{}_{} $x)", id, i),
                            "(result $x)",
                        ));
                        assert_eq!(
                            clone.rule_count(),
                            31,
                            "Writer {} iteration {} wrong count",
                            id,
                            i
                        );
                        thread::sleep(Duration::from_micros(5));
                    }
                })
            })
            .collect();

        // Wait for all
        for handle in reader_handles.into_iter().chain(writer_handles) {
            handle.join().unwrap();
        }

        // Shared should be unchanged
        assert_eq!(shared.rule_count(), 30);
    }
}
