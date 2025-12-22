//! Bytecode opcodes for the MeTTa VM
//!
//! This module defines all bytecode instructions used by the VM.
//! Opcodes are grouped by category and assigned contiguous ranges
//! to enable efficient dispatch via computed goto or jump tables.

use std::fmt;

/// Bytecode opcode enumeration
///
/// Each opcode is assigned a unique u8 value. Opcodes are organized into
/// logical groups with reserved ranges for future expansion.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opcode {
    // === Stack Operations (0x00-0x0F) ===
    /// No operation
    Nop = 0x00,
    /// Discard top of stack
    Pop = 0x01,
    /// Duplicate top of stack
    Dup = 0x02,
    /// Swap top two stack elements
    Swap = 0x03,
    /// Rotate top 3: [a,b,c] -> [c,a,b]
    Rot3 = 0x04,
    /// Copy second element: [a,b] -> [a,b,a]
    Over = 0x05,
    /// Duplicate N elements from top of stack
    DupN = 0x06,
    /// Pop N elements from stack
    PopN = 0x07,

    // === Value Creation (0x10-0x2F) ===
    /// Push Nil value
    PushNil = 0x10,
    /// Push Bool(true)
    PushTrue = 0x11,
    /// Push Bool(false)
    PushFalse = 0x12,
    /// Push Unit value
    PushUnit = 0x13,
    /// Push small integer (-128 to 127), value is next byte
    PushLongSmall = 0x14,
    /// Push i64 from constant pool, index is next 2 bytes
    PushLong = 0x15,
    /// Push Symbol from constant pool, index is next 2 bytes
    PushAtom = 0x16,
    /// Push String from constant pool, index is next 2 bytes
    PushString = 0x17,
    /// Push URI from constant pool, index is next 2 bytes
    PushUri = 0x18,
    /// Push generic constant from pool, index is next 2 bytes
    PushConstant = 0x19,
    /// Make S-expression from top N values, N is next byte
    MakeSExpr = 0x1A,
    /// Make S-expression from top N values, N is next 2 bytes
    MakeSExprLarge = 0x1B,
    /// Make proper list from top N values
    MakeList = 0x1C,
    /// Wrap top of stack in Quote
    MakeQuote = 0x1D,
    /// Push empty expression ()
    PushEmpty = 0x1E,
    /// Push Variable from constant pool
    PushVariable = 0x1F,
    /// Cons-atom: prepend head to tail S-expression
    ConsAtom = 0x20,

    // === Variable Operations (0x30-0x3F) ===
    /// Load value from local slot, index is next byte
    LoadLocal = 0x30,
    /// Store value to local slot, index is next byte
    StoreLocal = 0x31,
    /// Load pattern variable by symbol index
    LoadBinding = 0x32,
    /// Store pattern variable by symbol index
    StoreBinding = 0x33,
    /// Load from enclosing scope, depth and index are next 2 bytes
    LoadUpvalue = 0x34,
    /// Check if binding exists, push bool
    HasBinding = 0x35,
    /// Clear all bindings in current frame
    ClearBindings = 0x36,
    /// Create new binding scope
    PushBindingFrame = 0x37,
    /// Exit binding scope
    PopBindingFrame = 0x38,
    /// Load local with 2-byte index for large frames
    LoadLocalWide = 0x39,
    /// Store local with 2-byte index
    StoreLocalWide = 0x3A,

    // === Environment Operations (0x40-0x4F) ===
    /// Load from global space by symbol
    LoadGlobal = 0x40,
    /// Store to global space by symbol
    StoreGlobal = 0x41,
    /// Add rule to environment
    DefineRule = 0x42,
    /// Load space handle by name
    LoadSpace = 0x43,
    /// Add atom to space: [atom, space] -> []
    SpaceAdd = 0x44,
    /// Remove atom from space: [atom, space] -> []
    SpaceRemove = 0x45,
    /// Match pattern against space: [pattern, space] -> [results...]
    SpaceMatch = 0x46,
    /// Get all atoms from space
    SpaceGetAtoms = 0x47,
    /// Create new mutable state cell: [initial_value] -> [state_handle]
    NewState = 0x48,
    /// Get current value from state cell: [state_handle] -> [value]
    GetState = 0x49,
    /// Change state cell value: [state_handle, new_value] -> [old_value]
    ChangeState = 0x4A,

    // === Control Flow (0x50-0x6F) ===
    /// Unconditional jump, offset is next 2 bytes (signed)
    Jump = 0x50,
    /// Jump if top is Bool(false), offset is next 2 bytes
    JumpIfFalse = 0x51,
    /// Jump if top is Bool(true), offset is next 2 bytes
    JumpIfTrue = 0x52,
    /// Jump if top is Nil, offset is next 2 bytes
    JumpIfNil = 0x53,
    /// Jump if top is Error, offset is next 2 bytes
    JumpIfError = 0x54,
    /// Multi-way branch via jump table
    JumpTable = 0x55,
    /// Short unconditional jump, offset is next byte (signed)
    JumpShort = 0x56,
    /// Short conditional jump if false
    JumpIfFalseShort = 0x57,
    /// Short conditional jump if true
    JumpIfTrueShort = 0x58,
    /// Call bytecode function, index is next 2 bytes
    Call = 0x60,
    /// Tail-optimized call
    TailCall = 0x61,
    /// Return from function with top of stack
    Return = 0x62,
    /// Return multiple values (nondeterminism)
    ReturnMulti = 0x63,
    /// Call with N arguments (N is next byte)
    CallN = 0x64,
    /// Tail call with N arguments
    TailCallN = 0x65,
    /// Call native Rust function by ID: <func_id: u16> <arity: u8>
    CallNative = 0x66,
    /// Call external FFI function: <symbol_idx: u16> <arity: u8>
    CallExternal = 0x67,
    /// Call with memoization: <head_idx: u16> <arity: u8>
    CallCached = 0x68,
    /// Ambiguous choice from N alternatives on stack: <count: u8>
    Amb = 0x69,
    /// Guard - backtrack if top of stack is false
    Guard = 0x6A,
    /// Commit - remove N choice points (soft cut): <count: u8>
    Commit = 0x6B,
    /// Force immediate backtracking
    Backtrack = 0x6C,

    // === Pattern Matching (0x70-0x8F) ===
    /// Full pattern match: [pattern, value] -> [bool]
    Match = 0x70,
    /// Match and bind variables: [pattern, value] -> [bool]
    MatchBind = 0x71,
    /// Match just head symbol: [symbol, expr] -> [bool]
    MatchHead = 0x72,
    /// Check arity matches: [expected, expr] -> [bool]
    MatchArity = 0x73,
    /// Evaluate guard expression
    MatchGuard = 0x74,
    /// Bidirectional unification: [a, b] -> [bool]
    Unify = 0x75,
    /// Unify with variable binding: [a, b] -> [bool]
    UnifyBind = 0x76,
    /// Check if value is variable
    IsVariable = 0x77,
    /// Check if value is S-expression
    IsSExpr = 0x78,
    /// Check if value is symbol
    IsSymbol = 0x79,
    /// Get S-expression head
    GetHead = 0x7A,
    /// Get S-expression tail (args)
    GetTail = 0x7B,
    /// Get S-expression arity
    GetArity = 0x7C,
    /// Get S-expression element by index
    GetElement = 0x7D,
    /// Deconstruct S-expression: [expr] -> [(head tail)]
    DeconAtom = 0x7E,
    /// String representation: [value] -> [string]
    Repr = 0x7F,
    /// Map over atoms: [list] -> [mapped_list] (chunk index follows)
    MapAtom = 0x80,
    /// Filter atoms: [list] -> [filtered_list] (chunk index follows)
    FilterAtom = 0x81,
    /// Fold left over atoms: [list, init] -> [result] (chunk index follows)
    FoldlAtom = 0x82,

    // === Rule Dispatch (0x90-0x9F) ===
    /// Find matching rules via MORK
    DispatchRules = 0x90,
    /// Try single rule, handle failure
    TryRule = 0x91,
    /// Advance to next matching rule
    NextRule = 0x92,
    /// Commit to current rule (cut)
    CommitRule = 0x93,
    /// Explicit rule failure
    FailRule = 0x94,
    /// Lookup rules by head symbol
    LookupRules = 0x95,
    /// Apply substitution to expression
    ApplySubst = 0x96,

    // === Special Forms (0xA0-0xBF) ===
    /// Lazy if-then-else
    EvalIf = 0xA0,
    /// Let binding
    EvalLet = 0xA1,
    /// Sequential let*
    EvalLetStar = 0xA2,
    /// Match expression
    EvalMatch = 0xA3,
    /// Case expression
    EvalCase = 0xA4,
    /// Chain/sequence
    EvalChain = 0xA5,
    /// Quote expression (prevent evaluation)
    EvalQuote = 0xA6,
    /// Unquote expression (force in quote context)
    EvalUnquote = 0xA7,
    /// Force evaluation
    EvalEval = 0xA8,
    /// Bind to space
    EvalBind = 0xA9,
    /// Create new space
    EvalNew = 0xAA,
    /// Collapse nondeterminism
    EvalCollapse = 0xAB,
    /// Introduce nondeterminism
    EvalSuperpose = 0xAC,
    /// Memoized evaluation
    EvalMemo = 0xAD,
    /// Memo returning first result only
    EvalMemoFirst = 0xAE,
    /// Pragma/directive
    EvalPragma = 0xAF,
    /// Function definition
    EvalFunction = 0xB0,
    /// Lambda/anonymous function
    EvalLambda = 0xB1,
    /// Apply function to args
    EvalApply = 0xB2,

    // === Grounded Arithmetic (0xC0-0xCF) ===
    /// Addition: [a, b] -> [a + b]
    Add = 0xC0,
    /// Subtraction: [a, b] -> [a - b]
    Sub = 0xC1,
    /// Multiplication: [a, b] -> [a * b]
    Mul = 0xC2,
    /// Division: [a, b] -> [a / b]
    Div = 0xC3,
    /// Modulo: [a, b] -> [a % b]
    Mod = 0xC4,
    /// Negation: [a] -> [-a]
    Neg = 0xC5,
    /// Absolute value: [a] -> [|a|]
    Abs = 0xC6,
    /// Floor division
    FloorDiv = 0xC7,
    /// Power: [a, b] -> [a^b]
    Pow = 0xC8,

    // === Grounded Comparison (0xD0-0xDF) ===
    /// Less than: [a, b] -> [a < b]
    Lt = 0xD0,
    /// Less than or equal: [a, b] -> [a <= b]
    Le = 0xD1,
    /// Greater than: [a, b] -> [a > b]
    Gt = 0xD2,
    /// Greater than or equal: [a, b] -> [a >= b]
    Ge = 0xD3,
    /// Equal: [a, b] -> [a == b]
    Eq = 0xD4,
    /// Not equal: [a, b] -> [a != b]
    Ne = 0xD5,
    /// Structural equality
    StructEq = 0xD6,

    // === Grounded Boolean (0xE0-0xE7) ===
    /// Logical and: [a, b] -> [a && b]
    And = 0xE0,
    /// Logical or: [a, b] -> [a || b]
    Or = 0xE1,
    /// Logical not: [a] -> [!a]
    Not = 0xE2,
    /// Exclusive or: [a, b] -> [a ^ b]
    Xor = 0xE3,

    // === Type Operations (0xE8-0xEF) ===
    /// Get type of value
    GetType = 0xE8,
    /// Check type matches
    CheckType = 0xE9,
    /// Type predicate
    IsType = 0xEA,
    /// Assert type
    AssertType = 0xEB,
    /// Get meta-type of value (Expression, Symbol, Variable, etc.)
    GetMetaType = 0xEC,

    // === Nondeterminism (0xF0-0xF7) ===
    /// Create choice point with alternatives
    Fork = 0xF0,
    /// Backtrack to choice point
    Fail = 0xF1,
    /// Remove choice points (cut)
    Cut = 0xF2,
    /// Collect all results into list
    Collect = 0xF3,
    /// Collect up to N results
    CollectN = 0xF4,
    /// Yield one result, continue for more
    Yield = 0xF5,
    /// Begin nondeterministic section
    BeginNondet = 0xF6,
    /// End nondeterministic section
    EndNondet = 0xF7,

    // === MORK Bridge (0xF8-0xFC) ===
    /// Direct MORK trie lookup
    MorkLookup = 0xF8,
    /// MORK pattern match
    MorkMatch = 0xF9,
    /// Insert into MORK space
    MorkInsert = 0xFA,
    /// Delete from MORK space
    MorkDelete = 0xFB,
    /// Fast bloom filter pre-check
    BloomCheck = 0xFC,

    // === Debug/Meta (0xFD-0xFF) ===
    /// Debugger breakpoint
    Breakpoint = 0xFD,
    /// Emit trace event
    Trace = 0xFE,
    /// Halt execution with error
    Halt = 0xFF,
}

