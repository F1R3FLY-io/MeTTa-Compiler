//! Wide MORK decoding: reconstruct typed `MettaValue` from Wide MORK storage bytes.
//!
//! This is the inverse of `encode_wide_storage()`.  Given a byte slice produced by
//! `encode_wide_storage`, it reconstructs the original typed value using the same
//! heuristic type detection as `mork_bytes_to_generic_value()`:
//!
//! - Symbols starting with a digit or `-` followed by a digit → `Long(i64)`
//! - `"True"` / `"False"` → `Bool`
//! - Quoted `"..."` → `String` (quotes stripped)
//! - Everything else → `Atom`
//!
//! ## Differences from `mork_bytes_to_generic_value()`
//!
//! | Aspect          | MORK decode                              | Wide MORK decode                          |
//! |-----------------|------------------------------------------|------------------------------------------|
//! | Tags            | `maybe_byte_item()` 2-bit+6-bit packed   | `WideTag::from_byte()` + LEB128 payload  |
//! | Symbols         | Symbol table lookup (interning)           | Raw UTF-8 (no interning)                 |
//! | Arity           | `u8` (max 63)                            | `u64` via LEB128                         |
//! | Variables       | `NewVar`/`VarRef` De Bruijn (max 64)     | Same, but unlimited var count via Vec     |
//! | Type heuristics | Identical                                | Identical                                |

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use smallvec::SmallVec;

use super::encoding::{decode_leb128, WideTag};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

// ============================================================================
// Epoch counter for unique variable names (shared with MORK decoder)
// ============================================================================

/// Global epoch counter for unique variable name generation.
/// Each wide-decode invocation increments this, ensuring variables from
/// different invocations never collide.
static WIDE_VARNAME_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Base variable names (same as MORK decoder).  For indices > 64 we generate
/// programmatically: `$v64`, `$v65`, etc.
static VARNAME_BASES: [&str; 64] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
    "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x",
    "y", "z", "a1", "b1", "c1", "d1", "e1", "f1", "g1", "h1",
    "i1", "j1", "k1", "l1", "m1", "n1", "o1", "p1", "q1", "r1",
    "s1", "t1", "u1", "v1", "w1", "x1", "y1", "z1", "a2", "b2",
    "c2", "d2", "e2", "f2", "g2", "h2", "i2", "j2", "k2", "l2",
];

// ============================================================================
// Thread-local variable name cache (unbounded)
// ============================================================================

thread_local! {
    static WIDE_DESER_STATE: RefCell<WideDeserState> = RefCell::new(WideDeserState::new());
}

/// Thread-local deserialization state for variable name caching (unbounded).
///
/// Unlike the MORK decoder's fixed-size `[String; 64]`, this uses a `Vec<String>`
/// that grows on demand.  String heap capacity is reused across calls (zero alloc
/// after warmup for repeated invocations with the same max variable count).
struct WideDeserState {
    /// The epoch for which cached names are valid.
    cached_epoch: u64,
    /// How many names have been built for the current epoch.
    names_built: u64,
    /// Growable variable name cache.
    cached_var_names: Vec<String>,
}

impl WideDeserState {
    fn new() -> Self {
        Self {
            cached_epoch: u64::MAX, // sentinel — forces rebuild on first call
            names_built: 0,
            cached_var_names: Vec::with_capacity(16),
        }
    }

