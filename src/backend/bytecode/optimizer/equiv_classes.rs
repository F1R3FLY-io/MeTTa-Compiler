//! Equivalence class partitioning for DFA byte inputs.
//!
//! Reduces the 256-byte alphabet to ~20-30 equivalence classes by grouping bytes
//! that behave identically across all NFA transitions. This shrinks the DFA
//! transition table by ~10x without losing any information.

use super::nfa::Nfa;

/// Result of equivalence class computation.
#[derive(Debug, Clone)]
pub struct EquivClasses {
    /// Maps each byte (0-255) to its equivalence class ID.
    pub byte_to_class: [u8; 256],
    /// Number of distinct equivalence classes.
    pub num_classes: usize,
}

/// Compute equivalence classes from the NFA.
///
/// Two bytes are in the same class if and only if they produce identical
/// transition behavior in every NFA state: for every state, either both bytes
/// have no transitions, or both bytes transition to the exact same set of
/// target states.
pub fn compute_equiv_classes(nfa: &Nfa) -> EquivClasses {
    // For each byte, compute a signature: the set of (state, target_states) pairs
    // across all NFA states. Bytes with identical signatures get the same class.

    // Build a per-state transition map: state → byte → sorted target set
    // Compute a signature for each byte value
    // signature[byte] = Vec of (state_idx, sorted_targets) for each NFA state that has transitions on this byte
    let mut signatures: Vec<Vec<(usize, Vec<usize>)>> = Vec::with_capacity(256);

    for byte_val in 0..=255u8 {
        let mut sig = Vec::new();
        for (state_idx, state) in nfa.states.iter().enumerate() {
            let mut targets: Vec<usize> = state
                .transitions
                .iter()
                .filter(|(b, _)| *b == byte_val)
                .map(|(_, t)| *t)
                .collect();
            if !targets.is_empty() {
                targets.sort_unstable();
                targets.dedup();
                sig.push((state_idx, targets));
            }
        }
        signatures.push(sig);
    }

    // Group bytes by identical signatures
    let mut byte_to_class = [0u8; 256];
    let mut next_class: u8 = 0;
    let mut class_map: Vec<(Vec<(usize, Vec<usize>)>, u8)> = Vec::new();

    for byte_val in 0..=255u8 {
        let sig = &signatures[byte_val as usize];
        let mut found = false;
        for (existing_sig, class_id) in &class_map {
            if sig == existing_sig {
                byte_to_class[byte_val as usize] = *class_id;
                found = true;
                break;
            }
        }
        if !found {
            byte_to_class[byte_val as usize] = next_class;
            class_map.push((sig.clone(), next_class));
            next_class += 1;
        }
    }

    // Ensure we don't overflow u8 (should never happen with ~42 patterns, expect ~30 classes)
    assert!(
        (next_class as usize) <= 256,
        "Too many equivalence classes: {next_class} (max 256)"
    );

    EquivClasses {
        byte_to_class,
        num_classes: next_class as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::optimizer::nfa::build_nfa;
    use crate::backend::bytecode::optimizer::pattern_defs::all_pattern_defs;

    #[test]
    fn test_equiv_classes_reasonable_count() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);
        let classes = compute_equiv_classes(&nfa);

        // With ~42 patterns using ~30 distinct opcode values, expect ~20-60 classes
        assert!(
            classes.num_classes > 5,
            "Too few classes: {}",
            classes.num_classes
        );
        assert!(
            classes.num_classes <= 100,
            "Too many classes: {}",
            classes.num_classes
        );

        // All byte values should map to a valid class
        for &c in &classes.byte_to_class {
            assert!((c as usize) < classes.num_classes);
        }
    }

    #[test]
    fn test_equiv_classes_distinct_opcodes() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);
        let classes = compute_equiv_classes(&nfa);

        // Opcodes that appear in different pattern positions should generally
        // get different equivalence classes (e.g., Swap vs Lt)
        let swap = crate::backend::bytecode::opcodes::Opcode::Swap.to_byte();
        let lt = crate::backend::bytecode::opcodes::Opcode::Lt.to_byte();
        assert_ne!(
            classes.byte_to_class[swap as usize], classes.byte_to_class[lt as usize],
            "Swap and Lt should be in different equivalence classes"
        );
    }
}
