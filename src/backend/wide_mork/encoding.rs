//! Wide MORK byte encoding: storage (literal symbols) and De Bruijn (variables).
//!
//! Uses single-byte tags + LEB128 varints — no arity, symbol-size, or variable
//! count limits.
//!
//! ## Format
//!
//! | Tag Byte | Meaning      | Followed By                                    |
//! |----------|-------------|------------------------------------------------|
//! | `0x00`   | `Arity`     | LEB128 arity, then that many children          |
//! | `0x01`   | `NewVar`    | nothing                                        |
//! | `0x02`   | `VarRef`    | LEB128 De Bruijn index                         |
//! | `0x03`   | `SymbolSize`| LEB128 byte count, then symbol bytes           |

use std::collections::HashMap;

use crate::backend::models::{MettaValueInner, MettaValueTrait};

// ============================================================================
// Tag Constants
// ============================================================================

/// Wide MORK tag byte values.
pub const TAG_ARITY: u8 = 0x00;
pub const TAG_NEWVAR: u8 = 0x01;
pub const TAG_VARREF: u8 = 0x02;
pub const TAG_SYMBOL_SIZE: u8 = 0x03;

/// Wide MORK tag discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WideTag {
    Arity = TAG_ARITY,
    NewVar = TAG_NEWVAR,
    VarRef = TAG_VARREF,
    SymbolSize = TAG_SYMBOL_SIZE,
}

impl WideTag {
    /// Decode a WideTag from a byte.  Returns `Err(byte)` for unknown tags.
    #[inline]
    pub fn from_byte(b: u8) -> Result<WideTag, u8> {
        match b {
            TAG_ARITY => Ok(WideTag::Arity),
            TAG_NEWVAR => Ok(WideTag::NewVar),
            TAG_VARREF => Ok(WideTag::VarRef),
            TAG_SYMBOL_SIZE => Ok(WideTag::SymbolSize),
            other => Err(other),
        }
    }
}

// ============================================================================
// LEB128 Encoding / Decoding
// ============================================================================

/// Encode a `u64` as LEB128 into `buf`.
#[inline]
pub fn encode_leb128(buf: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

/// Decode a LEB128 `u64` from `bytes`.  Returns `(value, bytes_consumed)`.
///
/// Returns `None` on truncated input or overflow (>10 bytes).
#[inline]
pub fn decode_leb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in bytes.iter().enumerate() {
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None; // overflow
        }
    }
    None // truncated
}

// ============================================================================
// Wide Conversion Context (no 64-var limit)
// ============================================================================

/// Variable tracking for Wide MORK De Bruijn encoding.
///
/// Unlike MORK's `ConversionContext` which is limited to 64 variables (u8 index),
/// this uses `u64` indices and has no upper bound.
#[derive(Default)]
pub struct WideConversionContext {
    /// Variable name → De Bruijn index.
    pub var_map: HashMap<String, u64>,
    /// De Bruijn index → variable name (reverse map).
    pub var_names: Vec<String>,
}

impl WideConversionContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a De Bruijn index for a variable.
    ///
    /// Returns `Some(idx)` if the variable already exists (write VarRef),
    /// or `None` if it's new (write NewVar — the caller records the new index).
    pub fn get_or_create_var(&mut self, name: &str) -> Option<u64> {
        if let Some(&idx) = self.var_map.get(name) {
            Some(idx)
        } else {
            let idx = self.var_names.len() as u64;
            self.var_map.insert(name.to_string(), idx);
            self.var_names.push(name.to_string());
            None // new variable — caller writes NewVar
        }
    }
}

// ============================================================================
// Storage Encoding (no variables — symbols written as-is)
// ============================================================================

/// Encode a `MettaValueTrait` value to Wide MORK **storage** bytes.
///
/// Variables are written as literal symbol bytes (no De Bruijn), identical to
/// MORK's `with_mork_bytes` semantics.  This is used for PathMap storage keys
/// (the data side of `extract_data`).
pub fn encode_wide_storage<V: MettaValueTrait>(value: &V, buf: &mut Vec<u8>) {
    encode_wide_storage_inner(value, buf);
}