    /// Get the variable name for the given De Bruijn index and epoch.
    ///
    /// Lazily builds names up to the requested index.  For indices < 64, uses
    /// the same base names as the MORK decoder (`$a%E`, `$b%E`, ...).  For
    /// indices ≥ 64, uses `$v{N}%E`.
    #[inline]
    fn get_var_name(&mut self, index: u64, epoch: u64) -> &str {
        if self.cached_epoch != epoch {
            self.cached_epoch = epoch;
            self.names_built = 0;
        }

        // Ensure the Vec is long enough
        while self.cached_var_names.len() <= index as usize {
            self.cached_var_names.push(String::new());
        }

        // Build names up to and including index if not yet built
        while self.names_built <= index {
            let i = self.names_built as usize;
            let name = &mut self.cached_var_names[i];
            name.clear(); // retains heap capacity
            name.push('$');
            if (self.names_built as usize) < VARNAME_BASES.len() {
                name.push_str(VARNAME_BASES[i]);
            } else {
                // Fallback for index ≥ 64: $v{N}
                name.push('v');
                let mut ibuf = itoa::Buffer::new();
                name.push_str(ibuf.format(self.names_built));
            }
            name.push('%');
            let mut ibuf = itoa::Buffer::new();
            name.push_str(ibuf.format(epoch));
            self.names_built += 1;
        }
        &self.cached_var_names[index as usize]
    }
}

// ============================================================================
// Public Decode Function
// ============================================================================

/// Reconstruct a typed `V` value from Wide MORK **storage** bytes.
///
/// Storage bytes contain no De Bruijn variables — all symbols are literal.
/// This is the inverse of `encode_wide_storage()`.
///
/// Uses the same heuristic type detection as `mork_bytes_to_generic_value()`:
/// - Numeric strings → `Long(i64)`
/// - `"True"` / `"False"` → `Bool`
/// - Quoted `"..."` → `String`
/// - Everything else → `Atom`
pub fn wide_bytes_to_generic_value<V, F>(bytes: &[u8], factory: &F) -> Result<V, String>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    wide_bytes_to_generic_value_inner(bytes, factory, false)
}

/// Reconstruct a typed `V` value from Wide MORK **De Bruijn** bytes.
///
/// De Bruijn bytes may contain `NewVar` / `VarRef` tags.  These are converted
/// to epoch-suffixed variable atoms (`$a%42`, `$b%42`, ...) for variable
/// isolation — same semantics as `mork_bytes_to_generic_value()`.
pub fn wide_debruijn_to_generic_value<V, F>(bytes: &[u8], factory: &F) -> Result<V, String>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    wide_bytes_to_generic_value_inner(bytes, factory, true)
}

