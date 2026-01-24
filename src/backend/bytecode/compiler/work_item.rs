//! Work items and continuations for iterative bytecode compilation.
//!
//! This module defines the types used by the iterative compiler to avoid
//! stack overflow from deep recursive calls. The pattern mirrors the evaluator
//! trampoline in `src/backend/eval/trampoline/`.

use std::collections::VecDeque;

use crate::backend::bytecode::chunk::JumpLabel;
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

/// Work item representing pending compilation work.
/// Each variant represents a distinct compilation task that may require
/// sub-compilations before completion.
#[derive(Debug)]
pub enum CompileWork {
    /// Compile an expression - the main entry point
    CompileExpr {
        expr: MettaValue,
        in_tail_position: bool,
        cont_id: usize,
    },

    /// Compile a binary operation (e.g., +, -, *, /, <, ==, and, or)
    CompileBinaryOp {
        op: BinaryOp,
        left: MettaValue,
        right: MettaValue,
        /// Folded value if constant folding succeeded
        folded: Option<MettaValue>,
        cont_id: usize,
    },

    /// Compile a unary operation (e.g., not, abs, neg, sqrt)
    CompileUnaryOp {
        op: UnaryOp,
        arg: MettaValue,
        /// Folded value if constant folding succeeded
        folded: Option<MettaValue>,
        cont_id: usize,
    },

    /// Compile function call arguments
    CompileCallArgs {
        head: String,
        args: VecDeque<MettaValue>,
        arity: usize,
        saved_tail_position: bool,
        cont_id: usize,
    },

    /// Compile S-expression elements as data (not call)
    CompileSExprElements {
        items: VecDeque<MettaValue>,
        total_count: usize,
        cont_id: usize,
    },

    /// Compile if/then/else - multi-phase with jump patching
    CompileIf {
        condition: MettaValue,
        then_branch: MettaValue,
        else_branch: MettaValue,
        /// Jump label to patch for else branch
        else_jump: Option<JumpLabel>,
        /// Jump label to patch for end
        end_jump: Option<JumpLabel>,
        /// Jump label for error in condition (jumps to end)
        error_jump: Option<JumpLabel>,
        parent_tail_position: bool,
        state: IfState,
        cont_id: usize,
    },

    /// Compile let binding - multi-phase with scope management
    CompileLet {
        pattern: MettaValue,
        value: MettaValue,
        body: MettaValue,
        scope_info: Option<ScopeInfo>,
        parent_tail_position: bool,
        state: LetState,
        cont_id: usize,
    },

    /// Compile let* binding - sequential bindings
    CompileLetStar {
        bindings: VecDeque<(MettaValue, MettaValue)>,
        body: MettaValue,
        scope_info: Option<ScopeInfo>,
        parent_tail_position: bool,
        state: LetStarState,
        cont_id: usize,
    },

    /// Compile unify expression - multi-phase with branches
    CompileUnify {
        left: MettaValue,
        right: MettaValue,
        success: MettaValue,
        failure: MettaValue,
        /// Jump label to patch for failure branch
        failure_jump: Option<JumpLabel>,
        /// Jump label to patch for done
        done_jump: Option<JumpLabel>,
        parent_tail_position: bool,
        state: UnifyState,
        cont_id: usize,
    },

    /// Compile case expression - multi-branch pattern matching
    CompileCase {
        scrutinee: MettaValue,
        cases: VecDeque<(MettaValue, MettaValue)>,
        /// Jump labels to patch at the end
        end_jumps: Vec<JumpLabel>,
        parent_tail_position: bool,
        state: CaseState,
        cont_id: usize,
    },

    /// Compile chain expression - sequential binding
    CompileChain {
        expr: MettaValue,
        var: MettaValue,
        body: MettaValue,
        scope_info: Option<ScopeInfo>,
        parent_tail_position: bool,
        state: ChainState,
        cont_id: usize,
    },

    /// Compile superpose - nondeterministic choice
    CompileSuperpose {
        alternatives: VecDeque<MettaValue>,
        state: SuperposeState,
        cont_id: usize,
    },

