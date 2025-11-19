# Atom Space Internal Structure

## Overview

This document provides detailed information about the internal implementation of MeTTa's atom space in the `hyperon-experimental` reference implementation. Understanding the internal structure helps with performance optimization and debugging.

## Architecture Overview

### High-Level Components

```
GroundingSpace
    ├── AtomIndex (trie-based)
    │   ├── AtomTrie
    │   │   ├── TrieNode tree
    │   │   └── TrieKeyStorage
    │   └── DuplicationStrategy
    ├── SpaceCommon
    │   ├── Observers
    │   └── Metadata
    └── Optional name
```

**Primary Components:**
1. **GroundingSpace** - Main space implementation
2. **AtomIndex** - Trie-based indexing structure
3. **SpaceCommon** - Shared infrastructure
4. **Observers** - Event notification system

## GroundingSpace

### Definition

**Location**: `lib/src/space/grounding/mod.rs:56-60`

```rust
pub struct GroundingSpace<D: DuplicationStrategy = AllowDuplication> {
    index: AtomIndex<D>,
    common: SpaceCommon,
    name: Option<String>,
}
```

**Fields:**

**index: AtomIndex<D>**
- Main storage and query structure
- Trie-based for efficient pattern matching
- Parameterized by duplication strategy

**common: SpaceCommon**
- Shared functionality across space types
- Observer management
- Metadata and state

**name: Option<String>**
- Optional human-readable name
- Used for debugging and logging
- Not required for functionality

### Implementation Methods

**Location**: `lib/src/space/grounding/mod.rs:100-300`

**Core Methods:**

```rust
impl<D: DuplicationStrategy> GroundingSpace<D> {
    // Create new empty space
    pub fn new() -> Self {
        Self {
            index: AtomIndex::new(),
            common: SpaceCommon::new(),
            name: None,
        }
    }

    // Add atom to space
    pub fn add(&mut self, atom: Atom) {
        let added = self.index.insert(&atom);
        if added {
            self.common.notify_observers(&SpaceEvent::Add(atom));
        }
    }

    // Remove atom from space
    pub fn remove(&mut self, atom: &Atom) -> bool {
        let removed = self.index.remove(atom);
        if removed {
            self.common.notify_observers(&SpaceEvent::Remove(atom.clone()));
        }
        removed
    }

    // Query atoms by pattern
    pub fn query(&self, pattern: &Atom) -> Vec<Atom> {
        self.index.query(pattern)
    }

    // Get all atoms
    pub fn get_atoms(&self) -> Vec<Atom> {
        self.index.get_all()
    }

    // Register observer
    pub fn register_observer(&mut self, observer: Rc<dyn SpaceObserver>) {
        self.common.register_observer(observer);
    }
}
```

## AtomIndex

### Definition

**Location**: `hyperon-space/src/index/mod.rs:30-50`

```rust
pub struct AtomIndex<D: DuplicationStrategy = AllowDuplication> {
    trie: AtomTrie<D>,
}
```

**Purpose:**
- Wraps AtomTrie
- Provides high-level indexing interface
- Manages duplication strategy

**Key Methods:**

```rust
impl<D: DuplicationStrategy> AtomIndex<D> {
    pub fn new() -> Self;
    pub fn insert(&mut self, atom: &Atom) -> bool;
    pub fn remove(&mut self, atom: &Atom) -> bool;
    pub fn query(&self, pattern: &Atom) -> Vec<Atom>;
    pub fn get_all(&self) -> Vec<Atom>;
}
```

## AtomTrie

### Definition

**Location**: `hyperon-space/src/index/trie.rs:50-80`

```rust
pub struct AtomTrie<D: DuplicationStrategy = AllowDuplication> {
    root: TrieNode<D>,
    storage: TrieKeyStorage,
    _phantom: PhantomData<D>,
}
```

**Fields:**

**root: TrieNode<D>**
- Root of the trie tree
- All atoms stored below this node
- Initially empty

**storage: TrieKeyStorage**
- Stores tokenized keys
- Deduplicates common tokens
- Optimizes memory usage

**_phantom: PhantomData<D>**
- Marker for duplication strategy type parameter
- Zero runtime cost

### Trie Structure

