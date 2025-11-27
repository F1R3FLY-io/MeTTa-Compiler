# Comparisons with Other Languages

## Abstract

This document compares MeTTa's order of operations and evaluation semantics with other programming languages, including Prolog, Lisp, Haskell, Python, and term rewriting systems. Understanding these comparisons helps situate MeTTa in the broader landscape of programming language design and provides insights for developers familiar with other languages.

## Table of Contents

1. [Prolog](#prolog)
2. [Lisp and Scheme](#lisp-and-scheme)
3. [Haskell](#haskell)
4. [Term Rewriting Systems](#term-rewriting-systems)
5. [Python](#python)
6. [Summary Comparison Table](#summary-comparison-table)

---

## Prolog

### Overview

**Prolog** is a logic programming language based on first-order logic and unification.

### Similarities with MeTTa

1. **Pattern Matching**: Both use pattern matching as a core feature
   ```prolog
   % Prolog
   color(red).
   color(green).
   color(blue).
   ```
   ```metta
   ; MeTTa
   (= (color) red)
   (= (color) green)
   (= (color) blue)
   ```

2. **Unification**: Both perform unification to bind variables
   ```prolog
   % Prolog
   ?- edge(X, Y).
   ```
   ```metta
   ; MeTTa
   !(match &space (edge $X $Y) (pair $X $Y))
   ```

3. **Declarative Style**: Both support declarative programming

### Key Differences

#### 1. **Determinism vs Non-Determinism**

**Prolog**: Sequential search with backtracking
```prolog
color(red).
color(green).
color(blue).

?- color(X).
X = red ;    % First result
X = green ;  % Backtrack for second
X = blue.    % Backtrack for third
```
- Returns **one result at a time**
- User must explicitly request backtracking (`;`)
- **Deterministic from user's perspective** (one result unless backtracking)

**MeTTa**: Parallel exploration
```metta
(= (color) red)
(= (color) green)
(= (color) blue)

!(color)
; → {red, green, blue}  All results at once
```
- Returns **all results simultaneously**
- **Non-deterministic evaluation** (all alternatives explored)

#### 2. **Clause Order Matters**

**Prolog**: First matching clause wins (unless backtracking)
```prolog
classify(0, zero).
classify(X, nonzero).

?- classify(0, Y).
Y = zero.  % First clause matches, second not tried
```
- **Order-dependent**: Changing clause order changes behavior
- **First-match semantics**

**MeTTa**: All matching rules applied
```metta
(= (classify 0) zero)
(= (classify $X) nonzero)

!(classify 0)
; → {zero, nonzero}  Both rules fire!
```
- **Order-independent**: All matches collected
- **All-match semantics**

#### 3. **Cut Operator**

**Prolog**: Has `!` (cut) to prevent backtracking
```prolog
max(X, Y, X) :- X >= Y, !.
max(X, Y, Y).
```
- Cut commits to current choice
- Prevents exploring other alternatives

**MeTTa**: No cut operator
- All alternatives always explored
- Cannot prune search space declaratively

#### 4. **Evaluation Order**

**Prolog**: Left-to-right, depth-first
```prolog
?- goal1(X), goal2(X), goal3(X).
```
- Goals evaluated left-to-right
- Each goal fully explored before next
- Depth-first search with backtracking

**MeTTa**: Non-deterministic (logically parallel)
```metta
!(triple (goal1) (goal2) (goal3))
```
- Alternatives explored in implementation-defined order
- All branches considered "simultaneously"
- Depth-first in implementation (LIFO), but logically parallel

### Comparison Table

| Feature | Prolog | MeTTa |
|---------|--------|-------|
| **Pattern Matching** | Yes | Yes |
| **Unification** | Yes | Yes |
| **Clause Order** | Matters | Doesn't matter |
| **Result Model** | One at a time (backtracking) | All at once (set) |
| **Cut/Pruning** | Yes (`!`) | No |
| **Determinism** | Sequential with backtrack | Non-deterministic |
| **Evaluation** | Depth-first, left-to-right | Logically parallel |

---

## Lisp and Scheme

### Overview

**Lisp/Scheme** are functional programming languages with s-expression syntax, similar to MeTTa's surface syntax.

### Similarities with MeTTa

1. **S-Expression Syntax**: Both use parenthesized expressions
   ```scheme
   ; Scheme
   (+ 1 2)
   (cons 'a '(b c))
   ```
   ```metta
   ; MeTTa
   (+ 1 2)
   (cons A (B C))
   ```

2. **Symbolic Computation**: Both manipulate symbolic expressions

3. **Homoiconicity**: Code is data (can manipulate programs as data)

### Key Differences

#### 1. **Evaluation Order**

**Lisp/Scheme**: Applicative order (eager)
```scheme
(define (f x) x)
(f (+ 1 2))
; Evaluates (+ 1 2) → 3 before passing to f
; f receives 3
```
- Arguments evaluated **before** function application
- **Call-by-value** semantics

**MeTTa**: Normal order (lazy)
```metta
(= (f $x) $x)
!(f (+ 1 2))
; Passes (+ 1 2) unevaluated to f
; f receives (+ 1 2) directly
```
- Arguments **not** evaluated before function application
- **Call-by-name** semantics

#### 2. **Determinism**

**Lisp/Scheme**: Fully deterministic
```scheme
(define (choose) 'a)
(choose)  ; → a (always)
```
- One input → one output
- No built-in non-determinism

**MeTTa**: Non-deterministic
```metta
(= (choose) a)
(= (choose) b)
!(choose)  ; → {a, b}
```
- One input → multiple outputs
- Non-determinism is core feature

#### 3. **Pattern Matching**

**Lisp/Scheme**: Not built-in (can be added via macros)
```scheme
; Requires pattern matching library
(match expr
  [(list 'foo x) (process x)]
  [_ 'no-match])
```
- Pattern matching is library feature
- Not central to language

**MeTTa**: Built-in and central
```metta
!(match &space (foo $x) (process $x))
```
- Pattern matching is core operation
- Integrated with evaluation

#### 4. **State and Mutation**

**Lisp/Scheme**: Explicit mutation operations
```scheme
(define x 5)
(set! x 10)  ; Explicit mutation
```
- Clear distinction between pure and mutating operations

**MeTTa**: Space mutations are side effects
```metta
!(add-atom &space A)
```
- Mutations affect shared space
- Less clear distinction (looks like regular operation)

### Comparison Table

| Feature | Lisp/Scheme | MeTTa |
|---------|-------------|-------|
| **S-Expression Syntax** | Yes | Yes |
| **Evaluation Order** | Applicative (eager) | Normal (lazy) |
| **Determinism** | Deterministic | Non-deterministic |
| **Pattern Matching** | Library feature | Built-in core feature |
| **Mutation** | Explicit (`set!`) | Space operations |
| **Macros** | Yes (hygienic in Scheme) | Not specified |

---

## Haskell

### Overview

**Haskell** is a pure functional programming language with lazy evaluation.

### Similarities with MeTTa

1. **Lazy Evaluation**: Both use normal order evaluation
   ```haskell
   -- Haskell
   const x y = x
   const 42 undefined  -- → 42 (y never evaluated)
   ```
   ```metta
   ; MeTTa
   (= (const $x $y) $x)
   !(const 42 undefined)  ; → 42 ($y never evaluated)
   ```

2. **Pattern Matching**: Both have powerful pattern matching
   ```haskell
   -- Haskell
   factorial 0 = 1
   factorial n = n * factorial (n - 1)
   ```
   ```metta
   ; MeTTa
   (= (factorial 0) 1)
   (= (factorial $n) (* $n (factorial (- $n 1))))
   ```

### Key Differences

#### 1. **Type System**

**Haskell**: Strong static type system
```haskell
factorial :: Int -> Int
factorial 0 = 1
factorial n = n * factorial (n - 1)
```
- Types checked at compile time
- Type errors caught early
- Type inference (infers types automatically)

**MeTTa**: Dynamically typed (current implementation)
```metta
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))
```
- Types checked at runtime (if at all)
- More flexible but less safe

**Note**: MeTTa has a type system under development, but not enforced in minimal MeTTa.

#### 2. **Purity**

**Haskell**: Pure by default
```haskell
-- Pure function
add :: Int -> Int -> Int
add x y = x + y

-- Side effects require IO monad
main :: IO ()
main = putStrLn "Hello"
```
- Side effects isolated in monads
- Clear separation of pure and effectful code

**MeTTa**: Effectful operations mixed with pure
```metta
; Pure
(= (add $x $y) (+ $x $y))

; Effectful (no special marking)
!(add-atom &space A)
```
- No syntactic distinction between pure and effectful
- Mutations can happen anywhere

#### 3. **Non-Determinism**

**Haskell**: Deterministic (non-determinism via monads)
```haskell
-- Deterministic
color = "red"

-- Non-determinism via list monad
colors = ["red", "green", "blue"]
```
- Non-determinism is explicit (list, Maybe, etc.)
- One value per expression (unless in monad)

**MeTTa**: Non-deterministic by default
```metta
(= (color) red)
(= (color) green)
(= (color) blue)
!(color)  ; → {red, green, blue}
```
- Non-determinism is built-in
- Multiple values naturally

#### 4. **Pattern Matching Order**

**Haskell**: First match wins
```haskell
classify :: Int -> String
classify 0 = "zero"
classify n = "nonzero"

classify 0  -- → "zero" (first clause matches, second not tried)
```
- **Sequential matching**
- Order matters

**MeTTa**: All matches explored
```metta
(= (classify 0) zero)
(= (classify $n) nonzero)
!(classify 0)  ; → {zero, nonzero}
```
- **Parallel matching**
- Order irrelevant

### Comparison Table

| Feature | Haskell | MeTTa |
|---------|---------|-------|
| **Lazy Evaluation** | Yes | Yes (minimal MeTTa) |
| **Type System** | Strong static | Dynamic (type system in development) |
| **Purity** | Pure (IO monad for effects) | Mixed pure/effectful |
| **Non-Determinism** | Via monads | Built-in |
| **Pattern Match Order** | First match | All matches |
| **Compilation** | Compiled | Interpreted (current impl) |

---

## Term Rewriting Systems

### Overview

**Term Rewriting Systems (TRS)** are formal systems for manipulating terms via rewrite rules.

### Similarities with MeTTa

1. **Rule-Based**: Both use rules to transform expressions
   ```
   TRS: f(0) → 1
   MeTTa: (= (f 0) 1)
   ```

2. **Pattern Matching**: Both match patterns on left-hand side
3. **Reduction**: Both reduce terms via rule application

### Key Differences

#### 1. **Strategy**

**TRS**: Various strategies
- **Leftmost-outermost**: Normal order
- **Leftmost-innermost**: Applicative order
- **Parallel-outermost**: Reduce all redexes at same level
- Strategy is explicit design choice

**MeTTa**: Non-deterministic
- All reductions explored (logically parallel)
- Strategy is "all strategies simultaneously"

#### 2. **Confluence**

**TRS**: Confluence is important property
- **Orthogonal TRS**: Guaranteed confluent
- **Confluent TRS**: Unique normal forms
- **Non-confluent TRS**: Multiple normal forms possible

**MeTTa**: Confluent for pure fragment, non-confluent with side effects
- Pure MeTTa is confluent (Theorem 1, §07)
- Side effects break confluence (Theorem 2, §07)

#### 3. **Formal Semantics**

**TRS**: Rigorous formal semantics
- Rules are equations
- Reduction is equality preservation
- Well-studied theory (Church-Rosser, etc.)

**MeTTa**: Operational semantics
- Rules define computation steps
- Less formal (no published formal semantics yet)
- Implementation-defined in places

### Comparison Table

| Feature | Term Rewriting Systems | MeTTa |
|---------|------------------------|-------|
| **Rule-Based** | Yes | Yes |
| **Pattern Matching** | Yes | Yes |
| **Strategy** | Explicit (many options) | Non-deterministic |
| **Confluence** | Important property | Pure: yes, Effects: no |
| **Formal Semantics** | Rigorous | Operational (implementation) |
| **Purpose** | Formal reasoning | Programming language |

---

## Python

### Overview

**Python** is an imperative, object-oriented, dynamically typed language.

### Similarities with MeTTa

1. **Dynamic Typing**: Both are dynamically typed
2. **Interpreted**: Both are typically interpreted
3. **High-Level**: Both are high-level languages

### Key Differences

#### 1. **Syntax**

**Python**: Statement-based
```python
def factorial(n):
    if n == 0:
        return 1
    else:
        return n * factorial(n - 1)
```

**MeTTa**: Expression-based (s-expressions)
```metta
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))
```

#### 2. **Evaluation Model**

**Python**: Eager (applicative order)
```python
def f(x):
    return x

f(expensive_computation())  # Computed before f is called
```

**MeTTa**: Lazy (normal order, minimal MeTTa)
```metta
(= (f $x) $x)
!(f (expensive-computation))  ; Not computed (unless f uses $x)
```

#### 3. **Determinism**

**Python**: Deterministic
```python
def choose():
    return 'a'

choose()  # → 'a' (always)
```

**MeTTa**: Non-deterministic
```metta
(= (choose) a)
(= (choose) b)
!(choose)  ; → {a, b}
```

#### 4. **Pattern Matching**

**Python**: Recently added (Python 3.10+)
```python
match expr:
    case ['foo', x]:
        return process(x)
    case _:
        return 'no-match'
```
- New feature (match statement)
- Not central to language design

**MeTTa**: Core feature
```metta
!(match &space (foo $x) (process $x))
```
- Central to language
- Integral to evaluation model

#### 5. **Rule-Based Programming**

**Python**: Not rule-based
- Imperative: sequence of commands
- Functions are primary abstraction

**MeTTa**: Rule-based
- Declarative: set of rules
- Rules define computation

### Comparison Table

| Feature | Python | MeTTa |
|---------|--------|-------|
| **Syntax** | Statement-based | S-expressions |
| **Evaluation** | Eager | Lazy (minimal) |
| **Typing** | Dynamic | Dynamic |
| **Determinism** | Deterministic | Non-deterministic |
| **Pattern Matching** | Recent addition | Core feature |
| **Paradigm** | Imperative/OO | Declarative/Rule-based |
| **Primary Use** | General-purpose | Logic/Symbolic AI |

---

## Summary Comparison Table

| Feature | MeTTa | Prolog | Lisp/Scheme | Haskell | TRS | Python |
|---------|-------|--------|-------------|---------|-----|--------|
| **Syntax** | S-expr | Logic | S-expr | Functional | Math | Statements |
| **Evaluation** | Lazy | Eager | Eager | Lazy | Various | Eager |
| **Typing** | Dynamic | Dynamic | Dynamic | Static | Untyped | Dynamic |
| **Pattern Matching** | Core | Core | Library | Core | Core | Recent |
| **Determinism** | Non-det | Sequential | Det | Det | Varies | Det |
| **Rule Order** | Irrelevant | Important | N/A | Important | Varies | N/A |
| **All Solutions** | Yes | Manual backtrack | No | Via monad | N/A | No |
| **Pure** | No | No | No | Yes | N/A | No |
| **Primary Paradigm** | Logic/Rule | Logic | Functional | Functional | Formal | Imperative |

**Legend**:
- S-expr: S-expression
- Det: Deterministic
- Non-det: Non-deterministic
- N/A: Not applicable

---

## Design Philosophy Comparison

### MeTTa's Unique Position

**MeTTa** combines features from multiple paradigms:

1. **From Logic Programming (Prolog)**:
   - Pattern matching
   - Unification
   - Logical reasoning

2. **From Functional Programming (Lisp, Haskell)**:
   - S-expression syntax
   - Lazy evaluation (minimal MeTTa)
   - Higher-order functions

3. **From Term Rewriting**:
   - Rule-based computation
   - Reduction semantics

4. **Unique Features**:
   - **Non-deterministic by default**: All solutions computed simultaneously
   - **Order-independent rules**: No rule priority or sequencing
   - **Atom spaces**: Shared mutable knowledge base
   - **Focus on AI/Symbolic reasoning**: Designed for OpenCog/Hyperon

### Trade-offs

**Advantages**:
- Expressivity: Can express non-deterministic computations naturally
- Flexibility: No rule ordering constraints
- AI-friendly: Pattern matching and symbolic reasoning are first-class

**Disadvantages**:
- Performance: Non-determinism can lead to exponential blowup
- Debugging: Non-determinism makes debugging harder
- Side effects: Mutations with non-determinism can be unpredictable

---

## Lessons from Other Languages

### Best Practices

1. **From Haskell**: Separate pure and effectful code
   - **Recommendation**: Consider marking effectful operations distinctly in MeTTa

2. **From Prolog**: Control search space
   - **Recommendation**: Add mechanisms to prune alternatives (like cut, but better)

3. **From TRS**: Ensure confluence
   - **Recommendation**: Provide tools to check/verify confluence

4. **From Lisp**: Powerful macro system
   - **Recommendation**: Add metaprogramming capabilities to MeTTa

### Potential Improvements

1. **Type System**: Add optional static typing (like Typed Racket)
2. **Effect System**: Track side effects in types (like Haskell's monads)
3. **Deterministic Mode**: Option to compute only first result (like Prolog's first-match)
4. **Pattern Guards**: Add conditions to patterns (like Haskell)
5. **Staged Computation**: Separate compile-time and run-time (like MetaML)

---

## See Also

- **§00-06**: MeTTa semantics (what is being compared)
- **§07**: Formal proofs (theoretical properties)
- **Language Websites**:
  - Prolog: https://www.swi-prolog.org/
  - Scheme: https://www.scheme.org/
  - Haskell: https://www.haskell.org/
  - Python: https://www.python.org/

---

## References

### Language Specifications

- **Prolog**: ISO/IEC 13211-1:1995 (Prolog standard)
- **Scheme**: R7RS (Revised⁷ Report on Scheme)
- **Haskell**: Haskell 2010 Language Report
- **Python**: Python Language Reference (version 3.x)

### Academic References

- **Kowalski, R.** (1974). "Predicate Logic as Programming Language". *IFIP Congress*.
  - Prolog foundations

- **Steele, G. & Sussman, G.** (1975). "Scheme: An Interpreter for Extended Lambda Calculus". *MIT AI Lab*.
  - Scheme design

- **Hudak, P. et al.** (1992). "Report on the Programming Language Haskell". *SIGPLAN*.
  - Haskell design

- **Dershowitz, N. & Jouannaud, J.-P.** (1990). "Rewrite Systems". *Handbook of TCS*.
  - Term rewriting theory

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
