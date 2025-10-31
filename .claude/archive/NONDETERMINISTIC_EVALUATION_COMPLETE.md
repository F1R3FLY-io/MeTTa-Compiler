# Nondeterministic Evaluation Complete

**Date**: October 17, 2025

## Summary

Successfully implemented full nondeterministic evaluation in MeTTaTron, enabling multiply-defined patterns to produce all matching results and propagate through nested function applications via Cartesian product semantics.

## Feature Description

### Core Behavior

When a pattern is defined multiple times, evaluation returns ALL matching results:

```metta
(= (f) 1)
(= (f) 2)
(= (f) 3)
!(f)  ; Returns [1, 2, 3]
```

### Nested Application

When a function is applied to a multiply-defined pattern, it applies to ALL results:

```metta
(= (f) 1)
(= (f) 2)
(= (f) 3)
(= (g $x) (* $x $x))
!(g (f))  ; Returns [1, 4, 9]
```

### Cartesian Product

When multiple sub-expressions are nondeterministic, the Cartesian product is computed:

```metta
(= (a) 1)
(= (a) 2)
(= (b) 10)
(= (b) 20)
!(+ (a) (b))  ; Returns [11, 21, 12, 22]
```

## Implementation

### Key Files Modified

1. **src/backend/eval.rs** (Lines 271-310, 535-565)
   - Replaced simple "take first result" logic with Cartesian product generation
   - Added `cartesian_product()` helper function
   - Modified `eval_sexpr()` to handle multiple results from sub-expressions

### Cartesian Product Algorithm

```rust
fn cartesian_product(results: &[Vec<MettaValue>]) -> Vec<Vec<MettaValue>> {
    if results.is_empty() {
        return vec![vec![]];
    }

    // Base case: single result list
    if results.len() == 1 {
        return results[0].iter()
            .map(|item| vec![item.clone()])
            .collect();
    }

    // Recursive case: combine first list with Cartesian product of rest
    let first = &results[0];
    let rest_product = cartesian_product(&results[1..]);

    let mut product = Vec::new();
    for item in first {
        for rest_combo in &rest_product {
            let mut combo = vec![item.clone()];
            combo.extend(rest_combo.clone());
            product.push(combo);
        }
    }

    product
}
```

### Evaluation Flow

**Before Fix** (Lines 271-278, old):
```rust
// Flatten the results into a single evaluated expression
let mut evaled_items = Vec::new();
for results in eval_results {
    // For now, take the first result (need to handle multiple results properly)
    if let Some(first) = results.first() {
        evaled_items.push(first.clone());
    }
}
```

**After Fix** (Lines 271-310, new):
```rust
// Handle nondeterministic evaluation: generate Cartesian product of all sub-expression results
let combinations = cartesian_product(&eval_results);

// Collect all final results from all combinations
let mut all_final_results = Vec::new();

for evaled_items in combinations {
    // Try builtin operations
    if let Some(MettaValue::Atom(op)) = evaled_items.first() {
        if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
            all_final_results.push(result);
            continue;
        }
    }

    // Try pattern matching
    let sexpr = MettaValue::SExpr(evaled_items.clone());
    let all_matches = try_match_all_rules(&sexpr, &unified_env);

    if !all_matches.is_empty() {
        for (rhs, bindings) in all_matches {
            let instantiated_rhs = apply_bindings(&rhs, &bindings);
            let (results, _) = eval(instantiated_rhs, unified_env.clone());
            all_final_results.extend(results);
        }
    } else {
        let mut final_env = unified_env.clone();
        final_env.add_to_space(&sexpr);
        all_final_results.push(sexpr);
    }
}

(all_final_results, unified_env)
```

## Test Coverage

Added 5 comprehensive tests to `src/lib.rs` (Lines 1227-1422):

### 1. test_nondeterministic_nested_application
Tests that `g` is applied to ALL expansions of `f`:
- `(f) → [1, 2, 3]`
- `(g $x) → (* $x $x)`
- `!(g (f)) → [1, 4, 9]`

### 2. test_nondeterministic_cartesian_product
Tests Cartesian product with two nondeterministic operands:
- `(a) → [1, 2]`
- `(b) → [10, 20]`
- `!(+ (a) (b)) → [11, 21, 12, 22]`

