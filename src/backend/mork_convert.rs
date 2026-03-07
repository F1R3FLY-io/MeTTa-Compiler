//! Conversion utilities between MettaValue and MORK Expr format
//!
//! This module handles the bidirectional conversion needed for query_multi integration:
//! - MettaValue → MORK Expr (for pattern queries)
//! - MORK bindings → SmallVec<[(String, MettaValue); 8]> (for pattern match results)
//!
//! ## Optimization: Unified Thread-Local State
//!
//! All conversion state (256KB buffer, scratch buffer, variable context) is held in a
//! single thread-local `ConvertState`. This eliminates:
//! - Pool management overhead (acquire/release/size hints)
//! - Multiple RefCell borrows per conversion
//! - Heap allocations for intermediate strings (using itoa/ryu + scratch buffer)
//!
//! ## Zero-Copy Callback API
//!
//! The primary API uses callbacks (`with_mork_bytes`, `with_mork_query_bytes`) that
//! receive `&[u8]` from the thread-local buffer, avoiding the final `to_vec()` copy
//! on every serialization call. Backward-compatible wrappers are provided for callers
//! that need owned `Vec<u8>`.

use super::hash_utils::FxBuildHasher;
use super::models::gc_allocator::global_allocator;
use super::models::{Bindings, MettaValue, MettaValueInner, MettaValueTrait};
use mork::space::{ParDataParser, Space};
use mork_expr::{Expr, ExprEnv, ExprZipper};
use mork_frontend::bytestring_parser::Parser;
use mork_interning::SharedMappingHandle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, trace, warn};

/// Returns `true` if `METTA_GC_TRACE` env var is set. Cached after first check.
fn gc_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("METTA_GC_TRACE").is_ok())
}

// ============================================================================
// Monotonic Epoch Counter — ABA-safe cache invalidation
// ============================================================================

/// Monotonic epoch counter for MORK symbol cache invalidation.
///
/// Each `GenericEnvironment` is assigned a unique epoch at construction.
/// Epochs are never reused, eliminating the ABA problem where a dropped
/// `SharedMapping` has its heap address reused by a new allocation.
static MORK_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Allocate a new unique epoch for a `GenericEnvironment`.
///
/// Monotonically increasing — guaranteed never to repeat. `Relaxed` ordering
/// is sufficient because epoch values are only compared for equality, never
/// used to establish happens-before relationships between threads.
#[inline]
pub fn next_mork_epoch() -> u64 {
    MORK_EPOCH.fetch_add(1, Ordering::Relaxed)
}

// ============================================================================
// Unified Thread-Local Conversion State
// ============================================================================

/// Maximum buffer size for MORK expression serialization (256KB).
const MAX_MORK_BUFFER: usize = 262144;

thread_local! {
    static CONVERT_STATE: RefCell<ConvertState> = RefCell::new(ConvertState::new());
}

/// Cached MORK symbol ID with its actual length.
///
/// Stores up to 8 bytes of the interned symbol ID returned by MORK's `tokenizer()`.
/// This avoids repeated `get_sym_or_insert()` calls (RwLock shard + PathMap trie traversal)
/// for the same symbols that appear thousands of times during serialization.
#[derive(Clone, Copy)]
struct CachedSymbolId {
    bytes: [u8; 8],
    len: u8,
}

/// Unified thread-local state for MORK conversion.
///
/// Combines the buffer, scratch space, variable context, and symbol cache into a
/// single struct to minimize RefCell borrows and avoid pool management overhead.
struct ConvertState {
    /// Reusable 256KB buffer for ExprZipper writing.
    buffer: Vec<u8>,
    /// Reusable scratch buffer for assembling quoted strings, numeric formatting, etc.
    scratch: Vec<u8>,
    /// Reusable ConversionContext (var_map + var_names for De Bruijn tracking).
    context: ConversionContext,
    /// Content-addressed cache of MORK symbol IDs, scoped to a SharedMappingHandle.
    ///
    /// Maps byte content → interned symbol ID. Unlike the previous pointer-based cache,
    /// this keys on actual byte content, so temporary buffers (scratch, itoa::Buffer,
    /// ryu::Buffer) with the same pointer but different content always get correct results.
    ///
    /// The cache is invalidated when the environment epoch changes (different
    /// environment = different symbol table = different interned IDs for same bytes).
    ///
    /// The cache grows monotonically but is bounded by the number of unique symbols
    /// (~50-200 for typical programs). Uses xxh3 hasher for fast key hashing.
    symbol_cache: HashMap<Vec<u8>, CachedSymbolId, FxBuildHasher>,
    /// Monotonic epoch of the environment that the symbol_cache is scoped to.
    /// When the caller uses a different epoch, the cache is cleared.
    /// Epochs are never reused, eliminating the ABA pointer reuse problem.
    symbol_cache_epoch: u64,
}

