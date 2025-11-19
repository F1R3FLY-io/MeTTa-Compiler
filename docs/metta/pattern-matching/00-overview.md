# MeTTa Pattern Matching - Overview

## Executive Summary

Pattern matching is a fundamental mechanism in MeTTa that enables querying, destructuring, and transforming symbolic expressions. Unlike simple equality testing, pattern matching uses **unification** to match structured data against patterns containing variables, binding those variables to matched values.

**Key Characteristics:**
- **Bidirectional Unification**: Both sides can contain variables
- **Structural Matching**: Matches shape and content of expressions
- **Variable Binding**: Creates substitutions mapping variables to values
- **Non-Deterministic**: Multiple matches produce multiple results
- **Trie-Optimized**: Efficient indexing for large knowledge bases

**Primary Operations:**
- `match` - Query space with pattern, substitute into template
- `unify` - Test if atom matches pattern with conditional execution
- Space queries - Pattern-based atom retrieval

## What is Pattern Matching?

### Specification

**Pattern Matching** is the process of checking whether a given atom conforms to a specified pattern, and if so, extracting variable bindings that make the match valid.

**Formal Definition:**
```
match(atom, pattern) → {bindings} | ∅
where bindings: Variables → Atoms
```

**Unification**: MeTTa uses bidirectional unification, meaning variables can appear on both sides and are bound symmetrically.

### Implementation

**Location**: `hyperon-atom/src/matcher.rs`

**Core Function** - `hyperon-atom/src/matcher.rs:1089-1099`:
```rust
pub fn match_atoms(left: &Atom, right: &Atom) -> MatchResultIter {
    let result = match_atoms_recursively(left, right, Bindings::new());
    Box::new(result.into_iter()
        .filter(|b| !has_loops(b)))  // Filter cyclic bindings
}
```

**Key Types:**
- `Bindings` - Maps variables to atoms
- `BindingsSet` - Set of possible bindings (multiple matches)
- `MatchResultIter` - Iterator over binding solutions

## Pattern Types

### Variable Patterns

**Syntax**: `$variable_name`

**Examples:**
```metta
$x          ; Matches any atom
$person     ; Matches any atom
$_          ; Wildcard (conventionally ignored)
```

**Behavior:**
- Matches any atom
- Binds variable to matched atom
- Same variable in pattern must match same value

**Example:**
```metta
; Pattern: (Human $x)
; Matches: (Human Socrates) with binding {$x ← Socrates}
```

### Ground Patterns

**Ground Terms** are literal values that must match exactly.

**Examples:**
```metta
Socrates        ; Symbol - exact match
42              ; Number - exact match
"hello"         ; String - exact match
True            ; Boolean - exact match
```

**Behavior:**
- No variables
- Requires exact structural equality
- No bindings created

### Expression Patterns

**Nested Structures** with sub-patterns.

**Examples:**
```metta
(Human $x)                          ; Match any human
(age $person $years)                ; Binary relation
(implies (Frog $x) (Green $x))      ; Nested with shared variable
```

**Behavior:**
- Recursively match sub-expressions
- Must have same arity (number of elements)
- Variables can appear multiple times

## The Match Operation

### Syntax

```metta
(match <space> <pattern> <template>)
```

**Parameters:**
- `<space>` - Atom space to query (e.g., `&self`)
- `<pattern>` - Pattern to match against atoms in space
- `<template>` - Expression with variables to substitute

**Returns**: Results of evaluating `<template>` for each match

### Semantics

**Operational Semantics:**
```
Space = {a₁, a₂, ..., aₙ}
───────────────────────────────────────────
(match Space Pattern Template) →
  [Template[σ₁], Template[σ₂], ..., Template[σₘ]]

where σᵢ are bindings such that match(aⱼ, Pattern) = σᵢ
```

**Process:**
1. Query space for atoms matching pattern
2. For each matching atom, create variable bindings
3. Substitute bindings into template
4. Evaluate and return results

### Implementation

**Location**: `lib/src/metta/runner/stdlib/core.rs:141-167`

