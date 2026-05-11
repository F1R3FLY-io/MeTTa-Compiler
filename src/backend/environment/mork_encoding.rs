//! MORK encoding and conversion operations for Environment.
//!
//! Provides methods for converting between MORK expressions and MettaValues.
//! Handles the low-level byte encoding used by PathMap trie storage.
//!
//! ## Epoch-Based Variable Names
//!
//! Variable names use epoch-suffixed format ("$a%0", "$b%0", etc.) to prevent
//! variable capture bugs when rules from different scopes share the same
//! De Bruijn index but represent different logical variables.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use mork::space::Space;
use mork_expr::{maybe_byte_item, Expr, Tag};
use smallvec::SmallVec;
use tracing::warn;

use super::MettaValue;
use crate::backend::models::{global_factory, MettaValueFactory, MettaValueTrait};

// ============================================================================
// Thread-local deserialization state — eliminates per-call String allocations
// ============================================================================

thread_local! {
    static DESER_STATE: RefCell<DeserState> = RefCell::new(DeserState::new());
}

/// Thread-local deserialization state for variable name caching.
///
/// Variable names have the format `$<base>%<epoch>` (e.g., `$a%42`).
/// Each deserialization call gets a unique epoch, but the String heap capacity
/// is reused across calls. After warmup, zero allocations per deserialization.
///
/// Names are built lazily — only up to the count actually used.
struct DeserState {
    /// The epoch for which cached names are valid.
    cached_epoch: u64,
    /// How many names have been built for the current epoch.
    names_built: u8,
    /// Pre-allocated variable name strings (capacity reused across calls).
    cached_var_names: [String; 64],
}

impl DeserState {
    fn new() -> Self {
        // Initialize with empty strings that will gain capacity on first use
        const EMPTY: String = String::new();
        Self {
            cached_epoch: u64::MAX, // Sentinel — forces rebuild on first call
            names_built: 0,
            cached_var_names: [EMPTY; 64],
        }
    }

    /// Get the variable name for the given index and epoch.
    ///
    /// Lazily builds names up to the requested index. Reuses String heap
    /// capacity from previous epochs (zero allocation after warmup).
    #[inline]
    fn get_var_name(&mut self, index: u8, epoch: u64) -> &str {
        if self.cached_epoch != epoch {
            self.cached_epoch = epoch;
            self.names_built = 0;
        }
        // Build names up to and including index if not yet built
        while self.names_built <= index {
            let i = self.names_built as usize;
            let name = &mut self.cached_var_names[i];
            name.clear(); // Retains heap capacity
            name.push('$');
            name.push_str(VARNAME_BASES[i]);
            name.push('%');
            let mut ibuf = itoa::Buffer::new();
            name.push_str(ibuf.format(epoch));
            self.names_built += 1;
        }
        &self.cached_var_names[index as usize]
    }
}

/// Base variable names for MORK variables (without `$` prefix).
/// Each invocation of a MORK-to-value conversion generates unique variable names
/// by combining these bases with an epoch counter (e.g., `$a%0`, `$b%0`, `$a%1`).
/// This prevents variable capture bugs when rules from different scopes share
/// the same De Bruijn index but represent different logical variables.
static VARNAME_BASES: [&str; 64] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z", "a1", "b1", "c1", "d1", "e1", "f1", "g1", "h1", "i1", "j1",
    "k1", "l1", "m1", "n1", "o1", "p1", "q1", "r1", "s1", "t1", "u1", "v1", "w1", "x1", "y1", "z1",
    "a2", "b2", "c2", "d2", "e2", "f2", "g2", "h2", "i2", "j2", "k2", "l2",
];

/// Global epoch counter for unique variable name generation.
/// Each MORK-to-value conversion increments this to get a unique epoch,
/// ensuring that variables from different rule applications never collide.
static VARNAME_EPOCH: AtomicU64 = AtomicU64::new(0);

impl super::MettaEnvironment {
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
    pub(crate) fn mork_expr_to_metta_value<V: Clone + Default + Send + Sync + Unpin>(
        expr: &Expr,
        space: &Space<V>,
    ) -> Result<MettaValue, String> {
        // Delegate to the efficient factory-based generic implementation.
        // Since MettaValue = MettaValue, the factory allocates directly
        // into the global slab allocator without intermediate heap Strings.
        let factory = global_factory();
        super::mork_encoding::mork_expr_to_generic_value(expr, space, &factory)
    }
}