/// Internal implementation — `has_variables` selects storage vs De Bruijn mode.
fn wide_bytes_to_generic_value_inner<V, F>(
    bytes: &[u8],
    factory: &F,
    has_variables: bool,
) -> Result<V, String>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    enum StackFrame<V> {
        Arity {
            remaining: u64,
            items: SmallVec<[V; 4]>,
        },
    }

    let mut stack: Vec<StackFrame<V>> = Vec::new();
    let mut offset = 0usize;
    let mut newvar_count: u64 = 0;
    let epoch = if has_variables {
        WIDE_VARNAME_EPOCH.fetch_add(1, Ordering::Relaxed)
    } else {
        0
    };

    WIDE_DESER_STATE.with(|state| {
        let mut ds = state.borrow_mut();

        'parsing: loop {
            if offset >= bytes.len() {
                if stack.is_empty() && offset > 0 {
                    return Err("Unexpected end of Wide MORK bytes (all consumed but no value produced)".to_string());
                }
                return Err("Unexpected end of Wide MORK bytes".to_string());
            }

            let tag_byte = bytes[offset];
            let tag = WideTag::from_byte(tag_byte).map_err(|b| {
                format!("Unknown Wide MORK tag byte 0x{:02x} at offset {}", b, offset)
            })?;
            offset += 1;

            let value = match tag {
                WideTag::NewVar => {
                    if !has_variables {
                        // Storage bytes should not contain variables, but we handle
                        // them gracefully by creating anonymous atoms
                        return Err(format!(
                            "NewVar tag in storage bytes at offset {}",
                            offset - 1
                        ));
                    }
                    let var_name = ds.get_var_name(newvar_count, epoch);
                    let atom = factory.atom(var_name);
                    newvar_count += 1;
                    atom
                }

                WideTag::VarRef => {
                    let (idx, consumed) = decode_leb128(&bytes[offset..]).ok_or_else(|| {
                        format!("Truncated LEB128 for VarRef at offset {}", offset)
                    })?;
                    offset += consumed;

                    if !has_variables {
                        return Err(format!(
                            "VarRef tag in storage bytes at offset {}",
                            offset - 1 - consumed
                        ));
                    }

                    if idx >= newvar_count {
                        return Err(format!(
                            "Variable reference {} out of range (only {} vars defined)",
                            idx, newvar_count
                        ));
                    }

                    let var_name = ds.get_var_name(idx, epoch);
                    factory.atom(var_name)
                }

                WideTag::SymbolSize => {
                    let (size, consumed) = decode_leb128(&bytes[offset..]).ok_or_else(|| {
                        format!("Truncated LEB128 for SymbolSize at offset {}", offset)
                    })?;
                    offset += consumed;

                    let end = offset + size as usize;
                    if end > bytes.len() {
                        return Err(format!(
                            "Symbol size {} exceeds available bytes at offset {}",
                            size, offset
                        ));
                    }

                    let symbol_bytes = &bytes[offset..end];
                    offset = end;

                    // Raw UTF-8 (no symbol table lookup — Wide MORK doesn't use interning)
                    let symbol_str = std::str::from_utf8(symbol_bytes).map_err(|e| {
                        format!("Invalid UTF-8 in symbol at offset {}: {}", offset - size as usize, e)
                    })?;

                    // Heuristic type detection (same as mork_bytes_to_generic_value)
                    heuristic_parse_symbol(symbol_str, factory)
                }

                WideTag::Arity => {
                    let (arity, consumed) = decode_leb128(&bytes[offset..]).ok_or_else(|| {
                        format!("Truncated LEB128 for Arity at offset {}", offset)
                    })?;
                    offset += consumed;

                    if arity == 0 {
                        factory.unit()
                    } else {
                        let capacity = (arity as usize).min(1024); // cap prealloc
                        stack.push(StackFrame::Arity {
                            remaining: arity,
                            items: SmallVec::with_capacity(capacity),
                        });
                        continue 'parsing;
                    }
                }
            };

            // Value complete — push to parent or return
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

// ============================================================================
// Heuristic Type Detection
// ============================================================================

/// Parse a symbol string into a typed value using the same heuristics as
/// `mork_bytes_to_generic_value()`.
///
/// - Digits / negative digits → `Long(i64)`
/// - `"True"` / `"False"` → `Bool` (note: Wide MORK uses capitalized forms)
/// - Quoted `"..."` → `String` (quotes stripped)
/// - `"%Empty%"` → `factory.empty()`
/// - Everything else → `Atom`
#[inline]
fn heuristic_parse_symbol<V, F>(symbol_str: &str, factory: &F) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    let first_byte = symbol_str.as_bytes().first().copied().unwrap_or(0);
    let could_be_number = first_byte.is_ascii_digit()
        || (first_byte == b'-'
            && symbol_str.len() > 1
            && symbol_str
                .as_bytes()
                .get(1)
                .is_some_and(|b| b.is_ascii_digit()));

    if could_be_number {
        // Try integer first
        if let Ok(n) = symbol_str.parse::<i64>() {
            return factory.long(n);
        }
        // Try float
        if let Ok(f) = symbol_str.parse::<f64>() {
            return factory.float(f);
        }
        factory.atom(symbol_str)
    } else if symbol_str == "True" {
        factory.bool(true)
    } else if symbol_str == "False" {
        factory.bool(false)
    } else if symbol_str.starts_with('"')
        && symbol_str.ends_with('"')
        && symbol_str.len() >= 2
    {
        factory.string(&symbol_str[1..symbol_str.len() - 1])
    } else if symbol_str == "%Empty%" {
        factory.empty()
    } else {
        factory.atom(symbol_str)
    }
}
