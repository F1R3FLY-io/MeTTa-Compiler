//! WAM Execution Engine: Instruction dispatch and state management.
//!
//! The engine executes WAM instruction sequences to perform pattern matching
//! against rule LHS patterns. It operates as a **rule dispatch accelerator**
//! inside the existing trampoline — not a replacement.
//!
//! # Execution Model
//!
//! 1. Input expression loaded into register A0
//! 2. WAM instructions execute sequentially, checking structure and binding variables
//! 3. On match success: RHS template + bindings returned to trampoline
//! 4. On match failure: backtrack to next alternative (choice point) or return empty
//! 5. All alternatives explored (all-solutions semantics)
//!
//! # Integration with Trampoline
//!
//! ```text
//! eval_trampoline_generic()
//!   → eval_step_generic() identifies S-expr with rules
//!   → wam_dispatch_rules() called with expression + compiled WAM code
//!   → returns Vec<(MettaValue, GenericBindings)>
//!   → dispatch_rule_matches() handles result distribution
//! ```
//!
//! The WAM engine does NOT replace the trampoline. It replaces only the
//! `try_match_all_rules_generic` + structural/MORK matching path with a faster
//! register-based instruction dispatch.

use std::sync::Arc;

use crate::backend::eval::trampoline::MettaEnvironment;
use crate::backend::models::{GenericBindings, MettaValue};

use super::binding_frame::WamBindingFrame;
use super::choice_point::WamChoicePoint;
use super::compiler::{WamCode, RhsInfo, WamIndexKey};
use super::instructions::{WamInstruction, GroundedBinaryOp};
use super::registers::WamRegisters;
use super::trail::{Trail, TrailEntry};

/// Result of a single WAM match: the RHS info + bindings from the match.
#[derive(Clone, Debug)]
pub struct WamMatchResult {
    /// The RHS info (template, has_variables, rhs_type, multiplicity).
    pub rhs_info: RhsInfo,
    /// Bindings produced by the match (converted from WamBindingFrame).
    pub bindings: GenericBindings<MettaValue>,
}

/// Saved caller state for the heap call stack (Phase 3 remediation).
///
/// When `CallUserFunc` dispatches a recursive call, the caller's state is
/// saved into a `WamCallFrame` and pushed onto `WamState.call_stack`.
/// The callee executes in the same `execute_wam` loop. On callee completion
/// (IP past code end), `handle_callee_return` restores the caller state.
struct WamCallFrame {
    /// Instruction pointer to resume at on callee success.
    return_ip: usize,
    /// Caller's WAM code (the code being executed before the call).
    return_code: Arc<WamCode>,
    /// Caller's register file.
    saved_registers: WamRegisters,
    /// Caller's binding frame.
    saved_frame: WamBindingFrame,
    /// Trail position at call time (callee entries discarded on return).
    trail_mark: usize,
    /// Caller's choice points (isolated from callee).
    saved_choice_points: Vec<WamChoicePoint>,
    /// Caller's accumulated match results.
    saved_match_results: Vec<WamMatchResult>,
    /// Caller's pending default offset.
    saved_pending_default: Option<usize>,
    /// Register to store the callee's result in on success.
    result_reg: u8,
    /// IP to jump to if the callee does not produce a single leaf result.
    fallback_ip: u16,
}

/// WAM execution state for a single rule dispatch operation.
///
/// Created fresh for each `wam_dispatch_rules` call. Not reused across
/// evaluations (the trampoline manages evaluation lifecycle).
pub struct WamState {
    /// Argument registers for value passing.
    pub registers: WamRegisters,
    /// Current binding frame (one per rule match attempt).
    pub frame: WamBindingFrame,
    /// Trail for binding undo on backtrack.
    pub trail: Trail,
    /// Choice point stack for nondeterministic branching.
    pub choice_points: Vec<WamChoicePoint>,
    /// Accumulated match results from all successful alternatives.
    pub match_results: Vec<WamMatchResult>,
    /// Current instruction pointer (index into code.instructions).
    pub ip: usize,
    /// The WAM code being executed.
    pub code: Arc<WamCode>,
    /// Whether a `Proceed` instruction was reached (successful match).
    /// Used by `wam_try_match` to distinguish success from failure.
    pub matched: bool,
    /// Pending default group offset for first-argument indexing.
    ///
    /// When `SwitchOnFirstArg` matches an indexed group, this is set to
    /// the default group's instruction offset. After the indexed group is
    /// fully exhausted (no more choice points), `wam_fail` transitions to
    /// the default group for all-solutions semantics.
    pub pending_default_offset: Option<usize>,
    /// Environment for Phase 3 recursive WAM calls (CallUserFunc).
    /// `None` for unit tests and Phase 1/2 usage without environment.
    pub env: Option<MettaEnvironment>,
    /// Current WAM call depth (for recursion limiting in Phase 3).
    pub depth: usize,
    /// Heap-allocated call stack for recursive CallUserFunc dispatch.
    /// Each entry saves the caller's state so the callee executes in the
    /// same `execute_wam` loop (no Rust stack recursion).
    call_stack: Vec<WamCallFrame>,
}

/// Maximum WAM call stack depth for CallUserFunc.
/// Heap-allocated, so no stack overflow risk — this is a policy limit
/// to prevent infinite recursion from consuming unbounded memory.
const MAX_WAM_DEPTH: usize = 256;

impl WamState {
    /// Create a new WAM state for executing the given code.
    pub fn new(code: Arc<WamCode>, input: MettaValue) -> Self {
        let mut registers = WamRegisters::new();
        registers.load_input(input);

        let frame = WamBindingFrame::with_names(&code.slot_names, 0);

        WamState {
            registers,
            frame,
            trail: Trail::new(),
            choice_points: Vec::with_capacity(4),
            match_results: Vec::new(),
            ip: 0,
            code,
            matched: false,
            pending_default_offset: None,
            env: None,
            depth: 0,
            call_stack: Vec::new(),
        }
    }

    /// Collect all GC roots from the WAM state.
    pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
        self.registers.collect_gc_roots(out);
        self.frame.collect_gc_roots(out);
        self.trail.collect_gc_roots(out);
        for cp in &self.choice_points {
            cp.collect_gc_roots(out);
        }
        collect_match_result_roots(&self.match_results, out);
        // Constants referenced by LoadConst during execution
        for c in &self.code.constants {
            out.push(*c);
        }
        // Heap call stack: saved caller frames
        for frame in &self.call_stack {
            frame.saved_registers.collect_gc_roots(out);
            frame.saved_frame.collect_gc_roots(out);
            for cp in &frame.saved_choice_points {
                cp.collect_gc_roots(out);
            }
            collect_match_result_roots(&frame.saved_match_results, out);
            for c in &frame.return_code.constants {
                out.push(*c);
            }
        }
    }
}

/// Match a single rule's compiled WAM code against an expression.
///
/// This is a drop-in replacement for `StructuralMatcher::try_match()`.
/// Returns `Some(bindings)` if the LHS pattern matches, `None` otherwise.
///
/// The WAM code must end with `Proceed` (from `compile_rule_lhs`).
/// The `matched` flag on `WamState` distinguishes success (`Proceed` reached)
/// from failure (`wam_fail` terminated execution).
///
/// Takes `&Arc<WamCode>` to avoid cloning the entire WamCode struct — only
/// the Arc refcount is incremented (O(1) instead of O(instructions + slots)).
pub fn wam_try_match(
    expr: &MettaValue,
    code: &Arc<WamCode>,
) -> Option<GenericBindings<MettaValue>> {
    let mut state = WamState::new(code.clone(), *expr);
    execute_wam(&mut state);

    if state.matched {
        Some(state.frame.to_generic_bindings())
    } else {
        None
    }
}

/// Collect GC roots from a slice of match results.
fn collect_match_result_roots(results: &[WamMatchResult], out: &mut Vec<MettaValue>) {
    for result in results {
        out.push(result.rhs_info.template);
        if let Some(rhs_type) = result.rhs_info.rhs_type {
            out.push(rhs_type);
        }
        for (_, v) in result.bindings.iter() {
            out.push(*v);
        }
    }
}

/// Execute WAM code to find all matching rules for an expression.
///
/// This is the primary entry point for WAM-based rule dispatch. It executes
/// the compiled WAM instructions, exploring all alternatives (all-solutions
/// semantics), and returns the match results as `(MettaValue, GenericBindings)`
/// pairs compatible with the existing `dispatch_rule_matches` function.
///
/// # Arguments
/// * `value` - The expression to match against rules
/// * `code` - Compiled WAM code for the rule group
///
/// # Returns
/// A vector of `(rhs_template, bindings)` pairs for each successful match,
/// expanded by multiplicity (matching existing behavior).
pub fn wam_dispatch_rules(
    value: MettaValue,
    code: &Arc<WamCode>,
) -> Vec<(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>, bool)> {
    wam_dispatch_rules_with_env(value, code, None)
}

/// Execute WAM with environment for Phase 3 recursive calls.
///
/// When `env` is `Some`, the WAM engine can recursively dispatch
/// user-defined function calls via `CallUserFunc` instructions.
pub fn wam_dispatch_rules_with_env(
    value: MettaValue,
    code: &Arc<WamCode>,
    env: Option<MettaEnvironment>,
) -> Vec<(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>, bool)> {
    let mut state = WamState::new(code.clone(), value);
    state.env = env;

    // Execute the instruction loop
    execute_wam(&mut state);

    // Convert match results to the format expected by dispatch_rule_matches.
    convert_match_results(state.match_results)
}

/// Convert WAM match results to the 4-tuple format for dispatch_rule_matches.
/// Moves bindings for the last multiplicity copy (no clone for multiplicity=1).
fn convert_match_results(
    match_results: Vec<WamMatchResult>,
) -> Vec<(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>, bool)> {
    let mut results = Vec::with_capacity(match_results.len());
    for match_result in match_results {
        let multiplicity = match_result.rhs_info.multiplicity as usize;
        if multiplicity == 0 {
            continue;
        }
        let template = match_result.rhs_info.template;
        let rhs_type = match_result.rhs_info.rhs_type;
        let has_vars = match_result.rhs_info.has_variables;

        // Clone for extra copies beyond the first
        for _ in 1..multiplicity {
            results.push((template, match_result.bindings.clone(), rhs_type, has_vars));
        }
        // Move bindings for the last copy (no clone)
        results.push((template, match_result.bindings, rhs_type, has_vars));
    }
    results
}

