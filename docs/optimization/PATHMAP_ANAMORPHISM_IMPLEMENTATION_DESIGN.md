# PathMap Anamorphism Implementation Design

**Date**: 2025-11-12
**Status**: Design Phase
**Target**: Optimize bulk fact/rule insertion using PathMap's `new_from_ana` API

---

## Executive Summary

Design document for implementing PathMap anamorphism-based bulk insertion to replace the current one-by-one insertion loop. This targets the **90% bottleneck** (PathMap insert operations).

**Current Performance**: 1,172 µs for 1000 facts (0.95 µs per PathMap insert + 0.10 µs per MORK conversion)

**Expected Performance**: 314-636 µs for 1000 facts (2-10× speedup via batch construction)

**Risk**: Low (PathMap API designed for this, well-documented with examples)

---

## Current Implementation Analysis

### Current Code (environment.rs:1115-1122)

```rust
pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    let mut fact_trie = self.fact_trie.write().unwrap();
    let temp_space = EvalSpace::new();

    for fact in facts {
        let mut ctx = ConversionContext::new();
        let mork_bytes = metta_to_mork_bytes(fact, &temp_space, &mut ctx)?;
        fact_trie.insert(&mork_bytes, ());  // <-- 90% of time spent here
    }

    Ok(())
}
```

**Performance Breakdown** (per-fact average):
- **PathMap insert**: ~0.95 µs (90% of time) - **TARGET FOR OPTIMIZATION**
- **MORK conversion**: ~0.10 µs (10% of time) - already optimized (Phase 1)

**Problem**: Each `insert()` call traverses the trie from root, even when inserting related paths

**Opportunity**: PathMap's `new_from_ana` can build entire trie in one operation, amortizing tree construction overhead

---

## PathMap Anamorphism API

### API Signature

```rust
pub fn new_from_ana<W, AlgF>(w: W, alg_f: AlgF) -> PathMap<V>
    where
    V: 'static,
    W: Default,
    AlgF: FnMut(W, &mut Option<V>, &mut TrieBuilder<V, W, GlobalAlloc>, &[u8])
```

**Parameters**:
- `w: W` - Seed value (initial state)
- `alg_f: AlgF` - Closure that generates trie structure recursively

**Closure Arguments**:
- `state: W` - Current state (from seed or previous recursion)
- `val: &mut Option<V>` - Value to set at this node (Some if terminal)
- `children: &mut TrieBuilder` - Builder for adding child nodes
- `path: &[u8]` - Current path from root (for debugging/context)

### TrieBuilder Methods

```rust
impl<V, W, A: Allocator> TrieBuilder<V, W, A> {
    // Push single child with byte sequence
    pub fn push(&mut self, path: &[u8], next_state: W);

    // Push single child with one byte
    pub fn push_byte(&mut self, byte: u8, next_state: W);

    // Batch set multiple children using byte mask
    pub fn set_child_mask(&mut self, mask: ByteMask, child_states: Vec<W>);

    // Graft existing PathMap subtrie at byte
    pub fn graft_at_byte(&mut self, byte: u8, zipper: &ReadZipper);
}
```

---

## Implementation Strategies

### Strategy 1: Iterator-Based Construction (Simple)

**Concept**: Build linear chain, one fact at a time via iterator

**Implementation**:

```rust
pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    let temp_space = EvalSpace::new();

    // Pre-convert all facts to MORK bytes
    let mork_facts: Vec<Vec<u8>> = facts
        .iter()
        .map(|fact| {
            let mut ctx = ConversionContext::new();
            metta_to_mork_bytes(fact, &temp_space, &mut ctx)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Build PathMap via anamorphism
    let new_trie = PathMap::new_from_ana(
        mork_facts.into_iter(),
        |mut iter, val, children, _path| {
            if let Some(mork_bytes) = iter.next() {
                // Set value at this path
                *val = Some(());
                // Push remaining facts as child
                children.push(&mork_bytes, iter);
            }
        }
    );

    // Merge with existing fact_trie
    let mut fact_trie = self.fact_trie.write().unwrap();
    *fact_trie = fact_trie.join(&new_trie);

    Ok(())
}
```

