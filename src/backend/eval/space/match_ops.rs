//! Pattern matching operations on spaces.
//!
//! This module handles the `match` operation which searches a space for
//! atoms matching a pattern and returns instantiated templates.

use std::sync::Arc;
use tracing::debug;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, SpaceHandle};

use super::super::{apply_bindings, eval, pattern_match};
use super::helpers::suggest_space_name;

/// Evaluate match: (match <space> <pattern> <template>)
/// Searches the space for all atoms matching the pattern and returns instantiated templates
///
/// Supports two syntaxes:
/// - New: (match space pattern template) - where space is a Space value (e.g., from mod-space! or &self)
/// - Legacy: (match & self pattern template) - backward compatible syntax
///
/// Optimized to use Environment::match_space which performs pattern matching
/// directly on MORK expressions without unnecessary intermediate allocations
pub(crate) fn eval_match(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    debug!(target: "mettatron::eval::eval_match", ?args, ?items);

    // Debug: Show what eval_match receives
    let debug = std::env::var("METTA_DEBUG_MATCH").is_ok();
    if debug {
        eprintln!("[DEBUG eval_match] items={:?}", items);
        eprintln!("[DEBUG eval_match] args.len()={}", args.len());
    }

    // Support both: (match space pattern template) and (match & self pattern template)
    if args.len() == 3 {
        // New-style syntax: (match space pattern template)
        let space_arg = &args[0];
        let pattern = &args[1];
        let template = &args[2];

        if debug {
            eprintln!("[DEBUG eval_match] space_arg={:?}", space_arg);
            eprintln!("[DEBUG eval_match] pattern={:?}", pattern);
            eprintln!("[DEBUG eval_match] template={:?}", template);
        }

        // Evaluate the space argument
        let (space_results, env1) = eval(space_arg.clone(), env);
        if space_results.is_empty() {
            let err = MettaValue::Error(
                "match: space evaluated to empty".to_string(),
                Arc::new(space_arg.clone()),
            );
            return (vec![err], env1);
        }

        match &space_results[0] {
            MettaValue::Space(handle) => {
                let results = match_with_space_handle(handle, pattern, template, &env1);
                (results, env1)
            }
            other => {
                let err = MettaValue::Error(
                    format!(
                        "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                        super::super::friendly_value_repr(other)
                    ),
                    Arc::new(other.clone()),
                );
                (vec![err], env1)
            }
        }
    } else if args.len() == 4 {
        // Legacy syntax: (match & self pattern template)
        let space_ref = &args[0];
        let space_name = &args[1];
        let pattern = &args[2];
        let template = &args[3];

        // Check that first arg is & (space reference operator)
        match space_ref {
            MettaValue::Atom(s) if s == "&" => {
                // Check space name (for now, only support "self")
                match space_name {
                    MettaValue::Atom(name) if name == "self" => {
                        // Use optimized match_space method that works directly with MORK
                        let results = env.match_space(pattern, template);
                        (results, env)
                    }
                    _ => {
                        // Try to suggest a valid space name
                        let name_str = match space_name {
                            MettaValue::Atom(s) => s.as_str(),
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

                        let err =
                            MettaValue::Error(msg, Arc::new(MettaValue::SExpr(args.to_vec())));
                        (vec![err], env)
                    }
                }
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "match requires & as first argument (legacy syntax), got: {}",
                        super::super::friendly_value_repr(space_ref)
                    ),
                    Arc::new(MettaValue::SExpr(args.to_vec())),
                );
                (vec![err], env)
            }
        }
    } else {
        let got = args.len();
        debug!(
            target: "mettatron::eval::eval_match",
            got = args.len(), expected = 4, args = ?args,
            "Match called with incorrect number of arguments"
        );

        let err = MettaValue::Error(
            format!(
                "match requires 3 or 4 arguments, got {}. Usage: (match space pattern template) or (match & self pattern template)",
                got
            ),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        (vec![err], env)
    }
}

/// Pattern match against a SpaceHandle and return evaluated instantiated templates.
/// HE-compatible: After pattern matching, the template is evaluated with bindings applied.
///
/// IMPORTANT: Each match result is evaluated in a forked environment to provide
/// nondeterministic branch isolation. This prevents one branch's mutations from
/// affecting other branches.
fn match_with_space_handle(
    handle: &SpaceHandle,
    pattern: &MettaValue,
    template: &MettaValue,
    env: &Environment,
) -> Vec<MettaValue> {
    // Debug logging
    let debug = std::env::var("METTA_DEBUG_MATCH").is_ok();
    if debug {
        eprintln!(
            "[DEBUG match] handle.name={}, is_module_space={}",
            handle.name,
            handle.is_module_space()
        );
        eprintln!("[DEBUG match] pattern={:?}", pattern);
        eprintln!("[DEBUG match] template={:?}", template);
    }

    // For module-backed spaces or the global "self" space, use Environment's MORK-based matching
    // For other owned spaces (from new-space), match against the atoms in the space
    if handle.is_module_space() || handle.name == "self" {
        // Module space or global "self" space - use Environment's match_space for MORK integration
        // The Environment has the rules from (= ...) definitions
        if debug {
            eprintln!("[DEBUG match] Using env.match_space (module/self path)");
        }
        env.match_space(pattern, template)
    } else {
        // Owned space (from new-space) - match against atoms stored in SpaceHandle
        let atoms = handle.collapse();
        if debug {
            eprintln!(
                "[DEBUG match] Using owned space path, {} atoms",
                atoms.len()
            );
        }

        // Collect all matching atoms first (don't evaluate yet)
        let mut matching_bindings: Vec<_> = Vec::new();
        for atom in &atoms {
            if debug {
                eprintln!("[DEBUG match] Trying to match atom={:?}", atom);
            }
            if let Some(bindings) = pattern_match(pattern, atom) {
                if debug {
                    eprintln!("[DEBUG match] MATCH! bindings={:?}", bindings);
                }
                matching_bindings.push(bindings);
            }
        }

        // If only one match, no need for forking (optimization)
        if matching_bindings.len() == 1 {
            let bindings = &matching_bindings[0];
            let instantiated = apply_bindings(template, bindings).into_owned();
            if debug {
                eprintln!(
                    "[DEBUG match] Single match, instantiated={:?}",
                    instantiated
                );
            }
            let (eval_results, _) = eval(instantiated, env.clone());
            if debug {
                eprintln!("[DEBUG match] eval_results={:?}", eval_results);
            }
            return eval_results;
        }

        // Multiple matches - fork environment for each to isolate mutations
        let mut results = Vec::new();
        for bindings in &matching_bindings {
            let instantiated = apply_bindings(template, bindings).into_owned();
            if debug {
                eprintln!("[DEBUG match] Multi-match, instantiated={:?}", instantiated);
            }

            // Fork environment for this branch to isolate space mutations
            let forked_env = env.fork_for_nondeterminism();

            // HE-compatible: Evaluate the instantiated template in forked environment
            let (eval_results, _) = eval(instantiated, forked_env);
            if debug {
                eprintln!("[DEBUG match] eval_results={:?}", eval_results);
            }
            results.extend(eval_results);
        }

        results
    }
}
