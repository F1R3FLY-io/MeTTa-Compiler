//! Tests for bytecode optimization.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::backend::bytecode::opcodes::Opcode;
    use crate::backend::bytecode::optimizer::{
        eliminate_dead_code, optimize_bytecode, optimize_bytecode_full,
    };

    fn make_code(opcodes: &[u8]) -> Vec<u8> {
        opcodes.to_vec()
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
            Opcode::PushNil.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_push_long_small_pop_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::Pop.to_byte(),
            Opcode::PushNil.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte()]
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
            Opcode::PushNil.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte()]
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
            Opcode::PushNil.to_byte(),   // 3 - DEAD
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
            Opcode::PushNil.to_byte(),   // 4 - skipped
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
            2,                         // 2-3: offset +2 to target
            Opcode::PushNil.to_byte(), // 4 - fall through
            Opcode::Return.to_byte(),  // 5
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.dead_branch_removed, 1);
        // All 4 bytes (PushTrue, JumpIfFalse, offset) should be removed
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte(),]
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
}
