# Pattern Matching Fundamentals

## Overview

This document provides detailed coverage of pattern syntax, pattern types, and the basic mechanics of pattern matching in MeTTa. Understanding these fundamentals is essential for effective use of MeTTa's query and reasoning capabilities.

## Pattern Syntax

### What is a Pattern?

**Definition**: A pattern is an atom (or expression) that may contain variables, used to match against other atoms.

**Formal Definition:**
```
Pattern := Variable
         | Symbol
         | Number
         | String
         | Grounded
         | Expression(Pattern*)
```

**Key Property**: Patterns can contain variables (denoted by `$`), which act as wildcards that can match any atom.

### Pattern vs Atom

**Atom**: Concrete value with no variables
```metta
Socrates
42
(Human Plato)
```

**Pattern**: May contain variables
```metta
$x                      ; Pure variable
(Human $x)              ; Mixed
($pred $x $y)           ; Multiple variables
```

**Matching**: Determines if atom conforms to pattern and extracts variable bindings.

## Variable Patterns

### Variable Syntax

**Format**: `$<name>` where `<name>` is an identifier

**Valid Variables:**
```metta
$x
$person
$my_var
$var123
$CamelCase
```

**Special Variable:**
```metta
$_          ; Wildcard (conventionally ignored in results)
```

### Variable Semantics

**Specification:**

A variable in a pattern matches any atom and binds to that atom.

**Formal Rule:**
```
a is any atom
─────────────────
match($x, a) = {$x ← a}
```

**Examples:**
```metta
; Pattern: $x
; Matches: Socrates
; Bindings: {$x ← Socrates}

; Pattern: $x
; Matches: (foo bar)
; Bindings: {$x ← (foo bar)}

; Pattern: $x
; Matches: 42
; Bindings: {$x ← 42}
```

### Variable Scope

**Specification**: Variables are scoped to the expression in which they appear.

**Location**: `hyperon-experimental/docs/minimal-metta.md:174-201`

**Rules:**
1. Each expression creates a new variable scope
2. Variables with same name in different scopes are distinct
3. Variables are distinguished by unique IDs internally

**Example:**
```metta
; Two separate scopes
(match &self ($x) $x)     ; $x in scope 1
(match &self ($x) (* $x 2))  ; $x in scope 2 (different variable)
```

### Variable Identity

**Implementation**: `hyperon-atom/src/lib.rs:230-300`

**Structure:**
```rust
pub struct VariableAtom {
    name: String,    // User-visible name
    id: u64,         // Unique identifier
}
```

**Format**: `<name>[#<id>]`

**Examples:**
- `$x` displayed as `$x`
- Internally: `$x#0` vs `$x#1` (different variables)

**Equality**: Two variables are equal if names AND IDs match.

### Shared Variables

**Specification**: Same variable appearing multiple times in a pattern must match the same atom.

**Formal Rule:**
```
match(atom, $x) = {$x ← atom}
match(atom, $x) = {$x ← atom}  (same $x)
───────────────────────────────
Atoms must be equal
```

**Examples:**
```metta
; Pattern: (same $x $x)
; Matches: (same A A)     ✓ {$x ← A}
; Doesn't match: (same A B)  ✗ (A ≠ B)

; Pattern: ($x foo $x)
; Matches: (bar foo bar)  ✓ {$x ← bar}
; Doesn't match: (bar foo baz)  ✗
```

### Wildcard Variable

**Convention**: `$_` used as a wildcard to ignore values.

**Behavior:**
- Matches any atom (like any variable)
- Conventionally not used in template
- Multiple `$_` in same pattern are independent

**Examples:**
```metta
; Ignore second field
!(match &self
    (person $name $_ $age)
    ($name $age))

; Match but don't capture
!(match &self
    (edge $_ $_ $weight)
    $weight)
```

**Note**: `$_` is not special syntax, just a naming convention. Each `$_` is a distinct variable.

## Ground Patterns

### Symbol Patterns

**Specification**: Symbols must match exactly (case-sensitive).