impl Opcode {
    /// Convert byte to opcode, returns None if invalid
    #[inline]
    pub fn from_byte(byte: u8) -> Option<Self> {
        // Use a lookup table for O(1) conversion
        OPCODE_TABLE.get(byte as usize).copied().flatten()
    }

    /// Convert opcode to byte
    #[inline]
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Get the number of immediate bytes following this opcode
    #[inline]
    pub fn immediate_size(self) -> usize {
        match self {
            // No immediate
            Self::Nop | Self::Pop | Self::Dup | Self::Swap | Self::Rot3 | Self::Over
            | Self::PushNil | Self::PushTrue | Self::PushFalse | Self::PushUnit | Self::PushEmpty
            | Self::ClearBindings | Self::PushBindingFrame | Self::PopBindingFrame
            | Self::Return | Self::Match | Self::MatchBind | Self::Unify | Self::UnifyBind
            | Self::IsVariable | Self::IsSExpr | Self::IsSymbol | Self::GetHead | Self::GetTail
            | Self::GetArity | Self::DeconAtom | Self::Repr | Self::GetMetaType
            | Self::ApplySubst | Self::Add | Self::Sub | Self::Mul | Self::Div
            | Self::Mod | Self::Neg | Self::Abs | Self::FloorDiv | Self::Pow
            | Self::Lt | Self::Le | Self::Gt | Self::Ge | Self::Eq | Self::Ne | Self::StructEq
            | Self::And | Self::Or | Self::Not | Self::Xor
            | Self::GetType | Self::CheckType | Self::IsType | Self::AssertType
            | Self::Fail | Self::Cut | Self::Yield | Self::BeginNondet | Self::EndNondet
            | Self::BloomCheck | Self::Breakpoint | Self::Trace | Self::Halt
            | Self::DispatchRules | Self::NextRule | Self::CommitRule | Self::FailRule
            | Self::SpaceAdd | Self::SpaceRemove | Self::SpaceMatch | Self::SpaceGetAtoms
            | Self::NewState | Self::GetState | Self::ChangeState
            | Self::ReturnMulti | Self::MakeQuote
            | Self::EvalIf | Self::EvalLet | Self::EvalLetStar | Self::EvalMatch | Self::EvalCase
            | Self::EvalChain | Self::EvalQuote | Self::EvalUnquote | Self::EvalEval
            | Self::EvalBind | Self::EvalNew | Self::EvalCollapse | Self::EvalSuperpose
            | Self::EvalMemo | Self::EvalMemoFirst | Self::EvalPragma | Self::EvalFunction
            | Self::EvalLambda | Self::EvalApply
            | Self::MorkLookup | Self::MorkMatch | Self::MorkInsert | Self::MorkDelete
            | Self::ConsAtom
            | Self::Guard | Self::Backtrack => 0,

            // 1-byte immediate
            Self::PushLongSmall | Self::LoadLocal | Self::StoreLocal | Self::MakeSExpr
            | Self::Amb | Self::Commit
            | Self::MakeList | Self::DupN | Self::PopN | Self::CallN | Self::TailCallN
            | Self::JumpShort | Self::JumpIfFalseShort | Self::JumpIfTrueShort
            | Self::MatchHead | Self::MatchArity | Self::GetElement | Self::CollectN => 1,

            // 2-byte immediate
            Self::PushLong | Self::PushAtom | Self::PushString | Self::PushUri | Self::PushConstant
            | Self::PushVariable | Self::MakeSExprLarge
            | Self::LoadLocalWide | Self::StoreLocalWide | Self::LoadBinding | Self::StoreBinding
            | Self::LoadUpvalue | Self::HasBinding
            | Self::LoadGlobal | Self::StoreGlobal | Self::DefineRule | Self::LoadSpace
            | Self::Jump | Self::JumpIfFalse | Self::JumpIfTrue | Self::JumpIfNil | Self::JumpIfError
            | Self::JumpTable
            | Self::MatchGuard | Self::TryRule | Self::LookupRules
            | Self::MapAtom | Self::FilterAtom | Self::FoldlAtom
            | Self::Fork | Self::Collect => 2,

            // 3-byte immediate (2-byte head_index + 1-byte arity)
            Self::Call | Self::TailCall | Self::CallNative | Self::CallExternal | Self::CallCached => 3,
        }
    }

