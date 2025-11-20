# Removing Atoms from Atom Space

## Overview

The `remove-atom` operation removes atoms from a MeTTa atom space using exact equality matching. This document provides comprehensive details about its behavior, implementation, and usage patterns.

## Specification

### Syntax

```metta
(remove-atom <space> <atom>) → Bool
```

**Parameters:**
- `<space>`: An atom space reference (e.g., `&self`, `&myspace`)
- `<atom>`: The exact atom to remove

**Return Value:**
- `True` - Atom was found and removed
- `False` - Atom was not found in the space

### Formal Semantics

**Type Signature:**
```
remove-atom : Space → Atom → Bool
```

**Operational Semantics:**

**Successful Removal:**
```
atom ∈ Space
Space = {..., atom, ...}
─────────────────────────────────
(remove-atom Space atom) → True
Side effect: Space' = Space \ {atom}
```

**Failed Removal:**
```
atom ∉ Space
─────────────────────────────────
(remove-atom Space atom) → False
Side effect: Space' = Space  (no change)
```

**With Duplicates (AllowDuplication):**
```
Space = [..., atom, ..., atom, ...]  (atom appears multiple times)
──────────────────────────────────────────────────────
(remove-atom Space atom) → True
Side effect: Space' = [..., atom, ...]  (removes one instance)
```

### Key Behaviors

**1. Exact Equality Matching:**
The atom must match exactly (structural equality):
```metta
(add-atom &self (fact 1))
(remove-atom &self (fact 1))      ; → True (exact match)
(remove-atom &self (fact 2))      ; → False (no match)
(remove-atom &self (fact $x))     ; → False ($x ≠ 1)
```

**2. Single Instance Removal:**
Only one instance is removed per call:
```metta
(add-atom &self (fact 1))
(add-atom &self (fact 1))
(add-atom &self (fact 1))

(remove-atom &self (fact 1))  ; → True (removes one)
!(match &self (fact 1) found) ; → [found, found] (two remain)
```

**3. No Pattern Matching:**
Variables and patterns are NOT matched:
```metta
(add-atom &self (Human Socrates))
(remove-atom &self (Human $x))    ; → False (literal $x ≠ Socrates)
```

**4. Observer Notification:**
Observers notified only on successful removal:
```
remove-atom called
    ↓
find exact match
    ↓
(if found) remove from trie
    ↓
observers notified (SpaceEvent::Remove)
    ↓
True returned
```

## Implementation

### Core Implementation

**Location**: `lib/src/metta/runner/stdlib/space.rs:178-201`

```rust
#[derive(Clone, Debug)]
pub struct RemoveAtomOp {
    space: DynSpace,
}

impl RemoveAtomOp {
    pub fn new(space: DynSpace) -> Self {
        Self { space }
    }
}

impl Grounded for RemoveAtomOp {
    fn type_(&self) -> Atom {
        Atom::expr([ARROW_SYMBOL, ATOM_TYPE_ATOM, BOOL_TYPE])
    }
}

impl CustomExecute for RemoveAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let atom = args.get(0).ok_or("remove-atom expects one argument")?;
        let removed = self.space.borrow_mut().remove(atom);
        Ok(vec![Atom::gnd(removed)])
    }
}
```

**Key Steps:**

1. **Argument Extraction** - Line 197:
   ```rust
   let atom = args.get(0).ok_or("remove-atom expects one argument")?;
   ```
   - Retrieves the atom to remove from arguments
   - Returns error if no argument provided

2. **Space Modification** - Line 198:
   ```rust
   let removed = self.space.borrow_mut().remove(atom);
   ```
   - Acquires mutable borrow of space
   - Calls `remove` method with atom reference
   - Returns boolean indicating success

3. **Return Bool** - Line 199:
   ```rust
   Ok(vec![Atom::gnd(removed)])
   ```
   - Wraps boolean in grounded atom
   - Returns as single-element vector

### Space.remove() Method