**Examples:**
```metta
; Pattern: Human
; Matches: Human     ✓
; Doesn't match: human    ✗
; Doesn't match: HUMAN    ✗

; Pattern: (Human Socrates)
; Matches: (Human Socrates)  ✓
; Doesn't match: (Human Plato)  ✗
```

**Implementation**: `hyperon-atom/src/lib.rs:100-150`

**Matching Rule:**
```rust
match (Atom::Symbol(l), Atom::Symbol(r)) {
    if l.name() == r.name() => BindingsSet::single(bindings),
    _ => BindingsSet::empty()
}
```

### Number Patterns

**Specification**: Numbers must match exactly (value and type).

**Examples:**
```metta
; Pattern: 42
; Matches: 42    ✓
; Doesn't match: 43     ✗
; Doesn't match: 42.0   ✗ (different type)

; Pattern: 3.14
; Matches: 3.14  ✓
; Doesn't match: 3.140  ? (depends on representation)
```

**Implementation**: `hyperon-atom/src/gnd/number.rs`

**Types**: Integer, Float (implementation-dependent precision)

### String Patterns

**Specification**: Strings must match exactly (including case, whitespace).

**Examples:**
```metta
; Pattern: "hello"
; Matches: "hello"   ✓
; Doesn't match: "Hello"   ✗
; Doesn't match: "hello "  ✗ (extra space)

; Pattern: ""
; Matches: ""  ✓ (empty string)
```

**Implementation**: `hyperon-atom/src/gnd/str.rs`

### Boolean Patterns

**Specification**: Booleans must match exactly.

**Examples:**
```metta
; Pattern: True
; Matches: True   ✓
; Doesn't match: False  ✗

; Pattern: (and True $x)
; Matches: (and True False)  ✓ {$x ← False}
```

**Implementation**: `hyperon-atom/src/gnd/bool.rs`

**Values**: `True`, `False`

### Grounded Patterns

**Specification**: Grounded atoms (Rust objects) can define custom matching.

**Implementation**: Custom `match_()` method

**Example**: `lib/examples/custom_match.rs:1-100`

```rust
impl Grounded for CustomType {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Custom matching logic
        // Can return multiple bindings or none
    }
}
```

**Behavior**: Depends on implementation (can be non-standard).

## Expression Patterns

### Basic Expression Patterns

**Specification**: Expression patterns match expressions with same structure.

**Syntax**: `(element1 element2 ... elementN)`

**Matching Rule:**
```
len(e₁) = len(e₂)
∀i: match(e₁[i], e₂[i]) = σᵢ
σ = merge(σ₁, σ₂, ..., σₙ)
────────────────────────────
match(e₁, e₂) = σ
```

**Examples:**
```metta
; Pattern: (Human $x)
; Matches: (Human Socrates)  ✓ {$x ← Socrates}
; Doesn't match: (Human)           ✗ (wrong arity)
; Doesn't match: (Human A B)       ✗ (wrong arity)

; Pattern: (age $person $years)
; Matches: (age John 30)  ✓ {$person ← John, $years ← 30}
```

### Nested Expression Patterns

**Specification**: Expressions can be nested to arbitrary depth.

**Examples:**
```metta
; Pattern: (parent (person $name) $child)
; Matches: (parent (person Alice) Bob)
; Bindings: {$name ← Alice, $child ← Bob}

; Pattern: (implies (Frog $x) (Green $x))
; Matches: (implies (Frog Kermit) (Green Kermit))
; Bindings: {$x ← Kermit}  (note: same $x in both places)
```

### Mixed Patterns

**Specification**: Patterns can mix ground terms and variables.

**Examples:**
```metta
; Pattern: (Human $x)
; Ground: Human
; Variable: $x

; Pattern: (color $object red)
; Variables: $object
; Ground: color, red
```

### Empty Expression

**Specification**: Empty expression `()` matches exactly.

**Examples:**
```metta
; Pattern: ()
; Matches: ()  ✓
; Doesn't match: (a)   ✗

; Pattern: (foo ())
; Matches: (foo ())  ✓
```

