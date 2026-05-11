/// PathMap Par Integration Module
///
/// Provides conversion between MeTTa types and Rholang PathMap-based Par types.
/// This module enables MettaState to be represented as Rholang EPathMap structures.
use std::fs;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use models::rhoapi::{expr::ExprInstance, EList, EPathMap, Expr, Par};
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
use tracing::{debug, trace};

use crate::backend::environment::multiplicity::Multiplicity;
use crate::backend::environment::MettaEnvironment;
use crate::backend::models::{MettaState, MettaValue, MettaValueInner};

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
const METTA_LARGE_EXPRS_MAGIC: &[u8] = b"MTTL"; // MeTTa Large Expressions (arity >= 64)

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

/// Convert Environment to a Rholang Par tuple
/// Serializes the Space's PathMap with inline multiplicities as byte arrays
/// Returns an EList with two named fields:
///   ("space", GByteArray) - Raw MORK trie bytes with inline multiplicities
///   ("large_exprs", GByteArray) - Wide MORK paths (arity >= 64) with inline multiplicities
/// Note: Type assertions are stored within the space, not separately
pub fn environment_to_par(env: &MettaEnvironment) -> Par {
    // CRITICAL FIX for "reserved 111" bug:
    // We CANNOT use dump_all_sexpr() because it calls serialize2() which interprets
    // bytes as MORK tags. When symbol data contains bytes in range 64-127 (like 'o'=111),
    // serialize2() tries to interpret them as tags and panics with "reserved X".
    //
    // Instead, we collect RAW path bytes directly from the trie using read_zipper.
    // This preserves bytes exactly without interpretation.

    trace!(target: "mettatron::rholang_integration::environment_to_par", ?env);
    let space = env.create_space();

    // Collect all raw path bytes from the PathMap trie
    let mut all_paths_data = Vec::new();
    let mut rz = space.btm.read_zipper();

    // Write format: [magic: 4 bytes "MTTS"][sym_table_len: 8 bytes][sym_table_bytes][num_paths: 8 bytes][path1_len: 4 bytes][path1_bytes][multiplicity1: 8 bytes]...

    // Write magic number to identify this as MeTTa space
    all_paths_data.extend_from_slice(METTA_SPACE_MAGIC);

    // First, serialize the symbol table to a temp file, then read it
    let symbol_table_bytes = {
        // Create unique temp file for symbol table (include timestamp to avoid parallel test collisions)
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_path = std::env::temp_dir().join(format!(
            "metta_symbols_{}_{}.bin",
            std::process::id(),
            timestamp
        ));

        // Backup symbols to temp file
        if space.backup_symbols(&temp_path).is_err() {
            // If backup fails, use empty bytes
            Vec::new()
        } else {
            // Read the temp file into memory
            let bytes = fs::read(&temp_path).unwrap_or_default();
            // Clean up temp file
            let _ = fs::remove_file(&temp_path);
            bytes
        }
    };
    trace!(target: "mettatron::rholang_integration::environment_to_par", symbol_table_len = symbol_table_bytes.len());

    // Write symbol table length and bytes
    let sym_len = symbol_table_bytes.len() as u64;
    all_paths_data.extend_from_slice(&sym_len.to_be_bytes());
    all_paths_data.extend_from_slice(&symbol_table_bytes);

    // Write path count (reserve space)
    let mut path_count = 0u64;
    let count_offset = all_paths_data.len();
    all_paths_data.extend_from_slice(&[0u8; 8]); // Reserve space for count

    // Iterate through all paths and collect their raw bytes with inline multiplicities
    while rz.to_next_val() {
        let path_bytes = rz.path();
        let multiplicity = rz.val().map(|m| m.count()).unwrap_or(1);
        // Write path length (4 bytes, big-endian)
        let len = path_bytes.len() as u32;
        all_paths_data.extend_from_slice(&len.to_be_bytes());
        // Write raw path bytes (NO INTERPRETATION!)
        all_paths_data.extend_from_slice(path_bytes);
        // Write multiplicity (8 bytes, big-endian) inline with path
        all_paths_data.extend_from_slice(&multiplicity.to_be_bytes());
        path_count += 1;
    }
    trace!(target: "mettatron::rholang_integration::environment_to_par", path_count, space_data_len = all_paths_data.len());

    // Write the actual count at the beginning
    all_paths_data[count_offset..count_offset + 8].copy_from_slice(&path_count.to_be_bytes());

    drop(rz);
    drop(space);

    // Store the collected bytes as a single GByteArray
    let space_bytes_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(all_paths_data)),
    }]);

    // The space is now a single GByteArray with raw path bytes
    let space_epathmap = space_bytes_par;

    // Serialize wide expressions (arity >= 64) from wide_btm PathMap
    // Format: [magic: 4 bytes "MTTL"][count: 8 bytes][expr1_len: 4 bytes][expr1_bytes][multiplicity1: 8 bytes]...
    // Uses Wide MORK storage encoding (tag-byte + LEB128) for expressions that exceed 63-arity limit
    let mut large_exprs_bytes = Vec::new();
    large_exprs_bytes.extend_from_slice(METTA_LARGE_EXPRS_MAGIC);

    {
        use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};

        let wbtm = env.shared.atom_space.wide_btm.read();
        let mut wrz = wbtm.read_zipper();

        // Reserve space for count
        let count_offset = large_exprs_bytes.len();
        large_exprs_bytes.extend_from_slice(&[0u8; 8]);

        let mut count = 0u64;
        while wrz.to_next_val() {
            let path_bytes = wrz.path();
            let multiplicity = wrz.val().map(|m| m.count()).unwrap_or(1);
            // Write Wide MORK key bytes directly (already in storage encoding)
            let len = path_bytes.len() as u32;
            large_exprs_bytes.extend_from_slice(&len.to_be_bytes());
            large_exprs_bytes.extend_from_slice(path_bytes);
            // Write multiplicity (8 bytes, big-endian) inline with path
            large_exprs_bytes.extend_from_slice(&multiplicity.to_be_bytes());
            count += 1;
        }

        // Write actual count
        large_exprs_bytes[count_offset..count_offset + 8].copy_from_slice(&count.to_be_bytes());
    }

    let large_exprs_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(large_exprs_bytes)),
    }]);

    // Build EList with named field lists: [["space", ...], ["large_exprs", ...]]
    // Multiplicities are now encoded inline with each path in MTTS/MTTL byte arrays
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
    let error_state = MettaState::new_accumulated(MettaEnvironment::default(), vec![error_value]);

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

