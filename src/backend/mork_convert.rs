//! Conversion utilities between MettaValue and MORK Expr format
//!
//! This module handles the bidirectional conversion needed for query_multi integration:
//! - MettaValue → MORK Expr (for pattern queries)
//! - MORK bindings → HashMap<String, MettaValue> (for pattern match results)

use std::collections::HashMap;
use mork_bytestring::{Expr, ExprZipper, ExprEnv};
use mork::space::Space;
use mork_frontend::bytestring_parser::Parser;
use super::types::MettaValue;

/// Context for tracking variables during MettaValue → Expr conversion
pub struct ConversionContext {
    /// Maps variable names to their De Bruijn indices
    pub var_map: HashMap<String, u8>,
    /// Reverse map: De Bruijn index → variable name
    pub var_names: Vec<String>,
}

impl ConversionContext {
    pub fn new() -> Self {
        ConversionContext {
            var_map: HashMap::new(),
            var_names: Vec::new(),
        }
    }

    /// Get or create a De Bruijn index for a variable
    pub fn get_or_create_var(&mut self, name: &str) -> Result<Option<u8>, String> {
        if let Some(&idx) = self.var_map.get(name) {
            // Variable already exists, return its index
            Ok(Some(idx))
        } else {
            // New variable
            if self.var_names.len() >= 64 {
                return Err("Too many variables (max 64)".to_string());
            }
            let idx = self.var_names.len() as u8;
            self.var_map.insert(name.to_string(), idx);
            self.var_names.push(name.to_string());
            Ok(None) // None means "write NewVar tag"
        }
    }
}

/// Convert MettaValue to MORK Expr bytes
///
/// This creates a MORK s-expression that can be used with query_multi.
/// Variables are converted to De Bruijn indices.
pub fn metta_to_mork_bytes(
    value: &MettaValue,
    space: &Space,
    ctx: &mut ConversionContext,
) -> Result<Vec<u8>, String> {
    let mut buffer = vec![0u8; 4096];
    let expr = Expr { ptr: buffer.as_mut_ptr() };
    let mut ez = ExprZipper::new(expr);

    write_metta_value(value, space, ctx, &mut ez)?;

    Ok(buffer[..ez.loc].to_vec())
}

/// Recursively write MettaValue to ExprZipper
fn write_metta_value(
    value: &MettaValue,
    space: &Space,
    ctx: &mut ConversionContext,
    ez: &mut ExprZipper,
) -> Result<(), String> {
    match value {
        MettaValue::Atom(name) => {
            // Check if it's a variable
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                // Variable - use De Bruijn encoding
                let var_id = &name[1..]; // Remove prefix
                match ctx.get_or_create_var(var_id)? {
                    None => {
                        // First occurrence - write NewVar
                        ez.write_new_var();
                        ez.loc += 1;
                    }
                    Some(idx) => {
                        // Subsequent occurrence - write VarRef
                        ez.write_var_ref(idx);
                        ez.loc += 1;
                    }
                }
            } else if name == "_" {
                // Wildcard - treat as anonymous variable
                ez.write_new_var();
                ez.loc += 1;
            } else {
                // Regular atom - write as symbol
                write_symbol(name.as_bytes(), space, ez)?;
            }
        }

        MettaValue::Bool(b) => {
            let s = if *b { "true" } else { "false" };
            write_symbol(s.as_bytes(), space, ez)?;
        }

        MettaValue::Long(n) => {
            let s = n.to_string();
            write_symbol(s.as_bytes(), space, ez)?;
        }

        MettaValue::String(s) => {
            // MORK uses quoted strings
            let quoted = format!("\"{}\"", s);
            write_symbol(quoted.as_bytes(), space, ez)?;
        }

        MettaValue::Uri(u) => {
            // MORK uses backticks for URIs
            let quoted = format!("`{}`", u);
            write_symbol(quoted.as_bytes(), space, ez)?;
        }

        MettaValue::Nil => {
            // Empty list
            ez.write_arity(0);
            ez.loc += 1;
        }

        MettaValue::SExpr(items) => {
            // Write arity tag
            let arity = items.len() as u8;
            ez.write_arity(arity);
            ez.loc += 1;

            // Write each element
            for item in items {
                write_metta_value(item, space, ctx, ez)?;
            }
        }

        MettaValue::Error(msg, details) => {
            // (error "msg" details)
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"error", space, ez)?;
            write_symbol(format!("\"{}\"", msg).as_bytes(), space, ez)?;
            write_metta_value(details, space, ctx, ez)?;
        }

        MettaValue::Type(t) => {
            // Types are just atoms/expressions
            write_metta_value(t, space, ctx, ez)?;
        }
    }

    Ok(())
}

