//! String-manipulation grounded operations.
//!
//! Currently houses `stringToChars` (MTT-FN-STRINGTOCHARS, Workstream X.5a).
//! Future string ops (substring, concat, ...) belong here too.

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use crate::backend::grounded::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// `(stringToChars "abc") -> (a b c)`.
///
/// Splits a string into a tuple of single-character atom symbols. Each char
/// becomes an `Atom`, not a `String`, per the HE Python stdlib convention and
/// the M09h conformance fixtures (`atoms: ["(a b c)"]`).
pub struct StringToCharsOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for StringToCharsOp {
    fn name(&self) -> &str {
        "stringToChars"
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
                        "stringToChars requires 1 argument, got {}",
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
                let arg = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    let Some(s) = value.as_string() else {
                        return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                            "stringToChars requires String argument, got {}",
                            value.friendly_type_name()
                        )));
                    };
                    let mut chars = Vec::with_capacity(s.chars().count());
                    let mut buf = [0u8; 4];
                    for c in s.chars() {
                        let slice = c.encode_utf8(&mut buf);
                        chars.push(factory.atom(slice));
                    }
                    results.push((factory.sexpr(chars), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!(
                "Invalid step {} for stringToChars operation",
                state.step
            ),
        }
    }
}
