//! WAM Instruction Set for MeTTa pattern matching and evaluation.
//!
//! The instruction set is derived from two sources:
//! 1. **StructuralMatcher operations** (StructuralCheck + VarOp) → `Get*` and `Bind*` families
//! 2. **MeTTa control flow** → `TryMeElse`, `Proceed`, `Fail`, `TailEval`
//!
//! # Key Improvement Over StructuralMatcher
//!
//! StructuralMatcher uses `MatchPath::navigate()` which re-traverses the expression
//! tree from the root for each check. WAM instructions use register-based decomposition:
//! `GetArg` loads a child into a register once, then subsequent checks operate on
//! registers. This eliminates redundant path traversal.
//!
//! # Instruction Categories
//!
//! | Category | Instructions | Purpose |
//! |----------|-------------|---------|
//! | Head matching | GetArity, GetAtom, GetLong, GetBool, GetFloat, GetString | Structural checks |
//! | Decomposition | GetArg | S-expression child extraction |
//! | Binding | BindSlot, EqualCheck, LoadSlot | Variable binding operations |
//! | Control | TryMeElse, RetryMeElse, TrustMe, Proceed, Fail | Nondeterministic branching |
//! | Evaluation | TailEval, CallGrounded, YieldToTrampoline | RHS evaluation dispatch |

use crate::backend::models::MettaValue;

/// WAM instruction for compiled pattern matching and evaluation.
///
/// Instructions are compact (typically 8-16 bytes including padding) and designed
/// for sequential execution with branch-on-failure semantics: any check instruction
/// that fails causes an immediate jump to the `Fail` handler.
#[derive(Clone, Debug)]
pub enum WamInstruction {
    // ════════════════════════════════════════════════════════════════════
    // Head Matching (derived from StructuralCheck)
    // ════════════════════════════════════════════════════════════════════

    /// Check that the value in register `reg` is an S-expression with exactly
    /// `expected` children. Fails if not an S-expression or wrong arity.
    GetArity {
        reg: u8,
        expected: u16,
    },

    /// Check that the value in register `reg` is an atom equal to `expected`.
    /// Atom strings are interned (`&'static str`), so this is typically a
    /// pointer comparison on the interned address.
    GetAtom {
        reg: u8,
        expected: &'static str,
    },

    /// Check that the value in register `reg` is a Long integer equal to `expected`.
    GetLong {
        reg: u8,
        expected: i64,
    },

    /// Check that the value in register `reg` is a Bool equal to `expected`.
    GetBool {
        reg: u8,
        expected: bool,
    },

    /// Check that the value in register `reg` is a Float with bits equal to
    /// `expected_bits`. Uses bitwise comparison to avoid NaN issues.
    GetFloat {
        reg: u8,
        expected_bits: u64,
    },

    /// Check that the value in register `reg` is a String equal to `expected`.
    GetString {
        reg: u8,
        expected: &'static str,
    },

    // ════════════════════════════════════════════════════════════════════
    // Argument Decomposition
    // ════════════════════════════════════════════════════════════════════

    /// Extract child at `child_index` from the S-expression in `source_reg`
    /// and store it in `target_reg`.
    ///
    /// Prerequisite: `source_reg` must contain an S-expression with sufficient
    /// arity (verified by a prior `GetArity` check).
    GetArg {
        source_reg: u8,
        child_index: u8,
        target_reg: u8,
    },

    // ════════════════════════════════════════════════════════════════════
    // Variable Binding (derived from VarOp)
    // ════════════════════════════════════════════════════════════════════

    /// Bind: copy the value in register `reg` to binding frame slot `slot`.
    /// The previous value at the slot is recorded on the trail for undo.
    BindSlot {
        reg: u8,
        slot: u16,
    },

    /// Check that the value in register `reg` equals the value already bound
    /// in slot `slot`. Used for repeated variables (e.g., `(f $x $x)`).
    /// Fails if the values are not equal.
    EqualCheck {
        reg: u8,
        slot: u16,
    },

    /// Load the value from binding frame slot `slot` into register `target_reg`.
    /// Used for accessing previously-bound variables during RHS evaluation.
    LoadSlot {
        slot: u16,
        target_reg: u8,
    },

    // ════════════════════════════════════════════════════════════════════
    // Control Flow — Nondeterministic Branching
    // ════════════════════════════════════════════════════════════════════

    /// Try the first alternative. Creates a choice point with the remaining
    /// alternatives. On failure, execution continues at the next alternative.
    ///
    /// `next_alternative` is the index into the WamCode's instruction stream
    /// where the next `RetryMeElse` or `TrustMe` begins.
    TryMeElse {
        next_alternative: u16,
    },

    /// Try an intermediate alternative. The choice point already exists.
    /// On failure, execution continues at `next_alternative`.
    RetryMeElse {
        next_alternative: u16,
    },

    /// Try the last alternative. The choice point is removed after this
    /// alternative completes (either success or failure).
    TrustMe,

    /// Evaluation succeeded: the result is in register A0.
    /// For all-solutions semantics, the result is accumulated in the current
    /// choice point's results vector, then execution backtracks to try
    /// the next alternative.
    Proceed,

    /// Evaluation failed: trigger backtracking to the most recent choice point.
    /// The trail is unwound and the next alternative is tried.
    Fail,

