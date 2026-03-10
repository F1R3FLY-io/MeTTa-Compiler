//! DFA transition and accept tables for peephole pattern matching.
//!
//! Tables are built once at process start (via `OnceLock`) from the declarative
//! pattern definitions. The DFA construction pipeline is:
//!   PatternDefs → NFA → EquivClasses → DFA (subset construction) → Minimized DFA
//!
//! The resulting tables fit in L1 cache (~3 KB) and enable branch-free O(max_pattern_len)
//! matching per bytecode position.

use std::sync::OnceLock;

use super::equiv_classes::compute_equiv_classes;
use super::minimize::minimize;
use super::nfa::build_nfa;
use super::pattern_defs::{all_pattern_defs, is_numeric_producer, PatternAction, PatternDef, PostCondition, StatKind};
use super::subset::{subset_construction, DEAD};
use super::types::{OptimizationStats, PeepholeAction};

use crate::backend::bytecode::opcodes::Opcode;

/// Maximum pattern length across all pattern definitions.
const MAX_PATTERN_LEN: usize = 5;

/// Pre-built DFA tables for peephole matching.
struct DfaTables {
    /// Maps each byte (0-255) to its equivalence class.
    byte_to_class: Vec<u8>,
    /// DFA transition table: `transition[state][class]` → next state (or `DEAD`).
    transition: Vec<Vec<u16>>,
    /// Accept table: `accept[state]` → `Some(pattern_id)` or `None`.
    accept: Vec<Option<u16>>,
    /// Pattern rules indexed by pattern_id.
    patterns: Vec<PatternDef>,
}

/// Global DFA tables, built once on first use.
static DFA: OnceLock<DfaTables> = OnceLock::new();

fn build_dfa_tables() -> DfaTables {
    let patterns = all_pattern_defs();
    let nfa = build_nfa(&patterns);
    let classes = compute_equiv_classes(&nfa);
    let dfa = subset_construction(&nfa, &classes);
    let min_dfa = minimize(&dfa);

    let transition: Vec<Vec<u16>> = min_dfa.states.iter().map(|s| s.transitions.clone()).collect();
    let accept: Vec<Option<u16>> = min_dfa.states.iter().map(|s| s.accept).collect();

    DfaTables {
        byte_to_class: classes.byte_to_class.to_vec(),
        transition,
        accept,
        patterns,
    }
}

fn get_tables() -> &'static DfaTables {
    DFA.get_or_init(build_dfa_tables)
}

/// Scan for a peephole optimization pattern at the given offset using the DFA.
///
/// The DFA matches the structural byte sequence in a single branch-free pass
/// (array lookups only). Post-conditions (guards, equality constraints) are
/// checked only when the DFA reaches an accept state.
///
/// Returns a `PeepholeAction` and updates the stats counter for the matching pattern.
pub fn dfa_scan_pattern(
    code: &[u8],
    offset: usize,
    prev_opcode: Option<Opcode>,
    stats: &mut OptimizationStats,
) -> PeepholeAction {
    let tables = get_tables();
    let remaining = code.len() - offset;

    if remaining == 0 {
        return PeepholeAction::Keep;
    }

    let mut state: u16 = 0; // start state
    let mut last_accept: Option<(u16, usize)> = None; // (pattern_id, match_end)

    let scan_len = remaining.min(MAX_PATTERN_LEN);
    for i in 0..scan_len {
        let byte = code[offset + i];
        let class = tables.byte_to_class[byte as usize];
        let next = tables.transition[state as usize][class as usize];
        if next == DEAD {
            break;
        }
        state = next;
        if let Some(pattern_id) = tables.accept[state as usize] {
            last_accept = Some((pattern_id, offset + i + 1));
        }
    }

    let (pattern_id, match_end) = match last_accept {
        None => return PeepholeAction::Keep,
        Some(x) => x,
    };

    let pattern = &tables.patterns[pattern_id as usize];

    // Check post-conditions
    if !check_postconditions(pattern, code, offset, prev_opcode) {
        return PeepholeAction::Keep;
    }

    // Increment stats
    increment_stat(stats, pattern.stat);

    // Build the peephole action from the pattern
    build_action(pattern, code, offset, match_end)
}

