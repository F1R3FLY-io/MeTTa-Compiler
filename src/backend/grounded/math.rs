//! PT-canonical math operators per `petta-specification/spec/06`
//! (`PeTTa/src/metta.pl` math suite). Provides the `*-math` family of
//! transcendental / floating-point operations that PT exposes as built-ins:
//!
//! - `sqrt-math`, `pow-math`, `abs-math`, `log-math`
//! - `trunc-math`, `ceil-math`, `floor-math`
//! - `sin-math`, `cos-math`, `tan-math`
//! - `asin-math`, `acos-math`, `atan-math`
//! - `isnan-math`, `isinf-math`
//! - `min-atom`, `max-atom` (operate on tuples; tuple-aware)
//!
//! All ops are unary float-in / float-out (except pow-math and log-math which
//! are binary; isnan/isinf return Bool; min-atom/max-atom operate on
//! S-expression tuples). Empty-annihilation per PT canonical (PHE-003):
//! Empty arg => branch drop.

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use super::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Coerce a value to f64, returning None for non-numeric. Long is widened.
fn as_f64<V: MettaValueTrait>(v: &V) -> Option<f64> {
    if let Some(x) = v.as_long() {
        return Some(x as f64);
    }
    v.as_float()
}

/// Generic unary float op: `(op-math x)` → `op(x)` as float.
/// Empty arg => branch drop. Non-numeric arg => IncorrectArgument.
fn run_unary_math<V, F>(
    name: &str,
    state: &mut GroundedState<V>,
    factory: &F,
    op: impl Fn(f64) -> f64,
) -> GroundedWork<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    match state.step {
        0 => {
            if state.args.len() != 1 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "{} requires 1 argument, got {}",
                    name,
                    state.args.len()
                )));
            }
            state.step = 1;
            GroundedWork::EvalArg {
                arg_idx: 0,
                state: state.clone(),
            }
        }
        1 => {
            let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
            if let Some(err) = find_error(a_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }
            let mut results = Vec::with_capacity(a_results.len());
            for a in a_results {
                if a.is_empty() {
                    continue;
                }
                match as_f64(a) {
                    Some(x) => results.push((factory.float(op(x)), None)),
                    None => {
                        return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                            "{} requires Number argument, got {}",
                            name,
                            a.friendly_type_name()
                        )));
                    }
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for unary math op", state.step),
    }
}

/// Generic binary float op: `(op-math a b)` → `op(a, b)` as float.
fn run_binary_math<V, F>(
    name: &str,
    state: &mut GroundedState<V>,
    factory: &F,
    op: impl Fn(f64, f64) -> f64,
) -> GroundedWork<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "{} requires 2 arguments, got {}",
                    name,
                    state.args.len()
                )));
            }
            state.step = 1;
            GroundedWork::EvalArg {
                arg_idx: 0,
                state: state.clone(),
            }
        }
        1 => {
            state.step = 2;
            GroundedWork::EvalArg {
                arg_idx: 1,
                state: state.clone(),
            }
        }
        2 => {
            let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
            let b_results = state.get_arg(1).expect("arg 1 should be evaluated");
            if let Some(err) = find_error(a_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }
            if let Some(err) = find_error(b_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }
            let mut results = Vec::with_capacity(a_results.len() * b_results.len());
            for a in a_results {
                for b in b_results {
                    if a.is_empty() || b.is_empty() {
                        continue;
                    }
                    match (as_f64(a), as_f64(b)) {
                        (Some(x), Some(y)) => {
                            results.push((factory.float(op(x, y)), None));
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                                "{} requires Number arguments",
                                name
                            )));
                        }
                    }
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for binary math op", state.step),
    }
}

