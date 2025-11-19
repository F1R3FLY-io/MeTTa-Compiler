# MORK Algebraic Operations: API Reference

**Version**: 1.0
**Last Updated**: 2025-11-13
**Author**: MORK Documentation Team

## Table of Contents

1. [Module Overview](#module-overview)
2. [Core Types](#core-types)
3. [Algebraic Operations API](#algebraic-operations-api)
4. [Trait Definitions](#trait-definitions)
5. [Return Types](#return-types)
6. [Zipper API](#zipper-api)
7. [Helper Methods](#helper-methods)
8. [Error Handling](#error-handling)
9. [Complete Examples](#complete-examples)

---

## Module Overview

### PathMap Module Structure

```
pathmap/
├── src/
│   ├── trie_map.rs          # PathMap<V, A> implementation
│   ├── write_zipper.rs      # WriteZipper<V, A> algebraic operations
│   ├── read_zipper.rs       # ReadZipper<V, A> read-only operations
│   ├── ring.rs              # Lattice traits and AlgebraicResult
│   ├── trie_node.rs         # Internal node structures
│   └── lib.rs               # Public API exports
```

### MORK Module Structure

```
MORK/
├── kernel/
│   ├── src/
│   │   ├── space.rs         # Space (uses PathMap)
│   │   ├── sources.rs       # Pattern matching sources
│   │   ├── sinks.rs         # Algebraic operation consumers
│   │   └── pattern_matching.rs  # Integration layer
```

### Import Paths

**PathMap** (used by MORK):
```rust
use pathmap::{PathMap, WriteZipper, ReadZipper};
use pathmap::ring::{Lattice, DistributiveLattice, AlgebraicResult, AlgebraicStatus};
```

**MORK**:
```rust
use mork::space::Space;
use mork::sinks::{AddSink, RemoveSink, HeadSink, CountSink};
```

---

## Core Types

### PathMap<V, A>

**Definition**:
```rust
pub struct PathMap<V, A = u8> {
    // Internal: Reference-counted root node
    root: Option<TrieNodeODRc<V, A>>,
}
```

**Type Parameters**:
- `V`: Value type (must implement `Lattice` for algebraic operations)
- `A`: Alphabet type (default: `u8` for byte paths)

**Constraints**:
- `V: Lattice` - For join/meet operations
- `V: DistributiveLattice` - For subtract operations
- `A: Ord + Clone` - For path storage and lookup

**Constructors**:
```rust
impl<V, A> PathMap<V, A> {
    /// Creates an empty PathMap
    pub fn new() -> Self;

    /// Creates a PathMap with a single path-value pair
    pub fn single<P>(path: P, value: V) -> Self
    where
        P: IntoIterator<Item = A>;

    /// Creates a PathMap from an iterator of (path, value) pairs
    pub fn from_iter<I, P>(iter: I) -> Self
    where
        I: IntoIterator<Item = (P, V)>,
        P: IntoIterator<Item = A>;
}
```

**Examples**:
```rust
// Empty map
let map: PathMap<(), u8> = PathMap::new();

// Single path
let map = PathMap::single(b"hello", ());

// From iterator
let map = PathMap::from_iter(vec![
    (b"path1".to_vec(), ()),
    (b"path2".to_vec(), ()),
]);
```

### WriteZipper<V, A>

**Definition**:
```rust
pub struct WriteZipper<'a, V, A = u8> {
    // Internal: Mutable reference to PathMap
    // Focus: Current position in trie
    // Context: Path to current position
}
```

**Purpose**: Mutable cursor into PathMap for modifications.

**Lifetime**: Borrows PathMap mutably for duration of zipper.

**Creation**:
```rust
impl<V, A> PathMap<V, A> {
    /// Creates a write zipper at the root
    pub fn write_zipper(&mut self) -> WriteZipper<V, A>;

    /// Creates a write zipper at a specific path
    pub fn write_zipper_at_path<P>(&mut self, path: P) -> WriteZipper<V, A>
    where
        P: IntoIterator<Item = A>;
}
```

**Examples**:
```rust
let mut map = PathMap::new();

// Zipper at root
let mut wz = map.write_zipper();

// Zipper at specific path
let mut wz = map.write_zipper_at_path(b"namespace/");
```

### ReadZipper<V, A>

**Definition**:
```rust
pub struct ReadZipper<'a, V, A = u8> {
    // Internal: Immutable reference to subtrie
    // Focus: Current position in trie
}
```

**Purpose**: Immutable cursor for reading and algebraic operations (as source).

**Lifetime**: Borrows PathMap or subtrie immutably.

**Creation**:
```rust
impl<V, A> PathMap<V, A> {
    /// Creates a read zipper at the root
    pub fn read_zipper(&self) -> ReadZipper<V, A>;

    /// Creates a read zipper at a specific path
    pub fn read_zipper_at_path<P>(&self, path: P) -> ReadZipper<V, A>
    where
        P: IntoIterator<Item = A>;
}
```

**Examples**:
```rust
let map = PathMap::new();

// Read zipper at root
let rz = map.read_zipper();

// Read zipper at path
let rz = map.read_zipper_at_path(b"prefix/");
```

---

## Algebraic Operations API

### join_into

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn join_into<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z
    ) -> AlgebraicStatus
    where
        V: Lattice;
}
```

**Purpose**: Unions self with read_zipper (self ∪ read_zipper).

**Parameters**:
- `self` - Modified in-place to contain union
- `read_zipper` - Source zipper (immutable)

**Returns**: `AlgebraicStatus`
- `Element` - Union contains new data (changed)
- `Identity` - Self unchanged (all paths already present)
- `None` - Both inputs empty

**Complexity**:
- Time: O(min(|self|, |read_zipper|) × log k)
- Space: O(new nodes)

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());

let mut map_b = PathMap::new();
map_b.insert(b"banana", ());

let status = map_a.write_zipper().join_into(&map_b.read_zipper());
assert_eq!(status, AlgebraicStatus::Element);
// map_a now contains {"apple", "banana"}
```

**See Also**:
- `join_map_into` - Variant accepting PathMap directly
- `join_into_take` - Consuming variant for efficiency
- `join_k_path_into` - Prefix-collapsing variant

### join_map_into

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn join_map_into(
        &mut self,
        path_map: PathMap<V, A>
    ) -> AlgebraicStatus
    where
        V: Lattice;
}
```

**Purpose**: Unions self with path_map (consumes path_map).

**Parameters**:
- `self` - Modified in-place
- `path_map` - Source PathMap (consumed)

**Returns**: `AlgebraicStatus`

**Complexity**: Same as `join_into`

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());

let map_b = PathMap::single(b"banana", ());

map_a.write_zipper().join_map_into(map_b);
// map_b is consumed (moved)
// map_a now contains {"apple", "banana"}
```

### join_into_take

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn join_into_take(
        &mut self,
        path_map: &mut PathMap<V, A>,
        swap: bool
    ) -> AlgebraicStatus
    where
        V: Lattice;
}
```

**Purpose**: Most efficient join - destructively reads path_map.

**Parameters**:
- `self` - Modified in-place
- `path_map` - Mutable reference (emptied after call)
- `swap` - If true, swaps self with path_map before joining

**Returns**: `AlgebraicStatus`

**Complexity**: Same as `join_into`, but more efficient memory reuse

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());

let mut map_b = PathMap::single(b"banana", ());

map_a.write_zipper().join_into_take(&mut map_b, false);
// map_b is now empty
// map_a contains {"apple", "banana"}
```

### join_k_path_into

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn join_k_path_into<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z,
        byte_count: usize
    ) -> AlgebraicStatus
    where
        V: Lattice;
}
```

**Purpose**: Joins while collapsing first `byte_count` bytes of paths.

**Parameters**:
- `self` - Modified in-place
- `read_zipper` - Source zipper
- `byte_count` - Number of leading bytes to strip from source paths

**Returns**: `AlgebraicStatus`

**Complexity**: O(min(|self|, |read_zipper|) × byte_count)

**Example**:
```rust
let mut target = PathMap::new();
target.insert(b"ns/path1", ());

let mut source = PathMap::new();
source.insert(b"prefix/path2", ());

// Join at "ns/" and strip "prefix/" from source
let mut wz = target.write_zipper_at_path(b"ns/");
wz.join_k_path_into(&source.read_zipper(), 7);  // Strip "prefix/"

// Result: target contains {"ns/path1", "ns/path2"}
```

### meet_into

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn meet_into<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z,
        prune: bool
    ) -> AlgebraicStatus
    where
        V: Lattice;
}
```

**Purpose**: Intersects self with read_zipper (self ∩ read_zipper).

**Parameters**:
- `self` - Modified in-place to contain intersection
- `read_zipper` - Source zipper (immutable)
- `prune` - Whether to remove dangling paths

**Returns**: `AlgebraicStatus`
- `Element` - Intersection changed self
- `Identity` - Self unchanged (self ⊆ read_zipper)
- `None` - Intersection is empty (disjoint)

**Complexity**:
- Time: O(min(|self|, |read_zipper|) × log k)
- Space: O(|intersection|) - can only shrink

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());
map_a.insert(b"banana", ());

let mut map_b = PathMap::new();
map_b.insert(b"banana", ());
map_b.insert(b"cherry", ());

let status = map_a.write_zipper().meet_into(&map_b.read_zipper(), true);
assert_eq!(status, AlgebraicStatus::Element);
// map_a now contains {"banana"}
```

### meet_k_path_into

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn meet_k_path_into<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z,
        byte_count: usize
    ) -> AlgebraicStatus
    where
        V: Lattice;
}
```

**Purpose**: Intersects while collapsing first `byte_count` bytes.

**WARNING**: Current implementation has suboptimal performance characteristics.

**Parameters**:
- `self` - Modified in-place
- `read_zipper` - Source zipper
- `byte_count` - Bytes to strip from source paths

**Returns**: `AlgebraicStatus`

**Note**: Consider using explicit iteration if performance critical.

### meet_2

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn meet_2<Z1, Z2>(
        &mut self,
        read_zipper_1: &Z1,
        read_zipper_2: &Z2,
        prune: bool
    ) -> AlgebraicStatus
    where
        Z1: ZipperSubtries<V, A>,
        Z2: ZipperSubtries<V, A>,
        V: Lattice;
}
```

**Purpose**: Ternary intersection - self ∩ read_zipper_1 ∩ read_zipper_2.

**Parameters**:
- `self` - Modified in-place
- `read_zipper_1` - First source
- `read_zipper_2` - Second source
- `prune` - Whether to remove dangling paths

**Returns**: `AlgebraicStatus`

**Complexity**: O(min(|self|, |rz1|, |rz2|) × log k) - single pass

**Example**:
```rust
let mut map = PathMap::new();
map.insert(b"a", ());
map.insert(b"b", ());
map.insert(b"c", ());

let filter1 = PathMap::from_iter(vec![(b"a".to_vec(), ()), (b"b".to_vec(), ())]);
let filter2 = PathMap::from_iter(vec![(b"b".to_vec(), ()), (b"c".to_vec(), ())]);

map.write_zipper().meet_2(&filter1.read_zipper(), &filter2.read_zipper(), true);
// map now contains {"b"} (only common element)
```

### subtract_into

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn subtract_into<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z,
        prune: bool
    ) -> AlgebraicStatus
    where
        V: DistributiveLattice;
}
```

**Purpose**: Set difference - removes paths in read_zipper from self (self ∖ read_zipper).

**Parameters**:
- `self` - Modified in-place
- `read_zipper` - Paths to remove (immutable)
- `prune` - Whether to remove dangling paths

**Trait Requirement**: `V: DistributiveLattice` (stronger than Lattice)

**Returns**: `AlgebraicStatus`
- `Element` - Paths removed (changed)
- `Identity` - No paths removed (disjoint)
- `None` - All paths removed (empty)

**Complexity**:
- Time: O(|self| × log k)
- Space: O(|difference|) - can only shrink

**Example**:
```rust
let mut map_a = PathMap::new();
map_a.insert(b"apple", ());
map_a.insert(b"banana", ());

let mut map_b = PathMap::new();
map_b.insert(b"banana", ());

let status = map_a.write_zipper().subtract_into(&map_b.read_zipper(), true);
assert_eq!(status, AlgebraicStatus::Element);
// map_a now contains {"apple"}
```

### restrict

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn restrict<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z
    ) -> AlgebraicStatus;
}
```

**Purpose**: Removes paths without corresponding prefixes in read_zipper.

**Parameters**:
- `self` - Modified in-place
- `read_zipper` - Allowed prefixes

**Returns**: `AlgebraicStatus`

**Complexity**: O(|self| × log k)

**Example**:
```rust
let mut data = PathMap::new();
data.insert(b"api/v1/users", ());
data.insert(b"api/v2/posts", ());
data.insert(b"internal/secret", ());

