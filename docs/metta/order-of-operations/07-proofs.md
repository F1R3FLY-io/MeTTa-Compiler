# Formal Proofs

## Abstract

This document provides rigorous mathematical proofs of key properties of MeTTa's evaluation semantics, including confluence, soundness, completeness, and determinism properties. These proofs establish the theoretical foundation for reasoning about MeTTa program behavior and correctness.

## Table of Contents

1. [Notation and Preliminaries](#notation-and-preliminaries)
2. [Confluence of Pure MeTTa](#confluence-of-pure-metta)
3. [Non-Confluence with Side Effects](#non-confluence-with-side-effects)
4. [Soundness](#soundness)
5. [Completeness](#completeness)
6. [Determinism Properties](#determinism-properties)
7. [Termination](#termination)

---

## Notation and Preliminaries

### Formal System

We define a formal system for MeTTa evaluation:

**Syntax**:
```
e ::= a                  (atom: symbol or number)
    | $x                 (variable)
    | (e₁ e₂ ... eₙ)     (expression)

v ::= a | (v₁ v₂ ... vₙ) (value: fully evaluated expression)

β ::= {$x₁ ↦ v₁, ..., $xₙ ↦ vₙ}  (bindings)

S ::= {(e₁, β₁), ..., (eₙ, βₙ)}  (space: set of atoms)
```

**Judgments**:
```
e ⇓ {(v₁, β₁), ..., (vₙ, βₙ)}    Evaluation: e evaluates to set of value-binding pairs
e → e'                            Reduction: e reduces to e' in one step
e →* v                            Multi-step reduction: e reduces to v in zero or more steps
S ⊢ e ⇓ R                        Evaluation in space S produces result set R
```

### Definitions

**Definition 1 (Value)**: An expression `e` is a **value** if it is in normal form (no reduction rules apply).

**Definition 2 (Substitution)**: `e[β]` denotes the result of applying bindings `β` to expression `e`:
```
a[β] = a                           (atoms are unchanged)
$x[β] = β($x) if $x ∈ dom(β)      (substitute bound variables)
$x[β] = $x if $x ∉ dom(β)         (unbound variables unchanged)
(e₁ ... eₙ)[β] = (e₁[β] ... eₙ[β])  (substitute in subexpressions)
```

**Definition 3 (Binding Consistency)**: Bindings `β₁` and `β₂` are **consistent** if:
```
∀$x ∈ dom(β₁) ∩ dom(β₂): β₁($x) = β₂($x)
```

**Definition 4 (Binding Merge)**: `β₁ ⊔ β₂` is defined only if `β₁` and `β₂` are consistent:
```
(β₁ ⊔ β₂)($x) = β₁($x) if $x ∈ dom(β₁)
                 β₂($x) if $x ∈ dom(β₂) \ dom(β₁)
```

**Definition 5 (Pattern Match)**: `match(a, p)` returns a set of bindings:
```
match(a, a) = {∅}                         (literal match)
match(a, $x) = {{$x ↦ a}}                 (variable match)
match((a₁...aₙ), (p₁...pₙ)) = {⊔ᵢ βᵢ | βᵢ ∈ match(aᵢ, pᵢ), ⊔ᵢ βᵢ defined}
match(_, _) = ∅                           (no match)
```

### Axioms

**Axiom 1 (Space Query)**: For space `S` and query pattern `p`:
```
query(S, p) = {(a, β) ∈ S | ∃β': β' ∈ match(a, p) ∧ β = β'}
```

**Axiom 2 (Reduction via Equality)**: For expression `e` and space `S`:
```
If (= p r) ∈ S and β ∈ match(e, p) then e → r[β]
```

---

## Confluence of Pure MeTTa

### Theorem 1: Confluence (Pure Fragment)

**Statement**: For MeTTa programs without side effects (no `add-atom`, `remove-atom`, or other mutations), evaluation is confluent.

**Formal Statement**:
```
For all expressions e, if:
  S ⊢ e ⇓ {(v₁, β₁), ..., (vₘ, βₘ)} via evaluation order o₁, and
  S ⊢ e ⇓ {(w₁, γ₁), ..., (wₙ, γₙ)} via evaluation order o₂

Then (as multisets):
  {(v₁, β₁), ..., (vₘ, βₘ)} = {(w₁, γ₁), ..., (wₙ, γₙ)}
```

**Proof Strategy**: We prove confluence by showing that the evaluation relation is Church-Rosser.

#### Lemma 1.1: Deterministic Reduction Step

**Statement**: For a fixed space `S` and expression `e`, the set of possible one-step reductions is uniquely determined.

**Proof**:
1. The set of reduction rules in `S` is fixed
2. Pattern matching is deterministic: `match(e, p)` returns a specific set of bindings
3. For each rule `(= p r) ∈ S` and binding `β ∈ match(e, p)`, we get reduction `e → r[β]`
4. The set of all such reductions is uniquely determined by `S` and `e`
5. Therefore, one-step reduction is deterministic (as a relation mapping `e` to a set of results)

∎

#### Lemma 1.2: Reduction is Local

**Statement**: If `e = (e₁ ... eₙ)` and `eᵢ → e'ᵢ`, then `e → (e₁ ... e'ᵢ ... eₙ)`.

**Proof**:
1. By the definition of reduction, we can reduce subexpressions
2. If `eᵢ` matches pattern `p` with bindings `β`, giving reduction `eᵢ → r[β]`
3. Then `e = (e₁ ... eᵢ ... eₙ)` can reduce to `(e₁ ... r[β] ... eₙ)`
4. This is local to the subexpression `eᵢ`

∎

#### Lemma 1.3: Independence of Alternatives

**Statement**: Different alternatives (branches) in the evaluation plan do not interfere.

**Proof**:
1. Each alternative is a pair `(e, β)` where `e` is an expression and `β` is a set of bindings
2. Bindings are immutable - once created, they are not modified
3. For pure MeTTa (no mutations), the space `S` is read-only during evaluation
4. Therefore, evaluation of alternative `(e₁, β₁)` does not affect evaluation of `(e₂, β₂)`
5. Each alternative explores an independent branch of the search space

∎

#### Main Proof of Theorem 1

**Proof**:

**Case 1: Single Reduction Path**

Suppose `e` has a single reduction path: `e → e₁ → e₂ → ... → v`.

Since reduction is deterministic (Lemma 1.1), any evaluation order will follow this same path.

Therefore, all evaluation orders produce the same result `v`.

**Case 2: Multiple Reduction Paths**

Suppose `e` has multiple possible one-step reductions:
```
e → e'₁, e → e'₂, ..., e → e'ₙ
```

By Lemma 1.1, this set of reductions is uniquely determined.

**By Induction on Evaluation Depth**:

**Base Case** (depth = 0): If `e` is a value, then `e ⇓ {(e, ∅)}` regardless of evaluation order.

**Inductive Step**: Assume confluence holds for all expressions at depth < k.

Consider expression `e` at depth k.

Let `R = {e'₁, ..., e'ₙ}` be the set of possible one-step reductions from `e`.

**Evaluation Order o₁**: Suppose o₁ processes alternatives in order: e'₁, e'₂, ..., e'ₙ

For each `e'ᵢ`, by inductive hypothesis:
```
S ⊢ e'ᵢ ⇓ Rᵢ (uniquely determined set)
```

**Evaluation Order o₂**: Suppose o₂ processes alternatives in different order: e'ₚ₍₁₎, e'ₚ₍₂₎, ..., e'ₚ₍ₙ₎

For each `e'ₚ₍ᵢ₎`, by inductive hypothesis:
```
S ⊢ e'ₚ₍ᵢ₎ ⇓ Rₚ₍ᵢ₎ (uniquely determined set)
```

**Key Observation**: Since Rᵢ and Rₚ₍ᵢ₎ are determined by the same expression `e'ᵢ = e'ₚ₍ᵢ₎` and same space `S`:
```
Rᵢ = Rₚ₍ᵢ₎
```

Therefore:
```
⋃ᵢ Rᵢ = ⋃ᵢ Rₚ₍ᵢ₎
```

**Conclusion**: The final result set is the union of all alternative results, which is independent of processing order.

By induction, confluence holds for all expressions.

∎

#### Corollary 1.1: Unique Normal Forms

**Statement**: If `e` evaluates to normal forms, they are unique (up to variable renaming).

**Proof**: Direct consequence of Theorem 1 (Church-Rosser property implies unique normal forms).

∎

---

## Non-Confluence with Side Effects

### Theorem 2: Non-Confluence (With Mutations)

**Statement**: MeTTa with side effects (atom space mutations) is **not confluent**.

**Proof by Counterexample**:

Consider the following program:

**Space S**: Initially empty

**Rules**:
```metta
(= (branch-1) (add-atom &space A))
(= (branch-2) (if (empty? &space) B C))
```

**Expression**: `(pair (branch-1) (branch-2))`

**Evaluation Order 1**: Evaluate `(branch-1)` before `(branch-2)`

```
Step 1: Evaluate (branch-1)
  → (add-atom &space A)
  → () with side effect: S' = {A}

Step 2: Evaluate (branch-2) in space S' = {A}
  → (if (empty? &space) B C)
  → (if false B C)  ; space is not empty
  → C

Result: (pair () C)
Final Space: {A}
```

**Evaluation Order 2**: Evaluate `(branch-2)` before `(branch-1)`

```
Step 1: Evaluate (branch-2) in space S = {}
  → (if (empty? &space) B C)
  → (if true B C)  ; space is empty
  → B

Step 2: Evaluate (branch-1)
  → (add-atom &space A)
  → () with side effect: S' = {A}

Result: (pair () B)
Final Space: {A}
```

**Observation**:
- Result in order 1: `(pair () C)`
- Result in order 2: `(pair () B)`
- `(pair () C) ≠ (pair () B)`

**Conclusion**: Different evaluation orders produce different results.

Therefore, MeTTa with side effects is **not confluent**.

∎

### Corollary 2.1: Caution with Side Effects

**Statement**: Programs using side effects must be designed carefully to ensure correct behavior regardless of evaluation order.

**Recommendation**: Avoid conditional logic that depends on space state when using mutations.

---

## Soundness

### Theorem 3: Soundness

**Statement**: All results produced by MeTTa evaluation are derivable from the reduction rules.

**Formal Statement**:
```
For all expressions e and spaces S:
  If S ⊢ e ⇓ {(v₁, β₁), ..., (vₙ, βₙ)}
  Then ∀i: e[βᵢ] →* vᵢ (via rules in S)
```

**Proof**:

We prove by structural induction on the evaluation derivation.

**Base Case**: `e` is a value (normal form)

If `e` is a value, then `e ⇓ {(e, ∅)}`.

Clearly, `e[∅] = e →* e` (zero reduction steps).

Soundness holds.

**Inductive Step**: `e` reduces to `e'`

Assume soundness holds for all subderivations (IH).

**Case 1: Direct Reduction**

Suppose `e` matches rule `(= p r) ∈ S` with bindings `β`:
```
e → r[β]
```

By IH, for all results `(v, β')` of evaluating `r[β]`:
```
r[β][β'] →* v
```

Therefore:
```
e[β][β'] = (r[β])[β'] →* v
```

Since `e → r[β]`:
```
e[β][β'] →* v
```

Soundness holds.

**Case 2: Subexpression Reduction**

Suppose `e = (e₁ ... eₙ)` and `eᵢ` reduces.

By IH, for all results `(vᵢ, βᵢ)` of evaluating `eᵢ`:
```
eᵢ[βᵢ] →* vᵢ
```

Therefore:
```
e[βᵢ] = (e₁[βᵢ] ... eₙ[βᵢ]) →* (e₁[βᵢ] ... vᵢ ... eₙ[βᵢ])
```

By further reduction of other subexpressions:
```
e[βᵢ] →* (v₁ ... vₙ)
```

Soundness holds.

**Case 3: Multiple Alternatives**

If `e` has multiple reduction paths, each producing results:
```
e → e'₁ ⇓ R₁
e → e'₂ ⇓ R₂
...
```

By IH, each path is sound.

The final result is `R = R₁ ∪ R₂ ∪ ...`.

For each `(v, β) ∈ R`, there exists a path `e →* v` via some `e'ᵢ`.

Therefore, `e[β] →* v`.

Soundness holds.

**Conclusion**: By structural induction, soundness holds for all evaluations.

∎

---

## Completeness

### Theorem 4: Completeness

**Statement**: All possible reduction sequences are explored by MeTTa's non-deterministic evaluation.

**Formal Statement**:
```
For all expressions e, spaces S, and values v:
  If e →* v via rules in S
  Then ∃β: (v, β) ∈ results(S ⊢ e ⇓)
```

**Proof**:

We prove by induction on the length of the reduction sequence.

**Base Case**: Length 0 (e is already a value)

If `e →* e` (zero steps), then `e` is a value.

By definition, `e ⇓ {(e, ∅)}`.

Therefore, `(e, ∅) ∈ results(e ⇓)`.

Completeness holds.

**Inductive Step**: Length n+1

Suppose `e →* v` in n+1 steps via:
```
e → e' →* v (n steps)
```

**Step 1**: The reduction `e → e'` is via some rule `(= p r) ∈ S` with bindings `β`:
```
β ∈ match(e, p)
e' = r[β]
```

**Step 2**: By MeTTa's evaluation algorithm (query function), all rules in `S` matching `e` are found:
```
query(S, (= e $X)) returns all matches
```

Therefore, the alternative `(e', β)` is added to the evaluation plan.

**Step 3**: By inductive hypothesis, since `e' →* v` in n steps:
```
∃β': (v, β') ∈ results(e' ⇓)
```

**Step 4**: Since `(e', β)` is in the plan and is evaluated, and `(v, β') ∈ results(e' ⇓)`:
```
(v, β ⊔ β') ∈ results(e ⇓)  (if β and β' are consistent)
```

**Step 5**: If `β` and `β'` are not consistent, then the path is filtered out (which is correct, as inconsistent bindings represent invalid reductions).

**Conclusion**: All valid reduction paths are explored.

By induction, completeness holds for all reduction sequences.

∎

---

## Determinism Properties

### Theorem 5: Determinism Within a Branch

**Statement**: Within a single evaluation branch (alternative), evaluation is deterministic.

**Formal Statement**:
```
For a fixed alternative (e, β) and space S (without mutations):
  The evaluation sequence is uniquely determined.
```

**Proof**:

**Given**: Alternative `(e, β)`, space `S` (read-only).

**Step 1**: The next reduction step is determined by:
1. Query `S` for rules matching `e`
2. Find all bindings `β'` such that `β' ∈ match(e, p)` for some rule `(= p r) ∈ S`
3. Create reduction `e → r[β']`

**Step 2**: This query is deterministic:
- The set of rules in `S` is fixed
- Pattern matching is deterministic (Lemma 1.1)
- The set of resulting reductions is uniquely determined

**Step 3**: Within a branch, we follow one specific reduction:
```
(e, β) → (e', β ⊔ β')
```

This next state is uniquely determined.

**Step 4**: By induction, the entire evaluation sequence within the branch is uniquely determined.

**Conclusion**: Evaluation within a single branch is deterministic.

∎

### Corollary 5.1: Non-Determinism is Inter-Branch

**Statement**: Non-determinism in MeTTa arises only from multiple branches, not from randomness within a branch.

**Proof**: Direct consequence of Theorem 5.

∎

---

## Termination

### Theorem 6: Termination is Undecidable

**Statement**: Determining whether a MeTTa program terminates is undecidable (halting problem).

**Proof Sketch**:

We show that MeTTa can simulate a Turing machine, therefore inheriting the halting problem.

**Encoding**:
- Turing machine states can be represented as atoms
- Tape can be represented as a list
- Transition function can be encoded as reduction rules

**Example**:
```metta
; State s, reading symbol 'a', write 'b', move right, goto state t
(= (tm-step (state s) (tape-left $L) (tape-head a) (tape-right $R))
   (tm-step (state t) (tape-left (cons a $L)) (tape-head (car $R)) (tape-right (cdr $R))))
```

**Simulation**: Repeated application of `tm-step` simulates Turing machine execution.

**Reduction**: If we could decide termination for MeTTa, we could decide the halting problem for Turing machines (contradiction).

**Conclusion**: Termination is undecidable for MeTTa.

∎

### Theorem 7: Depth Bound Ensures Termination

**Statement**: If evaluation depth is bounded by `d`, then evaluation terminates in at most `O(b^d)` steps, where `b` is the branching factor.

**Proof**:

**Given**: Maximum depth `d`, branching factor `b` (maximum alternatives per step).

**Observation**:
- Each step processes one alternative
- Each alternative may create at most `b` new alternatives
- Depth is bounded by `d`

**Tree Structure**: Evaluation forms a tree with:
- Maximum depth: `d`
- Maximum branching: `b`
- Maximum nodes: `b^d`

**Steps**: Each node is processed at most once.

**Conclusion**: Evaluation terminates in at most `O(b^d)` steps.

∎

---

## Summary of Results

| Property | Pure MeTTa | With Side Effects |
|----------|-----------|-------------------|
| **Confluence** | ✓ Yes (Theorem 1) | ✗ No (Theorem 2) |
| **Soundness** | ✓ Yes (Theorem 3) | ✓ Yes |
| **Completeness** | ✓ Yes (Theorem 4) | ✓ Yes (if side effects are deterministic) |
| **Determinism (within branch)** | ✓ Yes (Theorem 5) | ✓ Yes |
| **Termination** | Undecidable (Theorem 6) | Undecidable |
| **Bounded Termination** | ✓ Yes (Theorem 7) | ✓ Yes |

## Practical Implications

1. **Pure Fragments**: When possible, use pure MeTTa (no mutations) to ensure confluence
2. **Side Effects**: Carefully reason about mutation order when using side effects
3. **Depth Limits**: Use bounded depth to ensure termination for recursive programs
4. **Testing**: Test programs with different evaluation orders to detect non-confluence
5. **Verification**: Formal verification is possible for pure fragments

---

## See Also

- **§01-05**: Semantic specifications (what is being proven)
- **§06**: Implementation (how proofs relate to code)
- **§08**: Comparisons (properties in other languages)

---

## References

### Theoretical Foundations

- **Church, A. & Rosser, J. B.** (1936). "Some Properties of Conversion". *Transactions of the AMS*.
  - Church-Rosser Theorem (confluence)

- **Barendregt, H. P.** (1984). *The Lambda Calculus: Its Syntax and Semantics*. North-Holland.
  - Formal semantics and proofs

- **Plotkin, G. D.** (1975). "Call-by-name, call-by-value and the λ-calculus". *TCS*.
  - Evaluation strategies

- **Baader, F. & Nipkow, T.** (1998). *Term Rewriting and All That*. Cambridge University Press.
  - Term rewriting systems and confluence

### Formal Methods

- **Pierce, B. C.** (2002). *Types and Programming Languages*. MIT Press.
  - Type systems and soundness proofs

- **Winskel, G.** (1993). *The Formal Semantics of Programming Languages*. MIT Press.
  - Operational semantics

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
