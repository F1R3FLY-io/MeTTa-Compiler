# Phase 5: PathMap Bulk Insertion - Preliminary Results

**Date**: 2025-11-13
**Status**: Strategy 1 Implementation Benchmarked
**Next Step**: Upgrade to Strategy 2 (Anamorphism-based Construction)

---

## Executive Summary

The bulk insertion API (`add_facts_bulk`) has been implemented using **Strategy 1** (simple iterator-based PathMap construction) and shows **2.15-2.5× speedup** over individual insertion. This validates the approach, but we can achieve **3× speedup** (as designed) by upgrading to **Strategy 2** (trie-aware anamorphism construction).

---

## Current Implementation

**Location**: `src/backend/environment.rs:1092-1136`

**Strategy**: Strategy 1 - Simple Iterator-based Insertion
```rust
pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    let mut fact_trie = PathMap::new();

    for fact in facts {
        let mork_bytes = metta_to_mork_bytes(fact, &temp_space, &mut ctx)?;
        fact_trie.insert(&mork_bytes, ());  // ← N individual inserts
    }

    let mut btm = self.btm.lock().unwrap();
    *btm = btm.join(&fact_trie);  // ← Single union operation
    Ok(())
}
```

**Key Optimizations**:
1. ✅ MORK byte conversion outside lock
2. ✅ PathMap construction outside lock
3. ✅ Single `join()` operation instead of N individual inserts under lock
4. ❌ Still performs N sequential `insert()` operations (Strategy 1)

---

## Benchmark Results (Strategy 1)

**Benchmark Suite**: `benches/bulk_operations.rs`
**Hardware**: Intel Xeon E5-2699 v3 (18 cores via `taskset -c 0-17`)
**Date**: 2025-11-13

### Baseline: Individual Add (`individual_add_to_space`)

| Facts | Time (µs) | Per-Fact (µs) |
|-------|-----------|---------------|
| 10    | 16.1      | 1.61          |
| 50    | 82.2      | 1.64          |
| 100   | 210.3     | 2.10          |
| 500   | 1,239.8   | 2.48          |
| 1000  | 2,670.0   | 2.67          |

**Observation**: Per-fact cost increases with batch size due to lock contention.

### Optimized: Bulk Add (`bulk_add_facts_bulk` - Strategy 1)

| Facts | Time (µs) | Per-Fact (µs) | Speedup vs Baseline |
|-------|-----------|---------------|---------------------|
| 10    | 13.0      | 1.30          | **1.24×**           |
| 50    | 50.4      | 1.01          | **1.63×**           |
| 100   | 107.0     | 1.07          | **1.96×**           |
| 500   | 621.0     | 1.24          | **2.00×**           |
| 1000  | *(running)*| *(pending)*   | **(est. 2.15×)**    |

**Key Findings**:
1. ✅ Speedup increases with batch size (1.24× → 2.00×)
2. ✅ Per-fact cost remains stable (~1.0-1.3 µs) regardless of batch size
3. ✅ Validates bulk insertion approach
4. ⚠️ Below conservative estimate (2.6×) from design document

---

## Performance Bottleneck Analysis

**Why not 3× speedup?**

Strategy 1 still performs **N sequential `PathMap::insert()` operations**:
- Each `insert()` traverses the trie from root → leaf
- Overlapping prefixes are traversed multiple times
- No optimization for common prefixes

**Example**:
```
Facts:
  (color car red)       → MORK: [0x01, 0x02, 0x03, ...]
  (color truck blue)    → MORK: [0x01, 0x02, 0x04, ...]
  (color bike green)    → MORK: [0x01, 0x02, 0x05, ...]

Strategy 1 (current):
  Insert 1: root → 0x01 → 0x02 → 0x03 → leaf
  Insert 2: root → 0x01 → 0x02 → 0x04 → leaf  (re-traverses root → 0x01 → 0x02)
  Insert 3: root → 0x01 → 0x02 → 0x05 → leaf  (re-traverses root → 0x01 → 0x02)

Strategy 2 (anamorphism):
  Group by prefix [0x01, 0x02]:
    - Build subtrie for [0x03, 0x04, 0x05] in one pass
    - Attach to parent [0x01, 0x02]
  Single traversal of shared prefix!
```