let allowed = PathMap::single(b"api/v1", ());

data.write_zipper().restrict(&allowed.read_zipper());
// data now contains {"api/v1/users"} only
```

### restricting

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn restricting<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z,
        stems: &mut PathMap<Z::Subtries, A>
    ) -> AlgebraicStatus
    where
        Z::Subtries: Clone;
}
```

**Purpose**: Like `restrict`, but also populates `stems` with prefix→subtrie mappings.

**Parameters**:
- `self` - Modified in-place
- `read_zipper` - Allowed prefixes
- `stems` - Output map of prefix paths to subtries

**Returns**: `AlgebraicStatus`

**Example**:
```rust
let mut data = PathMap::new();
data.insert(b"api/v1/users", ());
data.insert(b"api/v2/posts", ());

let prefixes = PathMap::from_iter(vec![
    (b"api/v1".to_vec(), ()),
    (b"api/v2".to_vec(), ()),
]);

let mut stems = PathMap::new();
data.write_zipper().restricting(&prefixes.read_zipper(), &mut stems);

// data restricted
// stems contains: {"api/v1" → subtrie(users), "api/v2" → subtrie(posts)}
```

### graft

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn graft<Z: ZipperSubtries<V, A>>(
        &mut self,
        read_zipper: &Z
    );
}
```

**Purpose**: Replaces subtrie below focus with read_zipper's subtrie.

**Parameters**:
- `self` - Position where graft occurs
- `read_zipper` - Source subtrie

**Returns**: Nothing (always succeeds)

**Complexity**:
- Time: O(1) - just updates references
- Space: O(1) - uses structural sharing

**Example**:
```rust
let mut target = PathMap::new();
target.insert(b"root/old/path", ());

