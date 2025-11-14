//! MeTTaTron Configuration Module
//!
//! Provides configuration options for tuning the async runtime and parallel evaluation behavior.

use std::sync::OnceLock;

/// Global configuration for MeTTaTron's async evaluation
static EVAL_CONFIG: OnceLock<EvalConfig> = OnceLock::new();

/// Configuration for parallel evaluation in MeTTaTron
///
/// This controls how MeTTa expressions are evaluated in parallel when using `run_state_async`.
///
/// # Threading Model
///
/// MeTTaTron uses Tokio's async runtime with two thread pools:
///
/// 1. **Async Executor Threads** (managed by Tokio)
///    - Handles async coordination, I/O operations, and scheduling
///    - Default: Number of CPU cores
///    - Used by: Rholang operations, async control flow
///
/// 2. **Blocking Thread Pool** (managed by Tokio)
///    - Handles CPU-intensive MeTTa evaluation work
///    - Default: 512 threads (dynamically scaled)
///    - Used by: MeTTa expression evaluation via `spawn_blocking`
///    - Configurable via `max_blocking_threads`
///
/// # Resource Coordination
///
/// Both thread pools are managed by the **same Tokio runtime instance**:
///
/// ```text
/// Rholang (Tokio Runtime)
///   │
///   ├─► Async Executor Threads
///   │   └─► I/O operations, async coordination
///   │
///   └─► Blocking Thread Pool
///       └─► MeTTa evaluation (CPU-intensive)
/// ```
///
/// When Rholang calls MeTTa:
/// 1. Rholang runs on async executor threads
/// 2. Calls `run_state_async()` via `block_in_place()` + `block_on()`
/// 3. `run_state_async()` spawns parallel evaluations using `spawn_blocking()`
/// 4. Each evaluation runs on the blocking thread pool
/// 5. Results are coordinated back through the async runtime
///
/// # Why This Design?
///
/// - **Prevents Executor Starvation**: CPU-intensive MeTTa evaluation doesn't block I/O
/// - **Single Runtime**: All threads managed by Rholang's Tokio runtime
/// - **Work Stealing**: Tokio's scheduler balances load across cores
/// - **Scalability**: Blocking pool scales based on workload
///
/// # Example
///
/// ```rust
/// use mettatron::config::{EvalConfig, configure_eval};
///
/// // Configure before first use (typically in main())
/// configure_eval(EvalConfig {
///     max_blocking_threads: 256,
///     batch_size_hint: 16,
/// });
/// ```
#[derive(Debug, Clone, Copy)]
pub struct EvalConfig {
    /// Maximum number of threads in Tokio's blocking thread pool
    ///
    /// This controls how many MeTTa expressions can be evaluated in parallel.
    ///
    /// **Default**: 512 (Tokio's default)
    ///
    /// **Tuning Guidelines**:
    /// - For CPU-bound workloads: Set to `num_cpus * 2` to `num_cpus * 4`
    /// - For mixed workloads: Keep default (512) for dynamic scaling
    /// - For memory-constrained systems: Reduce to `num_cpus * 1` to `num_cpus * 2`
    /// - For high-throughput systems: Increase up to 1024 or higher
    ///
    /// **Note**: Tokio dynamically scales the pool, so this is a maximum, not a fixed size.
    pub max_blocking_threads: usize,

    /// Hint for batch size when parallelizing consecutive eval expressions
    ///
    /// When multiple `!(expr)` expressions are batched for parallel execution,
    /// this hint controls the maximum batch size before a synchronization point.
    ///
    /// **Default**: 32
    ///
    /// **Tuning Guidelines**:
    /// - Smaller values (8-16): Better latency, more synchronization overhead
    /// - Medium values (32-64): Balanced throughput and latency
    /// - Larger values (128+): Maximum throughput, higher latency
    ///
    /// **Note**: Rule definitions (`=`) always force batch boundaries to preserve semantics.
    pub batch_size_hint: usize,
}

impl Default for EvalConfig {
    fn default() -> Self {
        EvalConfig {
            max_blocking_threads: 512, // Tokio's default
            batch_size_hint: 32,
        }
    }
}

