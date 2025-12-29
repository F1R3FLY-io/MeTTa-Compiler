# Parallel PathMap vs Sequential HashMap: Crossover Analysis

**Date**: 2025-12-29
**Commit**: 4fabdf0 (feature/set-operation-functions)
**Branch**: feature/set-operation-functions
**System**: Intel Xeon (dual socket), 36 cores
**CPU Affinity**: Cores 0-17 (single NUMA node)

## Executive Summary

**CONCLUSION: There is NO crossover point where parallel PathMap outperforms sequential HashMap.**

Even at 1 million elements with optimal thread configuration (12-16 threads), HashMap remains 1.7x faster than parallel PathMap. The parallel implementation adds significant overhead at smaller sizes and provides minimal speedup at larger sizes.

## Primary Hypothesis Testing

| Hypothesis | Result | Evidence |
|------------|--------|----------|
| "Native parallel PathMap can outperform sequential HashMap at large scales (≥100K elements) if parallel efficiency exceeds 35%" | **REJECTED** | Best parallel efficiency: ~15% at 1M elements; HashMap still 1.7x faster |
| "Crossover point occurs between 100K and 1M elements with 8+ threads" | **REJECTED** | No crossover at any tested size (10K-1M) |

## Detailed Results

### Parallel Intersection by Size

#### 10,000 elements (50% overlap)
| Implementation | Time | Ratio vs HashMap |
|----------------|------|------------------|
| HashMap sequential | 937.67 µs | 1.0x (baseline) |
| PathMap sequential | 4.45 ms | 4.7x slower |
| PathMap parallel (1t) | 4.80 ms | 5.1x slower |
| PathMap parallel (2t) | 5.21 ms | 5.6x slower |
| PathMap parallel (4t) | 5.15 ms | 5.5x slower |
| PathMap parallel (8t) | 5.42 ms | 5.8x slower |
| PathMap parallel (16t) | 6.10 ms | 6.5x slower |

**Finding**: Parallelism adds overhead at 10K elements. All parallel configurations are slower than sequential PathMap.

#### 100,000 elements (50% overlap)
| Implementation | Time | Ratio vs HashMap |
|----------------|------|------------------|
| HashMap sequential | 13.58 ms | 1.0x (baseline) |
| PathMap sequential | 47.60 ms | 3.5x slower |
| PathMap parallel (1t) | 51.08 ms | 3.8x slower |
| PathMap parallel (2t) | 54.70 ms | 4.0x slower |
| PathMap parallel (4t) | 53.94 ms | 4.0x slower |
| PathMap parallel (8t) | 51.50 ms | 3.8x slower |
| PathMap parallel (16t) | 54.12 ms | 4.0x slower |

**Finding**: Parallelism provides no benefit at 100K elements. Best parallel is equal to sequential PathMap.

#### 500,000 elements (50% overlap)
| Implementation | Time | Ratio vs HashMap |
|----------------|------|------------------|
| HashMap sequential | 132.51 ms | 1.0x (baseline) |
| PathMap sequential | 285.95 ms | 2.2x slower |
| PathMap parallel (1t) | 322.70 ms | 2.4x slower |
| PathMap parallel (2t) | 309.02 ms | 2.3x slower |
| PathMap parallel (4t) | 316.34 ms | 2.4x slower |
| PathMap parallel (8t) | 306.98 ms | 2.3x slower |
| PathMap parallel (16t) | 312.53 ms | 2.4x slower |

**Finding**: Parallel PathMap is SLOWER than sequential PathMap at 500K elements!

#### 1,000,000 elements (50% overlap)
| Implementation | Time | Ratio vs HashMap |
|----------------|------|------------------|
| HashMap sequential | 324.38 ms | 1.0x (baseline) |
| PathMap sequential | 555.26 ms | 1.7x slower |
| PathMap parallel (1t) | 642.30 ms | 2.0x slower |
| PathMap parallel (2t) | 637.13 ms | 2.0x slower |
| PathMap parallel (4t) | 607.13 ms | 1.9x slower |
| PathMap parallel (8t) | 562.88 ms | 1.7x slower |
| PathMap parallel (16t) | 558.70 ms | 1.7x slower |

**Finding**: At 1M elements, parallel PathMap (16t) finally matches sequential PathMap, but HashMap is still 1.7x faster.

### Parallel Scaling at 1M Elements

| Threads | Time | Speedup vs 1 Thread | Efficiency |
|---------|------|---------------------|------------|
| 1 | 628.99 ms | 1.0x | 100% |
| 2 | 632.91 ms | 0.99x | 50% |
| 4 | 611.52 ms | 1.03x | 26% |
| 6 | 608.45 ms | 1.03x | 17% |
| 8 | 618.84 ms | 1.02x | 13% |
| 10 | 570.63 ms | 1.10x | 11% |
| 12 | 545.93 ms | 1.15x | 9.6% |
| 14 | 565.51 ms | 1.11x | 7.9% |
| 16 | 584.89 ms | 1.08x | 6.7% |
| 18 | 588.52 ms | 1.07x | 5.9% |

