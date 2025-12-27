//! Non-TCO arithmetic operations.
//!
//! Provides the standard arithmetic operations that evaluate arguments internally:
//! - `AddOp` - Addition (+)
//! - `SubOp` - Subtraction (-)
//! - `MulOp` - Multiplication (*)
//! - `DivOp` - Division (/)
//! - `ModOp` - Modulo (%)

use super::{
    find_error, friendly_type_name, Environment, EvalFn, ExecError, GroundedOperation,
    GroundedResult, MettaValue,
};

/// Addition operation: (+ a b)
pub struct AddOp;

impl GroundedOperation for AddOp {
    fn name(&self) -> &str {
        "+"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "+ requires 2 arguments, got {}",
                args.len()
            )));
        }

        // Evaluate arguments internally (lazy -> eager for concrete computation)
        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        // Compute Cartesian product of results
        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => match x.checked_add(*y) {
                        Some(sum) => results.push((MettaValue::Long(sum), None)),
                        None => {
                            return Err(ExecError::Runtime(format!(
                                "Integer overflow: {} + {}",
                                x, y
                            )))
                        }
                    },
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(x + y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(*x as f64 + y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        results.push((MettaValue::Float(x + *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '+': expected Number (integer), got {}",
                            friendly_type_name(
                                if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                    a
                                } else {
                                    b
                                }
                            )
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Subtraction operation: (- a b)
pub struct SubOp;

impl GroundedOperation for SubOp {
    fn name(&self) -> &str {
        "-"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "- requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => match x.checked_sub(*y) {
                        Some(diff) => results.push((MettaValue::Long(diff), None)),
                        None => {
                            return Err(ExecError::Runtime(format!(
                                "Integer overflow: {} - {}",
                                x, y
                            )))
                        }
                    },
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(x - y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(*x as f64 - y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        results.push((MettaValue::Float(x - *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '-': expected Number (integer), got {}",
                            friendly_type_name(
                                if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                    a
                                } else {
                                    b
                                }
                            )
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Multiplication operation: (* a b)
pub struct MulOp;

impl GroundedOperation for MulOp {
    fn name(&self) -> &str {
        "*"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "* requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => match x.checked_mul(*y) {
                        Some(prod) => results.push((MettaValue::Long(prod), None)),
                        None => {
                            return Err(ExecError::Runtime(format!(
                                "Integer overflow: {} * {}",
                                x, y
                            )))
                        }
                    },
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(x * y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(*x as f64 * y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        results.push((MettaValue::Float(x * *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '*': expected Number (integer), got {}",
                            friendly_type_name(
                                if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                    a
                                } else {
                                    b
                                }
                            )
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Division operation: (/ a b)
pub struct DivOp;

impl GroundedOperation for DivOp {
    fn name(&self) -> &str {
        "/"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "/ requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        if *y == 0 {
                            return Err(ExecError::Arithmetic("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Long(x / y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        if *y == 0.0 {
                            return Err(ExecError::Arithmetic("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Float(x / y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        if *y == 0.0 {
                            return Err(ExecError::Arithmetic("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Float(*x as f64 / y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        if *y == 0 {
                            return Err(ExecError::Arithmetic("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Float(x / *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '/': expected Number (integer), got {}",
                            friendly_type_name(
                                if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                    a
                                } else {
                                    b
                                }
                            )
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Modulo operation: (% a b)
pub struct ModOp;

impl GroundedOperation for ModOp {
    fn name(&self) -> &str {
        "%"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "% requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        if *y == 0 {
                            return Err(ExecError::Arithmetic("Modulo by zero".to_string()));
                        }
                        match x.checked_rem(*y) {
                            Some(r) => results.push((MettaValue::Long(r), None)),
                            None => {
                                return Err(ExecError::Arithmetic("Modulo overflow".to_string()))
                            }
                        }
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '%': expected Number (integer), got {}",
                            friendly_type_name(if !matches!(a, MettaValue::Long(_)) {
                                a
                            } else {
                                b
                            })
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}
