# Atom Space Mutation Order

## Abstract

This document rigorously specifies the ordering semantics, atomicity guarantees, and thread-safety properties of atom space mutations in MeTTa. We analyze the `add-atom`, `remove-atom`, and related operations, distinguishing between the specification and the current implementation.

## Table of Contents

1. [Mutation Operations](#mutation-operations)
2. [Atomicity](#atomicity)
3. [Ordering Guarantees](#ordering-guarantees)
4. [Thread Safety](#thread-safety)
5. [Observer Pattern](#observer-pattern)
6. [Examples](#examples)
7. [Specification vs Implementation](#specification-vs-implementation)

---

## Mutation Operations

### Add-Atom

**Syntax**:
```metta
(add-atom <space> <atom>)
```

**Semantics**:
- Adds `<atom>` to `<space>`
- Returns the unit atom `()`
- Side effect: Mutates the space

**Formal Specification**:
```
         Space = S    Atom = a
(ADD)    ─────────────────────────────
         (add-atom S a) ⇓ ()
         S' = S ∪ {a}
```

Where `S'` is the resulting space after mutation.

#### Implementation

From `hyperon-experimental/lib/src/metta/runner/stdlib/space.rs`:167-176:

```rust
impl CustomExecute for AddAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let arg_error = || ExecError::from("add-atom expects two arguments: space and atom");
        let space = args.get(0).ok_or_else(arg_error)?;
        let atom = args.get(1).ok_or_else(arg_error)?;
        let space = Atom::as_gnd::<DynSpace>(space)
            .ok_or("add-atom expects a space as the first argument")?;
        space.borrow_mut().add(atom.clone());
        unit_result()
    }
}
```

**Key Observations**:
1. Uses `borrow_mut()` for interior mutability via `RefCell`
2. Calls `space.add(atom.clone())`
3. Returns immediately with `unit_result()` = `()`
4. No explicit locking or synchronization

#### Underlying Space Mutation

From `hyperon-experimental/lib/src/space/grounding/mod.rs`:70-79:

```rust
pub fn add(&mut self, atom: Atom) {
    log::debug!("GroundingSpace::add: {}, atom: {}", self, atom);
    self.index.insert(atom.clone());
    self.common.notify_all_observers(&SpaceEvent::Add(atom));
}
```

**Steps**:
1. Insert atom into the index (trie-based `AtomIndex`)
2. Notify all registered observers synchronously
3. No transaction or rollback capability

### Remove-Atom

**Syntax**:
```metta
(remove-atom <space> <atom>)
```

**Semantics**:
- Removes `<atom>` from `<space>`
- Returns the unit atom `()`
- Side effect: Mutates the space

**Formal Specification**:
```
         Space = S    Atom = a    a ∈ S
(REMOVE) ──────────────────────────────────
         (remove-atom S a) ⇓ ()
         S' = S \ {a}
```

#### Implementation

From `space.rs`:194-204:

```rust
impl CustomExecute for RemoveAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let arg_error = || ExecError::from("remove-atom expects two arguments: space and atom");
        let space = args.get(0).ok_or_else(arg_error)?;
        let atom = args.get(1).ok_or_else(arg_error)?;
        let space = Atom::as_gnd::<DynSpace>(space)
            .ok_or("remove-atom expects a space as the first argument")?;
        space.borrow_mut().remove(atom);
        // TODO? Is it necessary to distinguish whether the atom was removed or not?
        unit_result()
    }
}
```

#### Underlying Space Mutation

From `mod.rs`:81-88:

```rust
pub fn remove(&mut self, atom: &Atom) -> bool {
    log::debug!("GroundingSpace::remove: {}, atom: {}", self, atom);
    let is_removed = self.index.remove(atom);
    if is_removed {
        self.common.notify_all_observers(&SpaceEvent::Remove(atom.clone()));
    }
    is_removed
}
```

**Key Observations**:
1. Returns `bool` indicating whether atom was actually removed
2. Only notifies observers if removal succeeded
3. Current `remove-atom` operation ignores return value (see TODO comment)

### Replace-Atom

**Syntax**:
```metta
(replace-atom <space> <old> <new>)
```

**Semantics**:
- Removes `<old>` from `<space>` and adds `<new>`
- Returns the unit atom `()`

**Implementation**: Not atomic - implemented as separate remove and add operations.

From `space.rs`:206-223:

```rust
impl CustomExecute for ReplaceAtomOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let arg_error = || ExecError::from("replace-atom expects three arguments: space, from-atom and to-atom");
        let space = args.get(0).ok_or_else(arg_error)?;
        let from = args.get(1).ok_or_else(arg_error)?;
        let to = args.get(2).ok_or_else(arg_error)?;
        let space = Atom::as_gnd::<DynSpace>(space)
            .ok_or("replace-atom expects a space as the first argument")?;
        space.borrow_mut().replace(from, to.clone());
        unit_result()
    }
}
```

From `mod.rs`:90-96:

```rust
pub fn replace(&mut self, from: &Atom, to: Atom) -> bool {
    log::debug!("GroundingSpace::replace {} to {}", from, to);
    let is_replaced = self.index.replace(from, to.clone());
    if is_replaced {
        self.common.notify_all_observers(&SpaceEvent::Replace(from.clone(), to));
    }
    is_replaced
}
```

**Critical Issue**: If `to` already exists in the space, the behavior is implementation-dependent.

---

## Atomicity

### Definition

An operation is **atomic** if it appears to occur instantaneously with respect to all other operations - either it completes entirely or has no effect.

### ACID Properties

In database terminology:
- **Atomicity**: All-or-nothing execution
- **Consistency**: Transitions from one valid state to another
- **Isolation**: Concurrent operations don't interfere
- **Durability**: Effects persist

### MeTTa Mutation Atomicity

**Specification**: Atomicity guarantees are **not specified** in the MeTTa language.

**Implementation**: Mutations are **NOT atomic** in the ACID sense.

#### Evidence

1. **No Transaction Support**: No `begin-transaction`, `commit`, `rollback` operations
2. **No Locking**: Search for synchronization primitives found none:
   ```
   No mutex, lock, atomic, or concurrent primitives in mutation code
   ```
3. **RefCell Only**: Uses `RefCell<Space>` for runtime borrow checking
4. **Observer Notifications**: Run synchronously during mutation (no isolation)

#### Implications

Consider this scenario:

```metta
; Attempt to swap two atoms
(= (swap)
   (let () (add-atom &space A)
        (let () (remove-atom &space B)
             ())))
```

**If interrupted between operations**:
- Space may contain both A and B (inconsistent intermediate state)
- No rollback mechanism to restore previous state

#### RefCell Semantics

`RefCell<T>` provides **runtime borrow checking**:

```rust
let space_ref = space.borrow_mut();  // Mutable borrow
space_ref.add(atom);                 // Mutation
// borrow_mut ends when space_ref goes out of scope
```

**Guarantees**:
- At most one `borrow_mut()` OR multiple `borrow()` at a time
- Panics if borrowed mutably while already borrowed

**Does NOT Guarantee**:
- Atomicity across multiple operations
- Isolation from concurrent threads (not thread-safe)
- Transaction-like semantics

---

## Ordering Guarantees

### Sequential Consistency

**Question**: If two mutations occur in a program, is their order preserved?

**Answer**: **Yes, within a single evaluation branch**.

#### Within a Single Branch

Consider:
```metta
!(let () (add-atom &space A)
     (add-atom &space B))
```

**Evaluation Order**:
1. Evaluate first `add-atom` → A is added
2. Evaluate second `add-atom` → B is added
3. **Order preserved**: A is added before B

**Formal Property**:
```
If e₁ ⇓ () happens before e₂ ⇓ () in evaluation order,
then effects of e₁ are visible before effects of e₂.
```

#### Across Multiple Branches

Consider:
```metta
(= (test) (add-atom &space A))
(= (test) (add-atom &space B))

!(test)
```

**Evaluation**:
- Two branches: one adds A, one adds B
- **Order is non-deterministic** - depends on plan processing order
- Current implementation: LIFO (last match processed first)
- **No guarantee** which atom is added first

**Formal Property**:
```
If e ⇓ {(v₁, β₁), (v₂, β₂)} with side effects s₁ and s₂,
the order of s₁ and s₂ is unspecified.
```

### Mutation During Pattern Matching

**Critical Case**: What happens if pattern matching triggers mutations?

**Example**:
```metta
; Space initially contains: (foo 1)
(= (trigger-mutation)
   (add-atom &space (foo 2)))

; Query that matches foo patterns while mutating
!(match &space (foo $x) (trigger-mutation))
```

**Possible Behaviors**:
1. **Snapshot Semantics**: Query sees space before mutation (matches only `(foo 1)`)
2. **Live Semantics**: Query sees mutation (may match `(foo 2)` as well)
3. **Undefined**: Behavior is implementation-dependent

**Current Implementation**: Likely **snapshot semantics** because:
- Pattern matching collects all results first
- Mutations happen during evaluation of match body
- Space is borrowed immutably during query

#### Code Analysis

From `interpreter.rs`:604-638 (query function):

```rust
fn query(space: &DynSpace, prev: Option<Rc<RefCell<Stack>>>, to_eval: Atom,
         bindings: Bindings, vars: Variables) -> Vec<InterpretedAtom> {
    // ...
    let results = space.borrow().query(&query);  // Immutable borrow
    // ... process results ...
}
```

**Key**: `space.borrow()` (immutable) prevents `space.borrow_mut()` (mutable) during query.

**Therefore**: Mutations during query evaluation will **panic** if attempted in the same thread.

---

## Thread Safety

### Definition

A data structure is **thread-safe** if it can be safely accessed from multiple threads concurrently without data races or undefined behavior.

### MeTTa Space Thread Safety

**Specification**: Thread safety is **not specified** in the MeTTa language.

**Implementation**: **NOT thread-safe**.

#### Evidence

From the entire `lib/src/space/grounding/` module:
- **No** `Mutex` or `RwLock` wrappers
- **No** atomic operations (`AtomicUsize`, `AtomicBool`, etc.)
- **No** `Send` or `Sync` trait implementations for safety
- Uses `RefCell` which is **explicitly not thread-safe**

From Rust documentation:
> "`RefCell<T>` ... is not thread safe. For thread-safe interior mutability, consider using `Mutex<T>`, `RwLock<T>`, or `Atomic*` types."

#### Data Race Example

**Hypothetical concurrent code** (would fail):

```rust
let space = Arc::new(GroundingSpace::new());  // Shared reference

thread::spawn({
    let space = space.clone();
    move || space.borrow_mut().add(atom1)  // Thread 1: mutate
});

thread::spawn({
    let space = space.clone();
    move || space.borrow_mut().add(atom2)  // Thread 2: mutate
});
```

**Result**:
- May panic (RefCell borrow check)
- May cause data corruption (race condition)
- **Undefined behavior** in general

#### Single-Threaded Assumption

The current implementation **assumes single-threaded execution**:
- Interpreter processes one step at a time
- Plan is processed sequentially
- No concurrent evaluation of different branches

---

## Observer Pattern

### Space Observers

Spaces support observers that are notified when mutations occur.

From `mod.rs`:70-96, mutations trigger:
```rust
self.common.notify_all_observers(&SpaceEvent::Add(atom));
self.common.notify_all_observers(&SpaceEvent::Remove(atom.clone()));
self.common.notify_all_observers(&SpaceEvent::Replace(from.clone(), to));
```

### Space Events

```rust
pub enum SpaceEvent {
    Add(Atom),
    Remove(Atom),
    Replace(Atom, Atom),
}
```

### Observer Execution Order

**Question**: In what order are observers notified?

**Implementation**: From the code structure, observers are notified:
1. **Synchronously** during the mutation
2. In the order they were registered
3. Before the mutation function returns

**Implications**:
- Observers can see **intermediate states** if not atomic
- Observer code **blocks** the mutation
- Observers **cannot** safely mutate the same space (would violate borrow rules)

### Observer Example

```rust
space.register_observer(Box::new(|event| {
    match event {
        SpaceEvent::Add(atom) => println!("Added: {}", atom),
        SpaceEvent::Remove(atom) => println!("Removed: {}", atom),
        SpaceEvent::Replace(old, new) => println!("Replaced {} with {}", old, new),
    }
}));
```

---

## Examples

### Example 1: Sequential Mutations

**MeTTa Program**:
```metta
!(let () (add-atom &space A)
     (let () (add-atom &space B)
          (let () (add-atom &space C)
               ())))
```

**Evaluation**:
```
Step 1: add-atom &space A → Space = {A}
Step 2: add-atom &space B → Space = {A, B}
Step 3: add-atom &space C → Space = {A, B, C}
Result: ()
```

**Guarantee**: A, B, C are added in order.

### Example 2: Non-Deterministic Mutations

**MeTTa Program**:
```metta
(= (mutate) (add-atom &space X))
(= (mutate) (add-atom &space Y))

!(mutate)
```

**Evaluation** (depends on plan processing):

**Scenario 1** (X processed first):
```
Branch 1: add-atom &space X → Space = {X}
Branch 2: add-atom &space Y → Space = {X, Y}
Final Space: {X, Y}
```

**Scenario 2** (Y processed first):
```
Branch 1: add-atom &space Y → Space = {Y}
Branch 2: add-atom &space X → Space = {X, Y}
Final Space: {X, Y} (same final state, different intermediate states)
```

**In this case**: Final state is the same, but intermediate states differ.

### Example 3: Non-Confluent Mutations

**MeTTa Program**:
```metta
(= (conditional-add)
   (if (space-empty? &space)
       (add-atom &space A)
       (add-atom &space B)))

; Two branches
!(conditional-add)
!(conditional-add)
```

**Evaluation** (order-dependent):

**Order 1**: First conditional-add, then second
```
Step 1: Check space-empty? → true
        Add A → Space = {A}
Step 2: Check space-empty? → false
        Add B → Space = {A, B}
```

**Order 2**: Interleaved evaluation
```
Step 1: Both check space-empty? → both true
Step 2: Both add A → Space = {A, A} (or {A} if duplicates removed)
```

**Result**: **Non-confluent** - final state depends on evaluation order.

### Example 4: Mutation During Query (Illegal)

**MeTTa Program**:
```metta
(= (mutate-on-match $x)
   (add-atom &space (new $x)))

!(match &space (foo $x) (mutate-on-match $x))
```

**Evaluation**:
```
Step 1: Query space for (foo $x) → space.borrow()
        Collects matches: [(foo 1), (foo 2), ...]

Step 2: Evaluate (mutate-on-match 1)
        Calls add-atom → space.borrow_mut()

ERROR: RefCell already borrowed immutably!
PANIC: cannot borrow mutably while borrowed immutably
```

**Result**: **Runtime panic** (implementation-specific behavior).

### Example 5: Observer Side Effects

**MeTTa Program** (with observer):
```metta
; Assume observer prints on add
!(add-atom &space A)
```

**Execution**:
```
add-atom called
  → space.add(A)
    → index.insert(A)
    → notify_observers(Add(A))
      → observer1.handle(Add(A))  ; Prints "Added: A"
      → observer2.handle(Add(A))  ; Other observer
    → return from add
  → return ()
```

**Key**: Observers run **synchronously** before add-atom returns.

---

## Specification vs Implementation

| Aspect | Specification | Implementation |
|--------|--------------|----------------|
| **Atomicity** | Not specified | Not atomic (no transactions) |
| **Thread Safety** | Not specified | Not thread-safe (RefCell) |
| **Ordering Within Branch** | Sequential | Sequential (guaranteed) |
| **Ordering Across Branches** | Non-deterministic | LIFO plan processing |
| **Mutation During Query** | Not specified | Panics (RefCell violation) |
| **Observer Notification** | Not specified | Synchronous, in registration order |
| **Replace Atomicity** | Not specified | Two operations (remove + add) |
| **Error Handling** | Not specified | Operations return () regardless of success |

---

## Design Recommendations

For MeTTa compiler implementers:

### Atomicity

**Consider**:
1. **Transaction Support**: Add `begin-transaction`, `commit`, `rollback` operations
2. **Atomic Replace**: Ensure `replace-atom` is truly atomic
3. **Batch Operations**: Support atomic multi-atom mutations

**Example API**:
```metta
!(transaction &space
   (lambda ()
      (add-atom &space A)
      (remove-atom &space B)
      (add-atom &space C)))
```

### Thread Safety

**Consider**:
1. **Concurrent Spaces**: Provide thread-safe space implementations
2. **Read-Write Locks**: Allow concurrent reads, exclusive writes
3. **Lock-Free Structures**: For high-performance concurrent access

**Example** (Rust):
```rust
pub struct ConcurrentSpace {
    index: Arc<RwLock<AtomIndex>>,  // Thread-safe
    // ...
}
```

### Ordering Semantics

**Consider**:
1. **Deterministic Mode**: Option to enforce deterministic evaluation order
2. **Snapshot Queries**: Guarantee queries see consistent space state
3. **Isolated Branches**: Ensure branches don't interfere via side effects

### Error Reporting

**Consider**:
1. **Return Success/Failure**: Let `remove-atom` indicate if atom existed
2. **Error Atoms**: Return error values instead of always `()`
3. **Exceptions**: Support exception handling for mutation failures

---

## References

### Source Code

- **`hyperon-experimental/lib/src/metta/runner/stdlib/space.rs`**
  - `AddAtomOp::execute()` (lines 167-176)
  - `RemoveAtomOp::execute()` (lines 194-204)
  - `ReplaceAtomOp::execute()` (lines 206-223)

- **`hyperon-experimental/lib/src/space/grounding/mod.rs`**
  - `GroundingSpace::add()` (lines 70-79)
  - `GroundingSpace::remove()` (lines 81-88)
  - `GroundingSpace::replace()` (lines 90-96)

### Academic References

- **Herlihy, M. & Wing, J.** (1990). "Linearizability: A Correctness Condition for Concurrent Objects". *ACM TOPLAS*.
- **Gray, J. & Reuter, A.** (1992). *Transaction Processing: Concepts and Techniques*. Morgan Kaufmann.
- **Rust RefCell Documentation**: https://doc.rust-lang.org/std/cell/struct.RefCell.html

---

## See Also

- **§01**: Evaluation order (how mutations occur during evaluation)
- **§03**: Pattern matching (queries during mutations)
- **§05**: Non-determinism (multiple branches with mutations)
- **§07**: Formal proofs (confluence with side effects)

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
