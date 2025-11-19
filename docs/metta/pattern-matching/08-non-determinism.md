# Non-Determinism in Pattern Matching

## Overview

Non-determinism is a fundamental feature of MeTTa's pattern matching system. Multiple patterns can match the same atom, and a single pattern can match multiple atoms, producing multiple possible results.

## What is Non-Determinism?

### Definition

**Non-Deterministic Evaluation**: When evaluating an expression can produce multiple possible results.

**Example:**
```metta
; Multiple rules for same pattern
(= (color) red)
(= (color) green)
(= (color) blue)

!(color)
; → May return: red, green, or blue (all are valid)
```

### Contrast with Determinism

**Deterministic:**
```metta
(= (double $x) (* $x 2))

!(double 5)
; → Always returns: 10 (single result)
```

**Non-Deterministic:**
```metta
(= (choose) 1)
(= (choose) 2)

!(choose)
; → May return: 1 or 2 (multiple results)
```

## Sources of Non-Determinism

### 1. Multiple Matching Rules

**Scenario**: Multiple rules match the same expression.

**Example:**
```metta
(= (ancestor $x $y) (parent $x $y))
(= (ancestor $x $z)
    (match &self
        (, (parent $x $y)
           (ancestor $y $z))
        True))

; Query
!(ancestor Alice Charlie)
; May match first rule (if direct parent)
; Or second rule (if indirect ancestor)
```

**Mechanism**: Space query returns all matching rules.

### 2. Multiple Space Matches

**Scenario**: Pattern matches multiple atoms in space.

**Example:**
```metta
; Space contents
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))

; Query
!(match &self (Human $x) $x)
; → [Socrates, Plato, Aristotle]
```

**Mechanism**: `match` operation returns all matches.

### 3. Variable Bindings Producing Multiple Results

**Scenario**: Custom matching or complex unification produces multiple binding sets.

**Example:**
```rust
// Custom matcher returning multiple results
impl Grounded for RangeMatcher {
    fn match_(&self, pattern: &Atom) -> MatchResultIter {
        // Match range pattern (range $x)
        // Return bindings for all values in range
        let results = (self.start..self.end)
            .map(|i| {
                Bindings::new().add_var_binding(
                    var_x,
                    Atom::value(i)
                )
            })
            .collect();
        Box::new(results.into_iter())
    }
}
```

**Usage:**
```metta
!(match &self (range $x) $x)
; → [1, 2, 3, 4, 5] (if range is 1..6)
```

### 4. Conjunction with Multiple Solutions

**Scenario**: Each pattern in conjunction has multiple matches.

**Example:**
```metta
; Space
(add-atom &self (color red))
(add-atom &self (color blue))
(add-atom &self (size small))
(add-atom &self (size large))

; Query: all color-size combinations
!(match &self
    (, (color $c)
       (size $s))
    (item $c $s))
; → [(item red small), (item red large),
;    (item blue small), (item blue large)]
```

**Mechanism**: Cartesian product of individual matches.

### 5. Recursive Rules

**Scenario**: Recursive patterns generate multiple results.

**Example:**
```metta
(= (path $start $end)
    (unify $start $end
        (list $start)  ; Base case
        ; Recursive case
        (match &self
            (edge $start $next)
            (cons $start (path $next $end)))))

; Multiple paths possible
!(path A D)
; → [[A, B, D], [A, C, D], [A, B, C, D]]
```

## BindingsSet: Representing Multiple Solutions

### Structure

**Location**: `hyperon-atom/src/matcher.rs:886-1044`

**Definition:**
```rust
pub enum BindingsSet {
    Empty,                   // No solutions
    Single(Bindings),        // One solution
    Multi(Vec<Bindings>),    // Multiple solutions
}
```

### Semantics

**Empty**: Match failed, no valid bindings.
```metta
!(match &self (nonexistent $x) $x)
; → [] (empty list)
```

**Single**: Exactly one solution.
```metta
!(match &self (Human Socrates) True)
; → [True] (single result)
```

**Multi**: Multiple valid solutions.
```metta
!(match &self (Human $x) $x)
; → [Socrates, Plato, Aristotle] (multiple results)
```

### Operations on BindingsSet

