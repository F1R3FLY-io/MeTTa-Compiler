# Pattern Matching Contexts

## Overview

Pattern matching in MeTTa appears in multiple contexts beyond the basic `match` operation. This document explores all the contexts where patterns are used, including queries, rule application, the `unify` operation, and destructuring.

## Pattern Matching Contexts

### 1. Match Operation (Queries)

**Primary Use**: Query atom spaces using patterns.

**Syntax:**
```metta
(match <space> <pattern> <template>)
```

**Example:**
```metta
!(match &self
    (Human $x)
    $x)
; → [Socrates, Plato, Aristotle]
```

**Characteristics:**
- Pattern matches against atoms in space
- Variables in pattern get bound
- Template evaluated for each match
- Returns list of results

**See**: [03-match-operation.md](03-match-operation.md)

### 2. Rule Application

**Use**: Defining rewrite rules.

**Syntax:**
```metta
(= <pattern> <result>)
```

**Example:**
```metta
; Define rule
(= (mortal $x) (Human $x))

; When evaluating (mortal Socrates):
; 1. Match (mortal Socrates) against (mortal $x)
; 2. Get binding {$x ← Socrates}
; 3. Substitute into (Human $x) → (Human Socrates)
; 4. Evaluate (Human Socrates)
```

**Process:**
1. Evaluator encounters expression `(mortal Socrates)`
2. Searches space for rules matching pattern `(mortal $x)`
3. Unifies expression with rule pattern
4. Applies bindings to rule result
5. Evaluates substituted result

**Multiple Matching Rules:**
```metta
(= (color) red)
(= (color) green)
(= (color) blue)

!(color)
; May return: red, green, or blue (non-deterministic)
```

**See**: `../atom-space/04-rules.md`

### 3. Conjunction Queries

**Use**: Matching multiple patterns with shared variables.

**Syntax:**
```metta
(match <space> (, <pattern1> <pattern2> ... <patternN>) <template>)
```

**Example:**
```metta
; Find human philosophers
!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
; → [Socrates, Plato]
```

**Semantics:**
```
For each atom a₁ matching pattern₁ with bindings σ₁:
  For each atom a₂ matching pattern₂ with bindings σ₂:
    If merge(σ₁, σ₂) succeeds with σ:
      Return template[σ]
```

**Process:**
1. Match first pattern, get binding sets
2. For each binding, try matching second pattern
3. Merge compatible bindings
4. Continue for remaining patterns
5. Apply final bindings to template

**Implementation**: `hyperon-space/src/lib.rs:340-368`

### 4. Unify Operation

**Use**: Test if atom matches pattern conditionally.

**Syntax:**
```metta
(unify <atom> <pattern> <then> <else>)
```

**Semantics:**
- If atom unifies with pattern, evaluate `<then>` with bindings
- Otherwise, evaluate `<else>`

**Example:**
```metta
!(unify (Human Socrates) (Human $x)
    $x              ; then: return bound variable
    "no match")     ; else: return this
; → Socrates
```

**Difference from match:**
- Tests single atom (not space query)
- Conditional branching (then/else)
- Direct control over match failure

**Documentation**: `hyperon-experimental/lib/src/metta/runner/stdlib/stdlib.metta:80-88`

### 5. Case/Switch (If Available)

**Use**: Pattern matching with multiple branches.

**Syntax** (conceptual):
```metta
(case <atom>
    (<pattern1> <result1>)
    (<pattern2> <result2>)
    (<pattern3> <result3>))
```

**Note**: Check current MeTTa version for case statement support.

### 6. Destructuring (Implicit)

**Use**: Extract values from structures.

**Example:**
```metta
; In function definition
(= (get-name (person $n $age)) $n)

; Call
!(get-name (person Alice 30))
; Destructures: (person Alice 30) matches (person $n $age)
; Bindings: {$n ← Alice, $age ← 30}
; Returns: Alice
```

**Mechanism**: Pattern in function head matches arguments.

## Unify Operation Details

### Specification

**Syntax:**
```metta
(unify <atom> <pattern> <then-template> <else-template>)
```

**Type Signature:**
```
unify : Atom → Atom → Atom → Atom → %Undefined%
```

**Semantics:**
```
unify(atom, pattern) = σ
─────────────────────────────────
(unify atom pattern then else) → then[σ]

unify(atom, pattern) = ⊥
─────────────────────────────────
(unify atom pattern then else) → else
```

### Examples

**Simple Matching:**
```metta
!(unify 42 $x
    $x
    "no match")
; → 42
```

**Pattern with Structure:**
```metta
!(unify (Human Socrates) (Human $x)
    (name is $x)
    "not human")
; → (name is Socrates)
```

**Multiple Variables:**
```metta
!(unify (age John 30) (age $person $years)
    ($person is $years years old)
    "invalid")
; → (John is 30 years old)
```

**Match Failure:**
```metta
!(unify (Animal Dog) (Human $x)
    $x
    "not human")
; → "not human"
```

### Unify vs Match

**unify:**
- Tests single atom
- Has then/else branches
- Returns single result
- No space query