    /// Compile quoted expression (no evaluation)
    CompileQuoted {
        expr: MettaValue,
        cont_id: usize,
    },

    /// Compile quoted S-expression elements
    CompileQuotedSExprElements {
        items: VecDeque<MettaValue>,
        total_count: usize,
        cont_id: usize,
    },

    /// Compile conjunction (multiple values via Fork)
    CompileConjunction {
        values: VecDeque<MettaValue>,
        state: ConjunctionState,
        cont_id: usize,
    },

    /// Compile pattern binding (creates locals)
    CompilePatternBinding {
        pattern: MettaValue,
        /// For destructuring: which element index we're at
        element_index: usize,
        /// Total elements in destructuring pattern
        total_elements: usize,
        state: PatternBindingState,
        cont_id: usize,
    },

    /// Compile match expression
    CompileMatch {
        space: MettaValue,
        pattern: MettaValue,
        template: MettaValue,
        default: Option<MettaValue>,
        state: MatchState,
        cont_id: usize,
    },

    /// Compile higher-order operation (map-atom, filter-atom, foldl-atom)
    /// Note: Template compilation creates new Compiler instance (natural isolation)
    CompileHigherOrder {
        op: HigherOrderOp,
        list: MettaValue,
        state: HigherOrderState,
        cont_id: usize,
    },

    /// Compile catch expression - error handling
    CompileCatch {
        expr: MettaValue,
        default: MettaValue,
        state: CatchState,
        /// Jump label to patch
        no_error_jump: Option<JumpLabel>,
        /// Jump label to patch
        done_jump: Option<JumpLabel>,
        cont_id: usize,
    },

    /// Compile is-error expression
    CompileIsError {
        expr: MettaValue,
        state: IsErrorState,
        /// Jump labels to patch
        not_error_jump: Option<JumpLabel>,
        done_jump: Option<JumpLabel>,
        cont_id: usize,
    },

    /// Emit an opcode (continuation action)
    EmitOpcode {
        opcode: Opcode,
        cont_id: usize,
    },

    /// Emit opcode with u8 operand
    EmitOpcodeU8 {
        opcode: Opcode,
        operand: u8,
        cont_id: usize,
    },

    /// Emit opcode with u16 operand
    EmitOpcodeU16 {
        opcode: Opcode,
        operand: u16,
        cont_id: usize,
    },

    /// Patch a jump offset
    PatchJump {
        jump_label: JumpLabel,
        cont_id: usize,
    },

    /// Resume continuation
    Resume { cont_id: usize },
}

/// Binary operation types for the compiler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    FloorDiv,
    Log,

    // Comparison
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,

    // Boolean
    And,
    Or,
    Xor,

    // Other
    IndexAtom,
    CheckType,
    ConsAtom,
    SpaceAdd,
    SpaceRemove,
}

impl BinaryOp {
    /// Get the opcode for this binary operation
    pub fn opcode(self) -> Opcode {
        match self {
            BinaryOp::Add => Opcode::Add,
            BinaryOp::Sub => Opcode::Sub,
            BinaryOp::Mul => Opcode::Mul,
            BinaryOp::Div => Opcode::Div,
            BinaryOp::Mod => Opcode::Mod,
            BinaryOp::Pow => Opcode::Pow,
            BinaryOp::FloorDiv => Opcode::FloorDiv,
            BinaryOp::Log => Opcode::Log,
            BinaryOp::Lt => Opcode::Lt,
            BinaryOp::Le => Opcode::Le,
            BinaryOp::Gt => Opcode::Gt,
            BinaryOp::Ge => Opcode::Ge,
            BinaryOp::Eq => Opcode::Eq,
            BinaryOp::Ne => Opcode::Ne,
            BinaryOp::And => Opcode::And,
            BinaryOp::Or => Opcode::Or,
            BinaryOp::Xor => Opcode::Xor,
            BinaryOp::IndexAtom => Opcode::IndexAtom,
            BinaryOp::CheckType => Opcode::CheckType,
            BinaryOp::ConsAtom => Opcode::ConsAtom,
            BinaryOp::SpaceAdd => Opcode::SpaceAdd,
            BinaryOp::SpaceRemove => Opcode::SpaceRemove,
        }
    }

