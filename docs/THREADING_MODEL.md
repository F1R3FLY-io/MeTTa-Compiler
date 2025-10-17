# MeTTaTron Threading Model

This document explains how MeTTaTron coordinates with Rholang's async runtime for parallel expression evaluation.

## Overview

MeTTaTron uses **Tokio's async runtime** with a two-pool threading model to achieve parallelism while maintaining MeTTa's semantic guarantees:

1. **Async Executor Threads** - For I/O and async coordination
2. **Blocking Thread Pool** - For CPU-intensive MeTTa evaluation

Both pools are managed by the **same Tokio runtime instance** used by Rholang, ensuring optimal resource coordination.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Rholang Tokio Runtime                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌───────────────────────┐      ┌──────────────────────────┐   │
│  │  Async Executor Pool  │      │  Blocking Thread Pool    │   │
│  │  (I/O + Coordination) │      │  (CPU-Intensive Work)    │   │
│  ├───────────────────────┤      ├──────────────────────────┤   │
│  │ • Rholang operations  │      │ • MeTTa evaluation       │   │
│  │ • Async coordination  │      │ • Pattern matching       │   │
│  │ • Task scheduling     │      │ • Grounded functions     │   │
│  │ • I/O multiplexing    │      │ • Rule application       │   │
│  │                       │      │                          │   │
│  │ Default: num_cpus     │      │ Default: 512 (dynamic)   │   │
│  │ (Fixed by Tokio)      │      │ (Configurable)           │   │
│  └───────────────────────┘      └──────────────────────────┘   │
│                                                                   │
└─────────────────────────────────────────────────────────────────┘
```

## Execution Flow

When Rholang calls MeTTa for evaluation:

```
1. Rholang Method Call (.run() on PathMap)
   │
   │ [Sync context on Rholang executor thread]
   │
   ├─► tokio::task::block_in_place(|| {
   │       │
   │       │ [Tells Tokio: "I'm about to block, move me off executor"]
   │       │
   │       └─► tokio::runtime::Handle::current().block_on(async {
   │               │
   │               │ [Runs async code synchronously in blocking context]
   │               │
   │               └─► run_state_async(accumulated, compiled).await
   │                       │
   │                       │ [Batches consecutive eval expressions]
   │                       │
   │                       ├─► For each batch:
   │                       │   │
   │                       │   ├─► tokio::task::spawn_blocking(|| {
   │                       │   │       │
   │                       │   │       │ [Moves to blocking thread pool]
   │                       │   │       │
   │                       │   │       └─► eval(expr, env)
   │                       │   │               │
   │                       │   │               └─► CPU-intensive pattern matching
   │                       │   │   })
   │                       │   │
   │                       │   └─► await all batch results
   │                       │
   │                       └─► Return MettaState
   │           })
   │   })
   │
   └─► Return to Rholang
