//! Hopcroft DFA minimization.
//!
//! Merges equivalent DFA states to produce the minimal DFA. Two states are
//! equivalent if they have the same accept status and, for every equivalence
//! class, their transitions lead to equivalent states.

use super::subset::{Dfa, DfaState, DEAD};

/// Minimize a DFA using Hopcroft's algorithm.
///
/// 1. Initial partition: group states by (accept status, pattern_id)
/// 2. Refine: split partitions where states disagree on which partition a
///    transition leads to
/// 3. Each final partition becomes one state in the minimal DFA
/// 4. BFS reorder so start state = 0
pub fn minimize(dfa: &Dfa) -> Dfa {
    let num_states = dfa.states.len();
    let num_classes = dfa.num_classes;

    if num_states == 0 {
        return Dfa {
            states: Vec::new(),
            start: 0,
            num_classes,
        };
    }

    // Step 1: Initial partition by accept status
    // Map each unique accept value to a partition ID
    let mut accept_to_partition: Vec<(Option<u16>, usize)> = Vec::new();
    let mut state_to_partition = vec![0usize; num_states];

    for (state_idx, state) in dfa.states.iter().enumerate() {
        let partition = accept_to_partition
            .iter()
            .find(|(acc, _)| *acc == state.accept)
            .map(|(_, pid)| *pid);

        match partition {
            Some(pid) => {
                state_to_partition[state_idx] = pid;
            }
            None => {
                let new_pid = accept_to_partition.len();
                accept_to_partition.push((state.accept, new_pid));
                state_to_partition[state_idx] = new_pid;
            }
        }
    }

    let mut num_partitions = accept_to_partition.len();

    // Step 2: Refine partitions until stable
    let mut changed = true;
    while changed {
        changed = false;

        // For each partition, check if all states agree on which partition
        // each transition class leads to
        let mut new_state_to_partition = state_to_partition.clone();
        let mut new_num_partitions = num_partitions;

        // Group states by current partition
        let mut partition_members: Vec<Vec<usize>> = vec![Vec::new(); num_partitions];
        for (state_idx, &pid) in state_to_partition.iter().enumerate() {
            partition_members[pid].push(state_idx);
        }

        for members in &partition_members {
            if members.len() <= 1 {
                continue; // Can't split a singleton
            }

            // Compute the "signature" of the first member
            let first = members[0];
            let first_sig: Vec<usize> = (0..num_classes)
                .map(|c| {
                    let target = dfa.states[first].transitions[c];
                    if target == DEAD {
                        usize::MAX // dead state gets its own partition
                    } else {
                        state_to_partition[target as usize]
                    }
                })
                .collect();

            // Check if all other members have the same signature
            for &member in &members[1..] {
                let member_sig: Vec<usize> = (0..num_classes)
                    .map(|c| {
                        let target = dfa.states[member].transitions[c];
                        if target == DEAD {
                            usize::MAX
                        } else {
                            state_to_partition[target as usize]
                        }
                    })
                    .collect();

                if member_sig != first_sig {
                    // Split: this member goes to a new partition
                    // Find or create a partition for this signature
                    new_state_to_partition[member] = new_num_partitions;
                    new_num_partitions += 1;
                    changed = true;
                }
            }
        }

        if changed {
            state_to_partition = new_state_to_partition;
            let _ = new_num_partitions; // used implicitly by re-normalization below

            // Re-normalize: merge states with identical signatures
            // that were split into separate new partitions
            let mut sig_to_partition: Vec<(Vec<usize>, usize)> = Vec::new();
            let mut normalized = vec![0usize; num_states];
            let mut next_pid = 0;

            for state_idx in 0..num_states {
                let sig: Vec<usize> = std::iter::once(
                    // Include accept status in signature
                    dfa.states[state_idx]
                        .accept
                        .map(|a| a as usize)
                        .unwrap_or(usize::MAX - 1),
                )
                .chain((0..num_classes).map(|c| {
                    let target = dfa.states[state_idx].transitions[c];
                    if target == DEAD {
                        usize::MAX
                    } else {
                        state_to_partition[target as usize]
                    }
                }))
                .collect();

                let found = sig_to_partition
                    .iter()
                    .find(|(s, _)| *s == sig)
                    .map(|(_, pid)| *pid);

                match found {
                    Some(pid) => {
                        normalized[state_idx] = pid;
                    }
                    None => {
                        normalized[state_idx] = next_pid;
                        sig_to_partition.push((sig, next_pid));
                        next_pid += 1;
                    }
                }
            }

            state_to_partition = normalized;
            num_partitions = next_pid;
        }
    }

    // Step 3: Build minimal DFA
    // Find a representative state for each partition
    let mut partition_rep = vec![0usize; num_partitions];
    let mut partition_seen = vec![false; num_partitions];
    for (state_idx, &pid) in state_to_partition.iter().enumerate() {
        if !partition_seen[pid] {
            partition_rep[pid] = state_idx;
            partition_seen[pid] = true;
        }
    }

    let mut min_states: Vec<DfaState> = Vec::with_capacity(num_partitions);
    for pid in 0..num_partitions {
        let rep = partition_rep[pid];
        let transitions: Vec<u16> = (0..num_classes)
            .map(|c| {
                let target = dfa.states[rep].transitions[c];
                if target == DEAD {
                    DEAD
                } else {
                    state_to_partition[target as usize] as u16
                }
            })
            .collect();
        min_states.push(DfaState {
            transitions,
            accept: dfa.states[rep].accept,
        });
    }

    let min_start = state_to_partition[dfa.start];

    // Step 4: BFS reorder so start = 0
    let reordered = bfs_reorder(
        Dfa {
            states: min_states,
            start: min_start,
            num_classes,
        },
    );

    reordered
}