/// Generic unary predicate op: `(op-math x)` → Bool.
fn run_unary_pred<V, F>(
    name: &str,
    state: &mut GroundedState<V>,
    factory: &F,
    pred: impl Fn(f64) -> bool,
) -> GroundedWork<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    match state.step {
        0 => {
            if state.args.len() != 1 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "{} requires 1 argument, got {}",
                    name,
                    state.args.len()
                )));
            }
            state.step = 1;
            GroundedWork::EvalArg {
                arg_idx: 0,
                state: state.clone(),
            }
        }
        1 => {
            let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
            if let Some(err) = find_error(a_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }
            let mut results = Vec::with_capacity(a_results.len());
            for a in a_results {
                if a.is_empty() {
                    continue;
                }
                match as_f64(a) {
                    Some(x) => results.push((factory.bool(pred(x)), None)),
                    None => {
                        // Non-numeric arg is not nan/inf (predicate returns false).
                        results.push((factory.bool(false), None));
                    }
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for unary pred op", state.step),
    }
}

macro_rules! unary_math_op {
    ($struct_name:ident, $op_name:literal, $f:expr) => {
        pub struct $struct_name;
        impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for $struct_name {
            fn name(&self) -> &str {
                $op_name
            }
            fn execute_step<F: MettaValueFactory<V>>(
                &self,
                state: &mut GroundedState<V>,
                factory: &F,
            ) -> GroundedWork<V> {
                run_unary_math($op_name, state, factory, $f)
            }
        }
    };
}

macro_rules! binary_math_op {
    ($struct_name:ident, $op_name:literal, $f:expr) => {
        pub struct $struct_name;
        impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for $struct_name {
            fn name(&self) -> &str {
                $op_name
            }
            fn execute_step<F: MettaValueFactory<V>>(
                &self,
                state: &mut GroundedState<V>,
                factory: &F,
            ) -> GroundedWork<V> {
                run_binary_math($op_name, state, factory, $f)
            }
        }
    };
}

macro_rules! unary_pred_op {
    ($struct_name:ident, $op_name:literal, $f:expr) => {
        pub struct $struct_name;
        impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for $struct_name {
            fn name(&self) -> &str {
                $op_name
            }
            fn execute_step<F: MettaValueFactory<V>>(
                &self,
                state: &mut GroundedState<V>,
                factory: &F,
            ) -> GroundedWork<V> {
                run_unary_pred($op_name, state, factory, $f)
            }
        }
    };
}

// Unary float→float operations.
unary_math_op!(SqrtMathOp, "sqrt-math", |x| x.sqrt());
unary_math_op!(AbsMathOp, "abs-math", |x| x.abs());
unary_math_op!(TruncMathOp, "trunc-math", |x| x.trunc());
unary_math_op!(CeilMathOp, "ceil-math", |x| x.ceil());
unary_math_op!(FloorMathOp, "floor-math", |x| x.floor());
unary_math_op!(SinMathOp, "sin-math", |x| x.sin());
unary_math_op!(CosMathOp, "cos-math", |x| x.cos());
unary_math_op!(TanMathOp, "tan-math", |x| x.tan());
unary_math_op!(AsinMathOp, "asin-math", |x| x.asin());
unary_math_op!(AcosMathOp, "acos-math", |x| x.acos());
unary_math_op!(AtanMathOp, "atan-math", |x| x.atan());

// `log-math` per PT: (log-math base x) → log_base(x) = ln(x) / ln(base).
binary_math_op!(LogMathOp, "log-math", |base, x| x.log(base));

/// `(pow-math base exp)` — returns int when both args are int and exp >= 0
/// (matching PT's polymorphic pow that preserves integer type for integer
/// inputs); else float.
pub struct PowMathOp;
impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for PowMathOp {
    fn name(&self) -> &str {
        "pow-math"
    }
    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "pow-math requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                let mut results = Vec::with_capacity(a_results.len() * b_results.len());
                for a in a_results {
                    for b in b_results {
                        if a.is_empty() || b.is_empty() {
                            continue;
                        }
                        // Both int and non-negative exp → int result.
                        match (a.as_long(), b.as_long()) {
                            (Some(base), Some(exp)) if exp >= 0 && exp <= u32::MAX as i64 => {
                                results.push((factory.long(base.pow(exp as u32)), None));
                                continue;
                            }
                            _ => {}
                        }
                        match (as_f64(a), as_f64(b)) {
                            (Some(x), Some(y)) => {
                                results.push((factory.float(x.powf(y)), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::IncorrectArgument(
                                    "pow-math requires Number arguments".to_string(),
                                ));
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for PowMathOp", state.step),
        }
    }
}

// Unary float→Bool predicates.
unary_pred_op!(IsNanMathOp, "isnan-math", |x| x.is_nan());
unary_pred_op!(IsInfMathOp, "isinf-math", |x| x.is_infinite());

/// `(min-atom (a b c ...))` — return the minimum of the tuple elements.
pub struct MinAtomOp;
impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for MinAtomOp {
    fn name(&self) -> &str {
        "min-atom"
    }
    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "min-atom requires 1 argument, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                let mut results = Vec::with_capacity(a_results.len());
                for a in a_results {
                    if a.is_empty() {
                        continue;
                    }
                    let items = match a.as_sexpr() {
                        Some(items) => items,
                        None => {
                            return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                                "min-atom requires Expression argument, got {}",
                                a.friendly_type_name()
                            )));
                        }
                    };
                    if items.is_empty() {
                        continue;
                    }
                    let mut min_long: Option<i64> = None;
                    let mut min_float: Option<f64> = None;
                    let mut all_long = true;
                    for it in items {
                        if let Some(x) = it.as_long() {
                            min_long = Some(match min_long {
                                Some(m) => m.min(x),
                                None => x,
                            });
                            let xf = x as f64;
                            min_float = Some(match min_float {
                                Some(m) => m.min(xf),
                                None => xf,
                            });
                        } else if let Some(x) = it.as_float() {
                            all_long = false;
                            min_float = Some(match min_float {
                                Some(m) => m.min(x),
                                None => x,
                            });
                        } else {
                            return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                                "min-atom requires Number tuple elements"
                            )));
                        }
                    }
                    let chosen = if all_long {
                        factory.long(min_long.expect("non-empty tuple has min"))
                    } else {
                        factory.float(min_float.expect("non-empty tuple has min"))
                    };
                    results.push((chosen, None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for MinAtomOp", state.step),
        }
    }
}