**MatchOp Implementation**:
```rust
impl CustomExecute for MatchOp {
    fn execute_bindings(&self, args: &[Atom]) -> Result<BoxedIter<(Atom, Option<Bindings>)>> {
        let space = Atom::as_gnd::<DynSpace>(&args[0])?;
        let pattern = &args[1];
        let template = &args[2];

        // Query space for matches
        let results = space.borrow().query(pattern);

        // Map each binding to (template, bindings)
        let results = results.into_iter()
            .map(move |b| (template.clone(), Some(b)));

        Ok(Box::new(results))
    }
}
```

### Example

```metta
; Add facts to space
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

; Query with pattern
!(match &self
    (Human $x)      ; Pattern
    $x)             ; Template
; → [Socrates, Plato]
```

## Unification

### What is Unification?

**Unification** is a bidirectional matching process where both atoms can contain variables.

**Formal Definition:**
```
unify(atom₁, atom₂) → {σ} | ∅
where σ: Variables → Atoms
and atom₁[σ] = atom₂[σ]
```

**Key Property**: Symmetric - `unify(A, B) = unify(B, A)`

### Unification Rules

**Symbol-Symbol**:
```
sym₁ = sym₂
──────────────
unify(sym₁, sym₂) = {}
```

**Variable-Atom**:
```
$x is a variable, a is any atom
──────────────────────────────
unify($x, a) = {$x ← a}
```

**Variable-Variable**:
```
$x, $y are variables
────────────────────
unify($x, $y) = {$x = $y}  (equality constraint)
```

**Expression-Expression**:
```
len(e₁) = len(e₂)
∀i: σᵢ = unify(e₁[i], e₂[i])
σ = merge(σ₁, σ₂, ..., σₙ)
──────────────────────────────
unify(e₁, e₂) = σ
```

### Implementation

**Location**: `hyperon-atom/src/matcher.rs:1101-1129`

**Algorithm**:
```rust
fn match_atoms_recursively(left: &Atom, right: &Atom, bindings: Bindings)
    -> BindingsSet
{
    match (left, right) {
        // Variable cases
        (Atom::Variable(v), atom) | (atom, Atom::Variable(v)) => {
            bindings.add_var_binding(v.clone(), atom.clone())
        }

        // Symbol case
        (Atom::Symbol(l), Atom::Symbol(r)) if l == r => {
            BindingsSet::single(bindings)
        }

        // Expression case
        (Atom::Expression(l), Atom::Expression(r))
            if l.children().len() == r.children().len() =>
        {
            let mut result = BindingsSet::single(bindings);
            for (l_child, r_child) in l.children().iter().zip(r.children()) {
                result = result.merge_v2(l_child, r_child, match_atoms_recursively);
            }
            result
        }

        // No match
        _ => BindingsSet::empty()
    }
}
```

## Variable Bindings

### Bindings Data Structure

**Specification:**

A **Binding** maps variables to atoms or to other variables (equalities).

```
Bindings = {
    var_equalities: { $x₁ = $x₂ = ... = $xₙ },
    var_assignments: { $y ← atom₁, $z ← atom₂, ... }
}
```

### Implementation

**Location**: `hyperon-atom/src/matcher.rs:140-765`

**Structure**:
```rust
pub struct Bindings {
    // Two-level structure:
    // 1. Map variables to binding group IDs
    values: HashMap<VariableAtom, usize>,

    // 2. Binding groups (holes allow None for variable-only groups)
    bindings: HoleyVec<Binding>,
}

pub enum Binding {
    Empty,                    // No variables in group
    Var(VariableAtom),        // Single variable
    Link(usize),              // Pointer to another group
    Atom(Atom, usize),        // Bound to atom (atom, generation)
}
```

**Key Methods:**
- `add_var_binding($x, atom)` - Bind variable to atom
- `add_var_equality($x, $y)` - Assert variables are equal
- `resolve($x)` - Get bound value (follows chains)
- `merge(b1, b2)` - Combine compatible bindings

### Example

