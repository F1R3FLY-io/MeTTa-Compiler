# Advanced Pattern Matching

## Overview

This document covers advanced pattern matching techniques in MeTTa, including nested patterns, custom matching, variable scope, variable equivalence, and sophisticated query strategies.

## Nested Patterns

### Deep Nesting

**Specification**: Patterns can be nested to arbitrary depth to match complex structures.

**Example:**
```metta
; Pattern with 3 levels
!(match &self
    (company
        (address
            (city $c)
            (state $s))
        (employees $count))
    ($c $s $count))
```

**Matching Process**:
1. Match outer `company` structure
2. Recursively match `address` sub-structure
3. Extract variables from all levels
4. Combine bindings

### Multi-Level Extraction

**Example:**
```metta
; Complex nested structure
(add-atom &self
    (person
        (id 1)
        (name (first "Alice") (last "Smith"))
        (contact
            (email "alice@example.com")
            (phone "555-1234"))))

; Extract email
!(match &self
    (person $id $name (contact (email $email) $phone))
    $email)
; → ["alice@example.com"]
```

### Nested with Shared Variables

**Pattern**: Same variable at different nesting levels.

**Example:**
```metta
; Find self-referential structures
!(match &self
    (references $x (contains $x))
    $x)
```

**Constraint**: `$x` must match at both levels.

## Custom Matching

### CustomMatch Trait

**Purpose**: Define custom matching logic for grounded atoms.

**Location**: `hyperon-atom/src/lib.rs`

**Trait Definition:**
```rust
pub trait Grounded {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Default: equality check
        if self == other {
            BindingsSet::single(Bindings::new())
        } else {
            BindingsSet::empty()
        }
    }
}
```

**Override**: Implement custom matching behavior.

### Custom Match Example

**Location**: `lib/examples/custom_match.rs:1-100`

**Implementation:**
```rust
#[derive(Clone, PartialEq)]
struct TestDict {
    map: HashMap<String, Atom>,
}

impl Grounded for TestDict {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        match other {
            // Match against dict pattern: (dict-get <key>)
            Atom::Expression(expr) if expr.children().len() == 2 => {
                let op = &expr.children()[0];
                let key = &expr.children()[1];

                if let (Atom::Symbol(s), Atom::Symbol(k)) = (op, key) {
                    if s.name() == "dict-get" {
                        if let Some(value) = self.map.get(k.name()) {
                            // Return binding: pattern → value
                            return Box::new(std::iter::once(
                                Bindings::new().add_var_binding(
                                    VariableAtom::new("result"),
                                    value.clone()
                                )
                            ));
                        }
                    }
                }
                BindingsSet::empty().into_iter()
            }
            _ => BindingsSet::empty().into_iter()
        }
    }
}
```

**Usage:**
```metta
; Create dict
!(bind! &dict (make-dict))

; Match with custom pattern
!(unify &dict (dict-get "key") $result "not found")
; Custom match_ method handles (dict-get "key") pattern
```

### Multiple Binding Results

**Feature**: Custom matchers can return multiple binding sets.

**Example:**
```rust
impl Grounded for MultiMatcher {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Return multiple solutions
        let b1 = Bindings::new().add_var_binding(var_x, atom_a);
        let b2 = Bindings::new().add_var_binding(var_x, atom_b);

        Box::new(vec![b1, b2].into_iter())
    }
}
```

**Effect**: One match produces multiple results (non-determinism).

## Variable Scope

### Scope Rules

**Specification**: Each expression creates a new variable scope.

**Location**: `hyperon-experimental/docs/minimal-metta.md:174-201`

**Rules:**
1. Variables scoped to containing expression
2. Same name in different scopes = different variables
3. Variables identified by unique IDs internally

**Example:**
```metta
; Two separate scopes
(match &self (Human $x) $x)       ; $x#0
(match &self (philosopher $x) $x) ; $x#1 (different variable!)
```

### Variable Identity

**Internal Representation:**
```rust
struct VariableAtom {
    name: String,    // e.g., "x"
    id: u64,         // e.g., 42
}
```

**Display Format**: `$x#42` (name + ID)

**Equality**: Variables equal only if name AND id match.

### Cross-Scope Variable Sharing

**Via Parameters:**
```metta
(= (process $x)
    (match &self
        (related $x $y)  ; $x from outer scope
        $y))
```

**Process:**
1. `$x` bound in outer scope (function parameter)
2. Same `$x` used in inner match pattern
3. Variable ID shared across scopes

### Scope Example

**Nested Expressions:**
```metta
; Outer expression
(match &self
    (parent $x $child)
    ; Inner expression - new scope
    (match &self
        (age $child $years)  ; $child from outer
        ($x has child aged $years)))
```

**Analysis:**
- `$x` and `$child` from outer scope
- `$years` from inner scope
- All have unique IDs

## Variable Equivalence

### Equivalence vs Equality

**Equality**: Exact match (same name and ID)
```rust
$x#42 == $x#42  // true
$x#42 == $x#43  // false
$x#42 == $y#42  // false
```

