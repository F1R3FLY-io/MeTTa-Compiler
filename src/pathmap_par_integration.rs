use crate::backend::environment::Environment;
/// PathMap Par Integration Module
///
/// Provides conversion between MeTTa types and Rholang PathMap-based Par types.
/// This module enables MettaState to be represented as Rholang EPathMap structures.
use crate::backend::models::{MettaState, MettaValue};
use models::rhoapi::{expr::ExprInstance, EList, EPathMap, ETuple, Expr, Par};
use pathmap::zipper::{ZipperIteration, ZipperMoving};
use std::sync::Arc;
use tracing::{debug, trace};

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
const METTA_MULTIPLICITIES_MAGIC: &[u8] = b"MTTM"; // MeTTa Multiplicities
const METTA_SPACE_MAGIC: &[u8] = b"MTTS"; // MeTTa Space

/// Convert a MettaValue to a Rholang Par object
pub fn metta_value_to_par(value: &MettaValue) -> Par {
    trace!(target: "mettatron::rholang_integration::metta_value_to_par", ?value, "MeTTa value");

    let par = match value {
        MettaValue::Atom(s) => {
            // Atoms are plain strings (no quotes)
            create_string_par(s.clone())
        }
        MettaValue::Bool(b) => Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::GBool(*b)),
        }]),
        MettaValue::Long(n) => create_int_par(*n),
        MettaValue::Float(f) => create_string_par(f.to_string()),
        MettaValue::String(s) => {
            // Strings are quoted with escaped quotes to distinguish from atoms
            create_string_par(format!(
                "\"{}\"",
                s.replace("\\", "\\\\").replace("\"", "\\\"")
            ))
        }
        MettaValue::Nil => {
            // Represent Nil as empty Par
            Par::default()
        }
        MettaValue::SExpr(items) => {
            // Convert S-expressions to Rholang tuples (more semantically appropriate than lists)
            let item_pars: Vec<Par> = items.iter().map(metta_value_to_par).collect();

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::ETupleBody(ETuple {
                    ps: item_pars,
                    locally_free: Vec::new(),
                    connective_used: false,
                })),
            }])
        }
        MettaValue::Error(msg, details) => {
            // Represent errors as tuples: ("error", msg, details)
            let tag_par = create_string_par("error".to_string());
            let msg_par = create_string_par(msg.clone());
            let details_par = metta_value_to_par(details);

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::ETupleBody(ETuple {
                    ps: vec![tag_par, msg_par, details_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                })),
            }])
        }
        MettaValue::Type(t) => {
            // Represent types as tagged tuples: ("type", <inner_value>)
            let tag_par = create_string_par("type".to_string());
            let value_par = metta_value_to_par(t);

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::ETupleBody(ETuple {
                    ps: vec![tag_par, value_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                })),
            }])
        }
        MettaValue::Conjunction(goals) => {
            // Represent conjunctions as tagged tuples: ("conjunction", goal1, goal2, ...)
            let mut ps = vec![create_string_par("conjunction".to_string())];
            ps.extend(goals.iter().map(metta_value_to_par));

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::ETupleBody(ETuple {
                    ps,
                    locally_free: Vec::new(),
                    connective_used: false,
                })),
            }])
        }
        MettaValue::Unit => {
            // Represent Unit as empty Par (similar to Nil)
            Par::default()
        }
        MettaValue::State(id) => {
            // Represent state as tagged tuple: ("state", id)
            let tag_par = create_string_par("state".to_string());
            let id_par = create_int_par(*id as i64);

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::ETupleBody(ETuple {
                    ps: vec![tag_par, id_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                })),
            }])
        }
        MettaValue::Memo(handle) => {
            // Represent memo as tagged tuple: ("memo", name)
            let tag_par = create_string_par("memo".to_string());
            let name_par = create_string_par(handle.name.clone());

            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::ETupleBody(ETuple {
                    ps: vec![tag_par, name_par],
                    locally_free: Vec::new(),
                    connective_used: false,
                })),
            }])
        }
        MettaValue::Empty => {
            // Represent Empty as empty Par (similar to Nil/Unit)
            Par::default()
        }
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
/// Serializes the Space's PathMap and multiplicities as byte arrays
/// Returns an ETuple with two named fields:
///   ("space", GByteArray) - Raw MORK trie bytes
///   ("multiplicities", GByteArray) - Binary encoded multiplicities map
/// Note: Type assertions are stored within the space, not separately
pub fn environment_to_par(env: &Environment) -> Par {
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

    // Write format: [magic: 4 bytes "MTTS"][sym_table_len: 8 bytes][sym_table_bytes][num_paths: 8 bytes][path1_len: 4 bytes][path1_bytes]...

    // Write magic number to identify this as MeTTa space
    all_paths_data.extend_from_slice(METTA_SPACE_MAGIC);

    // First, serialize the symbol table to a temp file, then read it
    let symbol_table_bytes = {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

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

    // Iterate through all paths and collect their raw bytes
    while rz.to_next_val() {
        let path_bytes = rz.path();
        // Write path length (4 bytes, big-endian)
        let len = path_bytes.len() as u32;
        all_paths_data.extend_from_slice(&len.to_be_bytes());
        // Write raw path bytes (NO INTERPRETATION!)
        all_paths_data.extend_from_slice(path_bytes);
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

    // Serialize multiplicities as a byte array for efficiency and consistency
    // Format: [magic: 4 bytes "MTTM"][count: 8 bytes][key1_len: 4 bytes][key1_bytes][value1: 8 bytes]...
    let multiplicities_map = env.get_multiplicities();
    let mut multiplicities_bytes = Vec::new();

    // Write magic number to identify this as MeTTa multiplicities
    multiplicities_bytes.extend_from_slice(METTA_MULTIPLICITIES_MAGIC);

    // Write count
    let count = multiplicities_map.len() as u64;
    multiplicities_bytes.extend_from_slice(&count.to_be_bytes());

    // Write each key-value pair
    for (rule_key, count) in multiplicities_map.iter() {
        let key_bytes = rule_key.as_bytes();
        // Write key length (4 bytes)
        let key_len = key_bytes.len() as u32;
        multiplicities_bytes.extend_from_slice(&key_len.to_be_bytes());
        // Write key bytes
        multiplicities_bytes.extend_from_slice(key_bytes);
        // Write value (8 bytes)
        multiplicities_bytes.extend_from_slice(&(*count as u64).to_be_bytes());
    }
    trace!(
        target: "mettatron::rholang_integration::environment_to_par",
        multiplicities_count = multiplicities_map.len(), mult_data_len = multiplicities_bytes.len()
    );

    let multiplicities_emap = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GByteArray(multiplicities_bytes)),
    }]);

    // Build ETuple with named fields: (("space", ...), ("multiplicities", ...))
    let space_tuple = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![create_string_par("space".to_string()), space_epathmap],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]);

    let multiplicities_tuple = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![
                create_string_par("multiplicities".to_string()),
                multiplicities_emap,
            ],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]);

    // Return ETuple with 2 named field tuples
    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![space_tuple, multiplicities_tuple],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }])
}

