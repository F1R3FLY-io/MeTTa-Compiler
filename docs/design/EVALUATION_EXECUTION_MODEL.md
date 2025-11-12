# MeTTa Evaluation Execution Model

## Table of Contents

1. [Introduction](#introduction)
2. [Two Dimensions: Nondeterminism vs Parallelism](#two-dimensions-nondeterminism-vs-parallelism)
3. [Nondeterministic Evaluation Semantics](#nondeterministic-evaluation-semantics)
4. [Parallel Execution Architecture](#parallel-execution-architecture)
5. [Threading Model](#threading-model)
6. [Batch Evaluation](#batch-evaluation)
7. [Thread Safety and Synchronization](#thread-safety-and-synchronization)
8. [Performance Characteristics](#performance-characteristics)
9. [Integration with Rholang](#integration-with-rholang)
10. [Configuration and Tuning](#configuration-and-tuning)
11. [Debugging and Profiling](#debugging-and-profiling)
12. [Future Enhancements](#future-enhancements)

---

## Introduction

MeTTa's evaluation model operates on two distinct but often confused dimensions:

1. **Nondeterministic Evaluation** (Semantic) - The language semantics that allow multiple valid results from a single evaluation
2. **Parallel Execution** (Performance) - The runtime implementation that executes independent computations concurrently

This document provides a comprehensive analysis of both aspects, explaining how they interact and how to leverage them for building high-performance symbolic reasoning systems.

### Key Distinction

**Nondeterminism ≠ Parallelism**

- **Nondeterminism**: A property of the *language semantics* where multiple rule definitions or pattern matches produce multiple results
- **Parallelism**: A property of the *runtime implementation* where independent evaluations execute simultaneously on multiple CPU cores

**Example:**
```metta
% Nondeterministic (semantic): Multiple valid results
(= (color) red)
(= (color) blue)
!(color)  → [red, blue]  % Both are valid results

% Sequential evaluation: Results computed one after another
% NOT parallel (even though there are multiple results)
```

**Contrast with Parallel Execution:**
```metta
% Independent evaluations can run in parallel
!(+ 1 2)  % Evaluation 1
!(+ 3 4)  % Evaluation 2 (can run simultaneously with Evaluation 1)
```

---

## Two Dimensions: Nondeterminism vs Parallelism

### Nondeterminism (Semantic Model)

**Definition:** The ability of an expression to evaluate to multiple valid results based on multiple matching rules or patterns.

**Characteristics:**
- **Declarative**: Multiple rule definitions describe all valid solutions
- **Deterministic ordering**: Results are produced in a predictable order (rule definition order)
- **Sequential within evaluation**: Each evaluation tries rules one by one
- **Cartesian product semantics**: Nested nondeterminism creates combinations

**Example from `robot_planning.rho`:**
```metta
(= (find_any_path $from $to) (find_path_1hop $from $to))
(= (find_any_path $from $to) (find_path_2hop $from $to))
(= (find_any_path $from $to) (find_path_3hop $from $to))

!(find_any_path room_a room_d)
% Tries all three rules sequentially
% Returns all valid paths found: [(path room_a room_e room_d), ...]
```

**Why Sequential?**
- Ensures deterministic result ordering
- Simplifies debugging and testing
- Prevents race conditions in result collection
- Matches mathematical semantics of logic programming

### Parallelism (Execution Model)

**Definition:** The runtime capability to execute multiple independent evaluations simultaneously on different CPU cores.

**Characteristics:**
- **Implementation detail**: Not visible in language semantics
- **Non-deterministic ordering**: Results may arrive in any order (but are sorted before returning)
- **True concurrency**: Leverages multi-core hardware
- **Batch-level**: Parallelism occurs across expressions, not within single evaluation

**Example:**
```rholang
% Rholang: Launch multiple MeTTa queries in parallel
robotAPI!("find_path", "room_a", "room_d", *r1) |
robotAPI!("find_path", "room_b", "room_c", *r2) |
robotAPI!("find_path", "room_c", "room_e", *r3) |

% All three queries execute in parallel
% Each uses a separate thread from the blocking thread pool
```

### Comparison Table

| Aspect | Nondeterminism | Parallelism |
|--------|----------------|-------------|
| **Nature** | Language semantics | Runtime implementation |
| **Visibility** | Explicit in code (multiple rules) | Hidden from programmer |
| **Ordering** | Deterministic (rule order) | Non-deterministic (arrival order) |
| **Execution** | Sequential (within evaluation) | Concurrent (across evaluations) |
| **Purpose** | Express multiple solutions | Improve performance |
| **Control** | Via rule definitions | Via thread pool configuration |
| **Example** | `(= (f) 1) (= (f) 2)` | Batch evaluation, spawn_blocking |

---

## Nondeterministic Evaluation Semantics

### Multiple Rule Definitions

**Core Mechanism:**
When multiple rules match the same pattern, MeTTa evaluates **all of them** and returns **all non-empty results**.

**Example:**
```metta
(= (animal) cat)
(= (animal) dog)
(= (animal) bird)

!(animal)
% Evaluation:
%   1. Try rule 1: cat → [cat]
%   2. Try rule 2: dog → [dog]
%   3. Try rule 3: bird → [bird]
%   4. Collect: [cat, dog, bird]
```

**Implementation** (`src/backend/eval/mod.rs`):
```rust
fn eval_sexpr(items: &[MettaValue], env: &Environment) -> Vec<MettaValue> {
    // 1. Evaluate sub-expressions
    let eval_results = items.iter().map(|item| eval(item, env)).collect();

    // 2. Generate Cartesian product
    let combinations = cartesian_product(eval_results);

    // 3. For each combination, try all matching rules
    let mut all_results = Vec::new();
    for combo in combinations {
        let matches = env.find_all_matching_rules(&combo);
        for (rhs, bindings) in matches {
            let instantiated = apply_bindings(&rhs, &bindings);
            let results = eval(instantiated, env);
            all_results.extend(results);
        }
    }

    all_results
}
```

**Key Points:**
- Rules are tried **sequentially** (not in parallel)
- Results are collected in **rule definition order**
- Empty results are **filtered out**

### Cartesian Product Semantics

**Definition:**
When an expression contains multiple sub-expressions that each produce multiple results, the evaluation produces the **Cartesian product** of all combinations.

**Example:**
```metta
(= (a) 1)
(= (a) 2)
(= (b) 10)
(= (b) 20)

!(+ (a) (b))

% Evaluation:
%   (a) → [1, 2]
%   (b) → [10, 20]
%   Cartesian product: {1, 2} × {10, 20} = {(1,10), (1,20), (2,10), (2,20)}
%   Apply + to each:
%     + 1 10 = 11
%     + 1 20 = 21
%     + 2 10 = 12
%     + 2 20 = 22
%   Result: [11, 21, 12, 22]
```

**Nested Nondeterminism:**
```metta
(= (neighbors room_a) room_b)
(= (neighbors room_a) room_e)
(= (neighbors room_b) room_a)
(= (neighbors room_b) room_c)

!(neighbors (neighbors room_a))
% Evaluation:
%   (neighbors room_a) → [room_b, room_e]
%   For room_b: (neighbors room_b) → [room_a, room_c]
%   For room_e: (neighbors room_e) → [room_a, room_d]
%   Combined: [room_a, room_c, room_a, room_d]
%   (may include duplicates)
```

**Mathematical Foundation:**
```
Eval: Expression → Environment → Set<Value>

Eval((f e₁ e₂ ... eₙ), env) =
  ⋃ {f(v₁, v₂, ..., vₙ) | v₁ ∈ Eval(e₁, env),
                           v₂ ∈ Eval(e₂, env),
                           ...,
                           vₙ ∈ Eval(eₙ, env)}
```

### Pattern Matching Nondeterminism

**Using `match & self`:**
```metta
(connected room_a room_b)
(connected room_a room_e)
(connected room_b room_c)

(= (all_neighbors $room)
   (match & self (connected $room $target) $target))

!(all_neighbors room_a)
% Pattern: (connected room_a $target)
% Matches:
%   (connected room_a room_b) → {$target ↦ room_b}
%   (connected room_a room_e) → {$target ↦ room_e}
% Results: [room_b, room_e]
```

**MORK Pattern Matching** (`src/backend/eval/space.rs`):
1. Convert pattern to MORK binary format (with De Bruijn indices for variables)
2. Query MORK trie with `query_multi()` → O(m) where m = number of matches
3. For each match, extract bindings
4. Apply bindings to template
5. Return all instantiated templates

**Performance:**
- **Time Complexity**: O(m) where m = number of matches
- **Space Complexity**: O(m × template size)
- **Advantage**: Prefix-based search in trie is much faster than linear scan

---

## Parallel Execution Architecture

### High-Level Overview

MeTTa's parallel execution operates at the **expression batch level**, not within individual evaluations.

**Key Principle:**
> Independent evaluations can run in parallel;
> Dependent evaluations must run sequentially.

**Example:**
```metta
% Independent (can parallelize):
!(locate ball1)
!(locate box1)
!(locate key1)

% Dependent (must be sequential):
(= (fact) data)              % Rule definition
!(query-using-fact)          % Depends on rule being defined
```

### Parallelization Strategy

**Batch Evaluation Algorithm** (`src/rholang_integration.rs:239-350`):

```rust
pub async fn run_state_async(
    accumulated_state: MettaState,
    compiled_state: MettaState,
) -> Result<MettaState, String> {
    let mut env = accumulated_state.environment.clone();
    let mut current_batch: Vec<(usize, MettaValue, bool)> = Vec::new();
    let mut all_results = Vec::new();

    for (idx, expr) in compiled_state.source.into_iter().enumerate() {
        match classify_expression(&expr) {
            ExprType::RuleDefinition => {
                // Force batch boundary: evaluate accumulated batch
                if !current_batch.is_empty() {
                    let batch_results = evaluate_batch_parallel(
                        current_batch,
                        env.clone()
                    ).await;
                    all_results.extend(batch_results);
                    current_batch.clear();
                }

                // Evaluate rule sequentially (modifies environment)
                eval(expr, &mut env);
            }
            ExprType::EvalExpression => {
                // Add to batch for parallel evaluation
                let should_output = matches!(&expr, MettaValue::SExpr(items)
                    if items[0] == MettaValue::Atom("!".to_string()));
                current_batch.push((idx, expr, should_output));
            }
        }
    }

    // Evaluate remaining batch
    if !current_batch.is_empty() {
        let batch_results = evaluate_batch_parallel(
            current_batch,
            env.clone()
        ).await;
        all_results.extend(batch_results);
    }

    // Sort results by original index to preserve ordering
    all_results.sort_by_key(|(idx, _, _)| *idx);

    Ok(MettaState {
        environment: env,
        output: all_results.into_iter().map(|(_, res, _)| res).collect(),
        ..accumulated_state
    })
}

async fn evaluate_batch_parallel(
    batch: Vec<(usize, MettaValue, bool)>,
    env: Environment,
) -> Vec<(usize, Vec<MettaValue>, bool)> {
    // Spawn parallel tasks using spawn_blocking
    let tasks: Vec<_> = batch.into_iter()
        .map(|(idx, expr, should_output)| {
            let env_clone = env.clone();  // Arc clone (cheap)
            tokio::task::spawn_blocking(move || {
                let (results, _) = eval(expr, &env_clone);
                (idx, results, should_output)
            })
        })
        .collect();

    // Await all tasks
    let mut results = Vec::new();
    for task_handle in tasks {
        results.push(task_handle.await.unwrap());
    }

    results
}
```

**Key Mechanisms:**
1. **Batch accumulation**: Consecutive eval expressions added to batch
2. **Batch boundaries**: Rule definitions force immediate batch evaluation
3. **Parallel spawning**: Each batch item spawned with `spawn_blocking`
4. **Result sorting**: Results sorted by original index to preserve order
5. **Environment sharing**: All batch items share same environment via `Arc<Mutex<>>`

### Execution Flow Diagram

```
Input: [(= rule1), !(eval1), !(eval2), (= rule2), !(eval3), !(eval4)]
         │           │         │          │          │         │
         ↓           ↓         ↓          ↓          ↓         ↓
      ┌─────┐    ┌──────────────┐     ┌─────┐   ┌──────────────┐
      │ Seq │    │   Batch 1    │     │ Seq │   │   Batch 2    │
      │     │    │  (Parallel)  │     │     │   │  (Parallel)  │
      │ Add │    │              │     │ Add │   │              │
      │rule1│    │ ┌────┐ ┌────┐│     │rule2│   │ ┌────┐ ┌────┐│
      │ to  │    │ │eval│ │eval││     │ to  │   │ │eval│ │eval││
      │ env │    │ │ 1  │ │ 2  ││     │ env │   │ │ 3  │ │ 4  ││
      │     │    │ └────┘ └────┘│     │     │   │ └────┘ └────┘│
      └─────┘    └──────────────┘     └─────┘   └──────────────┘
         │           │         │          │          │         │
         ↓           ↓         ↓          ↓          ↓         ↓
       env'      [results1, results2]   env''   [results3, results4]
                        │                              │
                        └──────────────┬───────────────┘
                                       ↓
                         Sort by original index
                                       ↓
                    [results1, results2, results3, results4]
```

---

## Threading Model

### Dual Thread Pool Architecture

MeTTa uses **Tokio's async runtime** with two thread pools:

```
┌─────────────────────────────────────────────────────────────┐
│               Rholang Process Layer                          │
│        (Async Executor: ~num_cpus threads)                   │
│                                                              │
│  • Rholang contract execution                                │
│  • Process calculus coordination                             │
│  • Async I/O operations                                      │
│  • Task scheduling                                           │
│                                                              │
│  robotAPI!("query1", *r1) | robotAPI!("query2", *r2)        │
│         │                              │                     │
└─────────┼──────────────────────────────┼─────────────────────┘
          │                              │
          ↓                              ↓
┌─────────────────────────────────────────────────────────────┐
│            Tokio Runtime (Shared)                            │
│                                                              │
│  • Work-stealing scheduler                                   │
│  • Async task management                                     │
│  • Integration point                                         │
│                                                              │
│  spawn_blocking()  spawn_blocking()  spawn_blocking()       │
│         │                  │                  │             │
└─────────┼──────────────────┼──────────────────┼─────────────┘
          │                  │                  │
          ↓                  ↓                  ↓
┌─────────────────────────────────────────────────────────────┐
│      Blocking Thread Pool (Configurable)                    │
│       (Default: 512 threads, dynamically scaled)            │
│                                                              │
│  • CPU-intensive MeTTa evaluation                            │
│  • Pattern matching (MORK queries)                           │
│  • Rule application                                          │
│  • Expression reduction                                      │
│                                                              │
│  [Thread 1]     [Thread 2]     ...     [Thread N]          │
│      │              │                       │               │
│      ↓              ↓                       ↓               │
│   eval()         eval()                  eval()            │
│      │              │                       │               │
│      └──────────────┴───────────────────────┘               │
│                     │                                       │
│                     ↓                                       │
│           Arc<Mutex<Space>>                                 │
│         (Shared MORK trie)                                  │
└─────────────────────────────────────────────────────────────┘
```

### Thread Pool 1: Async Executor

**Purpose:** Handle Rholang process execution and async coordination

**Characteristics:**
- **Size**: `num_cpus` threads (fixed by Tokio, not configurable in MeTTaTron)
- **Usage**: Rholang contract execution, process calculus operations, async I/O
- **Scheduler**: Work-stealing (Tokio default)
- **Managed by**: Rholang's Tokio runtime instance

**Why Fixed Size?**
- Optimal for I/O-bound and coordination tasks
- More threads would add overhead without benefit
- Work-stealing ensures good load balancing

### Thread Pool 2: Blocking Pool

**Purpose:** Handle CPU-intensive MeTTa evaluation

**Characteristics:**
- **Size**: Configurable (default 512, see `src/config.rs`)
- **Usage**: Pattern matching, rule application, expression evaluation
- **Scheduler**: Tokio's blocking pool (LIFO-ish, optimized for CPU work)
- **Managed by**: Same Tokio runtime (shared with Rholang)

**Why Separate Pool?**
- CPU-intensive work would starve async executor
- Allows independent tuning for CPU vs I/O workloads
- Prevents deadlocks (async tasks waiting on blocking work)

**Why Single Runtime?**
- No thread pool contention (unified scheduler)
- Efficient work distribution
- Simple coordination between Rholang and MeTTa

### Why `spawn_blocking` Instead of `spawn`?

**MeTTa evaluation is CPU-bound, not I/O-bound:**

```rust
// WRONG: Would starve async executor
tokio::spawn(async move {
    eval(expr, env)  // CPU-intensive, blocks thread
});

// CORRECT: Uses dedicated blocking pool
tokio::task::spawn_blocking(move || {
    eval(expr, env)  // CPU-intensive, doesn't block async executor
});
```

**Rationale:**
- `spawn`: For async I/O tasks (non-blocking)
- `spawn_blocking`: For CPU-intensive tasks (blocking OK)
- MeTTa evaluation involves: MORK trie traversal, pattern matching, rule application → all CPU-bound

---

## Batch Evaluation

### Batching Rules

**Rule 1: Consecutive eval expressions batch together**
```metta
!(eval1)  ─┐
!(eval2)   ├─→ Batch (parallel)
!(eval3)  ─┘
```

**Rule 2: Rule definitions force batch boundaries**
```metta
!(eval1)     ─→ Batch 1 (parallel)
(= (rule) x) ─→ Sequential (modifies environment)
!(eval2)     ─→ Batch 2 (parallel)
```

**Rule 3: Output order is preserved**
- Results are sorted by original expression index
- Guarantees deterministic output despite parallel execution

### Why Rules Force Boundaries

**Problem:**
Rules modify the shared environment (add facts, define patterns). If rules and evals execute in parallel, race conditions occur:

```metta
% BAD: If these run in parallel
(= (fact) data)      % Thread 1: Adds rule to environment
!(query-using-fact)  % Thread 2: May not see the rule yet!
```

**Solution:**
Force batch evaluation before processing rule definitions:

```rust
if is_rule_definition(expr) {
    // Force batch evaluation (wait for all parallel tasks to complete)
    evaluate_batch_parallel(current_batch).await;
    current_batch.clear();

    // Now safe to modify environment sequentially
    eval_rule(expr, &mut env);
}
```

**Result:**
- Environment modifications are serialized
- Later evaluations always see earlier rule definitions
- No race conditions or inconsistent state

### Example: Batch Evaluation in Practice

**Input:**
```metta
(= (distance room_a room_b) 10)  % Rule 1
(= (distance room_b room_c) 5)   % Rule 2
!(distance room_a room_b)         % Eval 1
!(distance room_b room_c)         % Eval 2
!(+ (distance room_a room_b) (distance room_b room_c))  % Eval 3
```

**Execution:**
```
Step 1: Process Rule 1 (sequential)
  → env' = env + {distance(room_a, room_b) = 10}

Step 2: Process Rule 2 (sequential)
  → env'' = env' + {distance(room_b, room_c) = 5}

Step 3: Batch Eval 1, Eval 2, Eval 3 (parallel)
  ├─ Thread 1: Eval 1 → [10]
  ├─ Thread 2: Eval 2 → [5]
  └─ Thread 3: Eval 3 → [15]

Step 4: Sort results by index
  → [10, 5, 15]
```

**Performance:**
- Sequential rule processing: 2 × T_rule
- Parallel eval processing: max(T_eval1, T_eval2, T_eval3) ≈ T_eval
- Total: 2 × T_rule + T_eval (vs. 2 × T_rule + 3 × T_eval sequential)
- **Speedup: 3× on eval portion**

---

## Thread Safety and Synchronization

### Shared State: Environment

**Structure** (`src/backend/environment.rs`):
```rust
#[derive(Clone)]
pub struct Environment {
    pub space: Arc<Mutex<Space>>,                        // MORK trie
    multiplicities: Arc<Mutex<HashMap<String, usize>>>,  // Rule counts
}
```

**Thread Safety Mechanisms:**
- **`Arc<T>`**: Atomic reference counting for shared ownership across threads
- **`Mutex<T>`**: Exclusive access for safe mutation
- **`Clone`**: Cheap Arc clone (pointer copy, not data copy)

**Why `Arc<Mutex<>>` Instead of Lock-Free?**
- Simpler implementation (no need for complex lock-free algorithms)
- Low contention in practice (reads dominate, writes are rare)
- Rust's type system prevents data races
- Performance is sufficient for current workloads

### Lock Ordering

**Critical Rule:** Always lock `multiplicities` before `space` to prevent deadlocks.

**Example:**
```rust
impl Environment {
    pub fn add_rule(&mut self, rule: Rule) {
        // Lock 1: Update multiplicities
        {
            let mut counts = self.multiplicities.lock().unwrap();
            *counts.entry(rule.key()).or_insert(0) += 1;
        }  // Lock automatically dropped (RAII)

        // Lock 2: Update MORK space
        {
            let mut space = self.space.lock().unwrap();
            space.insert(rule_bytes);
        }  // Lock automatically dropped
    }
}
```

**Why This Ordering?**
- Consistent across all operations
- Prevents circular wait (Thread A locks space, waits for multiplicities; Thread B locks multiplicities, waits for space → deadlock)
- Rust's type system + RAII ensures locks are always released

### Deadlock Prevention

**Rust's Type System Prevents Common Mistakes:**

1. **Lock poisoning**: If a thread panics while holding a lock, the Mutex is poisoned and subsequent lock attempts return `Err`
2. **RAII**: Locks are automatically released when MutexGuard goes out of scope (no forgotten unlocks)
3. **No shared mutable state**: Without `Arc<Mutex<>>`, data can't be shared across threads
4. **Borrow checker**: Prevents data races at compile time

**Testing:**
- Unit tests with parallel evaluation (see `tests/concurrency_tests.rs`)
- Stress tests with 1000+ parallel queries
- No deadlocks observed in production or testing

### MORK Space Thread Safety

**MORK Trie is NOT Inherently Thread-Safe:**
- Internal use of raw pointers and unsafe code
- No built-in synchronization primitives

**MeTTaTron's Solution:**
- Wrap MORK Space in `Arc<Mutex<Space>>`
- All MORK operations (insert, query, iter) acquire lock first
- Encapsulates unsafety within safe Rust API

**Safety Guarantee:**
- Only one thread can access MORK Space at a time
- No data races possible
- Memory safety maintained via Rust's ownership system

---

## Performance Characteristics

### Parallel Speedup

**Ideal Speedup (Amdahl's Law):**
```
Speedup = 1 / (S + (P / N))

where:
  S = fraction of sequential work (rule definitions)
  P = fraction of parallel work (eval expressions)
  N = number of cores/threads
```

**Example:**
- 10% rule definitions (S = 0.1)
- 90% eval expressions (P = 0.9)
- 8 cores (N = 8)

```
Speedup = 1 / (0.1 + (0.9 / 8))
        = 1 / (0.1 + 0.1125)
        = 1 / 0.2125
        ≈ 4.7×
```

**Real-World Observations:**
- Single query: T seconds
- N parallel queries (N ≤ max_blocking_threads):
  - Without batching: N × T seconds (sequential)
  - With batching: ≈T seconds (parallel)
  - **Actual speedup: ~N× (linear up to thread pool size)**

### Scalability

**Horizontal Scaling:**
- Add more blocking threads → more parallelism
- Effective up to number of CPU cores (then diminishing returns)
- Beyond cores: Overhead from context switching outweighs benefits

**Vertical Scaling:**
- MORK trie O(m) pattern matching scales well with dataset size
- Shared prefix compression reduces memory usage
- Bottleneck: Mutex contention on shared Environment (low in practice)

**Bottleneck Analysis:**
1. **MORK query**: O(m) where m = matches (very fast in practice)
2. **Lock contention**: Low (reads dominate, writes are rare)
3. **Cartesian product**: Can explode combinatorially (inherent to semantics)
4. **Thread spawning**: Negligible (Tokio's thread pool reuse)

### Memory Usage

**Per-Thread:**
- Stack: ~2-8 MB (Tokio default)
- Heap: Minimal (Arc clones don't duplicate data)

**Shared:**
- MORK Space: O(total bytes of all facts/rules)
  - Trie structure with prefix sharing
  - Symbol table for interned symbols
- Multiplicities: O(unique rules)

**Arc Cloning:**
- Cost: O(1) (atomic increment of reference count)
- Memory: No duplication (all threads share same MORK Space)

**Example:**
- 10,000 facts: ~200 KB MORK Space
- 512 threads: ~512 Arc pointers = ~4 KB
- **Total: ~200 KB (vs. ~100 MB if each thread had its own copy)**

### Comparison: Sequential vs Parallel

**Scenario:** Evaluate 100 independent queries

| Metric | Sequential | Parallel (8 cores) | Speedup |
|--------|-----------|-------------------|---------|
| **Time** | 100T | ~12.5T | ~8× |
| **Memory** | M | ~M (Arc sharing) | ~1× |
| **CPU Usage** | 100% of 1 core | ~100% of 8 cores | 8× |
| **Throughput** | 1 query/T | 8 queries/T | 8× |

**Assumptions:**
- Each query takes T time
- No lock contention (reads dominate)
- Batch size ≥ 8 (enough to saturate cores)

---

## Integration with Rholang

### Shared Tokio Runtime

**Key Design Decision:** Both Rholang and MeTTa use the **same Tokio runtime instance**.

**Benefits:**
1. **No thread pool contention**: Unified work-stealing scheduler
2. **Efficient coordination**: No cross-runtime synchronization needed
3. **Resource management**: Single pool of threads, optimal utilization
4. **Simplicity**: MeTTa is a library, Rholang manages runtime

**Alternative (rejected):** Separate runtimes for Rholang and MeTTa
- Would require cross-runtime communication (slower)
- Thread pool contention (multiple schedulers competing)
- More complex configuration and tuning

### State Passing via PathMap Par

**Rholang Integration Pattern:**
```rholang
// MeTTa state as first-class Rholang value (PathMap Par)
new state in {
  // Initialize: Compile knowledge base
  for (@compiled_state <- mettaCompile!?(metta_source_code)) {
    state!(compiled_state) |

    // Query 1: Use compiled state
    for (@s1 <- state) {
      new queryResult in {
        queryResult!({||}.run(s1).run(compiled_query1)) |
        for (@s2 <- queryResult) {
          state!(s2) |  // Updated state with query results

          // Query 2: Use updated state
          for (@s3 <- state) {
            queryResult!({||}.run(s3).run(compiled_query2)) |
            for (@s4 <- queryResult) {
              state!(s4)  // Final state
            }
          }
        }
      }
    }
  }
}
```

**State Structure:**
```rust
pub struct MettaState {
    pub source: Vec<MettaValue>,           // Expressions to evaluate
    pub environment: Environment,          // MORK Space + multiplicities
    pub output: Vec<Vec<MettaValue>>,      // Results from evaluations
}
```

**Serialization to Rholang (`src/pathmap_par_integration.rs`):**
```
PathMap Par {
  ("source", [expr1, expr2, ...])          // As S-expressions
  ("environment", (
    ("space", GByteArray),                 // Raw MORK trie bytes
    ("multiplicities", GByteArray)         // Bincode-encoded HashMap
  ))
  ("output", [[result1], [result2], ...])  // Nested lists of results
}
```

### No Message Passing

**Direct Function Calls:**
```rust
// Rholang → MeTTa: Direct call to run_state()
pub fn run_state(
    accumulated: MettaState,
    compiled: MettaState,
) -> Result<MettaState, String>

// Sync wrapper around async function
pub fn run_state_sync(...) -> Result<...> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            run_state_async(...).await
        })
    })
}
```

**Why Not Message Passing?**
- Direct calls are faster (no serialization overhead)
- Shared memory via `Arc<Mutex<>>` is efficient
- State serialization only at Rholang boundary (not for every internal operation)

### Parallel Queries in Rholang

**Example: Multi-Robot Coordination**
```rholang
contract robotCoordinator(@"plan_all", ret) = {
  new r1, r2, r3 in {
    // Launch all queries in parallel
    robotAPI!("find_path", "room_a", "room_d", *r1) |
    robotAPI!("find_path", "room_b", "room_e", *r2) |
    robotAPI!("find_path", "room_c", "room_a", *r3) |

    // Wait for all results
    for (@path1 <- r1; @path2 <- r2; @path3 <- r3) {
      ret!({|
        ("robot1", path1),
        ("robot2", path2),
        ("robot3", path3)
      |})
    }
  }
}
```

**Execution:**
1. Rholang spawns 3 parallel processes (one per `robotAPI` call)
2. Each process calls MeTTa via `mettaCompile` and `.run()`
3. MeTTa evaluations run on blocking thread pool
4. Results collected and returned to Rholang
5. Rholang joins results via `for` comprehension

**Speedup:** 3× (assuming independent paths)

---

## Configuration and Tuning

### Thread Pool Configuration

**API** (`src/config.rs`):
```rust
pub struct EvalConfig {
    pub max_blocking_threads: usize,    // Max parallel evaluations
    pub batch_size_hint: usize,          // Expected batch size (for optimization)
}

pub fn configure_eval(config: EvalConfig);
pub fn get_eval_config() -> EvalConfig;
```

**Preset Configurations:**

| Preset | `max_blocking_threads` | Best For |
|--------|------------------------|----------|
| **Default** | 512 | General use, high parallelism |
| **CPU-optimized** | `num_cpus * 2` | CPU-bound workloads, minimize overhead |
| **Memory-optimized** | `num_cpus` | Memory-constrained systems, reduce stack usage |
| **Throughput-optimized** | 1024 | High-throughput batch processing, many concurrent queries |

**Example:**
```rust
use mettatron::config::{EvalConfig, configure_eval};

// Configure once at application startup
configure_eval(EvalConfig::cpu_optimized());

// Or custom configuration
configure_eval(EvalConfig {
    max_blocking_threads: 256,
    batch_size_hint: 64,
});
```

### Tuning Guidelines

**1. CPU-Bound Workloads:**
- Use `cpu_optimized()`: `num_cpus * 2` threads
- Balances parallelism and context switching overhead
- Good default for most symbolic reasoning tasks

**2. Memory-Constrained Systems:**
- Use `memory_optimized()`: `num_cpus` threads
- Reduces stack memory usage (2-8 MB per thread)
- Prevents OOM on systems with limited RAM

**3. High-Throughput Batch Processing:**
- Use `throughput_optimized()`: 1024 threads
- Maximizes parallelism for large batches
- Suitable for systems with many cores (16+)

**4. Custom Tuning:**
- Benchmark with different thread counts
- Profile to find optimal balance
- Consider: CPU cores, memory, batch size, query complexity

### Monitoring

**Logging:**
```rust
env::set_var("RUST_LOG", "mettatron=debug");
env_logger::init();

// Logs include:
// - Thread pool spawning
// - Batch evaluation start/end
// - Lock acquisitions (if enabled)
// - Evaluation timings
```

**Metrics** (Future):
- Thread pool utilization
- Lock contention statistics
- Batch sizes and execution times
- Speedup vs sequential baseline

---

## Debugging and Profiling

### Logging

**Enable Debug Logging:**
```rust
env::set_var("RUST_LOG", "mettatron=debug");
env_logger::init();
```

**Log Output Includes:**
- Batch evaluation start/end
- Number of expressions in batch
- Thread pool spawning
- Evaluation results

### Thread Dumps

**Send SIGQUIT for Backtrace:**
```bash
# Find process ID
ps aux | grep mettatron

# Send signal
kill -QUIT <pid>

# Backtrace printed to stderr (shows all thread stacks)
```

### CPU Profiling

**Using `perf` (Linux):**
```bash
# Record CPU profile
perf record -g ./target/release/mettatron input.metta

# View report
perf report

# Generate flame graph
perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg
```

**Using `cargo flamegraph`:**
```bash
cargo install flamegraph

# Generate flame graph
cargo flamegraph --bin mettatron -- input.metta

# Opens flamegraph.svg in browser
```

### Contention Analysis

**Identify Lock Contention:**
```bash
# Sample lock waits
perf record -e lock:contention_begin -g ./target/release/mettatron input.metta

# View contention hotspots
perf report
```

**Expected Results:**
- Low contention on `Environment` mutex (reads dominate)
- No contention on `Arc` (atomic operations are fast)

### Performance Testing

**Benchmark Parallel Speedup:**
```rust
// benches/parallel_speedup.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_sequential(c: &mut Criterion) {
    c.bench_function("sequential_100_queries", |b| {
        b.iter(|| {
            for i in 0..100 {
                eval_query(i);
            }
        });
    });
}

fn bench_parallel(c: &mut Criterion) {
    c.bench_function("parallel_100_queries", |b| {
        b.iter(|| {
            // Batch evaluation
            let tasks: Vec<_> = (0..100)
                .map(|i| tokio::spawn_blocking(|| eval_query(i)))
                .collect();
            tokio::runtime::Handle::current().block_on(async {
                for task in tasks {
                    task.await.unwrap();
                }
            });
        });
    });
}

criterion_group!(benches, bench_sequential, bench_parallel);
criterion_main!(benches);
```

**Run Benchmarks:**
```bash
cargo bench --bench parallel_speedup
```

---

## Future Enhancements

### 1. Parallel Nondeterminism

**Goal:** Evaluate multiple rule definitions in parallel (currently sequential).

**Current:**
```rust
for rule in matching_rules {
    let results = eval_with_rule(expr, rule, env);
    all_results.extend(results);
}
// Sequential: O(n × T) where n = number of rules
```

**Future:**
```rust
let rule_results: Vec<_> = matching_rules.par_iter()
    .flat_map(|rule| eval_with_rule(expr, rule, env))
    .collect();
// Parallel: O(T) with n cores
```

**Challenge:** Maintaining deterministic ordering of results.

**Solution:** Tag results with rule index, sort before returning.

**Benefit:** N× speedup when many rules match (N = number of matching rules).

### 2. Lock-Free MORK Space

**Goal:** Replace `Mutex<Space>` with lock-free trie operations.

**Current:**
```rust
pub struct Environment {
    pub space: Arc<Mutex<Space>>,  // Exclusive access
}
```

**Future:**
```rust
pub struct Environment {
    pub space: Arc<ConcurrentSpace>,  // Lock-free
}

impl ConcurrentSpace {
    pub fn insert(&self, path: &[u8]) { /* lock-free insert */ }
    pub fn query(&self, pattern: &[u8]) -> Vec<Match> { /* lock-free query */ }
}
```

**Benefit:**
- Higher throughput under contention
- No lock poisoning
- Better scalability with many cores

**Challenge:**
- Complex implementation (epoch-based reclamation, atomic operations)
- MORK's internal structure not designed for concurrency

### 3. GPU Acceleration

**Goal:** Offload pattern matching to GPU for massive parallelism.

**Approach:**
- Represent MORK trie as GPU-friendly data structure (e.g., compact array)
- Perform pattern matching on GPU (thousands of patterns in parallel)
- Transfer results back to CPU

**Example:**
```rust
pub fn gpu_query_multi(patterns: &[Vec<u8>]) -> Vec<Vec<Match>> {
    // Upload patterns to GPU
    let gpu_patterns = upload_to_gpu(patterns);

    // Launch GPU kernel (thousands of threads)
    let gpu_results = gpu_pattern_match_kernel(gpu_patterns, gpu_trie);

    // Download results
    download_from_gpu(gpu_results)
}
```

**Benefit:**
- 10-100× speedup for pattern-heavy workloads
- Enables real-time reasoning on large knowledge bases

**Challenge:**
- GPU memory limitations (trie may not fit)
- Overhead of CPU-GPU transfer
- Limited branching in GPU kernels (pattern matching is branchy)

### 4. Adaptive Thread Pool Sizing

**Goal:** Dynamically adjust thread pool size based on workload.

**Current:** Fixed thread pool size (configured at startup).

**Future:**
```rust
pub struct AdaptiveEvalConfig {
    pub min_threads: usize,
    pub max_threads: usize,
    pub target_utilization: f64,  // e.g., 0.8 = 80% CPU usage
}

impl AdaptiveThreadPool {
    fn adjust_size(&mut self, current_load: f64) {
        if current_load > target_utilization && threads < max {
            add_thread();
        } else if current_load < target_utilization * 0.5 && threads > min {
            remove_thread();
        }
    }
}
```

**Benefit:**
- Efficient resource usage (scale down when idle)
- Better performance under varying load

**Challenge:**
- Overhead of thread creation/destruction
- Choosing optimal thresholds

### 5. Work-Stealing for Nondeterminism

**Goal:** Distribute nondeterministic branches across threads dynamically.

**Approach:**
- When evaluating multiple rules, create tasks in work-stealing queue
- Idle threads steal tasks from busy threads
- Load balancing without explicit scheduling

**Benefit:**
- Better load balancing (no thread idle while others are busy)
- Scales well with varying rule complexity

**Challenge:**
- Requires refactoring evaluation to task-based model
- Overhead of task management

---

## Conclusion

MeTTa's evaluation execution model elegantly separates **semantic nondeterminism** from **parallel execution**:

- **Nondeterminism** is a language feature that expresses multiple valid solutions declaratively
- **Parallelism** is a runtime optimization that executes independent computations concurrently

This separation provides:
1. **Predictable semantics**: Deterministic ordering despite parallelism
2. **High performance**: Linear speedup with batch evaluation
3. **Thread safety**: Rust's type system prevents data races
4. **Scalability**: Efficient use of multi-core hardware

For building high-performance symbolic reasoning systems like robot planning, understanding both dimensions is crucial: leverage nondeterminism to express search spaces concisely, and leverage parallelism to explore them efficiently.

---

## References

### Implementation
- **Rholang Integration:** `src/rholang_integration.rs:239-350` (batch evaluation)
- **Configuration:** `src/config.rs` (thread pool tuning)
- **Evaluation:** `src/backend/eval/mod.rs` (nondeterministic semantics)
- **Environment:** `src/backend/environment.rs` (thread-safe shared state)

### Documentation
- **Threading Model:** `docs/THREADING_MODEL.md` (comprehensive guide to Tokio integration)
- **Configuration Guide:** `examples/threading_config.rs` (working examples)
- **Robot Planning:** `examples/robot_planning.rho` (real-world application)

### Tests
- **Concurrency Tests:** `tests/concurrency_tests.rs` (parallel evaluation tests)
- **Stress Tests:** `tests/stress_tests.rs` (1000+ parallel queries)
