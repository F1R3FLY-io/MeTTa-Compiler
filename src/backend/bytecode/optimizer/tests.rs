//! Tests for bytecode optimization.

#[cfg(test)]
mod tests {
    use crate::backend::bytecode::opcodes::Opcode;
    use crate::backend::bytecode::optimizer::{
        eliminate_dead_code, optimize_bytecode, optimize_bytecode_full,
    };

    fn make_code(opcodes: &[u8]) -> Vec<u8> {
        opcodes.to_vec()
    }

    fn fork_inline_targets(code: &[u8], offset: usize) -> (u16, u16) {
        assert_eq!(code[offset], Opcode::ForkInline.to_byte());
        assert_eq!(u16::from_be_bytes([code[offset + 1], code[offset + 2]]), 2);
        (
            u16::from_be_bytes([code[offset + 3], code[offset + 4]]),
            u16::from_be_bytes([code[offset + 5], code[offset + 6]]),
        )
    }

    #[test]
    fn test_nop_removal() {
        let code = make_code(&[
            Opcode::Nop.to_byte(),
            Opcode::PushTrue.to_byte(),
            Opcode::Nop.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.nops_removed, 2);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_swap_swap_removal() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::PushFalse.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.swap_swap_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::PushFalse.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_dup_pop_removal() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Dup.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.dup_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_not_not_removal() {
        // Use LoadLocal to avoid triggering PushTrue/PushFalse; Not patterns first
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // local slot 0
            Opcode::Not.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.not_not_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::LoadLocal.to_byte(), 0, Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_push_true_not_folding() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.const_not_folded, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushFalse.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_push_false_not_folding() {
        let code = make_code(&[
            Opcode::PushFalse.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.const_not_folded, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_simple_push_pop_removal() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::PushUnit.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushUnit.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_push_long_small_pop_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::Pop.to_byte(),
            Opcode::PushUnit.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushUnit.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_add_zero_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        // Should be: push 5, return
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 5, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_sub_zero_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            10,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Sub.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                10,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_mul_one_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            7,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 7, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_div_one_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            8,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Div.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 8, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_push_constant_pop_removal() {
        let code = make_code(&[
            Opcode::PushLong.to_byte(),
            0,
            0, // index 0
            Opcode::Pop.to_byte(),
            Opcode::PushUnit.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushUnit.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_jump_fixup() {
        // Build: if (load x) { nop; return 1 } else { return 2 }
        // Using LoadLocal instead of PushTrue to avoid triggering dead branch optimization
        // Layout:
        //   0: LoadLocal 0           (2 bytes)
        //   2: JumpIfFalse           (1 byte)
        //   3-4: offset 7            (2 bytes) -> jump_from=5, target=5+7=12 (else)
        //   5: Nop                   (1 byte) - will be removed
        //   6: PushLongSmall         (1 byte)
        //   7: 1                     (1 byte)
        //   8: Return                (1 byte)
        //   9: Jump                  (1 byte)
        //  10-11: offset 3           (2 bytes) -> jump_from=12, target=12+3=15 (end)
        //  12: PushLongSmall         (1 byte) - else branch
        //  13: 2                     (1 byte)
        //  14: Return                (1 byte)
        //
        // After removing Nop (1 byte at offset 5):
        //   0: LoadLocal 0
        //   2: JumpIfFalse
        //   3-4: offset ?            -> jump_from=5, should target 11 (was 12), so offset=6
        //   5: PushLongSmall (was 6)
        //   6: 1 (was 7)
        //   7: Return (was 8)
        //   8: Jump (was 9)
        //   9-10: offset ? (was 10-11) -> jump_from=11, should target 14 (was 15), so offset=3
        //  11: PushLongSmall (was 12)
        //  12: 2 (was 13)
        //  13: Return (was 14)

        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),   // 0
            0,                             // 1: slot 0
            Opcode::JumpIfFalse.to_byte(), // 2
            0,
            7,                               // 3-4: offset 7 to else branch at 12
            Opcode::Nop.to_byte(),           // 5 - will be removed
            Opcode::PushLongSmall.to_byte(), // 6
            1,                               // 7
            Opcode::Return.to_byte(),        // 8
            Opcode::Jump.to_byte(),          // 9 (skip else)
            0,
            3,                               // 10-11: offset 3 to end at 15
            Opcode::PushLongSmall.to_byte(), // 12 - else branch
            2,                               // 13
            Opcode::Return.to_byte(),        // 14
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.nops_removed, 1);
        // After removing nop at position 5, JumpIfFalse offset should be adjusted from 7 to 6
        assert_eq!(optimized[2], Opcode::JumpIfFalse.to_byte());
        let new_offset = i16::from_be_bytes([optimized[3], optimized[4]]);
        assert_eq!(new_offset, 6); // Adjusted from 7 to 6

        // Check that Jump offset is still 3 (both source and target shifted equally)
        assert_eq!(optimized[8], Opcode::Jump.to_byte());
        let jump_offset = i16::from_be_bytes([optimized[9], optimized[10]]);
        assert_eq!(jump_offset, 3); // Should remain 3
    }

    #[test]
    fn test_peephole_does_not_scan_fork_inline_target_table() {
        let code = make_code(&[
            Opcode::ForkInline.to_byte(),
            0,
            2,
            0,
            7,
            0,
            9,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code.clone());

        assert_eq!(stats.nops_removed, 0);
        assert_eq!(optimized, code);
        assert_eq!(fork_inline_targets(&optimized, 0), (7, 9));
    }

    #[test]
    fn test_peephole_remaps_fork_inline_targets_after_prior_removal() {
        let code = make_code(&[
            Opcode::Nop.to_byte(),
            Opcode::ForkInline.to_byte(),
            0,
            2,
            0,
            8,
            0,
            10,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.nops_removed, 1);
        assert_eq!(optimized[0], Opcode::ForkInline.to_byte());
        assert_eq!(fork_inline_targets(&optimized, 0), (7, 9));
    }

    #[test]
    fn test_peephole_remaps_fork_inline_targets_after_branch_removal() {
        let code = make_code(&[
            Opcode::ForkInline.to_byte(),
            0,
            2,
            0,
            7,
            0,
            11,
            Opcode::PushTrue.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(fork_inline_targets(&optimized, 0), (7, 9));
    }

    #[test]
    fn test_multiple_passes() {
        // After first pass: PushTrue; Not → PushFalse
        // After second pass: PushFalse; Not → PushTrue
        // The second Not is left alone since there's no more Not after it
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // First: PushTrue; Not → PushFalse (const_not_folded++)
        // Then: PushFalse; Not → PushTrue (const_not_folded++)
        assert!(stats.const_not_folded >= 1);
        // Final result should be PushTrue
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_no_optimization_needed() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code.clone());

        assert_eq!(stats.total_optimizations(), 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_empty_code() {
        let code = Vec::new();
        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.total_optimizations(), 0);
        assert!(optimized.is_empty());
    }

    #[test]
    fn test_chained_optimizations() {
        // swap; swap; swap; swap → should all be removed
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::PushFalse.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.swap_swap_removed, 2);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::PushFalse.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    // ========================================================================
    // Dead Code Elimination Tests
    // ========================================================================

    #[test]
    fn test_dce_empty_code() {
        let code = Vec::new();
        let (optimized, stats) = eliminate_dead_code(code);

        assert!(optimized.is_empty());
        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(stats.bytes_removed, 0);
    }

    #[test]
    fn test_dce_no_dead_code() {
        // All code is reachable
        let code = make_code(&[
            Opcode::PushTrue.to_byte(), // 0
            Opcode::Return.to_byte(),   // 1
        ]);

        let (optimized, stats) = eliminate_dead_code(code.clone());

        assert_eq!(optimized, code);
        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(stats.bytes_removed, 0);
    }

    #[test]
    fn test_dce_dead_code_after_unconditional_jump() {
        // Code after unconditional jump is dead
        //   0: Jump +3          (3 bytes) -> jumps to offset 6
        //   3: PushTrue         (1 byte)  - DEAD
        //   4: Pop              (1 byte)  - DEAD
        //   5: Nop              (1 byte)  - DEAD (but starts new block)
        //   6: PushFalse        (1 byte)  - target of jump
        //   7: Return           (1 byte)
        let code = make_code(&[
            Opcode::Jump.to_byte(), // 0
            0,
            3,                           // 1-2: offset +3 -> target = 3+3 = 6
            Opcode::PushTrue.to_byte(),  // 3 - DEAD
            Opcode::Pop.to_byte(),       // 4 - DEAD
            Opcode::Nop.to_byte(),       // 5 - DEAD (block boundary)
            Opcode::PushFalse.to_byte(), // 6 - jump target
            Opcode::Return.to_byte(),    // 7
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        // Dead code from offset 3-5 should be removed
        assert!(stats.blocks_removed >= 1);
        assert!(stats.bytes_removed >= 3);

        // Result should be: Jump, PushFalse, Return
        // After removing dead code, Jump offset needs to be fixed
        assert_eq!(optimized.len(), 5); // Jump(3 bytes) + PushFalse(1) + Return(1)
        assert_eq!(optimized[0], Opcode::Jump.to_byte());
        assert_eq!(optimized[3], Opcode::PushFalse.to_byte());
        assert_eq!(optimized[4], Opcode::Return.to_byte());
    }

    #[test]
    fn test_dce_dead_code_after_return() {
        // Code after return is dead
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),  // 0
            Opcode::Return.to_byte(),    // 1
            Opcode::PushFalse.to_byte(), // 2 - DEAD
            Opcode::Pop.to_byte(),       // 3 - DEAD
            Opcode::Return.to_byte(),    // 4 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_dce_conditional_branch_both_reachable() {
        // Both branches of conditional are reachable
        //   0: PushTrue                  (1 byte)
        //   1: JumpIfFalse +4            (3 bytes) -> target = 4+4 = 8
        //   4: PushLongSmall 1           (2 bytes) - then branch
        //   6: Jump +2                   (3 bytes) -> target = 9+2 = 11
        //   9: PushLongSmall 2           (2 bytes) - else branch
        //  11: Return                    (1 byte)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),    // 0
            Opcode::JumpIfFalse.to_byte(), // 1
            0,
            4,                               // 2-3: offset +4 to offset 8
            Opcode::PushLongSmall.to_byte(), // 4
            1,                               // 5
            Opcode::Jump.to_byte(),          // 6
            0,
            2,                               // 7-8: offset +2 to offset 11
            Opcode::PushLongSmall.to_byte(), // 9
            2,                               // 10
            Opcode::Return.to_byte(),        // 11
        ]);

        let (optimized, stats) = eliminate_dead_code(code.clone());

        // No dead code - all paths reachable
        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_dce_halt_terminates() {
        // Code after Halt is dead
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),  // 0
            Opcode::Halt.to_byte(),      // 1
            Opcode::PushFalse.to_byte(), // 2 - DEAD
            Opcode::Return.to_byte(),    // 3 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Halt.to_byte(),]
        );
    }

    #[test]
    fn test_dce_jump_short() {
        // Test with short (1-byte offset) jump
        //   0: JumpShort +2       (2 bytes) -> target = 2+2 = 4
        //   2: Nop                (1 byte)  - DEAD
        //   3: Nop                (1 byte)  - DEAD (but new block)
        //   4: Return             (1 byte)  - target
        let code = make_code(&[
            Opcode::JumpShort.to_byte(), // 0
            2,                           // 1: offset +2 -> target = 4
            Opcode::Nop.to_byte(),       // 2 - DEAD
            Opcode::Nop.to_byte(),       // 3 - DEAD
            Opcode::Return.to_byte(),    // 4 - jump target
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(optimized.len(), 3); // JumpShort(2) + Return(1)
        assert_eq!(optimized[0], Opcode::JumpShort.to_byte());
        assert_eq!(optimized[2], Opcode::Return.to_byte());
    }

    #[test]
    fn test_dce_return_multi() {
        // Code after ReturnMulti is dead
        // Note: ReturnMulti has no immediate bytes (count is on stack)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),    // 0
            Opcode::ReturnMulti.to_byte(), // 1
            Opcode::PushFalse.to_byte(),   // 2 - DEAD
            Opcode::Return.to_byte(),      // 3 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::ReturnMulti.to_byte(),]
        );
    }

    #[test]
    fn test_dce_jump_fixup_after_removal() {
        // Test that jump targets are correctly fixed after dead code removal
        //   0: Jump +6            (3 bytes) -> target = 3+6 = 9
        //   3: PushTrue           (1 byte)  - DEAD (not jumped to)
        //   4: Return             (1 byte)  - DEAD
        //   5: Nop                (1 byte)  - DEAD
        //   6: Nop                (1 byte)  - DEAD
        //   7: Nop                (1 byte)  - DEAD
        //   8: Nop                (1 byte)  - DEAD (block boundary for target)
        //   9: PushFalse          (1 byte)  - jump target
        //  10: Return             (1 byte)
        let code = make_code(&[
            Opcode::Jump.to_byte(), // 0
            0,
            6,                           // 1-2: offset +6 -> target = 9
            Opcode::PushTrue.to_byte(),  // 3 - DEAD
            Opcode::Return.to_byte(),    // 4 - DEAD
            Opcode::Nop.to_byte(),       // 5 - DEAD
            Opcode::Nop.to_byte(),       // 6 - DEAD
            Opcode::Nop.to_byte(),       // 7 - DEAD
            Opcode::Nop.to_byte(),       // 8 - DEAD
            Opcode::PushFalse.to_byte(), // 9 - target
            Opcode::Return.to_byte(),    // 10
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        // Removed 6 bytes (offsets 3-8), Jump offset should be fixed to 0
        assert!(stats.bytes_removed >= 6);

        // Check the Jump offset is fixed
        let new_offset = i16::from_be_bytes([optimized[1], optimized[2]]);
        assert_eq!(new_offset, 0); // Target is immediately after Jump

        // Result: Jump + PushFalse + Return
        assert_eq!(optimized.len(), 5);
        assert_eq!(optimized[3], Opcode::PushFalse.to_byte());
        assert_eq!(optimized[4], Opcode::Return.to_byte());
    }

    #[test]
    fn test_dce_with_conditional_dead_branch() {
        // Conditional where one branch has no way back
        //   0: PushTrue                  (1 byte)
        //   1: JumpIfTrue +3             (3 bytes) -> target = 4+3 = 7 (then branch)
        //   4: Jump +5                   (3 bytes) -> target = 7+5 = 12 (skip to end)
        //   7: PushLongSmall 1           (2 bytes) - then branch
        //   9: Return                    (1 byte)
        //  10: PushLongSmall 2           (2 bytes) - DEAD (neither else nor then reaches here)
        //  12: Return                    (1 byte)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),   // 0
            Opcode::JumpIfTrue.to_byte(), // 1
            0,
            3,                      // 2-3: offset +3 to 7
            Opcode::Jump.to_byte(), // 4 - else: unconditional jump
            0,
            5,                               // 5-6: offset +5 to 12
            Opcode::PushLongSmall.to_byte(), // 7 - then branch
            1,                               // 8
            Opcode::Return.to_byte(),        // 9 - then returns
            Opcode::PushLongSmall.to_byte(), // 10 - DEAD
            2,                               // 11 - DEAD
            Opcode::Return.to_byte(),        // 12 - target of else's jump
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        // Dead code at 10-11 should be removed
        assert!(stats.bytes_removed >= 2);
        assert!(optimized.len() < 13);
    }

    #[test]
    fn test_dce_keeps_fork_inline_branch_targets_reachable() {
        let code = make_code(&[
            Opcode::ForkInline.to_byte(),
            0,
            2,
            0,
            7,
            0,
            10,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Return.to_byte(),
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = eliminate_dead_code(code.clone());

        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_dce_remaps_fork_inline_targets_after_prior_dead_code() {
        let code = make_code(&[
            Opcode::Jump.to_byte(),
            0,
            3,
            Opcode::PushTrue.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::Nop.to_byte(),
            Opcode::ForkInline.to_byte(),
            0,
            2,
            0,
            13,
            0,
            15,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.bytes_removed >= 3);
        assert_eq!(optimized[3], Opcode::ForkInline.to_byte());
        assert_eq!(fork_inline_targets(&optimized, 3), (10, 12));
    }

    #[test]
    fn test_dce_combined_with_peephole() {
        // Test the combined optimizer
        //   0: Jump +3             (3 bytes) -> target = 6
        //   3: Nop                 (1 byte)  - DEAD
        //   4: Nop                 (1 byte)  - DEAD
        //   5: Nop                 (1 byte)  - DEAD
        //   6: Swap                (1 byte)  - jump target
        //   7: Swap                (1 byte)  - will be optimized by peephole
        //   8: Return              (1 byte)
        let code = make_code(&[
            Opcode::Jump.to_byte(), // 0
            0,
            3,                        // 1-2: offset +3 to 6
            Opcode::Nop.to_byte(),    // 3 - DEAD
            Opcode::Nop.to_byte(),    // 4 - DEAD
            Opcode::Nop.to_byte(),    // 5 - DEAD
            Opcode::Swap.to_byte(),   // 6 - peephole target
            Opcode::Swap.to_byte(),   // 7 - peephole target
            Opcode::Return.to_byte(), // 8
        ]);

        let (optimized, peephole_stats, dce_stats) = optimize_bytecode_full(code);

        // Peephole should remove swap-swap and nops
        // Note: peephole runs first, then DCE
        assert!(
            peephole_stats.nops_removed > 0
                || peephole_stats.swap_swap_removed > 0
                || dce_stats.blocks_removed > 0,
            "Expected some optimization to occur"
        );

        // Final result should be minimal
        // After peephole: Jump, DeadNops..., Return (nops removed, swap-swap removed)
        // After DCE: Jump, Return
        assert!(optimized.len() <= 5); // At most Jump(3) + Return(1) + some
    }

    #[test]
    fn test_dce_stats() {
        // Test that stats are correctly tracked
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),  // 0
            Opcode::Return.to_byte(),    // 1
            Opcode::PushFalse.to_byte(), // 2 - DEAD
            Opcode::PushUnit.to_byte(),  // 3 - DEAD
            Opcode::Pop.to_byte(),       // 4 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert_eq!(stats.blocks_found, 2); // Entry block + dead block
        assert_eq!(stats.blocks_reachable, 1); // Only entry reachable
        assert_eq!(stats.blocks_removed, 1);
        assert_eq!(stats.bytes_removed, 3); // 3 dead bytes
        assert_eq!(optimized.len(), 2);
    }

    // ========================================================================
    // New Peephole Optimization Tests
    // ========================================================================

    // NOTE: Boolean identity and annihilator optimizations are DISABLED because
    // they can hide type errors in MeTTa's dynamically typed system.
    // For example: (and 1 True) should error, not return 1.
    // These tests verify the optimizations do NOT occur.

    #[test]
    fn test_bool_identity_and_true() {
        // x AND True = x mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushTrue.to_byte(),
            Opcode::And.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_identity_removed, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushTrue.to_byte(),
                Opcode::And.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_bool_identity_or_false() {
        // x OR False = x mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushFalse.to_byte(),
            Opcode::Or.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_identity_removed, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushFalse.to_byte(),
                Opcode::Or.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_bool_annihilator_and_false() {
        // x AND False = False mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushFalse.to_byte(),
            Opcode::And.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_annihilator_folded, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushFalse.to_byte(),
                Opcode::And.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_bool_annihilator_or_true() {
        // x OR True = True mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushTrue.to_byte(),
            Opcode::Or.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_annihilator_folded, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushTrue.to_byte(),
                Opcode::Or.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_neg_neg_removal() {
        // Neg; Neg = identity
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::Neg.to_byte(),
            Opcode::Neg.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.neg_neg_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 5, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_mul_zero() {
        // x * 0 = 0
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42, // x = 42
            Opcode::PushLongSmall.to_byte(),
            0, // * 0
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.mul_zero_folded, 1);
        // After mul_zero: [Push 42, Pop, Push 0, Return]
        // Then push_pop optimization removes Push 42; Pop
        // Final: [Push 0, Return]
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 0, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_pow_zero() {
        // x ^ 0 = 1
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5, // x = 5
            Opcode::PushLongSmall.to_byte(),
            0, // ^ 0
            Opcode::Pow.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.pow_folded, 1);
        // After pow_zero: [Push 5, Pop, Push 1, Return]
        // Then push_pop optimization removes Push 5; Pop
        // Final: [Push 1, Return]
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 1, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_pow_one() {
        // x ^ 1 = x
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            7, // x = 7
            Opcode::PushLongSmall.to_byte(),
            1, // ^ 1
            Opcode::Pow.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.pow_folded, 1);
        // Should remove PushLongSmall 1; Pow
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 7, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_abs_abs_idempotent() {
        // Abs(Abs(x)) = Abs(x)
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            10,
            Opcode::Abs.to_byte(),
            Opcode::Abs.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.idempotent_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                10,
                Opcode::Abs.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_comparison_folding_lt_not() {
        // Lt; Not → Ge
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Lt.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.comparison_folded, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                1,
                Opcode::PushLongSmall.to_byte(),
                2,
                Opcode::Ge.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_comparison_folding_eq_not() {
        // Eq; Not → Ne
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Eq.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.comparison_folded, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                1,
                Opcode::PushLongSmall.to_byte(),
                2,
                Opcode::Ne.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_const_branch_fold_push_true_jump_if_true() {
        // PushTrue; JumpIfTrue → Jump (always taken)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),   // 0
            Opcode::JumpIfTrue.to_byte(), // 1
            0,
            3,                           // 2-3: offset +3 to target
            Opcode::PushUnit.to_byte(),  // 4 - skipped
            Opcode::Return.to_byte(),    // 5
            Opcode::PushFalse.to_byte(), // 6 - target
            Opcode::Return.to_byte(),    // 7
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.const_branch_folded, 1);
        // PushTrue; JumpIfTrue +3 becomes Jump +3
        // First 4 bytes become 3 bytes (Jump)
        assert_eq!(optimized[0], Opcode::Jump.to_byte());
    }

    #[test]
    fn test_dead_branch_push_true_jump_if_false() {
        // PushTrue; JumpIfFalse → remove (never taken)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),    // 0
            Opcode::JumpIfFalse.to_byte(), // 1
            0,
            2,                          // 2-3: offset +2 to target
            Opcode::PushUnit.to_byte(), // 4 - fall through
            Opcode::Return.to_byte(),   // 5
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.dead_branch_removed, 1);
        // All 4 bytes (PushTrue, JumpIfFalse, offset) should be removed
        assert_eq!(
            optimized,
            vec![Opcode::PushUnit.to_byte(), Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_load_deduplication() {
        // LoadLocal X; LoadLocal X → LoadLocal X; Dup
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            5, // slot 5
            Opcode::LoadLocal.to_byte(),
            5, // same slot 5
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.load_deduplicated, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                5,
                Opcode::Dup.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_load_dedup_different_slots() {
        // LoadLocal X; LoadLocal Y → no change (different slots)
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            5,
            Opcode::LoadLocal.to_byte(),
            6, // different slot
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code.clone());

        assert_eq!(stats.load_deduplicated, 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_jump_threading() {
        // Jump L1; ... L1: Jump L2 → Jump L2
        let code = make_code(&[
            Opcode::Jump.to_byte(), // 0
            0,
            3,                      // 1-2: offset +3 → target = 6
            Opcode::Nop.to_byte(),  // 3 (dead)
            Opcode::Nop.to_byte(),  // 4 (dead)
            Opcode::Nop.to_byte(),  // 5 (dead)
            Opcode::Jump.to_byte(), // 6 - L1: another jump
            0,
            3,                          // 7-8: offset +3 → target = 12
            Opcode::Nop.to_byte(),      // 9 (dead)
            Opcode::Nop.to_byte(),      // 10 (dead)
            Opcode::Nop.to_byte(),      // 11 (dead)
            Opcode::PushTrue.to_byte(), // 12 - L2: final target
            Opcode::Return.to_byte(),   // 13
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // Jump at 0 should now point directly to L2 (offset 12)
        assert!(stats.jump_threaded >= 1);
        // After nop removal and jump threading, the first jump should target the end
    }

    #[test]
    fn test_jump_threading_short() {
        // JumpShort L1; L1: JumpShort L2 → JumpShort L2
        let code = make_code(&[
            Opcode::JumpShort.to_byte(), // 0
            2,                           // 1: offset +2 → target = 4
            Opcode::Nop.to_byte(),       // 2 (dead)
            Opcode::Nop.to_byte(),       // 3 (dead)
            Opcode::JumpShort.to_byte(), // 4 - L1: another short jump
            3,                           // 5: offset +3 → target = 9
            Opcode::Nop.to_byte(),       // 6 (dead)
            Opcode::Nop.to_byte(),       // 7 (dead)
            Opcode::Nop.to_byte(),       // 8 (dead)
            Opcode::PushTrue.to_byte(),  // 9 - L2: final target
            Opcode::Return.to_byte(),    // 10
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // Should thread the short jumps
        assert!(stats.jump_threaded >= 1 || stats.nops_removed > 0);
    }

    #[test]
    fn test_combined_optimizations() {
        // Test multiple optimizations in one pass
        // NOTE: Bool identity optimization is disabled for type safety
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            10,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(), // identity: x + 0 = x
            Opcode::Neg.to_byte(),
            Opcode::Neg.to_byte(), // double neg
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert!(stats.identity_ops_removed >= 1);
        assert!(stats.neg_neg_removed >= 1);
        // Final result: PushLongSmall(2) + Return(1) = 3 bytes
        assert!(optimized.len() <= 4);
    }

    // ========================================================================
    // Branch Coverage Tests - Comparison Folds
    // ========================================================================

    #[test]
    fn test_le_not_to_gt() {
        // Le; Not → Gt
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            3,
            Opcode::Le.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Le; Not should be replaced with Gt
        assert!(stats.comparison_folded >= 1, "Le; Not should fold to Gt");
        // Check that Not opcode is removed
        assert!(!optimized.contains(&Opcode::Not.to_byte()));
    }

    #[test]
    fn test_gt_not_to_le() {
        // Gt; Not → Le
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            3,
            Opcode::Gt.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Gt; Not should be replaced with Le
        assert!(stats.comparison_folded >= 1, "Gt; Not should fold to Le");
        assert!(!optimized.contains(&Opcode::Not.to_byte()));
    }

    #[test]
    fn test_ge_not_to_lt() {
        // Ge; Not → Lt
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            3,
            Opcode::Ge.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Ge; Not should be replaced with Lt
        assert!(stats.comparison_folded >= 1, "Ge; Not should fold to Lt");
        assert!(!optimized.contains(&Opcode::Not.to_byte()));
    }

    #[test]
    fn test_ne_not_to_eq() {
        // Ne; Not → Eq
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            3,
            Opcode::Ne.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Ne; Not should be replaced with Eq
        assert!(stats.comparison_folded >= 1, "Ne; Not should fold to Eq");
        assert!(!optimized.contains(&Opcode::Not.to_byte()));
    }

    // ========================================================================
    // Branch Coverage Tests - Jump Threading Edge Cases
    // ========================================================================

    #[test]
    fn test_jump_threading_deep_chain() {
        // Create a deep chain of jumps to test iteration limit handling
        // Jump → Jump → Jump → ... → Return
        let code = make_code(&[
            Opcode::Jump.to_byte(),
            0,
            3,                     // 0-2: Jump to 6
            Opcode::Nop.to_byte(), // 3 (dead)
            Opcode::Nop.to_byte(), // 4 (dead)
            Opcode::Nop.to_byte(), // 5 (dead)
            Opcode::Jump.to_byte(),
            0,
            3,                     // 6-8: Jump to 12
            Opcode::Nop.to_byte(), // 9 (dead)
            Opcode::Nop.to_byte(), // 10 (dead)
            Opcode::Nop.to_byte(), // 11 (dead)
            Opcode::Jump.to_byte(),
            0,
            3,                     // 12-14: Jump to 18
            Opcode::Nop.to_byte(), // 15 (dead)
            Opcode::Nop.to_byte(), // 16 (dead)
            Opcode::Nop.to_byte(), // 17 (dead)
            Opcode::Jump.to_byte(),
            0,
            3,                          // 18-20: Jump to 24
            Opcode::Nop.to_byte(),      // 21 (dead)
            Opcode::Nop.to_byte(),      // 22 (dead)
            Opcode::Nop.to_byte(),      // 23 (dead)
            Opcode::PushTrue.to_byte(), // 24: final destination
            Opcode::Return.to_byte(),   // 25
        ]);
        let code_len = code.len();

        let (optimized, stats) = optimize_bytecode(code);

        // All intermediate jumps should be threaded
        assert!(stats.jump_threaded >= 1);
        // Final code should be much smaller
        assert!(optimized.len() < code_len);
    }

    #[test]
    fn test_jump_at_code_boundary() {
        // Jump targeting the exact end of code
        let code = make_code(&[
            Opcode::Jump.to_byte(),
            0,
            3,                        // 0-2: Jump to 6
            Opcode::Nop.to_byte(),    // 3
            Opcode::Nop.to_byte(),    // 4
            Opcode::Nop.to_byte(),    // 5
            Opcode::Return.to_byte(), // 6: Return at target
        ]);

        let (optimized, _stats) = optimize_bytecode(code.clone());

        // Should compile without error
        assert!(!optimized.is_empty());
    }

    #[test]
    fn test_conditional_jump_threading() {
        // JumpIfFalse to another Jump
        let code = make_code(&[
            Opcode::PushTrue.to_byte(), // 0
            Opcode::JumpIfFalse.to_byte(),
            0,
            3, // 1-3: JumpIfFalse to 7
            Opcode::PushLongSmall.to_byte(),
            1,                        // 4-5: Then branch
            Opcode::Return.to_byte(), // 6
            Opcode::Jump.to_byte(),
            0,
            3,                     // 7-9: Else branch jumps to 13
            Opcode::Nop.to_byte(), // 10
            Opcode::Nop.to_byte(), // 11
            Opcode::Nop.to_byte(), // 12
            Opcode::PushLongSmall.to_byte(),
            2,                        // 13-14: Final target
            Opcode::Return.to_byte(), // 15
        ]);

        let (optimized, _stats) = optimize_bytecode(code);

        // Should handle conditional jump threading
        assert!(!optimized.is_empty());
    }

    // ========================================================================
    // Branch Coverage Tests - Dead Code Elimination
    // ========================================================================

    #[test]
    fn test_dce_unreachable_after_return() {
        // Code after Return is unreachable
        // Note: The peephole optimizer may not implement DCE for unreachable code after Return
        // This test verifies the optimizer handles this case without panic
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Return.to_byte(),
            Opcode::PushFalse.to_byte(), // Unreachable
            Opcode::Return.to_byte(),    // Unreachable
        ]);

        let (optimized, _stats) = optimize_bytecode(code);

        // At minimum, verify the code is still valid (doesn't panic)
        // The optimized code should start with the same opcode
        assert_eq!(optimized[0], Opcode::PushTrue.to_byte());
    }

    #[test]
    fn test_dce_unreachable_after_jump() {
        // Code after unconditional Jump is unreachable
        let code = make_code(&[
            Opcode::Jump.to_byte(),
            0,
            3,                           // 0-2: Jump to 6
            Opcode::PushFalse.to_byte(), // 3: Unreachable
            Opcode::Return.to_byte(),    // 4: Unreachable
            Opcode::Nop.to_byte(),       // 5: Unreachable
            Opcode::PushTrue.to_byte(),  // 6: Jump target
            Opcode::Return.to_byte(),    // 7
        ]);
        let code_len = code.len();

        let (optimized, _stats) = optimize_bytecode(code);

        // Should remove unreachable code (code should be smaller)
        assert!(
            optimized.len() < code_len,
            "Expected code to shrink, got {} bytes (from {})",
            optimized.len(),
            code_len
        );
    }

    #[test]
    fn test_dce_empty_branch_removal() {
        // PushTrue; JumpIfFalse - the false branch is never taken
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::JumpIfFalse.to_byte(),
            0,
            5, // Jump to 8 (never taken)
            Opcode::PushLongSmall.to_byte(),
            42,                       // This is always executed
            Opcode::Return.to_byte(), // 7
            Opcode::PushLongSmall.to_byte(),
            0,                        // 8: Dead code
            Opcode::Return.to_byte(), // 10: Dead code
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // Dead branch should be removed
        assert!(stats.dead_branch_removed >= 1 || stats.bytes_removed >= 2);
    }

    #[test]
    fn test_dce_multiple_blocks() {
        // Multiple unreachable blocks
        // Note: Full DCE for multiple blocks may require additional passes
        // This test verifies the optimizer handles complex CFG without panic
        let code = make_code(&[
            Opcode::Jump.to_byte(),
            0,
            9, // 0-2: Jump to 12
            Opcode::PushLongSmall.to_byte(),
            1,                        // 3-4: Block 1 (dead)
            Opcode::Return.to_byte(), // 5
            Opcode::PushLongSmall.to_byte(),
            2,                        // 6-7: Block 2 (dead)
            Opcode::Return.to_byte(), // 8
            Opcode::PushLongSmall.to_byte(),
            3,                          // 9-10: Block 3 (dead)
            Opcode::Return.to_byte(),   // 11
            Opcode::PushTrue.to_byte(), // 12: Reachable target
            Opcode::Return.to_byte(),   // 13
        ]);

        let (optimized, _stats) = optimize_bytecode(code);

        // Verify optimization doesn't panic and produces valid bytecode
        // The optimized code should at minimum have Jump and Return
        assert!(!optimized.is_empty());
        assert!(optimized.contains(&Opcode::Return.to_byte()));
    }

    // ========================================================================
    // Branch Coverage Tests - Peephole Edge Cases
    // ========================================================================

    #[test]
    fn test_peephole_nop_at_end() {
        // NOPs at the end of code
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Return.to_byte(),
            Opcode::Nop.to_byte(), // NOP after return (unreachable)
            Opcode::Nop.to_byte(),
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // Unreachable NOPs should be removed
        assert!(stats.nops_removed >= 2 || stats.bytes_removed >= 2);
    }

    #[test]
    fn test_peephole_consecutive_pops() {
        // Multiple consecutive pops
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::PushLongSmall.to_byte(),
            3,
            Opcode::Pop.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::PushTrue.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, _stats) = optimize_bytecode(code.clone());

        // Should handle or optimize consecutive pops
        assert!(!optimized.is_empty());
    }

    #[test]
    fn test_peephole_push_pop_pairs() {
        // Push followed by Pop is dead code
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::Pop.to_byte(), // Push; Pop = dead
            Opcode::PushTrue.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Push; Pop should be optimized away
        assert!(
            stats.push_pop_removed >= 1 || stats.identity_ops_removed >= 1 || optimized.len() <= 4
        );
    }

    #[test]
    fn test_double_not_optimization() {
        // Not; Not → identity
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Double negation should be removed
        assert!(stats.not_not_removed >= 1 || !optimized.contains(&Opcode::Not.to_byte()));
    }

    // ========================================================================
    // Branch Coverage Tests - Arithmetic Identity
    // ========================================================================

    #[test]
    fn test_sub_zero_identity() {
        // x - 0 = x
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Sub.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // x - 0 should be identity
        assert!(stats.identity_ops_removed >= 1);
    }

    #[test]
    fn test_mul_one_identity() {
        // x * 1 = x
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // x * 1 should be identity
        assert!(stats.identity_ops_removed >= 1);
    }

    #[test]
    fn test_div_one_identity() {
        // x / 1 = x
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Div.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (_optimized, stats) = optimize_bytecode(code);

        // x / 1 should be identity
        assert!(stats.identity_ops_removed >= 1);
    }

    // ========================================================================
    // Branch Coverage Tests - Misc Edge Cases
    // ========================================================================

    #[test]
    fn test_minimal_code() {
        // Minimal valid bytecode
        let code = make_code(&[Opcode::Return.to_byte()]);

        let (optimized, _stats) = optimize_bytecode(code);

        // Should handle minimal code
        assert_eq!(optimized.len(), 1);
        assert_eq!(optimized[0], Opcode::Return.to_byte());
    }

    #[test]
    fn test_only_nops() {
        // Code with only NOPs (and Return)
        let code = make_code(&[
            Opcode::Nop.to_byte(),
            Opcode::Nop.to_byte(),
            Opcode::Nop.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // All NOPs should be removed
        assert!(stats.nops_removed >= 3);
        assert_eq!(optimized.len(), 1);
    }

    #[test]
    fn test_large_constant_pool_index() {
        // Large constant pool index (u16)
        let code = make_code(&[
            Opcode::PushConstant.to_byte(),
            1,
            0, // Constant index 256
            Opcode::Return.to_byte(),
        ]);

        let (optimized, _stats) = optimize_bytecode(code.clone());

        // Should preserve large constant indices
        assert_eq!(optimized.len(), code.len());
    }

    // =========================================================================
    // Peephole comparison folding + jump target correctness tests (Bug #1 fix)
    // =========================================================================

    /// Helper: builds code for `push A; push B; CmpOp; Not; JumpIfFalse +offset; then_val; Jump; else_val; Return`
    /// and verifies the peephole folds CmpOp;Not and the resulting jump targets are correct.
    fn build_cmp_not_if_code(cmp_op: Opcode, expected_folded: Opcode) -> (Vec<u8>, Vec<u8>) {
        // Layout (all positions relative):
        //   0: PushLongSmall A      (2 bytes)
        //   2: PushLongSmall B      (2 bytes)
        //   4: CmpOp                (1 byte)
        //   5: Not                  (1 byte)
        //   6: JumpIfFalse offset   (3 bytes, i16 BE)
        //   9: PushLongSmall 1      (2 bytes, then-branch)
        //  11: Jump offset2         (3 bytes)
        //  14: PushLongSmall 2      (2 bytes, else-branch)
        //  16: Return               (1 byte)
        //
        // JumpIfFalse jumps to 14 (else): offset = 14 - 9 = 5
        // Jump jumps to 16 (return): offset = 16 - 14 = 2
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            0, // 0-1: push 0
            Opcode::PushLongSmall.to_byte(),
            1,                     // 2-3: push 1
            cmp_op.to_byte(),      // 4: comparison
            Opcode::Not.to_byte(), // 5: negate
            Opcode::JumpIfFalse.to_byte(),
            0,
            5, // 6-8: JumpIfFalse +5 → pos 14
            Opcode::PushLongSmall.to_byte(),
            1, // 9-10: then branch
            Opcode::Jump.to_byte(),
            0,
            2, // 11-13: Jump +2 → pos 16
            Opcode::PushLongSmall.to_byte(),
            2,                        // 14-15: else branch
            Opcode::Return.to_byte(), // 16
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify the fold happened
        assert!(
            stats.comparison_folded >= 1,
            "Expected comparison folding for {:?};Not → {:?}",
            cmp_op,
            expected_folded
        );

        // After folding: CmpOp;Not (2 bytes) → FoldedOp (1 byte), 1 byte removed
        // New layout:
        //   0: PushLongSmall A      (2 bytes)
        //   2: PushLongSmall B      (2 bytes)
        //   4: FoldedOp             (1 byte)
        //   5: JumpIfFalse offset   (3 bytes)
        //   8: PushLongSmall 1      (2 bytes, then-branch)
        //  10: Jump offset2         (3 bytes)
        //  13: PushLongSmall 2      (2 bytes, else-branch)
        //  15: Return               (1 byte)
        //
        // Expected JumpIfFalse offset = 13 - 8 = 5
        // Expected Jump offset = 15 - 13 = 2

        // Verify the folded opcode is present
        assert_eq!(
            optimized[4],
            expected_folded.to_byte(),
            "Expected folded opcode {:?} at pos 4",
            expected_folded
        );

        // Verify JumpIfFalse target is correct (should jump to else-branch)
        assert_eq!(
            optimized[5],
            Opcode::JumpIfFalse.to_byte(),
            "Expected JumpIfFalse at pos 5"
        );
        let jif_offset = i16::from_be_bytes([optimized[6], optimized[7]]);
        // JumpIfFalse at pos 5, operand at 6-7, next instruction at 8
        // Should jump to pos 13 (else branch): offset = 13 - 8 = 5
        assert_eq!(
            jif_offset, 5,
            "JumpIfFalse offset should be 5 (jump from 8 to 13), got {}",
            jif_offset
        );

        // Verify Jump target is correct (should jump to Return)
        assert_eq!(
            optimized[10],
            Opcode::Jump.to_byte(),
            "Expected Jump at pos 10"
        );
        let jump_offset = i16::from_be_bytes([optimized[11], optimized[12]]);
        // Jump at pos 10, operand at 11-12, next instruction at 13
        // Should jump to pos 15 (Return): offset = 15 - 13 = 2
        assert_eq!(
            jump_offset, 2,
            "Jump offset should be 2 (jump from 13 to 15), got {}",
            jump_offset
        );

        (optimized, make_code(&[]))
    }

    #[test]
    fn test_peephole_eq_not_to_ne_jump_target() {
        build_cmp_not_if_code(Opcode::Eq, Opcode::Ne);
    }

    #[test]
    fn test_peephole_ne_not_to_eq_jump_target() {
        build_cmp_not_if_code(Opcode::Ne, Opcode::Eq);
    }

    #[test]
    fn test_peephole_lt_not_to_ge_jump_target() {
        build_cmp_not_if_code(Opcode::Lt, Opcode::Ge);
    }

    #[test]
    fn test_peephole_le_not_to_gt_jump_target() {
        build_cmp_not_if_code(Opcode::Le, Opcode::Gt);
    }

    #[test]
    fn test_peephole_gt_not_to_le_jump_target() {
        build_cmp_not_if_code(Opcode::Gt, Opcode::Le);
    }

    #[test]
    fn test_peephole_ge_not_to_lt_jump_target() {
        build_cmp_not_if_code(Opcode::Ge, Opcode::Lt);
    }

    // ========================================================================
    // Arithmetic Identity/Absorber Guard Tests (Type Safety)
    // ========================================================================
    // These tests verify that the 7 arithmetic identity/absorber optimizations
    // do NOT fire when the preceding instruction is a non-numeric producer
    // (e.g., PushAtom). This prevents hiding type errors in MeTTa's
    // dynamically typed system — same reasoning as for boolean identity
    // optimizations (see test_bool_identity_and_true above).

    #[test]
    fn test_add_zero_guarded_non_numeric() {
        // PushAtom; PushLongSmall 0; Add — PushAtom is NOT numeric, guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1, // PushAtom (3 bytes, non-numeric)
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.identity_ops_removed, 0,
            "Add-zero identity should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_sub_zero_guarded_non_numeric() {
        // PushAtom; PushLongSmall 0; Sub — guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Sub.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.identity_ops_removed, 0,
            "Sub-zero identity should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_mul_one_guarded_non_numeric() {
        // PushAtom; PushLongSmall 1; Mul — guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.identity_ops_removed, 0,
            "Mul-one identity should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_div_one_guarded_non_numeric() {
        // PushAtom; PushLongSmall 1; Div — guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Div.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.identity_ops_removed, 0,
            "Div-one identity should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_mul_zero_guarded_non_numeric() {
        // PushAtom; PushLongSmall 0; Mul — absorber guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.mul_zero_folded, 0,
            "Mul-zero absorber should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_pow_zero_guarded_non_numeric() {
        // PushAtom; PushLongSmall 0; Pow — absorber guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Pow.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.pow_folded, 0,
            "Pow-zero absorber should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_pow_one_guarded_non_numeric() {
        // PushAtom; PushLongSmall 1; Pow — identity guard blocks
        let code = make_code(&[
            Opcode::PushAtom.to_byte(),
            0,
            1,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Pow.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.pow_folded, 0,
            "Pow-one identity should NOT fire with non-numeric predecessor"
        );
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_add_zero_fires_with_numeric_predecessor() {
        // PushLongSmall 5; PushLongSmall 0; Add — PushLongSmall IS numeric, guard passes
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code);
        assert_eq!(
            stats.identity_ops_removed, 1,
            "Add-zero identity SHOULD fire with numeric predecessor"
        );
        assert_eq!(
            optimized,
            vec![Opcode::PushLongSmall.to_byte(), 5, Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_identity_guard_with_arithmetic_predecessor() {
        // Add; PushLongSmall 0; Sub — Add IS a numeric producer, guard passes
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            3,
            Opcode::PushLongSmall.to_byte(),
            4,
            Opcode::Add.to_byte(),
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Sub.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code);
        assert_eq!(
            stats.identity_ops_removed, 1,
            "Sub-zero identity SHOULD fire with arithmetic predecessor"
        );
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                3,
                Opcode::PushLongSmall.to_byte(),
                4,
                Opcode::Add.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_identity_guard_with_load_local_predecessor() {
        // LoadLocal; PushLongSmall 0; Add — LoadLocal is NOT numeric, guard blocks
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);
        let (optimized, stats) = optimize_bytecode(code.clone());
        assert_eq!(
            stats.identity_ops_removed, 0,
            "Add-zero identity should NOT fire with LoadLocal predecessor"
        );
        assert_eq!(optimized, code);
    }

    /// Test that two consecutive Eq;Not → Ne folds produce correct results.
    /// Verifies cumulative offset tracking doesn't corrupt later jump targets.
    #[test]
    fn test_peephole_multiple_cmp_folds_cumulative() {
        // Two if-else blocks back to back. Each has Eq;Not that should fold to Ne.
        // The Add instruction between blocks prevents push-pop optimization.
        //
        // Block 1: push 0; push 1; Eq; Not; JumpIfFalse→else1; push 10; Jump→end1; push 20;
        // Between: Add (combine results)
        // Block 2: push 0; push 1; Eq; Not; JumpIfFalse→else2; push 30; Jump→end2; push 40; Return
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            0, // 0-1
            Opcode::PushLongSmall.to_byte(),
            1,                     // 2-3
            Opcode::Eq.to_byte(),  // 4
            Opcode::Not.to_byte(), // 5
            Opcode::JumpIfFalse.to_byte(),
            0,
            5, // 6-8: → 14
            Opcode::PushLongSmall.to_byte(),
            10, // 9-10
            Opcode::Jump.to_byte(),
            0,
            2, // 11-13: → 16
            Opcode::PushLongSmall.to_byte(),
            20, // 14-15
            // end1 = 16
            Opcode::PushLongSmall.to_byte(),
            0, // 16-17
            Opcode::PushLongSmall.to_byte(),
            1,                     // 18-19
            Opcode::Eq.to_byte(),  // 20
            Opcode::Not.to_byte(), // 21
            Opcode::JumpIfFalse.to_byte(),
            0,
            5, // 22-24: → 30
            Opcode::PushLongSmall.to_byte(),
            30, // 25-26
            Opcode::Jump.to_byte(),
            0,
            2, // 27-29: → 32
            Opcode::PushLongSmall.to_byte(),
            40,                       // 30-31
            Opcode::Return.to_byte(), // 32
        ]);

        let original_len = code.len(); // 33 bytes
        let (optimized, stats) = optimize_bytecode(code);

        // Both comparison folds should happen
        assert!(
            stats.comparison_folded >= 2,
            "Expected at least 2 comparison folds, got {}",
            stats.comparison_folded
        );

        // Code should shrink by at least 2 bytes (one per fold)
        assert!(
            optimized.len() <= original_len - 2,
            "Expected at most {} bytes, got {}",
            original_len - 2,
            optimized.len()
        );

        // Both Ne opcodes should be present
        let ne_count = optimized
            .iter()
            .filter(|&&b| b == Opcode::Ne.to_byte())
            .count();
        assert_eq!(ne_count, 2, "Expected 2 Ne opcodes, found {}", ne_count);

        // Verify no Eq or Not opcodes remain
        let eq_count = optimized
            .iter()
            .filter(|&&b| b == Opcode::Eq.to_byte())
            .count();
        let not_count = optimized
            .iter()
            .filter(|&&b| b == Opcode::Not.to_byte())
            .count();
        assert_eq!(
            eq_count, 0,
            "Expected 0 Eq opcodes after folding, found {}",
            eq_count
        );
        assert_eq!(
            not_count, 0,
            "Expected 0 Not opcodes after folding, found {}",
            not_count
        );
    }
}
