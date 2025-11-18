# Atom Space Operations

## Overview

This document provides comprehensive details about all operations available for working with MeTTa atom spaces, beyond basic `add-atom` and `remove-atom`. These operations enable querying, creating, and managing multiple atom spaces.

## Core Operations

### new-space

**Purpose**: Creates a new, independent atom space.

**Syntax:**
```metta
(new-space) → Space
```

**Semantics:**
- Creates empty atom space
- Independent from all other spaces
- Can be assigned to variables
- First-class value (can be passed to functions)

**Implementation**: `lib/src/metta/runner/stdlib/space.rs:210-235`

**Example:**
```metta
; Create new space
!(bind! &myspace (new-space))

; Add atoms to it
(add-atom &myspace (fact 1))
(add-atom &myspace (fact 2))

; Query it
!(get-atoms &myspace)
; → [(fact 1), (fact 2)]
```

**Use Cases:**
- Separate namespaces
- Modular knowledge bases
- Temporary workspaces
- Isolation for testing

**Properties:**
- **Empty**: New space contains no atoms initially
- **Independent**: Changes don't affect other spaces
- **Persistent**: Exists until garbage collected
- **Type**: Returns `Space` type

**Performance:**
- O(1) time complexity
- Minimal memory overhead
- Lazy initialization of trie structure

---

### get-atoms

**Purpose**: Retrieves all atoms from a space.

**Syntax:**
```metta
(get-atoms <space>) → List<Atom>
```

**Parameters:**
- `<space>`: Space reference (e.g., `&self`, `&myspace`)

**Return Value:**
- List of all atoms in the space
- No guaranteed order
- Includes facts, rules, and all other atoms

**Implementation**: `lib/src/metta/runner/stdlib/space.rs:240-265`

**Example:**
```metta
; Add some atoms
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (= (mortal $x) (Human $x)))

; Get all atoms
!(get-atoms &self)
; → [(Human Socrates), (Human Plato), (= (mortal $x) (Human $x))]
; (order not guaranteed)
```

**Characteristics:**
- **Complete**: Returns every atom in space
- **Unordered**: No guaranteed order
- **Snapshot**: May not reflect concurrent modifications
- **Duplicates**: If AllowDuplication, duplicates included

**Performance:**
- Time: O(n) where n = number of atoms
- Space: O(n) for result list
- Copies all atoms into result

**Use Cases:**
- Debugging (inspect space contents)
- Serialization (export space)
- Bulk operations (process all atoms)
- Space copying

**Limitations:**
- No filtering (returns everything)
- May be slow for large spaces
- No pagination or streaming

**Alternative**: For filtered retrieval, use `match` instead.

---

### match

**Purpose**: Pattern matching query over atom space.

**Syntax:**
```metta
(match <space> <pattern> <template>)
```

**Parameters:**
- `<space>`: Space to query
- `<pattern>`: Pattern with possible variables
- `<template>`: Expression to evaluate for each match (variables bound)

**Return Value:**
- List of results from evaluating `<template>` for each match
- Empty list if no matches

**Implementation**: `lib/src/metta/interpreter.rs:500-650`

**Examples:**

**Simple Pattern:**
```metta
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

!(match &self (Human $x) $x)
; → [Socrates, Plato]
```

**Complex Pattern:**
```metta
(add-atom &self (age John 30))
(add-atom &self (age Alice 25))

!(match &self (age $person $years) ($person $years))
; → [(John 30), (Alice 25)]
```

**Nested Pattern:**
```metta
(add-atom &self (person (name "Alice") (age 30)))

!(match &self (person (name $n) (age $a)) $n)
; → ["Alice"]
```

**Multiple Variables:**
```metta
(add-atom &self (edge A B 5))
(add-atom &self (edge B C 3))

!(match &self (edge $from $to $weight) ($from $to))
; → [(A B), (B C)]
```

**Computed Template:**
```metta
(add-atom &self (number 5))
(add-atom &self (number 10))

!(match &self (number $n) (* $n 2))
; → [10, 20]
```

**Characteristics:**
- **Pattern Matching**: Variables match any atom
- **Binding**: Variables bound for each match
- **Evaluation**: Template evaluated with bindings
- **Multiple Results**: Returns all matches

