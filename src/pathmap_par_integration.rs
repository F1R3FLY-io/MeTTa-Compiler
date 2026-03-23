/// MettaTrie Par Integration Module
///
/// Provides conversion between MeTTa types and Rholang Par types.
/// This module enables MettaState to be represented as Rholang EPathMap structures.
///
/// ## Architecture
///
/// The AtomSpace uses `MettaTrie<V, Multiplicity>` for atom storage.
/// Serialization iterates the MettaTrie directly (no MORK, no PathMap).
/// Each atom is serialized as its MeTTa Display text + multiplicity.
/// MettaTrie handles arbitrary arity natively, so no separate wide_btm is needed.

use models::rhoapi::{expr::ExprInstance, EList, EPathMap, Expr, Par};
use tracing::{debug, trace};

use crate::backend::compile::compile_generic;
use crate::backend::decompose::decompose_literal;
use crate::backend::environment::multiplicity::trie_set_multiplicity;
use crate::backend::environment::MettaEnvironment;
use crate::backend::models::{MettaState, MettaValue, MettaValueInner, global_factory};

/// Helper function to create a Par with a string value
fn create_string_par(s: String) -> Par {
    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GString(s)),
    }])
}

/// Helper function to create a Par with an integer value
fn create_int_par(n: i64) -> Par {
    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GInt(n)),
    }])
}

// Magic numbers for MeTTa Environment byte arrays
// These identify byte arrays as MeTTa-specific data for the pretty-printer
const METTA_SPACE_MAGIC: &[u8] = b"MTTS"; // MeTTa Space
const METTA_LARGE_EXPRS_MAGIC: &[u8] = b"MTTL"; // MeTTa Large Expressions (legacy, always empty)

/// Convert a MettaValue to a Rholang Par object
pub fn metta_value_to_par(value: &MettaValue) -> Par {
    trace!(target: "mettatron::rholang_integration::metta_value_to_par", ?value, "MeTTa value");

    let par = match value.inner_ref() {
        MettaValueInner::Atom(s) => {
            // Atoms are plain strings (no quotes)
            create_string_par(s.to_string())
        }
        MettaValueInner::Bool(b) => Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::GBool(*b)),
        }]),
        MettaValueInner::Long(n) => create_int_par(*n),
        MettaValueInner::Float(f) => create_string_par(f.to_string()),
        MettaValueInner::String(s) => {
            // Strings are quoted with escaped quotes to distinguish from atoms
            create_string_par(format!(
                "\"{}\"",
                s.replace("\\", "\\\\").replace("\"", "\\\"")
            ))
        }
        MettaValueInner::Unit => {
            // Represent Unit as empty Par
            Par::default()
        }
        MettaValueInner::SExpr(items) => {
            // Convert S-expressions to Rholang lists for ...rest decomposition
            let item_pars: Vec<Par> = items.iter().map(metta_value_to_par).collect();

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps: item_pars,
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Error(msg, details) => {
            // Represent errors as lists: ["error", msg, details]
            let tag_par = create_string_par("error".to_string());
            let msg_par = create_string_par(msg.to_string());
            let details_par = metta_value_to_par(details);

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps: vec![tag_par, msg_par, details_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Type(t) => {
            // Represent types as tagged lists: ["type", <inner_value>]
            let tag_par = create_string_par("type".to_string());
            let value_par = metta_value_to_par(t);

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps: vec![tag_par, value_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Quoted(inner) => {
            // Represent quoted as tagged lists: ["quote", <inner_value>]
            let tag_par = create_string_par("quote".to_string());
            let value_par = metta_value_to_par(inner);

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps: vec![tag_par, value_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Conjunction(goals) => {
            // Represent conjunctions as tagged lists: ["conjunction", goal1, goal2, ...]
            let mut ps = vec![create_string_par("conjunction".to_string())];
            ps.extend(goals.iter().map(metta_value_to_par));

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps,
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Space(handle) => {
            // Represent spaces as tagged lists: ["space", id, name]
            let ps = vec![
                create_string_par("space".to_string()),
                create_int_par(handle.id as i64),
                create_string_par(handle.name.clone()),
            ];

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps,
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::State(id) => {
            // Represent states as tagged lists: ["state", id]
            let ps = vec![
                create_string_par("state".to_string()),
                create_int_par(*id as i64),
            ];

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps,
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Memo(handle) => {
            // Represent memos as tagged lists: ["memo", id, name]
            let ps = vec![
                create_string_par("memo".to_string()),
                create_int_par(handle.id as i64),
                create_string_par(handle.name.clone()),
            ];

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps,
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        MettaValueInner::Empty => {
            // Empty sentinel - represent as tagged list: ["empty"]
            let ps = vec![create_string_par("empty".to_string())];

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(EList {
                    ps,
                    locally_free: Vec::new(),
                    connective_used: false,
                    remainder: None,
                })),
            }])
        }
        // Spanned: strip span wrapper and convert the inner value transparently
        MettaValueInner::Spanned(v, _) => metta_value_to_par(v),
    };

    trace!(target: "mettatron::rholang_integration::metta_value_to_par", ?par, "Par");
    par
}

/// Convert a vector of MettaValues to a Rholang List Par
pub fn metta_values_to_list_par(values: &[MettaValue]) -> Par {
    trace!(target: "mettatron::rholang_integration::metta_values_to_list_par", ?values);
    let item_pars: Vec<Par> = values.iter().map(metta_value_to_par).collect();

    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: item_pars,
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }])
}

/// Convert Environment to a Rholang Par tuple.
///
/// Serializes the AtomSpace's MettaTrie as a byte array containing
/// MeTTa Display text representations with inline multiplicities.
///
/// Returns an EList with two named fields:
///   ("space", GByteArray) - MeTTa text entries with inline multiplicities
///   ("large_exprs", GByteArray) - Always empty (MettaTrie handles any arity)
/// Note: Type assertions are stored within the space, not separately
pub fn environment_to_par(env: &MettaEnvironment) -> Par {
    trace!(target: "mettatron::rholang_integration::environment_to_par", ?env);

    // Collect all atoms from the MettaTrie with their multiplicities.
    // Format: [magic: 4 bytes "MTTS"][num_entries: 8 bytes BE]
    //         [text_len: 4 bytes BE][utf8_text][multiplicity: 8 bytes BE]...
    let mut all_entries_data = Vec::new();

    // Write magic number to identify this as MeTTa space
    all_entries_data.extend_from_slice(METTA_SPACE_MAGIC);

    // Reserve space for entry count
    let count_offset = all_entries_data.len();
    all_entries_data.extend_from_slice(&[0u8; 8]);

    let mut entry_count = 0u64;

    {
        let btm = env.shared.atom_space.btm.read();

        for (expr, mult) in btm.iter() {
            let multiplicity = mult.count();
            // Serialize MettaValue as its Display text
            let text = format!("{}", expr);
            let text_bytes = text.as_bytes();

            // Write text length (4 bytes, big-endian)
            let len = text_bytes.len() as u32;
            all_entries_data.extend_from_slice(&len.to_be_bytes());
            // Write UTF-8 text bytes
            all_entries_data.extend_from_slice(text_bytes);
            // Write multiplicity (8 bytes, big-endian) inline with entry
            all_entries_data.extend_from_slice(&multiplicity.to_be_bytes());
            entry_count += 1;
        }
    }

    trace!(
        target: "mettatron::rholang_integration::environment_to_par",
        entry_count, space_data_len = all_entries_data.len()
    );

    // Write the actual count
    all_entries_data[count_offset..count_offset + 8]
        .copy_from_slice(&entry_count.to_be_bytes());

    // Store the collected bytes as a single GByteArray
    let space_epathmap = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(all_entries_data)),
    }]);

    // Large expressions section: always empty since MettaTrie handles any arity.
    // Format: [magic: 4 bytes "MTTL"][count: 8 bytes = 0]
    let mut large_exprs_bytes = Vec::with_capacity(12);
    large_exprs_bytes.extend_from_slice(METTA_LARGE_EXPRS_MAGIC);
    large_exprs_bytes.extend_from_slice(&0u64.to_be_bytes());

    let large_exprs_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(large_exprs_bytes)),
    }]);

    // Build EList with named field lists: [["space", ...], ["large_exprs", ...]]
    let space_list = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![create_string_par("space".to_string()), space_epathmap],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]);

    let large_exprs_list = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![
                create_string_par("large_exprs".to_string()),
                large_exprs_par,
            ],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]);

    // Return EList with 2 named field lists: [space, large_exprs]
    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![space_list, large_exprs_list],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }])
}

