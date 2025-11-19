# Match Operation

## Overview

The `match` operation is the primary mechanism for querying atom spaces using patterns in MeTTa. It combines pattern matching with result construction, allowing expressive queries that extract and transform data from knowledge bases.

## Match Operation Specification

### Syntax

```metta
(match <space> <pattern> <template>)
```

**Parameters:**
- `<space>` - Atom space to query (e.g., `&self`, `&my-space`)
- `<pattern>` - Pattern to match against atoms in space
- `<template>` - Expression with variables to be substituted

**Returns**: List of results from evaluating template with each match's bindings

### Type Signature

**Location**: `lib/src/metta/runner/stdlib/core.rs:143-144`

```rust
(-> SpaceType Atom Atom %Undefined%)
```

**Meaning:**
- Takes a space, pattern atom, and template atom
- Returns `%Undefined%` (any type, depends on template)

### Formal Semantics

**Operational Semantics:**
```
Space = {a₁, a₂, ..., aₙ}
∀i: match(aᵢ, Pattern) = σᵢ  (if successful)
results = [Template[σ₁], Template[σ₂], ..., Template[σₘ]]
──────────────────────────────────────────────────────────
(match Space Pattern Template) → results
```

**Process:**
1. Query space for atoms matching pattern
2. For each match, obtain variable bindings (σ)
3. Apply bindings to template
4. Evaluate template with bindings
5. Return all results

**Empty Result**: If no atoms match, returns empty list `[]`

## Implementation

### MatchOp Structure

**Location**: `lib/src/metta/runner/stdlib/core.rs:141-167`

**Definition:**
```rust
#[derive(Clone, Debug)]
pub struct MatchOp {
    space: DynSpace,
}

impl MatchOp {
    pub fn new(space: DynSpace) -> Self {
        Self { space }
    }
}
```

**Type**: `Grounded` operation executed by interpreter

### Execution Method

**Location**: `lib/src/metta/runner/stdlib/core.rs:155-166`

```rust
impl CustomExecute for MatchOp {
    fn execute_bindings(&self, args: &[Atom]) -> Result<BoxedIter<(Atom, Option<Bindings>)>, ExecError> {
        let space = Atom::as_gnd::<DynSpace>(&args[0])?;
        let pattern = &args[1];
        let template = &args[2];

        // Query space for matching atoms
        let results = space.borrow().query(pattern);

        // Map each binding set to (template, bindings)
        let results = results.into_iter()
            .map(move |b| (template.clone(), Some(b)));

        Ok(Box::new(results))
    }
}
```

**Key Steps:**
1. **Extract Arguments**: space, pattern, template from args
2. **Query Space**: Call `space.query(pattern)` → `BindingsSet`
3. **Map to Templates**: Pair template with each binding
4. **Return Iterator**: Lazy evaluation of results

**Return Type**: Iterator of `(Atom, Option<Bindings>)` pairs
- The interpreter applies bindings to atoms during evaluation

### Space Query

**Trait Method** - `hyperon-space/src/lib.rs:156-175`:
```rust
pub trait Space {
    fn query(&self, pattern: &Atom) -> BindingsSet;
}
```

**GroundingSpace Implementation** - `lib/src/space/grounding/mod.rs:147-163`:
```rust
impl Space for GroundingSpace {
    fn query(&self, pattern: &Atom) -> BindingsSet {
        self.index.query(pattern)
    }
}
```

**AtomIndex Query** - `hyperon-space/src/index/mod.rs:189-193`:
```rust
pub fn query(&self, pattern: &Atom) -> BindingsSet {
    match &self.trie {
        Some(trie) => trie.query(pattern),
        None => BindingsSet::empty()
    }
}
```

**See**: `../atom-space/06-space-structure.md` for trie details

## Match Execution Flow

### Step-by-Step Process

**1. Parse Arguments:**
```
Input: (match &self (Human $x) $x)
Parse: space=&self, pattern=(Human $x), template=$x
```

