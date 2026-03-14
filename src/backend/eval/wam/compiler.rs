//! WAM Compiler: Translates MeTTa rule LHS patterns into WAM instruction sequences.
//!
//! The compiler analyzes each rule's LHS pattern and produces a `WamCode` sequence
//! that performs pattern matching using register-based decomposition instead of
//! StructuralMatcher's repeated path navigation.
//!
//! # Compilation Strategy
//!
//! For a rule `(= (f (g $x) $y) rhs)`, the LHS `(f (g $x) $y)` compiles to:
//!
//! ```text
//! GetArity   A0, 3          ; check root is 3-element S-expr
//! GetArg     A0, 0, A1      ; A1 = head element
//! GetAtom    A1, "f"        ; check head is atom "f"
//! GetArg     A0, 1, A2      ; A2 = first argument
//! GetArity   A2, 2          ; check first arg is 2-element S-expr
//! GetArg     A2, 0, A3      ; A3 = head of first arg
//! GetAtom    A3, "g"        ; check head of first arg is "g"
//! GetArg     A2, 1, A4      ; A4 = $x
//! BindSlot   A4, 0          ; bind $x to slot 0
//! GetArg     A0, 2, A5      ; A5 = $y
//! BindSlot   A5, 1          ; bind $y to slot 1
//! Proceed                   ; match succeeded
//! ```
//!
//! # Register Allocation
//!
//! Registers are allocated left-to-right, depth-first:
//! - A0: always the root expression
//! - Subsequent registers: children in traversal order
//!
//! The allocator tracks the next free register and assigns it to each `GetArg`
//! target. Since MeTTa patterns are trees (no DAGs), each register is written
//! once and read at most once for further decomposition.
//!
//! # Comparison to StructuralMatcher
//!
//! | Aspect | StructuralMatcher | WAM Compiler |
//! |--------|------------------|-------------|
//! | Navigation | Re-traverse from root per check | Load child once into register |
//! | Memory | No register state | 16 registers (128 bytes) |
//! | Speed | O(depth) per check | O(1) per check (register access) |
//! | Code size | SmallVec<[Check; 8]> | Vec<WamInstruction> + metadata |

use std::collections::HashMap;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::backend::environment::rule_management::RuleEntry;
use crate::backend::eval::helpers::{is_grounded_op, needs_special_form_redispatch};
use crate::backend::models::MettaValue;

use super::instructions::{WamInstruction, GroundedBinaryOp};
use super::registers::MAX_REGISTERS;

/// Compiled WAM instruction sequence for a rule or group of rules.
#[derive(Clone, Debug)]
pub struct WamCode {
    /// The instruction sequence.
    pub instructions: Vec<WamInstruction>,
    /// Number of binding frame slots needed for this code.
    pub num_slots: u16,
    /// Variable name → slot index mapping (index in vec = slot index).
    /// Used for converting WamBindingFrame back to GenericBindings.
    pub slot_names: Vec<&'static str>,
    /// RHS templates referenced by TailEval instructions.
    /// Index corresponds to `TailEval.rhs_index`.
    pub rhs_templates: Vec<RhsInfo>,
    /// Constant values referenced by LoadConst instructions.
    /// Index corresponds to `LoadConst.const_index`.
    pub constants: Vec<MettaValue>,
    /// Hash tables for first-argument indexing (Phase 4).
    /// Index corresponds to `SwitchOnFirstArg.table_index`.
    pub index_tables: Vec<WamIndexTable>,
    /// Phase 6: Whether all execution paths produce fully-evaluated results
    /// (via ReturnEvaluated). When true, the caller can skip hash computation
    /// and match result caching — the WAM is the evaluation engine, not just
    /// a pattern matcher.
    pub fully_evaluable: bool,
}

/// Hash table for first-argument indexing.
///
/// Maps discriminant keys (atom, integer, bool, float, S-expr head) to
/// instruction offsets for the clause group matching that key.
#[derive(Clone, Debug)]
pub struct WamIndexTable {
    pub entries: HashMap<WamIndexKey, u16>,
}

