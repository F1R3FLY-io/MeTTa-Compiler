# Basic Examples of MORK Conjunctions

**Version**: 1.0
**Date**: 2025-11-24
**Target**: MeTTaTron Compiler / MORK Integration
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Empty Conjunction Examples](#empty-conjunction-examples)
2. [Unary Conjunction Examples](#unary-conjunction-examples)
3. [Binary Conjunction Examples](#binary-conjunction-examples)
4. [N-ary Conjunction Examples](#n-ary-conjunction-examples)
5. [Pattern Matching with Conjunctions](#pattern-matching-with-conjunctions)
6. [Query Examples](#query-examples)
7. [Common Patterns](#common-patterns)
8. [Exercises](#exercises)

---

## Empty Conjunction Examples

### Example 1: Unconditional Rule

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:201-202`

```lisp
(exec P0 (,) (, (Always)))
```

**Meaning**:
- No antecedent conditions `(,)`
- Always produces `(Always)` fact
- Fires unconditionally

**Evaluation**:
```
Step 1: Check antecedent (,) → succeeds (empty always succeeds)
Step 2: Execute consequent (, (Always))
Step 3: Add (Always) to space
```

### Example 2: Initialization Rule

```lisp
(exec init (,) (, (system-ready) (version 1.0)))
```

**Meaning**:
- No preconditions
- Initialize system state with two facts
- Typical startup pattern

**Evaluation**:
```
Step 1: Antecedent (,) succeeds
Step 2: Add (system-ready) to space
Step 3: Add (version 1.0) to space
```

### Example 3: Empty Consequent

```lisp
(exec cleanup (, (temp $x)) (O (- (temp $x))))
```

**Meaning**:
- If `(temp $x)` exists, remove it
- Consequent is an operation, not a conjunction
- Note: Operations use `(O ...)` instead of `(, ...)`

---

## Unary Conjunction Examples

### Example 1: Simple Pattern Match

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:220`

```lisp
(exec P2 (, (NKV $x chr $y)) (,) (, (chr_of $y $x)))
```

**Meaning**:
- If `(NKV $x chr $y)` exists in space
- Then add `(chr_of $y $x)`
- Note the empty middle conjunction `(,)` - likely a guard position

**Example Evaluation**:
```
Given space: { (NKV gene1 chr chr7) }

Step 1: Match (, (NKV $x chr $y))
        Bindings: { $x ↦ gene1, $y ↦ chr7 }
Step 2: Execute consequent (, (chr_of $y $x))
        Substitute: (chr_of chr7 gene1)
Step 3: Add (chr_of chr7 gene1) to space
```

### Example 2: Simple Transitive Rule

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:204`

```lisp
(exec P2 (, (Transitive $x $y)) (, (Line $x $q)))
```

**Meaning**:
- If `(Transitive $x $y)` exists
- Then produce `(Line $x $q)` where `$q` is a fresh variable

**Evaluation**:
```
Given: (Transitive A B)

Step 1: Match (, (Transitive $x $y))
        Bindings: { $x ↦ A, $y ↦ B }
Step 2: Produce (, (Line $x $q))
        Result: (Line A $q)   ; $q is fresh/unbound
```

### Example 3: Fact Query

```lisp
(exec query1 (, (parent Alice $child)) (, (result $child)))
```

**Meaning**:
- Find all children of Alice
- For each match, produce `(result $child)`

**Example Evaluation**:
```
Given space: {
  (parent Alice Bob)
  (parent Alice Charlie)
  (parent Dave Eve)
}

Matches:
  1. (parent Alice Bob)     → Bindings: { $child ↦ Bob }
     Consequent: (result Bob)

  2. (parent Alice Charlie) → Bindings: { $child ↦ Charlie }
     Consequent: (result Charlie)

Results added: { (result Bob), (result Charlie) }
```

---

## Binary Conjunction Examples

### Example 1: Transitivity Rule

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:201`

```lisp
(exec P1 (, (Straight $x $y) (Straight $y $z))
         (, (Transitive $x $z)))
```

**Meaning**:
- If there's a `Straight` edge from `$x` to `$y`
- AND a `Straight` edge from `$y` to `$z`
- Then there's a `Transitive` edge from `$x` to `$z`

**Example Evaluation**:
```
Given space: {
  (Straight A B)
  (Straight B C)
  (Straight B D)
}

Match 1:
  $x ↦ A, $y ↦ B, $z ↦ C
  Consequent: (Transitive A C)

Match 2:
  $x ↦ A, $y ↦ B, $z ↦ D
  Consequent: (Transitive A D)

Results: { (Transitive A C), (Transitive A D) }
```

**Key Point**: The variable `$y` must be the **same** in both antecedent goals (join condition).

### Example 2: Grandparent Query

```lisp
(exec P (, (parent $x $y) (parent $y $z))
         (, (grandparent $x $z)))
```

**Meaning**:
- Classic grandparent relation
- `$x` is parent of `$y`, `$y` is parent of `$z`
- Therefore `$x` is grandparent of `$z`

**Example Evaluation**:
```
Given space: {
  (parent Alice Bob)
  (parent Bob Charlie)
  (parent Bob Dave)
  (parent Eve Frank)
}

Match 1:
  $x ↦ Alice, $y ↦ Bob, $z ↦ Charlie
  Consequent: (grandparent Alice Charlie)

Match 2:
  $x ↦ Alice, $y ↦ Bob, $z ↦ Dave
  Consequent: (grandparent Alice Dave)

Results: { (grandparent Alice Charlie), (grandparent Alice Dave) }
```

### Example 3: Sibling Query

```lisp
(exec P (, (parent $p $x) (parent $p $y))
         (, (sibling $x $y)))
```

**Meaning**:
- If `$p` is parent of both `$x` and `$y`
- Then `$x` and `$y` are siblings

**Example Evaluation**:
```
Given space: {
  (parent Alice Bob)
  (parent Alice Charlie)
  (parent Dave Eve)
}

Match 1:
  $p ↦ Alice, $x ↦ Bob, $y ↦ Bob
  Consequent: (sibling Bob Bob)        ; Self-sibling!

Match 2:
  $p ↦ Alice, $x ↦ Bob, $y ↦ Charlie
  Consequent: (sibling Bob Charlie)

Match 3:
  $p ↦ Alice, $x ↦ Charlie, $y ↦ Bob
  Consequent: (sibling Charlie Bob)

Results: { (sibling Bob Bob), (sibling Bob Charlie), (sibling Charlie Bob) }
```

**Note**: This produces self-siblings and duplicate pairs. A refined version would filter these.

---

## N-ary Conjunction Examples

### Example 1: Complex Gene Query

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:120-121`

```lisp
(exec P1 (, (gene_name_of TP73-AS1 $x)
            (SPO $x includes $y)
            (SPO $x transcribed_from $z))
         (,)
         (, (res0 $x $y $z)))
```

**Meaning**:
- Find gene named `TP73-AS1` (bind to `$x`)
- AND that gene includes something (bind to `$y`)
- AND that gene is transcribed from something (bind to `$z`)
- Produce result with all three values

**Example Evaluation**:
```
Given space: {
  (gene_name_of TP73-AS1 gene_123)
  (SPO gene_123 includes exon_456)
  (SPO gene_123 transcribed_from chr1)
}

Step 1: Match (gene_name_of TP73-AS1 $x)
        Bindings: { $x ↦ gene_123 }

Step 2: Match (SPO $x includes $y) with $x = gene_123
        Bindings: { $x ↦ gene_123, $y ↦ exon_456 }

Step 3: Match (SPO $x transcribed_from $z) with $x = gene_123
        Bindings: { $x ↦ gene_123, $y ↦ exon_456, $z ↦ chr1 }

Step 4: Produce (res0 gene_123 exon_456 chr1)
```

### Example 2: Four-Way Join

```lisp
(exec P (, (employee $e $name)
           (department $e $dept)
           (salary $e $sal)
           (manager $dept $mgr))
         (, (employee-info $name $dept $sal $mgr)))
```

**Meaning**:
- Join four relations on employee/department
- Collect all information into single fact

**Example Evaluation**:
```
Given space: {
  (employee emp1 Alice)
  (department emp1 Engineering)
  (salary emp1 100000)
  (manager Engineering Bob)
}

Step 1: (employee $e $name) → { $e ↦ emp1, $name ↦ Alice }
Step 2: (department $e $dept) with $e = emp1
        → { $e ↦ emp1, $name ↦ Alice, $dept ↦ Engineering }
Step 3: (salary $e $sal) with $e = emp1
        → { $e ↦ emp1, $name ↦ Alice, $dept ↦ Engineering, $sal ↦ 100000 }
Step 4: (manager $dept $mgr) with $dept = Engineering
        → { $e ↦ emp1, $name ↦ Alice, $dept ↦ Engineering,
            $sal ↦ 100000, $mgr ↦ Bob }
Step 5: Produce (employee-info Alice Engineering 100000 Bob)
```

### Example 3: Path Query

```lisp
(exec P (, (edge $a $b)
           (edge $b $c)
           (edge $c $d))
         (, (path-3 $a $d)))
```

**Meaning**:
- Find all 3-hop paths in graph
- Start at `$a`, end at `$d` via `$b` and `$c`

**Example Evaluation**:
```
Given space: {
  (edge A B)
  (edge B C)
  (edge C D)
  (edge B E)
  (edge E D)
}

Match 1:
  $a ↦ A, $b ↦ B, $c ↦ C, $d ↦ D
  Path: A → B → C → D
  Consequent: (path-3 A D)

Match 2:
  $a ↦ A, $b ↦ B, $c ↦ E, $d ↦ D
  Path: A → B → E → D
  Consequent: (path-3 A D)

Results: { (path-3 A D) }  ; Deduplicated if same result
```

---

## Pattern Matching with Conjunctions

### Example 1: Nested Pattern

```lisp
(exec P (, (parent $x $y)
           (person $x (age $age)))
         (, (parent-age $x $age)))
```

**Meaning**:
- Match parent relation
- AND match person record with nested age structure
- Extract age value

**Example Evaluation**:
```
Given space: {
  (parent Alice Bob)
  (person Alice (age 35))
}

Step 1: (parent $x $y) → { $x ↦ Alice, $y ↦ Bob }
Step 2: (person $x (age $age)) with $x = Alice
        Match (person Alice (age $age)) against (person Alice (age 35))
        → { $x ↦ Alice, $y ↦ Bob, $age ↦ 35 }
Step 3: Produce (parent-age Alice 35)
```

### Example 2: List Pattern

```lisp
(exec P (, (cons $head $tail)
           (length $tail $n))
         (, (length (cons $head $tail) (+ $n 1))))
```

**Meaning**:
- If we have a cons cell and know the tail length
- Then the full list length is tail length + 1

**Example Evaluation**:
```
Given space: {
  (cons A (cons B nil))
  (length (cons B nil) 1)
  (length nil 0)
}

Step 1: (cons $head $tail)
        → { $head ↦ A, $tail ↦ (cons B nil) }
Step 2: (length $tail $n) with $tail = (cons B nil)
        → { $head ↦ A, $tail ↦ (cons B nil), $n ↦ 1 }
Step 3: Produce (length (cons A (cons B nil)) (+ 1 1))
        Simplified: (length (cons A (cons B nil)) 2)
```

---

## Query Examples

### Example 1: Find All

```lisp
(exec Q1 (, (parent Alice $child)) (, (child-of-alice $child)))
```

**Query**: Find all children of Alice.

### Example 2: Existential Query

```lisp
(exec Q2 (, (parent $x Alice)) (, (alice-has-parent)))
```

**Query**: Does Alice have a parent? (existential check)

### Example 3: Join Query

```lisp
(exec Q3 (, (parent $p $c) (age $c 5)) (, (parent-of-5yo $p)))
```

**Query**: Find all parents of 5-year-olds.

### Example 4: Aggregate Query

```lisp
(exec Q4 (, (employee $e $_) (salary $e $s)) (, (all-salaries $s)))
```

**Query**: Collect all salaries (using `$_` for unused variable).

---

## Common Patterns

### Pattern 1: Fact Insertion

```lisp
(exec P (,) (, (initial-fact)))
```
Insert a fact unconditionally.

### Pattern 2: Fact Transformation

```lisp
(exec P (, (old-format $x)) (, (new-format $x)))
```
Transform old format to new format.

### Pattern 3: Join

```lisp
(exec P (, (rel1 $x $y) (rel2 $y $z)) (, (joined $x $z)))
```
Join two relations on common variable.

### Pattern 4: Filtering

```lisp
(exec P (, (data $x) (predicate $x)) (, (filtered $x)))
```
Filter data by predicate.

### Pattern 5: Aggregation

```lisp
(exec P (, (item $x) (count-items $n))
         (, (count-items (+ $n 1))))
```
Count items (requires careful ordering).

---

## Exercises

### Exercise 1: Ancestry

**Given**:
```lisp
(parent Alice Bob)
(parent Bob Charlie)
(parent Charlie Dave)
```

**Write a rule**: Find all ancestor relationships (transitive closure of parent).

**Solution**:
```lisp
; Direct parent is ancestor
(exec A1 (, (parent $x $y)) (, (ancestor $x $y)))

; Transitive: if $x ancestor of $y and $y ancestor of $z, then $x ancestor of $z
(exec A2 (, (ancestor $x $y) (ancestor $y $z))
         (, (ancestor $x $z)))
```

### Exercise 2: Cousin

**Given**:
```lisp
(parent Alice Bob)
(parent Alice Charlie)
(parent Dave Eve)
(parent Dave Frank)
(parent Bob G)
(parent Charlie H)
```

**Write a rule**: Define cousin relationship (children of siblings).

**Solution**:
```lisp
(exec C (, (parent $gp $p1)
           (parent $gp $p2)
           (parent $p1 $c1)
           (parent $p2 $c2))
        (, (cousin $c1 $c2)))
```

### Exercise 3: Shortest Path

**Given**: A graph with weighted edges.

**Write a rule**: Find paths with total weight less than 10.

**Solution** (simplified):
```lisp
(exec P (, (edge $a $b $w1)
           (edge $b $c $w2)
           (< (+ $w1 $w2) 10))
        (, (short-path $a $c (+ $w1 $w2))))
```

---

## Summary

### Key Takeaways

1. **Empty conjunction** `(,)` - Always succeeds, no conditions
2. **Unary conjunction** `(, e)` - Single condition wrapper
3. **Binary conjunction** `(, e1 e2)` - Join two conditions
4. **N-ary conjunction** `(, e1 ... en)` - Multiple conditions
5. **Variable binding** - Propagates left-to-right through conjunction
6. **Pattern matching** - Each goal can use complex patterns
7. **Non-determinism** - Multiple matches explored automatically

### Common Use Cases

- **Queries**: Finding data matching patterns
- **Rules**: Deriving new facts from existing ones
- **Joins**: Combining multiple relations
- **Transformations**: Converting data formats
- **Filters**: Selecting data meeting criteria

---

## Next Steps

Continue to [Advanced Examples](04-examples-advanced.md) for coalgebra patterns and meta-programming.

---

**Related Documentation**:
- [Introduction](01-introduction.md)
- [Syntax and Semantics](02-syntax-and-semantics.md)
- [Advanced Examples](04-examples-advanced.md)