**Conceptual Model:**

```
Trie Node Types:
  1. Branch - Has child nodes indexed by tokens
  2. Leaf - Contains actual atoms
  3. Variable - Represents variable patterns (for optimizations)
```

**Example Trie:**

```metta
; Atoms in space:
(Human Socrates)
(Human Plato)
(age John 30)
```

**Resulting Trie:**

```
root (Branch)
  ├── Token::OpenParen
  │   ├── Token::Symbol("Human") (Branch)
  │   │   ├── Token::Symbol("Socrates")
  │   │   │   └── Token::CloseParen → Leaf: [(Human Socrates)]
  │   │   └── Token::Symbol("Plato")
  │   │       └── Token::CloseParen → Leaf: [(Human Plato)]
  │   └── Token::Symbol("age") (Branch)
  │       ├── Token::Symbol("John")
  │       │   ├── Token::Number(30)
  │       │       └── Token::CloseParen → Leaf: [(age John 30)]
```

**Benefits:**
- Shared prefixes save memory
- Query optimization via prefix pruning
- Efficient pattern matching

## TrieNode

### Definition

**Location**: `hyperon-space/src/index/trie.rs:150-170`

```rust
enum TrieNode<D: DuplicationStrategy> {
    Leaf(Vec<Atom>),
    Branch(HashMap<Token, Box<TrieNode<D>>>),
    Variable(Box<TrieNode<D>>),  // Special node for variables
}
```

**Variants:**

**Leaf(Vec<Atom>)**
- Terminal nodes storing actual atoms
- Vec allows multiple atoms (with AllowDuplication)
- Atoms at this node share exact token path

**Branch(HashMap<Token, Box<TrieNode<D>>>)**
- Interior nodes with children
- Indexed by Token (see below)
- Box provides indirection for recursive structure

**Variable(Box<TrieNode<D>>)**
- Represents variable patterns
- Optimizes queries with variables
- May traverse multiple branches

### Token Types

**Location**: `hyperon-space/src/index/trie.rs:90-110`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Token {
    Symbol(Symbol),
    Number(Number),
    String(String),
    Variable(Variable),
    Grounded(GroundedAtom),
    OpenParen,
    CloseParen,
}
```

**Purpose:**
- Decomposes atoms into indexable units
- Enables efficient HashMap-based child lookup
- Supports all atom types

**Tokenization Examples:**

```metta
; Atom: Socrates
; Tokens: [Symbol("Socrates")]

; Atom: 42
; Tokens: [Number(42)]

; Atom: (Human Socrates)
; Tokens: [OpenParen, Symbol("Human"), Symbol("Socrates"), CloseParen]

; Atom: (age John 30)
; Tokens: [OpenParen, Symbol("age"), Symbol("John"), Number(30), CloseParen]