/// Check all post-conditions for a pattern.
fn check_postconditions(
    pattern: &PatternDef,
    code: &[u8],
    offset: usize,
    prev_opcode: Option<Opcode>,
) -> bool {
    for pc in pattern.postconditions {
        match pc {
            PostCondition::BytesEqual(a, b) => {
                let pos_a = offset + *a as usize;
                let pos_b = offset + *b as usize;
                if pos_a >= code.len() || pos_b >= code.len() {
                    return false;
                }
                if code[pos_a] != code[pos_b] {
                    return false;
                }
            }
            PostCondition::NumericProducerGuard => {
                if !is_numeric_producer(prev_opcode) {
                    return false;
                }
            }
        }
    }
    true
}

/// Build a `PeepholeAction` from a matched pattern.
fn build_action(
    pattern: &PatternDef,
    code: &[u8],
    offset: usize,
    match_end: usize,
) -> PeepholeAction {
    match &pattern.action {
        PatternAction::Remove => PeepholeAction::Remove {
            start: offset,
            end: match_end,
        },
        PatternAction::RemoveFirst(n) => PeepholeAction::Remove {
            start: offset,
            end: offset + n,
        },
        PatternAction::ReplaceOpcode(opcode) => PeepholeAction::ReplaceWithOpcode {
            start: offset,
            end: match_end,
            opcode: *opcode,
        },
        PatternAction::ReplaceBytes(bytes) => PeepholeAction::ReplaceWithBytes {
            start: offset,
            end: match_end,
            bytes: bytes.to_vec(),
        },
        PatternAction::ReplaceBytesWithCapture { template, captures } => {
            let mut result = template.to_vec();
            for &(template_pos, source_offset) in *captures {
                let src_pos = offset + source_offset;
                if src_pos < code.len() && template_pos < result.len() {
                    result[template_pos] = code[src_pos];
                }
            }
            // Fill in opcode bytes from the pattern action context
            // The template has placeholder zeros that need to be filled with actual opcode bytes
            fill_template_opcodes(&mut result, pattern, code, offset);
            PeepholeAction::ReplaceWithBytes {
                start: offset,
                end: match_end,
                bytes: result,
            }
        }
        PatternAction::Custom(f) => match f(code, offset, match_end) {
            Some(bytes) => PeepholeAction::ReplaceWithBytes {
                start: offset,
                end: match_end,
                bytes,
            },
            None => PeepholeAction::Keep,
        },
    }
}

/// Fill template opcode bytes based on the pattern's stat kind.
///
/// For `ReplaceBytesWithCapture`, the template has zero placeholders for opcodes
/// that depend on the pattern. This function fills them in.
fn fill_template_opcodes(
    result: &mut [u8],
    pattern: &PatternDef,
    _code: &[u8],
    _offset: usize,
) {
    match pattern.stat {
        StatKind::ComparisonBranchFused => {
            // Template: [InvCmp, JumpIfXxx, hi, lo]
            // Only invert the comparison; KEEP the jump type unchanged.
            // Semantics: Cmp; Not; JumpIfFalse = jump when Cmp is True
            //          → InvCmp; JumpIfFalse = jump when InvCmp is False = when Cmp is True ✓
            let first_byte = pattern.bytes[0];
            let third_byte = pattern.bytes[2];

            if let super::pattern_defs::ByteMatch::Exact(cmp_byte) = first_byte {
                if let super::pattern_defs::ByteMatch::Exact(jump_byte) = third_byte {
                    let inv_cmp = invert_comparison(cmp_byte);
                    result[0] = inv_cmp;
                    result[1] = jump_byte; // keep jump type unchanged
                }
            }
        }
        StatKind::BranchInverted => {
            // Template: [JumpIfXxx, hi, lo]
            // Invert the jump type
            let second_byte = pattern.bytes[1];
            if let super::pattern_defs::ByteMatch::Exact(jump_byte) = second_byte {
                result[0] = invert_jump(jump_byte);
            }
        }
        StatKind::ConstBranchFolded => {
            // Template: [Jump, hi, lo]
            result[0] = Opcode::Jump.to_byte();
        }
        StatKind::LoadDeduplicated => {
            // Template: [LoadLocal, slot, Dup]
            result[0] = Opcode::LoadLocal.to_byte();
            result[2] = Opcode::Dup.to_byte();
        }
        StatKind::StoreLoadFolded => {
            // Template: [Dup, StoreLocal, slot]
            // Already filled by const values in pattern_defs.rs
        }
        StatKind::OverDupFolded => {
            // Template: [Over, Dup] — already filled by const values
        }
        _ => {
            // No opcode filling needed for other patterns
        }
    }
}

