//! MeTTaTron Configuration Module
//!
//! Provides configuration options for async integration and parallel evaluation behavior.

use std::sync::OnceLock;
use std::thread;

/// Global configuration for MeTTaTron's async evaluation
static EVAL_CONFIG: OnceLock<EvalConfig> = OnceLock::new();

/// Configuration for parallel evaluation in MeTTaTron
///
/// This controls how MeTTa expressions are evaluated in parallel when using `run_state_async`.
///
/// # Threading Model
///
/// MeTTaTron coordinates two execution resources:
///
/// 1. **Rholang Async Runtime**
///    - Handles async coordination and I/O.
///    - Owned by the embedding Rholang runtime.
///    - Tuned with `apply_to_runtime_builder()` when the caller builds Tokio.
///
/// 2. **Unified WorkPool**
///    - Handles CPU-intensive MeTTa evaluation work.
///    - Uses a priority queue and adaptive worker scaling.
///    - Retains eval tasks submitted during startup and drains them once workers
///      are started on first global-pool access.
///
/// # Resource Coordination
///
/// Rholang calls into `run_state_async()`, which batches independent evals and
/// submits them to the global eval WorkPool. The async caller waits for the
/// scatter-gather result while WorkPool workers run `eval_trampoline()`.
///
/// ```text
/// Rholang async runtime
///   │
///   └─► run_state_async()
///       └─► global eval WorkPool
///           ├─► priority queue
///           ├─► adaptive eval workers
///           └─► eval_trampoline()
/// ```
///
/// When Rholang calls MeTTa:
/// 1. Rholang remains responsible for async coordination.
/// 2. `run_state_async()` batches consecutive independent eval expressions.
/// 3. Each batch item is submitted to the WorkPool with eval priority.
/// 4. WorkPool workers execute CPU-bound evaluation and publish results.
/// 5. Results are gathered in original program order.
///
/// # Why This Design?
///
/// - **Prevents Executor Starvation**: CPU-intensive MeTTa evaluation does not
///   block async I/O threads.
/// - **Priority Scheduling**: Eval, compile, and maintenance work have explicit
///   priorities instead of relying on a generic blocking queue.
/// - **Startup Safety**: The WorkPool startup-drain proof covers tasks submitted
///   before worker threads are ready.
/// - **Scalability**: Worker count adapts to throughput, queue depth, and memory
///   pressure.
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
    /// Compatibility cap for Tokio's blocking thread pool when this crate
    /// constructs a runtime builder.
    ///
    /// MeTTa eval parallelism is controlled by the global WorkPool thread
    /// configuration. This field is retained for callers that still use
    /// `apply_to_runtime_builder()` to tune non-eval blocking callbacks owned by
    /// their Tokio runtime.
    ///
    /// **Default**: 512 (Tokio's default blocking cap)
    ///
    /// **Tuning Guidelines**:
    /// - For CPU-bound workloads: Set to `num_cpus * 2` to `num_cpus * 4`
    /// - For mixed workloads: Keep default (512) for dynamic scaling
    /// - For memory-constrained systems: Reduce to `num_cpus * 1` to `num_cpus * 2`
    /// - For high-throughput systems: Increase up to 1024 or higher
    ///
    /// **Note**: This is a Tokio runtime cap, not the WorkPool worker count.
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
            max_blocking_threads: 512, // Tokio's default blocking cap
            batch_size_hint: 32,
        }
    }
}

impl EvalConfig {
    /// Create a new configuration with recommended settings for CPU-bound workloads
    ///
    /// Sets the Tokio blocking cap to `num_cpus * 2` for callers that still use
    /// runtime-builder integration.
    pub fn cpu_optimized() -> Self {
        let num_cpus = thread::available_parallelism()
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
        let num_cpus = thread::available_parallelism()
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
        let num_cpus = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        assert_eq!(config.max_blocking_threads, num_cpus * 2);
        assert_eq!(config.batch_size_hint, 32);
    }

    #[test]
    fn test_memory_optimized() {
        let config = EvalConfig::memory_optimized();
        let num_cpus = thread::available_parallelism()
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
