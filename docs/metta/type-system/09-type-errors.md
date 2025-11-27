# Type Errors and Debugging

## Abstract

This document covers type errors, error messages, and debugging strategies for MeTTa's type system.

## Error Types

### Three Main Error Categories

**Location**: `hyperon-experimental/lib/src/metta/mod.rs:26-28`

```rust
pub const BAD_TYPE_SYMBOL : Atom = metta_const!(BadType);
pub const BAD_ARG_TYPE_SYMBOL : Atom = metta_const!(BadArgType);
pub const INCORRECT_NUMBER_OF_ARGUMENTS_SYMBOL : Atom = metta_const!(IncorrectNumberOfArguments);
```

### 1. BadType

**Format**:
```metta
(Error <expr> (BadType <expected> <actual>))
```

**Example**:
```metta
!(pragma! type-check auto)
(: x Number)
(= x "hello")  ; String assigned to Number

!(get-type x)
; → (Error x (BadType Number String))
```

### 2. BadArgType

**Format**:
```metta
(Error <expr> (BadArgType <index> <expected> <actual>))
```

**Example**:
```metta
!(pragma! type-check auto)
(: add (-> Number Number Number))

!(add 5 "hello")
; → (Error (add 5 "hello") (BadArgType 2 Number String))
;    Position 2 (second argument) expected Number, got String
```

### 3. IncorrectNumberOfArguments

**Format**:
```metta
(Error <expr> IncorrectNumberOfArguments)
```

**Example**:
```metta
!(pragma! type-check auto)
(: add (-> Number Number Number))

!(add 5)
; → (Error (add 5) IncorrectNumberOfArguments)
```

## Debugging Strategies

### Strategy 1: Enable Type Checking Selectively

```metta
; Narrow down problem area
!(pragma! type-check auto)
; Test specific functions
!(my-function test-input)
```

### Strategy 2: Use get-type for Inspection

```metta
; Check inferred types
!(get-type problematic-expr)

; Check function signature
!(get-type my-function)
```

### Strategy 3: Check Meta-Types

```metta
; Verify structural type
!(get-metatype mysterious-value)
```

### Strategy 4: Gradual Type Introduction

```metta
; Start without types
(= (my-func $x) ...)

; Add types incrementally
(: my-func (-> $t $t))  ; Start general

; Refine types
(: my-func (-> Number Number))  ; Make specific

; Enable checking
!(pragma! type-check auto)
```

## Common Error Patterns

### Pattern 1: Unification Failure

**Problem**: Type variables don't unify as expected.

```metta
(: pair-same (-> $t $t (Pair $t $t)))
!(pair-same 1 "hello")
; Error: Can't unify Number with String for $t
```

**Solution**: Use different type variables if types can differ.

```metta
(: pair (-> $a $b (Pair $a $b)))
!(pair 1 "hello")  ; Works
```

### Pattern 2: Missing Type Annotations

**Problem**: Type inference fails without annotations.

```metta
(= x 42)  ; No type → %Undefined%
!(check-type x Number)  ; May not work as expected
```

**Solution**: Add explicit type annotations.

```metta
(: x Number)
(= x 42)
```

### Pattern 3: Evaluation Order Confusion

**Problem**: Argument evaluated when it shouldn't be.

```metta
(: quote-like (-> Number Number))  ; Wrong! Should be Atom
(= (quote-like $expr) $expr)

!(quote-like (+ 1 2))  ; → 3 (evaluated!)
```

**Solution**: Use `Atom` meta-type.

```metta
(: quote-like (-> Atom Atom))
(= (quote-like $expr) $expr)

!(quote-like (+ 1 2))  ; → (+ 1 2) (not evaluated)
```

## See Also

- **§02**: Type checking
- **§03**: Type operations for debugging
- **hyperon-experimental/python/tests/scripts/d5_auto_types.metta**: Error examples

---

**Version**: 1.0
**Last Updated**: 2025-11-13
