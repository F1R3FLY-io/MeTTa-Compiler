//! NFA → DFA subset construction.
//!
//! Standard powerset construction that converts the NFA (with epsilon transitions
//! and wildcards) into a deterministic finite automaton using equivalence classes.

use std::collections::HashMap;

use super::equiv_classes::EquivClasses;
use super::nfa::{epsilon_closure, Nfa};

/// A DFA state during construction.
#[derive(Debug, Clone)]
pub struct DfaState {
    /// Transitions: one per equivalence class → target DFA state index.
    /// `DEAD` means no transition (dead state).
    pub transitions: Vec<u16>,
    /// If this is an accepting state, the pattern ID (lowest wins among NFA accept states).
    pub accept: Option<u16>,
}

/// A deterministic finite automaton.
#[derive(Debug)]
pub struct Dfa {
    pub states: Vec<DfaState>,
    pub start: usize,
    pub num_classes: usize,
}

/// Sentinel value for dead/invalid state transitions.
pub const DEAD: u16 = 0xFFFF;

/// Build a DFA from an NFA using subset construction with equivalence classes.
///
/// Each DFA state corresponds to a set of NFA states (the "subset"). Transitions
/// are computed by:
/// 1. For each equivalence class, pick a representative byte
/// 2. Follow all NFA transitions on that byte from the current subset
/// 3. Compute epsilon closure of the resulting NFA state set
/// 4. Map to existing DFA state or create a new one
pub fn subset_construction(nfa: &Nfa, classes: &EquivClasses) -> Dfa {
    let num_classes = classes.num_classes;

    // Precompute a representative byte for each equivalence class
    let mut class_representative = vec![0u8; num_classes];
    let mut class_seen = vec![false; num_classes];
    for byte in 0..=255u8 {
        let cls = classes.byte_to_class[byte as usize] as usize;
        if !class_seen[cls] {
            class_representative[cls] = byte;
            class_seen[cls] = true;
        }
    }

    // Start state = epsilon closure of NFA start
    let start_nfa_set = epsilon_closure(nfa, &[nfa.start]);

    let mut dfa_states: Vec<DfaState> = Vec::new();
    // Map from sorted NFA state set → DFA state index
    let mut state_map: HashMap<Vec<usize>, usize> = HashMap::new();
    let mut worklist: Vec<Vec<usize>> = Vec::new();

    // Create start DFA state
    let start_accept = best_accept(nfa, &start_nfa_set);
    dfa_states.push(DfaState {
        transitions: vec![DEAD; num_classes],
        accept: start_accept,
    });
    state_map.insert(start_nfa_set.clone(), 0);
    worklist.push(start_nfa_set);

    while let Some(current_nfa_set) = worklist.pop() {
        let current_dfa = *state_map
            .get(&current_nfa_set)
            .expect("state must exist in map");

        for class_id in 0..num_classes {
            let representative_byte = class_representative[class_id];

            // Compute the set of NFA states reachable by this byte from current set
            let mut target_nfa_states: Vec<usize> = Vec::new();
            for &nfa_state in &current_nfa_set {
                for &(byte, target) in &nfa.states[nfa_state].transitions {
                    if byte == representative_byte {
                        target_nfa_states.push(target);
                    }
                }
            }

            if target_nfa_states.is_empty() {
                continue; // transition stays DEAD
            }

            // Epsilon closure
            let target_set = epsilon_closure(nfa, &target_nfa_states);

            // Find or create DFA state for this NFA set
            let target_dfa = if let Some(&existing) = state_map.get(&target_set) {
                existing
            } else {
                let new_idx = dfa_states.len();
                let accept = best_accept(nfa, &target_set);
                dfa_states.push(DfaState {
                    transitions: vec![DEAD; num_classes],
                    accept,
                });
                state_map.insert(target_set.clone(), new_idx);
                worklist.push(target_set);
                new_idx
            };

            dfa_states[current_dfa].transitions[class_id] = target_dfa as u16;
        }
    }

    Dfa {
        states: dfa_states,
        start: 0,
        num_classes,
    }
}

/// Find the best (lowest ID) accepting pattern among a set of NFA states.
fn best_accept(nfa: &Nfa, nfa_states: &[usize]) -> Option<u16> {
    nfa_states
        .iter()
        .filter_map(|&s| nfa.states[s].accept)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::optimizer::equiv_classes::compute_equiv_classes;
    use crate::backend::bytecode::optimizer::nfa::build_nfa;
    use crate::backend::bytecode::optimizer::pattern_defs::all_pattern_defs;

    #[test]
    fn test_subset_construction() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);
        let classes = compute_equiv_classes(&nfa);
        let dfa = subset_construction(&nfa, &classes);

        // DFA should have a reasonable number of states
        assert!(dfa.states.len() > 1, "DFA should have more than 1 state");
        assert!(
            dfa.states.len() < 500,
            "DFA has too many states: {}",
            dfa.states.len()
        );

        // Start state should be 0
        assert_eq!(dfa.start, 0);

        // Should have some accepting states
        let num_accepting = dfa.states.iter().filter(|s| s.accept.is_some()).count();
        assert!(
            num_accepting > 0,
            "DFA should have at least one accepting state"
        );
    }

    #[test]
    fn test_nop_pattern_accepted() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);
        let classes = compute_equiv_classes(&nfa);
        let dfa = subset_construction(&nfa, &classes);

        // Feed Nop byte (0x00) to the DFA — should reach an accept state
        let nop_byte = crate::backend::bytecode::opcodes::Opcode::Nop.to_byte();
        let class = classes.byte_to_class[nop_byte as usize];
        let next_state = dfa.states[dfa.start].transitions[class as usize];
        assert_ne!(next_state, DEAD, "Nop should transition from start state");
        assert!(
            dfa.states[next_state as usize].accept.is_some(),
            "Nop should reach an accept state"
        );
    }
}
