# Pattern Matching Edge Cases

## Overview

This document covers edge cases, corner cases, and gotchas in MeTTa's pattern matching system. Understanding these cases helps write robust code and debug unexpected behavior.

## Empty Patterns and Atoms

### Empty Expression

**Pattern:** `()`

**Matching:**
```metta
!(unify () () "match" "no match")
; → "match" (empty expressions match)

!(unify (f) () "match" "no match")
; → "no match" (different lengths)
```

**In Space:**
```metta
(add-atom &self ())

!(match &self () found)
; → [found]

!(match &self () ())
; → [()] (returns empty expression)
```

**Edge Case**: Empty expressions are valid atoms.

### Empty Variable List

**Pattern with no variables:**
```metta
!(match &self (Human Socrates) True)
; Pattern is fully ground
; Returns: [True] if atom exists, [] otherwise
```

**Template with no variables:**
```metta
!(match &self (Human $x) constant)
; → [constant, constant, constant] (one per match)
```

### Empty Space

**Query on empty space:**
```metta
!(match &empty-space (anything $x) $x)
; → [] (no atoms to match)
```

**No Error**: Empty result is valid.

### Empty Result Set

**No matches:**
```metta
!(match &self (nonexistent $x) $x)
; → [] (empty list, not error)
```

**Handling:**
```metta
(= (safe-query)
    (let $results (match &self (pattern $x) $x)
        (if (empty? $results)
            default-value
            (process $results))))
```

## Variable Patterns

### All Variables

**Pattern**: `$x` (pure variable)

**Behavior**: Matches every atom in space.

```metta
!(match &self $x $x)
; Returns ALL atoms in space
; Potentially very expensive!
```

**Performance**: O(n) where n = space size

**Best Practice**: Avoid pure variable patterns unless space is small.

### Repeated Variables

**Same variable multiple times:**
```metta
; Match only when both positions equal
!(match &self (same $x $x) $x)

; Space: (same A A), (same B B), (same A B)
; Returns: [A, B] (not A B from (same A B))
```

**Constraint**: Variable must unify with all positions.

### Variable Shadowing

**Nested scopes:**
```metta
; Outer scope
(= (outer $x)
    ; Inner scope - different $x
    (match &self (inner $x) $x))

!(outer 5)
; Outer $x (= 5) is different from inner $x
```

**Resolution**: Variables identified by unique IDs, not just names.

**Display**: `$x#42` (name + ID) for disambiguation.

## Cyclic Structures

### Direct Cycles

**Attempt:**
```metta
; Try to create $x ← (f $x)
!(unify $x (f $x) "matched" "failed")
; → "failed" (occurs check prevents cycle)
```

**Occurs Check**: Prevents binding `$x` to term containing `$x`.

**Reason**: Would create infinite structure.

### Indirect Cycles

**Attempt:**
```metta
; Try $x ← (f $y), $y ← (g $x)
; Not directly prevented, but evaluation may loop
```

**Problem**: Can cause infinite loops during evaluation.

**Detection**: Not always caught statically.

**Mitigation**: Use depth limits or cycle detection in evaluator.

### Cyclic References in Space

**Space with circular references:**
```metta
(add-atom &self (parent A B))
(add-atom &self (parent B A))  ; Circular!

; Query transitive closure
(= (ancestor $x $y) (parent $x $y))
(= (ancestor $x $z)
    (match &self
        (, (parent $x $y)
           (ancestor $y $z))
        True))

!(ancestor A A)
; May loop infinitely without cycle detection!
```

**Solution**: Implement cycle detection:
```metta
(= (ancestor-safe $x $y $visited)
    (if (member? $x $visited)
        False  ; Cycle detected
        (or (parent $x $y)
            (match &self
                (parent $x $z)
                (ancestor-safe $z $y (cons $x $visited))))))
```

## Type Mismatches

### Pattern vs Atom Type Mismatch

**Different atom types:**
```metta
!(unify Symbol (Expression a b) "match" "no match")
; → "no match" (different types)

!(unify (f $x) GroundedAtom "match" "no match")
; → "no match" (unless GroundedAtom has custom match)
```

**Rule**: Symbols don't match Expressions, etc.

### Expression Length Mismatch

**Different arity:**
```metta
!(unify (f $x) (f $y $z) "match" "no match")
; → "no match" (length 2 vs 3)
```

**Pattern must have same length** as atom (unless using variadic).

### Unexpected Grounded Atoms

**Pattern expects expression:**
```metta
!(match &self (compute $x $y) ($x + $y))
; If (compute 1 2) is grounded (e.g., native function)
; May not match as expected
```

**Solution**: Check atom type or use custom matching.

## Unbound Variables

### Variable in Template Not in Pattern

**Error:**
```metta
!(match &self (Human $x) $y)
; $y not bound by pattern
; Result: Undefined behavior or error
```

**Best Practice**: Only use variables that appear in pattern.

### Partially Bound Variables

**Multiple patterns with different variables:**
```metta
!(match &self
    (, (a $x)
       (b $y))
    ($x $y $z))  ; $z is unbound!
; Error or returns $z as-is
```

