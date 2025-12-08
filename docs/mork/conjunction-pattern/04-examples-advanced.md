# Advanced Examples: Coalgebra and Meta-Programming

**Version**: 1.0
**Date**: 2025-11-24
**Target**: MeTTaTron Compiler / MORK Integration
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Coalgebra Patterns](#coalgebra-patterns)
2. [Tree-to-Space Transformation](#tree-to-space-transformation)
3. [Meta-Programming with rulify](#meta-programming-with-rulify)
4. [Nested Exec Forms](#nested-exec-forms)
5. [Complete Tree Analysis Walkthrough](#complete-tree-analysis-walkthrough)
6. [Fixed Point Rewriting](#fixed-point-rewriting)
7. [Advanced Patterns](#advanced-patterns)

---

## Coalgebra Patterns

### What is a Coalgebra?

A **coalgebra** is a pattern that **unfolds** a structure into one or more results. Think of it as the opposite of an algebra:

- **Algebra**: Combine multiple inputs → single output (e.g., `add(2, 3) → 5`)
- **Coalgebra**: Unfold single input → multiple outputs (e.g., `split(tree) → [left, right]`)

### Coalgebra Syntax in MORK

```lisp
(coalg <pattern> <templates>)
```

Where `<templates>` is **always a conjunction** of unfold results.

### Why Conjunctions for Coalgebra?

The conjunction wrapper makes it explicit how many results the coalgebra produces:

```lisp
; Single result: wraps into context
(coalg (tree $tree) (, (ctx $tree nil)))

; Two results: split into left and right
(coalg (ctx (branch $left $right) $path)
       (, (ctx $left  (cons $path L))
          (ctx $right (cons $path R))))

; No results: termination
(coalg (done) (,))
```

This uniformity enables meta-programs to generate and manipulate coalgebras mechanically.

---

## Tree-to-Space Transformation

### Complete Example

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:847-856`

This example transforms a tree into a space by unfolding it step by step.

### Step 1: Lift (Initial)

```lisp
(tree-to-space lift-tree
  (coalg (tree $tree) (, (ctx $tree nil))))
```

**Meaning**:
- Takes a `(tree <structure>)` expression
- Wraps it into a context: `(ctx <structure> nil)`
- Path starts as `nil` (root position)

**Example**:
```
Input:  (tree (branch (leaf 1) (leaf 2)))
Output: (, (ctx (branch (leaf 1) (leaf 2)) nil))
```

**Key Point**: Single result, but wrapped in conjunction `(, ...)`.

### Step 2: Explode (Bulk Processing)

```lisp
(tree-to-space explode-tree
  (coalg (ctx (branch $left $right) $path)
         (, (ctx $left  (cons $path L))
            (ctx $right (cons $path R)))))
```

**Meaning**:
- Takes a context with a `branch` node
- **Unfolds into TWO results**: left subtree and right subtree
- Extends path with `L` (left) or `R` (right) markers

**Example**:
```
Input:  (ctx (branch (leaf 1) (leaf 2)) nil)

Output: (, (ctx (leaf 1) (cons nil L))
           (ctx (leaf 2) (cons nil R)))

Two separate contexts:
  1. (ctx (leaf 1) (cons nil L))   ; Left subtree at path [L]
  2. (ctx (leaf 2) (cons nil R))   ; Right subtree at path [R]
```

**Key Point**: Binary conjunction produces TWO results.

### Step 3: Drop (Terminal)

```lisp
(tree-to-space drop-tree
  (coalg (ctx (leaf $value) $path) (, (value $path $value))))
```

**Meaning**:
- Takes a context with a `leaf` node (terminal)
- Produces a final `value` fact with path and data
- Single result

**Example**:
```
Input:  (ctx (leaf 1) (cons nil L))
Output: (, (value (cons nil L) 1))
```

**Key Point**: Single result, conjunction wrapper maintained.

### Complete Transformation

**Input**:
```lisp
(tree (branch (leaf 11) (leaf 12)))
```

**Step-by-Step Execution**:

```
Step 0 (Initial):
  (tree (branch (leaf 11) (leaf 12)))

Step 1 (Lift):
  (ctx (branch (leaf 11) (leaf 12)) nil)

Step 2 (Explode):
  (ctx (leaf 11) (cons nil L))
  (ctx (leaf 12) (cons nil R))

Step 3 (Drop left):
  (value (cons nil L) 11)

Step 4 (Drop right):
  (value (cons nil R) 12)

Final Space:
  { (value (cons nil L) 11),
    (value (cons nil R) 12) }
```

### Why Conjunctions Matter Here

Without conjunction wrappers, how would you know if a coalgebra produces 0, 1, or 2 results?

```lisp
; Ambiguous:
(coalg pattern template)           ; Is template one result or a list?

; Clear:
(coalg pattern (, template))        ; One result
(coalg pattern (, t1 t2))          ; Two results
(coalg pattern (,))                 ; Zero results
```

The conjunction makes the **cardinality explicit**.

---

## Meta-Programming with rulify

### The rulify Meta-Program

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:862-863`

```lisp
(rulify $name (, $p0) (, $t0)
  (, (tmp $p0))
  (O (- (tmp $p0)) (+ (tmp $t0)) (+ (has changed))))

(rulify $name (, $p0) (, $t0 $t1)
  (, (tmp $p0))
  (O (- (tmp $p0)) (+ (tmp $t0)) (+ (tmp $t1)) (+ (has changed))))
```

### What is rulify?

`rulify` is a **meta-program** that generates execution rules from coalgebra definitions.

**Purpose**: Convert coalgebra patterns into rewrite rules that operate on the space.

### First Rule: Single Template

```lisp
(rulify $name (, $p0) (, $t0) (, (tmp $p0)) ...)
```

**Pattern Matching**:
- `$name` - Name of the coalgebra
- `(, $p0)` - **Unary conjunction** (single input pattern)
- `(, $t0)` - **Unary conjunction** (single output template)
- `(, (tmp $p0))` - Antecedent (match temp data)

**Generated Rule Effect**:
```lisp
If (tmp <matches-p0>) exists:
  Remove (tmp <matches-p0>)
  Add <matches-t0>
  Mark (has changed)
```

### Second Rule: Binary Template

```lisp
(rulify $name (, $p0) (, $t0 $t1) (, (tmp $p0)) ...)
```

**Pattern Matching**:
- Same `$name` and pattern `(, $p0)`
- `(, $t0 $t1)` - **Binary conjunction** (two output templates)
- `(, (tmp $p0))` - Same antecedent

**Generated Rule Effect**:
```lisp
If (tmp <matches-p0>) exists:
  Remove (tmp <matches-p0>)
  Add <matches-t0>
  Add <matches-t1>
  Mark (has changed)
```

### Why Uniform Conjunctions Enable This

The rulify meta-program can **pattern match on conjunction structure**:

```lisp
(, $t0)      ; Matches single-template coalgebras
(, $t0 $t1)  ; Matches binary-template coalgebras
```

Without explicit conjunction wrappers, this would be impossible—you couldn't distinguish:
- "One template that happens to be a pair"
- "Two separate templates"

### Practical Application

**Given Coalgebra**:
```lisp
(tree-to-space explode-tree
  (coalg (ctx (branch $left $right) $path)
         (, (ctx $left  (cons $path L))
            (ctx $right (cons $path R)))))
```

**Rulify Matches Second Form** (binary template):
- `$name` ↦ `explode-tree`
- `$p0` ↦ `(ctx (branch $left $right) $path)`
- `$t0` ↦ `(ctx $left  (cons $path L))`
- `$t1` ↦ `(ctx $right (cons $path R))`

**Generated Rule** (conceptual):
```lisp
(exec (0 explode-tree)
  (, (tmp (ctx (branch $left $right) $path)))
  (O (- (tmp (ctx (branch $left $right) $path)))
     (+ (ctx $left  (cons $path L)))
     (+ (ctx $right (cons $path R)))
     (+ (has changed))))
```

This rule will:
1. Match any temp context with a branch
2. Remove the temp
3. Add two new contexts (left and right subtrees)
4. Signal that space changed

---

## Nested Exec Forms

### Complex Example

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:906-928`

```lisp
(exec 0
  (,
    (ana $cx $x $p $cpt $t $y $cy)
    $cx
    $cpt
  )
  (,
    (exec 0
      (, (lookup $x $px $tx))
      (, (exec (0 $x) $px $tx))
    )

    (lookup $p
      (, (lookup $t $px $tx))
      (, (exec (0 $t) $px $tx))
    )

    (lookup $y
      (, T)
      (, $cy)
    )
  )
)
```

### Breaking Down the Structure

**Outer Exec**:
```lisp
(exec 0 <antecedent> <consequent>)
```

**Antecedent** (ternary conjunction):
```lisp
(,
  (ana $cx $x $p $cpt $t $y $cy)  ; Match ana definition
  $cx                               ; Check $cx
  $cpt                              ; Check $cpt
)
```

All three must be satisfied for rule to fire.

**Consequent** (ternary conjunction of complex forms):
```lisp
(,
  <inner-exec-1>
  <lookup-form-1>
  <lookup-form-2>
)
```

Three separate actions execute in sequence.

### Inner Exec 1

```lisp
(exec 0
  (, (lookup $x $px $tx))
  (, (exec (0 $x) $px $tx))
)
```

**Meaning**:
- **Antecedent**: Unary conjunction `(, (lookup $x $px $tx))`
  - Match a `lookup` pattern for `$x`
- **Consequent**: Unary conjunction `(, (exec (0 $x) $px $tx))`
  - Generate a new exec rule dynamically

**Key Point**: Both antecedent and consequent use unary conjunctions for single elements.

### Lookup Form 1

```lisp
(lookup $p
  (, (lookup $t $px $tx))
  (, (exec (0 $t) $px $tx))
)
```

**Meaning**:
- Try to find `$p` in space
- **Success branch**: Unary conjunction `(, (lookup $t $px $tx))`
  - Execute nested lookup for `$t`
- **Failure branch**: Unary conjunction `(, (exec (0 $t) $px $tx))`
  - Generate exec rule

**Key Point**: Both branches use unary conjunctions even though they're single actions.

### Lookup Form 2

```lisp
(lookup $y
  (, T)
  (, $cy)
)
```

**Meaning**:
- Try to find `$y` in space
- **Success branch**: `(, T)` - Just the symbol `T` (true)
- **Failure branch**: `(, $cy)` - Execute `$cy`

**Key Point**: Even a single symbol `T` is wrapped in conjunction `(, T)`.

### Why This Structure Works

The **uniform conjunction pattern** allows:

1. **Parser Uniformity**: Every goal position is a conjunction
2. **Evaluator Simplicity**: Same code handles all branches
3. **Meta-Level Manipulation**: Outer exec can introspect inner structures
4. **Composability**: Inner execs and lookups nest arbitrarily deep

Without uniform conjunctions, the parser would need special cases:
- "Is this position a single action or multiple?"
- "Is this a naked symbol or a wrapped goal?"
- "Should I flatten this structure?"

---

## Complete Tree Analysis Walkthrough

### Problem

Transform a binary tree into a trie by unfolding it step-by-step.

**Input Tree**:
```lisp
(branch (branch (leaf 111) (leaf 112)) (leaf 12))
```

**Desired Output**:
```lisp
(value (cons (cons (cons nil L) L) L) 111)
(value (cons (cons (cons nil L) L) R) 112)
(value (cons (cons nil L) R) 12)
```

### Solution Components

**1. Coalgebra Definitions** (from main.rs:901-902):
```lisp
(tree-to-space (ctx (branch $left $right) $path)
               (ctx $left  (cons $path L)))
(tree-to-space (ctx (branch $left $right) $path)
               (ctx $right (cons $path R)))
```

**Note**: These are split into two separate rules (not one rule with binary conjunction).

**2. Ana (Anamorphism) Definition** (main.rs:904):
```lisp
(ana (tree-example $tree)       ; Seed
     (ctx $tree nil)            ; Initial context
     $p                         ; Pattern variable
     (tree-to-space $p $t)      ; Coalgebra
     $t                         ; Template variable
     (ctx (leaf $value) $path)  ; Terminal pattern
     (space-example (value $path $value)))  ; Final result
```

**3. Exec Rule** (main.rs:906-928):
Processes the ana definition (shown above).

### Execution Trace

**Initial State**:
```lisp
Space: { (tree-example (branch (branch (leaf 111) (leaf 112)) (leaf 12))) }
```

**Step 1: Initialize**:
```
Ana matches, creates initial context:
  (ctx (branch (branch (leaf 111) (leaf 112)) (leaf 12)) nil)
```

**Step 2: Unfold Root**:
```
Match: (ctx (branch $left $right) $path)
  $left  = (branch (leaf 111) (leaf 112))
  $right = (leaf 12)
  $path  = nil

Apply coalgebra (two rules):
  Rule 1: (ctx (branch (leaf 111) (leaf 112)) (cons nil L))
  Rule 2: (ctx (leaf 12) (cons nil R))
```

**Step 3: Unfold Left Branch**:
```
Match: (ctx (branch $left $right) $path)
  $left  = (leaf 111)
  $right = (leaf 112)
  $path  = (cons nil L)

Apply coalgebra:
  Rule 1: (ctx (leaf 111) (cons (cons nil L) L))
  Rule 2: (ctx (leaf 112) (cons (cons nil L) R))
```

**Step 4: Process Right (Leaf)**:
```
Match terminal: (ctx (leaf $value) $path)
  $value = 12
  $path  = (cons nil R)

Produce: (space-example (value (cons nil R) 12))
```

**Step 5: Process Left-Left (Leaf)**:
```
Match terminal: (ctx (leaf $value) $path)
  $value = 111
  $path  = (cons (cons nil L) L)

Produce: (space-example (value (cons (cons nil L) L) 111))
```

**Step 6: Process Left-Right (Leaf)**:
```
Match terminal: (ctx (leaf $value) $path)
  $value = 112
  $path  = (cons (cons nil L) R)

Produce: (space-example (value (cons (cons nil L) R) 112))
```

**Final Space**:
```lisp
{
  (space-example (value (cons (cons nil L) L) 111))
  (space-example (value (cons (cons nil L) R) 112))
  (space-example (value (cons nil R) 12))
}
```

### Role of Conjunctions

Throughout this process:

1. **Coalgebra templates** use conjunctions to indicate multiple results
2. **Lookup branches** use conjunctions for uniform structure
3. **Exec antecedents/consequents** use conjunctions consistently
4. **Meta-level processing** relies on this uniformity

---

## Fixed Point Rewriting

### The Pattern

**Source**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:864-865`

```lisp
(exec (1 system)
  (, (tree-to-space $name (coalg $p $ts))
     (rulify $name (, $p) $ts $ruleps $rulets)
     (has changed)
     (exec (1 system) $sps $sts))
  (O (+ (exec (0 $name) $ruleps $rulets))
     (- (has changed))
     (+ (exec (1 system) $sps $sts))))
```

### Breaking It Down

**Antecedent** (quaternary conjunction):
```lisp
(,
  (tree-to-space $name (coalg $p $ts))     ; Match coalgebra definition
  (rulify $name (, $p) $ts $ruleps $rulets) ; Generate rule components
  (has changed)                             ; Check change flag
  (exec (1 system) $sps $sts)              ; Match system exec (self-reference!)
)
```

**Consequent** (operation):
```lisp
(O
  (+ (exec (0 $name) $ruleps $rulets))     ; Add generated rule
  (- (has changed))                         ; Clear change flag
  (+ (exec (1 system) $sps $sts))          ; Restore system exec
)
```

### What This Does

This is a **self-modifying rule** that:

1. Finds coalgebra definitions
2. Compiles them into executable rules using `rulify`
3. Installs the generated rules into the space
4. Continues until no more changes occur (fixed point)

### Why Conjunctions Are Essential

The antecedent needs to match **four separate conditions** in sequence:
- Coalgebra exists
- Rulify succeeds
- Change flag is set
- System exec exists

Without uniform conjunctions, expressing this complex condition would require nested special forms or different syntax.

---

## Advanced Patterns

### Pattern 1: Dynamic Rule Generation

```lisp
(exec meta
  (, (define-rule $name $pattern $template))
  (, (exec 0 (, $pattern) (, $template))))
```

Generates new rules from specifications.

### Pattern 2: Rule Composition

```lisp
(exec compose
  (, (rule $r1 $p1 $t1) (rule $r2 $p2 $t2))
  (, (composed-rule (seq $r1 $r2)
                    (, $p1 $p2)
                    (, $t1 $t2))))
```

Composes two rules into a sequential rule.

### Pattern 3: Stratified Evaluation

```lisp
(exec stratum-1
  (, (stratum 1) (rule-priority $p) (< $p 100))
  (, (active-rule $p)))

(exec stratum-2
  (, (stratum 2) (rule-priority $p) (>= $p 100))
  (, (active-rule $p)))
```

Controls evaluation order by strata.

---

## Summary

### Key Takeaways

1. **Coalgebras use conjunctions** to express unfold cardinality
2. **Meta-programming** relies on uniform conjunction structure for pattern matching
3. **Nested forms** maintain uniformity at all levels
4. **Fixed-point rewriting** enabled by self-referential rules with complex antecedents
5. **Tree transformations** naturally express as coalgebra unfoldings

### Conjunction Benefits in Advanced Patterns

- **Coalgebra**: Explicit result count
- **Meta-programming**: Structural pattern matching
- **Nested exec**: Uniform composition
- **Self-modification**: Complex multi-condition rules

---

## Next Steps

Continue to [Implementation Details](05-implementation.md) to see how these patterns are implemented in the parser and evaluator.

---

**Related Documentation**:
- [Basic Examples](03-examples-basic.md)
- [Syntax and Semantics](02-syntax-and-semantics.md)
- [Implementation](05-implementation.md)
- [Algebraic Operations](../algebraic-operations.md)
