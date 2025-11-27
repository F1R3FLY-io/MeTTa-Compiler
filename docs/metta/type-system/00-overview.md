# MeTTa Type System: Overview

## Executive Summary

This document provides a high-level overview of MeTTa's type system, based on analysis of the hyperon-experimental reference implementation. MeTTa features a sophisticated **gradual type system** that combines the flexibility of dynamic typing with the safety of static type checking.

## Key Characteristics

### 1. Gradual/Optional Typing

**MeTTa uses gradual typing** - types are optional and type checking can be selectively enabled.

```metta
; Without type checking - works like dynamic typing
(= (add $x $y) (+ $x $y))

; With type annotations
(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

; Enable type checking
!(pragma! type-check auto)
```

**Key Feature**: `%Undefined%` type acts as a universal type that matches anything, providing an escape hatch from strict typing.

### 2. Runtime Type Checking

**Type checking is OFF by default** and occurs at runtime when enabled:

```
Specification: Types are annotations; checking is optional
Implementation: Runtime checking controlled by pragma
```

**Enable with**:
```metta
!(pragma! type-check auto)
```

**When enabled**:
- Badly typed expressions produce `Error` atoms
- Well-typed expressions evaluate normally
- Type errors are caught before reduction

### 3. Rich Type Features

MeTTa supports advanced type system features rare in dynamically-typed languages:

| Feature | Support | Notes |
|---------|---------|-------|
| **Dependent Types** | ✓ Yes | Types can depend on values (e.g., `Vec $t $length`) |
| **Higher-Kinded Types** | ✓ Yes | Type constructors (e.g., `List :: Type -> Type`) |
| **Polymorphism** | ✓ Yes | Type variables (e.g., `$t`) with unification |
| **Subtyping** | ✓ Yes | Transitive subtype relations via `:<` |
| **Type Inference** | ✓ Yes | Automatic type inference for expressions |
| **Meta-Types** | ✓ Yes | Control evaluation strategy |

### 4. Type Syntax

**Type Assignment**:
```metta
(: atom Type)              ; Assigns Type to atom
(: Z Nat)                  ; Z has type Nat
(: S (-> Nat Nat))         ; S is a function Nat -> Nat
```

**Function Types**:
```metta
(-> ArgType1 ArgType2 ... ReturnType)

(: + (-> Number Number Number))           ; Binary addition
(: cons (-> $t (List $t) (List $t)))      ; Polymorphic list constructor
```

**Subtyping**:
```metta
(:< SubType SuperType)

(:< Int Number)            ; Int is a subtype of Number
(:< Nat Int)               ; Nat is a subtype of Int
```

**Polymorphic Types**:
```metta
(: map (-> (-> $a $b) (List $a) (List $b)))  ; map function with type variables
```

## Built-in Types

MeTTa provides the following built-in types:

### Core Types

1. **%Undefined%** - Default type for untyped atoms; matches any type
2. **Type** - The type of types (kind)
3. **Number** - Numeric values
4. **String** - String values
5. **Bool** - Boolean values (True/False)

### Structural Types

6. **Atom** - Any atom (universal type)
7. **Symbol** - Symbol atoms
8. **Variable** - Variable atoms
9. **Expression** - Expression atoms (lists)
10. **Grounded** - Grounded (embedded) atoms

### Special Types

11. **SpaceType** - Atom space type
12. **ErrorType** - Error atoms

### Function Type Constructor

13. **`->`** - Function type constructor (e.g., `(-> A B C)`)

## Type System Philosophy

### Design Principles

1. **Flexibility over Strictness**
   - Gradual typing allows mixing typed and untyped code
   - `%Undefined%` provides escape hatch
   - No forced type annotations

2. **Runtime over Compile-Time**
   - Type checking at runtime (when enabled)
   - Dynamic language ergonomics
   - No compilation phase required

3. **Expressiveness**
   - Dependent types for precise specifications
   - Higher-kinded types for abstraction
   - Polymorphism for code reuse

4. **Meta-Programming Support**
   - Meta-types control evaluation
   - `Atom` type prevents evaluation for meta-programming
   - Enables writing macros and transformations

5. **Optional Safety**
   - Enable type checking when desired
   - Disable for prototyping or dynamic code
   - Per-module or per-scope control

## Quick Reference

### Common Operations

**Get type of an atom**:
```metta
!(get-type atom)
```