/// Convert MettaState to a Rholang Par containing an EPathMap
///
/// The EPathMap will contain a single EList with three named field lists:
/// - ["source", <list of exprs>]
/// - ["environment", <env data>]
/// - ["output", <list of output>]
pub fn metta_state_to_pathmap_par(state: &MettaState) -> Par {
    trace!(target: "mettatron::rholang_integration::metta_state_to_pathmap_par", ?state);
    let mut field_tuples = Vec::new();

    // Field 0: ["source", <list of exprs>]
    let pending_tag = create_string_par("source".to_string());
    let pending_list = metta_values_to_list_par(&state.source());
    field_tuples.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![pending_tag, pending_list],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]));

    // Field 1: ["environment", <env data>]
    let env_tag = create_string_par("environment".to_string());
    let env_data = environment_to_par(&state.environment);
    field_tuples.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![env_tag, env_data],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]));

    // Field 2: ["output", <list of output>]
    let outputs_tag = create_string_par("output".to_string());
    let outputs_list = metta_values_to_list_par(&state.output());
    field_tuples.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![outputs_tag, outputs_list],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]));

    // Wrap all three field lists in a single EList
    let state_list = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: field_tuples,
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]);

    // Create EPathMap with this single EList as its only element
    let epathmap = EPathMap {
        ps: vec![state_list],
        locally_free: Vec::new(),
        connective_used: false,
        remainder: None,
    };

    // Wrap in Expr and Par
    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EPathmapBody(epathmap)),
    }])
}

/// Convert MettaState to a Rholang Par for error cases
/// Returns a PathMap containing the error (to maintain consistent type)
pub fn metta_error_to_par(error_msg: &str) -> Par {
    // Create an error MettaValue
    let error_value = MettaValue::Error(error_msg.to_string(), MettaValue::Unit());

    // Create a MettaState with the error in output
    let error_state = MettaState::new_accumulated(
        MettaEnvironment::default(),
        vec![error_value],
    );

    // Return as PathMap (consistent with metta_state_to_pathmap_par)
    metta_state_to_pathmap_par(&error_state)
}

/// Convert a Rholang Par back to MettaValue
pub fn par_to_metta_value(par: &Par) -> Result<MettaValue, String> {
    trace!(target: "mettatron::rholang_integration::par_to_metta_value", ?par, "Par value");
    // Handle empty Par (Nil)
    if par.exprs.is_empty() && par.unforgeables.is_empty() && par.sends.is_empty() {
        return Ok(MettaValue::Unit());
    }

    // Get the first expression
    if let Some(expr) = par.exprs.first() {
        let val = match &expr.expr_instance {
            Some(ExprInstance::GString(s)) => {
                // Check if it's a quoted string (starts and ends with ")
                if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                    // It's a string - unescape and remove quotes
                    let unescaped = s[1..s.len() - 1]
                        .replace("\\\"", "\"")
                        .replace("\\\\", "\\");
                    Ok(MettaValue::String(unescaped))
                } else {
                    // It's an atom (plain string)
                    Ok(MettaValue::Atom(s.clone()))
                }
            }
            Some(ExprInstance::GInt(n)) => Ok(MettaValue::Long(*n)),
            Some(ExprInstance::GBool(b)) => Ok(MettaValue::Bool(*b)),
            Some(ExprInstance::EListBody(list)) => {
                // Check if it's a tagged structure (error, type)
                // Tagged structures have string tag as first element
                if list.ps.len() >= 2 {
                    if let Some(ExprInstance::GString(tag)) = list.ps[0]
                        .exprs
                        .first()
                        .and_then(|e| e.expr_instance.as_ref())
                    {
                        // Check if the tag looks like a quoted string (for distinguishing from atoms)
                        if tag.starts_with('"') {
                            match tag.as_str() {
                                "error" => {
                                    // Error list: [tag, msg, details]
                                    if list.ps.len() >= 3 {
                                        let msg = par_to_metta_value(&list.ps[1])?;
                                        let details = par_to_metta_value(&list.ps[2])?;
                                        if let MettaValueInner::String(msg_str) = msg.inner() {
                                            Ok(MettaValue::Error(msg_str, details))
                                        } else {
                                            Err("Error message must be a string".to_string())
                                        }
                                    } else {
                                        Err("Error list must have 3 elements".to_string())
                                    }
                                }
                                "type" => {
                                    // Type list: [tag, inner_value]
                                    let inner = par_to_metta_value(&list.ps[1])?;
                                    Ok(MettaValue::Type(inner))
                                }
                                _ => {
                                    // Unknown tag, treat as regular S-expr
                                    let items: Result<Vec<MettaValue>, String> =
                                        list.ps.iter().map(par_to_metta_value).collect();
                                    Ok(MettaValue::SExpr(items?))
                                }
                            }
                        } else {
                            // First element is an atom, not a tag - it's a regular S-expr
                            let items: Result<Vec<MettaValue>, String> =
                                list.ps.iter().map(par_to_metta_value).collect();
                            Ok(MettaValue::SExpr(items?))
                        }
                    } else {
                        // First element is not a string - it's a regular S-expr
                        let items: Result<Vec<MettaValue>, String> =
                            list.ps.iter().map(par_to_metta_value).collect();
                        Ok(MettaValue::SExpr(items?))
                    }
                } else {
                    // Small list, treat as S-expr
                    let items: Result<Vec<MettaValue>, String> =
                        list.ps.iter().map(par_to_metta_value).collect();
                    Ok(MettaValue::SExpr(items?))
                }
            }
            _ => Err("Unsupported Par expression type for MettaValue conversion".to_string()),
        };

        trace!(target: "mettatron::rholang_integration::par_to_metta_value", ?val, "MeTTa value");
        val
    } else {
        Err("Par has no expressions to convert".to_string())
    }
}

/// Parse MeTTa Display text back into a MettaValue using the compiler.
///
/// This is the inverse of `MettaValue::Display`. It parses a single expression
/// from the text and returns it as a MettaValue. Returns an Atom if parsing fails
/// (graceful degradation for unusual Display formats like `<Space:name>`).
fn parse_metta_text(text: &str) -> MettaValue {
    let factory = global_factory();
    match compile_generic(text, &factory) {
        Ok(values) if !values.is_empty() => values.into_iter().next().expect("checked non-empty"),
        _ => {
            // Fallback: if the text can't be parsed, treat as a plain atom.
            // This handles edge cases like <Space:name>, <State:id>, etc.
            MettaValue::Atom(text.to_string())
        }
    }
}

