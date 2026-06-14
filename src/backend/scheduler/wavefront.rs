//! Wavefront parallelism: dependency DAG and topological wave grouping.
//!
//! Provides structure-aware wavefront scheduling for dependency-aware task
//! batches. Tasks are grouped into waves based on their data dependencies:
//!
//! - **Wave 0**: Tasks with no unresolved dependencies (ready immediately)
//! - **Wave 1**: Tasks whose dependencies are all in Wave 0
//! - ...and so on...
//!
//! Within each wave, all tasks are independent and can execute in parallel.
//!
//! ## Common Cases
//!
//! - **Nondeterministic rule matches**: All branches are independent → single
//!   wave → full parallelism. Production direct fanout implements this
//!   all-independent refinement without calling `compute_wavefront`.
//! - **`let*` chains**: Each binding depends on previous → N waves of 1 task.
//! - **`if` branches**: then/else are independent (conditional on guard) → 2 waves.
//!
//! The general dependency-DAG scheduler is a verified library primitive. A
//! dependency-bearing instruction DAG must call this module with complete
//! dependency/effect-conflict edges before claiming wavefront reordering.
//!
//! ## Algorithm
//!
//! Uses Kahn's algorithm with level grouping (identical to `wavefront_schedule()`
//! from MeTTaIL2Matrix).

use super::cost_class::CostClass;

// ══════════════════════════════════════════════════════════════════════════════
// Wavefront task representation
// ══════════════════════════════════════════════════════════════════════════════

/// A task in the wavefront scheduler.
#[derive(Debug, Clone)]
pub struct WavefrontTask {
    /// Task index (position in the original task list).
    pub index: usize,
    /// Cost class from the tree automaton.
    pub cost_class: CostClass,
    /// Indices of tasks this task depends on.
    ///
    /// Callers must include data dependencies and effect-conflict edges. The
    /// wavefront builder treats missing edges as proof that same-wave tasks can
    /// commute safely.
    pub dependencies: Vec<usize>,
}

impl WavefrontTask {
    /// Create a new wavefront task with no dependencies.
    pub fn independent(index: usize, cost_class: CostClass) -> Self {
        WavefrontTask {
            index,
            cost_class,
            dependencies: Vec::new(),
        }
    }

