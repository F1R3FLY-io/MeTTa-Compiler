//! Bytecode analysis for JIT compilation
//!
//! This module contains functions for analyzing bytecode chunks to determine
//! compilability and extract control flow information.

use std::collections::HashMap;

use crate::backend::bytecode::instruction::{fork_inline_targets, instruction_size};
use crate::backend::bytecode::{BytecodeChunk, Opcode};

use super::BlockInfo;

/// Check if raw bytecode can be JIT compiled (bytecode-only check).
///
/// This is a low-level function that operates directly on bytecode bytes.
/// It's useful for generic bytecode chunks (like `GenericBytecodeChunk<MettaValue>`)
/// where the bytecode structure is identical regardless of the value type.
///
/// Note: This does NOT check for nondeterminism flags - caller must ensure
/// the chunk doesn't have nondeterministic operations or handle them appropriately.
pub fn can_compile_stage1_bytecode(code: &[u8]) -> bool {
    let mut offset = 0;

    while offset < code.len() {
        let Some(op) = Opcode::from_byte(code[offset]) else {
            return false;
        };

        // Check if opcode is compilable (same logic as can_compile_stage1)
        match op {
            // Stack operations (all Stage 1)
            Opcode::Nop
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::Swap
            | Opcode::Rot3
            | Opcode::Over
            | Opcode::DupN
            | Opcode::PopN => {}

            // Value creation (Stage 1: simple constants, Stage 2+13: via runtime calls)
            Opcode::PushUnit
            | Opcode::PushTrue
            | Opcode::PushFalse
            | Opcode::PushLongSmall
            | Opcode::PushLong
            | Opcode::PushConstant
            | Opcode::PushEmpty
            | Opcode::PushAtom
            | Opcode::PushString
            | Opcode::PushVariable => {}

            // S-expression operations
            Opcode::GetHead
            | Opcode::GetTail
            | Opcode::StructuralHead
            | Opcode::StructuralTail
            | Opcode::GetArity
            | Opcode::GetElement => {}

            // Arithmetic
            Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Div
            | Opcode::Mod
            | Opcode::Neg
            | Opcode::Abs
            | Opcode::FloorDiv
            | Opcode::Pow => {}

            // Extended math operations
            Opcode::Sqrt
            | Opcode::Log
            | Opcode::Trunc
            | Opcode::Ceil
            | Opcode::FloorMath
            | Opcode::Round
            | Opcode::Sin
            | Opcode::Cos
            | Opcode::Tan
            | Opcode::Asin
            | Opcode::Acos
            | Opcode::Atan
            | Opcode::IsNan
            | Opcode::IsInf => {}

            // Expression manipulation
            Opcode::IndexAtom | Opcode::MinAtom | Opcode::MaxAtom => {}

            // Boolean
            Opcode::And | Opcode::Or | Opcode::Not | Opcode::Xor => {}

            // Comparisons
            Opcode::Lt
            | Opcode::Le
            | Opcode::Gt
            | Opcode::Ge
            | Opcode::Eq
            | Opcode::Ne
            | Opcode::StructEq => {}

            // Control
            Opcode::Return
            | Opcode::Jump
            | Opcode::JumpIfFalse
            | Opcode::JumpIfTrue
            | Opcode::JumpShort
            | Opcode::JumpIfFalseShort
            | Opcode::JumpIfTrueShort => {}

            // Local variables
            Opcode::LoadLocal
            | Opcode::StoreLocal
            | Opcode::LoadLocalWide
            | Opcode::StoreLocalWide => {}

            // Type-based jumps
            Opcode::JumpIfUnit | Opcode::JumpIfError | Opcode::JumpIfNotBool => {}

            // Type predicates
            Opcode::IsVariable | Opcode::IsSExpr | Opcode::IsSymbol => {}

            // Type operations
            Opcode::GetType | Opcode::CheckType | Opcode::IsType | Opcode::AssertType => {}

            // Value creation
            Opcode::MakeSExpr
            | Opcode::MakeSExprLarge
            | Opcode::ConsAtom
            | Opcode::PushUri
            | Opcode::MakeList
            | Opcode::MakeQuote => {}

            // Call operations
            Opcode::Call | Opcode::TailCall | Opcode::CallN | Opcode::TailCallN => {}

            // Binding operations
            Opcode::LoadBinding
            | Opcode::StoreBinding
            | Opcode::HasBinding
            | Opcode::ClearBindings
            | Opcode::PushBindingFrame
            | Opcode::PopBindingFrame => {}

            // Pattern matching
            Opcode::Match
            | Opcode::MatchBind
            | Opcode::MatchHead
            | Opcode::MatchArity
            | Opcode::MatchGuard
            | Opcode::Unify
            | Opcode::UnifyBind
            | Opcode::Unify4 => {}

            // Space operations
            Opcode::SpaceAdd | Opcode::SpaceRemove | Opcode::SpaceGetAtoms | Opcode::SpaceMatch => {
            }

            // State operations
            Opcode::NewState | Opcode::GetState | Opcode::ChangeState => {}

            // Rule dispatch
            Opcode::DispatchRules
            | Opcode::TryRule
            | Opcode::NextRule
            | Opcode::CommitRule
            | Opcode::FailRule
            | Opcode::LookupRules
            | Opcode::ApplySubst
            | Opcode::DefineRule => {}

            // Binding-sensitive special forms need provenance projection that
            // currently lives in the tree-walker/VM tiers.
            Opcode::EvalIf
            | Opcode::EvalLet
            | Opcode::EvalLetStar
            | Opcode::EvalCase
            | Opcode::EvalCollapse
            | Opcode::EvalMemo
            | Opcode::EvalMemoFirst => return false,

            // Special forms
            Opcode::EvalMatch
            | Opcode::EvalChain
            | Opcode::EvalQuote
            | Opcode::EvalUnquote
            | Opcode::EvalEval
            | Opcode::EvalBind
            | Opcode::EvalNew
            | Opcode::EvalPragma
            | Opcode::EvalFunction
            | Opcode::EvalLambda
            | Opcode::EvalApply
            | Opcode::CollapseBegin
            | Opcode::CollapseEnd => {}

            // Advanced nondeterminism
            Opcode::Cut | Opcode::Guard | Opcode::Amb | Opcode::Commit | Opcode::Backtrack => {}

            // Advanced calls
            Opcode::CallNative | Opcode::CallExternal | Opcode::CallCached => {}

            // MORK bridge
            Opcode::MorkLookup | Opcode::MorkMatch | Opcode::MorkInsert | Opcode::MorkDelete => {}

            // Debug/Meta
            Opcode::Trace | Opcode::Breakpoint => {}

            // Core nondeterminism markers
            Opcode::Fail | Opcode::BeginNondet | Opcode::EndNondet => {}

            // Multi-value return
            Opcode::ReturnMulti | Opcode::CollectN => {}

            // Multi-way branch
            Opcode::JumpTable => {}

            // Global/Space access
            Opcode::LoadGlobal | Opcode::StoreGlobal | Opcode::LoadSpace => {}

            // Closure support
            Opcode::LoadUpvalue => {}

            // Atom operations
            Opcode::DeconsAtom | Opcode::Repr => {}

            // Higher-order operations
            Opcode::MapAtom | Opcode::FilterAtom | Opcode::FoldlAtom => {}

            // Meta-type
            Opcode::GetMetaType => {}

            // MORK and debug
            Opcode::BloomCheck | Opcode::Halt => {}

            // S1 TOPLEVEL (2026-05-13): HE runner-mode directives
            Opcode::EnterInterpretMode | Opcode::ExitInterpretMode => {}

            // Anything else is not compilable
            _ => return false,
        }

        offset += instruction_size(code, offset);
    }

    true
}