**union** - Combine multiple binding sets:
```rust
pub fn union(self, other: BindingsSet) -> BindingsSet
```

**Example:**
```rust
// Query1 results: {$x ← A}
// Query2 results: {$x ← B}
// Union: [{$x ← A}, {$x ← B}]
```

**merge_v2** - Apply function to each binding:
```rust
pub fn merge_v2<F>(self, left: &Atom, right: &Atom, f: F) -> Self
where F: Fn(&Atom, &Atom, Bindings) -> BindingsSet
```

**Example (in conjunction):**
```rust
// Start: [{$c ← red}, {$c ← blue}]
// Match (size $s) for each:
//   {$c ← red} → [{$c ← red, $s ← small}, {$c ← red, $s ← large}]
//   {$c ← blue} → [{$c ← blue, $s ← small}, {$c ← blue, $s ← large}]
// Result: 4 binding sets (Cartesian product)
```

## Non-Deterministic Evaluation

### Evaluation Strategy

**MeTTa Evaluation**: Explore all possible reductions.

**Process:**
1. Find all matching rules/atoms
2. Generate BindingsSet with all solutions
3. Apply each binding to template
4. Evaluate each result
5. Return all results

**Example:**
```metta
(= (f 1) a)
(= (f 1) b)

!(f 1)
; Evaluation:
; 1. Find rules: both (= (f 1) a) and (= (f 1) b) match
; 2. BindingsSet: [{} (rule 1), {} (rule 2)]
; 3. Apply: [a, b]
; 4. Return: [a, b]
```

### Result Collection

**Match Operation** returns list:
```metta
!(match &self (Human $x) $x)
; → [Socrates, Plato, Aristotle]
```

**Rule Evaluation** may return multiple:
```metta
!(color)
; → [red, green, blue] (or subset, non-deterministic order)
```

### Ordering

**Not Guaranteed**: Result order is implementation-dependent.

**Example:**
```metta
!(match &self (Human $x) $x)
; May return: [Socrates, Plato, Aristotle]
; Or: [Plato, Socrates, Aristotle]
; Or any other permutation
```

**Best Practice**: Don't rely on order unless explicitly sorted.

## Conjunction and Non-Determinism

### Cartesian Product

**Semantics**: Conjunction produces all valid combinations.

**Example:**
```metta
; Space
(add-atom &self (a 1))
(add-atom &self (a 2))
(add-atom &self (b x))
(add-atom &self (b y))

; Query
!(match &self
    (, (a $n)
       (b $l))
    ($n $l))
; → [(1 x), (1 y), (2 x), (2 y)]
```

**Process:**
1. Match `(a $n)` → `{$n ← 1}`, `{$n ← 2}`
2. For each, match `(b $l)`:
   - `{$n ← 1}` → `{$n ← 1, $l ← x}`, `{$n ← 1, $l ← y}`
   - `{$n ← 2}` → `{$n ← 2, $l ← x}`, `{$n ← 2, $l ← y}`
3. Result: 4 binding sets

### Early Termination

**Optimization**: If one pattern fails, entire conjunction fails.

**Example:**
```metta
!(match &self
    (, (Human $x)
       (Dog $x))  ; Impossible constraint
    $x)
; → [] (empty, no atom is both Human and Dog)
```

**Process:**
1. Match `(Human $x)` → `{$x ← Socrates}`, ...
2. For each, try `(Dog Socrates)` → Fail
3. Discard binding
4. Result: empty

### Shared Variables

**Constraint**: Shared variables must unify consistently.

**Example:**
```metta
!(match &self
    (, (parent $p $c1)
       (parent $p $c2))
    (siblings $c1 $c2))
; Returns all sibling pairs with shared parent $p
```

**Non-Determinism**: Multiple parents → multiple sibling groups.

## Controlling Non-Determinism

### 1. Limiting Results

**Strategy**: Use conditional to return first match only.

**Example:**
```metta
(= (first-human)
    (let $humans (match &self (Human $x) $x)
        (if (> (length $humans) 0)
            (car $humans)
            (error "no humans"))))
```

### 2. Filtering Results

**Strategy**: Apply predicate to filter.

**Example:**
```metta
(= (adult-humans)
    (match &self
        (age $person $years)
        (if (> $years 18)
            $person
            ())))  ; Empty = filter out
```