/// BFS-reorder DFA states so that the start state is state 0.
fn bfs_reorder(dfa: Dfa) -> Dfa {
    let num_states = dfa.states.len();
    let num_classes = dfa.num_classes;

    if num_states == 0 || dfa.start == 0 {
        return dfa;
    }

    // BFS from start
    let mut old_to_new = vec![DEAD; num_states];
    let mut new_to_old: Vec<usize> = Vec::with_capacity(num_states);
    let mut queue = std::collections::VecDeque::new();

    old_to_new[dfa.start] = 0;
    new_to_old.push(dfa.start);
    queue.push_back(dfa.start);

    while let Some(old_idx) = queue.pop_front() {
        for c in 0..num_classes {
            let target = dfa.states[old_idx].transitions[c];
            if target != DEAD && old_to_new[target as usize] == DEAD {
                let new_idx = new_to_old.len() as u16;
                old_to_new[target as usize] = new_idx;
                new_to_old.push(target as usize);
                queue.push_back(target as usize);
            }
        }
    }

    // Add any unreachable states (shouldn't happen in a properly constructed DFA)
    for old_idx in 0..num_states {
        if old_to_new[old_idx] == DEAD {
            let new_idx = new_to_old.len() as u16;
            old_to_new[old_idx] = new_idx;
            new_to_old.push(old_idx);
        }
    }

    // Build reordered states
    let mut new_states: Vec<DfaState> = Vec::with_capacity(num_states);
    for &old_idx in &new_to_old {
        let old_state = &dfa.states[old_idx];
        let transitions: Vec<u16> = old_state
            .transitions
            .iter()
            .map(|&t| {
                if t == DEAD {
                    DEAD
                } else {
                    old_to_new[t as usize]
                }
            })
            .collect();
        new_states.push(DfaState {
            transitions,
            accept: old_state.accept,
        });
    }

    Dfa {
        states: new_states,
        start: 0,
        num_classes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::optimizer::equiv_classes::compute_equiv_classes;
    use crate::backend::bytecode::optimizer::nfa::build_nfa;
    use crate::backend::bytecode::optimizer::pattern_defs::all_pattern_defs;

    #[test]
    fn test_minimization_reduces_states() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);
        let classes = compute_equiv_classes(&nfa);
        let dfa = subset_construction_for_test(&nfa, &classes);
        let min_dfa = minimize(&dfa);

        // Minimal DFA should have fewer or equal states
        assert!(
            min_dfa.states.len() <= dfa.states.len(),
            "Minimal DFA ({}) should have ≤ states than original ({})",
            min_dfa.states.len(),
            dfa.states.len()
        );

        // Start state should be 0
        assert_eq!(min_dfa.start, 0);

        // Should preserve all accepting states' pattern IDs
        let orig_accepts: std::collections::HashSet<u16> = dfa
            .states
            .iter()
            .filter_map(|s| s.accept)
            .collect();
        let min_accepts: std::collections::HashSet<u16> = min_dfa
            .states
            .iter()
            .filter_map(|s| s.accept)
            .collect();
        assert_eq!(
            orig_accepts, min_accepts,
            "Minimization should preserve all accept pattern IDs"
        );
    }

    // Re-export subset_construction for test use
    fn subset_construction_for_test(
        nfa: &crate::backend::bytecode::optimizer::nfa::Nfa,
        classes: &crate::backend::bytecode::optimizer::equiv_classes::EquivClasses,
    ) -> Dfa {
        crate::backend::bytecode::optimizer::subset::subset_construction(nfa, classes)
    }
}