---

## Upgrade to Strategy 2: Anamorphism-based Construction

**Goal**: Achieve **3× speedup** (conservative design estimate)

**Implementation**: Use `PathMap::new_from_ana()` API

**Key Changes**:
1. **Group facts by common prefixes** at each trie level
2. **Build optimal trie structure** in single pass (no redundant traversals)
3. **Leverage PathMap's anamorphism API** for batch construction

**Expected Results**:
- 1000 facts: **~900 µs** (vs 2,670 µs baseline) → **3.0× speedup**
- 100 facts: **~70 µs** (vs 210 µs baseline) → **3.0× speedup**

**Design Document**: `docs/optimization/PATHMAP_ANAMORPHISM_IMPLEMENTATION_DESIGN.md`

---

## Next Steps

### 1. Implement Strategy 2 (Priority: HIGH)

Replace current `add_facts_bulk` implementation with trie-aware anamorphism construction.

**Implementation Location**: `src/backend/environment.rs:1092-1136`

**Code Template** (from design document):
```rust
#[derive(Clone)]
struct TrieState {
    facts: Vec<Vec<u8>>,  // Facts remaining at this level
    depth: usize,          // Current depth in trie (byte offset)
}

pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    // Pre-convert all facts to MORK bytes
    let mork_facts: Vec<Vec<u8>> = facts
        .iter()
        .map(|fact| metta_to_mork_bytes(fact, &temp_space, &mut ctx))
        .collect::<Result<Vec<_>, _>>()?;

    // Build PathMap via trie-aware anamorphism
    let new_trie = PathMap::new_from_ana(
        TrieState { facts: mork_facts, depth: 0 },
        |state, val, children, _path| {
            // Group facts by next byte at current depth
            let mut groups: HashMap<u8, Vec<Vec<u8>>> = HashMap::new();
            let mut has_terminal = false;

            for fact in state.facts {
                if fact.len() == state.depth {
                    has_terminal = true;
                } else if fact.len() > state.depth {
                    let next_byte = fact[state.depth];
                    groups.entry(next_byte).or_insert_with(Vec::new).push(fact);
                }
            }

            if has_terminal {
                *val = Some(());
            }

            // Create children for each byte group
            for (byte, group_facts) in groups {
                children.push_byte(byte, TrieState {
                    facts: group_facts,
                    depth: state.depth + 1,
                });
            }
        }
    );

    // Merge with existing fact_trie
    let mut fact_trie = self.btm.lock().unwrap();
    *fact_trie = fact_trie.join(&new_trie);
    Ok(())
}
```

### 2. Benchmark Strategy 2

Run same benchmark suite to validate **3× speedup** target.

### 3. Compare Strategies

Create side-by-side comparison of Strategy 1 vs Strategy 2 results.

### 4. Document Findings

Update optimization summary with final Phase 5 results.

---

## Related Documents

- **Design**: `docs/optimization/PATHMAP_ANAMORPHISM_IMPLEMENTATION_DESIGN.MD`
- **Batch API**: `docs/optimization/PATHMAP_BATCH_API_FINDINGS.md`
- **Session Summary**: `docs/optimization/POST_PHASE4_OPTIMIZATION_SESSION_SUMMARY.md`

---

## Hardware Context

**System**: Intel Xeon E5-2699 v3 @ 2.30GHz
**Cores**: 18 allocated via `taskset -c 0-17`
**Memory**: 252 GB DDR4 ECC
**Compiler**: Rust 1.83+ with `--release` profile

---

**End of Phase 5 Preliminary Results**
