# Syntax and Semantics of MORK Conjunctions

**Version**: 1.0
**Date**: 2025-11-24
**Target**: MeTTaTron Compiler / MORK Integration
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Formal Syntax](#formal-syntax)
2. [Evaluation Semantics](#evaluation-semantics)
3. [Conjunction in exec Forms](#conjunction-in-exec-forms)
4. [Conjunction in coalg Forms](#conjunction-in-coalg-forms)
5. [Conjunction in lookup Forms](#conjunction-in-lookup-forms)
6. [Variable Binding and Conjunction](#variable-binding-and-conjunction)
7. [Type System Implications](#type-system-implications)
8. [Syntactic Properties](#syntactic-properties)

---

## Formal Syntax

### BNF Grammar

```bnf
<expr> ::= <symbol>
         | <variable>
         | <arity-expr>

<arity-expr> ::= "(" <symbol> <expr>* ")"

<conjunction> ::= "(" "," <expr>* ")"

<exec-form> ::= "(exec" <priority> <conjunction> <conjunction> ")"
              | "(exec" <priority> <conjunction> <operation> ")"

<coalg-form> ::= "(coalg" <pattern> <conjunction> ")"

<lookup-form> ::= "(lookup" <pattern> <conjunction> <conjunction> ")"

<operation> ::= "(O" <op-elem>* ")"

<op-elem> ::= "(+" <expr> ")"
            | "(-" <expr> ")"
```

### Conjunction Syntax

**Empty Conjunction**:
```lisp
(,)
```

**Unary Conjunction**:
```lisp
(, <expr>)
```

**N-ary Conjunction** (n ≥ 2):
```lisp
(, <expr1> <expr2> ... <exprN>)
```

### Examples

```lisp
; Empty conjunction
(,)

; Unary conjunction with symbol
(, foo)

; Unary conjunction with expression
(, (parent Alice Bob))

; Binary conjunction
(, (parent $x $y) (parent $y $z))

; Ternary conjunction
(, (gene_name_of TP73-AS1 $x)
   (SPO $x includes $y)
   (SPO $x transcribed_from $z))
```

---

## Evaluation Semantics

### Conjunction Evaluation Rules

**Empty Conjunction**:
```
⟦(,)⟧_ρ = { ρ }
```
- Returns the current environment unchanged
- Always succeeds
- Used for unconditional rules

**Unary Conjunction**:
```
⟦(, e)⟧_ρ = ⟦e⟧_ρ
```
- Evaluates the single expression
- Returns bindings if successful, fails otherwise
- Equivalent to evaluating `e` directly

**N-ary Conjunction** (n ≥ 2):
```
⟦(, e₁ e₂ ... eₙ)⟧_ρ =
  let ρ₁ = ⟦e₁⟧_ρ in
  let ρ₂ = ⟦e₂⟧_ρ₁ in
  ...
  let ρₙ = ⟦eₙ⟧_ρₙ₋₁ in
  ρₙ
```
- Evaluates expressions left-to-right
- Each expression extends the environment
- Fails if any expression fails
- Threading bindings through conjunction

### Operational Semantics

#### Step-by-Step Evaluation

**Empty**:
```
    ρ ⊢ true
─────────────────
ρ ⊢ (,) ⇒ { ρ }
```

**Unary**:
```
    ρ ⊢ e ⇒ ρ'
──────────────────
ρ ⊢ (, e) ⇒ ρ'
```

**Binary**:
```
ρ ⊢ e₁ ⇒ ρ'    ρ' ⊢ e₂ ⇒ ρ''
────────────────────────────────
    ρ ⊢ (, e₁ e₂) ⇒ ρ''
```

**N-ary** (recursive):
```
ρ ⊢ e₁ ⇒ ρ'    ρ' ⊢ (, e₂ ... eₙ) ⇒ ρ''
──────────────────────────────────────────
    ρ ⊢ (, e₁ e₂ ... eₙ) ⇒ ρ''
```

### Non-Deterministic Evaluation

When goals can produce multiple solutions:

```
ρ ⊢ e₁ ⇒ { ρ'₁, ρ'₂, ..., ρ'ₖ }
ρ'ᵢ ⊢ e₂ ⇒ { ρ''ᵢ₁, ρ''ᵢ₂, ..., ρ''ᵢⱼ } for each i
────────────────────────────────────────────────────
ρ ⊢ (, e₁ e₂) ⇒ { ρ''₁₁, ρ''₁₂, ..., ρ''ₖⱼ }
```

All compatible bindings are explored.

---

## Conjunction in exec Forms

### Exec Form Structure

```lisp
(exec <priority> <antecedent> <consequent>)
```

Where:
- `<priority>` - Rule priority (number or tuple)
- `<antecedent>` - Conjunction of conditions (must match)
- `<consequent>` - Conjunction of actions (execute if antecedent succeeds)
                   OR operation `(O ...)` for space modifications

### Antecedent Semantics

The antecedent is **always** a conjunction:

```lisp
(exec P (, (condition1) (condition2) ...) <consequent>)
```

**Evaluation**:
1. Try to match all conditions in the conjunction
2. If all match with compatible bindings, proceed to consequent
3. If any fails, the rule does not fire

**Example**:
```lisp
(exec 0
      (, (parent $x $y) (parent $y $z))
      (, (grandparent $x $z)))
```

This matches when:
- There exists a `parent` relation between `$x` and `$y`
- AND there exists a `parent` relation between `$y` and `$z`
- The `$y` variable must be consistent across both

### Consequent Semantics

The consequent can be:

**Conjunction** (for pattern-based rules):
```lisp
(exec P <antecedent> (, (result1) (result2) ...))
```

**Operation** (for space modifications):
```lisp
(exec P <antecedent> (O (+ (fact1)) (- (fact2)) ...))
```

### Empty Antecedent

```lisp
(exec P (,) (, (always-true)))
```

Meaning: "This rule always fires (no preconditions)."

**Use Cases**:
- Initialization
- Unconditional facts
- Bootstrap rules

---

## Conjunction in coalg Forms

### Coalgebra Form Structure

```lisp
(coalg <pattern> <templates>)
```

Where:
- `<pattern>` - Input pattern to match
- `<templates>` - Conjunction of output templates (unfold results)

### Templates as Conjunctions

The templates are **always** a conjunction representing unfolding results:

```lisp
(coalg (tree $tree) (, (ctx $tree nil)))
```

**Single Result**: Coalgebra produces one output

```lisp
(coalg (ctx (branch $left $right) $path)
       (, (ctx $left  (cons $path L))
          (ctx $right (cons $path R))))
```

**Multiple Results**: Coalgebra produces multiple outputs

### Evaluation Semantics

```
Given: (coalg pattern templates)
If input matches pattern with bindings ρ:
  For each template t in templates:
    Produce output by substituting ρ into t
```

**Example**:
```lisp
Input:  (ctx (branch (leaf 1) (leaf 2)) nil)
Pattern: (ctx (branch $left $right) $path)
Bindings: { $left ↦ (leaf 1), $right ↦ (leaf 2), $path ↦ nil }

Templates: (, (ctx $left  (cons $path L))
              (ctx $right (cons $path R)))

Results:
  - (ctx (leaf 1) (cons nil L))
  - (ctx (leaf 2) (cons nil R))
```

### Empty Templates

```lisp
(coalg (terminal-state) (,))
```

Meaning: "This pattern produces no outputs (termination)."

---

## Conjunction in lookup Forms

### Lookup Form Structure

```lisp
(lookup <pattern> <success-goals> <failure-goals>)
```

Where:
- `<pattern>` - Pattern to search for
- `<success-goals>` - Conjunction executed if pattern found
- `<failure-goals>` - Conjunction executed if pattern not found

### Both Branches Use Conjunctions

```lisp
(lookup $y
  (, T)                      ; Success branch: single goal
  (, $cy)                    ; Failure branch: single goal
)
```

**Evaluation**:
1. Try to find pattern in space
2. If found, execute `success-goals` conjunction
3. If not found, execute `failure-goals` conjunction

### Nested Example

```lisp
(lookup $p
  (, (lookup $t $px $tx))              ; Success: nested lookup
  (, (exec (0 $t) $px $tx))            ; Failure: exec rule
)
```

Source: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/main.rs:918-921`

---

## Variable Binding and Conjunction

### Binding Propagation

Variables bind left-to-right through conjunction:

```lisp
(, (parent $x Alice) (age $x 30))
```

**Step 1**: `(parent $x Alice)` binds `$x` to some value (e.g., `Bob`)
**Step 2**: `(age $x 30)` uses that binding, checking `(age Bob 30)`

### Multiple Solutions

If a goal produces multiple bindings:

```lisp
(, (parent $x $y) (age $y 10))
```

**Step 1**: `(parent $x $y)` might produce:
- `{ $x ↦ Alice, $y ↦ Bob }`
- `{ $x ↦ Alice, $y ↦ Charlie }`
- `{ $x ↦ Dave, $y ↦ Eve }`

**Step 2**: For each binding, check `(age $y 10)`:
- Try `(age Bob 10)`
- Try `(age Charlie 10)`
- Try `(age Eve 10)`

Only bindings where both goals succeed are kept.

### Variable Scoping

Variables in conjunctions follow standard scoping:

```lisp
(exec P
  (, (parent $x $y) (age $y $n))      ; $x, $y, $n bound here
  (, (young-parent $x $y $n)))        ; Can use $x, $y, $n here
```

Variables bound in antecedent are available in consequent.

---

## Type System Implications

### Conjunction Type

In a typed setting, conjunctions have type:

```
(,) : Bool                              ; Empty conjunction
(, e) : τ if e : τ                      ; Unary preserves type
(, e₁ e₂) : Bool if e₁, e₂ : Bool      ; Multi-ary is Boolean AND
```

### Exec Type

```
exec : Priority → Conjunction → Conjunction → Rule
exec : Priority → Conjunction → Operation → Rule
```

### Coalg Type

```
coalg : Pattern → Conjunction → CoalgebraRule
```

### Type Checking Conjunctions

**Homogeneous Typing**:
```lisp
(, (parent $x $y) (age $y 10))
```
Both expressions are predicates (type `Bool` / `Atom`).

**Heterogeneous Typing** (for templates):
```lisp
(, (ctx $left (cons $path L))
   (ctx $right (cons $path R)))
```
Both expressions are data constructors (type `Context`).

---

## Syntactic Properties

### Associativity

Conjunction is **associative**:

```
(, (, e₁ e₂) e₃) ≡ (, e₁ (, e₂ e₃)) ≡ (, e₁ e₂ e₃)
```

MORK uses the flattened form `(, e₁ e₂ e₃)` consistently.

### Commutativity

Conjunction is **NOT commutative** due to variable binding:

```
(, (parent $x Alice) (age $x 30))  ≠  (, (age $x 30) (parent $x Alice))
```

First form: `$x` bound by first goal, used by second.
Second form: `$x` unbound in first goal (pattern match failure).

### Identity Element

The empty conjunction `(,)` acts as identity for sequential composition:

```
(, (,) e) ≡ e ≡ (, e (,))
```

In practice, MORK normalizes these to `(, e)`.

### Idempotence

Conjunction is **NOT idempotent**:

```
(, e) ≠ (, e e)
```

Evaluating `e` twice can have different effects (side effects, non-determinism).

---

## Summary

### Key Points

1. **Syntax**: Conjunctions always use `(, ...)` form
2. **Evaluation**: Left-to-right with binding propagation
3. **Exec**: Both antecedent and consequent are conjunctions
4. **Coalg**: Templates are conjunctions (unfold results)
5. **Lookup**: Both success and failure branches are conjunctions
6. **Bindings**: Thread through conjunction left-to-right
7. **Non-determinism**: Multiple solutions explored systematically
8. **Types**: Conjunction preserves or combines types appropriately

### Invariants

- Every goal position is a conjunction
- Empty conjunction represents "true" / no-op
- Unary conjunction wraps single goal
- N-ary conjunction represents sequential AND
- Variable bindings propagate left-to-right

---

## Next Steps

Continue to [Basic Examples](03-examples-basic.md) to see these semantics in action.

---

**Related Documentation**:
- [Introduction](01-introduction.md)
- [Basic Examples](03-examples-basic.md)
- [Pattern Matching](../pattern-matching.md)
- [Evaluation Engine](../evaluation-engine.md)