**Location**: `lib/src/space/grounding/mod.rs:140-155`

```rust
impl<D: DuplicationStrategy> GroundingSpace<D> {
    pub fn remove(&mut self, atom: &Atom) -> bool {
        // Remove from trie index
        let removed = self.index.remove(atom);

        // Notify observers if actually removed
        if removed {
            let event = SpaceEvent::Remove(atom.clone());
            self.common.notify_observers(&event);
        }

        removed
    }
}
```

**Process:**

1. **Trie Removal** - `self.index.remove(atom)`:
   - Searches trie for exact atom match
   - Removes one instance if found
   - Returns `true` if removed, `false` if not found

2. **Observer Notification** - If removed:
   - Creates `SpaceEvent::Remove(atom)` event
   - Synchronously calls all registered observers
   - Observers receive clone of removed atom

3. **Return Success Flag**:
   - `true` - Atom was found and removed
   - `false` - Atom not found

### Trie Index Removal

**Location**: `hyperon-space/src/index/trie.rs:350-450`

**Removal Algorithm:**
```rust
pub fn remove(&mut self, atom: &Atom) -> bool {
    let tokens = tokenize(atom);
    let mut path: Vec<&mut TrieNode<D>> = Vec::new();
    let mut node = &mut self.root;

    // Traverse trie following exact token path
    for token in &tokens {
        match node {
            TrieNode::Branch(map) => {
                if let Some(child) = map.get_mut(token) {
                    path.push(node);
                    node = child;
                } else {
                    return false;  // Path doesn't exist
                }
            }
            _ => return false,
        }
    }

    // At leaf, attempt removal via duplication strategy
    match node {
        TrieNode::Leaf(atoms) => {
            let removed = D::remove(atoms, atom);

            // Clean up empty nodes if needed
            if removed && atoms.is_empty() {
                self.cleanup_empty_path(&mut path);
            }

            removed
        }
        _ => false,
    }
}
```

**Key Aspects:**

1. **Exact Token Matching:**
   - Must follow exact token path
   - Any deviation returns `false`
   - No wildcard or variable matching

2. **Leaf Node Removal:**
   - Delegates to `DuplicationStrategy::remove`
   - Removes first matching instance

3. **Cleanup:**
   - If leaf becomes empty, may clean up parent nodes
   - Prevents trie from growing indefinitely with empty branches

**Complexity:**
- Time: O(k + m) where k = tokens in atom, m = atoms at leaf
- Space: O(1) (in-place removal)

## Equality Matching

### Atom Equality

**Definition**: Two atoms are equal if they are structurally identical.

**Equality Rules:**

**Symbols:**
```rust
Atom::Symbol(s1) == Atom::Symbol(s2)  ⟺  s1.name == s2.name
```

Example:
```metta
(add-atom &self hello)
(remove-atom &self hello)  ; → True (names match)
(remove-atom &self Hello)  ; → False (case-sensitive)
```

**Numbers:**
```rust
Atom::Number(n1) == Atom::Number(n2)  ⟺  n1.value == n2.value
```

Example:
```metta
(add-atom &self 42)
(remove-atom &self 42)    ; → True
(remove-atom &self 42.0)  ; → False (different types)
```

**Strings:**
```rust
Atom::String(s1) == Atom::String(s2)  ⟺  s1 == s2
```

Example:
```metta
(add-atom &self "hello")
(remove-atom &self "hello")  ; → True
(remove-atom &self "Hello")  ; → False
```

**Variables:**
```rust
Atom::Variable(v1) == Atom::Variable(v2)  ⟺  v1.name == v2.name
```

Example:
```metta
(add-atom &self $x)
(remove-atom &self $x)  ; → True (same variable name)
(remove-atom &self $y)  ; → False
```

**Expressions:**
```rust
Atom::Expression(e1) == Atom::Expression(e2)  ⟺
    e1.len() == e2.len() ∧
    ∀i. e1[i] == e2[i]  (recursive)
```

