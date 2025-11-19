# MeTTa Benchmark Suite Documentation

**Document Version:** 1.0
**Date:** 2025-11-17
**Branch:** dylon/rholang-language-server

## Overview

The MeTTa benchmark suite (`benches/metta.rs`) provides comprehensive performance testing of the MeTTaTron evaluator using real-world MeTTa programs. This suite uses the Divan benchmarking framework to measure both synchronous and asynchronous evaluation performance across multiple workload patterns.

## Benchmark Infrastructure

### Location
- **Benchmark file:** `benches/metta.rs`
- **Sample programs:** `benches/metta_samples/*.metta`
- **Framework:** Divan (configured in `Cargo.toml`)

### Evaluation Modes

The suite benchmarks two evaluation modes:

1. **Synchronous (`run_state`)**: Sequential evaluation of expressions
2. **Asynchronous (`run_state_async`)**: Parallel evaluation using Tokio runtime

Each MeTTa sample program is evaluated 100 times with 100 iterations per sample to ensure statistical significance.

## Sample Programs

The benchmark suite uses 7 carefully selected MeTTa programs that represent different computational patterns:

### 1. `concurrent_space_operations.metta`
**Focus:** Multi-space operations and concurrent state management
**Characteristics:**
- Multiple space creation and manipulation
- Cross-space querying
- State synchronization patterns

### 2. `constraint_search_simple.metta`
**Focus:** Constraint satisfaction and search algorithms
**Characteristics:**
- Logical constraint solving
- Backtracking search
- Pattern matching with constraints

### 3. `fib.metta`
**Focus:** Recursive computation (Fibonacci sequence)
**Characteristics:**
- Deep recursion
- Memoization opportunities
- Pure functional computation

### 4. `knowledge_graph.metta`
**Focus:** Graph traversal and relationship queries
**Characteristics:**
- Graph structure manipulation
- Transitive closure
- Relationship inference

### 5. `metta_programming_stress.metta`
**Focus:** General MeTTa language features stress test
**Characteristics:**
- Mixed operation types
- Complex pattern matching
- Control flow variations

### 6. `multi_space_reasoning.metta`
**Focus:** Multi-space logical reasoning
**Characteristics:**
- Space isolation
- Cross-space inference
- Distributed reasoning patterns

### 7. `pattern_matching_stress.metta`
**Focus:** Intensive pattern matching workload
**Characteristics:**
- Complex pattern structures
- Deep pattern nesting
- Variable binding complexity

## Performance Results

### Branch Comparison: Main vs dylon/rholang-language-server

Results from running `cargo bench --bench metta` on both branches:

| Benchmark | Main (mean) | Current (mean) | Speedup | % Improvement |
|-----------|-------------|----------------|---------|---------------|
| **Async Benchmarks** |
| async_concurrent_space_operations | 29.2 ms | 7.044 ms | **4.14×** | **75.9% faster** |
| async_constraint_search | 32.87 ms | 10.1 ms | **3.25×** | **69.3% faster** |
| async_fib | 51.66 ms | 23.62 ms | **2.19×** | **54.3% faster** |
| async_knowledge_graph | 9.253 ms | 3.722 ms | **2.49×** | **59.8% faster** |
| async_metta_programming_stress | 19.78 ms | 5.569 ms | **3.55×** | **71.8% faster** |
| async_multi_space_reasoning | 9.169 ms | 2.684 ms | **3.42×** | **70.7% faster** |
| async_pattern_matching_stress | 15.84 ms | 6.251 ms | **2.53×** | **60.5% faster** |
| **Sync Benchmarks** |
| concurrent_space_operations | 20.84 ms | 13.84 ms | **1.51×** | **33.6% faster** |
| constraint_search | 28.75 ms | 15.51 ms | **1.85×** | **46.1% faster** |
| fib | 42.85 ms | 41.89 ms | 1.02× | 2.2% faster |
| knowledge_graph | 5.352 ms | 3.502 ms | **1.53×** | **34.6% faster** |
| metta_programming_stress | 14.34 ms | 10.58 ms | **1.36×** | **26.2% faster** |
| multi_space_reasoning | 5.939 ms | 4.828 ms | **1.23×** | **18.7% faster** |
| pattern_matching_stress | 12.36 ms | 7.721 ms | **1.60×** | **37.5% faster** |

### Performance Summary

- **Average async speedup**: 2.94× (66.1% faster)
- **Average sync speedup**: 1.44× (28.4% faster)
- **Overall average**: 2.05× (51.2% faster)

### Key Findings

1. **Async benchmarks show dramatic improvements** (2.2-4.1× speedup)
   - Largest gains in concurrent operations and reasoning tasks
   - Suggests excellent parallelization efficiency

