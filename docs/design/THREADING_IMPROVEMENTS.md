# Threading Improvements Implementation Guide

## Overview

This document provides a complete implementation guide for optimizing MeTTaTron's threading model. It is intended for the engineer responsible for threading optimizations and contains detailed migration plans, code examples, testing strategies, and expected performance improvements.

**Status**: 🚧 **READY FOR IMPLEMENTATION** (Do not implement yet - assigned to threading specialist)

**Prerequisites**: Read [`threading_and_pathmap_integration.md`](threading_and_pathmap_integration.md) first for background.

---

## Table of Contents

1. [Phase 3: Parallel File Loading](#phase-3-parallel-file-loading)
2. [Phase 4: RwLock Migration](#phase-4-rwlock-migration)
3. [Testing Strategy](#testing-strategy)
4. [Performance Benchmarking](#performance-benchmarking)
5. [Migration Checklist](#migration-checklist)
6. [Rollback Plan](#rollback-plan)
7. [Related Work](#related-work)

---

## Phase 3: Parallel File Loading

### Current Implementation

**Location**: `src/main.rs` (file loading), `src/backend/compile.rs` (parsing)

**Current Flow** (Sequential):
```
For each file:
  1. Read file to string
  2. Parse MeTTa source → AST
  3. Compile AST → MettaValue
  4. Add to Environment Space (requires lock)

Total time = N files × (read + parse + compile + insert)
```

**Bottleneck**: Files are processed sequentially even though parsing is independent.

### Proposed Implementation

**Pattern**: Parallel parsing, sequential insertion (from Rholang LSP)

```rust
use rayon::prelude::*;
use std::path::PathBuf;

/// Load multiple MeTTa files in parallel
///
/// Phase 1: Parallel parsing (CPU-bound, independent)
/// Phase 2: Sequential insertion (Space requires exclusive lock)
///
/// Expected speedup: 10-20x for workspaces with 100+ files
pub fn load_workspace_parallel(files: Vec<PathBuf>) -> Result<Environment, String> {
    use std::fs;

    // Phase 1: Parse all files in parallel (no lock contention)
    let parsed_files: Result<Vec<_>, String> = files
        .par_iter()
        .map(|path| {
            // Read file
            let content = fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;

            // Parse (CPU-bound, independent of other files)
            let ast = crate::tree_sitter_parser::parse(&content)
                .map_err(|e| format!("Failed to parse {:?}: {}", path, e))?;

            // Compile to MettaValue (CPU-bound, independent)
            let values = crate::backend::compile::compile_all(&ast)
                .map_err(|e| format!("Failed to compile {:?}: {}", path, e))?;

            Ok((path.clone(), values))
        })
        .collect();

    let parsed_files = parsed_files?;

    // Phase 2: Sequential insertion into Environment Space
    // This requires exclusive access to Space, cannot be parallelized
    let mut env = Environment::new();

    for (path, values) in parsed_files {
        for value in values {
            // Add to Space (requires lock)
            env.add_to_space(&value);

            // If it's a rule, add to rule index
            if let MettaValue::SExpr(items) = &value {
                if items.len() == 3 {
                    if let MettaValue::Atom(op) = &items[0] {
                        if op == "=" {
                            // Parse as rule and add
                            let rule = Rule {
                                lhs: items[1].clone(),
                                rhs: items[2].clone(),
                            };
                            env.add_rule(rule);
                        }
                    }
                }
            }
        }
    }

    Ok(env)
}
```

### Detailed Implementation Steps

**Step 1: Add Rayon Dependency**

`Cargo.toml`:
```toml
[dependencies]
rayon = "1.8"
```

**Step 2: Create Parallel Loading Module**

`src/parallel_loader.rs`:
```rust
//! Parallel file loading for MeTTa workspaces
//!
//! This module provides optimized parallel file loading for large workspaces.
//!
//! # Performance
//!
//! - **Parallel parsing**: N files / num_cpus time
//! - **Sequential insertion**: N files × insert_time (unavoidable, Space requires lock)
//! - **Expected speedup**: 10-20x for 100+ files on multi-core systems
//!
//! # Example
//!
//! ```rust
//! use mettatron::parallel_loader::load_workspace_parallel;
//! use std::path::PathBuf;
//!
//! let files = vec![
//!     PathBuf::from("file1.metta"),
//!     PathBuf::from("file2.metta"),
//!     // ... 100 more files ...
//! ];
//!
//! let env = load_workspace_parallel(files)?;
//! ```

use rayon::prelude::*;
use std::path::PathBuf;
use crate::backend::Environment;
use crate::backend::MettaValue;

/// Parsed file data ready for insertion
struct ParsedFile {
    path: PathBuf,
    values: Vec<MettaValue>,
}

/// Load multiple MeTTa files in parallel
///
/// This function parallelizes the CPU-bound parsing phase while keeping
/// the Space insertion sequential (required for thread safety).
///
/// # Arguments
///
/// * `files` - Paths to MeTTa files to load
///
/// # Returns
///
/// Environment with all files loaded, or error if any file fails
///
/// # Performance
///
/// Parsing is parallelized across available CPU cores. Insertion into
/// Space is sequential to avoid lock contention. Overall speedup depends
/// on parse/insert time ratio (typically 10-20x for large workspaces).
pub fn load_workspace_parallel(files: Vec<PathBuf>) -> Result<Environment, String> {
    // Phase 1: Parallel parsing (CPU-bound, no lock contention)
    let parsed_files: Result<Vec<ParsedFile>, String> = files
        .par_iter()
        .map(parse_file)
        .collect();

    let parsed_files = parsed_files?;

    // Phase 2: Sequential insertion (requires Space lock)
    insert_into_environment(parsed_files)
}

/// Parse a single file (called in parallel)
fn parse_file(path: &PathBuf) -> Result<ParsedFile, String> {
    use std::fs;

    // Read file
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;

    // Parse using Tree-Sitter
    let ast = crate::tree_sitter_parser::parse(&content)
        .map_err(|e| format!("Failed to parse {:?}: {}", path, e))?;

    // Compile to MettaValue
    let values = crate::backend::compile::compile_all(&ast)
        .map_err(|e| format!("Failed to compile {:?}: {}", path, e))?;

    Ok(ParsedFile {
        path: path.clone(),
        values,
    })
}

/// Insert all parsed values into Environment (sequential)
fn insert_into_environment(parsed_files: Vec<ParsedFile>) -> Result<Environment, String> {
    let mut env = Environment::new();

    for file in parsed_files {
        for value in file.values {
            // Determine if this is a rule, type assertion, or plain fact
            if let MettaValue::SExpr(ref items) = value {
                if items.len() == 3 {
                    match items[0] {
                        MettaValue::Atom(ref op) if op == "=" => {
                            // Rule definition: (= lhs rhs)
                            let rule = crate::backend::Rule {
                                lhs: items[1].clone(),
                                rhs: items[2].clone(),
                            };
                            env.add_rule(rule);
                            continue; // add_rule already adds to Space
                        }
                        MettaValue::Atom(ref op) if op == ":" => {
                            // Type assertion: (: name type)
                            if let MettaValue::Atom(ref name) = items[1] {
                                env.add_type_assertion(name, items[2].clone());
                                continue; // add_type_assertion already adds to Space
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Plain fact or unmatched s-expression
            env.add_to_space(&value);
        }
    }

    Ok(env)
}

/// Configuration for parallel loading
pub struct ParallelLoadConfig {
    /// Number of threads to use (default: num_cpus)
    pub num_threads: Option<usize>,

    /// Batch size for processing files (default: 10)
    pub batch_size: usize,
}

impl Default for ParallelLoadConfig {
    fn default() -> Self {
        ParallelLoadConfig {
            num_threads: None, // Use Rayon default (num_cpus)
            batch_size: 10,
        }
    }
}

/// Load workspace with custom configuration
pub fn load_workspace_with_config(
    files: Vec<PathBuf>,
    config: ParallelLoadConfig,
) -> Result<Environment, String> {
    // Configure Rayon thread pool if specified
    if let Some(num_threads) = config.num_threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .map_err(|e| format!("Failed to build thread pool: {}", e))?
            .install(|| load_workspace_parallel(files))
    } else {
        load_workspace_parallel(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parallel_loading_single_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "(= (fib 0) 1)").unwrap();
        writeln!(file, "(= (fib 1) 1)").unwrap();

        let files = vec![file.path().to_path_buf()];
        let env = load_workspace_parallel(files).unwrap();

        assert_eq!(env.rule_count(), 2);
    }

    #[test]
    fn test_parallel_loading_multiple_files() {
        let mut file1 = NamedTempFile::new().unwrap();
        let mut file2 = NamedTempFile::new().unwrap();

        writeln!(file1, "(= (add 0 $n) $n)").unwrap();
        writeln!(file2, "(= (mul 0 $n) 0)").unwrap();

        let files = vec![
            file1.path().to_path_buf(),
            file2.path().to_path_buf(),
        ];

        let env = load_workspace_parallel(files).unwrap();
        assert_eq!(env.rule_count(), 2);
    }
}
```

**Step 3: Expose in Public API**

`src/lib.rs`:
```rust
pub mod parallel_loader;
```

**Step 4: Update CLI to Use Parallel Loading**

`src/main.rs`:
```rust
use mettatron::parallel_loader::load_workspace_parallel;

// When loading multiple files from directory
if args.files.len() > 1 {
    let env = load_workspace_parallel(args.files)?;
    // ... continue with env
} else {
    // Single file: use sequential loading
    // ...
}
```

### Performance Expectations

**Benchmark Setup**:
```rust
#[bench]
fn bench_sequential_loading(b: &mut Bencher) {
    let files = generate_test_files(100); // 100 files, 10 rules each
    b.iter(|| {
        for file in &files {
            load_file_sequential(file);
        }
    });
}

#[bench]
fn bench_parallel_loading(b: &mut Bencher) {
    let files = generate_test_files(100);
    b.iter(|| {
        load_workspace_parallel(files.clone()).unwrap();
    });
}
```

**Expected Results** (36 cores, 100 files):
- Sequential: ~1000ms (10ms per file)
- Parallel: ~50ms (10ms per file / 20 cores + insertion overhead)
- **Speedup**: ~20x

**Scaling**:
- 10 files: 2-3x speedup (parsing overhead)
- 50 files: 10-15x speedup
- 100 files: 15-20x speedup
- 500+ files: 18-22x speedup (approaches num_cpus limit)

### Testing Strategy

**Unit Tests**:
1. Single file loading
2. Multiple file loading
3. Error handling (missing file, parse error)
4. Rule vs fact insertion

**Integration Tests**:
1. Load stdlib (if exists)
2. Load large workspace (100+ files)
3. Verify Environment correctness (all rules present)
4. Compare sequential vs parallel results (must be identical)

**Stress Tests**:
1. 1000 files × 100 rules each
2. Very large files (10MB+)
3. Concurrent loading from multiple threads

### Known Limitations

1. **Insertion Bottleneck**: Space insertion is sequential (unavoidable with current `Arc<Mutex<Space>>`)
   - Mitigation: Phase 4 RwLock won't help here (writes still exclusive)
   - Future: Batch insertion API or lock-free data structure

2. **Memory Usage**: All files parsed before insertion starts
   - Mitigation: Process in batches of N files
   - Typical: 100 files × 100 rules × 1KB = ~10MB (acceptable)

3. **Error Reporting**: First error stops entire workspace loading
   - Mitigation: Return `Vec<Result<>>` instead of `Result<Vec<>>`
   - Trade-off: Partial loading vs fail-fast semantics

---

## Phase 4: RwLock Migration

### Current Implementation

**Location**: `src/backend/environment.rs:20`

```rust
pub struct Environment {
    pub space: Arc<Mutex<Space>>,
    // ... other fields
}
```

**Problem**: `Mutex` serializes ALL operations (reads + writes)
- Thread 1 reading → Thread 2 reading = **blocked** ❌
- 95%+ of operations are reads (queries, pattern matching)
- Huge opportunity for parallelism

### Proposed Implementation

**Pattern**: `RwLock` allows multiple concurrent readers

```rust
use std::sync::{Arc, RwLock};

pub struct Environment {
    /// MORK Space: primary fact database
    /// Thread-safe via Arc<RwLock<>> for parallel read access
    /// Multiple readers can query simultaneously, writes are exclusive
    pub space: Arc<RwLock<Space>>,

    /// Rule index: Maps (head_symbol, arity) -> Vec<Rule>
    rule_index: Arc<RwLock<HashMap<(String, usize), Vec<Rule>>>>,

    /// Wildcard rules: Rules without clear head symbol
    wildcard_rules: Arc<RwLock<Vec<Rule>>>,

    /// Multiplicities: tracks rule definition counts
    multiplicities: Arc<RwLock<HashMap<String, usize>>>,

    /// Pattern cache: LRU cache for MORK serialization
    pattern_cache: Arc<RwLock<LruCache<MettaValue, Vec<u8>>>>,

    /// Fuzzy matcher: Symbol suggestions (already thread-safe)
    fuzzy_matcher: FuzzyMatcher,
}
```

### Migration Plan

**Step 1: Update Environment Struct**

Change all `Mutex` to `RwLock`:

```rust
// Before
pub space: Arc<Mutex<Space>>,

// After
pub space: Arc<RwLock<Space>>,
```

**Step 2: Update All Lock Sites (22 locations)**

Read operations (18 sites):
```rust
// Before
let space = self.space.lock().unwrap();

// After
let space = self.space.read().unwrap();
```

Write operations (4 sites):
```rust
// Before
let mut space = self.space.lock().unwrap();

// After
let mut space = self.space.write().unwrap();
```

**Step 3: Detailed Lock Site Audit**

| Function | Line | Operation | Before | After |
|----------|------|-----------|--------|-------|
| `get_type()` | 306 | Read | `.lock()` | `.read()` |
| `iter_rules()` | 348 | Read | `.lock()` | `.read()` |
| `rebuild_rule_index()` | 387 | Read | `.lock()` | `.read()` |
| `match_space()` | 431 | Read | `.lock()` | `.read()` |
| `get_multiplicity()` | 514 | Read | `.lock()` | `.read()` |
| `get_multiplicities()` | 520 | Read | `.lock()` | `.read()` |
| `set_multiplicities()` | 537 | Write | `.lock()` | `.write()` |
| `has_fact()` | 552 | Read | `.lock()` | `.read()` |
| `has_sexpr_fact_optimized()` | 578 | Read | `.lock()` | `.read()` |
| `has_sexpr_fact_linear()` | 618 | Read | `.lock()` | `.read()` |
| `value_to_mork_bytes()` | 666 | Read | `.lock()` | `.read()` |
| `add_to_space()` | 738 | Write | `.lock()` | `.write()` |
| `get_matching_rules()` | 752 | Read | `.lock()` | `.read()` |
| `get_matching_rules()` | 760 | Read | `.lock()` | `.read()` |
| `add_rule()` | 476 | Read | `.lock()` | `.read()` |
| `add_rule()` | 486 | Write | `.lock()` | `.write()` |
| `add_rule()` | 496 | Write | `.lock()` | `.write()` |

**Step 4: Update Rule Index Locks**

```rust
// Before
let index = self.rule_index.lock().unwrap();

// After (read)
let index = self.rule_index.read().unwrap();

// After (write)
let mut index = self.rule_index.write().unwrap();
```

**Step 5: Update Other Locked Fields**

Apply same pattern to:
- `wildcard_rules`
- `multiplicities`
- `pattern_cache`

### Code Changes Summary

**File**: `src/backend/environment.rs`

**Import Change**:
```rust
// Before
use std::sync::{Arc, Mutex};

// After
use std::sync::{Arc, RwLock};
```

**Constructor Change**:
```rust
// Before
Environment {
    space: Arc::new(Mutex::new(Space::new())),
    // ...
}

// After
Environment {
    space: Arc::new(RwLock::new(Space::new())),
    // ...
}
```

**Read Operation Pattern** (18 sites):
```rust
// Before
let space = self.space.lock().unwrap();

// After
let space = self.space.read().unwrap();
```

**Write Operation Pattern** (4 sites):
```rust
// Before
let mut space = self.space.lock().unwrap();

// After
let mut space = self.space.write().unwrap();
```

### Performance Expectations

**Benchmark Setup**:
```rust
use std::sync::Arc;
use std::thread;

#[bench]
fn bench_mutex_concurrent_reads(b: &mut Bencher) {
    let env = Arc::new(Environment::new()); // Using Mutex

    b.iter(|| {
        let handles: Vec<_> = (0..10).map(|_| {
            let env = Arc::clone(&env);
            thread::spawn(move || {
                for _ in 0..100 {
                    let _ = env.get_type("test");
                }
            })
        }).collect();

        for h in handles {
            h.join().unwrap();
        }
    });
}

#[bench]
fn bench_rwlock_concurrent_reads(b: &mut Bencher) {
    let env = Arc::new(Environment::new()); // Using RwLock

    // Same as above
}
```

**Expected Results** (10 threads, 1000 reads):
- Mutex: ~50ms (serialized reads, 10 threads waiting)
- RwLock: ~10ms (concurrent reads, no waiting)
- **Speedup**: ~5x

**Scaling**:
- 2 threads: 1.5-2x speedup
- 4 threads: 3-3.5x speedup
- 8 threads: 4-5x speedup
- 16+ threads: 5-8x speedup (diminishing returns)

### Testing Strategy

**Unit Tests**:
1. Single-threaded tests (should pass unchanged)
2. Multi-threaded read tests (verify no deadlocks)
3. Multi-threaded write tests (verify exclusivity)
4. Mixed read/write tests (verify correctness)

**Stress Tests**:
```rust
#[test]
fn test_concurrent_reads() {
    let env = Arc::new(Environment::new());

    // Add some rules
    env.add_rule(...);

    // Spawn 100 threads, each doing 1000 reads
    let handles: Vec<_> = (0..100).map(|_| {
        let env = Arc::clone(&env);
        thread::spawn(move || {
            for _ in 0..1000 {
                let _ = env.get_matching_rules("test", 1);
            }
        })
    }).collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_read_write_interleaving() {
    let env = Arc::new(Environment::new());

    // Spawn readers
    let readers: Vec<_> = (0..50).map(|_| {
        let env = Arc::clone(&env);
        thread::spawn(move || {
            for i in 0..100 {
                let _ = env.get_matching_rules("test", 1);
                thread::sleep(Duration::from_micros(10));
            }
        })
    }).collect();

    // Spawn writers
    let writers: Vec<_> = (0..5).map(|i| {
        let env = Arc::clone(&env);
        thread::spawn(move || {
            for j in 0..20 {
                let rule = Rule { /* ... */ };
                env.add_rule(rule);
                thread::sleep(Duration::from_millis(1));
            }
        })
    }).collect();

    for h in readers.into_iter().chain(writers) {
        h.join().unwrap();
    }
}
```

### Potential Issues and Mitigations

**1. Writer Starvation**

**Problem**: Many readers can starve writers
**Symptom**: `add_rule()` blocks indefinitely
**Solution**: RwLock in Rust uses fair scheduling (no starvation by default)
**Monitoring**: Add metrics for write lock wait times

**2. Deadlocks with Nested Locks**

**Problem**: `read()` followed by `write()` in same thread can deadlock
**Example**:
```rust
// ❌ DEADLOCK
let space = self.space.read().unwrap();
// ... do something ...
let mut space = self.space.write().unwrap(); // Deadlock!
```

**Solution**: Never upgrade read lock to write lock
**Pattern**: Drop read lock before acquiring write lock
```rust
// ✅ CORRECT
{
    let space = self.space.read().unwrap();
    // ... read data ...
} // Drop read lock

let mut space = self.space.write().unwrap(); // Acquire write lock
```

**3. Poisoned Locks**

**Problem**: Panic while holding lock poisons it
**Mitigation**: Use `.expect()` with clear messages for debugging
```rust
let space = self.space.read()
    .expect("Space lock poisoned - check for panics in other threads");
```

**4. Performance Regression on Single-Threaded Workloads**

**Expected**: RwLock has ~5-10ns higher overhead than Mutex
**Impact**: Negligible (<1%) for typical operations (microseconds)
**Verification**: Run single-threaded benchmarks before/after

### Monitoring and Metrics

**Add to Environment**:
```rust
#[derive(Default)]
pub struct LockMetrics {
    pub read_lock_acquisitions: AtomicUsize,
    pub write_lock_acquisitions: AtomicUsize,
    pub read_lock_wait_time_ns: AtomicU64,
    pub write_lock_wait_time_ns: AtomicU64,
}

impl Environment {
    pub fn lock_metrics(&self) -> &LockMetrics {
        &self.metrics
    }
}
```

**Instrument Lock Sites**:
```rust
let start = Instant::now();
let space = self.space.read().unwrap();
self.metrics.read_lock_acquisitions.fetch_add(1, Ordering::Relaxed);
self.metrics.read_lock_wait_time_ns.fetch_add(
    start.elapsed().as_nanos() as u64,
    Ordering::Relaxed
);
```

### Rollback Plan

If RwLock migration causes issues:

**1. Immediate Rollback** (< 1 hour):
```bash
git revert <commit-hash>
cargo test
cargo build --release
```

**2. Partial Rollback** (keep some RwLocks):
- Keep RwLock for `space` (highest benefit)
- Revert RwLock for `rule_index` (if issues)
- Revert RwLock for `pattern_cache` (if contention)

**3. Staged Migration**:
- Week 1: Migrate only `space`
- Week 2: Migrate `rule_index` and `wildcard_rules`
- Week 3: Migrate remaining fields

---

## Testing Strategy

### Unit Tests

**Coverage Requirements**:
- ✅ All existing tests must pass
- ✅ New tests for concurrent operations
- ✅ Lock acquisition patterns
- ✅ Error handling

**New Tests**:
```rust
#[test]
fn test_concurrent_rule_queries() {
    // Multiple threads querying rules simultaneously
}

#[test]
fn test_concurrent_fact_insertions() {
    // Multiple threads adding facts (should serialize)
}

#[test]
fn test_read_write_correctness() {
    // Verify reads during writes see consistent state
}
```

### Integration Tests

**Scenarios**:
1. Load stdlib + evaluate queries (parallel)
2. REPL with background rule additions
3. Concurrent file loading (Phase 3)

### Performance Tests

**Benchmarks** (see [Performance Benchmarking](#performance-benchmarking)):
1. Baseline (current Mutex)
2. After Phase 3 (parallel loading)
3. After Phase 4 (RwLock)

**Metrics**:
- Throughput (ops/sec)
- Latency (p50, p95, p99)
- Lock contention (wait times)
- CPU utilization

---

## Performance Benchmarking

### Benchmark Suite

**Location**: `benches/threading_benchmarks.rs`

```rust
#![feature(test)]
extern crate test;

use test::Bencher;
use mettatron::*;
use std::sync::Arc;
use std::thread;

// Baseline: Single-threaded operations
#[bench]
fn bench_get_type_single_thread(b: &mut Bencher) {
    let env = Environment::new();
    env.add_type_assertion("test", MettaValue::Atom("Int".to_string()));

    b.iter(|| {
        env.get_type("test")
    });
}

// Concurrent reads (Mutex vs RwLock comparison)
#[bench]
fn bench_get_type_10_threads(b: &mut Bencher) {
    let env = Arc::new(Environment::new());
    env.add_type_assertion("test", MettaValue::Atom("Int".to_string()));

    b.iter(|| {
        let handles: Vec<_> = (0..10).map(|_| {
            let env = Arc::clone(&env);
            thread::spawn(move || {
                for _ in 0..100 {
                    env.get_type("test");
                }
            })
        }).collect();

        for h in handles {
            h.join().unwrap();
        }
    });
}

// Parallel file loading
#[bench]
fn bench_sequential_file_loading(b: &mut Bencher) {
    let files = generate_test_files(50);
    b.iter(|| {
        load_files_sequential(&files)
    });
}

#[bench]
fn bench_parallel_file_loading(b: &mut Bencher) {
    let files = generate_test_files(50);
    b.iter(|| {
        load_workspace_parallel(files.clone())
    });
}

// Lock contention stress test
#[bench]
fn bench_high_contention_reads(b: &mut Bencher) {
    let env = Arc::new(Environment::new());

    // Add many rules
    for i in 0..1000 {
        env.add_rule(generate_test_rule(i));
    }

    b.iter(|| {
        let handles: Vec<_> = (0..100).map(|_| {
            let env = Arc::clone(&env);
            thread::spawn(move || {
                for _ in 0..10 {
                    env.get_matching_rules("test", 1);
                }
            })
        }).collect();

        for h in handles {
            h.join().unwrap();
        }
    });
}
```

### Running Benchmarks

```bash
# Baseline (before changes)
cargo bench --bench threading_benchmarks > baseline.txt

# After Phase 3 (parallel loading)
cargo bench --bench threading_benchmarks > phase3.txt

# After Phase 4 (RwLock)
cargo bench --bench threading_benchmarks > phase4.txt

# Compare
cargo benchcmp baseline.txt phase3.txt
cargo benchcmp baseline.txt phase4.txt
```

### Flamegraph Generation

```bash
# Install cargo-flamegraph
cargo install flamegraph

# Generate flamegraph for baseline
cargo flamegraph --bench threading_benchmarks -- --bench bench_high_contention_reads

# Generates flamegraph.svg
firefox flamegraph.svg
```

### Profiling with perf

```bash
# Record performance data
cargo build --release
perf record --call-graph dwarf ./target/release/mettatron benchmark.metta

# Analyze
perf report

# Generate flamegraph
perf script | stackcollapse-perf.pl | flamegraph.pl > perf.svg
```

---

## Migration Checklist

### Pre-Implementation

- [ ] Read threading documentation thoroughly
- [ ] Understand current lock patterns (22 sites)
- [ ] Set up benchmark baseline
- [ ] Create feature branch: `feat/threading-optimizations`

### Phase 3: Parallel File Loading

- [ ] Add Rayon dependency
- [ ] Create `src/parallel_loader.rs`
- [ ] Implement `load_workspace_parallel()`
- [ ] Write unit tests (3 minimum)
- [ ] Write integration tests
- [ ] Benchmark against sequential loading
- [ ] Document performance gains
- [ ] Update CLI to use parallel loading
- [ ] Update README with performance notes

### Phase 4: RwLock Migration

- [ ] Update imports (`Mutex` → `RwLock`)
- [ ] Update Environment struct (5 fields)
- [ ] Update constructor
- [ ] Migrate read operations (18 sites)
- [ ] Migrate write operations (4 sites)
- [ ] Run all unit tests
- [ ] Add concurrent read tests
- [ ] Add read/write interleaving tests
- [ ] Benchmark against Mutex baseline
- [ ] Generate flamegraphs (before/after)
- [ ] Document performance gains
- [ ] Update threading documentation

### Post-Implementation

- [ ] Run full test suite
- [ ] Verify no performance regressions
- [ ] Update `optimization_summary.md` with results
- [ ] Code review with team
- [ ] Merge to main

---

## Rollback Plan

### If Tests Fail

**Scenario 1: Deadlock Detected**
```bash
# Identify deadlocking test
cargo test -- --nocapture

# Review lock acquisition order
# Fix: Ensure consistent lock ordering

# If unfixable quickly:
git revert <commit>
```

**Scenario 2: Performance Regression**
```bash
# Compare benchmarks
cargo benchcmp baseline.txt current.txt

# If > 5% regression on critical path:
# 1. Profile with perf to identify bottleneck
# 2. Adjust RwLock strategy (e.g., keep Mutex for small locks)
# 3. If unfixable: rollback
```

**Scenario 3: Race Condition**
```bash
# Run with thread sanitizer
RUSTFLAGS="-Z sanitizer=thread" cargo test

# Fix identified races
# Add memory ordering annotations if needed
```

### Rollback Commands

```bash
# Full rollback
git log --oneline | grep "threading"
git revert <commit-hash>
cargo test
cargo bench

# Partial rollback (keep Phase 3, revert Phase 4)
git revert <phase4-commit>
git cherry-pick <phase3-commit>
```

---

## Related Work

### Dependencies

- **Rayon 1.8**: Parallel file loading (Phase 3)
- **std::sync::RwLock**: Read-write lock (Phase 4, no new deps)
- **cargo-flamegraph**: Performance profiling (dev)

### Documentation

- **Primary**: [`threading_and_pathmap_integration.md`](threading_and_pathmap_integration.md)
- **Performance**: [`optimization_summary.md`](optimization_summary.md)
- **Architecture**: [`.claude/CLAUDE.md`](../.claude/CLAUDE.md)

### External References

- **Rholang LSP**: [`mork_pathmap_integration.md`](../../../rholang-language-server/docs/architecture/mork_pathmap_integration.md)
- **Rust RwLock Docs**: https://doc.rust-lang.org/std/sync/struct.RwLock.html
- **Rayon Docs**: https://docs.rs/rayon/latest/rayon/

---

## Summary

**Phase 3: Parallel File Loading**
- **Complexity**: Medium (new module, ~200 LOC)
- **Risk**: Low (doesn't modify Environment internals)
- **Expected Impact**: 10-20x workspace loading speedup
- **Time Estimate**: 2-3 days

**Phase 4: RwLock Migration**
- **Complexity**: Low (find-replace on 22 sites)
- **Risk**: Medium (potential deadlocks if done incorrectly)
- **Expected Impact**: 2-5x concurrent read speedup
- **Time Estimate**: 2-3 days + 1 day testing

**Total Effort**: ~1 week for both phases

**Success Criteria**:
- ✅ All 403+ tests passing
- ✅ No performance regressions on single-threaded workloads
- ✅ Measurable speedup on multi-threaded benchmarks
- ✅ No deadlocks under stress testing
- ✅ Clean flamegraphs showing improved parallelism

Good luck! 🚀