/// Write a symbol to ExprZipper using Space's symbol table
fn write_symbol(bytes: &[u8], space: &Space, ez: &mut ExprZipper) -> Result<(), String> {
    // Use MORK's ParDataParser to intern the symbol
    let mut pdp = mork::space::ParDataParser::new(&space.sm);
    let token = pdp.tokenizer(bytes);

    ez.write_symbol(token);
    ez.loc += 1 + token.len();

    Ok(())
}

/// Convert MORK bindings to our Bindings format
///
/// MORK uses BTreeMap<(u8, u8), ExprEnv> where the key is (old_var, new_var).
/// We need to convert this to HashMap<String, MettaValue> using the original variable names.
#[allow(unused_variables)]
pub fn mork_bindings_to_metta(
    mork_bindings: &std::collections::BTreeMap<(u8, u8), ExprEnv>,
    ctx: &ConversionContext,
    space: &Space,
) -> Result<HashMap<String, MettaValue>, String> {
    use super::compile::compile;

    let mut bindings = HashMap::new();

    for (&(old_var, _new_var), expr_env) in mork_bindings {
        // Get the variable name from context
        if (old_var as usize) >= ctx.var_names.len() {
            continue; // Skip if variable not in our context
        }
        let var_name = &ctx.var_names[old_var as usize];

        // Serialize the ExprEnv to string
        let expr = expr_env.subsexpr();
        let mut buffer = Vec::new();
        expr.serialize2(&mut buffer,
            |s| {
                #[cfg(feature="interning")]
                {
                    let symbol = i64::from_be_bytes(s.try_into().unwrap()).to_be_bytes();
                    let mstr = space.sm.get_bytes(symbol).map(|x| unsafe { std::str::from_utf8_unchecked(x) });
                    unsafe { std::mem::transmute(mstr.unwrap_or("")) }
                }
                #[cfg(not(feature="interning"))]
                unsafe { std::mem::transmute(std::str::from_utf8_unchecked(s)) }
            },
            |i, _intro| { Expr::VARNAMES[i as usize] });

        let sexpr_str = String::from_utf8_lossy(&buffer);

        // Parse back to MettaValue
        if let Ok(state) = compile(&sexpr_str) {
            if let Some(value) = state.source.first() {
                bindings.insert(format!("${}", var_name), value.clone());
            }
        }
    }

    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::types::Environment;

    #[test]
    fn test_simple_atom_conversion() {
        let env = Environment::new();
        let space = env.space.lock().unwrap();
        let mut ctx = ConversionContext::new();

        let atom = MettaValue::Atom("foo".to_string());
        let result = metta_to_mork_bytes(&atom, &space, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_variable_conversion() {
        let env = Environment::new();
        let space = env.space.lock().unwrap();
        let mut ctx = ConversionContext::new();

        // First occurrence should create NewVar
        let var = MettaValue::Atom("$x".to_string());
        let result = metta_to_mork_bytes(&var, &space, &mut ctx);
        assert!(result.is_ok());
        assert_eq!(ctx.var_names.len(), 1);
        assert_eq!(ctx.var_names[0], "x");
    }

    #[test]
    fn test_sexpr_conversion() {
        let env = Environment::new();
        let space = env.space.lock().unwrap();
        let mut ctx = ConversionContext::new();

        // (double $x)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let result = metta_to_mork_bytes(&sexpr, &space, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_repeated_variable() {
        let env = Environment::new();
        let space = env.space.lock().unwrap();
        let mut ctx = ConversionContext::new();

        // (* $x $x) - same variable twice
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let result = metta_to_mork_bytes(&sexpr, &space, &mut ctx);
        assert!(result.is_ok());
        // Should only have one variable in context
        assert_eq!(ctx.var_names.len(), 1);
    }
}
