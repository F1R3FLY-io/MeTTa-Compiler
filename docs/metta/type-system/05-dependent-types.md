# Dependent Types

## Abstract

MeTTa supports dependent types where types can depend on runtime values. This enables expressing precise invariants like vector lengths, bounded numbers, and type-level computation.

## Table of Contents

1. [Dependent Type Fundamentals](#dependent-type-fundamentals)
2. [Value Dependencies](#value-dependencies)
3. [Type-Level Computation](#type-level-computation)
4. [Examples](#examples)

---

## Dependent Type Fundamentals

### Definition

**Dependent Type**: A type that can reference values.

**Examples**:
```metta
(Vec $t $n)          ; Vector of type $t with length $n
(Bounded $min $max)  ; Number between $min and $max
(Array $t $size)     ; Array of type $t with size $size
```

### Test File

**Location**: `hyperon-experimental/python/tests/scripts/d3_deptypes.metta`

This file demonstrates dependent types extensively.

---

## Value Dependencies

### Example: Length-Indexed Vectors

**Type Definition**:
```metta
(: Vec (-> Type Nat Type))  ; Vec depends on element type AND length
```

**Constructors**:
```metta
(: Nil (Vec $t Z))                              ; Empty vector has length Z
(: Cons (-> $t (Vec $t $n) (Vec $t (S $n))))   ; Cons increments length
```

**Usage**:
```metta
; Type signature shows exact length
(: my-vec (Vec Number (S (S Z))))  ; Vector of 2 Numbers
(= my-vec (Cons 1 (Cons 2 Nil)))

!(get-type my-vec)
; → (Vec Number (S (S Z)))
```

### Type-Safe Operations

**Head Function** (requires non-empty vector):
```metta
(: head (-> (Vec $t (S $n)) $t))  ; Requires S $n (not Z)
(= (head (Cons $x $xs)) $x)

!(head (Cons 1 (Cons 2 Nil)))  ; Works: length is S (S Z)
; !(head Nil)  ; Type error: Nil has type (Vec $t Z), not (Vec $t (S $n))
```

**Tail Function**:
```metta
(: tail (-> (Vec $t (S $n)) (Vec $t $n)))  ; Decrements length
(= (tail (Cons $x $xs)) $xs)

!(tail (Cons 1 (Cons 2 Nil)))
; → (Cons 2 Nil) : Vec Number (S Z)
```

---

## Type-Level Computation

### Natural Number Type

```metta
(: Nat Type)
(: Z Nat)
(: S (-> Nat Nat))
```

### Addition at Type Level

```metta
(: add-nat (-> Nat Nat Nat))
(= (add-nat Z $n) $n)
(= (add-nat (S $m) $n) (S (add-nat $m $n)))

; Use in types
(: append (-> (Vec $t $m) (Vec $t $n) (Vec $t (add-nat $m $n))))
```

### Dependent Pairs (Σ-types)

```metta
(: DPair (-> Type (-> $a Type) Type))
(: MkDPair (-> $a ($p $a) (DPair $a $p)))

; Example: Pair of number and vector of that length
(: num-and-vec (DPair Nat (lambda $n (Vec Number $n))))
(= num-and-vec (MkDPair (S (S Z)) (Cons 1 (Cons 2 Nil))))
```

---

## Examples

### Example 1: Safe Array Access

```metta
(: Fin (-> Nat Type))  ; Finite type: numbers < n
(: FZ (Fin (S $n)))    ; Zero is in Fin (S n)
(: FS (-> (Fin $n) (Fin (S $n))))  ; Increment index

(: get (-> (Vec $t $n) (Fin $n) $t))  ; Index must be < length
(= (get (Cons $x $xs) FZ) $x)
(= (get (Cons $x $xs) (FS $i)) (get $xs $i))

; Safe: index is within bounds
!(get (Cons 1 (Cons 2 (Cons 3 Nil))) (FS FZ))  ; → 2
```

### Example 2: Matrix Operations

```metta
(: Matrix (-> Type Nat Nat Type))  ; Matrix with dimensions

(: mat-mult
   (-> (Matrix Number $m $n)
       (Matrix Number $n $p)
       (Matrix Number $m $p)))  ; Types enforce dimension compatibility
```

### Example 3: Bounded Numbers

```metta
(: InRange (-> Nat Nat Nat Type))  ; Number in range [low, high]

(: bounded-add
   (-> (InRange $low1 $high1 $n1)
       (InRange $low2 $high2 $n2)
       (InRange (add-nat $low1 $low2) (add-nat $high1 $high2) (add-nat $n1 $n2))))
```

---

## See Also

- **§01**: Type fundamentals
- **§06**: Advanced features (higher-kinded types)
- **hyperon-experimental/python/tests/scripts/d3_deptypes.metta**: Complete examples

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