let replacement = PathMap::single(b"new/path", ());

let mut wz = target.write_zipper_at_path(b"root/old");
wz.graft(&replacement.read_zipper());

// target now contains: {"root/old/new/path"}
```

### graft_map

**Signature**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    pub fn graft_map(
        &mut self,
        path_map: PathMap<V, A>
    );
}
```

**Purpose**: Like `graft`, but consumes PathMap directly.

**Parameters**:
- `self` - Position where graft occurs
- `path_map` - Source PathMap (consumed)

**Returns**: Nothing

**Complexity**: Same as `graft`

**Example**:
```rust
let mut target = PathMap::new();
target.insert(b"root/path", ());

let replacement = PathMap::single(b"new", ());

target.write_zipper_at_path(b"root").graft_map(replacement);
// replacement is consumed
```

---

## Trait Definitions

### Lattice

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:532`

**Definition**:
```rust
pub trait Lattice: Sized {
    /// Partial join (least upper bound)
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self>;

    /// Partial meet (greatest lower bound)
    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self>;

    /// In-place join
    fn join_into(&mut self, other: &Self) -> AlgebraicStatus {
        match self.pjoin(other) {
            AlgebraicResult::None => AlgebraicStatus::None,
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

    /// In-place meet
    fn meet_into(&mut self, other: &Self) -> AlgebraicStatus {
        // Similar to join_into
    }
}
```

**Axioms**:
1. Commutativity: `a.pjoin(b) = b.pjoin(a)`
2. Associativity: `(a ∨ b) ∨ c = a ∨ (b ∨ c)`
3. Idempotence: `a.pjoin(a) = a`
4. Absorption: `a ∨ (a ∧ b) = a`

**Standard Implementations**:

**Unit Type**:
```rust
impl Lattice for () {
    fn pjoin(&self, _: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT)
    }
    fn pmeet(&self, _: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT)
    }
}
```

**Option<T>**:
```rust
impl<T: Lattice> Lattice for Option<T> {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        match (self, other) {
            (None, None) => AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT),
            (Some(_), None) => AlgebraicResult::Identity(SELF_IDENT),
            (None, Some(_)) => AlgebraicResult::Identity(COUNTER_IDENT),
            (Some(a), Some(b)) => match a.pjoin(b) {
                AlgebraicResult::None => AlgebraicResult::None,
                AlgebraicResult::Identity(m) => AlgebraicResult::Identity(m),
                AlgebraicResult::Element(v) => AlgebraicResult::Element(Some(v)),
            }
        }
    }

    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
        match (self, other) {
            (None, _) | (_, None) => AlgebraicResult::None,
            (Some(a), Some(b)) => match a.pmeet(b) {
                AlgebraicResult::None => AlgebraicResult::None,
                AlgebraicResult::Identity(m) => AlgebraicResult::Identity(m),
                AlgebraicResult::Element(v) => AlgebraicResult::Element(Some(v)),
            }
        }
    }
}
```

**HashSet<T>**:
```rust
impl<T: Eq + Hash + Clone> Lattice for HashSet<T> {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        if self == other {
            return AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT);
        }
        let mut result = self.clone();
        result.extend(other.iter().cloned());
        AlgebraicResult::Element(result)
    }

    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
        let intersection: HashSet<_> = self.intersection(other).cloned().collect();
        if intersection.is_empty() {
            AlgebraicResult::None
        } else if intersection.len() == self.len() {
            AlgebraicResult::Identity(SELF_IDENT)
        } else {
            AlgebraicResult::Element(intersection)
        }
    }
}
```

### DistributiveLattice

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:602`