```

## Key Design Decisions

### 1. Why `spawn_blocking` Instead of `spawn`?

**`spawn_blocking`** is used because:

- ✅ **CPU-Intensive Work**: MeTTa evaluation is compute-bound, not I/O-bound
- ✅ **Prevents Executor Starvation**: Doesn't block async executor threads
- ✅ **Dedicated Thread Pool**: Separate pool scales independently
- ✅ **Tokio's Recommendation**: For synchronous, CPU-intensive operations

Using regular `spawn` would require making `eval()` async, which provides no benefit since the work is CPU-bound.

### 2. Why `block_in_place` + `block_on`?

This combination is required because:

- **Rholang's `.run()` method is synchronous** - Can't be made async without major refactoring
- **`block_in_place`** - Tells Tokio to move the current task off the executor
- **`block_on`** - Runs async code synchronously within the blocking context

Alternative considered: Making Rholang's method async would require:
- Changing Rholang's interpreter to async (major refactoring)
- Propagating async through all method calls
- More complex control flow

Current approach provides parallelism without requiring Rholang changes.

### 3. Single Runtime vs. Multiple Runtimes

**Single Runtime** (current approach):

- ✅ All threads coordinated by one Tokio instance
- ✅ Efficient work-stealing across pools
- ✅ Lower overhead
- ✅ Shared thread pool resources

**Multiple Runtimes** (not used):

- ❌ Higher overhead (multiple schedulers)
- ❌ No work-stealing between runtimes
- ❌ Potential resource contention
- ❌ More complex configuration

## Performance Characteristics

### Thread Pool Sizing

#### Async Executor Pool
- **Fixed by Tokio**: Typically `num_cpus` threads
- **Not configurable** in MeTTaTron (Rholang controls this)
- **Purpose**: Coordinate async operations, not CPU work

#### Blocking Thread Pool
- **Default**: 512 threads (Tokio default)
- **Configurable**: Via `EvalConfig::max_blocking_threads`
- **Dynamic Scaling**: Tokio spawns threads as needed up to max
- **Purpose**: Parallel MeTTa evaluation

### Recommended Settings

**CPU-Optimized** (default for most workloads):
```rust
EvalConfig::cpu_optimized()  // max_blocking_threads = num_cpus * 2
```

**Memory-Constrained**:
```rust
EvalConfig::memory_optimized()  // max_blocking_threads = num_cpus
```

**High-Throughput**:
```rust
EvalConfig::throughput_optimized()  // max_blocking_threads = 1024
```

### Scalability Analysis

#### Single Expression
```
Overhead: ~10-20μs (spawn_blocking + context switch)
Benefit: None (sequential anyway)
Recommendation: Use sync run_state() if only 1 expression
```

#### Multiple Independent Expressions (Ideal Case)
```
Expressions: N eval expressions
Parallelism: min(N, max_blocking_threads)
Speedup: Near-linear up to num_cpus
Example: 10 expressions @ 100ms each = ~100ms total (10x speedup)
```

#### Mixed Rules and Evals
```
Batching: Consecutive evals batched until rule definition
Synchronization: Rule definitions force batch completion
Overhead: Batch coordination ~50-100μs per batch
```

#### Nested Evaluations
```
Pattern: Rule calls another rule
Threads: Each blocking thread can spawn more blocking tasks
Depth: Limited by max_blocking_threads
```

## Resource Management

### Thread Stack Size
- **Default**: Platform-dependent (typically 2MB on Linux)
- **Configurable**: Via Tokio runtime builder
- **Considerations**: Deep recursion in MeTTa may require larger stacks

### Memory Overhead
```
Per Thread: ~2-4MB (stack + metadata)
Max Overhead: max_blocking_threads * 4MB
Example: 512 threads = ~2GB maximum
Note: Threads created lazily, not all at once
```

### Context Switching
```
Frequency: On each spawn_blocking call
Cost: ~1-10μs depending on system
Amortization: Batching reduces context switches
```

## Configuration Examples

### Application Initialization (Recommended)

```rust
use mettatron::config::{EvalConfig, configure_eval};

fn main() {
    // Configure before any async operations
    configure_eval(EvalConfig::cpu_optimized());

    // Start your Rholang runtime
    // ...
}
```

### Custom Tokio Runtime

```rust
use mettatron::config::{EvalConfig, apply_to_runtime_builder};
use tokio::runtime::Builder;

let config = EvalConfig {
    max_blocking_threads: 256,
    batch_size_hint: 64,
};

let runtime = apply_to_runtime_builder(
    Builder::new_multi_thread(),
    config
)
.worker_threads(8)  // Async executor threads
.enable_all()
.build()
.unwrap();

// Use runtime for Rholang
runtime.block_on(async {
    // Your Rholang code
});
```

### Environment-Based Configuration

```rust
use mettatron::config::{EvalConfig, configure_eval};
use std::env;

fn init_config() {
    let config = match env::var("METTA_PROFILE").as_deref() {
        Ok("cpu") => EvalConfig::cpu_optimized(),
        Ok("memory") => EvalConfig::memory_optimized(),
        Ok("throughput") => EvalConfig::throughput_optimized(),
        _ => EvalConfig::default(),
    };

    configure_eval(config);
}
```

## Debugging and Monitoring

### Enable Tokio Console (Development)

```rust
// In Cargo.toml
[dependencies]
console-subscriber = "0.2"

