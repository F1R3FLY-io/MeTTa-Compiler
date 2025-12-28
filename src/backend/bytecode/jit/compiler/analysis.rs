//! Bytecode analysis for JIT compilation
//!
//! This module contains functions for analyzing bytecode chunks to determine
//! compilability and extract control flow information.

use std::collections::HashMap;

use crate::backend::bytecode::{BytecodeChunk, Opcode};

use super::BlockInfo;

/// Check if a bytecode chunk can be JIT compiled (Stage 1-5 + Phase A-I)
///
/// Supported features:
/// - Stack ops: Nop, Pop, Dup, Swap, Rot3, Over, DupN, PopN
/// - Arithmetic: Add, Sub, Mul, Div, Mod, Neg, Abs, FloorDiv, Pow (runtime call)
/// - Boolean: And, Or, Not, Xor
/// - Comparisons: Lt, Le, Gt, Ge, Eq, Ne
/// - Constants: PushLongSmall, PushTrue, PushFalse, PushNil, PushConstant (runtime call)
/// - Control: Return, Jump, JumpIfFalse, JumpIfTrue, JumpShort, JumpIfFalseShort, JumpIfTrueShort
/// - Stage 4: Local variables - LoadLocal, StoreLocal, LoadLocalWide, StoreLocalWide
/// - Stage 5: Type jumps - JumpIfNil, JumpIfError
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
            Opcode::PushNil
            | Opcode::PushTrue
            | Opcode::PushFalse
            | Opcode::PushUnit
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
            Opcode::JumpIfNil
            | Opcode::JumpIfError => {}

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

            // NOTE: Fork/Yield/Collect are NOT compilable - they are detected
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
            | Opcode::UnifyBind => {} // Phase B: unify with binding [a, b] -> [bool]

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

            // Phase E: Special Forms (via runtime calls)
            Opcode::EvalIf          // Phase E: if expression [cond, then, else] -> [result]
            | Opcode::EvalLet       // Phase E: let binding [name, value] -> [Unit]
            | Opcode::EvalLetStar   // Phase E: sequential let bindings
            | Opcode::EvalMatch     // Phase E: match expression [value, pattern] -> [bool]
            | Opcode::EvalCase      // Phase E: case expression [value] -> [case_index]
            | Opcode::EvalChain     // Phase E: chain expression [first, second] -> [second]
            | Opcode::EvalQuote     // Phase E: quote expression [expr] -> [quoted]
            | Opcode::EvalUnquote   // Phase E: unquote expression [quoted] -> [result]
            | Opcode::EvalEval      // Phase E: eval expression [expr] -> [result]
            | Opcode::EvalBind      // Phase E: bind expression [name, value] -> [Unit]
            | Opcode::EvalNew       // Phase E: new space [] -> [space]
            | Opcode::EvalCollapse  // Phase E: collapse [expr] -> [list]
            | Opcode::EvalSuperpose // Phase E: superpose [list] -> [choice]
            | Opcode::EvalMemo      // Phase E: memoized eval [expr] -> [result]
            | Opcode::EvalMemoFirst // Phase E: memoize first [expr] -> [result]
            | Opcode::EvalPragma    // Phase E: pragma directive [directive] -> [Unit]
            | Opcode::EvalFunction  // Phase E: function definition [name, params, body] -> [Unit]
            | Opcode::EvalLambda    // Phase E: lambda expression [params, body] -> [closure]
            | Opcode::EvalApply => {} // Phase E: apply closure [closure, args] -> [result]

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
            Opcode::DeconAtom       // Phase 1.7: deconstruct S-expr [expr] -> [(head, tail)]
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

            // Stage 7: Stack operations and Negation (duplicates for completeness)
            // Stage 8: More arithmetic and stack operations (duplicates for completeness)

            // Anything else is not compilable
            _ => return false,
        }

        // Advance by opcode size (1 byte) + operand size
        offset += 1 + op.immediate_size();
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

        let instr_size = 1 + op.immediate_size();
        let next_ip = offset + instr_size; // IP after instruction

        match op {
            Opcode::Jump
            | Opcode::JumpIfFalse
            | Opcode::JumpIfTrue
            | Opcode::JumpIfNil
            | Opcode::JumpIfError => {
                // 2-byte signed offset, relative to next_ip
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
        let instr_size = 1 + op.immediate_size();
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
                | Opcode::JumpIfFalse
                | Opcode::JumpIfTrue
                | Opcode::JumpIfFalseShort
                | Opcode::JumpIfTrueShort
                | Opcode::JumpIfNil
                | Opcode::JumpIfError
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