; Atom: ((nested) structure)
; Tokens: [OpenParen, OpenParen, Symbol("nested"), CloseParen,
;          Symbol("structure"), CloseParen]
```

## Token Decomposition

### Tokenization Algorithm

**Location**: `hyperon-space/src/index/trie.rs:200-280`

```rust
fn tokenize(atom: &Atom) -> Vec<Token> {
    match atom {
        Atom::Symbol(sym) => vec![Token::Symbol(sym.clone())],

        Atom::Variable(var) => vec![Token::Variable(var.clone())],

        Atom::Grounded(gnd) => vec![Token::Grounded(gnd.clone())],

        Atom::Expression(expr) => {
            let mut tokens = vec![Token::OpenParen];
            for child in expr.children() {
                tokens.extend(tokenize(child));
            }
            tokens.push(Token::CloseParen);
            tokens
        }
    }
}
```

**Process:**
1. **Atomic Atoms** (Symbol, Number, etc.): Single token
2. **Expressions**: Recursive decomposition with parens
3. **Nested Structure**: Preserves hierarchy via paren tokens

**Complexity:**
- Time: O(n) where n = total atoms in expression tree
- Space: O(n) for token vector

## Trie Operations

### Insertion

**Algorithm** - `hyperon-space/src/index/trie.rs:300-380`:

```rust
pub fn insert(&mut self, atom: &Atom) -> bool {
    let tokens = tokenize(atom);
    let mut node = &mut self.root;

    // Traverse/create path following tokens
    for token in tokens.iter() {
        node = match node {
            TrieNode::Branch(map) => {
                map.entry(token.clone())
                   .or_insert_with(|| Box::new(TrieNode::Leaf(Vec::new())))
            }
            TrieNode::Leaf(_) => {
                // Convert leaf to branch if needed
                let mut new_branch = HashMap::new();
                let old_leaf = std::mem::replace(node,
                    TrieNode::Branch(new_branch));
                // Handle old leaf contents...
                node
            }
            TrieNode::Variable(child) => child,
        };
    }

    // At final node (leaf), add atom via duplication strategy
    match node {
        TrieNode::Leaf(atoms) => D::insert(atoms, atom.clone()),
        _ => {
            // Convert to leaf if needed
            *node = TrieNode::Leaf(vec![atom.clone()]);
            true
        }
    }
}
```

**Steps:**
1. Tokenize atom
2. Traverse trie following tokens
3. Create branches as needed
4. At leaf, apply duplication strategy
5. Return success boolean

**Complexity:**
- Time: O(k) where k = number of tokens
- Space: O(k) amortized (shared prefixes)

### Query

**Algorithm** - `hyperon-space/src/index/trie.rs:450-580`:

```rust
pub fn query(&self, pattern: &Atom) -> Vec<Atom> {
    let tokens = tokenize(pattern);
    let mut results = Vec::new();
    self.query_recursive(&self.root, &tokens, 0, &mut results);
    results
}

fn query_recursive(
    &self,
    node: &TrieNode<D>,
    tokens: &[Token],
    index: usize,
    results: &mut Vec<Atom>
) {
    // Base case: consumed all tokens
    if index >= tokens.len() {
        if let TrieNode::Leaf(atoms) = node {
            results.extend(atoms.iter().cloned());
        }
        return;
    }

    let token = &tokens[index];

    match node {
        TrieNode::Branch(map) => {
            match token {
                Token::Variable(_) => {
                    // Variable matches all branches
                    for child in map.values() {
                        self.query_recursive(child, tokens, index + 1, results);
                    }
                }
                _ => {
                    // Ground token: follow exact match
                    if let Some(child) = map.get(token) {
                        self.query_recursive(child, tokens, index + 1, results);
                    }
                }
            }
        }
        TrieNode::Leaf(atoms) => {
            // Leaf reached early (shouldn't happen if well-formed)
        }
        TrieNode::Variable(child) => {
            // Follow variable node
            self.query_recursive(child, tokens, index, results);
        }
    }
}
```

**Steps:**
1. Tokenize pattern
2. Recursively traverse trie
3. Ground tokens: follow exact branch
4. Variables: explore all branches
5. At leaves: collect atoms
6. Return all matches

**Complexity:**
- Best case (all ground): O(k) where k = tokens
- Worst case (all variables): O(n × k) where n = atoms
- Typical: O(m × k) where m = matching atoms

### Removal

**Algorithm** - `hyperon-space/src/index/trie.rs:600-700`:

```rust
pub fn remove(&mut self, atom: &Atom) -> bool {
    let tokens = tokenize(atom);
    self.remove_recursive(&mut self.root, &tokens, 0)
}

fn remove_recursive(
    &mut self,
    node: &mut TrieNode<D>,
    tokens: &[Token],
    index: usize
) -> bool {
    if index >= tokens.len() {
        // At leaf: remove atom
        if let TrieNode::Leaf(atoms) = node {
            return D::remove(atoms, atom);
        }
        return false;
    }

    let token = &tokens[index];

    match node {
        TrieNode::Branch(map) => {
            if let Some(child) = map.get_mut(token) {
                let removed = self.remove_recursive(child, tokens, index + 1);

                // Cleanup: remove empty child branches
                if removed && child_is_empty(child) {
                    map.remove(token);
                }

                removed
            } else {
                false  // Path doesn't exist
            }
        }
        _ => false,
    }
}
```

**Steps:**
1. Tokenize atom
2. Traverse exact path
3. At leaf: remove via duplication strategy
4. Cleanup: remove empty branches
5. Return success boolean

**Complexity:**
- Time: O(k + m) where k = tokens, m = atoms at leaf
- Space: O(1) in-place removal

## SpaceCommon

### Definition

**Location**: `lib/src/space/grounding/mod.rs:20-45`

```rust
pub struct SpaceCommon {
    observers: Vec<Rc<dyn SpaceObserver>>,
    // ... other shared state
}
```

**Purpose:**
- Shared infrastructure across space types
- Observer management
- Common metadata and state

**Key Methods:**

```rust
impl SpaceCommon {
    pub fn new() -> Self;