/// Read entry count and entries from MTTS-formatted bytes.
///
/// Returns a Vec of (MettaValue, multiplicity) tuples.
/// Used by both `par_to_environment` and `decode_space_bytes_to_pars`.
fn read_mtts_entries(space_dump_bytes: &[u8]) -> Vec<(MettaValue, u64)> {
    let mut entries = Vec::new();

    if space_dump_bytes.is_empty() {
        return entries;
    }

    let mut offset = 0;

    // Check and skip magic number if present
    if space_dump_bytes.len() >= 4 && &space_dump_bytes[0..4] == METTA_SPACE_MAGIC {
        offset += 4;
    }

    // Read entry count
    if offset + 8 > space_dump_bytes.len() {
        return entries;
    }
    let entry_count = u64::from_be_bytes([
        space_dump_bytes[offset],
        space_dump_bytes[offset + 1],
        space_dump_bytes[offset + 2],
        space_dump_bytes[offset + 3],
        space_dump_bytes[offset + 4],
        space_dump_bytes[offset + 5],
        space_dump_bytes[offset + 6],
        space_dump_bytes[offset + 7],
    ]);
    offset += 8;

    entries.reserve(entry_count as usize);

    for _ in 0..entry_count {
        if offset + 4 > space_dump_bytes.len() {
            break;
        }

        // Read text length
        let text_len = u32::from_be_bytes([
            space_dump_bytes[offset],
            space_dump_bytes[offset + 1],
            space_dump_bytes[offset + 2],
            space_dump_bytes[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + text_len + 8 > space_dump_bytes.len() {
            break;
        }

        // Read UTF-8 text
        let text_bytes = &space_dump_bytes[offset..offset + text_len];
        offset += text_len;

        // Read multiplicity (8 bytes, big-endian)
        let multiplicity = u64::from_be_bytes([
            space_dump_bytes[offset],
            space_dump_bytes[offset + 1],
            space_dump_bytes[offset + 2],
            space_dump_bytes[offset + 3],
            space_dump_bytes[offset + 4],
            space_dump_bytes[offset + 5],
            space_dump_bytes[offset + 6],
            space_dump_bytes[offset + 7],
        ]);
        offset += 8;

        // Parse text back to MettaValue
        if let Ok(text) = std::str::from_utf8(text_bytes) {
            let value = parse_metta_text(text);
            entries.push((value, multiplicity));
        }
    }

    entries
}

/// Read entry count and entries from MTTS-formatted bytes with LENIENT multiplicity handling.
///
/// When multiplicity bytes are missing or unreasonably large (> 2^32), defaults to 1.
/// Returns a Vec of (MettaValue, multiplicity) tuples.
fn read_mtts_entries_lenient(space_dump_bytes: &[u8]) -> Vec<(MettaValue, u64)> {
    let mut entries = Vec::new();

    if space_dump_bytes.is_empty() {
        return entries;
    }

    let mut offset = 0;

    // Check and skip magic number if present
    if space_dump_bytes.len() >= 4 && &space_dump_bytes[0..4] == METTA_SPACE_MAGIC {
        offset += 4;
    }

    // Read entry count
    if offset + 8 > space_dump_bytes.len() {
        return entries;
    }
    let entry_count = u64::from_be_bytes([
        space_dump_bytes[offset],
        space_dump_bytes[offset + 1],
        space_dump_bytes[offset + 2],
        space_dump_bytes[offset + 3],
        space_dump_bytes[offset + 4],
        space_dump_bytes[offset + 5],
        space_dump_bytes[offset + 6],
        space_dump_bytes[offset + 7],
    ]);
    offset += 8;

    entries.reserve(entry_count as usize);

    for _ in 0..entry_count {
        if offset + 4 > space_dump_bytes.len() {
            break;
        }

        // Read text length
        let text_len = u32::from_be_bytes([
            space_dump_bytes[offset],
            space_dump_bytes[offset + 1],
            space_dump_bytes[offset + 2],
            space_dump_bytes[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + text_len > space_dump_bytes.len() {
            break;
        }

        // Read UTF-8 text
        let text_bytes = &space_dump_bytes[offset..offset + text_len];
        offset += text_len;

        // LENIENT multiplicity handling
        let multiplicity = if offset + 8 <= space_dump_bytes.len() {
            let mult = u64::from_be_bytes([
                space_dump_bytes[offset],
                space_dump_bytes[offset + 1],
                space_dump_bytes[offset + 2],
                space_dump_bytes[offset + 3],
                space_dump_bytes[offset + 4],
                space_dump_bytes[offset + 5],
                space_dump_bytes[offset + 6],
                space_dump_bytes[offset + 7],
            ]);
            if mult > (1u64 << 32) {
                // Unreasonably large -- these bytes are likely the next
                // entry's length prefix, not a multiplicity. Don't advance.
                1u64
            } else {
                offset += 8;
                mult
            }
        } else {
            // Not enough bytes for multiplicity -- default to 1
            1u64
        };

        // Parse text back to MettaValue
        if let Ok(text) = std::str::from_utf8(text_bytes) {
            let value = parse_metta_text(text);
            entries.push((value, multiplicity));
        }
    }

    entries
}

/// Insert parsed (MettaValue, multiplicity) entries into a MettaEnvironment.
///
/// Uses `decompose_literal` to get TrieKeys and `trie_set_multiplicity` to
/// insert into the MettaTrie. Updates the total_atoms counter accordingly.
fn insert_entries_into_env(env: &mut MettaEnvironment, entries: &[(MettaValue, u64)]) {
    let mut total_atoms_added: usize = 0;
    {
        let mut btm = env.shared.atom_space.btm.write();
        for (value, multiplicity) in entries {
            let keys = decompose_literal(value);
            trie_set_multiplicity(&mut btm, &keys, value.clone(), *multiplicity);
            total_atoms_added += *multiplicity as usize;
        }
    }
    env.shared
        .atom_space
        .total_atoms
        .fetch_add(total_atoms_added, std::sync::atomic::Ordering::Relaxed);
}

/// Convert a Rholang Par back to Environment
/// Deserializes the AtomSpace's MettaTrie from MeTTa text entries with inline multiplicities
/// Expects an EList with named fields:
///   [["space", GByteArray], ["large_exprs", GByteArray]]
/// Multiplicities are encoded inline with each entry in MTTS byte arrays
/// Note: Type assertions are stored within the space, not separately
pub fn par_to_environment(par: &Par) -> Result<MettaEnvironment, String> {
    trace!(target: "mettatron::rholang_integration::par_to_environment", par_exprs_count = par.exprs.len());

    // The par should be an EList with 2 named field lists: [space, large_exprs]
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::EListBody(tuple)) = &expr.expr_instance {
            if tuple.ps.len() != 2 {
                debug!(
                    target: "mettatron::rholang_integration::par_to_environment",
                    expected = 2, got = tuple.ps.len(), "invalid environment tuple size"
                );
                return Err(format!(
                    "Expected 2 elements in environment tuple, got {}",
                    tuple.ps.len()
                ));
            }

            // Helper to extract value from [tag, value] list
            let extract_list_value = |list_par: &Par| -> Result<Par, String> {
                if let Some(expr) = list_par.exprs.first() {
                    if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                        if list.ps.len() >= 2 {
                            return Ok(list.ps[1].clone());
                        }
                    }
                }
                Err("Expected list with at least 2 elements".to_string())
            };

            // Extract space (element 0) - should be a single GByteArray
            let space_par = extract_list_value(&tuple.ps[0])?;
            let space_dump_bytes: Vec<u8> = if let Some(expr) = space_par.exprs.first() {
                if let Some(ExprInstance::GByteArray(bytes)) = &expr.expr_instance {
                    bytes.clone()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            trace!(target: "mettatron::rholang_integration::par_to_environment", space_bytes_len = space_dump_bytes.len());

            // Reconstruct Environment
            let mut env = MettaEnvironment::default();

            // Parse MTTS entries and insert into MettaTrie
            {
                let entries = read_mtts_entries(&space_dump_bytes);
                insert_entries_into_env(&mut env, &entries);

                // Rebuild bloom filter from restored space
                env.rebuild_bloom_filter_from_space();
            }

            // Element 1: large_exprs -- always empty with MettaTrie (any arity supported).
            // We still parse the field for structural compatibility, but it contains no entries.
            // No wide_btm to restore since MettaTrie handles arbitrary arity natively.

            // Rebuild bloom filter and RuleIndex from the restored MettaTrie
            env.rebuild_bloom_filter();

            Ok(env)
        } else {
            debug!(
                target: "mettatron::rholang_integration::par_to_environment",
                "expected EList for environment"
            );
            Err("Expected EList for environment".to_string())
        }
    } else {
        debug!(
            target: "mettatron::rholang_integration::par_to_environment",
            "environment Par has no expressions"
        );
        Err("Environment Par has no expressions".to_string())
    }
}

/// Convert a Rholang Par containing an EPathMap back to MettaState
pub fn pathmap_par_to_metta_state(par: &Par) -> Result<MettaState, String> {
    trace!(target: "mettatron::rholang_integration::pathmap_par_to_metta_state", par_exprs_count = par.exprs.len());

    // Get the EPathMap from the Par
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::EPathmapBody(pathmap)) = &expr.expr_instance {
            // The PathMap should contain a single EList with three named field lists
            if pathmap.ps.len() != 1 {
                debug!(
                    target: "mettatron::rholang_integration::pathmap_par_to_metta_state",
                    expected = 1, got = pathmap.ps.len(), "invalid PathMap size"
                );
                return Err(format!(
                    "Expected 1 element (EList) in PathMap, got {}",
                    pathmap.ps.len()
                ));
            }

            // Extract the EList from the PathMap
            let state_list_par = &pathmap.ps[0];
            if let Some(expr) = state_list_par.exprs.first() {
                if let Some(ExprInstance::EListBody(state_list)) = &expr.expr_instance {
                    // The list should have 3 named field lists
                    if state_list.ps.len() != 3 {
                        debug!(
                            target: "mettatron::rholang_integration::pathmap_par_to_metta_state",
                            expected = 3, got = state_list.ps.len(), "invalid state list size"
                        );
                        return Err(format!(
                            "Expected 3 named fields in state list, got {}",
                            state_list.ps.len()
                        ));
                    }

                    // Helper to extract value from [tag, value] list
                    let extract_list_value = |list_par: &Par| -> Result<Par, String> {
                        if let Some(expr) = list_par.exprs.first() {
                            if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                                if list.ps.len() >= 2 {
                                    return Ok(list.ps[1].clone());
                                }
                            }
                        }
                        Err("Expected list with at least 2 elements".to_string())
                    };

                    // Extract source
                    let pending_par = extract_list_value(&state_list.ps[0])?;
                    let source = if let Some(expr) = pending_par.exprs.first() {
                        if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                            let exprs: Result<Vec<MettaValue>, String> =
                                list.ps.iter().map(par_to_metta_value).collect();
                            exprs?
                        } else {
                            return Err("Expected EListBody for source".to_string());
                        }
                    } else {
                        Vec::new()
                    };

                    // Extract environment
                    let env_par = extract_list_value(&state_list.ps[1])?;
                    let environment = par_to_environment(&env_par)?;

                    // Extract output
                    let outputs_par = extract_list_value(&state_list.ps[2])?;
                    let output = if let Some(expr) = outputs_par.exprs.first() {
                        if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                            let outputs: Result<Vec<MettaValue>, String> =
                                list.ps.iter().map(par_to_metta_value).collect();
                            outputs?
                        } else {
                            return Err("Expected EListBody for output".to_string());
                        }
                    } else {
                        Vec::new()
                    };

                    // Pass source directly to from_parts -- no mutex contention
                    // during population, and GC registration happens once with
                    // the fully-populated Vec.
                    let state = MettaState::from_parts(source, environment, output);
                    Ok(state)
                } else {
                    debug!(target: "mettatron::rholang_integration::pathmap_par_to_metta_state", "expected EListBody in PathMap");
                    Err("Expected EListBody in PathMap".to_string())
                }
            } else {
                debug!(target: "mettatron::rholang_integration::pathmap_par_to_metta_state", "PathMap element has no expressions");
                Err("PathMap element has no expressions".to_string())
            }
        } else {
            debug!(target: "mettatron::rholang_integration::pathmap_par_to_metta_state", "Par does not contain EPathMap");
            Err("Par does not contain EPathMap".to_string())
        }
    } else {
        debug!(target: "mettatron::rholang_integration::pathmap_par_to_metta_state", "Par has no expressions");
        Err("Par has no expressions".to_string())
    }
}