**Pattern Matching Rules:**

**Variables**:
- `$x` matches any atom
- Binds matched atom to variable name
- Scope limited to template

**Ground Terms**:
- Literal symbols, numbers, strings must match exactly
- `(Human Socrates)` only matches `(Human Socrates)`

**Expressions**:
- Recursive matching of nested structures
- `(f $x)` matches any expression starting with `f`

**Wildcards**:
- `$_` matches anything without binding
- Useful when value not needed

**Variadic**:
- `$xs...` matches zero or more atoms (in some contexts)

**Performance:**
- Time: O(n) worst case (all atoms), O(m) typical (m = matches)
- Benefits from trie indexing with ground prefixes
- Variables require broader search

**Use Cases:**
- Querying facts by pattern
- Finding rules
- Filtering atoms
- Extracting data

**Trie Optimization:**
```metta
; Efficient (ground prefix "Human")
!(match &self (Human $x) $x)

; Less efficient (variable prefix)
!(match &self ($relation Socrates) $relation)

; Inefficient (all variables)
!(match &self $anything $anything)
```

---

### atom-count

**Purpose**: Returns the number of atoms in a space.

**Syntax:**
```metta
(atom-count <space>) → Number
```

**Example:**
```metta
(add-atom &self (fact 1))
(add-atom &self (fact 2))
(add-atom &self (fact 3))

!(atom-count &self)
; → 3
```

**Note**: May not be available in all MeTTa implementations. Check hyperon-experimental for current support.

**Performance:**
- O(n) if counting required
- O(1) if space maintains count

---

### space-query (Advanced)

**Purpose**: Low-level query interface for space.

**Note**: This is typically an internal operation. Use `match` for most use cases.

**Functionality:**
- Direct trie traversal
- Efficient pattern matching
- Used by `match` implementation

**Location**: `lib/src/space/grounding/mod.rs:200-250`

---

## Multi-Space Operations

### Copying Atoms Between Spaces

**Pattern:**
```metta
; Copy all atoms from &source to &target
!(match &source $atom
    (add-atom &target $atom))
```

**Filtered Copy:**
```metta
; Copy only humans
!(match &source (Human $x)
    (add-atom &target (Human $x)))
```

### Merging Spaces

**Pattern:**
```metta
; Merge &space1 and &space2 into &merged
!(bind! &merged (new-space))

!(match &space1 $atom (add-atom &merged $atom))
!(match &space2 $atom (add-atom &merged $atom))
```

**Note**: With AllowDuplication, duplicates from both spaces included.

### Space Difference

**Pattern:**
```metta
; Atoms in &space1 but not in &space2
!(match &space1 $atom
    (if (not (match &space2 $atom True))
        $atom
        ()))
```

### Space Intersection

**Pattern:**
```metta
; Atoms in both &space1 and &space2
!(match &space1 $atom
    (if (match &space2 $atom True)
        $atom
        ()))
```

## Working with Multiple Spaces

### Space References

**Default Space:**
```metta
&self  ; Current module's default space
```

**User-Created Spaces:**
```metta
!(bind! &myspace (new-space))
!(bind! &database (new-space))
!(bind! &temp (new-space))
```

**Passing Spaces:**
```metta
; Spaces are first-class values
(: process-space (-> Space ()))
(= (process-space $space)
    (match $space $atom
        (print $atom)))

!(process-space &myspace)
```

### Space Isolation

**Example:**
```metta
; Separate concerns
!(bind! &facts (new-space))
!(bind! &rules (new-space))

; Add facts to fact space
(add-atom &facts (Human Socrates))
(add-atom &facts (Human Plato))

; Add rules to rule space
(add-atom &rules (= (mortal $x) (Human $x)))

; Query specific space
!(match &facts (Human $x) $x)
; → [Socrates, Plato]

!(match &rules (= $p $r) $p)
; → [(mortal $x)]
```

**Benefits:**
- Organizational clarity
- Performance (smaller search spaces)
- Modularity
- Controlled access

### Cross-Space Queries

**Pattern:**
```metta
; Query one space from context of another
(= (cross-query)
    (match &other-space (data $x) $x))

; Space reference must be accessible
```

## Observer Operations

### Registering Observers

