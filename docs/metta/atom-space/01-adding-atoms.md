# Adding Atoms to Atom Space

## Overview

The `add-atom` operation is the primary mechanism for inserting atoms into a MeTTa atom space. This document provides comprehensive details about its behavior, implementation, and usage patterns.

## Specification

### Syntax

```metta
(add-atom <space> <atom>) → ()
```

**Parameters:**
- `<space>`: An atom space reference (e.g., `&self`, `&myspace`)
- `<atom>`: Any valid MeTTa atom (symbol, number, string, expression, etc.)

**Return Value:**
- `()` - The unit/empty value

### Formal Semantics

**Type Signature:**
```
add-atom : Space → Atom → ()
```

**Operational Semantics:**

```
Space = {a₁, a₂, ..., aₙ}
───────────────────────────────────────
(add-atom Space atom) → ()
Side effect: Space' = Space ∪ {atom}
```

**With Duplication Strategy:**

**AllowDuplication:**
```
Space = [a₁, a₂, ..., aₙ]  (list with possible duplicates)
──────────────────────────────────────────────────
(add-atom Space atom) → ()
Side effect: Space' = [a₁, a₂, ..., aₙ, atom]
```

**NoDuplication:**
```
Space = {a₁, a₂, ..., aₙ}  (set with unique elements)
atom ∉ Space
──────────────────────────────────────
(add-atom Space atom) → ()
Side effect: Space' = Space ∪ {atom}

Space = {a₁, a₂, ..., aₙ}
atom ∈ Space
─────────────────────────────────
(add-atom Space atom) → ()
Side effect: Space' = Space  (no change)
```

### Key Behaviors

**1. No Evaluation:**
The atom is stored exactly as provided, without evaluation:
```metta
; Expression is stored as-is, not evaluated
(add-atom &self (+ 1 2))
; Space now contains the expression (+ 1 2), not 3
```

**2. Immediate Effect:**
The atom is immediately available for querying after `add-atom` returns:
```metta
(add-atom &self (fact 1))
!(match &self (fact $x) $x)  ; → [1]
```

**3. Observer Notification:**
All registered observers are notified synchronously:
```
add-atom executes → atom stored → observers notified → () returned
```

**4. No Validation:**
No structural or type validation is performed:
```metta
; All of these are accepted
(add-atom &self invalid-atom-if-any)
(add-atom &self 42)
(add-atom &self (deeply (nested (expression))))
```

## Implementation

### Core Implementation

**Location**: `lib/src/metta/runner/stdlib/space.rs:152-176`

```rust
#[derive(Clone, Debug)]
pub struct AddAtomOp {
    space: DynSpace,
}

impl AddAtomOp {
    pub fn new(space: DynSpace) -> Self {
        Self { space }
    }
}

impl Grounded for AddAtomOp {
    fn type_(&self) -> Atom {
        Atom::expr([ARROW_SYMBOL, ATOM_TYPE_ATOM, UNIT_TYPE()])
    }
}

impl CustomExecute for AddAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let atom = args.get(0).ok_or("add-atom expects one argument")?;
        self.space.borrow_mut().add(atom.clone());
        unit_result()
    }
}
```

**Key Steps:**

1. **Argument Extraction** - Line 170:
   ```rust
   let atom = args.get(0).ok_or("add-atom expects one argument")?;
   ```
   - Retrieves the atom to add from arguments
   - Returns error if no argument provided

2. **Space Modification** - Line 171:
   ```rust
   self.space.borrow_mut().add(atom.clone());
   ```
   - Acquires mutable borrow of space
   - Calls `add` method with cloned atom
   - Atom is cloned to satisfy ownership requirements

3. **Return Unit** - Line 172:
   ```rust
   unit_result()
   ```
   - Returns `Ok(vec![Atom::gnd(())])`
   - Represents the empty/unit result

### Space.add() Method

**Location**: `lib/src/space/grounding/mod.rs:120-135`

```rust
impl<D: DuplicationStrategy> GroundingSpace<D> {
    pub fn add(&mut self, atom: Atom) {
        // Insert into trie index
        let added = self.index.insert(&atom);

        // Notify observers if actually added
        if added {
            let event = SpaceEvent::Add(atom);
            self.common.notify_observers(&event);
        }
    }
}
```