Example:
```metta
(add-atom &self (Human Socrates))
(remove-atom &self (Human Socrates))  ; → True
(remove-atom &self (Human Plato))     ; → False
(remove-atom &self (Mortal Socrates)) ; → False
```

**Grounded Atoms:**
```rust
Atom::Grounded(g1) == Atom::Grounded(g2)  ⟺  g1.eq(g2)
```
- Delegated to grounded value's equality implementation

### Non-Matching Examples

**Pattern vs Ground:**
```metta
(add-atom &self (fact 42))
(remove-atom &self (fact $x))  ; → False (pattern ≠ ground)
```

**Different Structure:**
```metta
(add-atom &self (a b c))
(remove-atom &self (a (b c)))  ; → False (different nesting)
```

**Order Matters:**
```metta
(add-atom &self (parent Alice Bob))
(remove-atom &self (parent Bob Alice))  ; → False (order differs)
```

## Duplication Handling

### AllowDuplication (Default)

**Behavior:**
- Removes first matching instance
- Other instances remain
- Returns `true` if any instance removed

**Example:**
```metta
; Add three instances
(add-atom &self (fact 1))
(add-atom &self (fact 1))
(add-atom &self (fact 1))

; Remove one at a time
(remove-atom &self (fact 1))  ; → True (2 remain)
(remove-atom &self (fact 1))  ; → True (1 remains)
(remove-atom &self (fact 1))  ; → True (0 remain)
(remove-atom &self (fact 1))  ; → False (none left)
```

**Implementation** - `lib/src/space/grounding/mod.rs:42-48`:
```rust
impl DuplicationStrategy for AllowDuplication {
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

**Process:**
1. Linear search for first matching atom
2. Remove via `Vec::remove(pos)`
3. Shifts remaining elements down
4. Returns success

**Complexity:**
- Time: O(m) where m = atoms at leaf
- Space: O(1)

### NoDuplication (Set Semantics)

**Behavior:**
- At most one instance exists
- First `remove-atom` succeeds
- Subsequent `remove-atom` fails

**Example:**
```metta
; Assuming NoDuplication strategy
(add-atom &self (fact 1))
(add-atom &self (fact 1))  ; No effect (duplicate)