impl ConvertState {
    fn new() -> Self {
        Self {
            buffer: vec![0u8; MAX_MORK_BUFFER],
            scratch: Vec::with_capacity(256),
            context: ConversionContext::new(),
            symbol_cache: HashMap::with_hasher(FxBuildHasher),
            symbol_cache_epoch: 0,
        }
    }

    /// Ensure the symbol cache is valid for the given environment epoch.
    /// If the epoch changed (different environment), clear the cache.
    ///
    /// Epochs are monotonically increasing `u64` values assigned at environment
    /// construction. Unlike pointer-based identity, epochs are never reused,
    /// eliminating the ABA problem where a dropped `SharedMapping` has its heap
    /// address recycled by a new allocation.
    #[inline]
    fn validate_symbol_cache(&mut self, epoch: u64) {
        if self.symbol_cache_epoch != epoch {
            self.symbol_cache.clear();
            self.symbol_cache_epoch = epoch;
        }
    }
}

// ============================================================================
// ConversionContext — Variable tracking for De Bruijn encoding
// ============================================================================

/// Context for tracking variables during MettaValue → Expr conversion
#[derive(Default)]
pub struct ConversionContext {
    /// Maps variable names to their De Bruijn indices
    pub var_map: HashMap<String, u8>,
    /// Reverse map: De Bruijn index → variable name
    pub var_names: Vec<String>,
}

impl ConversionContext {
    pub fn new() -> Self {
        ConversionContext {
            var_map: HashMap::new(),
            var_names: Vec::new(),
        }
    }

    /// Get or create a De Bruijn index for a variable
    pub fn get_or_create_var(&mut self, name: &str) -> Result<Option<u8>, String> {
        if let Some(&idx) = self.var_map.get(name) {
            // Variable already exists, return its index
            Ok(Some(idx))
        } else {
            // New variable
            if self.var_names.len() >= 64 {
                return Err("Too many variables (max 64)".to_string());
            }
            let idx = self.var_names.len() as u8;
            self.var_map.insert(name.to_string(), idx);
            self.var_names.push(name.to_string());
            Ok(None) // None means "write NewVar tag"
        }
    }
}

// ============================================================================
// Zero-Copy Callback API
// ============================================================================

/// Serialize a MettaValueTrait value to MORK bytes (literal symbol encoding for storage).
///
/// Zero heap allocation — the callback receives `&[u8]` from a thread-local buffer.
/// Variables are written as literal symbols, preserving names through MORK round-trip.
///
/// ## Re-entrancy
///
/// The thread-local `CONVERT_STATE` is borrowed for the duration of the callback.
/// The callback must NOT call `with_mork_bytes` or `with_mork_query_bytes` again
/// (this would panic on double RefCell borrow). Deserialization via `DESER_STATE`
/// in `mork_encoding.rs` is safe because it uses a separate thread-local.
pub fn with_mork_bytes<V: MettaValueTrait, R>(
    value: &V,
    sm: &SharedMappingHandle,
    cache_epoch: u64,
    f: impl FnOnce(&[u8]) -> R,
) -> Result<R, String> {
    CONVERT_STATE.with(|state| {
        let mut state = state.borrow_mut();
        // Invalidate symbol cache if the environment epoch changed.
        state.validate_symbol_cache(cache_epoch);
        let ConvertState { buffer, scratch, context, symbol_cache, .. } = &mut *state;
        context.var_map.clear();
        context.var_names.clear();
        // buffer is initialized to MAX_MORK_BUFFER in ConvertState::new()
        // and never shrinks, so no resize check needed
        let expr = Expr { ptr: buffer.as_mut_ptr() };
        let mut ez = ExprZipper::new(expr);
        let mut pdp = ParDataParser::new(sm);
        // Dispatch through MettaValueInner for efficient single-match (jump table)
        let inner = unsafe { &*value.inner_ptr() };
        write_metta_value_inner(inner, &mut pdp, context, &mut ez, scratch, symbol_cache)?;
        if ez.loc > MAX_MORK_BUFFER {
            return Err(format!(
                "Expression too large: {} bytes (max {})",
                ez.loc, MAX_MORK_BUFFER
            ));
        }
        Ok(f(&buffer[..ez.loc]))
    })
}