**Process:**

1. **Trie Insertion** - `self.index.insert(&atom)`:
   - Atom is decomposed into tokens
   - Trie is traversed/created following token path
   - Atom stored at leaf node
   - Returns `true` if added, `false` if duplicate rejected

2. **Observer Notification** - If added:
   - Creates `SpaceEvent::Add(atom)` event
   - Synchronously calls all registered observers
   - Observers receive immutable reference to event

### Trie Index Insertion

**Location**: `hyperon-space/src/index/trie.rs:250-350`

**Token Decomposition** - `hyperon-space/src/index/trie.rs:100-150`:
```rust
fn tokenize(atom: &Atom) -> Vec<Token> {
    match atom {
        Atom::Symbol(sym) => vec![Token::Symbol(sym.clone())],
        Atom::Variable(var) => vec![Token::Variable(var.clone())],
        Atom::Grounded(gnd) => vec![Token::Grounded(gnd.clone())],
        Atom::Expression(expr) => {
            let mut tokens = vec![Token::OpenParen];
            for atom in expr.children() {
                tokens.extend(tokenize(atom));
            }
            tokens.push(Token::CloseParen);
            tokens
        }
    }
}
```

**Trie Insertion Algorithm** - `hyperon-space/src/index/trie.rs:270-310`:
```rust
pub fn insert(&mut self, atom: &Atom) -> bool {
    let tokens = tokenize(atom);
    let mut node = &mut self.root;

    // Traverse trie following tokens
    for token in tokens {
        node = match node {
            TrieNode::Branch(map) => {
                map.entry(token.clone())
                   .or_insert_with(|| TrieNode::Leaf(Vec::new()))
            }
            TrieNode::Leaf(_) => {
                // Convert leaf to branch if needed
                let new_branch = TrieNode::Branch(HashMap::new());
                *node = new_branch;
                node
            }
            _ => node,
        };
    }

    // At leaf, apply duplication strategy
    match node {
        TrieNode::Leaf(atoms) => D::insert(atoms, atom.clone()),
        _ => unreachable!(),
    }
}
```

**Complexity:**
- Time: O(k) where k = number of tokens in atom
- Space: O(k) for path creation (amortized over shared prefixes)

## Duplication Handling

### AllowDuplication (Default)

**Behavior:**
- Every `add-atom` inserts a new instance
- Same atom can exist multiple times
- Useful for counting occurrences

**Example:**
```metta
(add-atom &self (fact 1))
(add-atom &self (fact 1))
(add-atom &self (fact 1))

!(match &self (fact 1) found)
; → [found, found, found]  (three results)
```

**Implementation** - `lib/src/space/grounding/mod.rs:42-48`:
```rust
impl DuplicationStrategy for AllowDuplication {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool {
        atoms.push(atom);
        true  // Always succeeds
    }
}
```

### NoDuplication (Set Semantics)

**Behavior:**
- First `add-atom` inserts the atom
- Subsequent `add-atom` of same atom ignored
- Maintains set invariant (unique elements)

**Example:**
```metta
; Assuming NoDuplication strategy
(add-atom &self (fact 1))
(add-atom &self (fact 1))
(add-atom &self (fact 1))

!(match &self (fact 1) found)
; → [found]  (one result only)
```

**Implementation** - `lib/src/space/grounding/mod.rs:50-60`:
```rust
impl DuplicationStrategy for NoDuplication {
    fn insert(atoms: &mut Vec<Atom>, atom: Atom) -> bool {
        if !atoms.contains(&atom) {
            atoms.push(atom);
            true
        } else {
            false  // Duplicate not added
        }
    }
}
```

**Equality Check:**
Uses `Atom::eq()` for exact structural equality:
- Symbols: name must match
- Numbers: value must match
- Expressions: recursive equality of all children
- Variables: name must match

## Atom Types

### Symbols

```metta
(add-atom &self hello)
(add-atom &self my-symbol)
(add-atom &self Symbol123)
```

**Storage:**
- Tokenized as `Token::Symbol(name)`
- Indexed by symbol name in trie

### Numbers

```metta
(add-atom &self 42)
(add-atom &self 3.14)
(add-atom &self -100)
```