## Pattern Matching Rules

### Exact Arity Matching

**Rule**: Expression arity (number of elements) must match exactly.

**Examples:**
```metta
; Pattern: (f $x)
; Matches: (f a)     ✓
; Doesn't match: (f a b)   ✗
; Doesn't match: (f)       ✗

; Pattern: (f $x $y $z)
; Matches: (f 1 2 3)  ✓
; Doesn't match: (f 1 2)    ✗
```

**No Varargs in Basic Patterns**: MeTTa doesn't support `...` in patterns (unlike some Lisps).

### Recursive Matching

**Specification**: Matching proceeds recursively through expression structure.

**Algorithm:**
```
match_expression(pattern, atom):
  if len(pattern) ≠ len(atom):
    return NO_MATCH

  bindings = {}
  for i in 0..len(pattern):
    result = match(pattern[i], atom[i])
    if result = NO_MATCH:
      return NO_MATCH
    bindings = merge(bindings, result)

  return bindings
```

**Example:**
```metta
; Pattern: ((a $x) (b $y))
; Atom:    ((a 1) (b 2))

; Step 1: Match (a $x) with (a 1)
;   → {$x ← 1}

; Step 2: Match (b $y) with (b 2)
;   → {$y ← 2}

; Final: {$x ← 1, $y ← 2}
```

### Binding Consistency

**Rule**: Bindings must be consistent across entire pattern.

**Examples:**
```metta
; Pattern: ($x foo $x)
; Atom: (A foo A)
; Step 1: $x ← A
; Step 2: foo matches foo
; Step 3: $x ← A (consistent with step 1) ✓

; Pattern: ($x foo $x)
; Atom: (A foo B)
; Step 1: $x ← A
; Step 2: foo matches foo
; Step 3: $x ← B (conflicts with step 1) ✗
```

## Pattern Examples

### Simple Patterns

```metta
; Match any atom
$x

; Match specific symbol
Socrates

; Match specific number
42

; Match any human
(Human $x)

; Match specific human
(Human Socrates)
```

### Relational Patterns

```metta
; Binary relation
(parent $p $c)

; Ternary relation
(edge $from $to $weight)

; Typed relation
(age $person $years)
```

### Structural Patterns

```metta
; Nested structure
(person (name $n) (age $a))

; Deep nesting
(company (address (city $c) (state $s)))

; Complex structure
(record (id $id) (data (field1 $f1) (field2 $f2)))
```

### Constraint Patterns

```metta
; Same variable multiple times
(same $x $x)

; Shared across sub-expressions
(implies (Frog $x) (Green $x))

; Multiple shared variables
(path $a $b $a)  ; Path from $a to $b back to $a
```

## Pattern Composition

### Building Complex Patterns

**Strategy**: Compose simple patterns into complex ones.

**Example:**
```metta
; Simple patterns
(Human $x)
(philosopher $x)

; Composed in conjunction
!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
; Finds all humans who are philosophers
```

### Pattern Decomposition

**Strategy**: Break complex patterns into manageable parts.

**Example:**
```metta
; Complex pattern
(person (name $first $last) (age $years) (city $city))

; Can be thought of as:
; - person record
; - name sub-pattern: (name $first $last)
; - age sub-pattern: (age $years)
; - city sub-pattern: (city $city)
```

## Pattern Matching Semantics

### Match Success

**Definition**: A match succeeds if pattern and atom unify.

**Result**: Variable bindings (substitution)

**Example:**
```metta
; Pattern: (Human $x)
; Atom: (Human Socrates)
; Result: SUCCESS with {$x ← Socrates}
```

### Match Failure

**Definition**: A match fails if pattern and atom cannot unify.

**Result**: No bindings (empty set)

**Examples:**
```metta
; Pattern: (Human $x)
; Atom: (Animal Dog)
; Result: FAIL (Human ≠ Animal)

; Pattern: (same $x $x)
; Atom: (same A B)
; Result: FAIL (A ≠ B for $x)

; Pattern: (f $x $y)
; Atom: (f A)
; Result: FAIL (arity mismatch)
```

