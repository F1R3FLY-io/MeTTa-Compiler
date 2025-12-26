# Hybrid P2 Priority Scheduler

This document describes the Hybrid P2 Priority Scheduler, a sophisticated background task scheduler that optimizes compilation latency in MeTTaTron's tiered execution system.

## Table of Contents

1. [Introduction and Motivation](#introduction-and-motivation)
2. [The P² Algorithm](#the-p²-algorithm)
3. [Why "Hybrid"?](#why-hybrid)
4. [Sequential Mode Detection](#sequential-mode-detection)
5. [Priority Levels and Scoring](#priority-levels-and-scoring)
6. [Runtime Tracking](#runtime-tracking)
7. [Integration with Tiered Compilation](#integration-with-tiered-compilation)
8. [Worker Thread Architecture](#worker-thread-architecture)
9. [Implementation Reference](#implementation-reference)

---

## Introduction and Motivation

Background compilation is essential for tiered execution systems. When an expression becomes "hot" (executed frequently), we want to compile it to more efficient representations (bytecode, then JIT native code) without blocking the main execution path. This creates a scheduling problem: how do we order background compilation tasks?

Naive approaches have drawbacks:

- **FIFO (First-In-First-Out)**: Simple but ignores task importance. A low-priority batch job may block interactive compilation.
- **Static Priority**: Requires knowing task importance upfront. Compilation times vary widely.
- **Round-Robin**: Fair but not optimal. Some tasks are more urgent than others.

The ideal scheduler should:

1. Prioritize short tasks (Shortest Job First reduces average wait time)
2. Prevent starvation of low-priority tasks
3. Adapt to actual runtime characteristics
4. Have low overhead (scheduling shouldn't cost more than the work)

The P2 Priority Scheduler addresses all these requirements.

---

## The P² Algorithm

The heart of the scheduler is the **P² (Piecewise-Parabolic) algorithm** for estimating task runtimes. This algorithm, published by Jain and Chlamtac in 1985, provides dynamic quantile estimation without storing observations.

### The Problem

To implement Shortest Job First (SJF) scheduling, we need to estimate how long a task will take. But storing all past runtimes would consume unbounded memory. We need a streaming algorithm.

### The Solution: Five Markers

P² maintains only 5 markers that track quantile estimates:

```text
q[0] = minimum
q[1] = 25th percentile estimate
q[2] = median (50th percentile) estimate
q[3] = 75th percentile estimate
q[4] = maximum
```

Each marker has:
- **Height (q_i)**: The actual quantile value estimate
- **Position (n_i)**: Integer index in the conceptual sorted sequence
- **Desired position (n'_i)**: Target position as a real number

### Algorithm Steps

For each new observation x:

1. **Find bracket k** where q[k] <= x < q[k+1]

2. **Update extremes** if x is new min or max

3. **Increment positions** for markers above k

4. **Update desired positions** by adding increments:
   - For median: increments = [0.0, 0.25, 0.5, 0.75, 1.0]

5. **Adjust middle markers** (1, 2, 3) using P² formula when their position deviates from desired

### The P² Formula

When marker i needs adjustment by direction d (±1):

```text
q'_i = q_i + (d / (n_{i+1} - n_{i-1})) ×
       [(n_i - n_{i-1} + d)(q_{i+1} - q_i)/(n_{i+1} - n_i) +
        (n_{i+1} - n_i - d)(q_i - q_{i-1})/(n_i - n_{i-1})]
```

This parabolic interpolation adjusts marker heights to better approximate the true quantile. If the result falls outside valid bounds, a simpler linear interpolation is used as fallback.

### Properties

- **O(1) space**: Only 5 markers stored
- **O(1) time**: Constant operations per observation
- **Accuracy**: Converges to true quantiles for most distributions

### Implementation

```rust
// From src/backend/priority_scheduler.rs:47-197
pub struct P2MedianEstimator {
    heights: [f64; 5],           // Marker heights (quantile estimates)
    positions: [i32; 5],         // Marker positions in sorted sequence
    desired_positions: [f64; 5], // Target positions
    increments: [f64; 5],        // Position increments per observation
    count: u32,                  // Observations seen
}

impl P2MedianEstimator {
    pub fn median(&self) -> f64 {
        self.heights[2] // Center marker is the median
    }
}
```

---

## Why "Hybrid"?

The scheduler is called "hybrid" because it switches between two scheduling strategies based on workload characteristics.

### The Overhead Problem

Priority scheduling has overhead:
- Calculating priority scores: ~50-100ns
- Heap operations: ~100-200ns
- Context switch to worker: ~500-1000ns

For sequential workloads with one evaluation at a time, this overhead exceeds the benefit. Rayon's work-stealing scheduler has lower per-task overhead (~200-300ns) and is optimized for batch throughput.

### The Hybrid Strategy

```text
┌─────────────────────────────────────────────────────────┐
│                  Hybrid Scheduler Decision              │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  concurrent_evals < 2?                                  │
│         │                                               │
│    ┌────┴────┐                                          │
│    │         │                                          │
│   YES       NO                                          │
│    │         │                                          │
│    ▼         ▼                                          │
│  Rayon    P2 Scheduler                                  │
│  spawn    spawn_with_priority                           │
│    │         │                                          │
│  Low       Smart                                        │
│  overhead  ordering                                     │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

- **Sequential mode** (< 2 concurrent evals): Use Rayon's `spawn()` for minimal overhead
- **Parallel mode** (≥ 2 concurrent evals): Use P2 scheduler for priority-based ordering

The threshold of 2 was determined empirically. At 2+ concurrent evaluations, the benefits of smart scheduling (shorter wait times, better interactive response) outweigh the overhead.

---

## Sequential Mode Detection

Sequential mode detection uses a simple atomic counter with lock-free operations.

### Mechanism

```rust
// From src/backend/bytecode/tiered_cache.rs:40-85
static CONCURRENT_EVALS: AtomicUsize = AtomicUsize::new(0);
const SEQUENTIAL_THRESHOLD: usize = 2;

pub fn is_sequential_mode() -> bool {
    CONCURRENT_EVALS.load(Ordering::Relaxed) < SEQUENTIAL_THRESHOLD
}

pub fn enter_eval() {
    CONCURRENT_EVALS.fetch_add(1, Ordering::Relaxed);
}

pub fn exit_eval() {
    CONCURRENT_EVALS.fetch_sub(1, Ordering::Relaxed);
}
```

### Instrumentation

The eval function is instrumented with enter/exit calls:

```rust
// From src/backend/eval/mod.rs:106-170
pub fn eval(value: MettaValue, env: Environment) -> EvalResult {
    #[cfg(feature = "hybrid-p2-priority-scheduler")]
    enter_eval();  // Increment counter

    // ... evaluation logic ...

    #[cfg(feature = "hybrid-p2-priority-scheduler")]
    exit_eval();   // Decrement counter
    result
}
```

### Why Relaxed Ordering?

We use `Ordering::Relaxed` because:

1. **Approximate is sufficient**: The exact count doesn't need to be precise. Even if we occasionally use the "wrong" scheduler, correctness is maintained.

2. **Performance critical**: This code runs on every evaluation. Stronger ordering would add memory barriers.

3. **Monotonicity not required**: Brief inconsistencies between threads are acceptable.

---

## Priority Levels and Scoring

Tasks are assigned priority levels that influence their scheduling order.

### Predefined Levels

```rust
// From src/backend/priority_scheduler.rs:343-358
pub mod priority_levels {
    pub const INTERACTIVE: u32 = 0;         // Highest priority
    pub const NORMAL: u32 = 5;              // Standard evaluation
    pub const BACKGROUND_COMPILE: u32 = 10; // Bytecode/JIT compilation
    pub const LOW: u32 = 20;                // Low-priority background work
    pub const BATCH: u32 = 50;              // Lowest priority batch jobs
}
```

Lower numeric values = higher priority.

### Score Calculation

The effective priority score combines multiple factors:

```text
score = base_priority + (estimated_runtime × runtime_weight) - (age × decay_rate)
```

Where:
- **base_priority**: User-specified level (0 = highest)
- **estimated_runtime**: P² median of past runtimes for similar tasks (in seconds)
- **runtime_weight**: Coefficient for SJF behavior (default: 1.0)
- **age**: Time since task was enqueued (seconds)
- **decay_rate**: How fast priority increases over time (default: 0.1/second)

### Score Properties

1. **Lower score = scheduled first** (min-heap)
2. **Short tasks get lower scores** (SJF approximation)
3. **Old tasks get lower scores** (starvation prevention)
4. **Configurable weights** allow tuning for different workloads

### Implementation

```rust
// From src/backend/priority_scheduler.rs:398-415
pub fn score(&self, runtime_tracker: &RuntimeTracker, config: &SchedulerConfig) -> f64 {
    let base = self.base_priority as f64;

    // SJF component: prefer shorter tasks
    let estimated_runtime = runtime_tracker.estimated_runtime(self.task_type);
    let runtime_component = (estimated_runtime / 1_000_000_000.0) * config.runtime_weight;

    // Age component: prevent starvation
    let age_secs = self.enqueued_at.elapsed().as_secs_f64();
    let age_component = age_secs * config.decay_rate;

    base + runtime_component - age_component
}
```

---

## Runtime Tracking

The scheduler tracks runtimes per task type to improve SJF estimates.

### Task Type Classification

```rust
// From src/backend/priority_scheduler.rs:206-230
pub enum TaskTypeId {
    Eval(u64),         // Expression eval with structural hash
    BytecodeCompile,   // Bytecode compilation
    JitCompile,        // JIT compilation (Stage 1 or 2)
    Generic,           // Unclassified tasks
}
```

For `Eval` tasks, the expression's structural hash groups similar expressions together. Expressions with similar structure tend to have similar runtimes.

### Per-Type Estimators

```rust
// From src/backend/priority_scheduler.rs:239-303
pub struct RuntimeTracker {
    estimators: DashMap<u64, Mutex<P2MedianEstimator>>,  // Per-type
    global_estimator: Mutex<P2MedianEstimator>,          // Fallback
}
```

Each task type gets its own P² estimator. When a task type has fewer than 5 observations, the global estimator provides a fallback.

### Why Full-Hash Tracking?

We use the full expression hash rather than a coarser categorization because:

1. **Expression similarity correlates with runtime**: Structurally identical expressions have similar execution characteristics.

2. **Hot expressions dominate**: By Zipf's law, a small number of expressions account for most executions. These benefit most from accurate per-expression tracking.

3. **DashMap overhead is acceptable**: Lock-free concurrent access makes per-hash tracking practical.

---

## Integration with Tiered Compilation

The P2 scheduler integrates with MeTTaTron's tiered compilation system.

### Tier Thresholds

```text
Tier 0: Interpreter (0-1 executions)
Tier 1: Bytecode VM (2+ executions)
Tier 2: JIT Stage 1 (100+ executions)
Tier 3: JIT Stage 2 (500+ executions)
```

### Compilation Task Spawning

When a threshold is crossed, compilation is spawned as a background task:

```rust
// From src/backend/bytecode/tiered_cache.rs:634-653
#[cfg(feature = "hybrid-p2-priority-scheduler")]
{
    if is_sequential_mode() {
        rayon::spawn(compile_task);  // Low overhead path
    } else {
        global_priority_eval_pool().spawn_with_priority(
            compile_task,
            priority_levels::BACKGROUND_COMPILE,
            TaskTypeId::BytecodeCompile,
        );
    }
}

#[cfg(not(feature = "hybrid-p2-priority-scheduler"))]
{
    rayon::spawn(compile_task);  // Always use Rayon without feature
}
```

### Priority Assignment

Compilation tasks use `BACKGROUND_COMPILE` priority (10), which is lower than `NORMAL` evaluation (5). This ensures compilation doesn't starve interactive evaluations while still completing in reasonable time.

---

## Worker Thread Architecture

The P2 scheduler uses a persistent thread pool with blocking workers.

### Pool Structure

```rust
// From src/backend/priority_scheduler.rs:646-667
pub struct PriorityEvalThreadPool {
    queue: Arc<PriorityQueue>,           // Shared priority queue
    runtime_tracker: Arc<RuntimeTracker>, // Shared P² trackers
    workers: Vec<JoinHandle<()>>,        // Worker threads
    shutdown: Arc<AtomicBool>,           // Shutdown signal
    num_threads: usize,                  // Worker count
    config: SchedulerConfig,             // Tuning parameters
    sequence: AtomicU64,                 // Task ordering
}
```

### Priority Queue

```rust
// From src/backend/priority_scheduler.rs:470-555
pub struct PriorityQueue {
    heap: Mutex<BinaryHeap<ScoredTask>>,  // Min-heap by score
    runtime_tracker: Arc<RuntimeTracker>,
    config: SchedulerConfig,
    not_empty: Condvar,                   // Wake blocked workers
    count: Mutex<usize>,                  // Task count
}
```

The queue uses a `BinaryHeap` with reversed comparison to implement a min-heap (lowest score = highest priority).

### Worker Loop

Each worker runs a simple loop:

```rust
// From src/backend/priority_scheduler.rs:793-823
fn priority_worker_loop(
    queue: Arc<PriorityQueue>,
    runtime_tracker: Arc<RuntimeTracker>,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        if shutdown.load(Ordering::SeqCst) { break; }

        match queue.pop_blocking(&shutdown) {
            Some(task) => {
                let task_type = task.task_type();
                let runtime_nanos = task.execute();  // Time the task
                runtime_tracker.record_runtime(task_type, runtime_nanos);
            }
            None => {
                if shutdown.load(Ordering::SeqCst) { break; }
            }
        }
    }
}
```

Workers:
1. Block on `Condvar` when queue is empty
2. Pop highest-priority task (lowest score)
3. Execute and measure runtime
4. Record runtime for future P² estimates
5. Repeat

### Graceful Shutdown

The pool supports graceful shutdown:

```rust
// From src/backend/priority_scheduler.rs:782-789
pub fn shutdown(self) {
    self.shutdown.store(true, Ordering::SeqCst);
    self.queue.notify_all();  // Wake blocked workers

    for worker in self.workers {
        let _ = worker.join();
    }
}
```

---

## Implementation Reference

### Source Files

| File | Content |
|------|---------|
| `src/backend/priority_scheduler.rs` | P² algorithm, RuntimeTracker, PriorityQueue, thread pool |
| `src/backend/bytecode/tiered_cache.rs` | Sequential mode detection, compilation spawning |
| `src/backend/eval/mod.rs` | enter_eval/exit_eval instrumentation |

### Feature Flag

The hybrid P2 scheduler is feature-gated:

```toml
# Cargo.toml
[features]
hybrid-p2-priority-scheduler = []
```

Without this feature, all background compilation uses Rayon directly. This provides compatibility with projects (like Rholang) that share a Rayon scheduler.

### Configuration

```rust
// From src/backend/priority_scheduler.rs:310-336
pub struct SchedulerConfig {
    pub runtime_weight: f64,   // SJF coefficient (default: 1.0)
    pub decay_rate: f64,       // Starvation prevention (default: 0.1/sec)
    pub max_queue_size: usize, // Backpressure limit (default: num_cpus × 16)
}
```

### Global Access

The scheduler provides a global instance:

```rust
// From src/backend/priority_scheduler.rs:632-639
static GLOBAL_PRIORITY_POOL: LazyLock<PriorityEvalThreadPool> =
    LazyLock::new(|| {
        let num_threads = num_cpus::get();
        PriorityEvalThreadPool::new(num_threads, SchedulerConfig::default())
    });

pub fn global_priority_eval_pool() -> &'static PriorityEvalThreadPool {
    &GLOBAL_PRIORITY_POOL
}
```

---

## References

1. Jain, R. and Chlamtac, I. "The P² algorithm for dynamic calculation of quantiles and histograms without storing observations." Communications of the ACM 28, no. 10 (1985): 1076-1085.

2. See also: `docs/THREADING_MODEL.md` for the overall threading architecture.

3. See also: `docs/optimization/jit/JIT_PIPELINE_ARCHITECTURE.md` for tiered compilation details.