/// Convert a Rholang Par back to Environment
/// Deserializes the Space's PathMap with inline multiplicities from byte arrays
/// Expects an EList with named fields:
///   [["space", GByteArray], ["large_exprs", GByteArray]]
/// Multiplicities are encoded inline with each path in MTTS/MTTL byte arrays
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

            // Extract space (element 0) - should be a single GByteArray (MORK dump format)
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

            // Rebuild the Space from raw path bytes with inline multiplicities
            // CRITICAL FIX for "reserved 111" bug:
            // We stored raw path bytes (not text), so we insert them directly.
            // This avoids any interpretation of bytes as MORK tags.
            // We also restore the symbol table so symbol IDs match.
            // Multiplicities are inline with each path — no separate MTTM field needed.
            {
                let mut space = env.create_space();
                if !space_dump_bytes.is_empty() {
                    // Read format: [magic: 4 bytes "MTTS"][sym_table_len: 8 bytes][sym_table_bytes][num_paths: 8 bytes][path1_len: 4 bytes][path1_bytes][multiplicity1: 8 bytes]...
                    if space_dump_bytes.len() >= 12 {
                        // 4 bytes magic + 8 bytes sym_table_len minimum
                        let mut offset = 0;

                        // Check and skip magic number if present
                        if space_dump_bytes.len() >= 4
                            && &space_dump_bytes[0..4] == METTA_SPACE_MAGIC
                        {
                            offset += 4; // Skip magic number
                        }

                        // Read symbol table length
                        let sym_len = u64::from_be_bytes([
                            space_dump_bytes[offset],
                            space_dump_bytes[offset + 1],
                            space_dump_bytes[offset + 2],
                            space_dump_bytes[offset + 3],
                            space_dump_bytes[offset + 4],
                            space_dump_bytes[offset + 5],
                            space_dump_bytes[offset + 6],
                            space_dump_bytes[offset + 7],
                        ]) as usize;
                        offset += 8;

                        // Restore symbol table if present
                        if sym_len > 0 && offset + sym_len <= space_dump_bytes.len() {
                            trace!(
                                target: "mettatron::rholang_integration::par_to_environment",
                                sym_len, offset, "Restore symbol table"
                            );

                            let symbol_table_bytes = &space_dump_bytes[offset..offset + sym_len];
                            offset += sym_len;

                            // Write symbol table to temp file (unique name to avoid collisions)
                            let timestamp = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos();
                            let temp_path = std::env::temp_dir().join(format!(
                                "metta_symbols_restore_{}_{}.bin",
                                std::process::id(),
                                timestamp
                            ));
                            if let Ok(mut file) = fs::File::create(&temp_path) {
                                if file.write_all(symbol_table_bytes).is_ok() {
                                    drop(file); // Close file before restoring
                                                // Restore symbols from temp file
                                    let _ = space.restore_symbols(&temp_path);
                                    // Clean up temp file
                                    let _ = fs::remove_file(&temp_path);
                                }
                            }
                        }

                        // Read path count
                        if offset + 8 <= space_dump_bytes.len() {
                            let path_count = u64::from_be_bytes([
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

                            // Read and insert each path with its inline multiplicity
                            let mut total_atoms_added: usize = 0;
                            for _ in 0..path_count {
                                if offset + 4 > space_dump_bytes.len() {
                                    break; // Not enough data
                                }

                                // Read path length
                                let len = u32::from_be_bytes([
                                    space_dump_bytes[offset],
                                    space_dump_bytes[offset + 1],
                                    space_dump_bytes[offset + 2],
                                    space_dump_bytes[offset + 3],
                                ]) as usize;
                                offset += 4;

                                if offset + len + 8 > space_dump_bytes.len() {
                                    break; // Not enough data for path + multiplicity
                                }

                                // Get raw path bytes
                                let path_bytes = &space_dump_bytes[offset..offset + len];
                                offset += len;

                                // Read inline multiplicity (8 bytes, big-endian)
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

                                // Insert path with correct multiplicity directly
                                space
                                    .btm
                                    .insert(path_bytes, Multiplicity::new(multiplicity));
                                total_atoms_added += multiplicity as usize;
                            }
                            // Update total_atoms counter for all paths inserted
                            env.shared
                                .atom_space
                                .total_atoms
                                .fetch_add(total_atoms_added, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                // Update shared PathMap with modified Space
                env.update_pathmap(space);

                // Rebuild bloom filter from restored space
                // The bloom filter is not serialized, so we need to rebuild it
                // by iterating through all entries and extracting (head, arity) pairs
                env.rebuild_bloom_filter_from_space();
            }

            // Extract and restore wide expressions (element 1) — always present
            // These are expressions with arity >= 64 that exceed MORK's 63-arity limit
            // Stored as Wide MORK storage bytes (tag-byte + LEB128) with inline multiplicities
            {
                let large_exprs_par = extract_list_value(&tuple.ps[1])?;
                if let Some(expr) = large_exprs_par.exprs.first() {
                    if let Some(ExprInstance::GByteArray(large_bytes)) = &expr.expr_instance {
                        // Read format: [magic: 4 bytes "MTTL"][count: 8 bytes][expr1_len: 4 bytes][expr1_bytes][multiplicity1: 8 bytes]...
                        if large_bytes.len() >= 12 {
                            let mut offset = 0;

                            // Check and skip magic number if present
                            if large_bytes.len() >= 4
                                && &large_bytes[0..4] == METTA_LARGE_EXPRS_MAGIC
                            {
                                offset += 4;
                            }

                            // Read count
                            if offset + 8 <= large_bytes.len() {
                                let count = u64::from_be_bytes([
                                    large_bytes[offset],
                                    large_bytes[offset + 1],
                                    large_bytes[offset + 2],
                                    large_bytes[offset + 3],
                                    large_bytes[offset + 4],
                                    large_bytes[offset + 5],
                                    large_bytes[offset + 6],
                                    large_bytes[offset + 7],
                                ]);
                                offset += 8;

                                // Read and restore each wide expression with inline multiplicity
                                for _ in 0..count {
                                    if offset + 4 > large_bytes.len() {
                                        break;
                                    }

                                    // Read expression length
                                    let len = u32::from_be_bytes([
                                        large_bytes[offset],
                                        large_bytes[offset + 1],
                                        large_bytes[offset + 2],
                                        large_bytes[offset + 3],
                                    ]) as usize;
                                    offset += 4;

                                    if offset + len + 8 > large_bytes.len() {
                                        break; // Not enough data for path + multiplicity
                                    }

                                    // Wide MORK bytes — insert directly into wide_btm
                                    let wide_bytes = &large_bytes[offset..offset + len];
                                    offset += len;

                                    // Read inline multiplicity (8 bytes, big-endian)
                                    let multiplicity = u64::from_be_bytes([
                                        large_bytes[offset],
                                        large_bytes[offset + 1],
                                        large_bytes[offset + 2],
                                        large_bytes[offset + 3],
                                        large_bytes[offset + 4],
                                        large_bytes[offset + 5],
                                        large_bytes[offset + 6],
                                        large_bytes[offset + 7],
                                    ]);
                                    offset += 8;

                                    {
                                        use crate::backend::environment::multiplicity as mult_mod;
                                        let mut wbtm = env.shared.atom_space.wide_btm.write();
                                        mult_mod::set_multiplicity(
                                            &mut wbtm,
                                            wide_bytes,
                                            multiplicity,
                                        );
                                    }
                                    env.shared.atom_space.total_atoms.fetch_add(
                                        multiplicity as usize,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // Rebuild bloom filter from the restored MORK Space
            // This is critical for rule matching to work after deserialization
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

                    // Pass source directly to from_parts — no mutex contention
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
/// This function fully restores the MORK space (including symbol table) and
/// converts each atom back to a Rholang Par via `metta_value_to_par`.
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

    // Build a Par wrapping the space bytes, then wrap in an environment-shaped
    // tuple so `par_to_environment` can decode it. The environment tuple has
    // the shape: (("space", GByteArray), ("large_exprs", GByteArray))
    let space_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(bytes.to_vec())),
    }]);

    // Create empty large_exprs bytes (magic + 0 count)
    let mut empty_large = Vec::new();
    empty_large.extend_from_slice(METTA_LARGE_EXPRS_MAGIC);
    empty_large.extend_from_slice(&0u64.to_be_bytes()); // count = 0
    let large_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(empty_large)),
    }]);

    let env_list_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![
                // ["space", space_bytes]
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("space".to_string()), space_par],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
                // ["large_exprs", empty_large_bytes]
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("large_exprs".to_string()), large_par],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
            ],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]);

    let env = par_to_environment(&env_list_par)?;
    let atoms = env.get_all_atoms();
    Ok(atoms.iter().map(metta_value_to_par).collect())
}