### Free Variables in Rules

**Rule with unbound variable:**
```metta
(= (broken $x) $y)  ; $y not bound!
; Behavior undefined
```

**Fix**: Bind all variables or use constants.

## Infinite Patterns

### Infinite Recursion

**Unbounded recursion:**
```metta
(= (loop $x) (loop $x))

!(loop 1)
; Never terminates (no base case)
```

**Solution**: Always provide base case.
```metta
(= (safe-loop 0) done)
(= (safe-loop $n) (safe-loop (- $n 1)))
```

### Infinite Result Generation

**Unbounded matches:**
```metta
(= (naturals $n) $n)
(= (naturals $n) (naturals (+ $n 1)))

!(naturals 0)
; Generates infinite results: [0, 1, 2, 3, ...]
; May exhaust memory or time out
```

**Mitigation**: Use limits or lazy evaluation.

## Large Expressions

### Deep Nesting

**Very deep expressions:**
```metta
; Pattern 100 levels deep
(a (a (a ... (a $x) ... )))
```

**Issue**: May cause stack overflow in recursive unification.

**Complexity**: O(depth) recursion

**Mitigation**: Iterative implementation or stack limits.

### Wide Expressions

**Many children:**
```metta
; Expression with 10,000 children
(f $x1 $x2 $x3 ... $x10000)
```

**Issue**: Slow unification, large memory usage.

**Complexity**: O(n) where n = number of children

**Performance**: May be slow for very large n.

### Exponential Blowup

**Cartesian product explosion:**
```metta
; Each pattern matches 100 atoms
!(match &self
    (, (a $v1) (b $v2) (c $v3) (d $v4) (e $v5))
    ($v1 $v2 $v3 $v4 $v5))
; Result count: 100^5 = 10 billion!
; May exhaust memory
```

**Mitigation**: Add constraints to reduce matches.

## Grounded Atom Edge Cases

### Custom Match Asymmetry

**Problem**: Custom match not symmetric.

```rust
impl Grounded for BadMatcher {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Only matches in one direction!
        match other {
            Atom::Symbol(s) if s.name() == "special" => {
                // Return binding
            }
            _ => BindingsSet::empty().into_iter()
        }
    }
}
```

**Issue:**
```metta
!(unify BadGrounded special "match" "no")  ; Matches
!(unify special BadGrounded "match" "no")  ; Doesn't match!
```

**Solution**: Ensure symmetry or document asymmetry.

### Custom Match Side Effects

**Problem**: Match has side effects.

```rust
impl Grounded for Logger {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        println!("Matching: {:?}", other);  // Side effect!
        // ...
    }
}
```

**Issue**: Evaluation order affects output.

**Best Practice**: Keep matching pure (no side effects).

### Custom Match Returning Invalid Bindings

**Problem**: Custom matcher returns inconsistent bindings.

```rust
impl Grounded for Broken {
    fn match_(&self, _other: &Atom) -> MatchResultIter {
        // Bad: binding $x to two different values
        let b1 = Bindings::new()
            .add_var_binding(var_x, atom_a);
        let b2 = b1.add_var_binding(var_x, atom_b);  // Conflict!
        // b2 is Empty, but we return it anyway
        Box::new(vec![b2].into_iter())
    }
}
```

**Solution**: Validate bindings before returning.

## Performance Edge Cases

### Pathological Patterns

**Worst-case unification:**
```metta
; Pattern and atom both deeply nested with all variables
!(unify
    ($v1 ($v2 ($v3 ($v4 $v5))))
    ($x1 ($x2 ($x3 ($x4 $x5))))
    ...)
; Complexity: O(size × size) in worst case
```

### Trie Degeneration

**Poor trie structure:**
```metta
; All atoms start with variable
(add-atom &self ($x 1))
(add-atom &self ($y 2))
; Trie can't optimize on variable prefix
```

**Impact**: Queries become O(n) instead of O(log n).

**Mitigation**: Use ground prefixes when possible.

### Memory Exhaustion

**Large result sets:**
```metta
; Query returns 1 million results
!(match &large-space (common-pattern $x) $x)
; May exceed memory limits
```

**Solution**: Use pagination or streaming.

## Debugging Edge Cases

### Unexpected Match Failure

**Problem:**
```metta
!(match &self (Human Socrates) found)
; → [] (expected [found])
```

**Debugging Steps:**
1. Check space contents: `!(get-atoms &self)`
2. Verify atom structure: Symbols vs Strings, etc.
3. Check for typos in atom names
4. Ensure atoms were actually added

### Unexpected Multiple Matches

**Problem:**
```metta
!(match &self (unique $x) $x)
; → [A, B, C] (expected single result)
```

**Cause**: Multiple atoms match pattern.

**Fix**: Make pattern more specific.

### Variable Not Substituted

**Problem:**
```metta
!(match &self (data $x) $y)
; → [$y, $y, $y] (variables not substituted)
```

**Cause**: `$y` not in pattern, remains unbound.

**Fix**: Use `$x` in template.

### Infinite Loop Detection

**Problem**: Query never returns.

