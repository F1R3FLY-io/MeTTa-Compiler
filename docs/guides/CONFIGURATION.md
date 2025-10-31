# MeTTaTron Configuration Guide

Quick reference for configuring MeTTaTron's parallel evaluation.

## Quick Start

```rust
use mettatron::config::{EvalConfig, configure_eval};

fn main() {
    // Configure once at startup (before any async operations)
    configure_eval(EvalConfig::cpu_optimized());

    // Rest of your application
    // ...
}
```

## Preset Configurations

### Default Configuration
```rust
EvalConfig::default()
```
- `max_blocking_threads`: 512 (Tokio default)
- `batch_size_hint`: 32
- **Use when**: General-purpose workloads, unsure of requirements

### CPU-Optimized
```rust
EvalConfig::cpu_optimized()
```
- `max_blocking_threads`: `num_cpus × 2`
- `batch_size_hint`: 32
- **Use when**: CPU-bound MeTTa evaluation, most common use case
- **Example**: Complex pattern matching, recursive rules

### Memory-Optimized
```rust
EvalConfig::memory_optimized()
```
- `max_blocking_threads`: `num_cpus`
- `batch_size_hint`: 16
- **Use when**: Limited memory, embedded systems, containers with memory limits
- **Trade-off**: Lower parallelism for reduced memory footprint

### Throughput-Optimized
```rust
EvalConfig::throughput_optimized()
```
- `max_blocking_threads`: 1024
- `batch_size_hint`: 128
- **Use when**: High-throughput batch processing, many independent expressions
- **Trade-off**: Higher memory usage for maximum parallelism

## Custom Configuration

```rust
configure_eval(EvalConfig {
    max_blocking_threads: 256,  // Your custom value
    batch_size_hint: 64,        // Your custom value
});
```

### Parameters

#### `max_blocking_threads`
Controls the maximum number of MeTTa expressions that can be evaluated in parallel.

**Guidelines:**
- **Conservative**: `num_cpus × 1` to `num_cpus × 2`
- **Balanced**: `num_cpus × 2` to `num_cpus × 4` (recommended)
- **Aggressive**: `512` to `1024+`

**Considerations:**
- Higher values → More parallelism, more memory
- Lower values → Less memory, more sequential execution
- Tokio dynamically scales the pool (this is a maximum)

#### `batch_size_hint`
Controls how many consecutive `!(expr)` expressions are batched together for parallel execution.

**Guidelines:**
- **Low latency**: `8` to `16`
- **Balanced**: `32` to `64` (recommended)
- **High throughput**: `128` to `256`

**Considerations:**
- Higher values → Better throughput, higher latency
- Lower values → Lower latency, more synchronization overhead
- Rule definitions (`=`) always force batch boundaries

## Environment-Based Configuration

```rust
use std::env;
use mettatron::config::{EvalConfig, configure_eval};

fn init_config() {
    let config = match env::var("METTA_PROFILE").as_deref() {
        Ok("cpu") => EvalConfig::cpu_optimized(),
        Ok("memory") => EvalConfig::memory_optimized(),
        Ok("throughput") => EvalConfig::throughput_optimized(),
        Ok("default") | _ => EvalConfig::default(),
    };

    configure_eval(config);
}
```

**Usage:**
```bash
# CPU-optimized
METTA_PROFILE=cpu ./your_app

# Memory-optimized
METTA_PROFILE=memory ./your_app

# Throughput-optimized
METTA_PROFILE=throughput ./your_app
```

## Tokio Runtime Integration

If you're creating a custom Tokio runtime, you can apply the configuration:

```rust
use mettatron::config::{EvalConfig, apply_to_runtime_builder};
use tokio::runtime::Builder;

let config = EvalConfig::cpu_optimized();

let runtime = apply_to_runtime_builder(
    Builder::new_multi_thread(),
    config
)
.worker_threads(8)  // Async executor threads
.enable_all()
.build()
.unwrap();
```

## Common Scenarios

### Scenario 1: High-Performance Server

