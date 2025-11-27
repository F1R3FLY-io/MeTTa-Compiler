# MORK Algebraic Operations: Comprehensive Guide

**Version**: 1.0
**Last Updated**: 2025-11-13
**Author**: MORK Documentation Team

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Introduction](#introduction)
3. [Core Algebraic Operations](#core-algebraic-operations)
   - [Join (Union)](#join-union)
   - [Meet (Intersection)](#meet-intersection)
   - [Subtract (Set Difference)](#subtract-set-difference)
   - [Restrict](#restrict)
   - [Graft](#graft)
4. [Auxiliary Operations](#auxiliary-operations)
5. [Algebraic Structures](#algebraic-structures)
6. [Best Practices](#best-practices)
7. [Edge Cases and Caveats](#edge-cases-and-caveats)
8. [Integration with MORK](#integration-with-mork)
9. [References](#references)

---

## Executive Summary

MORK (MeTTa Optimal Reduction Kernel) leverages the PathMap library's algebraic operations to perform efficient hypergraph transformations. These operations provide set-theoretic primitives at the trie level with sophisticated structural sharing and prefix compression, achieving performance characteristics unattainable with naive implementations.

**Key Insights**:
- Operations are performed at the **trie structure level**, not just on values
- **Structural sharing** via reference counting enables O(1) copying and efficient memory usage
- **AlgebraicStatus return values** communicate operation outcomes for optimization
- **Batching operations** is critical for maintaining structural sharing benefits
- **Lattice trait hierarchy** provides clean abstraction for value-level semantics

**Complexity Overview**:

| Operation | Time Complexity | Space Complexity | Typical Use Case |
|-----------|----------------|------------------|------------------|
| join_into | O(min(\|A\|,\|B\|) log k) | O(\|result\|) | Accumulation, union |
| meet_into | O(min(\|A\|,\|B\|) log k) | O(\|intersection\|) | Filtering, intersection |
| subtract_into | O(\|self\| log k) | O(\|difference\|) | Pattern removal |
| restrict | O(\|self\| log k) | O(\|restricted\|) | Prefix-based filtering |
| graft | O(1) | O(1) | Subtrie replacement |

Where:
- |A|, |B| = number of nodes in input tries
- k = maximum branching factor (typically 256 for byte keys)
- All space complexities benefit from structural sharing

---

## Introduction

### What Are Algebraic Operations?

In the context of MORK and PathMap, algebraic operations are structure-level transformations on prefix-compressed tries that preserve or combine the paths stored within them. Unlike traditional set operations that work on collections of elements, these operations:

1. **Operate on tree structures** - The trie structure itself is the algebraic domain
2. **Preserve structural sharing** - Common subtries are shared via reference counting
3. **Combine path and value semantics** - Both structure and values participate in operations
4. **Return status information** - Operations report whether results are new, identical, or empty

### Why PathMap-Based Algebraic Operations?

Traditional hypergraph operations suffer from:
- **Memory overhead** - Explicit storage of all paths
- **Poor locality** - Scattered data structures
- **Redundant structure** - Repeated path prefixes
- **Expensive copying** - Deep clones required for most operations

PathMap's approach solves these problems:
- **Prefix compression** - Common prefixes stored once
- **Structural sharing** - Reference-counted nodes enable cheap clones
- **Cache-friendly** - Depth-first layout improves locality
- **Lazy evaluation** - Some operations defer structural changes

**Example**: Consider storing 256 four-byte paths:
- Naive approach: 256 × 4 = 1,024 bytes
- PathMap with perfect sharing: 16 bytes (64× reduction)
- Real-world datasets often achieve 100-1000× compression

### Mathematical Foundation

PathMap operations implement algebraic structures from lattice theory:

**Lattice** - A partially ordered set where every pair of elements has:
- **Join (∨)**: Least upper bound (supremum)
- **Meet (∧)**: Greatest lower bound (infimum)

**Distributive Lattice** - A lattice where join distributes over meet:
- a ∧ (b ∨ c) = (a ∧ b) ∨ (a ∧ c)
- a ∨ (b ∧ c) = (a ∨ b) ∧ (a ∨ c)

**Set-Theoretic Interpretation**:
- Join ≡ Union (∪)
- Meet ≡ Intersection (∩)
- Subtract ≡ Set difference (∖) - requires distributive lattice

These algebraic properties enable:
- **Commutativity** - Order independence (for join/meet)
- **Associativity** - Grouping independence
- **Idempotence** - Repeated application gives same result
- **Identity elements** - Empty set, universal set

---

## Core Algebraic Operations

### Join (Union)

**Semantic Meaning**: Creates the union of two tries. A path present in **any** operand will be present in the result.

**Mathematical Definition**:
```
join(A, B) = {p | p ∈ A ∨ p ∈ B}
```

#### Implementations

##### 1. `join_into`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1404`

**Signature**:
```rust
fn join_into<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z
) -> AlgebraicStatus
where
    V: Lattice
```

**Parameters**:
- `self` - Mutable write zipper (modified in-place)
- `read_zipper` - Source zipper to join with (immutable)

**Returns**: `AlgebraicStatus`
- `Element` - Result contains new data (self was changed)
- `Identity` - Self was unchanged (all paths from read_zipper already present)
- `None` - Result is empty (both inputs were empty)

**Behavior**:
1. Traverses both tries in parallel
2. For paths only in `self`: keeps unchanged (structural sharing)
3. For paths only in `read_zipper`: adds to `self`
4. For paths in both: joins values using `V::pjoin`
5. Returns `Identity` if no changes made

**Complexity**:
- **Time**: O(min(|A|, |B|) × log k)
  - Must visit every node in the smaller trie
  - k = branching factor (256 for bytes)
  - Log factor from child lookup
- **Space**: O(|new nodes|)
  - Only allocates for structurally different nodes
  - Shared nodes use existing references
  - Best case: O(1) if all paths already present
  - Worst case: O(|B|) if no paths overlap

**Example**:
```rust
use pathmap::PathMap;

let mut map_a = PathMap::new();
map_a.insert(b"apple", ());
map_a.insert(b"apricot", ());

let mut map_b = PathMap::new();
map_b.insert(b"apricot", ());  // Duplicate
map_b.insert(b"banana", ());    // New

let mut wz = map_a.write_zipper();
let status = wz.join_into(&map_b.read_zipper());

// status == AlgebraicStatus::Element (changed)
// Result: {"apple", "apricot", "banana"}
// Only "banana" branch allocated (structural sharing)
```

##### 2. `join_map_into`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1440`

**Signature**:
```rust
fn join_map_into(
    &mut self,
    path_map: PathMap<V, A>
) -> AlgebraicStatus
where
    V: Lattice
```

**Difference from `join_into`**: Accepts a `PathMap` directly and consumes it (takes ownership).

**Advantages**:
- Slightly more efficient when source won't be reused
- Avoids creating temporary read zipper
- Can reuse source nodes directly

**Use When**: Source PathMap is no longer needed after join.

##### 3. `join_into_take`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1494`

**Signature**:
```rust
fn join_into_take(
    &mut self,
    path_map: &mut PathMap<V, A>,
    swap: bool
) -> AlgebraicStatus
where
    V: Lattice
```

**Parameters**:
- `path_map` - Mutable reference to source (will be emptied)
- `swap` - If true, swaps self with source before joining

**Behavior**:
- Most efficient join variant
- Destructively reads from `path_map` (leaves it empty)
- Can reuse nodes without cloning
- `swap=true` useful when source is larger

**Complexity**:
- **Time**: Same as `join_into`
- **Space**: O(1) best case - can often reuse all nodes from source

**Use When**: Maximum performance needed and source can be destroyed.

##### 4. `join_k_path_into`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1522`

**Signature**:
```rust
fn join_k_path_into<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z,
    byte_count: usize
) -> AlgebraicStatus
where
    V: Lattice
```

**Purpose**: Joins while **collapsing** the first `byte_count` bytes of paths into the zipper's current position.

**Behavior**:
1. Strips `byte_count` bytes from paths in `read_zipper`
2. Joins remaining suffixes at current zipper position
3. Useful for namespace transformations

**Example**:
```rust
// Self at path [a, b]:
//   └─ [c, d] → value1
// Source:
//   └─ [x, y, z, w] → value2
// join_k_path_into(source, 2)
// Result at [a, b]:
//   ├─ [c, d] → value1
//   └─ [z, w] → value2  // Stripped [x, y]
```

**Use When**: Reorganizing namespaces or collapsing path prefixes.

#### MORK Usage: HeadSink

**Location**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/sinks.rs:144`

**Context**: The `HeadSink` maintains only the lexicographically smallest N paths.

**Implementation**:
```rust
match wz.join_into(&self.head.read_zipper()) {
    AlgebraicStatus::Element => { true }
    AlgebraicStatus::Identity => { false }
    AlgebraicStatus::None => { true }
}
```

**Why `join_into`**:
- Accumulates top N paths efficiently
- Structural sharing minimizes memory overhead
- Single operation merges entire PathMap
- Identity detection avoids redundant work

**Performance Impact**:
- Batch joining is O(N log k) instead of O(N² log k) for individual insertions
- Structural sharing reduces memory by ~10-100× vs explicit storage
- Identity returns prevent unnecessary observer notifications

#### Best Practices

**1. Prefer `join_into_take` for Maximum Performance**:
```rust
// Good
let mut accumulator = PathMap::new();
for mut chunk in chunks {
    accumulator.write_zipper().join_into_take(&mut chunk, false);
}

// Less efficient
for chunk in chunks {
    accumulator.write_zipper().join_into(&chunk.read_zipper());
}
```

**2. Check Status to Skip Work**:
```rust
if wz.join_into(&additions) == AlgebraicStatus::Identity {
    return; // No changes, skip expensive propagation
}
notify_observers();
```

**3. Batch Joins**:
```rust
// Bad: Many small joins lose structural sharing
for path in paths {
    let mut temp = PathMap::new();
    temp.insert(path, ());
    wz.join_into(&temp.read_zipper());
}

// Good: Single batched join
let mut batch = PathMap::new();
for path in paths {
    batch.insert(path, ());
}
wz.join_into(&batch.read_zipper());
```

**4. Leverage Structural Sharing**:
```rust
// Share common prefixes
let base = PathMap::new();
base.insert(b"common/prefix/file1", ());
base.insert(b"common/prefix/file2", ());

// Clone is cheap due to reference counting
let mut variant = base.clone();
variant.insert(b"common/prefix/file3", ());
// "common/prefix" structure is shared, not duplicated
```

#### Edge Cases

**1. Empty Tries**:
```rust
let empty = PathMap::new();
let non_empty = PathMap::new();
non_empty.insert(b"key", ());

assert_eq!(
    non_empty.write_zipper().join_into(&empty.read_zipper()),
    AlgebraicStatus::Identity
);
```

**2. Identical Tries**:
```rust
let map = PathMap::new();
map.insert(b"key", ());

assert_eq!(
    map.write_zipper().join_into(&map.read_zipper()),
    AlgebraicStatus::Identity
);
// Identity mask includes both SELF_IDENT | COUNTER_IDENT
```

**3. Value Joining**:
```rust
// For unit type (), join returns identity
let mut map_a = PathMap::new();
map_a.insert(b"key", ());

let mut map_b = PathMap::new();
map_b.insert(b"key", ());

map_a.write_zipper().join_into(&map_b.read_zipper());
// Value at "key" remains ()
// Lattice::pjoin for () always returns Identity
```

**4. Root Values** (with `graft_root_vals` feature disabled - MORK default):
```rust
let mut map_a = PathMap::new();
map_a.set_val(Some(())); // Root value

let mut map_b = PathMap::new();
map_b.set_val(Some(()));

map_a.write_zipper().join_into(&map_b.read_zipper());
// Root values are NOT joined (feature disabled)
// Only subtrie structure is joined
```

---

### Meet (Intersection)

**Semantic Meaning**: Intersects two tries. A path present in **all** operands will be present in the result.

**Mathematical Definition**:
```
meet(A, B) = {p | p ∈ A ∧ p ∈ B}
```

#### Implementations

##### 1. `meet_into`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1623`

**Signature**:
```rust
fn meet_into<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z,
    prune: bool
) -> AlgebraicStatus
where
    V: Lattice
```

**Parameters**:
- `self` - Mutable write zipper (modified in-place to contain intersection)
- `read_zipper` - Source zipper to intersect with (immutable)
- `prune` - Whether to remove dangling paths after intersection

**Returns**: `AlgebraicStatus`
- `Element` - Result contains intersection data (self was changed)
- `Identity` - Self was unchanged (self ⊆ read_zipper)
- `None` - Result is empty (disjoint sets)

**Behavior**:
1. Traverses both tries in parallel
2. For paths only in `self`: removes them
3. For paths only in `read_zipper`: ignores them
4. For paths in both: meets values using `V::pmeet`
5. If `prune=true`: removes empty structural branches
6. Returns `None` if result is completely empty

**Complexity**:
- **Time**: O(min(|A|, |B|) × log k)
  - Must visit nodes in both tries to determine intersection
  - Early termination when read_zipper exhausted
  - Log factor from child lookup
- **Space**: O(|intersection|)
  - Can only shrink self (never grows)
  - Pruning removes unnecessary structure
  - Best case: O(1) if already subset
  - Worst case: O(1) if disjoint (all removed)

**Pruning Semantics**:
- `prune=false`: Keeps structural "scaffolding" even if values removed
- `prune=true`: Removes branches with no values
- Pruning is O(depth) per removed path
- Keep `prune=false` if structure will be refilled later

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());
map_a.insert(b"apricot", ());
map_a.insert(b"banana", ());

let mut map_b = PathMap::new();
map_b.insert(b"apricot", ());
map_b.insert(b"banana", ());
map_b.insert(b"cherry", ());

let mut wz = map_a.write_zipper();
let status = wz.meet_into(&map_b.read_zipper(), true);

// status == AlgebraicStatus::Element (changed)
// Result: {"apricot", "banana"}
// "apple" removed, "cherry" not added
```

##### 2. `meet_k_path_into`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1538`

**Signature**:
```rust
fn meet_k_path_into<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z,
    byte_count: usize
) -> AlgebraicStatus
where
    V: Lattice
```

**Purpose**: Intersects while **collapsing** the first `byte_count` bytes of paths.

**WARNING**: Current implementation has suboptimal performance characteristics.

From source code:
```rust
// GOAT, this is a provisional implementation with the wrong performance characteristics
```

**Behavior**:
- Strips `byte_count` bytes from paths in `read_zipper`
- Meets remaining suffixes at current zipper position
- May have higher complexity than expected

**Recommendation**: Use explicit iteration with `meet_into` if performance is critical.

##### 3. `meet_2`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1690`

**Signature**:
```rust
fn meet_2<Z1, Z2>(
    &mut self,
    read_zipper_1: &Z1,
    read_zipper_2: &Z2,
    prune: bool
) -> AlgebraicStatus
where
    Z1: ZipperSubtries<V, A>,
    Z2: ZipperSubtries<V, A>,
    V: Lattice
```

**Purpose**: Ternary intersection - meets self with two source zippers simultaneously.

**Behavior**:
- Computes `self ∩ read_zipper_1 ∩ read_zipper_2`
- More efficient than two sequential `meet_into` calls
- Single traversal instead of two
- Experimental feature

**Complexity**:
- **Time**: O(min(|A|, |B|, |C|) × log k)
- **Space**: O(|intersection|)

**Use When**: Need to intersect three tries and performance matters.

#### Best Practices

**1. Always Prune Unless Structure Is Reused**:
```rust
// Clean result: prune dangling paths
wz.meet_into(&filter, true);

// Preserve structure: might refill later
wz.meet_into(&partial_filter, false);
// ... later operations use the scaffolding
wz.join_into(&additional_data);
```

**2. Use for Filtering**:
```rust
// Filter paths based on allowed set
let allowed = PathMap::new();
allowed.insert(b"allowed/path/1", ());
allowed.insert(b"allowed/path/2", ());

data.write_zipper().meet_into(&allowed.read_zipper(), true);
// data now contains only allowed paths
```

**3. Check for Empty Result**:
```rust
if wz.meet_into(&constraint, true) == AlgebraicStatus::None {
    // Intersection is empty - no valid solutions
    return None;
}
```

**4. Prefer `meet_2` for Ternary Intersection**:
```rust
// Less efficient: two passes
wz.meet_into(&filter1, false);
wz.meet_into(&filter2, true);

// More efficient: single pass
wz.meet_2(&filter1, &filter2, true);
```

#### Edge Cases

**1. Empty Tries**:
```rust
let empty = PathMap::new();
let non_empty = PathMap::new();
non_empty.insert(b"key", ());

assert_eq!(
    non_empty.write_zipper().meet_into(&empty.read_zipper(), true),
    AlgebraicStatus::None
);
// Result is empty
```

**2. Subset Relationship**:
```rust
let subset = PathMap::new();
subset.insert(b"key", ());

let superset = subset.clone();
superset.insert(b"key2", ());

assert_eq!(
    subset.write_zipper().meet_into(&superset.read_zipper(), true),
    AlgebraicStatus::Identity
);
// subset unchanged (already ⊆ superset)
```

**3. Disjoint Sets**:
```rust
let map_a = PathMap::new();
map_a.insert(b"a", ());

let map_b = PathMap::new();
map_b.insert(b"b", ());

assert_eq!(
    map_a.write_zipper().meet_into(&map_b.read_zipper(), true),
    AlgebraicStatus::None
);
// Completely disjoint - empty result
```

**4. Value Meeting**:
```rust
// For unit type (), meet can annihilate
let mut map_a = PathMap::new();
map_a.insert(b"key", ());

let mut map_b = PathMap::new();
map_b.insert(b"key", ());

map_a.write_zipper().meet_into(&map_b.read_zipper(), true);
// Path "key" preserved
// Value remains () (Lattice::pmeet for () returns Identity)
```

---

### Subtract (Set Difference)

**Semantic Meaning**: Removes all paths present in one trie from another. Non-commutative operation.

**Mathematical Definition**:
```
subtract(A, B) = {p | p ∈ A ∧ p ∉ B} = A ∖ B
```

**Note**: A ∖ B ≠ B ∖ A (order matters)

#### Implementation

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1729`

**Signature**:
```rust
fn subtract_into<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z,
    prune: bool
) -> AlgebraicStatus
where
    V: DistributiveLattice
```

**Parameters**:
- `self` - Mutable write zipper (paths removed in-place)
- `read_zipper` - Source zipper containing paths to remove (immutable)
- `prune` - Whether to remove dangling paths after subtraction

**Trait Requirement**: `DistributiveLattice` (stronger than `Lattice`)
- Requires distributive property: a ∧ (b ∨ c) = (a ∧ b) ∨ (a ∧ c)
- Enables sound value-level subtraction
- Not all lattices support subtraction

**Returns**: `AlgebraicStatus`
- `Element` - Result contains remaining data (self was changed)
- `Identity` - Self was unchanged (no paths from read_zipper were present)
- `None` - Result is empty (all paths removed)

**Behavior**:
1. Traverses both tries in parallel
2. For paths only in `self`: keeps unchanged
3. For paths only in `read_zipper`: ignores them
4. For paths in both: subtracts values using `V::psubtract`
5. If `prune=true`: removes empty branches
6. **Identity mask can only be `SELF_IDENT`** (subtraction never returns counter identity)

**Complexity**:
- **Time**: O(|self| × log k)
  - Must traverse self completely
  - Read_zipper only consulted for matching paths
  - Early termination possible if read_zipper exhausted
- **Space**: O(|difference|)
  - Can only shrink self
  - Best case: O(1) if disjoint (no changes)
  - Worst case: O(1) if all removed

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());
map_a.insert(b"apricot", ());
map_a.insert(b"banana", ());

let mut map_b = PathMap::new();
map_b.insert(b"apricot", ());
map_b.insert(b"cherry", ());

let mut wz = map_a.write_zipper();
let status = wz.subtract_into(&map_b.read_zipper(), true);

// status == AlgebraicStatus::Element (changed)
// Result: {"apple", "banana"}
// "apricot" removed, "cherry" wasn't in map_a
```

#### MORK Usage: RemoveSink

**Location**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/sinks.rs:84`

**Context**: The `RemoveSink` removes all paths matching a pattern from the space.

**Implementation**:
```rust
// In finalize():
match wz.subtract_into(&self.remove.read_zipper(), true) {
    AlgebraicStatus::Element => { true }
    AlgebraicStatus::Identity => { false }
    AlgebraicStatus::None => { true }
}
```

**Why `subtract_into`**:
- Batch removal more efficient than individual deletions
- Single operation preserves maximum structural sharing
- Automatic pruning cleans up empty branches
- Identity detection avoids redundant space updates

**Performance Impact**:
- Batching: O(N log k) instead of O(N² log k) for sequential removals
- Structural sharing: Unchanged portions reference original nodes
- Pruning: O(depth) per removed path removes scaffolding

**Workflow**:
1. During pattern matching, collect paths to remove
2. Build PathMap containing all removals
3. Single `subtract_into` call removes all at once
4. Prune dangling structure
5. Return status indicates whether space changed

#### Best Practices

**1. Always Use `prune=true` for Clean Results**:
```rust
// Good: Clean structure
wz.subtract_into(&to_remove, true);

// Bad: Leaves empty scaffolding (unless you have a reason)
wz.subtract_into(&to_remove, false);
```

**2. Batch Removals**:
```rust
// Bad: Many individual removals
for path in paths_to_remove {
    let mut temp = PathMap::new();
    temp.insert(path, ());
    wz.subtract_into(&temp.read_zipper(), true);
}

// Good: Single batched removal
let mut batch = PathMap::new();
for path in paths_to_remove {
    batch.insert(path, ());
}
wz.subtract_into(&batch.read_zipper(), true);
```

**3. Check for Complete Removal**:
```rust
if wz.subtract_into(&invalid_states, true) == AlgebraicStatus::None {
    // All data was invalid - handle error
    return Err("No valid data remaining");
}
```

**4. Ensure `DistributiveLattice` Trait**:
```rust
// Good: Unit type implements DistributiveLattice
PathMap::<(), u8>::new();

// Good: Option<T> where T: DistributiveLattice
PathMap::<Option<MyValue>, u8>::new();

// Bad: Custom type without DistributiveLattice
// Won't compile!
// PathMap::<MyNonDistributiveValue, u8>::new();
```

#### Edge Cases

**1. Removing from Empty**:
```rust
let mut empty = PathMap::new();
let to_remove = PathMap::new();
to_remove.insert(b"key", ());

assert_eq!(
    empty.write_zipper().subtract_into(&to_remove.read_zipper(), true),
    AlgebraicStatus::Identity
);
// Empty ∖ anything = Empty (unchanged)
```

**2. Removing Empty**:
```rust
let mut map = PathMap::new();
map.insert(b"key", ());
let empty = PathMap::new();

assert_eq!(
    map.write_zipper().subtract_into(&empty.read_zipper(), true),
    AlgebraicStatus::Identity
);
// anything ∖ Empty = anything (unchanged)
```

**3. Removing Self**:
```rust
let map = PathMap::new();
map.insert(b"key", ());

assert_eq!(
    map.write_zipper().subtract_into(&map.read_zipper(), true),
    AlgebraicStatus::None
);
// A ∖ A = ∅
```

**4. Disjoint Sets**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"a", ());

let map_b = PathMap::new();
map_b.insert(b"b", ());

assert_eq!(
    map_a.write_zipper().subtract_into(&map_b.read_zipper(), true),
    AlgebraicStatus::Identity
);
// Disjoint sets: A ∖ B = A (unchanged)
```

**5. Value Subtraction**:
```rust
// For unit type (), subtraction removes entire path
let mut map_a = PathMap::new();
map_a.insert(b"key", ());

let mut map_b = PathMap::new();
map_b.insert(b"key", ());

map_a.write_zipper().subtract_into(&map_b.read_zipper(), true);
// Path "key" completely removed
// DistributiveLattice::psubtract for () returns None
```

---

### Restrict

**Semantic Meaning**: Removes paths that don't have a corresponding prefix in another trie. Generalized meet with wildcard suffixes.

**Mathematical Definition**:
```
restrict(A, B) = {p ∈ A | ∃q ∈ B : q is prefix of p}
```

**Intuition**: Keep paths in A that have "permission" from some prefix in B.

#### Implementations

##### 1. `restrict`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1794`

**Signature**:
```rust
fn restrict<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z
) -> AlgebraicStatus
```

**Parameters**:
- `self` - Mutable write zipper (paths without prefixes removed)
- `read_zipper` - Source zipper containing allowed prefixes

**Returns**: `AlgebraicStatus`
- `Element` - Result contains restricted data (self was changed)
- `Identity` - Self was unchanged (all paths have prefixes in read_zipper)
- `None` - Result is empty (no paths have valid prefixes)

**Behavior**:
1. Traverses self and read_zipper in parallel
2. For each path in self:
   - If any prefix exists in read_zipper: keep path
   - Otherwise: remove path
3. Uses `prestrict_dyn` for recursive restriction
4. Implements a quantale operation (internal trait)

**Complexity**:
- **Time**: O(|self| × log k)
- **Space**: O(|restricted result|)

**Example**:
```rust
let mut data = PathMap::new();
data.insert(b"api/v1/users", ());
data.insert(b"api/v1/posts", ());
data.insert(b"api/v2/users", ());
data.insert(b"internal/secret", ());

let allowed_prefixes = PathMap::new();
allowed_prefixes.insert(b"api/v1", ());  // Prefix only

let mut wz = data.write_zipper();
wz.restrict(&allowed_prefixes.read_zipper());

// Result: {"api/v1/users", "api/v1/posts"}
// "api/v2/users" removed (no v2 prefix)
// "internal/secret" removed (no internal prefix)
```

##### 2. `restricting`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1821`

**Signature**:
```rust
fn restricting<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z,
    stems: &mut PathMap<Z::Subtries, A>
) -> AlgebraicStatus
where
    Z::Subtries: Clone
```

**Additional Parameter**:
- `stems` - Output PathMap populated with prefix → subtrie mappings

**Behavior**:
- Same restriction semantics as `restrict`
- Additionally populates `stems` with prefix paths mapping to their corresponding subtries
- Useful for analyzing which prefixes matched

**Example**:
```rust
let mut data = PathMap::new();
data.insert(b"api/v1/users", ());
data.insert(b"api/v2/posts", ());

let prefixes = PathMap::new();
prefixes.insert(b"api/v1", ());

let mut stems = PathMap::new();
data.write_zipper().restricting(&prefixes.read_zipper(), &mut stems);

// data restricted to api/v1/* paths
// stems contains mapping: b"api/v1" → subtrie(users)
```

#### Best Practices

**1. Use for Namespace Filtering**:
```rust
// Allow only certain namespace prefixes
let namespaces = PathMap::new();
namespaces.insert(b"public/", ());
namespaces.insert(b"user/123/", ());

data.write_zipper().restrict(&namespaces.read_zipper());
// Only paths under public/ or user/123/ remain
```

**2. Combine with Other Operations**:
```rust
// Restrict then intersect
wz.restrict(&allowed_prefixes.read_zipper());
wz.meet_into(&specific_filter.read_zipper(), true);
```

**3. Use `restricting` for Analysis**:
```rust
let mut stems = PathMap::new();
data.write_zipper().restricting(&prefixes.read_zipper(), &mut stems);

// Analyze which prefixes matched
for (prefix, subtrie) in stems.iter() {
    println!("Prefix {:?} matched {} paths", prefix, subtrie.val_count());
}
```

#### Edge Cases

**1. Empty Prefix Set**:
```rust
let mut data = PathMap::new();
data.insert(b"key", ());
let empty = PathMap::new();

assert_eq!(
    data.write_zipper().restrict(&empty.read_zipper()),
    AlgebraicStatus::None
);
// No allowed prefixes - all removed
```

**2. Universal Prefix (Empty String)**:
```rust
let mut data = PathMap::new();
data.insert(b"any/path", ());

let universal = PathMap::new();
universal.set_val(Some(()));  // Root value = empty prefix

data.write_zipper().restrict(&universal.read_zipper());
// All paths kept (empty string is prefix of everything)
```

**3. Exact Match vs Prefix**:
```rust
let mut data = PathMap::new();
data.insert(b"exact", ());
data.insert(b"exact/more", ());

let prefixes = PathMap::new();
prefixes.insert(b"exact", ());

data.write_zipper().restrict(&prefixes.read_zipper());
// Result: {"exact", "exact/more"}
// Both kept: "exact" matches exactly, "exact/more" has prefix
```

#### Future Considerations

From source code comments:
> "may be replaced by a restrict policy passed to meet_into in a future version"

Current `restrict` may eventually become a policy parameter to `meet_into`, providing more flexible restriction semantics.

---

### Graft

**Semantic Meaning**: Replaces the entire subtrie below the zipper's focus with another subtrie. Wholesale structural replacement.

**Mathematical Definition**:
```
graft(T, p, S) = T with subtrie at path p replaced by S
```

#### Implementations

##### 1. `graft`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1381`

**Signature**:
```rust
fn graft<Z: ZipperSubtries<V, A>>(
    &mut self,
    read_zipper: &Z
)
```

**Parameters**:
- `self` - Write zipper positioned at graft point
- `read_zipper` - Source zipper whose subtrie will replace self's subtrie

**Returns**: Nothing (always succeeds)

**Behavior**:
1. Completely replaces subtrie below current focus
2. Does **not** affect value at focus (unless `graft_root_vals` feature enabled)
3. Uses structural sharing (references source nodes)
4. Extremely efficient - just updates references

**Complexity**:
- **Time**: O(1) - just replaces a reference
- **Space**: O(1) - uses structural sharing (reference counting)

**Example**:
```rust
let mut target = PathMap::new();
target.insert(b"root/old/path1", ());
target.insert(b"root/old/path2", ());
target.insert(b"root/keep", ());

let replacement = PathMap::new();
replacement.insert(b"new/path1", ());
replacement.insert(b"new/path2", ());

let mut wz = target.write_zipper();
wz.move_to_path(b"root/old");
wz.graft(&replacement.read_zipper());

// Result:
// "root/keep" → (unchanged)
// "root/old/new/path1" → (grafted)
// "root/old/new/path2" → (grafted)
// Old "root/old/{path1,path2}" replaced
```

##### 2. `graft_map`

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1391`

**Signature**:
```rust
fn graft_map(
    &mut self,
    path_map: PathMap<V, A>
)
```

**Difference from `graft`**: Accepts a `PathMap` directly and consumes it.

**Advantages**:
- Avoids creating temporary read zipper
- More efficient when source won't be reused
- Clearer intent (source is consumed)

**Use When**: Source PathMap is no longer needed after graft.

#### MORK Usage: Swap Operation

**Location**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/sinks.rs:292`

**Context**: Swapping subtries in the space during query execution.

**Implementation**:
```rust
rooted_input
    .write_zipper_at_path(wz.root_prefix_path())
    .graft_map(_to_swap);
```

**Why `graft`**:
- O(1) operation for wholesale replacement
- Structural sharing prevents deep copies
- Clean semantics for namespace replacement
- No value-level merging complexity

**Use Case**: Replacing entire namespace or context in single operation.

#### Best Practices

**1. Use for Wholesale Replacement**:
```rust
// Efficient: Replace entire subtrie at once
wz.move_to_path(b"namespace");
wz.graft(&new_namespace.read_zipper());

// Inefficient: Remove old + join new
wz.move_to_path(b"namespace");
wz.remove_branches();
wz.join_into(&new_namespace.read_zipper());
```

**2. Prefer `graft_map` When Consuming Source**:
```rust
// Good: Consume source
let replacement = build_replacement();
wz.graft_map(replacement);

// Less efficient: Create zipper only to discard source
let replacement = build_replacement();
wz.graft(&replacement.read_zipper());
drop(replacement);
```

**3. Navigate Before Grafting**:
```rust
// Graft at specific location
wz.move_to_path(b"target/location");
wz.graft(&new_subtrie.read_zipper());

// Graft at root (replace everything)
wz.move_to_root();
wz.graft(&entire_replacement.read_zipper());
```

**4. Combine with Other Operations**:
```rust
// Build composite structure
wz.move_to_path(b"namespace/a");
wz.graft(&subtrie_a.read_zipper());

wz.move_to_path(b"namespace/b");
wz.graft(&subtrie_b.read_zipper());

// Now namespace/{a,b} are independent subtries
```

#### Edge Cases

**1. Grafting Empty Subtrie**:
```rust
let empty = PathMap::new();

wz.move_to_path(b"target");
wz.graft(&empty.read_zipper());

// Effectively calls remove_branches()
// Subtrie at "target" becomes empty
```

**2. Root Value Handling** (default: `graft_root_vals` disabled):
```rust
let mut target = PathMap::new();
target.insert(b"root", ());
target.move_to_path(b"root");
target.set_val(Some(()));  // Value at "root"

let replacement = PathMap::new();
replacement.set_val(Some(()));  // Different value at root

target.write_zipper_at_path(b"root").graft(&replacement.read_zipper());

// Value at "root" is PRESERVED (feature disabled)
// Only subtrie below "root" is replaced
```

**3. Grafting Self**:
```rust
// Creates structural sharing - same nodes referenced
wz.graft(&wz.read_zipper());
// Result: Subtrie points to itself (via reference counting)
// Usually not useful, but safe
```

**4. Structural Sharing**:
```rust
let shared = PathMap::new();
shared.insert(b"common/path", ());

// Graft into multiple locations
wz.move_to_path(b"location/a");
wz.graft(&shared.read_zipper());

wz.move_to_path(b"location/b");
wz.graft(&shared.read_zipper());

// "common/path" structure shared between both locations
// Changes to shared affect both (if mutable references exist)
// With reference counting: separate copies if modified
```

---

## Auxiliary Operations

### Insert Prefix

**Purpose**: Inserts a path prefix before all paths in a subtrie.

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs`

**Signature**:
```rust
fn insert_prefix<I>(&mut self, prefix: I)
where
    I: IntoIterator<Item = A>
```

**Behavior**:
- Adds `prefix` before all paths below current focus
- Creates new internal nodes for prefix
- Adjusts structural sharing

**Example**:
```rust
let mut map = PathMap::new();
map.insert(b"path", ());

map.write_zipper().insert_prefix(b"new/prefix/");
// Result: b"new/prefix/path"
```

### Remove Prefix

**Purpose**: Removes a common prefix from all paths in a subtrie.

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs`

**Signature**:
```rust
fn remove_prefix(&mut self, byte_count: usize)
```

**Behavior**:
- Strips first `byte_count` bytes from all paths
- Adjusts node structure
- May collapse nodes

### Remove Branches

**Purpose**: Removes all branches (subtrie) below current focus.

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs`

**Signature**:
```rust
fn remove_branches(&mut self)
```

**Behavior**:
- Deletes entire subtrie below focus
- Preserves value at focus
- O(1) operation (just drops reference)

**Example**:
```rust
wz.move_to_path(b"namespace");
wz.remove_branches();
// All paths under "namespace" removed
// Value at "namespace" preserved
```

### Remove Unmasked Branches

**Purpose**: Removes branches not matching a byte mask.

**Signature**:
```rust
fn remove_unmasked_branches(&mut self, byte_mask: &ByteMask)
```

**Behavior**:
- Selectively removes child branches
- Keeps only branches matching mask
- Used for pattern-based pruning

### Take Map

**Purpose**: Extracts a PathMap from the current subtrie.

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs`

**Signature**:
```rust
fn take_map(&mut self) -> PathMap<V, A>
```

**Behavior**:
- Removes subtrie from current position
- Returns it as a new PathMap
- Leaves empty structure behind

### Prune Path

**Purpose**: Removes empty structural paths after operations.

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs`

**Signatures**:
```rust
fn prune_path(&mut self, byte_count: usize) -> bool
fn prune_ascend(&mut self) -> bool
```

**Behavior**:
- Removes scaffolding with no values
- Called automatically when `prune=true` in algebraic operations
- `prune_path`: Prunes specific depth
- `prune_ascend`: Prunes upward from focus

---

## Algebraic Structures

### AlgebraicResult<V>

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:23`

**Definition**:
```rust
pub enum AlgebraicResult<V> {
    None,            // Operation resulted in annihilation/empty
    Identity(u64),   // Result is identity of one or more inputs
    Element(V),      // New result value
}
```

**Bitmask Constants**:
```rust
pub const SELF_IDENT: u64 = 0x1;      // Result equals self
pub const COUNTER_IDENT: u64 = 0x2;   // Result equals counter-party
```

**Interpretation**:
- `None`: Operation annihilated (e.g., meet of disjoint sets)
- `Identity(SELF_IDENT)`: Result equals first operand
- `Identity(COUNTER_IDENT)`: Result equals second operand
- `Identity(SELF_IDENT | COUNTER_IDENT)`: Operands are equal
- `Element(v)`: New value `v` computed

**Usage in Value-Level Operations**:
```rust
impl Lattice for () {
    fn pjoin(&self, _: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT)
    }
}

impl Lattice for Option<T> where T: Lattice {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        match (self, other) {
            (None, None) => AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT),
            (Some(_), None) => AlgebraicResult::Identity(SELF_IDENT),
            (None, Some(_)) => AlgebraicResult::Identity(COUNTER_IDENT),
            (Some(a), Some(b)) => match a.pjoin(b) {
                AlgebraicResult::None => AlgebraicResult::None,
                AlgebraicResult::Identity(mask) => AlgebraicResult::Identity(mask),
                AlgebraicResult::Element(v) => AlgebraicResult::Element(Some(v)),
            }
        }
    }
}
```

### AlgebraicStatus

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:339`

**Definition**:
```rust
pub enum AlgebraicStatus {
    Element,   // Contains new output data
    Identity,  // Unchanged from input
    None,      // Completely empty/annihilated
}
```

**Conversion from `AlgebraicResult<V>`**:
```rust
impl<V> From<AlgebraicResult<V>> for AlgebraicStatus {
    fn from(result: AlgebraicResult<V>) -> Self {
        match result {
            AlgebraicResult::None => AlgebraicStatus::None,
            AlgebraicResult::Identity(_) => AlgebraicStatus::Identity,
            AlgebraicResult::Element(_) => AlgebraicStatus::Element,
        }
    }
}
```

**Usage in Structure-Level Operations**:
```rust
let status = wz.join_into(&other.read_zipper());
match status {
    AlgebraicStatus::Element => {
        // Changed - need to propagate updates
        notify_observers();
    }
    AlgebraicStatus::Identity => {
        // Unchanged - skip expensive work
    }
    AlgebraicStatus::None => {
        // Empty - may need special handling
        handle_empty_case();
    }
}
```

### Lattice Trait

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:532`

**Definition**:
```rust
pub trait Lattice: Sized {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self>;
    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self>;

    fn join_into(&mut self, other: &Self) -> AlgebraicStatus {
        match self.pjoin(other) {
            AlgebraicResult::None => {
                // Annihilated - implementation specific
                AlgebraicStatus::None
            }
            AlgebraicResult::Identity(mask) => {
                if mask & COUNTER_IDENT > 0 {
                    *self = other.clone();
                    if mask & SELF_IDENT > 0 {
                        AlgebraicStatus::Identity
                    } else {
                        AlgebraicStatus::Element
                    }
                } else {
                    AlgebraicStatus::Identity
                }
            }
            AlgebraicResult::Element(v) => {
                *self = v;
                AlgebraicStatus::Element
            }
        }
    }
}
```

**Axioms**:
1. **Commutativity**: `a ∨ b = b ∨ a` and `a ∧ b = b ∧ a`
2. **Associativity**: `(a ∨ b) ∨ c = a ∨ (b ∨ c)`
3. **Idempotence**: `a ∨ a = a` and `a ∧ a = a`
4. **Absorption**: `a ∨ (a ∧ b) = a` and `a ∧ (a ∨ b) = a`

**Standard Implementations**:
- `()` - Unit type (trivial lattice)
- `bool` - Boolean lattice (∨ = OR, ∧ = AND)
- `Option<T>` - Lifted lattice
- `Box<T>` - Boxed lattice
- `HashSet<T>` - Set lattice (∨ = ∪, ∧ = ∩)
- `HashMap<K, V>` - Map lattice (pointwise operations)

### DistributiveLattice Trait

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:602`

**Definition**:
```rust
pub trait DistributiveLattice: Lattice {
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self>;
}
```

**Distributive Axiom**:
```
a ∧ (b ∨ c) = (a ∧ b) ∨ (a ∧ c)
a ∨ (b ∧ c) = (a ∨ b) ∧ (a ∨ c)
```

**Subtraction Semantics**:
```
a ∖ b = a ∧ ¬b (in Boolean algebra)
```

**Standard Implementations**:
- `()` - Unit type
- `bool` - Boolean algebra
- `Option<T>` where `T: DistributiveLattice`
- `Box<T>` where `T: DistributiveLattice`
- `HashMap<K, V>` where `V: DistributiveLattice`

**Why Required for `subtract_into`**:
- Ensures sound value-level subtraction
- Not all lattices support subtraction (e.g., arbitrary posets)
- Guarantees `(a ∨ b) ∖ b = a ∖ b` (distributivity)

### SetLattice Trait

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:918`

**Definition**:
```rust
pub trait SetLattice: Eq {
    fn insert_element(&mut self, elem: Self::Element) -> bool;
    fn remove_element(&mut self, elem: &Self::Element) -> bool;
    fn contains_element(&self, elem: &Self::Element) -> bool;
    fn is_empty(&self) -> bool;

    type Element;
    type Iter<'a>: Iterator<Item = &'a Self::Element> where Self: 'a;
    fn iter(&self) -> Self::Iter<'_>;
}
```

**Purpose**: Abstraction for set-like structures.

**Automatic Derivations**:
- `Lattice` implementation (via macro)
- `DistributiveLattice` implementation (via macro)
- Standard set operations

**Implementations**:
- `HashSet<T>`
- `BTreeSet<T>`
- Custom set types

---

## Best Practices

### 1. Choose the Right Operation

**Decision Tree**:
```
Need to combine data?
├─ Yes → Use join_into
└─ No
   ├─ Need only common elements? → Use meet_into
   ├─ Need to remove elements? → Use subtract_into
   ├─ Need prefix-based filtering? → Use restrict
   └─ Need wholesale replacement? → Use graft
```

**Examples**:
```rust
// Accumulation
results.write_zipper().join_into(&new_results.read_zipper());

// Filtering
candidates.write_zipper().meet_into(&valid_set.read_zipper(), true);

// Removal
space.write_zipper().subtract_into(&to_remove.read_zipper(), true);

// Namespace filtering
data.write_zipper().restrict(&allowed_namespaces.read_zipper());

// Replacement
wz.move_to_path(b"old/namespace");
wz.graft(&new_namespace.read_zipper());
```

### 2. Batch Operations

**Anti-Pattern**:
```rust
// Bad: Many individual operations lose structural sharing
for item in items {
    let mut temp = PathMap::new();
    temp.insert(item.path, item.value);
    wz.join_into(&temp.read_zipper());
}
```

**Good Pattern**:
```rust
// Good: Single batched operation
let mut batch = PathMap::new();
for item in items {
    batch.insert(item.path, item.value);
}
wz.join_into(&batch.read_zipper());
```

**Why**:
- Batching: O(N log k) vs O(N² log k)
- Structural sharing maximized
- Single structural modification

### 3. Use Prune Appropriately

**Always Prune** (default):
```rust
// Clean structure after modification
wz.meet_into(&filter, true);
wz.subtract_into(&removals, true);
```

**Skip Pruning** (rare):
```rust
// Preserve scaffolding for later refill
wz.meet_into(&partial_filter, false);
// ... later
wz.join_into(&additional_data.read_zipper());
// Scaffolding allows efficient insertion
```

**Cost**: O(depth) per pruned path (usually negligible)

### 4. Check Return Status

**Optimize Based on Status**:
```rust
match wz.join_into(&updates) {
    AlgebraicStatus::Element => {
        // Changed - propagate to observers
        notify_observers();
        invalidate_caches();
        return true;
    }
    AlgebraicStatus::Identity => {
        // No change - skip expensive work
        return false;
    }
    AlgebraicStatus::None => {
        // Empty - may need special handling
        handle_empty_state();
        return true;
    }
}
```

### 5. Leverage Structural Sharing

**Pattern 1: Variant Creation**:
```rust
// Base configuration
let base = PathMap::new();
base.insert(b"common/setting1", ());
base.insert(b"common/setting2", ());

// Create variants (cheap due to sharing)
let variant_a = base.clone();
variant_a.insert(b"specific/a", ());

let variant_b = base.clone();
variant_b.insert(b"specific/b", ());

// "common/*" structure shared between all
```

**Pattern 2: Incremental Updates**:
```rust
// Previous state shared
let v1 = current_state.clone();

// Apply updates (only new structure allocated)
current_state.write_zipper().join_into(&updates.read_zipper());

// Can diff: v1 vs current_state share unchanged portions
```

### 6. Prefer Consuming Operations

**When Possible**:
```rust
// Best: Consume source
wz.join_into_take(&mut source, false);
wz.graft_map(replacement);

// Good: Borrow
wz.join_into(&source.read_zipper());
wz.graft(&replacement.read_zipper());
```

**Why**: Consuming operations can reuse source nodes directly, avoiding reference count manipulation.

### 7. Use Appropriate Value Types

**For Pure Set Operations**:
```rust
// Unit type: no value-level data
PathMap::<(), u8>::new();
```

**For Annotated Sets**:
```rust
// Option<T>: Some values present
PathMap::<Option<Metadata>, u8>::new();
```

**For Counters/Accumulators**:
```rust
// Numeric types with appropriate lattice semantics
PathMap::<usize, u8>::new();  // If you implement Lattice
```

**For Complex Data**:
```rust
// Custom types with domain-specific lattices
impl Lattice for MyDomainValue {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        // Domain-specific join logic
    }
    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
        // Domain-specific meet logic
    }
}
```

---

## Edge Cases and Caveats

### 1. Value-Level Semantics

**Caveat**: Operations on values depend on `Lattice` / `DistributiveLattice` implementations.

**Unit Type `()`**:
```rust
impl Lattice for () {
    fn pjoin(&self, _: &Self) -> AlgebraicResult<Self> {
        // Always returns identity
        AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT)
    }
    fn pmeet(&self, _: &Self) -> AlgebraicResult<Self> {
        // Always returns identity
        AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT)
    }
}
```

**Implication**: For `PathMap<(), A>`, operations are purely structural (no value merging).

**Option<T>**:
```rust
// join: Some(a) ∨ Some(b) = Some(a ∨ b)
//       Some(a) ∨ None = Some(a)
//       None ∨ None = None

// meet: Some(a) ∧ Some(b) = Some(a ∧ b)
//       Some(a) ∧ None = None
//       None ∧ None = None
```

**Custom Types**: Must implement appropriate semantics.

### 2. Root Value Handling

**Feature**: `graft_root_vals` (disabled by default in MORK)

**When Disabled** (MORK default):
- `graft` does NOT transfer root value
- `join_into` / `meet_into` do NOT operate on root values
- Root values are separate from subtrie structure

**When Enabled**:
- `graft` transfers root value from source
- Algebraic operations include root values
- More intuitive for some use cases

**Example** (feature disabled):
```rust
let mut map = PathMap::new();
map.set_val(Some(()));

let replacement = PathMap::new();
replacement.set_val(Some(()));

map.write_zipper().graft(&replacement.read_zipper());
// map.val() is still Some(()) from original
// Root value NOT replaced
```

### 3. Identity Masks

**Caveat**: Identity can apply to both operands simultaneously.

**Example**:
```rust
let mut a = PathMap::new();
a.insert(b"key", ());

let b = a.clone();

match a.pjoin(&b) {
    AlgebraicResult::Identity(mask) => {
        assert_eq!(mask, SELF_IDENT | COUNTER_IDENT);
        // Both bits set: a == b
    }
    _ => unreachable!(),
}
```

**Implication**: Always check both bits, don't assume only one is set.

### 4. Subtract Only Returns `SELF_IDENT`

**Caveat**: `subtract_into` never returns `COUNTER_IDENT` in identity mask.

**Reason**:
```
A ∖ B = A  ⟺  A ∩ B = ∅  (A and B are disjoint)
A ∖ B = B  ⟺  impossible (unless both empty)
```

**Code**:
```rust
// subtract_into identity mask is always SELF_IDENT (or none)
let status = wz.subtract_into(&other, true);
if status == AlgebraicStatus::Identity {
    // Can only mean: self ∩ other = ∅
    // NOT: self == other
}
```

### 5. Empty Set Behavior

**Join**:
```rust
A ∪ ∅ = A  // Identity
∅ ∪ A = A  // Identity
∅ ∪ ∅ = ∅  // Identity
```

**Meet**:
```rust
A ∩ ∅ = ∅  // None (annihilated)
∅ ∩ A = ∅  // None
∅ ∩ ∅ = ∅  // Identity
```

**Subtract**:
```rust
A ∖ ∅ = A  // Identity
∅ ∖ A = ∅  // Identity
∅ ∖ ∅ = ∅  // Identity
A ∖ A = ∅  // None
```

### 6. Non-Commutative Operations

**Commutative**:
- `join_into`: A ∪ B = B ∪ A
- `meet_into`: A ∩ B = B ∩ A

**Non-Commutative**:
- `subtract_into`: A ∖ B ≠ B ∖ A (in general)
- `restrict`: restrict(A, B) ≠ restrict(B, A)

**Implication**: Order matters for subtract and restrict.

### 7. meet_k_path_into Performance

**WARNING**: Current implementation suboptimal.

**Source Comment** (`/home/dylon/Workspace/f1r3fly.io/PathMap/src/write_zipper.rs:1539`):
```rust
// GOAT, this is a provisional implementation with the wrong performance characteristics
```

**Workaround**: Use explicit iteration if performance critical:
```rust
// Instead of meet_k_path_into(source, k)
for path in source.paths() {
    let suffix = &path[k..];
    wz.move_to_path(suffix);
    // ... meet logic
}
```

### 8. Lattice Requirements

**join_into / meet_into**: Require `V: Lattice`

**subtract_into**: Requires `V: DistributiveLattice`

**Implication**: Not all types support all operations.

**Example**:
```rust
// OK: Unit type implements DistributiveLattice
PathMap::<(), u8>::new().write_zipper().subtract_into(...);

// OK: Option<T> implements DistributiveLattice if T does
PathMap::<Option<bool>, u8>::new().write_zipper().subtract_into(...);

// NOT OK: General Lattice without distributivity
struct NonDistributiveLatticeValue;
impl Lattice for NonDistributiveLatticeValue { /* ... */ }
// This will NOT compile:
// PathMap::<NonDistributiveLatticeValue, u8>::new().write_zipper().subtract_into(...);
```

---

## Integration with MORK

### Space Structure

**Location**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/space.rs`

**Definition**:
```rust
pub struct Space {
    pub btm: PathMap<()>,  // Base trie map (main storage)
    pub sm: SharedMappingHandle,  // Symbol interning
    pub mmaps: HashMap<&'static str, ArenaCompactTree<Mmap>>  // Memory-mapped tries
}
```

**Roles**:
- `btm`: Mutable storage for patterns (PathMap<(), u8>)
- `sm`: Symbol table for term serialization
- `mmaps`: Read-only memory-mapped tries for efficient queries

### Source Types

**1. BTMSource**: Queries the base trie map
```rust
pub struct BTMSource {
    // Queries btm directly
    // Uses coreferential_transition for pattern matching
}
```

**2. ACTSource**: Queries memory-mapped tries
```rust
pub struct ACTSource {
    // Read-only queries on mmapped data
    // Efficient for large static datasets
}
```

**3. CmpSource**: Equality/inequality comparisons
```rust
pub struct CmpSource {
    // Dependent enrollment based on bound variables
    // Implements != and == predicates
}
```

### Sink Types and Algebraic Operations

**1. AddSink**: Adds patterns to space
```rust
impl Sink for AddSink {
    fn finalize(&mut self, space: &mut Space) -> bool {
        // Uses set_val / insert internally
        // May use join_into for batch additions
    }
}
```

**2. RemoveSink**: Removes patterns using `subtract_into`
```rust
impl Sink for RemoveSink {
    fn finalize(&mut self, space: &mut Space) -> bool {
        let mut wz = rooted_input.write_zipper();
        match wz.subtract_into(&self.remove.read_zipper(), true) {
            AlgebraicStatus::Element => true,
            AlgebraicStatus::Identity => false,
            AlgebraicStatus::None => true,
        }
    }
}
```

**Why `subtract_into`**:
- Batch removal (O(N log k) vs O(N² log k))
- Automatic pruning of empty branches
- Structural sharing preserved
- Clean status reporting

**3. HeadSink**: Selects top-N using `join_into`
```rust
impl Sink for HeadSink {
    fn finalize(&mut self, space: &mut Space) -> bool {
        let mut wz = rooted_input.write_zipper();
        match wz.join_into(&self.head.read_zipper()) {
            AlgebraicStatus::Element => true,
            AlgebraicStatus::Identity => false,
            AlgebraicStatus::None => true,
        }
    }
}
```

**Why `join_into`**:
- Accumulates top-N paths efficiently
- Structural sharing minimizes overhead
- Single operation merges entire selection
- Identity detection avoids redundant updates

**4. CountSink**: Counts patterns
```rust
impl Sink for CountSink {
    fn finalize(&mut self, space: &mut Space) -> bool {
        let count = self.matches.val_count();
        // Generate substitutions with count
        // Uses set_val to add results
    }
}
```

**No Direct Algebraic Operation**: Uses `val_count()` to get count, then inserts results.

### Query Execution Flow

**1. Parse Query**:
```
!(query! (&space <source1> <source2> ...) <sink1> <sink2> ...)
```

**2. Create Product Zipper**:
```rust
let product = ProductZipper::new(vec![
    source1.read_zipper(),
    source2.read_zipper(),
    // ...
]);
```

**3. Execute Pattern Matching**:
```rust
coreferential_transition(
    &product,
    &pattern,
    &mut context,
    &mut sink_callbacks
);
```

**4. Finalize Sinks** (Algebraic Operations):
```rust
for sink in sinks {
    let changed = sink.finalize(space);
    if changed {
        space_modified = true;
    }
}
```

**5. Return Status**:
- Whether space was modified
- Allows upstream change propagation

### Performance Characteristics in MORK

**Pattern Removal** (RemoveSink):
- **Complexity**: O(M + N log k)
  - M = pattern matching cost
  - N = paths to remove
  - k = branching factor
- **Memory**: O(N) for removal PathMap
- **Batching Benefit**: ~100-1000× vs individual removals

**Top-N Selection** (HeadSink):
- **Complexity**: O(M + N log N + log k)
  - M = pattern matching cost
  - N = total matches
  - Maintains sorted set of size N
- **Memory**: O(N) for top-N PathMap
- **Structural Sharing**: ~10-100× memory reduction

**Pattern Counting** (CountSink):
- **Complexity**: O(M + N)
  - M = pattern matching cost
  - N = unique matches (val_count is O(1))
- **Memory**: O(N) for match collection

---

## References

### PathMap Documentation

**PathMap Book**: `/home/dylon/Workspace/f1r3fly.io/PathMap/pathmap-book/`
- `src/1.01.00_algebraic_ops.md` - Algebraic Operations Overview
- `src/1.01.01_algebraic_traits.md` - Lattice Traits
- `src/1.02.07_zipper_algebra.md` - Zipper Algebraic Operations

### Source Code Locations

**PathMap Core**:
- `src/trie_map.rs` - PathMap implementation
- `src/write_zipper.rs` - Algebraic operations (lines 1381-1821)
- `src/ring.rs` - Lattice traits and implementations (lines 23-918)

**MORK Integration**:
- `kernel/src/space.rs` - Space structure
- `kernel/src/sources.rs` - Source implementations
- `kernel/src/sinks.rs` - Sink implementations (lines 55-340)
- `kernel/src/pattern_matching.rs` - Pattern matching integration

### Tests

**PathMap Tests**:
- `src/ring.rs` (lines 1194+) - Lattice tests
- Throughout PathMap source files - Operation tests

**MORK Tests**:
- `kernel/tests/` - Integration tests
- Query execution tests demonstrate algebraic operations in context

### Academic References

**Lattice Theory**:
- Davey, B. A., & Priestley, H. A. (2002). *Introduction to Lattices and Order*. Cambridge University Press.

**Trie Data Structures**:
- Morrison, D. R. (1968). "PATRICIA—Practical Algorithm To Retrieve Information Coded in Alphanumeric". *Journal of the ACM*.

**Structural Sharing**:
- Okasaki, C. (1999). *Purely Functional Data Structures*. Cambridge University Press.

---

## Appendix: Complexity Proofs

### Theorem 1: join_into Time Complexity

**Claim**: `join_into` has time complexity O(min(|A|, |B|) × log k).

**Proof**:
1. `join_into` performs parallel traversal of tries A (self) and B (read_zipper)
2. At each node:
   - Child lookup: O(log k) with binary search in sorted child array
   - Recursive join: O(1) reference manipulation or recursive call
3. Number of nodes visited: min(|A|, |B|)
   - Must visit all nodes in smaller trie
   - May visit fewer nodes in larger trie
   - Can terminate early if entire branch present in self
4. Total: min(|A|, |B|) × O(log k) = O(min(|A|, |B|) × log k)

**Note**: k = maximum branching factor (256 for byte keys), typically log₂(256) = 8.

### Theorem 2: Structural Sharing Space Efficiency

**Claim**: With structural sharing, space complexity is O(|unique nodes|), not O(|total paths|).

**Proof**:
1. Each node is reference-counted (TrieNodeODRc)
2. Common subtries share the same node reference
3. Space allocated only for unique node structures
4. Example: N paths with common prefix of length P
   - Without sharing: N × P bytes
   - With sharing: P + (N × suffix_length) bytes
   - Reduction: ~P × (N - 1) bytes saved
5. Worst case: All paths completely disjoint → no sharing benefit
6. Best case: All paths share maximum prefix → O(1) shared nodes

### Theorem 3: subtract_into Never Returns COUNTER_IDENT

**Claim**: `subtract_into` identity mask can only contain `SELF_IDENT`, never `COUNTER_IDENT`.

**Proof** (by contradiction):
1. Suppose `subtract_into(A, B)` returns `Identity(COUNTER_IDENT)`
2. This means: A ∖ B = B
3. By definition: A ∖ B = {x | x ∈ A ∧ x ∉ B}
4. For A ∖ B = B: ∀x ∈ B : x ∈ A ∧ x ∉ B
5. Contradiction: x ∈ B and x ∉ B cannot both be true
6. Therefore: `subtract_into` never returns `COUNTER_IDENT`
7. Can return `SELF_IDENT` when A ∩ B = ∅ (disjoint)

**QED**

---

**End of Document**

*For additional information, consult the PathMap book and source code. For questions or contributions, see the MORK and PathMap repositories.*
