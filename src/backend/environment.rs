use lru::LruCache;
use mork::space::Space;
use mork_interning::SharedMappingHandle;
use pathmap::{zipper::*, PathMap};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use super::fuzzy_match::FuzzyMatcher;
use super::grounded::{GroundedRegistry, GroundedRegistryTCO};
use super::modules::{ModId, ModuleRegistry, Tokenizer};
use super::{MettaValue, Rule};

/// Shared state across all Environment clones.
/// Consolidates 17 Arc<RwLock<T>> fields into a single Arc<EnvironmentShared>
/// for O(1) clone operations (1 atomic increment instead of 17).
///
/// Thread-safe: All fields use RwLock for concurrent read/exclusive write access.
struct EnvironmentShared {
    /// PathMap trie for fact storage
    btm: RwLock<PathMap<()>>,

    /// Rule index: Maps (head_symbol, arity) -> Vec<Rule> for O(1) rule lookup
    #[allow(clippy::type_complexity)]
    rule_index: RwLock<HashMap<(String, usize), Vec<Rule>>>,

    /// Wildcard rules: Rules without a clear head symbol
    wildcard_rules: RwLock<Vec<Rule>>,

    /// Multiplicities: tracks how many times each rule is defined
    multiplicities: RwLock<HashMap<String, usize>>,

    /// Pattern cache: LRU cache for MORK serialization results
    pattern_cache: RwLock<LruCache<MettaValue, Vec<u8>>>,

    /// Type index: Lazy-initialized subtrie containing only type assertions
    type_index: RwLock<Option<PathMap<()>>>,

    /// Type index invalidation flag
    type_index_dirty: RwLock<bool>,

    /// Named spaces registry: Maps space_id -> (name, atoms)
    #[allow(clippy::type_complexity)]
    named_spaces: RwLock<HashMap<u64, (String, Vec<MettaValue>)>>,

    /// Counter for generating unique space IDs
    next_space_id: RwLock<u64>,

    /// Mutable state cells registry
    states: RwLock<HashMap<u64, MettaValue>>,

    /// Counter for generating unique state IDs
    next_state_id: RwLock<u64>,

    /// Symbol bindings registry
    bindings: RwLock<HashMap<String, MettaValue>>,

    /// Module registry
    module_registry: RwLock<ModuleRegistry>,

    /// Per-module tokenizer
    tokenizer: RwLock<Tokenizer>,

    /// Grounded operations registry (legacy)
    grounded_registry: RwLock<GroundedRegistry>,

    /// TCO-compatible grounded operations registry
    grounded_registry_tco: RwLock<GroundedRegistryTCO>,

    /// Fallback store for large expressions
    large_expr_pathmap: RwLock<Option<PathMap<MettaValue>>>,

    /// Fuzzy matcher for "Did you mean?" suggestions
    fuzzy_matcher: RwLock<FuzzyMatcher>,
}

/// The environment contains the fact database and type assertions
/// All facts (rules, atoms, s-expressions, type assertions) are stored in MORK PathMap
///
/// Thread-safe with Copy-on-Write (CoW) semantics:
/// - Clones share data until first modification (owns_data = false)
/// - First mutation triggers deep copy via make_owned() (owns_data = true)
/// - RwLock enables concurrent reads (4× improvement over Mutex)
/// - Modifications tracked via Arc<AtomicBool> for fast union() paths
///
/// Performance optimization: All shared state is consolidated into a single
/// Arc<EnvironmentShared> for O(1) clone operations (1 atomic increment instead of 17).
pub struct Environment {
    /// Consolidated shared state - single Arc for O(1) cloning
    /// Contains all RwLock-wrapped fields that can be shared across clones
    shared: Arc<EnvironmentShared>,

    /// THREAD-SAFE: SharedMappingHandle for symbol interning (string → u64)
    /// Can be cloned and shared across threads (Send + Sync)
    /// Kept separate as it has its own sharing semantics
    shared_mapping: SharedMappingHandle,

    /// CoW: Tracks if this clone owns its data (true = can modify in-place, false = must deep copy first)
    /// Set to true on new(), false on clone(), true after make_owned()
    owns_data: bool,