**2. Query Space:**
```
Space contents: [(Human Socrates), (Human Plato), (age John 30)]
Query: (Human $x)
Trie traversal finds: [(Human Socrates), (Human Plato)]
```

**3. Generate Bindings:**
```
Match (Human $x) with (Human Socrates) → {$x ← Socrates}
Match (Human $x) with (Human Plato) → {$x ← Plato}
Result: BindingsSet = [{$x ← Socrates}, {$x ← Plato}]
```

**4. Apply to Template:**
```
Template: $x
Binding 1: {$x ← Socrates} → Socrates
Binding 2: {$x ← Plato} → Plato
```

**5. Return Results:**
```
Output: [Socrates, Plato]
```

### Evaluation Timeline

```
match called
    ↓
MatchOp.execute_bindings()
    ↓
space.query(pattern)
    ↓
AtomIndex.query()
    ↓
Trie traversal + unification
    ↓
BindingsSet returned
    ↓
Iterator of (template, bindings) pairs
    ↓
Interpreter applies bindings
    ↓
Template evaluation (recursive)
    ↓
Results returned
```

## Pattern Matching in Queries

### Simple Variable Extraction

**Example:**
```metta
; Find all humans
!(match &self
    (Human $x)
    $x)
; → [Socrates, Plato, Aristotle]
```

**Process:**
- Pattern: `(Human $x)` - matches any human
- Template: `$x` - extracts the name
- Result: List of all human names

### Multiple Variable Extraction

**Example:**
```metta
; Find ages
!(match &self
    (age $person $years)
    ($person is $years years old))
; → [(Socrates is 70 years old), (Plato is 80 years old)]
```

**Process:**
- Pattern: `(age $person $years)` - matches age facts
- Template: `($person is $years years old)` - constructs result
- Bindings applied to entire template

### Constant Template

**Example:**
```metta
; Check if any humans exist
!(match &self
    (Human $x)
    found)
; → [found, found, found]  (one per match)
```

**Note**: Template can be constant (no variables)

### Nested Extraction

**Example:**
```metta
; Extract city from address
!(match &self
    (person $name (address (city $c) $rest))
    ($name lives in $c))
```

**Process:**
- Deep pattern matching in nested structures
- Multiple variables at different depths
- All extracted in one query

## Template Evaluation

### Simple Substitution

**Behavior**: Variables in template replaced with bound values.

**Example:**
```metta
; Pattern: (Human $x)
; Bindings: {$x ← Socrates}
; Template: $x
; Result: Socrates
```

### Expression Construction

**Behavior**: Build new expressions from bindings.

**Example:**
```metta
; Pattern: (parent $p $c)
; Bindings: {$p ← Alice, $c ← Bob}
; Template: ($c has parent $p)
; Result: (Bob has parent Alice)
```

### Computed Templates

**Behavior**: Templates can contain function calls.

**Example:**
```metta
; Pattern: (number $n)
; Bindings: {$n ← 5}
; Template: (* $n 2)
; Result: 10  (evaluated)
```

**Process:**
1. Substitute: `(* 5 2)`
2. Evaluate: `10`

### Complex Templates

**Example:**
```metta
; Find grandchildren
!(match &self
    (parent $gp $p)
    (match &self
        (parent $p $gc)
        ($gp grandparent of $gc)))
```

**Process:**
- Outer match binds `$gp` and `$p`
- Template contains another match (uses `$p`)
- Inner match finds grandchildren
- Nested evaluation

## Match Variants and Patterns

### match with Ground Prefixes

**Pattern**: Start with concrete symbols for efficiency.

**Example:**
```metta
; Efficient (ground prefix "Human")
!(match &self (Human $x) $x)

; Less efficient (variable prefix)
!(match &self ($relation Socrates) $relation)
```

**Reason**: Trie can prune branches with ground terms

### match with Multiple Variables

