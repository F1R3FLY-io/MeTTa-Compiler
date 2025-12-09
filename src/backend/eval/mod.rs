// Eval function: Lazy evaluation with pattern matching and built-in dispatch
//
// eval(a: atom, env) = a, env
// eval((t1 .. tn), env):
//   r1, env_1 = eval(t1, env) | ... | rn, env_n = eval(tn, env)
//   env' = union env_i
//   return fold over rules & grounded functions (emptyset, env')

#[macro_use]
mod macros;

mod bindings;
mod control_flow;
mod errors;
mod evaluation;
pub mod fixed_point;
mod list_ops;
mod mork_forms;
pub mod priority;
mod quoting;
mod space;
mod types;

use std::collections::VecDeque;
use std::sync::Arc;
use tracing::{debug, trace, warn};

use crate::backend::environment::Environment;
use crate::backend::models::{Bindings, EvalResult, MettaValue, Rule};
use crate::backend::mork_convert::{mork_bindings_to_metta, ConversionContext};
use mork_expr::Expr;

// =============================================================================
// Iterative Trampoline Types
// =============================================================================
// These types enable iterative evaluation using an explicit work stack instead
// of recursive function calls. This prevents stack overflow for large expressions.

/// Work item representing pending evaluation work
#[derive(Debug)]
enum WorkItem {
    /// Evaluate a value and send result to continuation
    Eval {
        value: MettaValue,
        env: Environment,
        depth: usize,
        cont_id: usize,
    },
    /// Resume a continuation with a result
    Resume { cont_id: usize, result: EvalResult },
}

/// Continuation representing what to do with an evaluation result
#[derive(Debug)]
enum Continuation {
    /// Final result - return from eval()
    Done,
    /// Collecting S-expression sub-results before processing
    CollectSExpr {
        /// Items still to evaluate (VecDeque for O(1) pop_front)
        remaining: VecDeque<MettaValue>,
        /// Results collected so far: (results_vec, env)
        collected: Vec<EvalResult>,
        /// Original environment for the S-expression
        original_env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after processing
        parent_cont: usize,
    },
    /// Processing rule match results
    ProcessRuleMatches {
        /// Remaining (rhs, bindings) pairs to evaluate (VecDeque for O(1) pop_front)
        remaining_matches: VecDeque<(MettaValue, Bindings)>,
        /// Results accumulated so far
        results: Vec<MettaValue>,
        /// Environment
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
}

/// Maximum evaluation depth to prevent stack overflow
/// This limits how deep the evaluation can recurse through nested expressions
/// Set to 1000 to allow legitimate deep nesting while still catching runaway recursion
const MAX_EVAL_DEPTH: usize = 1000;

/// Maximum number of results in Cartesian product to prevent combinatorial explosion
/// This limits the total number of combinations explored during nondeterministic evaluation
const MAX_CARTESIAN_RESULTS: usize = 10000;

/// MeTTa special forms for "did you mean" suggestions during evaluation
const SPECIAL_FORMS: &[&str] = &[
    "=",
    "!",
    "quote",
    "if",
    "error",
    "is-error",
    "catch",
    "eval",
    "function",
    "return",
    "chain",
    "match",
    "case",
    "switch",
    "let",
    ":",
    "get-type",
    "check-type",
    "map-atom",
    "filter-atom",
    "foldl-atom",
];

/// Convert MettaValue to a friendly type name for error messages
/// This provides user-friendly type names instead of debug format like "Long(5)"
fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value {
        MettaValue::Long(_) => "Number (integer)",
        MettaValue::Float(_) => "Number (float)",
        MettaValue::Bool(_) => "Bool",
        MettaValue::String(_) => "String",
        MettaValue::Atom(_) => "Atom",
        MettaValue::Nil => "Nil",
        MettaValue::SExpr(_) => "S-expression",
        MettaValue::Error(_, _) => "Error",
        MettaValue::Type(_) => "Type",
        MettaValue::Conjunction(_) => "Conjunction",
    }
}

/// Convert MettaValue to a user-friendly representation for error messages
/// Unlike debug format, this shows values in MeTTa syntax
pub(crate) fn friendly_value_repr(value: &MettaValue) -> String {
    match value {
        MettaValue::Long(n) => n.to_string(),
        MettaValue::Float(f) => f.to_string(),
        MettaValue::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MettaValue::String(s) => format!("\"{}\"", s),
        MettaValue::Atom(a) => a.clone(),
        MettaValue::Nil => "Nil".to_string(),
        MettaValue::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(friendly_value_repr).collect();
            format!("({})", inner.join(" "))
        }
        MettaValue::Error(msg, _) => format!("(error \"{}\")", msg),
        MettaValue::Type(t) => format!("(: {})", friendly_value_repr(t)),
        MettaValue::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(friendly_value_repr).collect();
            format!("(, {})", inner.join(" "))
        }
    }
}

/// Check if an operator is close to a known special form
/// Uses max edit distance of 1 to avoid false positives on short words
fn suggest_special_form(op: &str) -> Option<String> {
    use crate::backend::fuzzy_match::FuzzyMatcher;
    use std::sync::OnceLock;

    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    let matcher = MATCHER.get_or_init(|| FuzzyMatcher::from_terms(SPECIAL_FORMS.iter().copied()));

    matcher.did_you_mean(op, 1, 3)
}

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
/// This is the public entry point that uses iterative evaluation with an explicit work stack
/// to prevent stack overflow for large expressions.
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    debug!(metta_val = ?value);
    eval_trampoline(value, env)
}