**Goal**: Maximum throughput for parallel requests

```rust
configure_eval(EvalConfig::throughput_optimized());
```

**Why**: Large thread pool handles many concurrent evaluations

### Scenario 2: Embedded Device

**Goal**: Minimal memory footprint

```rust
configure_eval(EvalConfig::memory_optimized());
```

**Why**: Limits thread pool to CPU count, reduces memory overhead

### Scenario 3: Development / Testing

**Goal**: Balanced performance, easy debugging

```rust
configure_eval(EvalConfig::cpu_optimized());
```

**Why**: Predictable parallelism based on available CPUs

### Scenario 4: Container with CPU Limits

**Goal**: Match thread pool to allocated CPUs

```rust
// Read CPU quota from cgroup
let cpu_quota = read_cpu_quota().unwrap_or(4);

configure_eval(EvalConfig {
    max_blocking_threads: cpu_quota * 2,
    batch_size_hint: 32,
});
```

## Performance Tuning

### Too Much Parallelism?

**Symptoms:**
- High memory usage
- Excessive context switching
- Poor performance despite parallelism

**Solution:**
```rust
// Reduce thread pool size
configure_eval(EvalConfig {
    max_blocking_threads: num_cpus,
    batch_size_hint: 16,
});
```

### Not Enough Parallelism?

**Symptoms:**
- CPU cores underutilized
- Sequential execution despite independent expressions
- Low throughput

**Solution:**
```rust
// Increase thread pool size
configure_eval(EvalConfig {
    max_blocking_threads: num_cpus * 4,
    batch_size_hint: 64,
});
```

### High Synchronization Overhead?

**Symptoms:**
- Many small expressions
- Frequent batch boundaries
- Poor scaling

**Solution:**
```rust
// Increase batch size to amortize overhead
configure_eval(EvalConfig {
    max_blocking_threads: 512,
    batch_size_hint: 128,  // Larger batches
});
```

## Monitoring

### Check Current Configuration

```rust
use mettatron::config::get_eval_config;

let config = get_eval_config();
println!("Max blocking threads: {}", config.max_blocking_threads);
println!("Batch size hint: {}", config.batch_size_hint);
```

### Tokio Metrics (Development)

```rust
// In Cargo.toml
[dependencies]
console-subscriber = "0.2"

// In main.rs
fn main() {
    console_subscriber::init();
    // ... rest of code
}
```

**Usage:**
```bash
RUSTFLAGS="--cfg tokio_unstable" cargo run
# Open tokio-console in another terminal
tokio-console
```

## Best Practices

1. **Configure Early**: Call `configure_eval()` before any async operations
2. **Configure Once**: Configuration is global and immutable after first set
3. **Start Conservative**: Use `EvalConfig::cpu_optimized()` as default
4. **Profile First**: Measure before optimizing
5. **Monitor Resources**: Watch memory and CPU usage
6. **Test at Scale**: Performance characteristics change with workload size

## See Also

- [Threading Model Documentation](THREADING_MODEL.md) - Detailed architecture and design
- [Examples](../examples/threading_config.rs) - Working code examples
- [Tokio Documentation](https://tokio.rs/tokio/topics/bridging) - Bridging sync and async

## FAQ

**Q: Can I change configuration after initialization?**
A: No, configuration is immutable after first set to ensure consistency.

**Q: What happens if I don't configure?**
A: Default configuration is used (512 max threads, batch size 32).

**Q: Does configuration affect the sync `run_state()`?**
A: No, only affects async `run_state_async()`. Sync version is always sequential.

**Q: How many CPUs does my system have?**
```rust
let num_cpus = std::thread::available_parallelism()
    .map(|n| n.get())
    .unwrap_or(4);
println!("Available CPUs: {}", num_cpus);
```

**Q: Can I have different configurations for different PathMaps?**
A: No, configuration is global. Use separate processes if needed.

**Q: Does this affect Rholang's async executor?**
A: No, only configures the blocking thread pool. Rholang's executor is managed separately.