/// Convert MORK Expr to generic value V using factory.
///
/// This enables direct PathMap ↔ V conversion without MettaValue intermediate.
/// Uses factory methods to construct V values from MORK bytes.
///
/// ## Zero-Conversion Design
///
/// The factory-based approach means:
/// - `GcFactory` constructs `MettaValue` (= `MettaValue`) directly
/// - No intermediate type conversion needed
///
/// ## Performance
///
/// Stack-based traversal to avoid recursion limits on deeply nested expressions.
/// Uses epoch-based unique variable names to prevent variable capture across scopes.
#[allow(unused_variables)]
pub(crate) fn mork_expr_to_generic_value<V, F, M>(
    expr: &Expr,
    space: &Space<M>,
    factory: &F,
) -> Result<V, String>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
    M: Clone + Default + Send + Sync + Unpin,
{
    // Delegate to the bytes-based implementation
    // SAFETY: expr.ptr points to valid MORK bytes in PathMap memory
    let bytes = unsafe { std::slice::from_raw_parts(expr.ptr, mork_expr_len(expr.ptr)) };
    mork_bytes_to_generic_value(bytes, space, factory)
}

/// Get the length of a MORK expression by traversing it.
///
/// # Safety
/// The ptr must point to valid MORK expression bytes.
#[inline]
unsafe fn mork_expr_len(ptr: *const u8) -> usize {
    let mut offset = 0usize;
    let mut depth = 1u32; // Count of values we need to parse

    while depth > 0 {
        let byte = *ptr.add(offset);
        let tag = match maybe_byte_item(byte) {
            Ok(t) => t,
            Err(_) => return offset + 1, // Include the reserved byte
        };
        offset += 1;
        depth -= 1;

        match tag {
            Tag::NewVar | Tag::VarRef(_) => {}
            Tag::SymbolSize(size) => {
                offset += size as usize;
            }
            Tag::Arity(arity) => {
                depth += arity as u32;
            }
        }
    }
    offset
}

/// Compute the byte length of one MORK expression starting at `bytes[0]`.
///
/// Traverses the tag structure (Arity/SymbolSize/NewVar/VarRef) to determine
/// where the expression ends. No allocation, no value construction.
///
/// Returns 0 if the slice is empty. Returns a best-effort length on reserved bytes.
///
/// This is the safe, slice-based counterpart to `mork_expr_len(ptr)`.
#[inline]
pub(crate) fn mork_expr_byte_len(bytes: &[u8]) -> usize {
    let mut offset = 0usize;
    let mut depth = 1u32; // One expression to consume

    while depth > 0 && offset < bytes.len() {
        let byte = bytes[offset];
        let tag = match maybe_byte_item(byte) {
            Ok(t) => t,
            Err(_) => return offset + 1, // Reserved byte — include it and stop
        };
        offset += 1;
        depth -= 1;

        match tag {
            Tag::NewVar | Tag::VarRef(_) => {}
            Tag::SymbolSize(size) => {
                offset += size as usize;
            }
            Tag::Arity(arity) => {
                depth += arity as u32;
            }
        }
    }
    offset
}