### 3. Aggregating Results

**Strategy**: Collect and process all results.

**Example:**
```metta
(= (count-humans)
    (length (match &self (Human $x) $x)))

(= (average-age)
    (let $ages (match &self (age $ $y) $y)
        (/ (sum $ages) (length $ages))))
```

### 4. Deterministic Rules

**Strategy**: Write rules that don't overlap.

**Good:**
```metta
(= (classify 0) zero)
(= (classify $x) (if (> $x 0) positive negative))
```

**Avoid:**
```metta
(= (classify 0) zero)
(= (classify $x) positive)  ; Overlaps with 0!
```

### 5. Explicit Choice

**Strategy**: Use explicit choice operators (if available).

**Example** (conceptual):
```metta
; Choose one result
!(choose (match &self (Human $x) $x))
; → Returns single result (e.g., Socrates)
```

## Non-Determinism in Complex Queries

### Transitive Closure

**Problem**: Multiple paths to same node.

**Example:**
```metta
(= (connected $a $b)
    (match &self (edge $a $b) True))

(= (connected $a $c)
    (match &self
        (, (edge $a $b)
           (connected $b $c))
        True))

; Graph: A → B → C, A → C
!(connected A C)
; → [True, True] (two paths: direct and via B)
```

**Deduplication**: May need explicit handling.

### Combinatorial Explosion

**Problem**: Conjunction produces exponential results.

**Example:**
```metta
; N choices for each of M variables
; Result count: N^M

; 3 colors × 3 sizes × 3 materials = 27 combinations
!(match &self
    (, (color $c)
       (size $s)
       (material $m))
    (product $c $s $m))
```

**Mitigation:**
- Add constraints to reduce matches
- Use specific patterns
- Limit conjunction depth

### Recursive Non-Determinism

**Problem**: Recursion generates many results.

**Example:**
```metta
; All subsets
(= (subsets ()) (()))
(= (subsets ($h $t...))
    (let $st (subsets $t)
        (union $st (map (cons $h) $st))))

!(subsets (1 2 3))
; → [(), (1), (2), (3), (1 2), (1 3), (2 3), (1 2 3)]
; Count: 2^n = 8 results
```

**Control**: Use limits or pruning strategies.

## Performance Implications

### Memory Usage

**Multiple Results**: O(n × size(result))
- n results, each occupying memory

**Example:**
```metta
; 10,000 humans → 10,000 results in memory
!(match &self (Human $x) $x)
```

**Mitigation**: Use lazy evaluation or streaming.

### Computation Time

**Cartesian Product**: Exponential blowup
```
2 matches × 3 matches × 4 matches = 24 results
Time: O(2 × 3 × 4) = O(24)
```

**Recursive Queries**: Potential infinite loops or exponential time.

**Mitigation:**
- Depth limits
- Cycle detection
- Memoization

### Lazy Evaluation

**Strategy**: Don't compute all results upfront.

**Implementation** (conceptual):
```rust
// Return iterator, not Vec
fn query(&self, pattern: &Atom) -> impl Iterator<Item = Bindings> {
    // Lazily generate results on demand
}
```

**Benefits:**
- Reduced memory usage
- Faster for "first N results" queries
- Can stop early

## Best Practices

### 1. Expect Multiple Results

```metta
; Good: handle list of results
(= (process-humans)
    (let $humans (match &self (Human $x) $x)
        (map show-details $humans)))

; Avoid: assume single result
(= (process-humans)
    (show-details (match &self (Human $x) $x)))  ; Error if multiple!
```

### 2. Use Specific Patterns

```metta
; Better: specific pattern (fewer matches)
!(match &self (age Socrates $y) $y)

; Avoid: general pattern (many matches)
!(match &self (age $x $y) ($x $y))
```

### 3. Filter Early

```metta
; Good: filter in pattern
!(match &self
    (, (Human $x)
       (age $x $y)
       (> $y 50))
    $x)

; Avoid: filter after
(filter (λ $x (> (get-age $x) 50))
    (match &self (Human $x) $x))
```

### 4. Document Non-Determinism

```metta
; Document expected result count
; Returns: List of all humans (0 to N results)
(= (all-humans)
    (match &self (Human $x) $x))
```

