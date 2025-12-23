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
mod builtin;
mod cartesian;
mod control_flow;
mod errors;
mod evaluation;
mod expression;
pub mod fixed_point;
mod helpers;
mod io;
mod list_ops;
mod modules;
mod mork_forms;
mod pattern;
pub mod priority;
mod quoting;
mod rules;
mod space;
mod strings;
mod types;
mod utilities;

use std::collections::VecDeque;
use std::sync::Arc;
use tracing::{debug, trace, warn};

use smallvec::SmallVec;

use crate::backend::environment::Environment;
use crate::backend::grounded::{ExecError, GroundedState, GroundedWork};
use crate::backend::models::{Bindings, EvalResult, MettaValue, Rule};

// Re-export from cartesian module
use cartesian::{
    cartesian_product, cartesian_product_lazy, CartesianProductIter, CartesianProductResult,
    Combination, MAX_CARTESIAN_RESULTS,
};

// Re-export from pattern module
pub use pattern::pattern_match;
use pattern::pattern_match_impl;

// Re-export from helpers module
pub use helpers::apply_bindings;
pub(crate) use helpers::friendly_value_repr;
use helpers::{
    friendly_type_name, get_head_symbol, is_grounded_op, pattern_specificity,
    preprocess_space_refs, resolve_tokens_shallow, suggest_special_form_with_context,
    try_eval_builtin, values_equal, SPECIAL_FORMS,
};

// Re-export from rules module
use rules::{try_match_all_rules, try_match_all_rules_iterative, try_match_all_rules_query_multi};

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
        /// If true, this is a tail call - don't increment depth
        /// Tail calls include: rule RHS, if branches, let* final body, match templates
        is_tail_call: bool,
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
        /// RHS is Arc-wrapped for O(1) cloning
        remaining_matches: VecDeque<(Arc<MettaValue>, Bindings)>,
        /// Results accumulated so far
        results: Vec<MettaValue>,
        /// Environment
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing TCO grounded operation (e.g., +, -, and, or)
    /// This continuation tracks state across multiple argument evaluations
    ProcessGroundedOp {
        /// State of the grounded operation (tracks which args have been evaluated)
        state: GroundedState,
        /// Environment for evaluating arguments
        env: Environment,
        /// Parent continuation to resume after operation completes
        parent_cont: usize,
        /// Evaluation depth
        depth: usize,
    },
    /// Processing lazy Cartesian product combinations one at a time
    /// This continuation enables memory-efficient nondeterministic evaluation
    ProcessCombinations {
        /// Iterator over remaining combinations (lazy evaluation)
        combinations: CartesianProductIter,
        /// Results accumulated so far from processing combinations
        results: Vec<MettaValue>,
        /// Pending rule matches for the current combination (VecDeque for O(1) pop_front)
        /// RHS is Arc-wrapped for O(1) cloning
        pending_rule_matches: VecDeque<(Arc<MettaValue>, Bindings)>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after all combinations processed
        parent_cont: usize,
    },
    /// Processing let binding - tracks state across value and body evaluations
    /// This enables let body evaluation to participate in the trampoline (TCO)
    ProcessLet {
        /// Value results to process (None if awaiting value evaluation)
        pending_values: Option<VecDeque<MettaValue>>,
        /// Pattern to match against values
        pattern: MettaValue,
        /// Body template to instantiate with bindings
        body: MettaValue,
        /// Collected body evaluation results
        results: Vec<MettaValue>,
        /// Environment for body evaluation
        env: Environment,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
        /// Parent continuation to resume after all values processed
        parent_cont: usize,
    },
}

/// Maximum evaluation depth to prevent stack overflow
/// This limits how deep the evaluation can recurse through nested expressions
/// Set to 1000 to allow legitimate deep nesting while still catching runaway recursion
const MAX_EVAL_DEPTH: usize = 1000;

/// Evaluate a MettaValue in the given environment
/// Returns (results, new_environment)
/// This is the public entry point that uses iterative evaluation with an explicit work stack
/// to prevent stack overflow for large expressions.
///
/// When the `bytecode` feature is enabled, supported expressions are compiled to bytecode
/// and executed by the bytecode VM for improved performance. Complex expressions that
/// require environment access (rules, spaces, etc.) fall back to the tree-walking evaluator.
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    debug!(metta_val = ?value);

    // Try JIT-enabled hybrid evaluation when the jit feature is enabled
    #[cfg(feature = "jit")]
    {
        use crate::backend::bytecode::{
            can_compile_cached, can_compile_with_env, eval_bytecode_hybrid,
            eval_bytecode_with_env, BYTECODE_ENABLED,
        };

        // Only try bytecode/JIT for expressions that don't need environment
        // and when bytecode is enabled at runtime
        if BYTECODE_ENABLED && can_compile_cached(&value) {
            if let Ok(results) = eval_bytecode_hybrid(&value) {
                // Hybrid (JIT/bytecode) evaluation succeeded - return results with unchanged env
                return (results, env);
            }
            // Hybrid evaluation failed (e.g., unsupported operation encountered)
            // Fall through to environment-aware check or tree-walker
        }

        // Try environment-aware bytecode for expressions that need rule dispatch
        // This enables bytecode for workloads like mmverify that use rules
        if BYTECODE_ENABLED && can_compile_with_env(&value) {
            if let Ok((results, new_env)) = eval_bytecode_with_env(&value, env.clone()) {
                return (results, new_env);
            }
            // Environment-aware bytecode failed - fall through to tree-walker
        }
    }

    // Bytecode-only path (without JIT) when jit feature is not enabled
    #[cfg(all(feature = "bytecode", not(feature = "jit")))]
    {
        use crate::backend::bytecode::{
            can_compile_cached, can_compile_with_env, eval_bytecode, eval_bytecode_with_env,
            BYTECODE_ENABLED,
        };

        // Only try bytecode for expressions that don't need environment
        // and when bytecode is enabled at runtime
        if BYTECODE_ENABLED && can_compile_cached(&value) {
            if let Ok(results) = eval_bytecode(&value) {
                // Bytecode evaluation succeeded - return results with unchanged env
                return (results, env);
            }
            // Bytecode failed (e.g., unsupported operation encountered)
            // Fall through to environment-aware check or tree-walker
        }

        // Try environment-aware bytecode for expressions that need rule dispatch
        // This enables bytecode for workloads like mmverify that use rules
        if BYTECODE_ENABLED && can_compile_with_env(&value) {
            if let Ok((results, new_env)) = eval_bytecode_with_env(&value, env.clone()) {
                return (results, new_env);
            }
            // Environment-aware bytecode failed - fall through to tree-walker
        }
    }

    eval_trampoline(value, env)
}

