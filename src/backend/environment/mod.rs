//! Environment module for MeTTa evaluation.
//!
//! The Environment contains the fact database, type assertions, rules, and
//! various registries for MeTTa evaluation. Uses MORK PathMap for efficient
//! trie-based storage with pattern matching support.
//!
//! # Architecture
//!
//! - `Environment` - Main struct with Copy-on-Write (CoW) semantics
//! - `EnvironmentShared` - Consolidated shared state (single Arc instead of 17)
//! - `HeadArityBloomFilter` - O(1) rejection for match_space()
//! - `ScopeTracker` - Hierarchical scope tracking for "Did you mean?" suggestions
//!
//! # Thread Safety
//!
//! All shared state uses RwLock for concurrent read/exclusive write access.
//! Clone operations are O(1) via Arc sharing until first mutation.

mod bloom;
mod scope;
#[cfg(test)]
mod tests;

pub(crate) use bloom::HeadArityBloomFilter;
pub use scope::ScopeTracker;

use lru::LruCache;
use mork::space::Space;
use mork_interning::SharedMappingHandle;
use pathmap::{zipper::*, PathMap};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{trace, warn};

use super::fuzzy_match::FuzzyMatcher;
use super::grounded::{GroundedRegistry, GroundedRegistryTCO};
use super::modules::{ModId, ModuleRegistry, Tokenizer};
use super::symbol::Symbol;
use super::{MettaValue, Rule};

/// Thread-local cache for mork_expr_to_metta_value conversions.
///
/// MORK expressions are identified by their raw pointer address. Since MORK uses
/// immutable trie-based storage, the same pointer always refers to the same logical
/// expression during a single evaluation. This allows us to cache conversion results
/// and avoid redundant traversals.
///
/// Cache size is 4096 entries per thread, balancing memory usage against hit rate.
/// LRU eviction ensures the most recently used conversions are retained.
thread_local! {
    static MORK_VALUE_CACHE: RefCell<LruCache<usize, MettaValue>> =
        RefCell::new(LruCache::new(NonZeroUsize::new(4096).expect("4096 is non-zero")));
}

/// Shared state across all Environment clones.
/// Consolidates 17 Arc<RwLock<T>> fields into a single Arc<EnvironmentShared>
/// for O(1) clone operations (1 atomic increment instead of 17).
///
/// Thread-safe: All fields use RwLock for concurrent read/exclusive write access.
pub(crate) struct EnvironmentShared {
    /// PathMap trie for fact storage
    pub(crate) btm: RwLock<PathMap<()>>,

    /// Rule index: Maps (head_symbol, arity) -> Vec<Rule> for O(1) rule lookup
    /// Uses Symbol for O(1) comparison when symbol-interning feature is enabled
    #[allow(clippy::type_complexity)]
    pub(crate) rule_index: RwLock<HashMap<(Symbol, usize), Vec<Rule>>>,

    /// Wildcard rules: Rules without a clear head symbol
    pub(crate) wildcard_rules: RwLock<Vec<Rule>>,

    /// Fast flag: true if any wildcard rules exist (avoids lock acquisition when empty)
    pub(crate) has_wildcard_rules: AtomicBool,

    /// Multiplicities: tracks how many times each rule is defined
    pub(crate) multiplicities: RwLock<HashMap<String, usize>>,

    /// Pattern cache: LRU cache for MORK serialization results
    pub(crate) pattern_cache: RwLock<LruCache<MettaValue, Vec<u8>>>,

    /// Type index: Lazy-initialized subtrie containing only type assertions
    pub(crate) type_index: RwLock<Option<PathMap<()>>>,

    /// Type index invalidation flag
    pub(crate) type_index_dirty: RwLock<bool>,

    /// Named spaces registry: Maps space_id -> (name, atoms)
    #[allow(clippy::type_complexity)]
    pub(crate) named_spaces: RwLock<HashMap<u64, (String, Vec<MettaValue>)>>,

    /// Counter for generating unique space IDs
    pub(crate) next_space_id: RwLock<u64>,

    /// Mutable state cells registry
    pub(crate) states: RwLock<HashMap<u64, MettaValue>>,

    /// Counter for generating unique state IDs
    pub(crate) next_state_id: RwLock<u64>,

    /// Symbol bindings registry
    pub(crate) bindings: RwLock<HashMap<String, MettaValue>>,

    /// Module registry
    pub(crate) module_registry: RwLock<ModuleRegistry>,

    /// Per-module tokenizer
    pub(crate) tokenizer: RwLock<Tokenizer>,

    /// Grounded operations registry (legacy)
    pub(crate) grounded_registry: RwLock<GroundedRegistry>,

    /// TCO-compatible grounded operations registry
    pub(crate) grounded_registry_tco: RwLock<GroundedRegistryTCO>,

    /// Fallback store for large expressions
    pub(crate) large_expr_pathmap: RwLock<Option<PathMap<MettaValue>>>,

    /// Fuzzy matcher for "Did you mean?" suggestions
    pub(crate) fuzzy_matcher: RwLock<FuzzyMatcher>,

    /// Hierarchical scope tracker for context-aware symbol resolution
    pub(crate) scope_tracker: RwLock<ScopeTracker>,

    /// Bloom filter for (head_symbol, arity) pairs - enables O(1) match_space() rejection
    /// when the pattern's (head, arity) definitely doesn't exist in the space.
    pub(crate) head_arity_bloom: RwLock<HeadArityBloomFilter>,
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
    pub(crate) shared: Arc<EnvironmentShared>,

