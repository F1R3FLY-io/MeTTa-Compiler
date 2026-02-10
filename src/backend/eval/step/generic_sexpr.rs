//! Generic S-Expression Step Evaluation
//!
//! This module handles the generic evaluation step for S-expressions, including
//! special forms dispatch and rule matching. It works with any value type
//! implementing `MettaValueTrait`.
//!
//! ## Design
//!
//! The generic sexpr step uses:
//! - `MettaValueTrait` for type checking and value inspection
//! - `MettaValueFactory` (via `EvalContext`) for value construction
//! - `GenericEnvironment<V, F>` directly for environment operations
//!
//! Special forms use generic implementations that work with `GenericEnvironment`,
//! achieving full genericization over the value type.

use tracing::trace;

use crate::backend::environment::GenericEnvironment;
use crate::backend::eval::list_ops::helpers::suggest_variable_format;
// Generic module operations - used directly (no boundary conversion)
use crate::backend::eval::modules_generic::{
    eval_import_generic, eval_include_generic, eval_mod_space_generic, eval_print_mods_generic,
};
// Generic MORK operations - used directly (no boundary conversion)
use crate::backend::eval::mork_forms_generic::{
    eval_coalg_generic, eval_exec_generic, eval_lookup_generic, eval_rulify_generic,
};
use crate::backend::eval::trampoline::{ContextEnv, EvalContext};
use crate::backend::grounded::{has_generic_grounded_op, GenericGroundedState};
use crate::backend::models::{GenericRule, MettaValueFactory, MettaValueTrait};

use super::generic_types::GenericEvalStep;
use super::grounded::find_grounded_arg_indices_generic;