/// Convert MORK bytes directly to a generic value V without Expr wrapper.
///
/// This is the zero-wrapper version that operates directly on byte slices.
/// Use this when you have the path bytes from a PathMap zipper.
///
/// ## Performance
///
/// Stack-based traversal to avoid recursion limits on deeply nested expressions.
/// Uses epoch-based unique variable names to prevent variable capture across scopes.
#[allow(unused_variables)]
pub(crate) fn mork_bytes_to_generic_value<V, F, M>(
    bytes: &[u8],
    space: &Space<M>,
    factory: &F,
) -> Result<V, String>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
    M: Clone + Default + Send + Sync + Unpin,
{
    // Stack-based traversal to avoid recursion limits.
    // SmallVec<[V; 4]> avoids heap allocation for S-expressions with ≤4 children
    // (the overwhelmingly common case: head + 1-3 args).
    enum StackFrame<V> {
        Arity {
            remaining: u8,
            items: SmallVec<[V; 4]>,
        },
    }

    let mut stack: Vec<StackFrame<V>> = Vec::new();
    let mut offset = 0usize;
    let mut newvar_count = 0u8;
    let epoch = VARNAME_EPOCH.fetch_add(1, Ordering::Relaxed);

    // Use thread-local DeserState for variable name caching (zero alloc after warmup)
    DESER_STATE.with(|state| {
        let mut ds = state.borrow_mut();

        'parsing: loop {
            if offset >= bytes.len() {
                return Err("Unexpected end of MORK bytes".to_string());
            }
            let byte = bytes[offset];
            let tag = match maybe_byte_item(byte) {
                Ok(t) => t,
                Err(reserved_byte) => {
                    warn!(
                        target: "mettatron::environment::mork_expr_to_generic_value",
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

            let value = match tag {
                Tag::NewVar => {
                    // De Bruijn index - NewVar introduces a new variable with the next index
                    // Use epoch-suffixed names to prevent variable capture across scopes
                    if (newvar_count as usize) >= VARNAME_BASES.len() {
                        return Err(format!("Too many variables: {}", newvar_count));
                    }
                    let var_name = ds.get_var_name(newvar_count, epoch);
                    let atom = factory.atom(var_name);
                    newvar_count += 1;
                    atom
                }
                Tag::VarRef(i) => {
                    if (i as usize) < (newvar_count as usize) {
                        let var_name = ds.get_var_name(i, epoch);
                        factory.atom(var_name)
                    } else {
                        return Err(format!(
                            "Variable reference {} out of range (only {} vars defined)",
                            i, newvar_count
                        ));
                    }
                }
                Tag::SymbolSize(size) => {
                    let end = offset + size as usize;
                    if end > bytes.len() {
                        return Err(format!(
                            "Symbol size {} exceeds available bytes at offset {}",
                            size, offset
                        ));
                    }
                    let symbol_bytes = &bytes[offset..end];
                    offset = end;

                    // Symbol table lookup (same logic as existing decoders)
                    let symbol_str: &str = {
                        #[cfg(feature = "interning")]
                        {
                            if symbol_bytes.len() == 8 {
                                let symbol_id = i64::from_be_bytes(
                                    symbol_bytes.try_into().expect("8 bytes expected"),
                                )
                                .to_be_bytes();
                                if let Some(actual_bytes) = space.sm.get_bytes(symbol_id) {
                                    // Found in symbol table - use actual symbol string
                                    // SAFETY: MORK stores valid UTF-8 symbols
                                    std::str::from_utf8(actual_bytes).unwrap_or("")
                                } else {
                                    std::str::from_utf8(symbol_bytes).unwrap_or("")
                                }
                            } else {
                                std::str::from_utf8(symbol_bytes).unwrap_or("")
                            }
                        }
                        #[cfg(not(feature = "interning"))]
                        {
                            std::str::from_utf8(symbol_bytes).unwrap_or("")
                        }
                    };

                    // Single-pass classify-AND-parse via the unified
                    // literal classifier. The DFA walks the bytes ONCE,
                    // accumulating the integer value digit-by-digit during
                    // the same scan that determines the kind. Float still
                    // requires `f64::from_str` (manual mantissa parsing is
                    // error-prone) but the shape is pre-validated. Bool /
                    // String / Atom need no further parsing.
                    //
                    // Overflow (`LongOverflow`) and unparseable shapes
                    // fall through to `Atom`, preserving pre-refactor
                    // semantics for huge digit strings.
                    //
                    // See `crate::backend::literal_classifier` for the
                    // full state-machine description and tests.
                    use crate::backend::literal_classifier::{
                        classify_and_parse, ClassifiedLiteral,
                    };
                    match classify_and_parse(symbol_str) {
                        ClassifiedLiteral::Long(n) => factory.long(n),
                        ClassifiedLiteral::Float(f) => factory.float(f),
                        ClassifiedLiteral::BoolTrue => factory.bool(true),
                        ClassifiedLiteral::BoolFalse => factory.bool(false),
                        ClassifiedLiteral::String(inner) => factory.string(inner),
                        ClassifiedLiteral::LongOverflow | ClassifiedLiteral::Atom => {
                            factory.atom(symbol_str)
                        }
                    }
                }
                Tag::Arity(arity) => {
                    if arity == 0 {
                        factory.unit()
                    } else {
                        stack.push(StackFrame::Arity {
                            remaining: arity,
                            items: SmallVec::with_capacity(arity as usize),
                        });
                        continue 'parsing;
                    }
                }
            };

            // Value complete - add to parent or return
            let mut current_value = Some(value);
            'popping: loop {
                let v = current_value
                    .take()
                    .expect("value must be Some at start of popping loop");

                if stack.is_empty() {
                    return Ok(v);
                }

                let should_pop = match stack.last_mut() {
                    None => unreachable!(),
                    Some(StackFrame::Arity { remaining, items }) => {
                        items.push(v);
                        *remaining -= 1;
                        *remaining == 0
                    }
                };

                if should_pop {
                    if let Some(StackFrame::Arity { items, .. }) = stack.pop() {
                        // into_vec() is still a win: SmallVec ≤4 items avoids the
                        // initial alloc entirely; into_vec() does one alloc at
                        // completion vs one alloc at start for the old Vec path.
                        current_value = Some(factory.sexpr(items.into_vec()));
                        continue 'popping;
                    }
                } else {
                    continue 'parsing;
                }
            }
        }
    })
}
