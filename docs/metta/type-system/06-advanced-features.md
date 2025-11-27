# Advanced Type Features

## Abstract

MeTTa supports advanced type features including higher-kinded types, meta-types, and subtyping hierarchies.

## Higher-Kinded Types

### Type Constructors as Parameters

```metta
(: Functor (-> (-> Type Type) Type))  ; Takes type constructor

(: List (-> Type Type))     ; List :: Type -> Type
(: Maybe (-> Type Type))    ; Maybe :: Type -> Type

; Functor instance
(: FunctorList (Functor List))
```

### Examples from d1_gadt.metta

**Location**: `hyperon-experimental/python/tests/scripts/d1_gadt.metta`

```metta
(: EitherP (-> Type Type))
(: LeftP (-> $t (EitherP $t)))
(: RightP (-> Bool (EitherP $t)))
```

## Meta-Types

### The Five Meta-Types

**Location**: `hyperon-experimental/lib/src/metta/types.rs:606-617`

1. **Atom** - Universal meta-type
2. **Symbol** - Symbol atoms only
3. **Variable** - Variable atoms only
4. **Expression** - Expression atoms only
5. **Grounded** - Grounded atoms only

### Special Property

**Evaluation Control**: Arguments with `Atom` meta-type are NOT evaluated.

```metta
(: quote (-> Atom Atom))
(= (quote $expr) $expr)

!(quote (+ 1 2))  ; → (+ 1 2) (not evaluated)
```

See **§07** for detailed interaction with evaluation.

## Subtyping

### Subtype Declaration

```metta
(:< Nat Int)
(:< Int Number)
```

### Transitivity

**Location**: `hyperon-experimental/lib/src/metta/types.rs:34-63`

```rust
fn add_super_types(space: &DynSpace, sub_types: &mut Vec<Atom>, from: usize)
```

Subtyping is transitive:
```
If A <: B and B <: C, then A <: C
```

### Example from b5_types_prelim.metta

```metta
(:< A B)
(:< B C)
(:< C D)
(: a A)

!(get-type a)  ; → A, B, C, D (all supertypes)
```

## See Also

- **§01**: Type fundamentals
- **§05**: Dependent types
- **§07**: Evaluation interaction
- **hyperon-experimental/python/tests/scripts/d1_gadt.metta**: GADT examples
- **hyperon-experimental/python/tests/scripts/b5_types_prelim.metta**: Subtyping examples

---

**Version**: 1.0
**Last Updated**: 2025-11-13