/// Generic S-expression step evaluation.
///
/// This is the generic version of `eval_sexpr_step` that works with any value type
/// implementing `MettaValueTrait`. It handles special forms dispatch and rule matching.
///
/// # Type Parameters
///
/// - `C`: The evaluation context (e.g., `StaticArenaContext`)
///
/// # Arguments
///
/// - `items`: The S-expression items to evaluate
/// - `env`: The evaluation environment (`GenericEnvironment<C::Value, C::Factory>`)
/// - `depth`: Current evaluation depth
/// - `ctx`: The evaluation context providing the factory
///
/// All environment operations use `GenericEnvironment` methods directly.
pub fn eval_sexpr_step_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    depth: usize,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    trace!(target: "mettatron::backend::eval::eval_sexpr_step_generic", ?items, depth);

    // Preprocess to combine `& self` into `&self` for HE-compatible space references
    let items = preprocess_space_refs_generic(items, ctx);

    if items.is_empty() {
        // HE-compatible: empty SExpr () evaluates to itself, not Nil
        return GenericEvalStep::Done((vec![ctx.factory().sexpr(vec![])], env));
    }

    // Check for special forms - these are handled directly
    if let Some(op) = items.first().and_then(|v| v.as_atom()) {
        match op {
            // Rule definition - native generic implementation (zero-conversion)
            "=" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "= requires exactly 2 arguments, got {}. Usage: (= pattern body)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }

                // Add rule directly using GenericRule - zero-conversion storage
                let rule = GenericRule::new(items[1].clone(), items[2].clone());
                let mut new_env = env.clone();
                new_env.add_generic_rule(rule);

                // Rule definitions return empty list
                return GenericEvalStep::Done((vec![], new_env));
            }

            // Force evaluation operator - defer to trampoline for TCO
            "!" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "! requires exactly 1 argument, got {}. Usage: (! expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                // Defer evaluation to trampoline - this IS a tail call (TCO)
                return GenericEvalStep::EvalIfBranch {
                    branch: items[1].clone(),
                    env,
                    depth,
                };
            }

            // Quote - returns argument unevaluated (NO conversion needed)
            "quote" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "quote requires exactly 1 argument, got {}. Usage: (quote expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::Done((vec![items[1].clone()], env));
            }

            // Conditional - defers condition evaluation to trampoline
            "if" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "if requires exactly 3 arguments, got {}. Usage: (if condition then else)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::EvalIfCondition {
                    condition: items[1].clone(),
                    then_branch: items[2].clone(),
                    else_branch: items[3].clone(),
                    env,
                    depth,
                };
            }

            // Error construction (NO conversion needed)
            "error" => {
                if items.len() < 2 {
                    return GenericEvalStep::Done((vec![], env));
                }
                // Extract message from atom or string
                let msg = items[1].as_string()
                    .or_else(|| items[1].as_atom())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{:?}", items[1]));
                let details = if items.len() > 2 {
                    items[2].clone()
                } else {
                    ctx.factory().unit()
                };
                return GenericEvalStep::Done((vec![ctx.factory().error(&msg, details)], env));
            }

            // HE-compatible Error form (NO conversion needed)
            "Error" => {
                if items.len() < 2 {
                    return GenericEvalStep::Done((vec![], env));
                }
                // HE format: (Error details msg)
                let (details, msg) = if items.len() == 2 {
                    (items[1].clone(), String::new())
                } else {
                    let details = items[1].clone();
                    let msg = items[2].as_string()
                        .or_else(|| items[2].as_atom())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("{:?}", items[2]));
                    (details, msg)
                };
                return GenericEvalStep::Done((vec![ctx.factory().error(&msg, details)], env));
            }

            // is-error - defers evaluation to trampoline
            "is-error" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "is-error requires exactly 1 argument, got {}. Usage: (is-error expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::EvalIsError {
                    expr: items[1].clone(),
                    env,
                    depth,
                };
            }

            // catch - defers evaluation to trampoline
            "catch" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "catch requires exactly 2 arguments, got {}. Usage: (catch expr default)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartCatch {
                    expr: items[1].clone(),
                    default: items[2].clone(),
                    env,
                    depth,
                };
            }

            // eval - defers evaluation to trampoline
            "eval" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "eval requires exactly 1 argument, got {}. Usage: (eval expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::EvalEval {
                    arg: items[1].clone(),
                    env,
                    depth,
                };
            }

            // function - defers evaluation to trampoline
            "function" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "function requires exactly 1 argument, got {}. Usage: (function expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartFunction {
                    expr: items[1].clone(),
                    env,
                    depth,
                };
            }

            // return - defers evaluation to trampoline
            "return" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "return requires exactly 1 argument, got {}. Usage: (return value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::EvalReturn {
                    value: items[1].clone(),
                    env,
                    depth,
                };
            }

            // chain - defers evaluation to trampoline
            "chain" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "chain requires exactly 3 arguments, got {}. Usage: (chain expr $var body)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartChain {
                    expr: items[1].clone(),
                    var: items[2].clone(),
                    body: items[3].clone(),
                    env,
                    depth,
                };
            }

            // match - defers space evaluation to trampoline
            // Supports: (match space pattern template) - 3 args
            //       or: (match & self pattern template) - 4 args (legacy, preprocessed to 3)
            "match" => {
                if !(items.len() == 4 || items.len() == 5) {
                    let err = ctx.factory().error(
                        &format!(
                            "match requires 3 or 4 arguments, got {}. Usage: (match space pattern template) or (match & self pattern template)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                // Handle both syntaxes
                if items.len() == 4 {
                    // New syntax: (match space pattern template)
                    return GenericEvalStep::StartMatch {
                        space_arg: items[1].clone(),
                        pattern: items[2].clone(),
                        template: items[3].clone(),
                        env,
                        depth,
                    };
                } else {
                    // Legacy syntax: (match & self pattern template)
                    // The & and self should have been preprocessed into &self
                    // If not, this is an error
                    let err = ctx.factory().error(
                        &format!(
                            "match requires & as first argument (legacy syntax), got: {}",
                            if let Some(atom) = items[1].as_atom() { atom.to_string() } else { "non-atom".to_string() }
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
            }

            // case - defers atom evaluation to trampoline
            "case" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "case requires exactly 2 arguments, got {}. Usage: (case atom ((pattern template) ...))",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::EvalCaseAtom {
                    atom: items[1].clone(),
                    cases: items[2].clone(),
                    env,
                    depth,
                };
            }

            // switch - pattern matches atom WITHOUT evaluation (unlike case)
            "switch" | "switch-minimal" | "switch-internal" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "{} requires exactly 2 arguments, got {}. Usage: ({} atom cases)",
                            op,
                            items.len() - 1,
                            op
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::SwitchAtom {
                    atom: items[1].clone(),
                    cases: items[2].clone(),
                    env,
                    depth,
                };
            }

            // let - defers value evaluation to trampoline
            "let" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "let requires exactly 3 arguments, got {}. Usage: (let pattern value body)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartLetBinding {
                    pattern: items[1].clone(),
                    value_expr: items[2].clone(),
                    body: items[3].clone(),
                    env,
                    depth,
                };
            }

            // let* - native generic implementation (zero conversion)
            // Sequential bindings - desugars to nested let
            "let*" => {
                if items.len() < 3 {
                    let err = ctx.factory().error(
                        "let* requires at least 2 arguments. Usage: (let* ((pat val) ...) body)",
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }

                let bindings_expr = &items[1];
                let body = &items[2];

                // Extract bindings list
                let bindings = match bindings_expr.as_sexpr() {
                    Some(items) => items,
                    None if bindings_expr.is_unit() => {
                        // Empty bindings - evaluate body via trampoline (tail call)
                        return GenericEvalStep::EvalIfBranch {
                            branch: body.clone(),
                            env,
                            depth,
                        };
                    }
                    None => {
                        let err = ctx.factory().error(
                            "let* bindings must be a list. Usage: (let* ((pattern value) ...) body)",
                            bindings_expr.clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                };

                if bindings.is_empty() {
                    // No bindings - evaluate body via trampoline (tail call)
                    return GenericEvalStep::EvalIfBranch {
                        branch: body.clone(),
                        env,
                        depth,
                    };
                }

                // Transform to nested let
                // (let* ((a 1) (b 2) (c 3)) body) -> (let a 1 (let b 2 (let c 3 body)))
                let mut result_body = body.clone();

                // Process bindings in reverse order to build nested structure
                for binding in bindings.iter().rev() {
                    if let Some(pair) = binding.as_sexpr() {
                        if pair.len() == 2 {
                            let pattern = &pair[0];
                            let value = &pair[1];

                            result_body = ctx.factory().sexpr(vec![
                                ctx.factory().atom("let"),
                                pattern.clone(),
                                value.clone(),
                                result_body,
                            ]);
                        } else {
                            let err = ctx.factory().error(
                                "let* binding must be (pattern value) pair. Usage: (let* ((pattern value) ...) body)",
                                binding.clone(),
                            );
                            return GenericEvalStep::Done((vec![err], env));
                        }
                    } else {
                        let err = ctx.factory().error(
                            "let* binding must be (pattern value) pair. Usage: (let* ((pattern value) ...) body)",
                            binding.clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                }

                // Evaluate the nested let structure via trampoline (tail call)
                return GenericEvalStep::EvalIfBranch {
                    branch: result_body,
                    env,
                    depth,
                };
            }

            // unify - defers pattern evaluation to trampoline
            "unify" => {
                if items.len() != 5 {
                    let err = ctx.factory().error(
                        &format!(
                            "unify requires exactly 4 arguments, got {}. Usage: (unify pattern1 pattern2 success failure)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartUnify {
                    pattern1: items[1].clone(),
                    pattern2: items[2].clone(),
                    success_body: items[3].clone(),
                    failure_body: items[4].clone(),
                    env,
                    depth,
                };
            }

            // sealed - native generic implementation (zero conversion)
            "sealed" => {
                use crate::backend::eval::bindings_generic::eval_sealed_generic;
                let results = eval_sealed_generic(&items, ctx.factory());
                return GenericEvalStep::Done((results, env));
            }

            // atom-subst - native generic implementation (zero conversion)
            "atom-subst" => {
                use crate::backend::eval::bindings_generic::eval_atom_subst_generic;
                let results = eval_atom_subst_generic(&items, ctx.factory());
                return GenericEvalStep::Done((results, env));
            }

            // Type assertion - native generic implementation (zero-conversion)
            ":" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            ": requires exactly 2 arguments, got {}. Usage: (: expr type)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }

                // Extract name from expression (atom or first element of sexpr)
                let name = if let Some(atom) = items[1].as_atom() {
                    atom.to_string()
                } else if let Some(expr_items) = items[1].as_sexpr() {
                    if let Some(first) = expr_items.first() {
                        if let Some(atom) = first.as_atom() {
                            atom.to_string()
                        } else {
                            format!("{:?}", items[1])
                        }
                    } else {
                        format!("{:?}", items[1])
                    }
                } else {
                    format!("{:?}", items[1])
                };

                // Add type directly (V is already the correct type)
                let mut new_env = env.clone();
                new_env.add_type_generic(&name, items[2].clone());

                // Type assertions return empty list
                return GenericEvalStep::Done((vec![], new_env));
            }

            // get-type - native generic implementation
            "get-type" => {
                use crate::backend::eval::types_generic::eval_get_type_generic;
                let results = eval_get_type_generic(&items, ctx.factory(), &env);
                return GenericEvalStep::Done((results, env));
            }

            // check-type - native generic implementation
            "check-type" => {
                use crate::backend::eval::types_generic::eval_check_type_generic;
                let results = eval_check_type_generic(&items, ctx.factory(), &env);
                return GenericEvalStep::Done((results, env));
            }

            // map-atom - defers iteration to trampoline
            "map-atom" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "map-atom requires exactly 3 arguments, got {}. Usage: (map-atom list $var template)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                // Extract list elements
                let list_arg = &items[1];
                let var_arg = &items[2];
                let template = items[3].clone();

                // Get variable name - match heap engine's error message format
                let var_name = match var_arg.as_atom() {
                    Some(name) if name.starts_with('$') => name.to_string(),
                    Some(name) => {
                        // Atom but not a variable - provide helpful suggestion
                        let msg = match suggest_variable_format(name) {
                            Some(suggestion) => format!(
                                "map-atom: second argument must be a variable (starting with $). {}",
                                suggestion
                            ),
                            None => {
                                "map-atom: second argument must be a variable (starting with $)".to_string()
                            }
                        };
                        let err = ctx.factory().error(&msg, var_arg.clone());
                        return GenericEvalStep::Done((vec![err], env));
                    }
                    None => {
                        // Not an atom at all
                        let err = ctx.factory().error(
                            "map-atom: second argument must be a variable (starting with $)",
                            var_arg.clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                };

                // Extract elements from list (Nil is treated as empty list)
                let elements: Vec<C::Value> = if list_arg.is_unit() {
                    vec![]
                } else {
                    match list_arg.as_sexpr() {
                        Some(elems) => elems.iter().cloned().collect(),
                        None => {
                            let err = ctx.factory().error(
                                "map-atom requires a list as first argument",
                                ctx.factory().sexpr(items),
                            );
                            return GenericEvalStep::Done((vec![err], env));
                        }
                    }
                };

                return GenericEvalStep::StartMapAtom {
                    elements,
                    var_name,
                    template,
                    env,
                    depth,
                };
            }

            // filter-atom - defers iteration to trampoline
            "filter-atom" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "filter-atom requires exactly 3 arguments, got {}. Usage: (filter-atom list $var predicate)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }

                let list_arg = &items[1];
                let var_arg = &items[2];
                let predicate = items[3].clone();

                // Get variable name - match heap engine's error message format
                let var_name = match var_arg.as_atom() {
                    Some(name) if name.starts_with('$') => name.to_string(),
                    Some(name) => {
                        // Atom but not a variable - provide helpful suggestion
                        let msg = match suggest_variable_format(name) {
                            Some(suggestion) => format!(
                                "filter-atom: second argument must be a variable (starting with $). {}",
                                suggestion
                            ),
                            None => {
                                "filter-atom: second argument must be a variable (starting with $)".to_string()
                            }
                        };
                        let err = ctx.factory().error(&msg, var_arg.clone());
                        return GenericEvalStep::Done((vec![err], env));
                    }
                    None => {
                        // Not an atom at all
                        let err = ctx.factory().error(
                            "filter-atom: second argument must be a variable (starting with $)",
                            var_arg.clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                };

                // Extract elements from list (Nil is treated as empty list)
                let elements: Vec<C::Value> = if list_arg.is_unit() {
                    vec![]
                } else {
                    match list_arg.as_sexpr() {
                        Some(elems) => elems.iter().cloned().collect(),
                        None => {
                            let err = ctx.factory().error(
                                "filter-atom requires a list as first argument",
                                ctx.factory().sexpr(items),
                            );
                            return GenericEvalStep::Done((vec![err], env));
                        }
                    }
                };

                return GenericEvalStep::StartFilterAtom {
                    elements,
                    var_name,
                    predicate,
                    env,
                    depth,
                };
            }

            // foldl-atom - defers iteration to trampoline
            "foldl-atom" => {
                if items.len() != 6 {
                    let err = ctx.factory().error(
                        &format!(
                            "foldl-atom requires exactly 5 arguments, got {}. Usage: (foldl-atom list init $acc $x op)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }

                let list_arg = &items[1];
                let init = items[2].clone();
                let acc_var = &items[3];
                let item_var = &items[4];
                let operation = items[5].clone();

                // Get accumulator variable name - match heap engine's error message format
                let acc_var_name = match acc_var.as_atom() {
                    Some(name) if name.starts_with('$') => name.to_string(),
                    Some(name) => {
                        // Atom but not a variable - provide helpful suggestion
                        let msg = match suggest_variable_format(name) {
                            Some(suggestion) => format!(
                                "foldl-atom: third argument must be a variable (starting with $). {}",
                                suggestion
                            ),
                            None => {
                                "foldl-atom: third argument must be a variable (starting with $)".to_string()
                            }
                        };
                        let err = ctx.factory().error(&msg, acc_var.clone());
                        return GenericEvalStep::Done((vec![err], env));
                    }
                    None => {
                        // Not an atom at all
                        let err = ctx.factory().error(
                            "foldl-atom: third argument must be a variable (starting with $)",
                            acc_var.clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                };

                // Get item variable name - match heap engine's error message format
                let item_var_name = match item_var.as_atom() {
                    Some(name) if name.starts_with('$') => name.to_string(),
                    Some(name) => {
                        // Atom but not a variable - provide helpful suggestion
                        let msg = match suggest_variable_format(name) {
                            Some(suggestion) => format!(
                                "foldl-atom: fourth argument must be a variable (starting with $). {}",
                                suggestion
                            ),
                            None => {
                                "foldl-atom: fourth argument must be a variable (starting with $)".to_string()
                            }
                        };
                        let err = ctx.factory().error(&msg, item_var.clone());
                        return GenericEvalStep::Done((vec![err], env));
                    }
                    None => {
                        // Not an atom at all
                        let err = ctx.factory().error(
                            "foldl-atom: fourth argument must be a variable (starting with $)",
                            item_var.clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                };

                // Extract elements from list (Nil is treated as empty list)
                let elements: Vec<C::Value> = if list_arg.is_unit() {
                    vec![]
                } else {
                    match list_arg.as_sexpr() {
                        Some(elems) => elems.iter().cloned().collect(),
                        None => {
                            let err = ctx.factory().error(
                                "foldl-atom requires a list as first argument",
                                ctx.factory().sexpr(items),
                            );
                            return GenericEvalStep::Done((vec![err], env));
                        }
                    }
                };

                return GenericEvalStep::StartFoldlAtom {
                    elements,
                    init,
                    acc_var_name,
                    item_var_name,
                    operation,
                    env,
                    depth,
                };
            }

            // List operations - native generic implementations (zero conversion)
            "car-atom" | "cdr-atom" | "cons-atom" | "decons-atom" | "size-atom" | "max-atom"
            | "index-atom" | "min-atom" => {
                use crate::backend::eval::list_ops::generic::*;
                let results = match op {
                    "car-atom" => eval_car_atom_generic(&items, ctx.factory()),
                    "cdr-atom" => eval_cdr_atom_generic(&items, ctx.factory()),
                    "cons-atom" => eval_cons_atom_generic(&items, ctx.factory()),
                    "decons-atom" => eval_decons_atom_generic(&items, ctx.factory()),
                    "size-atom" => eval_size_atom_generic(&items, ctx.factory()),
                    "max-atom" => eval_max_atom_generic(&items, ctx.factory()),
                    "index-atom" => eval_index_atom_generic(&items, ctx.factory()),
                    "min-atom" => eval_min_atom_generic(&items, ctx.factory()),
                    _ => unreachable!(),
                };
                return GenericEvalStep::Done((results, env));
            }

            // Space operations - native generic implementation (zero-conversion)
            "new-space" => {
                use crate::backend::models::SpaceHandle;

                // Get optional name, default to "unnamed"
                let name = if items.len() > 1 {
                    if let Some(s) = items[1].as_string() {
                        s.to_string()
                    } else if let Some(s) = items[1].as_atom() {
                        s.to_string()
                    } else {
                        let err = ctx.factory().error(
                            &format!(
                                "new-space: optional name must be a string, got {:?}. Usage: (new-space) or (new-space \"name\")",
                                items[1]
                            ),
                            items[1].clone(),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                } else {
                    "unnamed".to_string()
                };

                // Create named space via GenericEnvironment
                let mut new_env = env.clone();
                let space_id = new_env.create_named_space(&name);
                let handle = SpaceHandle::new(space_id, name);

                // Return Space value using factory
                let space_val = ctx.factory().space(handle);
                return GenericEvalStep::Done((vec![space_val], new_env));
            }

            "add-atom" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "add-atom requires exactly 2 arguments, got {}. Usage: (add-atom space atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartAddAtom {
                    space_ref: items[1].clone(),
                    atom: items[2].clone(),
                    env,
                    depth,
                };
            }

            "remove-atom" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "remove-atom requires exactly 2 arguments, got {}. Usage: (remove-atom space atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartRemoveAtom {
                    space_ref: items[1].clone(),
                    atom: items[2].clone(),
                    env,
                    depth,
                };
            }

            "collapse" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "collapse requires exactly 1 argument, got {}. Usage: (collapse expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartCollapse {
                    expr: items[1].clone(),
                    env,
                    depth,
                };
            }

            "collapse-bind" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "collapse-bind requires exactly 1 argument, got {}. Usage: (collapse-bind expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartCollapseBind {
                    expr: items[1].clone(),
                    env,
                    depth,
                };
            }

            // superpose - native generic implementation (zero conversion)
            "superpose" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "superpose requires 1 argument, got {}. Usage: (superpose list)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                let expr = &items[1];
                // DON'T evaluate the argument - treat it as a data list (HE-compatible)
                if let Some(elements) = expr.as_sexpr() {
                    if elements.is_empty() {
                        // Empty superpose returns empty (no results) - nondeterministic failure
                        return GenericEvalStep::Done((vec![], env));
                    }
                    // Return each element as a separate result (nondeterministic)
                    return GenericEvalStep::Done((elements.to_vec(), env));
                }
                if expr.is_unit() {
                    // Unit superposes to empty (no results)
                    return GenericEvalStep::Done((vec![], env));
                }
                // Single value superposes to itself
                return GenericEvalStep::Done((vec![expr.clone()], env));
            }

            // Advanced nondeterminism
            "amb" => {
                if items.len() < 2 {
                    // Empty amb returns empty
                    return GenericEvalStep::Done((vec![], env));
                }
                let alternatives: Vec<C::Value> = items[1..].iter().cloned().collect();
                return GenericEvalStep::StartAmb {
                    alternatives,
                    env,
                    depth,
                };
            }

            "guard" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "guard requires exactly 1 argument, got {}. Usage: (guard condition)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartGuard {
                    condition: items[1].clone(),
                    env,
                    depth,
                };
            }

            // commit - native generic implementation (zero conversion)
            // In tree-walker evaluation, commit is a no-op - returns Unit
            "commit" => {
                return GenericEvalStep::Done((vec![ctx.factory().unit()], env));
            }

            // backtrack - native generic implementation (zero conversion)
            // Force immediate backtracking - returns empty (nondeterministic failure)
            "backtrack" => {
                return GenericEvalStep::Done((vec![], env));
            }

            "get-atoms" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "get-atoms requires exactly 1 argument, got {}. Usage: (get-atoms space)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartGetAtoms {
                    space_ref: items[1].clone(),
                    env,
                    depth,
                };
            }

            // State operations
            "new-state" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "new-state requires exactly 1 argument, got {}. Usage: (new-state initial-value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartNewState {
                    initial_value: items[1].clone(),
                    env,
                    depth,
                };
            }

            "get-state" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "get-state requires exactly 1 argument, got {}. Usage: (get-state state)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartGetState {
                    state_ref: items[1].clone(),
                    env,
                    depth,
                };
            }

            "change-state!" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "change-state! requires exactly 2 arguments, got {}. Usage: (change-state! state new-value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartChangeState {
                    state_ref: items[1].clone(),
                    new_value: items[2].clone(),
                    env,
                    depth,
                };
            }

            // Memoization operations
            "new-memo" => {
                if items.len() < 2 || items.len() > 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "new-memo requires 1-2 arguments, got {}. Usage: (new-memo name [size])",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                let size_arg = if items.len() == 3 {
                    Some(items[2].clone())
                } else {
                    None
                };
                return GenericEvalStep::StartNewMemo {
                    name_arg: items[1].clone(),
                    size_arg,
                    env,
                    depth,
                };
            }

            "memo" | "memo-first" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "{} requires exactly 2 arguments, got {}. Usage: ({} memo-table expr)",
                            op,
                            items.len() - 1,
                            op
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                let first_only = op == "memo-first";
                return GenericEvalStep::StartMemo {
                    memo_ref: items[1].clone(),
                    expr: items[2].clone(),
                    first_only,
                    env,
                    depth,
                };
            }

            "clear-memo!" | "memo-stats" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "{} requires exactly 1 argument, got {}. Usage: ({} memo-table)",
                            op,
                            items.len() - 1,
                            op
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                let op_type = if op == "clear-memo!" {
                    super::MemoOpType::Clear
                } else {
                    super::MemoOpType::Stats
                };
                return GenericEvalStep::StartMemoOp {
                    memo_ref: items[1].clone(),
                    op_type,
                    env,
                    depth,
                };
            }

            // Token binding
            "bind!" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "bind! requires exactly 2 arguments, got {}. Usage: (bind! token atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                let token = match items[1].as_atom() {
                    Some(t) => t.to_string(),
                    None => {
                        let err = ctx.factory().error(
                            "bind! requires an atom as first argument",
                            ctx.factory().sexpr(items),
                        );
                        return GenericEvalStep::Done((vec![err], env));
                    }
                };
                return GenericEvalStep::StartBind {
                    token,
                    atom_expr: items[2].clone(),
                    env,
                    depth,
                };
            }

            // I/O operations
            "println!" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "println! requires exactly 1 argument, got {}. Usage: (println! atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartPrintln {
                    atom: items[1].clone(),
                    env,
                    depth,
                };
            }

            "trace!" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "trace! requires exactly 2 arguments, got {}. Usage: (trace! message value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartTrace {
                    message: items[1].clone(),
                    value_expr: items[2].clone(),
                    env,
                    depth,
                };
            }

            // nop - returns Unit (NO conversion needed)
            "nop" => {
                return GenericEvalStep::Done((vec![ctx.factory().unit()], env));
            }

            // String operations
            "repr" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "repr requires exactly 1 argument, got {}. Usage: (repr atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartRepr {
                    atom: items[1].clone(),
                    env,
                    depth,
                };
            }

            "format-args" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "format-args requires exactly 2 arguments, got {}. Usage: (format-args format-string args)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartFormatArgs {
                    format_arg: items[1].clone(),
                    args_arg: items[2].clone(),
                    env,
                    depth,
                };
            }

            // empty - native generic implementation (zero conversion)
            // Returns the Empty sentinel atom - will be filtered at result collection
            "empty" => {
                return GenericEvalStep::Done((vec![ctx.factory().empty()], env));
            }

            "get-metatype" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "get-metatype requires exactly 1 argument, got {}. Usage: (get-metatype atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((vec![err], env));
                }
                return GenericEvalStep::StartGetMetatype {
                    atom: items[1].clone(),
                    env,
                    depth,
                };
            }

            // Module operations - use generic implementations directly (zero-conversion)
            "include" => {
                let (results, new_env) = eval_include_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }
            "import!" => {
                let (results, new_env) = eval_import_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }
            "mod-space!" => {
                let (results, new_env) = eval_mod_space_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }
            "print-mods!" => {
                let (results, new_env) = eval_print_mods_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }

            // MORK special forms - use generic implementations directly (zero-conversion)
            "exec" => {
                let (results, new_env) = eval_exec_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }
            "coalg" => {
                let (results, new_env) = eval_coalg_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }
            "lookup" => {
                let (results, new_env) = eval_lookup_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }
            "rulify" => {
                let (results, new_env) = eval_rulify_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((results, new_env));
            }

            _ => {}
        }
    }

    // HE-compatible lazy evaluation: try grounded operations and rules with UNEVALUATED args first
    // Step 1: Try grounded operations with RAW (unevaluated) arguments
    if let Some(op) = items.first().and_then(|v| v.as_atom()) {
        // Try generic grounded operation first (zero-conversion path)
        // Uses static dispatch - works with any V: MettaValueTrait
        if has_generic_grounded_op(op) {
            // Use GenericGroundedState with native value type - NO conversion needed
            let args: Vec<C::Value> = items[1..].to_vec();
            let state = GenericGroundedState::new(op.to_string(), args);
            return GenericEvalStep::StartGroundedOp { state, env, depth };
        }

        // TCO operations are also in the generic registry now - use generic path
        // All standard operations (+, -, *, /, <, >, ==, and, or, not) are in generic registry
        if env.get_grounded_operation_tco(op).is_some() {
            let args: Vec<C::Value> = items[1..].to_vec();
            let state = GenericGroundedState::new(op.to_string(), args);
            return GenericEvalStep::StartGroundedOp { state, env, depth };
        }

        // Note: Legacy grounded operations (non-TCO) are no longer supported in the generic
        // evaluator. All standard operations should be in the generic registry. If a custom
        // operation needs to be supported, it should be added to the generic registry.
    }

    // Step 2: Check for grounded args that need evaluation BEFORE rule matching
    // Use generic version to avoid heap conversion
    let grounded_indices = find_grounded_arg_indices_generic(&items, &env);
    if !grounded_indices.is_empty() {
        return GenericEvalStep::EvalGroundedArgs {
            items,
            grounded_indices,
            env,
            depth,
        };
    }

    // Step 3: No grounded args - proceed with rule matching directly
    // Skip token resolution for now - not required with GenericEnvironment.
    // Items are typically already resolved in the evaluation context.
    let resolved_sexpr = ctx.factory().sexpr(items.clone());
    let all_matches = crate::backend::eval::trampoline::try_match_all_rules_generic(&resolved_sexpr, &env, *ctx.factory());

    if !all_matches.is_empty() {
        // User rules matched - evaluate RHS with bindings from pattern match
        // Already generic types - no conversion needed!
        return GenericEvalStep::EvalRuleMatchesLazy {
            matches: all_matches,
            env,
            depth,
        };
    }

    // Step 4: No lazy rules matched - expression is irreducible (data constructor)
    GenericEvalStep::EvalSExpr { items, env, depth }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Preprocess space references: combine `& self` into `&self`.