**Example:**
```metta
!(match &self
    (edge $from $to $weight)
    (path from $from to $to weight $weight))
```

**Extracts**: All three fields from edge facts

### match with Shared Variables

**Example:**
```metta
; Find self-loops
!(match &self
    (edge $node $node)
    $node)
```

**Constraint**: Both positions must match same value

### match with Wildcards

**Example:**
```metta
; Ignore middle field
!(match &self
    (record $id $_ $data)
    ($id $data))
```

**Behavior**: `$_` matches anything, not used in template

## Conjunction Queries

### Comma Operator

**Syntax**: `(, <pattern1> <pattern2> ... <patternN>)`

**Semantics**: All patterns must match with consistent bindings.

**Example:**
```metta
; Find human philosophers
!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
```

**Process:**
1. Match `(Human $x)` → bindings for all humans
2. For each binding, try `(philosopher $x)`
3. Only return where both match

**Implementation**: `hyperon-space/src/lib.rs:340-368`

### Multi-Pattern Example

**Example:**
```metta
; Find people and their ages
!(match &self
    (, (person $name)
       (age $name $years))
    ($name is $years))
```

**Binding**: `$name` must be consistent across patterns

### Nested Conjunction

**Example:**
```metta
; Find grandparents
!(match &self
    (, (parent $gp $p)
       (parent $p $gc))
    ($gp grandparent of $gc))
```

**Process:**
- Find all parent pairs
- For each, find children of second parent
- Build grandparent relationships

## Query Optimization

### Trie-Based Pruning

**Mechanism**: Ground terms in pattern prune search space.

**Example:**
```metta
; Pattern: (Human $x)
; Trie traversal: root → "(" → "Human" → (collect atoms)
; Skips: All non-Human atoms
```

**Benefit**: Sublinear query time for specific patterns

### Query Complexity

**Best Case** (fully ground):
- O(log n) - exact trie lookup
- Example: `(Human Socrates)`

**Average Case** (partial ground):
- O(m) where m = matching atoms
- Example: `(Human $x)` - traverse Human branch

**Worst Case** (pure variable):
- O(n) - check all atoms
- Example: `$anything`

**See**: `06-space-structure.md` for trie details

### Optimization Strategies

**1. Ground Prefixes:**
```metta
; Good
!(match &self (Human $x) $x)

; Avoid
!(match &self ($relation $x) $x)
```

**2. Specific Patterns:**
```metta
; Good (narrows search)
!(match &self (age Socrates $y) $y)

; Less specific
!(match &self (age $x $y) ($x $y))
```

**3. Order in Conjunctions:**
```metta
; Good (specific first)
!(match &self
    (, (rare-relation $x)
       (common-relation $x))
    $x)
```

## Error Handling

### Missing Arguments

**Error:**
```metta
!(match &self (Human $x))
; Error: match expects 3 arguments
```

**Implementation** checks argument count.

### Invalid Space

**Error:**
```metta
!(match not-a-space (Human $x) $x)
; Error: Expected Space type
```

### Unbound Variables in Template

**Behavior**: Template variables not in pattern remain unbound.

**Example:**
```metta
!(match &self (Human $x) $y)
; $y is unbound → may error or return $y as-is
```

**Best Practice**: Only use pattern variables in template

### Empty Results

**Not an Error:**
```metta
!(match &self (nonexistent $x) $x)
; → []  (empty list, valid result)
```

## Match vs Other Operations

### match vs unify

**match**: Queries space, returns all matches
```metta
!(match &self (Human $x) $x)
; → [Socrates, Plato, ...]
```

**unify**: Tests single atom against pattern
```metta
!(unify (Human Socrates) (Human $x) $x "no match")
; → Socrates
```

