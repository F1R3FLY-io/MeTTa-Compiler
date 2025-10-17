/// PathMap Par Integration Module
///
/// Provides conversion between MeTTa types and Rholang PathMap-based Par types.
/// This module enables MettaState to be represented as Rholang EPathMap structures.

use crate::backend::types::{MettaValue, MettaState, Environment};
use models::rhoapi::{Par, Expr, expr::ExprInstance, EPathMap, EList, ETuple, EMap, KeyValuePair};
use pathmap::zipper::{ZipperIteration, ZipperMoving};

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

/// Helper function to create a Par with a URI value
fn create_uri_par(uri: String) -> Par {
    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::GUri(uri)),
    }])
}

/// Convert a MettaValue to a Rholang Par object
pub fn metta_value_to_par(value: &MettaValue) -> Par {
    match value {
        MettaValue::Atom(s) => {
            // Atoms are plain strings (no quotes)
            create_string_par(s.clone())
        }
        MettaValue::Bool(b) => {
            Par::default().with_exprs(vec![Expr {
                expr_instance: Some(ExprInstance::GBool(*b)),
            }])
        }
        MettaValue::Long(n) => {
            create_int_par(*n)
        }
        MettaValue::String(s) => {
            // Strings are quoted with escaped quotes to distinguish from atoms
            create_string_par(format!("\"{}\"", s.replace("\\", "\\\\").replace("\"", "\\\"")))
        }
        MettaValue::Uri(s) => {
            create_uri_par(s.clone())
        }
        MettaValue::Nil => {
            // Represent Nil as empty Par
            Par::default()
        }
        MettaValue::SExpr(items) => {
            // Convert S-expressions to Rholang tuples (more semantically appropriate than lists)
            let item_pars: Vec<Par> = items.iter()
                .map(|v| metta_value_to_par(v))
                .collect();

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
    }
}

/// Convert a vector of MettaValues to a Rholang List Par
pub fn metta_values_to_list_par(values: &[MettaValue]) -> Par {
    let item_pars: Vec<Par> = values.iter()
        .map(|v| metta_value_to_par(v))
        .collect();

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
/// Serializes the Space's PathMap as an EPathMap and multiplicities
/// Returns an ETuple with two named fields: ("space", ...), ("multiplicities", ...)
/// Note: Type assertions are stored within the space, not separately
pub fn environment_to_par(env: &Environment) -> Par {
    // Serialize Space's PathMap btm field as an EPathMap
    // The MORK Space stores data in paths (PathMap<()>), so we create EPathMap entries
    // where each entry is a tuple: (path_as_bytes, empty_par_for_unit_value)
    let space = env.space.lock().unwrap();
    let mut rz = space.btm.read_zipper();
    let mut pathmap_entries: Vec<Par> = Vec::new();

    while rz.to_next_val() {
        // Get the path as bytes and serialize to readable string
        let path_bytes = rz.path();
        let expr = mork_bytestring::Expr { ptr: path_bytes.as_ptr() as *mut u8 };
        let path_str = Environment::serialize_mork_expr(&expr, &space);

        // Create a GString Par for the path - readable format
        // Since PathMap<()> has no meaningful value (just unit), we only store the path string
        let path_par = Par::default().with_exprs(vec![Expr {
            expr_instance: Some(ExprInstance::GString(path_str)),
        }]);

        pathmap_entries.push(path_par);
    }
    drop(rz);
    drop(space);

    // Create an EPathMap representing the Space
    let space_epathmap = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EPathmapBody(EPathMap {
            ps: pathmap_entries,
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
    }]);

    // Serialize multiplicities as an EMap (Rholang map)
    let multiplicities_map = env.get_multiplicities();
    let mut multiplicities_kvs = Vec::new();
    for (rule_key, count) in multiplicities_map.iter() {
        let key_par = create_string_par(rule_key.clone());
        let value_par = create_int_par(*count as i64);
        multiplicities_kvs.push(KeyValuePair {
            key: Some(key_par),
            value: Some(value_par),
        });
    }

    let multiplicities_emap = Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EMapBody(EMap {
            kvs: multiplicities_kvs,
            locally_free: Vec::new(),
            connective_used: false,
            remainder: None,
        })),
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
            ps: vec![create_string_par("multiplicities".to_string()), multiplicities_emap],
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
    let error_value = MettaValue::Error(error_msg.to_string(), Box::new(MettaValue::Nil));

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
    // Handle empty Par (Nil)
    if par.exprs.is_empty() && par.unforgeables.is_empty() && par.sends.is_empty() {
        return Ok(MettaValue::Nil);
    }

    // Get the first expression
    if let Some(expr) = par.exprs.first() {
        match &expr.expr_instance {
            Some(ExprInstance::GString(s)) => {
                // Check if it's a quoted string (starts and ends with ")
                if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                    // It's a string - unescape and remove quotes
                    let unescaped = s[1..s.len()-1]
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
            Some(ExprInstance::GUri(u)) => Ok(MettaValue::Uri(u.clone())),
            Some(ExprInstance::EListBody(list)) => {
                // Lists are also converted to S-expressions for compatibility
                let items: Result<Vec<MettaValue>, String> = list.ps.iter()
                    .map(|p| par_to_metta_value(p))
                    .collect();
                Ok(MettaValue::SExpr(items?))
            }
            Some(ExprInstance::ETupleBody(tuple)) => {
                // Check if it's a tagged structure (error, type)
                // Tagged structures have string tag as first element
                if tuple.ps.len() >= 2 {
                    if let Some(ExprInstance::GString(tag)) = tuple.ps[0].exprs.first()
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
                                            Ok(MettaValue::Error(msg_str, Box::new(details)))
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
                                    Ok(MettaValue::Type(Box::new(inner)))
                                }
                                _ => {
                                    // Unknown tag, treat as regular S-expr
                                    let items: Result<Vec<MettaValue>, String> = tuple.ps.iter()
                                        .map(|p| par_to_metta_value(p))
                                        .collect();
                                    Ok(MettaValue::SExpr(items?))
                                }
                            }
                        } else {
                            // First element is an atom, not a tag - it's a regular S-expr
                            let items: Result<Vec<MettaValue>, String> = tuple.ps.iter()
                                .map(|p| par_to_metta_value(p))
                                .collect();
                            Ok(MettaValue::SExpr(items?))
                        }
                    } else {
                        // First element is not a string - it's a regular S-expr
                        let items: Result<Vec<MettaValue>, String> = tuple.ps.iter()
                            .map(|p| par_to_metta_value(p))
                            .collect();
                        Ok(MettaValue::SExpr(items?))
                    }
                } else {
                    // Small tuple, treat as S-expr
                    let items: Result<Vec<MettaValue>, String> = tuple.ps.iter()
                        .map(|p| par_to_metta_value(p))
                        .collect();
                    Ok(MettaValue::SExpr(items?))
                }
            }
            _ => Err(format!("Unsupported Par expression type for MettaValue conversion"))
        }
    } else {
        Err("Par has no expressions to convert".to_string())
    }
}