    /// Get the mnemonic name for this opcode
    pub fn mnemonic(self) -> &'static str {
        match self {
            Self::Nop => "nop",
            Self::Pop => "pop",
            Self::Dup => "dup",
            Self::Swap => "swap",
            Self::Rot3 => "rot3",
            Self::Over => "over",
            Self::DupN => "dupn",
            Self::PopN => "popn",
            Self::PushNil => "push_nil",
            Self::PushTrue => "push_true",
            Self::PushFalse => "push_false",
            Self::PushUnit => "push_unit",
            Self::PushLongSmall => "push_long_small",
            Self::PushLong => "push_long",
            Self::PushAtom => "push_atom",
            Self::PushString => "push_string",
            Self::PushUri => "push_uri",
            Self::PushConstant => "push_const",
            Self::MakeSExpr => "make_sexpr",
            Self::MakeSExprLarge => "make_sexpr_large",
            Self::MakeList => "make_list",
            Self::MakeQuote => "make_quote",
            Self::PushEmpty => "push_empty",
            Self::PushVariable => "push_var",
            Self::ConsAtom => "cons_atom",
            Self::LoadLocal => "load_local",
            Self::StoreLocal => "store_local",
            Self::LoadBinding => "load_binding",
            Self::StoreBinding => "store_binding",
            Self::LoadUpvalue => "load_upvalue",
            Self::HasBinding => "has_binding",
            Self::ClearBindings => "clear_bindings",
            Self::PushBindingFrame => "push_binding_frame",
            Self::PopBindingFrame => "pop_binding_frame",
            Self::LoadLocalWide => "load_local_wide",
            Self::StoreLocalWide => "store_local_wide",
            Self::LoadGlobal => "load_global",
            Self::StoreGlobal => "store_global",
            Self::DefineRule => "define_rule",
            Self::LoadSpace => "load_space",
            Self::SpaceAdd => "space_add",
            Self::SpaceRemove => "space_remove",
            Self::SpaceMatch => "space_match",
            Self::SpaceGetAtoms => "space_get_atoms",
            Self::NewState => "new_state",
            Self::GetState => "get_state",
            Self::ChangeState => "change_state",
            Self::Jump => "jump",
            Self::JumpIfFalse => "jump_if_false",
            Self::JumpIfTrue => "jump_if_true",
            Self::JumpIfNil => "jump_if_nil",
            Self::JumpIfError => "jump_if_error",
            Self::JumpTable => "jump_table",
            Self::JumpShort => "jump_short",
            Self::JumpIfFalseShort => "jump_if_false_short",
            Self::JumpIfTrueShort => "jump_if_true_short",
            Self::Call => "call",
            Self::TailCall => "tail_call",
            Self::Return => "return",
            Self::ReturnMulti => "return_multi",
            Self::CallN => "call_n",
            Self::TailCallN => "tail_call_n",
            Self::CallNative => "call_native",
            Self::CallExternal => "call_external",
            Self::CallCached => "call_cached",
            Self::Amb => "amb",
            Self::Guard => "guard",
            Self::Commit => "commit",
            Self::Backtrack => "backtrack",
            Self::Match => "match",
            Self::MatchBind => "match_bind",
            Self::MatchHead => "match_head",
            Self::MatchArity => "match_arity",
            Self::MatchGuard => "match_guard",
            Self::Unify => "unify",
            Self::UnifyBind => "unify_bind",
            Self::IsVariable => "is_variable",
            Self::IsSExpr => "is_sexpr",
            Self::IsSymbol => "is_symbol",
            Self::GetHead => "get_head",
            Self::GetTail => "get_tail",
            Self::GetArity => "get_arity",
            Self::GetElement => "get_element",
            Self::DeconAtom => "decon_atom",
            Self::Repr => "repr",
            Self::MapAtom => "map_atom",
            Self::FilterAtom => "filter_atom",
            Self::FoldlAtom => "foldl_atom",
            Self::DispatchRules => "dispatch_rules",
            Self::TryRule => "try_rule",
            Self::NextRule => "next_rule",
            Self::CommitRule => "commit_rule",
            Self::FailRule => "fail_rule",
            Self::LookupRules => "lookup_rules",
            Self::ApplySubst => "apply_subst",
            Self::EvalIf => "eval_if",
            Self::EvalLet => "eval_let",
            Self::EvalLetStar => "eval_let_star",
            Self::EvalMatch => "eval_match",
            Self::EvalCase => "eval_case",
            Self::EvalChain => "eval_chain",
            Self::EvalQuote => "eval_quote",
            Self::EvalUnquote => "eval_unquote",
            Self::EvalEval => "eval_eval",
            Self::EvalBind => "eval_bind",
            Self::EvalNew => "eval_new",
            Self::EvalCollapse => "eval_collapse",
            Self::EvalSuperpose => "eval_superpose",
            Self::EvalMemo => "eval_memo",
            Self::EvalMemoFirst => "eval_memo_first",
            Self::EvalPragma => "eval_pragma",
            Self::EvalFunction => "eval_function",
            Self::EvalLambda => "eval_lambda",
            Self::EvalApply => "eval_apply",
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Mod => "mod",
            Self::Neg => "neg",
            Self::Abs => "abs",
            Self::FloorDiv => "floor_div",
            Self::Pow => "pow",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::StructEq => "struct_eq",
            Self::And => "and",
            Self::Or => "or",
            Self::Not => "not",
            Self::Xor => "xor",
            Self::GetType => "get_type",
            Self::CheckType => "check_type",
            Self::IsType => "is_type",
            Self::AssertType => "assert_type",
            Self::GetMetaType => "get_metatype",
            Self::Fork => "fork",
            Self::Fail => "fail",
            Self::Cut => "cut",
            Self::Collect => "collect",
            Self::CollectN => "collect_n",
            Self::Yield => "yield",
            Self::BeginNondet => "begin_nondet",
            Self::EndNondet => "end_nondet",
            Self::MorkLookup => "mork_lookup",
            Self::MorkMatch => "mork_match",
            Self::MorkInsert => "mork_insert",
            Self::MorkDelete => "mork_delete",
            Self::BloomCheck => "bloom_check",
            Self::Breakpoint => "breakpoint",
            Self::Trace => "trace",
            Self::Halt => "halt",
        }
    }

    /// Check if this opcode is a jump instruction
    #[inline]
    pub fn is_jump(self) -> bool {
        matches!(
            self,
            Self::Jump | Self::JumpIfFalse | Self::JumpIfTrue | Self::JumpIfNil
            | Self::JumpIfError | Self::JumpTable | Self::JumpShort
            | Self::JumpIfFalseShort | Self::JumpIfTrueShort
        )
    }

    /// Check if this opcode is a call instruction
    #[inline]
    pub fn is_call(self) -> bool {
        matches!(self, Self::Call | Self::TailCall | Self::CallN | Self::TailCallN)
    }

    /// Check if this opcode can terminate execution
    #[inline]
    pub fn is_terminator(self) -> bool {
        matches!(self, Self::Return | Self::ReturnMulti | Self::Halt | Self::Fail)
    }

    /// Check if this opcode affects control flow
    #[inline]
    pub fn affects_control_flow(self) -> bool {
        self.is_jump() || self.is_call() || self.is_terminator()
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.mnemonic())
    }
}

