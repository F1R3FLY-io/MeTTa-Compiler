//! Wide MORK pattern matching: `wide_extract_data()`.
//!
//! Behaviorally identical to MORK's `extract_data()` but operates on Wide MORK
//! bytes (tag-byte + LEB128 encoding).
//!
//! ## Algorithm
//!
//! Template (pattern, De Bruijn encoded) and data (storage encoded) are walked
//! in lockstep.  The template drives navigation:
//!
//! | Template Tag     | Data Tag     | Action                                    |
//! |-----------------|-------------|-------------------------------------------|
//! | `NewVar`        | `SymbolSize`| Capture symbol sub-expression as binding  |
//! | `NewVar`        | `Arity`     | Capture entire sub-expression as binding  |
//! | `VarRef(i)`     | any         | Check binding[i] matches this data element|
//! | `SymbolSize(a)` | `SymbolSize`| Check symbol bytes match exactly          |
//! | `Arity(a)`      | `Arity(b)`  | Check arities match, descend into children|
//! | anything        | `NewVar`/`VarRef` | Error (data must not contain variables)|

use super::encoding::*;

/// A captured binding from `wide_extract_data`.
///
/// Records the byte range in the **data** buffer that was captured by a `NewVar`
/// in the template.
#[derive(Debug, Clone)]
pub struct WideBinding {
    /// Start offset in data bytes.
    pub offset: usize,
    /// Byte length of the captured sub-expression.
    pub len: usize,
}

/// Error from `wide_extract_data` when matching fails.
#[derive(Debug)]
pub enum WideExtractFailure {
    /// Structural mismatch (different tags, different arities, different symbols).
    Mismatch,
    /// A VarRef back-reference didn't match the previously captured binding.
    VarRefMismatch { var_idx: u64 },
    /// Data contains variable tags (NewVar/VarRef) which is invalid.
    DataContainsVariables,
    /// Truncated or malformed input bytes.
    MalformedInput,
}