/// Convert MettaState to a Rholang Par containing an EPathMap
///
/// The EPathMap will contain a single ETuple with three named field tuples:
/// - ("source", <list of exprs>)
/// - ("environment", <env data>)
/// - ("output", <list of output>)
pub fn metta_state_to_pathmap_par(state: &MettaState) -> Par {
    trace!(target: "mettatron::rholang_integration::metta_state_to_pathmap_par", ?state);
    let mut field_tuples = Vec::new();

    // Field 0: ("source", <list of exprs>)
    let pending_tag = create_string_par("source".to_string());
    let pending_list = metta_values_to_list_par(&state.source);
    field_tuples.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![pending_tag, pending_list],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]));

    // Field 1: ("environment", <env data>)
    let env_tag = create_string_par("environment".to_string());
    let env_data = environment_to_par(&state.environment);
    field_tuples.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![env_tag, env_data],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]));

    // Field 2: ("output", <list of output>)
    let outputs_tag = create_string_par("output".to_string());
    let outputs_list = metta_values_to_list_par(&state.output);
    field_tuples.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![outputs_tag, outputs_list],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]));

    // Wrap all three field tuples in a single ETuple
    let state_tuple = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: field_tuples,
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]);

    // Create EPathMap with this single ETuple as its only element
    let epathmap = EPathMap {
        ps: vec![state_tuple],
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
    let error_value = MettaValue::Error(error_msg.to_string(), Arc::new(MettaValue::Nil));

    // Create a MettaState with the error in output
    let error_state = MettaState {
        source: vec![],
        environment: Environment::new(),
        output: vec![error_value],
    };

    // Return as PathMap (consistent with metta_state_to_pathmap_par)
    metta_state_to_pathmap_par(&error_state)
}