/// Lookup table for byte -> Opcode conversion
/// This enables O(1) opcode decoding
static OPCODE_TABLE: [Option<Opcode>; 256] = {
    let mut table = [None; 256];

    // Stack operations
    table[0x00] = Some(Opcode::Nop);
    table[0x01] = Some(Opcode::Pop);
    table[0x02] = Some(Opcode::Dup);
    table[0x03] = Some(Opcode::Swap);
    table[0x04] = Some(Opcode::Rot3);
    table[0x05] = Some(Opcode::Over);
    table[0x06] = Some(Opcode::DupN);
    table[0x07] = Some(Opcode::PopN);

    // Value creation
    table[0x10] = Some(Opcode::PushNil);
    table[0x11] = Some(Opcode::PushTrue);
    table[0x12] = Some(Opcode::PushFalse);
    table[0x13] = Some(Opcode::PushUnit);
    table[0x14] = Some(Opcode::PushLongSmall);
    table[0x15] = Some(Opcode::PushLong);
    table[0x16] = Some(Opcode::PushAtom);
    table[0x17] = Some(Opcode::PushString);
    table[0x18] = Some(Opcode::PushUri);
    table[0x19] = Some(Opcode::PushConstant);
    table[0x1A] = Some(Opcode::MakeSExpr);
    table[0x1B] = Some(Opcode::MakeSExprLarge);
    table[0x1C] = Some(Opcode::MakeList);
    table[0x1D] = Some(Opcode::MakeQuote);
    table[0x1E] = Some(Opcode::PushEmpty);
    table[0x1F] = Some(Opcode::PushVariable);
    table[0x20] = Some(Opcode::ConsAtom);

    // Variable operations
    table[0x30] = Some(Opcode::LoadLocal);
    table[0x31] = Some(Opcode::StoreLocal);
    table[0x32] = Some(Opcode::LoadBinding);
    table[0x33] = Some(Opcode::StoreBinding);
    table[0x34] = Some(Opcode::LoadUpvalue);
    table[0x35] = Some(Opcode::HasBinding);
    table[0x36] = Some(Opcode::ClearBindings);
    table[0x37] = Some(Opcode::PushBindingFrame);
    table[0x38] = Some(Opcode::PopBindingFrame);
    table[0x39] = Some(Opcode::LoadLocalWide);
    table[0x3A] = Some(Opcode::StoreLocalWide);

    // Environment operations
    table[0x40] = Some(Opcode::LoadGlobal);
    table[0x41] = Some(Opcode::StoreGlobal);
    table[0x42] = Some(Opcode::DefineRule);
    table[0x43] = Some(Opcode::LoadSpace);
    table[0x44] = Some(Opcode::SpaceAdd);
    table[0x45] = Some(Opcode::SpaceRemove);
    table[0x46] = Some(Opcode::SpaceMatch);
    table[0x47] = Some(Opcode::SpaceGetAtoms);
    table[0x48] = Some(Opcode::NewState);
    table[0x49] = Some(Opcode::GetState);
    table[0x4A] = Some(Opcode::ChangeState);

    // Control flow
    table[0x50] = Some(Opcode::Jump);
    table[0x51] = Some(Opcode::JumpIfFalse);
    table[0x52] = Some(Opcode::JumpIfTrue);
    table[0x53] = Some(Opcode::JumpIfNil);
    table[0x54] = Some(Opcode::JumpIfError);
    table[0x55] = Some(Opcode::JumpTable);
    table[0x56] = Some(Opcode::JumpShort);
    table[0x57] = Some(Opcode::JumpIfFalseShort);
    table[0x58] = Some(Opcode::JumpIfTrueShort);
    table[0x60] = Some(Opcode::Call);
    table[0x61] = Some(Opcode::TailCall);
    table[0x62] = Some(Opcode::Return);
    table[0x63] = Some(Opcode::ReturnMulti);
    table[0x64] = Some(Opcode::CallN);
    table[0x65] = Some(Opcode::TailCallN);
    table[0x66] = Some(Opcode::CallNative);
    table[0x67] = Some(Opcode::CallExternal);
    table[0x68] = Some(Opcode::CallCached);
    table[0x69] = Some(Opcode::Amb);
    table[0x6A] = Some(Opcode::Guard);
    table[0x6B] = Some(Opcode::Commit);
    table[0x6C] = Some(Opcode::Backtrack);

    // Pattern matching
    table[0x70] = Some(Opcode::Match);
    table[0x71] = Some(Opcode::MatchBind);
    table[0x72] = Some(Opcode::MatchHead);
    table[0x73] = Some(Opcode::MatchArity);
    table[0x74] = Some(Opcode::MatchGuard);
    table[0x75] = Some(Opcode::Unify);
    table[0x76] = Some(Opcode::UnifyBind);
    table[0x77] = Some(Opcode::IsVariable);
    table[0x78] = Some(Opcode::IsSExpr);
    table[0x79] = Some(Opcode::IsSymbol);
    table[0x7A] = Some(Opcode::GetHead);
    table[0x7B] = Some(Opcode::GetTail);
    table[0x7C] = Some(Opcode::GetArity);
    table[0x7D] = Some(Opcode::GetElement);
    table[0x7E] = Some(Opcode::DeconAtom);
    table[0x7F] = Some(Opcode::Repr);
    table[0x80] = Some(Opcode::MapAtom);
    table[0x81] = Some(Opcode::FilterAtom);
    table[0x82] = Some(Opcode::FoldlAtom);

    // Rule dispatch
    table[0x90] = Some(Opcode::DispatchRules);
    table[0x91] = Some(Opcode::TryRule);
    table[0x92] = Some(Opcode::NextRule);
    table[0x93] = Some(Opcode::CommitRule);
    table[0x94] = Some(Opcode::FailRule);
    table[0x95] = Some(Opcode::LookupRules);
    table[0x96] = Some(Opcode::ApplySubst);

    // Special forms
    table[0xA0] = Some(Opcode::EvalIf);
    table[0xA1] = Some(Opcode::EvalLet);
    table[0xA2] = Some(Opcode::EvalLetStar);
    table[0xA3] = Some(Opcode::EvalMatch);
    table[0xA4] = Some(Opcode::EvalCase);
    table[0xA5] = Some(Opcode::EvalChain);
    table[0xA6] = Some(Opcode::EvalQuote);
    table[0xA7] = Some(Opcode::EvalUnquote);
    table[0xA8] = Some(Opcode::EvalEval);
    table[0xA9] = Some(Opcode::EvalBind);
    table[0xAA] = Some(Opcode::EvalNew);
    table[0xAB] = Some(Opcode::EvalCollapse);
    table[0xAC] = Some(Opcode::EvalSuperpose);
    table[0xAD] = Some(Opcode::EvalMemo);
    table[0xAE] = Some(Opcode::EvalMemoFirst);
    table[0xAF] = Some(Opcode::EvalPragma);
    table[0xB0] = Some(Opcode::EvalFunction);
    table[0xB1] = Some(Opcode::EvalLambda);
    table[0xB2] = Some(Opcode::EvalApply);

    // Grounded arithmetic
    table[0xC0] = Some(Opcode::Add);
    table[0xC1] = Some(Opcode::Sub);
    table[0xC2] = Some(Opcode::Mul);
    table[0xC3] = Some(Opcode::Div);
    table[0xC4] = Some(Opcode::Mod);
    table[0xC5] = Some(Opcode::Neg);
    table[0xC6] = Some(Opcode::Abs);
    table[0xC7] = Some(Opcode::FloorDiv);
    table[0xC8] = Some(Opcode::Pow);

    // Grounded comparison
    table[0xD0] = Some(Opcode::Lt);
    table[0xD1] = Some(Opcode::Le);
    table[0xD2] = Some(Opcode::Gt);
    table[0xD3] = Some(Opcode::Ge);
    table[0xD4] = Some(Opcode::Eq);
    table[0xD5] = Some(Opcode::Ne);
    table[0xD6] = Some(Opcode::StructEq);

    // Grounded boolean
    table[0xE0] = Some(Opcode::And);
    table[0xE1] = Some(Opcode::Or);
    table[0xE2] = Some(Opcode::Not);
    table[0xE3] = Some(Opcode::Xor);

    // Type operations
    table[0xE8] = Some(Opcode::GetType);
    table[0xE9] = Some(Opcode::CheckType);
    table[0xEA] = Some(Opcode::IsType);
    table[0xEB] = Some(Opcode::AssertType);
    table[0xEC] = Some(Opcode::GetMetaType);

    // Nondeterminism
    table[0xF0] = Some(Opcode::Fork);
    table[0xF1] = Some(Opcode::Fail);
    table[0xF2] = Some(Opcode::Cut);
    table[0xF3] = Some(Opcode::Collect);
    table[0xF4] = Some(Opcode::CollectN);
    table[0xF5] = Some(Opcode::Yield);
    table[0xF6] = Some(Opcode::BeginNondet);
    table[0xF7] = Some(Opcode::EndNondet);

    // MORK bridge
    table[0xF8] = Some(Opcode::MorkLookup);
    table[0xF9] = Some(Opcode::MorkMatch);
    table[0xFA] = Some(Opcode::MorkInsert);
    table[0xFB] = Some(Opcode::MorkDelete);
    table[0xFC] = Some(Opcode::BloomCheck);

    // Debug/meta
    table[0xFD] = Some(Opcode::Breakpoint);
    table[0xFE] = Some(Opcode::Trace);
    table[0xFF] = Some(Opcode::Halt);

    table
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opcode_roundtrip() {
        // Test that all defined opcodes can be converted to bytes and back
        let opcodes = [
            Opcode::Nop, Opcode::Pop, Opcode::Dup, Opcode::Swap,
            Opcode::PushNil, Opcode::PushTrue, Opcode::PushFalse,
            Opcode::PushLong, Opcode::PushAtom, Opcode::MakeSExpr,
            Opcode::LoadLocal, Opcode::StoreLocal, Opcode::LoadBinding,
            Opcode::Jump, Opcode::JumpIfFalse, Opcode::Call, Opcode::Return,
            Opcode::Match, Opcode::MatchBind, Opcode::Unify,
            Opcode::Add, Opcode::Sub, Opcode::Mul, Opcode::Div,
            Opcode::Lt, Opcode::Le, Opcode::Gt, Opcode::Ge, Opcode::Eq,
            Opcode::And, Opcode::Or, Opcode::Not,
            Opcode::Fork, Opcode::Fail, Opcode::Yield,
            Opcode::Halt,
        ];

        for op in opcodes {
            let byte = op.to_byte();
            let decoded = Opcode::from_byte(byte).expect("Should decode valid opcode");
            assert_eq!(op, decoded, "Opcode {:?} roundtrip failed", op);
        }
    }

    #[test]
    fn test_invalid_opcode() {
        // Test that gaps in the opcode space return None
        assert!(Opcode::from_byte(0x08).is_none()); // Gap in stack ops
        assert!(Opcode::from_byte(0x21).is_none()); // Gap after value creation (0x20 is ConsAtom)
    }

    #[test]
    fn test_immediate_sizes() {
        assert_eq!(Opcode::Nop.immediate_size(), 0);
        assert_eq!(Opcode::PushLongSmall.immediate_size(), 1);
        assert_eq!(Opcode::PushLong.immediate_size(), 2);
        assert_eq!(Opcode::Jump.immediate_size(), 2);
        assert_eq!(Opcode::JumpShort.immediate_size(), 1);
    }

    #[test]
    fn test_opcode_categories() {
        assert!(Opcode::Jump.is_jump());
        assert!(Opcode::JumpIfFalse.is_jump());
        assert!(!Opcode::Call.is_jump());

        assert!(Opcode::Call.is_call());
        assert!(Opcode::TailCall.is_call());
        assert!(!Opcode::Jump.is_call());

        assert!(Opcode::Return.is_terminator());
        assert!(Opcode::Halt.is_terminator());
        assert!(!Opcode::Jump.is_terminator());
    }

    #[test]
    fn test_mnemonic() {
        assert_eq!(Opcode::Nop.mnemonic(), "nop");
        assert_eq!(Opcode::PushLong.mnemonic(), "push_long");
        assert_eq!(Opcode::MakeSExpr.mnemonic(), "make_sexpr");
        assert_eq!(Opcode::JumpIfFalse.mnemonic(), "jump_if_false");
    }
}