    /// Get the operation name for error messages
    pub fn name(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "pow",
            BinaryOp::FloorDiv => "floor-div",
            BinaryOp::Log => "log-math",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
            BinaryOp::Xor => "xor",
            BinaryOp::IndexAtom => "index-atom",
            BinaryOp::CheckType => "check-type",
            BinaryOp::ConsAtom => "cons-atom",
            BinaryOp::SpaceAdd => "add-atom",
            BinaryOp::SpaceRemove => "remove-atom",
        }
    }
}

/// Unary operation types for the compiler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    // Arithmetic
    Abs,
    Neg,
    Sqrt,
    Trunc,
    Ceil,
    Floor,
    Round,

    // Trigonometric
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,

    // Boolean/check
    Not,
    IsNan,
    IsInf,

    // List operations
    GetHead,
    GetTail,
    GetArity,
    DeconAtom,
    MinAtom,
    MaxAtom,

    // Type operations
    GetType,
    GetMetaType,
    Repr,

    // State operations
    NewState,
    GetState,

    // Space operations
    SpaceGetAtoms,

    // Evaluation
    EvalEval,
    EvalCollapse,
    Trace,
}

impl UnaryOp {
    /// Get the opcode for this unary operation
    pub fn opcode(self) -> Opcode {
        match self {
            UnaryOp::Abs => Opcode::Abs,
            UnaryOp::Neg => Opcode::Neg,
            UnaryOp::Sqrt => Opcode::Sqrt,
            UnaryOp::Trunc => Opcode::Trunc,
            UnaryOp::Ceil => Opcode::Ceil,
            UnaryOp::Floor => Opcode::FloorMath,
            UnaryOp::Round => Opcode::Round,
            UnaryOp::Sin => Opcode::Sin,
            UnaryOp::Cos => Opcode::Cos,
            UnaryOp::Tan => Opcode::Tan,
            UnaryOp::Asin => Opcode::Asin,
            UnaryOp::Acos => Opcode::Acos,
            UnaryOp::Atan => Opcode::Atan,
            UnaryOp::Not => Opcode::Not,
            UnaryOp::IsNan => Opcode::IsNan,
            UnaryOp::IsInf => Opcode::IsInf,
            UnaryOp::GetHead => Opcode::GetHead,
            UnaryOp::GetTail => Opcode::GetTail,
            UnaryOp::GetArity => Opcode::GetArity,
            UnaryOp::DeconAtom => Opcode::DeconAtom,
            UnaryOp::MinAtom => Opcode::MinAtom,
            UnaryOp::MaxAtom => Opcode::MaxAtom,
            UnaryOp::GetType => Opcode::GetType,
            UnaryOp::GetMetaType => Opcode::GetMetaType,
            UnaryOp::Repr => Opcode::Repr,
            UnaryOp::NewState => Opcode::NewState,
            UnaryOp::GetState => Opcode::GetState,
            UnaryOp::SpaceGetAtoms => Opcode::SpaceGetAtoms,
            UnaryOp::EvalEval => Opcode::EvalEval,
            UnaryOp::EvalCollapse => Opcode::EvalCollapse,
            UnaryOp::Trace => Opcode::Trace,
        }
    }