    pub fn register_observer(&mut self, observer: Rc<dyn SpaceObserver>);

    pub fn notify_observers(&self, event: &SpaceEvent);
}
```

### Observer System

**SpaceObserver Trait** - `lib/src/space/grounding/mod.rs:245-260`:

```rust
pub trait SpaceObserver {
    fn notify(&self, event: &SpaceEvent);
}
```

**SpaceEvent Enum** - `lib/src/space/grounding/mod.rs:262-268`:

```rust
#[derive(Clone, Debug)]
pub enum SpaceEvent {
    Add(Atom),
    Remove(Atom),
    Replace(Atom, Atom),  // (old, new)
}
```

**Notification Process:**

```rust
impl SpaceCommon {
    pub fn notify_observers(&self, event: &SpaceEvent) {
        for observer in &self.observers {
            observer.notify(event);
        }
    }
}
```

**Characteristics:**
- Synchronous notification
- All observers called in sequence
- Observers receive immutable reference
- No guaranteed order

## Duplication Strategies

### DuplicationStrategy Trait

**Location**: `lib/src/space/grounding/mod.rs:25-35`

```rust
pub trait DuplicationStrategy {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool;
    fn remove(atoms: &mut Vec<Atom>, atom: &Atom) -> bool;
}
```

**Purpose:**
- Abstract insertion/removal behavior
- Enables different duplicate handling
- Compile-time selection (zero-cost abstraction)

### AllowDuplication

**Implementation** - `lib/src/space/grounding/mod.rs:42-50`:

```rust
pub struct AllowDuplication;

impl DuplicationStrategy for AllowDuplication {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool {
        atoms.push(atom);
        true
    }

    fn remove(atoms: &mut Vec<Atom>, atom: &Atom) -> bool {
        if let Some(pos) = atoms.iter().position(|a| a == atom) {
            atoms.remove(pos);
            true
        } else {
            false
        }
    }
}
```

**Behavior:**
- Insert: Always succeeds, appends to vec
- Remove: Removes first matching instance
- Duplicates: Allowed and common

### NoDuplication

**Implementation** - `lib/src/space/grounding/mod.rs:52-65`:

```rust
pub struct NoDuplication;

impl DuplicationStrategy for NoDuplication {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool {
        if !atoms.contains(&atom) {
            atoms.push(atom);
            true
        } else {
            false
        }
    }