**Definition**:
```rust
pub trait DistributiveLattice: Lattice {
    /// Partial subtraction
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self>;

    /// In-place subtraction
    fn subtract_into(&mut self, other: &Self) -> AlgebraicStatus {
        match self.psubtract(other) {
            AlgebraicResult::None => AlgebraicStatus::None,
            AlgebraicResult::Identity(_) => AlgebraicStatus::Identity,
            AlgebraicResult::Element(v) => {
                *self = v;
                AlgebraicStatus::Element
            }
        }
    }
}
```

**Distributive Axiom**:
```
a ∧ (b ∨ c) = (a ∧ b) ∨ (a ∧ c)
a ∨ (b ∧ c) = (a ∨ b) ∧ (a ∨ c)
```

**Standard Implementations**:

**Unit Type**:
```rust
impl DistributiveLattice for () {
    fn psubtract(&self, _: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::None
    }
}
```

**Option<T>**:
```rust
impl<T: DistributiveLattice> DistributiveLattice for Option<T> {
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self> {
        match (self, other) {
            (None, _) => AlgebraicResult::None,
            (Some(_), None) => AlgebraicResult::Identity(SELF_IDENT),
            (Some(a), Some(b)) => match a.psubtract(b) {
                AlgebraicResult::None => AlgebraicResult::None,
                AlgebraicResult::Identity(m) => AlgebraicResult::Identity(m),
                AlgebraicResult::Element(v) => AlgebraicResult::Element(Some(v)),
            }
        }
    }
}
```

