//! Pattern matching operations on spaces.
//!
//! This module handles the `match` operation which searches a space for
//! atoms matching a pattern and returns instantiated templates.

use tracing::debug;

use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::EvalStep;
use super::helpers::suggest_space_name;

/// Step version of eval_match - defers evaluation to trampoline.
/// Handles both 3-arg (match space pattern template) and 4-arg (match & self pattern template) syntaxes.
pub(crate) fn eval_match_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    let args = &items[1..];
    debug!(target: "mettatron::eval::eval_match_step", ?args, ?items);

    // Debug: Show what eval_match_step receives
    let debug = std::env::var("METTA_DEBUG_MATCH").is_ok();
    if debug {
        eprintln!("[DEBUG eval_match_step] items={:?}", items);
        eprintln!("[DEBUG eval_match_step] args.len()={}", args.len());
    }

    // Support both: (match space pattern template) and (match & self pattern template)
    if args.len() == 3 {
        // New-style syntax: (match space pattern template)
        // Defer evaluation to trampoline via StartMatch
        let space_arg = args[0].clone();
        let pattern = args[1].clone();
        let template = args[2].clone();

        if debug {
            eprintln!("[DEBUG eval_match_step] space_arg={:?}", space_arg);
            eprintln!("[DEBUG eval_match_step] pattern={:?}", pattern);
            eprintln!("[DEBUG eval_match_step] template={:?}", template);
        }

        EvalStep::StartMatch {
            space_arg,
            pattern,
            template,
            env,
            depth,
        }
    } else if args.len() == 4 {
        // Legacy syntax: (match & self pattern template)
        // This path doesn't need eval() - env.match_space handles everything internally
        let space_ref = &args[0];
        let space_name = &args[1];
        let pattern = &args[2];
        let template = &args[3];

        // Check that first arg is & (space reference operator)
        match space_ref.inner() {
            MettaValueInner::Atom(s) if s == "&" => {
                // Check space name (for now, only support "self")
                match space_name.inner() {
                    MettaValueInner::Atom(name) if name == "self" => {
                        // Use optimized match_space method that works directly with MORK
                        // Expand multiplicity matches to Vec<MettaValue> for API compatibility
                        let results: Vec<MettaValue> = env
                            .match_space(pattern, template)
                            .into_iter()
                            .flat_map(|m| m.expand())
                            .collect();
                        EvalStep::Done((results, env))
                    }
                    _ => {
                        // Try to suggest a valid space name
                        let name_str = match space_name.inner() {
                            MettaValueInner::Atom(s) => s.as_str(),
                            _ => "",
                        };

                        let suggestion = suggest_space_name(name_str);
                        let msg = match suggestion {
                            Some(s) => format!(
                                "match only supports 'self' as space name, got: {:?}. {}",
                                space_name, s
                            ),
                            None => format!(
                                "match only supports 'self' as space name, got: {:?}",
                                space_name
                            ),
                        };

                        let err = MettaValue::Error(msg, MettaValue::SExpr(args.to_vec()));
                        EvalStep::Done((vec![err], env))
                    }
                }
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "match requires & as first argument (legacy syntax), got: {}",
                        super::super::friendly_value_repr(space_ref)
                    ),
                    MettaValue::SExpr(args.to_vec()),
                );
                EvalStep::Done((vec![err], env))
            }
        }
    } else {
        let got = args.len();
        debug!(
            target: "mettatron::eval::eval_match_step",
            got = args.len(), expected = 4, args = ?args,
            "Match called with incorrect number of arguments"
        );

        let err = MettaValue::Error(
            format!(
                "match requires 3 or 4 arguments, got {}. Usage: (match space pattern template) or (match & self pattern template)",
                got
            ),
            MettaValue::SExpr(args.to_vec()),
        );
        EvalStep::Done((vec![err], env))
    }
}