/// Check if a bytecode chunk can be JIT compiled (Stage 1-5 + Phase A-I)
///
/// Supported features:
/// - Stack ops: Nop, Pop, Dup, Swap, Rot3, Over, DupN, PopN
/// - Arithmetic: Add, Sub, Mul, Div, Mod, Neg, Abs, FloorDiv, Pow (runtime call)
/// - Boolean: And, Or, Not, Xor
/// - Comparisons: Lt, Le, Gt, Ge, Eq, Ne
/// - Constants: PushLongSmall, PushTrue, PushFalse, PushUnit, PushConstant (runtime call)
/// - Control: Return, Jump, JumpIfFalse, JumpIfTrue, JumpShort, JumpIfFalseShort, JumpIfTrueShort
/// - Stage 4: Local variables - LoadLocal, StoreLocal, LoadLocalWide, StoreLocalWide
/// - Stage 5: Type jumps - JumpIfUnit, JumpIfError
/// - Stage 6: Type predicates - IsVariable, IsSExpr, IsSymbol
/// - Phase A: Bindings - LoadBinding, StoreBinding, HasBinding, ClearBindings, PushBindingFrame, PopBindingFrame
/// - Phase B: Pattern matching - Match, MatchBind, MatchHead, MatchArity, MatchGuard, Unify, UnifyBind
/// - Phase C: Rule dispatch - DispatchRules, TryRule, NextRule, CommitRule, FailRule, LookupRules, ApplySubst, DefineRule
/// - Phase D: Space operations - SpaceAdd, SpaceRemove, SpaceGetAtoms, SpaceMatch
/// - Phase G: Advanced nondeterminism - Cut
/// - Phase H: MORK bridge - MorkLookup, MorkMatch, MorkInsert, MorkDelete
/// - Phase I: Debug/Meta - Trace, Breakpoint
pub fn can_compile_stage1(chunk: &BytecodeChunk) -> bool {
    // Fast path: reject nondeterministic chunks immediately
    // This avoids wasteful JIT compilation followed by bailout for
    // chunks containing Fork/Yield/Collect/etc.
    if chunk.has_nondeterminism() {
        return false;
    }

    let code = chunk.code();
    let mut offset = 0;

    while offset < code.len() {
        let Some(op) = chunk.read_opcode(offset) else {
            return false;
        };

        match op {
            // Stack operations (all Stage 1)
            Opcode::Nop
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::Swap
            | Opcode::Rot3
            | Opcode::Over
            | Opcode::DupN
            | Opcode::PopN => {}

            // Value creation (Stage 1: simple constants, Stage 2+13: via runtime calls)
            Opcode::PushUnit
            | Opcode::PushTrue
            | Opcode::PushFalse
            | Opcode::PushLongSmall
            | Opcode::PushLong      // Stage 2: large integers via runtime call
            | Opcode::PushConstant  // Stage 2: generic constants via runtime call
            | Opcode::PushEmpty     // Stage 13: empty S-expr via runtime call
            | Opcode::PushAtom      // Stage 13: atom from constant pool via runtime call
            | Opcode::PushString    // Stage 13: string from constant pool via runtime call
            | Opcode::PushVariable => {} // Stage 13: variable from constant pool via runtime call

            // S-expression operations (Stage 14: via runtime calls)
            Opcode::GetHead     // Stage 14: get first element via runtime call
            | Opcode::GetTail   // Stage 14: get all but first via runtime call
            | Opcode::StructuralHead   // Stage 14: car-atom with runtime pre-eval
            | Opcode::StructuralTail   // Stage 14: cdr-atom with runtime pre-eval
            | Opcode::GetArity  // Stage 14: get element count via runtime call
            | Opcode::GetElement => {} // Stage 14b: get element by index via runtime call

            // Arithmetic (Stage 1 + Stage 2 Pow with runtime call)
            Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Div
            | Opcode::Mod
            | Opcode::Neg
            | Opcode::Abs
            | Opcode::FloorDiv
            | Opcode::Pow => {} // Stage 2: Pow uses runtime call

            // Extended math operations (PR #62) - all use runtime calls
            Opcode::Sqrt
            | Opcode::Log
            | Opcode::Trunc
            | Opcode::Ceil
            | Opcode::FloorMath
            | Opcode::Round
            | Opcode::Sin
            | Opcode::Cos
            | Opcode::Tan
            | Opcode::Asin
            | Opcode::Acos
            | Opcode::Atan
            | Opcode::IsNan
            | Opcode::IsInf => {}

            // Expression manipulation (PR #63) - all use runtime calls
            Opcode::IndexAtom
            | Opcode::MinAtom
            | Opcode::MaxAtom => {}

            // Boolean
            Opcode::And | Opcode::Or | Opcode::Not | Opcode::Xor => {}

            // Comparisons
            Opcode::Lt
            | Opcode::Le
            | Opcode::Gt
            | Opcode::Ge
            | Opcode::Eq
            | Opcode::Ne
            | Opcode::StructEq => {}

            // Control (Stage 1: Return, Stage 3: Jumps)
            Opcode::Return => {}

            // Stage 3: Jump instructions
            Opcode::Jump
            | Opcode::JumpIfFalse
            | Opcode::JumpIfTrue
            | Opcode::JumpShort
            | Opcode::JumpIfFalseShort
            | Opcode::JumpIfTrueShort => {}

            // Stage 4: Local variables
            Opcode::LoadLocal
            | Opcode::StoreLocal
            | Opcode::LoadLocalWide
            | Opcode::StoreLocalWide => {}

            // Stage 5: Type-based jumps
            Opcode::JumpIfUnit
            | Opcode::JumpIfError
            | Opcode::JumpIfNotBool => {}

            // Stage 6: Type predicates
            Opcode::IsVariable
            | Opcode::IsSExpr
            | Opcode::IsSymbol => {}

            // Phase 1: Type operations (via runtime calls)
            Opcode::GetType
            | Opcode::CheckType
            | Opcode::IsType => {}

            // Phase J: Type assertion (via runtime call)
            Opcode::AssertType => {}

            // Phase 2a: Value creation (via runtime calls)
            Opcode::MakeSExpr
            | Opcode::MakeSExprLarge
            | Opcode::ConsAtom => {}

            // Phase 2b: More value creation (via runtime calls)
            Opcode::PushUri     // Stage 2b: URI from constant pool (same as PushConstant)
            | Opcode::MakeList  // Stage 2b: proper list (Cons elem (Cons ... Nil))
            | Opcode::MakeQuote => {} // Stage 2b: quote wrapper (quote value)

            // Phase 3: Call/TailCall (bailout to VM for rule dispatch)
            Opcode::Call        // Stage 3: call with bailout
            | Opcode::TailCall  // Stage 3: tail call with bailout
            | Opcode::CallN     // Phase 1.2: call with N args (stack-based head)
            | Opcode::TailCallN => {} // Phase 1.2: tail call with N args (stack-based head)

            // NOTE: Fork/ForkInline/Yield/Collect are NOT compilable - they are detected
            // statically via has_nondeterminism() and routed to bytecode tier.
            // This avoids wasteful JIT compilation followed by immediate bailout.

            // Phase A: Binding operations (via runtime calls)
            Opcode::LoadBinding       // Phase A: load binding by name index
            | Opcode::StoreBinding    // Phase A: store binding by name index
            | Opcode::HasBinding      // Phase A: check if binding exists
            | Opcode::ClearBindings   // Phase A: clear all bindings
            | Opcode::PushBindingFrame  // Phase A: push new binding frame
            | Opcode::PopBindingFrame => {} // Phase A: pop binding frame

            // Phase B: Pattern matching operations (via runtime calls)
            Opcode::Match           // Phase B: pattern match [pattern, value] -> [bool]
            | Opcode::MatchBind     // Phase B: match and bind [pattern, value] -> [bool]
            | Opcode::MatchHead     // Phase B: match head symbol [symbol, expr] -> [bool]
            | Opcode::MatchArity    // Phase B: match arity [expr] -> [bool]
            | Opcode::MatchGuard    // Phase B: match with guard condition
            | Opcode::Unify         // Phase B: unify [a, b] -> [bool]
            | Opcode::UnifyBind     // Phase B: unify with binding [a, b] -> [bool]
            | Opcode::Unify4 => {} // Phase B: 4-arg unify (native JIT path, conditional jump + bindings)

            // Phase D: Space operations (via runtime calls)
            Opcode::SpaceAdd        // Phase D: add atom to space [space, atom] -> [bool]
            | Opcode::SpaceRemove   // Phase D: remove atom from space [space, atom] -> [bool]
            | Opcode::SpaceGetAtoms // Phase D: get all atoms from space [space] -> [list]
            | Opcode::SpaceMatch => {} // Phase D: match pattern in space [space, pattern, template] -> [results]

            // Phase D.1: State operations (via runtime calls)
            Opcode::NewState        // Phase D.1: create state [initial] -> [State(id)]
            | Opcode::GetState      // Phase D.1: get state value [State(id)] -> [value]
            | Opcode::ChangeState => {} // Phase D.1: change state [State(id), value] -> [State(id)]

            // Phase C: Rule dispatch operations (via runtime calls)
            Opcode::DispatchRules   // Phase C: dispatch rules [expr] -> [count]
            | Opcode::TryRule       // Phase C: try single rule [expr] -> [result]
            | Opcode::NextRule      // Phase C: advance to next rule
            | Opcode::CommitRule    // Phase C: commit to current rule (cut)
            | Opcode::FailRule      // Phase C: signal rule failure
            | Opcode::LookupRules   // Phase C: look up rules by head [head_idx] -> [count]
            | Opcode::ApplySubst    // Phase C: apply substitution [expr] -> [result]
            | Opcode::DefineRule => {} // Phase C: define new rule [pattern, body] -> [Unit]

            // Phase E binding-sensitive forms fall back until JIT has native
            // provenance projection equivalent to tree-walker/VM.
            Opcode::EvalIf
            | Opcode::EvalLet
            | Opcode::EvalLetStar
            | Opcode::EvalCase
            | Opcode::EvalCollapse
            | Opcode::EvalMemo
            | Opcode::EvalMemoFirst => return false,

            // Phase E: Special Forms (via runtime calls)
            Opcode::EvalMatch       // Phase E: match expression [value, pattern] -> [bool]
            | Opcode::EvalChain     // Phase E: chain expression [first, second] -> [second]
            | Opcode::EvalQuote     // Phase E: quote expression [expr] -> [quoted]
            | Opcode::EvalUnquote   // Phase E: unquote expression [quoted] -> [result]
            | Opcode::EvalEval      // Phase E: eval expression [expr] -> [result]
            | Opcode::EvalBind      // Phase E: bind expression [name, value] -> [Unit]
            | Opcode::EvalNew       // Phase E: new space [] -> [space]
            | Opcode::EvalPragma    // Phase E: pragma directive [directive] -> [Unit]
            | Opcode::EvalFunction  // Phase E: function definition [name, params, body] -> [Unit]
            | Opcode::EvalLambda    // Phase E: lambda expression [params, body] -> [closure]
            | Opcode::EvalApply     // Phase E: apply closure [closure, args] -> [result]
            | Opcode::CollapseBegin // Native collapse: nondeterminism sandboxing
            | Opcode::CollapseEnd => {} // Native collapse: collect results

            // Phase G: Advanced Nondeterminism (via runtime calls)
            Opcode::Cut               // Phase G: prune search space
            | Opcode::Guard           // Phase G: guard condition [bool] -> [] (backtrack if false)
            | Opcode::Amb             // Phase G: amb choice [alts...] -> [selected]
            | Opcode::Commit          // Phase G: commit (soft cut) [] -> [Unit]
            | Opcode::Backtrack => {} // Phase G: force backtracking [] -> []

            // Phase F: Advanced Calls (via runtime calls)
            Opcode::CallNative        // Phase F: call native function [args...] -> [result]
            | Opcode::CallExternal    // Phase F: call external function [args...] -> [result]
            | Opcode::CallCached => {} // Phase F: cached function call [args...] -> [result]

            // Phase H: MORK Bridge (via runtime calls)
            Opcode::MorkLookup      // Phase H: lookup in MORK [path] -> [value]
            | Opcode::MorkMatch     // Phase H: match pattern in MORK [path, pattern] -> [results]
            | Opcode::MorkInsert    // Phase H: insert into MORK [path, value] -> [bool]
            | Opcode::MorkDelete => {} // Phase H: delete from MORK [path] -> [bool]

            // Phase I: Debug/Meta (via runtime calls)
            Opcode::Trace           // Phase I: emit trace event [msg_idx, value] -> []
            | Opcode::Breakpoint => {} // Phase I: debugger breakpoint [bp_id] -> []

            // Phase 1.1: Core Nondeterminism Markers (native or runtime calls)
            Opcode::Fail            // Phase 1.1: explicit failure (return FAIL signal)
            | Opcode::BeginNondet   // Phase 1.1: mark start of nondet section
            | Opcode::EndNondet => {} // Phase 1.1: mark end of nondet section

            // Phase 1.3: Multi-value Return (via runtime calls)
            Opcode::ReturnMulti     // Phase 1.3: return multiple values [count] -> signal
            | Opcode::CollectN => {} // Phase 1.3: collect up to N results [] -> [sexpr]

            // Phase 1.4: Multi-way Branch (native jump table)
            Opcode::JumpTable => {} // Phase 1.4: switch/case dispatch [index] -> []

            // Phase 1.5: Global/Space Access (via runtime calls)
            Opcode::LoadGlobal      // Phase 1.5: load global variable [symbol_idx] -> [value]
            | Opcode::StoreGlobal   // Phase 1.5: store global variable [symbol_idx, value] -> [unit]
            | Opcode::LoadSpace => {} // Phase 1.5: load space handle [name_idx] -> [space]

            // Phase 1.6: Closure Support (via runtime calls)
            Opcode::LoadUpvalue => {} // Phase 1.6: load from enclosing scope [depth, index] -> [value]

            // Phase 1.7: Atom Operations (via runtime calls)
            Opcode::DeconsAtom       // Phase 1.7: deconstruct S-expr [expr] -> [(head, tail)]
            | Opcode::Repr => {}    // Phase 1.7: string representation [value] -> [string]

            // Phase 1.8: Higher-Order Operations (via runtime calls, may bailout)
            Opcode::MapAtom         // Phase 1.8: map function over list [list, func] -> [result]
            | Opcode::FilterAtom    // Phase 1.8: filter list by predicate [list, pred] -> [result]
            | Opcode::FoldlAtom => {} // Phase 1.8: left fold over list [list, init, func] -> [result]

            // Phase 1.9: Meta-Type Operations (via runtime calls)
            Opcode::GetMetaType => {} // Phase 1.9: get meta-level type [value] -> [metatype]

            // Phase 1.10: MORK and Debug (via runtime calls)
            Opcode::BloomCheck      // Phase 1.10: bloom filter pre-check [key] -> [bool]
            | Opcode::Halt => {}    // Phase 1.10: halt execution (return HALT signal)

            // S1 TOPLEVEL (2026-05-13): HE runner-mode directives.
            // Lowered to a 1-byte store-immediate-to-context-field; no value-
            // stack effect. See compile_enter_interpret_mode / _exit handlers.
            Opcode::EnterInterpretMode | Opcode::ExitInterpretMode => {}

            // Stage 7: Stack operations and Negation (duplicates for completeness)
            // Stage 8: More arithmetic and stack operations (duplicates for completeness)

            // Anything else is not compilable
            _ => return false,
        }

        offset += instruction_size(code, offset);
    }

    true
}

