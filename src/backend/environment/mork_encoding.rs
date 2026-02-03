//! MORK encoding and conversion operations for Environment.
//!
//! Provides methods for converting between MORK expressions and MettaValues.
//! Handles the low-level byte encoding used by PathMap trie storage.
//!
//! ## Optimization: Arena-based Conversion
//!
//! The `mork_expr_to_arena_value` function converts MORK expressions directly to
//! arena-allocated values, avoiding individual heap allocations. This is 2-3x
//! faster than converting to heap-allocated MettaValue for transient values.
//!
//! ## Static Variable Names
//!
//! Variable names ("$a", "$b", etc.) use static string references instead of
//! allocating new strings for each variable encountered.

use bumpalo::collections::Vec as BumpVec;
use bumpalo::Bump;
use mork::space::Space;
use mork_expr::{maybe_byte_item, Expr, Tag};
use std::slice::from_raw_parts;
use tracing::{trace, warn};

use super::MettaValue;
use crate::backend::models::{ArenaValue, MettaValueFactory, MettaValueTrait};

/// Static variable names for MORK variables.
/// Using static strings eliminates allocation for the common case of <64 variables.
pub(crate) static VARNAMES: [&str; 64] = [
    "$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "$k", "$l",
    "$m", "$n", "$o", "$p", "$q", "$r", "$s", "$t", "$u", "$v", "$w", "$x",
    "$y", "$z", "$a1", "$b1", "$c1", "$d1", "$e1", "$f1", "$g1", "$h1",
    "$i1", "$j1", "$k1", "$l1", "$m1", "$n1", "$o1", "$p1", "$q1", "$r1",
    "$s1", "$t1", "$u1", "$v1", "$w1", "$x1", "$y1", "$z1", "$a2", "$b2",
    "$c2", "$d2", "$e2", "$f2", "$g2", "$h2", "$i2", "$j2", "$k2", "$l2",
];