```metta
; Pattern: (same $x $x)
; Atom: (same A A)
; Bindings: {$x ← A}

; Pattern: ($x $y $x)
; Atom: (A B A)
; Bindings: {$x ← A, $y ← B}

; Pattern: ($a $b)
; Atom: ($x $y)
; Bindings: {$a = $x, $b = $y}  (variable equalities)
```

## Query Process

### Space Query

**Specification:**

Given a space S and pattern P, return all atoms in S that match P along with their bindings.

```
query(S, P) = {(a, σ) | a ∈ S ∧ unify(a, P) = σ}
```

### Implementation

**Space.query()** - `hyperon-space/src/lib.rs:156-175`:
```rust
pub trait Space {
    fn query(&self, pattern: &Atom) -> BindingsSet;
}
```

**GroundingSpace Implementation** - `lib/src/space/grounding/mod.rs:147-163`:
```rust
impl Space for GroundingSpace {
    fn query(&self, pattern: &Atom) -> BindingsSet {
        // Use AtomIndex for efficient trie-based query
        self.index.query(pattern)
    }
}
```

**AtomIndex.query()** - `hyperon-space/src/index/mod.rs:189-193`:
```rust
pub fn query(&self, pattern: &Atom) -> BindingsSet {
    match &self.trie {
        Some(trie) => trie.query(pattern),
        None => BindingsSet::empty()
    }
}
```

### Trie-Based Optimization

**Key Idea**: Index atoms by structure for fast pattern matching

**AtomTrie** - `hyperon-space/src/index/trie.rs`:
- Atoms decomposed into keys
- Trie structure allows prefix-based pruning
- Variables explore all branches
- Ground terms follow exact path

**Query Modes**:
- `AtomMatchMode::Equality` - Exact matching only
- `AtomMatchMode::Unification` - Variable matching enabled

**Complexity**:
- Ground pattern: O(log n) - follow exact path
- Variable pattern: O(n) - explore all branches
- Partial ground: O(m) - m = matching atoms

## Non-Determinism

### Multiple Matches

When multiple atoms match a pattern, all matches are returned.

**Example:**
```metta
(add-atom &self (color apple red))
(add-atom &self (color banana yellow))
(add-atom &self (color grape purple))

!(match &self
    (color $fruit $c)
    $fruit)
; → [apple, banana, grape]
```

### Multiple Solutions from Single Match

Unification can produce multiple binding sets.

**Example:**
```metta
; Custom matcher that returns two solutions
; See: lib/examples/custom_match.rs

; Pattern might split into:
; Bindings 1: {$x ← A}
; Bindings 2: {$x ← B}
```

### BindingsSet

**Specification:**

A `BindingsSet` represents multiple possible variable bindings.

```
BindingsSet = {σ₁, σ₂, ..., σₙ} ∪ {empty}
```

**Special Values:**
- `empty` - No matches (contradiction)
- `single(σ)` - One unconditional match

**Operations:**
- `merge(S₁, S₂)` - Cartesian product of binding sets
- `add_var_binding($x, a)` - May split into multiple bindings

## Pattern Matching Contexts

### In Queries

```metta
!(match &self
    (Human $x)
    $x)
```

### In Rule Application

```metta
; Rule definition
(= (mortal $x) (Human $x))

; When evaluating (mortal Socrates):
; 1. Match (mortal Socrates) against (mortal $x)
; 2. Get binding {$x ← Socrates}
; 3. Substitute into (Human $x) → (Human Socrates)
; 4. Evaluate result
```

### In Conjunction Queries

```metta
!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
; Finds all humans who are also philosophers
```

### In Unify Operation

```metta
!(unify (foo $x) (foo 42)
    $x              ; then: return binding
    "no match")     ; else: return this
; → 42
```

## Common Patterns

### Extracting Fields

```metta
; Extract name from person record
!(match &self
    (person (name $n) (age $a))
    $n)
```

### Filtering with Shared Variables

```metta
; Find parents and their children
!(match &self
    (parent $p $c)
    ($p has-child $c))
```

### Multi-Pattern Queries

```metta
; Find grandparents
!(match &self
    (, (parent $gp $p)
       (parent $p $c))
    ($gp grandparent-of $c))
```

