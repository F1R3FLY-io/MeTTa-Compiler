# Atom Space Edge Cases and Special Behaviors

## Overview

This document catalogs edge cases, special behaviors, gotchas, and corner cases related to MeTTa's atom space operations. Understanding these helps avoid bugs and unexpected behavior.

## Empty Atoms and Structures

### Empty Expressions

**Behavior:**
```metta
; Empty expression is valid
(add-atom &self ())

; Can be queried
!(match &self () found)
; → [found]

; Can be removed
(remove-atom &self ())  ; → True
```

**Tokenization:**
```
Atom: ()
Tokens: [OpenParen, CloseParen]
```

**Use Cases:**
- Unit value representation
- Placeholder atoms
- Terminator in lists

### Empty Space

**Behavior:**
```metta
; Create empty space
!(bind! &empty (new-space))

; Query empty space
!(get-atoms &empty)
; → []

!(match &empty $anything $anything)
; → []

; Remove from empty space
(remove-atom &empty (anything))  ; → False
```

**No Special Handling:**
- Operations on empty space behave normally
- No errors thrown
- Queries return empty results

## Self-Referential Structures

### Space Containing Itself

**Behavior:**
```metta
; Add space to itself
(add-atom &self &self)

!(match &self &self found)
; → [found]

; Can be removed
(remove-atom &self &self)  ; → True
```

**Implications:**
- Creates self-reference
- May cause infinite loops in naive traversals
- Space is just an atom, so this is valid

**Caution:**
```metta
; Dangerous: recursive query
!(match &self $x
    (if (is-space? $x)
        (match $x $y $y)  ; May infinitely recurse
        $x))
```

### Recursive Atoms

**Behavior:**
```metta
; Create cyclic structure (if language permits)
; Note: Direct cycles not possible in MeTTa's immutable atoms
; But can create reference cycles via spaces

!(bind! &space1 (new-space))
!(bind! &space2 (new-space))

(add-atom &space1 (refers-to &space2))
(add-atom &space2 (refers-to &space1))

; Cycle exists via spaces
```

**No Cycle Detection:**
- MeTTa doesn't detect or prevent cycles
- Programmer must avoid infinite traversals

## Duplicate Handling

### AllowDuplication Strategy

**Adding Same Atom Multiple Times:**
```metta
(add-atom &self (fact 1))
(add-atom &self (fact 1))
(add-atom &self (fact 1))

!(match &self (fact 1) found)
; → [found, found, found]
```

**Implications:**
- Each `add-atom` creates new instance
- Queries return multiple results
- Each requires separate `remove-atom`

**Removing Duplicates:**
```metta
; Only removes one instance at a time
(remove-atom &self (fact 1))  ; → True
(remove-atom &self (fact 1))  ; → True
(remove-atom &self (fact 1))  ; → True
(remove-atom &self (fact 1))  ; → False (all gone)
```

**Remove-All Pattern:**
```metta
; Remove all instances
(= (remove-all $space $atom)
    (if (remove-atom $space $atom)
        (remove-all $space $atom)
        ()))

!(remove-all &self (fact 1))
```

### NoDuplication Strategy

**Subsequent Additions Ignored:**
```metta
; Assuming NoDuplication
(add-atom &self (fact 1))  ; Added
(add-atom &self (fact 1))  ; Ignored (duplicate)
(add-atom &self (fact 1))  ; Ignored

!(match &self (fact 1) found)
; → [found]  (only one)
```

**Idempotent Additions:**
- Multiple `add-atom` calls = single instance
- No observable effect after first addition

## Variable Name Sensitivity

### Exact Matching Required for Removal

**Problem:**
```metta
; Add with variable $x
(add-atom &self (= (f $x) (* $x 2)))

; Try to remove with variable $y
(remove-atom &self (= (f $y) (* $y 2)))  ; → False (doesn't match!)

; Must use exact same variable name
(remove-atom &self (= (f $x) (* $x 2)))  ; → True
```

**Reason:**
- Variables are atoms with names
- `$x` ≠ `$y` (different names)
- `remove-atom` uses exact equality

**Workaround:**
```metta
; Query to find exact atom, then remove
!(match &self (= (f $var) $result)
    (remove-atom &self (= (f $var) $result)))
```

### Variables in Patterns

**Variables Don't Match Ground Terms for Removal:**
```metta
(add-atom &self (Human Socrates))

; This doesn't work:
(remove-atom &self (Human $x))  ; → False

; Variable $x is stored literally, not a wildcard
; To remove, must use exact atom:
(remove-atom &self (Human Socrates))  ; → True
```

**Pattern Matching vs Exact Matching:**
- `match`: Variables are wildcards
- `remove-atom`: Variables are literals

## Large and Deep Atoms

### Very Large Atoms

**Behavior:**
```metta
; Large expression
(add-atom &self (huge
    (data (with (many (nested (levels))))))
    (more (stuff (here))))
```

**Implications:**
- No size limit enforced
- Memory consumption proportional to size
- Trie depth increases with nesting
- Query performance may degrade

**Performance:**
- Tokenization: O(n) where n = atom size
- Trie traversal: O(depth)
- Memory: O(n)

