# Phase 4: RwLock Threading Pattern Refactoring

## Executive Summary

This document describes the ongoing refactoring to migrate MeTTaTron from `Arc<Mutex<Space>>` to the Rholang LSP threading pattern using `SharedMappingHandle` + `PathMap` with thread-local `Space` creation.

**Status**: In Progress (Struct updated, methods pending)
**Reason**: Enable `Send` for Environment (cross-thread usage) without requiring `Sync`
**Blocker**: PathMap's `Cell<u64>` prevents `Arc<RwLock<Space>>`

---

## Problem Statement

### Initial Attempt: Arc<RwLock<Space>>

We attempted to replace `Arc<Mutex<Space>>` with `Arc<RwLock<Space>>` to enable concurrent reads:

```rust
// ❌ FAILED: Does not compile
pub struct Environment {
    pub space: Arc<RwLock<Space>>,  // Compilation error!
    // ...
}
```

**Compilation Error**:
```
error[E0277]: `Cell<u64>` cannot be shared between threads safely
  --> src/rholang_integration.rs:362:34
   |
362 |   task::spawn_blocking(move || {
   |   ^^^^^^^^^^^^^^^^^^^^ `Cell<u64>` cannot be shared between threads safely
```

### Root Cause

PathMap's `ArenaCompactTree` uses `Cell<u64>` internally:

```rust
// From PathMap source
pub struct ArenaCompactTree<A: Allocator> {
    next_id: Cell<u64>,  // ❌ NOT Sync!
    // ...
}
```

**Type Requirements**:
- `Mutex<T>` requires: `T: Send` ✅ Space is Send
- `RwLock<T>` requires: `T: Send + Sync` ❌ Space is NOT Sync

**Current State**:
- `Arc<Mutex<Space>>`: ✅ Compiles, ❌ Serializes all reads
- `Arc<RwLock<Space>>`: ❌ Doesn't compile

---

## Solution: Rholang LSP Pattern

### Pattern Overview

From `rholang-language-server/docs/architecture/mork_pathmap_integration.md`:

**Core Principle**: Don't store `Space` - store its components and create `Space` thread-locally

```rust
pub struct Environment {
    // THREAD-SAFE: Can be cloned and shared
    shared_mapping: SharedMappingHandle,  // Send + Sync

    // THREAD-SAFE: Structural sharing (O(1) clone)
    btm: Arc<Mutex<PathMap<()>>>,

    // Other indices...
}
```

### Key Operations

**Read Operation** (create thread-local Space):
```rust
pub fn get_type(&self, name: &str) -> Option<MettaValue> {
    // Create thread-local Space (cheap via structural sharing)
    let space = self.create_space();

    // Use Space for MORK operations
    let mut rz = space.btm.read_zipper();
    while rz.to_next_val() {
        // ... pattern matching logic ...
    }

    None
    // Space dropped here (no need to update PathMap)
}
```

**Write Operation** (update PathMap after modification):
```rust
pub fn add_to_space(&mut self, value: &MettaValue) {
    // Create thread-local Space
    let mut space = self.create_space();

    // Modify Space
    let mork_bytes = value.to_mork_string();
    space.load_all_sexpr_impl(mork_bytes.as_bytes(), true)?;

    // Update PathMap with modified version
    self.update_pathmap(space);
}
```

### Helper Methods

Already implemented in `src/backend/environment.rs:77-91`:

```rust
/// Create a thread-local Space for operations
/// Following the Rholang LSP pattern: cheap clone via structural sharing
fn create_space(&self) -> Space {
    let btm = self.btm.lock().unwrap().clone();
    Space {
        btm,
        sm: self.shared_mapping.clone(),
        mmaps: HashMap::new(),
    }
}

/// Update PathMap after Space modifications (write operations)
fn update_pathmap(&mut self, space: Space) {
    *self.btm.lock().unwrap() = space.btm;
}
```

---

## Migration Plan

### Phase 4a: Update Environment Methods (IN PROGRESS)

**File**: `src/backend/environment.rs`

