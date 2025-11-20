# MeTTa Atom Space - Overview

## Executive Summary

This document provides a comprehensive overview of MeTTa's atom space system, which is the fundamental storage and retrieval mechanism for atoms (facts and rules) in MeTTa programs.

**Key Characteristics:**
- **Flexible Storage**: Atoms are stored as-is without interpretation
- **Efficient Indexing**: Trie-based data structure for fast queries
- **Observable**: Notification system for space modifications
- **Duplicate Handling**: Configurable strategies for duplicate atoms
- **No Atomicity**: Operations are not atomic or transactional

**Primary Operations:**
- `add-atom` - Add atoms to a space
- `remove-atom` - Remove atoms from a space
- `get-atoms` - Query atoms from a space
- `new-space` - Create new atom spaces

## What is an Atom Space?

An **atom space** is a container that stores atoms. Atoms represent:
- **Facts**: Data or assertions about the world
- **Rules**: Rewrite rules using the `=` operator
- **Functions**: Function definitions with patterns and bodies
- **Types**: Type annotations and constraints

### Specification

**Atom Space Definition:**
```
Space := { Atom₁, Atom₂, ..., Atomₙ }
```

An atom space is an unordered collection of atoms. The same atom may appear multiple times (depending on the duplication strategy).

**Conceptual Properties:**
- Spaces are first-class values that can be passed around
- Multiple spaces can coexist in a program
- Spaces can be queried using pattern matching
- Changes to spaces are observable through event notifications

### Implementation

In `hyperon-experimental`, atom spaces are implemented as:

**Primary Type** - `lib/src/space/grounding/mod.rs:56-60`:
```rust
pub struct GroundingSpace<D: DuplicationStrategy = AllowDuplication> {
    index: AtomIndex<D>,
    common: SpaceCommon,
    name: Option<String>,
}
```

**Key Components:**
- `AtomIndex`: Trie-based structure for efficient pattern matching
- `SpaceCommon`: Shared infrastructure (observers, metadata)
- `DuplicationStrategy`: Controls how duplicates are handled

## Adding Atoms

### Specification

**Syntax:**
```metta
(add-atom <space> <atom>)
```

**Semantics:**
- Adds `<atom>` to `<space>` exactly as provided (no evaluation)
- Returns `()` (unit/empty result)
- Triggers observer notifications
- Handles duplicates according to space's duplication strategy

**Formal Rule:**
```
Space ⊢ (add-atom s a) → ()
Side effect: s' = s ∪ {a}
```

### Implementation

**Location**: `lib/src/metta/runner/stdlib/space.rs:152-176`

```rust
impl CustomExecute for AddAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let space = Atom::as_gnd::<DynSpace>(space)?;
        space.borrow_mut().add(atom.clone());
        unit_result()
    }
}
```

**Key Behaviors:**
- Atom is cloned and stored as-is
- No validation of atom structure
- Trie index is updated immediately
- Observers are notified via `SpaceEvent::Add`

**Example:**
```metta
(add-atom &self (Human Socrates))
(add-atom &self (Mortal Socrates))
```

## Removing Atoms

### Specification

**Syntax:**
```metta
(remove-atom <space> <atom>)
```

**Semantics:**
- Removes one instance of `<atom>` from `<space>` using exact matching
- Returns `Bool` (true if removed, false if not found)
- Triggers observer notifications
- With duplicates, only removes one instance

**Formal Rule:**
```
Space ⊢ (remove-atom s a) → Bool
Side effect: s' = s \ {a}  (removes one instance)
```

### Implementation

**Location**: `lib/src/metta/runner/stdlib/space.rs:178-201`

```rust
impl CustomExecute for RemoveAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let space = Atom::as_gnd::<DynSpace>(space)?;
        Ok(vec![Atom::gnd(space.borrow_mut().remove(&atom))])
    }
}
```

**Key Behaviors:**
- Uses exact atom equality (not pattern matching)
- Only removes first matching instance
- Returns boolean success indicator
- Observers notified via `SpaceEvent::Remove`

**Example:**
```metta
(remove-atom &self (Human Socrates))  ; Returns True if removed
```

## Facts vs Rules

### Facts

**Definition**: A fact is any atom stored in an atom space that represents data or an assertion.

**Examples:**
```metta
(Human Socrates)           ; Fact: Socrates is human
(age John 30)              ; Fact: John's age is 30
(parent Alice Bob)         ; Fact: Alice is parent of Bob
42                         ; Fact: the number 42
"Hello"                    ; Fact: the string "Hello"
```

**Characteristics:**
- No special syntax or structure required
- Stored exactly as provided
- Retrieved via pattern matching
- No distinction from rules at storage level

### Rules

**Definition**: A rule is an atom using the `=` operator that defines a rewrite or computation.

**Syntax:**
```metta
(= <pattern> <result>)
```