fn encode_wide_storage_inner<V: MettaValueTrait>(value: &V, buf: &mut Vec<u8>) {
    match value.inner_raw() {
        MettaValueInner::SExpr(_) => {
            let items = value.as_sexpr().expect("matched SExpr");
            buf.push(TAG_ARITY);
            encode_leb128(buf, items.len() as u64);
            for item in items {
                encode_wide_storage_inner(item, buf);
            }
        }

        MettaValueInner::Atom(s) => {
            write_wide_symbol(buf, s.as_bytes());
        }

        MettaValueInner::Bool(b) => {
            if *b {
                write_wide_symbol(buf, b"True");
            } else {
                write_wide_symbol(buf, b"False");
            }
        }

        MettaValueInner::Long(n) => {
            let mut ibuf = itoa::Buffer::new();
            let s = ibuf.format(*n);
            write_wide_symbol(buf, s.as_bytes());
        }

        MettaValueInner::Float(f) => {
            let mut rbuf = ryu::Buffer::new();
            let s = rbuf.format(*f);
            write_wide_symbol(buf, s.as_bytes());
        }

        MettaValueInner::String(s) => {
            // Quoted string: "hello"
            let mut scratch = Vec::with_capacity(s.len() + 2);
            scratch.push(b'"');
            scratch.extend_from_slice(s.as_bytes());
            scratch.push(b'"');
            write_wide_symbol(buf, &scratch);
        }

        MettaValueInner::Unit => {
            // Empty list → Arity(0)
            buf.push(TAG_ARITY);
            encode_leb128(buf, 0);
        }

        MettaValueInner::Error(_, _) => {
            let (offending, detail) = value.as_error().expect("matched Error");
            // (error offending detail) — HE-bisimilar 3-element form. Both
            // operands are arbitrary atoms, so encode each recursively.
            buf.push(TAG_ARITY);
            encode_leb128(buf, 3);
            write_wide_symbol(buf, b"error");
            encode_wide_storage_inner(offending, buf);
            encode_wide_storage_inner(detail, buf);
        }

        MettaValueInner::Type(_) => {
            let inner = value.as_type().expect("matched Type");
            encode_wide_storage_inner(inner, buf);
        }

        MettaValueInner::Quoted(_) => {
            let inner = value.as_quoted_ref().expect("matched Quoted");
            // (quote inner)
            buf.push(TAG_ARITY);
            encode_leb128(buf, 2);
            write_wide_symbol(buf, b"quote");
            encode_wide_storage_inner(inner, buf);
        }

        MettaValueInner::Conjunction(_) => {
            let goals = value.as_conjunction().expect("matched Conjunction");
            // Conjunction written as arity(goals+1) with comma as first child
            buf.push(TAG_ARITY);
            encode_leb128(buf, (goals.len() + 1) as u64);
            write_wide_symbol(buf, b",");
            for goal in goals {
                encode_wide_storage_inner(goal, buf);
            }
        }

        MettaValueInner::Space(handle) => {
            // (Space id name)
            buf.push(TAG_ARITY);
            encode_leb128(buf, 3);
            write_wide_symbol(buf, b"Space");
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(handle.id);
            write_wide_symbol(buf, id_str.as_bytes());
            let mut scratch = Vec::with_capacity(handle.name.len() + 2);
            scratch.push(b'"');
            scratch.extend_from_slice(handle.name.as_bytes());
            scratch.push(b'"');
            write_wide_symbol(buf, &scratch);
        }

        MettaValueInner::State(id) => {
            // (State id)
            buf.push(TAG_ARITY);
            encode_leb128(buf, 2);
            write_wide_symbol(buf, b"State");
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(*id);
            write_wide_symbol(buf, id_str.as_bytes());
        }

        MettaValueInner::Memo(handle) => {
            // (Memo id name) — runtime-only, but we encode it for completeness
            buf.push(TAG_ARITY);
            encode_leb128(buf, 3);
            write_wide_symbol(buf, b"Memo");
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(handle.id);
            write_wide_symbol(buf, id_str.as_bytes());
            write_wide_symbol(buf, handle.name.as_bytes());
        }

        MettaValueInner::Empty => {
            // Empty sentinel — encode as atom for storage completeness
            write_wide_symbol(buf, b"%Empty%");
        }

        MettaValueInner::NotReducible => {
            // Plan S0a (2026-05-13) — HE `NotReducible` sentinel encoded as
            // its interned atom name. MORK queries can match it like any
            // other symbol.
            write_wide_symbol(buf, b"NotReducible");
        }

        MettaValueInner::Spanned(..) => {
            let stripped = value.strip_one_span();
            encode_wide_storage_inner(&stripped, buf);
        }
    }
}