**Pros**:
- ✅ Simple to implement (20 lines)
- ✅ Easy to understand
- ✅ Low risk (straightforward logic)

**Cons**:
- ❌ **Creates linear chain** (not optimal trie structure)
- ❌ **Each path fully redundant** (no prefix sharing during construction)
- ❌ **Limited speedup** (1.5-2× at best)

**Expected Speedup**: 1.5-2× (eliminates some overhead, but not optimal)

**Verdict**: **Too simple** - doesn't leverage anamorphism's full power

---

### Strategy 2: Trie-Aware Construction (Optimal)

**Concept**: Group facts by common prefixes, build optimal trie structure

**State Type**:

```rust
#[derive(Clone)]
struct TrieState {
    /// Facts remaining to insert at this level
    facts: Vec<Vec<u8>>,
    /// Current depth in trie (byte offset)
    depth: usize,
}

impl Default for TrieState {
    fn default() -> Self {
        Self {
            facts: Vec::new(),
            depth: 0,
        }
    }
}
```

**Implementation**:

```rust
pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    let temp_space = EvalSpace::new();

    // Pre-convert all facts to MORK bytes
    let mork_facts: Vec<Vec<u8>> = facts
        .iter()
        .map(|fact| {
            let mut ctx = ConversionContext::new();
            metta_to_mork_bytes(fact, &temp_space, &mut ctx)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Build PathMap via trie-aware anamorphism
    let new_trie = PathMap::new_from_ana(
        TrieState {
            facts: mork_facts,
            depth: 0,
        },
        |state, val, children, _path| {
            if state.facts.is_empty() {
                return; // No facts to insert
            }

            // Group facts by next byte at current depth
            let mut groups: HashMap<u8, Vec<Vec<u8>>> = HashMap::new();
            let mut has_terminal = false;

            for fact in state.facts {
                if fact.len() == state.depth {
                    // This fact terminates here
                    has_terminal = true;
                } else if fact.len() > state.depth {
                    // Group by next byte
                    let next_byte = fact[state.depth];
                    groups.entry(next_byte).or_insert_with(Vec::new).push(fact);
                }
            }

            // Set value if any fact terminates here
            if has_terminal {
                *val = Some(());
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

    // Merge with existing fact_trie
    let mut fact_trie = self.fact_trie.write().unwrap();
    *fact_trie = fact_trie.join(&new_trie);

    Ok(())
}
```

**Pros**:
- ✅ **Optimal trie structure** (proper prefix sharing)
- ✅ **Single-pass construction** (no redundant traversals)
- ✅ **Maximum speedup potential** (5-10×)
- ✅ **Correct by design** (follows trie construction algorithm)

**Cons**:
- ❌ More complex implementation (~40 lines vs 20)
- ❌ Requires HashMap for grouping (allocation overhead)
- ❌ State type is more complex

**Expected Speedup**: 5-10× (eliminates redundant tree traversals, builds optimal structure)

**Verdict**: **RECOMMENDED** - maximizes performance benefit

---

### Strategy 3: Hybrid (Batched Groups)

**Concept**: Pre-group facts by first byte, then use Strategy 2 for each group

**Implementation**:

```rust
pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    let temp_space = EvalSpace::new();

    // Pre-convert and group by first byte
    let mut groups: HashMap<u8, Vec<Vec<u8>>> = HashMap::new();

    for fact in facts {
        let mut ctx = ConversionContext::new();
        let mork_bytes = metta_to_mork_bytes(fact, &temp_space, &mut ctx)?;

        if let Some(&first_byte) = mork_bytes.first() {
            groups.entry(first_byte).or_insert_with(Vec::new).push(mork_bytes);
        }
    }

    // Build PathMap for each group, then merge
    let mut fact_trie = self.fact_trie.write().unwrap();

    for (_byte, group_facts) in groups {
        let group_trie = PathMap::new_from_ana(
            TrieState { facts: group_facts, depth: 0 },
            |state, val, children, _path| {
                // Same as Strategy 2
                // ...
            }
        );

        *fact_trie = fact_trie.join(&group_trie);
    }

    Ok(())
}
```