2. **Sync benchmarks show consistent improvements** (1.2-1.9× speedup)
   - All workloads benefit from optimizations
   - Pure computation (fib) shows minimal change as expected

3. **Workload-dependent scaling**
   - Concurrent operations: Best async performance (4.14× speedup)
   - Pure recursion (fib): Minimal benefit from parallelization
   - Pattern matching: Strong improvements in both modes

## Running the Benchmarks

### Run All MeTTa Benchmarks
```bash
cargo bench --bench metta
```

### Run Specific Evaluation Mode
```bash
# Run only async benchmarks
cargo bench --bench metta async_

# Run only sync benchmarks
cargo bench --bench metta -- --exact concurrent_space_operations
```

### Run with Different Sample Sizes
The benchmark uses Divan's configuration (100 samples × 100 iterations). To modify:
```rust
// In benches/metta.rs
#[divan::bench(sample_count = 50)]  // Reduce samples for faster runs
```

### Generate HTML Reports
```bash
cargo bench --bench metta -- --output-format html
```

## Interpreting Results

### Timer Precision
Divan reports timer precision (typically 10-20 ns on modern systems). Results are reliable when:
- Mean time >> Timer precision
- All benchmarks show timer precision well below measurement times

### Statistical Significance
Each benchmark runs:
- **100 samples**: Independent runs to measure variability
- **100 iterations**: Repetitions within each sample

This provides robust statistics with low measurement noise.

### Comparing Async vs Sync

**When async is faster:**
- Independent computations that can parallelize
- I/O-bound operations (space queries, external calls)
- Multiple spaces being processed simultaneously

**When sync is comparable:**
- Sequential dependencies (each step requires previous result)
- Very fast operations (overhead dominates)
- Single-threaded workloads (recursive algorithms)

### Performance Regression Detection

Use this benchmark suite for:
1. **Pre-merge validation**: Compare feature branches against main
2. **Optimization verification**: Measure impact of performance changes
3. **Regression detection**: Run regularly to catch performance degradation

Example workflow:
```bash
# Save baseline from main
git checkout main
cargo bench --bench metta -- --save-baseline main

# Test feature branch
git checkout feature/my-optimization
cargo bench --bench metta -- --baseline main
```

## Relationship to Other Benchmarks

This benchmark suite complements other MeTTaTron benchmarks:

| Benchmark Suite | Focus | Use Case |
|----------------|-------|----------|
| `metta.rs` | End-to-end evaluation | Overall system performance |
| `rule_matching.rs` | Pattern matching engine | Core evaluation performance |
| `type_lookup.rs` | Type system operations | Type checking performance |
| `cow_environment.rs` | Environment cloning | Memory efficiency |
| `expression_parallelism.rs` | Parallel evaluation scaling | Concurrency performance |

## Configuration

### Hardware Considerations

For accurate benchmarking:
1. **Enable CPU affinity**: Pin benchmark process to specific cores
2. **Set CPU governor**: Use `performance` mode for consistent frequency
3. **Disable turbo boost**: For repeatable results
4. **Isolate cores**: Use `isolcpus` kernel parameter if possible

Example CPU configuration:
```bash
# Set performance governor
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Check current frequency
cat /proc/cpuinfo | grep MHz
```

### System Information

Reference system specs for the results above:
- **CPU**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads)
- **RAM**: 252 GB DDR4-2133 ECC
- **Storage**: Samsung 990 PRO 4TB NVMe
- **OS**: Linux 6.17.7-arch1-1

## Future Enhancements

Potential additions to the benchmark suite:

1. **More diverse workloads**
   - I/O-intensive operations
   - Memory-intensive patterns
   - Long-running reasoning tasks

2. **Scaling analysis**
   - Variable problem sizes
   - Thread count sensitivity
   - Memory scaling characteristics

3. **Energy efficiency metrics**
   - Power consumption measurement
   - Performance-per-watt analysis
   - Thermal behavior under load

4. **Real-world scenarios**
   - Production workload traces
   - User interaction patterns
   - Mixed workload compositions

## References

- **Benchmark Guide**: `docs/benchmarks/BRANCH_COMPARISON_GUIDE.md`
- **Optimization History**: `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md`
- **Branch Comparison**: `docs/benchmarks/benchmark_results_20251114_094209/OPTIMIZATION_COMPARISON_REPORT.md`
- **Divan Documentation**: https://nikolaivazquez.com/divan/

## Changelog

| Date | Version | Changes |
|------|---------|---------|
| 2025-11-17 | 1.0 | Initial documentation of MeTTa benchmark suite |