(remove-atom &self (fact 1))  ; → True (removed unique instance)
(remove-atom &self (fact 1))  ; → False (already gone)
```

**Implementation** - `lib/src/space/grounding/mod.rs:50-60`:
```rust
impl DuplicationStrategy for NoDuplication {
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

**Note**: Implementation is identical to `AllowDuplication` because uniqueness is enforced during insertion, not removal.

## Observer Pattern

### SpaceEvent::Remove

When `remove-atom` succeeds, observers receive:

```rust
SpaceEvent::Remove(atom: Atom)
```

**Event Contents:**
- `atom`: Clone of the removed atom

**Notification Timing:**
```
remove-atom called
    ↓
search for exact match
    ↓
(if found)
    ↓
remove from trie
    ↓
SpaceEvent::Remove created
    ↓
observers.notify() called
    ↓
(all observers notified synchronously)
    ↓
True returned to caller
```

**No Notification:**
If atom not found, observers are NOT notified.

### Observer Implementation Example

```rust
use hyperon::space::{SpaceObserver, SpaceEvent};

struct RemovalLogger;

impl SpaceObserver for RemovalLogger {
    fn notify(&self, event: &SpaceEvent) {
        match event {
            SpaceEvent::Remove(atom) => {
                println!("Removed: {:?}", atom);
            }
            _ => {}
        }
    }
}

// Register observer
space.register_observer(Rc::new(RemovalLogger));

// Now all successful remove-atom operations will log
```

## Usage Patterns

### Removing Facts

```metta
; Remove specific fact
(add-atom &self (Human Socrates))
(remove-atom &self (Human Socrates))  ; → True

; Attempt to remove non-existent fact
(remove-atom &self (Human Plato))  ; → False
```

### Removing Rules

```metta
; Add and remove rule
(add-atom &self (= (mortal $x) (Human $x)))
(remove-atom &self (= (mortal $x) (Human $x)))  ; → True

; Must match exactly
(remove-atom &self (= (mortal $y) (Human $y)))  ; → False ($y ≠ $x)
```

### Conditional Removal

```metta
; Remove only if exists
!(if (remove-atom &self (fact $x))
    (print "Removed fact")
    (print "Fact not found"))
```

### Bulk Removal

```metta
; Remove multiple atoms
(remove-atom &self (fact 1))
(remove-atom &self (fact 2))
(remove-atom &self (fact 3))

; Or programmatically
!(match &self (to-remove $x)
    (remove-atom &self (fact $x)))
```

### Remove and Replace

```metta
; Atomic-like removal and replacement
!(if (remove-atom &self (old-fact))
    (add-atom &self (new-fact))
    ())
```

**Note**: Not truly atomic - separate operations.

## Performance Characteristics

### Time Complexity

**Single remove-atom:**
- Trie traversal: O(k) where k = tokens in atom
- Leaf search: O(m) where m = atoms at leaf
- Observer notification: O(n) where n = observers
- Total: O(k + m + n)

**Worst Case:**
- Many duplicates at same leaf: O(k + m)
- Large atom (many tokens): O(k)

**Best Case:**
- Atom not in space: O(k) (early return)

### Space Complexity

**Removal:**
- In-place operation: O(1)
- No additional allocations

**Cleanup:**
- May deallocate empty trie nodes
- Reduces overall space usage over time

### Optimization Strategies

**1. Cache Removal Targets:**
```metta
; If removing many instances, store reference
!(bind! &to-remove (fact 42))
(remove-atom &self &to-remove)
(remove-atom &self &to-remove)
```

**2. Batch Removals:**
```metta
; Group related removals together
(remove-atom &self (fact 1))
(remove-atom &self (fact 2))
(remove-atom &self (fact 3))
```

**3. Check Before Removing:**
```metta
; Avoid unnecessary trie traversals
!(if (match &self $atom $atom)  ; Check existence
    (remove-atom &self $atom)
    False)
```

## Error Handling

### Specification Errors

**Missing Argument:**
```metta
(remove-atom &self)  ; Error: expects one argument
```

**Invalid Space:**
```metta
(remove-atom not-a-space (fact 1))  ; Error: not a space
```

### Implementation Behavior

**Error Result:**
```rust
Err(ExecError::from("remove-atom expects one argument"))
```

**Type Error:**
If space is not a `DynSpace`:
```rust
Err(ExecError::from("Expected Space type"))
```

**No Error on Not Found:**
```metta
(remove-atom &self (nonexistent atom))  ; → False (not an error)
```

## Thread Safety

### Current Implementation

**Not Thread-Safe:**
- Uses `borrow_mut()` (runtime borrow checking)
- Concurrent `remove-atom` → panic
- No internal synchronization

**Concurrent Access:**
```rust
// This will panic if executed concurrently:
thread::spawn(|| remove_atom(&space, atom1));
thread::spawn(|| remove_atom(&space, atom2));
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
        s.remove(&atom);
    }
});
```

## Edge Cases

### Removing Empty Expression

```metta
(add-atom &self ())
(remove-atom &self ())  ; → True
```
- Valid operation
- Removes empty expression atom

### Removing Space References

```metta
!(bind! &inner (new-space))
(add-atom &self &inner)
(remove-atom &self &inner)  ; → True
```
- Spaces can be removed like any atom
- Does not delete the space itself, just the reference

### Removing Variables

```metta
(add-atom &self $x)
(remove-atom &self $x)  ; → True
```
- Variables are atoms and can be removed
- Must match exact variable name

### Removing from Empty Space

```metta
(remove-atom &self (anything))  ; → False
```
- Always returns `false`
- No side effects

### Multiple Identical Rules

```metta
(add-atom &self (= (f $x) $x))
(add-atom &self (= (f $x) $x))  ; Duplicate (if AllowDuplication)

(remove-atom &self (= (f $x) $x))  ; → True (removes one)
!(match &self (= (f $x) $x) found) ; → [found] (one remains)
```

## Comparison with Other Operations

### remove-atom vs add-atom

**remove-atom:**
- Decreases space size
- May fail (returns False)
- Returns Bool

**add-atom:**
- Increases space size
- Always succeeds (with appropriate strategy)
- Returns ()

### remove-atom vs match

**remove-atom:**
- Exact equality only
- Mutates space
- Returns Bool

**match:**
- Pattern matching with variables
- No mutation
- Returns all matches

**Example:**
```metta
(add-atom &self (Human Socrates))

; match can use patterns
!(match &self (Human $x) $x)  ; → [Socrates]

; remove-atom cannot
(remove-atom &self (Human $x))  ; → False (no literal $x)
(remove-atom &self (Human Socrates))  ; → True (exact match)
```

## Pattern-Based Removal (Not Supported)

MeTTa does not provide built-in pattern-based removal. To remove atoms matching a pattern:

**Manual Pattern Removal:**
```metta
; 1. Find matches
!(match &self (to-remove $x)
    ; 2. Remove each exact match
    (remove-atom &self (to-remove $x)))
```

**Limitations:**
- Requires explicit match → remove loop
- Not atomic (match and remove are separate)
- Must construct exact atom for removal

## Best Practices

### 1. Check Return Value

```metta
; Verify removal succeeded
!(if (remove-atom &self (fact 1))
    (print "Removed successfully")
    (print "Atom not found"))
```

### 2. Remove with Exact Atoms

```metta
; Store exact atom for later removal
!(bind! &my-atom (complex (expression with (structure))))
(add-atom &self &my-atom)
; ... later ...
(remove-atom &self &my-atom)  ; Guaranteed to match
```

### 3. Handle Duplicates Explicitly

```metta
; When duplicates expected, remove in loop
(= (remove-all $atom)
    (if (remove-atom &self $atom)
        (remove-all $atom)
        ()))

!(remove-all (fact 1))  ; Removes all instances
```

### 4. Document Side Effects

```metta
; Function that removes atoms should be documented:
; (: delete-fact (-> Atom Bool))
; Side effect: Removes atom from &self if present
(= (delete-fact $x) (remove-atom &self $x))
```

### 5. Use with Caution in Rules

```metta
; Be careful with remove-atom in rule bodies
; Each evaluation removes an atom
(= (dangerous-rule)
    (remove-atom &self (important-fact)))

; Better: check conditions first
(= (safe-rule)
    (if (should-remove?)
        (remove-atom &self (safe-to-remove))
        ()))
```

## Related Operations

- **[add-atom](01-adding-atoms.md)** - Add atoms to space
- **[get-atoms](05-space-operations.md#get-atoms)** - Retrieve all atoms
- **[match](05-space-operations.md#match)** - Pattern matching queries
- **[replace-atom](05-space-operations.md#replace-atom)** - Replace atoms (if available)

## Summary

**remove-atom characteristics:**
✅ Exact equality matching only
✅ Returns Bool (True/False) for success
✅ Removes only one instance at a time
✅ Observable via event system
✅ Efficient trie-based lookup

❌ No pattern matching (must be exact)
❌ Not atomic with other operations
❌ Not thread-safe without external synchronization
❌ No bulk pattern-based removal

**Key Implementation Details:**
- Location: `lib/src/metta/runner/stdlib/space.rs:178-201`
- Trie removal: `hyperon-space/src/index/trie.rs:350-450`
- Complexity: O(k + m) where k = tokens, m = atoms at leaf
- Returns: `Bool` (True/False)

**Critical Difference from add-atom:**
- `add-atom` returns `()` (unit)
- `remove-atom` returns `Bool` (success indicator)

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