**Pros**:
- ✅ Balances grouping overhead with construction efficiency
- ✅ Can process groups in parallel (if beneficial)

**Cons**:
- ❌ Multiple trie constructions + joins (overhead)
- ❌ More complex than Strategy 2
- ❌ Unclear benefit over Strategy 2

**Expected Speedup**: 3-7× (between Strategy 1 and Strategy 2)

**Verdict**: **Not recommended** - added complexity without clear benefit

---

## Recommended Implementation: Strategy 2

### Detailed Implementation

**File**: `src/backend/environment.rs`

**Changes**:

1. **Add TrieState struct** (before `impl Environment`):

```rust
/// State for PathMap anamorphism-based bulk construction
#[derive(Clone)]
struct TrieState {
    /// Facts remaining to insert at this level
    facts: Vec<Vec<u8>>,
    /// Current depth in trie (byte offset)
    depth: usize,
}

impl Default for TrieState {
    fn default() -> Self {
        Self {
            facts: Vec::new(),
            depth: 0,
        }
    }
}
```

2. **Replace `add_facts_bulk` implementation** (environment.rs:1115-1122):

```rust
pub fn add_facts_bulk(&mut self, facts: &[MettaValue]) -> Result<(), String> {
    use std::collections::HashMap;

    if facts.is_empty() {
        return Ok(());
    }

    let temp_space = EvalSpace::new();

    // Pre-convert all facts to MORK bytes
    // Time: ~100 µs for 1000 facts (10% of current total time)
    let mork_facts: Vec<Vec<u8>> = facts
        .iter()
        .map(|fact| {
            let mut ctx = ConversionContext::new();
            metta_to_mork_bytes(fact, &temp_space, &mut ctx)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Build PathMap via trie-aware anamorphism
    // Time: ~200 µs for 1000 facts (expected, vs ~950 µs current)
    let new_trie = PathMap::new_from_ana(
        TrieState {
            facts: mork_facts,
            depth: 0,
        },
        |state, val, children, _path| {
            if state.facts.is_empty() {
                return; // No facts at this level
            }

            // Group facts by next byte at current depth
            let mut groups: HashMap<u8, Vec<Vec<u8>>> = HashMap::new();
            let mut has_terminal = false;

            for fact in state.facts {
                if fact.len() == state.depth {
                    // This fact terminates at this node
                    has_terminal = true;
                } else if fact.len() > state.depth {
                    // Group by next byte
                    let next_byte = fact[state.depth];
                    groups.entry(next_byte).or_insert_with(Vec::new).push(fact);
                }
            }

            // Set value if any fact terminates here
            if has_terminal {
                *val = Some(());
            }

            // Create children for each byte group
            for (byte, group_facts) in groups {
                let next_state = TrieState {
                    facts: group_facts,
                    depth: state.depth + 1,
                };
                children.push_byte(byte, next_state);
            }
        }
    );

    // Merge with existing fact_trie using PathMap's join operation
    // Time: ~50 µs (amortized via structural sharing)
    let mut fact_trie = self.fact_trie.write().unwrap();
    *fact_trie = fact_trie.join(&new_trie);

    Ok(())
}
```

3. **Apply same pattern to `add_rules_bulk`** (environment.rs:708-724):

```rust
pub fn add_rules_bulk(&mut self, rules: &[Rule]) -> Result<(), String> {
    use std::collections::HashMap;

    if rules.is_empty() {
        return Ok(());
    }

    let temp_space = EvalSpace::new();

    // Pre-convert all rules to MORK bytes
    let mork_rules: Vec<Vec<u8>> = rules
        .iter()
        .map(|rule| {
            let mut ctx = ConversionContext::new();
            let rule_bytes = format!("{:?}", rule);  // Simplified for design doc
            metta_to_mork_bytes(&MettaValue::Atom(rule_bytes), &temp_space, &mut ctx)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Build PathMap via trie-aware anamorphism
    let new_trie = PathMap::new_from_ana(
        TrieState {
            facts: mork_rules,  // Reuse same state type
            depth: 0,
        },
        |state, val, children, _path| {
            // Same logic as add_facts_bulk
            // ...
        }
    );

    // Merge with existing rule_trie
    let mut rule_trie = self.rule_trie.write().unwrap();
    *rule_trie = rule_trie.join(&new_trie);

    Ok(())
}
```