**HashMap<K, V>**:
```rust
impl<K, V> DistributiveLattice for HashMap<K, V>
where
    K: Eq + Hash + Clone,
    V: DistributiveLattice + Clone,
{
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self> {
        let mut result = HashMap::new();
        let mut changed = false;

        for (k, v) in self.iter() {
            match other.get(k) {
                None => {
                    result.insert(k.clone(), v.clone());
                }
                Some(other_v) => match v.psubtract(other_v) {
                    AlgebraicResult::None => {
                        changed = true;
                    }
                    AlgebraicResult::Identity(_) => {
                        result.insert(k.clone(), v.clone());
                    }
                    AlgebraicResult::Element(new_v) => {
                        result.insert(k.clone(), new_v);
                        changed = true;
                    }
                }
            }
        }

        if result.is_empty() {
            AlgebraicResult::None
        } else if !changed && result.len() == self.len() {
            AlgebraicResult::Identity(SELF_IDENT)
        } else {
            AlgebraicResult::Element(result)
        }
    }
}
```

---

## Return Types

### AlgebraicResult<V>

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:23`

**Definition**:
```rust
pub enum AlgebraicResult<V> {
    /// Operation resulted in annihilation/empty
    None,

    /// Result is identity of one or more inputs (bitmask)
    Identity(u64),

