//! NFA construction from declarative pattern definitions.
//!
//! Builds a non-deterministic finite automaton where each pattern becomes a
//! chain of states from a shared start state. Wildcards (`ByteMatch::Any`)
//! generate transitions for all 256 byte values to the same target state.

use super::pattern_defs::{ByteMatch, PatternDef};

/// A single NFA state.
#[derive(Debug, Clone)]
pub struct NfaState {
    /// Byte-labeled transitions: (byte_value, target_state_index).
    pub transitions: Vec<(u8, usize)>,
    /// Epsilon (unlabeled) transitions to other states.
    pub epsilon: Vec<usize>,
    /// If this is an accepting state, the pattern ID it accepts.
    pub accept: Option<u16>,
}

impl NfaState {
    fn new() -> Self {
        Self {
            transitions: Vec::new(),
            epsilon: Vec::new(),
            accept: None,
        }
    }
}

/// A non-deterministic finite automaton.
#[derive(Debug)]
pub struct Nfa {
    pub states: Vec<NfaState>,
    pub start: usize,
}

/// Build an NFA from a slice of pattern definitions.
///
/// Structure:
/// - State 0 is the start state
/// - Each pattern creates a chain of states: start →ε→ p0 →b0→ p1 →b1→ ... →bN→ pN (accept)
/// - `ByteMatch::Exact(b)` creates a single transition on byte `b`
/// - `ByteMatch::Any` creates 256 transitions (one per byte value) to the same target
pub fn build_nfa(patterns: &[PatternDef]) -> Nfa {
    let mut states = Vec::with_capacity(patterns.len() * 6);
    // Start state
    states.push(NfaState::new());
    let start = 0;

    for (pattern_id, pattern) in patterns.iter().enumerate() {
        // Create the first state of this pattern's chain
        let chain_start = states.len();
        states.push(NfaState::new());

        // Epsilon transition from global start to this pattern's start
        states[start].epsilon.push(chain_start);

        let mut current = chain_start;

        for byte_match in pattern.bytes.iter() {
            let next = states.len();
            states.push(NfaState::new());

            match byte_match {
                ByteMatch::Exact(b) => {
                    states[current].transitions.push((*b, next));
                }
                ByteMatch::Any => {
                    // All 256 byte values transition to the same state
                    for b in 0..=255u8 {
                        states[current].transitions.push((b, next));
                    }
                }
            }

            current = next;
        }

        // Mark final state as accepting
        states[current].accept = Some(pattern_id as u16);
    }

    Nfa { states, start }
}

/// Compute the epsilon closure of a set of NFA states.
///
/// Returns all states reachable from the input set via epsilon transitions only,
/// including the input states themselves. Result is sorted and deduplicated.
pub fn epsilon_closure(nfa: &Nfa, states: &[usize]) -> Vec<usize> {
    let mut result = Vec::with_capacity(states.len() * 2);
    let mut stack = Vec::from(states);
    let mut visited = vec![false; nfa.states.len()];

    for &s in states {
        if s < visited.len() {
            visited[s] = true;
        }
    }

    while let Some(s) = stack.pop() {
        result.push(s);
        for &target in &nfa.states[s].epsilon {
            if !visited[target] {
                visited[target] = true;
                stack.push(target);
            }
        }
    }

    result.sort_unstable();
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::optimizer::pattern_defs::all_pattern_defs;

    #[test]
    fn test_nfa_construction() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);

        // Start state should have epsilon transitions to each pattern
        assert_eq!(nfa.states[nfa.start].epsilon.len(), patterns.len());

        // Total states: 1 (start) + sum(1 + pattern.bytes.len()) for each pattern
        let expected_states: usize =
            1 + patterns.iter().map(|p| 1 + p.bytes.len()).sum::<usize>();
        assert_eq!(nfa.states.len(), expected_states);
    }

    #[test]
    fn test_epsilon_closure() {
        let patterns = all_pattern_defs();
        let nfa = build_nfa(&patterns);

        let closure = epsilon_closure(&nfa, &[nfa.start]);
        // Should include start state + all pattern chain starts
        assert_eq!(closure.len(), 1 + patterns.len());
    }
}