**Examples:**
```metta
(= (mortal $x) (Human $x))           ; Rule: humans are mortal
(= (fib 0) 0)                        ; Rule: base case
(= (fib 1) 1)                        ; Rule: base case
(= (fib $n) (+ (fib (- $n 1))        ; Rule: recursive case
               (fib (- $n 2))))
```

**Characteristics:**
- Stored as atoms (no special treatment in storage)
- Distinguished by the `=` symbol during evaluation
- Matched against expressions during reduction
- Can be queried like any other atom

**Storage**: Both facts and rules are stored identically in the atom space:
```rust
// Both stored as Atom::Expression with Vec<Atom>
space.add(expr![sym!("Human"), sym!("Socrates")]);
space.add(expr![sym!("="), pattern, result]);
```

## Space Operations

### Querying Atoms

**Syntax:**
```metta
(get-atoms <space>)
```

**Behavior:**
- Returns all atoms in the space
- No guaranteed order
- Includes both facts and rules

**Pattern Matching:**
```metta
(match <space> <pattern> <body>)
```

**Behavior:**
- Searches space for atoms matching pattern
- Binds variables in pattern
- Evaluates body for each match

### Creating Spaces

**Syntax:**
```metta
(new-space)
```

**Behavior:**
- Creates a new, empty atom space
- Independent from other spaces
- Can be assigned to variables

**Example:**
```metta
!(bind! &my-space (new-space))
(add-atom &my-space (fact 1))
```

### Multiple Spaces

MeTTa supports multiple independent atom spaces:
- `&self` - The default space for the current module
- User-created spaces via `new-space`
- Spaces can be passed as arguments to functions

## Trie-Based Indexing

### Specification

The atom space uses a trie (prefix tree) data structure for efficient pattern matching.

**Trie Structure:**
```
Trie :=
  | LeafNode(Set<Atom>)
  | BranchNode(Map<Token, Trie>)
  | WildcardNode(Trie)
```

**Query Efficiency:**
- Variable patterns: O(n) where n = number of atoms
- Ground atoms: O(log n) average case with trie
- Partial patterns: Prunes search space effectively

### Implementation

**Location**: `hyperon-space/src/index/trie.rs:1-1400+`

**Key Components:**

1. **AtomTrie** - Root structure:
```rust
pub struct AtomTrie<D: DuplicationStrategy = AllowDuplication> {
    root: TrieNode<D>,
    storage: TrieKeyStorage,
}
```

2. **TrieNode Variants** - `hyperon-space/src/index/trie.rs:150-160`:
```rust
enum TrieNode<D> {
    Leaf(Vec<Atom>),                    // Terminal atoms
    Branch(HashMap<Token, TrieNode<D>>), // Token-indexed children
    Variable(Box<TrieNode<D>>),         // Variable pattern node
}
```

3. **Token-Based Indexing**:
   - Atoms are decomposed into tokens
   - Tokens include: symbols, numbers, grounded values, expression markers
   - Variables create wildcard branches

**Query Process** - `hyperon-space/src/index/trie.rs:450-520`:
1. Decompose query pattern into tokens
2. Traverse trie following matching tokens
3. At variable nodes, explore all branches
4. Collect atoms at matching leaf nodes

## Observer Pattern

### Specification

Atom spaces support observation of modifications via event callbacks.

**Event Types:**
```metta
SpaceEvent :=
  | Add(Atom)       ; Atom was added
  | Remove(Atom)    ; Atom was removed
  | Replace(Atom, Atom)  ; Atom was replaced (rare)
```

**Observer Protocol:**
- Observers register callbacks for space events
- Events fired synchronously after operations
- Multiple observers can watch same space
- Observers receive immutable event data

### Implementation

**Location**: `lib/src/space/grounding/mod.rs:245-270`

**SpaceObserver Trait**:
```rust
pub trait SpaceObserver {
    fn notify(&self, event: &SpaceEvent);
}
```

**Registration** - `lib/src/space/grounding/mod.rs:180-195`:
```rust
impl<D: DuplicationStrategy> GroundingSpace<D> {
    pub fn register_observer(&mut self, observer: Rc<dyn SpaceObserver>) {
        self.common.register_observer(observer);
    }
}
```

**Event Dispatch** - `lib/src/space/grounding/mod.rs:200-210`:
```rust
fn notify_observers(&self, event: SpaceEvent) {
    for observer in &self.common.observers {
        observer.notify(&event);
    }
}
```

## Duplication Strategies

### Specification

Atom spaces can handle duplicate atoms in two ways:

**AllowDuplication**:
- Same atom can be stored multiple times
- Each `add-atom` creates a new instance
- `remove-atom` removes only one instance
- Query results may include duplicates

**NoDuplication** (Set semantics):
- Each unique atom stored at most once
- Subsequent `add-atom` of same atom has no effect
- `remove-atom` removes the unique instance
- Query results have no duplicates

**Default**: `AllowDuplication`

### Implementation