**match:**
- Queries atom space
- Returns list of results
- No explicit else handling
- Empty list on no matches

**When to use unify:**
- Testing specific atom
- Need explicit failure handling
- Conditional logic based on structure

**When to use match:**
- Querying knowledge base
- Need all matches
- Building result lists

## Pattern Matching in Rule Evaluation

### Rule Matching Process

**Specification:**

When evaluating expression `E`, the interpreter:
1. Searches space for rules `(= P R)`
2. For each rule, tries unify(E, P)
3. If successful with bindings σ, evaluates R[σ]
4. Returns all results (non-deterministic)

**Example:**
```metta
; Rule in space
(= (double $x) (* $x 2))

; Evaluation of (double 5)
; 1. Find rule (= (double $x) (* $x 2))
; 2. Unify (double 5) with (double $x)
; 3. Bindings: {$x ← 5}
; 4. Substitute: (* 5 2)
; 5. Evaluate: 10
; → 10
```

### Multiple Matching Rules

**Behavior**: All matching rules may be applied.

**Example:**
```metta
(= (ancestor $x $y) (parent $x $y))
(= (ancestor $x $z)
    (match &self
        (, (parent $x $y)
           (ancestor $y $z))
        True))

; Query: (ancestor Alice Charlie)
; May match first rule directly
; Or match second rule (transitive)
; Non-deterministic result
```

### Rule Precedence

**Note**: MeTTa does not enforce rule order or precedence.

**Implication**: All matching rules are potential solutions.

**Best Practice**: Use guards or non-overlapping patterns.

## Conjunction Query Details

### Multi-Pattern Matching

**Syntax:**
```metta
(match <space> (, <p1> <p2> ... <pN>) <template>)
```

**Semantics**: All patterns must match with consistent bindings.

**Algorithm:**
```
results = []
For each match of p1 with bindings σ₁:
  For each match of p2 with bindings σ₂:
    σ = merge(σ₁, σ₂)
    If σ is consistent:
      For each match of p3 with bindings σ₃:
        ...
          results.append(template[σ_final])
Return results
```

### Conjunction Examples

**Two Patterns:**
```metta
; Find human philosophers
!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
```

**Process:**
1. Match `(Human $x)` → `{$x ← Socrates}`, `{$x ← Plato}`, `{$x ← Aristotle}`
2. For `{$x ← Socrates}`:
   - Match `(philosopher Socrates)` → success
   - Keep Socrates
3. For `{$x ← Plato}`:
   - Match `(philosopher Plato)` → success
   - Keep Plato
4. For `{$x ← Aristotle}`:
   - Match `(philosopher Aristotle)` → fails
   - Discard

**Three Patterns:**
```metta
; Find grandparents
!(match &self
    (, (parent $gp $p)
       (parent $p $gc)
       (Human $gp))
    ($gp grandparent of $gc))
```

**Shared Variables:**
```metta
; Find symmetric relations
!(match &self
    (, (friend $a $b)
       (friend $b $a))
    (mutual friends $a $b))
```

### Conjunction Optimization

**Order Matters** (for performance):

**Better:**
```metta
; Specific pattern first (fewer matches)
!(match &self
    (, (rare-property $x)
       (common-property $x))
    $x)
```

**Worse:**
```metta
; General pattern first (many matches)
!(match &self
    (, (common-property $x)
       (rare-property $x))
    $x)
```

**Reason**: Reduces intermediate bindings to check.

## Destructuring Patterns

### In Function Definitions

**Syntax:**
```metta
(= (<function-name> <pattern-args>) <result>)
```

**Example:**
```metta
; Extract first element
(= (first ($h $t...)) $h)

; Extract from record
(= (get-age (person $name $age)) $age)

; Nested destructuring
(= (get-city (address (city $c) $rest)) $c)
```

**Usage:**
```metta
!(first (1 2 3))
; Matches: (1 2 3) with ($h $t...)
; Bindings: {$h ← 1, $t ← (2 3)}
; Returns: 1

!(get-age (person Alice 30))
; Matches: (person Alice 30) with (person $name $age)
; Bindings: {$name ← Alice, $age ← 30}
; Returns: 30
```

### In Let Bindings (If Available)

**Syntax** (conceptual):
```metta
(let (pattern <value>) <body>)
```

**Example:**
```metta
(let (($x $y) (1 2))
    (+ $x $y))
; Destructures (1 2) into $x=1, $y=2
; Evaluates: (+ 1 2) → 3
```

**Note**: Check current MeTTa version for destructuring let support.

## Pattern Matching in Different Evaluation Contexts

### Eager Evaluation

**Behavior**: Arguments evaluated before pattern matching.

**Example:**
```metta
(= (process $x) (* $x 2))

!(process (+ 1 2))
; 1. Evaluate (+ 1 2) → 3
; 2. Match (process 3) with (process $x)
; 3. Bindings: {$x ← 3}
; 4. Result: (* 3 2) → 6
```

### Lazy Evaluation (If Applicable)

**Behavior**: Arguments passed unevaluated.

**Example** (if supported):
```metta
(= (quote $expr) $expr)

!(quote (+ 1 2))
; Pattern receives: (+ 1 2) unevaluated
; Returns: (+ 1 2)
```

