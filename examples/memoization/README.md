# Memoization Examples

This directory contains examples demonstrating the memoization feature in MeTTa.

## Overview

Memoization is a technique that caches the results of expensive function calls
and returns the cached result when the same inputs occur again. It's particularly
effective for recursive functions with overlapping subproblems.

## MeTTa Memoization API

```metta
; Create a memo table
!(bind! &cache (new-memo "cache-name"))

; Memoize an expression (caches all results)
(memo &cache expr)

; Memoize and return only first result (more efficient)
(memo-first &cache expr)

; Clear the cache
(clear-memo! &cache)

; Get cache statistics
; Returns: (stats entries hits misses evictions hit_rate_percent)
(memo-stats &cache)
```

## Examples

### Fibonacci (`fibonacci.metta`)

The classic example of exponential-to-linear transformation:

| Version | Time Complexity | fib(30) Time | fib(90) Possible? |
|---------|-----------------|--------------|-------------------|
| Without memo | O(2^n) | ~minutes | No (heat death of universe) |
| With memo | O(n) | ~instant | Yes (~9ms) |

**Speedup: ~1000x or more**

### Levenshtein Distance (`levenshtein.metta`)

Edit distance algorithm used in spell checkers, DNA sequence alignment, etc:

| Version | Time Complexity | "kitten"→"sitting" |
|---------|-----------------|-------------------|
| Without memo | O(3^(n+m)) | ~4 seconds |
| With memo | O(n*m) | ~23ms |

**Speedup: ~178x**

## When to Use Memoization

**Good candidates:**
- Recursive functions with overlapping subproblems
- Pure functions (same inputs always produce same outputs)
- Expensive computations called multiple times with same arguments
- Dynamic programming algorithms

**Not effective for:**
- Functions with mostly unique inputs (low cache hit rate)
- Functions with complex expression keys (hashing overhead)
- Frequently changing data (cache invalidation overhead)

## Running the Examples

```bash
# With memoization
./target/release/mettatron examples/memoization/fibonacci.metta
./target/release/mettatron examples/memoization/levenshtein.metta

# Without memoization (for comparison - slow!)
./target/release/mettatron examples/memoization/fibonacci_no_memo.metta
./target/release/mettatron examples/memoization/levenshtein_no_memo.metta
```

## Benchmark Results

Run with `time` to see the difference:

```bash
time ./target/release/mettatron examples/memoization/fibonacci_no_memo.metta
# ~5 seconds for fib(25)

time ./target/release/mettatron examples/memoization/fibonacci.metta
# ~9ms for fib(90)!
```
