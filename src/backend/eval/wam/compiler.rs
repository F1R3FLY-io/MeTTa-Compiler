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

use std::sync::Arc;

use smallvec::SmallVec;

use crate::backend::environment::rule_management::RuleEntry;
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

            return Some(Arc::new(code));
        }

        // Priority 2: Try RHS body construction (eliminates apply_bindings)
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

            return Some(Arc::new(code));
        }
    }

    // Priority 3: Fall back to TailEval (trampoline handles apply_bindings)
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

        // Try to compile the RHS (grounded first, then body construction, then TailEval)
        let mut rhs_compiled = false;

        if entry.rhs_has_variables {
            // Priority 1: Try inline grounded binary operation
            let mut next_reg = base_next_reg;
            if let Some(grounded_instrs) = try_compile_grounded_rhs(
                &entry.rhs, &slot_map, rhs_index, &mut next_reg,
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

            // Priority 2: Try RHS body construction
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

        // Priority 3: Fall back to TailEval
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

    Some(Arc::new(WamCode {
        instructions: all_instructions,
        num_slots: max_slots,
        slot_names,
        rhs_templates,
        constants,
    }))
}

// ════════════════════════════════════════════════════════════════════════
// Multi-Rule Helpers
// ════════════════════════════════════════════════════════════════════════

/// Emit the appropriate choice point instruction for alternative `i` of `n` total.
fn emit_choice_point(instructions: &mut Vec<WamInstruction>, i: usize, n: usize) {
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
    emit_rhs_arg_load(&items[1], slot_map, left_reg, &mut instructions)?;

    let right_reg = alloc_rhs_reg(next_reg)?;
    emit_rhs_arg_load(&items[2], slot_map, right_reg, &mut instructions)?;

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
    instructions: &mut Vec<WamInstruction>,
) -> Option<()> {
    // Variable argument: load from binding frame slot
    if let Some(name) = arg.as_atom() {
        if (name.starts_with('$')
            || name.starts_with('\'')
            || (name.starts_with('&') && name != "&"))
            && name.len() > 1
        {
            let slot = slot_map.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| *s)?;
            instructions.push(WamInstruction::LoadSlot {
                slot,
                target_reg,
            });
            return Some(());
        }
    }

    // For non-variable arguments, this optimization doesn't apply.
    // The value would need to be materialized from the RHS template, which
    // TailEval already handles efficiently. Return None to fall back.
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
}
