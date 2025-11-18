# Reduction Order

## Abstract

This document specifies the ordering semantics of reduction rules in MeTTa, including how reduction rules are defined, matched, applied, and the order in which multiple matching rules are processed. Reduction is the core mechanism for computation in MeTTa, making understanding its ordering critical for program correctness.

## Table of Contents

1. [Reduction Fundamentals](#reduction-fundamentals)
2. [Reduction Rules](#reduction-rules)
3. [Rule Application Order](#rule-application-order)
4. [Multiple Matching Rules](#multiple-matching-rules)
5. [Reduction Strategies](#reduction-strategies)
6. [Implementation Details](#implementation-details)
7. [Examples](#examples)

---

## Reduction Fundamentals

### Definition

**Reduction** is the process of transforming an expression according to defined rules.

**Formal Notation**:
```
e → e'
```

Reads as: "expression `e` reduces to `e'` in one step".

### Reduction vs Evaluation

**Distinction**:
- **Reduction**: Single transformation step (e → e')
- **Evaluation**: Complete computation (e ⇓ v)

**Relationship**:
```
e ⇓ v  iff  e →* v  (v is a value)
```

Where `→*` is the reflexive transitive closure of `→` (zero or more reduction steps).

### Normal Forms

**Definition**: An expression is in **normal form** if no reduction rules apply.

**Example Normal Forms**:
- Numbers: `42`, `3.14`
- Symbols: `foo`, `bar`
- Some expressions: `(A B C)` (if no rules match)

**Non-Normal Forms**:
- Reducible expressions: `(+ 1 2)` → `3`
- Function applications: `(double 5)` → `10`

---

## Reduction Rules

### Equality-Based Rules

In MeTTa, reduction rules are defined using the equality symbol `=`.

**Syntax**:
```metta
(= <pattern> <result>)
```

**Semantics**:
- When an expression matches `<pattern>`, it can reduce to `<result>`
- Variables in `<pattern>` are bound and substituted into `<result>`

**Example**:
```metta
(= (double $x) (* $x 2))
```

This rule says:
```
(double <expr>) → (* <expr> 2)
```

### Built-in vs User-Defined Rules

**Built-in Rules**: Defined in the interpreter (e.g., `+`, `-`, `*`, `/`)

**User-Defined Rules**: Defined in MeTTa code via `=`

**Priority**: Implementation-specific (typically built-ins checked first)

### Conditional Reductions

Rules can have implicit conditions:

```metta
; Only reduces if $x is bound to a number
(= (inc $x) (+ $x 1))
```

If `(inc foo)` is evaluated and `foo` is not a number, `+` may fail to reduce.

### Recursive Reductions

Rules can be recursive:

```metta
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))
```

**Termination**: Not guaranteed - recursive rules may loop forever.

---

## Rule Application Order

### Single Rule Application

When an expression matches a single rule:

**Process**:
1. Match expression against rule pattern
2. Bind variables
3. Substitute bindings into result
4. Return result

**Example**:
```metta
(= (double $x) (* $x 2))

!(double 5)
```

**Reduction**:
```
(double 5)
→ Match pattern: (double $x) with {$x ↦ 5}
→ Substitute into result: (* 5 2)
→ Result: (* 5 2)
→ Further reduce: 10
```

### Multiple Rule Application

When multiple rules match:

**Process**:
1. Query space for all rules matching expression
2. Create alternative branch for each match
3. Evaluate all branches (non-deterministic)

**Example**:
```metta
(= (color) red)
(= (color) green)
(= (color) blue)

!(color)
```

**Reduction**:
```
(color)
→ Match against all three rules
→ Branch 1: red
→ Branch 2: green
→ Branch 3: blue

Result: {red, green, blue}
```

### Rule Priority

**Specification**: MeTTa does **not** specify rule priorities.

**Implementation**: All matching rules are applied (non-deterministically).

**Comparison with Other Languages**:
- **Prolog**: First matching clause (sequential)
- **Haskell**: First matching pattern (sequential)
- **MeTTa**: All matching rules (parallel)

---

## Multiple Matching Rules

### Non-Deterministic Reduction

When multiple rules match, **all** are explored.

**Formal Rule**:
```
         e matches patterns p₁, ..., pₙ
         e →ᵢ eᵢ  for each rule i
(MULTI)  ──────────────────────────────
         e ⇓ {e₁, ..., eₙ}
```

**Key Property**: Non-determinism is a **feature**, not a bug.

### Confluence

**Question**: If an expression can reduce in multiple ways, do all paths lead to the same result?

**Definition**: A reduction system is **confluent** if:
```
∀ e, e₁, e₂: (e →* e₁ ∧ e →* e₂) ⟹ ∃ e': (e₁ →* e' ∧ e₂ →* e')
```

**Church-Rosser Property**: For confluent systems, normal forms (if they exist) are unique.

**MeTTa**: Generally confluent for pure computation, **not confluent** with side effects.

### Deterministic vs Non-Deterministic Rules

**Deterministic**: Single result

```metta
(= (successor $n) (+ $n 1))

!(successor 5)  ; Always produces 6
```

**Non-Deterministic**: Multiple results

```metta
(= (choose) A)
(= (choose) B)

!(choose)  ; Produces {A, B}
```

### Ambiguous Rules

**Example**:
```metta
(= (ambiguous $x $x) same)
(= (ambiguous $x $y) different)

!(ambiguous A A)
```

**Reduction**:
```
(ambiguous A A)
→ Matches rule 1: {$x ↦ A} → same
→ Matches rule 2: {$x ↦ A, $y ↦ A} → different

Result: {same, different}  (both branches)
```

---

## Reduction Strategies

### Evaluation Strategies

Different strategies for when and where to apply reductions:

1. **Normal Order**: Reduce leftmost outermost redex first
2. **Applicative Order**: Reduce arguments before function application
3. **Lazy Evaluation**: Reduce only when value is needed
4. **Eager Evaluation**: Reduce as soon as possible

### MeTTa's Strategy

From `hyperon-experimental/docs/minimal-metta.md`:44-51:

> "**Minimal MeTTa uses the fixed normal evaluation order**, arguments are passed to the function without evaluation."

**Key Points**:
- **Normal order** by default
- Arguments are **not** reduced before function application
- Use `chain` or similar to force evaluation

### Reduction Location

**Question**: Where in an expression should reduction occur?

**Possible Locations**:
- **Outermost**: `(f (g x))` → reduce `f` first
- **Innermost**: `(f (g x))` → reduce `g` first
- **Parallel**: Reduce all redexes simultaneously

**MeTTa**: Non-deterministic - all alternatives explored.

### Chain for Sequencing

The `chain` operation forces evaluation order:

**Syntax**:
```metta
(chain <expr> <var> <body>)
```

**Semantics**:
1. Reduce `<expr>` to normal form
2. Bind result to `<var>`
3. Reduce `<body>` with binding

**Example**:
```metta
; Without chain: arguments not reduced
(= (my-func $x) (print $x))

!(my-func (+ 1 2))
; Prints: (+ 1 2)

; With chain: argument reduced first
!(chain (+ 1 2) $result (my-func $result))
; Prints: 3
```

---

## Implementation Details

### Rule Storage

Reduction rules are stored as atoms in the space:

```metta
(= <pattern> <result>)
```

These are regular atoms, queryable like any other.

### Rule Lookup

When reducing expression `e`:

1. Create query: `(= e $X)` where `$X` is fresh variable
2. Query space for all matching atoms
3. Extract `$X` bindings (these are the reduction results)
4. Create alternative branch for each result

**Code Reference**: `interpreter.rs`:604-638 (`query` function)

### Reduction Implementation

From `hyperon-experimental/lib/src/metta/interpreter.rs`:

The interpreter doesn't have a separate "reduce" function. Reduction happens via:
1. Query for `(= <expr> $X)`
2. Each match provides a reduction alternative

**Key Insight**: Reduction is **pattern matching against equality rules**.

### Built-in Operations

Built-in operations (like `+`, `-`, `*`, `/`) are implemented as `CustomExecute`:

```rust
impl CustomExecute for AddOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        // ... implementation ...
    }
}
```

**Execution**: Direct computation, not rule-based reduction.

### Rule Ordering in Space

**Question**: Does the order of rule definitions in the space matter?

**Answer**: **No** - all matching rules are applied non-deterministically.

**Example**:
```metta
; Order of definition
(= (test) first)
(= (test) second)

!(test)
; Results: {first, second} (order unspecified)
```

Flipping definition order:
```metta
(= (test) second)
(= (test) first)

!(test)
; Results: {first, second} or {second, first} (unspecified)
```

---

## Examples

### Example 1: Single Reduction

**Rules**:
```metta
(= (square $x) (* $x $x))
```

**Evaluation**:
```metta
!(square 5)
```

**Reduction Steps**:
```
(square 5)
→ Match (square $x) with {$x ↦ 5}
→ Substitute: (* 5 5)
→ Built-in reduction: 25

Result: 25
```

### Example 2: Multiple Reductions

**Rules**:
```metta
(= (choose) A)
(= (choose) B)
(= (choose) C)
```

**Evaluation**:
```metta
!(choose)
```

**Reduction**:
```
(choose)
→ Query: (= (choose) $X)
→ Matches: A, B, C

Alternatives:
  Branch 1: A
  Branch 2: B
  Branch 3: C

Result: {A, B, C}
```

### Example 3: Recursive Reduction

**Rules**:
```metta
(= (sum-to 0) 0)
(= (sum-to $n) (+ $n (sum-to (- $n 1))))
```

**Evaluation**:
```metta
!(sum-to 3)
```

**Reduction Steps**:
```
(sum-to 3)
→ Match second rule: {$n ↦ 3}
→ (+ 3 (sum-to (- 3 1)))
→ (+ 3 (sum-to 2))
→ (+ 3 (+ 2 (sum-to 1)))
→ (+ 3 (+ 2 (+ 1 (sum-to 0))))
→ (+ 3 (+ 2 (+ 1 0)))
→ (+ 3 (+ 2 1))
→ (+ 3 3)
→ 6

Result: 6
```

### Example 4: Non-Confluent Reduction

**Rules**:
```metta
(= (mutate) (add-atom &space A))
(= (mutate) (add-atom &space B))
```

**Evaluation**:
```metta
!(mutate)
```

**Reduction** (order-dependent):

**Scenario 1** (A first):
```
Branch 1: (add-atom &space A) → () with side effect: Space = {A}
Branch 2: (add-atom &space B) → () with side effect: Space = {A, B}

Final Space: {A, B}
```

**Scenario 2** (B first):
```
Branch 1: (add-atom &space B) → () with side effect: Space = {B}
Branch 2: (add-atom &space A) → () with side effect: Space = {A, B}

Final Space: {A, B}  (same in this case, but intermediate states differ)
```

**Non-Confluent**: If rules have conditionals based on space state, different orders yield different results.

### Example 5: Conditional Reduction

**Rules**:
```metta
(= (safe-div $x 0) infinity)
(= (safe-div $x $y) (/ $x $y))
```

**Evaluation 1**:
```metta
!(safe-div 10 2)
```

**Reduction**:
```
(safe-div 10 2)
→ Try rule 1: (safe-div $x 0) - no match (2 ≠ 0)
→ Try rule 2: (safe-div $x $y) - match with {$x ↦ 10, $y ↦ 2}
→ (/ 10 2)
→ 5

Result: 5
```

**Evaluation 2**:
```metta
!(safe-div 10 0)
```

**Reduction**:
```
(safe-div 10 0)
→ Try rule 1: (safe-div $x 0) - match with {$x ↦ 10}
→ infinity
→ Try rule 2: (safe-div $x $y) - match with {$x ↦ 10, $y ↦ 0}
→ (/ 10 0) - may error or return ∞

Result: {infinity, <error or ∞>}  (both branches if both match)
```

### Example 6: Overlapping Patterns

**Rules**:
```metta
(= (classify 0) zero)
(= (classify $n) non-zero)
```

**Evaluation**:
```metta
!(classify 0)
```

**Reduction**:
```
(classify 0)
→ Rule 1: (classify 0) - exact match → zero
→ Rule 2: (classify $n) - match with {$n ↦ 0} → non-zero

Result: {zero, non-zero}  (both branches)
```

**Note**: Unlike Prolog or Haskell, **both** rules fire.

### Example 7: Chain for Deterministic Reduction

**Rules**:
```metta
(= (report $x) (print $x))
```

**Without Chain**:
```metta
; (color) reduces to {red, green, blue}
; report sees unevaluated (color)
!(report (color))
; Prints: (color)
```

**With Chain**:
```metta
; Force evaluation of (color) first
!(chain (color) $c (report $c))
; Prints: red (in one branch)
; Prints: green (in another branch)
; Prints: blue (in third branch)
```

---

## Specification vs Implementation

| Aspect | Specification | Implementation |
|--------|--------------|----------------|
| **Rule Priority** | None (all rules equal) | All matching rules applied |
| **Determinism** | Non-deterministic | Non-deterministic via branches |
| **Built-in Priority** | Not specified | Likely checked before user rules |
| **Rule Order** | Irrelevant | Irrelevant (all matches collected) |
| **Confluence** | Not guaranteed | Confluent for pure, not for side effects |
| **Evaluation Strategy** | Normal order | Normal order |
| **Termination** | Not guaranteed | Not guaranteed (may loop) |

---

## Theoretical Properties

### Confluence (Pure Reductions)

**Theorem**: For MeTTa programs without side effects, reduction is confluent.

**Proof Sketch**:
1. All reduction rules are equality-based
2. Pattern matching is deterministic (same inputs → same matches)
3. Substitution is deterministic
4. No shared mutable state
5. Therefore, all reduction paths explore the same semantic space

**Counterexample (with side effects)**:
```metta
(= (test) (add-atom &space A))
(= (test) (if (empty? &space) B C))
```

Different evaluation orders can produce different results.

### Termination

**Problem**: Determining if reduction terminates is undecidable (Halting Problem).

**Example Non-Terminating Reduction**:
```metta
(= (loop) (loop))

!(loop)  ; Infinite reduction
```

**Practical Approach**: Set maximum recursion depth.

### Completeness

**Question**: Can all computable functions be expressed with MeTTa reduction rules?

**Answer**: Yes, MeTTa is Turing-complete.

**Proof**: Can encode λ-calculus or Turing machines.

---

## Design Recommendations

For MeTTa compiler implementers:

### Rule Priorities

**Consider**:
1. **User-Specified Priorities**: Allow explicit rule ordering
2. **Specificity-Based Priorities**: More specific patterns match first
3. **Mode Annotations**: Deterministic vs non-deterministic rules

**Example API**:
```metta
; Priority annotation
(= (test) first :priority 1)
(= (test) second :priority 2)

; Deterministic mode
(=! (test) single-result)  ; Only one result
```

### Performance Optimizations

**Consider**:
1. **Rule Indexing**: Index rules by head symbol for fast lookup
2. **Memoization**: Cache reduction results
3. **Partial Evaluation**: Specialize rules at compile-time

### Debugging Support

**Provide**:
1. **Reduction Traces**: Show step-by-step reduction
2. **Rule Provenance**: Which rule was applied
3. **Branch Visualization**: Visualize non-deterministic branches

**Example**:
```metta
!(trace (factorial 3))
; Output:
; Step 1: (factorial 3) → (* 3 (factorial 2)) via rule at line 5
; Step 2: (factorial 2) → (* 2 (factorial 1)) via rule at line 5
; ...
```

### Termination Checking

**Provide**:
1. **Termination Analysis**: Warn about potentially non-terminating rules
2. **Depth Limits**: Configurable maximum reduction depth
3. **Cycle Detection**: Detect and break reduction cycles

---

## References

### Source Code

- **`hyperon-experimental/lib/src/metta/interpreter.rs`**
  - Query function (lines 604-638): Rule lookup via pattern matching

- **`hyperon-experimental/docs/minimal-metta.md`**
  - Lines 44-51: Evaluation order discussion
  - Lines 21-26: Non-deterministic evaluation

### Academic References

- **Church, A. & Rosser, J. B.** (1936). "Some Properties of Conversion". *Transactions of the AMS*.
- **Baader, F. & Nipkow, T.** (1998). *Term Rewriting and All That*. Cambridge University Press.
- **Terese** (2003). *Term Rewriting Systems*. Cambridge University Press.
- **Klop, J. W.** (1992). "Term Rewriting Systems". *Handbook of Logic in Computer Science*.

---

## See Also

- **§01**: Evaluation order (when reductions occur)
- **§03**: Pattern matching (how rules are matched)
- **§05**: Non-determinism (multiple reduction alternatives)
- **§07**: Formal proofs (confluence properties)

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