    /// New result value
    Element(V),
}
```

**Identity Bitmask Constants**:
```rust
pub const SELF_IDENT: u64 = 0x1;      // Result equals self
pub const COUNTER_IDENT: u64 = 0x2;   // Result equals counter-party
```

**Interpretation**:
```rust
match result {
    AlgebraicResult::None => {
        // Operation annihilated (e.g., empty ∩ X = empty)
    }
    AlgebraicResult::Identity(SELF_IDENT) => {
        // Result equals first operand
    }
    AlgebraicResult::Identity(COUNTER_IDENT) => {
        // Result equals second operand
    }
    AlgebraicResult::Identity(SELF_IDENT | COUNTER_IDENT) => {
        // Both operands are equal
    }
    AlgebraicResult::Element(v) => {
        // New value computed
    }
}
```

**Methods**:
```rust
impl<V> AlgebraicResult<V> {
    /// Converts to AlgebraicStatus (discards value)
    pub fn status(&self) -> AlgebraicStatus;

    /// Extracts value if Element
    pub fn into_element(self) -> Option<V>;

    /// Checks if identity
    pub fn is_identity(&self) -> bool;

    /// Checks if none
    pub fn is_none(&self) -> bool;
}
```

### AlgebraicStatus

**Location**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/ring.rs:339`

**Definition**:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgebraicStatus {
    /// Contains new output data
    Element,

    /// Unchanged from input
    Identity,

    /// Completely empty/annihilated
    None,
}
```

**Usage**:
```rust
match wz.join_into(&other.read_zipper()) {
    AlgebraicStatus::Element => {
        // Changed - perform side effects
        println!("Map was modified");
    }
    AlgebraicStatus::Identity => {
        // Unchanged - skip expensive operations
        println!("Map unchanged");
    }
    AlgebraicStatus::None => {
        // Empty result
        println!("Map is now empty");
    }
}
```

**Conversion**:
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

---

## Zipper API

### Navigation

**move_to_root**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    /// Moves focus to root
    pub fn move_to_root(&mut self);
}
```

**move_to_path**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    /// Moves focus to specified path
    pub fn move_to_path<P>(&mut self, path: P) -> bool
    where
        P: IntoIterator<Item = A>;
}
```

**move_up**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    /// Moves focus up one level
    pub fn move_up(&mut self) -> bool;
}
```

**Examples**:
```rust
let mut wz = map.write_zipper();

// Navigate to root
wz.move_to_root();

// Navigate to path
let success = wz.move_to_path(b"path/to/node");
if success {
    // Path exists
}

// Move up
wz.move_up();  // Now at "path/to"
```

### Value Operations

