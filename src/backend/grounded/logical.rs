//! TCO logical operations.
//!
//! Provides tail-call optimized logical operations that work with any
//! value type implementing `MettaValueTrait`:
//!
//! - `AndOp` - Logical AND with short-circuit evaluation
//! - `OrOp` - Logical OR with short-circuit evaluation
//! - `NotOp` - Logical NOT
//! - `XorOp` - Logical XOR (no short-circuit — both operands always needed)
//!
//! ## Zero-Conversion Design
//!
//! These operations use `MettaValueTrait` methods (e.g., `as_bool()`)
//! instead of pattern matching on `MettaValueInner`, enabling them to work with
//! both heap and arena allocation without conversion.

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use super::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// TCO Logical AND operation: (and a b ...) — variadic, Z.A.1 (2026-05-12).
///
/// HE bisimilarity: `(and)` returns `True` (identity); `(and x)` returns
/// `x` coerced to Bool; `(and x1 x2 ...)` left-folds with short-circuit
/// on the first arg whose results are ALL False. Cartesian product
/// semantics across nondeterministic operand results is preserved.
pub struct AndOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for AndOp {
    fn name(&self) -> &str {
        "and"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        variadic_logical_step(state, factory, "and", LogicalKind::And)
    }
}

/// TCO Logical OR operation: (or a b ...) — variadic, Z.A.1 (2026-05-12).
///
/// HE bisimilarity: `(or)` returns `False` (identity); `(or x)` returns
/// `x` coerced to Bool; `(or x1 x2 ...)` left-folds with short-circuit
/// on the first arg whose results are ALL True. Cartesian product
/// semantics across nondeterministic operand results is preserved.
pub struct OrOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for OrOp {
    fn name(&self) -> &str {
        "or"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        variadic_logical_step(state, factory, "or", LogicalKind::Or)
    }
}

/// Discriminator for the shared variadic logical step machine.
#[derive(Clone, Copy)]
enum LogicalKind {
    And,
    Or,
}

impl LogicalKind {
    /// Identity element for empty-args invocation.
    fn identity(self) -> bool {
        match self {
            LogicalKind::And => true,
            LogicalKind::Or => false,
        }
    }

    /// Short-circuit sentinel: a result set of all `sentinel` halts the fold.
    /// AND short-circuits on all-False; OR on all-True.
    fn short_circuit_sentinel(self) -> bool {
        match self {
            LogicalKind::And => false,
            LogicalKind::Or => true,
        }
    }

    /// Step combinator for the Cartesian-product fold.
    fn combine(self, a: bool, b: bool) -> bool {
        match self {
            LogicalKind::And => a && b,
            LogicalKind::Or => a || b,
        }
    }
}

/// Shared variadic state machine for `and`/`or`.
///
/// Step semantics:
/// - `state.step` is the index of the NEXT arg to evaluate (0..=N).
/// - `step == 0` with `N == 0`: return identity (True for AND, False for OR).
/// - Before issuing EvalArg(k), check args[0..k] for short-circuit:
///   * Error in prev → propagate Error.
///   * Non-Bool in prev → Runtime error.
///   * All-sentinel in prev → short-circuit Done(sentinel).
/// - `step == N`: Cartesian-product fold over all args, returning all bool combos.
fn variadic_logical_step<V, F>(
    state: &mut GroundedState<V>,
    factory: &F,
    op_name: &str,
    kind: LogicalKind,
) -> GroundedWork<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let n = state.args.len();

    // Empty-args identity.
    if state.step == 0 && n == 0 {
        return GroundedWork::Done(vec![(factory.bool(kind.identity()), None)]);
    }

    // Inspect the most recently-evaluated arg (state.step is "next to eval";
    // step > 0 means args[step - 1] is already evaluated).
    if state.step > 0 {
        let prev_idx = state.step - 1;
        let prev_results = state.get_arg(prev_idx).expect("prev arg evaluated");

        if let Some(err) = find_error(prev_results) {
            return GroundedWork::Done(vec![(err.clone(), None)]);
        }

        // Type-check Bool.
        for v in prev_results {
            if v.as_bool().is_none() {
                return GroundedWork::Error(ExecError::Runtime(format!(
                    "Cannot perform '{}': expected Bool, got {}",
                    op_name,
                    v.friendly_type_name()
                )));
            }
        }

        // Short-circuit: every result equals the sentinel.
        let sentinel = kind.short_circuit_sentinel();
        let all_sentinel = prev_results.iter().all(|v| v.as_bool() == Some(sentinel));
        if all_sentinel {
            return GroundedWork::Done(vec![(factory.bool(sentinel), None)]);
        }
    }

    // Need more args? Eval the next.
    if state.step < n {
        let arg_idx = state.step;
        state.step = arg_idx + 1;
        return GroundedWork::EvalArg {
            arg_idx,
            state: state.clone(),
        };
    }

    // All args evaluated; combine via Cartesian fold.
    let mut combos: Vec<bool> = vec![kind.identity()];
    for i in 0..n {
        let arg_results = state.get_arg(i).expect("arg evaluated");
        let mut new_combos: Vec<bool> = Vec::with_capacity(combos.len() * arg_results.len());
        for prev in &combos {
            for v in arg_results {
                let b = v
                    .as_bool()
                    .expect("Bool already type-checked in short-circuit pass");
                new_combos.push(kind.combine(*prev, b));
            }
        }
        combos = new_combos;
    }
    let results: Vec<(V, Option<crate::backend::models::GenericBindings<V>>)> = combos
        .into_iter()
        .map(|b| (factory.bool(b), None))
        .collect();
    GroundedWork::Done(results)
}