fn preprocess_space_refs_generic<C: EvalContext>(
    items: Vec<C::Value>,
    ctx: &C,
) -> Vec<C::Value>
where
    C::Value: Clone,
{
    // Look for pattern: [... , "&", "self", ...]
    // and combine into [... , "&self", ...]
    let mut result = Vec::with_capacity(items.len());
    let mut i = 0;

    while i < items.len() {
        if i + 1 < items.len() {
            if let (Some("&"), Some("self")) = (items[i].as_atom(), items[i + 1].as_atom()) {
                result.push(ctx.factory().atom("&self"));
                i += 2;
                continue;
            }
        }
        result.push(items[i].clone());
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::trampoline::StaticArenaContext;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_eval_sexpr_step_generic_empty() {
        let ctx = StaticArenaContext::get();
        let env = StaticArenaContext::new_env();

        match eval_sexpr_step_generic(vec![], env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert!(results[0].is_sexpr());
                assert_eq!(results[0].as_sexpr().map(|s| s.len()), Some(0));
            }
            _ => panic!("Expected Done with empty sexpr"),
        }
    }

    #[test]
    fn test_eval_sexpr_step_generic_quote() {
        let ctx = StaticArenaContext::get();
        let env = StaticArenaContext::new_env();
        let factory = ctx.factory();

        let items = vec![
            factory.atom("quote"),
            factory.atom("foo"),
        ];

        match eval_sexpr_step_generic(items, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_atom(), Some("foo"));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_sexpr_step_generic_if_returns_condition_step() {
        let ctx = StaticArenaContext::get();
        let env = StaticArenaContext::new_env();
        let factory = ctx.factory();

        let items = vec![
            factory.atom("if"),
            factory.bool(true),
            factory.long(1),
            factory.long(2),
        ];

        match eval_sexpr_step_generic(items, env, 0, &ctx) {
            GenericEvalStep::EvalIfCondition { condition, then_branch, else_branch, .. } => {
                assert_eq!(condition.as_bool(), Some(true));
                assert_eq!(then_branch.as_long(), Some(1));
                assert_eq!(else_branch.as_long(), Some(2));
            }
            _ => panic!("Expected EvalIfCondition"),
        }
    }

    #[test]
    fn test_preprocess_space_refs_generic() {
        let ctx = StaticArenaContext::get();
        let factory = ctx.factory();

        let items = vec![
            factory.atom("match"),
            factory.atom("&"),
            factory.atom("self"),
            factory.atom("foo"),
        ];

        let result = preprocess_space_refs_generic(items, &ctx);
        assert_eq!(result.len(), 3);
        assert_eq!(result[1].as_atom(), Some("&self"));
    }
}