### Deeply Nested Expressions

**Behavior:**
```metta
; Very deep nesting
(add-atom &self (a (b (c (d (e (f (g (h (i (j (k (l (m (n (o (p (q (r (s (t (u (v (w (x (y (z)))))))))))))))))))))))))))))
```

**Implications:**
- Stack depth during tokenization
- Trie path length increases
- May hit recursion limits in implementation

**Practical Limit:**
- Typically hundreds of levels before issues
- Implementation-dependent

## Concurrent Modification

### Modification During Iteration

**Problem:**
```metta
; This may not work as expected:
!(match &self $atom
    (remove-atom &self $atom))
```

**Issue:**
- Modifies space while iterating
- Behavior undefined/implementation-dependent
- May skip atoms or repeat

**Safe Pattern:**
```metta
; Collect first, then modify
!(bind! &to-remove (match &self $atom $atom))
!(match &to-remove $atom
    (remove-atom &self $atom))
```

### Adding During Query

**Problem:**
```metta
!(match &self (fact $x)
    (add-atom &self (derived-fact $x)))
```

**Issue:**
- Adds atoms while querying
- Newly added atoms may or may not be seen
- Non-deterministic behavior

**Safe Pattern:**
```metta
; Use separate space for new atoms
!(bind! &new-facts (new-space))
!(match &self (fact $x)
    (add-atom &new-facts (derived-fact $x)))

; Then merge
!(match &new-facts $atom
    (add-atom &self $atom))
```

## Type Mismatches

### Passing Non-Space to Operations

**Error:**
```metta
!(get-atoms not-a-space)
; Error: Expected Space type

!(add-atom 42 (fact 1))
; Error: Expected Space type
```

**Prevention:**
- Ensure arguments are Space type
- Use `new-space` to create spaces

### Invalid Atom Arguments

**Error:**
```metta
; Missing atom argument
(add-atom &self)
; Error: add-atom expects one argument

; Extra arguments (implementation-dependent)
(add-atom &self (fact 1) (fact 2))
; May error or ignore extra argument
```

## Observer-Related Edge Cases

### Observer Modifying Space

**Dangerous Pattern:**
```rust
struct DangerousObserver;

impl SpaceObserver for DangerousObserver {
    fn notify(&self, event: &SpaceEvent) {
        match event {
            SpaceEvent::Add(atom) => {
                // DON'T DO THIS: modifying space from observer
                // space.add(new_atom);  // Would cause borrow error!
            }
            _ => {}
        }
    }
}
```

**Issue:**
- Space already borrowed mutably during add/remove
- Observer attempting to modify would panic
- `already borrowed: BorrowMutError`

**Safe Pattern:**
- Observers should only read/log
- Queue modifications for later application

### Observer Exceptions

**Behavior:**
```rust
impl SpaceObserver for PanickyObserver {
    fn notify(&self, event: &SpaceEvent) {
        panic!("Observer panicked!");
    }
}
```

**Impact:**
- Panic propagates up
- Operation (add/remove) interrupted
- Space may be in inconsistent state

**Best Practice:**
- Observers should not panic
- Catch and log errors internally

## Space References and Lifetime

### Dangling Space References

**Potential Issue:**
```metta
; Create space
!(bind! &temp (new-space))
(add-atom &temp (data 1))

; Later, if &temp goes out of scope or is rebound...
!(bind! &temp (new-space))  ; Old space now unreachable

; Old space will be garbage collected
```

**Safe Practices:**
- Keep space references as long as needed
- Be aware of scope and rebinding

### Passing Spaces to Functions

**Behavior:**
```metta
(: process-space (-> Space ()))
(= (process-space $space)
    (match $space $atom (print $atom)))

!(bind! &myspace (new-space))
(add-atom &myspace (data 1))

!(process-space &myspace)  ; Works fine
```

**Note:**
- Spaces are first-class values
- Can be passed, returned, stored

## Pattern Matching Edge Cases

### Matching Variables Literally

**Behavior:**
```metta
; Add a variable atom literally
(add-atom &self $x)

; Query for it (exact match)
!(match &self $x found)
; → [found]

; Query with different variable
!(match &self $y found)
; → []  (no match)
```

**Note:**
- `$x` stored as Variable("x") atom
- Matches only exact variable name

### Wildcard Patterns

**If Supported:**
```metta
; Match anything with $_
!(match &self $_ found)
; → [found, found, ...]  (one per atom)
```

**Behavior:**
- `$_` matches any atom
- Doesn't bind value
- Useful for existence checks

## Ordering and Non-Determinism

### No Guaranteed Query Order

**Behavior:**
```metta
(add-atom &self (fact 1))
(add-atom &self (fact 2))
(add-atom &self (fact 3))

!(get-atoms &self)
; → [(fact 2), (fact 1), (fact 3)]  (example order)
; Next run may return different order
```

**Implications:**
- Don't rely on result order
- Order may vary between runs
- Implementation-dependent

### Multiple Matching Rules

