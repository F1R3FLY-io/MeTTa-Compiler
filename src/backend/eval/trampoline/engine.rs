//! Trampoline Engine - Iterative Evaluation
//!
//! This module contains the main `eval_trampoline` function that implements
//! iterative evaluation using an explicit work stack instead of recursive
//! function calls. This prevents stack overflow for deeply nested expressions.

use std::collections::VecDeque;
use std::sync::Arc;

use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::grounded::{ExecError, GroundedWork};
use crate::backend::models::{EvalResult, MettaValue};

use super::types::{Continuation, WorkItem, MAX_EVAL_DEPTH};
use super::super::{
    apply_bindings, eval_step, friendly_value_repr, process_collected_sexpr,
    EvalStep, ProcessedSExpr,
};

/// Iterative evaluation using a trampoline pattern with explicit work stack.
/// This prevents stack overflow by using heap-allocated work items instead of
/// recursive function calls.
pub fn eval_trampoline(value: MettaValue, env: Environment) -> EvalResult {
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
                                            super::super::try_eval_builtin(op, &evaled_items[1..])
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
                                    let all_matches = super::super::try_match_all_rules(&sexpr, &env);

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
                                    let result_value = super::super::handle_no_rule_match(evaled_vec, &sexpr, &mut env);
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
                                            if let Some(bindings) = super::super::pattern_match(&pattern, &value)
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
                                            if let Some(bindings) = super::super::pattern_match(&pattern, &value)
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