    fn remove(atoms: &mut Vec<Atom>, atom: &Atom) -> bool {
        if let Some(pos) = atoms.iter().position(|a| a == atom) {
            atoms.remove(pos);
            true
        } else {
            false
        }
    }
}
```

**Behavior:**
- Insert: Succeeds only if not already present
- Remove: Identical to AllowDuplication (at most one instance)
- Duplicates: Prevented during insertion

## Memory Layout

### Size Estimates

**GroundingSpace:**
```rust
size_of::<GroundingSpace<AllowDuplication>>()
// = size_of::<AtomIndex>() + size_of::<SpaceCommon>() + size_of::<Option<String>>()
// ≈ pointer + vec + hashmap metadata + option
// ≈ 64-128 bytes base overhead
```

**TrieNode:**
```rust
enum TrieNode {
    Leaf(Vec<Atom>),              // 24 bytes (vec: ptr + len + cap)
    Branch(HashMap<...>),          // 48+ bytes (hashmap overhead)
    Variable(Box<TrieNode>),      // 8 bytes (boxed pointer)
}
// + tag: typically 8 bytes (enum discriminant + padding)
```

**Per Atom Storage:**
- Atom itself: 16-32 bytes (depending on type)
- Trie path overhead: Amortized across shared prefixes
- Leaf vector overhead: 24 bytes + atom pointers

**Example:**
```metta
; 1000 atoms like (Human $name)
; Shared prefix: [OpenParen, Symbol("Human")]
; Space usage:
;   - Shared path: ~200 bytes
;   - 1000 unique name paths: ~50 KB
;   - 1000 atom structures: ~24 KB
;   - Total: ~75 KB (vs ~200 KB for flat list)
```

## Performance Characteristics

### Time Complexity

**Operations:**

| Operation | Best Case | Average Case | Worst Case |
|-----------|-----------|--------------|------------|
| insert | O(k) | O(k) | O(k) |
| remove | O(k) | O(k + m) | O(k + m) |
| query (ground) | O(k) | O(k) | O(k) |
| query (variables) | O(k × m) | O(k × m) | O(n × k) |
| get_all | O(n) | O(n) | O(n) |

Where:
- k = number of tokens in atom/pattern
- m = number of matching atoms
- n = total atoms in space

### Space Complexity

**Storage:**
- Base: O(n × k) for n atoms with average k tokens
- With shared prefixes: O(n × k / s) where s = sharing factor
- Typical sharing: 2-5× reduction for structured data

**Query:**
- Temporary: O(m) for m matching atoms (result vector)

## Optimizations

### Trie Benefits

**1. Prefix Sharing:**
- Common prefixes stored once
- Saves memory proportional to sharing

**2. Query Pruning:**
- Ground tokens eliminate branches early
- Reduces search space significantly

**3. Indexing:**
- HashMap-based child lookup: O(1) average
- Faster than linear search

### Potential Improvements

**1. Path Compression:**
- Collapse chains of single-child nodes
- Reduces depth and memory

**2. Lazy Deletion:**
- Mark nodes as deleted instead of removing
- Amortize cleanup cost

**3. Bloom Filters:**
- Quick existence checks
- Avoid trie traversal for non-existent atoms

**4. Concurrent Access:**
- Reader-writer locks
- Lock-free data structures
- Currently not implemented (single-threaded)

## Implementation Files

### Key Source Files

**Space Implementation:**
- `lib/src/space/grounding/mod.rs` - GroundingSpace
- `hyperon-space/src/index/mod.rs` - AtomIndex
- `hyperon-space/src/index/trie.rs` - AtomTrie

**Supporting:**
- `lib/src/atom/mod.rs` - Atom types
- `lib/src/space/mod.rs` - Space trait definitions

### Line References

**GroundingSpace:**
- Definition: `lib/src/space/grounding/mod.rs:56-60`
- Methods: `lib/src/space/grounding/mod.rs:100-300`

**AtomTrie:**
- Definition: `hyperon-space/src/index/trie.rs:50-80`
- Insertion: `hyperon-space/src/index/trie.rs:300-380`
- Query: `hyperon-space/src/index/trie.rs:450-580`
- Removal: `hyperon-space/src/index/trie.rs:600-700`

**TrieNode:**
- Definition: `hyperon-space/src/index/trie.rs:150-170`
- Operations: `hyperon-space/src/index/trie.rs:200-800`

## Related Documentation

- **[Overview](00-overview.md)** - High-level atom space concepts
- **[Adding Atoms](01-adding-atoms.md)** - Using the implementation
- **[Removing Atoms](02-removing-atoms.md)** - Removal operations
- **[Space Operations](05-space-operations.md)** - All operations

## Summary

**Internal Structure:**
- **GroundingSpace** - Main container
- **AtomTrie** - Trie-based indexing
- **TrieNode** - Branch/Leaf/Variable nodes
- **Tokens** - Atom decomposition units
- **Observers** - Event notification

**Key Algorithms:**
- **Tokenization** - O(k) decomposition
- **Insertion** - O(k) trie traversal
- **Query** - O(k) to O(n×k) depending on pattern
- **Removal** - O(k + m) with cleanup

**Design Benefits:**
✅ Efficient pattern matching
✅ Memory savings via prefix sharing
✅ Scalable to large atom sets
✅ Extensible observer system
✅ Configurable duplication handling

**Trade-offs:**
- Memory overhead for trie structure
- Complexity vs simple flat list
- Single-threaded (no concurrency)

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