**Rust API** (not typically available in MeTTa scripts):
```rust
use hyperon::space::{SpaceObserver, SpaceEvent, GroundingSpace};
use std::rc::Rc;

struct MyObserver;

impl SpaceObserver for MyObserver {
    fn notify(&self, event: &SpaceEvent) {
        match event {
            SpaceEvent::Add(atom) => println!("Added: {:?}", atom),
            SpaceEvent::Remove(atom) => println!("Removed: {:?}", atom),
            _ => {}
        }
    }
}

let mut space = GroundingSpace::new();
space.register_observer(Rc::new(MyObserver));
```

**Event Types:**
```rust
pub enum SpaceEvent {
    Add(Atom),           // Atom was added
    Remove(Atom),        // Atom was removed
    Replace(Atom, Atom), // Atom was replaced (old, new)
}
```

**Use Cases:**
- Logging space modifications
- Incremental processing
- Cache invalidation
- Event-driven reactions

## Advanced Patterns

### Caching Query Results

**Problem**: Repeated queries expensive

**Solution**:
```metta
; Cache results in separate space
!(bind! &cache (new-space))

(= (cached-query $pattern)
    (let $cached (match &cache (result $pattern $r) $r)
        (if (not (empty? $cached))
            $cached
            (let $result (match &self $pattern $pattern)
                (add-atom &cache (result $pattern $result))
                $result))))
```

### Transactional-Like Updates

**Problem**: Multiple updates not atomic

**Workaround**:
```metta
; 1. Prepare changes in temporary space
!(bind! &temp (new-space))
(add-atom &temp (new-fact 1))
(add-atom &temp (new-fact 2))

; 2. Validate
!(if (valid? &temp)
    ; 3. Commit: copy to main space
    (match &temp $atom (add-atom &self $atom))
    ; 4. Rollback: discard temp
    ())
```

**Note**: Still not truly atomic, but reduces window of inconsistency.

### Versioning Spaces

**Pattern**:
```metta
; Create versions
!(bind! &v1 (new-space))
!(bind! &v2 (new-space))

; Version 1
(add-atom &v1 (data 1))

; Version 2 (copy v1 + modifications)
!(match &v1 $atom (add-atom &v2 $atom))
(add-atom &v2 (data 2))

; Now have two versions
```

### Space Snapshots

**Pattern**:
```metta
; Create snapshot
!(bind! &snapshot (new-space))
!(match &self $atom (add-atom &snapshot $atom))

; Modify original
(add-atom &self (new-data))

; Snapshot unchanged
```

### Incremental Updates

**Pattern**:
```metta
; Track changes in delta space
!(bind! &delta (new-space))

; Add new facts
(= (add-tracked $atom)
    (seq (add-atom &self $atom)
         (add-atom &delta (added $atom))))

; Apply delta to another space
!(match &delta (added $atom)
    (add-atom &target $atom))
```

## Performance Optimization

### Query Optimization

**Use Ground Prefixes:**
```metta
; Good
!(match &self (Human $x) $x)

; Bad
!(match &self ($relation Socrates) $relation)
```

**Avoid Full Scans:**
```metta
; Avoid
!(match &self $x $x)  ; Returns all atoms (expensive)

; Prefer
!(get-atoms &self)    ; Explicit intent
```

### Space Partitioning

**Problem**: Large monolithic space slow

**Solution**: Partition into smaller spaces
```metta
!(bind! &humans (new-space))
!(bind! &animals (new-space))

(add-atom &humans (Human Socrates))
(add-atom &animals (Animal Cat))

; Queries faster (smaller search space)
!(match &humans (Human $x) $x)
```

### Selective Copying

**Problem**: Copying entire space expensive

**Solution**: Copy only needed atoms
```metta
; Bad
!(match &source $atom (add-atom &target $atom))

; Good
!(match &source (needed-pattern $x)
    (add-atom &target (needed-pattern $x)))
```

## Common Use Cases

### Knowledge Base

**Setup:**
```metta
!(bind! &kb (new-space))

; Add facts
(add-atom &kb (Human Socrates))
(add-atom &kb (Human Plato))

; Add rules
(add-atom &kb (= (mortal $x) (Human $x)))

; Query
!(match &kb (Human $x) $x)
```