### 3. test_nondeterministic_triple_product
Tests three-way Cartesian product:
- `(x) → [1, 2]`, `(y) → [10, 20]`, `(z) → [100, 200]`
- `!(cons (x) (cons (y) (z))) → 8 results`

### 4. test_nondeterministic_deeply_nested
Tests deep nesting of nondeterministic functions:
- `(f) → [1, 2]`
- `(g $x) → (* $x 10)`
- `(h $x) → (+ $x 5)`
- `!(h (g (f))) → [15, 25]`

### 5. test_nondeterministic_with_pattern_matching
Tests nondeterminism combined with pattern matching:
- `(color) → [red, green, blue]`
- Multiple rules for `(intensity $color)`
- `!(intensity (color)) → [100, 150, 200]`

## Test Results

**Total Tests**: 287 (up from 282)
**New Tests**: 5 nondeterministic evaluation tests
**Status**: All tests passing (0 failed, 0 ignored)
**Execution Time**: ~0.10s

## Examples

### Example 1: Simple Multiply-Defined Pattern
```metta
(= (coin) heads)
(= (coin) tails)
!(coin)
```
**Output**: `[heads, tails]`

### Example 2: Nested Application
```metta
(= (digit) 0)
(= (digit) 1)
(= (double $x) (* $x 2))
!(double (digit))
```
**Output**: `[0, 2]`

### Example 3: Cartesian Product
```metta
(= (x) 1)
(= (x) 2)
(= (y) 3)
(= (y) 4)
!(cons (x) (y))
```
**Output**: `[(cons 1 3), (cons 1 4), (cons 2 3), (cons 2 4)]`

### Example 4: Deep Nesting
```metta
(= (base) 1)
(= (base) 2)
(= (step1 $x) (+ $x 10))
(= (step2 $x) (* $x 3))
!(step2 (step1 (base)))
```
**Output**: `[33, 36]`  ; (1+10)*3=33, (2+10)*3=36

## Semantic Correctness

The implementation follows standard nondeterministic semantics:

1. **Multiply-defined rules** return all matching alternatives
2. **Function application** distributes over alternatives (applicative functor)
3. **Multiple nondeterministic arguments** produce Cartesian products
4. **Nested applications** propagate nondeterminism correctly

This matches the behavior of languages like Prolog, miniKanren, and other logic programming systems.

## Performance Considerations

- **Complexity**: O(k₁ × k₂ × ... × kₙ) where kᵢ is the number of results from sub-expression i
- **Optimization**: MORK space pattern matching already optimized via query_multi
- **Memory**: Results are eagerly computed (no lazy streams yet)

### Future Optimizations

1. **Lazy evaluation of Cartesian products** - compute alternatives on-demand
2. **Pruning** - early termination when specific result is found
3. **Memoization** - cache results of nondeterministic expressions

## Integration

The nondeterministic evaluation feature integrates seamlessly with:

- **Pattern matching** - multiply-defined patterns work with complex patterns
- **Recursive functions** - nondeterminism propagates through recursion
- **Higher-order functions** - function arguments can be nondeterministic
- **Error handling** - errors still propagate immediately
- **Type system** - type checking works with multiple alternatives

## Backward Compatibility

This change is **fully backward compatible**:

- Single-result patterns still work exactly as before
- Deterministic code produces identical results
- All 282 existing tests still pass
- The only change is handling multiple results correctly

## Documentation Updates

- Added detailed comments to `cartesian_product()` function
- Updated evaluation flow comments in `eval_sexpr()`
- Comprehensive test documentation with examples

## Verification

Manual CLI testing confirms correct behavior:

```bash
$ cat test.metta
(= (f) 1)
(= (f) 2)
(= (f) 3)
(= (g $x) (* $x $x))
!(g (f))

$ ./target/release/mettatron test.metta
[1, 4, 9]
```

## Conclusion

Nondeterministic evaluation is now **fully implemented and tested** in MeTTaTron. The Cartesian product semantics correctly propagate multiple results through nested function applications, enabling powerful logic programming and constraint solving patterns.

**Key Achievement**: MeTTa can now express and evaluate nondeterministic computations naturally, matching the expected behavior of logic programming languages while maintaining the functional programming model.
