# MeTTa Order of Operations: Overview

## Executive Summary

This document provides a high-level overview of the order of operations in MeTTa, based on analysis of the hyperon-experimental reference implementation. Understanding these ordering semantics is critical for reasoning about MeTTa program behavior, especially when side effects (like atom space mutations) are involved.

## Key Findings

### 1. Non-Deterministic Evaluation (§01, §05)

**MeTTa uses non-deterministic evaluation by default** - all alternative evaluation branches are explored in parallel (logically, not physically).

```
Specification: When an expression has multiple possible reductions,
               ALL alternatives are pursued simultaneously.

Implementation: Alternatives are stored in a plan vector and processed
                sequentially in LIFO (stack) order.
```

**Implication**: Programs must be designed to work correctly regardless of the order in which alternatives are explored.

### 2. Normal Evaluation Order (§01)

**Arguments are NOT evaluated before being passed to functions** (normal order evaluation).

```
Specification: Function arguments are passed unevaluated.
               Use (chain) or similar constructs to force evaluation.

Implementation: Minimal MeTTa uses normal order; full MeTTa uses
                applicative order with type-based control.
```

**Comparison**:
- Normal order (MeTTa): `(f (+ 1 2))` → f receives `(+ 1 2)` unevaluated
- Applicative order (Lisp): `(f (+ 1 2))` → f receives `3`

### 3. No Atomicity for Mutations (§02)

**Atom space mutations (add-atom, remove-atom) are NOT atomic** and have no transactional guarantees.

```
Specification: Mutation ordering is undefined when multiple mutations
               occur during evaluation of a single expression.

Implementation: Uses Rust RefCell for runtime borrow checking.
                No mutex, lock, or atomic primitives.
                NOT thread-safe.
```

**Implication**: Concurrent mutations or mutations during pattern matching may have undefined behavior.

### 4. Pattern Matching Order is Implementation-Dependent (§03)

**The order in which pattern matches are returned is not specified**.

```
Specification: Pattern matching returns all matches, but order is unspecified.

Implementation: Order depends on the underlying trie-based space index
                iteration order (AtomIndex).
```

**Implication**: Do not rely on any specific pattern match ordering.

### 5. All Reduction Rules are Explored (§04)

**When multiple reduction rules match, ALL are applied** (creating multiple evaluation branches).

```
Specification: Non-deterministic reduction - all matching rules create
               alternative branches.

Implementation: All (= <pattern> <result>) matches are collected and
                added to the evaluation plan.
```

**Implication**: This is the source of non-determinism in MeTTa.

## Critical Properties

### Determinism vs Non-Determinism

| Aspect | Deterministic? | Notes |
|--------|----------------|-------|
| Expression evaluation within a branch | ✓ Yes | Single branch follows deterministic normal order |
| Multiple reduction matches | ✗ No | All matches create new branches |
| Pattern matching | ✗ No | Multiple matches create branches |
| Superpose alternatives | ✗ No | Creates multiple branches by design |
| Atom space mutations | ⚠ Partial | Deterministic within single thread, order-dependent with side effects |
| Plan processing | ⚠ Implementation | LIFO in current impl, but logically all alternatives are equal |

### Confluence

**Question**: Is MeTTa confluent? (Do all evaluation paths lead to the same result?)

**Answer**:
- **Without side effects**: MeTTa evaluation is generally confluent - all branches explore the same semantic space.
- **With side effects** (add-atom, remove-atom): **NOT confluent** - different evaluation orders can produce different final atom spaces.

See §07 for formal proofs.

### Thread Safety

**MeTTa is NOT thread-safe** in the current implementation:
- No locks or mutexes
- Uses RefCell for interior mutability (panics on concurrent borrow)
- Assumes single-threaded execution

## Document Structure

This documentation is organized into the following files:

1. **00-overview.md** (this file) - Executive summary
2. **01-evaluation-order.md** - S-expression evaluation semantics
3. **02-mutation-order.md** - Atom space mutation ordering and atomicity
4. **03-pattern-matching.md** - Pattern matching and query order
5. **04-reduction-order.md** - Reduction rule application order
6. **05-non-determinism.md** - Non-deterministic evaluation semantics
7. **06-implementation-notes.md** - Implementation-specific details from hyperon-experimental
8. **07-proofs.md** - Formal proofs of semantic properties
9. **08-comparisons.md** - Comparisons with other languages (Prolog, Lisp, Haskell)
10. **examples/*.metta** - Executable examples demonstrating behaviors

## Quick Reference

### When Order Matters

Order of operations matters in these scenarios:

1. **Side Effects**: add-atom, remove-atom, print, etc.
2. **Resource-Dependent Computations**: Operations that depend on current atom space state
3. **Performance**: Different evaluation orders may have different performance characteristics
4. **Debugging**: Understanding which branch executed first

### When Order Doesn't Matter

Order is irrelevant for:

1. **Pure Computations**: No side effects, no atom space queries
2. **Final Results**: All branches should produce semantically equivalent results (if confluent)
3. **Logical Correctness**: Programs should work correctly regardless of branch exploration order

## Recommendations for MeTTa Compiler Developers

When implementing a MeTTa compiler:

1. **Preserve Non-Determinism**: Ensure all alternatives are explored (or document limitations)
2. **Document Evaluation Strategy**: Clearly specify whether implementation is eager, lazy, or hybrid
3. **Handle Side Effects Carefully**: Define clear semantics for mutation ordering
4. **Consider Confluence**: Provide tools to detect non-confluent programs
5. **Thread Safety**: If adding concurrency, ensure proper synchronization around mutations
6. **Performance**: Consider optimizations like:
   - Lazy branch exploration
   - Memoization of repeated computations
   - Parallel branch exploration (with proper mutation handling)

## References

- **hyperon-experimental**: Official MeTTa reference implementation
  - Repository: https://github.com/trueagi-io/hyperon-experimental
  - Key files analyzed:
    - `lib/src/metta/interpreter.rs` - Core interpreter
    - `lib/src/metta/runner/stdlib/space.rs` - Space operations
    - `docs/minimal-metta.md` - Minimal MeTTa specification

- **Related Research**:
  - Plotkin, G. D. (1975). Call-by-name, call-by-value and the λ-calculus
  - Barendregt, H. P. (1984). The Lambda Calculus: Its Syntax and Semantics

## Version

- Document Version: 1.0
- Based on: hyperon-experimental commit `164c22e9` (2025-01-13)
- Author: Generated analysis of MeTTa reference implementation
- Last Updated: 2025-11-13