**Storage:**
- Tokenized as `Token::Number(value)`
- Indexed by numeric value

### Strings

```metta
(add-atom &self "Hello, World!")
(add-atom &self "")
```

**Storage:**
- Tokenized as `Token::String(content)`
- Indexed by string content

### Variables

```metta
(add-atom &self $x)
(add-atom &self $MyVar)
```

**Storage:**
- Tokenized as `Token::Variable(name)`
- Creates variable branches in trie for pattern matching

### Expressions

```metta
(add-atom &self (Human Socrates))
(add-atom &self (age John 30))
(add-atom &self ((nested expression) with (complex (structure))))
```

**Storage:**
- Tokenized as sequence: `[OpenParen, ...children..., CloseParen]`
- Indexed hierarchically in trie
- Recursive structure preserved

### Grounded Atoms

```metta
; Grounded atoms (e.g., Rust objects)
(add-atom &self <grounded-value>)
```

**Storage:**
- Tokenized as `Token::Grounded(gnd)`
- Indexed by grounded value identity

## Observer Pattern

### SpaceEvent::Add

When `add-atom` succeeds, observers receive:

```rust
SpaceEvent::Add(atom: Atom)
```

**Event Contents:**
- `atom`: Immutable reference to the added atom

**Notification Timing:**
```
add-atom called
    ↓
trie insertion
    ↓
(if added)
    ↓
SpaceEvent::Add created
    ↓
observers.notify() called
    ↓
(all observers notified synchronously)
    ↓
() returned to caller
```

### Observer Implementation Example

```rust
use hyperon::space::{SpaceObserver, SpaceEvent};

struct MyObserver;

impl SpaceObserver for MyObserver {
    fn notify(&self, event: &SpaceEvent) {
        match event {
            SpaceEvent::Add(atom) => {
                println!("Atom added: {:?}", atom);
            }
            _ => {}
        }
    }
}

// Register observer
space.register_observer(Rc::new(MyObserver));

// Now all add-atom operations will trigger MyObserver
```

## Usage Patterns

### Adding Facts

```metta
; Simple facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (age Socrates 70))

; Complex facts
(add-atom &self (knows Alice Bob))
(add-atom &self (parent (Person "Alice" 30) (Person "Bob" 5)))
```

### Adding Rules

```metta
; Simple rule
(add-atom &self (= (mortal $x) (Human $x)))

; Recursive rule
(add-atom &self (= (fib 0) 0))
(add-atom &self (= (fib 1) 1))
(add-atom &self (= (fib $n)
                   (+ (fib (- $n 1))
                      (fib (- $n 2)))))
```

### Bulk Addition

```metta
; Add multiple atoms
(add-atom &self (fact 1))
(add-atom &self (fact 2))
(add-atom &self (fact 3))

; Or programmatically
!(bind! &nums (1 2 3 4 5))
!(match &nums $n
    (add-atom &self (fact $n)))
```

### Conditional Addition

```metta
; Add only if condition holds
!(if (> $x 0)
    (add-atom &self (positive $x))
    ())
```

## Performance Characteristics

### Time Complexity

**Single add-atom:**
- O(k) where k = number of tokens in atom
- Trie traversal: O(k) path following
- Duplicate check (NoDuplication): O(m) where m = atoms at leaf
- Observer notification: O(n) where n = number of observers

**Batch additions:**
- O(t × k) where t = number of atoms, k = average tokens
- Shared prefixes amortize cost

### Space Complexity

**Per atom:**
- O(k) for unique path tokens
- O(1) for atom storage at leaf
- Shared prefixes reduce overhead

**Trie growth:**
- New atoms may create new branches
- Worst case: O(t × k) total space
- Best case: O(k) for all identical atoms

### Optimization Strategies

**1. Batch Similar Atoms:**
```metta
; Good: shared prefix (Human ...)
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))
```

**2. Avoid Redundant Duplicates:**
```metta
; Use NoDuplication if uniqueness needed
; Or check before adding:
!(if (not (match &self (fact $x) $x))
    (add-atom &self (fact $x))
    ())
```

**3. Minimize Observer Count:**
- Fewer observers = faster notification
- Unregister observers when not needed

## Error Handling

### Specification Errors

