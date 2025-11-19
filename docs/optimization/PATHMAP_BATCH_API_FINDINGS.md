# PathMap Batch API Discovery

**Date**: 2025-11-12
**Status**: Discovery Complete - Implementation Pending

---

## Executive Summary

**Good news!** PathMap DOES have a batch construction API that we're not currently using. The `new_from_ana` method provides **anamorphism-based batch construction** that could significantly optimize our bulk fact/rule insertion operations.

**Current Status**: We insert MORK bytes one-by-one in a loop (environment.rs:1115-1122), but PathMap provides `new_from_ana` for batch construction from a collection.

**Recommendation**: Refactor bulk insertion to use `new_from_ana` for potential 2-10× speedup.

---

## PathMap Batch API: `new_from_ana`

### API Signature

```rust
pub fn new_from_ana<W, AlgF>(w: W, alg_f: AlgF) -> PathMap<V>
    where
    V: 'static,
    W: Default,
    AlgF: FnMut(W, &mut Option<V>, &mut TrieBuilder<V, W, GlobalAlloc>, &[u8])
```

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/trie_map.rs:116-123`

### What is Anamorphism?

**Anamorphism** is the functional programming concept for "building up" a structure from a seed value, as opposed to **catamorphism** which "tears down" a structure via folding.

**In PathMap context**:
- **Input**: A seed value `W` and a closure `alg_f`
- **Process**: Closure generates trie structure recursively from root
- **Output**: Fully constructed PathMap in one operation

**Contrast with Current Approach**:
```rust
// CURRENT (one-by-one insertion):
let mut trie = PathMap::new();
for item in items {
    trie.insert(&item, ());  // 90% of time spent here
}

// ANAMORPHISM (batch construction):
let trie = PathMap::new_from_ana(items, |state, val, children, path| {
    // Generate trie structure in one pass
});
```

---

## Example Usage from PathMap Source

### Example 1: Binary Tree (Simple)

From `trie_map.rs:107-114`:

```rust
// Creates a binary tree 3 levels deep with 'L' and 'R' branches
let map = PathMap::<()>::new_from_ana(3, |idx, val, children, _path| {
    if idx > 0 {
        children.push(b"L", idx - 1);  // Push left child
        children.push(b"R", idx - 1);  // Push right child
    } else {
        *val = Some(());  // Set value at leaf
    }
});
```

**How it works**:
1. Start with seed value `3`
2. Closure generates two children with seed values `2`
3. Recursively build until seed value reaches `0`
4. Set values at leaves

### Example 2: Linear Chain

From `morphisms.rs:1959-1966`:

```rust
// Generate 5 'i's in a row: "iiiii"
let map: PathMap<()> = PathMap::<()>::new_from_ana(5, |idx, val, children, _path| {
    *val = Some(());  // Set value at every step
    if idx > 0 {
        children.push_byte(b'i', idx - 1);  // Continue chain
    }
});
```

**Result**: PathMap with values at "", "i", "ii", "iii", "iiii", "iiiii"

### Example 3: Multiple Children (Advanced)

From `morphisms.rs:1989-1999`:

```rust
let map: PathMap<()> = PathMap::<()>::new_from_ana(
    ([0u64; 4], 0),  // Seed: (ByteMask, index)
    |(mut mask, idx), val, children, _path| {
        if idx < 5 {
            mask[1] |= 1u64 << 1+idx;  // Set bits in mask
            let child_vec = vec![(mask, idx+1); idx+1];  // Generate children
            children.set_child_mask(mask, child_vec);  // Batch set children
        }
    }
);
```

**Advanced feature**: `set_child_mask` allows setting multiple children at once

---

## TrieBuilder API

The `children` parameter in the closure provides a `TrieBuilder` with these methods:

### 1. `push(path: &[u8], next_state: W)`
Push a single child with a byte sequence and next state

```rust
children.push(b"fact", next_state);
```

### 2. `push_byte(byte: u8, next_state: W)`
Push a single child with one byte

```rust
children.push_byte(b'A', next_state);
```

### 3. `set_child_mask(mask: ByteMask, child_states: Vec<W>)`
Batch set multiple children at once using a byte mask

```rust
let mask = [0b00000110, 0, 0, 0];  // Bytes 1 and 2
children.set_child_mask(mask, vec![state1, state2]);
```

### 4. `graft_at_byte(byte: u8, zipper: &ReadZipper)`
Graft an existing PathMap subtrie at a specific byte

```rust
children.graft_at_byte(b'X', &existing_trie.read_zipper());
```

---

## Application to MeTTa Bulk Operations

### Current Implementation

**File**: `src/backend/environment.rs:1115-1122`

```rust
// Phase 1+2 optimized code
for fact in facts {
    let mut ctx = ConversionContext::new();
    let mork_bytes = metta_to_mork_bytes(fact, &temp_space, &mut ctx)?;
    fact_trie.insert(&mork_bytes, ());  // PathMap insert (90% of time)
}
```

**Performance**: ~0.95 µs per fact (PathMap insert dominates)

### Proposed Anamorphism-Based Implementation

**Strategy 1: Iterator-Based (Simplest)**

```rust
// Pre-convert all facts to MORK bytes
let mork_facts: Vec<Vec<u8>> = facts
    .iter()
    .map(|fact| {
        let mut ctx = ConversionContext::new();
        metta_to_mork_bytes(fact, &temp_space, &mut ctx).unwrap()
    })
    .collect();