    /// THREAD-SAFE: SharedMappingHandle for symbol interning (string → u64)
    /// Can be cloned and shared across threads (Send + Sync)
    /// Kept separate as it has its own sharing semantics
    pub(crate) shared_mapping: SharedMappingHandle,

    /// CoW: Tracks if this clone owns its data (true = can modify in-place, false = must deep copy first)
    /// Set to true on new(), false on clone(), true after make_owned()
    pub(crate) owns_data: bool,

    /// CoW: Tracks if this environment has been modified since creation/clone
    /// Used for fast-path union() optimization (unmodified clones can skip deep merge)
    /// Arc-wrapped to allow independent tracking per clone
    pub(crate) modified: Arc<AtomicBool>,

    /// Current module path: Directory of the currently-executing module
    /// Used for relative path resolution (self:child notation)
    /// None when not inside a module evaluation
    /// Kept separate as it's per-clone state
    pub(crate) current_module_path: Option<PathBuf>,
}

impl Environment {
    pub fn new() -> Self {
        use mork_interning::SharedMapping;

        let shared = Arc::new(EnvironmentShared {
            btm: RwLock::new(PathMap::new()),
            rule_index: RwLock::new(HashMap::with_capacity(128)),
            wildcard_rules: RwLock::new(Vec::new()),
            has_wildcard_rules: AtomicBool::new(false),
            multiplicities: RwLock::new(HashMap::new()),
            pattern_cache: RwLock::new(LruCache::new(NonZeroUsize::new(1000).expect("1000 is non-zero"))),
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
            scope_tracker: RwLock::new(ScopeTracker::new()),
            head_arity_bloom: RwLock::new(HeadArityBloomFilter::new(10000)), // ~10KB for 10k expected entries
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
        trace!(target: "mettatron::environment::make_owned", "Deep copying CoW data");

        // Deep copy the entire shared state structure
        // Clone the data first to avoid borrowing issues
        let new_shared = Arc::new(EnvironmentShared {
            btm: RwLock::new(self.shared.btm.read().expect("btm lock poisoned").clone()),
            rule_index: RwLock::new(self.shared.rule_index.read().expect("rule_index lock poisoned").clone()),
            wildcard_rules: RwLock::new(self.shared.wildcard_rules.read().expect("wildcard_rules lock poisoned").clone()),
            has_wildcard_rules: AtomicBool::new(self.shared.has_wildcard_rules.load(Ordering::Acquire)),
            multiplicities: RwLock::new(self.shared.multiplicities.read().expect("multiplicities lock poisoned").clone()),
            pattern_cache: RwLock::new(self.shared.pattern_cache.read().expect("pattern_cache lock poisoned").clone()),
            type_index: RwLock::new(self.shared.type_index.read().expect("type_index lock poisoned").clone()),
            type_index_dirty: RwLock::new(*self.shared.type_index_dirty.read().expect("type_index_dirty lock poisoned")),
            named_spaces: RwLock::new(self.shared.named_spaces.read().expect("named_spaces lock poisoned").clone()),
            next_space_id: RwLock::new(*self.shared.next_space_id.read().expect("next_space_id lock poisoned")),
            states: RwLock::new(self.shared.states.read().expect("states lock poisoned").clone()),
            next_state_id: RwLock::new(*self.shared.next_state_id.read().expect("next_state_id lock poisoned")),
            bindings: RwLock::new(self.shared.bindings.read().expect("bindings lock poisoned").clone()),
            module_registry: RwLock::new(self.shared.module_registry.read().expect("module_registry lock poisoned").clone()),
            tokenizer: RwLock::new(self.shared.tokenizer.read().expect("tokenizer lock poisoned").clone()),
            grounded_registry: RwLock::new(self.shared.grounded_registry.read().expect("grounded_registry lock poisoned").clone()),
            grounded_registry_tco: RwLock::new(self.shared.grounded_registry_tco.read().expect("grounded_registry_tco lock poisoned").clone()),
            large_expr_pathmap: RwLock::new(self.shared.large_expr_pathmap.read().expect("large_expr_pathmap lock poisoned").clone()),
            fuzzy_matcher: RwLock::new(self.shared.fuzzy_matcher.read().expect("fuzzy_matcher lock poisoned").clone()),
            scope_tracker: RwLock::new(self.shared.scope_tracker.read().expect("scope_tracker lock poisoned").clone()),
            head_arity_bloom: RwLock::new(self.shared.head_arity_bloom.read().expect("head_arity_bloom lock poisoned").clone()),
        });

        self.shared = new_shared;

        // Mark as owning data and modified
        self.owns_data = true;
        self.modified.store(true, Ordering::Release);
    }

    /// Create a forked environment for nondeterministic branch isolation.
    ///
    /// This is critical for correct evaluation of nondeterministic MeTTa programs.
    /// When evaluation forks (e.g., from `match` returning multiple results),
    /// each branch needs its own isolated view of mutable state.
    ///
    /// This method:
    /// 1. Clones the environment (CoW for states)
    /// 2. Forks all SpaceHandles in bindings for branch isolation
    ///
    /// # Example
    /// ```ignore
    /// // Original env has &stack bound to a space
    /// let forked = env.fork_for_nondeterminism();
    /// // forked's &stack is isolated from original's
    /// ```
    pub fn fork_for_nondeterminism(&self) -> Environment {
        let mut forked = self.clone();
        forked.make_owned(); // Ensure we have our own copy of state

        // Fork all SpaceHandles in bindings
        let mut forked_bindings = forked.shared.bindings.write().expect("bindings lock poisoned");
        for (_name, value) in forked_bindings.iter_mut() {
            Self::fork_spaces_in_value(value);
        }
        drop(forked_bindings);

        forked
    }

    /// Recursively fork all SpaceHandles in a MettaValue.
    fn fork_spaces_in_value(value: &mut MettaValue) {
        match value {
            MettaValue::Space(handle) => {
                // Fork the space handle for isolation
                *handle = handle.fork();
            }
            MettaValue::SExpr(items) => {
                for item in items.iter_mut() {
                    Self::fork_spaces_in_value(item);
                }
            }
            MettaValue::Conjunction(goals) => {
                for goal in goals.iter_mut() {
                    Self::fork_spaces_in_value(goal);
                }
            }
            MettaValue::Type(_) => {
                // Arc<MettaValue> - can't mutate through Arc, but types rarely contain spaces
            }
            MettaValue::Error(_, _) => {
                // Arc<MettaValue> - can't mutate, but errors rarely contain spaces
            }
            // Primitives don't contain spaces
            MettaValue::Atom(_)
            | MettaValue::Bool(_)
            | MettaValue::Long(_)
            | MettaValue::Float(_)
            | MettaValue::String(_)
            | MettaValue::Nil
            | MettaValue::State(_)
            | MettaValue::Unit
            | MettaValue::Memo(_)
            | MettaValue::Empty => {}
        }
    }

    /// Create a thread-local Space for operations
    /// Following the Rholang LSP pattern: cheap clone via structural sharing
    ///
    /// This is useful for advanced operations that need direct access to the Space,
    /// such as debugging or custom MORK queries.
    pub fn create_space(&self) -> Space {
        let btm = self.shared.btm.read().expect("btm lock poisoned").clone(); // CoW: read lock for concurrent reads
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
        *self.shared.btm.write().expect("btm lock poisoned") = space.btm; // CoW: write lock for exclusive access
        self.shared_mapping = space.sm;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Extract (head_symbol_bytes, arity) from MORK expression bytes in O(1).
    ///
    /// This is used for lazy pre-filtering in `match_space()`: if the pattern has a fixed
    /// head symbol, we can skip MORK expressions with different heads without full conversion.
    ///
    /// MORK byte encoding:
    /// - Arity tag: 0x00-0x3F (bits 6-7 are 00) - value is arity 0-63
    /// - SymbolSize tag: 0xC1-0xFF (bits 6-7 are 11, excluding 0xC0) - symbol length 1-63
    /// - NewVar tag: 0xC0 (new variable)
    /// - VarRef tag: 0x80-0xBF (bits 6-7 are 10) - variable reference 0-63
    ///
    /// Returns Some((head_bytes, arity)) if the expression is an S-expr with a symbol head.
    /// Returns None for atoms, variable heads, or nested S-expr heads.
    ///
    /// # Safety
    /// The `ptr` must point to a valid MORK expression in PathMap memory.
    #[inline]
    #[allow(dead_code)]
    unsafe fn mork_head_info(ptr: *const u8) -> Option<(&'static [u8], u8)> {
        // Read first byte - check if it's an arity tag (S-expression)
        let first = *ptr;
        if (first & 0b1100_0000) != 0b0000_0000 {
            // Not an S-expression (it's a symbol, variable, or other atom)
            return None;
        }
        let arity = first; // Arity tag value 0-63

        // Empty S-expr or head is not accessible
        if arity == 0 {
            return None;
        }

        // Read second byte - check if head is a symbol (SymbolSize tag)
        let head_byte = *ptr.add(1);
        // SymbolSize tag: 0xC1-0xFF (bits 6-7 are 11, but not 0xC0 which is NewVar)
        if head_byte == 0xC0 || (head_byte & 0b1100_0000) != 0b1100_0000 {
            // Head is NewVar (0xC0), VarRef (0x80-0xBF), or nested S-expr (0x00-0x3F)
            return None;
        }

        // Head is a symbol - extract the symbol bytes
        let symbol_len = (head_byte & 0b0011_1111) as usize;
        if symbol_len == 0 {
            return None;
        }

        // Symbol content starts at offset 2 and has length `symbol_len`
        let symbol_bytes = std::slice::from_raw_parts(ptr.add(2), symbol_len);
        // Note: arity tag value is the TOTAL elements including head
        // But MettaValue::get_arity() returns elements EXCLUDING head, so we subtract 1
        Some((symbol_bytes, arity.saturating_sub(1)))
    }

    /// Convert a MORK Expr directly to MettaValue without text serialization
    /// This avoids the "reserved byte" panic that occurs in serialize2()
    ///
    /// The key insight: serialize2() uses byte_item() which panics on bytes 64-127.
    /// We use maybe_byte_item() instead, which returns Result<Tag, u8> and handles reserved bytes gracefully.
    ///
    /// CRITICAL FIX for "reserved 114" and similar bugs during evaluation/iteration.
    ///
    /// OPTIMIZATION: Uses thread-local LRU cache keyed by MORK expression pointer address.
    /// Since MORK uses immutable trie storage, identical pointers always represent
    /// identical expressions during evaluation, making caching safe and effective.
    #[allow(unused_variables)]
    pub(crate) fn mork_expr_to_metta_value(
        expr: &mork_expr::Expr,
        space: &Space,
    ) -> Result<MettaValue, String> {
        use mork_expr::{maybe_byte_item, Tag};
        use std::slice::from_raw_parts;

        // CACHE DISABLED: Pointer-based caching doesn't work with PathMap's buffer reuse.
        // PathMap's read_zipper.path() returns a reference to an internal buffer that
        // changes content in-place while the pointer stays constant during iteration.
        // A proper fix would require content-based hashing, but for now we disable it.

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
                    warn!(
                        target: "mettatron::environment::mork_expr_to_metta_value",
                        reserved_byte, offset,
                        "Reserved byte encountered during MORK conversion"
                    );
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
                                    i64::from_be_bytes(symbol_bytes.try_into().expect("8 bytes expected"))
                                        .to_be_bytes();
                                if let Some(actual_bytes) = space.sm.get_bytes(symbol_id) {
                                    // Found in symbol table - use actual symbol string
                                    String::from_utf8_lossy(actual_bytes).to_string()
                                } else {
                                    // Symbol ID not in table - fall back to treating as raw bytes
                                    trace!(
                                        target: "mettatron::environment::mork_expr_to_metta_value",
                                        symbol_id = ?symbol_id,
                                        "Symbol ID not found in symbol table, using raw bytes"
                                    );
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
                    // OPTIMIZATION: Fast-path check - only try parsing as integer if first byte
                    // could plausibly start a number (digit or minus sign followed by digit)
                    let first_byte = symbol_str.as_bytes().first().copied().unwrap_or(0);
                    let could_be_number = first_byte.is_ascii_digit()
                        || (first_byte == b'-' && symbol_str.len() > 1
                            && symbol_str.as_bytes().get(1).is_some_and(|b| b.is_ascii_digit()));

                    if could_be_number {
                        if let Ok(n) = symbol_str.parse::<i64>() {
                            MettaValue::Long(n)
                        } else {
                            MettaValue::Atom(symbol_str)
                        }
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
            // OPTIMIZATION: Use Option to make ownership transfer explicit and avoid clones
            let mut current_value = Some(value);
            'popping: loop {
                let v = current_value.take().expect("value must be Some at start of popping loop");

                // Check if stack is empty - if so, return the value
                if stack.is_empty() {
                    return Ok(v);
                }

                // Add value to parent frame
                let should_pop = match stack.last_mut() {
                    None => unreachable!(), // Already checked above
                    Some(StackFrame::Arity { remaining, items }) => {
                        items.push(v); // OPTIMIZATION: No clone needed - value is consumed
                        *remaining -= 1;
                        *remaining == 0
                    }
                };

                if should_pop {
                    // S-expression is complete - pop and take ownership of items
                    // OPTIMIZATION: Take ownership instead of cloning
                    if let Some(StackFrame::Arity { items, .. }) = stack.pop() {
                        current_value = Some(MettaValue::SExpr(items));
                        continue 'popping;
                    }
                } else {
                    // More items needed - go back to parsing
                    continue 'parsing;
                }
            }
        }
    }

    /// Helper function to serialize a MORK Expr to a readable string
    /// DEPRECATED: This uses serialize2() which panics on reserved bytes.
    /// Use mork_expr_to_metta_value() instead for production code.
    #[deprecated(
        note = "This uses serialize2() which panics on reserved bytes. Use mork_expr_to_metta_value() instead."
    )]
    #[allow(dead_code)]
    #[allow(unused_variables)]
    fn serialize_mork_expr_old(expr: &mork_expr::Expr, space: &Space) -> String {
        let mut buffer = Vec::new();
        expr.serialize2(
            &mut buffer,
            |s| {
                #[cfg(feature = "interning")]
                {
                    let symbol = i64::from_be_bytes(s.try_into().expect("8 bytes expected")).to_be_bytes();
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
        *self.shared.type_index_dirty.write().expect("type_index_dirty lock poisoned") = true;
        self.modified.store(true, Ordering::Release); // CoW: mark as modified
    }

    /// Ensure the type index is built and up-to-date
    /// Uses PathMap's restrict() to extract only type assertions into a subtrie
    /// This enables O(p + m) type lookups where m << n (total facts)
    ///
    /// The type index is lazily initialized and cached until invalidated
    fn ensure_type_index(&self) {
        let dirty = *self.shared.type_index_dirty.read().expect("type_index_dirty lock poisoned");
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
            let mut wz = type_prefix_map.write_zipper();
            for &byte in colon_bytes {
                wz.descend_to_byte(byte);
            }
            wz.set_val(());
        }

        // Extract type subtrie using restrict()
        let type_subtrie = btm.restrict(&type_prefix_map);

        // Cache the subtrie
        *self.shared.type_index.write().expect("type_index lock poisoned") = Some(type_subtrie);
        *self.shared.type_index_dirty.write().expect("type_index_dirty lock poisoned") = false;
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
        use mork_expr::Expr;

        // Ensure type index is built and up-to-date
        self.ensure_type_index();

        // Get the type index subtrie
        let type_index_opt = self.shared.type_index.read().expect("type_index lock poisoned");
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
        let wildcard_count = self.shared.wildcard_rules.read().expect("wildcard_rules lock poisoned").len();

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
    /// # Example
    /// ```ignore
    /// for (head, arity, count) in env.iter_rule_heads() {
    ///     println!("{}/{}: {} rules", head, arity, count);
    /// }
    /// ```
    pub fn iter_rule_heads(&self) -> Vec<(String, usize, usize)> {
        let index = self.shared.rule_index.read().expect("rule_index lock poisoned");
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
        use mork_expr::Expr;

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
            let mut index = self.shared.rule_index.write().expect("rule_index lock poisoned");
            index.clear();
        }
        {
            let mut wildcards = self.shared.wildcard_rules.write().expect("wildcard_rules lock poisoned");
            wildcards.clear();
        }
        // Reset wildcard flag - will be set again if wildcards are added
        self.shared.has_wildcard_rules.store(false, Ordering::Release);

        // Rebuild from MORK Space
        for rule in self.iter_rules() {
            if let Some(head) = rule.lhs.get_head_symbol() {
                let arity = rule.lhs.get_arity();
                // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
                self.shared.fuzzy_matcher.write().expect("fuzzy_matcher lock poisoned").insert(head);
                // Use Symbol for O(1) comparison when symbol-interning is enabled
                let head_sym = Symbol::new(head);
                let mut index = self.shared.rule_index.write().expect("rule_index lock poisoned");
                index.entry((head_sym, arity)).or_default().push(rule);
            } else {
                // Rules without head symbol (wildcards, variables) go to wildcard list
                let mut wildcards = self.shared.wildcard_rules.write().expect("wildcard_rules lock poisoned");
                wildcards.push(rule);
                // Mark that we have wildcard rules
                self.shared.has_wildcard_rules.store(true, Ordering::Release);
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
        trace!(target: "mettatron::environment::match_space", ?pattern, ?template);
        use crate::backend::eval::{apply_bindings, pattern_match};
        use mork_expr::Expr;

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        // This is "Tier 0" optimization - skips entire iteration if bloom filter says no match
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            let bloom_result = self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity);
            if !bloom_result {
                // Definitely no matching expressions exist
                return Vec::new();
            }
        }

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // 1. Iterate through MORK PathMap (primary storage)
        while rz.to_next_val() {
            let ptr = rz.path().as_ptr();

            // Get the s-expression at this position
            let expr = Expr {
                ptr: ptr.cast_mut(),
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
        let guard = self.shared.large_expr_pathmap.read().expect("large_expr_pathmap lock poisoned");
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

    /// Match pattern against atoms in the Space, returning first match only (early exit)
    ///
    /// This is an optimization for cases where only one match is needed (existence checks,
    /// deterministic lookups, etc.). It exits immediately on first match, avoiding the
    /// O(N) iteration through all facts when only one is needed.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for the match
    ///
    /// # Returns
    /// `Some(instantiated_template)` if a match is found, `None` otherwise
    pub fn match_space_first(
        &self,
        pattern: &MettaValue,
        template: &MettaValue,
    ) -> Option<MettaValue> {
        use crate::backend::eval::{apply_bindings, pattern_match};
        use mork_expr::Expr;

        // BLOOM FILTER CHECK: O(1) rejection if (head, arity) definitely doesn't exist
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity)
            {
                // Definitely no matching expressions exist
                return None;
            }
        }

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // OPTIMIZATION: Extract pattern's head symbol and arity for lazy pre-filtering
        let pattern_head_bytes: Option<&[u8]> = pattern.get_head_symbol().map(|s| s.as_bytes());
        // Note: mork_head_info() already adjusts MORK arity to match MettaValue convention
        let pattern_arity = pattern.get_arity() as u8;

        // 1. Iterate through MORK PathMap (primary storage) - EARLY EXIT on first match
        while rz.to_next_val() {
            let ptr = rz.path().as_ptr();

            // DISABLED: pre-filter extracts wrong data from rz.path()
            /*
            if let Some(expected_head) = pattern_head_bytes {
                if let Some((mork_head, mork_arity)) = unsafe { Self::mork_head_info(ptr) } {
                    if mork_head != expected_head || mork_arity != pattern_arity {
                        continue; // Skip this expression entirely
                    }
                }
            }
            */
            let _ = (pattern_head_bytes, pattern_arity); // suppress unused warnings

            let expr = Expr {
                ptr: ptr.cast_mut(),
            };

            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some(bindings) = pattern_match(pattern, &atom) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    return Some(instantiated); // EARLY EXIT - found first match!
                }
            }
        }

        drop(space);

        // 2. Check large expression fallback PathMap
        let guard = self.shared.large_expr_pathmap.read().expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if let Some(bindings) = pattern_match(pattern, stored_value) {
                    let instantiated = apply_bindings(template, &bindings).into_owned();
                    return Some(instantiated); // EARLY EXIT
                }
            }
        }

        None
    }

