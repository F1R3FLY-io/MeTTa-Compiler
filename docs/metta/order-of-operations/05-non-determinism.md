# Non-Deterministic Semantics

## Abstract

This document provides a rigorous specification of non-deterministic evaluation in MeTTa. Non-determinism is a core feature of MeTTa that distinguishes it from most other programming languages. Understanding how alternatives are created, managed, and resolved is essential for writing correct MeTTa programs.

## Table of Contents

1. [Non-Determinism Fundamentals](#non-determinism-fundamentals)
2. [Superpose and Collapse](#superpose-and-collapse)
3. [Alternative Management](#alternative-management)
4. [Deterministic vs Non-Deterministic](#deterministic-vs-non-deterministic)
5. [Combining Alternatives](#combining-alternatives)
6. [Implementation Details](#implementation-details)
7. [Examples](#examples)

---

## Non-Determinism Fundamentals

### Definition

**Non-deterministic computation** explores multiple possible execution paths simultaneously, producing multiple results.

**Contrast with Deterministic**:
- **Deterministic**: `f(x)` always produces the same single result
- **Non-Deterministic**: `f(x)` may produce a **set** of results

### Formal Semantics

The evaluation judgment for MeTTa produces a **set** of results:

```
e ⇓ {v₁, v₂, ..., vₙ}
```

Where each `vᵢ` is a possible result of evaluating `e`.

**Key Property**: The result is a **set**, not a single value.

### Sources of Non-Determinism

In MeTTa, non-determinism arises from:

1. **Multiple Reduction Rules**:
   ```metta
   (= (color) red)
   (= (color) green)
   !(color)  ; → {red, green}
   ```

2. **Pattern Matching**:
   ```metta
   ; Space contains: (item A), (item B)
   !(match &space (item $x) $x)  ; → {A, B}
   ```

3. **Explicit Superpose**:
   ```metta
   !(superpose (A B C))  ; → {A, B, C}
   ```

4. **Combinations**:
   ```metta
   !(cons (color) (shape))
   ; If (color) → {red, green} and (shape) → {circle, square}
   ; Result: {(red circle), (red square), (green circle), (green square)}
   ```

### Interpretation Plan

From `hyperon-experimental/docs/minimal-metta.md`:21-26:

> "Each step of interpretation inputs and outputs a list of pairs (`<atom>`,
> `<bindings>`) which is called **interpretation plan**. Each pair in the plan
> represents one possible way of interpreting the original atom or possible
> branch of the evaluation. **Interpreter doesn't select one of them for further
> processing. It continues interpreting all of the branches in parallel.**"

**Key Insight**: All branches are evaluated, not just one.

---

## Superpose and Collapse

### Superpose Operation

**Syntax**:
```metta
(superpose (<atom₁> <atom₂> ... <atomₙ>))
```

**Semantics**: Creates n alternative branches, one for each atom.

**Formal Rule**:
```
(SUPERPOSE)  ─────────────────────────────────────────
             (superpose (a₁ ... aₙ)) ⇓ {a₁, ..., aₙ}
```

**Implementation**: From `hyperon-experimental/lib/src/metta/runner/stdlib/core.rs`:201-222:

```rust
impl CustomExecute for SuperposeOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let arg_error = || ExecError::from("superpose expects single expression as an argument");
        let atom = args.get(0).ok_or_else(arg_error)?;
        let expr  = TryInto::<&ExpressionAtom>::try_into(atom).map_err(|_| arg_error())?;
        Ok(expr.clone().into_children())  // Returns all children as separate results
    }
}
```

**Key**: Returns `Vec<Atom>` containing all children - each becomes an alternative.

### Collapse Operation

**Syntax**:
```metta
(collapse <expr>)
```

**Semantics**: Collects all alternatives from `<expr>` into a single expression.

**Formal Rule**:
```
             e ⇓ {v₁, ..., vₙ}
(COLLAPSE)   ─────────────────────────
             (collapse e) ⇓ (v₁ ... vₙ)
```

**Example**:
```metta
!(collapse (color))
; If (color) → {red, green, blue}
; Result: (red green blue)
```

### Collapse-Bind

**Syntax**:
```metta
(collapse-bind <expr>)
```

**Semantics**: Similar to collapse, but used internally for binding collection.

**Implementation**: From `interpreter.rs`:746-792:

```rust
fn collapse_bind(stack: Stack, bindings: Bindings) -> Vec<InterpretedAtom> {
    // Creates a Stack that will collect all alternatives
    // All alternatives share the same collapse-bind Stack instance via Rc<RefCell<>>
    // When alternatives finish, they modify the shared state
    // ...
}
```

**Key Mechanism**:
- Uses **shared mutable state** (`Rc<RefCell<Stack>>`)
- All alternatives reference the same collector
- Results are accumulated as alternatives complete

---

## Alternative Management

### Alternative Creation

Alternatives are created at various points:

1. **Query Results**:
   ```rust
   let results = space.borrow().query(&query);  // Returns iterator
   for result in results {
       state.push(InterpretedAtom(stack.clone(), result));  // Each is an alternative
   }
   ```

2. **Reduction Matches**:
   ```metta
   ; Multiple matching rules create alternatives
   (= (test) A)
   (= (test) B)
   ```

3. **Superpose**:
   ```rust
   expr.into_children()  // Each child becomes an alternative
   ```

### Alternative Storage

From `interpreter.rs`:172-183:

```rust
pub struct InterpreterState {
    /// List of the alternatives to evaluate further.
    plan: Vec<InterpretedAtom>,
    /// List of the completely evaluated results to be returned.
    finished: Vec<Atom>,
    // ...
}
```

**Two Collections**:
- `plan`: Alternatives still being evaluated (in progress)
- `finished`: Alternatives that have completed evaluation

### Alternative Processing

**Algorithm**:
```
while plan is not empty:
    alt = plan.pop()  // LIFO: last in, first out
    results = evaluate_one_step(alt)
    for result in results:
        if result.is_finished():
            finished.push(result)
        else:
            plan.push(result)

return finished
```

**LIFO Property**: Most recently added alternatives are processed first (depth-first).

### Alternative Termination

An alternative terminates when:
1. Reduced to a value (normal form)
2. No more reduction rules apply
3. Maximum depth reached
4. Error occurs (implementation-specific)

---

## Deterministic vs Non-Deterministic

### Deterministic Operations

**Definition**: Operations that always produce exactly one result.

**Examples**:
```metta
; Arithmetic
!(+ 1 2)  ; → 3 (single result)

; Symbols
!(foo)    ; → foo (if no rules match)

; Sequential evaluation
!(let $x 42 $x)  ; → 42 (single result)
```

**Property**: `|results| = 1`

### Non-Deterministic Operations

**Definition**: Operations that may produce multiple results.

**Examples**:
```metta
; Multiple rules
(= (choice) A)
(= (choice) B)
!(choice)  ; → {A, B}

; Multiple matches
!(match &space (item $x) $x)  ; → {all matching items}

; Superpose
!(superpose (1 2 3))  ; → {1, 2, 3}
```

**Property**: `|results| ≥ 0` (may be zero, one, or many)

### Empty Results

**Question**: What if no alternatives are produced?

**Example**:
```metta
; No matching rules
!(undefined-function)  ; → {} (empty set)

; No matches in space
!(match &space (nonexistent $x) $x)  ; → {} (empty set)
```

**Semantics**: Empty result set (no values)

**Practical Handling**: Implementation may:
- Return empty list
- Return the original expression (no reduction)
- Signal error

---

## Combining Alternatives

### Cartesian Product

When combining non-deterministic expressions:

**Example**:
```metta
(= (color) red)
(= (color) blue)
(= (shape) circle)
(= (shape) square)

!(pair (color) (shape))
```

**Evaluation**:
```
(color) ⇓ {red, blue}
(shape) ⇓ {circle, square}

(pair (color) (shape)) ⇓ {
  (pair red circle),
  (pair red square),
  (pair blue circle),
  (pair blue square)
}
```

**Formal Rule**:
```
         e₁ ⇓ {v₁¹, v₁², ..., v₁ᵐ}
         e₂ ⇓ {v₂¹, v₂², ..., v₂ⁿ}
(CART)   ──────────────────────────────────
         (f e₁ e₂) ⇓ {(f v₁ⁱ v₂ʲ) | i,j}
```

**Result Size**: `m × n` alternatives (Cartesian product)

### Independent vs Dependent Alternatives

**Independent**: Alternatives in different subexpressions combine independently.

```metta
!(cons (choice1) (choice2))
; If choice1 → {A, B} and choice2 → {X, Y}
; Result: {(cons A X), (cons A Y), (cons B X), (cons B Y)}
```

**Dependent**: Alternatives share bindings.

```metta
; Bindings constrain alternatives
!(match &space (edge $x $y) (pair $x $y))
; Each alternative has consistent bindings for $x and $y
```

### Nested Non-Determinism

**Example**:
```metta
(= (outer) (inner))
(= (outer) X)
(= (inner) A)
(= (inner) B)

!(outer)
```

**Evaluation**:
```
Branch 1: (outer) → (inner) → {A, B}
Branch 2: (outer) → X

Result: {A, B, X}  (flattened alternatives)
```

**Property**: Alternatives are **flattened** (not nested sets).

---

## Implementation Details

### InterpretedAtom Structure

From `interpreter.rs`:

```rust
pub struct InterpretedAtom(Stack, Bindings);
```

Each alternative consists of:
- **Stack**: The expression(s) remaining to evaluate
- **Bindings**: Variable bindings for this alternative

### Alternative Addition

From `interpreter.rs`:211:

```rust
fn push(&mut self, interpreted_atom: InterpretedAtom) {
    self.plan.push(interpreted_atom);
}
```

**LIFO**: New alternatives are added to the end (pushed onto stack).

### Alternative Removal

From `interpreter.rs`:224:

```rust
pub fn pop(&mut self) -> Option<InterpretedAtom> {
    self.plan.pop()
}
```

**LIFO**: Most recently added alternative is processed first (popped from stack).

### Depth-First vs Breadth-First

**Current Implementation**: Depth-first (LIFO)

**Alternative**: Breadth-first (FIFO)
- Change `pop()` to `remove(0)` (pop from front)
- Ensures all branches at same depth are processed before going deeper

**Trade-offs**:
- **Depth-first**: Better memory usage, may loop forever on infinite branch
- **Breadth-first**: Explores all branches evenly, higher memory usage

### Parallel vs Sequential

**Specification**: Branches are evaluated "in parallel" (logically).

**Implementation**: Branches are evaluated **sequentially** (one at a time).

**Key Point**: "Parallel" is a logical property (all branches considered), not a physical property (no actual concurrency).

---

## Examples

### Example 1: Simple Non-Determinism

**Program**:
```metta
(= (coin) heads)
(= (coin) tails)

!(coin)
```

**Evaluation**:
```
Initial plan: {((coin), {})}

Step 1: Query (= (coin) $X)
  Matches: heads, tails
  New plan: {(heads, {}), (tails, {})}

Step 2: heads is a value → finished
  Finished: {heads}
  Plan: {(tails, {})}

Step 3: tails is a value → finished
  Finished: {heads, tails}
  Plan: {}

Result: {heads, tails}
```

### Example 2: Superpose

**Program**:
```metta
!(superpose (A B C D))
```

**Evaluation**:
```
Step 1: Execute SuperposeOp
  Returns: [A, B, C, D] (Vec<Atom>)

Result: {A, B, C, D}
```

### Example 3: Collapse

**Program**:
```metta
(= (color) red)
(= (color) green)
(= (color) blue)

!(collapse (color))
```

**Evaluation**:
```
Step 1: Evaluate (color)
  Results: {red, green, blue}

Step 2: Collapse collects all alternatives
  Result: (red green blue)  (single expression)
```

### Example 4: Cartesian Product

**Program**:
```metta
(= (first) A)
(= (first) B)
(= (second) X)
(= (second) Y)

!(pair (first) (second))
```

**Evaluation**:
```
Step 1: Evaluate (pair (first) (second))
  (first) ⇓ {A, B}
  (second) ⇓ {X, Y}

Step 2: Combine alternatives
  Branch 1: (pair A X)
  Branch 2: (pair A Y)
  Branch 3: (pair B X)
  Branch 4: (pair B Y)

Result: {(pair A X), (pair A Y), (pair B X), (pair B Y)}
```

### Example 5: Nested Non-Determinism

**Program**:
```metta
(= (outer) (inner 1))
(= (outer) (inner 2))
(= (inner $x) (* $x 10))
(= (inner $x) (+ $x 100))

!(outer)
```

**Evaluation**:
```
Step 1: Evaluate (outer)
  Branch 1: (inner 1)
  Branch 2: (inner 2)

Step 2: Evaluate (inner 1)
  Branch 1.1: (* 1 10) → 10
  Branch 1.2: (+ 1 100) → 101

Step 3: Evaluate (inner 2)
  Branch 2.1: (* 2 10) → 20
  Branch 2.2: (+ 2 100) → 102

Result: {10, 101, 20, 102}  (flattened)
```

### Example 6: Empty Results

**Program**:
```metta
; No rules defined for (undefined)
!(undefined)
```

**Evaluation**:
```
Step 1: Query (= (undefined) $X)
  Matches: [] (empty)

Step 2: No alternatives created
  Plan: {}

Result: {} (empty set) or (undefined) (no reduction)
```

### Example 7: Deterministic Chain in Non-Deterministic Context

**Program**:
```metta
(= (value) 1)
(= (value) 2)

!(chain (value) $x (* $x 10))
```

**Evaluation**:
```
Step 1: Evaluate (value)
  Alternatives: {1, 2}

Step 2: Chain with $x = 1
  Evaluate (* 1 10) → 10

Step 3: Chain with $x = 2
  Evaluate (* 2 10) → 20

Result: {10, 20}
```

**Key**: Each alternative flows through the chain independently.

### Example 8: Collapse with Side Effects

**Program**:
```metta
(= (mutate) (add-atom &space A))
(= (mutate) (add-atom &space B))

!(collapse (mutate))
```

**Evaluation** (order-dependent):
```
Branch 1: (add-atom &space A) → () with side effect
Branch 2: (add-atom &space B) → () with side effect

Collapse collects: (() ())

Final Space: {A, B} (both added, but order unspecified)
```

**Warning**: Side effects with non-determinism can be unpredictable.

---

## Specification vs Implementation

| Aspect | Specification | Implementation |
|--------|--------------|----------------|
| **Evaluation** | Logically parallel | Physically sequential |
| **Processing Order** | Unspecified | LIFO (depth-first) |
| **Alternative Storage** | Set semantics | Vector (plan) |
| **Termination** | All alternatives evaluated | All alternatives evaluated (or depth limit) |
| **Empty Results** | Valid (empty set) | Returns empty list or no reduction |
| **Nested Alternatives** | Flattened | Flattened |
| **Duplicate Results** | Undefined | May contain duplicates (implementation-specific) |

---

## Theoretical Properties

### Soundness

**Property**: All produced results are valid according to the reduction rules.

**Formal**:
```
If e ⇓ {v₁, ..., vₙ} then ∀i: e →* vᵢ
```

**Proof**: Each alternative follows valid reduction steps.

### Completeness

**Property**: All possible reductions are explored.

**Formal**:
```
If e →* v then ∃{v₁, ..., vₙ}: e ⇓ {v₁, ..., vₙ} ∧ v ∈ {v₁, ..., vₙ}
```

**Proof**: All matching rules are collected and processed.

### Confluence

**Property**: For pure programs, the set of final results is the same regardless of evaluation order.

**Formal**:
```
∀ evaluation orders o₁, o₂:
  eval(e, o₁) = eval(e, o₂)  (as sets)
```

**Holds**: For pure MeTTa (no side effects).

**Fails**: For programs with side effects (mutations, I/O).

---

## Design Recommendations

For MeTTa compiler implementers:

### Alternative Strategies

**Consider**:
1. **Breadth-First**: Explore all branches evenly
2. **Best-First**: Heuristic-guided exploration
3. **Limited Depth**: Bound branch depth for non-termination safety
4. **Lazy Evaluation**: Generate alternatives on demand

### Performance Optimizations

**Consider**:
1. **Memoization**: Cache results of repeated subexpressions
2. **Pruning**: Detect equivalent alternatives early
3. **Parallel Evaluation**: Actually evaluate branches in parallel threads
4. **Indexing**: Efficient duplicate detection

### Result Handling

**Provide**:
1. **Deterministic Mode**: Optionally return only first result
2. **Limit Results**: Cap number of alternatives
3. **Stream Results**: Return iterator instead of collecting all
4. **Prioritize Results**: Order results by some criteria

**Example API**:
```metta
; Limit to first 10 results
!(limit 10 (match &space (item $x) $x))

; Return as stream
!(lazy (expensive-computation))
```

### Debugging

**Provide**:
1. **Branch Visualization**: Show evaluation tree
2. **Alternative Counting**: Report number of branches
3. **Trace Mode**: Log when alternatives are created/finished

---

## References

### Source Code

- **`hyperon-experimental/lib/src/metta/interpreter.rs`**
  - `InterpreterState` (lines 172-183): Alternative storage
  - `interpret_step()` (lines 269-277): Alternative processing
  - `collapse_bind()` (lines 746-792): Collapse implementation

- **`hyperon-experimental/lib/src/metta/runner/stdlib/core.rs`**
  - `SuperposeOp` (lines 201-222): Superpose implementation

- **`hyperon-experimental/docs/minimal-metta.md`**
  - Lines 21-26: Interpretation plan definition

### Academic References

- **Dijkstra, E. W.** (1976). *A Discipline of Programming*. Prentice Hall. (Non-deterministic choice)
- **Floyd, R. W.** (1967). "Nondeterministic Algorithms". *Journal of the ACM*.
- **Apt, K. R. & Plotkin, G. D.** (1986). "Countable Nondeterminism and Random Assignment". *Journal of the ACM*.

---

## See Also

- **§01**: Evaluation order (how alternatives are evaluated)
- **§03**: Pattern matching (source of alternatives)
- **§04**: Reduction order (multiple rules create alternatives)
- **§07**: Formal proofs (soundness, completeness, confluence)

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