/// Iterative evaluation using a trampoline pattern with explicit work stack.
/// This prevents stack overflow by using heap-allocated work items instead of
/// recursive function calls.
fn eval_trampoline(value: MettaValue, env: Environment) -> EvalResult {
    debug!(target: "mettatron::backend::eval::eval_trampoline", metta_val = ?value);

    // Initialize work stack with the initial evaluation
    let mut work_stack: Vec<WorkItem> = vec![WorkItem::Eval {
        value,
        env: env.clone(),
        depth: 0,
        cont_id: 0, // Done continuation
    }];

    // Continuation storage - index 0 is always Done
    let mut continuations: Vec<Continuation> = vec![Continuation::Done];

    // Final result storage
    let mut final_result: Option<EvalResult> = None;

    // Main trampoline loop
    while let Some(work) = work_stack.pop() {
        match work {
            WorkItem::Eval {
                value,
                env,
                depth,
                cont_id,
            } => {
                trace!(?value, depth, cont_id, "Processing work item");

                // Perform one step of evaluation
                let step_result = eval_step(value, env.clone(), depth);

                match step_result {
                    // Direct result - resume continuation
                    EvalStep::Done(result) => {
                        work_stack.push(WorkItem::Resume { cont_id, result });
                    }

                    // Need to evaluate S-expression sub-items
                    EvalStep::EvalSExpr { items, env, depth } => {
                        if items.is_empty() {
                            // Empty s-expression evaluates to Nil
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![MettaValue::Nil], env),
                            });
                        } else {
                            // Convert to VecDeque ONCE (O(n)) and pop front (O(1))
                            // This avoids O(n) slice copy + O(n) remove(0) = O(n²) total
                            let mut items_deque: VecDeque<MettaValue> = items.into_iter().collect();
                            let first = items_deque.pop_front().unwrap();

                            // Create continuation to collect results
                            let collect_cont_id = continuations.len();
                            continuations.push(Continuation::CollectSExpr {
                                remaining: items_deque, // Already a VecDeque, no copy needed
                                collected: Vec::new(),
                                original_env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate first item (moved, not cloned)
                            work_stack.push(WorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                cont_id: collect_cont_id,
                            });
                        }
                    }
                }
            }

            WorkItem::Resume { cont_id, result } => {
                // Take ownership of continuation for processing
                let cont = std::mem::replace(&mut continuations[cont_id], Continuation::Done);

                match cont {
                    Continuation::Done => {
                        // Final result
                        final_result = Some(result);
                    }

                    Continuation::CollectSExpr {
                        mut remaining,
                        mut collected,
                        original_env,
                        depth,
                        parent_cont,
                    } => {
                        // Add result to collected
                        collected.push(result);

                        if remaining.is_empty() {
                            // All items evaluated, process collected results
                            let processed = process_collected_sexpr(collected, original_env, depth);

                            match processed {
                                ProcessedSExpr::Done(result) => {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result,
                                    });
                                }
                                ProcessedSExpr::EvalRuleMatches {
                                    matches,
                                    env,
                                    depth,
                                    base_results,
                                } => {
                                    if matches.is_empty() {
                                        // No rule matches, return base results
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (base_results, env),
                                        });
                                    } else {
                                        // Convert to VecDeque ONCE and pop front (O(n) + O(1) vs O(n²))
                                        let mut matches_deque: VecDeque<_> =
                                            matches.into_iter().collect();
                                        let (rhs, bindings) = matches_deque.pop_front().unwrap();

                                        // Create continuation to process remaining rule matches
                                        let match_cont_id = continuations.len();
                                        continuations.push(Continuation::ProcessRuleMatches {
                                            remaining_matches: matches_deque,
                                            results: base_results,
                                            env: env.clone(),
                                            depth,
                                            parent_cont,
                                        });

                                        // Evaluate first rule RHS (values moved, not cloned)
                                        let instantiated_rhs = apply_bindings(&rhs, &bindings);
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_rhs,
                                            env,
                                            depth: depth + 1,
                                            cont_id: match_cont_id,
                                        });
                                    }
                                }
                            }
                        } else {
                            // More items to evaluate - O(1) pop from VecDeque front
                            let next = remaining.pop_front().unwrap();

                            // Put continuation back (modified)
                            continuations[cont_id] = Continuation::CollectSExpr {
                                remaining,
                                collected,
                                original_env: original_env.clone(),
                                depth,
                                parent_cont,
                            };

                            // Evaluate next item
                            work_stack.push(WorkItem::Eval {
                                value: next,
                                env: original_env,
                                depth: depth + 1,
                                cont_id,
                            });
                        }
                    }

                    Continuation::ProcessRuleMatches {
                        mut remaining_matches,
                        mut results,
                        env,
                        depth,
                        parent_cont,
                    } => {
                        // Add results from this rule evaluation
                        results.extend(result.0);

                        if remaining_matches.is_empty() {
                            // All rules evaluated
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (results, env),
                            });
                        } else {
                            // More rules to evaluate - O(1) pop from VecDeque front
                            let (rhs, bindings) = remaining_matches.pop_front().unwrap();

                            // Put continuation back (modified)
                            continuations[cont_id] = Continuation::ProcessRuleMatches {
                                remaining_matches,
                                results,
                                env: env.clone(),
                                depth,
                                parent_cont,
                            };

                            // Evaluate next rule RHS
                            let instantiated_rhs = apply_bindings(&rhs, &bindings);
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth: depth + 1,
                                cont_id,
                            });
                        }
                    }
                }
            }
        }
    }

    final_result.unwrap_or_else(|| (vec![], env))
}

/// Result of a single evaluation step
enum EvalStep {
    /// Evaluation complete, return this result
    Done(EvalResult),
    /// Need to evaluate S-expression items (iteratively)
    EvalSExpr {
        items: Vec<MettaValue>,
        env: Environment,
        depth: usize,
    },
}

/// Result of processing collected S-expression results
enum ProcessedSExpr {
    /// Processing complete, return this result
    Done(EvalResult),
    /// Need to evaluate rule matches
    EvalRuleMatches {
        matches: Vec<(MettaValue, Bindings)>,
        env: Environment,
        depth: usize,
        base_results: Vec<MettaValue>,
    },
}

/// Perform a single step of evaluation.
/// Returns either a final result or indicates more work is needed.
fn eval_step(value: MettaValue, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::step", ?value, depth);

    // Check depth limit
    if depth > MAX_EVAL_DEPTH {
        warn!(
            depth = depth,
            max_depth = MAX_EVAL_DEPTH,
            "Maximum evaluation depth exceeded - possible infinite recursion or combinatorial explosion"
        );

        return EvalStep::Done((
            vec![MettaValue::Error(
                format!(
                    "Maximum evaluation depth ({}) exceeded. Possible causes:\n\
                     - Infinite recursion: check for missing base case in recursive rules\n\
                     - Combinatorial explosion: rule produces too many branches\n\
                     Hint: Use (function ...) and (return ...) for tail-recursive evaluation",
                    MAX_EVAL_DEPTH
                ),
                Arc::new(value),
            )],
            env,
        ));
    }

    match value {
        // Errors propagate immediately
        MettaValue::Error(_, _) => EvalStep::Done((vec![value], env)),

        // Atoms evaluate to themselves
        MettaValue::Atom(_) => EvalStep::Done((vec![value], env)),

        // Ground types evaluate to themselves
        MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Type(_) => EvalStep::Done((vec![value], env)),

        // S-expressions need special handling
        MettaValue::SExpr(items) => eval_sexpr_step(items, env, depth),

        // For conjunctions, evaluate goals left-to-right with binding threading
        MettaValue::Conjunction(goals) => EvalStep::Done(eval_conjunction(goals, env, depth)),
    }
}

/// Evaluate an S-expression step - handles special forms and delegates to iterative collection
fn eval_sexpr_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_sexpr_step",?items, depth);
    if items.is_empty() {
        return EvalStep::Done((vec![MettaValue::Nil], env));
    }

    // Check for special forms - these are handled directly (they manage their own recursion)
    if let Some(MettaValue::Atom(op)) = items.first() {
        match op.as_str() {
            "=" => return EvalStep::Done(space::eval_add(items, env)),
            "!" => return EvalStep::Done(evaluation::force_eval(items, env)),
            "quote" => return EvalStep::Done(quoting::eval_quote(items, env)),
            "if" => return EvalStep::Done(control_flow::eval_if(items, env)),
            "error" => return EvalStep::Done(errors::eval_error(items, env)),
            "is-error" => return EvalStep::Done(errors::eval_if_error(items, env)),
            "catch" => return EvalStep::Done(errors::eval_catch(items, env)),
            "eval" => return EvalStep::Done(evaluation::eval_eval(items, env)),
            "function" => return EvalStep::Done(evaluation::eval_function(items, env)),
            "return" => return EvalStep::Done(evaluation::eval_return(items, env)),
            "chain" => return EvalStep::Done(evaluation::eval_chain(items, env)),
            "match" => return EvalStep::Done(space::eval_match(items, env)),
            "case" => return EvalStep::Done(control_flow::eval_case(items, env)),
            "switch" => return EvalStep::Done(control_flow::eval_switch(items, env)),
            "switch-minimal" => {
                return EvalStep::Done(control_flow::eval_switch_minimal_handler(items, env))
            }
            "switch-internal" => {
                return EvalStep::Done(control_flow::eval_switch_internal_handler(items, env))
            }
            "let" => return EvalStep::Done(bindings::eval_let(items, env)),
            ":" => return EvalStep::Done(types::eval_type_assertion(items, env)),
            "get-type" => return EvalStep::Done(types::eval_get_type(items, env)),
            "check-type" => return EvalStep::Done(types::eval_check_type(items, env)),
            "map-atom" => return EvalStep::Done(list_ops::eval_map_atom(items, env)),
            "filter-atom" => return EvalStep::Done(list_ops::eval_filter_atom(items, env)),
            "foldl-atom" => return EvalStep::Done(list_ops::eval_foldl_atom(items, env)),
            // MORK Special Forms
            "exec" => return EvalStep::Done(mork_forms::eval_exec(items, env)),
            "coalg" => return EvalStep::Done(mork_forms::eval_coalg(items, env)),
            "lookup" => return EvalStep::Done(mork_forms::eval_lookup(items, env)),
            "rulify" => return EvalStep::Done(mork_forms::eval_rulify(items, env)),
            _ => {}
        }
    }

    // Not a special form - need to evaluate all sub-expressions iteratively
    EvalStep::EvalSExpr { items, env, depth }
}