    /// Get the operation name for error messages
    pub fn name(self) -> &'static str {
        match self {
            UnaryOp::Abs => "abs",
            UnaryOp::Neg => "neg",
            UnaryOp::Sqrt => "sqrt-math",
            UnaryOp::Trunc => "trunc-math",
            UnaryOp::Ceil => "ceil-math",
            UnaryOp::Floor => "floor-math",
            UnaryOp::Round => "round-math",
            UnaryOp::Sin => "sin-math",
            UnaryOp::Cos => "cos-math",
            UnaryOp::Tan => "tan-math",
            UnaryOp::Asin => "asin-math",
            UnaryOp::Acos => "acos-math",
            UnaryOp::Atan => "atan-math",
            UnaryOp::Not => "not",
            UnaryOp::IsNan => "isnan-math",
            UnaryOp::IsInf => "isinf-math",
            UnaryOp::GetHead => "car-atom",
            UnaryOp::GetTail => "cdr-atom",
            UnaryOp::GetArity => "size-atom",
            UnaryOp::DeconAtom => "decons-atom",
            UnaryOp::MinAtom => "min-atom",
            UnaryOp::MaxAtom => "max-atom",
            UnaryOp::GetType => "get-type",
            UnaryOp::GetMetaType => "get-metatype",
            UnaryOp::Repr => "repr",
            UnaryOp::NewState => "new-state",
            UnaryOp::GetState => "get-state",
            UnaryOp::SpaceGetAtoms => "get-atoms",
            UnaryOp::EvalEval => "eval",
            UnaryOp::EvalCollapse => "collapse",
            UnaryOp::Trace => "trace!",
        }
    }
}

/// Higher-order operation types
#[derive(Debug, Clone)]
pub enum HigherOrderOp {
    /// map-atom: (map-atom list $var template)
    MapAtom {
        var_name: String,
        template: MettaValue,
    },
    /// filter-atom: (filter-atom list $var predicate)
    FilterAtom {
        var_name: String,
        predicate: MettaValue,
    },
    /// foldl-atom: (foldl-atom list init $acc $item op)
    FoldlAtom {
        init: MettaValue,
        acc_name: String,
        item_name: String,
        op: MettaValue,
    },
}

// ============================================================================
// State machines for multi-phase operations
// ============================================================================

/// State for if/then/else compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfState {
    CompileCondition,
    CompileThen,
    CompileElse,
    Done,
}

/// State for let binding compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetState {
    CompileValue,
    BindPattern,
    CompileBody,
    Cleanup,
    Done,
}

/// State for let* binding compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetStarState {
    CompileNextBinding,
    BindPattern,
    CompileBody,
    Cleanup,
    Done,
}

/// State for unify compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifyState {
    CompileLeft,
    CompileRight,
    EmitUnify,
    CompileSuccess,
    CompileFailure,
    Done,
}

/// State for case compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseState {
    CompileScrutinee,
    CompilingCase { index: usize },
    Done,
}

/// State for chain compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainState {
    CompileExpr,
    BindPattern,
    CompileBody,
    Cleanup,
    Done,
}

/// State for superpose compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperposeState {
    Analyzing,
    Done,
}

/// State for conjunction compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConjunctionState {
    Analyzing,
    Done,
}

/// State for pattern binding compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternBindingState {
    Binding,
    DestructuringElement,
    Done,
}

/// State for match compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchState {
    CompileSpace,
    CompilePattern,
    CompileTemplate,
    CompileDefault,
    EmitMatch,
    Done,
}

/// State for higher-order operation compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HigherOrderState {
    CompileList,
    /// For foldl: compile init expression before template
    CompileFoldlInit,
    CompileTemplate,
    Done,
}

/// State for catch compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchState {
    CompileExpr,
    CompileDefault,
    Done,
}

/// State for is-error compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsErrorState {
    CompileExpr,
    Done,
}

// ============================================================================
// Scope tracking
// ============================================================================

/// Information about scope state for cleanup after body evaluation
#[derive(Debug, Clone)]
pub struct ScopeInfo {
    /// Scope depth when we began
    pub depth: u16,
    /// Number of locals when we began this scope
    pub initial_local_count: u16,
    /// Local variable names declared in this scope
    pub locals_declared: Vec<String>,
}

// ============================================================================
// Continuations
// ============================================================================

/// Continuation representing what to do after a sub-compilation completes.
/// Index 0 is always Done (no more work).
#[derive(Debug)]
pub enum Continuation {
    /// Final result - compilation complete
    Done,

    /// Parent continuation to resume after current work
    Parent { cont_id: usize },
}

impl Default for Continuation {
    fn default() -> Self {
        Continuation::Done
    }
}