**Missing Argument:**
```metta
(add-atom &self)  ; Error: expects one argument
```

**Invalid Space:**
```metta
(add-atom invalid-space (fact 1))  ; Error: not a space
```

### Implementation Behavior

**Error Result:**
```rust
Err(ExecError::from("add-atom expects one argument"))
```

**Type Error:**
If space is not a `DynSpace`:
```rust
Err(ExecError::from("Expected Space type"))
```

## Thread Safety

### Current Implementation

**Not Thread-Safe:**
- `add-atom` uses `borrow_mut()` (runtime borrow checking)
- Concurrent `add-atom` from multiple threads → panic
- No internal synchronization

**Concurrent Access:**
```rust
// This will panic if executed concurrently:
thread::spawn(|| add-atom(&space, atom1));
thread::spawn(|| add-atom(&space, atom2));
// → "already borrowed: BorrowMutError"
```

### Safe Concurrent Usage

**External Synchronization Required:**
```rust
use std::sync::Mutex;

let space = Arc::new(Mutex::new(space));

thread::spawn({
    let space = Arc::clone(&space);
    move || {
        let mut s = space.lock().unwrap();
        s.add(atom);
    }
});
```

## Edge Cases

### Empty Expressions

```metta
(add-atom &self ())  ; Adds the empty expression
```
- Valid atom
- Stored as `[]` (empty token sequence)
- Can be queried

### Nested Spaces

```metta
!(bind! &inner (new-space))
(add-atom &self &inner)  ; Adds space reference as atom
```
- Spaces are atoms themselves
- Can be stored in other spaces
- Creates nested space structure

### Self-Reference

```metta
(add-atom &self &self)  ; Adds space to itself
```
- Creates self-referential structure
- May cause issues in some queries
- Use with caution

### Large Atoms

```metta
(add-atom &self (very (deeply (nested (expression (with (many (levels))))))))
```
- No size limit enforced
- Performance degrades with depth
- Memory consumption proportional to size

## Comparison with Other Operations

### add-atom vs match

**add-atom:**
- Mutates space
- Immediate effect
- No result beyond ()

**match:**
- Queries space
- No mutation
- Returns matches

### add-atom vs remove-atom

**add-atom:**
- Increases space size (usually)
- Always succeeds (with appropriate strategy)
- Returns ()

**remove-atom:**
- Decreases space size
- May fail if atom not found
- Returns Bool

## Best Practices

### 1. Validate Before Adding

```metta
; Check preconditions
!(if (valid? $atom)
    (add-atom &self $atom)
    (print "Invalid atom"))
```

### 2. Use Appropriate Duplication Strategy

```metta
; For facts (allow duplicates):
AllowDuplication

; For unique constraints:
NoDuplication
```

### 3. Avoid Unnecessary Observers

```metta
; Register observers only when needed
; Unregister when done to improve performance
```

### 4. Document Side Effects

```metta
; Function that adds atoms should be documented:
; (: add-fact (-> Atom ()))
; Side effect: Adds atom to &self
(= (add-fact $x) (add-atom &self $x))
```

### 5. Handle Errors Gracefully

```rust
match add_atom(&space, &atom) {
    Ok(()) => println!("Added successfully"),
    Err(e) => eprintln!("Failed to add: {}", e),
}
```

## Related Operations

- **[remove-atom](02-removing-atoms.md)** - Remove atoms from space
- **[get-atoms](05-space-operations.md#get-atoms)** - Retrieve all atoms
- **[match](05-space-operations.md#match)** - Pattern matching queries
- **[new-space](05-space-operations.md#new-space)** - Create new spaces

## Summary

**add-atom characteristics:**
✅ Stores atoms exactly as provided (no evaluation)
✅ Immediate availability for queries
✅ Trie-based efficient indexing
✅ Configurable duplication handling
✅ Observable via event system

❌ No validation or constraints
❌ No atomicity with other operations
❌ Not thread-safe without external synchronization
❌ No rollback mechanism

**Key Implementation Details:**
- Location: `lib/src/metta/runner/stdlib/space.rs:152-176`
- Trie insertion: `hyperon-space/src/index/trie.rs:250-350`
- Complexity: O(k) where k = atom size
- Returns: `()` (unit value)

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