**See**: [05-pattern-contexts.md](05-pattern-contexts.md#unify)

### match vs get-atoms

**match**: Pattern-based query
```metta
!(match &self (Human $x) $x)
; → [Socrates, Plato]
```

**get-atoms**: Returns all atoms
```metta
!(get-atoms &self)
; → [(Human Socrates), (Human Plato), (age John 30), ...]
```

**Difference**: match filters and extracts; get-atoms returns everything

## Common Patterns

### Existence Check

```metta
; Check if pattern matches anything
!(if (match &self (Human $x) True)
    (print "Humans exist")
    (print "No humans"))
```

### Counting Matches

```metta
; Count results
!(let $humans (match &self (Human $x) $x)
    (length $humans))
```

### Filtering

```metta
; Filter with condition
!(match &self
    (age $person $years)
    (if (> $years 50)
        $person
        ()))
```

### Aggregation

```metta
; Collect related data
!(match &self
    (person $name)
    (collect-facts $name))

(= (collect-facts $name)
    (match &self
        (property $name $prop $val)
        ($prop $val)))
```

### Transformation

```metta
; Transform data
!(match &self
    (celsius $temp)
    (fahrenheit (+ 32 (* 1.8 $temp))))
```

## Performance Considerations

### Query Cost

**Factors:**
- Space size (n)
- Pattern specificity
- Number of matches (m)
- Template complexity

**Time Complexity:**
- Query: O(k + m) typical (k = trie depth, m = matches)
- Per match: O(|pattern|) unification
- Template evaluation: O(|template|) per result

### Memory Usage

**Bindings Storage:**
- O(v × m) where v = variables, m = matches

**Result Construction:**
- O(m × |result|) for all results

### Optimization Tips

**1. Minimize Space Size:**
- Use multiple smaller spaces instead of one large space
- Partition data by type

**2. Specific Patterns:**
- More ground terms = faster queries

**3. Efficient Templates:**
- Avoid heavy computation in templates
- Keep templates simple

**4. Early Filtering:**
- Use conjunction to filter early
- Put most specific pattern first

## Best Practices

### 1. Clear Patterns

```metta
; Good (clear intent)
!(match &self (Human $x) $x)

; Avoid (unclear)
!(match &self $anything $anything)
```

### 2. Meaningful Templates

```metta
; Good (descriptive)
!(match &self (parent $p $c) ($p is parent of $c))

; Avoid (obscure)
!(match &self (parent $p $c) ($c $p))
```

### 3. Handle Empty Results

```metta
!(let $results (match &self (Human $x) $x)
    (if (empty? $results)
        (print "No matches")
        (process $results)))
```

### 4. Avoid Side Effects in Templates

```metta
; Avoid
!(match &self (data $x) (print $x))  ; Side effect per match

; Better
!(let $data (match &self (data $x) $x)
    (for-each $data print))
```

### 5. Use Type Annotations

```metta
; Document expected types
(: find-humans (-> Space (List Atom)))
(= (find-humans $space)
    (match $space (Human $x) $x))
```

## Related Operations

**Space Operations**: `../atom-space/05-space-operations.md`
**Unification**: [02-unification.md](02-unification.md)
**Bindings**: [04-bindings.md](04-bindings.md)
**Pattern Contexts**: [05-pattern-contexts.md](05-pattern-contexts.md)

## Summary

**Match Operation:**
- **Syntax**: `(match <space> <pattern> <template>)`
- **Purpose**: Query space with pattern, construct results
- **Process**: Query → Unify → Bind → Substitute → Evaluate
- **Returns**: List of template evaluations

**Key Features:**
✅ Pattern-based queries
✅ Variable extraction and binding
✅ Result construction with templates
✅ Multiple matches returned
✅ Trie-optimized queries

**Implementation:**
- Location: `lib/src/metta/runner/stdlib/core.rs:141-167`
- Uses: Space.query(), unification, bindings
- Optimization: Trie-based indexing

**Best Practices:**
- Use ground prefixes for efficiency
- Clear, meaningful patterns and templates
- Handle empty results gracefully
- Avoid side effects in templates

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