---

## Performance Analysis

### Expected Time Breakdown (1000 facts)

| Operation | Current Time | Optimized Time | Speedup |
|-----------|--------------|----------------|---------|
| MORK conversion (1000×) | 100 µs | 100 µs | 1.0× (unchanged) |
| PathMap construction | 950 µs | 150-250 µs | **3.8-6.3×** |
| PathMap join | 0 µs | 50 µs | New overhead |
| **Total** | **1,050 µs** | **300-400 µs** | **2.6-3.5×** |

**Conservative Estimate**: **2.6× speedup** (400 µs vs 1,050 µs)

**Optimistic Estimate**: **3.5× speedup** (300 µs vs 1,050 µs)

**Realistic Estimate**: **3.0× speedup** (350 µs vs 1,050 µs)

### Why PathMap Construction is Faster

**Current Approach** (one-by-one insertion):
- Each `insert()` traverses from root to insertion point
- 1000 facts × average depth 20 = ~20,000 tree traversals
- Redundant navigation for shared prefixes
- Time: O(n × d) where n = facts, d = average depth

**Anamorphism Approach** (batch construction):
- Single recursive construction from root
- Each byte position visited once
- Optimal trie structure built directly
- Time: O(Σ|fact|) - proportional to total bytes, not facts × depth

**Example**:
```
Facts: ["abc", "abd", "xyz"]

Current (3 inserts):
  insert("abc") → traverse [], a, ab, abc (4 steps)
  insert("abd") → traverse [], a, ab, abd (4 steps) - REDUNDANT [, a, ab]
  insert("xyz") → traverse [], x, xy, xyz (4 steps) - REDUNDANT []
  Total: 12 steps

Anamorphism (1 construction):
  Build from root:
    - At []: children {a, x}
    - At [a]: children {b}
    - At [ab]: children {c, d}
    - At [x]: children {y}
    - At [xy]: children {z}
  Total: 5 nodes visited (vs 12 traversals)
```

**Speedup Factor**: ~(n × d) / Σ|fact| ≈ (1000 × 20) / 50,000 ≈ 0.4
**Inverse (slowdown if not using)**: ~2.5× → **Using anamorphism = 2.5× faster**

---

## Edge Cases and Error Handling

### 1. Empty Facts List

```rust
if facts.is_empty() {
    return Ok(());  // Early return, no trie construction
}
```

### 2. Duplicate Facts

**Behavior**: PathMap naturally handles duplicates via trie structure
- Multiple facts with same MORK bytes → single path in trie
- Value set to Some(()) at terminal node (idempotent)

**No special handling required**

### 3. Very Large Bulk Operations (10,000+ facts)

**Memory Consideration**:
- Pre-converting all facts: 10,000 × 50 bytes = 500 KB (acceptable)
- HashMap grouping: 256 max groups × overhead = ~10 KB per level (acceptable)
- PathMap construction: Structural sharing minimizes memory

**No memory issues expected**

### 4. MORK Conversion Failures

**Current Handling**: Early return on first error (collect with `?`)

```rust
let mork_facts: Vec<Vec<u8>> = facts
    .iter()
    .map(|fact| metta_to_mork_bytes(fact, &temp_space, &mut ctx))
    .collect::<Result<Vec<_>, _>>()?;  // Returns Err on first failure
```

**Behavior**: All-or-nothing (either all facts inserted or none)