**Non-Determinism:**
```metta
(add-atom &self (= (choose $x) (optionA $x)))
(add-atom &self (= (choose $x) (optionB $x)))

!(choose 42)
; May return: (optionA 42) or (optionB 42) or both
```

**Behavior:**
- No guaranteed rule precedence
- All matching rules may be tried
- Result depends on evaluation strategy

## Special Atom Types

### Grounded Atoms

**Behavior:**
```metta
; Grounded atoms (Rust objects) can be stored
(add-atom &self <some-grounded-value>)

; Equality based on grounded value's impl
(remove-atom &self <some-grounded-value>)  ; Depends on Eq impl
```

**Implications:**
- Grounded values may have custom equality
- Removal depends on proper Eq implementation
- May not be serializable

### Type Atoms

**Behavior:**
```metta
; Type atoms (like Number, String) can be stored
(add-atom &self Number)
(add-atom &self String)

!(match &self Number found)
; → [found]
```

**Note:**
- Types are atoms themselves
- Can be stored and queried like any atom

## Atomicity and Transactions

### No Atomic Multi-Operation

**Problem:**
```metta
; These are separate operations (not atomic):
(remove-atom &self (old-value))
(add-atom &self (new-value))

; If program crashes between them, old-value is lost
```

**Workaround:**
```metta
; Add first, then remove (safer order)
(add-atom &self (new-value))
(remove-atom &self (old-value))

; Now crash leaves both (redundant but safe)
```

**No Built-in Transactions:**
- MeTTa doesn't support transactions
- No rollback mechanism
- Programmer must ensure consistency

## Memory and Performance Edge Cases

### Memory Exhaustion

**Behavior:**
```metta
; Adding huge number of atoms
(= (add-many 0) ())
(= (add-many $n)
    (seq (add-atom &self (fact $n))
         (add-many (- $n 1))))

!(add-many 1000000)  ; May exhaust memory
```

**No Limit Enforcement:**
- MeTTa doesn't limit space size
- Programmer must manage memory usage

### Query Performance Degradation

**Behavior:**
```metta
; With many atoms, variable-heavy queries slow:
!(match &self $anything $anything)
; O(n) where n = total atoms
```

**Mitigation:**
- Use ground prefixes when possible
- Limit space size
- Partition into multiple spaces

## Error Handling Edge Cases

### Silent Failures

**No Error Cases:**
```metta
; These don't error, just return expected values:
(remove-atom &self (nonexistent))  ; → False (not an error)
!(match &self (nomatch $x) $x)    ; → [] (empty, not an error)
```

**Explicit Error Checking:**
```metta
!(if (not (remove-atom &self (expected-atom)))
    (print "Warning: expected-atom not found")
    ())
```

### Type Errors

**Caught at Runtime:**
```metta
!(add-atom "not a space" (fact 1))
; Error: Expected Space type

!(get-atoms 42)
; Error: Expected Space type
```

**Prevention:**
- Use type annotations
- Enable type checking with pragma

## Best Practices Summary

### Do's

✅ **Collect before modifying during iteration**
```metta
!(bind! &items (match &self $x $x))
!(match &items $x (remove-atom &self $x))
```

✅ **Use exact atoms for removal**
```metta
!(bind! &to-remove (exact-atom))
(remove-atom &self &to-remove)
```

✅ **Check results of operations**
```metta
!(if (remove-atom &self (atom))
    (print "Removed")
    (print "Not found"))
```

✅ **Use separate spaces for isolation**
```metta
!(bind! &temp (new-space))
; Work in &temp, commit to &self later
```

### Don'ts

❌ **Don't modify space during iteration**
```metta
; Bad:
!(match &self $x (remove-atom &self $x))
```

❌ **Don't rely on query result order**
```metta
; Bad:
!(first (match &self $x $x))  ; Order undefined!
```

❌ **Don't use pattern variables in remove-atom**
```metta
; Bad (doesn't work as expected):
(remove-atom &self (fact $x))  ; Removes literal $x, not wildcard
```

❌ **Don't create deep recursion without base cases**
```metta
; Bad:
(add-atom &self (= (loop) (loop)))  ; Infinite recursion
```

## Related Documentation

- **[Adding Atoms](01-adding-atoms.md)** - Detailed add-atom behavior
- **[Removing Atoms](02-removing-atoms.md)** - Detailed remove-atom behavior
- **[Space Operations](05-space-operations.md)** - All operations
- **[Space Structure](06-space-structure.md)** - Internal implementation

## Summary

**Common Edge Cases:**
- Empty atoms and spaces
- Self-referential structures
- Duplicate handling differences
- Variable name sensitivity
- Concurrent modification issues
- No atomicity guarantees

**Key Gotchas:**
- `remove-atom` requires exact match (variables are literal)
- No guaranteed query order
- Modifying space during iteration undefined
- No transaction support
- Observer can't modify space

**Safe Patterns:**
- Collect before modify
- Use exact atoms for removal
- Check operation results
- Separate spaces for isolation
- Ground prefixes for efficient queries

**Always Remember:**
- Atom spaces are single-threaded
- Operations are not atomic
- Variables in removal are literals, not wildcards
- Query order is undefined
- No automatic constraint enforcement

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
