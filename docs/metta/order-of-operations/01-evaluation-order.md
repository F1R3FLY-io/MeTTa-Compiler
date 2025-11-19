# S-Expression Evaluation Order

## Abstract

This document provides a rigorous specification of the evaluation order for MeTTa s-expressions, including both the formal semantics and the implementation details from hyperon-experimental.

## Table of Contents

1. [Formal Semantics](#formal-semantics)
2. [Normal vs Applicative Order](#normal-vs-applicative-order)
3. [Plan-Based Evaluation](#plan-based-evaluation)
4. [Evaluation Steps](#evaluation-steps)
5. [Implementation Details](#implementation-details)
6. [Examples](#examples)

---

## Formal Semantics

### Notation

We use the following notation throughout this document:

- **Atom**: `a, b, c` - atomic values (symbols, numbers, variables)
- **Expression**: `e ::= a | (e₁ e₂ ... eₙ)` - atom or list of expressions
- **Variable**: `$x, $y` - variables that can be bound
- **Bindings**: `β ⊢ e` - expression `e` under bindings `β`
- **Evaluation relation**: `e ⇓ v` - expression `e` evaluates to value `v`
- **Reduction relation**: `e → e'` - expression `e` reduces to `e'` in one step
- **Multi-step reduction**: `e →* v` - expression `e` reduces to `v` in zero or more steps

### Evaluation Judgment

The evaluation judgment has the form:

```
Γ; β; s ⊢ e ⇓ {(v₁, β₁), (v₂, β₂), ..., (vₙ, βₙ)}
```

Where:
- `Γ` is the evaluation context (space, environment)
- `β` is the current variable bindings
- `s` is the evaluation stack (continuation)
- `e` is the expression to evaluate
- `{(vᵢ, βᵢ)}` is the set of result-bindings pairs (non-deterministic results)

**Key Property**: The result is a **set** of pairs, reflecting the non-deterministic nature of MeTTa evaluation.

---

## Normal vs Applicative Order

### Definition: Evaluation Order

An evaluation strategy determines **when** and **in what order** subexpressions are evaluated.

#### Applicative Order (Call-by-Value)

In applicative order:
1. Evaluate all arguments first
2. Then apply the function to the evaluated arguments

**Formal Rule**:
```
         e₁ ⇓ f    e₂ ⇓ v₂   ...   eₙ ⇓ vₙ    f(v₂, ..., vₙ) ⇓ v
(APP)    ──────────────────────────────────────────────────────────
                        (e₁ e₂ ... eₙ) ⇓ v
```

**Example** (Lisp-style):
```lisp
(+ (* 2 3) (* 4 5))
→ (* 2 3) ⇓ 6
→ (* 4 5) ⇓ 20
→ (+ 6 20) ⇓ 26
```

#### Normal Order (Call-by-Name)

In normal order:
1. Arguments are **not** evaluated before function application
2. Arguments are passed unevaluated and substituted into the function body
3. Evaluation occurs only when needed

**Formal Rule**:
```
         e₁ ⇓ f    f(e₂, ..., eₙ) ⇓ v
(NORM)   ───────────────────────────────
            (e₁ e₂ ... eₙ) ⇓ v
```

**Example** (Haskell-style):
```haskell
let const x y = x in
  const 42 (error "never evaluated")
→ const(42, error "never evaluated")
→ 42  -- y is never evaluated
```

### MeTTa's Evaluation Strategy

#### Specification

From `hyperon-experimental/docs/minimal-metta.md`:44-51:

> "MeTTa implements the applicative evaluation order by default, arguments are
> evaluated before they are passed to the function. User can change this order
> using special meta-types as the types of the arguments. **Minimal MeTTa
> operations don't rely on types and minimal MeTTa uses the fixed normal
> evaluation order**, arguments are passed to the function without evaluation. But
> there is a [chain](#chain) operation which can be used to evaluate an argument
> before passing it. Thus `chain` can be used to change evaluation order in MeTTa
> interpreter."

#### Key Points

1. **Minimal MeTTa**: Normal order (unevaluated arguments)
2. **Full MeTTa**: Applicative order by default, with type-based overrides
3. **Explicit Control**: Use `chain` or similar constructs to force evaluation

### Forcing Evaluation with Chain

The `chain` operation allows explicit control over evaluation order:

**Syntax**:
```metta
(chain <expr> <var> <body>)
```

**Semantics**:
1. Evaluate `<expr>` to value `v`
2. Bind `v` to `<var>`
3. Evaluate `<body>` with this binding

**Formal Rule**:
```
         e ⇓ v    β' = β ∪ {x ↦ v}    b[x ↦ v] ⇓ w
(CHAIN)  ────────────────────────────────────────────
              (chain e x b) ⇓ w
```

**Example**:
```metta
; Without chain (normal order)
(my-func (+ 1 2))  ; my-func receives (+ 1 2) unevaluated

; With chain (forced evaluation)
(chain (+ 1 2) $x (my-func $x))  ; my-func receives 3
```

---

## Plan-Based Evaluation

### The Evaluation Plan

MeTTa uses a **plan-based interpreter** where evaluation proceeds by maintaining a collection of alternative evaluation states.

#### Data Structure

From `hyperon-experimental/lib/src/metta/interpreter.rs`:172-183:

```rust
pub struct InterpreterState {
    /// List of the alternatives to evaluate further.
    plan: Vec<InterpretedAtom>,
    /// List of the completely evaluated results to be returned.
    finished: Vec<Atom>,
    /// Evaluation context.
    context: InterpreterContext,
    /// Maximum stack depth
    max_stack_depth: usize,
}
```

**Key Components**:
- `plan`: Vector of alternatives still being evaluated
- `finished`: Vector of completed results
- Each `InterpretedAtom` is a pair: `(Stack, Bindings)`

### Interpretation Plan

From `minimal-metta.md`:21-26:

> "Each step of interpretation inputs and outputs a list of pairs (`<atom>`,
> `<bindings>`) which is called interpretation plan. Each pair in the plan
> represents one possible way of interpreting the original atom or possible
> branch of the evaluation. Interpreter doesn't select one of them for further
> processing. **It continues interpreting all of the branches in parallel**."

**Formal Definition**:

An interpretation plan is a set of pairs:
```
Plan = {(e₁, β₁), (e₂, β₂), ..., (eₙ, βₙ)}
```

Where each `(eᵢ, βᵢ)` represents one possible evaluation branch.

### Non-Deterministic Evaluation

**Specification**: All branches are evaluated **in parallel** (logically).

**Implementation**: Branches are evaluated **sequentially** in LIFO order (stack-based).

**Formal Rule**:
```
         Plan = {(e₁, β₁), ..., (eₙ, βₙ)}
         ∀i: eᵢ ⇓ Planᵢ
         Plan' = Plan₁ ∪ Plan₂ ∪ ... ∪ Planₙ
(PLAN)   ───────────────────────────────────
         Plan ⇓ Plan'
```

**Invariant**: The order in which branches are explored should **not** affect the final set of results (for pure programs).

---

## Evaluation Steps

### Single Step Evaluation

From `interpreter.rs`:269-277:

```rust
pub fn interpret_step(mut state: InterpreterState) -> InterpreterState {
    let interpreted_atom = state.pop().unwrap();  // LIFO pop from plan
    log::debug!("interpret_step:\n{}", interpreted_atom);
    let InterpretedAtom(stack, bindings) = interpreted_atom;
    for result in interpret_stack(&state.context, stack, bindings, state.max_stack_depth) {
        state.push(result);
    }
    state
}
```

**Algorithm**:
1. Pop one `InterpretedAtom` from the plan (LIFO order)
2. Interpret the stack with current bindings
3. Push all resulting alternatives back onto the plan
4. Repeat until plan is empty

### Complete Evaluation

**Algorithm**:
```
function eval(e, β₀):
    plan = {(e, β₀)}
    finished = {}

    while plan is not empty:
        (e, β) = pop(plan)  // LIFO

        if e is fully evaluated:
            finished = finished ∪ {(e, β)}
        else:
            for each (e', β') in step(e, β):
                push(plan, (e', β'))  // New alternatives

    return finished
```

**Termination**: Evaluation terminates when the plan is empty and all results are in the finished set.

**Non-Termination**: Evaluation may not terminate if reduction rules create infinite chains.

---

## Implementation Details

### LIFO Plan Processing

**Implementation Choice**: The current implementation uses **LIFO (Last In, First Out)** ordering.

From `interpreter.rs`:224:
```rust
pub fn pop(&mut self) -> Option<InterpretedAtom> {
    self.plan.pop()
}
```

**Implications**:
- Depth-first exploration of alternatives
- Most recently added branches are explored first
- Can lead to infinite loops if leftmost branch is infinite

**Alternative**: FIFO (First In, First Out) would give breadth-first exploration.

### Stack Depth Limit

From `interpreter.rs`:172-183, the interpreter includes `max_stack_depth` to prevent infinite recursion.

**Default**: Configurable limit (prevents stack overflow)

**Behavior on Limit**: Evaluation stops when depth is exceeded (implementation-specific error handling)

### Evaluation Context

The `InterpreterContext` provides:
- Access to the atom space
- Module system
- Type system
- Built-in operations

### Pure vs Effectful Operations

**Pure Operations**:
- Arithmetic: `+`, `-`, `*`, `/`
- Logical: `and`, `or`, `not`
- Structural: `car`, `cdr`, `cons`

**Effectful Operations**:
- Space mutations: `add-atom`, `remove-atom`
- I/O: `print`, `println`
- Queries: Pattern matching against the space

**Key Difference**: Pure operations are confluent; effectful operations may not be.

---

## Examples

### Example 1: Normal Order Evaluation

**MeTTa Program**:
```metta
; Define a function that ignores its second argument
(= (first $x $y) $x)

; Evaluate with an error in the second argument
!(first 42 (/ 1 0))
```

**Evaluation**:
```
!(first 42 (/ 1 0))
→ Apply first with arguments [42, (/ 1 0)] (unevaluated)
→ Match pattern (first $x $y) with bindings {$x ↦ 42, $y ↦ (/ 1 0)}
→ Return $x = 42
```

**Result**: `42` (division by zero never evaluated)

**Comparison with Applicative Order** (hypothetical):
```
!(first 42 (/ 1 0))
→ Evaluate 42 ⇓ 42
→ Evaluate (/ 1 0) ⇓ ERROR!
```

### Example 2: Multiple Alternatives

**MeTTa Program**:
```metta
; Define multiple reduction rules
(= (color) red)
(= (color) green)
(= (color) blue)

; Evaluate
!(color)
```

**Evaluation**:
```
Initial plan: {((color), {})}

Step 1: Query space for (= (color) $X)
  Matches: red, green, blue
  New plan: {(red, {}), (green, {}), (blue, {})}

Step 2-4: Each is fully evaluated
  Finished: {red, green, blue}
```

**Result**: `{red, green, blue}` (all three alternatives)

### Example 3: Chain for Forced Evaluation

**MeTTa Program**:
```metta
; Function that needs evaluated argument
(= (double $x) (* $x 2))

; Without chain (normal order - may not work as expected)
!(double (+ 1 2))  ; Tries to match (* (+ 1 2) 2) - fails if no eval

; With chain (forced evaluation)
!(chain (+ 1 2) $x (double $x))
```

**Evaluation with Chain**:
```
!(chain (+ 1 2) $x (double $x))
→ Evaluate (+ 1 2) ⇓ 3
→ Bind $x ↦ 3
→ Evaluate (double 3) with {$x ↦ 3}
→ Match (double $x) with {$x ↦ 3}
→ Return (* 3 2) ⇓ 6
```

**Result**: `6`

### Example 4: Non-Termination

**MeTTa Program**:
```metta
; Infinite recursion
(= (loop) (loop))

; Evaluate
!(loop)
```

**Evaluation**:
```
Initial plan: {((loop), {})}

Step 1: Query space for (= (loop) $X)
  Match: (loop)
  New plan: {((loop), {})}

Step 2: Query space for (= (loop) $X)
  Match: (loop)
  New plan: {((loop), {})}

... (infinite loop)
```

**Result**: Non-termination (stack depth limit may trigger)

---

## Specification vs Implementation

| Aspect | Specification | Implementation |
|--------|--------------|----------------|
| **Evaluation Order** | Normal order (Minimal MeTTa)<br>Applicative order (Full MeTTa) | Normal order in `interpreter.rs` |
| **Alternative Exploration** | All branches in parallel (logical) | Sequential LIFO processing |
| **Termination** | Not guaranteed | Stack depth limit for safety |
| **Argument Evaluation** | Unevaluated by default | Unevaluated (normal order) |
| **Chain Operation** | Forces evaluation | Implemented via `chain` operation |

---

## Theoretical Properties

### Confluence (Without Side Effects)

**Theorem**: For pure MeTTa programs (no side effects), evaluation is confluent.

**Definition**: A reduction system is confluent if:
```
∀ e, v₁, v₂: (e →* v₁ ∧ e →* v₂) ⟹ ∃ w: (v₁ →* w ∧ v₂ →* w)
```

**Intuition**: No matter which order we explore branches, we reach the same final results.

**Proof Sketch**:
1. Pure operations are deterministic given the same inputs
2. Pattern matching is deterministic (same space ⟹ same matches)
3. Alternative branches are independent (no shared state)
4. Therefore, all branches converge to the same set of results

**See §07** for complete proof.

### Non-Confluence (With Side Effects)

**Counterexample**: With side effects, confluence fails.

```metta
; Add atoms in different orders
(= (test1) (add-atom &space A))
(= (test1) (add-atom &space B))

!(test1)
```

Depending on evaluation order, the space may end up with:
- `{A}` (if first rule fires first)
- `{B}` (if second rule fires first)
- `{A, B}` (if both fire)

**Non-confluent**: Different orderings yield different final states.

---

## References

### Source Code

- **`hyperon-experimental/lib/src/metta/interpreter.rs`**
  - `InterpreterState` (lines 172-183): Plan-based evaluation state
  - `interpret_step()` (lines 269-277): Single evaluation step
  - `interpret_stack()`: Stack interpretation logic

- **`hyperon-experimental/docs/minimal-metta.md`**
  - Lines 21-26: Interpretation plan definition
  - Lines 44-51: Normal vs applicative order discussion

### Academic References

- **Plotkin, G. D.** (1975). "Call-by-name, call-by-value and the λ-calculus". *Theoretical Computer Science*.
- **Barendregt, H. P.** (1984). *The Lambda Calculus: Its Syntax and Semantics*. North-Holland.
- **Felleisen, M. & Friedman, D. P.** (1987). "A Reduction Semantics for Imperative Higher-Order Languages".

---

## See Also

- **§02**: Mutation order (side effects during evaluation)
- **§04**: Reduction order (how reduction rules are applied)
- **§05**: Non-determinism (detailed semantics of alternative exploration)
- **§07**: Formal proofs (confluence, determinism properties)

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
