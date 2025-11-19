# Phase 2: Prefix-Based Fast Path - Design Document

**Date:** 2025-11-11
**Objective:** Implement O(p) exact match lookups using ReadZipper::descend_to_check()
**Expected Speedup:** 7,000-11,000× for exact matches in large datasets

---

## Problem Statement

Current implementation uses O(n) linear search for ALL lookups:
- `get_type("fibonacci")` with 10,000 types: **2,196 µs** (2.2 ms)
- `has_sexpr_fact((fibonacci 10))` with 1,000 facts: **167 µs**

Target implementation uses O(p) prefix navigation for exact matches:
- Expected: **~200-300 ns** for exact match lookups
- **Speedup: 7,000-11,000×** for get_type() with large datasets

---

## Rholang LSP Pattern (Proven Solution)

From `rholang-language-server/docs/architecture/mork_pathmap_integration.md`:

```rust
// Fast path: Exact match via ReadZipper::descend_to_check()
let mork_bytes = par_to_mork_bytes(pattern);
let mut rz = self.patterns.read_zipper();

if rz.descend_to_check(&mork_bytes) {
    // Found exact match in O(p) time!
    // p = path length (typically 3-5 for MeTTa patterns)
    return Some(result);
}

// Slow path: Pattern query via query_multi() for variables
mork::space::Space::query_multi(&self.patterns.btm, pattern_expr, |bindings, matched| {
    // O(k) where k = candidate matches
    ...
});
```

**Key Insight:** Split lookups into two code paths based on whether the pattern contains variables.

---

## Implementation Plan

### Target Functions

1. **`get_type(name: &str)`** (src/backend/environment.rs:331)
   - Current: O(n) iteration via `while rz.to_next_val()`
   - Optimize: O(p) exact match for `(: name ?)`
   - Pattern: `(: "fibonacci" $type)` - exact prefix match on `(:` and `"fibonacci")`

2. **`has_sexpr_fact(sexpr: &MettaValue)`** (src/backend/environment.rs:594)
   - Current: O(n) iteration via `has_sexpr_fact_linear()`
   - Optimize: O(p) exact match for ground expressions
   - Example: `(fibonacci 10)` - exact match

### Algorithm Design

```rust
fn get_type_optimized(&self, name: &str) -> Option<MettaValue> {
    // Build exact match pattern: (: name)
    let prefix_pattern = format!("(: {})", name);
    let mork_bytes = prefix_pattern_to_mork_bytes(&prefix_pattern)?;

    let space = self.create_space();
    let mut rz = space.btm.read_zipper();

    // Fast path: O(p) exact prefix match
    if rz.descend_to_check(&mork_bytes) {
        // Navigate to the exact location in the trie
        // Extract the type from position: (: name TYPE)
        let expr = Expr { ptr: rz.path().as_ptr().cast_mut() };
        if let Ok(MettaValue::SExpr(items)) = mork_expr_to_metta_value(&expr, &space) {
            if items.len() == 3 {
                return Some(items[2].clone()); // Return TYPE
            }
        }
    }

    // Slow path: Fall back to O(n) if needed
    None
}
```

### Key Implementation Details

1. **Pattern Encoding:**
   - Use existing `metta_to_mork_bytes()` for ground patterns
   - Build exact prefix: `(: atom-name)` for type lookups
   - Must handle MORK symbol interning correctly

2. **Zipper Navigation:**
   - `descend_to_check(&mork_bytes)` navigates trie by prefix
   - Returns `true` if exact path exists
   - Zipper positioned at match location for value extraction

3. **Fallback Strategy:**
   - If `descend_to_check()` returns `false`, pattern not found
   - No need for slow path iteration in most cases
   - For patterns with variables, use existing `query_multi()` path

---

## Expected Performance Impact

### Before (Baseline)

| Operation | Dataset Size | Time (µs) | Algorithm |
|-----------|--------------|-----------|-----------|
| `get_type()` | 10 types | 2.6 | O(n) linear |
| `get_type()` | 1,000 types | 221 | O(n) linear |
| `get_type()` | 10,000 types | 2,196 | O(n) linear |
| `has_fact()` | 1,000 facts | 167 | O(n) linear |

### After (Target)