/// Process collected S-expression evaluation results.
/// This handles Cartesian products, builtins, and rule matching.
fn process_collected_sexpr(
    collected: Vec<EvalResult>,
    original_env: Environment,
    depth: usize,
) -> ProcessedSExpr {
    trace!(target: "mettatron::backend::eval::process_collected_sexpr", ?collected, depth);

    // Check for errors in sub-expression results
    for (results, new_env) in &collected {
        if let Some(first) = results.first() {
            if matches!(first, MettaValue::Error(_, _)) {
                return ProcessedSExpr::Done((vec![first.clone()], new_env.clone()));
            }
        }
    }

    // Split results and environments
    let (eval_results, envs): (Vec<_>, Vec<_>) = collected.into_iter().unzip();

    // Union all environments
    let mut unified_env = original_env;
    for e in envs {
        unified_env = unified_env.union(&e);
    }

    // Generate Cartesian product of all sub-expression results
    let combinations = match cartesian_product(&eval_results) {
        Ok(c) => c,
        Err(err) => {
            warn!(
                target: "mettatron::backend::eval::process_collected_sexpr",
                ?err, "Cartesian product processing error detected"
            );
            return ProcessedSExpr::Done((vec![err], unified_env));
        }
    };

    // Collect results and rule matches that need evaluation
    let mut all_final_results = Vec::new();
    let mut rule_matches_to_eval: Vec<(MettaValue, Bindings)> = Vec::new();

    for evaled_items in combinations {
        // Check if this is a grounded operation
        if let Some(MettaValue::Atom(op)) = evaled_items.first() {
            if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
                all_final_results.push(result);
                continue;
            }
        }

        // Try to match against rules
        let sexpr = MettaValue::SExpr(evaled_items.clone());
        let all_matches = try_match_all_rules(&sexpr, &unified_env);

        if !all_matches.is_empty() {
            // Collect rule matches for later evaluation
            rule_matches_to_eval.extend(all_matches);
        } else {
            // No rule matched - check for typos and handle ADD mode
            let result = handle_no_rule_match(evaled_items, &sexpr, &mut unified_env);
            all_final_results.push(result);
        }
    }

    if rule_matches_to_eval.is_empty() {
        // No rules to evaluate, we're done
        ProcessedSExpr::Done((all_final_results, unified_env))
    } else {
        // Need to evaluate rule matches iteratively
        ProcessedSExpr::EvalRuleMatches {
            matches: rule_matches_to_eval,
            env: unified_env,
            depth,
            base_results: all_final_results,
        }
    }
}

/// Handle the case where no rule matches an s-expression
fn handle_no_rule_match(
    evaled_items: Vec<MettaValue>,
    sexpr: &MettaValue,
    unified_env: &mut Environment,
) -> MettaValue {
    // Check for likely typos before falling back to ADD mode
    if let Some(MettaValue::Atom(head)) = evaled_items.first() {
        if head.len() >= 3 {
            // Suggest possible misspelling of special form, but don't error
            if let Some(suggestion) = suggest_special_form(head) {
                trace!(
                    target: "mettatron::backend::eval::handle_no_rule_match",
                    head, suggestion, "Unknown special form"
                );
            }
            // Check for misspelled rule head
            if let Some(suggestion) = unified_env.did_you_mean(head, 1) {
                trace!(
                    target: "mettatron::backend::eval::handle_no_rule_match",
                    head, suggestion, "No rule matches"
                );
                return MettaValue::Error(
                    format!("No rule matches '{}'. {}", head, suggestion),
                    Arc::new(sexpr.clone()),
                );
            }
        }
    }

    // ADD mode: add to space and return unreduced s-expression
    // In official MeTTa's default ADD mode, bare expressions are automatically added to &self
    unified_env.add_to_space(sexpr);
    sexpr.clone()
}

/// Evaluate a conjunction: (,), (, expr), or (, expr1 expr2 ...)
/// Implements MORK-style goal evaluation with left-to-right binding threading
///
/// Semantics:
/// - (,)          → succeed with empty result (always true)
/// - (, expr)     → evaluate expr directly (unary passthrough)
/// - (, e1 e2 ... en) → evaluate goals left-to-right, threading bindings through
fn eval_conjunction(goals: Vec<MettaValue>, env: Environment, _depth: usize) -> EvalResult {
    // Empty conjunction: (,) succeeds with empty result
    if goals.is_empty() {
        return (vec![MettaValue::Nil], env);
    }

    // Unary conjunction: (, expr) evaluates expr directly
    if goals.len() == 1 {
        return eval(goals[0].clone(), env);
    }

    // N-ary conjunction: evaluate left-to-right with binding threading
    // Start with the first goal
    let (mut results, mut current_env) = eval(goals[0].clone(), env);

    // For each subsequent goal, evaluate it in the context of previous results
    for goal in &goals[1..] {
        let mut next_results = Vec::new();

        // For each result from previous goals, evaluate the current goal
        for result in results {
            // If previous result is an error, propagate it
            if matches!(result, MettaValue::Error(_, _)) {
                next_results.push(result);
                continue;
            }

            // Evaluate the current goal
            let (goal_results, goal_env) = eval(goal.clone(), current_env.clone());

            // Union the environment
            current_env = current_env.union(&goal_env);

            // Collect all results from this goal
            next_results.extend(goal_results);
        }

        results = next_results;
    }

    (results, current_env)
}

/// Try to evaluate a built-in operation
/// Dispatches directly to built-in functions without going through Rholang interpreter
/// Uses operator symbols (+, -, *, etc.) instead of normalized names
fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    match op {
        "+" => eval_checked_arithmetic(args, |a, b| a.checked_add(b), "+"),
        "-" => eval_checked_arithmetic(args, |a, b| a.checked_sub(b), "-"),
        "*" => eval_checked_arithmetic(args, |a, b| a.checked_mul(b), "*"),
        "/" => eval_division(args),
        "<" => eval_comparison(args, |a, b| a < b),
        "<=" => eval_comparison(args, |a, b| a <= b),
        ">" => eval_comparison(args, |a, b| a > b),
        ">=" => eval_comparison(args, |a, b| a >= b),
        "==" => eval_comparison(args, |a, b| a == b),
        "!=" => eval_comparison(args, |a, b| a != b),
        // Logical operators
        "and" => eval_logical_binary(args, |a, b| a && b, "and"),
        "or" => eval_logical_binary(args, |a, b| a || b, "or"),
        "not" => eval_logical_not(args),
        _ => None,
    }
}