**Read Operations** (~18 methods):
| Method | Line | Status | Notes |
|--------|------|--------|-------|
| `get_type()` | 326 | ⏳ Pending | Replace `self.space.lock()` with `self.create_space()` |
| `iter_rules()` | 352 | ⏳ Pending | Create thread-local Space |
| `match_space()` | 434 | ⏳ Pending | Create thread-local Space |
| `has_fact()` | 544 | ⏳ Pending | Create thread-local Space |
| `has_sexpr_fact_optimized()` | 600 | ⏳ Pending | Create thread-local Space |
| `has_sexpr_fact_linear()` | 645 | ⏳ Pending | Create thread-local Space |
| `metta_to_mork_bytes_cached()` | 675 | ⏳ Pending | Create thread-local Space (line 696) |
| `get_matching_rules()` | 777 | ⏳ Pending | No Space access (index only) |
| ... | ... | ... | ... |

**Write Operations** (~4 methods):
| Method | Line | Status | Notes |
|--------|------|--------|-------|
| `add_to_space()` | 763 | ⏳ Pending | Create Space, modify, update PathMap |
| `add_rule()` | 469 | ⏳ Pending | Calls `add_to_space()` |
| `rebuild_rule_index()` | 391 | ⏳ Pending | Calls `iter_rules()` (read-only) |
| `set_multiplicities()` | 531 | ✅ No change | No Space access |

### Phase 4b: Update Call Sites

**Files to update**:

1. **`src/backend/eval/mod.rs`**
   - Line 587: `env.space.lock()` → `env.create_space()` (read)

2. **`src/backend/mork_convert.rs`**
   - Lines 224, 235, 249, 265: Test functions
   - Replace `env.space.lock()` with `env.create_space()`

3. **`src/pathmap_par_integration.rs`**
   - Line 131: `env.space.lock()` → `env.create_space()` (read)
   - Line 569: `env.space.lock()` → Create Space, modify, update (write)

4. **`src/rholang_integration.rs`**
   - Lines 266, 276: `env.space.lock()` → `env.create_space()` (read)

### Phase 4c: Public API Changes

**Breaking Change**: Remove `pub space` field

```rust
// Before
pub struct Environment {
    pub space: Arc<Mutex<Space>>,  // Public field
}

// After
pub struct Environment {
    shared_mapping: SharedMappingHandle,  // Private
    btm: Arc<Mutex<PathMap<()>>>,        // Private
}
```

**Impact**: External code accessing `env.space` will break

**Solution**: Provide accessor methods if needed:
```rust
impl Environment {
    /// Get a thread-local Space for advanced operations
    pub fn get_space(&self) -> Space {
        self.create_space()
    }
}
```

---

## Implementation Checklist

### Completed ✅
- [x] Update imports (SharedMappingHandle, PathMap)
- [x] Update Environment struct definition
- [x] Update constructor
- [x] Add `create_space()` helper
- [x] Add `update_pathmap()` helper
- [x] Commit partial changes

### In Progress ⏳
- [ ] Update `get_type()` (read operation example)
- [ ] Update `add_to_space()` (write operation example)
- [ ] Update remaining ~50 methods in `environment.rs`
- [ ] Update call sites in `eval/mod.rs`
- [ ] Update call sites in `mork_convert.rs`
- [ ] Update call sites in `pathmap_par_integration.rs`
- [ ] Update call sites in `rholang_integration.rs`

### Testing 🧪
- [ ] Fix all compilation errors
- [ ] Run all 403 unit tests
- [ ] Verify no regressions
- [ ] Test cross-thread Environment usage (Rholang integration)
- [ ] Run baseline benchmarks
- [ ] Compare performance (before/after)

---

## Expected Benefits

### 1. Cross-Thread Usage ✅
```rust
// NOW WORKS: Environment is Send
task::spawn_blocking(move || {
    let env = Environment::new();  // Can move across threads!
    eval(expr, env)
})
```

### 2. No Lock Overhead for Reads
- Old: `Arc<Mutex<Space>>` - locks even for reads
- New: Create thread-local `Space` - no locking during read

### 3. Proven Architecture
- Matches Rholang LSP (production-tested)
- Follows PathMap's intended usage pattern
- Avoids Cell<u64> Sync requirement

### 4. Cleaner Semantics
- Space is explicitly thread-local
- Write operations clearly update shared PathMap
- Read operations don't modify shared state

---

## Performance Considerations

### PathMap Clone Cost

**Claim**: "Cloning is O(1) via structural sharing"

