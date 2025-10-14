# Reduction Prevention in MeTTa Evaluator

This document describes the comprehensive reduction prevention mechanisms implemented in the MeTTa Evaluator as required by GitHub Issue #3.

## Overview

Reduction prevention allows fine-grained control over evaluation, preventing unwanted reduction of expressions and providing error recovery mechanisms. The implementation includes quotation, explicit evaluation, and error handling.

## Features Implemented

### 1. Quote - Prevent Evaluation

**Syntax**: `(quote expr)`

**Description**: Returns the expression unevaluated, preventing all reduction.

**Examples**:
```lisp
(quote (+ 1 2))        ; Returns (+ 1 2), not 3
(quote $x)             ; Returns $x, not its value
(quote (error "x" 0))  ; Returns the error expression, doesn't create error
```

**Use Cases**:
- Pass expressions as data
- Metaprogramming
- Symbolic computation
- Template definitions

**Implementation**: `src/backend/eval.rs:72-83`

---

### 2. Eval - Force Evaluation

**Syntax**: `(eval expr)`

**Description**: Complementary to `quote`, forces evaluation of an expression. Useful for evaluating quoted expressions or dynamically constructed expressions.

**Examples**:
```lisp
(eval (quote (+ 1 2)))  ; Returns 3

; Dynamic evaluation
(= (apply $f $x) (eval (quote ($f $x))))
!(apply double 21)      ; Returns 42
```

**Use Cases**:
- Evaluating quoted expressions
- Dynamic code evaluation
- Metaprogramming
- Delayed evaluation

**Implementation**: `src/backend/eval.rs:113-132`

---

### 3. Catch - Error Recovery

**Syntax**: `(catch expr default)`

**Description**: Evaluates `expr`. If it returns an error, evaluates and returns `default` instead. This **prevents error propagation** (reduction prevention).

**Examples**:
```lisp
; Basic error recovery
(catch (error "fail" 0) 42)  ; Returns 42

; No error case
(catch (+ 1 2) "default")    ; Returns 3

; Prevent error propagation in arithmetic
(+ 10 (catch (error "x" 0) 5))  ; Returns 15, not error
```

**Use Cases**:
- Error recovery without terminating evaluation
- Providing default values for failed computations
- Robust computation pipelines
- Graceful degradation

**Implementation**: `src/backend/eval.rs:107-111, 257-286`

---

### 4. Is-Error - Error Detection

**Syntax**: `(is-error expr)`

**Description**: Evaluates `expr` and returns `true` if the result is an error, `false` otherwise. Used for conditional logic based on error status.

**Examples**:
```lisp
(is-error (error "test" 0))  ; Returns true
(is-error 42)                 ; Returns false
(is-error (+ 1 2))           ; Returns false

; Conditional based on error
(if (is-error (computation))
    "handle-error"
    "success")
```

**Use Cases**:
- Conditional error handling
- Error detection in pipelines
- Validation
- Testing

**Implementation**: `src/backend/eval.rs:134-151`

---

### 5. Error - Error Construction

**Syntax**: `(error msg details)`

**Description**: Creates an error value with a message and optional details. The details are **not evaluated** (reduction prevention).

**Examples**:
```lisp
(error "message" 42)
(error "division by zero" $y)
(error "type mismatch" (list $x $y))
```

**Use Cases**:
- Creating errors in rules
- Error reporting
- Validation failures
- Type checking

**Implementation**: `src/backend/eval.rs:90-105`

---

## Combined Usage Examples

### Example 1: Safe Division with Error Recovery

```lisp
(= (safe-div $x $y)
   (if (== $y 0)
       (error "division by zero" $y)
       (/ $x $y)))

; Without recovery - error propagates
!(safe-div 10 0)  ; Returns Error("division by zero", 0)

; With recovery - error caught
(catch !(safe-div 10 0) 0)  ; Returns 0
```

### Example 2: Conditional Error Handling