### Meta-Type Patterns

**Use**: Control evaluation with Atom meta-type.

**Example:**
```metta
(: quote (-> Atom Atom))
(= (quote $expr) $expr)

!(quote (+ 1 2))
; Atom type prevents evaluation
; Returns: (+ 1 2)
```

**See**: `../type-system/06-advanced-features.md#meta-types`

## Pattern Context Summary

### Context Comparison

| Context | Purpose | Space Query | Multiple Results | Failure Handling |
|---------|---------|-------------|------------------|------------------|
| match | Query space | Yes | Yes (list) | Empty list |
| Rule | Define computation | Implicit | Yes (non-det) | No match → no result |
| unify | Test structure | No | No (single) | Explicit else |
| Conjunction | Multi-constraint | Yes | Yes (list) | Empty list |
| Destructure | Extract values | No | No | Match fail → error |

### Choosing the Right Context

**Use match when:**
- Querying knowledge base
- Need all matching atoms
- Building result lists

**Use rule when:**
- Defining functions
- Rewrite patterns
- Computation logic

**Use unify when:**
- Testing specific atom
- Need explicit failure handling
- Conditional branching

**Use conjunction when:**
- Multiple constraints
- Shared variables across patterns
- Relational queries

**Use destructuring when:**
- Extracting from known structure
- Function parameters
- Guaranteed match scenarios

## Advanced Context Patterns

### Nested Matches

**Pattern**: Match within match template.

**Example:**
```metta
; Find all edges from each node
!(match &self
    (node $n)
    (edges-from $n (match &self
                       (edge $n $to $w)
                       $to)))
```

### Match in Rule Results

**Pattern**: Use match in rule body.

**Example:**
```metta
(= (children $parent)
    (match &self
        (parent $parent $child)
        $child))
```

### Conditional Matching

**Pattern**: Combine unify with match.

**Example:**
```metta
(= (process $data)
    (unify $data (valid-format $x)
        (match &self
            (handler $x $h)
            (apply $h $x))
        (error "invalid format")))
```

### Higher-Order Patterns

**Pattern**: Patterns as arguments.

**Example:**
```metta
(= (filter-by $pattern)
    (match &self $pattern $pattern))

!(filter-by (Human $x))
; Returns all humans
```

## Best Practices

### 1. Choose Appropriate Context

```metta
; Good: match for queries
!(match &self (Human $x) $x)

; Avoid: using rules when match is clearer
```

### 2. Handle Match Failures

```metta
; Good: check for empty results
!(let $results (match &self (Human $x) $x)
    (if (empty? $results)
        (print "No humans")
        (process $results)))
```

### 3. Use unify for Single Tests

```metta
; Good: unify for single atom
!(unify $atom (expected-structure $x) $x "invalid")

; Avoid: match for single atom test
```

### 4. Optimize Conjunction Order

```metta
; Good: specific first
!(match &self
    (, (rare $x)
       (common $x))
    $x)

; Avoid: general first
!(match &self
    (, (common $x)
       (rare $x))
    $x)
```

### 5. Document Pattern Expectations

```metta
; Good: document expected structure
; (: get-name (-> Person Symbol))
; Expects: (person <name> <age>)
(= (get-name (person $n $a)) $n)
```

## Common Pitfalls

### 1. Unbound Variables

**Problem:**
```metta
!(match &self (Human $x) $y)
; $y not bound in pattern!
```

### 2. Match vs Unify Confusion

**Problem:**
```metta
; Trying to query space with unify
!(unify &self (Human $x) $x "no match")
; Won't work - unify doesn't query spaces
```

### 3. Assuming Rule Order

**Problem:**
```metta
(= (f $x) result1)  ; Assuming this checked first
(= (f $x) result2)  ; Then this
; Actually: both may match non-deterministically!
```

### 4. Forgetting Conjunction Semantics

**Problem:**
```metta
!(match &self
    (, (Human $x)
       (Dog $x))  ; Impossible - same $x!
    $x)
; Always returns empty (no atom is both Human and Dog)
```

## Related Documentation

**Match Operation**: [03-match-operation.md](03-match-operation.md)
**Rules**: `../atom-space/04-rules.md`
**Unification**: [02-unification.md](02-unification.md)
**Bindings**: [04-bindings.md](04-bindings.md)

## Summary

**Pattern Matching Contexts:**
- **match**: Query spaces with patterns
- **Rules**: Define computations via patterns
- **unify**: Test atoms conditionally
- **Conjunction**: Multi-pattern queries
- **Destructuring**: Extract from structures

**Key Differences:**
- match queries spaces, unify tests atoms
- Rules apply during evaluation
- Conjunction requires all patterns to match
- Destructuring expects specific structure

**Best Practices:**
- Choose appropriate context
- Handle failures gracefully
- Optimize conjunction order
- Document pattern expectations
- Use unify for conditional logic

**Common Contexts:**
✅ match - space queries
✅ Rules - function definitions
✅ unify - conditional testing
✅ Conjunction - multi-constraint
✅ Destructuring - parameter extraction

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
