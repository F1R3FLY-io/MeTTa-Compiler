//! WFST Transducer: CostClass → SchedulingAction.
//!
//! The scheduling transducer is a deterministic finite-state transducer that maps
//! each of the 8 cost classes to a concrete scheduling action. Since the transducer
//! is deterministic and has exactly 8 states, it is implemented as a flat array
//! lookup — O(1) per task.
//!
//! ## Transduction Table
//!
//! | CostClass          | Priority | ParDegree    | Affinity | Memo  |
//! |--------------------|----------|-------------|----------|-------|
//! | GroundCheap        | 0        | 1           | Any      | false |
//! | GroundArith        | 0        | 1           | Any      | true  |
//! | SymbolicCheap      | 2        | 1           | Sticky   | true  |
//! | SymbolicModerate   | 5        | branch_count| Any      | false |
//! | RecursiveBounded   | 8        | 1           | Sticky   | true  |
//! | RecursiveUnbounded | 10       | 1           | Sticky   | false |
//! | ParallelPure       | 3        | max_parallel| Any      | true  |
//! | ImpureSequential   | 5        | 1           | Sticky   | false |

use super::cost_class::{AffinityHint, CostClass, SchedulingAction};

// ══════════════════════════════════════════════════════════════════════════════
// Transducer
// ══════════════════════════════════════════════════════════════════════════════

/// Build the default transduction table.
///
/// The table is a flat `[SchedulingAction; 8]` array indexed by `CostClass as usize`.
/// Each entry specifies the scheduling parameters for the corresponding cost class.
pub fn build_default_transduction_table() -> [SchedulingAction; CostClass::COUNT] {
    [
        // GroundCheap (0): immediate evaluation, single-threaded, no memo
        SchedulingAction::new(0, 1, AffinityHint::Any, false),
        // GroundArith (1): immediate evaluation, memoizable (e.g., (+ 1 2) always = 3)
        SchedulingAction::new(0, 1, AffinityHint::Any, true),
        // SymbolicCheap (2): low priority, sticky for cache locality
        SchedulingAction::new(2, 1, AffinityHint::Sticky, true),
        // SymbolicModerate (3): normal priority, fan out to 4 workers
        SchedulingAction::new(5, 4, AffinityHint::Any, false),
        // RecursiveBounded (4): higher priority (finish chains), sticky
        SchedulingAction::new(8, 1, AffinityHint::Sticky, true),
        // RecursiveUnbounded (5): deprioritized (may diverge), sticky
        SchedulingAction::new(10, 1, AffinityHint::Sticky, false),
        // ParallelPure (6): moderate priority, high parallelism, memoizable
        SchedulingAction::new(3, 8, AffinityHint::Any, true),
        // ImpureSequential (7): normal priority, sequential, sticky
        SchedulingAction::new(5, 1, AffinityHint::Sticky, false),
    ]
}

/// Transduce a CostClass into a SchedulingAction with dynamic parallelism override.
///
/// When `branch_count > 1` and the class is `SymbolicModerate` or `ParallelPure`,
/// the parallelism degree is set to `min(branch_count, max_parallel)`.
#[inline]
pub fn transduce_with_branches(
    table: &[SchedulingAction; CostClass::COUNT],
    class: CostClass,
    branch_count: u8,
    max_parallel: u8,
) -> SchedulingAction {
    let mut action = table[class as usize];
    match class {
        CostClass::SymbolicModerate | CostClass::ParallelPure => {
            if branch_count > 1 {
                action.parallelism_degree = branch_count.min(max_parallel);
            }
        }
        _ => {}
    }
    action
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_table() {
        let table = build_default_transduction_table();
        assert_eq!(table.len(), CostClass::COUNT);

        // Ground cheap: highest priority, no memo
        assert_eq!(table[CostClass::GroundCheap as usize].priority_class, 0);
        assert!(!table[CostClass::GroundCheap as usize].memoizable);

        // Ground arith: highest priority, memoizable
        assert!(table[CostClass::GroundArith as usize].memoizable);

        // Impure: sequential, sticky
        assert_eq!(table[CostClass::ImpureSequential as usize].parallelism_degree, 1);
        assert_eq!(
            table[CostClass::ImpureSequential as usize].affinity_hint,
            AffinityHint::Sticky
        );
    }

    #[test]
    fn test_transduce_with_branches() {
        let table = build_default_transduction_table();

        // SymbolicModerate with 6 branches, max 8
        let action = transduce_with_branches(&table, CostClass::SymbolicModerate, 6, 8);
        assert_eq!(action.parallelism_degree, 6);

        // SymbolicModerate with 12 branches, max 8 → capped at 8
        let action = transduce_with_branches(&table, CostClass::SymbolicModerate, 12, 8);
        assert_eq!(action.parallelism_degree, 8);

        // GroundCheap ignores branch count
        let action = transduce_with_branches(&table, CostClass::GroundCheap, 10, 8);
        assert_eq!(action.parallelism_degree, 1);
    }
}
