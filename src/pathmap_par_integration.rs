/// PathMap Par Integration Module
///
/// Provides conversion between MeTTa types and Rholang PathMap-based Par types.
/// This module enables MettaState to be represented as Rholang EPathMap structures.

use crate::backend::types::{MettaValue, MettaState, Environment};
use models::rhoapi::{Par, Expr, expr::ExprInstance, EPathMap, EList, ETuple};

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
            // Represent atoms as tagged strings: "atom:name"
            create_string_par(format!("atom:{}", s))
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
            create_string_par(s.clone())
        }
        MettaValue::Uri(s) => {
            create_uri_par(s.clone())
        }
        MettaValue::Nil => {
            // Represent Nil as empty Par
            Par::default()
        }
        MettaValue::SExpr(items) => {
            // Convert S-expressions to Rholang lists
            let item_pars: Vec<Par> = items.iter()
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

/// Convert Environment to a Rholang Par object
/// For now, we just store metadata (facts count)
pub fn environment_to_par(env: &Environment) -> Par {
    // Create a simple representation with facts count
    let facts_count = env.rule_cache.len() as i64;

    // Create tuple: ("environment", facts_count)
    let tag_par = create_string_par("environment".to_string());
    let count_par = create_int_par(facts_count);

    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![tag_par, count_par],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }])
}

/// Convert MettaState to a Rholang Par containing an EPathMap
///
/// The EPathMap will contain three elements representing the MettaState fields:
/// - Element 0: pending_exprs (as a tagged tuple)
/// - Element 1: environment (as a tagged tuple)
/// - Element 2: eval_outputs (as a tagged tuple)
pub fn metta_state_to_pathmap_par(state: &MettaState) -> Par {
    let mut elements = Vec::new();

    // Element 0: ("pending_exprs", <list of exprs>)
    let pending_tag = create_string_par("pending_exprs".to_string());
    let pending_list = metta_values_to_list_par(&state.pending_exprs);
    elements.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![pending_tag, pending_list],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]));

    // Element 1: ("environment", <env data>)
    let env_tag = create_string_par("environment".to_string());
    let env_data = environment_to_par(&state.environment);
    elements.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![env_tag, env_data],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]));

    // Element 2: ("eval_outputs", <list of outputs>)
    let outputs_tag = create_string_par("eval_outputs".to_string());
    let outputs_list = metta_values_to_list_par(&state.eval_outputs);
    elements.push(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![outputs_tag, outputs_list],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }]));

    // Create EPathMap with these elements
    let epathmap = EPathMap {
        ps: elements,
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
/// Returns a simple error tuple instead of a PathMap
pub fn metta_error_to_par(error_msg: &str) -> Par {
    let tag_par = create_string_par("error".to_string());
    let msg_par = create_string_par(error_msg.to_string());

    Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::ETupleBody(ETuple {
            ps: vec![tag_par, msg_par],
            locally_free: Vec::new(),
            connective_used: false,
        })),
    }])
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
                // Check if it's a tagged atom
                if let Some(atom_name) = s.strip_prefix("atom:") {
                    Ok(MettaValue::Atom(atom_name.to_string()))
                } else {
                    Ok(MettaValue::String(s.clone()))
                }
            }
            Some(ExprInstance::GInt(n)) => Ok(MettaValue::Long(*n)),
            Some(ExprInstance::GBool(b)) => Ok(MettaValue::Bool(*b)),
            Some(ExprInstance::GUri(u)) => Ok(MettaValue::Uri(u.clone())),
            Some(ExprInstance::EListBody(list)) => {
                // Convert list items back to MettaValues
                let items: Result<Vec<MettaValue>, String> = list.ps.iter()
                    .map(|p| par_to_metta_value(p))
                    .collect();
                Ok(MettaValue::SExpr(items?))
            }
            Some(ExprInstance::ETupleBody(tuple)) => {
                // Check if it's a tagged structure
                if tuple.ps.len() >= 2 {
                    if let Some(ExprInstance::GString(tag)) = tuple.ps[0].exprs.first()
                        .and_then(|e| e.expr_instance.as_ref())
                    {
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
                            _ => Err(format!("Unknown tagged tuple: {}", tag))
                        }
                    } else {
                        Err("Expected tuple with string tag".to_string())
                    }
                } else {
                    Err("Tuple must have at least 2 elements".to_string())
                }
            }
            _ => Err(format!("Unsupported Par expression type for MettaValue conversion"))
        }
    } else {
        Err("Par has no expressions to convert".to_string())
    }
}

/// Convert a Rholang Par containing an EPathMap back to MettaState
pub fn pathmap_par_to_metta_state(par: &Par) -> Result<MettaState, String> {
    // Get the EPathMap from the Par
    if let Some(expr) = par.exprs.first() {
        if let Some(ExprInstance::EPathmapBody(pathmap)) = &expr.expr_instance {
            // Extract the three elements from the PathMap
            if pathmap.ps.len() != 3 {
                return Err(format!("Expected 3 elements in PathMap, got {}", pathmap.ps.len()));
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

            // Extract pending_exprs
            let pending_par = extract_tuple_value(&pathmap.ps[0])?;
            let pending_exprs = if let Some(expr) = pending_par.exprs.first() {
                if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                    let exprs: Result<Vec<MettaValue>, String> = list.ps.iter()
                        .map(|p| par_to_metta_value(p))
                        .collect();
                    exprs?
                } else {
                    return Err("Expected EListBody for pending_exprs".to_string());
                }
            } else {
                Vec::new()
            };

            // Extract environment (just use empty for now since compile returns empty env)
            let environment = Environment::new();

            // Extract eval_outputs
            let outputs_par = extract_tuple_value(&pathmap.ps[2])?;
            let eval_outputs = if let Some(expr) = outputs_par.exprs.first() {
                if let Some(ExprInstance::EListBody(list)) = &expr.expr_instance {
                    let outputs: Result<Vec<MettaValue>, String> = list.ps.iter()
                        .map(|p| par_to_metta_value(p))
                        .collect();
                    outputs?
                } else {
                    return Err("Expected EListBody for eval_outputs".to_string());
                }
            } else {
                Vec::new()
            };

            Ok(MettaState {
                pending_exprs,
                environment,
                eval_outputs,
            })
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

    #[test]
    fn test_metta_value_atom_to_par() {
        let atom = MettaValue::Atom("test".to_string());
        let par = metta_value_to_par(&atom);

        // Should be a string Par
        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::GString(s)) = &par.exprs[0].expr_instance {
            assert_eq!(s, "atom:test");
        } else {
            panic!("Expected GString");
        }
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
            // Should have 3 elements (pending_exprs, environment, eval_outputs)
            assert_eq!(pathmap.ps.len(), 3);
        } else {
            panic!("Expected EPathmapBody");
        }
    }

    #[test]
    fn test_metta_error_to_par() {
        let par = metta_error_to_par("test error");

        assert_eq!(par.exprs.len(), 1);
        if let Some(ExprInstance::ETupleBody(tuple)) = &par.exprs[0].expr_instance {
            assert_eq!(tuple.ps.len(), 2);
        } else {
            panic!("Expected ETupleBody");
        }
    }
}