// In main.rs
fn main() {
    console_subscriber::init();
    // ... rest of initialization
}
```

Run with:
```bash
RUSTFLAGS="--cfg tokio_unstable" cargo run
```

### Thread Pool Metrics

```rust
// Log thread pool statistics
let handle = tokio::runtime::Handle::current();
println!("Active blocking threads: {}", handle.metrics().num_blocking_threads());
```

### Performance Profiling

```bash
# CPU profiling with perf
perf record -g ./your_rholang_app
perf report

# Thread activity
perf record -e sched:sched_switch -g ./your_rholang_app
```

## Common Patterns

### Pattern 1: Batch Evaluation

**Problem**: Many independent expressions to evaluate

```metta
!(+ 1 2)
!(* 3 4)
!(- 10 5)
!(/ 20 4)
```

**Solution**: All batched and parallelized automatically
```
Threads: 4 parallel evaluations
Time: ~max(expr_times) instead of sum(expr_times)
```

### Pattern 2: Rule Chaining

**Problem**: Rules that call other rules

```metta
(= (double $x) (* $x 2))
(= (quadruple $x) (double (double $x)))
!(quadruple 10)
```

**Solution**: Sequential rule definitions, parallel final eval
```
Step 1: Define double (sequential)
Step 2: Define quadruple (sequential)
Step 3: Evaluate quadruple (can spawn blocking tasks for nested calls)
```

### Pattern 3: Mixed Workload

**Problem**: Alternating rules and evals

```metta
(= (inc $x) (+ $x 1))
!(inc 5)
(= (dec $x) (- $x 1))
!(dec 10)
```

**Solution**: Batches with synchronization points
```
Batch 1: []
Sync: Define inc
Batch 2: [!(inc 5)]
Sync: Define dec
Batch 3: [!(dec 10)]
```

## Troubleshooting

### Issue: High Memory Usage

**Symptoms**: Memory grows with concurrent evaluations

**Diagnosis**:
```bash
# Check max blocking threads
RUST_LOG=debug ./your_app | grep "blocking_threads"
```

**Solution**:
```rust
// Reduce max blocking threads
configure_eval(EvalConfig::memory_optimized());
```

### Issue: Poor Parallelism

**Symptoms**: Multiple evals not speeding up

**Diagnosis**:
1. Check if expressions are separated by rule definitions
2. Verify max_blocking_threads > number of expressions
3. Profile to ensure work is CPU-bound

**Solution**:
```rust
// Increase max threads if needed
configure_eval(EvalConfig {
    max_blocking_threads: 1024,
    batch_size_hint: 128,
});
```

### Issue: Context Switch Overhead

**Symptoms**: Small expressions slower in parallel

**Diagnosis**: spawn_blocking overhead dominates for fast expressions

**Solution**:
1. Use sync `run_state()` for single/few expressions
2. Increase batch_size_hint to amortize overhead
3. Profile to measure actual expression time

## Future Improvements

### Potential Optimizations

1. **Adaptive Batching**: Dynamically adjust batch size based on expression complexity
2. **Work Stealing**: Allow blocking threads to steal work from batch queue
3. **Thread Affinity**: Pin threads to cores for better cache locality
4. **NUMA Awareness**: Consider NUMA topology on multi-socket systems

### Considered but Not Implemented

1. **Green Threads**: Would require async `eval()`, no clear benefit for CPU-bound work
2. **Thread Pool per PathMap**: Higher overhead, complexity without clear benefit
3. **Priority Queues**: MeTTa semantics require deterministic ordering

## References

- [Tokio Documentation - CPU-Bound Tasks](https://tokio.rs/tokio/topics/bridging)
- [Tokio Runtime Configuration](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html)
- [Async Rust Performance](https://tokio.rs/tokio/topics/performance)

## See Also

- `src/config.rs` - Configuration implementation
- `src/rholang_integration.rs` - Async evaluation implementation
- `examples/threading_demo.rs` - Threading model examples (TODO)
