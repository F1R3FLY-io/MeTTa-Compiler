# Maximum Single-Threaded Performance Optimizations for MeTTa Evaluator

## Context

After the VecDeque + cartesian fast path optimizations, the benchmark improved from 6.58s to 81ms (81x faster) for 32768 operations.

**Goal**: Maximum single-threaded performance, SmallVec accepted (speed > size)

---

## Implemented Optimizations Summary

### Optimization 1: Arc<MettaValue> for Error/Type ✓ KEEP
**Status**: Implemented and retained
**Result**: Neutral performance (no regression, no improvement in arithmetic benchmark)
**Rationale**: Structural benefit - O(1) clone for Error/Type values. Benefits error-heavy code paths.

### Optimization 2: apply_bindings Fast Path ✗ REJECTED
**Status**: Tested and reverted
**Result**: **3-15% regression** at small operation sizes (2-32 ops)
**Reason**: The `is_empty()` check adds overhead that hurts small expressions more than it helps large ones.

### Optimization 3: get_head_symbol Return &str ✓ KEEP
**Status**: Implemented and retained
**Result**: **3-15% improvement** at small-to-medium operation sizes (2-4096 ops)
**Rationale**: Avoids String allocation on every rule lookup. Returns reference to existing string.

| Operations | Improvement |
|------------|-------------|
| 2-8 ops | 5-8% |
| 12 ops | 12.5% |
| 32 ops | 13.3% |
| 64-4096 ops | 4-8% |
| 32768 ops | ~neutral |

### Optimization 4: get_matching_rules Simplification ✓ KEEP
**Status**: Implemented
**Result**: Minor improvement (reduced from 4 lock acquisitions to 2, eliminated duplicate string allocation)
**Rationale**: Hot path optimization for rule lookup.

### Optimization 5: SmallVec for SExpr ✗ NOT POSSIBLE
**Status**: Cannot implement due to recursive type limitation
**Reason**: `SmallVec<[MettaValue; 4]>` requires fixed size at compile time, but MettaValue is recursive.
Rust error: `recursive type 'MettaValue' has infinite size`

---

## Future Optimization: String Interning for Atoms (DOCUMENTED)

**Status**: Module implemented (`src/backend/intern.rs`), not yet integrated
**Impact**: Estimated 15-25% improvement
**Effort**: 4-6 hours (requires ~1526 call site changes)
**Decision**: Skipped for now - integration requires significant refactoring

### Module Location
Create `src/backend/intern.rs` and add `pub mod intern;` to `src/backend/mod.rs`

### Implementation Design

```rust
// src/backend/intern.rs
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// A global string interner for atom symbols
static INTERNER: LazyLock<RwLock<StringInterner>> =
    LazyLock::new(|| RwLock::new(StringInterner::new()));

/// An interned string handle - cheap to copy and compare
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol(u32);

impl Symbol {
    /// Get the string value of this symbol
    pub fn resolve(&self) -> String {
        INTERNER.read().unwrap().resolve(*self).to_string()
    }

    /// Check if this symbol starts with a given prefix
    pub fn starts_with(&self, prefix: &str) -> bool {
        INTERNER.read().unwrap().resolve(*self).starts_with(prefix)
    }

    /// Check if this symbol equals a string
    pub fn eq_str(&self, s: &str) -> bool {
        INTERNER.read().unwrap().resolve(*self) == s
    }
}

/// Intern a string and return its symbol
pub fn intern(s: &str) -> Symbol {
    INTERNER.write().unwrap().intern(s)
}

/// Intern a String, consuming it
pub fn intern_string(s: String) -> Symbol {
    INTERNER.write().unwrap().intern_string(s)
}
```

### Integration Steps (for future implementation)

1. **Change MettaValue::Atom** (requires updating ~1526 call sites):
   ```rust
   // Before (current)
   Atom(String),

   // After (with interning)
   Atom(Symbol),
   ```

2. **Update all Atom construction sites**:
   ```rust
   // Before
   MettaValue::Atom("foo".to_string())

   // After
   MettaValue::Atom(intern("foo"))
   ```

3. **Update all Atom pattern matches**:
   ```rust
   // Before
   MettaValue::Atom(s) => { ... s.as_str() ... }

   // After
   MettaValue::Atom(sym) => { ... sym.resolve() ... }
   // Or use sym.eq_str("...") for comparisons
   ```

4. **Update rule_index HashMap key**:
   ```rust
   // Before
   HashMap<(String, usize), Vec<Rule>>

   // After
   HashMap<(Symbol, usize), Vec<Rule>>
   ```

### Benefits
- **O(1) atom comparison**: u32 vs string comparison
- **Reduced memory**: Duplicate atoms share storage
- **Faster HashMap lookups**: u32 hash faster than string hash
- **O(1) clone**: Symbol is Copy

### Call Site Count by File
Run this to see all locations requiring changes:
```bash
grep -rn "MettaValue::Atom" src/backend/ | wc -l
# Result: ~1526 matches
```

Major files to update:
- `src/backend/compile.rs` - Atom construction from parsing
- `src/backend/eval/*.rs` - Atom pattern matching
- `src/backend/models/metta_value.rs` - Atom definition and methods
- `src/backend/environment.rs` - Rule index keys
- `src/backend/mork_convert.rs` - Conversion between formats

---

## Future Optimization: Rule Vec Reference Sharing

**Status**: Not yet implemented
**Impact**: Estimated 8-15% improvement
**Effort**: 2 hours

### Changes Required
1. Change rule_index type:
   ```rust
   // Before
   rule_index: Arc<RwLock<HashMap<(String, usize), Vec<Rule>>>>,

   // After
   rule_index: Arc<RwLock<HashMap<(String, usize), Arc<Vec<Rule>>>>>,
   ```

2. Update `get_matching_rules()` to return `Arc<Vec<Rule>>` or slice reference instead of cloning

3. Update `add_rule()` to wrap in Arc

**Rationale**: Avoid cloning entire Vec<Rule> on every S-expression evaluation.

---

## Final Benchmark Results (2024-11-27)

Current absolute timings with all implemented optimizations:

| Operations | Time |
|------------|------|
| 2 ops | 5.4-5.5 µs |
| 4 ops | 7.7-7.8 µs |
| 8 ops | 9.8-9.9 µs |
| 12 ops | 11.9 µs |
| 16 ops | 14 µs |
| 32 ops | 23.1-23.8 µs |
| 64 ops | 36.5-37.2 µs |
| 128 ops | 73.9-78.9 µs |
| 256 ops | 145-148 µs |
| 512 ops | 302-312 µs |
| 1024 ops | 597-616 µs |
| 2048 ops | 1.25-1.27 ms |
| 4096 ops | 2.37-2.50 ms |
| 8192 ops | 5.27-5.54 ms |
| 16384 ops | 10.7-11.1 ms |
| 32768 ops | 21.4-22 ms |
| 65536 ops | 37.6-38.2 ms |
| 131072 ops | 83.3-89.1 ms |

**Summary**: The evaluator has been significantly optimized from the original 6.58s to ~22ms for 32768 operations (~300x improvement).

---

## Files Modified

| File | Changes |
|------|---------|
| `src/backend/models/metta_value.rs` | Arc for Error/Type, get_head_symbol returns &str |
| `src/backend/eval/mod.rs` | get_head_symbol returns &str |
| `src/backend/environment.rs` | get_matching_rules simplification, get_head_symbol callers updated |

---

## Benchmarking Protocol

```bash
taskset -c 0-3 cargo bench --bench expression_parallelism -- "threshold_tuning" --sample-size 20
```