// Build PathMap in one operation via anamorphism
let fact_trie = PathMap::new_from_ana(
    mork_facts.into_iter(),  // Seed: iterator of MORK bytes
    |mut iter, val, children, _path| {
        if let Some(mork_bytes) = iter.next() {
            // Set value at this path
            *val = Some(());

            // Push child for next fact
            children.push(&mork_bytes, iter);
        }
    }
);
```

**Challenge**: This creates a linear chain, not a proper trie structure

### Strategy 2: Trie-Aware Construction

```rust
// Group facts by common prefixes for optimal trie structure
struct TrieState {
    facts: Vec<Vec<u8>>,
    depth: usize,
}

let fact_trie = PathMap::new_from_ana(
    TrieState { facts: mork_facts, depth: 0 },
    |state, val, children, _path| {
        if state.facts.is_empty() {
            return;
        }

        // Group by next byte at current depth
        let mut groups: HashMap<u8, Vec<Vec<u8>>> = HashMap::new();
        for fact in state.facts {
            if fact.len() == state.depth {
                // Reached end of this fact - set value
                *val = Some(());
            } else if fact.len() > state.depth {
                // Group by next byte
                let next_byte = fact[state.depth];
                groups.entry(next_byte).or_insert_with(Vec::new).push(fact);
            }
        }

        // Create children for each group
        for (byte, group_facts) in groups {
            let next_state = TrieState {
                facts: group_facts,
                depth: state.depth + 1,
            };
            children.push_byte(byte, next_state);
        }
    }
);
```

**Benefits**:
- Builds optimal trie structure (no redundant paths)
- Single pass construction
- Eliminates repeated tree traversals

**Estimated Speedup**: 2-10× (eliminates 90% bottleneck overhead)

---

## Challenges and Considerations

### 1. Complexity vs Performance Trade-off

**Simple approach** (Strategy 1):
- Easy to implement
- May not build optimal trie structure
- Limited performance gain

**Trie-aware approach** (Strategy 2):
- More complex implementation
- Optimal trie structure
- Maximum performance gain

### 2. Memory Overhead

Pre-converting all facts to MORK bytes requires:
- Temporary storage for all MORK byte vectors
- Memory proportional to number of facts

**For 1000 facts**:
- Average MORK bytes: ~50 bytes per fact
- Memory: 1000 × 50 = 50 KB (negligible)

### 3. Error Handling

Current implementation handles errors incrementally:
```rust
for fact in facts {
    let mork_bytes = metta_to_mork_bytes(fact, ...)?;  // Early return on error
    fact_trie.insert(&mork_bytes, ());
}
```

Anamorphism approach requires all facts converted before construction:
- All-or-nothing conversion
- Need to collect errors during conversion phase

---

## Alternative: Use `from_iter` if Available

Let me check if PathMap has a simpler `from_iter` API:

**Search Result**: No direct `from_iter` implementation in PathMap for key-value pairs

**Conclusion**: `new_from_ana` is the intended batch construction API

---

## Comparison with Current Performance

### Current Performance (Phase 1+2)

**100 facts**: 95.84 µs total
- MORK conversion: ~10 µs (10%)
- PathMap insert: ~85 µs (90%)

**1000 facts**: 1,172.30 µs total
- MORK conversion: ~100 µs (10%)
- PathMap insert: ~1,072 µs (90%)

### Expected Performance with Anamorphism

**Optimistic Scenario** (2× speedup on PathMap operations):
- 100 facts: 95.84 µs → 52.5 µs (1.83× faster)
- 1000 facts: 1,172.30 µs → 636 µs (1.84× faster)

**Realistic Scenario** (5× speedup on PathMap operations):
- 100 facts: 95.84 µs → 27 µs (3.55× faster)
- 1000 facts: 1,172.30 µs → 314 µs (3.73× faster)

**Best Case** (10× speedup on PathMap operations):
- 100 facts: 95.84 µs → 18.5 µs (5.18× faster)
- 1000 facts: 1,172.30 µs → 207 µs (5.66× faster)

---

## Recommendation

### Phase 5: PathMap Anamorphism-Based Bulk Insertion

**Goal**: Refactor bulk insertion to use `new_from_ana` for batch construction

**Steps**:
1. **Design Phase**: Choose between Strategy 1 (simple) vs Strategy 2 (optimal)
2. **Prototype**: Implement chosen strategy
3. **Benchmark**: Compare against Phase 1+2 baseline
4. **Measure**: Quantify actual speedup
5. **Test**: Ensure all 403 tests pass
6. **Document**: Record results and lessons learned

**Expected Impact**: 2-10× speedup on bulk operations (targets 90% of time)

**Risk Assessment**: Medium
- API is well-documented with examples
- Requires careful state management
- May need iteration on design

**Priority**: HIGH - Targets the dominant bottleneck (90% of time)

---

## Next Steps

1. ✅ **Document PathMap batch API** (this document)
2. ⏭️ **Design anamorphism-based implementation** (Strategy 1 vs Strategy 2)
3. 🔜 **Prototype and benchmark**
4. 🔜 **Expression parallelism threshold tuning** (per user request)
5. 🔜 **Explore liblevenshtein integration**

---

## Related Documents

- `docs/optimization/PATHMAP_OPTIMIZATION_RESEARCH.md` - Initial PathMap research
- `docs/optimization/OPTIMIZATION_PHASES_SUMMARY.md` - Phase 1-4 summary
- `docs/optimization/PHASE_1_MORK_DIRECT_CONVERSION_COMPLETE.md` - Current baseline
- PathMap source: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/trie_map.rs`
- PathMap morphisms: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/morphisms.rs`

---

**End of PathMap Batch API Findings**