**Alternative** (if partial insertion desired):
```rust
let mork_facts: Vec<Vec<u8>> = facts
    .iter()
    .filter_map(|fact| {
        let mut ctx = ConversionContext::new();
        metta_to_mork_bytes(fact, &temp_space, &mut ctx).ok()  // Skip failures
    })
    .collect();
```

**Recommendation**: Keep all-or-nothing (current behavior) for atomicity

### 5. Thread Safety (RwLock)

**Current**: `fact_trie: Arc<RwLock<PathMap<()>>>`

**Anamorphism Implementation**:
```rust
let mut fact_trie = self.fact_trie.write().unwrap();  // Lock for write
*fact_trie = fact_trie.join(&new_trie);  // Atomic update
// Lock released on drop
```

**Safety**: ✅ Correct (write lock held during join)

---

## Testing Strategy

### Unit Tests

**Test 1: Empty Facts**
```rust
#[test]
fn test_add_facts_bulk_empty() {
    let mut env = Environment::new();
    assert!(env.add_facts_bulk(&[]).is_ok());
    // Verify fact_trie unchanged
}
```

**Test 2: Single Fact**
```rust
#[test]
fn test_add_facts_bulk_single() {
    let mut env = Environment::new();
    let fact = MettaValue::Atom("test".to_string());
    assert!(env.add_facts_bulk(&[fact.clone()]).is_ok());
    assert!(env.has_fact(&fact).unwrap());
}
```

**Test 3: Multiple Facts with Common Prefix**
```rust
#[test]
fn test_add_facts_bulk_shared_prefix() {
    let mut env = Environment::new();
    let facts = vec![
        MettaValue::Atom("test1".to_string()),
        MettaValue::Atom("test2".to_string()),
        MettaValue::Atom("test3".to_string()),
    ];
    assert!(env.add_facts_bulk(&facts).is_ok());
    for fact in &facts {
        assert!(env.has_fact(fact).unwrap());
    }
}
```

**Test 4: Duplicates**
```rust
#[test]
fn test_add_facts_bulk_duplicates() {
    let mut env = Environment::new();
    let fact = MettaValue::Atom("duplicate".to_string());
    let facts = vec![fact.clone(), fact.clone(), fact.clone()];
    assert!(env.add_facts_bulk(&facts).is_ok());
    assert!(env.has_fact(&fact).unwrap());
}
```

**Test 5: Large Bulk (1000 facts)**
```rust
#[test]
fn test_add_facts_bulk_large() {
    let mut env = Environment::new();
    let facts: Vec<_> = (0..1000)
        .map(|i| MettaValue::Atom(format!("fact-{}", i)))
        .collect();
    assert!(env.add_facts_bulk(&facts).is_ok());
    for fact in &facts {
        assert!(env.has_fact(fact).unwrap());
    }
}
```

### Integration Tests

**Test 6: Merging with Existing Facts**
```rust
#[test]
fn test_add_facts_bulk_merge() {
    let mut env = Environment::new();

    // Add initial facts individually
    env.add_fact(MettaValue::Atom("existing1".to_string())).unwrap();
    env.add_fact(MettaValue::Atom("existing2".to_string())).unwrap();

    // Add bulk facts
    let new_facts = vec![
        MettaValue::Atom("new1".to_string()),
        MettaValue::Atom("new2".to_string()),
    ];
    env.add_facts_bulk(&new_facts).unwrap();

    // Verify all facts present
    assert!(env.has_fact(&MettaValue::Atom("existing1".to_string())).unwrap());
    assert!(env.has_fact(&MettaValue::Atom("existing2".to_string())).unwrap());
    assert!(env.has_fact(&MettaValue::Atom("new1".to_string())).unwrap());
    assert!(env.has_fact(&MettaValue::Atom("new2".to_string())).unwrap());
}
```

### Benchmark Tests

**Benchmark: Anamorphism vs One-by-One**
```rust
fn bench_anamorphism_construction(c: &mut Criterion) {
    let facts: Vec<_> = (0..1000)
        .map(|i| MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(i),
            MettaValue::Atom(format!("value-{}", i)),
        ]))
        .collect();

    c.bench_function("add_facts_bulk_anamorphism", |b| {
        b.iter(|| {
            let mut env = Environment::new();
            env.add_facts_bulk(black_box(&facts)).unwrap();
        });
    });
}
```