### 5. Handle Empty Results

```metta
; Good: explicit check
(= (process)
    (let $results (match &self (pattern $x) $x)
        (if (empty? $results)
            (default-value)
            (process-list $results))))
```

## Common Pitfalls

### 1. Assuming Single Result

**Problem:**
```metta
!(head (match &self (Human $x) $x))
; Assumes at least one result - may fail!
```

**Solution:**
```metta
!(let $humans (match &self (Human $x) $x)
    (if (empty? $humans)
        (error "no humans")
        (head $humans)))
```

### 2. Ignoring Result Order

**Problem:**
```metta
!(== (match &self (Human $x) $x)
     (Socrates Plato Aristotle))
; May fail due to different order!
```

**Solution:**
```metta
!(set-equal?
    (match &self (Human $x) $x)
    (Socrates Plato Aristotle))
```

### 3. Combinatorial Explosion

**Problem:**
```metta
; Generates millions of results
!(match &self
    (, (a $x) (b $y) (c $z) (d $w))
    ($x $y $z $w))
```

**Solution**: Add constraints or limit scope.

### 4. Infinite Recursion

**Problem:**
```metta
(= (loop $x) (loop $x))
!(loop 1)  ; Never terminates!
```

**Solution**: Ensure base case and termination.

### 5. Duplicate Results

**Problem:**
```metta
; Multiple paths produce duplicates
!(connected A C)
; → [True, True, True] (3 paths)
```

**Solution**: Use deduplication.
```metta
!(unique (connected A C))
; → [True]
```

## Examples

### Example 1: Multiple Rules

```metta
(= (fib 0) 0)
(= (fib 1) 1)
(= (fib $n)
    (+ (fib (- $n 1))
       (fib (- $n 2))))

!(fib 5)
; Single result (deterministic): 5
; But evaluation explores multiple rules
```

### Example 2: Space Query Non-Determinism

```metta
(add-atom &self (likes Alice pizza))
(add-atom &self (likes Alice pasta))
(add-atom &self (likes Bob pizza))

!(match &self (likes Alice $food) $food)
; → [pizza, pasta]
```

### Example 3: Conjunction Non-Determinism

```metta
(add-atom &self (color red))
(add-atom &self (color blue))
(add-atom &self (shape circle))
(add-atom &self (shape square))

!(match &self
    (, (color $c)
       (shape $s))
    (object $c $s))
; → [(object red circle), (object red square),
;    (object blue circle), (object blue square)]
```

### Example 4: Controlled Non-Determinism

```metta
; Get first matching human
(= (any-human)
    (unify (match &self (Human $x) $x)
        ($first $rest...)
        $first          ; Return first
        (error "none")))  ; Or fail

!(any-human)
; → Socrates (single result)
```

## Related Documentation

**Bindings**: [04-bindings.md](04-bindings.md#bindingsset)
**Match Operation**: [03-match-operation.md](03-match-operation.md)
**Implementation**: [07-implementation.md](07-implementation.md)
**Edge Cases**: [09-edge-cases.md](09-edge-cases.md)

## Summary

**Non-Determinism Sources:**
- Multiple matching rules
- Multiple space matches
- Custom matchers
- Conjunction Cartesian products
- Recursive patterns

**BindingsSet:**
- Represents 0, 1, or many solutions
- Variants: Empty, Single(Bindings), Multi(Vec<Bindings>)
- Operations: union, merge_v2

**Evaluation:**
- Explores all possible reductions
- Returns list of results
- Order not guaranteed

**Control Strategies:**
- Limit results (first, take N)
- Filter with predicates
- Aggregate (count, sum, etc.)
- Write deterministic rules
- Use explicit choice

**Performance:**
- Memory: O(n × result_size)
- Time: Can be exponential (Cartesian product)
- Mitigation: Lazy evaluation, constraints, pruning

**Best Practices:**
✅ Expect multiple results
✅ Use specific patterns
✅ Filter early in queries
✅ Document non-determinism
✅ Handle empty results

**Pitfalls:**
❌ Assuming single result
❌ Relying on result order
❌ Combinatorial explosion
❌ Infinite recursion
❌ Ignoring duplicates

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-17