/// Pattern matching: template (De Bruijn pattern) vs data (storage expression).
///
/// Returns captured bindings (one per `NewVar` in template order) on success,
/// or a failure reason on mismatch.
///
/// Behaviorally identical to MORK's `extract_data()`.
pub fn wide_extract_data(
    template: &[u8],
    data: &[u8],
) -> Result<Vec<WideBinding>, WideExtractFailure> {
    let mut t_off = 0usize;   // template offset
    let mut d_off = 0usize;   // data offset
    let mut bindings: Vec<WideBinding> = Vec::new();

    // Stack for tracking compound expression descent.
    // Each entry is (remaining_children_template, remaining_children_data).
    let mut stack: Vec<(u64, u64)> = Vec::new();

    // We process one element at a time.  For compound expressions (Arity),
    // we push the arity onto the stack and descend into children.
    loop {
        // Check if we need to ascend from completed compound expressions
        while let Some(top) = stack.last_mut() {
            if top.0 == 0 && top.1 == 0 {
                stack.pop();
            } else {
                break;
            }
        }

        // Are we done?
        if t_off >= template.len() && d_off >= data.len() {
            // Both exhausted — success only if stack is empty
            if stack.is_empty() {
                return Ok(bindings);
            }
            return Err(WideExtractFailure::Mismatch);
        }

        if t_off >= template.len() || d_off >= data.len() {
            return Err(WideExtractFailure::Mismatch);
        }

        let t_tag = WideTag::from_byte(template[t_off])
            .map_err(|_| WideExtractFailure::MalformedInput)?;

        match t_tag {
            WideTag::NewVar => {
                // Template has a fresh variable — capture whatever data element is here
                t_off += 1;

                let d_start = d_off;
                let d_elem_len = wide_expr_byte_len(&data[d_off..]);
                if d_elem_len == 0 {
                    return Err(WideExtractFailure::MalformedInput);
                }
                d_off += d_elem_len;

                bindings.push(WideBinding {
                    offset: d_start,
                    len: d_elem_len,
                });

                // Decrement parent's remaining children
                if let Some(top) = stack.last_mut() {
                    top.0 = top.0.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                    top.1 = top.1.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                }
            }

            WideTag::VarRef => {
                // Template references a previously bound variable — verify match
                t_off += 1;
                let (var_idx, consumed) = decode_leb128(&template[t_off..])
                    .ok_or(WideExtractFailure::MalformedInput)?;
                t_off += consumed;

                if var_idx as usize >= bindings.len() {
                    return Err(WideExtractFailure::VarRefMismatch { var_idx });
                }

                let binding = &bindings[var_idx as usize];
                let d_elem_len = wide_expr_byte_len(&data[d_off..]);
                if d_elem_len == 0 {
                    return Err(WideExtractFailure::MalformedInput);
                }

                // Compare data bytes at current position with previously captured bytes
                let bound_bytes = &data[binding.offset..binding.offset + binding.len];
                let current_bytes = &data[d_off..d_off + d_elem_len];
                if bound_bytes != current_bytes {
                    return Err(WideExtractFailure::VarRefMismatch { var_idx });
                }

                d_off += d_elem_len;

                if let Some(top) = stack.last_mut() {
                    top.0 = top.0.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                    top.1 = top.1.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                }
            }

            WideTag::SymbolSize => {
                // Template has a concrete symbol — data must have the same symbol
                t_off += 1;
                let (t_size, t_consumed) = decode_leb128(&template[t_off..])
                    .ok_or(WideExtractFailure::MalformedInput)?;
                t_off += t_consumed;
                let t_sym_bytes = &template[t_off..t_off + t_size as usize];
                t_off += t_size as usize;

                // Data must also be a SymbolSize with matching bytes
                let d_tag = WideTag::from_byte(data[d_off])
                    .map_err(|_| WideExtractFailure::MalformedInput)?;
                match d_tag {
                    WideTag::SymbolSize => {
                        d_off += 1;
                        let (d_size, d_consumed) = decode_leb128(&data[d_off..])
                            .ok_or(WideExtractFailure::MalformedInput)?;
                        d_off += d_consumed;
                        let d_sym_bytes = &data[d_off..d_off + d_size as usize];
                        d_off += d_size as usize;

                        if t_sym_bytes != d_sym_bytes {
                            return Err(WideExtractFailure::Mismatch);
                        }
                    }
                    WideTag::NewVar | WideTag::VarRef => {
                        return Err(WideExtractFailure::DataContainsVariables);
                    }
                    WideTag::Arity => {
                        return Err(WideExtractFailure::Mismatch);
                    }
                }

                if let Some(top) = stack.last_mut() {
                    top.0 = top.0.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                    top.1 = top.1.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                }
            }

            WideTag::Arity => {
                // Template has a compound expression — data must have matching arity
                t_off += 1;
                let (t_arity, t_consumed) = decode_leb128(&template[t_off..])
                    .ok_or(WideExtractFailure::MalformedInput)?;
                t_off += t_consumed;

                let d_tag = WideTag::from_byte(data[d_off])
                    .map_err(|_| WideExtractFailure::MalformedInput)?;
                match d_tag {
                    WideTag::Arity => {
                        d_off += 1;
                        let (d_arity, d_consumed) = decode_leb128(&data[d_off..])
                            .ok_or(WideExtractFailure::MalformedInput)?;
                        d_off += d_consumed;

                        if t_arity != d_arity {
                            return Err(WideExtractFailure::Mismatch);
                        }

                        // Decrement parent's remaining children (this Arity consumed one slot)
                        if let Some(top) = stack.last_mut() {
                            top.0 = top.0.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                            top.1 = top.1.checked_sub(1).ok_or(WideExtractFailure::Mismatch)?;
                        }

                        // Push new level if there are children to process
                        if t_arity > 0 {
                            stack.push((t_arity, d_arity));
                        }
                    }
                    WideTag::NewVar | WideTag::VarRef => {
                        return Err(WideExtractFailure::DataContainsVariables);
                    }
                    WideTag::SymbolSize => {
                        return Err(WideExtractFailure::Mismatch);
                    }
                }
            }
        }
    }
}