| Operation | Dataset Size | Time (µs) | Algorithm | Speedup |
|-----------|--------------|-----------|-----------|---------|
| `get_type()` | 10 types | **0.2-0.3** | O(p) exact | **8-13×** |
| `get_type()` | 1,000 types | **0.2-0.3** | O(p) exact | **735-1,105×** |
| `get_type()` | 10,000 types | **0.2-0.3** | O(p) exact | **7,320-10,980×** |
| `has_fact()` | 1,000 facts | **0.2-0.3** | O(p) exact | **556-835×** |

**Note:** Speedup increases with dataset size - this is the key benefit of O(p) vs O(n)!

---

## Implementation Steps

### Step 1: Add Helper Function (descend_to_exact_match)

```rust
/// Try exact match lookup using ReadZipper::descend_to_check()
/// Returns Some(value) if exact match found, None otherwise
fn descend_to_exact_match(&self, pattern: &MettaValue) -> Option<MettaValue> {
    // Convert pattern to MORK bytes
    let mork_bytes = self.metta_to_mork_bytes_cached(pattern).ok()?;

    let space = self.create_space();
    let mut rz = space.btm.read_zipper();

    // O(p) exact match navigation
    if rz.descend_to_check(&mork_bytes) {
        let expr = Expr { ptr: rz.path().as_ptr().cast_mut() };
        return Self::mork_expr_to_metta_value(&expr, &space).ok();
    }

    None
}
```

### Step 2: Optimize get_type()

```rust
pub fn get_type(&self, name: &str) -> Option<MettaValue> {
    // Build exact match prefix: (: name)
    let type_prefix = MettaValue::SExpr(vec![
        MettaValue::Atom(":".to_string()),
        MettaValue::Atom(name.to_string()),
    ]);

    // Fast path: Try exact match
    if let Some(value) = self.descend_to_exact_match(&type_prefix) {
        if let MettaValue::SExpr(items) = value {
            if items.len() == 3 && items[0] == MettaValue::Atom(":".to_string()) {
                return Some(items[2].clone());
            }
        }
    }

    // Slow path: Linear search (only if exact match fails)
    self.get_type_linear(name)
}
```

### Step 3: Optimize has_sexpr_fact()

```rust
pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
    // Fast path: If ground (no variables), try exact match
    if !Self::contains_variables(sexpr) {
        return self.descend_to_exact_match(sexpr).is_some();
    }

    // Slow path: Linear search for patterns with variables
    self.has_sexpr_fact_linear(sexpr)
}
```

---

## Testing Strategy

### Unit Tests

1. **Exact match correctness:**
   - Verify `get_type("fibonacci")` returns correct type
   - Verify `has_sexpr_fact((fibonacci 10))` returns true

2. **Fallback correctness:**
   - Verify pattern queries with variables still work
   - Verify missing keys return None/false

### Benchmark Suite

Create `benches/prefix_fast_path.rs`:

```rust
// Compare O(n) vs O(p) performance
fn bench_get_type_exact_10k(c: &mut Criterion) {
    let mut env = setup_10k_types();
    c.bench_function("get_type_exact_10k", |b| {
        b.iter(|| black_box(env.get_type("fibonacci")))
    });
}
```

### Performance Validation

Must show:
- ✅ **Speedup proportional to dataset size** (O(p) vs O(n))
- ✅ **Exact match ~200-300ns regardless of dataset size**
- ✅ **No regression for pattern queries with variables**

---

## Risks & Mitigation

### Risk 1: MORK Encoding Mismatch

**Problem:** `descend_to_check()` requires exact byte-level match
**Mitigation:** Use same `metta_to_mork_bytes()` for storage and lookup

### Risk 2: Symbol Interning Differences

**Problem:** MORK uses symbol interning, might affect byte encoding
**Mitigation:** Test with fresh Environment instances to verify consistency

### Risk 3: Variable Handling

**Problem:** Prefix match doesn't work for patterns with variables
**Mitigation:** Check `contains_variables()` before attempting fast path

---

## Success Criteria

1. **Performance:** ✅ 1,000× speedup for `get_type()` with 10,000 types
2. **Correctness:** ✅ All existing tests pass
3. **No Regressions:** ✅ Pattern queries with variables still work
4. **Documentation:** ✅ Baseline → Optimized comparison report generated

---

## References

- Rholang LSP MORK/PathMap Integration: `/home/dylon/Workspace/f1r3fly.io/rholang-language-server/docs/architecture/mork_pathmap_integration.md`
- Baseline Analysis: `docs/benchmarks/pattern_matching_optimization/baseline_analysis.md`
- MeTTaTron Environment: `src/backend/environment.rs`