impl EvalConfig {
    /// Create a new configuration with recommended settings for CPU-bound workloads
    ///
    /// Sets `max_blocking_threads` to `num_cpus * 2` for optimal CPU utilization
    /// without excessive context switching.
    pub fn cpu_optimized() -> Self {
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        EvalConfig {
            max_blocking_threads: num_cpus * 2,
            batch_size_hint: 32,
        }
    }

    /// Create a new configuration with recommended settings for memory-constrained systems
    ///
    /// Limits thread pool to match CPU count to minimize memory overhead.
    pub fn memory_optimized() -> Self {
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        EvalConfig {
            max_blocking_threads: num_cpus,
            batch_size_hint: 16,
        }
    }

    /// Create a new configuration with recommended settings for high-throughput systems
    ///
    /// Maximizes parallelism for processing large batches of independent expressions.
    pub fn throughput_optimized() -> Self {
        EvalConfig {
            max_blocking_threads: 1024,
            batch_size_hint: 128,
        }
    }
}

/// Configure the global evaluation settings
///
/// This should be called **before** any async evaluation occurs, typically in your
/// application's initialization code.
///
/// # Panics
///
/// Panics if called more than once. Configuration is immutable after first set.
///
/// # Example
///
/// ```rust
/// use mettatron::config::{EvalConfig, configure_eval};
///
/// // Option 1: Use preset configuration
/// configure_eval(EvalConfig::cpu_optimized());
///
/// // Or Option 2: Custom configuration (choose one, not both!)
/// // configure_eval(EvalConfig {
/// //     max_blocking_threads: 256,
/// //     batch_size_hint: 64,
/// // });
///
/// // Now start your application
/// // ...
/// ```
pub fn configure_eval(config: EvalConfig) {
    EVAL_CONFIG.set(config).expect(
        "EvalConfig can only be set once. Call configure_eval() before any async evaluation.",
    );
}

/// Get the current evaluation configuration
///
/// Returns the configured settings, or the default if not explicitly configured.
pub fn get_eval_config() -> EvalConfig {
    EVAL_CONFIG.get().copied().unwrap_or_default()
}

/// Apply the configuration to the current Tokio runtime builder
///
/// This is a helper for applications that create their own Tokio runtime.
///
/// # Example
///
/// ```rust,no_run
/// use mettatron::config::{EvalConfig, apply_to_runtime_builder};
///
/// let config = EvalConfig::cpu_optimized();
/// let runtime = apply_to_runtime_builder(
///     tokio::runtime::Builder::new_multi_thread(),
///     config
/// )
/// .enable_all()
/// .build()
/// .unwrap();
/// ```
#[cfg(feature = "async")]
pub fn apply_to_runtime_builder(
    mut builder: tokio::runtime::Builder,
    config: EvalConfig,
) -> tokio::runtime::Builder {
    builder.max_blocking_threads(config.max_blocking_threads);
    builder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EvalConfig::default();
        assert_eq!(config.max_blocking_threads, 512);
        assert_eq!(config.batch_size_hint, 32);
    }

    #[test]
    fn test_cpu_optimized() {
        let config = EvalConfig::cpu_optimized();
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        assert_eq!(config.max_blocking_threads, num_cpus * 2);
        assert_eq!(config.batch_size_hint, 32);
    }

    #[test]
    fn test_memory_optimized() {
        let config = EvalConfig::memory_optimized();
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        assert_eq!(config.max_blocking_threads, num_cpus);
        assert_eq!(config.batch_size_hint, 16);
    }

    #[test]
    fn test_throughput_optimized() {
        let config = EvalConfig::throughput_optimized();
        assert_eq!(config.max_blocking_threads, 1024);
        assert_eq!(config.batch_size_hint, 128);
    }

    #[test]
    fn test_get_config_default() {
        // Don't call configure_eval in this test to test default behavior
        // Note: This might fail if another test calls configure_eval first
        // In practice, this is fine since config is global and set once
        let config = get_eval_config();
        assert!(config.max_blocking_threads > 0);
        assert!(config.batch_size_hint > 0);
    }
}
