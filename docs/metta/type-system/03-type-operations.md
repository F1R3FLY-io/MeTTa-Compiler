# Type Operations

## Abstract

This document provides a comprehensive reference for runtime type operations in MeTTa, including querying types, checking types, casting types, and inspecting meta-types. These operations enable runtime type introspection and dynamic type checking.

## Table of Contents

1. [get-type](#get-type)
2. [get-type-space](#get-type-space)
3. [get-metatype](#get-metatype)
4. [check-type](#check-type)
5. [type-cast](#type-cast)
6. [validate-atom](#validate-atom)
7. [Examples](#examples)

---

## get-type

### Signature

```metta
(: get-type (-> Atom %Undefined%))
```

### Purpose

Returns the type(s) of an atom in the current space.

### Location

`hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs:358-380`

```rust
impl CustomExecute for GetTypeOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let arg_error = || ExecError::from("get-type expects single atom as an argument");
        let atom = args.get(0).ok_or_else(arg_error)?;
        let space = args.get(1).ok_or_else(arg_error)?;
        let space = Atom::as_gnd::<DynSpace>(space).ok_or("...")?;

        Ok(get_atom_types(&space.borrow(), atom).into_iter()
            .map(|t| t.into()).collect())
    }
}
```

### Behavior

**Returns**:
- Empty if atom is badly typed or has no type
- One or more types if atom is well-typed
- May return multiple types if atom has multiple type annotations

**Examples**:

```metta
; Simple type query
(: x Number)
!(get-type x)  ; → Number

; Multiple types (via subtyping)
(:< Nat Int)
(:< Int Number)
(: y Nat)
!(get-type y)  ; → Nat (or also Int, Number via subtyping)

; Polymorphic function
(: identity (-> $t $t))
!(get-type identity)  ; → (-> $t $t)

; Untyped atom
(= z 42)  ; No type annotation
!(get-type z)  ; → %Undefined%

; Grounded type
!(get-type 42)  ; → Number
!(get-type "hello")  ; → String
!(get-type True)  ; → Bool
```

### Type Inference

`get-type` uses the same type inference algorithm as type checking (see §02):

1. Query space for explicit `(: atom Type)` declarations
2. For expressions, compute application types
3. Include supertypes via `:<` relations
4. Return all inferred types

---

## get-type-space

### Signature

```metta
(: get-type-space (-> SpaceType Atom Atom))
```

### Purpose

Returns the type of an atom in a specific space (not the current space).

### Location

`hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs:382-403`

### Parameters

1. **space**: The space to query (SpaceType)
2. **atom**: The atom whose type to get

### Examples

```metta
; Create a new space
!(bind! &my-space (new-space))

; Add type annotation in that space
!(add-atom &my-space (: x Number))

; Get type from specific space
!(get-type-space &my-space x)  ; → Number

; Different from current space
(: x String)  ; In current space
!(get-type x)  ; → String (current space)
!(get-type-space &my-space x)  ; → Number (my-space)
```

### Use Cases

- Working with multiple spaces
- Module systems with separate type namespaces
- Sandboxed type environments

---

## get-metatype

### Signature

```metta
(: get-metatype (-> Atom Atom))
```

### Purpose

Returns the structural meta-type of an atom (Atom, Symbol, Variable, Expression, or Grounded).

### Location

`hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs:405-447`

```rust
impl CustomExecute for GetMetaTypeOp {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let arg_error = || ExecError::from("get-metatype expects single atom as an argument");
        let atom = args.get(0).ok_or_else(arg_error)?;
        Ok(vec![Atom::expr([ATOM_TYPE_SYMBOL, get_meta_type(atom)])])
    }
}

fn get_meta_type(atom: &Atom) -> Atom {
    match atom {
        Atom::Symbol(_) => ATOM_TYPE_SYMBOL,
        Atom::Variable(_) => ATOM_TYPE_VARIABLE,
        Atom::Expression(_) => ATOM_TYPE_EXPRESSION,
        Atom::Grounded(_) => ATOM_TYPE_GROUNDED,
    }
}
```

### Meta-Types

Five possible meta-types:

1. **Symbol** - Symbol atoms (e.g., `foo`, `bar`)
2. **Variable** - Variable atoms (e.g., `$x`, `$y`)
3. **Expression** - Expression atoms (e.g., `(foo bar)`)
4. **Grounded** - Grounded atoms (e.g., `42`, `"hello"`)
5. **Atom** - Universal (matches any of the above)

### Examples

```metta
!(get-metatype foo)         ; → Symbol
!(get-metatype $x)          ; → Variable
!(get-metatype (foo bar))   ; → Expression
!(get-metatype 42)          ; → Grounded
!(get-metatype "hello")     ; → Grounded
```

### Distinction from get-type

**get-type**: Returns semantic type (e.g., Number, String, List Number)

**get-metatype**: Returns structural type (Symbol, Variable, Expression, Grounded)

```metta
(: x Number)

!(get-type x)       ; → Number (semantic type)
!(get-metatype x)   ; → Symbol (structural type)

!(get-type 42)      ; → Number (semantic type)
!(get-metatype 42)  ; → Grounded (structural type)
```

---

## check-type

### Signature

```metta
(: check-type (-> Atom Type Bool))
```

### Purpose

Checks if an atom has a given type.

### Location

`hyperon-experimental/lib/src/metta/types.rs:602-639`

```rust
pub fn check_type(space: &DynSpace, atom: &Atom, typ: &Atom) -> bool {
    let atom_types = get_atom_types(space, atom);
    for atom_type in atom_types {
        if match_reducted_types(&atom_type.typ, typ).next().is_some() {
            return true;
        }
    }
    false
}
```

### Algorithm

1. Get all types of the atom
2. For each type, try to unify with the given type
3. Return `true` if any unification succeeds
4. Return `false` otherwise

### Examples

```metta
(: x Number)

!(check-type x Number)   ; → True
!(check-type x String)   ; → False

; With subtyping
(:< Nat Number)
(: y Nat)

!(check-type y Nat)      ; → True
!(check-type y Number)   ; → True (via subtyping)
!(check-type y String)   ; → False

; Polymorphic types
(: identity (-> $t $t))

!(check-type identity (-> Number Number))   ; → True ($t = Number)
!(check-type identity (-> String String))   ; → True ($t = String)

; %Undefined% matches everything
(= z 42)  ; No type annotation → %Undefined%

!(check-type z Number)   ; → True (%Undefined% matches Number)
!(check-type z String)   ; → True (%Undefined% matches String)
```

---

## type-cast

### Signature

```metta
(: type-cast (-> Atom Type SpaceType %Undefined%))
```

### Purpose

Attempts to cast an atom to a specific type. Returns the atom if type check succeeds, or an Error atom if it fails.

### Location

Defined in standard library

### Parameters

1. **atom**: The atom to cast
2. **type**: The target type
3. **space**: The space for type checking

### Behavior

**Success**: Returns the atom unchanged (but type-checked)

**Failure**: Returns `(Error <atom> (BadType <expected> <actual>))`

### Examples

```metta
(: x Number)
(= x 42)

; Successful cast
!(type-cast x Number &self)  ; → 42

; Failed cast
!(type-cast x String &self)
; → (Error 42 (BadType String Number))

; Cast with polymorphic type
(: identity (-> $t $t))

!(type-cast identity (-> Number Number) &self)   ; → identity
!(type-cast identity (-> String String) &self)   ; → identity
```

### Use Cases

- Runtime type assertions
- Defensive programming
- Type-directed dispatch

---

## validate-atom

### Signature

```metta
(: validate-atom (-> Atom Bool))
```

### Purpose

Checks if an atom is well-typed (no type errors).

### Location

`hyperon-experimental/lib/src/metta/types.rs:641-658`

```rust
pub fn validate_atom(space: &DynSpace, atom: &Atom) -> bool {
    let types = get_atom_types(space, atom);
    for typ in types {
        if typ.is_error() {
            return false;
        }
    }
    !types.is_empty()
}
```

### Algorithm

1. Get all types of the atom
2. Check if any type is an error type
3. Return `false` if errors found or no types
4. Return `true` otherwise

### Examples

```metta
; Well-typed atom
(: x Number)
!(validate-atom x)  ; → True

; Badly-typed expression
!(pragma! type-check auto)
(: bad-expr (+ 1 "hello"))  ; Type error
!(validate-atom bad-expr)  ; → False

; Untyped atom (still valid)
(= y 42)
!(validate-atom y)  ; → True (%Undefined% is valid)
```

---

## Examples

### Example 1: Type Introspection

```metta
; Define a function with type
(: double (-> Number Number))
(= (double $x) (* $x 2))

; Introspect its type
!(get-type double)       ; → (-> Number Number)
!(get-metatype double)   ; → Symbol

; Check specific types
!(check-type double (-> Number Number))   ; → True
!(check-type double (-> String String))   ; → False
```

### Example 2: Runtime Type Checking

```metta
; Function that checks argument types at runtime
(= (safe-call $f $x)
   (if (check-type $f (-> Number Number))
       (if (check-type $x Number)
           ($f $x)
           (Error "Argument is not a Number"))
       (Error "Function has wrong type")))

(: double (-> Number Number))
(= (double $x) (* $x 2))

!(safe-call double 5)        ; → 10
!(safe-call double "hello")  ; → (Error "Argument is not a Number")
```

### Example 3: Type-Directed Dispatch

```metta
; Dispatch based on type
(= (process $x)
   (if (check-type $x Number)
       (process-number $x)
       (if (check-type $x String)
           (process-string $x)
           (process-other $x))))

(= (process-number $n) (* $n 2))
(= (process-string $s) (concat $s "!"))
(= (process-other $x) $x)

!(process 42)       ; → 84
!(process "hello")  ; → "hello!"
!(process True)     ; → True
```

### Example 4: Working with Multiple Spaces

```metta
; Create separate type environments
!(bind! &space1 (new-space))
!(bind! &space2 (new-space))

; Define x differently in each space
!(add-atom &space1 (: x Number))
!(add-atom &space2 (: x String))

; Query types in different spaces
!(get-type-space &space1 x)  ; → Number
!(get-type-space &space2 x)  ; → String

; Current space might have yet another type
(: x Bool)
!(get-type x)  ; → Bool
```

### Example 5: Validating Complex Structures

```metta
!(pragma! type-check auto)

; Define types
(: List (-> Type Type))
(: Nil (List $t))
(: Cons (-> $t (List $t) (List $t)))

; Valid list
(: valid-list (List Number))
(= valid-list (Cons 1 (Cons 2 (Cons 3 Nil))))
!(validate-atom valid-list)  ; → True

; Invalid list (type mismatch)
(: invalid-list (List Number))
(= invalid-list (Cons 1 (Cons "two" (Cons 3 Nil))))  ; String in Number list
!(validate-atom invalid-list)  ; → False
```

---

## Performance Considerations

### Type Query Cost

**get-type**:
- Cost: O(number of type annotations in space)
- Requires querying space for `(: atom Type)` patterns
- May involve multiple pattern matches

**get-metatype**:
- Cost: O(1)
- Simple pattern match on atom structure
- Very fast

**check-type**:
- Cost: O(number of types × unification cost)
- Must unify each type with target
- Unification cost depends on type complexity

### Optimization Strategies

**Cache Types**:
```metta
; Instead of repeated get-type calls
(= (process $x)
   (let $t (get-type $x)
        ; Use $t multiple times
        ...))
```

**Use Meta-Types for Fast Dispatch**:
```metta
; Fast structural check
(= (dispatch $x)
   (if (== (get-metatype $x) Symbol)
       (handle-symbol $x)
       (handle-other $x)))
```

---

## See Also

- **§01**: Type fundamentals
- **§02**: Type checking and inference
- **§04**: Gradual typing
- **§06**: Meta-types and their special properties
- **§09**: Error handling

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