/// Iterative evaluation using a trampoline pattern with explicit work stack.
/// This prevents stack overflow by using heap-allocated work items instead of
/// recursive function calls.
fn eval_trampoline(value: MettaValue, env: Environment) -> EvalResult {
    // Debug tracing controlled by environment variable
    let debug_eval = std::env::var("METTA_DEBUG_EVAL").is_ok();
    let mut eval_count: u64 = 0;

    // Initialize work stack with the initial evaluation
    let mut work_stack: Vec<WorkItem> = vec![WorkItem::Eval {
        value,
        env: env.clone(),
        depth: 0,
        cont_id: 0, // Done continuation
        is_tail_call: false, // Initial evaluation is not a tail call
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
                is_tail_call,
            } => {
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?value, depth, cont_id, "eval work item");

                // Debug trace
                if debug_eval {
                    eval_count += 1;
                    if eval_count % 1000 == 0 || eval_count < 100 {
                        eprintln!(
                            "[EVAL#{}] depth={} work_stack={} conts={} value={}",
                            eval_count,
                            depth,
                            work_stack.len(),
                            continuations.len(),
                            friendly_value_repr(&value)
                        );
                    }
                }

                // Perform one step of evaluation
                // For tail calls, we don't increment depth - this enables TCO
                let step_result = eval_step(value, env.clone(), depth);
                let _ = is_tail_call; // Used to determine depth in push sites
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?step_result);

                match step_result {
                    // Direct result - resume continuation
                    EvalStep::Done(result) => {
                        work_stack.push(WorkItem::Resume { cont_id, result });
                    }

                    // Need to evaluate S-expression sub-items
                    EvalStep::EvalSExpr { items, env, depth } => {
                        if items.is_empty() {
                            // HE-compatible: empty SExpr () evaluates to itself, not Nil
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![MettaValue::SExpr(vec![])], env),
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
                            // NOT a tail call - more items to process after this
                            work_stack.push(WorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                cont_id: collect_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start a TCO grounded operation (e.g., +, -, and, or)
                    EvalStep::StartGroundedOp { state, env, depth } => {
                        // Look up the TCO operation
                        if let Some(grounded_op) = env.get_grounded_operation_tco(&state.op_name) {
                            let mut state = state;
                            match grounded_op.execute_step(&mut state) {
                                GroundedWork::Done(results) => {
                                    // Operation completed immediately (rare: all args already evaluated)
                                    let values: Vec<MettaValue> =
                                        results.into_iter().map(|(v, _)| v).collect();
                                    work_stack.push(WorkItem::Resume {
                                        cont_id,
                                        result: (values, env),
                                    });
                                }
                                GroundedWork::EvalArg {
                                    arg_idx,
                                    state: new_state,
                                } => {
                                    // Need to evaluate an argument first
                                    let grounded_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessGroundedOp {
                                        state: new_state.clone(),
                                        env: env.clone(),
                                        parent_cont: cont_id,
                                        depth,
                                    });

                                    // Get the argument to evaluate
                                    let arg_to_eval = new_state.args[arg_idx].clone();

                                    // Push eval work item - TCO: don't increment depth
                                    work_stack.push(WorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth, // TCO: reuse depth for grounded arg eval
                                        cont_id: grounded_cont_id,
                                        is_tail_call: true,
                                    });
                                }
                                GroundedWork::Error(e) => {
                                    // Operation failed
                                    let error_value = match e {
                                        ExecError::Runtime(msg) => MettaValue::Error(
                                            msg,
                                            Arc::new(MettaValue::Atom("TypeError".to_string())),
                                        ),
                                        ExecError::Arithmetic(msg) => MettaValue::Error(
                                            msg,
                                            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
                                        ),
                                        ExecError::IncorrectArgument(msg) => MettaValue::Error(
                                            msg,
                                            Arc::new(MettaValue::Atom("ArityError".to_string())),
                                        ),
                                        ExecError::NoReduce => MettaValue::Error(
                                            "NoReduce".to_string(),
                                            Arc::new(MettaValue::Atom("EvalError".to_string())),
                                        ),
                                    };
                                    work_stack.push(WorkItem::Resume {
                                        cont_id,
                                        result: (vec![error_value], env),
                                    });
                                }
                            }
                        } else {
                            // TCO operation not found - shouldn't happen if we check first
                            let error_value = MettaValue::Error(
                                format!("TCO operation '{}' not found", state.op_name),
                                Arc::new(MettaValue::Atom("InternalError".to_string())),
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![error_value], env),
                            });
                        }
                    }

                    // Start let binding evaluation - evaluate value expression first
                    EvalStep::StartLetBinding {
                        pattern,
                        value_expr,
                        body,
                        env,
                        depth,
                    } => {
                        // Create ProcessLet continuation to handle value results
                        let let_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessLet {
                            pending_values: None, // Will be filled when value eval completes
                            pattern,
                            body,
                            results: Vec::new(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Push value expression evaluation
                        work_stack.push(WorkItem::Eval {
                            value: value_expr,
                            env,
                            depth: depth + 1, // Value is not tail position
                            cont_id: let_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate if branch - condition already evaluated, now evaluate selected branch
                    EvalStep::EvalIfBranch { branch, env, depth } => {
                        // Push branch evaluation - THIS IS TAIL CALL (TCO)
                        // The branch inherits the continuation from the if expression
                        work_stack.push(WorkItem::Eval {
                            value: branch,
                            env,
                            depth, // TCO: reuse depth for branch eval
                            cont_id,
                            is_tail_call: true,
                        });
                    }

                    // Evaluate rule matches with UNEVALUATED arguments (lazy evaluation)
                    // This is used when user-defined rules match before argument evaluation.
                    EvalStep::EvalRuleMatchesLazy { matches, env, depth } => {
                        if matches.is_empty() {
                            // No rule matches - shouldn't happen (this variant is only used when matches exist)
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![], env),
                            });
                        } else {
                            // Convert to VecDeque ONCE and pop front (O(n) + O(1) vs O(n²))
                            let mut matches_deque: VecDeque<_> = matches.into_iter().collect();
                            let (rhs, bindings) = matches_deque.pop_front().unwrap();

                            // Create continuation to process remaining rule matches
                            let match_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessRuleMatches {
                                remaining_matches: matches_deque,
                                results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate first rule RHS with bindings - THIS IS A TAIL CALL
                            // The bindings contain UNEVALUATED expressions from pattern match
                            // TCO: Don't increment depth for tail calls
                            let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth, // TCO: reuse depth
                                cont_id: match_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }
                }
            }

            WorkItem::Resume { cont_id, result } => {
                // Take ownership of continuation for processing
                let cont = std::mem::replace(&mut continuations[cont_id], Continuation::Done);
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?cont, result_values = ?result.0, "resume work item");

                match cont {
                    Continuation::Done => {
                        // Final result
                        final_result = Some(result);
                        trace!(target: "mettatron::backend::eval::eval_trampoline", ?final_result);
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
                            trace!(target: "mettatron::backend::eval::eval_trampoline", processed_sexpr=?processed);

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

                                        // Evaluate first rule RHS - THIS IS A TAIL CALL
                                        // TCO: Don't increment depth for tail calls
                                        let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_rhs,
                                            env,
                                            depth, // TCO: reuse depth
                                            cont_id: match_cont_id,
                                            is_tail_call: true,
                                        });
                                    }
                                }
                                ProcessedSExpr::EvalCombinations {
                                    combinations,
                                    env,
                                    depth,
                                } => {
                                    // Create continuation to process combinations lazily
                                    let combo_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCombinations {
                                        combinations,
                                        results: vec![],
                                        pending_rule_matches: VecDeque::new(),
                                        env: env.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    // Resume to process first combination
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: combo_cont_id,
                                        result: (vec![], env),
                                    });
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
                            // NOT a tail call - collecting results for S-expr
                            work_stack.push(WorkItem::Eval {
                                value: next,
                                env: original_env,
                                depth: depth + 1,
                                cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    Continuation::ProcessRuleMatches {
                        mut remaining_matches,
                        mut results,
                        env: _,
                        depth,
                        parent_cont,
                    } => {
                        // Add results from this rule evaluation
                        results.extend(result.0);
                        // IMPORTANT: Propagate environment changes (including state mutations)
                        // from rule evaluation to ensure side effects like change-state! are visible
                        let env = result.1;

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

                            // Evaluate next rule RHS - THIS IS A TAIL CALL
                            // TCO: Don't increment depth for tail calls
                            let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth, // TCO: reuse depth
                                cont_id,
                                is_tail_call: true,
                            });
                        }
                    }

                    Continuation::ProcessGroundedOp {
                        mut state,
                        env,
                        parent_cont,
                        depth,
                    } => {
                        // Add evaluation results to state
                        // The arg_idx is (step - 1) because step was incremented before EvalArg
                        let arg_idx = state.step - 1;
                        state.set_arg(arg_idx, result.0);

                        // Look up the TCO operation and continue
                        if let Some(grounded_op) =
                            env.get_grounded_operation_tco(&state.op_name)
                        {
                            match grounded_op.execute_step(&mut state) {
                                GroundedWork::Done(results) => {
                                    // Operation complete
                                    let values: Vec<MettaValue> =
                                        results.into_iter().map(|(v, _)| v).collect();
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (values, env),
                                    });
                                }
                                GroundedWork::EvalArg {
                                    arg_idx: next_arg_idx,
                                    state: new_state,
                                } => {
                                    // Need to evaluate another argument
                                    continuations[cont_id] = Continuation::ProcessGroundedOp {
                                        state: new_state.clone(),
                                        env: env.clone(),
                                        parent_cont,
                                        depth,
                                    };

                                    // Get the argument to evaluate
                                    let arg_to_eval = new_state.args[next_arg_idx].clone();

                                    // Push eval work item - TCO: don't increment depth
                                    work_stack.push(WorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth, // TCO: reuse depth for grounded arg eval
                                        cont_id,
                                        is_tail_call: true,
                                    });
                                }
                                GroundedWork::Error(e) => {
                                    // Operation failed
                                    let error_value = match e {
                                        ExecError::Runtime(msg) => MettaValue::Error(
                                            msg,
                                            Arc::new(MettaValue::Atom("TypeError".to_string())),
                                        ),
                                        ExecError::Arithmetic(msg) => MettaValue::Error(
                                            msg,
                                            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
                                        ),
                                        ExecError::IncorrectArgument(msg) => MettaValue::Error(
                                            msg,
                                            Arc::new(MettaValue::Atom("ArityError".to_string())),
                                        ),
                                        ExecError::NoReduce => MettaValue::Error(
                                            "NoReduce".to_string(),
                                            Arc::new(MettaValue::Atom("EvalError".to_string())),
                                        ),
                                    };
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![error_value], env),
                                    });
                                }
                            }
                        } else {
                            // Operation not found - shouldn't happen
                            let error_value = MettaValue::Error(
                                format!("TCO operation '{}' not found", state.op_name),
                                Arc::new(MettaValue::Atom("InternalError".to_string())),
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![error_value], env),
                            });
                        }
                    }

                    Continuation::ProcessCombinations {
                        mut combinations,
                        mut results,
                        mut pending_rule_matches,
                        mut env,
                        depth,
                        parent_cont,
                    } => {
                        // First, add any results from rule evaluation
                        results.extend(result.0);
                        // IMPORTANT: Propagate environment changes (including state mutations)
                        // from rule evaluation to ensure side effects like change-state! are visible
                        env = result.1;

                        // If we have pending rule matches, process the next one
                        if !pending_rule_matches.is_empty() {
                            let (rhs, bindings) = pending_rule_matches.pop_front().unwrap();

                            // Update continuation with remaining matches
                            continuations[cont_id] = Continuation::ProcessCombinations {
                                combinations,
                                results,
                                pending_rule_matches,
                                env: env.clone(),
                                depth,
                                parent_cont,
                            };

                            // Evaluate the rule RHS - THIS IS A TAIL CALL
                            let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth,
                                cont_id,
                                is_tail_call: true,
                            });
                        } else {
                            // No pending rule matches - get next combination
                            match combinations.next() {
                                None => {
                                    // All combinations processed, return results
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (results, env),
                                    });
                                }
                                Some(evaled_items) => {
                                    // Process this combination
                                    // Check if this is a grounded operation
                                    if let Some(MettaValue::Atom(op)) = evaled_items.first() {
                                        if let Some(builtin_result) =
                                            try_eval_builtin(op, &evaled_items[1..])
                                        {
                                            results.push(builtin_result);

                                            // Continue to next combination
                                            continuations[cont_id] =
                                                Continuation::ProcessCombinations {
                                                    combinations,
                                                    results,
                                                    pending_rule_matches: VecDeque::new(),
                                                    env: env.clone(),
                                                    depth,
                                                    parent_cont,
                                                };

                                            // Resume to process next combination
                                            work_stack.push(WorkItem::Resume {
                                                cont_id,
                                                result: (vec![], env),
                                            });
                                            continue;
                                        }
                                    }

                                    // MeTTa HE semantics: After argument evaluation, try rule matching AGAIN.
                                    // The evaluated arguments may now match rules that didn't match before.
                                    // Example: (intensity (color)) → (intensity red) → 100
                                    let evaled_vec: Vec<MettaValue> = evaled_items.into_vec();
                                    let sexpr = MettaValue::SExpr(evaled_vec.clone());
                                    let all_matches = try_match_all_rules(&sexpr, &env);

                                    if !all_matches.is_empty() {
                                        // Rules match! Queue them for evaluation
                                        pending_rule_matches = all_matches.into_iter().collect();

                                        // Take the first match and evaluate it
                                        let (rhs, bindings) = pending_rule_matches.pop_front().unwrap();

                                        // Update continuation with pending matches
                                        continuations[cont_id] = Continuation::ProcessCombinations {
                                            combinations,
                                            results,
                                            pending_rule_matches,
                                            env: env.clone(),
                                            depth,
                                            parent_cont,
                                        };

                                        // Evaluate the rule RHS
                                        let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_rhs,
                                            env,
                                            depth,
                                            cont_id,
                                            is_tail_call: true,
                                        });
                                        continue;
                                    }

                                    // No rules matched even with evaluated arguments - data constructor
                                    let result_value = handle_no_rule_match(evaled_vec, &sexpr, &mut env);
                                    results.push(result_value);

                                    // Continue to next combination
                                    continuations[cont_id] = Continuation::ProcessCombinations {
                                        combinations,
                                        results,
                                        pending_rule_matches: VecDeque::new(),
                                        env: env.clone(),
                                        depth,
                                        parent_cont,
                                    };

                                    // Resume to process next combination
                                    work_stack.push(WorkItem::Resume {
                                        cont_id,
                                        result: (vec![], env),
                                    });
                                }
                            }
                        }
                    }

                    // Handle let binding continuation
                    Continuation::ProcessLet {
                        pending_values,
                        pattern,
                        body,
                        mut results,
                        env: _let_env, // Unused - we use result_env from the resumed result
                        depth,
                        parent_cont,
                    } => {
                        let (result_values, result_env) = result;

                        match pending_values {
                            None => {
                                // First resumption: received value evaluation results
                                // Now process each value, trying to match pattern
                                let mut values = VecDeque::from(result_values);

                                // Try to find a matching value - use explicit loop for ownership clarity
                                loop {
                                    match values.pop_front() {
                                        Some(value) => {
                                            if let Some(bindings) = pattern_match(&pattern, &value)
                                            {
                                                // Pattern matches - evaluate body with bindings
                                                let instantiated_body =
                                                    apply_bindings(&body, &bindings).into_owned();

                                                // Restore continuation for collecting more results
                                                continuations[cont_id] = Continuation::ProcessLet {
                                                    pending_values: Some(values),
                                                    pattern,
                                                    body,
                                                    results,
                                                    env: result_env.clone(),
                                                    depth,
                                                    parent_cont,
                                                };

                                                // Push body evaluation - THIS IS TAIL CALL (TCO)
                                                work_stack.push(WorkItem::Eval {
                                                    value: instantiated_body,
                                                    env: result_env,
                                                    depth, // TCO: reuse depth for body eval
                                                    cont_id,
                                                    is_tail_call: true,
                                                });
                                                break; // Exit loop, work is pushed
                                            }
                                            // Pattern doesn't match - continue to next value
                                        }
                                        None => {
                                            // No pattern matched - return results to parent
                                            work_stack.push(WorkItem::Resume {
                                                cont_id: parent_cont,
                                                result: (results, result_env),
                                            });
                                            break; // Exit loop
                                        }
                                    }
                                }
                            }

                            Some(mut remaining_values) => {
                                // Subsequent resumption: received body evaluation results
                                // Add body results to collected results
                                results.extend(result_values);

                                // Try next value - use explicit loop for ownership clarity
                                loop {
                                    match remaining_values.pop_front() {
                                        Some(value) => {
                                            if let Some(bindings) = pattern_match(&pattern, &value)
                                            {
                                                // Pattern matches - evaluate body with bindings
                                                let instantiated_body =
                                                    apply_bindings(&body, &bindings).into_owned();

                                                // Restore continuation for collecting more results
                                                continuations[cont_id] = Continuation::ProcessLet {
                                                    pending_values: Some(remaining_values),
                                                    pattern,
                                                    body,
                                                    results,
                                                    env: result_env.clone(),
                                                    depth,
                                                    parent_cont,
                                                };

                                                // Push body evaluation - THIS IS TAIL CALL (TCO)
                                                work_stack.push(WorkItem::Eval {
                                                    value: instantiated_body,
                                                    env: result_env,
                                                    depth, // TCO: reuse depth for body eval
                                                    cont_id,
                                                    is_tail_call: true,
                                                });
                                                break; // Exit loop, work is pushed
                                            }
                                            // Pattern doesn't match - continue to next value
                                        }
                                        None => {
                                            // All values processed - return results to parent
                                            work_stack.push(WorkItem::Resume {
                                                cont_id: parent_cont,
                                                result: (results, result_env),
                                            });
                                            break; // Exit loop
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    final_result.unwrap_or_else(|| (vec![], env))
}

/// Result of a single evaluation step
#[derive(Debug)]
enum EvalStep {
    /// Evaluation complete, return this result
    Done(EvalResult),
    /// Need to evaluate S-expression items (iteratively)
    EvalSExpr {
        items: Vec<MettaValue>,
        env: Environment,
        depth: usize,
    },
    /// Start TCO grounded operation (e.g., +, -, and, or)
    /// This defers evaluation to the trampoline for proper tail call handling
    StartGroundedOp {
        state: GroundedState,
        env: Environment,
        depth: usize,
    },
    /// Start let binding - first evaluates value expression, then pattern matches
    /// and evaluates body. This enables let body to participate in trampoline (TCO).
    StartLetBinding {
        /// Pattern to match against evaluated value
        pattern: MettaValue,
        /// Value expression to evaluate first
        value_expr: MettaValue,
        /// Body template to instantiate with bindings
        body: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
    },
    /// Evaluate if branch - condition has been evaluated, now evaluate selected branch.
    /// This enables if branches to participate in trampoline (TCO).
    EvalIfBranch {
        /// Branch expression to evaluate (then or else)
        branch: MettaValue,
        /// Environment after condition evaluation
        env: Environment,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
    },
    /// Evaluate rule matches with UNEVALUATED arguments (lazy evaluation semantics).
    /// This is used when user-defined rules match before argument evaluation.
    /// MeTTa HE uses normal-order (lazy) evaluation for rule arguments.
    EvalRuleMatchesLazy {
        /// Matched rules: (RHS expression, bindings from pattern match)
        /// RHS is Arc-wrapped for O(1) cloning
        matches: Vec<(Arc<MettaValue>, Bindings)>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
}

/// Result of processing collected S-expression results
#[derive(Debug)]
enum ProcessedSExpr {
    /// Processing complete, return this result
    Done(EvalResult),
    /// Need to evaluate rule matches
    /// RHS is Arc-wrapped for O(1) cloning
    EvalRuleMatches {
        matches: Vec<(Arc<MettaValue>, Bindings)>,
        env: Environment,
        depth: usize,
        base_results: Vec<MettaValue>,
    },
    /// Need to lazily process Cartesian product combinations
    EvalCombinations {
        combinations: CartesianProductIter,
        env: Environment,
        depth: usize,
    },
}

/// Perform a single step of evaluation.
/// Returns either a final result or indicates more work is needed.
fn eval_step(value: MettaValue, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_step", ?value, depth);

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

        // Atoms: check special tokens first, then tokenizer, then evaluate to themselves
        // This enables HE-compatible bind! semantics where tokens are replaced during evaluation
        MettaValue::Atom(ref name) => {
            // Special handling for &self - evaluates to the current module's space
            // This is HE-compatible behavior where &self is a space reference
            if name == "&self" {
                let space_handle = env.self_space();
                return EvalStep::Done((vec![MettaValue::Space(space_handle)], env));
            }

            if let Some(bound_value) = env.lookup_token(name) {
                // Token was registered via bind! - return the bound value
                EvalStep::Done((vec![bound_value], env))
            } else {
                // No binding - atom evaluates to itself
                EvalStep::Done((vec![value], env))
            }
        }

        // Ground types evaluate to themselves
        MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Type(_)
        | MettaValue::Space(_)
        | MettaValue::State(_)
        | MettaValue::Unit
        | MettaValue::Memo(_) => EvalStep::Done((vec![value], env)),

        // Empty sentinel - gets filtered out at result collection
        MettaValue::Empty => EvalStep::Done((vec![], env)),

        // S-expressions need special handling
        MettaValue::SExpr(items) => eval_sexpr_step(items, env, depth),

        // For conjunctions, evaluate goals left-to-right with binding threading
        MettaValue::Conjunction(goals) => EvalStep::Done(eval_conjunction(goals, env, depth)),
    }
}

/// Evaluate arguments that are grounded operations (hybrid lazy/eager evaluation).
///
/// This function implements a key insight for MeTTa evaluation:
/// - Grounded operations (like arithmetic) should be evaluated BEFORE pattern matching
/// - User-defined expressions should remain unevaluated for lazy pattern matching
///
/// Example: For `(countdown (- 3 1))`:
/// - The argument `(- 3 1)` is a grounded operation, so evaluate it to `2`
/// - Result: `(countdown 2)` - now pattern matching works correctly
///
/// Example: For `(wrapper $a (add-atom &stack x))`:
/// - The argument `(add-atom &stack x)` is NOT grounded (user-defined side effect)
/// - Keep it unevaluated for lazy pattern matching
fn evaluate_grounded_args(items: &[MettaValue], env: &Environment) -> Vec<MettaValue> {
    if items.is_empty() {
        return items.to_vec();
    }

    let mut result = Vec::with_capacity(items.len());

    // Keep the first item (operator) as-is
    result.push(items[0].clone());

    // Process arguments (items after the first)
    for item in &items[1..] {
        match item {
            MettaValue::SExpr(sub_items) if !sub_items.is_empty() => {
                // Check if this is a grounded operation
                if let Some(MettaValue::Atom(op)) = sub_items.first() {
                    if is_grounded_op(op) {
                        // This is a grounded operation - evaluate it eagerly
                        // Recursively evaluate grounded args in sub-expression first
                        let evaluated_sub = evaluate_grounded_args(sub_items, env);
                        let (results, _) = eval(MettaValue::SExpr(evaluated_sub), env.clone());

                        // Use the first result (deterministic evaluation for grounded ops)
                        if let Some(first_result) = results.first() {
                            result.push(first_result.clone());
                        } else {
                            // Evaluation returned nothing - keep original
                            result.push(item.clone());
                        }
                        continue;
                    }
                }
                // Not a grounded operation - keep unevaluated (lazy)
                result.push(item.clone());
            }
            _ => {
                // Not an S-expression - keep as-is
                result.push(item.clone());
            }
        }
    }

    result
}

/// Evaluate an S-expression step - handles special forms and delegates to iterative collection
fn eval_sexpr_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_sexpr_step",?items, depth);

    // Preprocess to combine `& self` into `&self` for HE-compatible space references
    let items = preprocess_space_refs(items);

    if items.is_empty() {
        // HE-compatible: empty SExpr () evaluates to itself, not Nil
        // This is important for collapse semantics: collapse of one () result is (())
        return EvalStep::Done((vec![MettaValue::SExpr(vec![])], env));
    }

    // Check for special forms - these are handled directly (they manage their own recursion)
    if let Some(MettaValue::Atom(op)) = items.first() {
        match op.as_str() {
            "=" => return EvalStep::Done(space::eval_add(items, env)),
            "!" => return EvalStep::Done(evaluation::force_eval(items, env)),
            "quote" => return EvalStep::Done(quoting::eval_quote(items, env)),
            "if" => return control_flow::eval_if_step(items, env, depth),
            "error" => return EvalStep::Done(errors::eval_error(items, env)),
            // HE compatibility: (Error details msg) -> adapt to MeTTaTron's (error msg details)
            "Error" => return EvalStep::Done(errors::eval_error_he(items, env)),
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
            "let" => return bindings::eval_let_step(items, env, depth),
            "let*" => return EvalStep::Done(bindings::eval_let_star(items, env)),
            "unify" => return EvalStep::Done(bindings::eval_unify(items, env)),
            "sealed" => return EvalStep::Done(bindings::eval_sealed(items, env)),
            "atom-subst" => return EvalStep::Done(bindings::eval_atom_subst(items, env)),
            ":" => return EvalStep::Done(types::eval_type_assertion(items, env)),
            "get-type" => return EvalStep::Done(types::eval_get_type(items, env)),
            "check-type" => return EvalStep::Done(types::eval_check_type(items, env)),
            "map-atom" => return EvalStep::Done(list_ops::eval_map_atom(items, env)),
            "filter-atom" => return EvalStep::Done(list_ops::eval_filter_atom(items, env)),
            "foldl-atom" => return EvalStep::Done(list_ops::eval_foldl_atom(items, env)),
            "car-atom" => return EvalStep::Done(list_ops::eval_car_atom(items, env)),
            "cdr-atom" => return EvalStep::Done(list_ops::eval_cdr_atom(items, env)),
            "cons-atom" => return EvalStep::Done(list_ops::eval_cons_atom(items, env)),
            "decons-atom" => return EvalStep::Done(list_ops::eval_decons_atom(items, env)),
            "size-atom" => return EvalStep::Done(list_ops::eval_size_atom(items, env)),
            "max-atom" => return EvalStep::Done(list_ops::eval_max_atom(items, env)),
            // Additional expression operations from main
            "index-atom" => return EvalStep::Done(expression::eval_index_atom(items, env)),
            "min-atom" => return EvalStep::Done(expression::eval_min_atom(items, env)),
            // Space Operations
            "new-space" => return EvalStep::Done(space::eval_new_space(items, env)),
            "add-atom" => return EvalStep::Done(space::eval_add_atom(items, env)),
            "remove-atom" => return EvalStep::Done(space::eval_remove_atom(items, env)),
            "collapse" => return EvalStep::Done(space::eval_collapse(items, env)),
            "collapse-bind" => return EvalStep::Done(space::eval_collapse_bind(items, env)),
            "superpose" => return EvalStep::Done(space::eval_superpose(items, env)),
            // Advanced Nondeterminism (Phase G)
            "amb" => return EvalStep::Done(space::eval_amb(items, env)),
            "guard" => return EvalStep::Done(space::eval_guard(items, env)),
            "commit" => return EvalStep::Done(space::eval_commit(items, env)),
            "backtrack" => return EvalStep::Done(space::eval_backtrack(items, env)),
            "get-atoms" => return EvalStep::Done(space::eval_get_atoms(items, env)),
            // State Operations
            "new-state" => return EvalStep::Done(space::eval_new_state(items, env)),
            "get-state" => return EvalStep::Done(space::eval_get_state(items, env)),
            "change-state!" => return EvalStep::Done(space::eval_change_state(items, env)),
            // Memoization Operations
            "new-memo" => return EvalStep::Done(space::eval_new_memo(items, env)),
            "memo" => return EvalStep::Done(space::eval_memo(items, env)),
            "memo-first" => return EvalStep::Done(space::eval_memo_first(items, env)),
            "clear-memo!" => return EvalStep::Done(space::eval_clear_memo(items, env)),
            "memo-stats" => return EvalStep::Done(space::eval_memo_stats(items, env)),
            // Token Binding (HE-compatible tokenizer-based bind!)
            "bind!" => return EvalStep::Done(modules::eval_bind(items, env)),
            // I/O Operations
            "println!" => return EvalStep::Done(io::eval_println(items, env)),
            "trace!" => return EvalStep::Done(io::eval_trace(items, env)),
            "nop" => return EvalStep::Done(io::eval_nop(items, env)),
            // String Operations
            "repr" => return EvalStep::Done(strings::eval_repr(items, env)),
            "format-args" => return EvalStep::Done(strings::eval_format_args(items, env)),
            // Utility Operations
            "empty" => return EvalStep::Done(utilities::eval_empty(items, env)),
            "get-metatype" => return EvalStep::Done(utilities::eval_get_metatype(items, env)),
            // Module Operations
            "include" => return EvalStep::Done(modules::eval_include(items, env)),
            "import!" => return EvalStep::Done(modules::eval_import(items, env)),
            "mod-space!" => return EvalStep::Done(modules::eval_mod_space(items, env)),
            "print-mods!" => return EvalStep::Done(modules::eval_print_mods(items, env)),
            // MORK Special Forms
            "exec" => return EvalStep::Done(mork_forms::eval_exec(items, env)),
            "coalg" => return EvalStep::Done(mork_forms::eval_coalg(items, env)),
            "lookup" => return EvalStep::Done(mork_forms::eval_lookup(items, env)),
            "rulify" => return EvalStep::Done(mork_forms::eval_rulify(items, env)),
            _ => {}
        }
    }

    // HE-compatible lazy evaluation: try grounded operations and rules with UNEVALUATED args first
    let sexpr = MettaValue::SExpr(items.clone());

    // Step 1: Try grounded operations with RAW (unevaluated) arguments
    // First try TCO-enabled operations (which use trampoline for deep recursion),
    // then fall back to legacy operations if no TCO version exists.
    if let Some(MettaValue::Atom(op)) = items.first() {
        // Try TCO operation first - these don't call eval() internally and are
        // safe for arbitrarily deep recursion
        if env.get_grounded_operation_tco(op).is_some() {
            // Create initial state for the grounded operation
            let state = GroundedState::new(op.clone(), items[1..].to_vec());
            return EvalStep::StartGroundedOp { state, env, depth };
        }

        // Fall back to legacy grounded operations (non-TCO)
        // These call eval() internally and may overflow the Rust stack on deep recursion
        if let Some(grounded_op) = env.get_grounded_operation(op) {
            // Create an eval function closure for grounded operations to use
            let eval_fn = |value: MettaValue, env_inner: Environment| -> (Vec<MettaValue>, Environment) {
                eval(value, env_inner)
            };

            match grounded_op.execute_raw(&items[1..], &env, &eval_fn) {
                Ok(results) => {
                    // Grounded operation succeeded
                    let values: Vec<MettaValue> = results.into_iter().map(|(v, _)| v).collect();
                    return EvalStep::Done((values, env));
                }
                Err(ExecError::NoReduce) => {
                    // Not applicable - fall through to rule matching
                }
                Err(ExecError::Runtime(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            Arc::new(MettaValue::Atom("TypeError".to_string())),
                        )],
                        env,
                    ));
                }
                Err(ExecError::IncorrectArgument(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            Arc::new(MettaValue::Atom("ArityError".to_string())),
                        )],
                        env,
                    ));
                }
                Err(ExecError::Arithmetic(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
                        )],
                        env,
                    ));
                }
            }
        }
    }

    // Step 2: Evaluate grounded sub-expressions BEFORE rule matching (hybrid lazy/eager)
    // This ensures that arithmetic operations like (- 3 1) are evaluated to values
    // before being used in pattern matching, while keeping user-defined expressions lazy.
    //
    // WHY: Pure lazy evaluation causes infinite loops with recursive rules like:
    //   (= (countdown $n) (countdown (- $n 1)))
    // Because `$n` binds to `(- 3 1)` instead of `2`, the expression grows infinitely.
    //
    // SOLUTION: Evaluate arguments that are GROUNDED operations (like +, -, *, /)
    // but keep user-defined expressions unevaluated (for lazy pattern matching).
    let items_with_grounded_evaluated = evaluate_grounded_args(&items, &env);

    // Step 3: Try user rule matching with partially-evaluated arguments
    // Grounded operations in arguments are now values, user-defined expressions are still lazy.
    let resolved_items = resolve_tokens_shallow(&items_with_grounded_evaluated, &env);
    let resolved_sexpr = MettaValue::SExpr(resolved_items.clone());
    let all_matches = try_match_all_rules(&resolved_sexpr, &env);

    if !all_matches.is_empty() {
        // User rules matched - evaluate RHS with bindings from pattern match
        return EvalStep::EvalRuleMatchesLazy {
            matches: all_matches,
            env,
            depth,
        };
    }

    // Step 4: No lazy rules matched - expression is irreducible (data constructor).
    // In HE semantics, if no rule matches the unevaluated expression, it's a data constructor.
    // We still evaluate arguments (for grounded operations within them) but don't retry rule matching.
    EvalStep::EvalSExpr { items: items_with_grounded_evaluated, env, depth }
}