/// Serialize a MettaValueTrait value to MORK query bytes (De Bruijn encoding for pattern matching).
///
/// Zero heap allocation — the callback receives both `&[u8]` AND `&ConversionContext`
/// for variable name recovery in binding results.
///
/// Variables (`$x`, `&y`, `'z`) are encoded as MORK NewVar/VarRef using De Bruijn indices.
/// Wildcards (`_`) are encoded as MORK NewVar (anonymous variables).
pub fn with_mork_query_bytes<V: MettaValueTrait, R>(
    value: &V,
    sm: &SharedMappingHandle,
    cache_epoch: u64,
    f: impl FnOnce(&[u8], &ConversionContext) -> R,
) -> Result<R, String> {
    CONVERT_STATE.with(|state| {
        let mut state = state.borrow_mut();
        // Invalidate symbol cache if the environment epoch changed.
        state.validate_symbol_cache(cache_epoch);
        let ConvertState { buffer, scratch, context, symbol_cache, .. } = &mut *state;
        context.var_map.clear();
        context.var_names.clear();
        let expr = Expr { ptr: buffer.as_mut_ptr() };
        let mut ez = ExprZipper::new(expr);
        let mut pdp = ParDataParser::new(sm);
        let inner = unsafe { &*value.inner_ptr() };
        write_metta_value_debruijn_inner(inner, &mut pdp, context, &mut ez, scratch, symbol_cache)?;
        if ez.loc > MAX_MORK_BUFFER {
            return Err(format!(
                "Expression too large: {} bytes (max {})",
                ez.loc, MAX_MORK_BUFFER
            ));
        }
        Ok(f(&buffer[..ez.loc], context))
    })
}

// ============================================================================
// Backward-Compatible Wrappers (allocate Vec<u8>)
// ============================================================================

/// Convert MettaValue to MORK Expr bytes (backward-compatible wrapper).
///
/// This allocates a `Vec<u8>` copy. Prefer `with_mork_bytes()` when the bytes
/// are used transiently (e.g., PathMap insert/lookup within a callback).
///
/// The `ctx` parameter is accepted for API compatibility but ignored —
/// the thread-local context is used instead.
pub fn metta_to_mork_bytes(
    value: &MettaValue,
    sm: &SharedMappingHandle,
    cache_epoch: u64,
    _ctx: &mut ConversionContext,
) -> Result<Vec<u8>, String> {
    trace!(
        target: "mettatron::conversion::metta_to_mork_bytes",
        ?value, "Converting MettaValue to MORK bytes"
    );
    with_mork_bytes(value, sm, cache_epoch, |bytes| bytes.to_vec())
}

/// Convert MettaValue to MORK query pattern bytes (backward-compatible wrapper).
///
/// Variables are encoded using De Bruijn indices for MORK's `query_multi()`.
/// The `ctx` is populated with variable mappings for `mork_bindings_to_metta()`.
pub fn metta_to_mork_query_bytes(
    value: &MettaValue,
    sm: &SharedMappingHandle,
    cache_epoch: u64,
    ctx: &mut ConversionContext,
) -> Result<Vec<u8>, String> {
    with_mork_query_bytes(value, sm, cache_epoch, |bytes, inner_ctx| {
        // Copy var_names to caller's ctx for backward compat
        ctx.var_map.clone_from(&inner_ctx.var_map);
        ctx.var_names.clone_from(&inner_ctx.var_names);
        bytes.to_vec()
    })
}

// ============================================================================
// Internal Write Functions — MettaValueInner dispatch with scratch buffer
// ============================================================================