/// `(max-atom (a b c ...))` — return the maximum of the tuple elements.
pub struct MaxAtomOp;
impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for MaxAtomOp {
    fn name(&self) -> &str {
        "max-atom"
    }
    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "max-atom requires 1 argument, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                let mut results = Vec::with_capacity(a_results.len());
                for a in a_results {
                    if a.is_empty() {
                        continue;
                    }
                    let items = match a.as_sexpr() {
                        Some(items) => items,
                        None => {
                            return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                                "max-atom requires Expression argument, got {}",
                                a.friendly_type_name()
                            )));
                        }
                    };
                    if items.is_empty() {
                        continue;
                    }
                    let mut max_long: Option<i64> = None;
                    let mut max_float: Option<f64> = None;
                    let mut all_long = true;
                    for it in items {
                        if let Some(x) = it.as_long() {
                            max_long = Some(match max_long {
                                Some(m) => m.max(x),
                                None => x,
                            });
                            let xf = x as f64;
                            max_float = Some(match max_float {
                                Some(m) => m.max(xf),
                                None => xf,
                            });
                        } else if let Some(x) = it.as_float() {
                            all_long = false;
                            max_float = Some(match max_float {
                                Some(m) => m.max(x),
                                None => x,
                            });
                        } else {
                            return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                                "max-atom requires Number tuple elements"
                            )));
                        }
                    }
                    let chosen = if all_long {
                        factory.long(max_long.expect("non-empty tuple has max"))
                    } else {
                        factory.float(max_float.expect("non-empty tuple has max"))
                    };
                    results.push((chosen, None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for MaxAtomOp", state.step),
        }
    }
}
