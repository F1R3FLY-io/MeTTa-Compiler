//! MORK encoding and conversion operations for Environment.
//!
//! Provides methods for converting between MORK expressions and MettaValues.
//! Handles the low-level byte encoding used by PathMap trie storage.

use mork_expr::{maybe_byte_item, Expr, Tag};
use mork::space::Space;
use std::slice::from_raw_parts;
use tracing::{trace, warn};

use super::MettaValue;

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
    pub(crate) fn mork_expr_to_metta_value(
        expr: &Expr,
        space: &Space,
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
    pub(crate) fn serialize_mork_expr_old(expr: &Expr, space: &Space) -> String {
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
            |i, _intro| Expr::VARNAMES[i as usize],
        );

        String::from_utf8_lossy(&buffer).to_string()
    }
}