**Equivalence**: Same structure up to variable renaming
```rust
($x $x) ≡ ($y $y)    // true (same pattern)
($x $y) ≡ ($a $b)    // true (both have 2 distinct vars)
($x $y) ≢ ($x $x)    // false (different pattern)
```

### Equivalence Checking

**Function**: `atoms_are_equivalent()` - `matcher.rs:1196-1229`

**Purpose**: Check if two atoms are the same up to variable renaming.

**Algorithm:**
```rust
pub fn atoms_are_equivalent(left: &Atom, right: &Atom) -> bool {
    let mut var_map = HashMap::new();

    fn check(l: &Atom, r: &Atom, map: &mut HashMap<VariableAtom, VariableAtom>) -> bool {
        match (l, r) {
            (Atom::Variable(lv), Atom::Variable(rv)) => {
                match map.entry(lv.clone()) {
                    Entry::Occupied(e) => e.get() == rv,  // Must map consistently
                    Entry::Vacant(e) => {
                        e.insert(rv.clone());
                        true
                    }
                }
            }
            (Atom::Symbol(ls), Atom::Symbol(rs)) => ls == rs,
            (Atom::Expression(le), Atom::Expression(re)) => {
                le.children().len() == re.children().len() &&
                le.children().iter().zip(re.children())
                    .all(|(lc, rc)| check(lc, rc, map))
            }
            _ => false
        }
    }

    check(left, right, &mut var_map)
}
```

**Examples:**
```metta
; Equivalent patterns
(a $x "b") ≡ (a $y "b")           ; true
(f $x $x) ≡ (f $y $y)             ; true
($a $b $c) ≡ ($x $y $z)           ; true

; Not equivalent
(f $x $y) ≢ (f $x $x)             ; false (different structure)
(a $x "b") ≢ (a $x "c")           ; false (different constant)
(f $x) ≢ (g $x)                   ; false (different symbol)
```

### Use Cases

**Duplicate Detection:**
```metta
; Check if two rules are equivalent
(= (double $x) (* $x 2))
(= (double $y) (* $y 2))  ; Equivalent to first!
```

**Rule Subsumption:**
```metta
; More general rule
(= (process $x) (generic $x))

; More specific rule
(= (process (special $y)) (specific $y))
```

## Pattern Guards (Constraints)

### Implicit Guards via Shared Variables

**Pattern**: Use same variable multiple times.

**Example:**
```metta
; Match only when both fields equal
!(match &self
    (same $x $x)
    $x)
```

**Constraint**: `$x` must unify with both positions.

### Explicit Guards with If

**Pattern**: Add condition in template.

**Example:**
```metta
!(match &self
    (age $person $years)
    (if (> $years 18)
        ($person is adult)
        ()))
```

**Process:**
1. Match pattern, bind variables
2. Evaluate condition
3. Return result only if condition true

### Type-Based Guards

**Pattern**: Use type checking as guard.

**Example:**
```metta
(: process-number (-> Number %Undefined%))
(= (process-number $x)
    (if (number? $x)
        (* $x 2)
        (error "not a number")))
```

## Recursive Patterns

### Self-Referential Patterns

**Example:**
```metta
; Match nested lists
(= (flatten ()) ())
(= (flatten ($h $t...))
    (if (list? $h)
        (append (flatten $h) (flatten $t))
        (cons $h (flatten $t))))
```

**Usage:**
```metta
!(flatten ((1 2) (3 (4 5))))
; Recursively processes nested structure
```

### Mutually Recursive Patterns

**Example:**
```metta
(= (even? 0) True)
(= (even? $n) (odd? (- $n 1)))

(= (odd? 0) False)
(= (odd? $n) (even? (- $n 1)))
```

## Pattern Optimization Strategies

### Ground Prefix Optimization

**Strategy**: Start patterns with ground terms.

**Good:**
```metta
!(match &self (Human $x) $x)
; Trie prunes non-Human branches immediately
```

**Bad:**
```metta
!(match &self ($relation Socrates) $relation)
; Must check all relations
```

**Reason**: Trie index can skip irrelevant branches with ground prefixes.

### Specificity Ordering

**Strategy**: Order patterns from specific to general.

**Example:**
```metta
; Specific first
(= (process special-case) special-result)
(= (process $x) (generic-handler $x))
```

**Note**: MeTTa doesn't enforce order, but specificity helps reasoning.

### Early Failure with Conjunction

**Strategy**: Put most restrictive pattern first in conjunction.

**Good:**
```metta
!(match &self
    (, (rare-property $x)
       (common-property $x))
    $x)
```

**Bad:**
```metta
!(match &self
    (, (common-property $x)  ; Many matches
       (rare-property $x))    ; Then filtered
    $x)
```

**Benefit**: Fewer intermediate bindings to process.

## Complex Query Patterns

### Transitive Closure

**Pattern**: Find all reachable nodes.

**Example:**
```metta
; Direct connections
(= (connected $a $b)
    (match &self (edge $a $b) True))

; Transitive connections
(= (connected $a $c)
    (match &self
        (, (edge $a $b)
           (connected $b $c))
        True))
```