**get_val**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    /// Gets value at current focus
    pub fn get_val(&self) -> Option<&V>;
}
```

**set_val**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    /// Sets value at current focus
    pub fn set_val(&mut self, value: Option<V>);
}
```

**remove_val**:
```rust
impl<'a, V, A> WriteZipper<'a, V, A> {
    /// Removes value at focus, optionally pruning path
    pub fn remove_val(&mut self, prune: bool) -> bool;
}
```

**Examples**:
```rust
let mut wz = map.write_zipper();
wz.move_to_path(b"key");

// Get value
if let Some(v) = wz.get_val() {
    println!("Value: {:?}", v);
}

// Set value
wz.set_val(Some(()));

// Remove value
wz.remove_val(true);  // Prune empty path
```

### Structural Operations

**insert**:
```rust
impl<V, A> PathMap<V, A> {
    /// Inserts a path-value pair
    pub fn insert<P>(&mut self, path: P, value: V) -> Option<V>
    where
        P: IntoIterator<Item = A>;
}
```

**remove**:
```rust
impl<V, A> PathMap<V, A> {
    /// Removes a path
    pub fn remove<P>(&mut self, path: P) -> Option<V>
    where
        P: IntoIterator<Item = A>;
}
```

**get**:
```rust
impl<V, A> PathMap<V, A> {
    /// Gets value at path
    pub fn get<P>(&self, path: P) -> Option<&V>
    where
        P: IntoIterator<Item = A>;
}
```

**contains**:
```rust
impl<V, A> PathMap<V, A> {
    /// Checks if path exists
    pub fn contains<P>(&self, path: P) -> bool
    where
        P: IntoIterator<Item = A>;
}
```

**Examples**:
```rust
let mut map = PathMap::new();

// Insert
map.insert(b"key", ());

// Get
if let Some(v) = map.get(b"key") {
    println!("Found: {:?}", v);
}

// Contains
assert!(map.contains(b"key"));

// Remove
map.remove(b"key");
```

---

## Helper Methods

### node_count

**Signature**:
```rust
impl<V, A> PathMap<V, A> {
    /// Returns number of unique nodes
    pub fn node_count(&self) -> usize;
}
```

**Purpose**: Measures structural sharing.

**Example**:
```rust
let map = /* ... */;
println!("Unique nodes: {}", map.node_count());
```

### val_count

**Signature**:
```rust
impl<V, A> PathMap<V, A> {
    /// Returns number of values (paths with values)
    pub fn val_count(&self) -> usize;
}
```

**Purpose**: Counts entries.

**Complexity**: O(1) - cached

**Example**:
```rust
let map = /* ... */;
println!("Total values: {}", map.val_count());
```

### is_empty

**Signature**:
```rust
impl<V, A> PathMap<V, A> {
    /// Checks if map is empty
    pub fn is_empty(&self) -> bool;
}
```

**Example**:
```rust
if map.is_empty() {
    println!("No data");
}
```

### clone

**Signature**:
```rust
impl<V, A> Clone for PathMap<V, A> {
    /// Cheap clone via reference counting
    fn clone(&self) -> Self;
}
```

**Complexity**: O(1) - just increments refcount

**Example**:
```rust
let map1 = PathMap::new();
let map2 = map1.clone();  // O(1)
```

---

## Error Handling

### No Error Returns

**Key Property**: Algebraic operations never return `Result<_, Error>`.

**Why**: Operations are defined for all inputs (total functions).

**Status Communication**: Via `AlgebraicStatus` and `AlgebraicResult`.

**Example**:
```rust
// No error handling needed
let status = wz.join_into(&other.read_zipper());

// Check status for semantic information
match status {
    AlgebraicStatus::Element => { /* changed */ }
    AlgebraicStatus::Identity => { /* unchanged */ }
    AlgebraicStatus::None => { /* empty */ }
}
```

### Panics

**Will NOT Panic** (safe operations):
- All algebraic operations
- Navigation (returns bool on failure)
- Value access (returns Option)

**May Panic** (contract violations):
- Invalid alphabet type (non-Ord)
- Memory exhaustion (system-level)

