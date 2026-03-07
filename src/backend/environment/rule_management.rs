//! Rule management operations for Environment.
//!
//! Provides methods for adding, indexing, and querying rules.
//! Rules are stored as (= lhs rhs) in MORK PathMap as De Bruijn-encoded MORK bytes.
//!
//! # Multiplicity Tracking
//!
//! Rules can be defined multiple times, and we track multiplicities efficiently
//! using PathMap<Multiplicity> — each MORK byte key maps to its multiplicity count.
//!
//! # Rule Discovery
//!
//! Rules are discovered via a two-level index:
//!
//! 1. **Bloom filter** — O(1) rejection for non-matching head/arity combinations
//! 2. **RuleIndex** — HashMap-backed `(head, arity) → Vec<RuleEntry>` for O(1) candidate lookup
//! 3. **MORK `extract_data()`** — O(n) byte-level structural pattern matching per candidate
//! 4. **MettaValue binding application** — `apply_bindings_generic()` on cached RHS template
//!
//! Rules are stored in PathMap with De Bruijn encoding (via `with_mork_query_bytes`).
//! The RuleIndex caches De Bruijn bytes and metadata at insertion time for zero-deserialization
//! matching. Only the final matched result is deserialized to MettaValue.

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;

use crate::backend::hash_utils::{FxBuildHasher, PtrBuildHasher};

/// Global epoch counter for rule/type mutations.
///
/// Incremented on `add_rule()`, `add_type_generic()`, and `remove_type_generic()`.
/// Used by `ExprCompilationState` to cache `TypeSignatureRegistry` across JIT
/// entries — the registry is rebuilt only when the epoch changes.
pub static RULE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Increment the global rule/type epoch counter.
///
/// Must be called after any mutation that could change type signatures:
/// adding/removing rules, adding/removing type declarations.
#[inline]
pub fn increment_rule_epoch() {
    RULE_EPOCH.fetch_add(1, Ordering::Release);
}

use mork::space::Space;
use mork_expr::{maybe_byte_item, Expr, ExprZipper, Tag};
// Disabled: PathMap no longer directly constructed in this module — add_rules_bulk now
// delegates to add_rule() for consistent De Bruijn encoding.
// use pathmap::PathMap;
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use smallvec::SmallVec;
use tracing::trace;

thread_local! {
    /// Reusable buffer for MORK-serialized expressions in `match_rules_native`.
    /// Grows as needed but is never freed — amortized zero allocation after warmup.
    static MATCH_EXPR_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(256));

    /// Reusable ExprZipper for `extract_data()` calls in `match_rules_native`.
    /// The `trace` Vec capacity grows monotonically — `reset()` uses `set_len(0)` to
    /// clear without deallocating, so after warmup all match attempts are zero-alloc.
    /// Pre-allocated with capacity 16 for typical PLN expression depths (6-8).
    static MATCH_ZIPPER: RefCell<ExprZipper> = RefCell::new({
        let trace = Vec::with_capacity(16);
        // ExprZipper::new() would push a Breadcrumb based on the root's first byte,
        // but with a null root, we skip that and just set up the capacity.
        // The actual root and trace initialization happen via reset() before each use.
        ExprZipper { root: Expr { ptr: std::ptr::null_mut() }, loc: 0, trace }
    });

    /// Thread-local MORK serialization cache for `match_rules_native`.
    ///
    /// Content-hash keyed: `u64` (xxh3 content hash of MettaValue). This survives GC
    /// safepoints (no pointer ABA issue) and shares entries across structurally-identical
    /// expressions at different slab addresses. Value includes the arity for cheap
    /// collision validation.
    ///
    /// 8192 entries × ~64 bytes avg = ~512 KB per thread. LRU eviction bounds memory.
    /// Increased from 2048 to improve hit rate for PLN's working set (>2048 distinct exprs).
    static MORK_BYTES_CACHE: RefCell<LruCache<u64, (Vec<u8>, usize), FxBuildHasher>> =
        RefCell::new(LruCache::with_hasher(NonZeroUsize::new(8192).expect("non-zero"), FxBuildHasher));
}

/// Clear the thread-local MORK serialization cache.
///
/// With content-hash keying, this is only needed when the SharedMapping changes
/// (different symbol table = different interned IDs for same bytes). GC safepoints
/// no longer require clearing since keys are content hashes, not pointers.
pub fn clear_mork_bytes_cache() {
    MORK_BYTES_CACHE.with(|c| c.borrow_mut().clear());
}

use super::generic::GenericEnvironment;
use super::mork_encoding::{mork_bytes_to_generic_value, mork_expr_byte_len};
// Disabled: mork_expr_to_generic_value no longer used directly — deserialization happens via
// mork_bytes_to_generic_value for individual binding bytes.
// use super::mork_encoding::mork_expr_to_generic_value;
use super::multiplicity::{
    decrement_multiplicity, get_multiplicity, increment_multiplicity,
    Multiplicity,
};
use super::{MettaEnvironment, MettaValue};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueInner, MettaValueTrait};
use crate::backend::mork_convert::{with_mork_bytes, with_mork_query_bytes};

