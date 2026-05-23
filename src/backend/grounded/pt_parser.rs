//! PT-canonical `parse`/`sread`/`swrite` grounded operators per PeTTa
//! `src/metta.pl`. Native MTT implementation: wraps the existing MettaParser
//! (no SWI call-out, respecting user constraint #4).

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use super::ExecError;
use crate::backend::compile::compile_generic;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// `(parse str)` — parse a MeTTa source string into an atom.
///
/// Returns the first parsed top-level form. Errors are signalled via the
/// standard MTT `Error` shape. Empty string yields no result (branch drop)
/// matching PT silent-fail.
pub struct ParseOp;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GroundedOperationTCO<V>
    for ParseOp
{
    fn name(&self) -> &str {
        "parse"
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
                        "parse requires 1 argument, got {}",
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
                    let src = match a.as_string() {
                        Some(s) => s.to_string(),
                        None => {
                            // Non-string arg: PT canonical drops (silent fail).
                            continue;
                        }
                    };
                    match compile_generic::<V, F>(&src, factory) {
                        Ok(parsed) => {
                            if let Some(first) = parsed.into_iter().next() {
                                results.push((first, None));
                            }
                            // empty parse → silent fail (no result)
                        }
                        Err(e) => {
                            let call_form = factory.atom("parse");
                            let detail = factory.string(&format!("parse error: {}", e));
                            results.push((factory.error_pt(detail, call_form), None));
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for ParseOp", state.step),
        }
    }
}

/// `(swrite atom)` — write an atom to its source-form string representation.
/// Inverse of `parse`. Returns a String value.
pub struct SwriteOp;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GroundedOperationTCO<V>
    for SwriteOp
{
    fn name(&self) -> &str {
        "swrite"
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
                        "swrite requires 1 argument, got {}",
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
                    // Use the value's friendly_repr for canonical printable form.
                    results.push((factory.string(&a.friendly_repr()), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for SwriteOp", state.step),
        }
    }
}

/// `(sread str)` — alias for `parse`. PeTTa exposes both names.
pub struct SreadOp;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GroundedOperationTCO<V>
    for SreadOp
{
    fn name(&self) -> &str {
        "sread"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        // Delegate to ParseOp's logic — name only differs.
        ParseOp.execute_step(state, factory)
    }
}

/// `(repr atom)` — like swrite but returns a string with extra structure
/// (PeTTa's `repr` is the readable-form printer that includes type tags).
/// For MTT, identical to swrite; PT's repr-vs-swrite distinction is a
/// display detail at PT's REPL level that doesn't affect atom-level
/// observation.
pub struct ReprOp;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GroundedOperationTCO<V>
    for ReprOp
{
    fn name(&self) -> &str {
        "repr"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        SwriteOp.execute_step(state, factory)
    }
}