/// Handle callee return: restore caller state and process callee results.
///
/// Called when the callee's IP runs past its code end (i.e., the callee
/// finished executing all its instructions/alternatives).
fn handle_callee_return(state: &mut WamState, frame: WamCallFrame) {
    // 1. Collect callee results
    let callee_results = std::mem::take(&mut state.match_results);

    // 2. Discard callee trail entries (caller frame is restored wholesale)
    state.trail.truncate(frame.trail_mark);

    // 3. Restore caller state
    state.registers = frame.saved_registers;
    state.frame = frame.saved_frame;
    state.code = frame.return_code;
    state.choice_points = frame.saved_choice_points;
    state.match_results = frame.saved_match_results;
    state.pending_default_offset = frame.saved_pending_default;
    state.depth -= 1;

    // 4. Process callee results: only handle single fully-evaluated leaf result
    let success = if callee_results.len() == 1 {
        let r = &callee_results[0];
        if !r.rhs_info.has_variables && r.bindings.is_empty() {
            let template = r.rhs_info.template;
            // Only accept leaf values (not S-expressions that need further eval)
            if template.as_sexpr().is_none() {
                state.registers.set(frame.result_reg, template);
                state.ip = frame.return_ip;
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if !success {
        state.ip = frame.fallback_ip as usize;
    }
}

/// The WAM instruction execution loop.
///
/// Dispatches instructions sequentially. On failure, backtracks to the most
/// recent choice point. Terminates when all alternatives have been explored.
fn execute_wam(state: &mut WamState) {
    loop {
        // Bounds check: if IP is past the end, check for callee return
        if state.ip >= state.code.instructions.len() {
            if let Some(frame) = state.call_stack.pop() {
                handle_callee_return(state, frame);
                continue;
            }
            break; // Top-level: done
        }

        let instruction = state.code.instructions[state.ip].clone();
        state.ip += 1;

        match instruction {
            // ════════════════════════════════════════════════════════════
            // Head Matching
            // ════════════════════════════════════════════════════════════

            WamInstruction::GetArity { reg, expected } => {
                let val = state.registers.get(reg);
                match val.as_sexpr() {
                    Some(items) if items.len() == expected as usize => {
                        // Arity matches, continue
                    }
                    _ => {
                        // Arity mismatch or not an S-expression
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::GetAtom { reg, expected } => {
                let val = state.registers.get(reg);
                match val.as_atom() {
                    Some(a) if std::ptr::eq(a, expected) || a == expected => {
                        // Atom matches (try pointer comparison first, then string)
                    }
                    _ => {
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::GetLong { reg, expected } => {
                let val = state.registers.get(reg);
                match val.as_long() {
                    Some(n) if n == expected => {}
                    _ => {
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::GetBool { reg, expected } => {
                let val = state.registers.get(reg);
                match val.as_bool() {
                    Some(b) if b == expected => {}
                    _ => {
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::GetFloat { reg, expected_bits } => {
                let val = state.registers.get(reg);
                match val.as_float() {
                    Some(f) if f.to_bits() == expected_bits => {}
                    _ => {
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::GetString { reg, expected } => {
                let val = state.registers.get(reg);
                match val.as_string() {
                    Some(s) if s == expected => {}
                    _ => {
                        wam_fail(state);
                        continue;
                    }
                }
            }

            // ════════════════════════════════════════════════════════════
            // Argument Decomposition
            // ════════════════════════════════════════════════════════════

            WamInstruction::GetArg {
                source_reg,
                child_index,
                target_reg,
            } => {
                let val = state.registers.get(source_reg);
                // SAFETY: GetArity has already verified the S-expression and its arity
                let items = val
                    .as_sexpr()
                    .expect("GetArg: source must be S-expr (verified by GetArity)");
                let child = items[child_index as usize];
                state.registers.set(target_reg, child);
            }

            // ════════════════════════════════════════════════════════════
            // Variable Binding
            // ════════════════════════════════════════════════════════════

            WamInstruction::BindSlot { reg, slot } => {
                let val = state.registers.get(reg);
                let prev = state.frame.get_slot(slot);
                state.trail.push(TrailEntry {
                    slot_index: slot,
                    previous: prev,
                });
                state.frame.set_slot_unchecked(slot, val);
            }

            WamInstruction::EqualCheck { reg, slot } => {
                let val = state.registers.get(reg);
                let bound = state.frame.get_slot(slot);
                if val != bound {
                    wam_fail(state);
                    continue;
                }
            }

            WamInstruction::LoadSlot { slot, target_reg } => {
                let val = state.frame.get_slot(slot);
                state.registers.set(target_reg, val);
            }

            // ════════════════════════════════════════════════════════════
            // Control Flow — Choice Points
            // ════════════════════════════════════════════════════════════

            WamInstruction::TryMeElse { next_alternative } => {
                // Create a choice point for backtracking
                state.choice_points.push(WamChoicePoint {
                    trail_mark: state.trail.mark(),
                    frame_slots: state.frame.num_slots(),
                    next_alternative: 0, // Not used directly (we use ip)
                    alternatives: Vec::new(), // Not used in this layout
                    results: Vec::new(),
                    env: MettaEnvironment::default_env(),
                    depth: 0,
                    parallel_budget: 0,
                });
                // Also save the IP for the next alternative
                if let Some(cp) = state.choice_points.last_mut() {
                    cp.next_alternative = next_alternative as usize;
                }
                // Continue with current alternative (next instruction)
            }

            WamInstruction::RetryMeElse { next_alternative } => {
                // Update the choice point's next alternative offset
                if let Some(cp) = state.choice_points.last_mut() {
                    cp.next_alternative = next_alternative as usize;
                }
                // Continue with current alternative
            }

            WamInstruction::TrustMe => {
                // Last alternative — choice point will be removed on completion/failure
                // Mark the choice point so we know to remove it
                if let Some(cp) = state.choice_points.last_mut() {
                    cp.next_alternative = usize::MAX; // Sentinel: no more alternatives
                }
            }

            // ════════════════════════════════════════════════════════════
            // Completion and Failure
            // ════════════════════════════════════════════════════════════

            WamInstruction::Proceed => {
                // Match succeeded without TailEval (standalone LHS compilation).
                // In practice, compile_rule_group replaces Proceed with TailEval.
                state.matched = true;
                break;
            }

            WamInstruction::TailEval {
                rhs_index,
                has_variables,
            } => {
                // Match succeeded! Extract bindings using per-rule slot names.
                let rhs_info = state.code.rhs_templates[rhs_index as usize].clone();

                // Phase 3: Skip binding extraction when RHS has no variables.
                // For ground RHS (e.g., `(= (f 0) "zero")`), bindings are unused
                // by apply_bindings_generic, so creating them wastes allocation.
                let bindings = if has_variables {
                    // Use the per-rule slot_names for correct variable name mapping.
                    // In multi-rule groups, each rule may have different variable names
                    // at different slot indices.
                    let saved_names = std::mem::replace(
                        &mut state.frame.names,
                        smallvec::SmallVec::from_slice(&rhs_info.slot_names),
                    );
                    let b = state.frame.to_generic_bindings();
                    state.frame.names = saved_names;
                    b
                } else {
                    GenericBindings::Empty
                };

                state.match_results.push(WamMatchResult {
                    rhs_info,
                    bindings,
                });

                // After recording the result, fall through to backtrack
                // to try the next alternative (all-solutions semantics).
                // The Fail instruction after TailEval handles this.
            }

            WamInstruction::Fail => {
                wam_fail(state);
                continue;
            }

            WamInstruction::YieldToTrampoline => {
                // Exit WAM, return to trampoline for evaluation of complex forms.
                break;
            }

            // ════════════════════════════════════════════════════════════
            // Phase 1: RHS Body Construction
            // ════════════════════════════════════════════════════════════

            WamInstruction::BuildSExpr {
                start_reg,
                count,
                target_reg,
            } => {
                if (count as usize) <= 16 {
                    // Stack-local array for small S-expressions (avoid Vec heap allocation)
                    let mut stack_buf: [MettaValue; 16] = [MettaValue::inline_unit(); 16];
                    for i in 0..count as usize {
                        stack_buf[i] = state.registers.get(start_reg + i as u8);
                    }
                    let sexpr = super::wam_alloc::wam_sexpr(&stack_buf[..count as usize]);
                    state.registers.set(target_reg, sexpr);
                } else {
                    // Fallback for large S-expressions (rare)
                    use crate::backend::models::{MettaValueFactory, gc_allocator::global_factory};
                    let factory = global_factory();
                    let items: Vec<MettaValue> = (0..count)
                        .map(|i| state.registers.get(start_reg + i))
                        .collect();
                    let sexpr = factory.sexpr(items);
                    state.registers.set(target_reg, sexpr);
                }
            }

            WamInstruction::LoadConst {
                const_index,
                target_reg,
            } => {
                let value = state.code.constants[const_index as usize];
                state.registers.set(target_reg, value);
            }

            // ════════════════════════════════════════════════════════════
            // Phase 4: Inline Grounded Operations
            // ════════════════════════════════════════════════════════════

            WamInstruction::CallGroundedBinary {
                op,
                left_reg,
                right_reg,
                target_reg,
            } => {
                let left = state.registers.args[left_reg as usize];
                let right = state.registers.args[right_reg as usize];

                match execute_grounded_binary(op, left, right) {
                    Some(result) => {
                        state.registers.args[target_reg as usize] = result;
                    }
                    None => {
                        // Type mismatch — fail this alternative
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::ReturnEvaluated {
                rhs_index,
                result_reg,
            } => {
                // The grounded operation result is already in result_reg.
                // Emit it as a match result with empty bindings.
                let rhs_info = state.code.rhs_templates[rhs_index as usize].clone();
                let result_value = state.registers.args[result_reg as usize];

                state.match_results.push(WamMatchResult {
                    rhs_info: RhsInfo {
                        template: result_value,
                        has_variables: false,
                        rhs_type: rhs_info.rhs_type,
                        multiplicity: rhs_info.multiplicity,
                        slot_names: Vec::new(),
                    },
                    bindings: GenericBindings::Empty,
                });

                // Continue to try next alternative (all-solutions semantics)
            }

            // ════════════════════════════════════════════════════════════
            // Phase 2: Body Evaluation — Control Flow
            // ════════════════════════════════════════════════════════════

            WamInstruction::BranchOnBool {
                cond_reg,
                then_ip,
                else_ip,
            } => {
                let val = state.registers.get(cond_reg);
                match val.as_bool() {
                    Some(true) => {
                        state.ip = then_ip as usize;
                    }
                    Some(false) => {
                        state.ip = else_ip as usize;
                    }
                    None => {
                        // Non-boolean condition: fail this alternative.
                        // The compiler guard (condition_guaranteed_boolean) should
                        // prevent this, but type mismatches in grounded ops at
                        // runtime can produce non-boolean values.
                        wam_fail(state);
                        continue;
                    }
                }
            }

            WamInstruction::Jump { target_ip } => {
                state.ip = target_ip as usize;
            }

            // ════════════════════════════════════════════════════════════
            // Phase 3: Recursive WAM Execution
            // ════════════════════════════════════════════════════════════

            WamInstruction::CallUserFunc {
                expr_reg,
                result_reg,
                fallback_ip,
            } => {
                let expr = state.registers.get(expr_reg);

                // Attempt heap call stack dispatch (no Rust stack recursion)
                let dispatched = 'dispatch: {
                    // Check depth limit (heap-allocated, policy limit only)
                    if state.call_stack.len() >= MAX_WAM_DEPTH {
                        break 'dispatch false;
                    }

                    // Extract head atom and arity from the call expression
                    let items = match expr.as_sexpr() {
                        Some(items) if !items.is_empty() => items,
                        _ => break 'dispatch false,
                    };
                    let head = match items[0].as_atom() {
                        Some(h) => h,
                        None => break 'dispatch false,
                    };
                    let arity = items.len() - 1;

                    // Need environment for rule lookup
                    let env = match &state.env {
                        Some(env) => env,
                        None => break 'dispatch false,
                    };

                    // Look up WAM code for the callee
                    let callee_code = {
                        let rule_index = env.shared.rule_index.read();
                        rule_index.get_wam_group_code(head, arity)
                    };
                    let callee_code = match callee_code {
                        Some(code) => code,
                        None => break 'dispatch false,
                    };

                    // Build call frame from current caller state.
                    // return_ip = state.ip (already past CallUserFunc due to pre-increment)
                    let callee_frame = WamBindingFrame::with_names(
                        &callee_code.slot_names, 0,
                    );
                    let frame = WamCallFrame {
                        return_ip: state.ip,
                        return_code: std::mem::replace(&mut state.code, callee_code),
                        saved_registers: std::mem::replace(
                            &mut state.registers,
                            WamRegisters::new(),
                        ),
                        saved_frame: std::mem::replace(&mut state.frame, callee_frame),
                        trail_mark: state.trail.mark(),
                        saved_choice_points: std::mem::take(&mut state.choice_points),
                        saved_match_results: std::mem::take(&mut state.match_results),
                        saved_pending_default: state.pending_default_offset.take(),
                        result_reg,
                        fallback_ip,
                    };

                    // Load callee input and set IP to start
                    state.registers.load_input(expr);
                    state.ip = 0;
                    state.depth += 1;

                    // Push frame and continue in the same execute_wam loop
                    state.call_stack.push(frame);
                    true
                };

                if !dispatched {
                    state.ip = fallback_ip as usize;
                }
            }

            // ════════════════════════════════════════════════════════════
            // Phase 4: First-Argument Indexing
            // ════════════════════════════════════════════════════════════

            WamInstruction::SwitchOnFirstArg {
                table_index,
                default_offset,
            } => {
                let val = state.registers.get(0); // A0 = root expression

                // Extract first argument (items[1]) from root S-expression
                // and compute its discriminant key for index table lookup.
                let first_arg_key = val
                    .as_sexpr()
                    .filter(|items| items.len() >= 2)
                    .and_then(|items| compute_index_key(&items[1]));

                let table = &state.code.index_tables[table_index as usize];

                if let Some(ref key) = first_arg_key {
                    if let Some(&target_ip) = table.entries.get(key) {
                        // Found matching indexed group — jump to it.
                        state.ip = target_ip as usize;
                        // Set pending default for all-solutions semantics:
                        // after the indexed group is exhausted, also try the
                        // default (variable first-arg) rules.
                        let def_off = default_offset as usize;
                        if def_off < state.code.instructions.len() {
                            state.pending_default_offset = Some(def_off);
                        }
                        continue;
                    }
                }

                // No match in index table — go directly to default group.
                state.ip = default_offset as usize;
            }
        }
    }
}

/// Execute a binary grounded operation on two MettaValues.
///
/// Returns `Some(result)` on success, `None` on type mismatch.
/// Handles Long×Long, Float×Float, and Long×Float type combinations
/// (matching the trampoline's grounded operation semantics).
fn execute_grounded_binary(
    op: GroundedBinaryOp,
    left: MettaValue,
    right: MettaValue,
) -> Option<MettaValue> {
    use crate::backend::models::metta_value::ValueView;

    let left_view = left.view();
    let right_view = right.view();

    match (left_view, right_view) {
        // Long × Long → Long (arithmetic) or Bool (comparison)
        (ValueView::Long(a), ValueView::Long(b)) => match op {
            GroundedBinaryOp::Add => Some(MettaValue::Long(a.wrapping_add(b))),
            GroundedBinaryOp::Sub => Some(MettaValue::Long(a.wrapping_sub(b))),
            GroundedBinaryOp::Mul => Some(MettaValue::Long(a.wrapping_mul(b))),
            GroundedBinaryOp::Div => {
                if b == 0 { None } else { Some(MettaValue::Long(a / b)) }
            }
            GroundedBinaryOp::Mod => {
                if b == 0 { None } else { Some(MettaValue::Long(a % b)) }
            }
            GroundedBinaryOp::Lt => Some(MettaValue::inline_bool(a < b)),
            GroundedBinaryOp::Le => Some(MettaValue::inline_bool(a <= b)),
            GroundedBinaryOp::Gt => Some(MettaValue::inline_bool(a > b)),
            GroundedBinaryOp::Ge => Some(MettaValue::inline_bool(a >= b)),
            GroundedBinaryOp::Eq => Some(MettaValue::inline_bool(a == b)),
        },

        // Float × Float → Float (arithmetic) or Bool (comparison)
        (ValueView::Float(a), ValueView::Float(b)) => match op {
            GroundedBinaryOp::Add => Some(MettaValue::Float(a + b)),
            GroundedBinaryOp::Sub => Some(MettaValue::Float(a - b)),
            GroundedBinaryOp::Mul => Some(MettaValue::Float(a * b)),
            GroundedBinaryOp::Div => {
                if b == 0.0 { None } else { Some(MettaValue::Float(a / b)) }
            }
            GroundedBinaryOp::Mod => {
                if b == 0.0 { None } else { Some(MettaValue::Float(a % b)) }
            }
            GroundedBinaryOp::Lt => Some(MettaValue::inline_bool(a < b)),
            GroundedBinaryOp::Le => Some(MettaValue::inline_bool(a <= b)),
            GroundedBinaryOp::Gt => Some(MettaValue::inline_bool(a > b)),
            GroundedBinaryOp::Ge => Some(MettaValue::inline_bool(a >= b)),
            GroundedBinaryOp::Eq => Some(MettaValue::inline_bool(a == b)),
        },

        // Long × Float → Float (type promotion)
        (ValueView::Long(a), ValueView::Float(b)) => {
            let a = a as f64;
            match op {
                GroundedBinaryOp::Add => Some(MettaValue::Float(a + b)),
                GroundedBinaryOp::Sub => Some(MettaValue::Float(a - b)),
                GroundedBinaryOp::Mul => Some(MettaValue::Float(a * b)),
                GroundedBinaryOp::Div => {
                    if b == 0.0 { None } else { Some(MettaValue::Float(a / b)) }
                }
                GroundedBinaryOp::Mod => {
                    if b == 0.0 { None } else { Some(MettaValue::Float(a % b)) }
                }
                GroundedBinaryOp::Lt => Some(MettaValue::inline_bool(a < b)),
                GroundedBinaryOp::Le => Some(MettaValue::inline_bool(a <= b)),
                GroundedBinaryOp::Gt => Some(MettaValue::inline_bool(a > b)),
                GroundedBinaryOp::Ge => Some(MettaValue::inline_bool(a >= b)),
                GroundedBinaryOp::Eq => Some(MettaValue::inline_bool(a == b)),
            }
        }

        // Float × Long → Float (type promotion)
        (ValueView::Float(a), ValueView::Long(b)) => {
            let b = b as f64;
            match op {
                GroundedBinaryOp::Add => Some(MettaValue::Float(a + b)),
                GroundedBinaryOp::Sub => Some(MettaValue::Float(a - b)),
                GroundedBinaryOp::Mul => Some(MettaValue::Float(a * b)),
                GroundedBinaryOp::Div => {
                    if b == 0.0 { None } else { Some(MettaValue::Float(a / b)) }
                }
                GroundedBinaryOp::Mod => {
                    if b == 0.0 { None } else { Some(MettaValue::Float(a % b)) }
                }
                GroundedBinaryOp::Lt => Some(MettaValue::inline_bool(a < b)),
                GroundedBinaryOp::Le => Some(MettaValue::inline_bool(a <= b)),
                GroundedBinaryOp::Gt => Some(MettaValue::inline_bool(a > b)),
                GroundedBinaryOp::Ge => Some(MettaValue::inline_bool(a >= b)),
                GroundedBinaryOp::Eq => Some(MettaValue::inline_bool(a == b)),
            }
        }

        // Bool × Bool equality
        (ValueView::Bool(a), ValueView::Bool(b)) if op == GroundedBinaryOp::Eq => {
            Some(MettaValue::inline_bool(a == b))
        }

        // Unsupported type combination
        _ => None,
    }
}

/// Compute the index key for a runtime value (used by SwitchOnFirstArg).
///
/// Maps the value to its discriminant for hash-table lookup:
/// - Atom → Atom(name)
/// - Long → Long(n)
/// - Bool → Bool(b)
/// - Float → FloatBits(bits)
/// - S-expression with atom head → SExprHead(head)
/// - Other → None
fn compute_index_key(val: &MettaValue) -> Option<WamIndexKey> {
    use crate::backend::models::metta_value::ValueView;
    match val.view() {
        ValueView::Atom(name) => Some(WamIndexKey::Atom(name)),
        ValueView::Long(n) => Some(WamIndexKey::Long(n)),
        ValueView::Bool(b) => Some(WamIndexKey::Bool(b)),
        ValueView::Float(f) => Some(WamIndexKey::FloatBits(f.to_bits())),
        ValueView::SExpr(items) if !items.is_empty() => {
            items[0].as_atom().map(WamIndexKey::SExprHead)
        }
        _ => None,
    }
}

/// Handle a match failure: backtrack to the most recent choice point.
///
/// 1. Unwind the trail to restore bindings
/// 2. Reset registers (reload input into A0)
/// 3. Jump to the next alternative's instruction offset
/// 4. If no choice points remain, check `pending_default_offset` for
///    first-argument indexing fallthrough to the default group
/// 5. If no pending default, execution terminates
fn wam_fail(state: &mut WamState) {
    loop {
        match state.choice_points.last() {
            None => {
                // No more choice points — check pending default group transition.
                // After an indexed group is exhausted, the default group (variable
                // first-arg rules) must also be explored for all-solutions semantics.
                if let Some(offset) = state.pending_default_offset.take() {
                    let input = state.registers.args[0];
                    state.registers.reset();
                    state.registers.load_input(input);
                    state.ip = offset;
                    return;
                }
                // All alternatives truly exhausted
                state.ip = state.code.instructions.len(); // Terminate loop
                return;
            }
            Some(cp) if cp.next_alternative == usize::MAX => {
                // TrustMe: this was the last alternative, remove choice point
                let cp = state.choice_points.pop().expect("checked non-empty");
                state.trail.unwind_to(cp.trail_mark, &mut state.frame);
                // Reload input expression
                let input = state.registers.args[0]; // A0 always holds the original input
                state.registers.reset();
                state.registers.load_input(input);
                // Continue backtracking to the previous choice point
                continue;
            }
            Some(_) => {
                // There's a next alternative to try
                let cp = state.choice_points.last().expect("checked non-empty");
                let next_ip = cp.next_alternative;
                let trail_mark = cp.trail_mark;

                // Unwind trail to restore bindings
                state.trail.unwind_to(trail_mark, &mut state.frame);

                // Reload input expression into registers
                // (A0 always holds the original input expression for the rule group)
                let input = state.registers.args[0];
                state.registers.reset();
                state.registers.load_input(input);

                // Jump to the next alternative
                state.ip = next_ip;
                return;
            }
        }
    }
}

/// Default environment for choice points (lightweight stub).
///
/// In Phase 2, the actual environment will be threaded through from the
/// trampoline. For Phase 1 (unit testing), we use a default.
impl MettaEnvironment {
    pub(crate) fn default_env() -> Self {
        crate::backend::eval::trampoline::new_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::wam::compiler::{compile_rule_lhs, compile_rule_group, RhsInfo};
    use crate::backend::environment::rule_management::RuleEntry;
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::{MettaValue, MettaValueFactory};

    fn factory() -> crate::backend::models::GcFactory {
        global_factory()
    }

    // ═══════════════════════════════════════════════════════════════════
    // Single Rule Matching Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_match_atom_success() {
        let f = factory();
        let lhs = f.atom("foo");
        let mut code = compile_rule_lhs(&lhs).expect("compile");
        // Replace Proceed with TailEval
        if let Some(last) = code.instructions.last_mut() {
            *last = WamInstruction::TailEval {
                rhs_index: 0,
                has_variables: false,
            };
        }
        code.instructions.push(WamInstruction::Fail);
        let sn = code.slot_names.clone();
        code.rhs_templates.push(RhsInfo {
            template: f.atom("result"),
            has_variables: false,
            rhs_type: None,
            multiplicity: 1,
            slot_names: sn,
        });

        let code = Arc::new(code);
        let input = f.atom("foo");
        let mut state = WamState::new(code, input);
        execute_wam(&mut state);

        assert_eq!(state.match_results.len(), 1);
        assert_eq!(state.match_results[0].rhs_info.template, f.atom("result"));
        assert!(state.match_results[0].bindings.is_empty());
    }

    #[test]
    fn test_match_atom_failure() {
        let f = factory();
        let lhs = f.atom("foo");
        let mut code = compile_rule_lhs(&lhs).expect("compile");
        if let Some(last) = code.instructions.last_mut() {
            *last = WamInstruction::TailEval {
                rhs_index: 0,
                has_variables: false,
            };
        }
        code.instructions.push(WamInstruction::Fail);
        let sn = code.slot_names.clone();
        code.rhs_templates.push(RhsInfo {
            template: f.atom("result"),
            has_variables: false,
            rhs_type: None,
            multiplicity: 1,
            slot_names: sn,
        });

        let code = Arc::new(code);
        let input = f.atom("bar"); // Wrong atom
        let mut state = WamState::new(code, input);
        execute_wam(&mut state);

        assert_eq!(state.match_results.len(), 0);
    }

    #[test]
    fn test_match_variable_always_succeeds() {
        let f = factory();
        let lhs = f.atom("$x");
        let mut code = compile_rule_lhs(&lhs).expect("compile");
        if let Some(last) = code.instructions.last_mut() {
            *last = WamInstruction::TailEval {
                rhs_index: 0,
                has_variables: true,
            };
        }
        code.instructions.push(WamInstruction::Fail);
        let sn = code.slot_names.clone();
        code.rhs_templates.push(RhsInfo {
            template: f.atom("$x"),
            has_variables: true,
            rhs_type: None,
            multiplicity: 1,
            slot_names: sn,
        });

        let code = Arc::new(code);
        let input = MettaValue::Long(42);
        let mut state = WamState::new(code, input);
        execute_wam(&mut state);

        assert_eq!(state.match_results.len(), 1);
        let bindings = &state.match_results[0].bindings;
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
    }

    #[test]
    fn test_match_sexpr_with_variables() {
        let f = factory();
        // Rule: (= (f $x $y) rhs)
        // Input: (f 10 20)
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(10), MettaValue::Long(20)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        let (_, ref bindings, _, _) = results[0];
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(10)));
        assert_eq!(bindings.get("$y"), Some(&MettaValue::Long(20)));
    }

    #[test]
    fn test_match_sexpr_wrong_arity() {
        let f = factory();
        // Rule: (= (f $x $y) rhs) — expects arity 3
        // Input: (f 10) — arity 2
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(10)]);
        let results = wam_dispatch_rules(input, &code);

        assert!(results.is_empty());
    }

    #[test]
    fn test_match_sexpr_wrong_head() {
        let f = factory();
        // Rule: (= (f $x) rhs)
        // Input: (g 10) — wrong head
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x")]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("g"), MettaValue::Long(10)]);
        let results = wam_dispatch_rules(input, &code);

        assert!(results.is_empty());
    }

    #[test]
    fn test_match_nested_sexpr() {
        let f = factory();
        // Rule: (= (f (g $x) $y) rhs)
        // Input: (f (g 42) 99)
        let lhs = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.atom("$y"),
        ]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), MettaValue::Long(42)]),
            MettaValue::Long(99),
        ]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        let (_, ref bindings, _, _) = results[0];
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
        assert_eq!(bindings.get("$y"), Some(&MettaValue::Long(99)));
    }

    #[test]
    fn test_match_repeated_variable_success() {
        let f = factory();
        // Rule: (= (f $x $x) rhs) — repeated variable
        // Input: (f 42 42) — both args equal
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$x")]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(42), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        let (_, ref bindings, _, _) = results[0];
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
    }

    #[test]
    fn test_match_repeated_variable_failure() {
        let f = factory();
        // Rule: (= (f $x $x) rhs)
        // Input: (f 42 99) — args NOT equal
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$x")]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(42), MettaValue::Long(99)]);
        let results = wam_dispatch_rules(input, &code);

        assert!(results.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════════
    // Multi-Rule (Nondeterministic) Matching Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_multi_rule_all_solutions() {
        let f = factory();
        // Rule 1: (= (f 0) "zero")
        // Rule 2: (= (f $n) "other")
        // Input: (f 0) — should match BOTH rules (all-solutions semantics)

        let entry1 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]),
            rhs: f.atom("zero"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec![],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: false,
            structural_matcher: None,
            wam_code: None,
        };

        let entry2 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$n")]),
            rhs: f.atom("other"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$n"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry1, entry2]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]);
        let results = wam_dispatch_rules(input, &code);

        // Both rules should match (f 0)
        assert_eq!(results.len(), 2, "expected 2 matches (all-solutions), got {}", results.len());

        // Rule 1: rhs = "zero", no bindings
        assert_eq!(results[0].0, f.atom("zero"));
        assert!(results[0].1.is_empty());

        // Rule 2: rhs = "other", bindings: $n = 0
        assert_eq!(results[1].0, f.atom("other"));
        assert_eq!(results[1].1.get("$n"), Some(&MettaValue::Long(0)));
    }

    #[test]
    fn test_multi_rule_partial_match() {
        let f = factory();
        // Rule 1: (= (f 0) "zero")
        // Rule 2: (= (f 1) "one")
        // Input: (f 0) — should match only rule 1

        let entry1 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]),
            rhs: f.atom("zero"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec![],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: false,
            structural_matcher: None,
            wam_code: None,
        };

        let entry2 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(1)]),
            rhs: f.atom("one"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec![],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: false,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry1, entry2]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, f.atom("zero"));
    }

    #[test]
    fn test_multi_rule_no_match() {
        let f = factory();
        let entry1 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]),
            rhs: f.atom("zero"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec![],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: false,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry1]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(99)]);
        let results = wam_dispatch_rules(input, &code);

        assert!(results.is_empty());
    }

    #[test]
    fn test_multiplicity_expansion() {
        let f = factory();
        // Rule with multiplicity 3
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.atom("result"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 3,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        // Should be expanded 3 times
        assert_eq!(results.len(), 3);
        for (rhs, bindings, _, _) in &results {
            assert_eq!(*rhs, f.atom("result"));
            assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Trail + Backtracking Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_trail_unwind_on_backtrack() {
        let f = factory();
        // Rule 1: (= (f 0 $x) "zero") — binds $x
        // Rule 2: (= (f $n $y) "other") — binds $n, $y
        // Input: (f 0 42)
        // After rule 1 matches and backtracks, $x binding must be undone
        // before rule 2 attempts to bind $n and $y.

        let entry1 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(0), f.atom("$x")]),
            rhs: f.atom("zero"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let entry2 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$n"), f.atom("$y")]),
            rhs: f.atom("other"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$n", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry1, entry2]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(0), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        // Both rules should match
        assert_eq!(results.len(), 2);

        // Rule 1: $x = 42
        assert_eq!(results[0].0, f.atom("zero"));
        assert_eq!(results[0].1.get("$x"), Some(&MettaValue::Long(42)));

        // Rule 2: $n = 0, $y = 42
        assert_eq!(results[1].0, f.atom("other"));
        assert_eq!(results[1].1.get("$n"), Some(&MettaValue::Long(0)));
        assert_eq!(results[1].1.get("$y"), Some(&MettaValue::Long(42)));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Equivalence with StructuralMatcher Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_wam_matches_structural_matcher() {
        use crate::backend::environment::rule_management::StructuralMatcher;

        let f = factory();
        // Pattern: (f (g $x) $y)
        let lhs = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.atom("$y"),
        ]);

        let sm = StructuralMatcher::analyze(&lhs).expect("SM should compile");
        let wam_code_raw = compile_rule_lhs(&lhs).expect("WAM should compile");

        // Test with matching input
        let input = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), MettaValue::Long(42)]),
            MettaValue::Long(99),
        ]);

        // StructuralMatcher result
        let sm_result = sm.try_match(&input).expect("SM should match");

        // WAM result (manually execute)
        let mut code = wam_code_raw;
        if let Some(last) = code.instructions.last_mut() {
            *last = WamInstruction::TailEval {
                rhs_index: 0,
                has_variables: true,
            };
        }
        code.instructions.push(WamInstruction::Fail);
        let sn = code.slot_names.clone();
        code.rhs_templates.push(RhsInfo {
            template: f.atom("rhs"),
            has_variables: true,
            rhs_type: None,
            multiplicity: 1,
            slot_names: sn,
        });
        let code = Arc::new(code);
        let mut state = WamState::new(code, input);
        execute_wam(&mut state);
        assert_eq!(state.match_results.len(), 1);
        let wam_bindings = &state.match_results[0].bindings;

        // Both should produce the same bindings
        assert_eq!(sm_result.get("$x"), wam_bindings.get("$x"));
        assert_eq!(sm_result.get("$y"), wam_bindings.get("$y"));

        // Test with non-matching input
        let bad_input = f.sexpr(vec![
            f.atom("h"), // Wrong head
            f.sexpr(vec![f.atom("g"), MettaValue::Long(42)]),
            MettaValue::Long(99),
        ]);

        let sm_result2 = sm.try_match(&bad_input);
        assert!(sm_result2.is_none(), "SM should not match");

        let code2 = {
            let mut c = compile_rule_lhs(&lhs).expect("compile");
            if let Some(last) = c.instructions.last_mut() {
                *last = WamInstruction::TailEval {
                    rhs_index: 0,
                    has_variables: true,
                };
            }
            c.instructions.push(WamInstruction::Fail);
            let sn = c.slot_names.clone();
            c.rhs_templates.push(RhsInfo {
                template: f.atom("rhs"),
                has_variables: true,
                rhs_type: None,
                multiplicity: 1,
                slot_names: sn,
            });
            Arc::new(c)
        };
        let mut state2 = WamState::new(code2, bad_input);
        execute_wam(&mut state2);
        assert!(state2.match_results.is_empty(), "WAM should not match either");
    }

    #[test]
    fn test_wildcard_match() {
        let f = factory();
        // Rule: (= (f _ $y) rhs) — wildcard matches anything
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("_"), f.atom("$y")]);
        let rhs = f.atom("result");

        let entry = RuleEntry {
            lhs,
            rhs,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");

        // Match with any first argument
        let input = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("complex"), MettaValue::Long(1), MettaValue::Long(2)]),
            MettaValue::Long(99),
        ]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.get("$y"), Some(&MettaValue::Long(99)));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Integration: WAM matching through full evaluation pipeline
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_wam_integration_through_eval_pipeline() {
        // Verify that WAM matching works end-to-end through the evaluation pipeline.
        // This test adds rules to an environment, evaluates an expression, and checks
        // that the WAM-compiled rules produce correct results.
        use crate::backend::eval::trampoline::new_env;

        let source = r#"
            (= (double $x) (+ $x $x))
            !(double 21)
        "#;

        let state = crate::backend::compile(source).expect("compile");
        let mut env = new_env();

        // Evaluate each source expression (mirroring main.rs evaluation loop)
        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        let mut all_results: Vec<MettaValue> = Vec::new();
        for expr in source_exprs {
            let (results, updated_env) = crate::backend::eval::eval(expr, env, &state);
            env = updated_env;
            all_results.extend(results);
        }

        // (double 21) should evaluate to 42 via the WAM-matched rule
        let output: Vec<String> = all_results.iter().map(|v| format!("{}", v)).collect();
        assert!(
            output.iter().any(|s| s.contains("42")),
            "Expected 42 in output, got: {:?}",
            output
        );
    }

    #[test]
    fn test_wam_rule_entry_has_wam_code() {
        // Verify that rules added to the environment get WAM code compiled.
        use crate::backend::eval::trampoline::new_env;

        let f = factory();
        let mut env = new_env();

        // Add a rule: (= (f $x) $x)
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x")]);
        let rhs = f.atom("$x");
        env.add_rule(lhs, rhs);

        // Check that the rule has WAM code
        // get_arity() returns items.len() - 1, so (f $x) has arity 1
        let rule_index = env.shared.rule_index.read();
        let candidates: Vec<_> = rule_index.get_candidates("f", 1, None).collect();
        assert!(!candidates.is_empty(), "Should have candidates for f/1");
        assert!(
            candidates[0].wam_code.is_some(),
            "Rule (f $x) should have WAM code compiled"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 4: Inline Grounded Binary Operation Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_inline_grounded_add() {
        // Rule: (= (add $x $y) (+ $x $y))
        // Input: (add 10 20)
        // Expected: result template = Long(30) with has_variables = false
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("add"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("add"), MettaValue::Long(10), MettaValue::Long(20)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1, "should have 1 result, got {}", results.len());
        let (rhs, ref bindings, _, has_vars) = results[0];
        // The result template should be the computed value (30), not the original RHS
        assert_eq!(rhs, MettaValue::Long(30), "10 + 20 = 30");
        assert!(bindings.is_empty(), "inline grounded should produce empty bindings");
        assert!(!has_vars, "inline grounded result has no variables");
    }

    #[test]
    fn test_inline_grounded_mul_floats() {
        // Rule: (= (mul $x $y) (* $x $y))
        // Input: (mul 2.5 4.0)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("mul"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![f.atom("*"), f.atom("$x"), f.atom("$y")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("mul"), MettaValue::Float(2.5), MettaValue::Float(4.0)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Float(10.0), "2.5 * 4.0 = 10.0");
    }

    #[test]
    fn test_inline_grounded_comparison() {
        // Rule: (= (less $x $y) (< $x $y))
        // Input: (less 3 5) → True
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("less"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![f.atom("<"), f.atom("$x"), f.atom("$y")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("less"), MettaValue::Long(3), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::inline_bool(true), "3 < 5 = True");
    }

    #[test]
    fn test_inline_grounded_type_mismatch_falls_through() {
        // Rule: (= (add $x $y) (+ $x $y))
        // Input: (add "hello" "world") — strings can't be added, should fail the grounded op
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("add"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("add"), f.atom("hello"), f.atom("world")]);
        let results = wam_dispatch_rules(input, &code);

        // CallGroundedBinary fails for strings, triggers wam_fail → no results
        assert!(results.is_empty(), "string addition should produce no results");
    }

    #[test]
    fn test_inline_grounded_repeated_variable() {
        // Rule: (= (square $x) (* $x $x))
        // Input: (square 7)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("square"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("*"), f.atom("$x"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("square"), MettaValue::Long(7)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(49), "7 * 7 = 49");
    }

    #[test]
    fn test_inline_grounded_div_by_zero() {
        // Rule: (= (div $x $y) (/ $x $y))
        // Input: (div 10 0) → should fail (division by zero)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("div"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![f.atom("/"), f.atom("$x"), f.atom("$y")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("div"), MettaValue::Long(10), MettaValue::Long(0)]);
        let results = wam_dispatch_rules(input, &code);

        assert!(results.is_empty(), "division by zero should produce no results");
    }

    #[test]
    fn test_inline_grounded_mixed_types() {
        // Rule: (= (add $x $y) (+ $x $y))
        // Input: (add 2 3.5) → Long+Float = Float(5.5)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("add"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("add"), MettaValue::Long(2), MettaValue::Float(3.5)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Float(5.5), "2 + 3.5 = 5.5");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 1: RHS Body Construction Engine Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_body_construction_identity() {
        // Rule: (= (id $x) $x) — identity function
        // Input: (id 42) → 42 (direct LoadSlot result)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("id"), f.atom("$x")]),
            rhs: f.atom("$x"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("id"), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(42), "identity should return 42");
        assert!(results[0].1.is_empty(), "body-compiled results have empty bindings");
    }

    #[test]
    fn test_body_construction_wrap_sexpr() {
        // Rule: (= (wrap $x) (box $x)) — wraps value in S-expression
        // Input: (wrap 42) → (box 42)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("wrap"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("box"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("wrap"), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        let expected = f.sexpr(vec![f.atom("box"), MettaValue::Long(42)]);
        assert_eq!(results[0].0, expected, "should construct (box 42)");
        assert!(results[0].1.is_empty());
    }

    #[test]
    fn test_body_construction_nested() {
        // Rule: (= (f $x $y) (g (h $x) $y))
        // Input: (f 10 20) → (g (h 10) 20)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![
                f.atom("g"),
                f.sexpr(vec![f.atom("h"), f.atom("$x")]),
                f.atom("$y"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(10), MettaValue::Long(20)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        let expected = f.sexpr(vec![
            f.atom("g"),
            f.sexpr(vec![f.atom("h"), MettaValue::Long(10)]),
            MettaValue::Long(20),
        ]);
        assert_eq!(results[0].0, expected, "should construct (g (h 10) 20)");
        assert!(results[0].1.is_empty());
    }

    #[test]
    fn test_body_construction_multi_rule() {
        // Rule 1: (= (f 0) "zero") — ground, uses TailEval
        // Rule 2: (= (f $n) (succ $n)) — variable, uses body compilation
        // Input: (f 0) → matches both
        let f = factory();

        let entry1 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]),
            rhs: f.atom("zero"),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec![],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: false,
            structural_matcher: None,
            wam_code: None,
        };

        let entry2 = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$n")]),
            rhs: f.sexpr(vec![f.atom("succ"), f.atom("$n")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$n"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry1, entry2]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 2, "both rules should match");

        // Rule 1: TailEval → template "zero"
        assert_eq!(results[0].0, f.atom("zero"));

        // Rule 2: body compilation → constructed (succ 0)
        let expected = f.sexpr(vec![f.atom("succ"), MettaValue::Long(0)]);
        assert_eq!(results[1].0, expected, "body compilation should construct (succ 0)");
        assert!(results[1].1.is_empty(), "body-compiled results have empty bindings");
    }

    #[test]
    fn test_body_construction_repeated_variable() {
        // Rule: (= (dup $x) (pair $x $x)) — variable used twice in RHS
        // Input: (dup 42) → (pair 42 42)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("dup"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("pair"), f.atom("$x"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("dup"), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        let expected = f.sexpr(vec![f.atom("pair"), MettaValue::Long(42), MettaValue::Long(42)]);
        assert_eq!(results[0].0, expected, "should construct (pair 42 42)");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 4: First-Argument Indexing Execution Tests
    // ═══════════════════════════════════════════════════════════════════

    fn make_indexed_rule_group() -> (Arc<WamCode>, crate::backend::models::GcFactory) {
        let f = factory();
        // 4 rules:
        //   (f "a" 1) → 10   — key Atom("a")
        //   (f "a" 2) → 20   — key Atom("a")
        //   (f "b" 3) → 30   — key Atom("b")
        //   (f $x $y) → 40   — default (variable first arg)
        let entries: Vec<RuleEntry<MettaValue>> = vec![
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("a"), MettaValue::Long(1)]),
                rhs: MettaValue::Long(10),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("a"), MettaValue::Long(2)]),
                rhs: MettaValue::Long(20),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("b"), MettaValue::Long(3)]),
                rhs: MettaValue::Long(30),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
                rhs: MettaValue::Long(40),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$x", "$y"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            },
        ];

        let code = compile_rule_group(&entries).expect("indexed compilation should succeed");
        (code, f)
    }

    #[test]
    fn test_indexed_dispatch_matching_key() {
        let (code, f) = make_indexed_rule_group();
        // Input (f "a" 1): should match rule 0 (indexed key "a") + rule 3 (default)
        let input = f.sexpr(vec![f.atom("f"), f.atom("a"), MettaValue::Long(1)]);
        let results = wam_dispatch_rules(input, &code);

        let rhs_values: Vec<i64> = results
            .iter()
            .filter_map(|(v, _, _, _)| v.as_long())
            .collect();
        assert!(
            rhs_values.contains(&10),
            "should match indexed rule (f a 1) → 10, got {:?}",
            rhs_values
        );
        assert!(
            rhs_values.contains(&40),
            "should also match default rule (f $x $y) → 40, got {:?}",
            rhs_values
        );
        // Should NOT match (f a 2) because second arg doesn't match
        assert!(
            !rhs_values.contains(&20),
            "should not match (f a 2), got {:?}",
            rhs_values
        );
    }

    #[test]
    fn test_indexed_dispatch_key_not_found() {
        let (code, f) = make_indexed_rule_group();
        // Input (f "c" 5): key "c" not in index → only default rule matches
        let input = f.sexpr(vec![f.atom("f"), f.atom("c"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        let rhs_values: Vec<i64> = results
            .iter()
            .filter_map(|(v, _, _, _)| v.as_long())
            .collect();
        assert_eq!(
            rhs_values,
            vec![40],
            "only default rule should match for unknown key, got {:?}",
            rhs_values
        );
    }

    #[test]
    fn test_indexed_dispatch_multiple_in_bucket() {
        let (code, f) = make_indexed_rule_group();
        // Input (f "a" 2): should match rule 1 (indexed) + rule 3 (default)
        let input = f.sexpr(vec![f.atom("f"), f.atom("a"), MettaValue::Long(2)]);
        let results = wam_dispatch_rules(input, &code);

        let rhs_values: Vec<i64> = results
            .iter()
            .filter_map(|(v, _, _, _)| v.as_long())
            .collect();
        assert!(
            rhs_values.contains(&20),
            "should match (f a 2) → 20, got {:?}",
            rhs_values
        );
        assert!(
            rhs_values.contains(&40),
            "should also match default, got {:?}",
            rhs_values
        );
        // Should NOT match (f a 1) because second arg mismatch
        assert!(
            !rhs_values.contains(&10),
            "should not match (f a 1), got {:?}",
            rhs_values
        );
    }

    #[test]
    fn test_indexed_dispatch_other_indexed_key() {
        let (code, f) = make_indexed_rule_group();
        // Input (f "b" 3): should match rule 2 (key "b") + rule 3 (default)
        let input = f.sexpr(vec![f.atom("f"), f.atom("b"), MettaValue::Long(3)]);
        let results = wam_dispatch_rules(input, &code);

        let rhs_values: Vec<i64> = results
            .iter()
            .filter_map(|(v, _, _, _)| v.as_long())
            .collect();
        assert!(
            rhs_values.contains(&30),
            "should match (f b 3) → 30, got {:?}",
            rhs_values
        );
        assert!(
            rhs_values.contains(&40),
            "should also match default, got {:?}",
            rhs_values
        );
    }

    #[test]
    fn test_indexed_dispatch_no_match_in_bucket() {
        let (code, f) = make_indexed_rule_group();
        // Input (f "b" 99): key "b" found, but second arg mismatch → only default
        let input = f.sexpr(vec![f.atom("f"), f.atom("b"), MettaValue::Long(99)]);
        let results = wam_dispatch_rules(input, &code);

        let rhs_values: Vec<i64> = results
            .iter()
            .filter_map(|(v, _, _, _)| v.as_long())
            .collect();
        // Rule 2 requires (f "b" 3) — mismatch on 99
        assert!(
            !rhs_values.contains(&30),
            "should not match (f b 3) with input b/99, got {:?}",
            rhs_values
        );
        // Default rule (f $x $y) still matches anything
        assert!(
            rhs_values.contains(&40),
            "default should still match, got {:?}",
            rhs_values
        );
    }

    #[test]
    fn test_indexed_dispatch_with_variables() {
        // Test that variable bindings work correctly through indexed dispatch
        let f = factory();
        let entries: Vec<RuleEntry<MettaValue>> = vec![
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("g"), f.atom("a"), f.atom("$x")]),
                rhs: f.atom("$x"),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$x"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("g"), f.atom("b"), f.atom("$x")]),
                rhs: f.atom("$x"),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$x"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("g"), f.atom("c"), f.atom("$x")]),
                rhs: f.atom("$x"),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$x"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("g"), f.atom("d"), f.atom("$x")]),
                rhs: f.atom("$x"),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$x"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
            },
        ];

        let code = compile_rule_group(&entries).expect("compile");
        // Input (g "b" 42): should match only the "b" rule, binding $x = 42
        let input = f.sexpr(vec![f.atom("g"), f.atom("b"), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1, "only one rule should match, got {}", results.len());
        // The RHS is $x which should be bound to 42
        // With body compilation, the template is the constructed result
        let (template, bindings, _, has_vars) = &results[0];
        if *has_vars {
            // TailEval path: template is $x, bindings contain $x → 42
            let bound = bindings.iter().find(|(k, _)| *k == "$x");
            assert!(bound.is_some(), "should have binding for $x");
            assert_eq!(*bound.unwrap().1, MettaValue::Long(42));
        } else {
            // Body construction path: template is already 42
            assert_eq!(*template, MettaValue::Long(42), "constructed result should be 42");
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 2: Special Form Tests (if, let, let*, chain)
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_if_true_branch() {
        // Rule: (= (f $x $y) (if (< $x $y) "yes" "no"))
        // Input: (f 3 5) → 3 < 5 = True → "yes"
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                f.sexpr(vec![f.atom("<"), f.atom("$x"), f.atom("$y")]),
                f.string("yes"),
                f.string("no"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(3), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1, "should have 1 result, got {}", results.len());
        let (rhs, ref bindings, _, has_vars) = results[0];
        assert_eq!(rhs, f.string("yes"), "3 < 5 → True → 'yes'");
        assert!(bindings.is_empty(), "special form result has empty bindings");
        assert!(!has_vars, "special form result has no variables");
    }

    #[test]
    fn test_if_false_branch() {
        // Rule: (= (f $x $y) (if (< $x $y) "yes" "no"))
        // Input: (f 10 3) → 10 < 3 = False → "no"
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                f.sexpr(vec![f.atom("<"), f.atom("$x"), f.atom("$y")]),
                f.string("yes"),
                f.string("no"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(10), MettaValue::Long(3)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, f.string("no"), "10 < 3 → False → 'no'");
    }

    #[test]
    fn test_if_with_equality_comparison() {
        // Rule: (= (eq? $x $y) (if (== $x $y) True False))
        // Input: (eq? 5 5) → True
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("eq?"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                f.sexpr(vec![f.atom("=="), f.atom("$x"), f.atom("$y")]),
                MettaValue::inline_bool(true),
                MettaValue::inline_bool(false),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("eq?"), MettaValue::Long(5), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::inline_bool(true), "5 == 5 → True");
    }

    #[test]
    fn test_if_with_grounded_branches() {
        // Rule: (= (abs $x) (if (< $x 0) (- 0 $x) $x))
        // Input: (abs -5) → -5 < 0 = True → 0 - (-5) = 5
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("abs"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                f.sexpr(vec![f.atom("<"), f.atom("$x"), MettaValue::Long(0)]),
                f.sexpr(vec![f.atom("-"), MettaValue::Long(0), f.atom("$x")]),
                f.atom("$x"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");

        // Test negative input
        let input = f.sexpr(vec![f.atom("abs"), MettaValue::Long(-5)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(5), "abs(-5) = 5");

        // Test positive input
        let input = f.sexpr(vec![f.atom("abs"), MettaValue::Long(7)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(7), "abs(7) = 7");
    }

    #[test]
    fn test_if_non_boolean_condition_fallback() {
        // Rule: (= (f $x) (if $x "yes" "no"))
        // Condition is just a variable — not guaranteed boolean
        // Special form compilation fails → falls to body construction (the RHS
        // is an S-expression referencing $x, so it can be built as data)
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                f.atom("$x"),
                f.string("yes"),
                f.string("no"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        // Should NOT have BranchOnBool (special form compilation fails)
        let has_branch = code.instructions.iter().any(|i| matches!(i, WamInstruction::BranchOnBool { .. }));
        assert!(!has_branch, "non-boolean condition should not produce BranchOnBool");
        // Falls through to body construction (BuildSExpr), not TailEval
        let has_build = code.instructions.iter().any(|i| matches!(i, WamInstruction::BuildSExpr { .. }));
        assert!(has_build, "should fall back to body construction (BuildSExpr)");
    }

    #[test]
    fn test_let_binding() {
        // Rule: (= (double $x) (let $y (+ $x $x) $y))
        // Input: (double 5) → let y = 5+5=10, return y → 10
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("double"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("let"),
                f.atom("$y"),
                f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$x")]),
                f.atom("$y"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("double"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(10), "double(5) = 10");
        assert!(results[0].1.is_empty(), "special form result has empty bindings");
    }

    #[test]
    fn test_let_with_body_using_binding() {
        // Rule: (= (f $x) (let $y (+ $x 1) (+ $y $y)))
        // Input: (f 3) → let y = 3+1=4, return y+y=8
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("let"),
                f.atom("$y"),
                f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)]),
                f.sexpr(vec![f.atom("+"), f.atom("$y"), f.atom("$y")]),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(3)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(8), "let y=(3+1)=4, y+y=8");
    }

    #[test]
    fn test_letstar_sequential_bindings() {
        // Rule: (= (f $x) (let* (($a (+ $x 1)) ($b (+ $a $a))) $b))
        // Input: (f 3) → a=4, b=8 → 8
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("let*"),
                f.sexpr(vec![
                    f.sexpr(vec![f.atom("$a"), f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)])]),
                    f.sexpr(vec![f.atom("$b"), f.sexpr(vec![f.atom("+"), f.atom("$a"), f.atom("$a")])]),
                ]),
                f.atom("$b"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(3)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(8), "let* a=4, b=8 → 8");
    }

    #[test]
    fn test_chain_binding() {
        // Rule: (= (f $x) (chain (+ $x 10) $y $y))
        // Input: (f 5) → chain value=(5+10)=15, bind $y=15, body=$y → 15
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("chain"),
                f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(10)]),
                f.atom("$y"),
                f.atom("$y"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(15), "chain 5+10=15 → 15");
    }

    #[test]
    fn test_nested_if_in_let() {
        // Rule: (= (clamp $x $lo $hi) (let $cond (< $x $lo) (if $cond $lo $x)))
        // Wait, this won't work because $cond might not be guaranteed boolean from the
        // compiler's perspective. Instead use a directly nested form.
        //
        // Rule: (= (f $x $y) (let $sum (+ $x $y) (if (< $sum 10) $sum 10)))
        // Input: (f 3 5) → sum=8, 8<10 → 8
        // Input: (f 5 7) → sum=12, 12<10 → False → 10
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![
                f.atom("let"),
                f.atom("$sum"),
                f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]),
                f.sexpr(vec![
                    f.atom("if"),
                    f.sexpr(vec![f.atom("<"), f.atom("$sum"), MettaValue::Long(10)]),
                    f.atom("$sum"),
                    MettaValue::Long(10),
                ]),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x", "$y"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");

        // Case 1: sum=8, 8<10 → 8
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(3), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(8), "3+5=8 < 10 → 8");

        // Case 2: sum=12, 12<10=False → 10
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5), MettaValue::Long(7)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(10), "5+7=12 >= 10 → 10");
    }

    #[test]
    fn test_multi_rule_with_if_special_form() {
        // Rule 1: (= (f 0) "zero") — ground RHS
        // Rule 2: (= (f $n) (if (< $n 0) "negative" "positive"))
        // Input: (f 0) → "zero" (rule 1) AND "positive" (rule 2)
        // Input: (f -1) → "negative" (rule 2 only)
        let f = factory();

        let entries: Vec<RuleEntry<MettaValue>> = vec![
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]),
                rhs: f.string("zero"),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("$n")]),
                rhs: f.sexpr(vec![
                    f.atom("if"),
                    f.sexpr(vec![f.atom("<"), f.atom("$n"), MettaValue::Long(0)]),
                    f.string("negative"),
                    f.string("positive"),
                ]),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$n"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
            },
        ];

        let code = compile_rule_group(&entries).expect("compile");

        // Input (f -1): only rule 2 matches, $n=-1, -1<0=True → "negative"
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(-1)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, f.string("negative"), "(f -1) → 'negative'");

        // Input (f 0): rule 1 → "zero", rule 2 → 0<0=False → "positive"
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 2);
        let result_set: std::collections::HashSet<MettaValue> =
            results.iter().map(|(r, _, _, _)| *r).collect();
        assert!(result_set.contains(&f.string("zero")), "should contain 'zero'");
        assert!(result_set.contains(&f.string("positive")), "should contain 'positive'");
    }

    #[test]
    fn test_if_constant_fold_true() {
        // Rule: (= (f $x) (if True $x "unreachable"))
        // Constant True → always take then branch
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                MettaValue::inline_bool(true),
                f.atom("$x"),
                f.string("unreachable"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        // Should NOT have BranchOnBool (constant-folded)
        let has_branch = code.instructions.iter().any(|i| matches!(i, WamInstruction::BranchOnBool { .. }));
        assert!(!has_branch, "constant True should be folded away");

        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(42)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(42), "constant True → then branch");
    }

    #[test]
    fn test_if_constant_fold_false() {
        // Rule: (= (f $x) (if False "unreachable" $x))
        let f = factory();

        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                MettaValue::inline_bool(false),
                f.string("unreachable"),
                f.atom("$x"),
            ]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("compile");
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(99)]);
        let results = wam_dispatch_rules(input, &code);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, MettaValue::Long(99), "constant False → else branch");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 3: Recursive WAM Execution Tests (CallUserFunc)
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_call_user_func_basic() {
        // Callee: (= (g $x) (+ $x 1))  — grounded RHS, compiled to WAM
        // Caller: (= (f $x) (g $x))    — user call RHS
        // Input:  (f 5)
        // Expected: 6 (g evaluates (+ 5 1) = 6, f returns g's result)
        let f = factory();

        // Set up environment with callee rule
        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)]),
        );

        // Compile caller rule
        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        // Verify CallUserFunc was compiled
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(has_call, "should have CallUserFunc instruction");

        // Execute with environment
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        // CallUserFunc should resolve g(5) = 6 via recursive WAM execution
        assert_eq!(results.len(), 1, "should produce exactly 1 result");
        assert_eq!(results[0].0, MettaValue::Long(6), "f(5) = g(5) = 5+1 = 6");
        // Result should be fully evaluated (no bindings needed)
        assert!(results[0].1.is_empty(), "result should have empty bindings");
    }

    #[test]
    fn test_call_user_func_fallback_no_env() {
        // Caller: (= (f $x) (g $x)) — user call with NO environment
        // CallUserFunc should fall back since env is None
        let f = factory();

        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        // Execute WITHOUT environment
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules(input, &code);

        // Should fall back: return the built expression (g 5) for trampoline evaluation
        assert_eq!(results.len(), 1, "should produce 1 result (fallback)");
        let result_items = results[0].0.as_sexpr().expect("result should be S-expression");
        assert_eq!(result_items.len(), 2, "should be (g 5)");
        assert_eq!(result_items[0].as_atom(), Some("g"));
        assert_eq!(result_items[1], MettaValue::Long(5));
    }

    #[test]
    fn test_call_user_func_fallback_no_wam_code() {
        // Callee has NO rules in the environment (no WAM code for g)
        // CallUserFunc should fall back
        let f = factory();

        let env = crate::backend::eval::trampoline::new_env();
        // No rules added for "g"

        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        // Should fall back: return (g 5) for trampoline
        assert_eq!(results.len(), 1);
        let result_items = results[0].0.as_sexpr().expect("result should be S-expression");
        assert_eq!(result_items[0].as_atom(), Some("g"));
        assert_eq!(result_items[1], MettaValue::Long(5));
    }

    #[test]
    fn test_call_user_func_depth_limit() {
        // Callee: (= (g $x) (g $x)) — infinite recursion
        // Should hit depth limit and fall back
        let f = factory();

        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
        );

        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        // Depth limit should cause fallback — returns built expression (g 5)
        assert_eq!(results.len(), 1, "should produce 1 result despite depth limit");
        let result_items = results[0].0.as_sexpr().expect("result should be S-expression");
        assert_eq!(result_items[0].as_atom(), Some("g"));
    }

    #[test]
    fn test_call_user_func_chain() {
        // h(x) = x * 2, g(x) = h(x) + 1, f(x) = g(x)
        // But WAM can only resolve one level of recursion at a time from the caller
        // (g calls h, but g's RHS (+ (h $x) 1) is a grounded op with a nested user call —
        // try_compile_inline_eval won't handle it, so g falls back to body construction)
        //
        // Instead, test: g(x) = x + 1, f(x) = g(x)
        // This verifies the basic CallUserFunc works end-to-end
        let f = factory();

        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(10)]),
        );

        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(7)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        assert_eq!(results.len(), 1, "should produce 1 result");
        assert_eq!(results[0].0, MettaValue::Long(17), "f(7) = g(7) = 7+10 = 17");
    }

    #[test]
    fn test_call_user_func_multi_result_fallback() {
        // Callee has two rules: g(x) = x and g(x) = (+ x 1)
        // Multiple results → CallUserFunc should fall back
        let f = factory();

        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.atom("$x"),
        );
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)]),
        );

        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        // Multi-result callee → CallUserFunc falls back, returns (g 5)
        assert_eq!(results.len(), 1, "should produce 1 fallback result");
        let result_items = results[0].0.as_sexpr().expect("result should be S-expression");
        assert_eq!(result_items[0].as_atom(), Some("g"), "fallback returns built (g 5)");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 3 Remediation: Heap Call Stack Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_heap_call_stack_deep_recursion() {
        // Chain: f(x) -> g(x), g(x) -> h(x), h(x) -> (+ x 100)
        // This tests 3-level deep heap call stack (no Rust stack overflow risk)
        let f = factory();

        let mut env = crate::backend::eval::trampoline::new_env();
        // h(x) = x + 100
        env.add_rule(
            f.sexpr(vec![f.atom("h"), f.atom("$x")]),
            f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(100)]),
        );
        // g(x) = h(x)
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.sexpr(vec![f.atom("h"), f.atom("$x")]),
        );

        // Compile f(x) = g(x) as the caller
        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile caller");

        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        assert_eq!(results.len(), 1, "should produce 1 result from 3-level chain");
        assert_eq!(results[0].0, MettaValue::Long(105), "f(5) -> g(5) -> h(5) -> 5+100 = 105");
    }

    #[test]
    fn test_heap_call_stack_callee_choice_points_isolated() {
        // Callee g has two rules, but the caller f should NOT see g's choice points.
        // g(0) = "zero", g($n) = "other"
        // f($x) = (g $x) — CallUserFunc falls back (multiple results)
        let f = factory();

        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("g"), MettaValue::Long(0)]),
            f.atom("zero"),
        );
        env.add_rule(
            f.sexpr(vec![f.atom("g"), f.atom("$n")]),
            f.atom("other"),
        );

        let caller_entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[caller_entry]).expect("compile");

        // g(0) produces 2 results → CallUserFunc falls back
        let input = f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        // Should get 1 result (fallback: the built (g 0) expression)
        assert_eq!(results.len(), 1, "multi-result callee → fallback");
        assert!(results[0].0.as_sexpr().is_some(), "fallback should be S-expression");
    }

    #[test]
    fn test_heap_call_stack_gc_roots() {
        // Verify GC root collection includes call stack frames
        let f = factory();
        let code = Arc::new(WamCode {
            instructions: Vec::new(),
            num_slots: 0,
            slot_names: Vec::new(),
            rhs_templates: Vec::new(),
            constants: vec![MettaValue::Long(999)],
            index_tables: Vec::new(),
            fully_evaluable: false,
        });

        let mut state = WamState::new(code.clone(), MettaValue::Long(1));

        // Simulate a saved call frame with known values
        let mut saved_regs = WamRegisters::new();
        saved_regs.set(0, MettaValue::Long(42));
        let mut saved_frame = WamBindingFrame::with_names(&["$a"], 0);
        saved_frame.set_slot_unchecked(0, MettaValue::Long(77));

        state.call_stack.push(WamCallFrame {
            return_ip: 0,
            return_code: code,
            saved_registers: saved_regs,
            saved_frame,
            trail_mark: 0,
            saved_choice_points: Vec::new(),
            saved_match_results: vec![WamMatchResult {
                rhs_info: RhsInfo {
                    template: MettaValue::Long(88),
                    has_variables: false,
                    rhs_type: None,
                    multiplicity: 1,
                    slot_names: Vec::new(),
                },
                bindings: GenericBindings::Empty,
            }],
            saved_pending_default: None,
            result_reg: 0,
            fallback_ip: 0,
        });

        let mut roots = Vec::new();
        state.collect_gc_roots(&mut roots);

        // Should include: register value (42), frame binding (77),
        // match result template (88), and constants (999)
        assert!(roots.contains(&MettaValue::Long(42)), "call stack registers");
        assert!(roots.contains(&MettaValue::Long(77)), "call stack frame bindings");
        assert!(roots.contains(&MettaValue::Long(88)), "call stack match results");
        assert!(roots.contains(&MettaValue::Long(999)), "call stack constants");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 6 Tests: operator cache, eval_memo bypass, leaf short-circuit
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_operator_cache_wam_fully_evaluable() {
        use crate::backend::eval::trampoline::dispatch_hints::{
            operator_cache_put, operator_cache_get, OperatorCacheEntry,
        };
        use crate::backend::environment::rule_management::RULE_EPOCH;

        let epoch = RULE_EPOCH.load(std::sync::atomic::Ordering::Acquire);

        // Fully evaluable entry
        operator_cache_put("test_wam_op_fe", 2, OperatorCacheEntry {
            rule_epoch: epoch,
            all_structural: false,
            candidate_count: 0,
            wam_fully_evaluable: true,
        });
        let entry = operator_cache_get("test_wam_op_fe", 2);
        assert!(entry.is_some(), "entry should be cached");
        assert!(entry.expect("checked").wam_fully_evaluable, "should be fully evaluable");

        // Non-fully-evaluable entry
        operator_cache_put("test_wam_op_nfe", 1, OperatorCacheEntry {
            rule_epoch: epoch,
            all_structural: false,
            candidate_count: 0,
            wam_fully_evaluable: false,
        });
        let entry2 = operator_cache_get("test_wam_op_nfe", 1);
        assert!(entry2.is_some(), "entry should be cached");
        assert!(!entry2.expect("checked").wam_fully_evaluable, "should NOT be fully evaluable");
    }

    #[test]
    fn test_eval_memo_bypass_correctness() {
        use crate::backend::models::metta_state::MettaState;

        let f = factory();
        let state = MettaState::new();
        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("memo_f"), f.atom("$x")]),
            f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)]),
        );

        let expr = f.sexpr(vec![f.atom("memo_f"), MettaValue::Long(5)]);
        let (r1, _) = crate::backend::eval::eval_trampoline(expr, env.clone(), &state);
        let (r2, _) = crate::backend::eval::eval_trampoline(expr, env, &state);
        assert_eq!(r1.as_slice(), &[MettaValue::Long(6)], "first eval: memo_f(5) = 6");
        assert_eq!(r2.as_slice(), &[MettaValue::Long(6)], "second eval: memo_f(5) = 6 (consistent)");
    }

    #[test]
    fn test_wam_leaf_result_short_circuit() {
        use crate::backend::models::metta_state::MettaState;

        let f = factory();
        let state = MettaState::new();
        let mut env = crate::backend::eval::trampoline::new_env();
        env.add_rule(
            f.sexpr(vec![f.atom("leaf_f"), f.atom("$x")]),
            f.sexpr(vec![f.atom("*"), f.atom("$x"), MettaValue::Long(2)]),
        );

        let expr = f.sexpr(vec![f.atom("leaf_f"), MettaValue::Long(7)]);
        let (results, _) = crate::backend::eval::eval_trampoline(expr, env, &state);
        assert_eq!(results.as_slice(), &[MettaValue::Long(14)], "leaf_f(7) = 7 * 2 = 14");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 3: Deep recursion test (256 levels)
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_heap_call_stack_256_levels() {
        let f = factory();
        let mut env = crate::backend::eval::trampoline::new_env();

        // Base case: f_255(x) = x + 1000
        let base_name = format!("f_{}", MAX_WAM_DEPTH - 1);
        env.add_rule(
            f.sexpr(vec![f.atom(&base_name), f.atom("$x")]),
            f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1000)]),
        );

        // Chain: f_i(x) = f_{i+1}(x) for i in 1..255
        for i in (1..MAX_WAM_DEPTH - 1).rev() {
            let caller_name = format!("f_{}", i);
            let callee_name = format!("f_{}", i + 1);
            env.add_rule(
                f.sexpr(vec![f.atom(&caller_name), f.atom("$x")]),
                f.sexpr(vec![f.atom(&callee_name), f.atom("$x")]),
            );
        }

        // Entry: f_0(x) = f_1(x) — compiled as WAM
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f_0"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("f_1"), f.atom("$x")]),
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
        };
        let code = compile_rule_group(&[entry]).expect("compile 256-level chain");
        let input = f.sexpr(vec![f.atom("f_0"), MettaValue::Long(5)]);
        let results = wam_dispatch_rules_with_env(input, &code, Some(env));

        assert_eq!(results.len(), 1, "should produce 1 result from 256-level chain");
        assert_eq!(results[0].0, MettaValue::Long(1005),
            "f_0(5) → ... → f_255(5) → 5 + 1000 = 1005");
    }
}