/// Extract (lhs, rhs) from a deserialized rule value `(= lhs rhs)`.
///
/// Returns `Some((lhs, rhs))` if the value is an s-expression with 3 elements
/// where the first element is the atom `"="`.
pub(crate) fn extract_rule_parts<V: MettaValueTrait + Clone>(value: &V) -> Option<(V, V)> {
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

// ============================================================================
// RuleIndex — In-memory index for O(1) rule lookup + byte-level matching
// ============================================================================

/// Result of a native rule match via `match_rules_native()`.
///
/// Contains the instantiated RHS (bindings applied), the original RHS template
/// (for bytecode compilation caching), and named bindings (for bytecode VM stack frames).
#[derive(Debug, Clone)]
pub struct RuleMatchResult<V: MettaValueTrait + Clone> {
    /// RHS with bindings applied (for trampoline evaluation)
    pub instantiated_rhs: V,
    /// Original RHS template with original variable names (for bytecode compilation caching)
    pub rhs_template: V,
    /// Named bindings ($x -> value, for bytecode VM stack frames)
    pub bindings: GenericBindings<V>,
    /// How many times this rule was defined (multiplicity)
    pub multiplicity: u64,
    /// Phase 8.7: Cached return type of the RHS (from RuleEntry).
    /// Used for branch pruning when `expected_type` is set.
    pub rhs_type: Option<V>,
}

/// A single rule entry in the RuleIndex.
///
/// Caches both the original MettaValues (for display, debugging, bytecode VM) and the
/// De Bruijn-encoded bytes (for MORK `extract_data()` byte-level matching).
#[derive(Debug, Clone)]
pub(crate) struct RuleEntry<V: MettaValueTrait + Clone> {
    // --- Cached MettaValues (original variable names) ---
    /// LHS pattern with original variable names (for display, debugging)
    pub lhs: V,
    /// RHS template with original variable names (for bytecode compilation/caching)
    pub rhs: V,

    // --- De Bruijn bytes (for extract_data matching) ---
    /// LHS with NewVar/VarRef De Bruijn encoding (extracted from PathMap key).
    /// Empty for wide rules (arity ≥ 64) — use `lhs_wide_debruijn` instead.
    pub lhs_debruijn: Vec<u8>,

    /// LHS with Wide MORK De Bruijn encoding (tag-byte + LEB128, no arity limit).
    /// Empty for narrow rules (arity < 64) — use `lhs_debruijn` instead.
    /// Populated when MORK encoding fails due to arity ≥ 64.
    pub lhs_wide_debruijn: Vec<u8>,

    // --- Metadata ---
    /// De Bruijn index → original variable name (e.g., "$x", "$y")
    /// Only contains variables from LHS (used for building named bindings)
    pub var_names: Vec<String>,
    /// Indices of `_` wildcards (skip these in named bindings)
    pub wildcard_indices: SmallVec<[u8; 4]>,
    // NOTE: The old `specificity` field (count of NewVar tags) was removed.
    // MeTTa HE has NO specificity filter — all matching rules fire nondeterministically.
    // The old filter dropped structurally-more-specific rules when a variable-only rule
    // happened to have fewer NewVar tags (e.g. `(f ($c $tv) $y)` with 3 vars beat
    // `(f ((Implication $A $B) $TV) $Y)` with 4 vars despite the latter being more specific).
    /// How many times this rule was added (synced with PathMap multiplicity)
    pub multiplicity: u64,
    /// Cached return type of the RHS, computed once at insertion time.
    /// Used by Phase 8 optimizations for rule pre-filtering by expected type.
    /// `None` if RHS type couldn't be inferred (e.g., variable RHS, untyped operators).
    pub rhs_type: Option<V>,
    /// Cached result of `rhs.contains_variables()`, computed once at insertion time.
    /// When `false`, `apply_bindings` can skip the RHS entirely (O(1) clone).
    pub rhs_has_variables: bool,
}

/// Lightweight in-memory index for O(1) rule lookup + MORK byte-level matching.
///
/// Populated at `add_rule()` time. Authoritative source for rule queries.
/// PathMap remains the storage-of-record (for `match_space`, serialization).
///
/// ## Indexing Strategy
///
/// Rules are indexed by `(head_symbol, arity)` for O(1) lookup. Rules with
/// non-S-expression LHS (e.g., `(= $x $x)`) are stored in a separate `wildcard`
/// vec and included in all query results since they can match any expression.
///
/// ## Duplicate Detection
///
/// When `add_rule()` is called with the same `(lhs, rhs)` (by `PartialEq`),
/// the existing entry's multiplicity is incremented rather than creating a duplicate.
#[derive(Debug, Clone)]
pub(crate) struct RuleIndex<V: MettaValueTrait + Clone> {
    /// Rules indexed by (head_symbol, arity) for O(1) lookup.
    by_head_arity: HashMap<(String, usize), Vec<RuleEntry<V>>>,

    /// Rules with non-S-expression LHS (atoms, variables like `$x`).
    /// Always included in query results since they can match any expression.
    wildcard: Vec<RuleEntry<V>>,
}

impl<V: MettaValueTrait + Clone> RuleIndex<V> {
    /// Create a new empty RuleIndex.
    pub fn new() -> Self {
        RuleIndex {
            by_head_arity: HashMap::new(),
            wildcard: Vec::new(),
        }
    }

    /// Insert a rule entry, or increment multiplicity if a duplicate exists.
    ///
    /// Duplicate detection uses `PartialEq` on `(lhs, rhs)` MettaValues.
    /// If `head` is `Some`, indexes by `(head, arity)`. Otherwise adds to wildcard list.
    pub fn add_rule(
        &mut self,
        head: Option<&str>,
        arity: usize,
        entry: RuleEntry<V>,
    ) {
        let entries = match head {
            Some(h) => self.by_head_arity
                .entry((h.to_string(), arity))
                .or_insert_with(Vec::new),
            None => &mut self.wildcard,
        };

        // Check for duplicate (same LHS + RHS by structural equality)
        for existing in entries.iter_mut() {
            if existing.lhs == entry.lhs && existing.rhs == entry.rhs {
                existing.multiplicity += 1;
                return;
            }
        }

        entries.push(entry);
    }

    /// Remove a rule by decrementing multiplicity. Returns true if the entry was removed entirely.
    pub fn remove_rule(&mut self, lhs: &V, rhs: &V) -> bool {
        // Search in all buckets
        for entries in self.by_head_arity.values_mut() {
            if let Some(pos) = entries.iter().position(|e| &e.lhs == lhs && &e.rhs == rhs) {
                if entries[pos].multiplicity > 1 {
                    entries[pos].multiplicity -= 1;
                    return false;
                } else {
                    entries.remove(pos);
                    return true;
                }
            }
        }
        // Check wildcard
        if let Some(pos) = self.wildcard.iter().position(|e| &e.lhs == lhs && &e.rhs == rhs) {
            if self.wildcard[pos].multiplicity > 1 {
                self.wildcard[pos].multiplicity -= 1;
                return false;
            } else {
                self.wildcard.remove(pos);
                return true;
            }
        }
        false
    }

    /// Get candidate rules for the given (head, arity) pair.
    ///
    /// Returns an iterator over head-specific rules chained with wildcard rules.
    /// Callers should use `extract_data` on each candidate's `lhs_debruijn` bytes.
    pub fn get_candidates(&self, head: &str, arity: usize) -> impl Iterator<Item = &RuleEntry<V>> {
        let head_specific = self.by_head_arity
            .get(&(head.to_string(), arity))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        head_specific.iter().chain(self.wildcard.iter())
    }

    /// Get all rules (for no-head queries).
    pub fn get_all_rules(&self) -> impl Iterator<Item = &RuleEntry<V>> {
        self.by_head_arity.values()
            .flat_map(|v| v.iter())
            .chain(self.wildcard.iter())
    }

    /// Check if there are any wildcard rules (rules with variable heads).
    #[inline]
    pub fn has_wildcard_rules(&self) -> bool {
        !self.wildcard.is_empty()
    }

    /// Get the number of rules in the index.
    pub fn len(&self) -> usize {
        self.by_head_arity
            .values()
            .map(|v| v.len())
            .sum::<usize>()
            + self.wildcard.len()
    }

    /// Clear the index.
    pub fn clear(&mut self) {
        self.by_head_arity.clear();
        self.wildcard.clear();
    }
}

/// Validate that ALL tag bytes in a MORK expression are valid (no reserved bytes 0x40-0x7F).
///
/// Uses the same traversal logic as `mork_expr_byte_len()`. Returns `Ok(len)` if all
/// bytes are valid MORK tags, or `Err((offset, byte))` for the first reserved byte found.
///
/// This is used as a diagnostic tool to catch byte misalignment issues before they
/// cause panics in `ExprZipper::new()` or `ExprZipper::tag()` (which call `byte_item()`).
#[cfg(any(debug_assertions, test))]
fn validate_mork_bytes(bytes: &[u8]) -> Result<usize, (usize, u8)> {
    let mut offset = 0usize;
    let mut depth = 1u32;

    while depth > 0 && offset < bytes.len() {
        let byte = bytes[offset];
        let tag = match maybe_byte_item(byte) {
            Ok(t) => t,
            Err(reserved) => return Err((offset, reserved)),
        };
        offset += 1;
        depth -= 1;

        match tag {
            Tag::NewVar | Tag::VarRef(_) => {}
            Tag::SymbolSize(size) => {
                let end = offset + size as usize;
                if end > bytes.len() {
                    // Symbol data extends past the buffer — truncated expression
                    return Err((offset - 1, byte));
                }
                offset = end;
            }
            Tag::Arity(arity) => {
                depth += arity as u32;
            }
        }
    }

    if depth > 0 {
        // Expression is incomplete — ran out of bytes before all children were consumed
        return Err((offset, 0xFF));
    }

    Ok(offset)
}

/// Count the number of NewVar tags in MORK bytes (used for specificity computation).
///
/// Each NewVar tag (0xC0) introduces a new variable binding position.
/// Fewer NewVar tags = more specific pattern (more concrete structure).
///
/// Uses `maybe_byte_item()` to validate the first byte before creating an `ExprZipper`.
/// Returns 0 if the bytes are empty or start with a reserved byte.
fn count_newvar_tags(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    // Validate first byte is a valid MORK tag before calling ExprZipper::new()
    // (which uses byte_item() and panics on reserved bytes 0x40-0x7F)
    if let Err(reserved) = maybe_byte_item(bytes[0]) {
        tracing::warn!(
            target: "mettatron::count_newvar_tags",
            "LHS De Bruijn bytes start with reserved byte 0x{:02x}, skipping",
            reserved
        );
        return 0;
    }
    let mut count = 0;
    let expr = Expr { ptr: bytes.as_ptr().cast_mut() };
    let mut ez = ExprZipper::new(expr);
    loop {
        if ez.tag() == Tag::NewVar {
            count += 1;
        }
        if !ez.next() {
            break;
        }
    }
    count
}

/// Build a list of original variable names from the ConversionContext,
/// identifying wildcard indices (anonymous variables from `_`).
///
/// Returns `(var_names, wildcard_indices)` where:
/// - `var_names[i]` is the full variable name including `$` prefix for De Bruijn index `i`
/// - `wildcard_indices` contains indices of anonymous wildcard variables
fn build_var_names_and_wildcards(
    ctx_var_names: &[String],
    lhs_var_count: usize,
) -> (Vec<String>, SmallVec<[u8; 4]>) {
    let mut var_names = Vec::with_capacity(lhs_var_count);
    let mut wildcard_indices = SmallVec::new();

    for (i, name) in ctx_var_names.iter().enumerate() {
        if i >= lhs_var_count {
            break;
        }
        if name.starts_with("__anon") {
            // Wildcard _ was encoded as __anonN
            var_names.push("_".to_string());
            wildcard_indices.push(i as u8);
        } else {
            // Regular variable — restore the $ prefix
            var_names.push(format!("${}", name));
        }
    }

    (var_names, wildcard_indices)
}

/// Extract bindings by walking De Bruijn bytes and the original expression in parallel.
///
/// Since `extract_data` already confirmed the structural match, we can skip all
/// matching logic and just navigate to NewVar positions to capture sub-expressions
/// from the original value. This preserves runtime types (SpaceHandle, State, etc.)
/// that can't survive a MORK serialize→deserialize round trip.
///
/// O(pattern_size) time — same as extract_data, but operates on MettaValues.
fn extract_bindings_from_expr<V>(
    lhs_debruijn: &[u8],
    expr: &V,
    var_names: &[String],
    wildcard_indices: &SmallVec<[u8; 4]>,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
{
    let mut bindings = GenericBindings::new();
    let mut offset = 0usize;
    let mut newvar_idx = 0u8;
    let mut expr_stack: Vec<&V> = vec![expr];
    // Exclude padding byte (0x00) appended for ExprZipper read-past-end safety
    let end = lhs_debruijn.len().saturating_sub(1);

    while offset < end && !expr_stack.is_empty() {
        let tag = match maybe_byte_item(lhs_debruijn[offset]) {
            Ok(t) => t,
            Err(_) => break,
        };
        offset += 1;

        match tag {
            Tag::NewVar => {
                let value = match expr_stack.pop() {
                    Some(v) => v,
                    None => break,
                };
                if !wildcard_indices.contains(&newvar_idx)
                    && (newvar_idx as usize) < var_names.len()
                {
                    bindings.insert(var_names[newvar_idx as usize].clone(), value.clone());
                }
                newvar_idx += 1;
            }
            Tag::VarRef(_) => {
                expr_stack.pop(); // Consume without binding
            }
            Tag::SymbolSize(size) => {
                offset += size as usize; // Skip symbol bytes
                expr_stack.pop(); // Consume the corresponding atom/leaf
            }
            Tag::Arity(n) => {
                if n == 0 {
                    expr_stack.pop(); // Unit / empty S-expression
                } else if let Some(parent) = expr_stack.pop() {
                    // Push children in reverse order so first child is on top
                    if let Some(items) = parent.as_sexpr() {
                        for child in items.iter().rev() {
                            expr_stack.push(child);
                        }
                    } else if let Some(goals) = parent.as_conjunction() {
                        // Conjunction: MORK writes Arity(goals+1) with comma as first child.
                        // Push goals in reverse, then a placeholder for the comma
                        // (the comma's SymbolSize tag will pop and discard it).
                        for goal in goals.iter().rev() {
                            expr_stack.push(goal);
                        }
                        expr_stack.push(parent); // Placeholder consumed by SymbolSize(",")
                    } else if let Some((_msg, details)) = parent.as_error() {
                        // Error: MORK writes Arity(3) with "error", "msg", details.
                        // Push details, then placeholders for msg and "error".
                        expr_stack.push(details);
                        expr_stack.push(parent); // Placeholder consumed by SymbolSize("\"msg\"")
                        expr_stack.push(parent); // Placeholder consumed by SymbolSize("error")
                    } else {
                        // Other compound types (Space, State serialized as S-expr in MORK).
                        // These shouldn't appear as Arity match targets in LHS patterns
                        // (they're opaque types matched by NewVar), but if they do,
                        // we can't drill into them — stop extraction.
                        break;
                    }
                }
            }
        }
    }

    bindings
}

/// Extract bindings by walking Wide MORK De Bruijn bytes and the original expression in parallel.
///
/// Same algorithm as `extract_bindings_from_expr` but for Wide MORK tag format
/// (tag-byte + LEB128 instead of MORK's 2-bit tag + 6-bit payload).
///
/// Since `wide_extract_data` already confirmed the structural match, we can skip all
/// matching logic and just navigate to NewVar positions to capture sub-expressions.
fn extract_bindings_from_wide_expr<V>(
    lhs_wide_debruijn: &[u8],
    expr: &V,
    var_names: &[String],
    wildcard_indices: &SmallVec<[u8; 4]>,
) -> GenericBindings<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
{
    use crate::backend::wide_mork::encoding::{
        WideTag, decode_leb128,
    };

    let mut bindings = GenericBindings::new();
    let mut offset = 0usize;
    let mut newvar_idx = 0u8;
    let mut expr_stack: Vec<&V> = vec![expr];
    let end = lhs_wide_debruijn.len();

    while offset < end && !expr_stack.is_empty() {
        let tag = match WideTag::from_byte(lhs_wide_debruijn[offset]) {
            Ok(t) => t,
            Err(_) => break,
        };
        offset += 1;

        match tag {
            WideTag::NewVar => {
                let value = match expr_stack.pop() {
                    Some(v) => v,
                    None => break,
                };
                if !wildcard_indices.contains(&newvar_idx)
                    && (newvar_idx as usize) < var_names.len()
                {
                    bindings.insert(var_names[newvar_idx as usize].clone(), value.clone());
                }
                newvar_idx += 1;
            }
            WideTag::VarRef => {
                // Consume the LEB128 index and the corresponding expression
                if let Some((_idx, consumed)) = decode_leb128(&lhs_wide_debruijn[offset..]) {
                    offset += consumed;
                } else {
                    break;
                }
                expr_stack.pop(); // Consume without binding
            }
            WideTag::SymbolSize => {
                // Skip the LEB128 size and the symbol bytes
                if let Some((size, consumed)) = decode_leb128(&lhs_wide_debruijn[offset..]) {
                    offset += consumed + size as usize;
                } else {
                    break;
                }
                expr_stack.pop(); // Consume the corresponding atom/leaf
            }
            WideTag::Arity => {
                if let Some((n, consumed)) = decode_leb128(&lhs_wide_debruijn[offset..]) {
                    offset += consumed;
                    if n == 0 {
                        expr_stack.pop(); // Unit / empty S-expression
                    } else if let Some(parent) = expr_stack.pop() {
                        // Push children in reverse order so first child is on top
                        if let Some(items) = parent.as_sexpr() {
                            for child in items.iter().rev() {
                                expr_stack.push(child);
                            }
                        } else if let Some(goals) = parent.as_conjunction() {
                            for goal in goals.iter().rev() {
                                expr_stack.push(goal);
                            }
                            expr_stack.push(parent); // Placeholder for comma
                        } else if let Some((_msg, details)) = parent.as_error() {
                            expr_stack.push(details);
                            expr_stack.push(parent); // Placeholder for msg
                            expr_stack.push(parent); // Placeholder for "error"
                        } else {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }
    }

    bindings
}

/// Build the head+arity MORK byte prefix for targeted rule lookup.
///
/// **NOTE**: Superseded by `RuleIndex` for the primary hot path. Retained for
/// `get_matching_rules_for_expr()` fallback used in tests and `match_space` compatibility.
///
/// Extends the cached rule prefix with `[Arity(arity+1)] + [head symbol MORK bytes]`.
/// The MORK arity includes the head element, so MeTTa arity (excludes head) needs `+1`.
///
/// Returns `None` if the head is empty, arity exceeds the MORK 6-bit limit (63),
/// or MORK serialization fails.
fn build_head_arity_prefix<V, F>(
    rule_prefix: &[u8],
    head: &str,
    arity: usize,
    factory: &F,
    sm: &mork_interning::SharedMappingHandle,
    cache_epoch: u64,
) -> Option<Vec<u8>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if head.is_empty() {
        return None;
    }
    let mork_arity = (arity + 1) as u8; // MORK arity includes head
    if mork_arity >= 64 {
        return None; // MORK arity limit (6 bits)
    }

    // Serialize head symbol to get its MORK bytes (SymbolSize tag + interned key)
    let head_atom = factory.atom(head);
    with_mork_bytes(&head_atom, sm, cache_epoch, |head_bytes| {
        let mut prefix = Vec::with_capacity(rule_prefix.len() + 1 + head_bytes.len());
        prefix.extend_from_slice(rule_prefix);
        // Arity tag: upper 2 bits = 00, lower 6 bits = arity value
        prefix.push(mork_arity);
        prefix.extend_from_slice(head_bytes);
        prefix
    })
    .ok()
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
    /// The rule is stored as `(= lhs rhs)` in the MORK PathMap using De Bruijn encoding
    /// (via `with_mork_query_bytes`). The RuleIndex is populated with cached MettaValues,
    /// De Bruijn bytes, and metadata for O(1) lookup + byte-level matching.
    ///
    /// Bloom filter is updated for O(1) rejection in match_space().
    ///
    /// # Arguments
    /// - `lhs`: The left-hand side pattern
    /// - `rhs`: The right-hand side template
    pub fn add_rule(&mut self, lhs: V, rhs: V) {
        trace!(target: "mettatron::environment::add_rule", "Adding rule");
        self.make_owned(); // CoW: ensure we own data before modifying

        // Phase 9.5: Invalidate normal-form memoization — new rules may make
        // previously normal-form expressions reducible.
        crate::backend::eval::trampoline::invalidate_normal_form_memo();

        // Clear eval memo and match result caches — new rules may change
        // evaluation and matching results for previously cached expressions.
        crate::backend::eval::trampoline::clear_eval_memo();
        crate::backend::eval::trampoline::clear_match_result_cache();

        // Increment rule/type epoch — invalidates cached TypeSignatureRegistry in JIT.
        increment_rule_epoch();

        // Get head symbol and arity for bloom filter (clone head string before moving lhs)
        let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
        let arity = lhs.get_arity();

        // Phase 8.1: Compute RHS type at insertion time for branch pruning (Phase 8.7).
        // Only stores non-trivial types — %Undefined% provides no pruning benefit.
        let rhs_type = {
            use crate::backend::eval::types_generic::infer_type_generic;
            let inferred = infer_type_generic(&rhs, &self.factory, self);
            if inferred.as_atom() == Some("%Undefined%") { None } else { Some(inferred) }
        };

        // Trace: RhsTypeComputed
        #[cfg(feature = "eval-trace")]
        {
            crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    crate::backend::trace::trace_value_generic(&lhs),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RhsTypeComputed {
                        head: head_owned.as_deref().unwrap_or("<none>").to_string(),
                        arity: arity as u32,
                        lhs: crate::backend::trace::trace_value_generic(&lhs),
                        rhs: crate::backend::trace::trace_value_generic(&rhs),
                        rhs_type: rhs_type.as_ref().map(crate::backend::trace::trace_value_generic),
                    },
                );
            });
        }

        // Phase 10.1: Register inferred return type in function return type index.
        // Makes rhs_type queryable by infer_types_generic for user-defined functions
        // without explicit (: f (-> ...)) type declarations.
        if let Some(ref rt) = rhs_type {
            if let Some(ref head) = head_owned {
                self.register_inferred_type(head, rt);

                // Trace: InferredTypeRegistered (Phase 10.1)
                #[cfg(feature = "eval-trace")]
                {
                    crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            crate::backend::trace::trace_value_generic(rt),
                            vec![],
                            None,
                            trace_format::TraceEventKind::InferredTypeRegistered {
                                function_name: head.clone(),
                                registered_type: crate::backend::trace::trace_value_generic(rt),
                                source: "phase-10.1-rhs".to_string(),
                            },
                        );
                    });
                }
            }
        }

        // Phase 10.4: Synthesize arrow type from rule LHS pattern + RHS body.
        // Analyzes parameter constraints from RHS usage to build (-> T1 T2 ... Tret).
        // Only for rules without explicit (: f (-> ...)) type declarations.
        if let Some(ref head) = head_owned {
            // Skip if the function already has a declared arrow type
            let has_declared_arrow = self.get_types_generic(head).iter().any(|t| {
                t.as_sexpr()
                    .and_then(|items| items.first().and_then(|v| v.as_atom()))
                    == Some("->")
            });
            if !has_declared_arrow {
                use crate::backend::eval::types_generic::infer_arrow_type_from_rule;
                if let Some(arrow) = infer_arrow_type_from_rule(
                    &lhs,
                    &rhs,
                    rhs_type.as_ref(),
                    &self.factory,
                    self,
                ) {
                    self.register_inferred_type(head, &arrow);

                    // Trace: InferredTypeRegistered (Phase 10.4)
                    #[cfg(feature = "eval-trace")]
                    {
                        crate::backend::trace::thread_local_sink::with_trace_collector_ref(|tc| {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                0,
                                crate::backend::trace::trace_value_generic(&arrow),
                                vec![],
                                None,
                                trace_format::TraceEventKind::InferredTypeRegistered {
                                    function_name: head.clone(),
                                    registered_type: crate::backend::trace::trace_value_generic(&arrow),
                                    source: "phase-10.4-arrow".to_string(),
                                },
                            );
                        });
                    }
                }
            }
        }

        // Track symbol name in fuzzy matcher for "Did you mean?" suggestions
        if let Some(ref head) = head_owned {
            self.shared.fuzzy_matcher.write().insert(head);
        }

        // Create rule s-expression: (= lhs rhs)
        let rule_sexpr = self.factory.sexpr(vec![
            self.factory.atom("="),
            lhs.clone(),
            rhs.clone(),
        ]);

        // Convert to De Bruijn bytes and insert into PathMap + RuleIndex
        let rule_prefix_len = self.rule_prefix.len();
        let result = with_mork_query_bytes(
            &rule_sexpr,
            &self.shared_mapping,
            self.mork_cache_epoch,
            |debruijn_bytes, ctx| {
                // 1. Insert De Bruijn bytes into PathMap (increment multiplicity)
                {
                    let mut btm = self.shared.atom_space.btm.write();
                    super::multiplicity::add_atom(&mut btm, debruijn_bytes);
                }
                self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

                // 2. Split De Bruijn bytes into LHS and RHS ranges
                // Layout: [Arity(3)] ["=" symbol bytes] [LHS bytes] [RHS bytes]
                //         |---------- rule_prefix_len --|
                if debruijn_bytes.len() <= rule_prefix_len {
                    return; // Shouldn't happen for valid rules
                }

                // Validate that rule_prefix matches the start of debruijn_bytes.
                // This catches interning inconsistencies between with_mork_bytes (used for
                // prefix computation) and with_mork_query_bytes (used for rule encoding).
                #[cfg(debug_assertions)]
                {
                    let prefix = &self.rule_prefix;
                    let actual_prefix = &debruijn_bytes[..prefix.len().min(debruijn_bytes.len())];
                    assert_eq!(
                        actual_prefix, &prefix[..],
                        "rule_prefix mismatch: debruijn_bytes prefix doesn't match pre-computed rule_prefix.\n\
                         Expected: {:02x?}\n\
                         Actual:   {:02x?}\n\
                         Full debruijn_bytes (first 32): {:02x?}",
                        prefix,
                        actual_prefix,
                        &debruijn_bytes[..debruijn_bytes.len().min(32)]
                    );
                }

                let lhs_start = rule_prefix_len;
                let lhs_byte_len = mork_expr_byte_len(&debruijn_bytes[lhs_start..]);

                // Validate LHS byte range, first byte, and ALL bytes
                #[cfg(debug_assertions)]
                {
                    if lhs_start + lhs_byte_len > debruijn_bytes.len() {
                        panic!(
                            "LHS byte range {}..{} exceeds debruijn_bytes len {}",
                            lhs_start, lhs_start + lhs_byte_len, debruijn_bytes.len()
                        );
                    }
                    let first_lhs_byte = debruijn_bytes[lhs_start];
                    if let Err(reserved) = maybe_byte_item(first_lhs_byte) {
                        panic!(
                            "LHS starts with reserved byte 0x{:02x} at offset {} in {:02x?}",
                            reserved, lhs_start, &debruijn_bytes[..debruijn_bytes.len().min(32)]
                        );
                    }
                }

                // Extract LHS De Bruijn bytes with one extra zero byte of padding.
                // See the comment on expr_bytes_owned in match_rules_native() for why
                // padding is needed (ExprZipper::gnext reads one byte past the end).
                let mut lhs_debruijn = Vec::with_capacity(lhs_byte_len + 1);
                lhs_debruijn.extend_from_slice(&debruijn_bytes[lhs_start..lhs_start + lhs_byte_len]);
                lhs_debruijn.push(0x00); // Padding byte for ExprZipper read-past-end safety

                // Validate ALL bytes in lhs_debruijn are valid MORK (no reserved 0x40-0x7F)
                #[cfg(debug_assertions)]
                {
                    if let Err((off, byte)) = validate_mork_bytes(&lhs_debruijn) {
                        panic!(
                            "lhs_debruijn has invalid byte 0x{:02x} at offset {} (len={}).\n\
                             lhs_debruijn: {:02x?}\n\
                             full debruijn_bytes (first 64): {:02x?}\n\
                             rule_prefix_len: {}, lhs_start: {}, lhs_byte_len: {}",
                            byte, off, lhs_debruijn.len(),
                            &lhs_debruijn[..lhs_debruijn.len().min(32)],
                            &debruijn_bytes[..debruijn_bytes.len().min(64)],
                            rule_prefix_len, lhs_start, lhs_byte_len
                        );
                    }
                }

                // 3. Compute metadata from De Bruijn encoding
                let lhs_var_count = count_newvar_tags(&lhs_debruijn);
                let (var_names, wildcard_indices) =
                    build_var_names_and_wildcards(&ctx.var_names, lhs_var_count);

                // 4. Populate RuleIndex
                let entry = RuleEntry {
                    lhs: lhs.clone(),
                    rhs_has_variables: rhs.contains_variables(),
                    rhs: rhs.clone(),
                    lhs_debruijn,
                    lhs_wide_debruijn: Vec::new(), // Narrow path — MORK encoding succeeded
                    var_names,
                    wildcard_indices,
                    multiplicity: 1,
                    rhs_type: rhs_type.clone(),
                };
                self.shared.rule_index.write().add_rule(
                    head_owned.as_deref(),
                    arity,
                    entry,
                );
            },
        );

        // Fallback for expressions that can't be MORK-encoded (arity >= 64).
        // Use Wide MORK encoding for proper byte-level pattern matching.
        // Store in wide_btm (PathMap<Multiplicity>) — same type as btm.
        if result.is_err() {
            let mut wide_key = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_storage(&rule_sexpr, &mut wide_key);

            {
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                super::multiplicity::add_atom(&mut wbtm, &wide_key);
            }

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);

            // Encode the LHS with Wide MORK De Bruijn encoding for byte-level matching.
            // This replaces the old structural fallback with O(n) byte-level matching.
            let mut wide_ctx = crate::backend::wide_mork::encoding::WideConversionContext::new();
            let mut lhs_wide_debruijn = Vec::new();
            crate::backend::wide_mork::encoding::encode_wide_debruijn(
                &lhs, &mut wide_ctx, &mut lhs_wide_debruijn,
            );

            let lhs_var_count = crate::backend::wide_mork::encoding::count_wide_newvar_tags(&lhs_wide_debruijn);
            let (var_names, wildcard_indices) =
                build_var_names_and_wildcards(&wide_ctx.var_names, lhs_var_count);

            let entry = RuleEntry {
                lhs: lhs.clone(),
                rhs_has_variables: rhs.contains_variables(),
                rhs: rhs.clone(),
                lhs_debruijn: Vec::new(), // Empty — this is a wide rule
                lhs_wide_debruijn,
                var_names,
                wildcard_indices,
                multiplicity: 1,
                rhs_type, // Phase 8.1: computed before closure, last use — no clone needed
            };
            self.shared.rule_index.write().add_rule(
                head_owned.as_deref(),
                arity,
                entry,
            );
        }

        // Update bloom filter with (head, arity) for O(1) match_space() rejection
        if let Some(ref head) = head_owned {
            let arity_u8 = arity as u8;
            self.shared
                .atom_space.head_arity_bloom
                .write()
                .insert(head.as_bytes(), arity_u8);
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Match rules natively using MORK byte-level `extract_data()` for pattern matching.
    ///
    /// This replaces the old pipeline of:
    /// 1. `get_matching_rules_for_expr()` → trie traversal + LHS/RHS deserialization
    /// 2. `pattern_match_generic()` → MettaValue-level structural comparison
    /// 3. `apply_bindings_generic()` → MettaValue-level binding substitution
    ///
    /// New pipeline:
    /// 1. **Bloom filter** — O(1) rejection by (head, arity)
    /// 2. **RuleIndex lookup** — O(1) HashMap lookup for `(head, arity) → Vec<RuleEntry>`
    /// 3. **Serialize expr ONCE** — `with_mork_bytes(expr)` → expr_bytes
    /// 4. **`extract_data()`** — O(n) byte-level pattern matching per candidate (no deserialization)
    /// 5. **Extract bindings** — Only for successful matches (all matching rules fire nondeterministically)
    /// 6. **`apply_bindings_generic()`** — Apply bindings to cached RHS MettaValue
    ///
    /// ## Performance
    ///
    /// Eliminates LHS deserialization per candidate (~27% of old wall time),
    /// trie traversal page faults (~23%), and MettaValue-level pattern matching (~15%).
    pub fn match_rules_native(
        &self,
        expr: &V,
        apply_bindings: impl Fn(&V, &GenericBindings<V>, &F) -> V,
    ) -> Vec<RuleMatchResult<V>> {
        let head = expr.get_head_symbol().unwrap_or("");
        let arity = expr.get_arity();

        // Bloom filter O(1) rejection: skip MORK serialization entirely when
        // the bloom filter says no head-specific rules exist for this head+arity
        // AND there are no wildcard rules (which match any head).
        if !head.is_empty() {
            let bloom_says_no = !self
                .shared
                .atom_space.head_arity_bloom
                .read()
                .may_contain(head.as_bytes(), arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        // Serialize the expression to MORK bytes ONCE using a thread-local buffer.
        // The buffer grows as needed but is never freed — amortized zero allocation.
        //
        // IMPORTANT: One extra zero byte is appended as padding. MORK's ExprZipper::gnext()
        // reads one byte past the last element of any S-expression to check if the next
        // sibling is an Arity tag. When expressions are embedded in PathMap memory, this
        // read is harmless (it reads a byte from the parent structure). But for standalone
        // Vec<u8> buffers, this reads past the allocation — causing UB and panics on
        // reserved bytes (0x40-0x7F) under valgrind. A trailing 0x00 is Arity(0), which
        // is valid and harmless (just pushes an empty breadcrumb that gets popped immediately).
        //
        // All three phases (byte matching, specificity filter, binding extraction) are
        // performed inside the thread-local borrow to avoid copying the buffer out.
        MATCH_EXPR_BUFFER.with(|buf_cell| {
            let mut buf = buf_cell.borrow_mut();

            // Check the MORK bytes cache first. Content-hash keyed: survives GC safepoints,
            // shares entries across structurally-identical expressions at different addresses.
            let cache_key = expr.hash_value();
            let expr_arity = arity;
            let cache_hit = MORK_BYTES_CACHE.with(|cache_cell| {
                let mut cache = cache_cell.borrow_mut();
                if let Some(entry) = cache.get(&cache_key) {
                    // Cheap collision validation: arity must match
                    if entry.1 == expr_arity {
                        buf.clear();
                        buf.reserve(entry.0.len());
                        buf.extend_from_slice(&entry.0);
                        return true;
                    }
                }
                false
            });

            let serialize_ok = if cache_hit {
                Ok(())
            } else {
                let result = with_mork_bytes(expr, &self.shared_mapping, self.mork_cache_epoch, |bytes| {
                    buf.clear();
                    buf.reserve(bytes.len() + 1);
                    buf.extend_from_slice(bytes);
                    buf.push(0x00); // Padding byte for ExprZipper read-past-end safety
                });
                // Cache the serialized bytes on success
                if result.is_ok() {
                    MORK_BYTES_CACHE.with(|cache_cell| {
                        cache_cell.borrow_mut().put(cache_key, (buf.clone(), expr_arity));
                    });
                }
                result
            };

            if serialize_ok.is_err() {
                // MORK can't encode this expression (e.g., a child S-expression
                // has arity >= 64).  Use Wide MORK byte-level matching for candidates
                // with lhs_wide_debruijn, and structural fallback for MORK-only candidates.
                drop(buf);

                // Encode expr to wide storage bytes ONCE for all wide candidates
                let mut expr_wide_buf = Vec::with_capacity(256);
                crate::backend::wide_mork::encoding::encode_wide_storage(expr, &mut expr_wide_buf);

                let rule_index = self.shared.rule_index.read();
                let candidates: Vec<&RuleEntry<V>> = if !head.is_empty() {
                    rule_index.get_candidates(head, arity).collect()
                } else {
                    rule_index.get_all_rules().collect()
                };

                let mut results: Vec<RuleMatchResult<V>> = Vec::new();
                for entry in candidates {
                    // Try wide MORK byte-level matching first (for wide rules)
                    let matched_bindings = if !entry.lhs_wide_debruijn.is_empty() {
                        if crate::backend::wide_mork::extract::wide_extract_data(
                            &entry.lhs_wide_debruijn,
                            &expr_wide_buf,
                        ).is_ok() {
                            Some(extract_bindings_from_wide_expr(
                                &entry.lhs_wide_debruijn,
                                expr,
                                &entry.var_names,
                                &entry.wildcard_indices,
                            ))
                        } else {
                            None
                        }
                    } else {
                        // Structural pattern match fallback for MORK-only candidates
                        // (their LHS has arity < 64, but the query has arity >= 64,
                        // so MORK can't encode the query — use MettaValue-level matching)
                        crate::backend::eval::trampoline::pattern_match_generic(&entry.lhs, expr)
                    };

                    if let Some(bindings) = matched_bindings {
                        // Phase 6: Skip apply_bindings for ground RHS (no variables).
                        // Cached at insertion time — avoids contains_variables() tree walk
                        // and apply_bindings recursion for rules with ground RHS.
                        let instantiated_rhs = if entry.rhs_has_variables {
                            apply_bindings(&entry.rhs, &bindings, &self.factory)
                        } else {
                            entry.rhs.clone()
                        };
                        let multiplicity = entry.multiplicity.max(1);
                        if multiplicity == 1 {
                            results.push(RuleMatchResult {
                                instantiated_rhs,
                                rhs_template: entry.rhs.clone(),
                                bindings,
                                multiplicity: 1,
                                rhs_type: entry.rhs_type.clone(),
                            });
                        } else {
                            for _ in 0..multiplicity {
                                results.push(RuleMatchResult {
                                    instantiated_rhs: instantiated_rhs.clone(),
                                    rhs_template: entry.rhs.clone(),
                                    bindings: bindings.clone(),
                                    multiplicity,
                                    rhs_type: entry.rhs_type.clone(),
                                });
                            }
                        }
                    }
                }
                return results;
            }

            // Validate expr buffer contains valid MORK bytes (no reserved 0x40-0x7F)
            #[cfg(debug_assertions)]
            {
                // Validate excluding the padding byte
                if let Err((off, byte)) = validate_mork_bytes(&buf[..buf.len() - 1]) {
                    panic!(
                        "expr buffer has invalid byte 0x{:02x} at offset {} (len={}).\n\
                         expr_bytes: {:02x?}\n\
                         expr: {:?}",
                        byte, off, buf.len() - 1,
                        &buf[..buf.len().min(64)],
                        expr
                    );
                }
            }

            // Get candidates from RuleIndex (read lock — multiple concurrent readers OK)
            let rule_index = self.shared.rule_index.read();

            // Phase 1: Byte-level pattern matching via extract_data.
            // Store entry references directly in MatchHit, eliminating the intermediate
            // candidates Vec allocation (which can hold 1000s of entries for large programs).
            struct MatchHit<'a, V: MettaValueTrait + Clone> {
                entry: &'a RuleEntry<V>,
                is_wide: bool,  // true if matched via Wide MORK
            }

            let mut hits: Vec<MatchHit<'_, V>> = Vec::new();

            // Lazily-computed wide storage encoding of expr (only allocated if needed)
            let mut expr_wide_storage: Option<Vec<u8>> = None;

            // Phase 7: Borrow the thread-local ExprZipper for reuse across match candidates.
            // reset() uses set_len(0) to clear the trace Vec without deallocating — after
            // warmup, the capacity grows to max expression depth and all subsequent matches
            // are zero-alloc (no jemalloc calls for Vec<Breadcrumb>).
            MATCH_ZIPPER.with(|zipper_cell| {
            let mut input_zipper = zipper_cell.borrow_mut();

            // Inline macro to avoid duplicating the match body for both iterator paths
            macro_rules! try_match_entry {
                ($entry:expr) => {
                    let entry = $entry;

                    // Try MORK byte-level matching first (fast path for arity < 64)
                    if !entry.lhs_debruijn.is_empty() {
                        if let Err(reserved) = maybe_byte_item(entry.lhs_debruijn[0]) {
                            tracing::warn!(
                                target: "mettatron::match_rules_native",
                                "RuleEntry has invalid first byte 0x{:02x} in lhs_debruijn (len={}), \
                                 head={}, arity={}, lhs_bytes={:02x?}",
                                reserved,
                                entry.lhs_debruijn.len(),
                                head,
                                arity,
                                &entry.lhs_debruijn[..entry.lhs_debruijn.len().min(16)]
                            );
                        } else {
                            let lhs_expr = Expr { ptr: entry.lhs_debruijn.as_ptr().cast_mut() };
                            // Phase 7: Reuse thread-local zipper — reset() preserves Vec capacity
                            input_zipper.root = Expr { ptr: buf.as_ptr().cast_mut() };
                            input_zipper.reset();
                            if lhs_expr.extract_data(&mut input_zipper).is_ok() {
                                hits.push(MatchHit { entry, is_wide: false });
                            }
                        }
                    } else if !entry.lhs_wide_debruijn.is_empty() {
                        // Wide MORK path: encode expr to wide storage bytes (lazy) and match
                        let wide_data = expr_wide_storage.get_or_insert_with(|| {
                            let mut wide_buf = Vec::with_capacity(256);
                            crate::backend::wide_mork::encoding::encode_wide_storage(expr, &mut wide_buf);
                            wide_buf
                        });
                        if crate::backend::wide_mork::extract::wide_extract_data(
                            &entry.lhs_wide_debruijn,
                            wide_data,
                        ).is_ok() {
                            hits.push(MatchHit { entry, is_wide: true });
                        }
                    }
                    // else: both empty — skip (shouldn't happen)
                };
            }

            if !head.is_empty() {
                for entry in rule_index.get_candidates(head, arity) {
                    try_match_entry!(entry);
                }
            } else {
                for entry in rule_index.get_all_rules() {
                    try_match_entry!(entry);
                }
            }

            }); // end MATCH_ZIPPER.with — drop zipper borrow before Phase 3

            if hits.is_empty() {
                return Vec::new();
            }

            // Phase 2 (removed): The old specificity filter was removed because MeTTa HE
            // has no specificity filter — all matching rules fire nondeterministically.
            // The old filter incorrectly dropped structurally-more-specific rules when a
            // variable-only rule happened to have fewer NewVar tags.

            // Phase 3: Extract bindings from original expression and build results.
            // Uses parallel tree walk instead of MORK deserialization to preserve
            // runtime types (SpaceHandle, State, etc.) that can't survive a round trip.
            let mut results: Vec<RuleMatchResult<V>> = Vec::with_capacity(hits.len());

            for hit in &hits {
                let entry = hit.entry;

                // Extract bindings using the appropriate decoder
                let bindings = if hit.is_wide {
                    extract_bindings_from_wide_expr(
                        &entry.lhs_wide_debruijn,
                        expr,
                        &entry.var_names,
                        &entry.wildcard_indices,
                    )
                } else {
                    extract_bindings_from_expr(
                        &entry.lhs_debruijn,
                        expr,
                        &entry.var_names,
                        &entry.wildcard_indices,
                    )
                };

                // Apply bindings to the cached RHS template.
                // Phase 6: Skip apply_bindings for ground RHS (no variables).
                let instantiated_rhs = if entry.rhs_has_variables {
                    apply_bindings(&entry.rhs, &bindings, &self.factory)
                } else {
                    entry.rhs.clone()
                };

                // Expand by multiplicity — fast path for common case (multiplicity=1)
                // avoids cloning bindings/instantiated_rhs when a move suffices
                let multiplicity = entry.multiplicity.max(1);
                if multiplicity == 1 {
                    results.push(RuleMatchResult {
                        instantiated_rhs,
                        rhs_template: entry.rhs.clone(),
                        bindings,
                        multiplicity: 1,
                        rhs_type: entry.rhs_type.clone(),
                    });
                } else {
                    for _ in 0..multiplicity {
                        results.push(RuleMatchResult {
                            instantiated_rhs: instantiated_rhs.clone(),
                            rhs_template: entry.rhs.clone(),
                            bindings: bindings.clone(),
                            multiplicity,
                            rhs_type: entry.rhs_type.clone(),
                        });
                    }
                }
            }

            results
        })
    }

    /// Get matching rules for an expression from PathMap via trie prefix navigation.
    ///
    /// **NOTE**: Superseded by `match_rules_native()` for the primary hot path.
    /// Retained for `match_space` compatibility and fallback scenarios where
    /// De Bruijn-encoded rules in PathMap need to be iterated directly.
    ///
    /// Returns `(lhs, rhs, multiplicity)` tuples for all rules whose LHS
    /// head symbol and arity match the given expression. The caller is
    /// responsible for performing full pattern matching on the returned
    /// candidates.
    pub fn get_matching_rules_for_expr(&self, expr: &V) -> Vec<(V, V, u64)> {
        let head = expr.get_head_symbol().unwrap_or("");
        let arity = expr.get_arity();

        // Bloom filter O(1) rejection: skip when no head-specific rules exist
        // AND there are no wildcard rules (which match any head).
        if !head.is_empty() {
            let bloom_says_no = !self
                .shared
                .atom_space.head_arity_bloom
                .read()
                .may_contain(head.as_bytes(), arity as u8);
            if bloom_says_no && !self.shared.rule_index.read().has_wildcard_rules() {
                return Vec::new();
            }
        }

        let space = self.create_space();
        let rule_prefix_len = self.rule_prefix.len();
        let mut rules: Vec<(V, V, u64)> = Vec::new();

        // 1. Head+arity-specific prefix navigation (most selective)
        if !head.is_empty() {
            if let Some(head_prefix) = build_head_arity_prefix::<V, F>(
                &self.rule_prefix,
                head,
                arity,
                &self.factory,
                &self.shared_mapping,
                self.mork_cache_epoch,
            ) {
                self.collect_rules_from_prefix(
                    &space,
                    &head_prefix,
                    rule_prefix_len,
                    &mut rules,
                );
            }
        } else {
            // No head info — collect all rules under the rule prefix
            self.collect_rules_from_prefix(
                &space,
                &self.rule_prefix,
                rule_prefix_len,
                &mut rules,
            );
        }

        // 2. Collect wildcard rules (LHS is atom/variable, not S-expression)
        self.collect_wildcard_rules(&space, &self.rule_prefix, rule_prefix_len, head, arity, &mut rules);

        rules
    }

    /// Collect rules from a trie subtree rooted at `prefix`.
    ///
    /// **NOTE**: Superseded by `RuleIndex + extract_data()` for the primary hot path.
    /// Retained for `get_matching_rules_for_expr()` fallback used in tests.
    ///
    /// Navigates the trie to `prefix`, then for each entry:
    /// 1. Splits the MORK path bytes into LHS and RHS ranges using `mork_expr_byte_len()`
    /// 2. Deserializes LHS and RHS independently — never constructs `(= LHS RHS)`
    /// 3. Reads multiplicity in-place from zipper `val()`
    fn collect_rules_from_prefix(
        &self,
        space: &Space<Multiplicity>,
        prefix: &[u8],
        rule_prefix_len: usize,
        rules: &mut Vec<(V, V, u64)>,
    ) {
        let mut rz = space.btm.read_zipper();
        let descended = rz.descend_to_existing(prefix);
        if descended < prefix.len() {
            return; // Prefix doesn't exist in trie
        }
        while rz.to_next_val() {
            let path = rz.path();
            if !path.starts_with(prefix) {
                break; // Left the subtree
            }
            // Multiplicity directly from zipper value (no separate lookup)
            let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1).max(1);

            // Split path into LHS and RHS byte ranges.
            // path layout: [Arity(3)] ["=" bytes] [LHS bytes] [RHS bytes]
            //              |--- rule_prefix_len --|
            if path.len() <= rule_prefix_len {
                continue; // Path too short to contain LHS+RHS
            }
            // De Bruijn encoding: NewVar is in LHS, VarRef in RHS references LHS vars.
            // Must deserialize the FULL rule (= lhs rhs) as a single unit to share
            // the variable context, then extract lhs and rhs from the result.
            let full_rule = match mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path,
                space,
                &self.factory,
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (lhs, rhs) = match extract_rule_parts(&full_rule) {
                Some(parts) => parts,
                None => continue, // Not a valid rule — skip
            };

            rules.push((lhs, rhs, multiplicity));
        }
    }

    /// Collect wildcard rules (LHS is atom/variable, not S-expression).
    ///
    /// **NOTE**: Superseded by `RuleIndex.wildcard` vec for the primary hot path.
    /// Retained for `get_matching_rules_for_expr()` fallback used in tests.
    ///
    /// Wildcard rules like `(= $x $x)` have a non-S-expression LHS. Their LHS byte
    /// starts with `SymbolSize` (0xC1-0xFF), `NewVar` (0xC0), or `VarRef` (0x80-0xBF),
    /// not `Arity` (0x00-0x3F). After collecting head-specific matches, this scans
    /// the rule prefix subtree for non-Arity LHS entries.
    ///
    /// Rules are filtered by head+arity to match the old behavior:
    /// - Variable LHS (no head symbol) matches everything
    /// - Atom LHS with a specific head matches only when head+arity agree
    fn collect_wildcard_rules(
        &self,
        space: &Space<Multiplicity>,
        rule_prefix: &[u8],
        rule_prefix_len: usize,
        query_head: &str,
        query_arity: usize,
        rules: &mut Vec<(V, V, u64)>,
    ) {
        let mut rz = space.btm.read_zipper();
        let descended = rz.descend_to_existing(rule_prefix);
        if descended < rule_prefix.len() {
            return;
        }
        while rz.to_next_val() {
            let path = rz.path();
            if !path.starts_with(rule_prefix) {
                break;
            }
            if path.len() <= rule_prefix_len {
                continue;
            }
            let lhs_first_byte = path[rule_prefix_len];
            // Arity tag (0x00-0x3F) = S-expr LHS → skip (handled by head_prefix or rule_prefix)
            if (lhs_first_byte & 0b1100_0000) == 0b0000_0000 {
                continue;
            }

            // Atom/variable LHS — potential wildcard rule.
            // De Bruijn encoding: deserialize full rule as single unit for shared var context.
            let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1).max(1);

            let full_rule = match mork_bytes_to_generic_value::<V, F, Multiplicity>(
                path,
                space,
                &self.factory,
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (lhs, rhs) = match extract_rule_parts(&full_rule) {
                Some(parts) => parts,
                None => continue,
            };

            // Filter by head+arity: variable LHS (no head) matches everything,
            // atom LHS with a specific head must match the query head+arity.
            let rule_head = lhs.get_head_symbol().unwrap_or("");
            if !rule_head.is_empty()
                && (rule_head != query_head || lhs.get_arity() != query_arity)
            {
                continue;
            }

            rules.push((lhs, rhs, multiplicity));
        }
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

    /// Bulk add rules using De Bruijn encoding + RuleIndex population.
    ///
    /// Each rule is serialized with `with_mork_query_bytes` (De Bruijn) and inserted
    /// individually into PathMap + RuleIndex. This is consistent with `add_rule()`.
    ///
    /// # Arguments
    /// * `rules` - Vec of (lhs, rhs) pairs
    pub fn add_rules_bulk(&mut self, rules: Vec<(MettaValue, MettaValue)>) -> Result<(), String> {
        trace!(target: "mettatron::environment::add_rules_bulk", rule_count = rules.len());
        if rules.is_empty() {
            return Ok(());
        }

        self.make_owned();

        // Delegate to add_rule() for each — ensures consistent De Bruijn encoding + RuleIndex
        for (lhs, rhs) in rules {
            self.add_rule(lhs, rhs);
        }

        self.modified.store(true, Ordering::Release);
        Ok(())
    }

    /// Get the number of times a rule has been defined (multiplicity).
    ///
    /// Uses De Bruijn encoding to match the PathMap entry (rules are stored with De Bruijn).
    pub fn get_rule_count(&self, lhs: &MettaValue, rhs: &MettaValue) -> usize {
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            lhs.clone(),
            rhs.clone(),
        ]);

        let sm = self.shared_mapping.clone();
        match with_mork_query_bytes(&rule_sexpr, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
            let btm = self.shared.atom_space.btm.read();
            let count = get_multiplicity(&btm, mork_bytes);
            if count == 0 { 1 } else { count as usize }
        }) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — check wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(&rule_sexpr, &mut wide_key);
                let wbtm = self.shared.atom_space.wide_btm.read();
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                if count == 0 { 1 } else { count as usize }
            }
        }
    }

    /// Get the multiplicities (for serialization).
    /// The keys are hex-encoded MORK bytes for serialization stability.
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        let btm = self.shared.atom_space.btm.read();
        let mut result = HashMap::new();

        for (path, multiplicity) in btm.iter() {
            let count = multiplicity.count() as usize;
            let count = if count == 0 { 1 } else { count };
            let hex_key = hex::encode(&path);
            result.insert(hex_key, count);
        }

        result
    }

    /// Rebuild bloom filter, fuzzy matcher, and RuleIndex from PathMap.
    ///
    /// This is needed after deserializing an Environment from PathMap Par,
    /// since the serialization only preserves the PathMap, not the bloom filter
    /// or RuleIndex.
    ///
    /// ## De Bruijn Encoding
    ///
    /// PathMap stores De Bruijn-encoded bytes for rules. When deserialized, variables
    /// get epoch-suffixed names (`$a%42` instead of original `$x`). These are
    /// re-encoded to De Bruijn bytes (structurally identical) when rebuilding the RuleIndex.
    pub fn rebuild_bloom_filter(&mut self) {
        trace!(target: "mettatron::environment::rebuild_bloom_filter", "Rebuilding bloom filter, fuzzy matcher, and RuleIndex from PathMap");
        self.make_owned();

        // Clear the existing RuleIndex before rebuilding
        self.shared.rule_index.write().clear();

        let space = self.create_space();
        let rule_prefix_len = self.rule_prefix.len();

        for (path_bytes, multiplicity_val) in space.btm.iter() {
            let expr = Expr {
                ptr: path_bytes.as_ptr().cast_mut(),
            };
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let Some((lhs, rhs)) = extract_rule_parts(&value) {
                    // Update bloom filter + fuzzy matcher
                    let head_owned: Option<String> = lhs.get_head_symbol().map(|s| s.to_string());
                    let arity = lhs.get_arity();
                    if let Some(ref head) = head_owned {
                        self.shared.fuzzy_matcher.write().insert(head);
                        self.shared
                            .atom_space.head_arity_bloom
                            .write()
                            .insert(head.as_bytes(), arity as u8);
                    }

                    // Rebuild RuleIndex entry from De Bruijn bytes
                    let multiplicity = multiplicity_val.count().max(1);

                    // Re-serialize the rule to get De Bruijn bytes + ConversionContext
                    let rule_sexpr = MettaValue::SExpr(vec![
                        MettaValue::Atom("=".to_string()),
                        lhs.clone(),
                        rhs.clone(),
                    ]);

                    let sm = self.shared_mapping.clone();
                    let _ = with_mork_query_bytes(&rule_sexpr, &sm, self.mork_cache_epoch, |debruijn_bytes, ctx| {
                        // Split De Bruijn bytes to get LHS range
                        if debruijn_bytes.len() <= rule_prefix_len {
                            return;
                        }
                        let lhs_start = rule_prefix_len;
                        let lhs_byte_len = mork_expr_byte_len(&debruijn_bytes[lhs_start..]);
                        // Pad with 0x00 for ExprZipper read-past-end safety
                        let mut lhs_debruijn = Vec::with_capacity(lhs_byte_len + 1);
                        lhs_debruijn.extend_from_slice(&debruijn_bytes[lhs_start..lhs_start + lhs_byte_len]);
                        lhs_debruijn.push(0x00);

                        let lhs_var_count = count_newvar_tags(&lhs_debruijn);
                        let (var_names, wildcard_indices) =
                            build_var_names_and_wildcards(&ctx.var_names, lhs_var_count);

                        // Phase 8.1: Compute RHS type for branch pruning
                        let rhs_type = {
                            use crate::backend::eval::types_generic::infer_type_generic;
                            let inferred = infer_type_generic(&rhs, &self.factory, self);
                            if inferred.as_atom() == Some("%Undefined%") { None } else { Some(inferred) }
                        };

                        // Phase 10.1: Register inferred return type (bulk path)
                        if let Some(ref rt) = rhs_type {
                            if let Some(ref head) = head_owned {
                                self.register_inferred_type(head, rt);
                            }
                        }

                        // Phase 10.4: Synthesize arrow type (bulk path)
                        if let Some(ref head) = head_owned {
                            let has_declared_arrow = self.get_types_generic(head).iter().any(|t| {
                                t.as_sexpr()
                                    .and_then(|items| items.first().and_then(|v| v.as_atom()))
                                    == Some("->")
                            });
                            if !has_declared_arrow {
                                use crate::backend::eval::types_generic::infer_arrow_type_from_rule;
                                if let Some(arrow) = infer_arrow_type_from_rule(
                                    &lhs,
                                    &rhs,
                                    rhs_type.as_ref(),
                                    &self.factory,
                                    self,
                                ) {
                                    self.register_inferred_type(head, &arrow);
                                }
                            }
                        }

                        let entry = RuleEntry {
                            lhs: lhs.clone(),
                            rhs_has_variables: rhs.contains_variables(),
                            rhs: rhs.clone(),
                            lhs_debruijn,
                            lhs_wide_debruijn: Vec::new(), // Bulk path uses MORK encoding
                            var_names,
                            wildcard_indices,
                            multiplicity,
                            rhs_type,
                        };

                        // Set correct multiplicity (don't let add_rule deduplicate)
                        self.shared.rule_index.write().add_rule(
                            head_owned.as_deref(),
                            arity,
                            entry,
                        );
                    });
                }
            }
        }

        self.modified.store(true, Ordering::Release);
    }

    /// Increment the multiplicity count for a rule.
    ///
    /// Uses De Bruijn encoding to match the PathMap entry (rules are stored with De Bruijn).
    /// Also syncs the RuleIndex multiplicity.
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after increment.
    pub fn increment_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::increment_rule_multiplicity", ?rule_sexpr);
        self.make_owned();

        let sm = self.shared_mapping.clone();
        match with_mork_query_bytes(rule_sexpr, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
            let mut btm = self.shared.atom_space.btm.write();
            let new_count = increment_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => {
                // Sync RuleIndex: increment the matching entry's multiplicity
                if let Some((lhs, rhs)) = extract_rule_parts(rule_sexpr) {
                    // RuleIndex.add_rule increments multiplicity for duplicates
                    // We need a simpler increment — just find and bump
                    let mut idx = self.shared.rule_index.write();
                    for entries in idx.by_head_arity.values_mut() {
                        if let Some(entry) = entries.iter_mut().find(|e| e.lhs == lhs && e.rhs == rhs) {
                            entry.multiplicity += 1;
                            drop(idx);
                            return count;
                        }
                    }
                    if let Some(entry) = idx.wildcard.iter_mut().find(|e| e.lhs == lhs && e.rhs == rhs) {
                        entry.multiplicity += 1;
                    }
                }
                count
            }
            Err(_) => {
                self.modified.store(true, Ordering::Release);
                1
            }
        }
    }

    /// Decrement the multiplicity count for a rule.
    ///
    /// Uses De Bruijn encoding to match the PathMap entry (rules are stored with De Bruijn).
    /// Also syncs the RuleIndex multiplicity.
    ///
    /// # Arguments
    /// * `rule_sexpr` - The rule as a MettaValue s-expression `(= lhs rhs)`
    ///
    /// # Returns
    /// The new count after decrement, or 0 if the rule wasn't tracked.
    pub fn decrement_rule_multiplicity(&mut self, rule_sexpr: &MettaValue) -> usize {
        trace!(target: "mettatron::environment::decrement_rule_multiplicity", ?rule_sexpr);
        self.make_owned();

        let sm = self.shared_mapping.clone();
        match with_mork_query_bytes(rule_sexpr, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
            let old_count = {
                let btm = self.shared.atom_space.btm.read();
                get_multiplicity(&btm, mork_bytes)
            };

            if old_count == 0 {
                self.modified.store(true, Ordering::Release);
                return 0;
            }

            let mut btm = self.shared.atom_space.btm.write();
            let new_count = decrement_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => {
                // Sync RuleIndex: decrement (or remove if multiplicity reaches 0)
                if let Some((lhs, rhs)) = extract_rule_parts(rule_sexpr) {
                    self.shared.rule_index.write().remove_rule(&lhs, &rhs);
                }
                count
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

        match with_mork_bytes(value, &self.shared_mapping, self.mork_cache_epoch, |mork_bytes| {
            let mut btm = self.shared.atom_space.btm.write();
            let new_count = increment_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — use wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                super::multiplicity::add_atom(&mut wbtm, &wide_key);
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                drop(wbtm);

                self.shared.atom_space.total_atoms.fetch_add(1, Ordering::Relaxed);
                self.modified.store(true, Ordering::Release);
                count as usize
            }
        }
    }

    /// Decrement multiplicity for ANY atom.
    pub fn decrement_atom_multiplicity(&mut self, value: &MettaValue) -> usize {
        self.make_owned();

        match with_mork_bytes(value, &self.shared_mapping, self.mork_cache_epoch, |mork_bytes| {
            let old_count = {
                let btm = self.shared.atom_space.btm.read();
                get_multiplicity(&btm, mork_bytes)
            };

            if old_count == 0 {
                return 0;
            }

            let mut btm = self.shared.atom_space.btm.write();
            let new_count = decrement_multiplicity(&mut btm, mork_bytes);
            drop(btm);

            self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
            self.modified.store(true, Ordering::Release);
            new_count as usize
        }) {
            Ok(count) => count,
            Err(_) => {
                // Wide expression (arity >= 64) — use wide_btm
                let mut wide_key = Vec::new();
                crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                let mut wbtm = self.shared.atom_space.wide_btm.write();
                let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                if count > 0 {
                    super::multiplicity::remove_atom(&mut wbtm, &wide_key);
                    drop(wbtm);
                    self.shared.atom_space.total_atoms.fetch_sub(1, Ordering::Relaxed);
                    self.modified.store(true, Ordering::Release);
                }
                count.saturating_sub(1) as usize
            }
        }
    }

    /// Get multiplicity for ANY atom.
    pub fn get_atom_multiplicity(&self, value: &MettaValue) -> usize {
        // Rules are stored with De Bruijn encoding, so we must look them up the same way.
        if extract_rule_parts(value).is_some() {
            let sm = self.shared_mapping.clone();
            match with_mork_query_bytes(value, &sm, self.mork_cache_epoch, |mork_bytes, _ctx| {
                let btm = self.shared.atom_space.btm.read();
                let count = get_multiplicity(&btm, mork_bytes);
                if count == 0 { 1 } else { count as usize }
            }) {
                Ok(count) => count,
                Err(_) => {
                    // Wide expression (arity >= 64) — check wide_btm
                    let mut wide_key = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                    let wbtm = self.shared.atom_space.wide_btm.read();
                    let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                    if count == 0 { 1 } else { count as usize }
                }
            }
        } else {
            match with_mork_bytes(value, &self.shared_mapping, self.mork_cache_epoch, |mork_bytes| {
                let btm = self.shared.atom_space.btm.read();
                let count = get_multiplicity(&btm, mork_bytes);
                if count == 0 { 1 } else { count as usize }
            }) {
                Ok(count) => count,
                Err(_) => {
                    // Wide expression (arity >= 64) — check wide_btm
                    let mut wide_key = Vec::new();
                    crate::backend::wide_mork::encoding::encode_wide_storage(value, &mut wide_key);
                    let wbtm = self.shared.atom_space.wide_btm.read();
                    let count = super::multiplicity::get_multiplicity(&wbtm, &wide_key);
                    if count == 0 { 1 } else { count as usize }
                }
            }
        }
    }

    /// Get atom multiplicity from raw MORK bytes.
    pub fn get_multiplicity_from_mork_bytes(&self, mork_bytes: &[u8]) -> usize {
        let btm = self.shared.atom_space.btm.read();
        let count = get_multiplicity(&btm, mork_bytes);
        if count == 0 { 1 } else { count as usize }
    }

    /// Get the count of distinct wide atoms (arity >= 64) stored in wide_btm.
    /// Returns 0 if no wide atoms exist.
    pub fn get_wide_atom_count(&self) -> usize {
        use pathmap::zipper::ZipperIteration;
        let wbtm = self.shared.atom_space.wide_btm.read();
        let mut count = 0usize;
        let mut rz = wbtm.read_zipper();
        while rz.to_next_val() {
            count += 1;
        }
        count
    }
}