**Expected Result**: 300-400 µs (vs 1,050 µs baseline from Phase 1+2)

---

## Implementation Checklist

### Phase 1: Core Implementation

- [ ] Add `TrieState` struct before `impl Environment`
- [ ] Implement `Default` for `TrieState`
- [ ] Replace `add_facts_bulk` with anamorphism-based implementation
- [ ] Replace `add_rules_bulk` with anamorphism-based implementation
- [ ] Add `use std::collections::HashMap` import

### Phase 2: Testing

- [ ] Add unit test: empty facts
- [ ] Add unit test: single fact
- [ ] Add unit test: multiple facts with shared prefix
- [ ] Add unit test: duplicates
- [ ] Add unit test: large bulk (1000 facts)
- [ ] Add integration test: merge with existing facts
- [ ] Run full test suite (403 tests must pass)

### Phase 3: Benchmarking

- [ ] Create benchmark: anamorphism construction (1000 facts)
- [ ] Run benchmark with CPU affinity (`taskset -c 0-17`)
- [ ] Compare against Phase 1+2 baseline (1,050 µs)
- [ ] Measure memory usage (before/after)
- [ ] Document results in CHANGELOG.md

### Phase 4: Documentation

- [ ] Update CHANGELOG.md with Phase 5 entry
- [ ] Create completion document (PHASE_5_PATHMAP_ANAMORPHISM_COMPLETE.md)
- [ ] Update OPTIMIZATION_PHASES_SUMMARY.md
- [ ] Update code comments with performance characteristics

---

## Risk Mitigation

### Risk 1: Anamorphism Slower Than Expected

**Mitigation**: Benchmark early, compare against baseline

**Fallback**: Revert to Phase 1+2 implementation (already optimized)

### Risk 2: HashMap Grouping Overhead

**Mitigation**: Use `Vec::with_capacity()` to preallocate groups

**Alternative**: Use array-based grouping for first byte (256 groups max)

### Risk 3: join() Operation Slow

**Mitigation**: Benchmark `join()` separately

**Alternative**: Use `fact_trie.insert_all()` if available, or build single large trie upfront

### Risk 4: Test Failures

**Mitigation**: Extensive unit testing before integration

**Rollback Plan**: Git branch for development, easy revert if needed

---

## Success Criteria

**Minimum Success**:
- ✅ All 403 tests pass
- ✅ 2× speedup on bulk operations (vs Phase 1+2 baseline)
- ✅ No memory regressions
- ✅ Correctness validated (all facts/rules retrievable)

**Target Success**:
- ✅ 3× speedup on bulk operations (350 µs vs 1,050 µs)
- ✅ Optimal trie structure (verified via PathMap inspection)
- ✅ Clean implementation (<50 lines)
- ✅ Well-documented with examples

**Ideal Success**:
- ✅ 5× speedup on bulk operations (200 µs vs 1,050 µs)
- ✅ Scales well to 10,000+ facts
- ✅ Memory efficient (structural sharing verified)
- ✅ Production-ready code quality

---

## Next Steps

1. **Implement Strategy 2** in `src/backend/environment.rs`
2. **Add unit tests** for edge cases
3. **Run benchmark** comparing anamorphism vs baseline
4. **Analyze results** and tune if necessary
5. **Document findings** in completion document
6. **Commit changes** with comprehensive commit message

---

## Related Documents

- `docs/optimization/PATHMAP_BATCH_API_FINDINGS.md` - API research
- `docs/optimization/OPTIMIZATION_PHASES_SUMMARY.md` - Phase 1-4 context
- `docs/optimization/PATHMAP_OPTIMIZATION_RESEARCH.md` - PathMap analysis
- PathMap source: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/trie_map.rs`
- PathMap morphisms: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/morphisms.rs`

---

**End of PathMap Anamorphism Implementation Design**