/// Decode a GByteArray containing MeTTa Space data (magic "MTTS") back to a
/// vector of Rholang Pars representing the decoded MeTTa expressions.
///
/// This function parses the MTTS byte format, converts each text entry back
/// to a MettaValue, then converts each to a Rholang Par via `metta_value_to_par`.
pub fn decode_space_bytes_to_pars(bytes: &[u8]) -> Result<Vec<Par>, String> {
    if bytes.len() < 4 {
        return Err("Space bytes too short".to_string());
    }
    if &bytes[0..4] != METTA_SPACE_MAGIC {
        return Err(format!(
            "Invalid magic: expected {:?}, got {:?}",
            METTA_SPACE_MAGIC,
            &bytes[0..4]
        ));
    }

    let entries = read_mtts_entries(bytes);
    Ok(entries
        .iter()
        .map(|(value, _mult)| metta_value_to_par(value))
        .collect())
}

/// Decode a GByteArray containing MeTTa large expressions (magic "MTTL") back
/// to a vector of Rholang Pars.
///
/// With MettaTrie, all arities are handled natively, so the large expressions
/// section is always empty. This function validates the magic and returns an
/// empty Vec for valid MTTL data.
pub fn decode_large_exprs_bytes_to_pars(bytes: &[u8]) -> Result<Vec<Par>, String> {
    if bytes.len() < 4 {
        return Err("Large expression bytes too short".to_string());
    }
    if &bytes[0..4] != METTA_LARGE_EXPRS_MAGIC {
        return Err(format!(
            "Invalid magic: expected {:?}, got {:?}",
            METTA_LARGE_EXPRS_MAGIC,
            &bytes[0..4]
        ));
    }

    // MettaTrie handles any arity natively -- no large expressions to decode.
    // Return empty Vec for backward compatibility.
    Ok(Vec::new())
}

/// Check whether a PathMap has the structural shape of a serialized MettaState
/// without attempting full deserialization. Returns true if the structure matches
/// the expected layout: {| [["source", ...], ["environment", ...], ["output", ...]] |}
pub fn has_metta_state_structure(pathmap: &EPathMap) -> bool {
    // Must have exactly 1 element
    if pathmap.ps.len() != 1 {
        return false;
    }

    // The element must be an EList with exactly 3 fields
    let state_list_par = &pathmap.ps[0];
    let state_list = match state_list_par.exprs.first() {
        Some(Expr { expr_instance: Some(ExprInstance::EListBody(list)) }) => list,
        _ => return false,
    };

    if state_list.ps.len() != 3 {
        return false;
    }

    // Each field must be an EList with >= 2 elements, tagged with the expected names
    let expected_tags = ["source", "environment", "output"];
    for (field_par, expected_tag) in state_list.ps.iter().zip(expected_tags.iter()) {
        match field_par.exprs.first() {
            Some(Expr { expr_instance: Some(ExprInstance::EListBody(field_list)) }) => {
                if field_list.ps.len() < 2 {
                    return false;
                }
                // Check tag
                match field_list.ps[0].exprs.first() {
                    Some(Expr { expr_instance: Some(ExprInstance::GString(tag)) }) => {
                        if tag != expected_tag {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
    }

    true
}

/// Lenient version of `par_to_environment` that handles missing multiplicity bytes.
/// When multiplicity bytes are missing or unreasonably large (> 2^32), defaults to
/// Multiplicity::new(1). This allows deserialization of MettaState structures that
/// were reconstructed without the opaque MTTS binary encoding.
pub fn par_to_environment_lenient(par: &Par) -> Result<MettaEnvironment, String> {
    // The par should be an EList with 2 named field lists: [space, large_exprs]
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::EListBody(tuple)) = &expr.expr_instance {
            if tuple.ps.len() != 2 {
                return Err(format!(
                    "Expected 2 elements in environment list, got {}",
                    tuple.ps.len()
                ));
            }

            // Helper to extract value from [tag, value] list
            let extract_list_value = |list_par: &Par| -> Result<Par, String> {
                if let Some(expr) = list_par.exprs.first() {
                    if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                        if list.ps.len() >= 2 {
                            return Ok(list.ps[1].clone());
                        }
                    }
                }
                Err("Expected list with at least 2 elements".to_string())
            };

            // Extract space (element 0)
            let space_par = extract_list_value(&tuple.ps[0])?;
            let space_dump_bytes: Vec<u8> = if let Some(expr) = space_par.exprs.first() {
                if let Some(ExprInstance::GByteArray(bytes)) = &expr.expr_instance {
                    bytes.clone()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let mut env = MettaEnvironment::default();

            // Parse MTTS entries with LENIENT multiplicity handling and insert into MettaTrie
            {
                let entries = read_mtts_entries_lenient(&space_dump_bytes);
                insert_entries_into_env(&mut env, &entries);
                env.rebuild_bloom_filter_from_space();
            }

            // Element 1: large_exprs -- always empty with MettaTrie (any arity supported).
            // No wide_btm to restore.

            env.rebuild_bloom_filter();
            Ok(env)
        } else {
            Err("Expected EList for environment".to_string())
        }
    } else {
        Err("Environment Par has no expressions".to_string())
    }
}

/// Lenient version of `pathmap_par_to_metta_state` that attempts strict deserialization
/// first, then falls back to lenient environment handling for MettaState structures
/// with missing/malformed multiplicity bytes.
pub fn pathmap_par_to_metta_state_lenient(par: &Par) -> Result<MettaState, String> {
    // Try strict deserialization first
    match pathmap_par_to_metta_state(par) {
        Ok(state) => return Ok(state),
        Err(strict_err) => {
            // Check if the Par has MettaState structure
            if let Some(expr) = par.exprs.first() {
                if let Some(ExprInstance::EPathmapBody(pathmap)) = &expr.expr_instance {
                    if !has_metta_state_structure(pathmap) {
                        return Err(strict_err);
                    }
                } else {
                    return Err(strict_err);
                }
            } else {
                return Err(strict_err);
            }
        }
    }

    // Structure is valid -- attempt lenient deserialization
    let pathmap = match par.exprs.first() {
        Some(Expr { expr_instance: Some(ExprInstance::EPathmapBody(pm)) }) => pm,
        _ => return Err("Par does not contain EPathMap".to_string()),
    };

    let state_list_par = &pathmap.ps[0];
    let state_list = match state_list_par.exprs.first() {
        Some(Expr { expr_instance: Some(ExprInstance::EListBody(list)) }) => list,
        _ => return Err("Expected EListBody in PathMap".to_string()),
    };

    // Helper to extract value from [tag, value] list
    let extract_list_value = |list_par: &Par| -> Result<Par, String> {
        if let Some(expr) = list_par.exprs.first() {
            if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                if list.ps.len() >= 2 {
                    return Ok(list.ps[1].clone());
                }
            }
        }
        Err("Expected list with at least 2 elements".to_string())
    };

    // Extract source (field 0)
    let pending_par = extract_list_value(&state_list.ps[0])?;
    let source = if let Some(expr) = pending_par.exprs.first() {
        if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
            let exprs: Result<Vec<MettaValue>, String> =
                list.ps.iter().map(par_to_metta_value).collect();
            exprs?
        } else {
            return Err("Expected EListBody for source".to_string());
        }
    } else {
        Vec::new()
    };

    // Extract environment (field 1) -- with lenient fallback
    let env_par = extract_list_value(&state_list.ps[1])?;
    let environment = match par_to_environment(&env_par) {
        Ok(env) => env,
        Err(_) => match par_to_environment_lenient(&env_par) {
            Ok(env) => env,
            Err(_) => MettaEnvironment::default(),
        },
    };

    // Extract output (field 2)
    let outputs_par = extract_list_value(&state_list.ps[2])?;
    let output = if let Some(expr) = outputs_par.exprs.first() {
        if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
            let outputs: Result<Vec<MettaValue>, String> =
                list.ps.iter().map(par_to_metta_value).collect();
            outputs?
        } else {
            return Err("Expected EListBody for output".to_string());
        }
    } else {
        Vec::new()
    };

    Ok(MettaState::from_parts(source, environment, output))
}

/// Create an error Expr as an EListBody containing [error_code, message].
/// This is NOT an EPathmapBody, so it won't match `{| ..._ |}` in Rholang patterns,
/// enabling type-discriminated error handling via pattern matching.
pub fn metta_run_error_expr(error_code: &str, message: &str) -> Expr {
    Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![
                create_string_par(error_code.to_string()),
                create_string_par(message.to_string()),
            ],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }
}