### Nested Extraction

```metta
; Extract from deeply nested structure
!(match &self
    (company (address (city $c)))
    $c)
```

## Key Concepts Summary

### Unification
- **Bidirectional**: Variables on both sides
- **Symmetric**: unify(A, B) = unify(B, A)
- **Structural**: Matches shape and content

### Bindings
- **Variable Assignments**: `{$x ← atom}`
- **Variable Equalities**: `{$x = $y = $z}`
- **Resolution**: Follow chains to get final values
- **Merging**: Combine compatible bindings

### Non-Determinism
- **Multiple Matches**: One pattern, many atoms
- **Multiple Solutions**: One match, many bindings
- **BindingsSet**: Represents all possibilities

### Occurs Check
- **Loop Detection**: Prevents `$x ← (f $x)`
- **Filtering**: Cyclic bindings removed from results
- **Recursive**: Checks entire binding graph

## Implementation Overview

### Core Components

**Matcher** (`hyperon-atom/src/matcher.rs`):
- `match_atoms()` - Main entry point
- `match_atoms_recursively()` - Recursive algorithm
- `Bindings` - Variable binding structure
- `BindingsSet` - Multiple binding sets

**Match Operation** (`lib/src/metta/runner/stdlib/core.rs`):
- `MatchOp` - Grounded operation for match
- `execute_bindings()` - Query and substitute

**Space Query** (`hyperon-space/src/`):
- `Space::query()` - Trait method
- `AtomIndex` - Trie-based indexing
- `AtomTrie` - Efficient pattern matching

### Performance Characteristics

**Time Complexity:**
- Unification: O(size(atom))
- Space query (ground): O(log n)
- Space query (variable): O(n)
- Binding resolution: O(depth)

**Space Complexity:**
- Bindings: O(vars)
- BindingsSet: O(solutions × vars)
- Trie: O(total atom size) with sharing

## Documentation Structure

This overview provides the foundation. For detailed information:

- **[01-fundamentals.md](01-fundamentals.md)** - Pattern syntax and types
- **[02-unification.md](02-unification.md)** - Unification algorithm
- **[03-match-operation.md](03-match-operation.md)** - Match operation details
- **[04-bindings.md](04-bindings.md)** - Bindings implementation
- **[05-pattern-contexts.md](05-pattern-contexts.md)** - Usage contexts
- **[06-advanced-patterns.md](06-advanced-patterns.md)** - Advanced techniques
- **[07-implementation.md](07-implementation.md)** - Implementation details
- **[08-non-determinism.md](08-non-determinism.md)** - Multiple matches
- **[09-edge-cases.md](09-edge-cases.md)** - Edge cases and gotchas
- **[examples/](examples/)** - Executable examples

## Cross-References

**Related Documentation:**
- **Atom Space** - `../atom-space/05-space-operations.md` (match and query)
- **Rules** - `../atom-space/04-rules.md` (rule application uses patterns)
- **Order of Operations** - `../order-of-operations/` (evaluation order)

## Quick Reference

### Basic Match

```metta
!(match &self <pattern> <template>)
```

### Common Patterns

```metta
$x              ; Any atom
(f $x)          ; Expression with variable
(f A $x)        ; Mixed ground/variable
(f $x $x)       ; Shared variable
```

### Unify

```metta
!(unify <atom> <pattern> <then> <else>)
```

## Summary

**Pattern Matching in MeTTa:**
✅ Bidirectional unification
✅ Structural matching with variables
✅ Variable binding and substitution
✅ Non-deterministic (multiple results)
✅ Trie-optimized for efficiency

**Key Operations:**
- `match` - Query and substitute
- `unify` - Conditional matching
- Space queries - Pattern-based retrieval

**Core Concepts:**
- Unification algorithm
- Variable bindings
- BindingsSet for multiple matches
- Occurs check for cycles
- Trie-based indexing

Pattern matching is fundamental to MeTTa's symbolic reasoning, enabling expressive queries, rule-based inference, and knowledge base operations.

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
**Status**: Complete