/// Invert a comparison opcode (Lt ↔ Ge, Le ↔ Gt, Eq ↔ Ne).
fn invert_comparison(cmp: u8) -> u8 {
    let lt = Opcode::Lt.to_byte();
    let le = Opcode::Le.to_byte();
    let gt = Opcode::Gt.to_byte();
    let ge = Opcode::Ge.to_byte();
    let eq = Opcode::Eq.to_byte();
    let ne = Opcode::Ne.to_byte();

    if cmp == lt { ge }
    else if cmp == le { gt }
    else if cmp == gt { le }
    else if cmp == ge { lt }
    else if cmp == eq { ne }
    else if cmp == ne { eq }
    else { cmp }
}

/// Invert a conditional jump opcode (JumpIfTrue ↔ JumpIfFalse).
fn invert_jump(jump: u8) -> u8 {
    let jt = Opcode::JumpIfTrue.to_byte();
    let jf = Opcode::JumpIfFalse.to_byte();

    if jump == jt { jf }
    else if jump == jf { jt }
    else { jump }
}

/// Increment the appropriate stat counter.
fn increment_stat(stats: &mut OptimizationStats, stat: StatKind) {
    match stat {
        StatKind::NopsRemoved => stats.nops_removed += 1,
        StatKind::SwapSwapRemoved => stats.swap_swap_removed += 1,
        StatKind::DupPopRemoved => stats.dup_pop_removed += 1,
        StatKind::NotNotRemoved => stats.not_not_removed += 1,
        StatKind::ConstNotFolded => stats.const_not_folded += 1,
        StatKind::PushPopRemoved => stats.push_pop_removed += 1,
        StatKind::NegNegRemoved => stats.neg_neg_removed += 1,
        StatKind::IdempotentRemoved => stats.idempotent_removed += 1,
        StatKind::ComparisonFolded => stats.comparison_folded += 1,
        StatKind::IdentityOpsRemoved => stats.identity_ops_removed += 1,
        StatKind::MulZeroFolded => stats.mul_zero_folded += 1,
        StatKind::PowFolded => stats.pow_folded += 1,
        StatKind::ConstBranchFolded => stats.const_branch_folded += 1,
        StatKind::DeadBranchRemoved => stats.dead_branch_removed += 1,
        StatKind::LoadDeduplicated => stats.load_deduplicated += 1,
        StatKind::BranchInverted | StatKind::ComparisonBranchFused => {
            stats.comparison_folded += 1;
        }
        StatKind::OverDupFolded | StatKind::PopFused | StatKind::BuildDeconstructFolded
        | StatKind::StoreLoadFolded => {
            stats.idempotent_removed += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::opcodes::Opcode;

    #[test]
    fn test_dfa_tables_build() {
        let tables = get_tables();
        assert!(tables.transition.len() > 0, "DFA should have at least one state");
        assert!(!tables.patterns.is_empty());
        assert_eq!(tables.byte_to_class.len(), 256);
    }

    #[test]
    fn test_dfa_nop_removal() {
        let code = vec![Opcode::Nop.to_byte(), Opcode::Return.to_byte()];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        assert!(matches!(action, PeepholeAction::Remove { start: 0, end: 1 }));
        assert_eq!(stats.nops_removed, 1);
    }

    #[test]
    fn test_dfa_swap_swap() {
        let code = vec![
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Return.to_byte(),
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        assert!(matches!(action, PeepholeAction::Remove { start: 0, end: 2 }));
        assert_eq!(stats.swap_swap_removed, 1);
    }

    #[test]
    fn test_dfa_push_true_not() {
        let code = vec![
            Opcode::PushTrue.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        assert!(matches!(
            action,
            PeepholeAction::ReplaceWithOpcode {
                start: 0,
                end: 2,
                opcode: Opcode::PushFalse
            }
        ));
        assert_eq!(stats.const_not_folded, 1);
    }

    #[test]
    fn test_dfa_numeric_guard() {
        // Without numeric producer guard, PushLongSmall 0; Add should NOT be removed
        let code = vec![
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(),
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        // Should not match identity pattern (no numeric producer)
        // but should match PushLongSmall X; Pop? No, no Pop here.
        // Without a guard, it should be Keep
        assert!(
            matches!(action, PeepholeAction::Keep),
            "Should not optimize without numeric producer guard"
        );

        // With numeric producer guard, should be removed
        let mut stats2 = OptimizationStats::new();
        let action2 =
            dfa_scan_pattern(&code, 0, Some(Opcode::PushLongSmall), &mut stats2);
        assert!(
            matches!(action2, PeepholeAction::Remove { .. }),
            "Should optimize with numeric producer guard"
        );
    }

    #[test]
    fn test_dfa_comparison_fold() {
        let code = vec![
            Opcode::Lt.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        assert!(matches!(
            action,
            PeepholeAction::ReplaceWithOpcode {
                start: 0,
                end: 2,
                opcode: Opcode::Ge
            }
        ));
    }

    #[test]
    fn test_dfa_const_branch_fold() {
        let code = vec![
            Opcode::PushTrue.to_byte(),
            Opcode::JumpIfTrue.to_byte(),
            0x00,
            0x05,
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        match action {
            PeepholeAction::ReplaceWithBytes { start, end, bytes } => {
                assert_eq!(start, 0);
                assert_eq!(end, 4);
                assert_eq!(bytes[0], Opcode::Jump.to_byte());
                assert_eq!(bytes[1], 0x00);
                assert_eq!(bytes[2], 0x05);
            }
            _ => panic!("Expected ReplaceWithBytes, got {:?}", action),
        }
    }

    #[test]
    fn test_dfa_load_dedup() {
        let code = vec![
            Opcode::LoadLocal.to_byte(),
            5, // slot 5
            Opcode::LoadLocal.to_byte(),
            5, // same slot
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        match action {
            PeepholeAction::ReplaceWithBytes { start, end, bytes } => {
                assert_eq!(start, 0);
                assert_eq!(end, 4);
                assert_eq!(bytes[0], Opcode::LoadLocal.to_byte());
                assert_eq!(bytes[1], 5);
                assert_eq!(bytes[2], Opcode::Dup.to_byte());
            }
            _ => panic!("Expected ReplaceWithBytes for load dedup, got {:?}", action),
        }
    }

    #[test]
    fn test_dfa_load_dedup_different_slots() {
        let code = vec![
            Opcode::LoadLocal.to_byte(),
            5,
            Opcode::LoadLocal.to_byte(),
            7, // different slot
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        // Should NOT match because slots differ (post-condition fails)
        assert!(
            matches!(action, PeepholeAction::Keep),
            "Should not dedup loads from different slots"
        );
    }

    #[test]
    fn test_dfa_longest_match_comparison_branch() {
        // Lt; Not; JumpIfFalse should match the 5-byte pattern (comparison branch fused)
        // NOT the 2-byte pattern (Lt; Not → Ge)
        let code = vec![
            Opcode::Lt.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::JumpIfFalse.to_byte(),
            0x00,
            0x05,
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        match action {
            PeepholeAction::ReplaceWithBytes { start, end, bytes } => {
                assert_eq!(start, 0);
                assert_eq!(end, 5); // 5-byte match, not 2
                assert_eq!(bytes[0], Opcode::Ge.to_byte()); // inverted comparison
                assert_eq!(bytes[1], Opcode::JumpIfFalse.to_byte()); // jump type preserved
                assert_eq!(bytes[2], 0x00);
                assert_eq!(bytes[3], 0x05);
            }
            _ => panic!(
                "Expected 5-byte comparison+branch fusion, got {:?}",
                action
            ),
        }
    }

    #[test]
    fn test_dfa_not_jump_if_false_inversion() {
        let code = vec![
            Opcode::Not.to_byte(),
            Opcode::JumpIfFalse.to_byte(),
            0x00,
            0x03,
        ];
        let mut stats = OptimizationStats::new();
        let action = dfa_scan_pattern(&code, 0, None, &mut stats);
        match action {
            PeepholeAction::ReplaceWithBytes { start, end, bytes } => {
                assert_eq!(start, 0);
                assert_eq!(end, 4);
                assert_eq!(bytes[0], Opcode::JumpIfTrue.to_byte());
                assert_eq!(bytes[1], 0x00);
                assert_eq!(bytes[2], 0x03);
            }
            _ => panic!("Expected branch inversion, got {:?}", action),
        }
    }
}