/// Process collected S-expression evaluation results.
/// This handles Cartesian products, builtins, and rule matching.
/// Uses lazy Cartesian product for memory-efficient nondeterministic evaluation.
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

    // Generate lazy Cartesian product of all sub-expression results
    match cartesian_product_lazy(eval_results) {
        CartesianProductResult::Empty => {
            // No combinations possible (empty result list)
            ProcessedSExpr::Done((vec![], unified_env))
        }
        CartesianProductResult::Single(evaled_items) => {
            // FAST PATH: Single combination (deterministic evaluation)
            // Process it directly without creating continuation
            // Convert SmallVec to Vec for downstream functions
            process_single_combination(evaled_items.into_vec(), unified_env, depth)
        }
        CartesianProductResult::Lazy(combinations) => {
            // LAZY PATH: Multiple combinations - process via continuation
            ProcessedSExpr::EvalCombinations {
                combinations,
                env: unified_env,
                depth,
            }
        }
    }
}

/// Process a single combination (fast path for deterministic evaluation).
/// This avoids creating a continuation when there's only one combination to process.
///
/// MeTTa HE semantics: After evaluating arguments, TRY RULE MATCHING AGAIN.
/// This is essential for patterns like (intensity (color)) where:
/// 1. (intensity (color)) doesn't match any rule (lazy)
/// 2. Evaluate args: (color) → [red, green, blue]
/// 3. (intensity red), (intensity green), (intensity blue) NOW match intensity rules
fn process_single_combination(
    evaled_items: Vec<MettaValue>,
    unified_env: Environment,
    depth: usize,
) -> ProcessedSExpr {
    // Check if this is a grounded operation
    if let Some(MettaValue::Atom(op)) = evaled_items.first() {
        if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
            return ProcessedSExpr::Done((vec![result], unified_env));
        }
    }

    // MeTTa HE semantics: After argument evaluation, try rule matching AGAIN.
    // The newly-evaluated arguments may now match rules that didn't match before.
    // Example: (intensity (color)) → (intensity red) → 100
    let sexpr = MettaValue::SExpr(evaled_items.clone());
    let all_matches = try_match_all_rules(&sexpr, &unified_env);

    if !all_matches.is_empty() {
        // Rules match with evaluated arguments - evaluate the rule RHS
        return ProcessedSExpr::EvalRuleMatches {
            matches: all_matches,
            env: unified_env,
            depth,
            base_results: vec![],
        };
    }

    // No rules matched even with evaluated arguments - this is a data constructor.
    // Check for typos and emit helpful warnings
    let mut env = unified_env;
    let result = handle_no_rule_match(evaled_items, &sexpr, &mut env);
    ProcessedSExpr::Done((vec![result], env))
}