/// Pre-scan bytecode to find all jump targets and their predecessor counts
///
/// Jump offsets are relative to the IP after reading the instruction and its operands.
/// For example, if a Jump is at offset 6 with size 3 (1 opcode + 2 operand bytes),
/// then the offset is relative to position 9 (6 + 3).

pub(super) fn find_block_info(chunk: &BytecodeChunk) -> BlockInfo {
    let code = chunk.code();
    let mut targets = Vec::new();
    let mut predecessor_count: HashMap<usize, usize> = HashMap::new();
    let mut offset = 0;

    // Helper function to add a target
    fn add_target(
        target: usize,
        code_len: usize,
        targets: &mut Vec<usize>,
        predecessor_count: &mut HashMap<usize, usize>,
    ) {
        if target <= code_len {
            if !targets.contains(&target) {
                targets.push(target);
            }
            *predecessor_count.entry(target).or_insert(0) += 1;
        }
    }

    while offset < code.len() {
        let Some(op) = chunk.read_opcode(offset) else {
            break;
        };

        let instr_size = instruction_size(code, offset);
        let next_ip = offset + instr_size; // IP after instruction

        match op {
            Opcode::ForkInline => {
                for target in fork_inline_targets(code, offset) {
                    add_target(target, code.len(), &mut targets, &mut predecessor_count);
                }
            }
            Opcode::Jump
            | Opcode::JumpIfFalse
            | Opcode::JumpIfTrue
            | Opcode::JumpIfUnit
            | Opcode::JumpIfError
            | Opcode::JumpIfNotBool
            | Opcode::Unify4
            | Opcode::UnifyDeep
            | Opcode::UnifyDeepBind => {
                // 2-byte signed offset, relative to next_ip.
                // Unify4 / UnifyDeep / UnifyDeepBind behave as conditional jumps:
                // fall through on success, jump to fail-offset on failure.
                let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                let target = (next_ip as isize + rel_offset as isize) as usize;
                add_target(target, code.len(), &mut targets, &mut predecessor_count);
                // For conditional jumps, the fallthrough is also a target
                if op != Opcode::Jump && next_ip < code.len() {
                    add_target(next_ip, code.len(), &mut targets, &mut predecessor_count);
                }
            }
            Opcode::JumpShort | Opcode::JumpIfFalseShort | Opcode::JumpIfTrueShort => {
                // 1-byte signed offset, relative to next_ip
                let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
                let target = (next_ip as isize + rel_offset as isize) as usize;
                add_target(target, code.len(), &mut targets, &mut predecessor_count);
                // For conditional jumps, the fallthrough is also a target
                if op != Opcode::JumpShort && next_ip < code.len() {
                    add_target(next_ip, code.len(), &mut targets, &mut predecessor_count);
                }
            }
            Opcode::JumpTable => {
                // JumpTable: table_index:u16
                // Read table index and add all targets from the table
                let table_index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;
                if let Some(jump_table) = chunk.get_jump_table(table_index) {
                    // Add all entry targets
                    for &(_hash, target) in &jump_table.entries {
                        add_target(target, code.len(), &mut targets, &mut predecessor_count);
                    }
                    // Add default target
                    add_target(
                        jump_table.default_offset,
                        code.len(),
                        &mut targets,
                        &mut predecessor_count,
                    );
                }
            }
            Opcode::Return => {
                // Return doesn't have a target
            }
            _ => {}
        }

        offset += instr_size;
    }

    // Second pass: count fallthroughs for blocks that aren't jump targets
    // but come after non-terminating instructions
    offset = 0;
    while offset < code.len() {
        let Some(op) = chunk.read_opcode(offset) else {
            break;
        };
        let instr_size = instruction_size(code, offset);
        let next_ip = offset + instr_size;

        // Instructions that don't fall through to next_ip
        // - Terminating: Return, Jump, JumpShort, JumpTable
        // - Conditional jumps: their fallthrough is already counted in first pass
        let has_fallthrough_to_next = !matches!(
            op,
            Opcode::Return
                | Opcode::Jump
                | Opcode::JumpShort
                | Opcode::JumpTable
                | Opcode::ForkInline
                | Opcode::JumpIfFalse
                | Opcode::JumpIfTrue
                | Opcode::JumpIfFalseShort
                | Opcode::JumpIfTrueShort
                | Opcode::JumpIfUnit
                | Opcode::JumpIfError
                | Opcode::JumpIfNotBool
                | Opcode::Unify4
                | Opcode::UnifyDeep
                | Opcode::UnifyDeepBind
        );
        if has_fallthrough_to_next && next_ip < code.len() && targets.contains(&next_ip) {
            // This is a fallthrough edge
            *predecessor_count.entry(next_ip).or_insert(0) += 1;
        }

        offset += instr_size;
    }

    targets.sort();
    BlockInfo {
        targets,
        predecessor_count,
    }
}