**Debugging:**
1. Check for unbounded recursion
2. Verify base cases
3. Add depth limits
4. Use cycle detection
5. Profile with timeouts

**Example fix:**
```metta
; Add depth limit
(= (safe-recursive $x 0) base-case)
(= (safe-recursive $x $depth)
    (if (> $depth 0)
        (safe-recursive (next $x) (- $depth 1))
        (error "depth exceeded")))
```

## Error Handling Best Practices

### 1. Validate Inputs

```metta
(= (safe-query $pattern)
    (if (valid-pattern? $pattern)
        (match &self $pattern $pattern)
        (error "invalid pattern")))
```

### 2. Handle Empty Results

```metta
(= (robust-get $key)
    (let $results (match &self (data $key $value) $value)
        (if (empty? $results)
            default-value
            (car $results))))
```

### 3. Limit Recursion Depth

```metta
(= (limited-recursive $x $max-depth)
    (limited-helper $x 0 $max-depth))

(= (limited-helper $x $depth $max)
    (if (>= $depth $max)
        (error "depth limit exceeded")
        (recursive-step $x (+ $depth 1) $max)))
```

### 4. Catch Type Errors

```metta
(= (type-safe-process $atom)
    (unify $atom (expected-structure $x $y)
        (process $x $y)                ; Match
        (error "unexpected structure")))  ; No match
```

### 5. Log Unexpected Cases

```metta
(= (logged-query $pattern)
    (let $results (match &self $pattern $pattern)
        (if (empty? $results)
            (do
                (println "Warning: no matches for " $pattern)
                $results)
            $results)))
```

## Testing Edge Cases

### Test Empty Cases

```metta
; Test empty space
!(assertEqual
    (match &empty-space (any $x) $x)
    ())

; Test empty pattern
!(assertEqual
    (match &self () ())
    (if (member? &self ()) (()) ()))
```

### Test Boundary Conditions

```metta
; Test single element
!(match &self (singleton) found)

; Test large expressions
!(match &self (deep $a ($b ($c ($d $e)))) ...)
```

### Test Conflict Cases

```metta
; Test variable conflict
!(assertEqual
    (unify (same $x $x) (same A B) "match" "no")
    "no")

; Test type mismatch
!(assertEqual
    (unify Symbol (f x) "match" "no")
    "no")
```

### Test Non-Termination

```metta
; Use timeout
!(with-timeout 1000  ; 1 second
    (infinite-recursion 0))
; Should timeout or return error
```

## Common Gotchas

### 1. Variable Scope Confusion

**Problem:**
```metta
!(match &self (a $x) $x)
!(match &self (b $x) $x)  ; Different $x!
```

**Each expression has separate scope.**

### 2. Order Dependence

**Problem:**
```metta
!(match &self
    (, (rare $x)
       (common $x))
    $x)
; Much faster than:
!(match &self
    (, (common $x)
       (rare $x))
    $x)
```

**Order affects performance significantly.**

### 3. Assuming Determinism

**Problem:**
```metta
(= (get-human) (match &self (Human $x) $x))
!(car (get-human))  ; Assumes list, but order undefined
```

**Multiple results have undefined order.**

### 4. Forgetting Occurs Check

**Attempted:**
```metta
; Won't work: occurs check prevents
!(unify $x (f $x) "matched" "failed")
; → "failed"
```

### 5. Grounded Atom Matching

**Problem:**
```metta
; Grounded atoms may not match as expected
!(match &self (NativeFunc add) found)
; May fail if 'add' is grounded
```

**Solution**: Use custom matching or type checks.

## Related Documentation

**Non-Determinism**: [08-non-determinism.md](08-non-determinism.md)
**Implementation**: [07-implementation.md](07-implementation.md)
**Fundamentals**: [01-fundamentals.md](01-fundamentals.md)
**Unification**: [02-unification.md](02-unification.md)

## Summary

**Empty Cases:**
- Empty expressions, spaces, and results are valid
- No implicit errors, handle explicitly

**Variable Issues:**
- Unbound variables in template
- Variable shadowing across scopes
- Pure variable patterns (expensive)

**Cyclic Structures:**
- Occurs check prevents direct cycles
- Indirect cycles may cause infinite loops
- Use cycle detection for graph queries

**Type Mismatches:**
- Different atom types don't unify
- Expression length must match
- Grounded atoms need custom handling

**Infinite Patterns:**
- Unbounded recursion
- Missing base cases
- Infinite result generation

**Large Expressions:**
- Deep nesting (stack overflow risk)
- Wide expressions (slow performance)
- Exponential blowup (Cartesian product)

**Performance:**
- Pathological patterns
- Trie degeneration
- Memory exhaustion

**Debugging:**
- Check space contents
- Verify atom structure
- Add logging and limits
- Use timeouts

**Best Practices:**
✅ Validate inputs
✅ Handle empty results
✅ Limit recursion depth
✅ Catch type errors
✅ Test edge cases
✅ Document assumptions

**Common Gotchas:**
❌ Variable scope confusion
❌ Order dependence
❌ Assuming determinism
❌ Forgetting occurs check
❌ Grounded atom matching

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-17