/// Recursively write MettaValueInner to ExprZipper.
///
/// Variables (`$x`, `&y`, `'z`) and wildcards (`_`) are written as literal symbols,
/// preserving their names through MORK round-trip (storage → PathMap → deserialization).
///
/// Uses `scratch` buffer for string quoting and `itoa`/`ryu` for numeric formatting
/// to avoid heap String allocations.
#[inline]
fn write_metta_value_inner(
    inner: &MettaValueInner,
    pdp: &mut ParDataParser,
    ctx: &mut ConversionContext,
    ez: &mut ExprZipper,
    scratch: &mut Vec<u8>,
    symbol_cache: &mut HashMap<Vec<u8>, CachedSymbolId, FxBuildHasher>,
) -> Result<(), String> {
    // Pre-bounds-check: detect buffer overrun before any write.
    // ExprZipper writes may advance loc past MAX_MORK_BUFFER in deeply nested
    // expressions (e.g., PLN's 15-deep S-expression trees). Without this check,
    // the write would corrupt stack memory and cause a SEGFAULT.
    if ez.loc >= MAX_MORK_BUFFER {
        return Err(format!(
            "MORK buffer overflow at loc={} (max={}): expression too deeply nested or too large",
            ez.loc, MAX_MORK_BUFFER
        ));
    }
    // GC trace mode: validate that inner ptr hasn't been freed by GC.
    // This catches use-after-free immediately with a diagnostic message
    // instead of a cryptic SIGSEGV deep in the match arms.
    if gc_trace_enabled() {
        let ptr = inner as *const MettaValueInner as *const u8;
        if !global_allocator().is_value_ptr_valid(ptr) {
            panic!(
                "write_metta_value_inner: DANGLING POINTER {:p} — value was freed by GC \
                 (slot epoch = u64::MAX or ptr not in any page). \
                 Run with ASAN for allocation/deallocation stacks.",
                ptr
            );
        }
    }
    match inner {
        MettaValueInner::Atom(name) => {
            // All atoms (including variables like $x and wildcards _) are written as symbols.
            // This preserves names through MORK round-trip for correct rule matching.
            write_symbol(name.as_bytes(), pdp, ez, symbol_cache)?;
        }

        MettaValueInner::Bool(b) => {
            if *b {
                write_symbol(b"true", pdp, ez, symbol_cache)?;
            } else {
                write_symbol(b"false", pdp, ez, symbol_cache)?;
            }
        }

        MettaValueInner::Long(n) => {
            let mut ibuf = itoa::Buffer::new();
            let s = ibuf.format(*n);
            write_symbol(s.as_bytes(), pdp, ez, symbol_cache)?;
        }

        MettaValueInner::Float(f) => {
            let mut rbuf = ryu::Buffer::new();
            let s = rbuf.format(*f);
            write_symbol(s.as_bytes(), pdp, ez, symbol_cache)?;
        }

        MettaValueInner::String(s) => {
            scratch.clear();
            scratch.push(b'"');
            scratch.extend_from_slice(s.as_bytes());
            scratch.push(b'"');
            write_symbol(scratch, pdp, ez, symbol_cache)?;
        }

        MettaValueInner::Unit => {
            // Empty list
            ez.write_arity(0);
            ez.loc += 1;
        }

        MettaValueInner::SExpr(items) => {
            // MORK arity is limited to 6 bits (0-63)
            if items.len() >= 64 {
                return Err(format!(
                    "Expression has too many children ({}) - MORK arity limit is 63",
                    items.len()
                ));
            }
            ez.write_arity(items.len() as u8);
            ez.loc += 1;

            for item in *items {
                write_metta_value_inner(item.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
            }
        }

        MettaValueInner::Error(msg, details) => {
            // (error "msg" details)
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"error", pdp, ez, symbol_cache)?;
            scratch.clear();
            scratch.push(b'"');
            scratch.extend_from_slice(msg.as_bytes());
            scratch.push(b'"');
            write_symbol(scratch, pdp, ez, symbol_cache)?;
            write_metta_value_inner(details.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }

        MettaValueInner::Type(t) => {
            // Types are just atoms/expressions
            write_metta_value_inner(t.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }

        MettaValueInner::Quoted(inner) => {
            // Write as (quote inner) for MORK compatibility
            ez.write_arity(2);
            ez.loc += 1;
            write_symbol(b"quote", pdp, ez, symbol_cache)?;
            write_metta_value_inner(inner.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }

        MettaValueInner::Conjunction(goals) => {
            // MORK arity is limited to 6 bits (0-63)
            // +1 for the comma symbol
            let total_arity = goals.len() + 1;
            if total_arity >= 64 {
                return Err(format!(
                    "Conjunction has too many goals ({}) - MORK arity limit is 63",
                    goals.len()
                ));
            }
            ez.write_arity(total_arity as u8);
            ez.loc += 1;

            // Write the comma symbol as first child
            write_symbol(b",", pdp, ez, symbol_cache)?;

            // Write each goal
            for goal in *goals {
                write_metta_value_inner(goal.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
            }
        }

        // Space references are written as (Space id name)
        MettaValueInner::Space(handle) => {
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"Space", pdp, ez, symbol_cache)?;
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(handle.id);
            write_symbol(id_str.as_bytes(), pdp, ez, symbol_cache)?;
            scratch.clear();
            scratch.push(b'"');
            scratch.extend_from_slice(handle.name.as_bytes());
            scratch.push(b'"');
            write_symbol(scratch, pdp, ez, symbol_cache)?;
        }

        // State references are written as (State id)
        MettaValueInner::State(id) => {
            ez.write_arity(2);
            ez.loc += 1;
            write_symbol(b"State", pdp, ez, symbol_cache)?;
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(*id);
            write_symbol(id_str.as_bytes(), pdp, ez, symbol_cache)?;
        }

        // Memo tables are runtime-only and cannot be stored in MORK
        MettaValueInner::Memo(handle) => {
            return Err(format!(
                "Cannot convert Memo table '{}' (id={}) to MORK - memoization tables are runtime-only",
                handle.name, handle.id
            ));
        }

        // Empty sentinel is runtime-only and should be filtered out before MORK conversion
        MettaValueInner::Empty => {
            return Err(
                "Cannot convert Empty sentinel to MORK - Empty should be filtered at result collection".to_string()
            );
        }

        // Spanned: strip span wrapper and serialize the inner value transparently
        MettaValueInner::Spanned(v, _) => {
            write_metta_value_inner(v.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }
    }

    Ok(())
}

/// Recursively write MettaValueInner to ExprZipper using De Bruijn encoding for variables.
///
/// Variables get De Bruijn indices; wildcards (`_`) become anonymous NewVar.
/// This is the encoding needed for MORK's `query_multi()` structural matching.
#[inline]
fn write_metta_value_debruijn_inner(
    inner: &MettaValueInner,
    pdp: &mut ParDataParser,
    ctx: &mut ConversionContext,
    ez: &mut ExprZipper,
    scratch: &mut Vec<u8>,
    symbol_cache: &mut HashMap<Vec<u8>, CachedSymbolId, FxBuildHasher>,
) -> Result<(), String> {
    // Pre-bounds-check: detect buffer overrun before any write.
    if ez.loc >= MAX_MORK_BUFFER {
        return Err(format!(
            "MORK buffer overflow at loc={} (max={}): expression too deeply nested or too large",
            ez.loc, MAX_MORK_BUFFER
        ));
    }
    // GC trace mode: validate that inner ptr hasn't been freed by GC.
    if gc_trace_enabled() {
        let ptr = inner as *const MettaValueInner as *const u8;
        if !global_allocator().is_value_ptr_valid(ptr) {
            panic!(
                "write_metta_value_debruijn_inner: DANGLING POINTER {:p} — value was freed by GC \
                 (slot epoch = u64::MAX or ptr not in any page). \
                 Run with ASAN for allocation/deallocation stacks.",
                ptr
            );
        }
    }
    match inner {
        MettaValueInner::Atom(name) => {
            if *name == "&" || *name == "&self" || *name == "&kb" || *name == "&stack" {
                write_symbol(name.as_bytes(), pdp, ez, symbol_cache)?;
            } else if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                let var_id = &name[1..];
                match ctx.get_or_create_var(var_id)? {
                    None => {
                        ez.write_new_var();
                        ez.loc += 1;
                    }
                    Some(idx) => {
                        ez.write_var_ref(idx);
                        ez.loc += 1;
                    }
                }
            } else if *name == "_" {
                // Wildcard — each occurrence is a unique anonymous variable.
                // Register in context to keep De Bruijn indices in sync.
                let mut ibuf = itoa::Buffer::new();
                let suffix = ibuf.format(ctx.var_names.len());
                scratch.clear();
                scratch.extend_from_slice(b"__anon");
                scratch.extend_from_slice(suffix.as_bytes());
                let anon_id = std::str::from_utf8(scratch).expect("valid utf8: __anon + integer");
                ctx.get_or_create_var(anon_id)?;
                ez.write_new_var();
                ez.loc += 1;
            } else {
                write_symbol(name.as_bytes(), pdp, ez, symbol_cache)?;
            }
        }

        MettaValueInner::Bool(b) => {
            if *b {
                write_symbol(b"true", pdp, ez, symbol_cache)?;
            } else {
                write_symbol(b"false", pdp, ez, symbol_cache)?;
            }
        }

        MettaValueInner::Long(n) => {
            let mut ibuf = itoa::Buffer::new();
            let s = ibuf.format(*n);
            write_symbol(s.as_bytes(), pdp, ez, symbol_cache)?;
        }

        MettaValueInner::Float(f) => {
            let mut rbuf = ryu::Buffer::new();
            let s = rbuf.format(*f);
            write_symbol(s.as_bytes(), pdp, ez, symbol_cache)?;
        }

        MettaValueInner::String(s) => {
            scratch.clear();
            scratch.push(b'"');
            scratch.extend_from_slice(s.as_bytes());
            scratch.push(b'"');
            write_symbol(scratch, pdp, ez, symbol_cache)?;
        }

        MettaValueInner::Unit => {
            ez.write_arity(0);
            ez.loc += 1;
        }

        MettaValueInner::SExpr(items) => {
            if items.len() >= 64 {
                return Err(format!(
                    "Expression has too many children ({}) - MORK arity limit is 63",
                    items.len()
                ));
            }
            ez.write_arity(items.len() as u8);
            ez.loc += 1;
            for item in *items {
                write_metta_value_debruijn_inner(item.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
            }
        }

        MettaValueInner::Error(msg, details) => {
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"error", pdp, ez, symbol_cache)?;
            scratch.clear();
            scratch.push(b'"');
            scratch.extend_from_slice(msg.as_bytes());
            scratch.push(b'"');
            write_symbol(scratch, pdp, ez, symbol_cache)?;
            write_metta_value_debruijn_inner(details.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }

        MettaValueInner::Type(t) => {
            write_metta_value_debruijn_inner(t.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }

        MettaValueInner::Quoted(inner) => {
            // Write as (quote inner) for MORK compatibility
            ez.write_arity(2);
            ez.loc += 1;
            write_symbol(b"quote", pdp, ez, symbol_cache)?;
            write_metta_value_debruijn_inner(inner.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }

        MettaValueInner::Conjunction(goals) => {
            let total_arity = goals.len() + 1;
            if total_arity >= 64 {
                return Err(format!(
                    "Conjunction has too many goals ({}) - MORK arity limit is 63",
                    goals.len()
                ));
            }
            ez.write_arity(total_arity as u8);
            ez.loc += 1;
            write_symbol(b",", pdp, ez, symbol_cache)?;
            for goal in *goals {
                write_metta_value_debruijn_inner(goal.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
            }
        }

        MettaValueInner::Space(handle) => {
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"Space", pdp, ez, symbol_cache)?;
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(handle.id);
            write_symbol(id_str.as_bytes(), pdp, ez, symbol_cache)?;
            scratch.clear();
            scratch.push(b'"');
            scratch.extend_from_slice(handle.name.as_bytes());
            scratch.push(b'"');
            write_symbol(scratch, pdp, ez, symbol_cache)?;
        }

        MettaValueInner::State(id) => {
            ez.write_arity(2);
            ez.loc += 1;
            write_symbol(b"State", pdp, ez, symbol_cache)?;
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(*id);
            write_symbol(id_str.as_bytes(), pdp, ez, symbol_cache)?;
        }

        MettaValueInner::Memo(handle) => {
            return Err(format!(
                "Cannot convert Memo table '{}' (id={}) to MORK - memoization tables are runtime-only",
                handle.name, handle.id
            ));
        }

        MettaValueInner::Empty => {
            return Err(
                "Cannot convert Empty sentinel to MORK - Empty should be filtered at result collection".to_string()
            );
        }

        // Spanned: strip span wrapper and serialize the inner value transparently
        MettaValueInner::Spanned(v, _) => {
            write_metta_value_debruijn_inner(v.inner_ref(), pdp, ctx, ez, scratch, symbol_cache)?;
        }
    }
    Ok(())
}

/// Write a symbol to ExprZipper using the provided ParDataParser.
///
/// The caller must provide a ParDataParser (which holds a WritePermit) that is held
/// for the duration of the entire conversion operation. This ensures MORK's threading
/// model is respected — each thread holds ONE WritePermit, not one per symbol.
///
/// Uses a content-addressed symbol cache to skip `get_sym_or_insert()` for previously
/// seen byte content (3.3% of total CPU time in profiling). The cache keys on actual
/// byte content (`&[u8]` → `Vec<u8>`) rather than pointer addresses, so temporary
/// buffers (scratch, itoa::Buffer, ryu::Buffer) that reuse the same memory address
/// with different content always produce correct results.
///
/// Zero allocation on cache hit: `HashMap::get(&[u8])` works because `Vec<u8>: Borrow<[u8]>`.
#[inline]
fn write_symbol(
    bytes: &[u8],
    pdp: &mut ParDataParser,
    ez: &mut ExprZipper,
    symbol_cache: &mut HashMap<Vec<u8>, CachedSymbolId, FxBuildHasher>,
) -> Result<(), String> {
    if let Some(&cached) = symbol_cache.get(bytes) {
        // Cache hit: write cached symbol ID directly (zero MORK interaction)
        let sym_slice = &cached.bytes[..cached.len as usize];
        ez.write_symbol(sym_slice);
        ez.loc += 1 + cached.len as usize;
    } else {
        // Cache miss: call MORK tokenizer and cache the result
        let token = pdp.tokenizer(bytes);
        let token_len = token.len();
        let mut sym_id = CachedSymbolId {
            bytes: [0u8; 8],
            len: token_len.min(8) as u8,
        };
        sym_id.bytes[..sym_id.len as usize].copy_from_slice(&token[..sym_id.len as usize]);
        symbol_cache.insert(bytes.to_vec(), sym_id); // one-time Vec alloc per unique symbol
        ez.write_symbol(token);
        ez.loc += 1 + token_len;
    }
    Ok(())
}

// ============================================================================
// MORK Bindings → MeTTa Bindings
// ============================================================================

/// Convert MORK bindings to Mettatron Bindings format
///
/// MORK uses BTreeMap<(u8, u8), ExprEnv> where the key is (old_var, new_var).
/// We need to convert this to SmallVec<[(String, MettaValue); 8]> using the original variable names.
///
/// FIXED: Uses mork_expr_to_metta_value() instead of serialize2() to avoid reserved byte panic
/// Now properly reports conversion errors instead of silently skipping bindings.
#[allow(unused_variables)]
pub fn mork_bindings_to_metta<V: Clone + Default + Send + Sync + Unpin>(
    mork_bindings: &std::collections::BTreeMap<(u8, u8), ExprEnv>,
    ctx: &ConversionContext,
    space: &Space<V>,
) -> Result<Bindings, String> {
    trace!(target: "mettatron::conversion::mork_bindings_to_metta", ?mork_bindings);

    use super::environment::MettaEnvironment;

    let mut bindings = Bindings::new();
    let mut conversion_errors: Vec<String> = Vec::new();

    for (&(namespace, var_index), expr_env) in mork_bindings {
        // MORK ExprVar = (namespace, var_index):
        //   namespace 0 = pattern-side bindings (what we want)
        //   namespace 1+ = stored-atom-side bindings (for future bidirectional matching)
        // Only process pattern-side bindings (namespace 0) for now.
        if namespace != 0 {
            continue;
        }

        // Get the variable name from context using var_index (NOT namespace)
        if (var_index as usize) >= ctx.var_names.len() {
            warn!(
                target: "mettatron::conversion::mork_bindings_to_metta",
                var_index, max_vars = ctx.var_names.len(),
                "Variable index exceeds known variables - internal inconsistency detected"
            );

            // Variable index out of bounds - this indicates an internal inconsistency
            conversion_errors.push(format!(
                "Variable index {} exceeds known variables (max: {})",
                var_index,
                ctx.var_names.len().saturating_sub(1)
            ));
            continue;
        }
        let var_name = &ctx.var_names[var_index as usize];

        // Convert MORK Expr directly to MettaValue
        // FIXED: Use mork_expr_to_metta_value() instead of serialize2()
        // This avoids the "reserved byte" panic when bindings contain symbols with reserved bytes
        let expr: Expr = expr_env.subsexpr();
        match MettaEnvironment::mork_expr_to_metta_value(&expr, space) {
            Ok(value) => {
                bindings.insert(format!("${}", var_name), value);
            }
            Err(e) => {
                debug!(
                    target: "mettatron::conversion::mork_bindings_to_metta",
                    var_name = %var_name, error = %e, "Failed to convert individual binding"
                );
                conversion_errors.push(format!(
                    "Failed to convert binding for ${}: {}",
                    var_name, e
                ));
            }
        }
    }

    // If there were any conversion errors, return an error with all failures listed
    if !conversion_errors.is_empty() {
        let errors = conversion_errors.join("\n  - ");
        warn!(
            target: "mettatron::conversion::mork_bindings_to_metta",
            errors, "MORK binding conversion partially failed"
        );
        return Err(format!("MORK binding conversion failed:\n  - {}", errors));
    }

    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;

    #[test]
    fn test_simple_atom_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();
        let mut ctx = ConversionContext::new();

        let atom = MettaValue::Atom("foo".to_string());
        let result = metta_to_mork_bytes(&atom, &space.sm, epoch, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_variable_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();
        let mut ctx = ConversionContext::new();

        // Variables are now written as literal symbols (not De Bruijn NewVar).
        // This preserves names through MORK round-trip for correct rule matching.
        let var = MettaValue::Atom("$x".to_string());
        let result = metta_to_mork_bytes(&var, &space.sm, epoch, &mut ctx);
        assert!(result.is_ok());
        // Context is NOT populated because variables are symbols, not De Bruijn.
        assert_eq!(ctx.var_names.len(), 0);
    }

    #[test]
    fn test_sexpr_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();
        let mut ctx = ConversionContext::new();

        // (double $x)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let result = metta_to_mork_bytes(&sexpr, &space.sm, epoch, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_repeated_variable() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();
        let mut ctx = ConversionContext::new();

        // (* $x $x) - same variable twice, written as literal symbols
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let result = metta_to_mork_bytes(&sexpr, &space.sm, epoch, &mut ctx);
        assert!(result.is_ok());
        // Variables are symbols now, not De Bruijn — context stays empty
        assert_eq!(ctx.var_names.len(), 0);
    }

    // =========================================================================
    // Zero-Copy Callback API Tests
    // =========================================================================

    #[test]
    fn test_with_mork_bytes_atom() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();

        let atom = MettaValue::Atom("foo".to_string());
        let len = with_mork_bytes(&atom, &space.sm, epoch, |bytes| bytes.len());
        assert!(len.is_ok());
        assert!(len.expect("should succeed") > 0);
    }

    #[test]
    fn test_with_mork_bytes_matches_compat() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();

        let atom = MettaValue::Atom("foo".to_string());
        let mut ctx = ConversionContext::new();
        let compat_bytes = metta_to_mork_bytes(&atom, &space.sm, epoch, &mut ctx).expect("compat ok");

        let callback_bytes = with_mork_bytes(&atom, &space.sm, epoch, |bytes| bytes.to_vec())
            .expect("callback ok");

        assert_eq!(compat_bytes, callback_bytes);
    }

    #[test]
    fn test_with_mork_bytes_ground_types() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();

        let values = vec![
            MettaValue::Bool(true),
            MettaValue::Bool(false),
            MettaValue::Long(42),
            MettaValue::Long(-123),
            MettaValue::Float(3.14159),
            MettaValue::String("hello world".to_string()),
            MettaValue::Unit(),
            MettaValue::Unit(),
        ];

        for value in values {
            let mut ctx = ConversionContext::new();
            let compat = metta_to_mork_bytes(&value, &space.sm, epoch, &mut ctx)
                .unwrap_or_else(|e| panic!("compat failed for {:?}: {}", value, e));

            let callback = with_mork_bytes(&value, &space.sm, epoch, |bytes| bytes.to_vec())
                .unwrap_or_else(|e| panic!("callback failed for {:?}: {}", value, e));

            assert_eq!(compat, callback, "Mismatch for {:?}", value);
        }
    }

    #[test]
    fn test_with_mork_bytes_complex_nested() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();

        // (exec P0 (, (a $x) (b $x)) (, (c $x)))
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![
                MettaValue::SExpr(vec![
                    MettaValue::Atom("a".to_string()),
                    MettaValue::Atom("$x".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("b".to_string()),
                    MettaValue::Atom("$x".to_string()),
                ]),
            ]),
            MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("$x".to_string()),
            ])]),
        ]);

        let mut ctx = ConversionContext::new();
        let compat = metta_to_mork_bytes(&sexpr, &space.sm, epoch, &mut ctx).expect("compat ok");

        let callback = with_mork_bytes(&sexpr, &space.sm, epoch, |bytes| bytes.to_vec())
            .expect("callback ok");

        assert_eq!(compat, callback);
    }

    #[test]
    fn test_with_mork_bytes_error_value() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();

        // (error "test error" (details here))
        let error = MettaValue::Error(
            "test error".to_string(),
            MettaValue::SExpr(vec![
                MettaValue::Atom("details".to_string()),
                MettaValue::Atom("here".to_string()),
            ]),
        );

        let mut ctx = ConversionContext::new();
        let compat = metta_to_mork_bytes(&error, &space.sm, epoch, &mut ctx).expect("compat ok");

        let callback = with_mork_bytes(&error, &space.sm, epoch, |bytes| bytes.to_vec())
            .expect("callback ok");

        assert_eq!(compat, callback);
    }

    #[test]
    fn test_with_mork_query_bytes_variables() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let epoch = env.mork_cache_epoch();

        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (callback_bytes, var_count) =
            with_mork_query_bytes(&pattern, &space.sm, epoch, |bytes, ctx| {
                (bytes.to_vec(), ctx.var_names.len())
            })
            .expect("callback ok");

        assert!(!callback_bytes.is_empty());
        assert_eq!(var_count, 1); // $x
    }
}