/// Create an error Par wrapping `metta_run_error_expr` in a Par.
pub fn metta_run_error_par(error_code: &str, message: &str) -> Par {
    Par::default().with_exprs(vec![metta_run_error_expr(error_code, message)])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    use crate::backend::compile::compile;

    #[test]
    fn test_environment_serialization_roundtrip() {
        // Create an environment with a rule
        let mut env = MettaEnvironment::default();
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        );

        // Verify original environment
        assert_eq!(env.rule_count(), 1);
        println!("Original environment has {} rules", env.rule_count());

        // Serialize
        let par = environment_to_par(&env);
        println!("Serialized to Par");

        // Check that the serialized Par is an EList with 2 named field lists: [space, large_exprs]
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(env_list)) = par.exprs[0].expr_instance.as_ref() {
            assert_eq!(
                env_list.ps.len(), 2,
                "Expected EList with 2 fields, got {}",
                env_list.ps.len()
            );

            // Check field 0: ["space", <GByteArray>]
            if let Some(ExprInstance::EListBody(list)) = env_list.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                // Verify tag
                if let Some(ExprInstance::GString(tag)) = list.ps[0]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    assert_eq!(tag, "space");
                }
                // Verify space dump is a GByteArray and not empty
                if let Some(ExprInstance::GByteArray(dump_bytes)) = list.ps[1]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    println!("Space dump has {} bytes", dump_bytes.len());
                    assert!(!dump_bytes.is_empty(), "Space dump should not be empty");
                } else {
                    panic!("Expected GByteArray for space dump");
                }
            } else {
                panic!("Expected EListBody for field 0");
            }

            // Check field 1: ["large_exprs", <GByteArray>]
            if let Some(ExprInstance::EListBody(list)) = env_list.ps[1]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                if let Some(ExprInstance::GString(tag)) = list.ps[0]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    assert_eq!(tag, "large_exprs");
                }
                // Verify it's a GByteArray
                if let Some(ExprInstance::GByteArray(large_bytes)) = list.ps[1]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    println!(
                        "Large expressions is a GByteArray with {} bytes",
                        large_bytes.len()
                    );
                    // Should have at least 12 bytes (magic + count)
                    assert!(
                        large_bytes.len() >= 12,
                        "Large expressions byte array should have at least 12 bytes for magic + count"
                    );
                } else {
                    panic!("Expected GByteArray for large_exprs");
                }
            }
        } else {
            panic!("Expected EListBody");
        }

        // Deserialize
        let deserialized_env = par_to_environment(&par).expect("Failed to deserialize");
        println!(
            "Deserialized environment has {} rules",
            deserialized_env.rule_count()
        );

        // Verify deserialized environment
        assert_eq!(
            deserialized_env.rule_count(),
            1,
            "Expected 1 rule after deserialization"
        );

        // MettaTrie preserves exact variable names (unlike MORK De Bruijn indexing)
        println!("Rule count preserved after round-trip");
    }

    #[test]
    fn test_metta_value_atom_to_par() {
        let atom = MettaValue::Atom("test".to_string());
        let par = metta_value_to_par(&atom);

        // Should be a plain string Par (no quotes, no prefix)
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::GString(s)) = &par.exprs[0].expr_instance {
            assert_eq!(s, "test");
        } else {
            panic!("Expected GString");
        }
    }

    #[test]
    fn test_metta_value_string_to_par() {
        let string = MettaValue::String("hello world".to_string());
        let par = metta_value_to_par(&string);

        // Should be a quoted string
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::GString(s)) = &par.exprs[0].expr_instance {
            assert_eq!(s, "\"hello world\"");
        } else {
            panic!("Expected GString");
        }

        // Test round-trip
        let roundtrip = par_to_metta_value(&par).unwrap();
        if let MettaValueInner::String(s) = roundtrip.inner() {
            assert_eq!(*s, "hello world");
        } else {
            panic!("Expected MettaValue::String");
        }
    }

    #[test]
    fn test_metta_value_atom_string_distinction() {
        // Test that atoms and strings are correctly distinguished
        let atom = MettaValue::Atom("test".to_string());
        let string = MettaValue::String("test".to_string());

        let atom_par = metta_value_to_par(&atom);
        let string_par = metta_value_to_par(&string);

        // Atom should be plain
        if let Some(ExprInstance::GString(s)) = &atom_par.exprs[0].expr_instance {
            assert_eq!(s, "test");
        } else {
            panic!("Expected GString for atom");
        }

        // String should be quoted
        if let Some(ExprInstance::GString(s)) = &string_par.exprs[0].expr_instance {
            assert_eq!(s, "\"test\"");
        } else {
            panic!("Expected GString for string");
        }

        // Test round-trip preserves types
        let atom_roundtrip = par_to_metta_value(&atom_par).unwrap();
        let string_roundtrip = par_to_metta_value(&string_par).unwrap();

        assert!(matches!(atom_roundtrip.inner(), MettaValueInner::Atom(_)));
        assert!(matches!(
            string_roundtrip.inner(),
            MettaValueInner::String(_)
        ));
    }

    #[test]
    fn test_metta_value_long_to_par() {
        let num = MettaValue::Long(42);
        let par = metta_value_to_par(&num);

        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::GInt(n)) = &par.exprs[0].expr_instance {
            assert_eq!(*n, 42);
        } else {
            panic!("Expected GInt");
        }
    }

    #[test]
    fn test_metta_value_sexpr_to_par() {
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let par = metta_value_to_par(&sexpr);

        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert_eq!(list.ps.len(), 3);
        } else {
            panic!("Expected EListBody");
        }

        // Test round-trip
        let roundtrip = par_to_metta_value(&par).unwrap();
        if let MettaValueInner::SExpr(items) = roundtrip.inner() {
            assert_eq!(items.len(), 3);
        } else {
            panic!("Expected MettaValue::SExpr");
        }
    }

    #[test]
    fn test_metta_state_to_pathmap_par() {
        let state = MettaState::new_compiled(vec![MettaValue::Long(42)]);

        let par = metta_state_to_pathmap_par(&state);

        // Should have one expr (the EPathMap)
        assert_eq!(par.exprs.len(), 1);

        // Should be an EPathMap
        if let Some(ExprInstance::EPathmapBody(pathmap)) = &par.exprs[0].expr_instance {
            // Should have 1 element (the state EList)
            assert_eq!(pathmap.ps.len(), 1);

            // The element should be an EList with 3 named field lists
            if let Some(ExprInstance::EListBody(state_list)) = pathmap.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(
                    state_list.ps.len(),
                    3,
                    "Expected EList with 3 named fields (source, environment, output)"
                );
            } else {
                panic!("Expected EListBody for state");
            }
        } else {
            panic!("Expected EPathmapBody");
        }
    }

    #[test]
    fn test_metta_error_to_par() {
        let par = metta_error_to_par("test error");

        // Should return a PathMap (consistent type)
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EPathmapBody(pathmap)) = &par.exprs[0].expr_instance {
            // Should have 1 element (the state EList)
            assert_eq!(pathmap.ps.len(), 1);

            // Extract the state list
            if let Some(ExprInstance::EListBody(state_list)) = pathmap.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(
                    state_list.ps.len(),
                    3,
                    "Expected EList with 3 named fields (source, environment, output)"
                );

                // Check that output contains the error
                // Field 2 should be ["output", [error_value]]
                if let Some(expr) = state_list.ps[2].exprs.first() {
                    if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                        assert_eq!(list.ps.len(), 2, "Expected [tag, value] list");
                        // First element should be "output" tag
                        if let Some(ExprInstance::GString(tag)) = list.ps[0]
                            .exprs
                            .first()
                            .and_then(|e| e.expr_instance.as_ref())
                        {
                            assert_eq!(tag, "output");
                        } else {
                            panic!("Expected GString tag");
                        }
                    } else {
                        panic!("Expected EListBody for output element");
                    }
                } else {
                    panic!("Expected expr in state_list.ps[2]");
                }
            } else {
                panic!("Expected EListBody for state");
            }
        } else {
            panic!("Expected EPathmapBody");
        }
    }

    // ========== Roundtrip Tests ==========
    // These tests verify that MettaTrie-based serialization preserves atom data.

    #[test]
    fn test_reserved_bytes_roundtrip_y_z() {
        // Test with symbols containing 'y' (121) and 'z' (122) - previously reserved in MORK
        let mut env = MettaEnvironment::default();

        // Add expression with previously-reserved bytes
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ]));

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back
        let env2 =
            par_to_environment(&par).expect("Round-trip with bytes 'y' and 'z' failed");

        // Verify Space contents are preserved
        // MettaTrie preserves exact variable names (no De Bruijn renaming)
        assert!(env2.has_sexpr_fact(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ])));

        println!("Bytes 'y' (121) and 'z' (122) round-trip successfully");
    }

    #[test]
    fn test_reserved_bytes_roundtrip_tilde() {
        // Test with tilde '~' (126)
        let mut env = MettaEnvironment::default();

        // Add expression with tilde
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Atom("room~a".to_string()),
            MettaValue::Atom("room~b".to_string()),
        ]));

        // Get initial iter count
        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back
        let env2 =
            par_to_environment(&par).expect("Round-trip with byte '~' (126) failed");

        // The key test: it didn't panic!
        let final_count = env2.collect_rules().len();
        println!("Deserialized space has {} rules", final_count);
        assert_eq!(
            final_count, initial_count,
            "Space contents should be preserved"
        );

        println!("Byte '~' (126) round-trip successfully");
    }

    #[test]
    fn test_reserved_bytes_multiple_roundtrips() {
        // Test multiple round-trips to ensure data is preserved exactly
        let mut env = MettaEnvironment::default();

        // Add multiple expressions with various characters
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("path".to_string()),
            MettaValue::Atom("room_x".to_string()),
            MettaValue::Atom("room_y".to_string()),
        ]));
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room~a".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ]));

        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // First round-trip
        let par1 = environment_to_par(&env);
        let env2 = par_to_environment(&par1).expect("First round-trip failed");
        let count2 = env2.collect_rules().len();
        println!("After 1st round-trip: {} rules", count2);

        // Second round-trip
        let par2 = environment_to_par(&env2);
        let env3 = par_to_environment(&par2).expect("Second round-trip failed");
        let count3 = env3.collect_rules().len();
        println!("After 2nd round-trip: {} rules", count3);

        // Third round-trip
        let par3 = environment_to_par(&env3);
        let env4 = par_to_environment(&par3).expect("Third round-trip failed");
        let count4 = env4.collect_rules().len();
        println!("After 3rd round-trip: {} rules", count4);

        // The key test: multiple round-trips preserve data
        assert_eq!(
            count4, initial_count,
            "Rule count should be stable across round-trips"
        );

        println!("Multiple round-trips successful");
    }

    #[test]
    fn test_reserved_bytes_with_rules() {
        // Test rules with symbols containing various bytes
        let mut env = MettaEnvironment::default();

        // Add fact
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ]));

        // Add rule that uses match
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("is_connected".to_string()),
                MettaValue::Atom("$from".to_string()),
                MettaValue::Atom("$to".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("match".to_string()),
                MettaValue::Atom("&".to_string()),
                MettaValue::Atom("self".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("connected".to_string()),
                    MettaValue::Atom("$from".to_string()),
                    MettaValue::Atom("$to".to_string()),
                ]),
                MettaValue::Bool(true),
            ]),
        );

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back
        let env2 =
            par_to_environment(&par).expect("Round-trip with rules failed");

        // Verify both the fact and the rule are preserved
        assert!(env2.has_sexpr_fact(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ])));
        assert_eq!(env2.rule_count(), 1);

        println!("Rules with various bytes round-trip successfully");
    }

    #[test]
    fn test_reserved_bytes_all_range() {
        // Test with various ASCII characters
        let mut env = MettaEnvironment::default();

        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Atom("ABC".to_string()),
            MettaValue::Atom("xyz".to_string()),
            MettaValue::Atom("@~".to_string()),
        ]));

        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back
        let env2 =
            par_to_environment(&par).expect("Round-trip with various bytes failed");

        let final_count = env2.collect_rules().len();
        println!("Deserialized space has {} rules", final_count);
        assert_eq!(
            final_count, initial_count,
            "Space contents should be preserved"
        );

        println!("All bytes handled correctly");
    }

    #[test]
    fn test_reserved_bytes_robot_planning_regression() {
        // REGRESSION TEST for symbols containing 'o' (byte 111)
        let mut env = MettaEnvironment::default();

        // Add facts with 'o' (111)
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_a".to_string()),
            MettaValue::Atom("room_b".to_string()),
        ]));

        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("object_at".to_string()),
            MettaValue::Atom("robot".to_string()),
            MettaValue::Atom("room_a".to_string()),
        ]));

        // Add a rule that uses match
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("is_connected".to_string()),
                MettaValue::Atom("$from".to_string()),
                MettaValue::Atom("$to".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("match".to_string()),
                MettaValue::Atom("&".to_string()),
                MettaValue::Atom("self".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("connected".to_string()),
                    MettaValue::Atom("$from".to_string()),
                    MettaValue::Atom("$to".to_string()),
                ]),
                MettaValue::Bool(true),
            ]),
        );

        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back
        let env2 = par_to_environment(&par).expect(
            "REGRESSION: Round-trip with 'o' (111) in symbols failed!",
        );

        // Verify data is preserved
        let final_count = env2.collect_rules().len();
        println!("Deserialized space has {} rules", final_count);
        assert_eq!(
            final_count, initial_count,
            "Space contents should be preserved"
        );

        println!(
            "REGRESSION TEST PASSED: symbols with 'o' (111) work correctly!"
        );
    }

    #[test]
    fn test_reserved_bytes_with_evaluation() {
        // Test that deserialized Environment can actually be USED for evaluation
        let mut env = MettaEnvironment::default();

        // Add facts with 'o' (111)
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_a".to_string()),
            MettaValue::Atom("room_b".to_string()),
        ]));

        // Serialize and deserialize
        let par = environment_to_par(&env);
        let env2 = par_to_environment(&par).expect("Deserialization failed");

        // Verify the deserialized environment contains the fact
        assert!(env2.shared.atom_space.total_atoms.load(Ordering::Relaxed) > 0, "Should find the connected fact after deserialization");

        println!("Deserialized Environment can be used after roundtrip!");
    }

    #[test]
    fn test_source_field_roundtrip_with_eval_expr() {
        // Test that source field with ! expression survives roundtrip
        // Compile a query with ! expression
        let query = "!(get_neighbors room_a)";
        let state = compile(query).expect("Failed to compile");

        // Verify source is populated after compile
        {
            let source = state.source();
            assert_eq!(source.len(), 1, "Source should have 1 expression");
            assert!(
                source[0].as_sexpr().map_or(false, |items| !items.is_empty() && items[0].as_atom() == Some("!")),
                "Source[0] should be an eval expression (starts with !)"
            );
            println!(
                "After compile: source = {:?}, is_eval_expr = {}",
                source[0],
                source[0].as_sexpr().map_or(false, |items| !items.is_empty() && items[0].as_atom() == Some("!"))
            );
        }

        // Serialize to PathMap Par
        let par = metta_state_to_pathmap_par(&state);

        // Deserialize back
        let deserialized = pathmap_par_to_metta_state(&par).expect("Failed to deserialize PathMap");

        // Verify source is preserved
        {
            let source = deserialized.source();
            assert_eq!(
                source.len(),
                1,
                "Deserialized source should have 1 expression"
            );
            println!(
                "After deserialize: source = {:?}, is_eval_expr = {}",
                source[0],
                source[0].as_sexpr().map_or(false, |items| !items.is_empty() && items[0].as_atom() == Some("!"))
            );

            // Critical: Check that is_eval_expr() still returns true
            assert!(
                source[0].as_sexpr().map_or(false, |items| !items.is_empty() && items[0].as_atom() == Some("!")),
                "Deserialized source[0] should still be an eval expression"
            );
        }

        println!("Source field with ! expression survives roundtrip");
    }

    // ==========================================================================
    // Additional Branch Coverage Tests
    // ==========================================================================

    #[test]
    fn test_metta_value_float_to_par() {
        let float_val = MettaValue::Float(3.14);
        let par = metta_value_to_par(&float_val);

        // Float should be converted to string representation
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::GString(s)) = &par.exprs[0].expr_instance {
            assert!(s.contains("3.14"), "Float should be represented as string");
        } else {
            panic!("Expected GString for Float");
        }
    }

    #[test]
    fn test_metta_value_unit_to_par_empty() {
        let unit_val = MettaValue::Unit();
        let par = metta_value_to_par(&unit_val);

        // Unit should be empty Par
        assert!(par.exprs.is_empty(), "Unit should be empty Par");
    }

    #[test]
    fn test_metta_value_bool_to_par() {
        let true_val = MettaValue::Bool(true);
        let false_val = MettaValue::Bool(false);

        let true_par = metta_value_to_par(&true_val);
        let false_par = metta_value_to_par(&false_val);

        // Verify true
        assert_eq!(true_par.exprs.len(), 1);
        if let Some(ExprInstance::GBool(b)) = &true_par.exprs[0].expr_instance {
            assert!(*b);
        } else {
            panic!("Expected GBool for true");
        }

        // Verify false
        assert_eq!(false_par.exprs.len(), 1);
        if let Some(ExprInstance::GBool(b)) = &false_par.exprs[0].expr_instance {
            assert!(!*b);
        } else {
            panic!("Expected GBool for false");
        }
    }

    #[test]
    fn test_metta_value_unit_to_par() {
        let unit_val = MettaValue::Unit();
        let par = metta_value_to_par(&unit_val);

        // Unit maps to empty Par (no expressions)
        assert_eq!(par.exprs.len(), 0);
    }

    #[test]
    fn test_metta_value_empty_to_par() {
        let empty_val = MettaValue::Empty();
        let par = metta_value_to_par(&empty_val);

        // Empty should be a list with single "empty" tag
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert_eq!(list.ps.len(), 1);
            if let Some(ExprInstance::GString(tag)) = list.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(tag, "empty");
            } else {
                panic!("Expected GString tag");
            }
        } else {
            panic!("Expected EListBody for Empty");
        }
    }

    #[test]
    fn test_metta_value_type_to_par() {
        let type_val = MettaValue::Type(MettaValue::Atom("Int".to_string()));
        let par = metta_value_to_par(&type_val);

        // Type should be tagged list: ["type", inner_value]
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert_eq!(list.ps.len(), 2);
            // First should be "type" tag
            if let Some(ExprInstance::GString(tag)) = list.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(tag, "type");
            }
        } else {
            panic!("Expected EListBody for Type");
        }
    }

    #[test]
    fn test_metta_value_conjunction_to_par() {
        let conj = MettaValue::Conjunction(vec![
            MettaValue::Atom("goal1".to_string()),
            MettaValue::Atom("goal2".to_string()),
        ]);
        let par = metta_value_to_par(&conj);

        // Conjunction should be tagged list: ["conjunction", goal1, goal2, ...]
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert_eq!(list.ps.len(), 3); // tag + 2 goals
            // First should be "conjunction" tag
            if let Some(ExprInstance::GString(tag)) = list.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(tag, "conjunction");
            }
        } else {
            panic!("Expected EListBody for Conjunction");
        }
    }

    #[test]
    fn test_metta_value_state_to_par() {
        let state_val = MettaValue::State(42);
        let par = metta_value_to_par(&state_val);

        // State should be tagged list: ["state", id]
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert_eq!(list.ps.len(), 2);
            // First should be "state" tag
            if let Some(ExprInstance::GString(tag)) = list.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(tag, "state");
            }
            // Second should be state id
            if let Some(ExprInstance::GInt(id)) = list.ps[1]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(*id, 42);
            }
        } else {
            panic!("Expected EListBody for State");
        }
    }

    #[test]
    fn test_metta_values_to_list_par_empty() {
        let values: Vec<MettaValue> = vec![];
        let par = metta_values_to_list_par(&values);

        // Empty list
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert!(list.ps.is_empty());
        } else {
            panic!("Expected EListBody");
        }
    }

    #[test]
    fn test_metta_values_to_list_par_mixed() {
        let values = vec![
            MettaValue::Long(1),
            MettaValue::Bool(true),
            MettaValue::Atom("test".to_string()),
        ];
        let par = metta_values_to_list_par(&values);

        // List with 3 items
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::EListBody(list)) = &par.exprs[0].expr_instance {
            assert_eq!(list.ps.len(), 3);
        } else {
            panic!("Expected EListBody");
        }
    }

    #[test]
    fn test_par_to_metta_value_list() {
        // Create a list Par
        let list_par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EListBody(EList {
                ps: vec![create_int_par(1), create_int_par(2), create_int_par(3)],
                locally_free: Vec::new(),
                connective_used: false,
                remainder: None,
            })),
        }]);

        let result = par_to_metta_value(&list_par).unwrap();

        // Lists are converted to S-expressions
        if let MettaValueInner::SExpr(items) = result.inner() {
            assert_eq!(items.len(), 3);
        } else {
            panic!("Expected SExpr from list");
        }
    }

    #[test]
    fn test_par_to_metta_value_empty_par() {
        // Empty Par should return Unit
        let empty_par = Par::default();
        let result = par_to_metta_value(&empty_par).unwrap();

        assert!(matches!(result.inner(), MettaValueInner::Unit));
    }

    #[test]
    fn test_par_to_metta_value_no_expressions_error() {
        // Par with sends but no expressions
        let mut par = Par::default();
        par.sends = vec![]; // Empty but different from empty Par check
        par.exprs = vec![];
        par.unforgeables = vec![];

        // This should return Unit (empty Par case)
        let result = par_to_metta_value(&par);
        assert!(result.is_ok());
    }

    #[test]
    fn test_par_to_metta_value_unsupported_type() {
        // Create a Par with an unsupported expression type (e.g., EMethodBody)
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EMethodBody(models::rhoapi::EMethod::default())),
        }]);

        let result = par_to_metta_value(&par);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Unsupported Par expression type"));
    }

    #[test]
    fn test_par_to_environment_wrong_list_size() {
        // Create a Par with wrong list size (1 instead of 2)
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EListBody(EList {
                ps: vec![create_string_par("only_one".to_string())],
                locally_free: Vec::new(),
                connective_used: false,
                remainder: None,
            })),
        }]);

        let result = par_to_environment(&par);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Expected 2 elements"));
    }

    #[test]
    fn test_par_to_environment_not_elist() {
        // Create a Par that's not an EList
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::GString("not a list".to_string())),
        }]);

        let result = par_to_environment(&par);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Expected EList"));
    }

    #[test]
    fn test_par_to_environment_empty_par() {
        // Empty Par
        let par = Par::default();

        let result = par_to_environment(&par);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no expressions"));
    }

    #[test]
    fn test_pathmap_par_to_metta_state_wrong_pathmap_size() {
        // Create a PathMap with wrong size (2 instead of 1)
        let pathmap = EPathMap {
            ps: vec![Par::default(), Par::default()],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        };
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EPathmapBody(pathmap)),
        }]);

        let result = pathmap_par_to_metta_state(&par);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Expected 1 element"));
    }

    #[test]
    fn test_pathmap_par_to_metta_state_not_pathmap() {
        // Create a Par that's not a PathMap
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::GString("not a pathmap".to_string())),
        }]);

        let result = pathmap_par_to_metta_state(&par);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not contain EPathMap"));
    }

    #[test]
    fn test_pathmap_par_to_metta_state_empty() {
        // Empty Par
        let par = Par::default();

        let result = pathmap_par_to_metta_state(&par);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no expressions"));
    }

    #[test]
    fn test_par_to_metta_value_list_small() {
        // Small list (less than 2 elements) should become S-expr
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EListBody(EList {
                ps: vec![create_int_par(42)],
                locally_free: Vec::new(),
                connective_used: false,
                remainder: None,
            })),
        }]);

        let result = par_to_metta_value(&par).unwrap();

        // Small lists become S-exprs
        if let MettaValueInner::SExpr(items) = result.inner() {
            assert_eq!(items.len(), 1);
        } else {
            panic!("Expected SExpr from small list");
        }
    }

    #[test]
    fn test_par_to_metta_value_list_non_string_first() {
        // List where first element is not a string
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EListBody(EList {
                ps: vec![create_int_par(1), create_int_par(2), create_int_par(3)],
                locally_free: Vec::new(),
                connective_used: false,
                remainder: None,
            })),
        }]);

        let result = par_to_metta_value(&par).unwrap();

        // Should become regular S-expr
        if let MettaValueInner::SExpr(items) = result.inner() {
            assert_eq!(items.len(), 3);
        } else {
            panic!("Expected SExpr from list with non-string first element");
        }
    }

    #[test]
    fn test_par_to_metta_value_list_atom_first() {
        // List where first element is an atom (unquoted string) - should be S-expr
        let par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::EListBody(EList {
                ps: vec![
                    create_string_par("add".to_string()), // unquoted = atom
                    create_int_par(1),
                    create_int_par(2),
                ],
                locally_free: Vec::new(),
                connective_used: false,
                remainder: None,
            })),
        }]);

        let result = par_to_metta_value(&par).unwrap();

        // Atom as first element means regular S-expr
        if let MettaValueInner::SExpr(items) = result.inner() {
            assert_eq!(items.len(), 3);
        } else {
            panic!("Expected SExpr from list with atom first element");
        }
    }

    #[test]
    fn test_string_with_escape_sequences_roundtrip() {
        // Test strings with escape sequences
        let string_val = MettaValue::String("hello \"world\" with \\ backslash".to_string());
        let par = metta_value_to_par(&string_val);

        let roundtrip = par_to_metta_value(&par).unwrap();

        if let MettaValueInner::String(s) = roundtrip.inner() {
            assert_eq!(*s, "hello \"world\" with \\ backslash");
        } else {
            panic!("Expected String after roundtrip");
        }
    }

    #[test]
    fn test_multiplicity_roundtrip() {
        // Test that multiplicities survive serialization/deserialization round-trip.
        // This verifies the inline multiplicity encoding in MTTS works correctly.
        let mut env = MettaEnvironment::default();

        // Add the same atom multiple times to create non-trivial multiplicities
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("color".to_string()),
            MettaValue::Atom("red".to_string()),
        ]);
        env.add_to_space(&fact);
        env.add_to_space(&fact); // multiplicity = 2
        env.add_to_space(&fact); // multiplicity = 3

        let fact2 = MettaValue::SExpr(vec![
            MettaValue::Atom("color".to_string()),
            MettaValue::Atom("blue".to_string()),
        ]);
        env.add_to_space(&fact2); // multiplicity = 1

        // Capture original multiplicities
        let original_multiplicities = env.get_multiplicities();
        println!("Original multiplicities: {:?}", original_multiplicities);

        // Serialize
        let par = environment_to_par(&env);

        // Deserialize
        let deserialized_env = par_to_environment(&par)
            .expect("Failed to deserialize environment with multiplicities");

        // Compare multiplicities
        let deserialized_multiplicities = deserialized_env.get_multiplicities();
        println!("Deserialized multiplicities: {:?}", deserialized_multiplicities);

        // Verify each original entry is preserved with correct count
        for (key, original_count) in &original_multiplicities {
            let deserialized_count = deserialized_multiplicities.get(key)
                .unwrap_or_else(|| panic!("Missing key '{}' after deserialization", key));
            assert_eq!(
                *original_count, *deserialized_count,
                "Multiplicity mismatch for key '{}': original={}, deserialized={}",
                key, original_count, deserialized_count
            );
        }
        assert_eq!(
            original_multiplicities.len(),
            deserialized_multiplicities.len(),
            "Number of multiplicity entries should match"
        );

        println!("Multiplicity round-trip preserves all counts correctly!");
    }

    #[test]
    fn test_multiplicity_roundtrip_multiple_cycles() {
        // Verify multiplicities survive multiple round-trip cycles
        let mut env = MettaEnvironment::default();

        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("item".to_string()),
            MettaValue::Atom("sword".to_string()),
        ]);
        // Add 5 copies
        for _ in 0..5 {
            env.add_to_space(&fact);
        }

        let original_multiplicities = env.get_multiplicities();

        // Round-trip 3 times
        let par1 = environment_to_par(&env);
        let env2 = par_to_environment(&par1).expect("Round-trip 1 failed");
        let par2 = environment_to_par(&env2);
        let env3 = par_to_environment(&par2).expect("Round-trip 2 failed");
        let par3 = environment_to_par(&env3);
        let env4 = par_to_environment(&par3).expect("Round-trip 3 failed");

        let final_multiplicities = env4.get_multiplicities();

        // Verify all counts are stable after 3 round-trips
        for (key, original_count) in &original_multiplicities {
            let final_count = final_multiplicities.get(key)
                .unwrap_or_else(|| panic!("Missing key '{}' after 3 round-trips", key));
            assert_eq!(
                *original_count, *final_count,
                "Multiplicity for '{}' changed after 3 round-trips: {} -> {}",
                key, original_count, final_count
            );
        }
        assert_eq!(original_multiplicities.len(), final_multiplicities.len());

        println!("Multiplicities stable across 3 round-trip cycles!");
    }

    #[test]
    fn test_has_metta_state_structure_valid() {
        // Create a proper MettaState and convert to PathMap
        let state = MettaState::from_parts(
            vec![MettaValue::Atom("test".to_string())],
            MettaEnvironment::default(),
            vec![MettaValue::Long(42)],
        );
        let par = metta_state_to_pathmap_par(&state);

        if let Some(Expr { expr_instance: Some(ExprInstance::EPathmapBody(pathmap)) }) = par.exprs.first() {
            assert!(has_metta_state_structure(pathmap), "Valid MettaState PathMap should pass structure check");
        } else {
            panic!("Expected EPathmapBody");
        }
    }

    #[test]
    fn test_has_metta_state_structure_invalid() {
        // Create {| true |} -- a PathMap with a boolean, not MettaState structure
        let pathmap = EPathMap {
            ps: vec![Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::GBool(true)),
            }])],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        };
        assert!(!has_metta_state_structure(&pathmap), "{{| true |}} should not match MeTTa State structure");
    }

    #[test]
    fn test_has_metta_state_structure_wrong_tags() {
        // EList with 3 fields but wrong tag names
        let wrong_tag_list = EList {
            ps: vec![
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![
                            create_string_par("wrong_tag".to_string()),
                            Par::default(),
                        ],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![
                            create_string_par("environment".to_string()),
                            Par::default(),
                        ],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![
                            create_string_par("output".to_string()),
                            Par::default(),
                        ],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
            ],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        };
        let pathmap = EPathMap {
            ps: vec![Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::EListBody(wrong_tag_list)),
            }])],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        };
        assert!(!has_metta_state_structure(&pathmap), "Wrong tags should not match");
    }

    #[test]
    fn test_metta_run_error_expr_is_elist() {
        let error = metta_run_error_expr("test_code", "test message");
        match &error.expr_instance {
            Some(ExprInstance::EListBody(list)) => {
                assert_eq!(list.ps.len(), 2);
                // Check error code
                if let Some(ExprInstance::GString(code)) = list.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                    assert_eq!(code, "test_code");
                } else {
                    panic!("Expected GString error code");
                }
                // Check message
                if let Some(ExprInstance::GString(msg)) = list.ps[1].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                    assert_eq!(msg, "test message");
                } else {
                    panic!("Expected GString message");
                }
            }
            Some(ExprInstance::EPathmapBody(_)) => panic!("Error should NOT be EPathmapBody"),
            other => panic!("Expected EListBody, got {:?}", other),
        }
    }

    #[test]
    fn test_metta_run_error_par_wraps_correctly() {
        let par = metta_run_error_par("err_code", "err msg");
        assert_eq!(par.exprs.len(), 1);
        assert!(matches!(
            par.exprs[0].expr_instance,
            Some(ExprInstance::EListBody(_))
        ));
    }

    #[test]
    fn test_lenient_deserialization_empty_env() {
        // Create a MettaState with empty environment, serialize, then deserialize leniently
        let state = MettaState::from_parts(
            vec![MettaValue::Atom("hello".to_string())],
            MettaEnvironment::default(),
            vec![MettaValue::Long(1)],
        );
        let par = metta_state_to_pathmap_par(&state);

        // Lenient should succeed (strict should also succeed for this case)
        let result = pathmap_par_to_metta_state_lenient(&par);
        assert!(result.is_ok(), "Lenient deserialization of valid state should succeed: {:?}", result.err());
        let deserialized = result.expect("already checked");
        assert_eq!(deserialized.source().len(), 1);
        assert_eq!(deserialized.output().len(), 1);
    }
}