/// TCO Logical NOT operation: (not a)
pub struct NotOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for NotOp {
    fn name(&self) -> &str {
        "not"
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
                        "not requires 1 argument, got {}",
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

                let mut results = Vec::new();
                for a in a_results {
                    match a.as_bool() {
                        Some(x) => {
                            results.push((factory.bool(!x), None));
                        }
                        None => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'not': expected Bool, got {}",
                                a.friendly_type_name()
                            )));
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for not operation", state.step),
        }
    }
}

/// TCO Logical XOR operation: (xor a b)
///
/// No short-circuit: XOR always needs both operands since the result depends
/// on both values (true iff exactly one operand is true).
pub struct XorOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for XorOp {
    fn name(&self) -> &str {
        "xor"
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
                        "xor requires 2 arguments, got {}",
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

                // No short-circuit for XOR — always need both operands
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a.as_bool(), b.as_bool()) {
                            (Some(x), Some(y)) => {
                                results.push((factory.bool(x ^ y), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform 'xor': expected Bool, got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )));
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for xor operation", state.step),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{active_factory, MettaValue};

    fn run_binary_logical<Op: GroundedOperationTCO<MettaValue>>(op: &Op, a: bool, b: bool) -> bool {
        let factory = active_factory();
        let mut state = GroundedState::new(
            op.name().to_string(),
            vec![MettaValue::Bool(a), MettaValue::Bool(b)],
        );

        op.execute_step(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(a)]);
        state.step = 1;

        let work = op.execute_step(&mut state, &factory);

        // Check for short-circuit
        match work {
            GroundedWork::Done(results) => {
                return results[0].0.as_bool().expect("should be bool");
            }
            GroundedWork::EvalArg { .. } => {
                // Need to continue
                state.set_arg(1, vec![MettaValue::Bool(b)]);
                state.step = 2;
            }
            _ => panic!("Unexpected work"),
        }

        let work = op.execute_step(&mut state, &factory);
        match work {
            GroundedWork::Done(results) => results[0].0.as_bool().expect("should be bool"),
            _ => panic!("Expected Done"),
        }
    }

    fn run_unary_logical<Op: GroundedOperationTCO<MettaValue>>(op: &Op, a: bool) -> bool {
        let factory = active_factory();
        let mut state = GroundedState::new(op.name().to_string(), vec![MettaValue::Bool(a)]);

        op.execute_step(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(a)]);
        state.step = 1;

        let work = op.execute_step(&mut state, &factory);
        match work {
            GroundedWork::Done(results) => results[0].0.as_bool().expect("should be bool"),
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_and_op() {
        assert!(run_binary_logical(&AndOp, true, true));
        assert!(!run_binary_logical(&AndOp, true, false));
        assert!(!run_binary_logical(&AndOp, false, true));
        assert!(!run_binary_logical(&AndOp, false, false));
    }

    #[test]
    fn test_or_op() {
        assert!(run_binary_logical(&OrOp, true, true));
        assert!(run_binary_logical(&OrOp, true, false));
        assert!(run_binary_logical(&OrOp, false, true));
        assert!(!run_binary_logical(&OrOp, false, false));
    }

    #[test]
    fn test_not_op() {
        assert!(!run_unary_logical(&NotOp, true));
        assert!(run_unary_logical(&NotOp, false));
    }

    #[test]
    fn test_xor_op() {
        assert!(!run_binary_logical(&XorOp, true, true));
        assert!(run_binary_logical(&XorOp, true, false));
        assert!(run_binary_logical(&XorOp, false, true));
        assert!(!run_binary_logical(&XorOp, false, false));
    }

    #[test]
    fn test_and_short_circuit() {
        // When first arg is all False, should short-circuit
        let factory = active_factory();
        let mut state = GroundedState::new(
            "and".to_string(),
            vec![MettaValue::Bool(false), MettaValue::Bool(true)],
        );

        AndOp.execute_step(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(false)]);
        state.step = 1;

        let work = AndOp.execute_step(&mut state, &factory);
        // Should be Done (short-circuit), not EvalArg
        match work {
            GroundedWork::Done(results) => {
                assert_eq!(results[0].0.as_bool(), Some(false));
            }
            _ => panic!("Expected short-circuit Done"),
        }
    }

    #[test]
    fn test_or_short_circuit() {
        // When first arg is all True, should short-circuit
        let factory = active_factory();
        let mut state = GroundedState::new(
            "or".to_string(),
            vec![MettaValue::Bool(true), MettaValue::Bool(false)],
        );

        OrOp.execute_step(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(true)]);
        state.step = 1;

        let work = OrOp.execute_step(&mut state, &factory);
        // Should be Done (short-circuit), not EvalArg
        match work {
            GroundedWork::Done(results) => {
                assert_eq!(results[0].0.as_bool(), Some(true));
            }
            _ => panic!("Expected short-circuit Done"),
        }
    }
}