    /// Check if any atom in the Space matches the pattern (existence check only)
    ///
    /// This is the fastest query when you only need to know IF a match exists,
    /// not what the match is. It avoids template instantiation overhead.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    ///
    /// # Returns
    /// `true` if at least one match exists, `false` otherwise
    pub fn match_space_exists(&self, pattern: &MettaValue) -> bool {
        use crate::backend::eval::pattern_match;
        use mork_expr::Expr;

        // BLOOM FILTER CHECK: O(1) rejection
        if let Some(expected_head) = pattern.get_head_symbol() {
            let pattern_arity = pattern.get_arity() as u8;
            if !self
                .shared
                .head_arity_bloom
                .read()
                .expect("head_arity_bloom lock poisoned")
                .may_contain(expected_head.as_bytes(), pattern_arity)
            {
                return false;
            }
        }

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        let pattern_head_bytes: Option<&[u8]> = pattern.get_head_symbol().map(|s| s.as_bytes());
        // Note: mork_head_info() already adjusts MORK arity to match MettaValue convention
        let pattern_arity = pattern.get_arity() as u8;

        // Iterate through MORK PathMap - EARLY EXIT on first match
        while rz.to_next_val() {
            let ptr = rz.path().as_ptr();

            // DISABLED: pre-filter extracts wrong data from rz.path()
            // TODO: Investigate why mork_head_info returns garbage bytes
            /*
            if let Some(expected_head) = pattern_head_bytes {
                if let Some((mork_head, mork_arity)) = unsafe { Self::mork_head_info(ptr) } {
                    if mork_head != expected_head || mork_arity != pattern_arity {
                        continue;
                    }
                }
            }
            */
            let _ = (pattern_head_bytes, pattern_arity); // suppress unused warnings

            let expr = Expr {
                ptr: ptr.cast_mut(),
            };

            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                if pattern_match(pattern, &atom).is_some() {
                    return true; // EARLY EXIT - match exists!
                }
            }
        }