impl super::Environment {
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
                    // Use static VARNAMES to avoid allocation
                    let var_name = if (newvar_count as usize) < VARNAMES.len() {
                        VARNAMES[newvar_count as usize].to_string()
                    } else {
                        format!("$var{}", newvar_count)
                    };
                    newvar_count += 1;
                    MettaValue::Atom(var_name)
                }
                Tag::VarRef(i) => {
                    // Variable reference - use static VARNAMES to avoid allocation
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
                                let symbol_id = i64::from_be_bytes(
                                    symbol_bytes.try_into().expect("8 bytes expected"),
                                )
                                .to_be_bytes();
                                if let Some(actual_bytes) = space.sm.get_bytes(symbol_id) {
                                    // Found in symbol table - use actual symbol string
                                    String::from_utf8_lossy(actual_bytes).into_owned()
                                } else {
                                    // Symbol ID not in table - fall back to treating as raw bytes
                                    trace!(
                                        target: "mettatron::environment::mork_expr_to_metta_value",
                                        symbol_id = ?symbol_id,
                                        "Symbol ID not found in symbol table, using raw bytes"
                                    );
                                    String::from_utf8_lossy(symbol_bytes).into_owned()
                                }
                            } else {
                                // Not 8 bytes - treat as raw symbol string
                                String::from_utf8_lossy(symbol_bytes).into_owned()
                            }
                        }
                        #[cfg(not(feature = "interning"))]
                        {
                            // Without interning, symbols are stored as raw UTF-8 bytes
                            String::from_utf8_lossy(symbol_bytes).into_owned()
                        }
                    };

                    // Parse the symbol to check if it's a number or string literal
                    // OPTIMIZATION: Fast-path check - only try parsing as integer if first byte
                    // could plausibly start a number (digit or minus sign followed by digit)
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
                        MettaValue::Nil()
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
                let v = current_value
                    .take()
                    .expect("value must be Some at start of popping loop");

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

    /// Convert a MORK Expr directly to ArenaValue (arena-allocated).
    ///
    /// This is an optimized version that allocates all values from the provided
    /// arena, avoiding individual heap allocations. Use this for transient values
    /// that don't need to outlive the arena.
    ///
    /// ## Performance Benefits
    ///
    /// - No individual Arc wrapping for each value
    /// - No reference counting overhead
    /// - O(1) bulk deallocation when arena drops
    /// - Better cache locality
    #[allow(unused_variables)]
    pub(crate) fn mork_expr_to_arena_value<'a, V: Clone + Default + Send + Sync + Unpin>(
        arena: &'a Bump,
        expr: &Expr,
        space: &Space<V>,
    ) -> Result<ArenaValue<'a>, String> {
        // Stack-based traversal to avoid recursion limits
        #[derive(Debug)]
        enum StackFrame<'a> {
            Arity {
                remaining: u8,
                items: BumpVec<'a, ArenaValue<'a>>,
            },
        }

        let mut stack: Vec<StackFrame<'a>> = Vec::new();
        let mut offset = 0usize;
        let ptr = expr.ptr;
        let mut newvar_count = 0u8;

        'parsing: loop {
            let byte = unsafe { *ptr.byte_add(offset) };
            let tag = match maybe_byte_item(byte) {
                Ok(t) => t,
                Err(reserved_byte) => {
                    warn!(
                        target: "mettatron::environment::mork_expr_to_arena_value",
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
                    // Use static VARNAMES - allocate in arena only if needed
                    if (newvar_count as usize) < VARNAMES.len() {
                        let var_name = VARNAMES[newvar_count as usize];
                        newvar_count += 1;
                        ArenaValue::atom(arena, var_name)
                    } else {
                        let var_name = arena.alloc_str(&format!("$var{}", newvar_count));
                        newvar_count += 1;
                        ArenaValue::atom(arena, var_name)
                    }
                }
                Tag::VarRef(i) => {
                    if (i as usize) < VARNAMES.len() {
                        ArenaValue::atom(arena, VARNAMES[i as usize])
                    } else {
                        let var_name = arena.alloc_str(&format!("$var{}", i));
                        ArenaValue::atom(arena, var_name)
                    }
                }
                Tag::SymbolSize(size) => {
                    let symbol_bytes =
                        unsafe { from_raw_parts(ptr.byte_add(offset), size as usize) };
                    offset += size as usize;

                    // Look up symbol in symbol table if interning is enabled
                    let symbol_str: &str = {
                        #[cfg(feature = "interning")]
                        {
                            if symbol_bytes.len() == 8 {
                                let symbol_id = i64::from_be_bytes(
                                    symbol_bytes.try_into().expect("8 bytes expected"),
                                )
                                .to_be_bytes();
                                if let Some(actual_bytes) = space.sm.get_bytes(symbol_id) {
                                    let s = std::str::from_utf8(actual_bytes).unwrap_or("");
                                    arena.alloc_str(s)
                                } else {
                                    let s = std::str::from_utf8(symbol_bytes).unwrap_or("");
                                    arena.alloc_str(s)
                                }
                            } else {
                                let s = std::str::from_utf8(symbol_bytes).unwrap_or("");
                                arena.alloc_str(s)
                            }
                        }
                        #[cfg(not(feature = "interning"))]
                        {
                            let s = std::str::from_utf8(symbol_bytes).unwrap_or("");
                            arena.alloc_str(s)
                        }
                    };

                    // Parse the symbol to check if it's a number or string literal
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
                            ArenaValue::long(arena, n)
                        } else {
                            ArenaValue::atom(arena, symbol_str)
                        }
                    } else if symbol_str == "true" {
                        ArenaValue::bool(arena, true)
                    } else if symbol_str == "false" {
                        ArenaValue::bool(arena, false)
                    } else if symbol_str.starts_with('"')
                        && symbol_str.ends_with('"')
                        && symbol_str.len() >= 2
                    {
                        // String literal - strip quotes and allocate in arena
                        let content = &symbol_str[1..symbol_str.len() - 1];
                        ArenaValue::string(arena, content)
                    } else {
                        ArenaValue::atom(arena, symbol_str)
                    }
                }
                Tag::Arity(arity) => {
                    if arity == 0 {
                        ArenaValue::nil(arena)
                    } else {
                        // Push new frame for this s-expression
                        stack.push(StackFrame::Arity {
                            remaining: arity,
                            items: BumpVec::with_capacity_in(arity as usize, arena),
                        });
                        continue 'parsing;
                    }
                }
            };

            // Value is complete - add to parent or return
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
                        current_value = Some(ArenaValue::sexpr(arena, items));
                        continue 'popping;
                    }
                } else {
                    continue 'parsing;
                }
            }
        }
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
/// - `HeapMettaValueFactory` constructs `MettaValue` directly
/// - `ArenaValueFactory` constructs `ArenaValue` directly
/// - No intermediate type conversion needed
///
/// ## Performance
///
/// Stack-based traversal to avoid recursion limits on deeply nested expressions.
/// Uses static `VARNAMES` array to avoid allocation for the common case.
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

/// Convert MORK bytes directly to a generic value V without Expr wrapper.
///
/// This is the zero-wrapper version that operates directly on byte slices.
/// Use this when you have the path bytes from a PathMap zipper.
///
/// ## Performance
///
/// Stack-based traversal to avoid recursion limits on deeply nested expressions.
/// Uses static `VARNAMES` array to avoid allocation for the common case.
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
                // Use static VARNAMES to avoid allocation
                let var_name = if (newvar_count as usize) < VARNAMES.len() {
                    VARNAMES[newvar_count as usize]
                } else {
                    // Fallback for large variable counts
                    return Err(format!("Too many variables: {}", newvar_count));
                };
                newvar_count += 1;
                factory.atom(var_name)
            }
            Tag::VarRef(i) => {
                if (i as usize) < VARNAMES.len() {
                    factory.atom(VARNAMES[i as usize])
                } else {
                    return Err(format!("Variable reference out of range: {}", i));
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
                    factory.nil()
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
}