---

## Complete Examples

### Example 1: Pattern Removal

**Scenario**: Remove all paths matching a set of patterns.

**Code**:
```rust
use pathmap::PathMap;

fn remove_patterns(space: &mut PathMap<(), u8>, patterns: Vec<Vec<u8>>) {
    // Build removal map
    let mut to_remove = PathMap::new();
    for pattern in patterns {
        to_remove.insert(pattern, ());
    }

    // Single batched removal
    let status = space.write_zipper().subtract_into(&to_remove.read_zipper(), true);

    match status {
        AlgebraicStatus::Element => {
            println!("Removed {} patterns", to_remove.val_count());
        }
        AlgebraicStatus::Identity => {
            println!("No patterns found to remove");
        }
        AlgebraicStatus::None => {
            println!("All data removed - space is empty");
        }
    }
}
```

### Example 2: Top-N Selection

**Scenario**: Keep only the N lexicographically smallest paths.

**Code**:
```rust
use pathmap::PathMap;
use std::cmp::Ordering;

fn keep_top_n(space: &mut PathMap<(), u8>, n: usize) {
    let mut top_n = PathMap::new();
    let mut max_path: Option<Vec<u8>> = None;
    let mut count = 0;

    // Collect top N
    for (path, value) in space.iter() {
        if count < n {
            top_n.insert(path.to_vec(), value.clone());
            if max_path.is_none() || path > max_path.as_ref().unwrap() {
                max_path = Some(path.to_vec());
            }
            count += 1;
        } else if let Some(ref max) = max_path {
            if path < max {
                // Remove old max
                top_n.remove(max.clone());

                // Add new path
                top_n.insert(path.to_vec(), value.clone());

                // Update max
                max_path = top_n.iter().map(|(p, _)| p.to_vec()).max();
            }
        }
    }

    // Replace space with top N
    *space = top_n;
}
```

### Example 3: Namespace Filtering

**Scenario**: Keep only paths under allowed namespaces.

**Code**:
```rust
use pathmap::PathMap;

fn filter_namespaces(data: &mut PathMap<(), u8>, allowed_prefixes: Vec<Vec<u8>>) {
    let mut prefixes = PathMap::new();
    for prefix in allowed_prefixes {
        prefixes.insert(prefix, ());
    }

    let status = data.write_zipper().restrict(&prefixes.read_zipper());

    match status {
        AlgebraicStatus::Element => {
            println!("Filtered to {} namespaces", prefixes.val_count());
        }
        AlgebraicStatus::Identity => {
            println!("All paths already under allowed namespaces");
        }
        AlgebraicStatus::None => {
            println!("No paths under allowed namespaces - data is empty");
        }
    }
}
```

### Example 4: Merging Multiple Sources

**Scenario**: Combine data from multiple sources efficiently.

**Code**:
```rust
use pathmap::PathMap;

fn merge_sources(sources: Vec<PathMap<(), u8>>) -> PathMap<(), u8> {
    let mut result = PathMap::new();
    let mut wz = result.write_zipper();

    for mut source in sources {
        // Use consuming operation for efficiency
        wz.join_into_take(&mut source, false);
    }

    drop(wz);
    result
}
```

### Example 5: Intersection of Multiple Sets

**Scenario**: Find common elements across multiple sets.

**Code**:
```rust
use pathmap::PathMap;

fn intersect_all(mut sets: Vec<PathMap<(), u8>>) -> PathMap<(), u8> {
    if sets.is_empty() {
        return PathMap::new();
    }

    // Start with first set
    let mut result = sets.remove(0);

    // Intersect with remaining sets
    for set in sets {
        let status = result.write_zipper().meet_into(&set.read_zipper(), true);

        if status == AlgebraicStatus::None {
            // Empty intersection - can terminate early
            return PathMap::new();
        }
    }

    result
}
```

---

**End of API Reference**

*For usage patterns and optimization strategies, see the companion documents: `algebraic-operations.md`, `performance-guide.md`, and `use-cases.md`.*