### Temporary Workspace

**Setup:**
```metta
; Create workspace
!(bind! &workspace (new-space))

; Do computations
(add-atom &workspace (intermediate-result 42))
!(match &workspace (intermediate-result $x) (* $x 2))

; Workspace can be discarded (GC'd)
```

### Module System

**Setup:**
```metta
; Module A
!(bind! &moduleA (new-space))
(add-atom &moduleA (= (funcA $x) (* $x 2)))

; Module B
!(bind! &moduleB (new-space))
(add-atom &moduleB (= (funcB $x) (+ $x 1)))

; Import: copy specific atoms
!(match &moduleA (= (funcA $x) $r)
    (add-atom &self (= (funcA $x) $r)))
```

### Testing

**Setup:**
```metta
; Test in isolated space
!(bind! &test-space (new-space))

; Add test data
(add-atom &test-space (test-fact 1))

; Run tests
!(match &test-space (test-fact $x)
    (assert (== $x 1)))

; Clean up (discard space)
```

## Error Handling

### Invalid Space Reference

**Error:**
```metta
!(get-atoms &nonexistent)
; Error: undefined variable &nonexistent
```

**Prevention**: Always create space before use
```metta
!(bind! &myspace (new-space))
!(get-atoms &myspace)  ; OK
```

### Type Errors

**Error:**
```metta
!(get-atoms not-a-space)
; Error: Expected Space type
```

**Prevention**: Ensure argument is Space type

### Empty Match Results

**Not an error**:
```metta
!(match &self (nonexistent $x) $x)
; → []  (empty list, not an error)
```

**Pattern**: Check for empty results
```metta
!(let $results (match &self (data $x) $x)
    (if (empty? $results)
        (print "No matches")
        (process $results)))
```

## Best Practices

### 1. Use Descriptive Names

```metta
; Good
!(bind! &user-database (new-space))
!(bind! &query-cache (new-space))

; Avoid
!(bind! &s1 (new-space))
!(bind! &temp (new-space))
```

### 2. Limit Space Scope

```metta
; Good: local scope
(= (process-data)
    (let &temp (new-space)
        (add-atom &temp ...)
        (match &temp ...)))

; Avoid: global proliferation
!(bind! &global1 (new-space))
!(bind! &global2 (new-space))
...
```

### 3. Document Space Purpose

```metta
; Create space for user authentication data
; Contains: (user <username> <hash>)
!(bind! &auth-db (new-space))
```

### 4. Clean Up Unused Spaces

**MeTTa uses garbage collection**, but help it:
```metta
; Let spaces go out of scope when done
(= (temporary-work)
    (let &workspace (new-space)
        ...  ; Use workspace
        result))  ; workspace GC'd after function
```

### 5. Use match Over get-atoms

```metta
; Good
!(match &self (Human $x) $x)

; Avoid (unless you need all atoms)
!(match (get-atoms &self) (Human $x) $x)
```

## Related Documentation

- **[Adding Atoms](01-adding-atoms.md)** - add-atom operation
- **[Removing Atoms](02-removing-atoms.md)** - remove-atom operation
- **[Space Structure](06-space-structure.md)** - Internal implementation
- **[Facts](03-facts.md)** - Working with facts
- **[Rules](04-rules.md)** - Working with rules

## Examples

See **[examples/03-space-operations.metta](examples/03-space-operations.metta)** for executable examples of:
- Creating multiple spaces
- Querying with match
- Copying between spaces
- Space isolation patterns

## Summary

**Primary Operations:**
- `new-space` - Create independent spaces
- `get-atoms` - Retrieve all atoms
- `match` - Pattern matching queries
- `add-atom` - Add atoms (detailed in 01-adding-atoms.md)
- `remove-atom` - Remove atoms (detailed in 02-removing-atoms.md)

**Key Capabilities:**
✅ Multiple independent spaces
✅ Pattern matching with variables
✅ First-class space values
✅ Cross-space operations
✅ Observer pattern (Rust API)

**Patterns:**
- Space isolation for modularity
- Temporary workspaces
- Caching and optimization
- Versioning and snapshots

**Performance:**
- Use ground prefixes in patterns
- Partition large spaces
- Leverage trie indexing
- Avoid full scans

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
