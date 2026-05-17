//! String-manipulation grounded operations.
//!
//! Houses `stringToChars` (MTT-FN-STRINGTOCHARS, Workstream X.5a) and
//! `sort-strings` (T06/060, mirrors HE `stdlib/string.rs::sort_strings`).
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

/// `(sort-strings (s1 s2 ... sN)) -> (sorted_s1 sorted_s2 ... sorted_sN)`.
///
/// Lexicographically sorts the strings in a single S-expression argument.
/// Mirrors HE `lib/src/metta/runner/stdlib/string.rs::sort_strings` —
/// HE signature `(-> Expression Expression)` and Rust impl uses
/// `Vec<&str>::sort()`. The argument is evaluated, then must reduce to an
/// S-expression whose children are all `String` values.
///
/// Empirically verified via metta-repl:
///   `!(sort-strings ("c" "a" "b"))` -> `[("a" "b" "c")]`
///   `!(sort-strings ("banana" "apple" "cherry"))` -> `[("apple" "banana" "cherry")]`
pub struct SortStringsOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for SortStringsOp {
    fn name(&self) -> &str {
        "sort-strings"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                // HE empirical: `!(sort-strings)` emits
                // `(Error (sort-strings) IncorrectNumberOfArguments)` —
                // tagged atom detail, not a string.
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
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

                // HE empirical:
                //   !(sort-strings (1 2 3))
                //   -> (Error (sort-strings (1 2 3)) sort-strings expects expression with strings as a first argument)
                // The detail is HE's literal arg_error string verbatim — keep
                // it byte-for-byte. (Note: HE emits it as a bare atom-shaped
                // detail in stdout; our exec_error_to_value wraps the string
                // into a `String` value which collapses to the same printed
                // sequence in the error pretty-printer.)
                const ARG_ERROR: &str =
                    "sort-strings expects expression with strings as a first argument";

                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    // Argument must reduce to an S-expression of strings.
                    let Some(items) = value.as_sexpr() else {
                        return GroundedWork::Error(ExecError::IncorrectArgument(
                            ARG_ERROR.to_string(),
                        ));
                    };

                    // Preallocate exactly len() entries — each child must be a string.
                    let mut strings: Vec<&str> = Vec::with_capacity(items.len());
                    for child in items {
                        let Some(s) = child.as_string() else {
                            return GroundedWork::Error(ExecError::IncorrectArgument(
                                ARG_ERROR.to_string(),
                            ));
                        };
                        strings.push(s);
                    }
                    strings.sort();

                    let mut sorted: Vec<V> = Vec::with_capacity(strings.len());
                    for s in strings {
                        sorted.push(factory.string(s));
                    }
                    results.push((factory.sexpr(sorted), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!(
                "Invalid step {} for sort-strings operation",
                state.step
            ),
        }
    }
}