// ============================================================================
// De Bruijn Encoding (variables → NewVar / VarRef)
// ============================================================================

/// Encode a `MettaValueTrait` value to Wide MORK **De Bruijn** bytes.
///
/// Variables (`$x`, `&y`, `'z`) get De Bruijn NewVar/VarRef tags.
/// Wildcards (`_`) get anonymous NewVar tags.
///
/// Returns the encoded bytes.  The caller should also inspect `ctx.var_names`
/// for the variable name mapping.
pub fn encode_wide_debruijn<V: MettaValueTrait>(
    value: &V,
    ctx: &mut WideConversionContext,
    buf: &mut Vec<u8>,
) {
    encode_wide_debruijn_inner(value, ctx, buf);
}

fn encode_wide_debruijn_inner<V: MettaValueTrait>(
    value: &V,
    ctx: &mut WideConversionContext,
    buf: &mut Vec<u8>,
) {
    match value.inner_raw() {
        MettaValueInner::Atom(name) => {
            // Check if it's a variable ($x, &y, 'z) or wildcard (_)
            if is_variable_name(name) || name == &"_" {
                if name == &"_" {
                    // Wildcard → anonymous NewVar
                    buf.push(TAG_NEWVAR);
                } else if let Some(idx) = ctx.get_or_create_var(name) {
                    // Existing variable → VarRef
                    buf.push(TAG_VARREF);
                    encode_leb128(buf, idx);
                } else {
                    // New variable → NewVar
                    buf.push(TAG_NEWVAR);
                }
            } else {
                // Regular atom → symbol
                write_wide_symbol(buf, name.as_bytes());
            }
        }

        MettaValueInner::Bool(b) => {
            if *b {
                write_wide_symbol(buf, b"True");
            } else {
                write_wide_symbol(buf, b"False");
            }
        }

        MettaValueInner::Long(n) => {
            let mut ibuf = itoa::Buffer::new();
            let s = ibuf.format(*n);
            write_wide_symbol(buf, s.as_bytes());
        }

        MettaValueInner::Float(f) => {
            let mut rbuf = ryu::Buffer::new();
            let s = rbuf.format(*f);
            write_wide_symbol(buf, s.as_bytes());
        }

        MettaValueInner::String(s) => {
            let mut scratch = Vec::with_capacity(s.len() + 2);
            scratch.push(b'"');
            scratch.extend_from_slice(s.as_bytes());
            scratch.push(b'"');
            write_wide_symbol(buf, &scratch);
        }

        MettaValueInner::Unit => {
            buf.push(TAG_ARITY);
            encode_leb128(buf, 0);
        }

        MettaValueInner::SExpr(_) => {
            let items = value.as_sexpr().expect("matched SExpr");
            buf.push(TAG_ARITY);
            encode_leb128(buf, items.len() as u64);
            for item in items {
                encode_wide_debruijn_inner(item, ctx, buf);
            }
        }

        MettaValueInner::Error(_, _) => {
            let (offending, detail) = value.as_error().expect("matched Error");
            buf.push(TAG_ARITY);
            encode_leb128(buf, 3);
            write_wide_symbol(buf, b"error");
            encode_wide_debruijn_inner(offending, ctx, buf);
            encode_wide_debruijn_inner(detail, ctx, buf);
        }

        MettaValueInner::Type(_) => {
            let inner = value.as_type().expect("matched Type");
            encode_wide_debruijn_inner(inner, ctx, buf);
        }

        MettaValueInner::Quoted(_) => {
            let inner = value.as_quoted_ref().expect("matched Quoted");
            buf.push(TAG_ARITY);
            encode_leb128(buf, 2);
            write_wide_symbol(buf, b"quote");
            encode_wide_debruijn_inner(inner, ctx, buf);
        }

        MettaValueInner::Conjunction(_) => {
            let goals = value.as_conjunction().expect("matched Conjunction");
            buf.push(TAG_ARITY);
            encode_leb128(buf, (goals.len() + 1) as u64);
            write_wide_symbol(buf, b",");
            for goal in goals {
                encode_wide_debruijn_inner(goal, ctx, buf);
            }
        }

        MettaValueInner::Space(handle) => {
            buf.push(TAG_ARITY);
            encode_leb128(buf, 3);
            write_wide_symbol(buf, b"Space");
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(handle.id);
            write_wide_symbol(buf, id_str.as_bytes());
            let mut scratch = Vec::with_capacity(handle.name.len() + 2);
            scratch.push(b'"');
            scratch.extend_from_slice(handle.name.as_bytes());
            scratch.push(b'"');
            write_wide_symbol(buf, &scratch);
        }

        MettaValueInner::State(id) => {
            buf.push(TAG_ARITY);
            encode_leb128(buf, 2);
            write_wide_symbol(buf, b"State");
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(*id);
            write_wide_symbol(buf, id_str.as_bytes());
        }

        MettaValueInner::Memo(handle) => {
            buf.push(TAG_ARITY);
            encode_leb128(buf, 3);
            write_wide_symbol(buf, b"Memo");
            let mut ibuf = itoa::Buffer::new();
            let id_str = ibuf.format(handle.id);
            write_wide_symbol(buf, id_str.as_bytes());
            write_wide_symbol(buf, handle.name.as_bytes());
        }

        MettaValueInner::Empty => {
            write_wide_symbol(buf, b"%Empty%");
        }

        MettaValueInner::NotReducible => {
            // Plan S0a (2026-05-13) — HE `NotReducible` sentinel symbol.
            write_wide_symbol(buf, b"NotReducible");
        }

        MettaValueInner::Spanned(..) => {
            let stripped = value.strip_one_span();
            encode_wide_debruijn_inner(&stripped, ctx, buf);
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Write a symbol in wide MORK format: `[TAG_SYMBOL_SIZE][LEB128 len][bytes]`.
#[inline]
fn write_wide_symbol(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.push(TAG_SYMBOL_SIZE);
    encode_leb128(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

/// Check if a name represents a variable (`$x`, `&y`, `'z`).
#[inline]
fn is_variable_name(name: &str) -> bool {
    let first = name.as_bytes().first().copied().unwrap_or(0);
    first == b'$' || first == b'&' || first == b'\''
}

/// Compute the byte length of a single wide MORK element starting at `bytes[0]`.
///
/// This is the wide analog of MORK's `mork_expr_byte_len()`.  Uses a depth
/// counter to measure compound expressions without recursion.
pub fn wide_expr_byte_len(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut offset = 0usize;
    // depth starts at 1 (the root element counts as one pending item)
    let mut depth: u64 = 1;

    while depth > 0 && offset < bytes.len() {
        match WideTag::from_byte(bytes[offset]) {
            Ok(WideTag::Arity) => {
                offset += 1;
                if let Some((arity, consumed)) = decode_leb128(&bytes[offset..]) {
                    offset += consumed;
                    // Replace this one pending item with `arity` children
                    depth = depth - 1 + arity;
                } else {
                    break; // truncated
                }
            }
            Ok(WideTag::NewVar) => {
                offset += 1;
                depth -= 1;
            }
            Ok(WideTag::VarRef) => {
                offset += 1;
                if let Some((_idx, consumed)) = decode_leb128(&bytes[offset..]) {
                    offset += consumed;
                } else {
                    break;
                }
                depth -= 1;
            }
            Ok(WideTag::SymbolSize) => {
                offset += 1;
                if let Some((size, consumed)) = decode_leb128(&bytes[offset..]) {
                    offset += consumed + size as usize;
                } else {
                    break;
                }
                depth -= 1;
            }
            Err(_) => break, // unknown tag
        }
    }
    offset
}

/// Count the number of `NewVar` tags in wide MORK bytes.
pub fn count_wide_newvar_tags(bytes: &[u8]) -> usize {
    let mut count = 0;
    let mut offset = 0;
    let end = bytes.len();
    while offset < end {
        match WideTag::from_byte(bytes[offset]) {
            Ok(WideTag::Arity) => {
                offset += 1;
                if let Some((_arity, consumed)) = decode_leb128(&bytes[offset..]) {
                    offset += consumed;
                } else {
                    break;
                }
            }
            Ok(WideTag::NewVar) => {
                offset += 1;
                count += 1;
            }
            Ok(WideTag::VarRef) => {
                offset += 1;
                if let Some((_idx, consumed)) = decode_leb128(&bytes[offset..]) {
                    offset += consumed;
                } else {
                    break;
                }
            }
            Ok(WideTag::SymbolSize) => {
                offset += 1;
                if let Some((size, consumed)) = decode_leb128(&bytes[offset..]) {
                    offset += consumed + size as usize;
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    count
}