### Partial Matches

**Note**: MeTTa does not support partial matches. Either entire pattern matches or it doesn't.

**Example:**
```metta
; Pattern: (a $x (b $y))
; Atom: (a 1 (c 2))
; Result: FAIL (entire match fails because b ≠ c)
; NOT: {$x ← 1} (partial binding not returned)
```

## Pattern Validation

### Syntactic Validity

**Valid Patterns:**
- Any atom is syntactically valid as a pattern
- Variables: `$<identifier>`
- Expressions: Properly nested with balanced parens

**Invalid Patterns:**
- Malformed expressions: `(a b`
- Invalid variable names: `$ x` (space), `$` (no name)

### Semantic Constraints

**No constraints enforced**:
- Pattern can be arbitrarily complex
- No limits on nesting depth
- No limits on number of variables

**Runtime Behavior:**
- Very deep patterns may hit stack limits
- Very complex patterns may be slow

## Best Practices

### 1. Use Descriptive Variable Names

```metta
; Good
!(match &self (age $person $years) ...)

; Avoid
!(match &self (age $x $y) ...)
```

### 2. Leverage Shared Variables

```metta
; Find self-loops
!(match &self (edge $node $node) $node)

; Find symmetric relations
!(match &self
    (, (friend $a $b)
       (friend $b $a))
    ($a $b))
```

### 3. Use Wildcards for Ignored Values

```metta
; Don't care about middle field
!(match &self (record $id $_ $data) ...)
```

### 4. Start with Ground Terms

```metta
; More specific (faster)
!(match &self (Human $x) $x)

; Less specific (slower)
!(match &self ($relation Socrates) $relation)
```

### 5. Minimize Pattern Complexity

```metta
; Simple and clear
!(match &self (parent $p $c) ...)

; Overly complex (hard to read)
!(match &self (((nested) (deeply) (pattern))) ...)
```

## Common Pitfalls

### 1. Variable Name Conflicts

**Problem:** Assuming variables with same name are same across scopes.

```metta
; These $x are DIFFERENT variables:
!(match &self (a $x) $x)
!(match &self (b $x) $x)
```

### 2. Arity Mismatches

**Problem:** Forgetting exact arity requirement.

```metta
; Pattern: (f $x)
; Won't match: (f 1 2) even though we "just want $x"
```

### 3. Ground Term Assumptions

**Problem:** Expecting partial matching.

```metta
; Pattern: (data $x)
; Won't match: (Data $x) (case-sensitive!)
```

### 4. Variable Scope

**Problem:** Variables not scoped as expected.

```metta
; $x in template is only bound if in pattern
!(match &self (a) $x)  ; Error: $x unbound
```

## Related Operations

### match

Uses patterns to query spaces:
```metta
!(match &self <pattern> <template>)
```

See: [03-match-operation.md](03-match-operation.md)

### unify

Tests if atom matches pattern:
```metta
!(unify <atom> <pattern> <then> <else>)
```

See: [05-pattern-contexts.md](05-pattern-contexts.md#unify)

### Rule Application

Patterns used in rule definitions:
```metta
(= <pattern> <result>)
```

See: `../atom-space/04-rules.md`

## Summary

**Pattern Fundamentals:**
- Variables (`$x`) match any atom
- Ground terms match exactly
- Expressions match recursively
- Shared variables must match same value

**Pattern Types:**
- Variable: `$x`, `$_`
- Symbol: `Socrates`, `Human`
- Number: `42`, `3.14`
- String: `"hello"`
- Expression: `(Human $x)`, `((nested) structure)`

**Key Rules:**
- Exact arity matching
- Recursive structure matching
- Binding consistency
- Case-sensitive symbols

**Best Practices:**
- Descriptive variable names
- Leverage shared variables
- Use wildcards for ignored values
- Start patterns with ground terms
- Keep patterns simple

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
