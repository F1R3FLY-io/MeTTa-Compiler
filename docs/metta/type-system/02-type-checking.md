# Type Checking

## Abstract

This document specifies when and how type checking occurs in MeTTa, including type inference mechanisms, the pragma system for controlling type checking, and type unification. Understanding these mechanisms is essential for practical use of MeTTa's type system.

## Table of Contents

1. [Type Checking Control](#type-checking-control)
2. [Type Inference](#type-inference)
3. [Type Unification](#type-unification)
4. [Runtime Type Checking](#runtime-type-checking)
5. [Examples](#examples)

---

## Type Checking Control

### Pragma System

**Default State**: Type checking is **OFF** by default.

**Enable Type Checking**:
```metta
!(pragma! type-check auto)
```

**Location**: `hyperon-experimental/lib/src/metta/runner/stdlib/stdlib.metta:1215`

**Effect**:
- When enabled: Expressions are type-checked before reduction
- When disabled: Type annotations stored but not checked

### Behavior Modes

**Without Type Checking** (default):
```metta
(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

!(add 5 "hello")  ; Executes, may fail at runtime in +
```

**With Type Checking** (after pragma):
```metta
!(pragma! type-check auto)

(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

!(add 5 "hello")  ; Returns: (Error (add 5 "hello") (BadArgType 2 Number String))
```

---

## Type Inference

### Inference Algorithm

**Location**: `hyperon-experimental/lib/src/metta/types.rs:327-410`

**Function**: `get_atom_types(space: &DynSpace, atom: &Atom) -> Vec<AtomType>`

**Algorithm**:

1. **Variables**: Return empty vector (can unify with anything)

2. **Grounded Atoms**: Query `gnd.type_()` method
   ```rust
   Atom::Grounded(gnd) => vec![AtomType::value(gnd.type_())]
   ```

3. **Symbols**: Query space for `(: symbol Type)` declarations
   ```rust
   query(space, (= (: symbol $X) ()))
   ```

4. **Empty Expressions**: Return `%Undefined%`

5. **Non-Empty Expressions**:
   - **Tuple Type**: Construct from component types
   - **Application Type**: If operator has function type, check application

### Inference Examples

**Example 1: Simple Symbol**:
```metta
(: x Number)

; Inference for x:
; 1. Query space for (: x $X)
; 2. Find: (: x Number)
; 3. Result: Number
```

**Example 2: Function Application**:
```metta
(: add (-> Number Number Number))

; Inference for (add 3 4):
; 1. Get type of add: (-> Number Number Number)
; 2. Get type of 3: Number
; 3. Get type of 4: Number
; 4. Check 3 : Number ✓
; 5. Check 4 : Number ✓
; 6. Result: Number
```

**Example 3: Polymorphic Function**:
```metta
(: identity (-> $t $t))

; Inference for (identity 42):
; 1. Get type of identity: (-> $t $t)
; 2. Get type of 42: Number
; 3. Unify $t with Number
; 4. Instantiate return type: $t becomes Number
; 5. Result: Number
```

---

## Type Unification

### Unification Process

**Location**: `hyperon-experimental/lib/src/metta/types.rs:563-567`

**Function**: `match_reducted_types(left: &Atom, right: &Atom) -> MatchResultIter`

**Purpose**: Determine how to bind type variables to make two types compatible.

### Unification Rules

**Rule 1: Identical Types**
```
T ≡ T   (always succeeds)

Number ≡ Number → success, no bindings
```

**Rule 2: Variable Unification**
```
$t ≡ T → {$t ↦ T}

$t ≡ Number → {$t ↦ Number}
```

**Rule 3: %Undefined% Universal**
```
%Undefined% ≡ T → success (for any T)
T ≡ %Undefined% → success (for any T)
```

**Rule 4: Structural Unification**
```
(C T₁ ... Tₙ) ≡ (C S₁ ... Sₙ)
requires: T₁ ≡ S₁, ..., Tₙ ≡ Sₙ

(List Number) ≡ (List Number) → success
(List Number) ≡ (List String) → fail
```

### Unification Examples

**Example 1: Simple Unification**:
```metta
; Unify: (-> $t $t) with (-> Number Number)
; Result: {$t ↦ Number}
```

**Example 2: Multiple Variables**:
```metta
; Unify: (-> $a $b (Pair $a $b)) with (-> Number String (Pair Number String))
; Step 1: $a ≡ Number → {$a ↦ Number}
; Step 2: $b ≡ String → {$b ↦ String}
; Step 3: (Pair $a $b) ≡ (Pair Number String) → apply bindings
; Result: {$a ↦ Number, $b ↦ String}
```

**Example 3: Occurs Check**:
```metta
; Unify: $t with (List $t)
; Fails: $t occurs in (List $t) → infinite type
```

---

## Runtime Type Checking

### When Checking Occurs

**Location**: `hyperon-experimental/lib/src/metta/interpreter.rs:1126-1159`

**Trigger**: During interpretation, before attempting reduction.

**Process**:
1. Get types of expression and expected type
2. Check if types unify
3. If success: proceed with reduction
4. If failure: return Error atom

### Type Checking Function

**Location**: `hyperon-experimental/lib/src/metta/interpreter.rs:1161-1336`

**Function**: `check_if_function_type_is_applicable()`

**Steps**:
1. Extract function type from operator
2. Check argument count matches
3. For each argument:
   - Get argument's actual type
   - Get expected type from function signature
   - Check if types unify
4. If all checks pass: compute return type
5. If any check fails: return error

### Error Generation

**Three Error Types**:

1. **BadType**: Entire expression has wrong type
   ```metta
   (Error <expr> (BadType <expected> <actual>))
   ```

2. **BadArgType**: Specific argument has wrong type
   ```metta
   (Error <expr> (BadArgType <index> <expected> <actual>))
   ```

3. **IncorrectNumberOfArguments**: Argument count mismatch
   ```metta
   (Error <expr> IncorrectNumberOfArguments)
   ```

---

## Examples

### Example 1: Type Checking Disabled (Default)

```metta
; Define typed function
(: square (-> Number Number))
(= (square $x) (* $x $x))

; Use with correct type
!(square 5)  ; → 25

; Use with wrong type (NO ERROR - checking disabled)
!(square "hello")  ; May error in *, but not caught by type checker
```

### Example 2: Type Checking Enabled

```metta
; Enable type checking
!(pragma! type-check auto)

; Define typed function
(: square (-> Number Number))
(= (square $x) (* $x $x))

; Use with correct type
!(square 5)  ; → 25

; Use with wrong type (ERROR - checking enabled)
!(square "hello")
; → (Error (square "hello") (BadArgType 1 Number String))
```

### Example 3: Polymorphic Type Inference

```metta
!(pragma! type-check auto)

; Define polymorphic identity
(: identity (-> $t $t))
(= (identity $x) $x)

; Infer for Number
!(identity 42)  ; $t unified with Number → result: 42 : Number

; Infer for String
!(identity "hello")  ; $t unified with String → result: "hello" : String

; Infer for complex type
!(identity (Cons 1 Nil))  ; $t unified with (List Number) → result: (Cons 1 Nil) : List Number
```

### Example 4: Type Error Detection

```metta
!(pragma! type-check auto)

; Binary function
(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

; Correct usage
!(add 3 4)  ; → 7

; Type error: wrong argument type
!(add 3 "four")
; → (Error (add 3 "four") (BadArgType 2 Number String))

; Type error: too few arguments
!(add 3)
; → (Error (add 3) IncorrectNumberOfArguments)
```

### Example 5: Gradual Typing with %Undefined%

```metta
!(pragma! type-check auto)

; Function expecting Number
(: double (-> Number Number))
(= (double $x) (* $x 2))

; Atom with %Undefined% type
(= mystery 42)  ; No type annotation → %Undefined%

; Works: %Undefined% unifies with Number
!(double mystery)  ; → 84 (type checking passes)
```

---

## See Also

- **§01**: Type fundamentals
- **§03**: Type operations (get-type, check-type)
- **§04**: Gradual typing details
- **§09**: Error handling

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