**Location**: `lib/src/space/grounding/mod.rs:25-40`

**Trait Definition**:
```rust
pub trait DuplicationStrategy {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool;
    fn remove(atoms: &mut Vec<Atom>, atom: &Atom) -> bool;
}

pub struct AllowDuplication;
pub struct NoDuplication;
```

**AllowDuplication Implementation** - `lib/src/space/grounding/mod.rs:42-48`:
```rust
impl DuplicationStrategy for AllowDuplication {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool {
        atoms.push(atom);
        true
    }
}
```

**NoDuplication Implementation** - `lib/src/space/grounding/mod.rs:50-58`:
```rust
impl DuplicationStrategy for NoDuplication {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool {
        if !atoms.contains(&atom) {
            atoms.push(atom);
            true
        } else {
            false
        }
    }
}
```

## Key Characteristics

### No Atomicity

**Important**: Atom space operations are not atomic or transactional.

**Implications:**
- Multiple operations are independent
- No rollback mechanism
- No transaction boundaries
- Concurrent access requires external synchronization

**Example of Non-Atomic Behavior:**
```metta
; These are separate operations:
(add-atom &self (fact 1))
(add-atom &self (fact 2))
; If second fails, first is not rolled back
```

### No Built-in Constraints

The atom space does not enforce:
- Type constraints (atoms of any type can be added)
- Structural validation (malformed atoms accepted)
- Uniqueness constraints (except with NoDuplication)
- Referential integrity

**Validation must be done at application level.**

### Order Independence

**Query Results**: No guaranteed order for `get-atoms` or pattern matching.

**Storage Order**: Internal trie organization does not preserve insertion order.

**Implications:**
- Do not rely on order of results
- Queries may return atoms in any order
- Order may change between runs

## Performance Characteristics

### Time Complexity

**Operations:**
- `add-atom`: O(k) where k = atom size (trie path length)
- `remove-atom`: O(k) for exact match lookup
- `get-atoms`: O(n) where n = total atoms
- Pattern matching: O(n) worst case, O(m) typical where m = matches

**Trie Benefits:**
- Reduces search space for ground queries
- Shared prefixes save space and time
- Variables require full branch exploration

### Space Complexity

**Storage:**
- O(n × k) where n = atoms, k = average atom size
- Trie nodes share common prefixes
- Overhead: HashMap nodes, token storage

**Trade-offs:**
- Trie adds overhead vs. flat list
- Benefits increase with query frequency
- Best for large spaces with many queries

## Documentation Structure

This overview covers the essentials. For detailed information:

- **[01-adding-atoms.md](01-adding-atoms.md)** - Complete `add-atom` specification
- **[02-removing-atoms.md](02-removing-atoms.md)** - Complete `remove-atom` specification
- **[03-facts.md](03-facts.md)** - Facts in detail
- **[04-rules.md](04-rules.md)** - Rules in detail
- **[05-space-operations.md](05-space-operations.md)** - All space operations
- **[06-space-structure.md](06-space-structure.md)** - Internal implementation
- **[07-edge-cases.md](07-edge-cases.md)** - Special cases and gotchas
- **[examples/](examples/)** - Executable examples

## Cross-References

**Related Documentation:**
- **Order of Operations** - `../order-of-operations/02-mutation-order.md`
  - Details on mutation ordering during evaluation
- **Type System** - `../type-system/03-type-operations.md`
  - Type-related space operations
- **Evaluation** - `../order-of-operations/01-evaluation-order.md`
  - How atoms in space are used during evaluation

## Quick Reference

### Common Operations

```metta
; Add atoms
(add-atom &self (fact 1))
(add-atom &self (= (rule $x) $x))

; Remove atoms
(remove-atom &self (fact 1))

; Query all atoms
!(get-atoms &self)

; Pattern matching
!(match &self (fact $x) $x)

; Create new space
!(bind! &myspace (new-space))
(add-atom &myspace (data 42))
```

### Common Patterns

**Knowledge Base Pattern:**
```metta
; Add facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

; Add rules
(add-atom &self (= (Mortal $x) (Human $x)))

; Query
!(match &self (Mortal $who) $who)
```

**Space Isolation Pattern:**
```metta
; Separate concerns with multiple spaces
!(bind! &facts (new-space))
!(bind! &rules (new-space))

(add-atom &facts (data 1))
(add-atom &rules (= (process $x) (* $x 2)))
```

## Summary

MeTTa's atom space provides:

✅ **Flexible storage** for facts and rules
✅ **Efficient indexing** via trie structure
✅ **Observable** modifications
✅ **Configurable** duplicate handling
✅ **Simple** operations (add, remove, query)

❌ **No atomicity** or transactions
❌ **No constraints** or validation
❌ **No ordering** guarantees

The atom space is the foundation of MeTTa's knowledge representation, enabling symbolic reasoning through pattern matching over stored atoms.

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
**Status**: Complete
