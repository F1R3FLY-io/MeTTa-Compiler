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

use mork::space::Space;
use mork_expr::{maybe_byte_item, Expr, Tag};
use tracing::warn;

use super::MettaValue;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

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
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
    "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x",
    "y", "z", "a1", "b1", "c1", "d1", "e1", "f1", "g1", "h1",
    "i1", "j1", "k1", "l1", "m1", "n1", "o1", "p1", "q1", "r1",
    "s1", "t1", "u1", "v1", "w1", "x1", "y1", "z1", "a2", "b2",
    "c2", "d2", "e2", "f2", "g2", "h2", "i2", "j2", "k2", "l2",
];

/// Global epoch counter for unique variable name generation.
/// Each MORK-to-value conversion increments this to get a unique epoch,
/// ensuring that variables from different rule applications never collide.
static VARNAME_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl super::MettaEnvironment {
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
    pub(crate) unsafe fn mork_head_info(ptr: *const u8) -> Option<(&'static [u8], u8)> {
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
    pub(crate) fn mork_expr_to_metta_value<V: Clone + Default + Send + Sync + Unpin>(
        expr: &Expr,
        space: &Space<V>,
    ) -> Result<MettaValue, String> {
        // Delegate to the efficient factory-based generic implementation.
        // Since MettaValue = MettaValue, the factory allocates directly
        // into the global slab allocator without intermediate heap Strings.
        use crate::backend::models::global_factory;
        let factory = global_factory();
        super::mork_encoding::mork_expr_to_generic_value(expr, space, &factory)
    }

    /// Helper function to serialize a MORK Expr to a readable string
    /// DEPRECATED: This uses serialize2() which panics on reserved bytes.
    /// Use mork_expr_to_metta_value() instead for production code.
    #[deprecated(
        note = "This uses serialize2() which panics on reserved bytes. Use mork_expr_to_metta_value() instead."
    )]
    #[allow(dead_code)]
    #[allow(unused_variables)]
    pub(crate) fn serialize_mork_expr_old(expr: &Expr, space: &Space) -> String {
        let mut buffer = Vec::new();
        expr.serialize2(
            &mut buffer,
            |s| {
                #[cfg(feature = "interning")]
                {
                    let symbol =
                        i64::from_be_bytes(s.try_into().expect("8 bytes expected")).to_be_bytes();
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
            |i, _intro| Expr::VARNAMES[i as usize],
        );

        String::from_utf8_lossy(&buffer).to_string()
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
    // Stack-based traversal to avoid recursion limits
    enum StackFrame<V> {
        Arity { remaining: u8, items: Vec<V> },
    }

    let mut stack: Vec<StackFrame<V>> = Vec::new();
    let mut offset = 0usize;
    let mut newvar_count = 0u8;
    let epoch = VARNAME_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

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
                            i,
                            newvar_count
                        ));
                    }
                }
                Tag::SymbolSize(size) => {
                    let end = offset + size as usize;
                    if end > bytes.len() {
                        return Err(format!("Symbol size {} exceeds available bytes at offset {}", size, offset));
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

                    // Parse as number, bool, or string
                    let first_byte = symbol_str.as_bytes().first().copied().unwrap_or(0);
                    let could_be_number = first_byte.is_ascii_digit()
                        || (first_byte == b'-'
                            && symbol_str.len() > 1
                            && symbol_str
                                .as_bytes()
                                .get(1)
                                .is_some_and(|b| b.is_ascii_digit()));

                    if could_be_number {
                        if let Ok(n) = symbol_str.parse::<i64>() {
                            factory.long(n)
                        } else {
                            factory.atom(symbol_str)
                        }
                    } else if symbol_str == "true" {
                        factory.bool(true)
                    } else if symbol_str == "false" {
                        factory.bool(false)
                    } else if symbol_str.starts_with('"')
                        && symbol_str.ends_with('"')
                        && symbol_str.len() >= 2
                    {
                        factory.string(&symbol_str[1..symbol_str.len() - 1])
                    } else {
                        factory.atom(symbol_str)
                    }
                }
                Tag::Arity(arity) => {
                    if arity == 0 {
                        factory.unit()
                    } else {
                        stack.push(StackFrame::Arity {
                            remaining: arity,
                            items: Vec::with_capacity(arity as usize),
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
                        current_value = Some(factory.sexpr(items));
                        continue 'popping;
                    }
                } else {
                    continue 'parsing;
                }
            }
        }
    })
}