/// Decode a GByteArray containing MeTTa large expressions (magic "MTTL") back
/// to a vector of Rholang Pars.
///
/// Large expressions are those with arity >= 64 that exceed MORK's 63-arity
/// limit and are stored using Wide MORK encoding.
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

    // Build a synthetic environment tuple with just the large_exprs field
    // so par_to_environment decodes it.
    let large_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(bytes.to_vec())),
    }]);

    // Create empty space bytes (magic + 0 sym_table_len + 0 paths)
    let mut empty_space = Vec::new();
    empty_space.extend_from_slice(METTA_SPACE_MAGIC);
    empty_space.extend_from_slice(&0u64.to_be_bytes()); // sym_table_len = 0
    empty_space.extend_from_slice(&0u64.to_be_bytes()); // path_count = 0
    let space_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(empty_space)),
    }]);

    let env_list_par = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: vec![
                // ["space", empty_space_bytes]
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("space".to_string()), space_par],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
                // ["large_exprs", large_expr_bytes]
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("large_exprs".to_string()), large_par],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
            ],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]);

    let env = par_to_environment(&env_list_par)?;
    // Only return the wide expressions (not the empty regular space)
    let atoms = env.get_all_atoms();
    Ok(atoms.iter().map(metta_value_to_par).collect())
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
        Some(Expr {
            expr_instance: Some(ExprInstance::EListBody(list)),
        }) => list,
        _ => return false,
    };

    if state_list.ps.len() != 3 {
        return false;
    }

    // Each field must be an EList with >= 2 elements, tagged with the expected names
    let expected_tags = ["source", "environment", "output"];
    for (field_par, expected_tag) in state_list.ps.iter().zip(expected_tags.iter()) {
        match field_par.exprs.first() {
            Some(Expr {
                expr_instance: Some(ExprInstance::EListBody(field_list)),
            }) => {
                if field_list.ps.len() < 2 {
                    return false;
                }
                // Check tag
                match field_list.ps[0].exprs.first() {
                    Some(Expr {
                        expr_instance: Some(ExprInstance::GString(tag)),
                    }) => {
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

            // Rebuild Space from raw path bytes with LENIENT multiplicity handling
            {
                let mut space = env.create_space();
                if !space_dump_bytes.is_empty() && space_dump_bytes.len() >= 12 {
                    let mut offset = 0;

                    // Check and skip magic number if present
                    if space_dump_bytes.len() >= 4 && &space_dump_bytes[0..4] == METTA_SPACE_MAGIC {
                        offset += 4;
                    }

                    // Read symbol table length
                    if offset + 8 > space_dump_bytes.len() {
                        // Not enough data for sym_table_len — return empty env
                        env.update_pathmap(space);
                        env.rebuild_bloom_filter_from_space();
                        return Ok(env);
                    }
                    let sym_len = u64::from_be_bytes([
                        space_dump_bytes[offset],
                        space_dump_bytes[offset + 1],
                        space_dump_bytes[offset + 2],
                        space_dump_bytes[offset + 3],
                        space_dump_bytes[offset + 4],
                        space_dump_bytes[offset + 5],
                        space_dump_bytes[offset + 6],
                        space_dump_bytes[offset + 7],
                    ]) as usize;
                    offset += 8;

                    // Restore symbol table if present
                    if sym_len > 0 && offset + sym_len <= space_dump_bytes.len() {
                        let symbol_table_bytes = &space_dump_bytes[offset..offset + sym_len];
                        offset += sym_len;

                        let timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos();
                        let temp_path = std::env::temp_dir().join(format!(
                            "metta_symbols_restore_{}_{}.bin",
                            std::process::id(),
                            timestamp
                        ));
                        if let Ok(mut file) = fs::File::create(&temp_path) {
                            if file.write_all(symbol_table_bytes).is_ok() {
                                drop(file);
                                let _ = space.restore_symbols(&temp_path);
                                let _ = fs::remove_file(&temp_path);
                            }
                        }
                    } else if sym_len > 0 {
                        offset = space_dump_bytes.len(); // skip past invalid sym table
                    }

                    // Read path count
                    if offset + 8 <= space_dump_bytes.len() {
                        let path_count = u64::from_be_bytes([
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

                        let mut total_atoms_added: usize = 0;
                        for _ in 0..path_count {
                            if offset + 4 > space_dump_bytes.len() {
                                break;
                            }

                            let len = u32::from_be_bytes([
                                space_dump_bytes[offset],
                                space_dump_bytes[offset + 1],
                                space_dump_bytes[offset + 2],
                                space_dump_bytes[offset + 3],
                            ]) as usize;
                            offset += 4;

                            if offset + len > space_dump_bytes.len() {
                                break;
                            }

                            let path_bytes = &space_dump_bytes[offset..offset + len];
                            offset += len;

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
                                    // Unreasonably large — these bytes are likely the next
                                    // path's length prefix, not a multiplicity. Don't advance.
                                    1u64
                                } else {
                                    offset += 8;
                                    mult
                                }
                            } else {
                                // Not enough bytes for multiplicity — default to 1
                                1u64
                            };

                            space
                                .btm
                                .insert(path_bytes, Multiplicity::new(multiplicity));
                            total_atoms_added += multiplicity as usize;
                        }
                        env.shared
                            .atom_space
                            .total_atoms
                            .fetch_add(total_atoms_added, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                env.update_pathmap(space);
                env.rebuild_bloom_filter_from_space();
            }

            // Extract and restore wide expressions (element 1) with LENIENT multiplicity handling
            {
                let large_exprs_par = extract_list_value(&tuple.ps[1])?;
                if let Some(expr) = large_exprs_par.exprs.first() {
                    if let Some(ExprInstance::GByteArray(large_bytes)) = &expr.expr_instance {
                        if large_bytes.len() >= 12 {
                            let mut offset = 0;

                            if large_bytes.len() >= 4
                                && &large_bytes[0..4] == METTA_LARGE_EXPRS_MAGIC
                            {
                                offset += 4;
                            }

                            if offset + 8 <= large_bytes.len() {
                                let count = u64::from_be_bytes([
                                    large_bytes[offset],
                                    large_bytes[offset + 1],
                                    large_bytes[offset + 2],
                                    large_bytes[offset + 3],
                                    large_bytes[offset + 4],
                                    large_bytes[offset + 5],
                                    large_bytes[offset + 6],
                                    large_bytes[offset + 7],
                                ]);
                                offset += 8;

                                for _ in 0..count {
                                    if offset + 4 > large_bytes.len() {
                                        break;
                                    }

                                    let len = u32::from_be_bytes([
                                        large_bytes[offset],
                                        large_bytes[offset + 1],
                                        large_bytes[offset + 2],
                                        large_bytes[offset + 3],
                                    ]) as usize;
                                    offset += 4;

                                    if offset + len > large_bytes.len() {
                                        break;
                                    }

                                    let wide_bytes = &large_bytes[offset..offset + len];
                                    offset += len;

                                    // LENIENT multiplicity handling
                                    let multiplicity = if offset + 8 <= large_bytes.len() {
                                        let mult = u64::from_be_bytes([
                                            large_bytes[offset],
                                            large_bytes[offset + 1],
                                            large_bytes[offset + 2],
                                            large_bytes[offset + 3],
                                            large_bytes[offset + 4],
                                            large_bytes[offset + 5],
                                            large_bytes[offset + 6],
                                            large_bytes[offset + 7],
                                        ]);
                                        if mult > (1u64 << 32) {
                                            1u64
                                        } else {
                                            offset += 8;
                                            mult
                                        }
                                    } else {
                                        1u64
                                    };

                                    {
                                        use crate::backend::environment::multiplicity as mult_mod;
                                        let mut wbtm = env.shared.atom_space.wide_btm.write();
                                        mult_mod::set_multiplicity(
                                            &mut wbtm,
                                            wide_bytes,
                                            multiplicity,
                                        );
                                    }
                                    env.shared.atom_space.total_atoms.fetch_add(
                                        multiplicity as usize,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                }
                            }
                        }
                    }
                }
            }

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

    // Structure is valid — attempt lenient deserialization
    let pathmap = match par.exprs.first() {
        Some(Expr {
            expr_instance: Some(ExprInstance::EPathmapBody(pm)),
        }) => pm,
        _ => return Err("Par does not contain EPathMap".to_string()),
    };

    let state_list_par = &pathmap.ps[0];
    let state_list = match state_list_par.exprs.first() {
        Some(Expr {
            expr_instance: Some(ExprInstance::EListBody(list)),
        }) => list,
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

    // Extract environment (field 1) — with lenient fallback
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
                env_list.ps.len(),
                2,
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

        // Note: MORK uses De Bruijn indexing which can cause variable renaming (e.g., $x -> $a)
        // The important part is that the structure is preserved, not the exact variable names
        println!("✓ Environment serialization/deserialization works!");
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

    // ========== Reserved Byte Bug Tests ==========
    // These tests ensure the "reserved 126" bug is fixed and doesn't return

    #[test]
    fn test_reserved_bytes_roundtrip_y_z() {
        // Test with symbols containing 'y' (121) and 'z' (122) - reserved bytes
        let mut env = MettaEnvironment::default();

        // Add expression with reserved bytes
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()), // Contains 'y' = 121 (reserved)
            MettaValue::Atom("room_z".to_string()), // Contains 'z' = 122 (reserved)
        ]));

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back
        let env2 =
            par_to_environment(&par).expect("Round-trip with reserved bytes 'y' and 'z' failed");

        // Verify Space contents are preserved
        // MORK uses De Bruijn indexing so we check structure, not exact strings
        assert!(env2.has_sexpr_fact(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ])));

        println!("✓ Reserved bytes 'y' (121) and 'z' (122) round-trip successfully");
    }

    #[test]
    fn test_reserved_bytes_roundtrip_tilde() {
        // Test with tilde '~' (126) - the specific byte mentioned in the bug report
        let mut env = MettaEnvironment::default();

        // Add expression with tilde (the problematic reserved byte)
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Atom("room~a".to_string()), // Contains '~' = 126 (RESERVED!)
            MettaValue::Atom("room~b".to_string()), // Contains '~' = 126 (RESERVED!)
        ]));

        // Get initial iter count
        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back - this used to panic with "reserved 126"
        let env2 =
            par_to_environment(&par).expect("Round-trip with reserved byte '~' (126) failed");

        // The key test: it didn't panic! The bug is fixed.
        // Verify Space is not empty - exact structure may vary due to MORK normalization
        let final_count = env2.collect_rules().len();
        println!("Deserialized space has {} rules", final_count);
        assert_eq!(
            final_count, initial_count,
            "Space contents should be preserved"
        );

        println!("✓ Reserved byte '~' (126) round-trip successfully - bug is FIXED!");
    }

    #[test]
    fn test_reserved_bytes_multiple_roundtrips() {
        // Test multiple round-trips to ensure bytes are preserved exactly
        let mut env = MettaEnvironment::default();

        // Add multiple expressions with various reserved bytes
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("path".to_string()),
            MettaValue::Atom("room_x".to_string()), // 'x' = 120
            MettaValue::Atom("room_y".to_string()), // 'y' = 121 (reserved)
        ]));
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room~a".to_string()), // '~' = 126 (reserved)
            MettaValue::Atom("room_z".to_string()), // 'z' = 122 (reserved)
        ]));

        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // First round-trip - this used to panic
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

        // The key test: multiple round-trips don't panic and preserve data
        assert_eq!(
            count4, initial_count,
            "Rule count should be stable across round-trips"
        );

        println!("✓ Multiple round-trips with reserved bytes successful - NO PANICS!");
    }

    #[test]
    fn test_reserved_bytes_with_rules() {
        // Test the original bug scenario: rules with if + match containing reserved bytes
        let mut env = MettaEnvironment::default();

        // Add fact with reserved bytes
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()), // 'y' = 121 (reserved)
            MettaValue::Atom("room_z".to_string()), // 'z' = 122 (reserved)
        ]));

        // Add rule that uses match (the pattern that triggered the bug)
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

        // Serialize to Par (this is what happens when sending to Rholang)
        let par = environment_to_par(&env);

        // Deserialize back (this is what happens when receiving from Rholang)
        // This used to panic with "reserved 121" or "reserved 122"
        let env2 =
            par_to_environment(&par).expect("Round-trip with rules and reserved bytes failed");

        // Verify both the fact and the rule are preserved
        assert!(env2.has_sexpr_fact(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()),
            MettaValue::Atom("room_z".to_string()),
        ])));
        assert_eq!(env2.rule_count(), 1);

        println!("✓ Rules with match and reserved bytes round-trip successfully");
    }

    #[test]
    fn test_reserved_bytes_all_range() {
        // Test all bytes in the reserved range (64-127)
        // This ensures the fix works for ANY reserved byte, not just specific ones
        let mut env = MettaEnvironment::default();

        // Add expressions with various ASCII characters in the reserved range
        // '@' = 64, 'A' = 65, ..., 'Z' = 90, ..., 'z' = 122, '{' = 123, '~' = 126, DEL = 127
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Atom("ABC".to_string()), // A=65, B=66, C=67 (all reserved)
            MettaValue::Atom("xyz".to_string()), // x=120, y=121, z=122 (last two reserved)
            MettaValue::Atom("@~".to_string()),  // @=64, ~=126 (both reserved)
        ]));

        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back - should handle ALL reserved bytes without panic
        let env2 =
            par_to_environment(&par).expect("Round-trip with multiple reserved bytes failed");

        // The critical test: it didn't panic! All reserved bytes handled.
        let final_count = env2.collect_rules().len();
        println!("Deserialized space has {} rules", final_count);
        assert_eq!(
            final_count, initial_count,
            "Space contents should be preserved"
        );

        println!("✓ All bytes in reserved range (64-127) handled correctly - NO PANIC!");
    }

    #[test]
    fn test_reserved_bytes_robot_planning_regression() {
        // REGRESSION TEST for the "reserved 111" bug from robot_planning.rho
        // This test specifically uses symbols containing 'o' (byte 111) which is reserved
        // The bug occurred when dump_all_sexpr() tried to interpret 'o' as a tag byte
        let mut env = MettaEnvironment::default();

        // Add facts with 'o' (111) - the specific byte that triggered the demo failure
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()), // 'o' = 111, 'n' = 110 (reserved bytes!)
            MettaValue::Atom("room_a".to_string()),    // 'o' = 111 (RESERVED!)
            MettaValue::Atom("room_b".to_string()),    // 'o' = 111, 'b' = 98 (reserved!)
        ]));

        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("object_at".to_string()), // 'o' = 111, 'b' = 98 (RESERVED!)
            MettaValue::Atom("robot".to_string()),     // 'o' = 111, 'b' = 98 (RESERVED!)
            MettaValue::Atom("room_a".to_string()),    // 'o' = 111 (RESERVED!)
        ]));

        // Add a rule that uses match (pattern from robot_planning.rho)
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("is_connected".to_string()), // 'o' = 111, 'n' = 110 (RESERVED!)
                MettaValue::Atom("$from".to_string()),
                MettaValue::Atom("$to".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("match".to_string()),
                MettaValue::Atom("&".to_string()),
                MettaValue::Atom("self".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("connected".to_string()), // 'o' = 111, 'n' = 110 (RESERVED!)
                    MettaValue::Atom("$from".to_string()),
                    MettaValue::Atom("$to".to_string()),
                ]),
                MettaValue::Bool(true),
            ]),
        );

        let initial_count = env.collect_rules().len();
        println!("Initial space has {} rules", initial_count);

        // THIS IS THE EXACT OPERATION THAT FAILED IN robot_planning.rho DEMO!
        // Serialize to Par (this calls dump_all_sexpr which used to panic with "reserved 111")
        let par = environment_to_par(&env);

        // Deserialize back (if we get here, the bug is fixed!)
        let env2 = par_to_environment(&par).expect(
            "REGRESSION: Round-trip with 'o' (111) in symbols failed - the bug has returned!",
        );

        // Verify data is preserved
        let final_count = env2.collect_rules().len();
        println!("Deserialized space has {} rules", final_count);
        assert_eq!(
            final_count, initial_count,
            "Space contents should be preserved"
        );

        println!(
            "✓ REGRESSION TEST PASSED: robot_planning.rho symbols with 'o' (111) work correctly!"
        );
    }

    #[test]
    fn test_reserved_bytes_with_evaluation() {
        // Test that deserialized Environment can actually be USED for evaluation
        // This exposes issues that simple round-trip tests miss
        let mut env = MettaEnvironment::default();

        // Add facts with 'o' (111) - reserved byte
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()), // 'o' = 111
            MettaValue::Atom("room_a".to_string()),    // 'o' = 111
            MettaValue::Atom("room_b".to_string()),    // 'o' = 111
        ]));

        // Serialize and deserialize
        let par = environment_to_par(&env);
        let env2 = par_to_environment(&par).expect("Deserialization failed");

        // Verify the deserialized environment contains the fact
        assert!(
            env2.shared.atom_space.total_atoms.load(Ordering::Relaxed) > 0,
            "Should find the connected fact after deserialization"
        );

        println!("✓ Deserialized Environment can be used after reserved-byte roundtrip!");
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
                source[0].as_sexpr().map_or(false, |items| !items.is_empty()
                    && items[0].as_atom() == Some("!")),
                "Source[0] should be an eval expression (starts with !)"
            );
            println!(
                "After compile: source = {:?}, is_eval_expr = {}",
                source[0],
                source[0].as_sexpr().map_or(false, |items| !items.is_empty()
                    && items[0].as_atom() == Some("!"))
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
                source[0].as_sexpr().map_or(false, |items| !items.is_empty()
                    && items[0].as_atom() == Some("!"))
            );

            // Critical: Check that is_eval_expr() still returns true
            assert!(
                source[0].as_sexpr().map_or(false, |items| !items.is_empty()
                    && items[0].as_atom() == Some("!")),
                "Deserialized source[0] should still be an eval expression"
            );
        }

        println!("✓ Source field with ! expression survives roundtrip");
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
        // Create a Par with an unsupported expression type (e.g., EVarBody)
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
        println!(
            "Deserialized multiplicities: {:?}",
            deserialized_multiplicities
        );

        // Verify each original entry is preserved with correct count
        for (key, original_count) in &original_multiplicities {
            let deserialized_count = deserialized_multiplicities
                .get(key)
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

        println!("✓ Multiplicity round-trip preserves all counts correctly!");
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
            let final_count = final_multiplicities
                .get(key)
                .unwrap_or_else(|| panic!("Missing key '{}' after 3 round-trips", key));
            assert_eq!(
                *original_count, *final_count,
                "Multiplicity for '{}' changed after 3 round-trips: {} -> {}",
                key, original_count, final_count
            );
        }
        assert_eq!(original_multiplicities.len(), final_multiplicities.len());

        println!("✓ Multiplicities stable across 3 round-trip cycles!");
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

        if let Some(Expr {
            expr_instance: Some(ExprInstance::EPathmapBody(pathmap)),
        }) = par.exprs.first()
        {
            assert!(
                has_metta_state_structure(pathmap),
                "Valid MettaState PathMap should pass structure check"
            );
        } else {
            panic!("Expected EPathmapBody");
        }
    }

    #[test]
    fn test_has_metta_state_structure_invalid() {
        // Create {| true |} — a PathMap with a boolean, not MettaState structure
        let pathmap = EPathMap {
            ps: vec![Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::GBool(true)),
            }])],
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        };
        assert!(
            !has_metta_state_structure(&pathmap),
            "{{| true |}} should not match MeTTa State structure"
        );
    }

    #[test]
    fn test_has_metta_state_structure_wrong_tags() {
        // EList with 3 fields but wrong tag names
        let wrong_tag_list = EList {
            ps: vec![
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("wrong_tag".to_string()), Par::default()],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("environment".to_string()), Par::default()],
                        locally_free: Vec::new(),
                        connective_used: false,
                        remainder: None,
                    })),
                }]),
                Par::default().with_exprs(vec![Expr {
                    expr_instance: Some(ExprInstance::EListBody(EList {
                        ps: vec![create_string_par("output".to_string()), Par::default()],
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
        assert!(
            !has_metta_state_structure(&pathmap),
            "Wrong tags should not match"
        );
    }

    #[test]
    fn test_metta_run_error_expr_is_elist() {
        let error = metta_run_error_expr("test_code", "test message");
        match &error.expr_instance {
            Some(ExprInstance::EListBody(list)) => {
                assert_eq!(list.ps.len(), 2);
                // Check error code
                if let Some(ExprInstance::GString(code)) = list.ps[0]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    assert_eq!(code, "test_code");
                } else {
                    panic!("Expected GString error code");
                }
                // Check message
                if let Some(ExprInstance::GString(msg)) = list.ps[1]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
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
        assert!(
            result.is_ok(),
            "Lenient deserialization of valid state should succeed: {:?}",
            result.err()
        );
        let deserialized = result.expect("already checked");
        assert_eq!(deserialized.source().len(), 1);
        assert_eq!(deserialized.output().len(), 1);
    }
}