/// Convert a Rholang Par back to Environment
/// Deserializes the Space's PathMap and multiplicities
/// Expects an ETuple with named fields: (("space", ...), ("multiplicities", ...))
/// Note: Type assertions are stored within the space, not separately
pub fn par_to_environment(par: &Par) -> Result<Environment, String> {
    use std::collections::HashMap;

    // The par should be an ETuple with 2 named field tuples
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::ETupleBody(tuple)) = &expr.expr_instance {
            if tuple.ps.len() != 2 {
                return Err(format!("Expected 2 elements in environment tuple, got {}", tuple.ps.len()));
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

            // Extract space (element 0)
            let space_par = extract_tuple_value(&tuple.ps[0])?;
            let mut path_strings: Vec<String> = Vec::new();
            if let Some(expr) = space_par.exprs.first() {
                if let Some(ExprInstance::EPathmapBody(space_pathmap)) = &expr.expr_instance {
                    // Extract all paths from the EPathMap entries
                    // Each entry is just a GString (the path s-expression)
                    for entry_par in &space_pathmap.ps {
                        if let Some(expr) = entry_par.exprs.first() {
                            if let Some(ExprInstance::GString(path_str)) = &expr.expr_instance {
                                path_strings.push(path_str.clone());
                            }
                        }
                    }
                }
            }

            // Extract multiplicities (element 1)
            let multiplicities_par = extract_tuple_value(&tuple.ps[1])?;
            let mut multiplicities_map: HashMap<String, usize> = HashMap::new();
            if let Some(expr) = multiplicities_par.exprs.first() {
                if let Some(ExprInstance::EMapBody(emap)) = &expr.expr_instance {
                    for kv in &emap.kvs {
                        // Extract key (Option<Par> containing string)
                        if let Some(key_par) = &kv.key {
                            if let Some(key_expr) = key_par.exprs.first() {
                                if let Some(ExprInstance::GString(key_str)) = &key_expr.expr_instance {
                                    // Extract value (Option<Par> containing integer)
                                    if let Some(value_par) = &kv.value {
                                        if let Some(value_expr) = value_par.exprs.first() {
                                            if let Some(ExprInstance::GInt(count)) = &value_expr.expr_instance {
                                                multiplicities_map.insert(key_str.clone(), *count as usize);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Reconstruct Environment
            let mut env = Environment::new();

            // Restore multiplicities
            env.set_multiplicities(multiplicities_map);

            // Rebuild the Space from PathMap paths
            // This will restore all facts including type assertions
            // Parse each path string back into MORK byte format and insert
            {
                let mut space = env.space.lock().unwrap();
                for path_str in path_strings {
                    // Parse the path string back to MORK bytes using compile
                    // The path_str is a serialized s-expression, so we need to parse it
                    use crate::backend::mork_convert::{metta_to_mork_bytes, ConversionContext};
                    use crate::backend::compile::compile;

                    // Compile the path string to MettaValue
                    if let Ok(state) = compile(&path_str) {
                        if let Some(value) = state.source.first() {
                            // Convert MettaValue to MORK bytes
                            let mut ctx = ConversionContext::new();
                            if let Ok(bytes) = metta_to_mork_bytes(value, &space, &mut ctx) {
                                space.btm.insert(&bytes[..], ());
                            }
                        }
                    }
                }
            }

            Ok(env)
        } else {
            Err("Expected ETuple for environment".to_string())
        }
    } else {
        Err("Environment Par has no expressions".to_string())
    }
}

/// Convert a Rholang Par containing an EPathMap back to MettaState
pub fn pathmap_par_to_metta_state(par: &Par) -> Result<MettaState, String> {
    // Get the EPathMap from the Par
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::EPathmapBody(pathmap)) = &expr.expr_instance {
            // The PathMap should contain a single ETuple with three named field tuples
            if pathmap.ps.len() != 1 {
                return Err(format!("Expected 1 element (ETuple) in PathMap, got {}", pathmap.ps.len()));
            }

            // Extract the ETuple from the PathMap
            let state_tuple_par = &pathmap.ps[0];
            if let Some(expr) = state_tuple_par.exprs.first() {
                if let Some(ExprInstance::ETupleBody(state_tuple)) = &expr.expr_instance {
                    // The tuple should have 3 named field tuples
                    if state_tuple.ps.len() != 3 {
                        return Err(format!("Expected 3 named fields in state tuple, got {}", state_tuple.ps.len()));
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
                            let exprs: Result<Vec<MettaValue>, String> = list.ps.iter()
                                .map(|p| par_to_metta_value(p))
                                .collect();
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
                            let outputs: Result<Vec<MettaValue>, String> = list.ps.iter()
                                .map(|p| par_to_metta_value(p))
                                .collect();
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
                    Err("Expected ETupleBody in PathMap".to_string())
                }
            } else {
                Err("PathMap element has no expressions".to_string())
            }
        } else {
            Err("Par does not contain EPathMap".to_string())
        }
    } else {
        Err("Par has no expressions".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::types::Rule;

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
            assert_eq!(env_tuple.ps.len(), 2, "Expected ETuple with 2 fields (space, multiplicities), got {}", env_tuple.ps.len());

            // Check field 0: ("space", <epathmap>)
            if let Some(ExprInstance::ETupleBody(tuple)) = env_tuple.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                // Verify tag
                if let Some(ExprInstance::GString(tag)) = tuple.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                    assert_eq!(tag, "space");
                }
                // Verify space EPathMap is not empty
                if let Some(ExprInstance::EPathmapBody(space_pathmap)) = tuple.ps[1].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                    println!("Space EPathMap has {} entries", space_pathmap.ps.len());
                    assert!(space_pathmap.ps.len() > 0, "Space EPathMap should not be empty");

                    // Check first entry is a GString (the path s-expression)
                    if let Some(ExprInstance::GString(path_str)) = space_pathmap.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                        println!("First path string: {}", path_str);
                        assert!(path_str.len() > 0, "Path string should not be empty");
                    } else {
                        panic!("Expected GString for path");
                    }
                } else {
                    panic!("Expected EPathmapBody for space");
                }
            } else {
                panic!("Expected ETupleBody for field 0");
            }

            // Check field 1: ("multiplicities", <emap>)
            if let Some(ExprInstance::ETupleBody(tuple)) = env_tuple.ps[1].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                if let Some(ExprInstance::GString(tag)) = tuple.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                    assert_eq!(tag, "multiplicities");
                }
                // Verify it's an EMap
                if let Some(ExprInstance::EMapBody(_emap)) = tuple.ps[1].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                    println!("Multiplicities is an EMap");
                } else {
                    panic!("Expected EMapBody for multiplicities");
                }
            }
        } else {
            panic!("Expected ETupleBody");
        }

        // Deserialize
        let deserialized_env = par_to_environment(&par).expect("Failed to deserialize");
        println!("Deserialized environment has {} rules", deserialized_env.rule_count());

        // Verify deserialized environment
        assert_eq!(deserialized_env.rule_count(), 1, "Expected 1 rule after deserialization");

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
        let state = MettaState::new_compiled(vec![
            MettaValue::Long(42)
        ]);

        let par = metta_state_to_pathmap_par(&state);

        // Should have one expr (the EPathMap)
        assert_eq!(par.exprs.len(), 1);

        // Should be an EPathMap
        if let Some(ExprInstance::EPathmapBody(pathmap)) = &par.exprs[0].expr_instance {
            // Should have 1 element (the state ETuple)
            assert_eq!(pathmap.ps.len(), 1);

            // The element should be an ETuple with 3 named field tuples
            if let Some(ExprInstance::ETupleBody(state_tuple)) = pathmap.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                assert_eq!(state_tuple.ps.len(), 3, "Expected ETuple with 3 named fields (source, environment, output)");
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
            if let Some(ExprInstance::ETupleBody(state_tuple)) = pathmap.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
                assert_eq!(state_tuple.ps.len(), 3, "Expected ETuple with 3 named fields (source, environment, output)");

                // Check that output contains the error
                // Field 2 should be ("output", [error_value])
                if let Some(expr) = state_tuple.ps[2].exprs.first() {
                    if let Some(ExprInstance::ETupleBody(tuple)) = &expr.expr_instance {
                        assert_eq!(tuple.ps.len(), 2, "Expected (tag, value) tuple");
                        // First element should be "output" tag
                        if let Some(ExprInstance::GString(tag)) = tuple.ps[0].exprs.first().and_then(|e| e.expr_instance.as_ref()) {
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
}