### Path Finding

**Pattern**: Find paths between nodes.

**Example:**
```metta
; Find path
(= (path $start $end)
    (unify $start $end
        (list $start)  ; Base case: start = end
        (match &self
            (edge $start $next)
            (cons $start (path $next $end)))))
```

### Aggregation Patterns

**Pattern**: Collect and process all matches.

**Example:**
```metta
; Count matches
(= (count-humans)
    (length (match &self (Human $x) $x)))

; Sum values
(= (total-age)
    (sum (match &self (age $ $years) $years)))
```

### Negation Patterns

**Pattern**: Find atoms NOT matching pattern.

**Example:**
```metta
; Find non-humans
!(match &self
    (entity $x)
    (if (not (match &self (Human $x) True))
        $x
        ()))
```

**Note**: Negation requires explicit check.

## Pattern Metaprogramming

### Pattern as Data

**Concept**: Patterns are atoms, can be manipulated.

**Example:**
```metta
; Store pattern
!(bind! &pattern (Human $x))

; Use pattern in match
!(match &self &pattern $x)
```

### Dynamic Pattern Construction

**Example:**
```metta
(= (match-by-type $type)
    (let $pattern (cons $type (list (gensym)))
        (match &self $pattern $pattern)))
```

### Higher-Order Pattern Functions

**Example:**
```metta
; Filter by arbitrary pattern
(= (filter-atoms $pattern)
    (match &self $pattern $pattern))

; Usage
!(filter-atoms (Human $x))
!(filter-atoms (age $p $y))
```

## Best Practices

### 1. Use Descriptive Variable Names

**Good:**
```metta
!(match &self
    (parent $parent $child)
    ...)
```

**Avoid:**
```metta
!(match &self
    (parent $x $y)
    ...)
```

### 2. Leverage Structural Constraints

**Good:**
```metta
; Shared variable enforces constraint
!(match &self
    (edge $node $node)  ; Self-loops
    $node)
```

### 3. Document Complex Patterns

**Good:**
```metta
; Find grandparent relationships:
; Match parent($gp, $p) AND parent($p, $gc)
!(match &self
    (, (parent $gp $p)
       (parent $p $gc))
    ($gp grandparent $gc))
```

### 4. Test Patterns Incrementally

**Strategy:**
```metta
; Test simple first
!(match &self (Human $x) $x)

; Add complexity
!(match &self
    (person (name $x) (age $a))
    ($x $a))

; Add constraints
!(match &self
    (, (person (name $x) (age $a))
       (> $a 18))
    $x)
```

### 5. Use Custom Matching Judiciously

**Good Use:**
- Domain-specific patterns
- Performance optimization
- Special semantics

**Avoid:**
- Over-complicating simple cases
- Breaking unification symmetry
- Unclear matching behavior

## Common Pitfalls

### 1. Variable Scope Confusion

**Problem:**
```metta
!(match &self (Human $x) $x)
!(match &self (age $x $y) $y)  ; Different $x!
```

### 2. Pattern Too General

**Problem:**
```metta
!(match &self $anything $anything)
; Returns ALL atoms (expensive!)
```

### 3. Missing Constraints

**Problem:**
```metta
!(match &self
    (, (parent $p $c1)
       (parent $p $c2))
    ($c1 $c2))
; Returns ALL pairs, including ($c $c)
```

**Solution:**
```metta
!(match &self
    (, (parent $p $c1)
       (parent $p $c2)
       (!= $c1 $c2))  ; Add constraint
    ($c1 $c2))
```

### 4. Infinite Recursion

**Problem:**
```metta
(= (loop $x) (loop $x))
!(loop 1)  ; Infinite!
```

**Solution**: Always have base case.

### 5. Custom Match Breaking Symmetry

**Problem:**
```rust
// Bad: non-symmetric matching
impl Grounded for Bad {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Only matches in one direction!
    }
}
```

## Related Documentation

**Fundamentals**: [01-fundamentals.md](01-fundamentals.md)
**Unification**: [02-unification.md](02-unification.md)
**Implementation**: [07-implementation.md](07-implementation.md)
**Non-Determinism**: [08-non-determinism.md](08-non-determinism.md)

## Summary

**Advanced Techniques:**
- **Nested Patterns**: Multi-level matching
- **Custom Matching**: Domain-specific logic
- **Variable Scope**: Understanding variable identity
- **Variable Equivalence**: Structural comparison
- **Pattern Guards**: Constraining matches

**Optimization:**
- Ground prefix strategy
- Specificity ordering
- Early failure in conjunctions
- Efficient query design

**Complex Patterns:**
- Transitive closure
- Path finding
- Aggregation
- Negation
- Metaprogramming

**Best Practices:**
✅ Descriptive names
✅ Leverage constraints
✅ Document complex patterns
✅ Test incrementally
✅ Use custom matching judiciously

**Avoid:**
❌ Variable scope confusion
❌ Overly general patterns
❌ Missing constraints
❌ Infinite recursion
❌ Non-symmetric custom matching

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