**Peak efficiency**: 12 threads with 9.6% efficiency (1.15x speedup)
**HashMap baseline**: 284.81 ms (still 1.92x faster than best parallel PathMap)

## Analysis: Why No Crossover?

### 1. Serialization Bottleneck
The `to_path_map_string()` conversion is the primary bottleneck. While we parallelize this, the overall savings are limited because:
- String allocation is already fast
- The actual work is proportional to data size, not structure

### 2. Sequential PathMap Construction
After parallel serialization, PathMap must be constructed sequentially because:
- PathMap's trie structure requires ordered insertion
- Each path insertion depends on previous tree state
- No lock-free or concurrent trie insertion available

### 3. Thread Coordination Overhead
The `thread::scope` + `mpsc::channel` pattern introduces:
- Thread spawn/join overhead
- Channel send/receive overhead
- Memory synchronization costs

### 4. Cache Effects
At larger sizes, memory bandwidth becomes limiting:
- PathMap's trie traversal has poor cache locality
- HashMap's linear scan has better cache utilization
- Parallel threads compete for memory bandwidth

## Ratio Trend Analysis

| Size | HashMap vs PathMap Ratio |
|------|-------------------------|
| 10K | HashMap 4.7x faster |
| 100K | HashMap 3.5x faster |
| 500K | HashMap 2.2x faster |
| 1M | HashMap 1.7x faster |

The ratio decreases with size, suggesting diminishing returns as data grows. However, the trend is asymptotic - extrapolating:

| Projected Size | Estimated Ratio |
|----------------|-----------------|
| 10M | ~1.3x (HashMap still faster) |
| 100M | ~1.1x (HashMap still faster) |
| 1B | Memory-bound; both impractical |

Even at theoretical 100M elements, HashMap would likely still be faster or equal.

## Nested Data Analysis

### Does PathMap benefit from deeply nested element types?

**Answer: NO.** HashMap still dominates even with complex nested structures.

#### Nested Depth Benchmark (1000 elements, 100% overlap)

| Nesting Depth | HashMap | PathMap | PathMap Parallel | PathMap Ratio |
|---------------|---------|---------|------------------|---------------|
| 1 | 246 µs | 1.48 ms | 2.35 ms | **6.0x slower** |
| 2 | 400 µs | 2.08 ms | 2.71 ms | **5.2x slower** |
| 3 | 555 µs | 2.80 ms | 3.10 ms | **5.0x slower** |
| 5 | 845 µs | 4.24 ms | 4.04 ms | **5.0x slower** |

#### Complex S-Expressions Benchmark (1000 elements, varying width)

| Width | HashMap | PathMap | PathMap Ratio |
|-------|---------|---------|---------------|
| 2 | 255 µs | 1.53 ms | **6.0x slower** |
| 5 | 500 µs | 2.09 ms | **4.2x slower** |
| 10 | 914 µs | 3.14 ms | **3.4x slower** |
| 20 | 1.64 ms | 5.35 ms | **3.3x slower** |

#### Why Nested Data Doesn't Help PathMap

1. **Ratio narrows but never reaches parity**: From 6x → 5x (depth) and 6x → 3.3x (width)
2. **Serialization overhead scales with complexity**: `to_path_map_string()` cost increases proportionally for both
3. **PathMap's advantages are elsewhere**: Structural sharing benefits prefix queries and incremental updates, not bulk set operations
4. **Parallel overhead still dominates**: At depth 5, parallel (4.04 ms) barely beats sequential (4.24 ms)

## Conclusion

**HashMap should be used for all set operations in MeTTa.**

The PathMap implementation provides:
- NO performance advantage at any tested size
- Higher memory overhead (trie structure)
- More complex code maintenance

The parallel PathMap implementation:
- Adds significant overhead at small-medium sizes
- Provides only ~15% speedup at 1M elements
- Never outperforms sequential HashMap

### Recommendation

For set operations (intersection, subtraction, union, unique):
1. **Use HashMap-based implementation** - 2-5x faster across all sizes
2. **Remove PathMap set operations** - They add complexity without benefit
3. **Reserve PathMap for** its strengths: prefix queries, incremental updates, structural pattern matching

## Files Generated

- `parallel_scaling_results.txt` - Raw parallel scaling benchmark output
- `parallel_intersection_results.txt` - Raw parallel intersection benchmark output
- `crossover_analysis.md` - This analysis document