    // ════════════════════════════════════════════════════════════════════
    // RHS Evaluation
    // ════════════════════════════════════════════════════════════════════

    /// Evaluate the RHS of the current rule with bindings from the frame.
    ///
    /// This is the terminal instruction for a successful match. It constructs
    /// the result by applying bindings from the binding frame to the RHS template.
    ///
    /// `rhs_index` indexes into the WamCode's `rhs_templates` array.
    /// `has_variables`: if false, skip apply_bindings (RHS is ground).
    TailEval {
        rhs_index: u16,
        has_variables: bool,
    },

    /// Yield to the existing trampoline for evaluation of expressions that
    /// the WAM engine doesn't handle natively (Tier 2 special forms).
    ///
    /// The expression to evaluate is constructed from the current bindings
    /// and placed in register A0.
    YieldToTrampoline,

    // ════════════════════════════════════════════════════════════════════
    // Phase 1 (WAM Execution): RHS Body Construction
    // ════════════════════════════════════════════════════════════════════

    /// Build an S-expression from consecutive register values.
    ///
    /// Reads `count` values from registers `start_reg..start_reg+count`,
    /// allocates a new S-expression via the global factory, and stores the
    /// result in `target_reg`.
    ///
    /// Used by compiled RHS bodies to construct result expressions directly
    /// from bound variable values, eliminating the `apply_bindings` step.
    BuildSExpr {
        start_reg: u8,
        count: u8,
        target_reg: u8,
    },

    /// Load a constant MettaValue into a register.
    ///
    /// The constant is referenced by index into `WamCode.constants`.
    /// Used for ground atoms, literals, and other non-variable values in
    /// compiled RHS bodies.
    LoadConst {
        const_index: u16,
        target_reg: u8,
    },

    // ════════════════════════════════════════════════════════════════════
    // Phase 4: Inline Grounded Operations
    // ════════════════════════════════════════════════════════════════════

    /// Call a binary grounded operation on two register values, storing the
    /// result in `target_reg`. Falls back to Fail if types are incompatible.
    ///
    /// Used for inline evaluation of simple RHS bodies like `(+ $x $y)`.
    CallGroundedBinary {
        op: GroundedBinaryOp,
        left_reg: u8,
        right_reg: u8,
        target_reg: u8,
    },

    /// Return the value in `reg` as an already-evaluated result.
    /// Used after inline grounded operations to emit the result without
    /// returning to the trampoline for evaluation.
    ReturnEvaluated {
        rhs_index: u16,
        result_reg: u8,
    },
}

/// Supported binary grounded operations for inline WAM evaluation.
///
/// These correspond to the most common arithmetic and comparison operations
/// in PLN truth value computation. Each operation handles Long×Long, Float×Float,
/// and Long×Float type combinations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundedBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

/// Compact representation for serialized instruction opcodes (future use).
///
/// Currently instructions are stored as the enum above. If profiling shows
/// that enum discrimination overhead is significant, instructions can be
/// serialized to a compact bytecode format using these opcodes.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum WamOpcode {
    GetArity = 0x01,
    GetAtom = 0x02,
    GetLong = 0x03,
    GetBool = 0x04,
    GetFloat = 0x05,
    GetString = 0x06,
    GetArg = 0x10,
    BindSlot = 0x20,
    EqualCheck = 0x21,
    LoadSlot = 0x22,
    TryMeElse = 0x30,
    RetryMeElse = 0x31,
    TrustMe = 0x32,
    Proceed = 0x33,
    Fail = 0x34,
    TailEval = 0x40,
    YieldToTrampoline = 0x41,
    BuildSExpr = 0x52,
    LoadConst = 0x53,
    CallGroundedBinary = 0x50,
    ReturnEvaluated = 0x51,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_size() {
        // Verify instructions aren't unreasonably large
        let size = std::mem::size_of::<WamInstruction>();
        // Should be <= 24 bytes (enum discriminant + largest variant)
        assert!(
            size <= 32,
            "WamInstruction is {} bytes, expected <= 32",
            size
        );
    }

    #[test]
    fn test_opcode_values_unique() {
        // Verify all opcode values are distinct
        let opcodes = [
            WamOpcode::GetArity as u8,
            WamOpcode::GetAtom as u8,
            WamOpcode::GetLong as u8,
            WamOpcode::GetBool as u8,
            WamOpcode::GetFloat as u8,
            WamOpcode::GetString as u8,
            WamOpcode::GetArg as u8,
            WamOpcode::BindSlot as u8,
            WamOpcode::EqualCheck as u8,
            WamOpcode::LoadSlot as u8,
            WamOpcode::TryMeElse as u8,
            WamOpcode::RetryMeElse as u8,
            WamOpcode::TrustMe as u8,
            WamOpcode::Proceed as u8,
            WamOpcode::Fail as u8,
            WamOpcode::TailEval as u8,
            WamOpcode::YieldToTrampoline as u8,
            WamOpcode::BuildSExpr as u8,
            WamOpcode::LoadConst as u8,
            WamOpcode::CallGroundedBinary as u8,
            WamOpcode::ReturnEvaluated as u8,
        ];
        for (i, &a) in opcodes.iter().enumerate() {
            for (j, &b) in opcodes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "opcodes at indices {} and {} collide", i, j);
                }
            }
        }
    }
}