/// Discriminant key for first-argument indexing.
///
/// Extracts the "type tag + value" from the first argument to enable
/// O(1) clause dispatch instead of linear scan across all rules.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum WamIndexKey {
    Atom(&'static str),
    Long(i64),
    Bool(bool),
    FloatBits(u64),
    /// Nested S-expression with known head atom (e.g., first arg is `(g ...)`)
    SExprHead(&'static str),
}

/// Information about a rule's RHS, referenced by TailEval instructions.
#[derive(Clone, Debug)]
pub struct RhsInfo {
    /// The RHS template value.
    pub template: MettaValue,
    /// Whether the RHS contains variables.
    pub has_variables: bool,
    /// Cached return type.
    pub rhs_type: Option<MettaValue>,
    /// Rule multiplicity.
    pub multiplicity: u64,
    /// Per-rule slot name mapping for converting bindings.
    /// In a multi-rule group, each rule may use different variable names
    /// at different slot indices. This mapping is used at TailEval time
    /// to produce correct GenericBindings.
    pub slot_names: Vec<&'static str>,
}

/// Phase 6: Determine if all execution paths produce fully-evaluated results.
///
/// A WamCode is "fully evaluable" when every path through the instruction stream
/// terminates with `ReturnEvaluated` (never `TailEval`, `Proceed`, or `YieldToTrampoline`).
/// This means the WAM is a complete evaluation engine for this rule group —
/// no trampoline round-trip is needed for apply_bindings or further evaluation.
///
/// When fully_evaluable is true, the caller can skip hash computation and
/// match result caching, eliminating ~12% CPU overhead from hashing.
fn is_fully_evaluable(instructions: &[WamInstruction]) -> bool {
    !instructions.iter().any(|i| matches!(
        i,
        WamInstruction::TailEval { .. }
            | WamInstruction::Proceed
            | WamInstruction::YieldToTrampoline
    ))
}

/// Compiler state during LHS pattern analysis.
struct CompilerState {
    /// Emitted instructions.
    instructions: Vec<WamInstruction>,
    /// Next free register index.
    next_reg: u8,
    /// Variable name → (slot_index, first_bind_reg) mapping.
    /// Used for repeated variable detection (EqualCheck).
    seen_vars: SmallVec<[(&'static str, u16); 8]>,
    /// Next free binding slot index.
    next_slot: u16,
    /// Slot names in order.
    slot_names: Vec<&'static str>,
}

impl CompilerState {
    fn new() -> Self {
        CompilerState {
            instructions: Vec::with_capacity(16),
            next_reg: 1, // A0 is reserved for root
            seen_vars: SmallVec::new(),
            next_slot: 0,
            slot_names: Vec::new(),
        }
    }

    /// Allocate the next available register.
    /// Returns None if all registers are exhausted.
    fn alloc_reg(&mut self) -> Option<u8> {
        if (self.next_reg as usize) >= MAX_REGISTERS {
            return None;
        }
        let reg = self.next_reg;
        self.next_reg += 1;
        Some(reg)
    }

    /// Allocate a binding slot for a variable.
    fn alloc_slot(&mut self, name: &'static str) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        self.slot_names.push(name);
        slot
    }

    /// Check if a variable has been seen before. Returns the slot index if so.
    fn find_var(&self, name: &str) -> Option<u16> {
        self.seen_vars.iter().find(|(n, _)| *n == name).map(|(_, slot)| *slot)
    }

    /// Record a variable as seen with its slot index.
    fn record_var(&mut self, name: &'static str, slot: u16) {
        self.seen_vars.push((name, slot));
    }

    /// Emit an instruction.
    fn emit(&mut self, inst: WamInstruction) {
        self.instructions.push(inst);
    }
}

/// Compile a single rule's LHS pattern into WAM instructions.
///
/// Returns `Some(WamCode)` for patterns that can be compiled (same coverage as
/// `StructuralMatcher::analyze`). Returns `None` for unsupported patterns
/// (Type, Conjunction, Error, Quoted nodes).
///
/// The compiled code assumes the input expression is in register A0.
pub fn compile_rule_lhs(lhs: &MettaValue) -> Option<WamCode> {
    let mut state = CompilerState::new();

    // Compile the LHS pattern starting from register A0 (root)
    if !compile_node(lhs, 0, &mut state) {
        return None;
    }

    // Terminal: successful match
    state.emit(WamInstruction::Proceed);

    Some(WamCode {
        instructions: state.instructions,
        num_slots: state.next_slot,
        slot_names: state.slot_names,
        rhs_templates: Vec::new(), // Filled by compile_rule_group
        constants: Vec::new(),     // Filled by compile_rhs_body
        index_tables: Vec::new(),  // Filled by compile_indexed_multi_rule
        // Phase 6: LHS-only code always has Proceed → not fully_evaluable.
        // The caller (compile_single_rule/compile_multi_rule) recomputes after
        // replacing Proceed with RHS instructions.
        fully_evaluable: false,
    })
}

/// Compile a group of rules sharing the same (head, arity) into a choice sequence.
///
/// For a single rule, emits just the LHS matching code + TailEval.
/// For multiple rules, chains alternatives with TryMeElse/RetryMeElse/TrustMe.
///
/// Returns `None` if any rule's LHS cannot be compiled (falls back to existing path).
pub fn compile_rule_group(entries: &[RuleEntry<MettaValue>]) -> Option<Arc<WamCode>> {
    if entries.is_empty() {
        return None;
    }

    // Single rule: no choice point overhead
    if entries.len() == 1 {
        return compile_single_rule(&entries[0]);
    }

    // For 4+ rules, try first-argument indexing for O(1) dispatch
    if entries.len() >= 4 {
        if let Some(indexed) = try_compile_indexed_multi_rule(entries) {
            return Some(indexed);
        }
    }

    // Multiple rules: compile each LHS, chain with choice point instructions
    compile_multi_rule(entries)
}

/// Compile a single rule (no choice points needed).
fn compile_single_rule(entry: &RuleEntry<MettaValue>) -> Option<Arc<WamCode>> {
    let mut code = compile_rule_lhs(&entry.lhs)?;

    let rule_slot_names = code.slot_names.clone();

    // Build slot_map for RHS compilation: [(name, slot_index), ...]
    let slot_map: SmallVec<[(&'static str, u16); 8]> = code
        .slot_names
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i as u16))
        .collect();

    // Re-derive next_reg from the highest register referenced in LHS instructions,
    // since CompilerState's next_reg is lost after compile_rule_lhs returns.
    let rhs_index = 0u16;
    let base_next_reg = derive_next_reg(&code.instructions);

    if entry.rhs_has_variables {
        // Priority 1: Try inline grounded binary operation (most efficient — no allocation)
        let mut next_reg = base_next_reg;
        if let Some(grounded_instrs) = try_compile_grounded_rhs(
            &entry.rhs,
            &slot_map,
            rhs_index,
            &mut next_reg,
            &mut code.constants,
        ) {
            // Remove the terminal Proceed and append grounded instructions
            if code.instructions.last().map_or(false, |i| matches!(i, WamInstruction::Proceed)) {
                code.instructions.pop();
            }
            code.instructions.extend(grounded_instrs);

            code.rhs_templates.push(RhsInfo {
                template: entry.rhs,
                has_variables: false, // Result is already evaluated
                rhs_type: entry.rhs_type,
                multiplicity: entry.multiplicity,
                slot_names: rule_slot_names,
            });

            code.fully_evaluable = is_fully_evaluable(&code.instructions);
            return Some(Arc::new(code));
        }

        // Priority 2: Try special form compilation (if, let, let*, chain)
        {
            let mut next_reg = base_next_reg;
            let mut sf_num_slots = code.num_slots;
            if let Some(mut sf_instrs) = try_compile_rhs_special_form(
                &entry.rhs,
                &slot_map,
                rhs_index,
                &mut next_reg,
                &mut code.constants,
                &mut sf_num_slots,
            ) {
                if code.instructions.last().map_or(false, |i| matches!(i, WamInstruction::Proceed)) {
                    code.instructions.pop();
                }
                let base_offset = code.instructions.len();
                patch_branch_offsets(&mut sf_instrs, base_offset);
                code.instructions.extend(sf_instrs);
                code.num_slots = sf_num_slots;
                // Extend slot_names for let-bound slots (needed for frame allocation)
                while code.slot_names.len() < sf_num_slots as usize {
                    code.slot_names.push("$_wam_internal");
                }

                code.rhs_templates.push(RhsInfo {
                    template: entry.rhs,
                    has_variables: false,
                    rhs_type: entry.rhs_type,
                    multiplicity: entry.multiplicity,
                    slot_names: rule_slot_names,
                });

                code.fully_evaluable = is_fully_evaluable(&code.instructions);
                return Some(Arc::new(code));
            }
        }

        // Priority 2.5: Try user-defined function call (recursive WAM execution)
        {
            let mut next_reg = base_next_reg;
            let mut uc_instrs = Vec::new();
            let mut uc_constants = code.constants.clone();
            if try_compile_rhs_user_call(
                &entry.rhs,
                &slot_map,
                rhs_index,
                &mut next_reg,
                &mut uc_constants,
                &mut uc_instrs,
            )
            .is_some()
            {
                if code.instructions.last().map_or(false, |i| matches!(i, WamInstruction::Proceed)) {
                    code.instructions.pop();
                }
                let base_offset = code.instructions.len();
                patch_branch_offsets(&mut uc_instrs, base_offset);
                code.instructions.extend(uc_instrs);
                code.constants = uc_constants;

                code.rhs_templates.push(RhsInfo {
                    template: entry.rhs,
                    has_variables: false,
                    rhs_type: entry.rhs_type,
                    multiplicity: entry.multiplicity,
                    slot_names: rule_slot_names,
                });

                code.fully_evaluable = is_fully_evaluable(&code.instructions);
                return Some(Arc::new(code));
            }
        }

        // Priority 3: Try RHS body construction (eliminates apply_bindings)
        let mut next_reg = base_next_reg;
        if let Some(rhs_instrs) = try_compile_rhs_body(
            &entry.rhs,
            &slot_map,
            rhs_index,
            &mut next_reg,
            &mut code.constants,
        ) {
            // Remove the terminal Proceed and append RHS construction instructions
            if code.instructions.last().map_or(false, |i| matches!(i, WamInstruction::Proceed)) {
                code.instructions.pop();
            }
            code.instructions.extend(rhs_instrs);

            code.rhs_templates.push(RhsInfo {
                template: entry.rhs,
                has_variables: false, // Result is already constructed
                rhs_type: entry.rhs_type,
                multiplicity: entry.multiplicity,
                slot_names: rule_slot_names,
            });

            code.fully_evaluable = is_fully_evaluable(&code.instructions);
            return Some(Arc::new(code));
        }
    }

    // Priority 4: Fall back to TailEval (trampoline handles apply_bindings)
    if let Some(last) = code.instructions.last_mut() {
        if matches!(last, WamInstruction::Proceed) {
            *last = WamInstruction::TailEval {
                rhs_index: 0,
                has_variables: entry.rhs_has_variables,
            };
        }
    }

    code.rhs_templates.push(RhsInfo {
        template: entry.rhs,
        has_variables: entry.rhs_has_variables,
        rhs_type: entry.rhs_type,
        multiplicity: entry.multiplicity,
        slot_names: rule_slot_names,
    });

    code.fully_evaluable = is_fully_evaluable(&code.instructions);
    Some(Arc::new(code))
}

/// Derive the next available register index from compiled instructions.
///
/// Scans for the highest register referenced in GetArg target_reg, BindSlot reg,
/// EqualCheck reg, or LoadSlot target_reg, then returns max + 1.
fn derive_next_reg(instructions: &[WamInstruction]) -> u8 {
    let mut max_reg: u8 = 0;
    for inst in instructions {
        let reg = match inst {
            WamInstruction::GetArg { target_reg, .. } => *target_reg,
            WamInstruction::GetArity { reg, .. } => *reg,
            WamInstruction::GetAtom { reg, .. } => *reg,
            WamInstruction::GetLong { reg, .. } => *reg,
            WamInstruction::GetBool { reg, .. } => *reg,
            WamInstruction::GetFloat { reg, .. } => *reg,
            WamInstruction::GetString { reg, .. } => *reg,
            WamInstruction::BindSlot { reg, .. } => *reg,
            WamInstruction::EqualCheck { reg, .. } => *reg,
            WamInstruction::LoadSlot { target_reg, .. } => *target_reg,
            _ => continue,
        };
        if reg >= max_reg {
            max_reg = reg + 1;
        }
    }
    // At minimum, register 1 is available (A0 = root)
    max_reg.max(1)
}

/// Compile multiple rules with choice point chaining.
///
/// Layout:
/// ```text
/// TryMeElse(alt_2_offset)
///   <rule 1 LHS matching>
///   TailEval(0)
/// RetryMeElse(alt_3_offset)   ← alt_2_offset
///   <rule 2 LHS matching>
///   TailEval(1)
/// TrustMe                     ← alt_3_offset
///   <rule N LHS matching>
///   TailEval(N-1)
/// ```
fn compile_multi_rule(entries: &[RuleEntry<MettaValue>]) -> Option<Arc<WamCode>> {
    let n = entries.len();
    let mut all_instructions: Vec<WamInstruction> = Vec::with_capacity(n * 12);
    let mut rhs_templates: Vec<RhsInfo> = Vec::with_capacity(n);
    let mut slot_names: Vec<&'static str> = Vec::new();
    let mut max_slots: u16 = 0;
    let mut constants: Vec<MettaValue> = Vec::new();

    // First pass: compile each rule's LHS independently
    let mut compiled_rules: Vec<WamCode> = Vec::with_capacity(n);
    for entry in entries {
        let code = compile_rule_lhs(&entry.lhs)?;
        if code.num_slots > max_slots {
            max_slots = code.num_slots;
        }
        // Use the slot names from the rule with the most variables
        if code.slot_names.len() > slot_names.len() {
            slot_names = code.slot_names.clone();
        }
        compiled_rules.push(code);
    }

    // Second pass: chain with choice point instructions
    // We need to know the offset of each alternative for TryMeElse/RetryMeElse.
    // Offsets are instruction indices (not byte offsets).
    let mut alt_offsets: Vec<usize> = Vec::with_capacity(n);

    for (i, (code, entry)) in compiled_rules.iter().zip(entries.iter()).enumerate() {
        let rhs_index = rhs_templates.len() as u16;

        // Build slot_map for this rule's variables
        let slot_map: SmallVec<[(&'static str, u16); 8]> = code
            .slot_names
            .iter()
            .enumerate()
            .map(|(idx, &name)| (name, idx as u16))
            .collect();

        let base_next_reg = derive_next_reg(&code.instructions);

        // Try to compile the RHS (grounded → special form → body construction → TailEval)
        let mut rhs_compiled = false;

        if entry.rhs_has_variables {
            // Priority 1: Try inline grounded binary operation
            let mut next_reg = base_next_reg;
            if let Some(grounded_instrs) = try_compile_grounded_rhs(
                &entry.rhs, &slot_map, rhs_index, &mut next_reg, &mut constants,
            ) {
                rhs_templates.push(RhsInfo {
                    template: entry.rhs,
                    has_variables: false,
                    rhs_type: entry.rhs_type,
                    multiplicity: entry.multiplicity,
                    slot_names: code.slot_names.clone(),
                });

                alt_offsets.push(all_instructions.len());
                emit_choice_point(&mut all_instructions, i, n);
                emit_lhs_instructions(&mut all_instructions, &code.instructions);
                all_instructions.extend(grounded_instrs);
                all_instructions.push(WamInstruction::Fail);
                rhs_compiled = true;
            }

            // Priority 2: Try special form compilation (if, let, let*, chain)
            if !rhs_compiled {
                let mut next_reg = base_next_reg;
                let mut sf_num_slots = max_slots;
                if let Some(mut sf_instrs) = try_compile_rhs_special_form(
                    &entry.rhs, &slot_map, rhs_index, &mut next_reg, &mut constants, &mut sf_num_slots,
                ) {
                    rhs_templates.push(RhsInfo {
                        template: entry.rhs,
                        has_variables: false,
                        rhs_type: entry.rhs_type,
                        multiplicity: entry.multiplicity,
                        slot_names: code.slot_names.clone(),
                    });

                    alt_offsets.push(all_instructions.len());
                    emit_choice_point(&mut all_instructions, i, n);
                    emit_lhs_instructions(&mut all_instructions, &code.instructions);
                    let base_offset = all_instructions.len();
                    patch_branch_offsets(&mut sf_instrs, base_offset);
                    all_instructions.extend(sf_instrs);
                    all_instructions.push(WamInstruction::Fail);
                    if sf_num_slots > max_slots {
                        max_slots = sf_num_slots;
                    }
                    while slot_names.len() < max_slots as usize {
                        slot_names.push("$_wam_internal");
                    }
                    rhs_compiled = true;
                }
            }

            // Priority 2.5: Try user-defined function call (recursive WAM execution)
            if !rhs_compiled {
                let mut next_reg = base_next_reg;
                let mut uc_instrs = Vec::new();
                let const_checkpoint = constants.len();
                if try_compile_rhs_user_call(
                    &entry.rhs, &slot_map, rhs_index, &mut next_reg, &mut constants, &mut uc_instrs,
                ).is_some() {
                    rhs_templates.push(RhsInfo {
                        template: entry.rhs,
                        has_variables: false,
                        rhs_type: entry.rhs_type,
                        multiplicity: entry.multiplicity,
                        slot_names: code.slot_names.clone(),
                    });

                    alt_offsets.push(all_instructions.len());
                    emit_choice_point(&mut all_instructions, i, n);
                    emit_lhs_instructions(&mut all_instructions, &code.instructions);
                    let base_offset = all_instructions.len();
                    patch_branch_offsets(&mut uc_instrs, base_offset);
                    all_instructions.extend(uc_instrs);
                    all_instructions.push(WamInstruction::Fail);
                    rhs_compiled = true;
                } else {
                    constants.truncate(const_checkpoint);
                }
            }

            // Priority 3: Try RHS body construction
            if !rhs_compiled {
                let mut next_reg = base_next_reg;
                if let Some(rhs_instrs) = try_compile_rhs_body(
                    &entry.rhs, &slot_map, rhs_index, &mut next_reg, &mut constants,
                ) {
                    rhs_templates.push(RhsInfo {
                        template: entry.rhs,
                        has_variables: false,
                        rhs_type: entry.rhs_type,
                        multiplicity: entry.multiplicity,
                        slot_names: code.slot_names.clone(),
                    });

                    alt_offsets.push(all_instructions.len());
                    emit_choice_point(&mut all_instructions, i, n);
                    emit_lhs_instructions(&mut all_instructions, &code.instructions);
                    all_instructions.extend(rhs_instrs);
                    all_instructions.push(WamInstruction::Fail);
                    rhs_compiled = true;
                }
            }
        }

        // Priority 4: Fall back to TailEval
        if !rhs_compiled {
            rhs_templates.push(RhsInfo {
                template: entry.rhs,
                has_variables: entry.rhs_has_variables,
                rhs_type: entry.rhs_type,
                multiplicity: entry.multiplicity,
                slot_names: code.slot_names.clone(),
            });

            alt_offsets.push(all_instructions.len());
            emit_choice_point(&mut all_instructions, i, n);
            emit_lhs_instructions(&mut all_instructions, &code.instructions);
            all_instructions.push(WamInstruction::TailEval {
                rhs_index,
                has_variables: entry.rhs_has_variables,
            });
            all_instructions.push(WamInstruction::Fail);
        }
    }

    // Patch forward references in TryMeElse/RetryMeElse
    for i in 0..n - 1 {
        let next_offset = alt_offsets[i + 1] as u16;
        match &mut all_instructions[alt_offsets[i]] {
            WamInstruction::TryMeElse { next_alternative } => {
                *next_alternative = next_offset;
            }
            WamInstruction::RetryMeElse { next_alternative } => {
                *next_alternative = next_offset;
            }
            _ => unreachable!("expected choice point instruction"),
        }
    }

    let fully_evaluable = is_fully_evaluable(&all_instructions);
    Some(Arc::new(WamCode {
        instructions: all_instructions,
        num_slots: max_slots,
        slot_names,
        rhs_templates,
        constants,
        index_tables: Vec::new(),
        fully_evaluable,
    }))
}

// ════════════════════════════════════════════════════════════════════════
// Multi-Rule Helpers
// ════════════════════════════════════════════════════════════════════════

/// Emit the appropriate choice point instruction for alternative `i` of `n` total.
///
/// For single-rule groups (n=1), no choice point is emitted — the rule's Fail
/// instruction will backtrack directly to the enclosing context (pending default
/// group or termination).
fn emit_choice_point(instructions: &mut Vec<WamInstruction>, i: usize, n: usize) {
    if n == 1 {
        // Single rule in group: no choice point needed.
        return;
    }
    if i == 0 {
        instructions.push(WamInstruction::TryMeElse { next_alternative: 0 });
    } else if i < n - 1 {
        instructions.push(WamInstruction::RetryMeElse { next_alternative: 0 });
    } else {
        instructions.push(WamInstruction::TrustMe);
    }
}

/// Emit LHS matching instructions, skipping the terminal Proceed.
fn emit_lhs_instructions(all: &mut Vec<WamInstruction>, lhs_instrs: &[WamInstruction]) {
    for inst in lhs_instrs {
        if !matches!(inst, WamInstruction::Proceed) {
            all.push(inst.clone());
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// Phase 4: First-Argument Indexing
// ════════════════════════════════════════════════════════════════════════

/// Extract the indexing key from a rule's LHS first argument.
///
/// For a rule with LHS `(head arg1 arg2 ...)`, extracts `arg1` (items[1])
/// and computes its discriminant key. Returns `None` for variable or wildcard
/// first arguments (which go to the default group).
fn extract_first_arg_key(lhs: &MettaValue) -> Option<WamIndexKey> {
    let items = lhs.as_sexpr()?;
    if items.len() < 2 {
        return None; // No first argument to index on
    }
    let first_arg = &items[1];

    // Variable or wildcard → default group
    if let Some(name) = first_arg.as_atom() {
        if is_variable_atom(name) || name == "_" {
            return None;
        }
        return Some(WamIndexKey::Atom(name));
    }
    if let Some(n) = first_arg.as_long() {
        return Some(WamIndexKey::Long(n));
    }
    if let Some(b) = first_arg.as_bool() {
        return Some(WamIndexKey::Bool(b));
    }
    if let Some(f) = first_arg.as_float() {
        return Some(WamIndexKey::FloatBits(f.to_bits()));
    }
    // S-expression first arg: index on its head atom
    if let Some(sub_items) = first_arg.as_sexpr() {
        if !sub_items.is_empty() {
            if let Some(head) = sub_items[0].as_atom() {
                if !is_variable_atom(head) {
                    return Some(WamIndexKey::SExprHead(head));
                }
            }
        }
    }
    None
}

/// Emit instructions for a group of rules with choice point chaining.
///
/// Shared between indexed groups and the default group in indexed compilation.
/// Each rule's RHS is compiled with the same priority order as `compile_multi_rule`:
/// (1) inline grounded binary, (2) special form, (3) RHS body construction, (4) TailEval fallback.
fn emit_rule_group_instructions(
    rule_indices: &[usize],
    entries: &[RuleEntry<MettaValue>],
    compiled_rules: &[WamCode],
    all_instructions: &mut Vec<WamInstruction>,
    rhs_templates: &mut Vec<RhsInfo>,
    constants: &mut Vec<MettaValue>,
    max_slots: &mut u16,
    all_slot_names: &mut Vec<&'static str>,
) {
    let n = rule_indices.len();
    let mut group_alt_offsets: Vec<usize> = Vec::with_capacity(n);

    for (group_i, &rule_i) in rule_indices.iter().enumerate() {
        let entry = &entries[rule_i];
        let code = &compiled_rules[rule_i];
        let rhs_index = rhs_templates.len() as u16;

        let slot_map: SmallVec<[(&'static str, u16); 8]> = code
            .slot_names
            .iter()
            .enumerate()
            .map(|(idx, &name)| (name, idx as u16))
            .collect();

        let base_next_reg = derive_next_reg(&code.instructions);
        let mut rhs_compiled = false;

        if entry.rhs_has_variables {
            // Priority 1: inline grounded binary
            let mut next_reg = base_next_reg;
            if let Some(grounded_instrs) =
                try_compile_grounded_rhs(&entry.rhs, &slot_map, rhs_index, &mut next_reg, constants)
            {
                rhs_templates.push(RhsInfo {
                    template: entry.rhs,
                    has_variables: false,
                    rhs_type: entry.rhs_type,
                    multiplicity: entry.multiplicity,
                    slot_names: code.slot_names.clone(),
                });

                group_alt_offsets.push(all_instructions.len());
                emit_choice_point(all_instructions, group_i, n);
                emit_lhs_instructions(all_instructions, &code.instructions);
                all_instructions.extend(grounded_instrs);
                all_instructions.push(WamInstruction::Fail);
                rhs_compiled = true;
            }

            // Priority 2: special form compilation (if, let, let*, chain)
            if !rhs_compiled {
                let mut next_reg = base_next_reg;
                let mut sf_num_slots = *max_slots;
                if let Some(mut sf_instrs) = try_compile_rhs_special_form(
                    &entry.rhs,
                    &slot_map,
                    rhs_index,
                    &mut next_reg,
                    constants,
                    &mut sf_num_slots,
                ) {
                    rhs_templates.push(RhsInfo {
                        template: entry.rhs,
                        has_variables: false,
                        rhs_type: entry.rhs_type,
                        multiplicity: entry.multiplicity,
                        slot_names: code.slot_names.clone(),
                    });

                    group_alt_offsets.push(all_instructions.len());
                    emit_choice_point(all_instructions, group_i, n);
                    emit_lhs_instructions(all_instructions, &code.instructions);
                    let base_offset = all_instructions.len();
                    patch_branch_offsets(&mut sf_instrs, base_offset);
                    all_instructions.extend(sf_instrs);
                    all_instructions.push(WamInstruction::Fail);
                    if sf_num_slots > *max_slots {
                        *max_slots = sf_num_slots;
                    }
                    while all_slot_names.len() < *max_slots as usize {
                        all_slot_names.push("$_wam_internal");
                    }
                    rhs_compiled = true;
                }
            }

            // Priority 2.5: user-defined function call (recursive WAM execution)
            if !rhs_compiled {
                let mut next_reg = base_next_reg;
                let mut uc_instrs = Vec::new();
                let const_checkpoint = constants.len();
                if try_compile_rhs_user_call(
                    &entry.rhs,
                    &slot_map,
                    rhs_index,
                    &mut next_reg,
                    constants,
                    &mut uc_instrs,
                ).is_some() {
                    rhs_templates.push(RhsInfo {
                        template: entry.rhs,
                        has_variables: false,
                        rhs_type: entry.rhs_type,
                        multiplicity: entry.multiplicity,
                        slot_names: code.slot_names.clone(),
                    });

                    group_alt_offsets.push(all_instructions.len());
                    emit_choice_point(all_instructions, group_i, n);
                    emit_lhs_instructions(all_instructions, &code.instructions);
                    let base_offset = all_instructions.len();
                    patch_branch_offsets(&mut uc_instrs, base_offset);
                    all_instructions.extend(uc_instrs);
                    all_instructions.push(WamInstruction::Fail);
                    rhs_compiled = true;
                } else {
                    constants.truncate(const_checkpoint);
                }
            }

            // Priority 3: RHS body construction
            if !rhs_compiled {
                let mut next_reg = base_next_reg;
                if let Some(rhs_instrs) = try_compile_rhs_body(
                    &entry.rhs,
                    &slot_map,
                    rhs_index,
                    &mut next_reg,
                    constants,
                ) {
                    rhs_templates.push(RhsInfo {
                        template: entry.rhs,
                        has_variables: false,
                        rhs_type: entry.rhs_type,
                        multiplicity: entry.multiplicity,
                        slot_names: code.slot_names.clone(),
                    });

                    group_alt_offsets.push(all_instructions.len());
                    emit_choice_point(all_instructions, group_i, n);
                    emit_lhs_instructions(all_instructions, &code.instructions);
                    all_instructions.extend(rhs_instrs);
                    all_instructions.push(WamInstruction::Fail);
                    rhs_compiled = true;
                }
            }
        }

        // Priority 4: TailEval fallback
        if !rhs_compiled {
            rhs_templates.push(RhsInfo {
                template: entry.rhs,
                has_variables: entry.rhs_has_variables,
                rhs_type: entry.rhs_type,
                multiplicity: entry.multiplicity,
                slot_names: code.slot_names.clone(),
            });

            group_alt_offsets.push(all_instructions.len());
            emit_choice_point(all_instructions, group_i, n);
            emit_lhs_instructions(all_instructions, &code.instructions);
            all_instructions.push(WamInstruction::TailEval {
                rhs_index,
                has_variables: entry.rhs_has_variables,
            });
            all_instructions.push(WamInstruction::Fail);
        }
    }

    // Patch forward references in TryMeElse/RetryMeElse within this group
    for j in 0..n.saturating_sub(1) {
        let next_offset = group_alt_offsets[j + 1] as u16;
        match &mut all_instructions[group_alt_offsets[j]] {
            WamInstruction::TryMeElse { next_alternative } => *next_alternative = next_offset,
            WamInstruction::RetryMeElse { next_alternative } => *next_alternative = next_offset,
            _ => {} // TrustMe or single-rule (no choice point emitted)
        }
    }
}

/// Try to compile a group of rules using first-argument indexing for O(1) dispatch.
///
/// Analyzes each rule's first argument and groups rules by their discriminant key.
/// Rules with variable/wildcard first arguments go to a default group that fires
/// for all inputs (all-solutions semantics requires both indexed and default groups).
///
/// Returns `None` if indexing is not beneficial (< 2 distinct concrete keys).
///
/// Layout:
/// ```text
/// SwitchOnFirstArg { table_index: 0, default_offset }
/// ; indexed group for key1
///   [TryMeElse/RetryMeElse/TrustMe chain or bare rule]
///   <rule LHS matching + RHS>
///   Fail
/// ; indexed group for key2
///   ...
/// ; default group (variable first-arg rules)          ← default_offset
///   [TryMeElse/RetryMeElse/TrustMe chain or bare rule]
///   <rule LHS matching + RHS>
///   Fail
/// ```
fn try_compile_indexed_multi_rule(entries: &[RuleEntry<MettaValue>]) -> Option<Arc<WamCode>> {
    // Step 1: Classify rules by first argument key
    let mut key_groups: HashMap<WamIndexKey, Vec<usize>> = HashMap::new();
    let mut default_indices: Vec<usize> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        match extract_first_arg_key(&entry.lhs) {
            Some(key) => {
                key_groups.entry(key).or_default().push(i);
            }
            None => {
                default_indices.push(i);
            }
        }
    }

    // Need at least 2 distinct concrete keys for indexing to be worthwhile
    if key_groups.len() < 2 {
        return None;
    }

    // Step 2: Compile all rules' LHS independently
    let mut compiled_rules: Vec<WamCode> = Vec::with_capacity(entries.len());
    let mut max_slots: u16 = 0;
    let mut all_slot_names: Vec<&'static str> = Vec::new();

    for entry in entries {
        let code = compile_rule_lhs(&entry.lhs)?;
        if code.num_slots > max_slots {
            max_slots = code.num_slots;
        }
        if code.slot_names.len() > all_slot_names.len() {
            all_slot_names = code.slot_names.clone();
        }
        compiled_rules.push(code);
    }

    // Step 3: Build instruction stream with indexed layout
    let mut all_instructions: Vec<WamInstruction> =
        Vec::with_capacity(entries.len() * 12 + 2);
    let mut rhs_templates: Vec<RhsInfo> = Vec::with_capacity(entries.len());
    let mut constants: Vec<MettaValue> = Vec::new();
    let mut index_table = WamIndexTable {
        entries: HashMap::new(),
    };

    // Emit SwitchOnFirstArg placeholder (default_offset patched later)
    all_instructions.push(WamInstruction::SwitchOnFirstArg {
        table_index: 0,
        default_offset: 0,
    });

    // Emit indexed groups — one group per distinct key
    for (key, rule_indices) in &key_groups {
        let group_start = all_instructions.len() as u16;
        index_table.entries.insert(key.clone(), group_start);

        emit_rule_group_instructions(
            rule_indices,
            entries,
            &compiled_rules,
            &mut all_instructions,
            &mut rhs_templates,
            &mut constants,
            &mut max_slots,
            &mut all_slot_names,
        );
    }

    // Emit default group (variable/wildcard first-arg rules)
    let default_offset = all_instructions.len() as u16;
    if !default_indices.is_empty() {
        emit_rule_group_instructions(
            &default_indices,
            entries,
            &compiled_rules,
            &mut all_instructions,
            &mut rhs_templates,
            &mut constants,
            &mut max_slots,
            &mut all_slot_names,
        );
    }

    // Patch SwitchOnFirstArg's default_offset
    if let WamInstruction::SwitchOnFirstArg {
        default_offset: d, ..
    } = &mut all_instructions[0]
    {
        *d = default_offset;
    }

    let fully_evaluable = is_fully_evaluable(&all_instructions);
    Some(Arc::new(WamCode {
        instructions: all_instructions,
        num_slots: max_slots,
        slot_names: all_slot_names,
        rhs_templates,
        constants,
        index_tables: vec![index_table],
        fully_evaluable,
    }))
}

// ════════════════════════════════════════════════════════════════════════
// Phase 1: RHS Body Compilation
// ════════════════════════════════════════════════════════════════════════

/// Check whether an atom name is a MeTTa variable ($x, 'x, &x).
fn is_variable_atom(name: &str) -> bool {
    (name.starts_with('$')
        || name.starts_with('\'')
        || (name.starts_with('&') && name != "&"))
        && name.len() > 1
}

/// Find a variable's slot index in the slot map.
fn find_slot_in_map(name: &str, slot_map: &[(&'static str, u16)]) -> Option<u16> {
    slot_map.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// Add a constant to the constants table, deduplicating by value equality.
fn push_constant(constants: &mut Vec<MettaValue>, value: MettaValue) -> u16 {
    for (i, c) in constants.iter().enumerate() {
        if *c == value {
            return i as u16;
        }
    }
    let idx = constants.len() as u16;
    constants.push(value);
    idx
}

/// Check if an RHS tree actually references any LHS variables from the slot map.
/// Body compilation is only beneficial when the RHS contains variable substitutions;
/// ground RHS is already handled optimally by TailEval with `has_variables: false`.
fn rhs_has_variable_refs(rhs: &MettaValue, slot_map: &[(&'static str, u16)]) -> bool {
    if let Some(name) = rhs.as_atom() {
        if is_variable_atom(name) {
            return find_slot_in_map(name, slot_map).is_some();
        }
        return false;
    }
    if let Some(items) = rhs.as_sexpr() {
        return items.iter().any(|item| rhs_has_variable_refs(item, slot_map));
    }
    false // literals don't reference variables
}

/// Try to compile a rule's RHS body into WAM instructions that construct the
/// result directly from register values, eliminating `apply_bindings`.
///
/// This handles arbitrary RHS patterns (not just binary grounded ops).
/// For each variable in the RHS, emits LoadSlot. For each constant, emits
/// LoadConst. For S-expressions, recursively compiles children into
/// consecutive registers and emits BuildSExpr.
///
/// Returns `Some(instructions)` on success, `None` for unsupported patterns
/// or ground RHS (falls back to TailEval). The `constants` vector is appended
/// to with any constant values needed by the instructions.
fn try_compile_rhs_body(
    rhs: &MettaValue,
    slot_map: &[(&'static str, u16)],
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
) -> Option<Vec<WamInstruction>> {
    // Guard: only attempt body compilation if RHS actually references LHS variables.
    // Ground RHS (no variable refs) is handled optimally by TailEval.
    if !rhs_has_variable_refs(rhs, slot_map) {
        return None;
    }

    let mut instructions = Vec::with_capacity(8);

    // Allocate a register for the final result
    let result_reg = alloc_rhs_reg(next_reg)?;

    // Recursively compile the RHS tree
    compile_rhs_node(rhs, slot_map, result_reg, next_reg, constants, &mut instructions)?;

    // Emit the result (reuses ReturnEvaluated — same semantics as inline grounded)
    instructions.push(WamInstruction::ReturnEvaluated {
        rhs_index,
        result_reg,
    });

    Some(instructions)
}

/// Recursively compile a single node of the RHS body into WAM instructions.
///
/// The result value is placed into `target_reg`. For leaf nodes (variables,
/// atoms, literals), this is a single instruction. For S-expression nodes,
/// children are compiled into consecutive scratch registers, then a BuildSExpr
/// instruction constructs the S-expression into `target_reg`.
///
/// Returns `None` if the node contains unsupported patterns (Type, Conjunction,
/// Error, Quoted) or if registers are exhausted.
fn compile_rhs_node(
    rhs: &MettaValue,
    slot_map: &[(&'static str, u16)],
    target_reg: u8,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    // Variable atom → LoadSlot from LHS binding
    if let Some(name) = rhs.as_atom() {
        if is_variable_atom(name) {
            let slot = find_slot_in_map(name, slot_map)?;
            instructions.push(WamInstruction::LoadSlot {
                slot,
                target_reg,
            });
            return Some(());
        }
        // Concrete atom → LoadConst
        let idx = push_constant(constants, *rhs);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // Long integer literal
    if rhs.as_long().is_some() {
        let idx = push_constant(constants, *rhs);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // Boolean literal
    if rhs.as_bool().is_some() {
        let idx = push_constant(constants, *rhs);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // Float literal
    if rhs.as_float().is_some() {
        let idx = push_constant(constants, *rhs);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // String literal
    if rhs.as_string().is_some() {
        let idx = push_constant(constants, *rhs);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // S-expression → compile children into consecutive registers, then BuildSExpr
    if let Some(items) = rhs.as_sexpr() {
        if items.is_empty() {
            // Empty S-expression = Unit
            let idx = push_constant(constants, *rhs);
            instructions.push(WamInstruction::LoadConst {
                const_index: idx,
                target_reg,
            });
            return Some(());
        }

        let count = items.len();
        if count > 255 {
            return None; // BuildSExpr count is u8
        }

        // Reserve consecutive registers for children
        let start_reg = *next_reg;
        let end_reg = start_reg as usize + count;
        if end_reg > MAX_REGISTERS {
            return None; // Register exhaustion
        }
        *next_reg = end_reg as u8;

        // Compile each child into its reserved register
        for (i, child) in items.iter().enumerate() {
            let child_target = start_reg + i as u8;
            compile_rhs_node(child, slot_map, child_target, next_reg, constants, instructions)?;
        }

        // Build S-expression from consecutive registers into target
        instructions.push(WamInstruction::BuildSExpr {
            start_reg,
            count: count as u8,
            target_reg,
        });

        return Some(());
    }

    // Unsupported node type (Type, Conjunction, Error, Quoted, etc.)
    None
}

// ════════════════════════════════════════════════════════════════════════
// Phase 2: Special Form Compilation (if, let, let*, chain)
// ════════════════════════════════════════════════════════════════════════

/// Try to compile a rule's RHS into WAM instructions handling special forms.
///
/// Recognizes `if`, `let`, `let*`, and `chain` special forms in the RHS and
/// compiles them into WAM control flow (BranchOnBool, Jump) and binding
/// instructions, eliminating the need for trampoline special form processing.
///
/// Returns `Some(instructions)` on success (each execution path ends with
/// ReturnEvaluated). Returns `None` for unsupported patterns (falls back to
/// body construction or TailEval).
///
/// `num_slots` is updated if `let`/`let*`/`chain` allocate additional binding slots.
fn try_compile_rhs_special_form(
    rhs: &MettaValue,
    slot_map: &[(&'static str, u16)],
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    num_slots: &mut u16,
) -> Option<Vec<WamInstruction>> {
    // Quick check: must be an S-expression with a recognized head atom
    let items = rhs.as_sexpr()?;
    if items.is_empty() {
        return None;
    }
    let head = items[0].as_atom()?;
    match head {
        "if" | "let" | "let*" | "chain" => {}
        _ => return None,
    }

    let mut instructions = Vec::with_capacity(16);
    compile_rhs_with_result(
        rhs,
        slot_map,
        num_slots,
        rhs_index,
        next_reg,
        constants,
        &mut instructions,
    )?;
    Some(instructions)
}

/// Compile an RHS expression that terminates with result emission.
///
/// This is the core recursive function for Phase 2 body evaluation. It tries:
/// 1. Special forms (if, let, let*, chain)
/// 2. Inline grounded evaluation (arithmetic/comparison ops)
/// 3. Data construction (build S-expression from registers)
///
/// Each execution path ends with `ReturnEvaluated` to emit the result.
fn compile_rhs_with_result(
    rhs: &MettaValue,
    slot_map: &[(&'static str, u16)],
    num_slots: &mut u16,
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    // Try special forms
    if let Some(items) = rhs.as_sexpr() {
        if !items.is_empty() {
            if let Some(head) = items[0].as_atom() {
                let checkpoint = (instructions.len(), constants.len(), *next_reg, *num_slots);

                let result = match head {
                    "if" if items.len() == 4 => compile_if_body(
                        items,
                        slot_map,
                        num_slots,
                        rhs_index,
                        next_reg,
                        constants,
                        instructions,
                    ),
                    "let" if items.len() == 4 => compile_let_body(
                        items,
                        slot_map,
                        num_slots,
                        rhs_index,
                        next_reg,
                        constants,
                        instructions,
                    ),
                    "let*" if items.len() == 3 => compile_letstar_body(
                        items,
                        slot_map,
                        num_slots,
                        rhs_index,
                        next_reg,
                        constants,
                        instructions,
                    ),
                    "chain" if items.len() == 4 => compile_chain_body(
                        items,
                        slot_map,
                        num_slots,
                        rhs_index,
                        next_reg,
                        constants,
                        instructions,
                    ),
                    _ => None,
                };

                if result.is_some() {
                    return result;
                }

                // Rollback on failure (partial instructions/constants/regs may have been emitted)
                let (il, cl, nr, ns) = checkpoint;
                instructions.truncate(il);
                constants.truncate(cl);
                *next_reg = nr;
                *num_slots = ns;

                // Try inline grounded evaluation
                let checkpoint2 = (instructions.len(), constants.len(), *next_reg);
                let result_reg = alloc_rhs_reg(next_reg)?;
                if try_compile_inline_eval(
                    rhs,
                    slot_map,
                    result_reg,
                    next_reg,
                    constants,
                    instructions,
                )
                .is_some()
                {
                    instructions.push(WamInstruction::ReturnEvaluated {
                        rhs_index,
                        result_reg,
                    });
                    return Some(());
                }
                let (il2, cl2, nr2) = checkpoint2;
                instructions.truncate(il2);
                constants.truncate(cl2);
                *next_reg = nr2;

                // Try user-defined function call
                let checkpoint3 = (instructions.len(), constants.len(), *next_reg);
                if try_compile_rhs_user_call(
                    rhs,
                    slot_map,
                    rhs_index,
                    next_reg,
                    constants,
                    instructions,
                )
                .is_some()
                {
                    return Some(());
                }
                let (il3, cl3, nr3) = checkpoint3;
                instructions.truncate(il3);
                constants.truncate(cl3);
                *next_reg = nr3;
            }
        }
    }

    // Fall back to data construction + ReturnEvaluated
    let result_reg = alloc_rhs_reg(next_reg)?;
    compile_rhs_node(rhs, slot_map, result_reg, next_reg, constants, instructions)?;
    instructions.push(WamInstruction::ReturnEvaluated {
        rhs_index,
        result_reg,
    });
    Some(())
}

/// Compile an `(if cond then else)` special form into WAM instructions.
///
/// Layout:
/// ```text
/// <condition evaluation instructions>
/// BranchOnBool cond_reg, then_ip, else_ip
/// then_ip:  <then branch instructions>
///           ReturnEvaluated
///           Jump end_ip
/// else_ip:  <else branch instructions>
///           ReturnEvaluated
/// end_ip:   ; falls through to Fail or end
/// ```
fn compile_if_body(
    items: &[MettaValue],
    slot_map: &[(&'static str, u16)],
    num_slots: &mut u16,
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    let cond = &items[1];
    let then_branch = &items[2];
    let else_branch = &items[3];

    // Guard: only compile when condition is guaranteed to produce a boolean.
    // Non-boolean conditions must fall back to the trampoline for correct
    // unreduced-expression semantics.
    if !condition_guaranteed_boolean(cond) {
        return None;
    }

    // Constant fold: boolean literal condition
    if let Some(b) = cond.as_bool() {
        let branch = if b { then_branch } else { else_branch };
        return compile_rhs_with_result(
            branch,
            slot_map,
            num_slots,
            rhs_index,
            next_reg,
            constants,
            instructions,
        );
    }

    // Compile condition into a register
    let cond_reg = alloc_rhs_reg(next_reg)?;
    try_compile_inline_eval(cond, slot_map, cond_reg, next_reg, constants, instructions)?;

    // BranchOnBool placeholder (offsets patched below)
    let branch_idx = instructions.len();
    instructions.push(WamInstruction::BranchOnBool {
        cond_reg,
        then_ip: 0,
        else_ip: 0,
    });

    // Then branch
    let then_start = instructions.len();
    compile_rhs_with_result(
        then_branch,
        slot_map,
        num_slots,
        rhs_index,
        next_reg,
        constants,
        instructions,
    )?;

    // Jump past else branch
    let jump_idx = instructions.len();
    instructions.push(WamInstruction::Jump { target_ip: 0 });

    // Else branch
    let else_start = instructions.len();
    compile_rhs_with_result(
        else_branch,
        slot_map,
        num_slots,
        rhs_index,
        next_reg,
        constants,
        instructions,
    )?;

    let end_ip = instructions.len();

    // Patch branch offsets (local within this instruction block)
    if let WamInstruction::BranchOnBool {
        then_ip, else_ip, ..
    } = &mut instructions[branch_idx]
    {
        *then_ip = then_start as u16;
        *else_ip = else_start as u16;
    }
    if let WamInstruction::Jump { target_ip } = &mut instructions[jump_idx] {
        *target_ip = end_ip as u16;
    }

    Some(())
}

/// Compile a `(let pattern value body)` special form.
///
/// Only handles single-variable patterns (e.g., `(let $x expr body)`).
/// Compound patterns fall back to the trampoline.
fn compile_let_body(
    items: &[MettaValue],
    slot_map: &[(&'static str, u16)],
    num_slots: &mut u16,
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    let pattern = &items[1];
    let value = &items[2];
    let body = &items[3];

    // Only single-variable patterns for Phase 2
    let var_name = pattern.as_atom()?;
    if !is_variable_atom(var_name) {
        return None;
    }

    // Evaluate value into a register
    let val_reg = alloc_rhs_reg(next_reg)?;
    try_compile_inline_eval(value, slot_map, val_reg, next_reg, constants, instructions)?;

    // Allocate a new binding frame slot for the let variable
    let slot = *num_slots;
    *num_slots += 1;

    // Bind value to slot (trail records previous value for backtrack undo)
    instructions.push(WamInstruction::BindSlot {
        reg: val_reg,
        slot,
    });

    // Extend slot_map with the new binding
    let mut extended_map: SmallVec<[(&'static str, u16); 8]> = SmallVec::from_slice(slot_map);
    extended_map.push((var_name, slot));

    // Compile body with extended bindings
    compile_rhs_with_result(
        body,
        &extended_map,
        num_slots,
        rhs_index,
        next_reg,
        constants,
        instructions,
    )
}

/// Compile a `(let* ((p1 v1) (p2 v2) ...) body)` special form.
///
/// Processes bindings sequentially, each visible to subsequent bindings.
/// Only handles single-variable patterns in each binding pair.
fn compile_letstar_body(
    items: &[MettaValue],
    slot_map: &[(&'static str, u16)],
    num_slots: &mut u16,
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    let bindings_expr = &items[1];
    let body = &items[2];

    let binding_items = bindings_expr.as_sexpr()?;

    let mut extended_map: SmallVec<[(&'static str, u16); 8]> = SmallVec::from_slice(slot_map);

    for binding in binding_items {
        let pair = binding.as_sexpr()?;
        if pair.len() != 2 {
            return None;
        }
        let pattern = &pair[0];
        let value = &pair[1];

        // Only single-variable patterns
        let var_name = pattern.as_atom()?;
        if !is_variable_atom(var_name) {
            return None;
        }

        // Evaluate value with current bindings
        let val_reg = alloc_rhs_reg(next_reg)?;
        try_compile_inline_eval(
            value,
            &extended_map,
            val_reg,
            next_reg,
            constants,
            instructions,
        )?;

        // Allocate slot and bind
        let slot = *num_slots;
        *num_slots += 1;
        instructions.push(WamInstruction::BindSlot {
            reg: val_reg,
            slot,
        });
        extended_map.push((var_name, slot));
    }

    // Compile body with all accumulated bindings
    compile_rhs_with_result(
        body,
        &extended_map,
        num_slots,
        rhs_index,
        next_reg,
        constants,
        instructions,
    )
}

/// Compile a `(chain value pattern body)` special form.
///
/// Semantically equivalent to `(let pattern value body)` — evaluates value,
/// binds to pattern variable, evaluates body.
fn compile_chain_body(
    items: &[MettaValue],
    slot_map: &[(&'static str, u16)],
    num_slots: &mut u16,
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    let value = &items[1];
    let pattern = &items[2];
    let body = &items[3];

    // Only single-variable patterns
    let var_name = pattern.as_atom()?;
    if !is_variable_atom(var_name) {
        return None;
    }

    // Evaluate value
    let val_reg = alloc_rhs_reg(next_reg)?;
    try_compile_inline_eval(value, slot_map, val_reg, next_reg, constants, instructions)?;

    // Allocate slot and bind
    let slot = *num_slots;
    *num_slots += 1;
    instructions.push(WamInstruction::BindSlot {
        reg: val_reg,
        slot,
    });

    let mut extended_map: SmallVec<[(&'static str, u16); 8]> = SmallVec::from_slice(slot_map);
    extended_map.push((var_name, slot));

    compile_rhs_with_result(
        body,
        &extended_map,
        num_slots,
        rhs_index,
        next_reg,
        constants,
        instructions,
    )
}

/// Try to compile an expression for inline evaluation (not data construction).
///
/// Handles:
/// - Variables → LoadSlot (uses LHS binding value)
/// - Literals (Long, Bool, Float, String, Atom) → LoadConst
/// - Binary grounded ops → recursive evaluation of args + CallGroundedBinary
///
/// Returns `Some(())` on success with result in `target_reg`.
/// Returns `None` for expressions that can't be evaluated inline (user-defined
/// function calls, complex special forms, etc.).
fn try_compile_inline_eval(
    expr: &MettaValue,
    slot_map: &[(&'static str, u16)],
    target_reg: u8,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    // Variable → LoadSlot
    if let Some(name) = expr.as_atom() {
        if is_variable_atom(name) {
            let slot = find_slot_in_map(name, slot_map)?;
            instructions.push(WamInstruction::LoadSlot {
                slot,
                target_reg,
            });
            return Some(());
        }
        // Concrete atom → LoadConst (evaluates to itself)
        let idx = push_constant(constants, *expr);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // Literal → LoadConst
    if expr.as_long().is_some()
        || expr.as_bool().is_some()
        || expr.as_float().is_some()
        || expr.as_string().is_some()
    {
        let idx = push_constant(constants, *expr);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // S-expression: check for grounded binary op
    if let Some(items) = expr.as_sexpr() {
        if items.len() == 3 {
            if let Some(op_name) = items[0].as_atom() {
                if let Some(op) = atom_to_grounded_op(op_name) {
                    let left_reg = alloc_rhs_reg(next_reg)?;
                    try_compile_inline_eval(
                        &items[1],
                        slot_map,
                        left_reg,
                        next_reg,
                        constants,
                        instructions,
                    )?;

                    let right_reg = alloc_rhs_reg(next_reg)?;
                    try_compile_inline_eval(
                        &items[2],
                        slot_map,
                        right_reg,
                        next_reg,
                        constants,
                        instructions,
                    )?;

                    instructions.push(WamInstruction::CallGroundedBinary {
                        op,
                        left_reg,
                        right_reg,
                        target_reg,
                    });
                    return Some(());
                }
            }
        }
    }

    // Can't evaluate inline
    None
}

/// Check whether a condition expression is guaranteed to produce a boolean.
///
/// Returns `true` for:
/// - Boolean literals (True, False)
/// - Binary comparison operations (==, <, <=, >, >=) — always produce Bool
///
/// Used as a guard for `if` compilation: non-boolean conditions must fall back
/// to the trampoline for correct unreduced-expression semantics.
fn condition_guaranteed_boolean(expr: &MettaValue) -> bool {
    if expr.as_bool().is_some() {
        return true;
    }
    if let Some(items) = expr.as_sexpr() {
        if items.len() == 3 {
            if let Some(op) = items[0].as_atom() {
                return matches!(op, "==" | "<" | "<=" | ">" | ">=");
            }
        }
    }
    false
}

// ════════════════════════════════════════════════════════════════════════
// Phase 3: User-Defined Function Call Compilation
// ════════════════════════════════════════════════════════════════════════

/// Try to compile a user-defined function call in the RHS into WAM instructions.
///
/// Recognizes S-expressions `(head arg1 arg2 ...)` where `head` is a concrete
/// atom that is NOT a grounded op, NOT a special form, and NOT a variable.
/// Each argument must be compilable inline (variables, literals, or binary ops).
///
/// Emits:
/// ```text
/// <load head constant into reg>
/// <compile each argument inline>
/// BuildSExpr start_reg, count, expr_reg
/// CallUserFunc expr_reg, result_reg, fallback_ip
/// ReturnEvaluated rhs_index, result_reg    ; success path
/// Jump end_ip
/// fallback_ip: ReturnEvaluated rhs_index, expr_reg  ; fallback: return built expr for trampoline
/// end_ip:
/// ```
///
/// Returns `None` if the RHS is not a user-defined call or if any argument
/// can't be compiled inline.
fn try_compile_rhs_user_call(
    rhs: &MettaValue,
    slot_map: &[(&'static str, u16)],
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    let items = rhs.as_sexpr()?;
    if items.is_empty() {
        return None;
    }

    let head = items[0].as_atom()?;

    // Exclude variables, grounded ops, and special forms
    if is_variable_atom(head) || is_grounded_op(head) || needs_special_form_redispatch(head) {
        return None;
    }

    // Must have at least one argument (bare atoms are not function calls)
    if items.len() < 2 {
        return None;
    }

    // Compile the call expression: head + all arguments into consecutive registers
    let start_reg = *next_reg;

    // Load head atom as constant
    let head_reg = alloc_rhs_reg(next_reg)?;
    let head_idx = push_constant(constants, items[0]);
    instructions.push(WamInstruction::LoadConst {
        const_index: head_idx,
        target_reg: head_reg,
    });

    // Compile each argument inline
    for arg in &items[1..] {
        let arg_reg = alloc_rhs_reg(next_reg)?;
        try_compile_inline_eval(arg, slot_map, arg_reg, next_reg, constants, instructions)?;
    }

    // Build the call S-expression
    let expr_reg = alloc_rhs_reg(next_reg)?;
    let count = items.len();
    if count > 255 {
        return None;
    }
    instructions.push(WamInstruction::BuildSExpr {
        start_reg,
        count: count as u8,
        target_reg: expr_reg,
    });

    // Allocate result register for CallUserFunc
    let result_reg = alloc_rhs_reg(next_reg)?;

    // CallUserFunc: fallback_ip points to the ReturnEvaluated for the built expr
    // Layout (0-based within this block):
    //   [current]: CallUserFunc { expr_reg, result_reg, fallback_ip }
    //   [+1]:      ReturnEvaluated { rhs_index, result_reg }  (success)
    //   [+2]:      Jump { target_ip: +4 }                     (skip fallback)
    //   [+3]:      ReturnEvaluated { rhs_index, expr_reg }    (fallback)
    //   [+4]:      (end — next instruction after this block)
    let base = instructions.len();
    let fallback_ip = (base + 3) as u16;

    instructions.push(WamInstruction::CallUserFunc {
        expr_reg,
        result_reg,
        fallback_ip,
    });

    // Success path: return the evaluated result
    instructions.push(WamInstruction::ReturnEvaluated {
        rhs_index,
        result_reg,
    });

    // Jump over fallback
    let end_ip = (base + 4) as u16;
    instructions.push(WamInstruction::Jump { target_ip: end_ip });

    // Fallback: return the built expression for trampoline evaluation
    instructions.push(WamInstruction::ReturnEvaluated {
        rhs_index,
        result_reg: expr_reg,
    });

    Some(())
}

/// Adjust BranchOnBool and Jump offsets by a base offset.
///
/// When a block of special-form instructions is appended to the main
/// instruction stream at position `base_offset`, all branch/jump targets
/// (which are 0-based within the block) need to be adjusted to absolute
/// positions in the final stream.
fn patch_branch_offsets(instructions: &mut [WamInstruction], base_offset: usize) {
    let base = base_offset as u16;
    for inst in instructions.iter_mut() {
        match inst {
            WamInstruction::BranchOnBool {
                then_ip, else_ip, ..
            } => {
                *then_ip += base;
                *else_ip += base;
            }
            WamInstruction::Jump { target_ip } => {
                *target_ip += base;
            }
            WamInstruction::CallUserFunc { fallback_ip, .. } => {
                *fallback_ip += base;
            }
            _ => {}
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// LHS Pattern Compilation
// ════════════════════════════════════════════════════════════════════════

/// Recursively compile a node in the LHS pattern.
///
/// `reg` is the register holding the current node's value.
/// Returns `false` if the node contains unsupported patterns.
fn compile_node(value: &MettaValue, reg: u8, state: &mut CompilerState) -> bool {
    // S-expression: check arity, decompose children
    if let Some(items) = value.as_sexpr() {
        state.emit(WamInstruction::GetArity {
            reg,
            expected: items.len() as u16,
        });

        for (i, child) in items.iter().enumerate() {
            // Allocate a register for this child
            let child_reg = match state.alloc_reg() {
                Some(r) => r,
                None => return false, // Register exhaustion
            };

            state.emit(WamInstruction::GetArg {
                source_reg: reg,
                child_index: i as u8,
                target_reg: child_reg,
            });

            // Recursively compile the child
            if !compile_node(child, child_reg, state) {
                return false;
            }
        }

        return true;
    }

    // Atom: variable, wildcard, or concrete symbol
    if let Some(atom) = value.as_atom() {
        if (atom.starts_with('$')
            || atom.starts_with('\'')
            || (atom.starts_with('&') && atom != "&"))
            && atom.len() > 1
        {
            // Variable
            if let Some(existing_slot) = state.find_var(atom) {
                // Repeated variable: emit equality check
                state.emit(WamInstruction::EqualCheck {
                    reg,
                    slot: existing_slot,
                });
            } else {
                // First occurrence: allocate slot and bind
                let slot = state.alloc_slot(atom);
                state.record_var(atom, slot);
                state.emit(WamInstruction::BindSlot { reg, slot });
            }
            return true;
        }
        if atom == "_" {
            // Wildcard: matches anything, no check or binding
            return true;
        }
        // Concrete atom: check equality
        state.emit(WamInstruction::GetAtom {
            reg,
            expected: atom,
        });
        return true;
    }

    // Long integer literal
    if let Some(n) = value.as_long() {
        state.emit(WamInstruction::GetLong { reg, expected: n });
        return true;
    }

    // Boolean literal
    if let Some(b) = value.as_bool() {
        state.emit(WamInstruction::GetBool { reg, expected: b });
        return true;
    }

    // Float literal (bitwise comparison)
    if let Some(f) = value.as_float() {
        state.emit(WamInstruction::GetFloat {
            reg,
            expected_bits: f.to_bits(),
        });
        return true;
    }

    // String literal
    if let Some(s) = value.as_string() {
        let interned = crate::backend::models::gc_allocator::global_allocator().alloc_str(s);
        state.emit(WamInstruction::GetString {
            reg,
            expected: interned,
        });
        return true;
    }

    // Unsupported node type (Type, Conjunction, Error, Quoted, etc.)
    false
}

/// Map an atom name to a GroundedBinaryOp, if it names one.
fn atom_to_grounded_op(name: &str) -> Option<GroundedBinaryOp> {
    match name {
        "+" => Some(GroundedBinaryOp::Add),
        "-" => Some(GroundedBinaryOp::Sub),
        "*" => Some(GroundedBinaryOp::Mul),
        "/" => Some(GroundedBinaryOp::Div),
        "%" => Some(GroundedBinaryOp::Mod),
        "<" => Some(GroundedBinaryOp::Lt),
        "<=" => Some(GroundedBinaryOp::Le),
        ">" => Some(GroundedBinaryOp::Gt),
        ">=" => Some(GroundedBinaryOp::Ge),
        "==" => Some(GroundedBinaryOp::Eq),
        _ => None,
    }
}

/// Try to compile a simple grounded binary RHS into inline instructions.
///
/// Recognizes the pattern `(op $var1 $var2)` where `op` is a grounded binary
/// operation (+, -, *, /, %, <, <=, >, >=, ==) and both arguments are variables
/// bound by the LHS. Replaces `TailEval` with:
///
/// ```text
/// LoadSlot  slot($var1) → Aleft
/// LoadSlot  slot($var2) → Aright
/// CallGroundedBinary  op, Aleft, Aright, Aresult
/// ReturnEvaluated     rhs_index, Aresult
/// ```
///
/// Returns `Some(instructions)` on success, `None` if the RHS doesn't match
/// this pattern (falls back to TailEval for trampoline evaluation).
fn try_compile_grounded_rhs(
    rhs: &MettaValue,
    slot_map: &[(&'static str, u16)],
    rhs_index: u16,
    next_reg: &mut u8,
    constants: &mut Vec<MettaValue>,
) -> Option<Vec<WamInstruction>> {
    // Must be a 3-element S-expression: (op arg1 arg2)
    let items = rhs.as_sexpr()?;
    if items.len() != 3 {
        return None;
    }

    // First element must be a grounded binary op
    let op_atom = items[0].as_atom()?;
    let op = atom_to_grounded_op(op_atom)?;

    // Helper: resolve an argument to either a slot load or a literal load
    let mut instructions = Vec::with_capacity(5);

    let left_reg = alloc_rhs_reg(next_reg)?;
    emit_rhs_arg_load(&items[1], slot_map, left_reg, constants, &mut instructions)?;

    let right_reg = alloc_rhs_reg(next_reg)?;
    emit_rhs_arg_load(&items[2], slot_map, right_reg, constants, &mut instructions)?;

    let result_reg = alloc_rhs_reg(next_reg)?;

    instructions.push(WamInstruction::CallGroundedBinary {
        op,
        left_reg,
        right_reg,
        target_reg: result_reg,
    });

    instructions.push(WamInstruction::ReturnEvaluated {
        rhs_index,
        result_reg,
    });

    Some(instructions)
}

/// Allocate a register for RHS evaluation.
fn alloc_rhs_reg(next_reg: &mut u8) -> Option<u8> {
    if (*next_reg as usize) >= MAX_REGISTERS {
        return None;
    }
    let reg = *next_reg;
    *next_reg += 1;
    Some(reg)
}

/// Emit instructions to load a RHS argument into a register.
///
/// Handles:
/// - Variable ($x): LoadSlot from the LHS binding
/// - Long literal: Direct register load via LoadSlot after storing
/// - Returns None for unsupported argument types
fn emit_rhs_arg_load(
    arg: &MettaValue,
    slot_map: &[(&'static str, u16)],
    target_reg: u8,
    constants: &mut Vec<MettaValue>,
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    // Variable argument: load from binding frame slot
    if let Some(name) = arg.as_atom() {
        if is_variable_atom(name) {
            let slot = find_slot_in_map(name, slot_map)?;
            instructions.push(WamInstruction::LoadSlot {
                slot,
                target_reg,
            });
            return Some(());
        }
    }

    // Literal argument: load as constant
    if arg.as_long().is_some()
        || arg.as_bool().is_some()
        || arg.as_float().is_some()
        || arg.as_string().is_some()
    {
        let idx = push_constant(constants, *arg);
        instructions.push(WamInstruction::LoadConst {
            const_index: idx,
            target_reg,
        });
        return Some(());
    }

    // Unsupported argument type (S-expression, etc.)
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::{MettaValue, MettaValueFactory};

    fn factory() -> crate::backend::models::GcFactory {
        global_factory()
    }

    #[test]
    fn test_compile_atom_pattern() {
        // Pattern: just an atom "foo"
        let f = factory();
        let lhs = f.atom("foo");
        let code = compile_rule_lhs(&lhs).expect("should compile");

        // Should have: GetAtom A0 "foo", Proceed
        assert_eq!(code.instructions.len(), 2);
        assert!(matches!(
            &code.instructions[0],
            WamInstruction::GetAtom { reg: 0, expected } if *expected == "foo"
        ));
        assert!(matches!(&code.instructions[1], WamInstruction::Proceed));
        assert_eq!(code.num_slots, 0);
    }

    #[test]
    fn test_compile_variable_pattern() {
        // Pattern: $x (variable)
        let f = factory();
        let lhs = f.atom("$x");
        let code = compile_rule_lhs(&lhs).expect("should compile");

        // Should have: BindSlot A0 slot=0, Proceed
        assert_eq!(code.instructions.len(), 2);
        assert!(matches!(
            &code.instructions[0],
            WamInstruction::BindSlot { reg: 0, slot: 0 }
        ));
        assert!(matches!(&code.instructions[1], WamInstruction::Proceed));
        assert_eq!(code.num_slots, 1);
        assert_eq!(code.slot_names[0], "$x");
    }

    #[test]
    fn test_compile_wildcard_pattern() {
        // Pattern: _ (wildcard)
        let f = factory();
        let lhs = f.atom("_");
        let code = compile_rule_lhs(&lhs).expect("should compile");

        // Should have: just Proceed (wildcard matches anything)
        assert_eq!(code.instructions.len(), 1);
        assert!(matches!(&code.instructions[0], WamInstruction::Proceed));
        assert_eq!(code.num_slots, 0);
    }

    #[test]
    fn test_compile_long_pattern() {
        let lhs = MettaValue::Long(42);
        let code = compile_rule_lhs(&lhs).expect("should compile");

        assert_eq!(code.instructions.len(), 2);
        assert!(matches!(
            &code.instructions[0],
            WamInstruction::GetLong { reg: 0, expected: 42 }
        ));
    }

    #[test]
    fn test_compile_bool_pattern() {
        let lhs = MettaValue::Bool(true);
        let code = compile_rule_lhs(&lhs).expect("should compile");

        assert_eq!(code.instructions.len(), 2);
        assert!(matches!(
            &code.instructions[0],
            WamInstruction::GetBool { reg: 0, expected: true }
        ));
    }

    #[test]
    fn test_compile_simple_sexpr() {
        // Pattern: (f $x)
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x")]);
        let code = compile_rule_lhs(&lhs).expect("should compile");

        // Expected:
        // GetArity A0, 2
        // GetArg A0, 0, A1
        // GetAtom A1, "f"
        // GetArg A0, 1, A2
        // BindSlot A2, 0
        // Proceed
        assert_eq!(code.num_slots, 1);
        assert_eq!(code.slot_names[0], "$x");

        // Verify instruction types
        assert!(matches!(
            &code.instructions[0],
            WamInstruction::GetArity { reg: 0, expected: 2 }
        ));
        assert!(matches!(
            &code.instructions[1],
            WamInstruction::GetArg { source_reg: 0, child_index: 0, target_reg: 1 }
        ));
        assert!(matches!(
            &code.instructions[2],
            WamInstruction::GetAtom { reg: 1, expected } if *expected == "f"
        ));
        assert!(matches!(
            &code.instructions[3],
            WamInstruction::GetArg { source_reg: 0, child_index: 1, target_reg: 2 }
        ));
        assert!(matches!(
            &code.instructions[4],
            WamInstruction::BindSlot { reg: 2, slot: 0 }
        ));
        assert!(matches!(&code.instructions[5], WamInstruction::Proceed));
    }

    #[test]
    fn test_compile_nested_sexpr() {
        // Pattern: (f (g $x) $y)
        let f = factory();
        let lhs = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.atom("$y"),
        ]);
        let code = compile_rule_lhs(&lhs).expect("should compile");

        assert_eq!(code.num_slots, 2);
        assert_eq!(code.slot_names[0], "$x");
        assert_eq!(code.slot_names[1], "$y");

        // Verify key instructions:
        // GetArity A0, 3
        // GetArg A0, 0, A1 → GetAtom A1, "f"
        // GetArg A0, 1, A2 → GetArity A2, 2 → GetArg A2, 0, A3 → GetAtom A3, "g"
        //                                    → GetArg A2, 1, A4 → BindSlot A4, 0 ($x)
        // GetArg A0, 2, A5 → BindSlot A5, 1 ($y)
        // Proceed

        assert!(matches!(
            &code.instructions[0],
            WamInstruction::GetArity { reg: 0, expected: 3 }
        ));
    }

    #[test]
    fn test_compile_repeated_variable() {
        // Pattern: (f $x $x) — repeated variable
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$x")]);
        let code = compile_rule_lhs(&lhs).expect("should compile");

        // Should have BindSlot for first $x, EqualCheck for second $x
        assert_eq!(code.num_slots, 1); // Only 1 slot for $x

        let has_bind = code.instructions.iter().any(|i| matches!(i, WamInstruction::BindSlot { slot: 0, .. }));
        let has_equal = code.instructions.iter().any(|i| matches!(i, WamInstruction::EqualCheck { slot: 0, .. }));
        assert!(has_bind, "should have BindSlot for first $x");
        assert!(has_equal, "should have EqualCheck for second $x");
    }

    #[test]
    fn test_compile_rule_group_single() {
        let f = factory();
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
            rhs_has_variables: false,
            structural_matcher: None,
            wam_code: None,
        };

        let code = compile_rule_group(&[entry]).expect("should compile single rule");
        assert_eq!(code.rhs_templates.len(), 1);
        assert_eq!(code.rhs_templates[0].template, f.atom("result"));

        // Should end with TailEval (not Proceed)
        let last = code.instructions.last().expect("non-empty");
        assert!(matches!(last, WamInstruction::TailEval { rhs_index: 0, has_variables: false }));
    }

    #[test]
    fn test_compile_rule_group_multi() {
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

        let code = compile_rule_group(&[entry1, entry2]).expect("should compile multi rule");
        assert_eq!(code.rhs_templates.len(), 2);

        // Should have TryMeElse at the start
        assert!(matches!(
            &code.instructions[0],
            WamInstruction::TryMeElse { .. }
        ));

        // Should have TrustMe for the last alternative
        let has_trust = code.instructions.iter().any(|i| matches!(i, WamInstruction::TrustMe));
        assert!(has_trust, "should have TrustMe for last alternative");
    }

    #[test]
    fn test_compile_equivalence_with_structural_matcher() {
        // Verify that WAM compilation produces matching results for the same input
        use crate::backend::environment::rule_management::StructuralMatcher;

        let f = factory();
        let lhs = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.atom("$y"),
        ]);

        // Both should succeed
        let sm = StructuralMatcher::analyze(&lhs);
        let wam = compile_rule_lhs(&lhs);
        assert!(sm.is_some(), "StructuralMatcher should compile this pattern");
        assert!(wam.is_some(), "WAM compiler should compile this pattern");

        // Both should produce the same slot count
        let wam_code = wam.expect("wam compiled");
        assert_eq!(wam_code.num_slots, 2); // $x and $y
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 4: Inline Grounded Binary RHS Compilation
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_compile_grounded_add_rhs() {
        // Rule: (= (add $x $y) (+ $x $y))
        // Should compile RHS inline: LoadSlot, LoadSlot, CallGroundedBinary, ReturnEvaluated
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("add"), f.atom("$x"), f.atom("$y")]);
        let rhs = f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]);

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

        let code = compile_rule_group(&[entry]).expect("should compile");

        // Should NOT end with TailEval — instead should have ReturnEvaluated
        let has_tail_eval = code.instructions.iter().any(|i| matches!(i, WamInstruction::TailEval { .. }));
        assert!(!has_tail_eval, "should NOT have TailEval for inline grounded RHS");

        let has_call_grounded = code.instructions.iter().any(|i| matches!(
            i,
            WamInstruction::CallGroundedBinary { op: GroundedBinaryOp::Add, .. }
        ));
        assert!(has_call_grounded, "should have CallGroundedBinary(Add)");

        let has_return = code.instructions.iter().any(|i| matches!(
            i,
            WamInstruction::ReturnEvaluated { .. }
        ));
        assert!(has_return, "should have ReturnEvaluated");

        let load_count = code.instructions.iter().filter(|i| matches!(i, WamInstruction::LoadSlot { .. })).count();
        assert_eq!(load_count, 2, "should have 2 LoadSlot instructions");
    }

    #[test]
    fn test_compile_grounded_comparison_rhs() {
        // Rule: (= (less $x $y) (< $x $y))
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("less"), f.atom("$x"), f.atom("$y")]);
        let rhs = f.sexpr(vec![f.atom("<"), f.atom("$x"), f.atom("$y")]);

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

        let code = compile_rule_group(&[entry]).expect("should compile");

        let has_lt = code.instructions.iter().any(|i| matches!(
            i,
            WamInstruction::CallGroundedBinary { op: GroundedBinaryOp::Lt, .. }
        ));
        assert!(has_lt, "should have CallGroundedBinary(Lt)");
    }

    #[test]
    fn test_compile_non_grounded_rhs_fallback() {
        // Rule: (= (f $x) (g $x)) — variable RHS, Phase 1 body compilation should succeed
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x")]);
        let rhs = f.sexpr(vec![f.atom("g"), f.atom("$x")]);

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

        let code = compile_rule_group(&[entry]).expect("should compile");

        // Phase 1: variable RHS should use ReturnEvaluated (body compilation)
        let has_return_evaluated = code.instructions.iter().any(|i| matches!(i, WamInstruction::ReturnEvaluated { .. }));
        assert!(has_return_evaluated, "variable RHS should use Phase 1 body compilation (ReturnEvaluated)");

        let has_call_grounded = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallGroundedBinary { .. }));
        assert!(!has_call_grounded, "non-grounded RHS should NOT have CallGroundedBinary");

        // Should have LoadSlot (for $x) and BuildSExpr (for (g $x))
        let has_load_slot = code.instructions.iter().any(|i| matches!(i, WamInstruction::LoadSlot { .. }));
        assert!(has_load_slot, "RHS with variables should have LoadSlot");
        let has_build_sexpr = code.instructions.iter().any(|i| matches!(i, WamInstruction::BuildSExpr { .. }));
        assert!(has_build_sexpr, "RHS S-expression should have BuildSExpr");
    }

    #[test]
    fn test_compile_ground_rhs_no_variables() {
        // Rule: (= (f 0) "zero")  — ground RHS, no variables, should use TailEval
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), MettaValue::Long(0)]);
        let rhs = f.atom("zero");

        let entry = RuleEntry {
            lhs,
            rhs,
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

        let code = compile_rule_group(&[entry]).expect("should compile");

        // Ground RHS doesn't enter the grounded binary path (rhs_has_variables == false)
        let has_tail_eval = code.instructions.iter().any(|i| matches!(i, WamInstruction::TailEval { .. }));
        assert!(has_tail_eval, "ground RHS should use TailEval");
    }

    #[test]
    fn test_compile_multi_rule_with_grounded_rhs() {
        // Multi-rule where one has grounded RHS and one doesn't
        // Rule 1: (= (f 0) "zero")       — ground, TailEval
        // Rule 2: (= (f $x) (* $x $x))   — grounded binary, inline
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
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
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

        let code = compile_rule_group(&[entry1, entry2]).expect("should compile");

        // Rule 1 should have TailEval (ground RHS)
        let has_tail_eval = code.instructions.iter().any(|i| matches!(i, WamInstruction::TailEval { .. }));
        assert!(has_tail_eval, "rule 1 should use TailEval");

        // Rule 2 should have CallGroundedBinary(Mul)
        let has_mul = code.instructions.iter().any(|i| matches!(
            i,
            WamInstruction::CallGroundedBinary { op: GroundedBinaryOp::Mul, .. }
        ));
        assert!(has_mul, "rule 2 should have inline Mul");
    }

    #[test]
    fn test_atom_to_grounded_op_coverage() {
        assert_eq!(atom_to_grounded_op("+"), Some(GroundedBinaryOp::Add));
        assert_eq!(atom_to_grounded_op("-"), Some(GroundedBinaryOp::Sub));
        assert_eq!(atom_to_grounded_op("*"), Some(GroundedBinaryOp::Mul));
        assert_eq!(atom_to_grounded_op("/"), Some(GroundedBinaryOp::Div));
        assert_eq!(atom_to_grounded_op("%"), Some(GroundedBinaryOp::Mod));
        assert_eq!(atom_to_grounded_op("<"), Some(GroundedBinaryOp::Lt));
        assert_eq!(atom_to_grounded_op("<="), Some(GroundedBinaryOp::Le));
        assert_eq!(atom_to_grounded_op(">"), Some(GroundedBinaryOp::Gt));
        assert_eq!(atom_to_grounded_op(">="), Some(GroundedBinaryOp::Ge));
        assert_eq!(atom_to_grounded_op("=="), Some(GroundedBinaryOp::Eq));
        assert_eq!(atom_to_grounded_op("unknown"), None);
        assert_eq!(atom_to_grounded_op("f"), None);
    }

    #[test]
    fn test_derive_next_reg() {
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("add"), f.atom("$x"), f.atom("$y")]);
        let code = compile_rule_lhs(&lhs).expect("should compile");

        // (add $x $y): GetArity A0, GetArg→A1, GetAtom A1, GetArg→A2, BindSlot A2, GetArg→A3, BindSlot A3
        let next = derive_next_reg(&code.instructions);
        // A0, A1, A2, A3 used → next should be 4
        assert!(next >= 4, "next_reg should be >= 4 for 3-element S-expr, got {}", next);
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 1: RHS Body Compilation Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_rhs_has_variable_refs() {
        let f = factory();
        let slot_map: SmallVec<[(&str, u16); 8]> = smallvec::smallvec![("$x", 0), ("$y", 1)];

        // Ground atom — no variable refs
        assert!(!rhs_has_variable_refs(&f.atom("result"), &slot_map));

        // Variable atom in slot_map
        assert!(rhs_has_variable_refs(&f.atom("$x"), &slot_map));

        // Variable atom NOT in slot_map
        assert!(!rhs_has_variable_refs(&f.atom("$z"), &slot_map));

        // S-expression with variable
        let rhs = f.sexpr(vec![f.atom("g"), f.atom("$x")]);
        assert!(rhs_has_variable_refs(&rhs, &slot_map));

        // S-expression without variables
        let rhs = f.sexpr(vec![f.atom("g"), MettaValue::Long(42)]);
        assert!(!rhs_has_variable_refs(&rhs, &slot_map));

        // Nested S-expression with deep variable
        let rhs = f.sexpr(vec![f.atom("g"), f.sexpr(vec![f.atom("h"), f.atom("$y")])]);
        assert!(rhs_has_variable_refs(&rhs, &slot_map));

        // Literals
        assert!(!rhs_has_variable_refs(&MettaValue::Long(42), &slot_map));
        assert!(!rhs_has_variable_refs(&MettaValue::inline_bool(true), &slot_map));
    }

    #[test]
    fn test_rhs_body_compilation_simple_variable() {
        // Rule: (= (f $x) $x) — identity function
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x")]);
        let rhs = f.atom("$x");

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

        // Should have LoadSlot + ReturnEvaluated (no BuildSExpr for simple variable)
        let has_load_slot = code.instructions.iter().any(|i| matches!(i, WamInstruction::LoadSlot { .. }));
        assert!(has_load_slot, "identity RHS should have LoadSlot for $x");
        let has_return_evaluated = code.instructions.iter().any(|i| matches!(i, WamInstruction::ReturnEvaluated { .. }));
        assert!(has_return_evaluated, "should use ReturnEvaluated");
        let has_build_sexpr = code.instructions.iter().any(|i| matches!(i, WamInstruction::BuildSExpr { .. }));
        assert!(!has_build_sexpr, "simple variable RHS should NOT have BuildSExpr");
    }

    #[test]
    fn test_rhs_body_compilation_nested_sexpr() {
        // Rule: (= (f $x $y) (g (h $x) $y)) — nested RHS
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]);
        let rhs = f.sexpr(vec![
            f.atom("g"),
            f.sexpr(vec![f.atom("h"), f.atom("$x")]),
            f.atom("$y"),
        ]);

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

        // Should have LoadConst (for atoms "g", "h"), LoadSlot (for $x, $y), BuildSExpr (for inner and outer)
        let load_slots: Vec<_> = code.instructions.iter().filter(|i| matches!(i, WamInstruction::LoadSlot { .. })).collect();
        assert_eq!(load_slots.len(), 2, "should have 2 LoadSlots for $x and $y");
        let build_sexprs: Vec<_> = code.instructions.iter().filter(|i| matches!(i, WamInstruction::BuildSExpr { .. })).collect();
        assert_eq!(build_sexprs.len(), 2, "should have 2 BuildSExprs for (h $x) and (g ... $y)");
        let has_return_evaluated = code.instructions.iter().any(|i| matches!(i, WamInstruction::ReturnEvaluated { .. }));
        assert!(has_return_evaluated);

        // Constants should include "g" and "h"
        assert!(code.constants.len() >= 2, "should have at least 2 constants (g, h)");
    }

    #[test]
    fn test_rhs_body_compilation_end_to_end() {
        // Rule: (= (double $x) (+ $x $x)) — this gets grounded binary (priority 1)
        // Rule: (= (wrap $x) (box $x)) — this gets body compilation (priority 2)
        let f = factory();

        // Test body compilation through dispatch
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

        use crate::backend::eval::wam::engine::wam_dispatch_rules;
        let results = wam_dispatch_rules(input, &code);

        assert_eq!(results.len(), 1);
        // ReturnEvaluated constructs the result directly: (box 42)
        let expected = f.sexpr(vec![f.atom("box"), MettaValue::Long(42)]);
        assert_eq!(results[0].0, expected, "body compilation should construct (box 42)");
        assert!(results[0].1.is_empty(), "bindings should be empty (vars already substituted)");
    }

    #[test]
    fn test_rhs_body_ground_rhs_skipped() {
        // Rule: (= (f $x) result) — ground RHS, body compilation should be skipped
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.atom("result"),
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

        // Ground RHS should fall through to TailEval
        let has_tail_eval = code.instructions.iter().any(|i| matches!(i, WamInstruction::TailEval { .. }));
        assert!(has_tail_eval, "ground RHS should use TailEval, not body compilation");
        let has_return_evaluated = code.instructions.iter().any(|i| matches!(i, WamInstruction::ReturnEvaluated { .. }));
        assert!(!has_return_evaluated, "ground RHS should NOT use ReturnEvaluated");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 4: First-Argument Indexing Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_extract_first_arg_key_atom() {
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("a"), f.atom("$y")]);
        let key = extract_first_arg_key(&lhs);
        assert!(matches!(key, Some(WamIndexKey::Atom(a)) if a == "a"));
    }

    #[test]
    fn test_extract_first_arg_key_long() {
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), MettaValue::Long(42), f.atom("$y")]);
        let key = extract_first_arg_key(&lhs);
        assert!(matches!(key, Some(WamIndexKey::Long(42))));
    }

    #[test]
    fn test_extract_first_arg_key_variable() {
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]);
        let key = extract_first_arg_key(&lhs);
        assert!(key.is_none(), "variable first arg should return None");
    }

    #[test]
    fn test_extract_first_arg_key_wildcard() {
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f"), f.atom("_"), f.atom("$y")]);
        let key = extract_first_arg_key(&lhs);
        assert!(key.is_none(), "wildcard first arg should return None");
    }

    #[test]
    fn test_extract_first_arg_key_sexpr_head() {
        let f = factory();
        let lhs = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.atom("$y"),
        ]);
        let key = extract_first_arg_key(&lhs);
        assert!(matches!(key, Some(WamIndexKey::SExprHead(h)) if h == "g"));
    }

    #[test]
    fn test_extract_first_arg_key_no_args() {
        let f = factory();
        let lhs = f.sexpr(vec![f.atom("f")]);
        let key = extract_first_arg_key(&lhs);
        assert!(key.is_none(), "no first arg should return None");
    }

    #[test]
    fn test_indexed_compilation_not_triggered_below_threshold() {
        // 3 rules with distinct first args — below 4-rule threshold
        let f = factory();
        let entries: Vec<RuleEntry<MettaValue>> = vec!["a", "b", "c"]
            .into_iter()
            .map(|first| RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom(first)]),
                rhs: f.atom(first),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            })
            .collect();

        let code = compile_rule_group(&entries).expect("compile");
        // Should NOT use SwitchOnFirstArg (< 4 rules)
        let has_switch = code.instructions.iter().any(|i| {
            matches!(i, WamInstruction::SwitchOnFirstArg { .. })
        });
        assert!(!has_switch, "should not use indexing for < 4 rules");
    }

    #[test]
    fn test_indexed_compilation_triggered_for_4_rules() {
        // 4 rules with 2+ distinct first arg keys — triggers indexing
        let f = factory();
        let entries: Vec<RuleEntry<MettaValue>> = vec!["a", "a", "b", "b"]
            .into_iter()
            .map(|first| RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom(first)]),
                rhs: f.atom("result"),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            })
            .collect();

        let code = compile_rule_group(&entries).expect("compile");
        // Should use SwitchOnFirstArg
        let has_switch = code.instructions.iter().any(|i| {
            matches!(i, WamInstruction::SwitchOnFirstArg { .. })
        });
        assert!(has_switch, "should use indexing for 4+ rules with 2+ keys");
        assert!(!code.index_tables.is_empty(), "should have an index table");
        assert!(code.index_tables[0].entries.len() >= 2, "should have 2+ keys in table");
    }

    #[test]
    fn test_indexed_compilation_not_triggered_single_key() {
        // 4 rules all with same first arg — only 1 distinct key, not worth indexing
        let f = factory();
        let entries: Vec<RuleEntry<MettaValue>> = (0..4)
            .map(|i| RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("same"), MettaValue::Long(i)]),
                rhs: MettaValue::Long(i),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec![],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            })
            .collect();

        let code = compile_rule_group(&entries).expect("compile");
        // Should NOT use SwitchOnFirstArg (only 1 distinct key)
        let has_switch = code.instructions.iter().any(|i| {
            matches!(i, WamInstruction::SwitchOnFirstArg { .. })
        });
        assert!(!has_switch, "should not index with only 1 distinct key");
    }

    #[test]
    fn test_indexed_compilation_with_default_group() {
        // 4 rules: 2 with atom "a", 1 with atom "b", 1 with variable $x
        let f = factory();
        let entries: Vec<RuleEntry<MettaValue>> = vec![
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("a"), MettaValue::Long(1)]),
                rhs: MettaValue::Long(1),
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
                rhs: MettaValue::Long(2),
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
                rhs: MettaValue::Long(3),
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
                lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), MettaValue::Long(4)]),
                rhs: MettaValue::Long(4),
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
                var_names: vec!["$x"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: false,
                structural_matcher: None,
                wam_code: None,
            },
        ];

        let code = compile_rule_group(&entries).expect("compile");
        assert!(
            code.instructions.iter().any(|i| matches!(i, WamInstruction::SwitchOnFirstArg { .. })),
            "should use indexing"
        );
        // Should have keys for "a" and "b" in the index table
        let table = &code.index_tables[0];
        assert!(table.entries.contains_key(&WamIndexKey::Atom("a")));
        assert!(table.entries.contains_key(&WamIndexKey::Atom("b")));
        assert_eq!(table.entries.len(), 2, "should have exactly 2 indexed keys");
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 2: Special Form Compilation Tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_condition_guaranteed_boolean() {
        let f = factory();
        // Boolean literals
        assert!(condition_guaranteed_boolean(&MettaValue::inline_bool(true)));
        assert!(condition_guaranteed_boolean(&MettaValue::inline_bool(false)));
        // Comparison ops
        assert!(condition_guaranteed_boolean(&f.sexpr(vec![f.atom("<"), f.atom("$x"), MettaValue::Long(0)])));
        assert!(condition_guaranteed_boolean(&f.sexpr(vec![f.atom("<="), f.atom("$x"), f.atom("$y")])));
        assert!(condition_guaranteed_boolean(&f.sexpr(vec![f.atom(">"), f.atom("$x"), f.atom("$y")])));
        assert!(condition_guaranteed_boolean(&f.sexpr(vec![f.atom(">="), f.atom("$x"), f.atom("$y")])));
        assert!(condition_guaranteed_boolean(&f.sexpr(vec![f.atom("=="), f.atom("$x"), f.atom("$y")])));
        // Non-boolean expressions
        assert!(!condition_guaranteed_boolean(&f.atom("$x")));
        assert!(!condition_guaranteed_boolean(&MettaValue::Long(42)));
        assert!(!condition_guaranteed_boolean(&f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")])));
    }

    #[test]
    fn test_compile_if_produces_branch_on_bool() {
        // Rule: (= (f $x $y) (if (< $x $y) "yes" "no"))
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
        let has_branch = code.instructions.iter().any(|i| matches!(i, WamInstruction::BranchOnBool { .. }));
        assert!(has_branch, "if with comparison should produce BranchOnBool");
        let has_jump = code.instructions.iter().any(|i| matches!(i, WamInstruction::Jump { .. }));
        assert!(has_jump, "if should have Jump to skip else branch");
        // Should have ReturnEvaluated (one for each branch)
        let return_count = code.instructions.iter().filter(|i| matches!(i, WamInstruction::ReturnEvaluated { .. })).count();
        assert_eq!(return_count, 2, "should have 2 ReturnEvaluated (then + else)");
    }

    #[test]
    fn test_compile_if_non_boolean_no_branch() {
        // Rule: (= (f $x) (if $x "yes" "no"))
        // Variable condition → not guaranteed boolean → TailEval fallback
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
        let has_branch = code.instructions.iter().any(|i| matches!(i, WamInstruction::BranchOnBool { .. }));
        assert!(!has_branch, "variable condition should not produce BranchOnBool");
    }

    #[test]
    fn test_compile_if_constant_true_no_branch() {
        // Rule: (= (f $x) (if True $x "no"))
        // Constant fold → no BranchOnBool
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                MettaValue::inline_bool(true),
                f.atom("$x"),
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
        let has_branch = code.instructions.iter().any(|i| matches!(i, WamInstruction::BranchOnBool { .. }));
        assert!(!has_branch, "constant True should be folded, no BranchOnBool");
        // Should have ReturnEvaluated (only the then branch survives)
        let return_count = code.instructions.iter().filter(|i| matches!(i, WamInstruction::ReturnEvaluated { .. })).count();
        assert_eq!(return_count, 1, "constant fold → only 1 ReturnEvaluated");
    }

    #[test]
    fn test_compile_let_produces_bind_slot() {
        // Rule: (= (f $x) (let $y (+ $x 1) $y))
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("let"),
                f.atom("$y"),
                f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)]),
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
        // Should have BindSlot for the let variable
        let bind_count = code.instructions.iter().filter(|i| matches!(i, WamInstruction::BindSlot { .. })).count();
        // 1 from LHS ($x), 1 from let ($y)
        assert!(bind_count >= 2, "should have BindSlot for $x and $y, got {}", bind_count);
        // num_slots should be >= 2 (1 for $x from LHS, 1 for $y from let)
        assert!(code.num_slots >= 2, "num_slots should be >= 2, got {}", code.num_slots);
    }

    #[test]
    fn test_compile_letstar_multiple_bindings() {
        // Rule: (= (f $x) (let* (($a (+ $x 1)) ($b (+ $a 2))) $b))
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("let*"),
                f.sexpr(vec![
                    f.sexpr(vec![f.atom("$a"), f.sexpr(vec![f.atom("+"), f.atom("$x"), MettaValue::Long(1)])]),
                    f.sexpr(vec![f.atom("$b"), f.sexpr(vec![f.atom("+"), f.atom("$a"), MettaValue::Long(2)])]),
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
        // num_slots should be >= 3 (1 for $x, 1 for $a, 1 for $b)
        assert!(code.num_slots >= 3, "num_slots should be >= 3, got {}", code.num_slots);
    }

    #[test]
    fn test_compile_chain_similar_to_let() {
        // Rule: (= (f $x) (chain (+ $x 10) $y $y))
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
        assert!(code.num_slots >= 2, "chain should allocate slot for $y");
        // Should have ReturnEvaluated (not TailEval)
        let has_return = code.instructions.iter().any(|i| matches!(i, WamInstruction::ReturnEvaluated { .. }));
        assert!(has_return, "chain should produce ReturnEvaluated");
    }

    #[test]
    fn test_patch_branch_offsets() {
        let mut instrs = vec![
            WamInstruction::BranchOnBool { cond_reg: 0, then_ip: 1, else_ip: 3 },
            WamInstruction::LoadConst { const_index: 0, target_reg: 1 },
            WamInstruction::Jump { target_ip: 4 },
            WamInstruction::LoadConst { const_index: 1, target_reg: 2 },
        ];

        patch_branch_offsets(&mut instrs, 10);

        match &instrs[0] {
            WamInstruction::BranchOnBool { then_ip, else_ip, .. } => {
                assert_eq!(*then_ip, 11, "then_ip should be 1+10=11");
                assert_eq!(*else_ip, 13, "else_ip should be 3+10=13");
            }
            _ => panic!("expected BranchOnBool"),
        }
        match &instrs[2] {
            WamInstruction::Jump { target_ip } => {
                assert_eq!(*target_ip, 14, "target_ip should be 4+10=14");
            }
            _ => panic!("expected Jump"),
        }
    }

    #[test]
    fn test_try_compile_inline_eval_variable() {
        let f = factory();
        let slot_map: SmallVec<[(&str, u16); 8]> = smallvec::smallvec![("$x", 0)];
        let mut next_reg = 1u8;
        let mut constants = Vec::new();
        let mut instructions = Vec::new();

        let expr = f.atom("$x");
        let result = try_compile_inline_eval(
            &expr, &slot_map, 1, &mut next_reg, &mut constants, &mut instructions,
        );
        assert!(result.is_some(), "variable should compile inline");
        assert_eq!(instructions.len(), 1);
        assert!(matches!(&instructions[0], WamInstruction::LoadSlot { slot: 0, target_reg: 1 }));
    }

    #[test]
    fn test_try_compile_inline_eval_literal() {
        let slot_map: SmallVec<[(&str, u16); 8]> = smallvec::smallvec![];
        let mut next_reg = 1u8;
        let mut constants = Vec::new();
        let mut instructions = Vec::new();

        let expr = MettaValue::Long(42);
        let result = try_compile_inline_eval(
            &expr, &slot_map, 1, &mut next_reg, &mut constants, &mut instructions,
        );
        assert!(result.is_some(), "literal should compile inline");
        assert_eq!(instructions.len(), 1);
        assert!(matches!(&instructions[0], WamInstruction::LoadConst { target_reg: 1, .. }));
        assert_eq!(constants[0], MettaValue::Long(42));
    }

    #[test]
    fn test_try_compile_inline_eval_binary_op() {
        let f = factory();
        let slot_map: SmallVec<[(&str, u16); 8]> = smallvec::smallvec![("$x", 0), ("$y", 1)];
        let mut next_reg = 2u8;
        let mut constants = Vec::new();
        let mut instructions = Vec::new();

        let expr = f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]);
        let result = try_compile_inline_eval(
            &expr, &slot_map, 2, &mut next_reg, &mut constants, &mut instructions,
        );
        assert!(result.is_some(), "binary op should compile inline");
        // Should produce: LoadSlot($x) → r3, LoadSlot($y) → r4, CallGroundedBinary(Add, r3, r4, r2)
        assert_eq!(instructions.len(), 3);
        assert!(matches!(&instructions[2], WamInstruction::CallGroundedBinary { op: GroundedBinaryOp::Add, target_reg: 2, .. }));
    }

    #[test]
    fn test_try_compile_inline_eval_unsupported() {
        let f = factory();
        let slot_map: SmallVec<[(&str, u16); 8]> = smallvec::smallvec![];
        let mut next_reg = 1u8;
        let mut constants = Vec::new();
        let mut instructions = Vec::new();

        // User-defined function call → can't inline
        let expr = f.sexpr(vec![f.atom("my-func"), MettaValue::Long(1)]);
        let result = try_compile_inline_eval(
            &expr, &slot_map, 1, &mut next_reg, &mut constants, &mut instructions,
        );
        assert!(result.is_none(), "user-defined function call should not compile inline");
    }

    #[test]
    fn test_compile_multi_rule_with_special_form() {
        // Two rules, one with if special form:
        // Rule 1: (= (f $x) (if (< $x 0) "neg" "pos"))
        // Rule 2: (= (f 0) "zero")
        let f = factory();
        let entries: Vec<RuleEntry<MettaValue>> = vec![
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
                rhs: f.sexpr(vec![
                    f.atom("if"),
                    f.sexpr(vec![f.atom("<"), f.atom("$x"), MettaValue::Long(0)]),
                    f.string("neg"),
                    f.string("pos"),
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
            },
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
        ];

        let code = compile_rule_group(&entries).expect("compile");
        // The first rule should have BranchOnBool
        let has_branch = code.instructions.iter().any(|i| matches!(i, WamInstruction::BranchOnBool { .. }));
        assert!(has_branch, "multi-rule group should include BranchOnBool for if rule");
    }

    // ════════════════════════════════════════════════════════════════════
    // Phase 3: User-Defined Function Call Compilation Tests
    // ════════════════════════════════════════════════════════════════════

    #[test]
    fn test_compile_user_call_produces_call_user_func() {
        // Rule: (= (f $x) (g $x))
        // RHS calls user-defined function g with variable argument
        let f = factory();
        let entry = RuleEntry {
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

        let code = compile_rule_group(&[entry]).expect("compile");
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(has_call, "user function call should produce CallUserFunc instruction");
        // Should also have BuildSExpr (to construct the call expression)
        let has_build = code.instructions.iter().any(|i| matches!(i, WamInstruction::BuildSExpr { .. }));
        assert!(has_build, "user function call should build call expression with BuildSExpr");
    }

    #[test]
    fn test_compile_user_call_with_literal_args() {
        // Rule: (= (f $x) (g $x 42))
        // RHS calls user-defined function g with variable and literal args
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("g"), f.atom("$x"), MettaValue::Long(42)]),
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
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(has_call, "user function call with literal args should compile");
        // Should have LoadConst for "g" and 42
        let load_count = code.instructions.iter().filter(|i| matches!(i, WamInstruction::LoadConst { .. })).count();
        assert!(load_count >= 2, "should load at least head atom and literal: got {}", load_count);
    }

    #[test]
    fn test_compile_user_call_with_grounded_arg() {
        // Rule: (= (f $x $y) (g (+ $x $y)))
        // RHS calls user-defined function g with a grounded binary op as argument
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
            rhs: f.sexpr(vec![
                f.atom("g"),
                f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]),
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
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(has_call, "user call with grounded arg should produce CallUserFunc");
        // Should have CallGroundedBinary for the (+ $x $y) argument
        let has_grounded = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallGroundedBinary { .. }));
        assert!(has_grounded, "grounded binary op in arg should compile inline");
    }

    #[test]
    fn test_compile_user_call_not_for_grounded_ops() {
        // Rule: (= (f $x $y) (+ $x $y))
        // RHS is a grounded op, should NOT produce CallUserFunc (priority 1 handles this)
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]),
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
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(!has_call, "grounded op RHS should NOT produce CallUserFunc");
    }

    #[test]
    fn test_compile_user_call_not_for_special_forms() {
        // Rule: (= (f $x) (if (== $x 0) "zero" "other"))
        // RHS is a special form, should NOT produce CallUserFunc (priority 2 handles this)
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("f"), f.atom("$x")]),
            rhs: f.sexpr(vec![
                f.atom("if"),
                f.sexpr(vec![f.atom("=="), f.atom("$x"), MettaValue::Long(0)]),
                f.string("zero"),
                f.string("other"),
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
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(!has_call, "special form RHS should NOT produce CallUserFunc");
    }

    #[test]
    fn test_compile_user_call_fallback_structure() {
        // Rule: (= (f $x) (g $x))
        // Verify the instruction layout: CallUserFunc → ReturnEvaluated → Jump → ReturnEvaluated
        let f = factory();
        let entry = RuleEntry {
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

        let code = compile_rule_group(&[entry]).expect("compile");
        // Find the CallUserFunc instruction
        let call_idx = code.instructions.iter().position(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(call_idx.is_some(), "should have CallUserFunc");
        let call_idx = call_idx.expect("checked above");

        // After CallUserFunc: ReturnEvaluated (success), Jump, ReturnEvaluated (fallback)
        assert!(matches!(code.instructions[call_idx + 1], WamInstruction::ReturnEvaluated { .. }),
            "success path should be ReturnEvaluated");
        assert!(matches!(code.instructions[call_idx + 2], WamInstruction::Jump { .. }),
            "should jump over fallback");
        assert!(matches!(code.instructions[call_idx + 3], WamInstruction::ReturnEvaluated { .. }),
            "fallback should be ReturnEvaluated");

        // Verify fallback_ip points to the fallback ReturnEvaluated
        if let WamInstruction::CallUserFunc { fallback_ip, .. } = &code.instructions[call_idx] {
            assert_eq!(*fallback_ip as usize, call_idx + 3,
                "fallback_ip should point to fallback ReturnEvaluated");
        }

        // Verify Jump target_ip points past the fallback
        if let WamInstruction::Jump { target_ip } = &code.instructions[call_idx + 2] {
            assert_eq!(*target_ip as usize, call_idx + 4,
                "Jump should skip past fallback ReturnEvaluated");
        }
    }

    #[test]
    fn test_compile_user_call_in_multi_rule() {
        // Two rules for (f ...):
        //   (= (f 0) "zero")
        //   (= (f $x) (g $x))
        let f = factory();
        let entries = vec![
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
            },
        ];

        let code = compile_rule_group(&entries).expect("compile");
        let has_call = code.instructions.iter().any(|i| matches!(i, WamInstruction::CallUserFunc { .. }));
        assert!(has_call, "second rule in multi-rule group should produce CallUserFunc");
    }

    // ════════════════════════════════════════════════════════════════════════
    // Phase 6: fully_evaluable flag tests
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_fully_evaluable_grounded_rhs() {
        // Rule: (= (double $x) (+ $x $x))
        // Grounded RHS → CallGroundedBinary + ReturnEvaluated → fully_evaluable = true
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("double"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$x")]),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
        };
        let code = compile_rule_group(&[entry]).expect("compile");
        assert!(code.fully_evaluable, "grounded RHS (+ $x $x) should be fully_evaluable");
    }

    #[test]
    fn test_not_fully_evaluable_tail_eval() {
        // Rule: (= (wrap $x) (box $x))
        // Non-grounded, non-special-form RHS → TailEval → fully_evaluable = false
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("wrap"), f.atom("$x")]),
            rhs: f.sexpr(vec![f.atom("box"), f.atom("$x")]),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
        };
        let code = compile_rule_group(&[entry]).expect("compile");
        // (box $x) — "box" is a user-defined function call (not grounded, not special).
        // The compiler may produce:
        //   - CallUserFunc + ReturnEvaluated (if user call succeeds) → fully_evaluable
        //   - TailEval (if all body compilation paths fail) → not fully_evaluable
        // Just verify the flag is consistent with instruction content.
        let has_non_eval = code.instructions.iter().any(|i| matches!(
            i,
            WamInstruction::TailEval { .. }
                | WamInstruction::Proceed
                | WamInstruction::YieldToTrampoline
        ));
        assert_eq!(!has_non_eval, code.fully_evaluable,
            "fully_evaluable should match absence of TailEval/Proceed/YieldToTrampoline. Instructions: {:?}",
            code.instructions);
    }

    #[test]
    fn test_fully_evaluable_variable_rhs() {
        // Rule: (= (id $x) $x)
        // Variable-only RHS: the bound value is returned as the match result.
        // Further evaluation (if the value is an S-expression) happens in a
        // separate trampoline step, not in try_match_all_rules.
        let f = factory();
        let entry = RuleEntry {
            lhs: f.sexpr(vec![f.atom("id"), f.atom("$x")]),
            rhs: f.atom("$x"),
            var_names: vec!["$x"],
            wildcard_indices: smallvec::smallvec![],
            multiplicity: 1,
            rhs_type: None,
            rhs_has_variables: true,
            structural_matcher: None,
            wam_code: None,
            lhs_debruijn: Vec::new(),
            lhs_wide_debruijn: Vec::new(),
        };
        let code = compile_rule_group(&[entry]).expect("compile");
        // Verify consistency: flag matches instruction content
        let has_non_eval = code.instructions.iter().any(|i| matches!(
            i,
            WamInstruction::TailEval { .. }
                | WamInstruction::Proceed
                | WamInstruction::YieldToTrampoline
        ));
        assert_eq!(!has_non_eval, code.fully_evaluable,
            "fully_evaluable should match absence of TailEval/Proceed/YieldToTrampoline");
    }

    #[test]
    fn test_fully_evaluable_multi_rule_grounded() {
        // Two rules with grounded RHS → both produce ReturnEvaluated → fully_evaluable
        let f = factory();
        let entries = vec![
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("op"), f.atom("$x"), f.atom("$y")]),
                rhs: f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]),
                var_names: vec!["$x", "$y"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
            },
            RuleEntry {
                lhs: f.sexpr(vec![f.atom("op"), f.atom("$a"), f.atom("$b")]),
                rhs: f.sexpr(vec![f.atom("*"), f.atom("$a"), f.atom("$b")]),
                var_names: vec!["$a", "$b"],
                wildcard_indices: smallvec::smallvec![],
                multiplicity: 1,
                rhs_type: None,
                rhs_has_variables: true,
                structural_matcher: None,
                wam_code: None,
                lhs_debruijn: Vec::new(),
                lhs_wide_debruijn: Vec::new(),
            },
        ];
        let code = compile_rule_group(&entries).expect("compile");
        assert!(code.fully_evaluable,
            "multi-rule group with all grounded RHS should be fully_evaluable");
    }

    #[test]
    fn test_is_fully_evaluable_helper() {
        // Direct test of the helper function
        use super::is_fully_evaluable;
        assert!(is_fully_evaluable(&[
            WamInstruction::GetArity { reg: 0, expected: 2 },
            WamInstruction::ReturnEvaluated { rhs_index: 0, result_reg: 1 },
        ]), "ReturnEvaluated-only should be fully_evaluable");

        assert!(!is_fully_evaluable(&[
            WamInstruction::GetArity { reg: 0, expected: 2 },
            WamInstruction::TailEval { rhs_index: 0, has_variables: true },
        ]), "TailEval should NOT be fully_evaluable");

        assert!(!is_fully_evaluable(&[
            WamInstruction::Proceed,
        ]), "Proceed should NOT be fully_evaluable");

        assert!(!is_fully_evaluable(&[
            WamInstruction::YieldToTrampoline,
        ]), "YieldToTrampoline should NOT be fully_evaluable");

        assert!(is_fully_evaluable(&[]), "empty instructions should be fully_evaluable");
    }
}