**Check if atom has type**:
```metta
!(check-type atom Type)
```

**Cast to type**:
```metta
!(type-cast atom Type &space)
```

**Enable type checking**:
```metta
!(pragma! type-check auto)
```

### Type Declaration Examples

**Simple types**:
```metta
(: pi Number)
(: name String)
(: flag Bool)
```

**Function types**:
```metta
(: square (-> Number Number))
(: concat (-> String String String))
```

**Polymorphic types**:
```metta
(: identity (-> $t $t))
(: pair (-> $a $b (Pair $a $b)))
```

**Dependent types**:
```metta
(: Vec (-> Type Nat Type))
(: Cons (-> $t (Vec $t $n) (Vec $t (S $n))))
```

## Type Checking Behavior

### Without Type Checking (Default)

```metta
; Type annotations are ignored
(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

!(add 5 "hello")  ; Works (no checking) - may error at runtime in +
```

### With Type Checking Enabled

```metta
!(pragma! type-check auto)

(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

!(add 5 "hello")  ; Error: BadArgType
; Returns: (Error (add 5 "hello") (BadArgType 2 Number String))
```

## When to Use Types

### Recommended Use Cases

1. **Library Functions**: Public APIs benefit from type signatures
2. **Complex Data Structures**: Dependent types ensure correctness
3. **Critical Code**: Type checking catches errors early
4. **Documentation**: Types serve as machine-checked documentation

### When to Skip Types

1. **Prototyping**: Fast iteration without type overhead
2. **One-off Scripts**: Simple programs don't need types
3. **Dynamic Logic**: When types are genuinely unknown
4. **Meta-Programming**: When manipulating arbitrary atoms

## Document Structure

This documentation is organized into the following files:

1. **00-overview.md** (this file) - Executive summary
2. **01-fundamentals.md** - Type syntax, built-in types, basic concepts
3. **02-type-checking.md** - When/how checking occurs, inference, pragmas
4. **03-type-operations.md** - Operations like get-type, check-type
5. **04-gradual-typing.md** - %Undefined%, mixing typed/untyped code
6. **05-dependent-types.md** - Types depending on values
7. **06-advanced-features.md** - Higher-kinded types, meta-types, subtyping
8. **07-evaluation-interaction.md** - How types affect evaluation
9. **08-implementation.md** - Implementation details
10. **09-type-errors.md** - Error handling and debugging
11. **10-formal-semantics.md** - Formal type rules
12. **11-comparisons.md** - Comparisons with other type systems
13. **examples/*.metta** - Executable examples

## Key Concepts at a Glance

### Gradual Typing

**Concept**: Seamlessly mix typed and untyped code

**Mechanism**: `%Undefined%` type matches any type

**Benefit**: Incrementally add types to existing code

**See**: §04

### Dependent Types

**Concept**: Types can depend on runtime values

**Example**: `Vec $t $length` - vector type depends on length value

**Benefit**: Express precise invariants

**See**: §05

### Meta-Types

**Concept**: Five special types affect evaluation

**Types**: Atom, Symbol, Variable, Expression, Grounded

**Effect**: Arguments with `Atom` type are NOT evaluated

**Benefit**: Write meta-programming functions

**See**: §06, §07

### Type Inference

**Concept**: Automatically compute types from expressions

**Mechanism**:
- Query space for type annotations
- Infer function application types
- Propagate type variables through unification

**Benefit**: Types without annotations

**See**: §02

### Polymorphism

**Concept**: Functions work for multiple types

**Syntax**: Type variables like `$t`, `$a`, `$b`

**Example**: `(: map (-> (-> $a $b) (List $a) (List $b)))`

**Benefit**: Generic, reusable code

**See**: §01, §06

## Error Handling

MeTTa produces three main categories of type errors:

### 1. BadType

Type mismatch for entire expression:
```metta
(Error <expr> (BadType <expected> <actual>))
```

### 2. BadArgType

Wrong argument type in function application:
```metta
(Error <expr> (BadArgType <position> <expected> <actual>))
```

### 3. IncorrectNumberOfArguments

Argument count mismatch:
```metta
(Error <expr> IncorrectNumberOfArguments)
```

**See**: §09 for detailed error handling

## Performance Considerations

### Type Checking Overhead

**When disabled**: Zero overhead (types are just atoms)

**When enabled**:
- Runtime type queries for each reduction
- Type inference for expressions
- Type unification for polymorphic functions

**Recommendation**: Enable selectively for critical code

### Type Inference Cost

**Cost**: O(expression size × number of type annotations in space)

**Optimization**: Cache inferred types (not currently implemented)

## Comparison with Other Type Systems

| Feature | MeTTa | Haskell | TypeScript | Python (mypy) |
|---------|-------|---------|------------|---------------|
| **Timing** | Runtime | Compile-time | Compile-time | Static analysis |
| **Optional** | Yes | No | Yes | Yes |
| **Dependent Types** | Yes | No | No | No |
| **Gradual Typing** | Yes | No | Yes | Yes |
| **Type Inference** | Yes | Yes | Yes | Partial |
| **Soundness** | Partial | Yes | No | Partial |

**See**: §11 for detailed comparisons

## Implementation Summary

### Core Files

- **`lib/src/metta/types.rs`** (1320 lines) - Main type system implementation
- **`lib/src/metta/interpreter.rs:1126-1336`** - Runtime type checking
- **`lib/src/metta/runner/stdlib/atom.rs:354-447`** - Type operations

### Key Data Structures

```rust
pub struct AtomType {
    typ: Atom,           // The type
    is_function: bool,   // Is this a function type?
    info: TypeInfo,      // Value, Application, or Error
}
```

### Key Functions

- `get_atom_types()` - Infer types of an atom
- `check_type()` - Check if atom has given type
- `validate_atom()` - Check if atom is well-typed
- `match_reducted_types()` - Unify two types

**See**: §08 for implementation details

## Getting Started

### Basic Usage

```metta
; 1. Define types for your functions
(: factorial (-> Nat Nat))
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))

; 2. Enable type checking (optional)
!(pragma! type-check auto)

; 3. Use your functions
!(factorial 5)  ; Works
!(factorial -1) ; Type error if Nat excludes negative numbers
```

### Learning Path

1. Start with **§01-fundamentals** for basic syntax
2. Learn **§02-type-checking** for practical usage
3. Explore **§03-type-operations** for runtime introspection
4. Study **§04-gradual-typing** for flexibility
5. Advanced: **§05-dependent-types**, **§06-advanced-features**
6. Deep dive: **§10-formal-semantics**

## FAQ

**Q: Are types required?**
A: No, types are completely optional. MeTTa works fine without any type annotations.

**Q: What happens if I don't enable type checking?**
A: Type annotations are stored but not checked. Your code runs as if untyped.

**Q: Can I mix typed and untyped code?**
A: Yes! Use `%Undefined%` type for untyped parts. Gradual typing is a core feature.

**Q: Is MeTTa type-safe?**
A: Partially. With type checking enabled, badly-typed expressions produce errors. However, `%Undefined%` provides an escape hatch that can compromise safety.

**Q: Does type checking guarantee no runtime errors?**
A: No. Type checking catches type errors but doesn't prevent other runtime errors (division by zero, missing atoms in space, etc.).

**Q: How do dependent types work?**
A: Types can reference values. Example: `(Vec Number 3)` is the type of 3-element number vectors. See §05.

**Q: What are meta-types?**
A: Five special types (Atom, Symbol, Variable, Expression, Grounded) that control whether arguments are evaluated before function application. See §06 and §07.

## References

### Source Code (hyperon-experimental)

- **Main type system**: `lib/src/metta/types.rs`
- **Type checking integration**: `lib/src/metta/interpreter.rs`
- **Type operations**: `lib/src/metta/runner/stdlib/atom.rs`
- **Type constants**: `lib/src/metta/mod.rs`

### Test Files

- **Type basics**: `python/tests/scripts/b5_types_prelim.metta`
- **GADTs**: `python/tests/scripts/d1_gadt.metta`
- **Higher-order functions**: `python/tests/scripts/d2_higherfunc.metta`
- **Dependent types**: `python/tests/scripts/d3_deptypes.metta`
- **Types as propositions**: `python/tests/scripts/d4_type_prop.metta`
- **Auto type checking**: `python/tests/scripts/d5_auto_types.metta`

### Academic Background

MeTTa's type system draws inspiration from:
- **Dependent Types**: Idris, Agda, Coq
- **Gradual Typing**: TypeScript, Typed Racket
- **Meta-Types**: Reflection and meta-programming in Lisp

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