**Reality**: PathMap uses `Arc` internally, so:
```rust
let btm_clone = self.btm.lock().unwrap().clone();
// This is Arc::clone() internally - cheap!
```

**Benchmarks Needed**:
- Measure `create_space()` overhead
- Compare with `Arc<Mutex<Space>>.lock()` overhead
- Expected: Similar or better (no cross-thread lock contention)

### Memory Usage

**Old**:
- 1 Space shared across all threads
- Arc + Mutex overhead

**New**:
- N thread-local Spaces (transient)
- Each Space contains Arc to same PathMap (shared)
- Memory: PathMap is shared, only Space metadata duplicated

**Expected**: Similar memory usage (PathMap is shared via Arc)

---

## Risks and Mitigations

### Risk 1: Breaking Changes
**Impact**: `pub space` field removed
**Mitigation**: Add `get_space()` accessor if needed

### Risk 2: Performance Regression
**Impact**: `create_space()` overhead on every operation
**Mitigation**: Benchmark before/after, PathMap clone is O(1)

### Risk 3: Subtle Bugs
**Impact**: Write operations forgetting to call `update_pathmap()`
**Mitigation**: Careful code review, comprehensive tests

### Risk 4: Increased Complexity
**Impact**: More verbose code (create_space everywhere)
**Mitigation**: Good documentation, helper methods

---

## Testing Strategy

### Unit Tests (Existing)
- All 403 tests must pass
- Focus on:
  - `has_fact()` (recently fixed)
  - `get_type()` (critical path)
  - `add_to_space()` / `add_rule()` (writes)

### Integration Tests
1. **Rholang Integration**
   - Test `spawn_blocking` with Environment
   - Verify parallel evaluation works
   - No deadlocks or race conditions

2. **Concurrent Access**
   - Multiple threads reading simultaneously
   - Mixed read/write workloads
   - Stress test with 100+ threads

### Performance Tests
1. **Baseline Benchmarks** (already established)
   - 19 benchmarks in `benches/prefix_navigation_benchmarks.rs`
   - Run with: `taskset -c 0-17 cargo bench`

2. **New Benchmarks**
   - Measure `create_space()` overhead
   - Concurrent read performance
   - Write operation throughput

---

## Rollback Plan

If refactoring causes issues:

### Option 1: Immediate Revert
```bash
git revert <commit-hash>
cargo test
```

### Option 2: Keep Helper Methods, Revert Struct
- Keep `create_space()` pattern
- Revert to `Arc<Mutex<Space>>` in struct
- Hybrid approach for gradual migration

### Option 3: Document and Defer
- Document findings
- Revert all changes
- Defer full refactoring to future sprint

---

## Timeline Estimate

**Total Effort**: 6-8 hours

| Phase | Effort | Status |
|-------|--------|--------|
| Struct updates | 1 hour | ✅ Complete |
| Update environment.rs methods | 3-4 hours | ⏳ In Progress |
| Update call sites | 1-2 hours | ⏳ Pending |
| Fix compilation errors | 1 hour | ⏳ Pending |
| Testing | 1 hour | ⏳ Pending |

**Current Progress**: ~15% complete (struct + helpers done)

---

## References

### Rholang LSP Documentation
- File: `rholang-language-server/docs/architecture/mork_pathmap_integration.md`
- Section: "Correct Threading Pattern" (lines 37-91)

### MeTTaTron Documentation
- `docs/threading_and_pathmap_integration.md` - Threading model analysis
- `docs/threading_improvements_for_implementation.md` - Phase 3-4 guides
- `docs/phase5_prefix_navigation_analysis.md` - Baseline benchmarks

### Related Commits
- `0c23260` - Threading model documentation
- `6274a45` - Partial struct refactoring (this phase)

---

## Next Steps

1. ✅ **Document refactoring plan** (this file)
2. ⏳ **Update `get_type()` as example read operation**
3. ⏳ **Update `add_to_space()` as example write operation**
4. ⏳ **Systematically update all ~50 methods**
5. ⏳ **Fix compilation errors**
6. ⏳ **Run tests and benchmarks**
7. ⏳ **Git commit with detailed message**

---

**Last Updated**: 2025-11-11
**Status**: Phase 4a in progress (methods being updated)
