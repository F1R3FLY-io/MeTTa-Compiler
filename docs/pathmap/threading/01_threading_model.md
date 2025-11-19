# PathMap Threading Model

**Purpose**: Detailed analysis of PathMap's threading model, trait bounds, and thread safety guarantees.

**Prerequisites**: Basic understanding of Rust's `Send` and `Sync` traits, thread safety concepts.

**Related Documents**:
- [Reference Counting](02_reference_counting.md) - Implementation details
- [Concurrent Access Patterns](03_concurrent_access_patterns.md) - Safe usage patterns

---

## Table of Contents

1. [Overview](#1-overview)
2. [Send and Sync Implementations](#2-send-and-sync-implementations)
3. [TrieValue Trait Bounds](#3-trievalue-trait-bounds)
4. [Design Philosophy](#4-design-philosophy)
5. [Thread Safety Mechanisms](#5-thread-safety-mechanisms)
6. [Type System Guarantees](#6-type-system-guarantees)
7. [Comparison with Standard Collections](#7-comparison-with-standard-collections)
8. [Summary](#8-summary)

---

## 1. Overview

PathMap is **designed for multi-threading from the ground up**, not merely retrofitted for thread safety. The architecture provides:

1. **Concurrent reads**: Multiple threads can read simultaneously without locks
2. **Structural sharing**: Clone operations share unmodified structure via atomic reference counting
3. **Coordinated writes**: ZipperHead enables parallel writes to disjoint paths
4. **Type safety**: Rust's type system prevents data races at compile time

### 1.1 Key Characteristics

| Property | Value | Mechanism |
|----------|-------|-----------|
| **Send** | ✅ Yes | Value type must be Send |
| **Sync** | ✅ Yes | Atomic reference counting |
| **Clone** | O(1) | Structural sharing |
| **Concurrent reads** | Lock-free | Immutable shared structure |
| **Concurrent writes** | Coordinated | ZipperHead or separate clones |
| **Data races** | ❌ Impossible | Type system + atomics |

**Quote from PathMap README**:
> "PathMap is optimized for large data sets and can be used efficiently in a multi-threaded environment."

---

## 2. Send and Sync Implementations

### 2.1 PathMap Send and Sync

**Source**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/trie_map.rs:36-37`

```rust
unsafe impl<V: Clone + Send + Sync, A: Allocator> Send for PathMap<V, A> {}
unsafe impl<V: Clone + Send + Sync, A: Allocator> Sync for PathMap<V, A> {}
```

**Analysis**:

#### Send Implementation

```rust
unsafe impl<V: Clone + Send + Sync, A: Allocator> Send for PathMap<V, A> {}
```

**Meaning**: A `PathMap<V, A>` can be **transferred** between threads (moved from one thread to another).

**Requirements**:
- Value type `V` must be `Send` (can be moved between threads)
- Value type `V` must be `Sync` (can be shared between threads)
- Allocator `A` must implement `Allocator` trait

**Why `unsafe impl`**:
The implementation is marked `unsafe` because the compiler cannot automatically verify thread safety of the internal structure. The PathMap maintainers manually verified that:

1. All internal state is either immutable or protected by atomic operations
2. Reference counting uses atomic operations (Arc-like)
3. No interior mutability without synchronization exists

**Implications**:
```rust
let map = PathMap::<u64>::new();
// Can move PathMap to another thread
thread::spawn(move || {
    // map is now owned by this thread
    let val = map.get(b"some_key");
});
// map is no longer accessible in original thread (moved)
```

#### Sync Implementation

```rust
unsafe impl<V: Clone + Send + Sync, A: Allocator> Sync for PathMap<V, A> {}
```

**Meaning**: Multiple threads can hold **references** (`&PathMap<V, A>`) to the same PathMap simultaneously.

**Requirements**: Same as Send (V must be `Clone + Send + Sync`)

**Why `unsafe impl`**:
Manual verification that:
1. All methods taking `&self` are thread-safe
2. Shared references don't allow mutation (or mutations are synchronized)
3. Internal reference counting is atomic

**Implications**:
```rust
let map = Arc::new(PathMap::<u64>::new());
let map_ref1 = Arc::clone(&map);
let map_ref2 = Arc::clone(&map);

// Multiple threads can hold references simultaneously
thread::spawn(move || {
    let val = map_ref1.get(b"key1"); // Read operation
});

thread::spawn(move || {
    let val = map_ref2.get(b"key2"); // Concurrent read operation
});
```

### 2.2 Conditional Bounds

**Important**: PathMap is only `Send + Sync` when the value type `V` is `Clone + Send + Sync`.

**Example - Thread-safe value**:
```rust
use std::sync::Arc;

// String is Clone + Send + Sync
let map = PathMap::<String>::new();  // ✅ Send + Sync

// Can share via Arc
let shared = Arc::new(map);  // ✅ Compiles
```

**Example - Non-thread-safe value**:
```rust
use std::rc::Rc;

// Rc<String> is Clone but NOT Send or Sync
let map = PathMap::<Rc<String>>::new();  // ❌ NOT Send + Sync

// Cannot share across threads
let shared = Arc::new(map);  // ❌ Compile error:
                              // "Rc<String> cannot be sent between threads safely"
```

**Compiler enforcement**:
The type system prevents using PathMap with non-thread-safe values in multi-threaded contexts:

```rust
let map = PathMap::<Rc<u64>>::new();

thread::spawn(move || {
    // ❌ Compile error:
    // `Rc<u64>` cannot be sent between threads safely
    // the trait `Send` is not implemented for `Rc<u64>`
    map.get(b"key");
});
```

---

## 3. TrieValue Trait Bounds

### 3.1 TrieValue Trait Definition

**Source**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/lib.rs:151-153`

```rust
pub trait TrieValue: Clone + Send + Sync + Unpin + 'static {}

impl<T> TrieValue for T where T: Clone + Send + Sync + Unpin + 'static {}
```

**Analysis**:

The `TrieValue` trait is a **marker trait** that bundles together all requirements for values stored in PathMap.

#### Bound: `Clone`

**Requirement**: Value must be cheaply copyable (via `clone()`).

**Rationale**:
- Structural sharing requires cloning values when creating new nodes
- Copy-on-Write (COW) requires cloning during mutation
- Iteration and query operations often return cloned values

**Example**:
```rust
// ✅ Cheap to clone
PathMap::<u64>::new()        // Implements Copy, Clone is trivial
PathMap::<String>::new()     // Clone allocates, but acceptable
PathMap::<Arc<BigData>>::new() // Clone is O(1), only increments refcount

// ⚠️ Expensive to clone (avoid if possible)
PathMap::<Vec<u8>>::new()    // Clone allocates and copies
PathMap::<HashMap<K,V>>::new() // Clone is O(n)
```

**Best practice**: For large data, wrap in `Arc`:
```rust
// Instead of:
PathMap::<LargeStruct>::new()

// Use:
PathMap::<Arc<LargeStruct>>::new()  // Clone is O(1)
```

#### Bound: `Send`

**Requirement**: Value can be moved between threads.

**Rationale**:
- PathMap can be moved between threads (implements Send)
- Values must be movable for this to be safe
- Prevents non-thread-safe types like `Rc<T>`

**Example**:
```rust
// ✅ Send types
PathMap::<String>::new()
PathMap::<Vec<u8>>::new()
PathMap::<Arc<T>>::new()

// ❌ NOT Send
PathMap::<Rc<T>>::new()          // Compile error
PathMap::<*const T>::new()       // Compile error
PathMap::<std::cell::Cell<T>>::new() // Compile error
```

#### Bound: `Sync`

**Requirement**: Value can be shared between threads (via `&T`).

**Rationale**:
- Multiple threads can read from shared PathMap
- Internal nodes contain values that may be accessed concurrently
- Prevents types with unsynchronized interior mutability

**Example**:
```rust
// ✅ Sync types
PathMap::<u64>::new()
PathMap::<String>::new()
PathMap::<Arc<T>>::new()

// ❌ NOT Sync
PathMap::<Cell<u64>>::new()      // Compile error (interior mutability without sync)
PathMap::<RefCell<T>>::new()     // Compile error
PathMap::<*mut T>::new()          // Compile error
```

**Safe alternative for interior mutability**:
```rust
use std::sync::{Arc, Mutex};

// Use synchronized interior mutability
PathMap::<Arc<Mutex<T>>>::new()  // ✅ Compiles, thread-safe
```

#### Bound: `Unpin`

**Requirement**: Value can be safely moved in memory after being pinned.

**Rationale**:
- PathMap performs internal moves during restructuring
- Most types are `Unpin` by default
- Prevents self-referential structs

**Example**:
```rust
// ✅ Unpin (default for most types)
PathMap::<String>::new()
PathMap::<Vec<u8>>::new()

// ❌ NOT Unpin (very rare)
// Types using Pin<&mut Self> in futures
```

**Note**: In practice, `Unpin` is almost never a constraint - 99.9% of types are `Unpin`.

#### Bound: `'static`

**Requirement**: Value must not contain non-static references.

**Rationale**:
- PathMap can outlive the scope where it was created
- Values stored must not reference stack data
- Prevents dangling references

**Example**:
```rust
// ✅ 'static types (no references or all refs are 'static)
PathMap::<String>::new()
PathMap::<u64>::new()
PathMap::<Vec<u8>>::new()

// ❌ NOT 'static
let s = String::from("hello");
let map = PathMap::<&str>::new();  // Compile error if 'static not satisfied
map.insert(b"key", &s);            // Lifetime issue
```

**Workaround**: Own the data instead of borrowing:
```rust
// Instead of:
PathMap::<&str>::new()

// Use:
PathMap::<String>::new()  // ✅ Owns the string data
```

### 3.2 Why These Bounds?

The combination `Clone + Send + Sync + Unpin + 'static` ensures:

1. **Memory safety**: No dangling pointers (`'static`)
2. **Thread safety**: Can be shared and sent between threads (`Send + Sync`)
3. **Structural sharing**: Can clone for COW (`Clone`)
4. **Internal operations**: Can be moved during restructuring (`Unpin`)

**These are the minimal bounds needed for safe, efficient multi-threaded operation.**

---

## 4. Design Philosophy

### 4.1 Structural Sharing Over Locking

**Traditional approach** (e.g., `Arc<RwLock<HashMap>>`):
```rust
let map = Arc::new(RwLock::new(HashMap::new()));

// Reader threads
let r = map.read().unwrap();  // ⚠️ Lock acquired, blocks on writes
let val = r.get(&key);
drop(r);                       // Lock released

// Writer threads
let mut w = map.write().unwrap(); // ⚠️ Exclusive lock, blocks ALL access
w.insert(key, value);
drop(w);                           // Lock released
```

**Problems**:
- Lock contention under high concurrency
- Readers block during writes
- Writers block all access (including other readers)

**PathMap approach** (structural sharing):
```rust
let map = Arc::new(PathMap::new());

// Reader threads - NO LOCKS
let map_ref = Arc::clone(&map);
let zipper = map_ref.read_zipper();  // ✅ Lock-free
let val = zipper.get_val();

// Writer threads - Clone for updates
let map_clone = (*map).clone();  // ✅ O(1), shares structure
let mut updated = map_clone;
updated.set_val_at(key, value);  // ✅ COW creates new nodes as needed
// Atomically swap: map = Arc::new(updated);
```

**Benefits**:
- Zero lock contention
- Reads never block
- Structural sharing minimizes memory overhead
- Copy-on-Write (COW) for efficient updates

### 4.2 Multi-Threading as First-Class Concern

PathMap's design decisions prioritize multi-threading:

1. **Atomic reference counting**: Uses `Arc`-like refcounting, not `Rc`
   - Every clone/drop is thread-safe by default
   - No need to wrap in `Arc` for the internal structure

2. **Immutable sharing**: Read-only operations don't require `&mut self`
   - Multiple threads can hold `&PathMap` safely
   - No lock needed for queries

3. **Explicit mutation**: Mutation requires either:
   - Owned `PathMap` (single-threaded)
   - `WriteZipper` (single-threaded)
   - `ZipperHead` coordination (multi-threaded)

4. **Type-driven safety**: Rust's type system prevents:
   - Data races (Send/Sync requirements)
   - Dangling references ('static bound)
   - Unsynchronized mutation (ownership system)

### 4.3 Design Quote

From PathMap's parallel benchmark (`benches/parallel.rs:6-11`):
```rust
// "These benchmarks are intended to test some parallel-processing
// patterns for editing a PathMap. The primary takeaway from these benchmarks
// is that in most scenarios, the main bottleneck is not the PathMap
// itself, but is Rust's default allocator. For parallel editing scenarios,
// prefer jemalloc as the allocator."
```

**Key insight**: The threading model is so efficient that **the allocator becomes the bottleneck**, not synchronization.

---

## 5. Thread Safety Mechanisms

### 5.1 Atomic Reference Counting

**Mechanism**: Every internal node uses atomic refcounts (like `Arc`).

**Source**: `src/trie_node.rs:2306-2769` (both Arc and slim_ptrs implementations)

```rust
// Clone increments refcount atomically
impl Clone for TrieNodeODRc {
    fn clone(&self) -> Self {
        // Atomic increment (Relaxed ordering)
        let old_count = unsafe{ &*ptr }.fetch_add(1, Relaxed);
        // ... safety checks ...
    }
}

// Drop decrements refcount atomically
impl Drop for TrieNodeODRc {
    fn drop(&mut self) {
        // Atomic decrement (Release ordering)
        let old_count = unsafe{ &*ptr }.fetch_sub(1, Release);
        if old_count == 1 {
            // Last reference, acquire fence before deleting
            let refcount = unsafe{ &*ptr }.load(Acquire);
            // ... safe to deallocate ...
        }
    }
}
```

**Thread safety**:
- Multiple threads can clone simultaneously (atomic increment)
- Multiple threads can drop simultaneously (atomic decrement)
- Last thread to drop acquires all prior writes before deallocation

**See**: [Reference Counting](02_reference_counting.md) for detailed analysis.

### 5.2 Immutability of Shared Structure

**Principle**: Nodes accessed via `&PathMap` are immutable.

**Example**:
```rust
let map = PathMap::new();
map.insert(b"key", 42);

let zipper = map.read_zipper();
// zipper can only read, not modify
let val = zipper.get_val();  // ✅ OK
// zipper.set_val(99);        // ❌ Compile error: method not available
```

**Thread safety**: Immutable data can be safely shared without synchronization.

### 5.3 Copy-on-Write for Mutations

**Mechanism**: When modifying a shared node, create a copy instead of mutating in-place.

**Source**: `src/trie_node.rs:2837-2861` (make_unique method)

```rust
pub(crate) fn make_unique(&mut self) {
    let (ptr, _tag) = self.ptr.get_raw_parts();
    // Check if we're the only owner (refcount == 1)
    if unsafe{ &*ptr }.compare_exchange(1, 0, Acquire, Relaxed).is_err() {
        // Someone else has a reference, clone the node
        let cloned_node = self.as_tagged().clone_self();
        *self = cloned_node;
    } else {
        // We're the sole owner, can mutate in-place
        unsafe{ &*ptr }.store(1, Release);
    }
}
```

**Thread safety**:
- If multiple threads hold references, modification creates a copy
- Original structure remains unchanged (other threads see consistent state)
- No locks needed (atomic refcount check)

**See**: [COW Analysis](../PATHMAP_COW_ANALYSIS.md) for detailed semantics.

### 5.4 ZipperHead Coordination

**Mechanism**: Tracks outstanding write zippers to prevent path conflicts.

**Source**: `src/zipper_head.rs` (full file)

**Guarantees**:
- No two write zippers can have overlapping paths
- Compile-time checks (type system)
- Runtime checks (path tracking in safe API)

**Example**:
```rust
let zipper_head = map.zipper_head();

// Get exclusive write zipper for path "a/b"
let wz1 = zipper_head.write_zipper_at_exclusive_path(b"a/b")?;

// Try to get overlapping zipper for "a"
let wz2 = zipper_head.write_zipper_at_exclusive_path(b"a");
// ❌ Returns Err - paths overlap!

// Non-overlapping path "c/d" is OK
let wz3 = zipper_head.write_zipper_at_exclusive_path(b"c/d")?;
// ✅ OK - disjoint paths
```

**Thread safety**: Path exclusivity prevents concurrent mutations to the same data.

**See**: [ZipperHead Pattern](06_usage_pattern_zipperhead.md) for usage details.

---

## 6. Type System Guarantees

### 6.1 Preventing Data Races at Compile Time

**Rust's ownership system + PathMap's design = Zero data races**

#### Guarantee 1: No Shared Mutable State

```rust
// ❌ Cannot compile - PathMap is not Clone + RefCell-like
let map = PathMap::new();
let ref1 = &map;
let ref2 = &map;

// Both threads hold &PathMap (shared reference)
thread::spawn(move || {
    ref1.set_val_at(b"key", 42);  // ❌ Compile error:
                                   // method requires &mut self
});

thread::spawn(move || {
    ref2.set_val_at(b"key", 99);  // ❌ Compile error
});
```

**Why it fails**: Mutation requires `&mut self`, but we only have `&self`.

**Safe alternative**:
```rust
let mut map1 = map.clone();  // Thread 1 owns this clone
let mut map2 = map.clone();  // Thread 2 owns this clone

thread::spawn(move || {
    map1.set_val_at(b"key", 42);  // ✅ OK - owns map1
});

thread::spawn(move || {
    map2.set_val_at(b"key", 99);  // ✅ OK - owns map2
});
```

#### Guarantee 2: No Unsynchronized Interior Mutability

```rust
// ❌ Cannot compile - Cell is not Sync
let map = PathMap::<Cell<u64>>::new();
let map_ref = Arc::new(map);  // ❌ Compile error:
                               // Cell<u64> cannot be shared between threads safely
                               // trait `Sync` is not implemented for `Cell<u64>`
```

**Why it fails**: `Cell` allows mutation through `&self`, but isn't thread-safe.

**Safe alternative**:
```rust
use std::sync::Mutex;

// Use synchronized interior mutability
let map = PathMap::<Mutex<u64>>::new();
let map_ref = Arc::new(map);  // ✅ OK - Mutex provides synchronization
```

#### Guarantee 3: No Dangling References

```rust
// ❌ Cannot compile - lifetime issue
fn create_map() -> PathMap<&str> {
    let s = String::from("hello");
    let map = PathMap::new();
    map.insert(b"key", &s);  // ❌ Compile error:
                              // `s` does not live long enough
    map
}  // s dropped here, but map still references it
```

**Why it fails**: `'static` bound prevents storing non-static references.

**Safe alternative**:
```rust
fn create_map() -> PathMap<String> {
    let map = PathMap::new();
    map.insert(b"key", String::from("hello"));  // ✅ OK - owns the string
    map
}
```

### 6.2 Formal Type-Level Guarantees

**Theorem**: PathMap's type system prevents all data races.

**Proof sketch**:
1. PathMap is `Sync` only if `V: Sync`
2. `Sync` means `&PathMap` can be shared between threads
3. All `&PathMap` methods are read-only (no `&mut self`)
4. Mutation requires `&mut PathMap` (exclusive ownership)
5. Rust's type system ensures only one `&mut` exists at a time
6. Therefore, no two threads can mutate the same PathMap simultaneously
7. Read-only operations on `Sync` types are always thread-safe
8. ∴ No data races possible □

**See**: [Formal Proofs](10_formal_proofs.md) for rigorous treatment.

---

## 7. Comparison with Standard Collections

### 7.1 PathMap vs HashMap

| Property | HashMap | Arc<RwLock<HashMap>> | PathMap | Arc<PathMap> |
|----------|---------|----------------------|---------|--------------|
| **Send** | ✅ | ✅ | ✅ | ✅ |
| **Sync** | ✅ (read-only) | ✅ | ✅ (read-only) | ✅ |
| **Concurrent reads** | ❌ Needs RwLock | ✅ With lock | ✅ Lock-free | ✅ Lock-free |
| **Concurrent writes** | ❌ | ✅ With exclusive lock | ✅ Via coordination | ✅ Via coordination |
| **Clone cost** | O(n) | O(1) (Arc clone) | O(1) | O(1) |
| **Update cost** | O(1) amortized | O(1) + lock | O(log n) + COW | O(log n) + COW |
| **Lock contention** | N/A | ⚠️ High under load | ✅ None | ✅ None |

**Key difference**: PathMap achieves thread-safety without locks via structural sharing and COW.

### 7.2 PathMap vs Arc<T>

| Property | Arc<T> | PathMap |
|----------|--------|---------|
| **Shared ownership** | ✅ | ✅ (internal nodes) |
| **Atomic refcount** | ✅ | ✅ |
| **Clone cost** | O(1) | O(1) |
| **Mutation** | ❌ (immutable) | ✅ (COW) |
| **Interior mutability** | Need Arc<Mutex<T>> | Built-in (COW) |
| **Structural sharing** | Whole object | Granular (per-node) |

**Key difference**: PathMap combines Arc-like sharing with granular COW for efficient updates.

### 7.3 PathMap vs im::HashMap (Immutable Data Structures)

| Property | im::HashMap | PathMap |
|----------|-------------|---------|
| **Persistent** | ✅ | ✅ |
| **Structural sharing** | ✅ | ✅ |
| **Clone cost** | O(1) | O(1) |
| **Thread-safe** | ✅ | ✅ |
| **Trie-based** | ✅ (HAMT) | ✅ (Byte trie) |
| **Path-oriented** | ❌ | ✅ |
| **Prefix queries** | ❌ | ✅ |
| **Algebraic ops** | ❌ | ✅ (join, meet, subtract) |

**Key difference**: PathMap is optimized for byte-string keys with prefix relationships (e.g., file paths, namespaces).

---

## 8. Summary

### 8.1 Threading Model Characteristics

PathMap's threading model provides:

1. **Type-driven safety**: Send/Sync bounds prevent data races at compile time
2. **Lock-free reads**: Multiple threads can query simultaneously with zero synchronization
3. **Efficient sharing**: O(1) clone with structural sharing via atomic refcounting
4. **Coordinated writes**: ZipperHead enables safe parallel updates to disjoint paths
5. **COW semantics**: Mutations create new nodes, preserving existing structure

### 8.2 Key Trait Bounds

```rust
V: Clone + Send + Sync + Unpin + 'static
```

- `Clone`: Enables structural sharing and COW
- `Send`: Allows PathMap to be moved between threads
- `Sync`: Allows shared references across threads
- `Unpin`: Allows internal moves (default for most types)
- `'static`: Prevents dangling references

### 8.3 Design Principles

1. **Structural sharing over locking** - eliminates lock contention
2. **Immutability of shared state** - enables lock-free reads
3. **Copy-on-Write for updates** - isolates mutations
4. **Type system enforcement** - prevents unsafe patterns at compile time

### 8.4 Guarantees

- ✅ **No data races**: Enforced by type system
- ✅ **No deadlocks**: No locks used
- ✅ **No race conditions**: Atomic operations + immutability
- ✅ **Memory safety**: Reference counting prevents use-after-free
- ✅ **Correct sharing**: COW ensures isolation

### 8.5 Limitations

- ❌ Value types must be `Clone + Send + Sync + Unpin + 'static`
- ❌ Cannot store references (without Arc wrapping)
- ❌ Overlapping write paths require coordination or separate clones
- ⚠️ Allocator can become bottleneck (use jemalloc for write-heavy workloads)

---

## Next Steps

- **[Reference Counting](02_reference_counting.md)**: Deep dive into atomic refcounting implementation
- **[Concurrent Access Patterns](03_concurrent_access_patterns.md)**: Safe multi-threaded usage patterns
- **[Usage Patterns](04_usage_pattern_read_only.md)**: Practical implementation guides

---

## References

1. **PathMap Source Code**:
   - Send/Sync: `src/trie_map.rs:36-37`
   - TrieValue: `src/lib.rs:151-153`
   - Atomic refcount: `src/trie_node.rs:2306-2769`
   - ZipperHead: `src/zipper_head.rs`

2. **Rust Documentation**:
   - [Send trait](https://doc.rust-lang.org/std/marker/trait.Send.html)
   - [Sync trait](https://doc.rust-lang.org/std/marker/trait.Sync.html)
   - [Nomicon: Atomics](https://doc.rust-lang.org/nomicon/atomics.html)

3. **Related PathMap Documentation**:
   - [COW Analysis](../PATHMAP_COW_ANALYSIS.md)
   - [Algebraic Operations](../PATHMAP_ALGEBRAIC_OPERATIONS.md)
   - [Main README](../README.md)
