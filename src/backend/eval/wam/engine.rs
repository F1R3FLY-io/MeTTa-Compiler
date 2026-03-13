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
use super::compiler::{WamCode, RhsInfo};
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
}

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
        for result in &self.match_results {
            out.push(result.rhs_info.template);
            if let Some(rhs_type) = result.rhs_info.rhs_type {
                out.push(rhs_type);
            }
            for (_, v) in result.bindings.iter() {
                out.push(*v);
            }
        }
        // Constants referenced by LoadConst during execution
        for c in &self.code.constants {
            out.push(*c);
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
    let mut state = WamState::new(code.clone(), value);

    // Execute the instruction loop
    execute_wam(&mut state);

    // Convert match results to the format expected by dispatch_rule_matches.
    // Phase 3: Move bindings for the last multiplicity copy instead of cloning all.
    // For the common case (multiplicity = 1), this eliminates the clone entirely.
    let mut results = Vec::with_capacity(state.match_results.len());
    for match_result in state.match_results {
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

/// The WAM instruction execution loop.
///
/// Dispatches instructions sequentially. On failure, backtracks to the most
/// recent choice point. Terminates when all alternatives have been explored.
fn execute_wam(state: &mut WamState) {
    loop {
        // Bounds check: if IP is past the end, we're done
        if state.ip >= state.code.instructions.len() {
            break;
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
                use crate::backend::models::{MettaValueFactory, gc_allocator::global_factory};
                let factory = global_factory();
                let items: Vec<MettaValue> = (0..count)
                    .map(|i| state.registers.get(start_reg + i))
                    .collect();
                let sexpr = factory.sexpr(items);
                state.registers.set(target_reg, sexpr);
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

/// Handle a match failure: backtrack to the most recent choice point.
///
/// 1. Unwind the trail to restore bindings
/// 2. Reset registers (reload input into A0)
/// 3. Jump to the next alternative's instruction offset
/// 4. If no choice points remain, execution terminates
fn wam_fail(state: &mut WamState) {
    loop {
        match state.choice_points.last() {
            None => {
                // No more choice points — all alternatives exhausted
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
}
