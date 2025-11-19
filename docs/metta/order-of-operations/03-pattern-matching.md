# Pattern Matching Order

## Abstract

This document specifies the ordering semantics of pattern matching in MeTTa, including how queries are processed, how matches are collected, and the order in which matching results are returned. We analyze both the formal semantics and the implementation details from hyperon-experimental.

## Table of Contents

1. [Pattern Matching Fundamentals](#pattern-matching-fundamentals)
2. [Query Operation](#query-operation)
3. [Match Ordering](#match-ordering)
4. [Unification Order](#unification-order)
5. [Match Expression](#match-expression)
6. [Implementation Details](#implementation-details)
7. [Examples](#examples)

---

## Pattern Matching Fundamentals

### Definition

**Pattern matching** is the process of finding atoms in the space that match a given pattern, producing variable bindings for each match.

**Key Operations**:
- **Query**: Find all atoms matching a pattern
- **Match**: Unify two expressions, producing bindings
- **Unify**: Check if two expressions can be made equal by variable substitution

### Pattern Syntax

Patterns in MeTTa can contain:
- **Literals**: `A`, `42`, `"hello"` - match exactly
- **Variables**: `$x`, `$y` - match anything and bind
- **Expressions**: `(foo $x $y)` - match structure recursively

**Example Patterns**:
```metta
; Match any atom
$x

; Match specific symbol
foo

; Match expression with specific head
(bar $x $y)

; Match nested structure
(outer (inner $x) $y)
```

### Matching Semantics

**Definition**: An atom `a` matches pattern `p` under bindings `β` if there exists a substitution that makes them equal.

**Formal Relation**:
```
a ≡ₚ p ⇝ β
```

Reads as: "atom `a` matches pattern `p`, producing bindings `β`".

**Rules**:

1. **Variable Match**:
   ```
   (VAR)  a ≡ₚ $x ⇝ {$x ↦ a}
   ```

2. **Literal Match**:
   ```
          c = c'
   (LIT)  ────────────
          c ≡ₚ c' ⇝ {}
   ```

3. **Expression Match**:
   ```
          a₁ ≡ₚ p₁ ⇝ β₁   ...   aₙ ≡ₚ pₙ ⇝ βₙ
          β = β₁ ∪ ... ∪ βₙ  (if consistent)
   (EXPR) ────────────────────────────────────────
          (a₁ ... aₙ) ≡ₚ (p₁ ... pₙ) ⇝ β
   ```

**Consistency**: Bindings are consistent if the same variable is not bound to different values.

---

## Query Operation

### Equality-Based Queries

MeTTa queries use the equality symbol `=` to find matches.

**Syntax**:
```metta
!(match &space <pattern> <body>)
```

Internally transformed to:
```metta
(= <pattern> $X)
```

Where `$X` is a fresh variable that captures the match.

### Query Function

From `hyperon-experimental/lib/src/metta/interpreter.rs`:604-638:

```rust
fn query(space: &DynSpace, prev: Option<Rc<RefCell<Stack>>>, to_eval: Atom,
         bindings: Bindings, vars: Variables) -> Vec<InterpretedAtom> {
    let var_x = &VariableAtom::new("X").make_unique();
    let query = Atom::expr([EQUAL_SYMBOL, to_eval.clone(), Atom::Variable(var_x.clone())]);
    let results = space.borrow().query(&query);

    // Convert results to InterpretedAtom alternatives
    let results: Vec<InterpretedAtom> = results.into_iter().flat_map(|b| {
        b.merge(&bindings).into_iter()
    }).filter_map(move |b| {
        b.resolve(&var_x).map_or(None, |res| {
            if b.has_loops() {
                None
            } else {
                Some(result(res, b))
            }
        })
    })
    .collect();

    // ... handle no results case ...
}
```

**Key Steps**:
1. Create query pattern: `(= <to_eval> $X)`
2. Query space: `space.borrow().query(&query)`
3. Merge with existing bindings
4. Filter out loops
5. Return all alternatives

### Space Query Implementation

The `query` method on spaces returns an iterator of bindings.

**Interface**:
```rust
fn query(&self, pattern: &Atom) -> BindingsSet;
```

Where `BindingsSet` is an iterator over `Bindings`.

---

## Match Ordering

### Specification

**Question**: In what order are pattern matches returned?

**Specification Answer**: **Order is unspecified and implementation-dependent**.

The MeTTa specification does not guarantee any particular ordering of pattern matches. Programs should be written to work correctly regardless of match order.

### Implementation

The current implementation uses a trie-based `AtomIndex` to store atoms.

From `hyperon-experimental/lib/src/space/grounding/mod.rs`:

```rust
pub struct GroundingSpace {
    index: AtomIndex,
    // ...
}
```

**AtomIndex** is a trie data structure optimized for pattern matching.

#### Trie-Based Matching

A trie (prefix tree) organizes atoms by their structure:

```
Root
├─ foo
│  ├─ (foo A)
│  ├─ (foo B)
│  └─ (foo C)
├─ bar
│  └─ (bar X)
└─ ...
```

**Query Process**:
1. Traverse trie based on pattern structure
2. Collect all matching atoms
3. Return as iterator

**Order Characteristics**:
- **Insertion order**: NOT preserved
- **Lexicographic order**: NOT guaranteed
- **Trie traversal order**: Implementation-dependent
- **Stable**: Same query on same space should return same order (within a single run)

### Iterator-Based Collection

From `interpreter.rs`:604-638, matches are collected via:

```rust
let results: Vec<InterpretedAtom> = results.into_iter()
    .flat_map(|b| b.merge(&bindings).into_iter())
    .filter_map(move |b| {
        // ... process binding ...
    })
    .collect();
```

**Implications**:
- Iterator fusion may affect ordering
- `flat_map` and `filter_map` preserve iterator order
- `collect()` materializes results in iteration order

### Multiple Patterns

When multiple patterns match:

```metta
(= (color) red)
(= (color) green)
(= (color) blue)

!(color)
```

**Query**: `(= (color) $X)`

**Matches**:
- `(= (color) red)` with `{$X ↦ red}`
- `(= (color) green)` with `{$X ↦ green}`
- `(= (color) blue)` with `{$X ↦ blue}`

**Order**: Implementation-dependent (trie traversal order).

---

## Unification Order

### Unify Operation

The `unify` operation explicitly unifies two expressions.

**Syntax**:
```metta
(unify <atom> <pattern> <then> <else>)
```

**Semantics**:
- Try to unify `<atom>` with `<pattern>`
- If successful, evaluate `<then>` with bindings
- If unsuccessful, evaluate `<else>`

### Implementation

From `interpreter.rs`:809-841:

```rust
fn unify(stack: Stack, bindings: Bindings) -> Vec<InterpretedAtom> {
    let (atom, pattern, then, else_) = match_atom!{
        unify ~ [_op, atom, pattern, then, else_] => (atom, pattern, then, else_),
        // ...
    };

    let matches: Vec<Bindings> = match_atoms(&atom, &pattern).collect();

    // If matches found, create alternatives
    let matches: Vec<InterpretedAtom> = matches.into_iter().flat_map(move |b| {
        b.merge(bindings_ref).into_iter().filter_map(move |b| {
            if b.has_loops() {
                None
            } else {
                Some(result(b))
            }
        })
    })
    .collect();

    // ... handle then/else branches ...
}
```

**Key Function**: `match_atoms(&atom, &pattern)` returns an iterator of bindings.

### match_atoms Function

**Purpose**: Unify two atoms structurally.

**Returns**: Iterator of all possible unifications.

**Example**:
```rust
match_atoms(&Atom::expr([sym!("foo"), sym!("A")]),
            &Atom::expr([sym!("foo"), var!("x")]))
// Returns: [{$x ↦ A}]

match_atoms(&Atom::expr([var!("x"), var!("y")]),
            &Atom::expr([sym!("A"), sym!("B")]))
// Returns: [{$x ↦ A, $y ↦ B}]
```

### Multiple Unifications

In some cases, multiple unifications are possible:

```metta
; Unify two variables
(unify ($x $y) ($a $b) <then> <else>)
```

**Possible Bindings**:
- `{$x ↦ $a, $y ↦ $b}`
- `{$x ↦ $b, $y ↦ $a}` (if symmetric)
- ... potentially more

**Order**: Determined by `match_atoms` implementation.

---

## Match Expression

### Match Syntax

The `match` expression provides pattern matching with case analysis:

**Syntax**:
```metta
!(match &space <pattern> <body>)
```

**Semantics**:
1. Query space for atoms matching `<pattern>`
2. For each match, bind variables and evaluate `<body>`
3. Return all results (non-deterministic)

### Implementation

The `match` operation is syntactic sugar for query + evaluation.

**Expansion**:
```metta
!(match &space (foo $x) (bar $x))
```

Expands to:
```metta
!(let $X (query &space (= (foo $x) $X))
     (bar $x))
```

### Match Alternatives

Each match creates an alternative:

```metta
; Space contains: (foo 1), (foo 2), (foo 3)
!(match &space (foo $x) (* $x 2))
```

**Evaluation**:
```
Query: (= (foo $x) $X)
Matches: (foo 1), (foo 2), (foo 3)

Alternatives:
  Branch 1: {$x ↦ 1} → (* 1 2) → 2
  Branch 2: {$x ↦ 2} → (* 2 2) → 4
  Branch 3: {$x ↦ 3} → (* 3 2) → 6

Results: {2, 4, 6}
```

**Order**: Depends on query result ordering (unspecified).

---

## Implementation Details

### AtomIndex Structure

The `AtomIndex` is a trie-based data structure for efficient pattern matching.

**Key Properties**:
- **Prefix-based**: Organizes atoms by symbol prefix
- **Efficient lookups**: O(pattern depth) for queries
- **No guaranteed ordering**: Iteration order is implementation-dependent

### Query Algorithm

**Pseudocode**:
```
function query_space(space, pattern):
    matches = []

    for atom in space.index.iter():
        if bindings = try_match(atom, pattern):
            matches.append(bindings)

    return matches
```

**Actual Implementation**: More efficient, prunes trie based on pattern structure.

### Binding Merging

When combining bindings from multiple sources:

From `interpreter.rs` query function:
```rust
results.into_iter().flat_map(|b| {
    b.merge(&bindings).into_iter()
})
```

**Merge Semantics**:
- If bindings are consistent, return merged binding
- If bindings conflict, return no result (filter out)

**Example**:
```
β₁ = {$x ↦ A}
β₂ = {$y ↦ B}
β₁.merge(β₂) = {$x ↦ A, $y ↦ B}

β₃ = {$x ↦ A}
β₄ = {$x ↦ B}
β₃.merge(β₄) = ∅  (conflict on $x)
```

### Loop Detection

From `interpreter.rs`:619-623:
```rust
.filter_map(move |b| {
    if b.has_loops() {
        None
    } else {
        Some(result(res, b))
    }
})
```

**Purpose**: Prevent infinite loops from circular bindings.

**Example of Loop**:
```
{$x ↦ (foo $x)}  ; $x refers to itself
```

---

## Examples

### Example 1: Simple Pattern Match

**Space**:
```metta
(foo A)
(foo B)
(bar C)
```

**Query**:
```metta
!(match &space (foo $x) $x)
```

**Evaluation**:
```
Pattern: (foo $x)
Matches:
  (foo A) → {$x ↦ A}
  (foo B) → {$x ↦ B}

Results: {A, B}  (order unspecified)
```

### Example 2: Nested Pattern Match

**Space**:
```metta
(outer (inner 1) X)
(outer (inner 2) Y)
(outer (other 3) Z)
```

**Query**:
```metta
!(match &space (outer (inner $x) $y) (pair $x $y))
```

**Evaluation**:
```
Pattern: (outer (inner $x) $y)
Matches:
  (outer (inner 1) X) → {$x ↦ 1, $y ↦ X}
  (outer (inner 2) Y) → {$x ↦ 2, $y ↦ Y}
  ; (outer (other 3) Z) does NOT match

Results: {(pair 1 X), (pair 2 Y)}
```

### Example 3: Multiple Variables

**Space**:
```metta
(edge A B)
(edge B C)
(edge C D)
```

**Query**:
```metta
!(match &space (edge $x $y) (path $x $y))
```

**Evaluation**:
```
Pattern: (edge $x $y)
Matches:
  (edge A B) → {$x ↦ A, $y ↦ B}
  (edge B C) → {$x ↦ B, $y ↦ C}
  (edge C D) → {$x ↦ C, $y ↦ D}

Results: {(path A B), (path B C), (path C D)}
```

### Example 4: Binding Conflicts

**Query**:
```metta
; Try to match ($x $x) with (A B)
(unify (A B) ($x $x) success failure)
```

**Evaluation**:
```
Try to unify:
  A with $x → {$x ↦ A}
  B with $x → requires {$x ↦ B}

Conflict: $x cannot be both A and B
Result: failure branch executed
```

### Example 5: Order Dependence

**Space** (atoms added in this order):
```metta
(item zebra)
(item apple)
(item monkey)
(item banana)
```

**Query**:
```metta
!(match &space (item $x) $x)
```

**Possible Results** (order unspecified):
- Option 1: `{zebra, apple, monkey, banana}` (insertion order)
- Option 2: `{apple, banana, monkey, zebra}` (alphabetical order)
- Option 3: Some other order (trie traversal order)

**Implementation**: Current implementation likely gives trie traversal order, not insertion or alphabetical.

### Example 6: Unify with Multiple Solutions

**Query**:
```metta
; Variables can unify in multiple ways
(unify ($x $y) ($y $x) (success $x $y) failure)
```

**Evaluation**:
```
Try to unify:
  $x with $y
  $y with $x

Possible bindings:
  {$x ↦ $y} (forward)
  {$y ↦ $x} (backward)

Both create circular bindings → filtered by has_loops()
Result: failure branch executed
```

---

## Specification vs Implementation

| Aspect | Specification | Implementation |
|--------|--------------|----------------|
| **Match Order** | Unspecified | Trie traversal order |
| **Query Stability** | Not guaranteed | Stable within single run |
| **Pattern Syntax** | Variables, literals, expressions | Fully supported |
| **Unification** | Structural matching | Structural matching via `match_atoms` |
| **Binding Conflicts** | Undefined | Filtered out (no result) |
| **Circular Bindings** | Undefined | Filtered via `has_loops()` |
| **Multiple Matches** | All returned | All collected via iterator |

---

## Design Recommendations

For MeTTa compiler implementers:

### Ordering Guarantees

**Consider**:
1. **Stable Ordering**: Guarantee same results for same query (e.g., insertion order)
2. **Sorted Results**: Option to return matches in sorted order
3. **Limit + Offset**: Support pagination of large result sets

**Example API**:
```metta
; Get first 10 matches in insertion order
!(match &space (pattern $x) $x :limit 10 :order insertion)
```

### Performance

**Optimize**:
1. **Index Selection**: Choose appropriate index for query pattern
2. **Lazy Evaluation**: Return iterator instead of collecting all matches
3. **Memoization**: Cache query results for repeated patterns

### Debugging

**Provide**:
1. **Match Count**: Return number of matches without full evaluation
2. **Explain**: Show why patterns did/didn't match
3. **Trace**: Log match order for debugging

**Example**:
```metta
!(explain-match &space (pattern $x))
; Returns: "Matched 3 atoms: (foo 1) at index 5, (foo 2) at index 12, ..."
```

---

## References

### Source Code

- **`hyperon-experimental/lib/src/metta/interpreter.rs`**
  - `query()` function (lines 604-638): Query implementation
  - `unify()` function (lines 809-841): Unification operation

- **`hyperon-experimental/lib/src/space/grounding/mod.rs`**
  - `GroundingSpace`: Space implementation with AtomIndex

### Academic References

- **Robinson, J. A.** (1965). "A Machine-Oriented Logic Based on the Resolution Principle". *Journal of the ACM*.
- **Martelli, A. & Montanari, U.** (1982). "An Efficient Unification Algorithm". *ACM TOPLAS*.
- **Clocksin, W. F. & Mellish, C. S.** (2003). *Programming in Prolog*. Springer.

---

## See Also

- **§01**: Evaluation order (how matches affect evaluation)
- **§02**: Mutation order (mutations during pattern matching)
- **§04**: Reduction order (pattern-based reductions)
- **§05**: Non-determinism (multiple matches create branches)

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
