//! Non-TCO logical operations.
//!
//! Provides the standard logical operations with short-circuit semantics:
//! - `AndOp` - Logical AND (and)
//! - `OrOp` - Logical OR (or)
//! - `NotOp` - Logical NOT (not)

use super::{
    friendly_type_name, Environment, EvalFn, ExecError, GroundedOperation, GroundedResult,
    MettaValue,
};

/// Logical AND operation: (and a b)
pub struct AndOp;

impl GroundedOperation for AndOp {
    fn name(&self) -> &str {
        "and"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "and requires 2 arguments, got {}",
                args.len()
            )));
        }

        // Short-circuit: only evaluate second arg if first is true
        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());

        let mut results = Vec::new();
        for a in &a_results {
            match a {
                MettaValue::Bool(false) => {
                    // Short-circuit: false AND anything = false
                    results.push((MettaValue::Bool(false), None));
                }
                MettaValue::Bool(true) => {
                    // Need to evaluate second argument
                    let (b_results, _) = eval_fn(args[1].clone(), env1.clone());
                    for b in &b_results {
                        match b {
                            MettaValue::Bool(bv) => {
                                results.push((MettaValue::Bool(*bv), None));
                            }
                            _ => {
                                return Err(ExecError::Runtime(format!(
                                    "Cannot perform 'and': expected Bool, got {}",
                                    friendly_type_name(b)
                                )))
                            }
                        }
                    }
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot perform 'and': expected Bool, got {}",
                        friendly_type_name(a)
                    )))
                }
            }
        }
        Ok(results)
    }
}

/// Logical OR operation: (or a b)
pub struct OrOp;

impl GroundedOperation for OrOp {
    fn name(&self) -> &str {
        "or"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "or requires 2 arguments, got {}",
                args.len()
            )));
        }

        // Short-circuit: only evaluate second arg if first is false
        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());

        let mut results = Vec::new();
        for a in &a_results {
            match a {
                MettaValue::Bool(true) => {
                    // Short-circuit: true OR anything = true
                    results.push((MettaValue::Bool(true), None));
                }
                MettaValue::Bool(false) => {
                    // Need to evaluate second argument
                    let (b_results, _) = eval_fn(args[1].clone(), env1.clone());
                    for b in &b_results {
                        match b {
                            MettaValue::Bool(bv) => {
                                results.push((MettaValue::Bool(*bv), None));
                            }
                            _ => {
                                return Err(ExecError::Runtime(format!(
                                    "Cannot perform 'or': expected Bool, got {}",
                                    friendly_type_name(b)
                                )))
                            }
                        }
                    }
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot perform 'or': expected Bool, got {}",
                        friendly_type_name(a)
                    )))
                }
            }
        }
        Ok(results)
    }
}

/// Logical NOT operation: (not a)
pub struct NotOp;

impl GroundedOperation for NotOp {
    fn name(&self) -> &str {
        "not"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 1 {
            return Err(ExecError::IncorrectArgument(format!(
                "not requires 1 argument, got {}",
                args.len()
            )));
        }

        let (a_results, _) = eval_fn(args[0].clone(), env.clone());

        let mut results = Vec::new();
        for a in &a_results {
            match a {
                MettaValue::Bool(v) => {
                    results.push((MettaValue::Bool(!v), None));
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot perform 'not': expected Bool, got {}",
                        friendly_type_name(a)
                    )))
                }
            }
        }
        Ok(results)
    }
}
