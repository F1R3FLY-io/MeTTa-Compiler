# Gradual Typing

## Abstract

This document explains MeTTa's gradual typing system, where the `%Undefined%` type serves as a universal type allowing seamless mixing of typed and untyped code. Gradual typing provides flexibility while maintaining optional type safety.

## Table of Contents

1. [The %Undefined% Type](#the-undefined-type)
2. [Gradual Typing Principles](#gradual-typing-principles)
3. [Mixing Typed and Untyped Code](#mixing-typed-and-untyped-code)
4. [Type Safety Considerations](#type-safety-considerations)
5. [Examples](#examples)

---

## The %Undefined% Type

### Definition

**Location**: `hyperon-experimental/lib/src/metta/mod.rs:13`

```rust
pub const ATOM_TYPE_UNDEFINED : Atom = metta_const!(%Undefined%);
```

**Purpose**: Universal type that matches any other type.

**Default**: Atoms without explicit type annotations have `%Undefined%` type.

### Key Property

```
%Undefined% ≡ T  (for any type T)
```

This means `%Undefined%` can unify with Number, String, Bool, or any other type.

### Examples

```metta
; Atom without type annotation
(= x 42)
!(get-type x)  ; → %Undefined%

; Can be used anywhere
(: add (-> Number Number Number))
!(add x 5)  ; Works: %Undefined% unifies with Number
```

---

## Gradual Typing Principles

### Principle 1: Types Are Optional

**You can write MeTTa without any type annotations**:

```metta
; No types - works fine
(= (factorial $n)
   (if (== $n 0)
       1
       (* $n (factorial (- $n 1)))))

!(factorial 5)  ; → 120
```

### Principle 2: Add Types Incrementally

**Add types gradually as code matures**:

```metta
; Step 1: Untyped
(= (double $x) (* $x 2))

; Step 2: Add partial types
(: double (-> Number Number))
(= (double $x) (* $x 2))

; Step 3: Enable checking
!(pragma! type-check auto)
```

### Principle 3: Undefined as Escape Hatch

**Use `%Undefined%` explicitly when type is truly unknown**:

```metta
; Function that works for any type
(: store-anything (-> %Undefined% Database %Undefined%))
(= (store-anything $value $db) ...)
```

### Principle 4: Static Guarantees Where Needed

**Enable type checking for critical code**:

```metta
; Critical financial calculations - enable type checking
!(pragma! type-check auto)

(: calculate-interest (-> Number Number Number Number))
(= (calculate-interest $principal $rate $time)
   (* $principal (* $rate $time)))
```

---

## Mixing Typed and Untyped Code

### Pattern 1: Typed Functions, Untyped Data

```metta
; Typed function interface
(: process (-> Data Result))

; But Data itself might be untyped
(= data-from-user (parse-input user-input))  ; No type on data
!(process data-from-user)  ; Works: data has %Undefined% type
```

### Pattern 2: Gradual Migration

**Start untyped**:
```metta
(= (map $f $list)
   (if (== $list Nil)
       Nil
       (Cons ($f (car $list)) (map $f (cdr $list)))))
```

**Add types to public interface**:
```metta
(: map (-> (-> $a $b) (List $a) (List $b)))
(= (map $f $list)
   (if (== $list Nil)
       Nil
       (Cons ($f (car $list)) (map $f (cdr $list)))))
```

**Enable checking when ready**:
```metta
!(pragma! type-check auto)
```

### Pattern 3: Type Boundaries

**Define type boundaries between modules**:

```metta
; Module A: Strictly typed
!(pragma! type-check auto)

(: public-api (-> Input Output))
(= (public-api $input)
   (internal-helper (validate-input $input)))

; Module B: Dynamically typed
; No pragma - type checking disabled

(= (experimental-feature $x)
   ; Rapid prototyping without type constraints
   ...)
```

---

## Type Safety Considerations

### When %Undefined% Helps

**1. Rapid Prototyping**:
```metta
; Quickly test ideas without type overhead
(= (experiment $x $y $z)
   ; Complex logic, types unclear
   ...)
```

**2. Truly Dynamic Data**:
```metta
; Data from external sources
(= (process-json $json)
   ; JSON structure may vary
   ...)
```

**3. Meta-Programming**:
```metta
; Functions that manipulate arbitrary atoms
(= (transform $expr)
   ; Works on any expression structure
   ...)
```

### When %Undefined% Can Be Dangerous

**1. Type Errors Slip Through**:
```metta
; Without type checking
(: add (-> Number Number Number))
(= untyped-value "not a number")  ; %Undefined%

!(add untyped-value 5)  ; No error until runtime in +
```

**2. Loss of Documentation**:
```metta
; What types does this function expect?
(= (mysterious-function $x $y) ...)  ; Unclear!
```

**3. Harder Debugging**:
```metta
; Error occurs far from source
(= data (complex-pipeline input))  ; %Undefined%
; ... many steps later ...
!(process data)  ; Error here - but problem was earlier
```

### Best Practices

**DO**:
- ✅ Use types for public APIs
- ✅ Type check critical code with pragma
- ✅ Document when %Undefined% is intentional
- ✅ Add types before enabling pragma

**DON'T**:
- ❌ Rely on %Undefined% in type-checked code
- ❌ Use %Undefined% to avoid fixing type errors
- ❌ Leave large codebases untyped permanently

---

## Examples

### Example 1: Gradual Type Introduction

```metta
; Phase 1: Completely untyped
(= (safe-div $x $y)
   (if (== $y 0)
       Nothing
       (Just (/ $x $y))))

!(safe-div 10 2)  ; Works

; Phase 2: Add types
(: safe-div (-> Number Number (Maybe Number)))
(= (safe-div $x $y)
   (if (== $y 0)
       Nothing
       (Just (/ $x $y))))

!(safe-div 10 2)  ; Still works, now typed

; Phase 3: Enable checking
!(pragma! type-check auto)
!(safe-div 10 2)   ; Type-checked
!(safe-div 10 "x") ; Error: (BadArgType 2 Number String)
```

### Example 2: Type Boundaries

```metta
; Library: Strictly typed interface
(: process-data (-> (List Number) Number))
(= (process-data $data)
   ; Implementation can use types
   (sum-list $data))

; User code: Can be untyped
(= my-data (load-from-file "data.txt"))  ; %Undefined%

; Works: %Undefined% unifies with (List Number)
!(process-data my-data)
```

### Example 3: Escape Hatch

```metta
!(pragma! type-check auto)

; Function requiring specific types
(: strict-func (-> Number String Bool))
(= (strict-func $n $s) ...)

; But sometimes need to pass dynamic data
(= dynamic-value (parse-input user-input))  ; %Undefined%

; Explicitly cast to bypass checking
!(strict-func
   (type-cast dynamic-value Number &self)
   (type-cast dynamic-value String &self))
```

### Example 4: Meta-Programming

```metta
; Meta-programming functions work on any atom
(: quote (-> Atom Atom))  ; Atom type, not %Undefined%
(= (quote $expr) $expr)

; Works for any type of expression
!(quote 42)              ; → 42
!(quote "hello")         ; → "hello"
!(quote (foo bar baz))   ; → (foo bar baz)
```

### Example 5: Mixed Type Environment

```metta
; Some functions typed
(: fibonacci (-> Nat Nat))
(= (fibonacci Z) (S Z))
(= (fibonacci (S Z)) (S Z))
(= (fibonacci (S (S $n)))
   (+ (fibonacci (S $n)) (fibonacci $n)))

; Some functions untyped
(= (helper-func $x) ...)  ; No type

; Some use gradual typing
(= partial-data (load-data))  ; %Undefined%
!(fibonacci partial-data)  ; Works if data is actually Nat
```

---

## Comparison with Other Gradual Systems

### TypeScript

**TypeScript**:
- `any` type is similar to `%Undefined%`
- Compile-time gradual typing
- Structural typing

**MeTTa**:
- `%Undefined%` type
- Runtime gradual typing
- Nominal + structural typing

### Typed Racket

**Typed Racket**:
- Mix typed and untyped modules
- Contracts at boundaries
- Blame tracking

**MeTTa**:
- Mix typed and untyped code freely
- No automatic contracts
- Pragma-based checking

### Python (mypy)

**Python + mypy**:
- Optional type hints
- Static analysis tool
- No runtime checking

**MeTTa**:
- Optional type annotations
- Runtime checking when enabled
- Dynamic by default

---

## See Also

- **§01**: Type fundamentals
- **§02**: Type checking mechanisms
- **§03**: Type operations
- **§09**: Error handling

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