/// Handle the case where no rule matches an s-expression
///
/// Uses context-aware smart suggestion heuristics (issue #51) to avoid false positives:
/// - **Arity filtering**: `(lit p)` won't suggest `let` (arity 1 != 3)
/// - **Type filtering**: `(match "hello" ...)` won't suggest match (String != Space)
/// - **Prefix compatibility**: `$steck` suggests `$stack`, not `&stack`
/// - Detects data constructor patterns (PascalCase, hyphenated names)
/// - Emits warnings (notes) instead of errors for suggestions
///
/// This allows intentional data constructors like `lit` to work without
/// triggering spurious "Did you mean: let?" errors.
fn handle_no_rule_match(
    evaled_items: Vec<MettaValue>,
    sexpr: &MettaValue,
    unified_env: &mut Environment,
) -> MettaValue {
    use crate::backend::fuzzy_match::SuggestionConfidence;

    // Check for likely typos before falling back to ADD mode
    if let Some(MettaValue::Atom(head)) = evaled_items.first() {
        // Check for misspelled special form using context-aware heuristics
        // The three-pillar validation filters out structurally incompatible suggestions
        if let Some(suggestion) = suggest_special_form_with_context(head, &evaled_items, unified_env)
        {
            trace!(
                target: "mettatron::backend::eval::handle_no_rule_match",
                head, ?suggestion, "Unknown special form"
            );
            // Always emit as a note/warning, never as an error
            // This allows the expression to continue evaluating in ADD mode
            match suggestion.confidence {
                SuggestionConfidence::High => {
                    eprintln!(
                        "Warning: '{}' is not defined. {}",
                        head, suggestion.message
                    );
                }
                SuggestionConfidence::Low => {
                    eprintln!("Note: '{}' is not defined. {}", head, suggestion.message);
                }
                SuggestionConfidence::None => {
                    // No suggestion - don't print anything
                }
            }
            // Fall through to ADD mode (don't return error)
        }

        // Check for misspelled rule head using smart heuristics
        // TODO: Add context-aware version for user-defined rules
        if let Some(suggestion) = unified_env.smart_did_you_mean(head, 2) {
            trace!(
                target: "mettatron::backend::eval::handle_no_rule_match",
                head, ?suggestion, "No rule matches"
            );
            match suggestion.confidence {
                SuggestionConfidence::High => {
                    eprintln!(
                        "Warning: No rule matches '{}'. {}",
                        head, suggestion.message
                    );
                }
                SuggestionConfidence::Low => {
                    eprintln!("Note: No rule matches '{}'. {}", head, suggestion.message);
                }
                SuggestionConfidence::None => {
                    // No suggestion - don't print anything
                }
            }
            // Fall through to ADD mode (don't return error)
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

        // or with non-boolean FIRST arg should error
        // (With short-circuit, (or True "hello") returns True without checking second arg)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::String("hello".to_string()),
            MettaValue::Bool(true),
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
    fn test_pattern_match_empty_sexpr_matches_empty_only() {
        // Empty S-expression () should ONLY match empty values (Nil, empty S-expr, Unit, Empty atom)
        // Use _ for wildcard pattern to match anything
        let pattern = MettaValue::SExpr(vec![]);

        // Should NOT match Long
        let value = MettaValue::Long(42);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());

        // Should NOT match String
        let value = MettaValue::String("hello".to_string());
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());

        // Should NOT match non-empty S-expression
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());

        // SHOULD match Nil
        let value = MettaValue::Nil;
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        assert!(bindings.unwrap().is_empty());

        // SHOULD match another empty S-expression
        let value = MettaValue::SExpr(vec![]);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        assert!(bindings.unwrap().is_empty());

        // SHOULD match Unit
        let value = MettaValue::Unit;
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        assert!(bindings.unwrap().is_empty());

        // SHOULD match Empty atom
        let value = MettaValue::Atom("Empty".to_string());
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some());
        assert!(bindings.unwrap().is_empty());

        // Should NOT match Bool
        let value = MettaValue::Bool(true);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_none());
    }

    #[test]
    fn test_eval_with_rule() {
        let mut env = Environment::new();

        // Add rule: (= (double $x) (mul $x 2))
        let rule = Rule::new(
        MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
    );
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
        let rule = Rule::new(
        MettaValue::SExpr(vec![
                MettaValue::Atom("safe-div".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        MettaValue::SExpr(vec![
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
    );
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

        let rule = Rule::new(
        MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
        MettaValue::SExpr(vec![
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
    );
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
        let rule = Rule::new(
        MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
        MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
    );
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
        let rule = Rule::new(
        MettaValue::SExpr(vec![
                MettaValue::Atom("add3".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
                MettaValue::Atom("$c".to_string()),
            ]),
        MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$b".to_string()),
                    MettaValue::Atom("$c".to_string()),
                ]),
            ]),
    );
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
        let rule = Rule::new(
        MettaValue::SExpr(vec![
                MettaValue::Atom("eval-pair".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("pair".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
        MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
    );
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
        let rule = Rule::new(
        MettaValue::SExpr(vec![
                MettaValue::Atom("abs".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        MettaValue::SExpr(vec![
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
    );
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
        // Error should propagate from the conjunction
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
        // Nested conjunction should evaluate to the last result
        assert_eq!(results[0], MettaValue::Long(7));
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
            MettaValue::Error(msg, _details) => {
                assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
                assert!(
                    msg.contains("expected Number"),
                    "Expected type info in: {}",
                    msg
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_string_comparison() {
        let env = Environment::new();

        // Test: !(< "a" "b") - lexicographic string comparison
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::String("b".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true)); // "a" < "b" lexicographically
    }

    #[test]
    fn test_comparison_mixed_type_error() {
        let env = Environment::new();

        // Test: !(< "hello" 42) - mixed types should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::String("hello".to_string()),
            MettaValue::Long(42),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _details) => {
                // The error should indicate incompatible types
                assert!(
                    msg.contains("type") || msg.contains("Cannot compare"),
                    "Expected type mismatch error in: {}",
                    msg
                );
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
    // Fuzzy Matching / Typo Detection Tests
    // ========================================================================

    #[test]
    fn test_misspelled_special_form() {
        // Issue #51: When a misspelled special form is detected, a warning is printed
        // to stderr but the expression is returned as-is (ADD mode semantics).
        // This allows intentional data constructors like `lit` to work without errors.
        let env = Environment::new();

        // Try to use "mach" instead of "match" (4 chars, passes min length check)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("mach".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::Atom("pattern".to_string()),
        ]);

        let (results, _) = eval(expr.clone(), env);
        assert_eq!(results.len(), 1);

        // Per issue #51: undefined symbols are treated as data (ADD mode)
        // A warning is printed to stderr, but the expression is returned as data
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::Atom("mach".to_string()));
                // &self is now resolved to a Space reference
                assert!(
                    matches!(items[1], MettaValue::Space(_)),
                    "Expected &self to be resolved to Space, got {:?}",
                    items[1]
                );
                assert_eq!(items[2], MettaValue::Atom("pattern".to_string()));
            }
            other => panic!("Expected SExpr (data), got {:?}", other),
        }
    }

    #[test]
    fn test_undefined_symbol_with_rule_suggestion() {
        // Issue #51: When a misspelled function is detected, a warning is printed
        // to stderr but the expression is returned as-is (ADD mode semantics).
        let mut env = Environment::new();

        // Add a rule for "fibonacci"
        let rule = Rule::new(
            MettaValue::SExpr(vec![
                MettaValue::Atom("fibonacci".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
        );
        env.add_rule(rule);

        // Try to call "fibonaci" (misspelled - missing 'n')
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("fibonaci".to_string()),
            MettaValue::Long(5),
        ]);

        let (results, _) = eval(expr.clone(), env);
        assert_eq!(results.len(), 1);

        // Per issue #51: Should return the expression unchanged (ADD mode)
        // A warning is printed to stderr, but no error is returned
        if let MettaValue::SExpr(items) = &results[0] {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Atom("fibonaci".to_string()));
            assert_eq!(items[1], MettaValue::Long(5));
        } else {
            panic!(
                "Expected SExpr returned unchanged (ADD mode), got: {:?}",
                results[0]
            );
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

    // ==========================================================================
    // Lazy Cartesian Product Iterator Tests
    // ==========================================================================

    #[test]
    fn test_cartesian_product_iter_basic() {
        // Test basic 2x2 cartesian product
        let results = vec![
            vec![MettaValue::Long(1), MettaValue::Long(2)],
            vec![MettaValue::Long(10), MettaValue::Long(20)],
        ];
        let iter = CartesianProductIter::new(results).expect("Should create iterator");

        let combos: Vec<Combination> = iter.collect();

        assert_eq!(combos.len(), 4);
        assert_eq!(combos[0].as_slice(), &[MettaValue::Long(1), MettaValue::Long(10)]);
        assert_eq!(combos[1].as_slice(), &[MettaValue::Long(1), MettaValue::Long(20)]);
        assert_eq!(combos[2].as_slice(), &[MettaValue::Long(2), MettaValue::Long(10)]);
        assert_eq!(combos[3].as_slice(), &[MettaValue::Long(2), MettaValue::Long(20)]);
    }

    #[test]
    fn test_cartesian_product_iter_single_element() {
        // All single-element lists - deterministic case
        let results = vec![
            vec![MettaValue::Long(1)],
            vec![MettaValue::Long(2)],
            vec![MettaValue::Long(3)],
        ];
        let iter = CartesianProductIter::new(results).expect("Should create iterator");

        let combos: Vec<Combination> = iter.collect();

        assert_eq!(combos.len(), 1);
        assert_eq!(
            combos[0].as_slice(),
            &[MettaValue::Long(1), MettaValue::Long(2), MettaValue::Long(3)]
        );
    }

    #[test]
    fn test_cartesian_product_iter_empty_input() {
        // Empty outer vector - the iterator is created but produces no combinations
        // Note: cartesian_product_lazy handles this case specially by returning Single(vec![])
        let results: Vec<Vec<MettaValue>> = vec![];
        let iter = CartesianProductIter::new(results);

        // Iterator is created (Some) but produces no items because results.is_empty() check in next()
        assert!(iter.is_some());
        let combos: Vec<Combination> = iter.unwrap().collect();
        assert!(combos.is_empty());
    }

    #[test]
    fn test_cartesian_product_iter_empty_list() {
        // One empty list should return None
        let results = vec![
            vec![MettaValue::Long(1), MettaValue::Long(2)],
            vec![], // Empty list
            vec![MettaValue::Long(10), MettaValue::Long(20)],
        ];
        let iter = CartesianProductIter::new(results);

        assert!(iter.is_none());
    }

    #[test]
    fn test_cartesian_product_iter_3x3x3() {
        // Test 3x3x3 = 27 combinations
        let results = vec![
            vec![MettaValue::Long(1), MettaValue::Long(2), MettaValue::Long(3)],
            vec![
                MettaValue::Atom("a".into()),
                MettaValue::Atom("b".into()),
                MettaValue::Atom("c".into()),
            ],
            vec![MettaValue::Bool(true), MettaValue::Bool(false), MettaValue::Nil],
        ];
        let iter = CartesianProductIter::new(results).expect("Should create iterator");

        let combos: Vec<Combination> = iter.collect();

        assert_eq!(combos.len(), 27);

        // Verify first and last combinations
        assert_eq!(
            combos[0].as_slice(),
            &[MettaValue::Long(1), MettaValue::Atom("a".into()), MettaValue::Bool(true)]
        );
        assert_eq!(
            combos[26].as_slice(),
            &[MettaValue::Long(3), MettaValue::Atom("c".into()), MettaValue::Nil]
        );
    }

    #[test]
    fn test_cartesian_product_lazy_count() {
        // Verify iterator is lazy by checking memory usage pattern
        let results = cartesian_product_lazy(vec![
            vec![MettaValue::Long(1), MettaValue::Long(2)],
            vec![MettaValue::Long(10), MettaValue::Long(20), MettaValue::Long(30)],
        ]);

        match results {
            CartesianProductResult::Lazy(iter) => {
                // Count combinations without storing them all
                let count = iter.count();
                assert_eq!(count, 6);
            }
            _ => panic!("Expected Lazy variant for nondeterministic case"),
        }
    }

    #[test]
    fn test_cartesian_product_lazy_single_returns_single() {
        // Fast path: single combination returns Single variant
        let results = cartesian_product_lazy(vec![
            vec![MettaValue::Long(1)],
            vec![MettaValue::Long(2)],
        ]);

        match results {
            CartesianProductResult::Single(combo) => {
                assert_eq!(combo.as_slice(), &[MettaValue::Long(1), MettaValue::Long(2)]);
            }
            _ => panic!("Expected Single variant for deterministic case"),
        }
    }

    #[test]
    fn test_cartesian_product_lazy_empty_returns_single_empty() {
        // Empty input returns Single(vec![]) - the identity element
        // The Cartesian product of nothing is a single empty tuple
        let results = cartesian_product_lazy(vec![]);

        match results {
            CartesianProductResult::Single(combo) => {
                assert!(combo.is_empty(), "Should be empty tuple");
            }
            _ => panic!("Expected Single(vec![]) for empty input"),
        }
    }

    #[test]
    fn test_cartesian_product_lazy_with_empty_list_returns_empty() {
        // Empty list in results returns Empty variant
        let results = cartesian_product_lazy(vec![
            vec![MettaValue::Long(1)],
            vec![], // Empty
        ]);

        match results {
            CartesianProductResult::Empty => {}
            _ => panic!("Expected Empty variant when one list is empty"),
        }
    }

    #[test]
    fn test_cartesian_product_ordering_preserved() {
        // Verify outer-product ordering is preserved (rightmost index varies fastest)
        let results = vec![
            vec![MettaValue::Long(1), MettaValue::Long(2)],     // First dimension
            vec![MettaValue::Long(10), MettaValue::Long(20)],   // Second dimension
        ];
        let iter = CartesianProductIter::new(results).expect("Should create iterator");

        let combos: Vec<Combination> = iter.collect();

        // Ordering: (1,10), (1,20), (2,10), (2,20)
        // Rightmost index varies fastest
        assert_eq!(combos[0].as_slice(), &[MettaValue::Long(1), MettaValue::Long(10)]);
        assert_eq!(combos[1].as_slice(), &[MettaValue::Long(1), MettaValue::Long(20)]);
        assert_eq!(combos[2].as_slice(), &[MettaValue::Long(2), MettaValue::Long(10)]);
        assert_eq!(combos[3].as_slice(), &[MettaValue::Long(2), MettaValue::Long(20)]);
    }

    #[test]
    fn test_nondeterministic_cartesian_product() {
        // Integration test: nondeterministic evaluation using lazy Cartesian product
        // (= (a) 1)
        // (= (a) 2)
        // (= (b) 10)
        // (= (b) 20)
        // !(+ (a) (b))
        // Expected: [11, 21, 12, 22]

        let mut env = Environment::new();

        // Add rules for (a) -> 1 and (a) -> 2
        env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
        MettaValue::Long(1),
    ));
        env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
        MettaValue::Long(2),
    ));

        // Add rules for (b) -> 10 and (b) -> 20
        env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::Atom("b".to_string())]),
        MettaValue::Long(10),
    ));
        env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::Atom("b".to_string())]),
        MettaValue::Long(20),
    ));

        // Evaluate (+ (a) (b))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
            MettaValue::SExpr(vec![MettaValue::Atom("b".to_string())]),
        ]);

        let (results, _) = eval(expr, env);

        // Should have 4 results: 1+10=11, 1+20=21, 2+10=12, 2+20=22
        assert_eq!(results.len(), 4);

        let mut result_values: Vec<i64> = results
            .iter()
            .filter_map(|v| match v {
                MettaValue::Long(n) => Some(*n),
                _ => None,
            })
            .collect();
        result_values.sort();

        assert_eq!(result_values, vec![11, 12, 21, 22]);
    }
}