    /// Create a new wavefront task with dependencies.
    pub fn dependent(index: usize, cost_class: CostClass, deps: Vec<usize>) -> Self {
        WavefrontTask {
            index,
            cost_class,
            dependencies: deps,
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Wavefront schedule
// ══════════════════════════════════════════════════════════════════════════════

/// A wavefront schedule: tasks grouped into parallel waves.
///
/// Each wave is a `Vec<usize>` of task indices that can execute concurrently.
/// Waves must execute in order (wave 0 before wave 1, etc.).
#[derive(Debug, Clone)]
pub struct WavefrontSchedule {
    /// Waves of task indices, ordered by dependency level.
    pub waves: Vec<Vec<usize>>,
    /// Total number of tasks.
    pub total_tasks: usize,
}

impl WavefrontSchedule {
    /// Whether all tasks are in a single wave (fully parallel).
    #[inline]
    pub fn is_fully_parallel(&self) -> bool {
        self.waves.len() == 1 && !self.waves[0].is_empty()
    }

    /// Whether all tasks are in separate waves (fully sequential).
    #[inline]
    pub fn is_fully_sequential(&self) -> bool {
        self.waves.iter().all(|w| w.len() <= 1)
    }

    /// Number of waves.
    #[inline]
    pub fn num_waves(&self) -> usize {
        self.waves.len()
    }

    /// Maximum parallelism (largest wave size).
    pub fn max_parallelism(&self) -> usize {
        self.waves.iter().map(|w| w.len()).max().unwrap_or(0)
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Wavefront computation (Kahn's algorithm with level grouping)
// ══════════════════════════════════════════════════════════════════════════════

/// Compute the wavefront schedule for a set of tasks.
///
/// Uses Kahn's topological sort with level grouping to identify parallel waves.
/// Tasks with no dependencies form wave 0; tasks whose dependencies are all
/// in prior waves form subsequent waves.
///
/// ## Complexity
///
/// - Time: O(V + E) where V = tasks, E = dependency edges
/// - Space: O(V + E)
///
/// For the common case (nondeterministic rule matches with no dependencies),
/// this is O(N) with a single wave.
pub fn compute_wavefront(tasks: &[WavefrontTask]) -> WavefrontSchedule {
    let n = tasks.len();

    if n == 0 {
        return WavefrontSchedule {
            waves: Vec::new(),
            total_tasks: 0,
        };
    }

    let well_formed_indices = tasks
        .iter()
        .enumerate()
        .all(|(expected, task)| task.index == expected);
    let well_formed_dependencies = tasks
        .iter()
        .all(|task| task.dependencies.iter().all(|&dep| dep < n));

    if !well_formed_indices || !well_formed_dependencies {
        return sequential_chain(n);
    }

    // Fast path: check if all tasks are independent (no dependencies)
    let all_independent = tasks.iter().all(|t| t.dependencies.is_empty());
    if all_independent {
        return WavefrontSchedule {
            waves: vec![(0..n).collect()],
            total_tasks: n,
        };
    }

    // Build in-degree counts and adjacency list (reverse edges)
    let mut in_degree = vec![0u32; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];

    for task in tasks {
        in_degree[task.index] = task.dependencies.len() as u32;
        for &dep in &task.dependencies {
            if dep < n {
                dependents[dep].push(task.index);
            }
        }
    }

    // Kahn's algorithm with level grouping
    let mut waves: Vec<Vec<usize>> = Vec::new();
    let mut current_wave: Vec<usize> = Vec::new();

    // Find initial ready tasks (in-degree = 0)
    for (i, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            current_wave.push(i);
        }
    }

    let mut processed = 0;

    while !current_wave.is_empty() {
        let mut next_wave: Vec<usize> = Vec::new();

        for &task_idx in &current_wave {
            processed += 1;
            for &dependent in &dependents[task_idx] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    next_wave.push(dependent);
                }
            }
        }

        waves.push(current_wave);
        current_wave = next_wave;
    }

    // If not all tasks were processed, there's a dependency cycle.
    // Keep the unresolved suffix sequential. A cycle cannot satisfy the
    // same-wave independence contract, so the safe fallback gives up
    // parallelism instead of claiming an invalid final wave.
    if processed < n {
        let remaining: Vec<usize> = (0..n).filter(|&i| in_degree[i] > 0).collect();
        for task_idx in remaining {
            waves.push(vec![task_idx]);
        }
    }

    WavefrontSchedule {
        waves,
        total_tasks: n,
    }
}

/// Convenience: compute wavefront for N independent tasks.
///
/// All tasks are placed in a single wave. This is the fast path for
/// nondeterministic rule match branches.
#[inline]
pub fn single_wave(n: usize) -> WavefrontSchedule {
    WavefrontSchedule {
        waves: vec![(0..n).collect()],
        total_tasks: n,
    }
}

/// Convenience: compute wavefront for a linear chain (fully sequential).
///
/// Each task depends on the previous one. N waves of 1 task each.
#[inline]
pub fn sequential_chain(n: usize) -> WavefrontSchedule {
    WavefrontSchedule {
        waves: (0..n).map(|i| vec![i]).collect(),
        total_tasks: n,
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_independent() {
        let tasks: Vec<WavefrontTask> = (0..5)
            .map(|i| WavefrontTask::independent(i, CostClass::SymbolicModerate))
            .collect();

        let schedule = compute_wavefront(&tasks);
        assert!(schedule.is_fully_parallel());
        assert_eq!(schedule.num_waves(), 1);
        assert_eq!(schedule.max_parallelism(), 5);
    }

    #[test]
    fn test_linear_chain() {
        let tasks = vec![
            WavefrontTask::independent(0, CostClass::SymbolicCheap),
            WavefrontTask::dependent(1, CostClass::SymbolicCheap, vec![0]),
            WavefrontTask::dependent(2, CostClass::SymbolicCheap, vec![1]),
        ];

        let schedule = compute_wavefront(&tasks);
        assert!(schedule.is_fully_sequential());
        assert_eq!(schedule.num_waves(), 3);
        assert_eq!(schedule.max_parallelism(), 1);
    }

    #[test]
    fn test_diamond_dag() {
        // Task 0 → Tasks 1,2 → Task 3
        let tasks = vec![
            WavefrontTask::independent(0, CostClass::SymbolicCheap),
            WavefrontTask::dependent(1, CostClass::SymbolicCheap, vec![0]),
            WavefrontTask::dependent(2, CostClass::SymbolicCheap, vec![0]),
            WavefrontTask::dependent(3, CostClass::SymbolicCheap, vec![1, 2]),
        ];

        let schedule = compute_wavefront(&tasks);
        assert_eq!(schedule.num_waves(), 3);
        // Wave 0: {0}, Wave 1: {1, 2}, Wave 2: {3}
        assert_eq!(schedule.waves[0], vec![0]);
        assert_eq!(schedule.waves[1].len(), 2);
        assert!(schedule.waves[1].contains(&1));
        assert!(schedule.waves[1].contains(&2));
        assert_eq!(schedule.waves[2], vec![3]);
    }

    #[test]
    fn test_empty() {
        let schedule = compute_wavefront(&[]);
        assert_eq!(schedule.num_waves(), 0);
        assert_eq!(schedule.total_tasks, 0);
    }

    #[test]
    fn test_single_wave_convenience() {
        let schedule = single_wave(4);
        assert!(schedule.is_fully_parallel());
        assert_eq!(schedule.max_parallelism(), 4);
    }

    #[test]
    fn test_sequential_chain_convenience() {
        let schedule = sequential_chain(3);
        assert!(schedule.is_fully_sequential());
        assert_eq!(schedule.num_waves(), 3);
    }

    #[test]
    fn test_mixed_parallel_sequential() {
        // Tasks 0,1 are independent; Task 2 depends on both; Tasks 3,4 depend on 2
        let tasks = vec![
            WavefrontTask::independent(0, CostClass::ParallelPure),
            WavefrontTask::independent(1, CostClass::ParallelPure),
            WavefrontTask::dependent(2, CostClass::SymbolicCheap, vec![0, 1]),
            WavefrontTask::dependent(3, CostClass::SymbolicCheap, vec![2]),
            WavefrontTask::dependent(4, CostClass::SymbolicCheap, vec![2]),
        ];

        let schedule = compute_wavefront(&tasks);
        assert_eq!(schedule.num_waves(), 3);
        // Wave 0: {0, 1}, Wave 1: {2}, Wave 2: {3, 4}
        assert_eq!(schedule.waves[0].len(), 2);
        assert_eq!(schedule.waves[1].len(), 1);
        assert_eq!(schedule.waves[2].len(), 2);
    }

    #[test]
    fn test_cycle_falls_back_to_sequential_suffix() {
        let tasks = vec![
            WavefrontTask::dependent(0, CostClass::SymbolicCheap, vec![1]),
            WavefrontTask::dependent(1, CostClass::SymbolicCheap, vec![0]),
        ];

        let schedule = compute_wavefront(&tasks);
        assert!(schedule.is_fully_sequential());
        assert_eq!(schedule.waves, vec![vec![0], vec![1]]);
    }

    #[test]
    fn test_invalid_dependency_falls_back_to_sequential() {
        let tasks = vec![
            WavefrontTask::independent(0, CostClass::SymbolicCheap),
            WavefrontTask::dependent(1, CostClass::SymbolicCheap, vec![7]),
        ];

        let schedule = compute_wavefront(&tasks);
        assert!(schedule.is_fully_sequential());
        assert_eq!(schedule.waves, vec![vec![0], vec![1]]);
    }

    #[test]
    fn test_non_positional_index_falls_back_to_sequential() {
        let tasks = vec![
            WavefrontTask::independent(0, CostClass::SymbolicCheap),
            WavefrontTask::independent(99, CostClass::SymbolicCheap),
        ];

        let schedule = compute_wavefront(&tasks);
        assert!(schedule.is_fully_sequential());
        assert_eq!(schedule.waves, vec![vec![0], vec![1]]);
    }
}