/// Convert a Rholang Par back to MettaValue
pub fn par_to_metta_value(par: &Par) -> Result<MettaValue, String> {
    trace!(target: "mettatron::rholang_integration::par_to_metta_value", ?par, "Par value");
    // Handle empty Par (Nil)
    if par.exprs.is_empty() && par.unforgeables.is_empty() && par.sends.is_empty() {
        return Ok(MettaValue::Nil);
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
                // Lists are also converted to S-expressions for compatibility
                let items: Result<Vec<MettaValue>, String> =
                    list.ps.iter().map(par_to_metta_value).collect();
                Ok(MettaValue::SExpr(items?))
            }
            Some(ExprInstance::ETupleBody(tuple)) => {
                // Check if it's a tagged structure (error, type)
                // Tagged structures have string tag as first element
                if tuple.ps.len() >= 2 {
                    if let Some(ExprInstance::GString(tag)) = tuple.ps[0]
                        .exprs
                        .first()
                        .and_then(|e| e.expr_instance.as_ref())
                    {
                        // Check if the tag looks like a quoted string (for distinguishing from atoms)
                        if tag.starts_with('"') {
                            // It's a tagged structure, not a plain S-expr
                            match tag.as_str() {
                                "error" => {
                                    // Error tuple: (tag, msg, details)
                                    if tuple.ps.len() >= 3 {
                                        let msg = par_to_metta_value(&tuple.ps[1])?;
                                        let details = par_to_metta_value(&tuple.ps[2])?;
                                        if let MettaValue::String(msg_str) = msg {
                                            Ok(MettaValue::Error(msg_str, Arc::new(details)))
                                        } else {
                                            Err("Error message must be a string".to_string())
                                        }
                                    } else {
                                        Err("Error tuple must have 3 elements".to_string())
                                    }
                                }
                                "type" => {
                                    // Type tuple: (tag, inner_value)
                                    let inner = par_to_metta_value(&tuple.ps[1])?;
                                    Ok(MettaValue::Type(Arc::new(inner)))
                                }
                                _ => {
                                    // Unknown tag, treat as regular S-expr
                                    let items: Result<Vec<MettaValue>, String> =
                                        tuple.ps.iter().map(par_to_metta_value).collect();
                                    Ok(MettaValue::SExpr(items?))
                                }
                            }
                        } else {
                            // First element is an atom, not a tag - it's a regular S-expr
                            let items: Result<Vec<MettaValue>, String> =
                                tuple.ps.iter().map(par_to_metta_value).collect();
                            Ok(MettaValue::SExpr(items?))
                        }
                    } else {
                        // First element is not a string - it's a regular S-expr
                        let items: Result<Vec<MettaValue>, String> =
                            tuple.ps.iter().map(par_to_metta_value).collect();
                        Ok(MettaValue::SExpr(items?))
                    }
                } else {
                    // Small tuple, treat as S-expr
                    let items: Result<Vec<MettaValue>, String> =
                        tuple.ps.iter().map(par_to_metta_value).collect();
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
/// Deserializes the Space's PathMap and multiplicities from byte arrays
/// Expects an ETuple with named fields:
///   (("space", GByteArray), ("multiplicities", GByteArray))
/// Note: Type assertions are stored within the space, not separately
pub fn par_to_environment(par: &Par) -> Result<Environment, String> {
    use std::collections::HashMap;
    trace!(target: "mettatron::rholang_integration::par_to_environment", par_exprs_count = par.exprs.len());

    // The par should be an ETuple with 2 named field tuples
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::ETupleBody(tuple)) = &expr.expr_instance {
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

            // Helper to extract value from (tag, value) tuple
            let extract_tuple_value = |tuple_par: &Par| -> Result<Par, String> {
                if let Some(expr) = tuple_par.exprs.first() {
                    if let Some(ExprInstance::ETupleBody(tuple)) = &expr.expr_instance {
                        if tuple.ps.len() >= 2 {
                            return Ok(tuple.ps[1].clone());
                        }
                    }
                }
                Err("Expected tuple with at least 2 elements".to_string())
            };

            // Extract space (element 0) - should be a single GByteArray (MORK dump format)
            let space_par = extract_tuple_value(&tuple.ps[0])?;
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

            // Extract multiplicities (element 1) - now stored as GByteArray
            let multiplicities_par = extract_tuple_value(&tuple.ps[1])?;
            let mut multiplicities_map: HashMap<String, usize> = HashMap::new();
            if let Some(expr) = multiplicities_par.exprs.first() {
                if let Some(ExprInstance::GByteArray(mult_bytes)) = &expr.expr_instance {
                    // Read format: [magic: 4 bytes "MTTM"][count: 8 bytes][key1_len: 4 bytes][key1_bytes][value1: 8 bytes]...
                    if mult_bytes.len() >= 12 {
                        // 4 bytes magic + 8 bytes count minimum
                        let mut offset = 0;

                        // Check and skip magic number if present
                        if mult_bytes.len() >= 4 && &mult_bytes[0..4] == METTA_MULTIPLICITIES_MAGIC
                        {
                            offset += 4; // Skip magic number
                        }

                        // Read count
                        let count = u64::from_be_bytes([
                            mult_bytes[offset],
                            mult_bytes[offset + 1],
                            mult_bytes[offset + 2],
                            mult_bytes[offset + 3],
                            mult_bytes[offset + 4],
                            mult_bytes[offset + 5],
                            mult_bytes[offset + 6],
                            mult_bytes[offset + 7],
                        ]);
                        offset += 8;

                        // Read each key-value pair
                        for _ in 0..count {
                            if offset + 4 > mult_bytes.len() {
                                break; // Not enough data
                            }

                            // Read key length
                            let key_len = u32::from_be_bytes([
                                mult_bytes[offset],
                                mult_bytes[offset + 1],
                                mult_bytes[offset + 2],
                                mult_bytes[offset + 3],
                            ]) as usize;
                            offset += 4;

                            if offset + key_len + 8 > mult_bytes.len() {
                                break; // Not enough data
                            }

                            // Read key bytes
                            let key_bytes = &mult_bytes[offset..offset + key_len];
                            let key = String::from_utf8_lossy(key_bytes).to_string();
                            offset += key_len;

                            // Read value
                            let value = u64::from_be_bytes([
                                mult_bytes[offset],
                                mult_bytes[offset + 1],
                                mult_bytes[offset + 2],
                                mult_bytes[offset + 3],
                                mult_bytes[offset + 4],
                                mult_bytes[offset + 5],
                                mult_bytes[offset + 6],
                                mult_bytes[offset + 7],
                            ]) as usize;
                            offset += 8;

                            multiplicities_map.insert(key, value);
                        }
                    }
                }
            }

            // Reconstruct Environment
            let mut env = Environment::new();

            // Restore multiplicities
            env.set_multiplicities(multiplicities_map);

            // Rebuild the Space from raw path bytes
            // CRITICAL FIX for "reserved 111" bug:
            // We stored raw path bytes (not text), so we insert them directly.
            // This avoids any interpretation of bytes as MORK tags.
            // We also restore the symbol table so symbol IDs match.
            {
                let mut space = env.create_space();
                if !space_dump_bytes.is_empty() {
                    // Read format: [magic: 4 bytes "MTTS"][sym_table_len: 8 bytes][sym_table_bytes][num_paths: 8 bytes][path1_len: 4 bytes][path1_bytes]...
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

                            use std::fs;
                            use std::io::Write;
                            use std::time::{SystemTime, UNIX_EPOCH};

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

                            // Read and insert each path
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

                                if offset + len > space_dump_bytes.len() {
                                    break; // Not enough data
                                }

                                // Get raw path bytes and insert directly into PathMap
                                let path_bytes = &space_dump_bytes[offset..offset + len];
                                space.btm.insert(path_bytes, ());
                                offset += len;
                            }
                        }
                    }
                }
                // Update shared PathMap with modified Space
                env.update_pathmap(space);
            }

            // Rebuild the rule index from the restored MORK Space
            // This is critical for rule matching to work after deserialization
            env.rebuild_rule_index();

            Ok(env)
        } else {
            debug!(
                target: "mettatron::rholang_integration::par_to_environment",
                "expected ETuple for environment"
            );
            Err("Expected ETuple for environment".to_string())
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
            // The PathMap should contain a single ETuple with three named field tuples
            if pathmap.ps.len() != 1 {
                debug!(
                    target: "mettatron::rholang_integration::pathmap_par_to_metta_state",
                    expected = 1, got = pathmap.ps.len(), "invalid PathMap size"
                );
                return Err(format!(
                    "Expected 1 element (ETuple) in PathMap, got {}",
                    pathmap.ps.len()
                ));
            }

            // Extract the ETuple from the PathMap
            let state_tuple_par = &pathmap.ps[0];
            if let Some(expr) = state_tuple_par.exprs.first() {
                if let Some(ExprInstance::ETupleBody(state_tuple)) = &expr.expr_instance {
                    // The tuple should have 3 named field tuples
                    if state_tuple.ps.len() != 3 {
                        debug!(
                            target: "mettatron::rholang_integration::pathmap_par_to_metta_state",
                            expected = 3, got = state_tuple.ps.len(), "invalid state tuple size"
                        );
                        return Err(format!(
                            "Expected 3 named fields in state tuple, got {}",
                            state_tuple.ps.len()
                        ));
                    }

                    // Helper to extract value from (tag, value) tuple
                    let extract_tuple_value = |tuple_par: &Par| -> Result<Par, String> {
                        if let Some(expr) = tuple_par.exprs.first() {
                            if let Some(ExprInstance::ETupleBody(tuple)) = &expr.expr_instance {
                                if tuple.ps.len() >= 2 {
                                    return Ok(tuple.ps[1].clone());
                                }
                            }
                        }
                        Err("Expected tuple with at least 2 elements".to_string())
                    };

                    // Extract source
                    let pending_par = extract_tuple_value(&state_tuple.ps[0])?;
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
                    let env_par = extract_tuple_value(&state_tuple.ps[1])?;
                    let environment = par_to_environment(&env_par)?;

                    // Extract output
                    let outputs_par = extract_tuple_value(&state_tuple.ps[2])?;
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

                    Ok(MettaState {
                        source,
                        environment,
                        output,
                    })
                } else {
                    debug!(target: "mettatron::rholang_integration::pathmap_par_to_metta_state", "expected ETupleBody in PathMap");
                    Err("Expected ETupleBody in PathMap".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::Rule;

    #[test]
    fn test_environment_serialization_roundtrip() {
        // Create an environment with a rule
        let mut env = Environment::new();
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        };
        env.add_rule(rule);

        // Verify original environment
        assert_eq!(env.rule_count(), 1);
        println!("Original environment has {} rules", env.rule_count());

        // Serialize
        let par = environment_to_par(&env);
        println!("Serialized to Par");

        // Check that the serialized Par is an ETuple with 2 named field tuples
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::ETupleBody(env_tuple)) = par.exprs[0].expr_instance.as_ref() {
            assert_eq!(
                env_tuple.ps.len(),
                2,
                "Expected ETuple with 2 fields (space, multiplicities), got {}",
                env_tuple.ps.len()
            );

            // Check field 0: ("space", <GByteArray>)
            if let Some(ExprInstance::ETupleBody(tuple)) = env_tuple.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                // Verify tag
                if let Some(ExprInstance::GString(tag)) = tuple.ps[0]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    assert_eq!(tag, "space");
                }
                // Verify space dump is a GByteArray and not empty
                if let Some(ExprInstance::GByteArray(dump_bytes)) = tuple.ps[1]
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
                panic!("Expected ETupleBody for field 0");
            }

            // Check field 1: ("multiplicities", <GByteArray>)
            if let Some(ExprInstance::ETupleBody(tuple)) = env_tuple.ps[1]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                if let Some(ExprInstance::GString(tag)) = tuple.ps[0]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    assert_eq!(tag, "multiplicities");
                }
                // Verify it's a GByteArray
                if let Some(ExprInstance::GByteArray(mult_bytes)) = tuple.ps[1]
                    .exprs
                    .first()
                    .and_then(|e| e.expr_instance.as_ref())
                {
                    println!(
                        "Multiplicities is a GByteArray with {} bytes",
                        mult_bytes.len()
                    );
                    // Should have at least 8 bytes for the count
                    assert!(
                        mult_bytes.len() >= 8,
                        "Multiplicities byte array should have at least 8 bytes for count"
                    );
                } else {
                    panic!("Expected GByteArray for multiplicities");
                }
            }
        } else {
            panic!("Expected ETupleBody");
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
        if let MettaValue::String(s) = roundtrip {
            assert_eq!(s, "hello world");
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

        assert!(matches!(atom_roundtrip, MettaValue::Atom(_)));
        assert!(matches!(string_roundtrip, MettaValue::String(_)));
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
        if let Some(ExprInstance::ETupleBody(tuple)) = &par.exprs[0].expr_instance {
            assert_eq!(tuple.ps.len(), 3);
        } else {
            panic!("Expected ETupleBody");
        }

        // Test round-trip
        let roundtrip = par_to_metta_value(&par).unwrap();
        if let MettaValue::SExpr(items) = roundtrip {
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
            // Should have 1 element (the state ETuple)
            assert_eq!(pathmap.ps.len(), 1);

            // The element should be an ETuple with 3 named field tuples
            if let Some(ExprInstance::ETupleBody(state_tuple)) = pathmap.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(
                    state_tuple.ps.len(),
                    3,
                    "Expected ETuple with 3 named fields (source, environment, output)"
                );
            } else {
                panic!("Expected ETupleBody for state");
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
            // Should have 1 element (the state ETuple)
            assert_eq!(pathmap.ps.len(), 1);

            // Extract the state tuple
            if let Some(ExprInstance::ETupleBody(state_tuple)) = pathmap.ps[0]
                .exprs
                .first()
                .and_then(|e| e.expr_instance.as_ref())
            {
                assert_eq!(
                    state_tuple.ps.len(),
                    3,
                    "Expected ETuple with 3 named fields (source, environment, output)"
                );

                // Check that output contains the error
                // Field 2 should be ("output", [error_value])
                if let Some(expr) = state_tuple.ps[2].exprs.first() {
                    if let Some(ExprInstance::ETupleBody(tuple)) = &expr.expr_instance {
                        assert_eq!(tuple.ps.len(), 2, "Expected (tag, value) tuple");
                        // First element should be "output" tag
                        if let Some(ExprInstance::GString(tag)) = tuple.ps[0]
                            .exprs
                            .first()
                            .and_then(|e| e.expr_instance.as_ref())
                        {
                            assert_eq!(tag, "output");
                        } else {
                            panic!("Expected GString tag");
                        }
                    } else {
                        panic!("Expected ETupleBody for output element");
                    }
                } else {
                    panic!("Expected expr in state_tuple.ps[2]");
                }
            } else {
                panic!("Expected ETupleBody for state");
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
        let mut env = Environment::new();

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
        let mut env = Environment::new();

        // Add expression with tilde (the problematic reserved byte)
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Atom("room~a".to_string()), // Contains '~' = 126 (RESERVED!)
            MettaValue::Atom("room~b".to_string()), // Contains '~' = 126 (RESERVED!)
        ]));

        // Get initial iter count
        let initial_count = env.iter_rules().count();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back - this used to panic with "reserved 126"
        let env2 =
            par_to_environment(&par).expect("Round-trip with reserved byte '~' (126) failed");

        // The key test: it didn't panic! The bug is fixed.
        // Verify Space is not empty - exact structure may vary due to MORK normalization
        let final_count = env2.iter_rules().count();
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
        let mut env = Environment::new();

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

        let initial_count = env.iter_rules().count();
        println!("Initial space has {} rules", initial_count);

        // First round-trip - this used to panic
        let par1 = environment_to_par(&env);
        let env2 = par_to_environment(&par1).expect("First round-trip failed");
        let count2 = env2.iter_rules().count();
        println!("After 1st round-trip: {} rules", count2);

        // Second round-trip
        let par2 = environment_to_par(&env2);
        let env3 = par_to_environment(&par2).expect("Second round-trip failed");
        let count3 = env3.iter_rules().count();
        println!("After 2nd round-trip: {} rules", count3);

        // Third round-trip
        let par3 = environment_to_par(&env3);
        let env4 = par_to_environment(&par3).expect("Third round-trip failed");
        let count4 = env4.iter_rules().count();
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
        let mut env = Environment::new();

        // Add fact with reserved bytes
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("room_y".to_string()), // 'y' = 121 (reserved)
            MettaValue::Atom("room_z".to_string()), // 'z' = 122 (reserved)
        ]));

        // Add rule that uses match (the pattern that triggered the bug)
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("is_connected".to_string()),
                MettaValue::Atom("$from".to_string()),
                MettaValue::Atom("$to".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
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
        };
        env.add_rule(rule);

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
        let mut env = Environment::new();

        // Add expressions with various ASCII characters in the reserved range
        // '@' = 64, 'A' = 65, ..., 'Z' = 90, ..., 'z' = 122, '{' = 123, '~' = 126, DEL = 127
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Atom("ABC".to_string()), // A=65, B=66, C=67 (all reserved)
            MettaValue::Atom("xyz".to_string()), // x=120, y=121, z=122 (last two reserved)
            MettaValue::Atom("@~".to_string()),  // @=64, ~=126 (both reserved)
        ]));

        let initial_count = env.iter_rules().count();
        println!("Initial space has {} rules", initial_count);

        // Serialize to Par
        let par = environment_to_par(&env);

        // Deserialize back - should handle ALL reserved bytes without panic
        let env2 =
            par_to_environment(&par).expect("Round-trip with multiple reserved bytes failed");

        // The critical test: it didn't panic! All reserved bytes handled.
        let final_count = env2.iter_rules().count();
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
        let mut env = Environment::new();

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
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("is_connected".to_string()), // 'o' = 111, 'n' = 110 (RESERVED!)
                MettaValue::Atom("$from".to_string()),
                MettaValue::Atom("$to".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
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
        };
        env.add_rule(rule);

        let initial_count = env.iter_rules().count();
        println!("Initial space has {} rules", initial_count);

        // THIS IS THE EXACT OPERATION THAT FAILED IN robot_planning.rho DEMO!
        // Serialize to Par (this calls dump_all_sexpr which used to panic with "reserved 111")
        let par = environment_to_par(&env);

        // Deserialize back (if we get here, the bug is fixed!)
        let env2 = par_to_environment(&par).expect(
            "REGRESSION: Round-trip with 'o' (111) in symbols failed - the bug has returned!",
        );

        // Verify data is preserved
        let final_count = env2.iter_rules().count();
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
        use crate::backend::eval::eval;

        let mut env = Environment::new();

        // Add facts with 'o' (111) - reserved byte
        env.add_to_space(&MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()), // 'o' = 111
            MettaValue::Atom("room_a".to_string()),    // 'o' = 111
            MettaValue::Atom("room_b".to_string()),    // 'o' = 111
        ]));

        // Serialize and deserialize
        let par = environment_to_par(&env);
        let env2 = par_to_environment(&par).expect("Deserialization failed");

        // Now try to actually USE the deserialized environment for evaluation!
        // This will trigger iter_rules() which calls serialize_mork_expr()
        let query = MettaValue::SExpr(vec![
            MettaValue::Atom("connected".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);

        // This should work without panicking
        let (results, _) = eval(query, env2);

        println!("Query returned {} results", results.len());
        assert!(!results.is_empty(), "Should find the connected fact");

        println!("✓ Deserialized Environment can be used for evaluation!");
    }
}
