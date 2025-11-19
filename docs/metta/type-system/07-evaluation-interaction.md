# Types and Evaluation Interaction

## Abstract

Meta-types in MeTTa control whether arguments are evaluated before being passed to functions, enabling meta-programming.

## Evaluation Strategy

**Default (from minimal-metta.md:44-51)**: Applicative order - arguments evaluated before function application.

**Exception**: Arguments with `Atom` meta-type are NOT evaluated.

## Meta-Type Effect

### Atom Meta-Type

```metta
(: quote (-> Atom Atom))  ; Atom type
(= (quote $expr) $expr)

!(quote (+ 1 2))  ; → (+ 1 2) (UNEVALUATED)
```

### Other Meta-Types

```metta
(: process (-> Number Number))  ; Number type (not Atom)
(= (process $x) (* $x 2))

!(process (+ 1 2))  ; → 6 (argument EVALUATED to 3 first)
```

## Implementation

**Location**: `hyperon-experimental/lib/src/metta/types.rs:606-639`

```rust
pub fn check_meta_type(typ: &Atom) -> MetaType {
    match typ {
        Atom::Symbol(s) if s.name() == "Atom" => MetaType::NoEval,
        _ => MetaType::Eval,
    }
}
```

## Examples

### Example 1: Meta-Programming

```metta
(: eval (-> Atom %Undefined%))
(= (eval $expr) ...)  ; Evaluates expression

!(eval (quote (+ 1 2)))
; Step 1: quote receives (+ 1 2) UNEVALUATED
; Step 2: quote returns (+ 1 2)
; Step 3: eval receives (+ 1 2) UNEVALUATED
; Step 4: eval evaluates it → 3
```

### Example 2: Macros

```metta
(: unless (-> Atom Atom Atom %Undefined%))
(= (unless $cond $then)
   (if (not $cond) $then ()))

; Both arguments receive UNEVALUATED expressions
!(unless (< x 10) (print "x is large"))
```

## See Also

- **§01**: Type fundamentals
- **§06**: Meta-types
- **../order-of-operations/01-evaluation-order.md**: Detailed evaluation semantics

---

**Version**: 1.0
**Last Updated**: 2025-11-13