```lisp
(= (process $x)
   (if (is-error $x)
       (error "invalid input" $x)
       (compute $x)))

; Or use catch
(= (robust-process $x)
   (catch (process $x) "default-value"))
```

### Example 3: Metaprogramming with Quote/Eval

```lisp
; Store expression as data
(= (template) (quote (+ $x $y)))

; Evaluate stored expression
(eval (template))  ; Evaluates (+ $x $y) with current bindings
```

### Example 4: Complex Error Prevention

```lisp
; Multi-stage error handling
(if (is-error (catch (computation) (error "caught" 0)))
    "error-path"
    "success-path")
```

---

## Test Coverage

All reduction prevention features have comprehensive tests (7 tests total):

1. **`test_catch_with_error`**: Tests `catch` with an error value
2. **`test_catch_without_error`**: Tests `catch` when no error occurs
3. **`test_catch_prevents_error_propagation`**: Tests that `catch` prevents errors from propagating
4. **`test_eval_with_quote`**: Tests `eval` forcing evaluation of quoted expressions
5. **`test_is_error_with_error`**: Tests `is-error` detecting errors
6. **`test_is_error_with_normal_value`**: Tests `is-error` with normal values
7. **`test_reduction_prevention_combo`**: Tests complex combinations of reduction prevention features

**Run tests**: `cargo test`

---

## REPL Examples

```bash
$ mettatron --repl

metta[1]> (catch (error "test" 42) "recovered")
"recovered"

metta[2]> (is-error (error "test" 0))
true

metta[3]> (eval (quote (+ 1 2)))
3

metta[4]> (quote (+ 1 2))
(+ 1 2)

metta[5]> (+ 10 (catch (error "fail" 0) 5))
15
```

---

## Comparison with Issue Requirements

From GitHub Issue #3, "Later Features - Reduction Prevention":

| Requirement | Implementation | Status |
|-------------|----------------|--------|
| **Quotation support** | `quote` special form | ✅ Complete |
| **Error handling mechanisms** | `error`, `catch`, `is-error` | ✅ Complete |
| Explicit evaluation | `eval` special form | ✅ Bonus |
| Error recovery | `catch` prevents propagation | ✅ Complete |
| Error detection | `is-error` conditional | ✅ Complete |

**Status**: ✅ **FULLY IMPLEMENTED** with additional features beyond requirements

---

## Implementation Details

### Error Propagation Prevention

When `catch` encounters an error, it **stops error propagation** by:
1. Detecting the error in the result
2. Evaluating the default expression
3. Returning the default value instead of the error

This is the core "reduction prevention" mechanism - it prevents the error from reducing/propagating further through the evaluation.

### Lazy Evaluation

All special forms respect lazy evaluation:
- `quote` never evaluates its argument
- `eval` evaluates exactly once
- `catch` evaluates default only if error occurs
- `is-error` evaluates argument once to check status
- `error` does not evaluate details (reduction prevention)

### Environment Threading

All reduction prevention features properly thread the environment through evaluation, ensuring that side effects (rule definitions) are preserved even when errors occur or evaluation is prevented.

---

## Future Enhancements

Potential future improvements (not required by MVP):

1. **Try-catch with error binding**: `(try expr (catch $err handler))`
2. **Multiple catch clauses**: `(try expr (catch ErrorType1 h1) (catch ErrorType2 h2))`
3. **Finally clause**: `(try expr (catch $e h) (finally cleanup))`
4. **Error types**: Structured error types beyond string messages
5. **Stack traces**: Include evaluation context in errors

---

## Conclusion

The MeTTa Evaluator provides comprehensive reduction prevention mechanisms that satisfy and exceed the requirements from GitHub Issue #3. All features are:

- ✅ Fully implemented
- ✅ Thoroughly tested (7 dedicated tests)
- ✅ Documented with examples
- ✅ Working in REPL and library modes

The implementation provides fine-grained control over evaluation, robust error handling, and metaprogramming capabilities through quotation and explicit evaluation.