/// Evaluate a binary arithmetic operation with overflow checking
fn eval_checked_arithmetic<F>(args: &[MettaValue], op: F, op_name: &str) -> Option<MettaValue>
where
    F: Fn(i64, i64) -> Option<i64>,
{
    if args.len() != 2 {
        return Some(MettaValue::Error(
            format!(
                "Arithmetic operation '{}' requires exactly 2 arguments, got {}",
                op_name,
                args.len()
            ),
            Arc::new(MettaValue::Nil),
        ));
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot perform '{}': expected Number (integer), got {}",
                    op_name,
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot perform '{}': expected Number (integer), got {}",
                    op_name,
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    match op(a, b) {
        Some(result) => Some(MettaValue::Long(result)),
        None => Some(MettaValue::Error(
            format!(
                "Arithmetic overflow: {} {} {} exceeds integer bounds",
                a, op_name, b
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        )),
    }
}

/// Evaluate division with division-by-zero and overflow checking
fn eval_division(args: &[MettaValue]) -> Option<MettaValue> {
    if args.len() != 2 {
        return Some(MettaValue::Error(
            format!("Division requires exactly 2 arguments, got {}", args.len()),
            Arc::new(MettaValue::Nil),
        ));
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot divide: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot divide: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    if b == 0 {
        return Some(MettaValue::Error(
            "Division by zero".to_string(),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        ));
    }

    // Use checked_div for overflow protection (e.g., i64::MIN / -1)
    match a.checked_div(b) {
        Some(result) => Some(MettaValue::Long(result)),
        None => Some(MettaValue::Error(
            format!("Arithmetic overflow: {} / {} exceeds integer bounds", a, b),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        )),
    }
}

/// Evaluate a comparison operation with strict type checking
fn eval_comparison<F>(args: &[MettaValue], op: F) -> Option<MettaValue>
where
    F: Fn(i64, i64) -> bool,
{
    if args.len() != 2 {
        return Some(MettaValue::Error(
            format!(
                "Comparison operation requires exactly 2 arguments, got {}",
                args.len()
            ),
            Arc::new(MettaValue::Nil),
        ));
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot compare: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "Cannot compare: expected Number (integer), got {}",
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    Some(MettaValue::Bool(op(a, b)))
}

/// Evaluate a binary logical operation (and, or)
fn eval_logical_binary<F>(args: &[MettaValue], op: F, op_name: &str) -> Option<MettaValue>
where
    F: Fn(bool, bool) -> bool,
{
    if args.len() != 2 {
        return Some(MettaValue::Error(
            format!(
                "'{}' requires exactly 2 arguments, got {}. Usage: ({} bool1 bool2)",
                op_name,
                args.len(),
                op_name
            ),
            Arc::new(MettaValue::Atom("ArityError".to_string())),
        ));
    }

    let a = match &args[0] {
        MettaValue::Bool(b) => *b,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "'{}': expected Bool, got {}",
                    op_name,
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    let b = match &args[1] {
        MettaValue::Bool(b) => *b,
        other => {
            return Some(MettaValue::Error(
                format!(
                    "'{}': expected Bool, got {}",
                    op_name,
                    friendly_type_name(other)
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            ));
        }
    };

    Some(MettaValue::Bool(op(a, b)))
}

/// Evaluate logical not (unary)
fn eval_logical_not(args: &[MettaValue]) -> Option<MettaValue> {
    if args.len() != 1 {
        return Some(MettaValue::Error(
            format!(
                "'not' requires exactly 1 argument, got {}. Usage: (not bool)",
                args.len()
            ),
            Arc::new(MettaValue::Atom("ArityError".to_string())),
        ));
    }

    match &args[0] {
        MettaValue::Bool(b) => Some(MettaValue::Bool(!b)),
        other => Some(MettaValue::Error(
            format!("'not': expected Bool, got {}", friendly_type_name(other)),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        )),
    }
}

/// Pattern match a pattern against a value
/// Returns bindings if successful, None otherwise
///
/// This is made public to support optimized match operations in Environment
/// and for benchmarking the core pattern matching algorithm.
pub fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
    trace!(target: "mettatron::backend::eval::pattern_match", ?pattern, ?value);
    let mut bindings = Bindings::new();
    if pattern_match_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn pattern_match_impl(pattern: &MettaValue, value: &MettaValue, bindings: &mut Bindings) -> bool {
    match (pattern, value) {
        // Wildcard matches anything
        (MettaValue::Atom(p), _) if p == "_" => true,

        // FAST PATH: First variable binding (empty bindings)
        // Optimization: Skip lookup when bindings are empty - directly insert
        // This reduces single-variable regression from 16.8% to ~5-7%
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                && p != "&"
                && bindings.is_empty() =>
        {
            bindings.insert(p.clone(), v.clone());
            true
        }

        // GENERAL PATH: Variable with potential existing bindings
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\'')) && p != "&" =>
        {
            // Check if variable is already bound (linear search for SmartBindings)
            if let Some((_, existing)) = bindings.iter().find(|(name, _)| name.as_str() == p) {
                existing == v
            } else {
                bindings.insert(p.clone(), v.clone());
                true
            }
        }

        // Atoms must match exactly
        (MettaValue::Atom(p), MettaValue::Atom(v)) => p == v,
        (MettaValue::Bool(p), MettaValue::Bool(v)) => p == v,
        (MettaValue::Long(p), MettaValue::Long(v)) => p == v,
        (MettaValue::Float(p), MettaValue::Float(v)) => p == v,
        (MettaValue::String(p), MettaValue::String(v)) => p == v,
        (MettaValue::Nil, MettaValue::Nil) => true,

        // S-expressions must have same length and all elements must match
        (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) => {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !pattern_match_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        // Conjunctions must have same length and all goals must match
        (MettaValue::Conjunction(p_goals), MettaValue::Conjunction(v_goals)) => {
            if p_goals.len() != v_goals.len() {
                return false;
            }
            for (p, v) in p_goals.iter().zip(v_goals.iter()) {
                if !pattern_match_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        // Errors match if message and details match
        (MettaValue::Error(p_msg, p_details), MettaValue::Error(v_msg, v_details)) => {
            p_msg == v_msg && pattern_match_impl(p_details, v_details, bindings)
        }

        _ => false,
    }
}

/// Generate Cartesian product of evaluation results for nondeterministic evaluation
/// When sub-expressions return multiple results, we need to try all combinations
///
/// Example: [[a, b], [1, 2]] -> [[a, 1], [a, 2], [b, 1], [b, 2]]
///
/// This function has a built-in limit (MAX_CARTESIAN_RESULTS) to prevent combinatorial explosion.
/// Returns Err with an error message if the limit is exceeded.
fn cartesian_product(results: &[Vec<MettaValue>]) -> Result<Vec<Vec<MettaValue>>, MettaValue> {
    trace!(target: "mettatron::backend::eval::cartesian_product", ?results);
    if results.is_empty() {
        return Ok(vec![vec![]]);
    }

    // FAST PATH: If all result lists have exactly 1 item (deterministic evaluation),
    // we can just concatenate them directly in O(n) instead of O(n²)
    // This is the common case for arithmetic and most builtin operations
    if results.iter().all(|r| r.len() == 1) {
        let single_combo: Vec<MettaValue> = results.iter().map(|r| r[0].clone()).collect();
        trace!(
            target: "mettatron::backend::eval::cartesian_product",
            ?single_combo, "Concatenate all rules to deterministic evaluation"
        );
        return Ok(vec![single_combo]);
    }

    // Calculate the total product size first to check if it would exceed the limit
    let total_size: usize = results
        .iter()
        .map(|r| r.len().max(1))
        .fold(1usize, |acc, len| acc.saturating_mul(len));

    if total_size > MAX_CARTESIAN_RESULTS {
        return Err(MettaValue::Error(
            format!(
                "Combinatorial explosion: evaluation would produce {} results, exceeding limit of {}. \
                 Consider simplifying the expression or adding constraints.",
                total_size, MAX_CARTESIAN_RESULTS
            ),
            Arc::new(MettaValue::Atom("LimitExceeded".to_string())),
        ));
    }

    // Iterative Cartesian product for non-deterministic cases
    // Start with a single empty combination
    let mut product = vec![Vec::with_capacity(results.len())];

    // Process each result list and extend all existing combinations
    for result_list in results {
        if result_list.is_empty() {
            // Empty list contributes nothing to combinations
            continue;
        }

        let new_capacity = product
            .len()
            .checked_mul(result_list.len())
            .ok_or_else(|| {
                MettaValue::Error(
                    "Combinatorial explosion: integer overflow in cartesian product".to_string(),
                    Arc::new(MettaValue::Atom("Overflow".to_string())),
                )
            })?;
        let mut new_product = Vec::with_capacity(new_capacity);

        for combo in &product {
            for item in result_list {
                let mut new_combo = combo.clone();
                new_combo.push(item.clone());
                new_product.push(new_combo);
            }
        }

        product = new_product;
    }

    Ok(product)
}

/// Apply variable bindings to a value
///
/// This is made public to support optimized match operations in Environment
pub(crate) fn apply_bindings(value: &MettaValue, bindings: &Bindings) -> MettaValue {
    trace!(target: "mettatron::backend::eval::apply_bindings", ?value, ?bindings);
    match value {
        // Apply bindings to variables (atoms starting with $, &, or ')
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValue::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && s != "&" =>
        {
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == s)
                .map(|(_, val)| val.clone())
                .unwrap_or_else(|| value.clone())
        }
        MettaValue::SExpr(items) => {
            let new_items: Vec<_> = items
                .iter()
                .map(|item| apply_bindings(item, bindings))
                .collect();
            MettaValue::SExpr(new_items)
        }
        MettaValue::Conjunction(goals) => {
            let new_goals: Vec<_> = goals
                .iter()
                .map(|goal| apply_bindings(goal, bindings))
                .collect();
            MettaValue::Conjunction(new_goals)
        }
        MettaValue::Error(msg, details) => {
            let new_details = apply_bindings(details, bindings);
            MettaValue::Error(msg.clone(), Arc::new(new_details))
        }
        _ => value.clone(),
    }
}

/// Extract the head symbol from a pattern for indexing
/// Returns None if the pattern doesn't have a clear head symbol
fn get_head_symbol(pattern: &MettaValue) -> Option<&str> {
    trace!(target: "mettatron::backend::eval::get_head_symbol", ?pattern);
    match pattern {
        // For s-expressions like (double $x), extract "double"
        // EXCEPT: standalone "&" is allowed as a head symbol (used in match)
        MettaValue::SExpr(items) if !items.is_empty() => match &items[0] {
            MettaValue::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || head == "&")
                    && !head.starts_with('\'')
                    && head != "_" =>
            {
                Some(head.as_str())
            }
            _ => None,
        },
        // For bare atoms like foo, use the atom itself
        // EXCEPT: standalone "&" is allowed (used in match)
        MettaValue::Atom(head)
            if !head.starts_with('$')
                && (!head.starts_with('&') || head == "&")
                && !head.starts_with('\'')
                && head != "_" =>
        {
            Some(head.as_str())
        }
        _ => None,
    }
}

/// Compute the specificity of a pattern (lower is more specific)
/// More specific patterns have fewer variables
fn pattern_specificity(pattern: &MettaValue) -> usize {
    match pattern {
        // Variables are least specific
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValue::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') || s == "_")
                && s != "&" =>
        {
            1000 // Variables are least specific
        }
        MettaValue::Atom(_)
        | MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::String(_)
        | MettaValue::Nil => {
            0 // Literals are most specific (including standalone "&")
        }
        MettaValue::SExpr(items) => {
            // Sum specificity of all items
            items.iter().map(pattern_specificity).sum()
        }
        // Conjunctions: sum specificity of all goals
        MettaValue::Conjunction(goals) => goals.iter().map(pattern_specificity).sum(),
        // Errors: use specificity of details
        MettaValue::Error(_, details) => pattern_specificity(details),
        // Types: use specificity of inner type
        MettaValue::Type(t) => pattern_specificity(t),
    }
}

/// Find ALL rules in the environment that match the given expression
/// Returns Vec<(rhs, bindings)> with all matching rules
///
/// This function supports MeTTa's non-deterministic semantics where multiple rules
/// can match the same expression and all results should be returned.
fn try_match_all_rules(expr: &MettaValue, env: &Environment) -> Vec<(MettaValue, Bindings)> {
    // Try query_multi optimization first
    let query_multi_results = try_match_all_rules_query_multi(expr, env);
    if !query_multi_results.is_empty() {
        return query_multi_results;
    }

    // Fall back to iteration-based approach
    try_match_all_rules_iterative(expr, env)
}

/// Try pattern matching using MORK's query_multi to find ALL matching rules (O(k) where k = matching rules)
fn try_match_all_rules_query_multi(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(MettaValue, Bindings)> {
    trace!(target: "mettatron::backend::eval::try_match_all_rules_query_multi", ?expr);

    // Create a pattern that queries for rules: (= <expr-pattern> $rhs)
    // This will find all rules where the LHS matches our expression

    // Convert expression to MORK format for querying (using cache)
    let expr_bytes = match env.metta_to_mork_bytes_cached(expr) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(), // Fallback to iterative if conversion fails
    };
    trace!(
        target: "mettatron::backend::eval::try_match_all_rules_query_multi",
        expr_bytes_len = ?expr_bytes.len(), "Convert expression to MORK format"
    );

    let space = env.create_space();
    let ctx = ConversionContext::new();

    // Create a query pattern: (= <expr> $rhs)
    let pattern_str = format!("(= {} $rhs)", String::from_utf8_lossy(&expr_bytes));
    let pattern_bytes = pattern_str.as_bytes();

    // Parse the pattern using MORK's parser
    let mut parse_buffer = vec![0u8; 4096];
    let mut pdp = mork::space::ParDataParser::new(&space.sm);
    use mork_frontend::bytestring_parser::Parser;
    let mut ez = mork_expr::ExprZipper::new(Expr {
        ptr: parse_buffer.as_mut_ptr(),
    });
    let mut context = mork_frontend::bytestring_parser::Context::new(pattern_bytes);

    if pdp.sexpr(&mut context, &mut ez).is_err() {
        return Vec::new(); // Fallback if parsing fails
    }

    let pattern_expr = Expr {
        ptr: parse_buffer.as_ptr().cast_mut(),
    };

    // Collect ALL matches using query_multi
    // Note: All matches from query_multi will have the same LHS pattern (since we're querying for it)
    // Therefore, they all have the same LHS specificity and we should return all of them
    let mut matches: Vec<(MettaValue, Bindings)> = Vec::new();

    mork::space::Space::query_multi(&space.btm, pattern_expr, |result, _matched_expr| {
        if let Err(bindings) = result {
            // Convert MORK bindings to mettatron format
            if let Ok(mettatron_bindings) = mork_bindings_to_metta(&bindings, &ctx, &space) {
                // Extract the RHS from bindings
                if let Some((_, rhs)) = mettatron_bindings
                    .iter()
                    .find(|(name, _)| name.as_str() == "$rhs")
                {
                    matches.push((rhs.clone(), mettatron_bindings));
                }
            }
        }
        true // Continue searching for ALL matches
    });
    trace!(
        target: "mettatron::backend::eval::try_match_all_rules_query_multi",
        matches_ctr = ?matches.len(), "Collected matches"
    );

    matches
    // space will be dropped automatically here
}

/// Optimized: Try pattern matching using indexed lookup to find ALL matching rules
/// Uses O(1) index lookup instead of O(n) iteration
/// Complexity: O(k) where k = rules with matching head symbol (typically k << n)
fn try_match_all_rules_iterative(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(MettaValue, Bindings)> {
    trace!(target: "mettatron::backend::eval::try_match_all_rules_iterative", ?expr);

    // Extract head symbol and arity for indexed lookup
    let matching_rules = if let Some(head) = get_head_symbol(expr) {
        let arity = expr.get_arity();
        // O(1) indexed lookup instead of O(n) iteration
        env.get_matching_rules(head, arity)
    } else {
        // For expressions without head symbol, check wildcard rules only
        // This is still O(k_wildcards) instead of O(n_total)
        env.get_matching_rules("", 0) // Empty head will return only wildcards
    };

    // Sort rules by specificity (more specific first)
    let mut sorted_rules = matching_rules;
    sorted_rules.sort_by_key(|rule| pattern_specificity(&rule.lhs));

    // Collect ALL matching rules, tracking LHS specificity
    let mut matches: Vec<(MettaValue, Bindings, usize, Rule)> = Vec::new();
    for rule in sorted_rules {
        if let Some(bindings) = pattern_match(&rule.lhs, expr) {
            let lhs_specificity = pattern_specificity(&rule.lhs);
            matches.push((rule.rhs.clone(), bindings, lhs_specificity, rule));
        }
    }

    // Find the best (lowest) specificity
    if let Some(best_spec) = matches.iter().map(|(_, _, spec, _)| *spec).min() {
        // Filter to only matches with the best specificity
        let best_matches: Vec<_> = matches
            .into_iter()
            .filter(|(_, _, spec, _)| *spec == best_spec)
            .collect();

        // Duplicate results based on rule count
        let mut final_matches = Vec::new();
        for (rhs, bindings, _, rule) in best_matches {
            let count = env.get_rule_count(&rule);
            for _ in 0..count {
                final_matches.push((rhs.clone(), bindings.clone()));
            }
        }
        final_matches
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::Rule;

    #[test]
    fn test_eval_atom() {
        let env = Environment::new();
        let value = MettaValue::Atom("foo".to_string());
        let (results, _) = eval(value.clone(), env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], value);
    }

    #[test]
    fn test_eval_builtin_add() {
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_eval_builtin_comparison() {
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_eval_logical_and() {
        let env = Environment::new();

        // True and True = True
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));

        // True and False = False
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));

        // False and True = False
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Bool(false),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));

        // False and False = False
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Bool(false),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_eval_logical_or() {
        let env = Environment::new();

        // True or True = True
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));

        // True or False = True
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));

        // False or True = True
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(false),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));

        // False or False = False
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(false),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_eval_logical_not() {
        let env = Environment::new();

        // not True = False
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));

        // not False = True
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_eval_logical_type_error() {
        let env = Environment::new();

        // and with non-boolean should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Long(1),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));

        // or with non-boolean should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(true),
            MettaValue::String("hello".to_string()),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));

        // not with non-boolean should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_eval_logical_arity_error() {
        let env = Environment::new();

        // and with wrong arity
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("and".to_string()),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));

        // not with wrong arity
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_pattern_match_simple() {
        let pattern = MettaValue::Atom("$x".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == "$x")
                .map(|(_, val)| val),
            Some(&MettaValue::Long(42))
        );
    }

    #[test]
    fn test_pattern_match_sexpr() {
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]);
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        let bindings = bindings.unwrap();
        assert_eq!(
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == "$x")
                .map(|(_, val)| val),
            Some(&MettaValue::Long(1))
        );
    }

    #[test]
    fn test_eval_with_rule() {
        let mut env = Environment::new();

        // Add rule: (= (double $x) (mul $x 2))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        };
        env.add_rule(rule);

        // Evaluate (double 5)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    // === Integration Test ===

    #[test]
    fn test_eval_with_quote() {
        let env = Environment::new();

        // (eval (quote (+ 1 2)))
        // Quote prevents evaluation, eval forces it
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("eval".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_mvp_complete() {
        let mut env = Environment::new();

        // Add a rule: (= (safe-div $x $y) (if (== $y 0) (error "division by zero" $y) (div $x $y)))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("safe-div".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("==".to_string()),
                    MettaValue::Atom("$y".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("division by zero".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("/".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
        };
        env.add_rule(rule);

        // Test successful division: (safe-div 10 2) -> 5
        let value1 = MettaValue::SExpr(vec![
            MettaValue::Atom("safe-div".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(2),
        ]);
        let (results1, env1) = eval(value1, env.clone());
        assert_eq!(results1[0], MettaValue::Long(5));

        // Test division by zero: (safe-div 10 0) -> Error
        let value2 = MettaValue::SExpr(vec![
            MettaValue::Atom("safe-div".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(0),
        ]);
        let (results2, _) = eval(value2, env1);
        match &results2[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "division by zero");
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    // === Tests adapted from hyperon-experimental ===
    // Source: https://github.com/trueagi-io/hyperon-experimental

    #[test]
    fn test_nested_arithmetic() {
        // From c1_grounded_basic.metta: (+ 2 (* 3 5))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(5),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(17)); // 2 + (3 * 5) = 17
    }

    #[test]
    fn test_comparison_with_arithmetic() {
        // From c1_grounded_basic.metta: (< 4 (+ 2 (* 3 5)))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(4),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(5),
                ]),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Bool(true)); // 4 < 17
    }

    #[test]
    fn test_equality_literals() {
        // From c1_grounded_basic.metta: (== 4 (+ 2 2))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("==".to_string()),
            MettaValue::Long(4),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(2),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Bool(true)); // 4 == 4
    }

    #[test]
    fn test_equality_sexpr() {
        // From c1_grounded_basic.metta: structural equality tests
        let env = Environment::new();

        // (== (A B) (A B)) should be supported via pattern matching
        // For now we test that equal atoms are equal
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("==".to_string()),
            MettaValue::Long(42),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_factorial_recursive() {
        // From c1_grounded_basic.metta: factorial example with if guard
        // (= (fact $n) (if (> $n 0) (* $n (fact (- $n 1))) 1))
        let mut env = Environment::new();

        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                // Condition: (> $n 0)
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                // Then branch: (* $n (fact (- $n 1)))
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("fact".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("-".to_string()),
                            MettaValue::Atom("$n".to_string()),
                            MettaValue::Long(1),
                        ]),
                    ]),
                ]),
                // Else branch: 1
                MettaValue::Long(1),
            ]),
        };
        env.add_rule(rule);

        // Test (fact 3) = 6
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_factorial_with_compile() {
        // Test factorial using compile() to ensure the compiled version works
        // This complements test_factorial_recursive which uses manual construction
        use crate::backend::compile::compile;

        let input = r#"
            (= (fact $n) (if (> $n 0) (* $n (fact (- $n 1))) 1))
            !(fact 0)
            !(fact 1)
            !(fact 2)
            !(fact 3)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut results = Vec::new();

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            // Collect non-empty results (skip rule definitions)
            if !expr_results.is_empty() {
                results.extend(expr_results);
            }
        }

        // Should have 4 results: fact(0)=1, fact(1)=1, fact(2)=2, fact(3)=6
        assert_eq!(results.len(), 4);
        assert_eq!(results[0], MettaValue::Long(1)); // fact(0)
        assert_eq!(results[1], MettaValue::Long(1)); // fact(1)
        assert_eq!(results[2], MettaValue::Long(2)); // fact(2)
        assert_eq!(results[3], MettaValue::Long(6)); // fact(3)
    }

    #[test]
    fn test_incremental_nested_arithmetic() {
        // From test_metta.py: !(+ 1 (+ 2 (+ 3 4)))
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(4),
                ]),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    #[test]
    fn test_function_definition_and_call() {
        // From test_run_metta.py: (= (f) (+ 2 3)) !(f)
        let mut env = Environment::new();

        // Define rule: (= (f) (+ 2 3))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        };
        env.add_rule(rule);

        // Evaluate (f)
        let value = MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(5));
    }

    #[test]
    fn test_multiple_pattern_variables() {
        // Test pattern matching with multiple variables
        let mut env = Environment::new();

        // (= (add3 $a $b $c) (+ $a (+ $b $c)))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("add3".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
                MettaValue::Atom("$c".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$b".to_string()),
                    MettaValue::Atom("$c".to_string()),
                ]),
            ]),
        };
        env.add_rule(rule);

        // (add3 10 20 30) = 60
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add3".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(60));
    }

    #[test]
    fn test_nested_pattern_matching() {
        // Test nested S-expression pattern matching
        let mut env = Environment::new();

        // (= (eval-pair (pair $x $y)) (+ $x $y))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("eval-pair".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("pair".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        };
        env.add_rule(rule);

        // (eval-pair (pair 5 7)) = 12
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("eval-pair".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(7),
            ]),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Long(12));
    }

    #[test]
    fn test_wildcard_pattern() {
        // Test wildcard matching
        let pattern = MettaValue::Atom("_".to_string());
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());

        // Wildcard should not bind the value
        let bindings = bindings.unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_variable_consistency_in_pattern() {
        // Test that the same variable in a pattern must match the same value
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        // Should match when both are the same
        let value1 = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
        ]);
        assert!(pattern_match(&pattern, &value1).is_some());

        // Should not match when they differ
        let value2 = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(7),
        ]);
        assert!(pattern_match(&pattern, &value2).is_none());
    }

    #[test]
    fn test_conditional_with_pattern_matching() {
        // Test combining if with pattern matching
        let mut env = Environment::new();

        // (= (abs $x) (if (< $x 0) (- 0 $x) $x))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("abs".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("<".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("-".to_string()),
                    MettaValue::Long(0),
                    MettaValue::Atom("$x".to_string()),
                ]),
                MettaValue::Atom("$x".to_string()),
            ]),
        };
        env.add_rule(rule);

        // abs(-5) = 5
        let value1 = MettaValue::SExpr(vec![
            MettaValue::Atom("abs".to_string()),
            MettaValue::Long(-5),
        ]);
        let (results, env1) = eval(value1, env.clone());
        assert_eq!(results[0], MettaValue::Long(5));

        // abs(7) = 7
        let value2 = MettaValue::SExpr(vec![
            MettaValue::Atom("abs".to_string()),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value2, env1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_string_values() {
        // Test string value handling
        let env = Environment::new();
        let value = MettaValue::String("test".to_string());
        let (results, _) = eval(value.clone(), env);
        assert_eq!(results[0], value);
    }

    #[test]
    fn test_boolean_values() {
        let env = Environment::new();

        let value_true = MettaValue::Bool(true);
        let (results, _) = eval(value_true.clone(), env.clone());
        assert_eq!(results[0], value_true);

        let value_false = MettaValue::Bool(false);
        let (results, _) = eval(value_false.clone(), env);
        assert_eq!(results[0], value_false);
    }

    #[test]
    fn test_nil_value() {
        let env = Environment::new();
        let value = MettaValue::Nil;
        let (results, _) = eval(value, env);
        assert_eq!(results[0], MettaValue::Nil);
    }

    // === Fact Database Tests ===

    #[test]
    fn test_symbol_added_to_fact_database() {
        // Bare atoms should NOT be added to the fact database
        // Only rules, type assertions, and unmatched s-expressions are stored
        let env = Environment::new();

        // Evaluate the symbol "Hello"
        let symbol = MettaValue::Atom("Hello".to_string());
        let (results, new_env) = eval(symbol.clone(), env);

        // Symbol should be returned unchanged
        assert_eq!(results[0], symbol);

        // Bare atoms should NOT be added to fact database (this prevents pollution)
        assert!(!new_env.has_fact("Hello"));
    }

    #[test]
    fn test_variables_not_added_to_fact_database() {
        let env = Environment::new();

        // Test $variable
        let var1 = MettaValue::Atom("$x".to_string());
        let (_, new_env) = eval(var1, env.clone());
        assert!(!new_env.has_fact("$x"));

        // Test &variable
        let var2 = MettaValue::Atom("&y".to_string());
        let (_, new_env) = eval(var2, env.clone());
        assert!(!new_env.has_fact("&y"));

        // Test 'variable
        let var3 = MettaValue::Atom("'z".to_string());
        let (_, new_env) = eval(var3, env.clone());
        assert!(!new_env.has_fact("'z"));

        // Test wildcard
        let wildcard = MettaValue::Atom("_".to_string());
        let (_, new_env) = eval(wildcard, env);
        assert!(!new_env.has_fact("_"));
    }

    #[test]
    fn test_multiple_symbols_in_fact_database() {
        // Bare atoms should NOT be added to fact database
        // This test verifies that evaluating multiple atoms doesn't pollute the environment
        let env = Environment::new();

        // Evaluate multiple symbols
        let symbol1 = MettaValue::Atom("Foo".to_string());
        let (_, env1) = eval(symbol1, env);

        let symbol2 = MettaValue::Atom("Bar".to_string());
        let (_, env2) = eval(symbol2, env1);

        let symbol3 = MettaValue::Atom("Baz".to_string());
        let (_, env3) = eval(symbol3, env2);

        // Bare atoms should NOT be in the fact database
        assert!(!env3.has_fact("Foo"));
        assert!(!env3.has_fact("Bar"));
        assert!(!env3.has_fact("Baz"));
    }

    #[test]
    fn test_sexpr_added_to_fact_database() {
        // Verify official MeTTa ADD mode semantics:
        // When an s-expression like (Hello World) is evaluated, it is automatically added to the space
        // This matches: `(leaf1 leaf2)` in REPL -> auto-added, queryable via `!(match &self ...)`
        let env = Environment::new();

        // Evaluate the s-expression (Hello World)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("Hello".to_string()),
            MettaValue::Atom("World".to_string()),
        ]);
        let expected_result = MettaValue::SExpr(vec![
            MettaValue::Atom("Hello".to_string()),
            MettaValue::Atom("World".to_string()),
        ]);

        let (results, new_env) = eval(sexpr.clone(), env);

        // S-expression should be returned (with evaluated elements)
        assert_eq!(results[0], expected_result);

        // S-expression should be added to fact database (ADD mode behavior)
        assert!(new_env.has_sexpr_fact(&expected_result));

        // Individual atoms are NOT stored separately
        // Only the full s-expression is stored in MORK format
        assert!(!new_env.has_fact("Hello"));
        assert!(!new_env.has_fact("World"));
    }

    #[test]
    fn test_nested_sexpr_in_fact_database() {
        // Official MeTTa semantics: only the top-level expression is stored
        // Nested sub-expressions are NOT extracted and stored separately
        let env = Environment::new();

        // Evaluate a nested s-expression
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);

        let (_, new_env) = eval(sexpr, env);

        // CORRECT: Outer s-expression should be in fact database
        let expected_outer = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);
        assert!(new_env.has_sexpr_fact(&expected_outer));

        // CORRECT: Inner s-expression should NOT be in fact database (not recursively stored)
        // Official MeTTa only stores the top-level expression passed to add-atom
        let expected_inner = MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]);
        assert!(!new_env.has_sexpr_fact(&expected_inner));

        // Individual atoms are NOT stored separately
        assert!(!new_env.has_fact("Outer"));
        assert!(!new_env.has_fact("Inner"));
        assert!(!new_env.has_fact("Nested"));
    }

    #[test]
    fn test_pattern_matching_extracts_nested_sexpr() {
        // Demonstrates that while nested s-expressions are NOT stored separately,
        // they can still be accessed via pattern matching with variables.
        // This is how official MeTTa handles nested data extraction.
        let mut env = Environment::new();

        // Store a nested s-expression: (Outer (Inner Nested))
        let nested_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("Outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Inner".to_string()),
                MettaValue::Atom("Nested".to_string()),
            ]),
        ]);

        // Evaluate to add to space (ADD mode behavior)
        let (_, env1) = eval(nested_expr.clone(), env);
        env = env1;

        // Verify only the outer expression is stored
        assert!(env.has_sexpr_fact(&nested_expr));
        let inner_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("Inner".to_string()),
            MettaValue::Atom("Nested".to_string()),
        ]);
        assert!(!env.has_sexpr_fact(&inner_expr)); // NOT stored separately

        // Use pattern matching to extract the nested part: (match & self (Outer $x) $x)
        let match_query = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("Outer".to_string()),
                MettaValue::Atom("$x".to_string()), // Variable to capture nested part
            ]),
            MettaValue::Atom("$x".to_string()), // Template: return the captured value
        ]);

        let (results, _) = eval(match_query, env);

        // Should return the nested s-expression even though it wasn't stored separately
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], inner_expr); // Pattern matching extracts (Inner Nested)
    }

    #[test]
    fn test_grounded_operations_not_added_to_sexpr_facts() {
        let env = Environment::new();

        // Evaluate an arithmetic operation (add 1 2)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let (results, new_env) = eval(sexpr.clone(), env);

        // Result should be 3
        assert_eq!(results[0], MettaValue::Long(3));

        // The s-expression should NOT be in the fact database
        // because it was reduced to a value by a grounded operation
        assert!(!new_env.has_sexpr_fact(&sexpr));
    }

    #[test]
    fn test_rule_definition_added_to_fact_database() {
        let env = Environment::new();

        // Define a rule: (= (double $x) (* $x 2))
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (result, new_env) = eval(rule_def.clone(), env);

        // Rule definition should return empty list
        assert!(result.is_empty());

        // Rule definition should also be in the fact database
        assert!(new_env.has_sexpr_fact(&rule_def));
    }

    // === Type Error Tests ===

    #[test]
    fn test_arithmetic_type_error_string() {
        let env = Environment::new();

        // Test: !(+ 1 "a") should produce TypeError with friendly message
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::String("a".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                // Error message should contain the friendly type name
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert!(
                    msg.contains("expected Number (integer)"),
                    "Expected 'expected Number (integer)' in: {}",
                    msg
                );
                // Error details should be TypeError
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_arithmetic_type_error_first_arg() {
        let env = Environment::new();

        // Test: !(+ "a" 1) - first argument wrong type
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::Long(1),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert!(
                    msg.contains("expected Number (integer)"),
                    "Expected type info in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_arithmetic_type_error_bool() {
        let env = Environment::new();

        // Test: !(* true false) - booleans not valid for arithmetic
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
                assert!(
                    msg.contains("expected Number (integer)"),
                    "Expected type info in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_comparison_type_error() {
        let env = Environment::new();

        // Test: !(< "a" "b") - strings not valid for comparison
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::String("b".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert!(
                    msg.contains("Cannot compare"),
                    "Expected 'Cannot compare' in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_arithmetic_wrong_arity() {
        let env = Environment::new();

        // Test: !(+ 1) - wrong number of arguments
        let value = MettaValue::SExpr(vec![MettaValue::Atom("+".to_string()), MettaValue::Long(1)]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("2 arguments"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    // ========================================================================
    // Conjunction Pattern Tests
    // ========================================================================

    #[test]
    fn test_empty_conjunction() {
        let env = Environment::new();

        // Empty conjunction: (,) → Nil
        let value = MettaValue::Conjunction(vec![]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_unary_conjunction() {
        let env = Environment::new();

        // Unary conjunction: (, expr) → evaluates expr directly
        let value = MettaValue::Conjunction(vec![MettaValue::Long(42)]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_unary_conjunction_with_expression() {
        let env = Environment::new();

        // Unary conjunction with expression: (, (+ 2 3)) → 5
        let value = MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ])]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));
    }

    #[test]
    fn test_binary_conjunction() {
        let env = Environment::new();

        // Binary conjunction: (, (+ 1 1) (+ 2 2)) → 2, 4
        let value = MettaValue::Conjunction(vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(1),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        // Binary conjunction returns results from the last goal
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(4));
    }

    #[test]
    fn test_nary_conjunction() {
        let env = Environment::new();

        // N-ary conjunction: (, (+ 1 1) (+ 2 2) (+ 3 3)) → 2, 4, 6 (returns last)
        let value = MettaValue::Conjunction(vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(1),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(3),
            ]),
        ]);

        let (results, _) = eval(value, env);
        // N-ary conjunction returns results from the last goal
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_conjunction_pattern_match() {
        // Test that conjunctions can be pattern matched
        let pattern = MettaValue::Conjunction(vec![
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);

        let value = MettaValue::Conjunction(vec![MettaValue::Long(1), MettaValue::Long(2)]);

        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());

        let bindings = bindings.unwrap();
        assert_eq!(
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == "$x")
                .map(|(_, val)| val),
            Some(&MettaValue::Long(1))
        );
        assert_eq!(
            bindings
                .iter()
                .find(|(name, _)| name.as_str() == "$y")
                .map(|(_, val)| val),
            Some(&MettaValue::Long(2))
        );
    }

    #[test]
    fn test_conjunction_with_error_propagation() {
        let env = Environment::new();

        // Conjunction with error should propagate the error
        let value = MettaValue::Conjunction(vec![
            MettaValue::Long(42),
            MettaValue::Error("test error".to_string(), Arc::new(MettaValue::Nil)),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_nested_conjunction() {
        let env = Environment::new();

        // Nested conjunction: (, (+ 1 2) (, (+ 3 4)))
        let value = MettaValue::Conjunction(vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ])]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    // ========================================================================
    // Fuzzy Matching / Typo Detection Tests
    // ========================================================================

    #[test]
    fn test_misspelled_special_form() {
        let env = Environment::new();

        // Try to use "mach" instead of "match"
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("mach".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::Atom("pattern".to_string()),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);

        // After fix: undefined symbols are treated as data (ADD mode)
        // A warning is printed to stderr, but the expression is returned as data
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::Atom("mach".to_string()));
                assert_eq!(items[1], MettaValue::Atom("&self".to_string()));
                assert_eq!(items[2], MettaValue::Atom("pattern".to_string()));
            }
            other => panic!("Expected SExpr (data), got {:?}", other),
        }
    }

    #[test]
    fn test_undefined_symbol_with_rule_suggestion() {
        let mut env = Environment::new();

        // Add a rule for "fibonacci"
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fibonacci".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
        };
        env.add_rule(rule);

        // Try to call "fibonaci" (misspelled - missing 'n')
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("fibonaci".to_string()),
            MettaValue::Long(5),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Did you mean"),
                    "Expected suggestion in: {}",
                    msg
                );
                assert!(
                    msg.contains("fibonacci"),
                    "Expected 'fibonacci' in: {}",
                    msg
                );
            }
            other => panic!("Expected Error with suggestion, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_symbol_returns_as_is() {
        let env = Environment::new();

        // Completely unknown symbols (not similar to any known term)
        // should be returned as-is per ADD mode semantics
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("xyzzy".to_string()),
            MettaValue::Long(1),
        ]);

        let (results, _) = eval(expr.clone(), env);
        assert_eq!(results.len(), 1);

        // Should return the expression as-is (ADD mode), not an error
        assert_eq!(results[0], expr, "Expected expression to be returned as-is");
    }

    #[test]
    fn test_short_symbol_not_flagged_as_typo() {
        let env = Environment::new();

        // Short symbols like "a" should NOT be flagged as typos even if
        // they're close to special forms like "=" (edit distance 1)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let (results, _) = eval(expr.clone(), env);
        assert_eq!(results.len(), 1);

        // Should return the expression as-is (ADD mode), not an error
        assert_eq!(
            results[0], expr,
            "Short symbols should not be flagged as typos"
        );
    }
}