    /// CoW: Tracks if this environment has been modified since creation/clone
    /// Used for fast-path union() optimization (unmodified clones can skip deep merge)
    /// Arc-wrapped to allow independent tracking per clone
    modified: Arc<AtomicBool>,

    /// Current module path: Directory of the currently-executing module
    /// Used for relative path resolution (self:child notation)
    /// None when not inside a module evaluation
    /// Kept separate as it's per-clone state
    current_module_path: Option<PathBuf>,
}

impl Environment {
    pub fn new() -> Self {
        use mork_interning::SharedMapping;

        let shared = Arc::new(EnvironmentShared {
            btm: RwLock::new(PathMap::new()),
            rule_index: RwLock::new(HashMap::new()),
            wildcard_rules: RwLock::new(Vec::new()),
            multiplicities: RwLock::new(HashMap::new()),
            pattern_cache: RwLock::new(LruCache::new(NonZeroUsize::new(1000).unwrap())),
            type_index: RwLock::new(None),
            type_index_dirty: RwLock::new(true),
            named_spaces: RwLock::new(HashMap::new()),
            next_space_id: RwLock::new(1), // Start from 1, 0 reserved for self
            states: RwLock::new(HashMap::new()),
            next_state_id: RwLock::new(1), // Start from 1
            bindings: RwLock::new(HashMap::new()),
            module_registry: RwLock::new(ModuleRegistry::new()),
            tokenizer: RwLock::new(Tokenizer::new()),
            grounded_registry: RwLock::new(GroundedRegistry::with_standard_ops()),
            grounded_registry_tco: RwLock::new(GroundedRegistryTCO::with_standard_ops()),
            large_expr_pathmap: RwLock::new(None), // Lazy: not allocated until needed
            fuzzy_matcher: RwLock::new(FuzzyMatcher::new()),
        });

        Environment {
            shared,
            shared_mapping: SharedMapping::new(),
            owns_data: true, // CoW: new environments own their data
            modified: Arc::new(AtomicBool::new(false)), // CoW: track modifications
            current_module_path: None,
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

        // Deep copy the entire shared state structure
        // Clone the data first to avoid borrowing issues
        let new_shared = Arc::new(EnvironmentShared {
            btm: RwLock::new(self.shared.btm.read().unwrap().clone()),
            rule_index: RwLock::new(self.shared.rule_index.read().unwrap().clone()),
            wildcard_rules: RwLock::new(self.shared.wildcard_rules.read().unwrap().clone()),
            multiplicities: RwLock::new(self.shared.multiplicities.read().unwrap().clone()),
            pattern_cache: RwLock::new(self.shared.pattern_cache.read().unwrap().clone()),
            type_index: RwLock::new(self.shared.type_index.read().unwrap().clone()),
            type_index_dirty: RwLock::new(*self.shared.type_index_dirty.read().unwrap()),
            named_spaces: RwLock::new(self.shared.named_spaces.read().unwrap().clone()),
            next_space_id: RwLock::new(*self.shared.next_space_id.read().unwrap()),
            states: RwLock::new(self.shared.states.read().unwrap().clone()),
            next_state_id: RwLock::new(*self.shared.next_state_id.read().unwrap()),
            bindings: RwLock::new(self.shared.bindings.read().unwrap().clone()),
            module_registry: RwLock::new(self.shared.module_registry.read().unwrap().clone()),
            tokenizer: RwLock::new(self.shared.tokenizer.read().unwrap().clone()),
            grounded_registry: RwLock::new(self.shared.grounded_registry.read().unwrap().clone()),
            grounded_registry_tco: RwLock::new(self.shared.grounded_registry_tco.read().unwrap().clone()),
            large_expr_pathmap: RwLock::new(self.shared.large_expr_pathmap.read().unwrap().clone()),
            fuzzy_matcher: RwLock::new(self.shared.fuzzy_matcher.read().unwrap().clone()),
        });

        self.shared = new_shared;

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
        let btm = self.shared.btm.read().unwrap().clone(); // CoW: read lock for concurrent reads
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
        *self.shared.btm.write().unwrap() = space.btm; // CoW: write lock for exclusive access
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
        *self.shared.type_index_dirty.write().unwrap() = true;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Ensure the type index is built and up-to-date
    /// Uses PathMap's restrict() to extract only type assertions into a subtrie
    /// This enables O(p + m) type lookups where m << n (total facts)
    ///
    /// The type index is lazily initialized and cached until invalidated
    fn ensure_type_index(&self) {
        let dirty = *self.shared.type_index_dirty.read().unwrap();
        if !dirty {
            return; // Index is up to date
        }

        // Build type index using PathMap::restrict()
        // This extracts a subtrie containing only paths that start with ":"
        let btm = self.shared.btm.read().unwrap();

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
        *self.shared.type_index.write().unwrap() = Some(type_subtrie);
        *self.shared.type_index_dirty.write().unwrap() = false;
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
        let type_index_opt = self.shared.type_index.read().unwrap();
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
                                rules.push(Rule::new(
                                    items[1].clone(),
                                    items[2].clone(),
                                ));
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
            let mut index = self.shared.rule_index.write().unwrap();
            index.clear();
        }
        {
            let mut wildcards = self.shared.wildcard_rules.write().unwrap();
            wildcards.clear();
        }

        // Rebuild from MORK Space
        for rule in self.iter_rules() {
            if let Some(head) = rule.lhs.get_head_symbol() {
                let arity = rule.lhs.get_arity();
                let head_owned = head.to_owned();
                // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
                self.shared.fuzzy_matcher.write().unwrap().insert(&head_owned);
                let mut index = self.shared.rule_index.write().unwrap();
                index.entry((head_owned, arity)).or_default().push(rule);
            } else {
                // Rules without head symbol (wildcards, variables) go to wildcard list
                let mut wildcards = self.shared.wildcard_rules.write().unwrap();
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

        // 1. Iterate through MORK PathMap (primary storage)
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
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    results.push(instantiated);
                }
            }
        }

        drop(space);

        // 2. Also check large expression fallback PathMap (if allocated)
        // These are expressions with arity >= 64 that couldn't fit in MORK
        let guard = self.shared.large_expr_pathmap.read().unwrap();
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    results.push(instantiated);
                }
            }
        }

        results
    }

    /// Add a rule to the environment
    /// Rules are stored in MORK Space as s-expressions: (= lhs rhs)
    /// Multiply-defined rules are tracked via multiplicities
    /// Rules are also indexed by (head_symbol, arity) for fast lookup
    pub fn add_rule(&mut self, rule: Rule) {
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
            let mut counts = self.shared.multiplicities.write().unwrap();
            let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
            counts.insert(rule_key.clone(), new_count);
        } // Drop the RefMut borrow before add_to_space

        // Add to rule index for O(k) lookup
        // Note: We store the rule only ONCE (in either index or wildcard list)
        // to avoid unnecessary clones. The rule is already in MORK Space.
        if let Some(head) = rule.lhs.get_head_symbol() {
            let arity = rule.lhs.get_arity();
            let head_owned = head.to_owned();
            // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
            self.shared.fuzzy_matcher.write().unwrap().insert(&head_owned);
            let mut index = self.shared.rule_index.write().unwrap();
            index.entry((head_owned, arity)).or_default().push(rule); // Move instead of clone
        } else {
            // Rules without head symbol (wildcards, variables) go to wildcard list
            let mut wildcards = self.shared.wildcard_rules.write().unwrap();
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
                let head_owned = head.to_owned();
                // Track symbol for fuzzy matching
                self.shared.fuzzy_matcher.write().unwrap().insert(&head_owned);
                rule_index_updates
                    .entry((head_owned, arity))
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
            let mut counts = self.shared.multiplicities.write().unwrap();
            for (key, delta) in multiplicity_updates {
                *counts.entry(key).or_insert(0) += delta;
            }
        }

        // Update rule index
        {
            let mut index = self.shared.rule_index.write().unwrap();
            for ((head, arity), mut rules) in rule_index_updates {
                index.entry((head, arity)).or_default().append(&mut rules);
            }
        }

        // Update wildcard rules
        {
            let mut wildcards = self.shared.wildcard_rules.write().unwrap();
            wildcards.extend(wildcard_updates);
        }

        // Single PathMap union (minimal critical section)
        {
            let mut btm = self.shared.btm.write().unwrap();
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

        let counts = self.shared.multiplicities.read().unwrap();
        *counts.get(&rule_key).unwrap_or(&1)
    }

    /// Get the multiplicities (for serialization)
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        self.shared.multiplicities.read().unwrap().clone()
    }

    /// Set the multiplicities (used for deserialization)
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self.shared.multiplicities.write().unwrap() = counts;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Get read access to the large expression fallback PathMap
    ///
    /// Returns the fallback PathMap that stores expressions with arity >= 64
    /// (which exceed MORK's 63-arity limit). Uses varint encoding for keys.
    /// Returns None if no large expressions have been stored.
    pub fn get_large_expr_pathmap(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, Option<PathMap<MettaValue>>> {
        self.shared.large_expr_pathmap.read().unwrap()
    }

    /// Insert a value into the large expressions fallback PathMap
    /// Used during deserialization to restore large expressions (arity >= 64)
    /// that exceed MORK's 63-arity limit
    pub fn insert_large_expr(&self, value: MettaValue) {
        use crate::backend::varint_encoding::metta_to_varint_key;
        let key = metta_to_varint_key(&value);
        let mut guard = self.shared.large_expr_pathmap.write().unwrap();
        let fallback = guard.get_or_insert_with(PathMap::new);
        fallback.insert(&key, value);
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
                let mut cache = self.shared.pattern_cache.write().unwrap();
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
            let mut cache = self.shared.pattern_cache.write().unwrap();
            cache.put(value.clone(), bytes.clone());
        }

        Ok(bytes)
    }

    /// Check if a MettaValue contains variables ($x, &y, 'z, or _)
    /// Space references like &self, &kb, &stack are NOT variables
    fn contains_variables(value: &MettaValue) -> bool {
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
            }
            Err(_e) => {
                // Fallback: Store in PathMap with varint encoding (arity >= 64)
                // Lazy allocation: only create PathMap on first use
                let key = metta_to_varint_key(value);
                self.make_owned(); // CoW: ensure we own data before modifying
                let mut guard = self.shared.large_expr_pathmap.write().unwrap();
                let fallback = guard.get_or_insert_with(PathMap::new);
                fallback.insert(&key, value.clone());

                #[cfg(debug_assertions)]
                eprintln!(
                    "Info: large expression stored in fallback PathMap: {}",
                    _e
                );
            }
        }
    }

    /// Remove a fact from MORK Space by exact match
    ///
    /// This removes the specified value from the PathMap trie if it exists.
    /// The value must match exactly - no pattern matching or wildcards.
    ///
    /// # Examples
    /// ```ignore
    /// env.add_to_space(&MettaValue::atom("foo"));
    /// env.remove_from_space(&MettaValue::atom("foo"));  // Removes "foo"
    /// ```
    ///
    /// # Performance
    /// - Ground values: O(m) where m = size of MORK encoding
    /// - Uses direct byte conversion for 10-20× speedup (same as add_to_space)
    ///
    /// # Thread Safety
    /// - Acquires write lock on PathMap
    /// - Marks environment as modified (CoW)
    pub fn remove_from_space(&mut self, value: &MettaValue) {
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
            }
            Err(_) => {
                // Remove from fallback PathMap (if it exists)
                let key = metta_to_varint_key(value);
                let mut guard = self.shared.large_expr_pathmap.write().unwrap();
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
    /// # Examples
    /// ```ignore
    /// // Remove all facts with head "parent":
    /// env.remove_matching(&sexpr![atom("parent"), var("$x"), var("$y")]);
    ///
    /// // Remove specific facts:
    /// env.remove_matching(&sexpr![atom("temp"), var("$_")]);
    /// ```
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
        // Query for all matches using match_space with identity template
        let matches = self.match_space(pattern, pattern);

        // Remove each match
        for m in &matches {
            self.remove_from_space(m);
        }

        matches
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
            let mut btm = self.shared.btm.write().unwrap();
            *btm = btm.join(&fact_trie);
        }

        // Invalidate type index if any facts were type assertions
        // Conservative: Assume any bulk insert might contain types
        *self.shared.type_index_dirty.write().unwrap() = true;

        self.modified.store(true, Ordering::Release); // CoW: mark as modified
        Ok(())
    }

    // ============================================================
    // Named Space Management (new-space, add-atom, remove-atom, collapse)
    // ============================================================

    /// Create a new named space and return its ID
    /// Used by new-space operation
    pub fn create_named_space(&mut self, name: &str) -> u64 {
        self.make_owned();

        let id = {
            let mut next_id = self.shared.next_space_id.write().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.shared.named_spaces
            .write()
            .unwrap()
            .insert(id, (name.to_string(), Vec::new()));

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Add an atom to a named space by ID
    /// Used by add-atom operation
    pub fn add_to_named_space(&mut self, space_id: u64, value: &MettaValue) -> bool {
        self.make_owned();

        let mut spaces = self.shared.named_spaces.write().unwrap();
        if let Some((_, atoms)) = spaces.get_mut(&space_id) {
            atoms.push(value.clone());
            self.modified.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Remove an atom from a named space by ID
    /// Used by remove-atom operation
    pub fn remove_from_named_space(&mut self, space_id: u64, value: &MettaValue) -> bool {
        self.make_owned();

        let mut spaces = self.shared.named_spaces.write().unwrap();
        if let Some((_, atoms)) = spaces.get_mut(&space_id) {
            // Remove first matching atom
            if let Some(pos) = atoms.iter().position(|x| x == value) {
                atoms.remove(pos);
                self.modified.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Get all atoms from a named space as a list
    /// Used by collapse operation
    pub fn collapse_named_space(&self, space_id: u64) -> Vec<MettaValue> {
        let spaces = self.shared.named_spaces.read().unwrap();
        if let Some((_, atoms)) = spaces.get(&space_id) {
            atoms.clone()
        } else {
            vec![]
        }
    }

    /// Check if a named space exists
    pub fn has_named_space(&self, space_id: u64) -> bool {
        self.shared.named_spaces.read().unwrap().contains_key(&space_id)
    }

    // ============================================================
    // Mutable State Management (new-state, get-state, change-state!)
    // ============================================================

    /// Create a new mutable state cell with an initial value
    /// Used by new-state operation
    pub fn create_state(&mut self, initial_value: MettaValue) -> u64 {
        self.make_owned();

        let id = {
            let mut next_id = self.shared.next_state_id.write().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.shared.states.write().unwrap().insert(id, initial_value);

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Get the current value of a state cell
    /// Used by get-state operation
    pub fn get_state(&self, state_id: u64) -> Option<MettaValue> {
        self.shared.states.read().unwrap().get(&state_id).cloned()
    }

    /// Change the value of a state cell
    /// Used by change-state! operation
    /// Returns true if successful, false if state doesn't exist
    pub fn change_state(&mut self, state_id: u64, new_value: MettaValue) -> bool {
        self.make_owned();

        let mut states = self.shared.states.write().unwrap();
        if let std::collections::hash_map::Entry::Occupied(mut e) = states.entry(state_id) {
            e.insert(new_value);
            self.modified.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Check if a state cell exists
    pub fn has_state(&self, state_id: u64) -> bool {
        self.shared.states.read().unwrap().contains_key(&state_id)
    }

    // ============================================================
    // Symbol Bindings Management (bind!)
    // ============================================================

    /// Bind a symbol to a value
    /// Used by bind! operation
    pub fn bind(&mut self, symbol: &str, value: MettaValue) {
        self.make_owned();

        self.shared.bindings
            .write()
            .unwrap()
            .insert(symbol.to_string(), value);

        // Also register in fuzzy matcher for suggestions
        self.shared.fuzzy_matcher.write().unwrap().insert(symbol);

        self.modified.store(true, Ordering::Release);
    }

    /// Get the value bound to a symbol
    /// Used for symbol resolution
    pub fn get_binding(&self, symbol: &str) -> Option<MettaValue> {
        self.shared.bindings.read().unwrap().get(symbol).cloned()
    }

    /// Check if a symbol is bound
    pub fn has_binding(&self, symbol: &str) -> bool {
        self.shared.bindings.read().unwrap().contains_key(symbol)
    }

    // ============================================================
    // Tokenizer Operations (bind! support)
    // ============================================================

    /// Register a token with its value in the tokenizer
    /// Used by bind! to register tokens for later resolution
    /// HE-compatible: tokens registered here affect subsequent atom resolution
    pub fn register_token(&mut self, token: &str, value: MettaValue) {
        self.make_owned();
        self.shared.tokenizer
            .write()
            .unwrap()
            .register_token_value(token, value);
        // Also register in fuzzy matcher for suggestions
        self.shared.fuzzy_matcher.write().unwrap().insert(token);
        self.modified.store(true, Ordering::Release);
    }

    /// Look up a token in the tokenizer
    /// Returns the bound value if found
    pub fn lookup_token(&self, token: &str) -> Option<MettaValue> {
        self.shared.tokenizer.read().unwrap().lookup(token)
    }

    /// Check if a token is registered in the tokenizer
    pub fn has_token(&self, token: &str) -> bool {
        self.shared.tokenizer.read().unwrap().has_token(token)
    }

    // ============================================================
    // Module Operations
    // ============================================================

    /// Get the current module path (directory of the executing module)
    pub fn current_module_dir(&self) -> Option<&std::path::Path> {
        self.current_module_path.as_deref()
    }

    /// Set the current module path
    pub fn set_current_module_path(&mut self, path: Option<PathBuf>) {
        self.current_module_path = path;
    }

    /// Check if a module is cached by path
    pub fn get_module_by_path(&self, path: &std::path::Path) -> Option<ModId> {
        self.shared.module_registry.read().unwrap().get_by_path(path)
    }

    /// Check if a module is cached by content hash
    pub fn get_module_by_content(&self, content_hash: u64) -> Option<ModId> {
        self.shared.module_registry
            .read()
            .unwrap()
            .get_by_content(content_hash)
    }

    /// Check if a module is currently being loaded (cycle detection)
    pub fn is_module_loading(&self, content_hash: u64) -> bool {
        self.shared.module_registry
            .read()
            .unwrap()
            .is_loading(content_hash)
    }

    /// Mark a module as being loaded
    pub fn mark_module_loading(&self, content_hash: u64) {
        self.shared.module_registry
            .write()
            .unwrap()
            .mark_loading(content_hash);
    }

    /// Unmark a module as loading
    pub fn unmark_module_loading(&self, content_hash: u64) {
        self.shared.module_registry
            .write()
            .unwrap()
            .unmark_loading(content_hash);
    }

    /// Register a new module in the registry
    pub fn register_module(
        &self,
        mod_path: String,
        file_path: &std::path::Path,
        content_hash: u64,
        resource_dir: Option<PathBuf>,
    ) -> ModId {
        self.shared.module_registry.write().unwrap().register(
            mod_path,
            file_path,
            content_hash,
            resource_dir,
        )
    }

    /// Add a path alias for an existing module
    pub fn add_module_path_alias(&self, path: &std::path::Path, mod_id: ModId) {
        self.shared.module_registry
            .write()
            .unwrap()
            .add_path_alias(path, mod_id);
    }

    /// Get the number of loaded modules
    pub fn module_count(&self) -> usize {
        self.shared.module_registry.read().unwrap().module_count()
    }

    /// Get a module's space by its ModId.
    ///
    /// Returns an Arc reference to the module's ModuleSpace for live access.
    /// This is used by `mod-space!` to create live space references.
    pub fn get_module_space(
        &self,
        mod_id: ModId,
    ) -> Option<std::sync::Arc<RwLock<crate::backend::modules::ModuleSpace>>> {
        let registry = self.shared.module_registry.read().unwrap();
        registry.get(mod_id).map(|module| module.space().clone())
    }

    /// Get the current module's space as a SpaceHandle ("&self" reference).
    ///
    /// Returns a SpaceHandle for the current module's space, or a new empty
    /// space if not currently inside a module evaluation.
    ///
    /// This is used to implement the `&self` token for match and space operations.
    pub fn self_space(&self) -> crate::backend::models::SpaceHandle {
        use crate::backend::models::SpaceHandle;

        // If we're inside a module, return its space
        if let Some(mod_path) = &self.current_module_path {
            if let Some(mod_id) = self.get_module_by_path(mod_path) {
                if let Some(space) = self.get_module_space(mod_id) {
                    return SpaceHandle::for_module(mod_id, "self".to_string(), space);
                }
            }
        }

        // Fallback: return the "self" named space if it exists, otherwise create empty
        // Use ID 0 for the global "self" space
        SpaceHandle::new(0, "self".to_string())
    }

    /// Check if strict mode is enabled
    pub fn is_strict_mode(&self) -> bool {
        self.shared.module_registry.read().unwrap().options().strict_mode
    }

    /// Enable or disable strict mode.
    ///
    /// When enabled:
    /// - Only submodules can be imported
    /// - Transitive imports are disabled
    /// - Cyclic imports are disallowed
    ///
    /// When disabled: HE-compatible permissive mode
    pub fn set_strict_mode(&mut self, strict: bool) {
        use super::modules::LoadOptions;
        self.make_owned();
        let options = if strict {
            LoadOptions::strict()
        } else {
            LoadOptions::permissive()
        };
        self.shared.module_registry.write().unwrap().set_options(options);
    }

    /// Get rules matching a specific head symbol and arity
    /// Returns Vec<Rule> for O(1) lookup instead of O(n) iteration
    /// Also includes wildcard rules that must be checked against all queries
    pub fn get_matching_rules(&self, head: &str, arity: usize) -> Vec<Rule> {
        // OPTIMIZATION: Single allocation for key to avoid double allocation
        let key = (head.to_owned(), arity);

        // Get indexed rules and wildcards in single lock scope where possible
        let index = self.shared.rule_index.read().unwrap();
        let wildcards = self.shared.wildcard_rules.read().unwrap();

        let indexed_rules = index.get(&key);
        let indexed_len = indexed_rules.map_or(0, |r| r.len());
        let wildcard_len = wildcards.len();

        // OPTIMIZATION: Preallocate capacity to avoid reallocation
        let mut matching_rules = Vec::with_capacity(indexed_len + wildcard_len);

        // Get indexed rules with matching head symbol and arity
        if let Some(rules) = indexed_rules {
            matching_rules.extend(rules.iter().cloned());
        }

        // Also include wildcard rules (must always be checked)
        matching_rules.extend(wildcards.iter().cloned());

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
        self.shared.fuzzy_matcher.read().unwrap().suggest(query, max_distance)
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
        self.shared.fuzzy_matcher.read().unwrap().did_you_mean(symbol, max_distance, 3)
    }

    /// Get a smart "Did you mean?" suggestion with sophisticated heuristics
    ///
    /// Unlike `did_you_mean`, this method applies heuristics to avoid false positives:
    /// - Rejects suggestions for short words (< 4 chars for distance 1)
    /// - Detects data constructor patterns (PascalCase, hyphenated names)
    /// - Considers relative edit distance (distance/length ratio)
    /// - Returns confidence level for appropriate error/warning handling
    ///
    /// # Returns
    /// - `Some(SmartSuggestion)` with message and confidence level
    /// - `None` if no appropriate suggestion is found
    ///
    /// # Example
    /// ```ignore
    /// if let Some(suggestion) = env.smart_did_you_mean("fibonaci", 2) {
    ///     match suggestion.confidence {
    ///         SuggestionConfidence::High => eprintln!("Warning: {}", suggestion.message),
    ///         SuggestionConfidence::Low => eprintln!("Note: {}", suggestion.message),
    ///         SuggestionConfidence::None => {} // Don't show anything
    ///     }
    /// }
    /// ```
    pub fn smart_did_you_mean(
        &self,
        symbol: &str,
        max_distance: usize,
    ) -> Option<super::fuzzy_match::SmartSuggestion> {
        self.shared
            .fuzzy_matcher
            .read()
            .unwrap()
            .smart_did_you_mean(symbol, max_distance, 3)
    }

    // ============================================================
    // Grounded Operations
    // ============================================================

    /// Get a grounded operation by name (e.g., "+", "-", "and")
    /// Used for lazy evaluation of built-in operations
    pub fn get_grounded_operation(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<dyn super::grounded::GroundedOperation>> {
        self.shared.grounded_registry.read().unwrap().get(name)
    }

    /// Get a TCO-compatible grounded operation by name (e.g., "+", "-", "and")
    /// TCO operations return work items instead of calling eval internally,
    /// enabling deep recursion without stack overflow
    pub fn get_grounded_operation_tco(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<dyn super::grounded::GroundedOperationTCO>> {
        self.shared.grounded_registry_tco.read().unwrap().get(name)
    }

    /// Union two environments (monotonic merge)
    /// PathMap and shared_mapping are shared via Arc, so facts (including type assertions) are automatically merged
    /// Multiplicities and rule indices are also merged via shared Arc
    pub fn union(&self, _other: &Environment) -> Environment {
        // All shared state is now consolidated into single Arc<EnvironmentShared>
        // Clone is O(1) - just one atomic increment instead of 17
        Environment {
            shared: Arc::clone(&self.shared),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: union creates a new shared environment
            modified: Arc::new(AtomicBool::new(false)), // CoW: fresh modification tracker
            current_module_path: self.current_module_path.clone(),
        }
    }
}

/// CoW: Manual Clone implementation
/// Clones share data (owns_data = false) until first modification triggers make_owned()
///
/// Performance: O(1) clone via single Arc increment instead of 17 separate Arc clones
impl Clone for Environment {
    fn clone(&self) -> Self {
        Environment {
            // O(1): Single atomic increment for all shared state
            shared: Arc::clone(&self.shared),
            shared_mapping: self.shared_mapping.clone(),
            owns_data: false, // CoW: clones do not own data initially
            modified: Arc::new(AtomicBool::new(false)), // CoW: fresh modification tracker
            current_module_path: self.current_module_path.clone(),
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
        Rule::new(
            MettaValue::Atom(lhs.to_string()),
            MettaValue::Atom(rhs.to_string()),
        )
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
    #[allow(dead_code)]
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

        // Get Arc pointer addresses before clone (consolidated shared pointer)
        let shared_ptr_before = StdArc::as_ptr(&env.shared);

        let clone = env.clone();

        // Get Arc pointer addresses after clone
        let shared_ptr_after = StdArc::as_ptr(&clone.shared);

        // Pointers should be identical (shared) - O(1) clone
        assert_eq!(
            shared_ptr_before, shared_ptr_after,
            "Clone should share consolidated Arc"
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
        let btm_ptr_before = StdArc::as_ptr(&clone.shared);

        // First mutation triggers make_owned()
        clone.add_rule(make_test_rule("(clone $y)", "(cloned $y)"));

        // After mutation
        assert!(clone.owns_data, "Clone should own data after mutation");
        assert!(
            clone.modified.load(Ordering::Acquire),
            "Clone should be marked modified"
        );

        // Arc pointers should be different (deep copy occurred)
        let btm_ptr_after = StdArc::as_ptr(&clone.shared);
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
        let env = Environment::new();
        let mut clone = env.clone();

        // First mutation triggers make_owned()
        clone.add_rule(make_test_rule("(test1 $x)", "(result1 $x)"));
        assert!(
            clone.owns_data,
            "Clone should own data after first mutation"
        );

        // Get Arc pointers after first make_owned()
        let shared_ptr_first = StdArc::as_ptr(&clone.shared);

        // Second mutation should NOT trigger another make_owned()
        clone.add_rule(make_test_rule("(test2 $y)", "(result2 $y)"));

        // Arc pointers should be same (no second deep copy)
        let shared_ptr_second = StdArc::as_ptr(&clone.shared);
        assert_eq!(
            shared_ptr_first, shared_ptr_second,
            "make_owned() should not run twice"
        );
    }

    #[test]
    fn test_deep_clone_copies_all_fields() {
        // Test: make_owned() should deep copy the consolidated shared state
        // (All 17 RwLock fields are now in one Arc<EnvironmentShared>)
        let mut env = Environment::new();
        env.add_rule(make_test_rule("(test $x)", "(result $x)"));

        let mut clone = env.clone();

        // Get Arc pointer before mutation (single consolidated pointer)
        let shared_before = StdArc::as_ptr(&clone.shared);

        // Trigger make_owned()
        clone.add_rule(make_test_rule("(clone $y)", "(cloned $y)"));

        // Get Arc pointer after mutation
        let shared_after = StdArc::as_ptr(&clone.shared);

        // The consolidated Arc should be different (deep copy occurred)
        assert_ne!(
            shared_before, shared_after,
            "shared should be deep copied after make_owned()"
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

            // Verify Arc pointers are different (consolidated shared pointer)
            let env_ptr = StdArc::as_ptr(&env.shared);
            let clone_ptr = StdArc::as_ptr(&clone.shared);
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

        Rule::new(lhs, rhs)
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
                    let rule = Rule::new(
        MettaValue::Atom(pattern),
        MettaValue::Atom(format!("(res{} $x)", i)),
    );
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