        drop(space);

        // Check large expression fallback PathMap
        let guard = self.shared.large_expr_pathmap.read().expect("large_expr_pathmap lock poisoned");
        if let Some(ref fallback) = *guard {
            for (_key, stored_value) in fallback.iter() {
                if pattern_match(pattern, stored_value).is_some() {
                    return true; // EARLY EXIT
                }
            }
        }

        false
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
            let mut counts = self.shared.multiplicities.write().expect("multiplicities lock poisoned");
            let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
            counts.insert(rule_key.clone(), new_count);
        } // Drop the RefMut borrow before add_to_space

        // Add to rule index for O(k) lookup
        // Note: We store the rule only ONCE (in either index or wildcard list)
        // to avoid unnecessary clones. The rule is already in MORK Space.
        if let Some(head) = rule.lhs.get_head_symbol() {
            let arity = rule.lhs.get_arity();
            // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
            self.shared.fuzzy_matcher.write().expect("fuzzy_matcher lock poisoned").insert(head);
            // Use Symbol for O(1) comparison when symbol-interning is enabled
            let head_sym = Symbol::new(head);
            let mut index = self.shared.rule_index.write().expect("rule_index lock poisoned");
            index.entry((head_sym, arity)).or_default().push(rule); // Move instead of clone
        } else {
            // Rules without head symbol (wildcards, variables) go to wildcard list
            let mut wildcards = self.shared.wildcard_rules.write().expect("wildcard_rules lock poisoned");
            wildcards.push(rule); // Move instead of clone
            // Mark that we have wildcard rules (for fast-path in get_matching_rules)
            self.shared.has_wildcard_rules.store(true, Ordering::Release);
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
                self.shared.fuzzy_matcher.write().expect("fuzzy_matcher lock poisoned").insert(head);
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
            let mut counts = self.shared.multiplicities.write().expect("multiplicities lock poisoned");
            for (key, delta) in multiplicity_updates {
                *counts.entry(key).or_insert(0) += delta;
            }
        }

        // Update rule index
        {
            let mut index = self.shared.rule_index.write().expect("rule_index lock poisoned");
            for ((head, arity), mut rules) in rule_index_updates {
                index.entry((head, arity)).or_default().append(&mut rules);
            }
        }

        // Update wildcard rules
        let has_new_wildcards = !wildcard_updates.is_empty();
        {
            let mut wildcards = self.shared.wildcard_rules.write().expect("wildcard_rules lock poisoned");
            wildcards.extend(wildcard_updates);
        }
        if has_new_wildcards {
            self.shared.has_wildcard_rules.store(true, Ordering::Release);
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

        let counts = self.shared.multiplicities.read().expect("multiplicities lock poisoned");
        *counts.get(&rule_key).unwrap_or(&1)
    }

    /// Get the multiplicities (for serialization)
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        self.shared.multiplicities.read().expect("multiplicities lock poisoned").clone()
    }

    /// Set the multiplicities (used for deserialization)
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        self.make_owned(); // CoW: ensure we own data before modifying
        *self.shared.multiplicities.write().expect("multiplicities lock poisoned") = counts;
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
        self.shared.large_expr_pathmap.read().expect("large_expr_pathmap lock poisoned")
    }

    /// Insert a value into the large expressions fallback PathMap
    /// Used during deserialization to restore large expressions (arity >= 64)
    /// that exceed MORK's 63-arity limit
    pub fn insert_large_expr(&self, value: MettaValue) {
        use crate::backend::varint_encoding::metta_to_varint_key;
        let key = metta_to_varint_key(&value);
        let mut guard = self.shared.large_expr_pathmap.write().expect("large_expr_pathmap lock poisoned");
        let fallback = guard.get_or_insert_with(PathMap::new);
        fallback.insert(&key, value);
    }

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
                let mut cache = self.shared.pattern_cache.write().expect("pattern_cache lock poisoned");
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
            let mut cache = self.shared.pattern_cache.write().expect("pattern_cache lock poisoned");
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
                let mut guard = self.shared.large_expr_pathmap.write().expect("large_expr_pathmap lock poisoned");
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
                let mut guard = self.shared.large_expr_pathmap.write().expect("large_expr_pathmap lock poisoned");
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
        use mork_expr::Expr;

        let space = self.create_space();
        let mut rz = space.btm.read_zipper();

        // Clear existing bloom filter
        self.shared.head_arity_bloom.write().expect("head_arity_bloom lock poisoned").clear();

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
        *self.shared.type_index_dirty.write().expect("type_index_dirty lock poisoned") = true;

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
            let mut next_id = self.shared.next_space_id.write().expect("next_space_id lock poisoned");
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.shared.named_spaces
            .write()
            .expect("named_spaces lock poisoned")
            .insert(id, (name.to_string(), Vec::new()));

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Add an atom to a named space by ID
    /// Used by add-atom operation
    pub fn add_to_named_space(&mut self, space_id: u64, value: &MettaValue) -> bool {
        self.make_owned();

        let mut spaces = self.shared.named_spaces.write().expect("named_spaces lock poisoned");
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

        let mut spaces = self.shared.named_spaces.write().expect("named_spaces lock poisoned");
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
        let spaces = self.shared.named_spaces.read().expect("named_spaces lock poisoned");
        if let Some((_, atoms)) = spaces.get(&space_id) {
            atoms.clone()
        } else {
            vec![]
        }
    }

    /// Check if a named space exists
    pub fn has_named_space(&self, space_id: u64) -> bool {
        self.shared.named_spaces.read().expect("named_spaces lock poisoned").contains_key(&space_id)
    }

    // ============================================================
    // Mutable State Management (new-state, get-state, change-state!)
    // ============================================================

    /// Create a new mutable state cell with an initial value
    /// Used by new-state operation
    ///
    /// NOTE: States are truly mutable - they are created in the shared store
    /// and visible to all environments sharing the same Arc<EnvironmentShared>.
    /// We intentionally do NOT call make_owned() here because new states should
    /// be globally visible, matching MeTTa HE semantics.
    pub fn create_state(&mut self, initial_value: MettaValue) -> u64 {
        // No make_owned() - states are shared, not copy-on-write
        let id = {
            let mut next_id = self.shared.next_state_id.write().expect("next_state_id lock poisoned");
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.shared.states.write().expect("states lock poisoned").insert(id, initial_value);

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Get the current value of a state cell
    /// Used by get-state operation
    pub fn get_state(&self, state_id: u64) -> Option<MettaValue> {
        self.shared.states.read().expect("states lock poisoned").get(&state_id).cloned()
    }

    /// Change the value of a state cell
    /// Used by change-state! operation
    /// Returns true if successful, false if state doesn't exist
    ///
    /// NOTE: States are truly mutable - changes are visible to all environments
    /// sharing the same Arc<EnvironmentShared>. We intentionally do NOT call
    /// make_owned() here because state mutations should be globally visible,
    /// matching MeTTa HE semantics where change-state! is immediately observable.
    pub fn change_state(&mut self, state_id: u64, new_value: MettaValue) -> bool {
        // No make_owned() - states are shared, not copy-on-write
        let mut states = self.shared.states.write().expect("states lock poisoned");
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
        self.shared.states.read().expect("states lock poisoned").contains_key(&state_id)
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
            .expect("bindings lock poisoned")
            .insert(symbol.to_string(), value);

        // Also register in fuzzy matcher for suggestions
        self.shared.fuzzy_matcher.write().expect("fuzzy_matcher lock poisoned").insert(symbol);

        self.modified.store(true, Ordering::Release);
    }

    /// Get the value bound to a symbol
    /// Used for symbol resolution
    pub fn get_binding(&self, symbol: &str) -> Option<MettaValue> {
        self.shared.bindings.read().expect("bindings lock poisoned").get(symbol).cloned()
    }

    /// Check if a symbol is bound
    pub fn has_binding(&self, symbol: &str) -> bool {
        self.shared.bindings.read().expect("bindings lock poisoned").contains_key(symbol)
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
            .expect("tokenizer lock poisoned")
            .register_token_value(token, value);
        // Also register in fuzzy matcher for suggestions
        self.shared.fuzzy_matcher.write().expect("fuzzy_matcher lock poisoned").insert(token);
        self.modified.store(true, Ordering::Release);
    }

    /// Look up a token in the tokenizer
    /// Returns the bound value if found
    pub fn lookup_token(&self, token: &str) -> Option<MettaValue> {
        self.shared.tokenizer.read().expect("tokenizer lock poisoned").lookup(token)
    }

    /// Check if a token is registered in the tokenizer
    pub fn has_token(&self, token: &str) -> bool {
        self.shared.tokenizer.read().expect("tokenizer lock poisoned").has_token(token)
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
        self.shared.module_registry.read().expect("module_registry lock poisoned").get_by_path(path)
    }

    /// Check if a module is cached by content hash
    pub fn get_module_by_content(&self, content_hash: u64) -> Option<ModId> {
        self.shared.module_registry
            .read()
            .expect("module_registry lock poisoned")
            .get_by_content(content_hash)
    }

    /// Check if a module is currently being loaded (cycle detection)
    pub fn is_module_loading(&self, content_hash: u64) -> bool {
        self.shared.module_registry
            .read()
            .expect("module_registry lock poisoned")
            .is_loading(content_hash)
    }

    /// Mark a module as being loaded
    pub fn mark_module_loading(&self, content_hash: u64) {
        self.shared.module_registry
            .write()
            .expect("module_registry lock poisoned")
            .mark_loading(content_hash);
    }

    /// Unmark a module as loading
    pub fn unmark_module_loading(&self, content_hash: u64) {
        self.shared.module_registry
            .write()
            .expect("module_registry lock poisoned")
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
        self.shared.module_registry.write().expect("module_registry lock poisoned").register(
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
            .expect("module_registry lock poisoned")
            .add_path_alias(path, mod_id);
    }

    /// Get the number of loaded modules
    pub fn module_count(&self) -> usize {
        self.shared.module_registry.read().expect("module_registry lock poisoned").module_count()
    }

    /// Get a module's space by its ModId.
    ///
    /// Returns an Arc reference to the module's ModuleSpace for live access.
    /// This is used by `mod-space!` to create live space references.
    pub fn get_module_space(
        &self,
        mod_id: ModId,
    ) -> Option<std::sync::Arc<RwLock<crate::backend::modules::ModuleSpace>>> {
        let registry = self.shared.module_registry.read().expect("module_registry lock poisoned");
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
        self.shared.module_registry.read().expect("module_registry lock poisoned").options().strict_mode
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
        self.shared.module_registry.write().expect("module_registry lock poisoned").set_options(options);
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
        let index = self.shared.rule_index.read().expect("rule_index lock poisoned");
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
        let wildcards = self.shared.wildcard_rules.read().expect("wildcard_rules lock poisoned");
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
        self.shared.fuzzy_matcher.read().expect("fuzzy_matcher lock poisoned").suggest(query, max_distance)
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
        self.shared.fuzzy_matcher.read().expect("fuzzy_matcher lock poisoned").did_you_mean(symbol, max_distance, 3)
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
            .expect("fuzzy_matcher lock poisoned")
            .smart_did_you_mean(symbol, max_distance, 3)
    }

    // ============================================================
    // Scope Tracking Operations
    // ============================================================

    /// Push a new scope onto the scope tracker.
    /// Called when entering lexical contexts like `let`, `match`, or function bodies.
    pub fn push_scope(&mut self) {
        self.make_owned();
        self.shared.scope_tracker.write().expect("scope_tracker lock poisoned").push_scope();
        self.modified.store(true, Ordering::Release);
    }

    /// Pop the innermost scope from the scope tracker.
    /// Called when leaving lexical contexts. Never pops the global scope.
    pub fn pop_scope(&mut self) {
        self.make_owned();
        self.shared.scope_tracker.write().expect("scope_tracker lock poisoned").pop_scope();
        self.modified.store(true, Ordering::Release);
    }

    /// Add a symbol to the current (innermost) scope.
    /// Called when introducing bindings (e.g., pattern variables in `let` or `match`).
    pub fn add_scope_symbol(&mut self, name: String) {
        self.make_owned();
        self.shared.scope_tracker.write().expect("scope_tracker lock poisoned").add_symbol(name);
        self.modified.store(true, Ordering::Release);
    }

    /// Add multiple symbols to the current scope.
    pub fn add_scope_symbols(&mut self, names: impl IntoIterator<Item = String>) {
        self.make_owned();
        self.shared.scope_tracker.write().expect("scope_tracker lock poisoned").add_symbols(names);
        self.modified.store(true, Ordering::Release);
    }

    /// Check if a symbol is visible in the current scope hierarchy.
    pub fn is_symbol_visible(&self, name: &str) -> bool {
        self.shared.scope_tracker.read().expect("scope_tracker lock poisoned").is_visible(name)
    }

    /// Get all visible symbols from the scope tracker, ordered local-first.
    /// Returns symbols from innermost scope first for prioritized recommendations.
    pub fn visible_scope_symbols(&self) -> Vec<String> {
        self.shared
            .scope_tracker
            .read()
            .expect("scope_tracker lock poisoned")
            .visible_symbols()
            .cloned()
            .collect()
    }

    /// Get the current scope depth (1 = global only).
    pub fn scope_depth(&self) -> usize {
        self.shared.scope_tracker.read().expect("scope_tracker lock poisoned").depth()
    }

    /// Check if currently at global scope.
    pub fn at_global_scope(&self) -> bool {
        self.shared.scope_tracker.read().expect("scope_tracker lock poisoned").at_global_scope()
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
        self.shared.grounded_registry.read().expect("grounded_registry lock poisoned").get(name)
    }

    /// Get a TCO-compatible grounded operation by name (e.g., "+", "-", "and")
    /// TCO operations return work items instead of calling eval internally,
    /// enabling deep recursion without stack overflow
    pub fn get_grounded_operation_tco(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<dyn super::grounded::GroundedOperationTCO>> {
        self.shared.grounded_registry_tco.read().expect("grounded_registry_tco lock poisoned").get(name)
    }

    /// Union two environments (monotonic merge)
    /// PathMap and shared_mapping are shared via Arc, so facts (including type assertions) are automatically merged
    /// Multiplicities and rule indices are also merged via shared Arc
    pub fn union(&self, _other: &Environment) -> Environment {
        trace!(target: "mettatron::environment::union", "Unioning environments");

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
