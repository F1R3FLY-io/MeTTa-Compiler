# Type System Fundamentals

## Abstract

This document provides a comprehensive foundation for understanding MeTTa's type system, including type syntax, built-in types, type constructors, polymorphism, and basic type operations. This is essential reading before exploring advanced features.

## Table of Contents

1. [Type Syntax](#type-syntax)
2. [Built-in Types](#built-in-types)
3. [Type Constructors](#type-constructors)
4. [Polymorphism](#polymorphism)
5. [Type Annotations](#type-annotations)
6. [Examples](#examples)

---

## Type Syntax

### Type Assignment Operator

**Syntax**: `(: <atom> <type>)`

**Purpose**: Assigns a type to an atom in the space.

**Location**: `hyperon-experimental/lib/src/metta/mod.rs:21`

```rust
pub const HAS_TYPE_SYMBOL : Atom = metta_const!(:);
```

**Examples**:
```metta
(: x Number)              ; x has type Number
(: name String)           ; name has type String
(: flag Bool)             ; flag has type Bool
(: Z Nat)                 ; Z has type Nat
```

**Formal Semantics**:
```
(: a T) adds the judgment "a : T" to the type environment
```

### Subtype Operator

**Syntax**: `(:< <subtype> <supertype>)`

**Purpose**: Declares that one type is a subtype of another.

**Location**: `hyperon-experimental/lib/src/metta/mod.rs:22`

```rust
pub const SUB_TYPE_SYMBOL : Atom = metta_const!(:<);
```

**Examples**:
```metta
(:< Nat Int)              ; Nat is a subtype of Int
(:< Int Number)           ; Int is a subtype of Number
(:< A B)                  ; A is a subtype of B
```

**Transitivity**: Subtyping is transitive:
```metta
(:< A B)
(:< B C)
; Implies: A is also a subtype of C
```

**Formal Semantics**:
```
(:< S T) adds the judgment "S <: T" to the subtyping environment
If S <: T and T <: U, then S <: U (transitive)
```

### Function Type Constructor

**Syntax**: `(-> <arg-type>... <return-type>)`

**Purpose**: Constructs function types.

**Examples**:
```metta
(-> Number Number)              ; Function: Number → Number
(-> Number Number Number)       ; Function: Number → Number → Number
(-> String Bool)                ; Function: String → Bool
(-> $t $t)                      ; Identity function type (polymorphic)
```

**Variadic**: Function types can have any number of argument types.

**Formal Notation**:
```
τ₁ → τ₂ → ... → τₙ → τᵣ

Where:
  τ₁, ..., τₙ are argument types
  τᵣ is the return type
```

**Reading Function Types**:
```metta
(-> A B C D)
```
Can be read as:
- "Function taking A, B, C and returning D"
- In curried form: A → (B → (C → D))

### Type Variables

**Syntax**: `$identifier`

**Purpose**: Represent polymorphic (generic) types.

**Examples**:
```metta
$t              ; A type variable
$a              ; Another type variable
$elem           ; Descriptive type variable name
```

**Usage in Types**:
```metta
(: identity (-> $t $t))                    ; For any type t
(: pair (-> $a $b (Pair $a $b)))          ; For any types a and b
(: map (-> (-> $a $b) (List $a) (List $b))) ; Multiple type variables
```

---

## Built-in Types

### Core Atomic Types

#### %Undefined%

**Location**: `lib/src/metta/mod.rs:13`

```rust
pub const ATOM_TYPE_UNDEFINED : Atom = metta_const!(%Undefined%);
```

**Purpose**: Default type for untyped atoms; universal type in gradual typing.

**Key Property**: `%Undefined%` matches ANY type.

**Usage**:
```metta
; Atom without type annotation has %Undefined%
(= x 42)
!(get-type x)  ; → %Undefined%

; Functions returning unknown types
(: unknown-function (-> Atom %Undefined%))
```

**Gradual Typing**:
```metta
; Can pass %Undefined% where any type is expected
(: takes-number (-> Number Number))
(= (takes-number $x) (* $x 2))

; If x has %Undefined% type, this works (with type checking disabled)
!(takes-number x)
```

**See**: §04 for detailed discussion of gradual typing.

#### Type

**Location**: `lib/src/metta/mod.rs:14`

```rust
pub const ATOM_TYPE_TYPE : Atom = metta_const!(Type);
```

**Purpose**: The type of types (kind).

**Examples**:
```metta
(: Number Type)           ; Number is a type
(: String Type)           ; String is a type
(: Bool Type)             ; Bool is a type
(: Nat Type)              ; Nat is a type
```

**Type Constructors**:
```metta
(: List (-> Type Type))   ; List :: Type → Type
; List takes a type and returns a type
```

**Higher-Kinded Types**:
```metta
(: Functor (-> (-> Type Type) Type))
; Functor takes a type constructor and returns a type
```

#### Number

**Purpose**: Numeric values (integers and floating-point).

**Examples**:
```metta
(: pi Number)
(: count Number)
(: 42 Number)
(: 3.14159 Number)
```

**Operations**:
```metta
(: + (-> Number Number Number))
(: - (-> Number Number Number))
(: * (-> Number Number Number))
(: / (-> Number Number Number))
```

#### String

**Purpose**: Text values.

**Examples**:
```metta
(: "hello" String)
(: name String)
```

**Operations**:
```metta
(: concat (-> String String String))
```

#### Bool

**Purpose**: Boolean values (True/False).

**Examples**:
```metta
(: True Bool)
(: False Bool)
(: flag Bool)
```

**Operations**:
```metta
(: and (-> Bool Bool Bool))
(: or (-> Bool Bool Bool))
(: not (-> Bool Bool))
```

### Structural Types

These types correspond to the internal structure of atoms.

#### Atom

**Location**: `lib/src/metta/mod.rs:15`

```rust
pub const ATOM_TYPE_ATOM : Atom = metta_const!(Atom);
```

**Purpose**: Universal type - matches any atom.

**Special Property**: When used as argument type in function, prevents evaluation (see §07).

**Examples**:
```metta
(: any-value Atom)        ; Can be any atom
```

**Meta-Programming**:
```metta
(: quote (-> Atom Atom))  ; Takes unevaluated expression
```

#### Symbol

**Location**: `lib/src/metta/mod.rs:16`

```rust
pub const ATOM_TYPE_SYMBOL : Atom = metta_const!(Symbol);
```

**Purpose**: Symbol atoms only.

**Examples**:
```metta
foo                       ; Symbol
bar                       ; Symbol
my-symbol                 ; Symbol
```

**Usage**:
```metta
(: my-symbol Symbol)
```

#### Variable

**Location**: `lib/src/metta/mod.rs:17`

```rust
pub const ATOM_TYPE_VARIABLE : Atom = metta_const!(Variable);
```

**Purpose**: Variable atoms only.

**Examples**:
```metta
$x                        ; Variable
$name                     ; Variable
```

**Usage**:
```metta
(: $var Variable)
```

#### Expression

**Location**: `lib/src/metta/mod.rs:18`

```rust
pub const ATOM_TYPE_EXPRESSION : Atom = metta_const!(Expression);
```

**Purpose**: Expression (list) atoms only.

**Examples**:
```metta
(foo bar)                 ; Expression
(+ 1 2)                   ; Expression
()                        ; Empty expression
```

**Usage**:
```metta
(: my-expr Expression)
```

#### Grounded

**Location**: `lib/src/metta/mod.rs:19`

```rust
pub const ATOM_TYPE_GROUNDED : Atom = metta_const!(Grounded);
```

**Purpose**: Grounded (embedded Rust) atoms.

**Examples**:
- Numbers (42, 3.14)
- Strings ("hello")
- Custom grounded types

**Usage**:
```metta
(: my-grounded Grounded)
```

### Special Types

#### SpaceType

**Purpose**: Type of atom spaces.

**Usage**:
```metta
(: &space SpaceType)
```

**Operations**:
```metta
(: add-atom (-> SpaceType Atom %Undefined%))
(: remove-atom (-> SpaceType Atom %Undefined%))
```

#### ErrorType

**Purpose**: Type of error atoms.

**Format**:
```metta
(Error <atom> <error-details>)
```

**Examples**:
```metta
(Error (+ 1 "x") (BadArgType 2 Number String))
```

---

## Type Constructors

### Definition

**Type Constructor**: A type-level function that takes types as arguments and produces a type.

**Kind**: Type constructors have kinds like:
- `Type → Type` (unary)
- `Type → Type → Type` (binary)
- etc.

### Defining Type Constructors

**Syntax**:
```metta
(: Constructor (-> Type ... Type Type))
```

**Examples**:

#### Unary Type Constructor

```metta
(: List (-> Type Type))
; List :: Type → Type
; List takes a type and returns a type

(: Maybe (-> Type Type))
; Maybe :: Type → Type
```

**Usage**:
```metta
(List Number)         ; List of Numbers
(List String)         ; List of Strings
(Maybe Bool)          ; Maybe Bool
```

#### Binary Type Constructor

```metta
(: Pair (-> Type Type Type))
; Pair :: Type → Type → Type
; Pair takes two types and returns a type

(: Either (-> Type Type Type))
; Either :: Type → Type → Type
```

**Usage**:
```metta
(Pair Number String)  ; Pair of Number and String
(Either Bool Number)  ; Either Bool or Number
```

#### Higher-Order Type Constructor

```metta
(: Functor (-> (-> Type Type) Type))
; Functor :: (Type → Type) → Type
; Functor takes a type constructor and returns a type
```

### Parameterized Data Types

**Pattern**: Define constructors with types that reference the type parameters.

**Example: List Type**

```metta
; Type constructor
(: List (-> Type Type))

; Data constructors
(: Nil (List $t))                              ; Empty list
(: Cons (-> $t (List $t) (List $t)))           ; Cons cell

; Usage
(: numbers (List Number))
(= numbers (Cons 1 (Cons 2 (Cons 3 Nil))))

!(get-type numbers)  ; → (List Number)
```

**Example: Pair Type**

```metta
; Type constructor
(: Pair (-> Type Type Type))

; Data constructor
(: MkPair (-> $a $b (Pair $a $b)))

; Usage
(: my-pair (Pair Number String))
(= my-pair (MkPair 42 "hello"))
```

**Example: Maybe Type**

```metta
; Type constructor
(: Maybe (-> Type Type))

; Data constructors
(: Nothing (Maybe $t))
(: Just (-> $t (Maybe $t)))

; Usage
(: result (Maybe Number))
(= result (Just 42))

(: no-result (Maybe String))
(= no-result Nothing)
```

---

## Polymorphism

### Type Variables

**Purpose**: Enable writing generic code that works for multiple types.

**Syntax**: Type variables start with `$`.

**Scope**: Type variables are implicitly quantified over the entire type expression.

**Example**:
```metta
(: identity (-> $t $t))
```
This means: "For all types t, identity is a function from t to t."

**Formal Notation**:
```
∀t. t → t
```

### Polymorphic Functions

#### Identity Function

```metta
(: identity (-> $t $t))
(= (identity $x) $x)

!(identity 42)        ; → 42 (with type Number)
!(identity "hello")   ; → "hello" (with type String)
!(identity True)      ; → True (with type Bool)
```

**Type Inference**:
- When `identity` is called with `42`, type variable `$t` is inferred as `Number`
- When called with `"hello"`, `$t` is inferred as `String`

#### Pair Function

```metta
(: pair (-> $a $b (Pair $a $b)))
(= (pair $x $y) (MkPair $x $y))

!(pair 1 "one")       ; → (MkPair 1 "one") : Pair Number String
!(pair True False)    ; → (MkPair True False) : Pair Bool Bool
```

#### Map Function

```metta
(: map (-> (-> $a $b) (List $a) (List $b)))
(= (map $f Nil) Nil)
(= (map $f (Cons $x $xs)) (Cons ($f $x) (map $f $xs)))

; Usage
(: double (-> Number Number))
(= (double $x) (* $x 2))

!(map double (Cons 1 (Cons 2 (Cons 3 Nil))))
; → (Cons 2 (Cons 4 (Cons 6 Nil))) : List Number
```

### Type Unification

**Purpose**: Determine how type variables should be instantiated to make types compatible.

**Example**:
```metta
(: id (-> $t $t))
!(id 42)
```

**Unification Process**:
1. Function type: `$t → $t`
2. Argument type: `Number`
3. Unify `$t` with `Number`
4. Result type: `Number`

**Multiple Type Variables**:
```metta
(: pair (-> $a $b (Pair $a $b)))
!(pair 1 "one")
```

**Unification**:
1. Function type: `$a → $b → Pair $a $b`
2. First argument: `Number` → unify `$a` with `Number`
3. Second argument: `String` → unify `$b` with `String`
4. Result type: `Pair Number String`

### Polymorphic Constraints

**Implicit Constraints**: Type variables can be constrained by their usage.

**Example**:
```metta
(: add-numbers (-> $t $t $t))
(= (add-numbers $x $y) (+ $x $y))
```

Here, `$t` is implicitly constrained to `Number` because `+` requires numbers.

### Parametric Polymorphism

**MeTTa uses parametric polymorphism**: Type variables are instantiated uniformly.

**Contrast with Ad-hoc Polymorphism** (overloading):
- MeTTa: Same implementation for all types
- Ad-hoc: Different implementations for different types

**Example**:
```metta
(: identity (-> $t $t))
(= (identity $x) $x)
```
Same implementation works for all types.

---

## Type Annotations

### Annotating Atoms

**Simple Values**:
```metta
(: pi Number)
(= pi 3.14159)

(: greeting String)
(= greeting "Hello, World!")

(: flag Bool)
(= flag True)
```

### Annotating Functions

**Monomorphic Functions**:
```metta
(: square (-> Number Number))
(= (square $x) (* $x $x))

(: concat (-> String String String))
(= (concat $s1 $s2) (concat-string $s1 $s2))
```

**Polymorphic Functions**:
```metta
(: identity (-> $t $t))
(= (identity $x) $x)

(: const (-> $a $b $a))
(= (const $x $y) $x)
```

### Annotating Data Constructors

```metta
; Natural numbers
(: Z Nat)
(: S (-> Nat Nat))

; Lists
(: Nil (List $t))
(: Cons (-> $t (List $t) (List $t)))

; Binary trees
(: Leaf (Tree $t))
(: Node (-> $t (Tree $t) (Tree $t) (Tree $t)))
```

---

## Examples

### Example 1: Natural Numbers

```metta
; Define natural number type
(: Nat Type)

; Constructors
(: Z Nat)
(: S (-> Nat Nat))

; Values
(: zero Nat)
(= zero Z)

(: one Nat)
(= one (S Z))

(: two Nat)
(= two (S (S Z)))

; Addition
(: add (-> Nat Nat Nat))
(= (add Z $n) $n)
(= (add (S $m) $n) (S (add $m $n)))

!(add two (S zero))  ; → (S (S (S Z)))
```

### Example 2: Polymorphic List Operations

```metta
; Type constructor
(: List (-> Type Type))

; Constructors
(: Nil (List $t))
(: Cons (-> $t (List $t) (List $t)))

; Length function
(: length (-> (List $t) Nat))
(= (length Nil) Z)
(= (length (Cons $x $xs)) (S (length $xs)))

; Append function
(: append (-> (List $t) (List $t) (List $t)))
(= (append Nil $ys) $ys)
(= (append (Cons $x $xs) $ys) (Cons $x (append $xs $ys)))

; Usage
(: nums (List Number))
(= nums (Cons 1 (Cons 2 (Cons 3 Nil))))

!(length nums)  ; → (S (S (S Z)))
```

### Example 3: Maybe Type

```metta
; Type constructor
(: Maybe (-> Type Type))

; Constructors
(: Nothing (Maybe $t))
(: Just (-> $t (Maybe $t)))

; Safe division
(: safe-div (-> Number Number (Maybe Number)))
(= (safe-div $x 0) Nothing)
(= (safe-div $x $y) (Just (/ $x $y)))

!(safe-div 10 2)  ; → (Just 5)
!(safe-div 10 0)  ; → Nothing
```

### Example 4: Binary Tree

```metta
; Type constructor
(: Tree (-> Type Type))

; Constructors
(: Leaf (Tree $t))
(: Node (-> $t (Tree $t) (Tree $t) (Tree $t)))

; Tree of numbers
(: my-tree (Tree Number))
(= my-tree (Node 5
             (Node 3 Leaf Leaf Leaf)
             (Node 7 Leaf Leaf Leaf)
             Leaf))

; Size function
(: tree-size (-> (Tree $t) Nat))
(= (tree-size Leaf) Z)
(= (tree-size (Node $v $l $r $extra))
   (S (add (tree-size $l) (add (tree-size $r) (tree-size $extra)))))

!(tree-size my-tree)  ; → (S (S (S Z)))
```

### Example 5: Higher-Order Functions

```metta
; Compose
(: compose (-> (-> $b $c) (-> $a $b) (-> $a $c)))
(= (compose $f $g) (lambda $x ($f ($g $x))))

; Apply
(: apply (-> (-> $a $b) $a $b))
(= (apply $f $x) ($f $x))

; Usage
(: inc (-> Number Number))
(= (inc $x) (+ $x 1))

(: double (-> Number Number))
(= (double $x) (* $x 2))

(: inc-then-double (-> Number Number))
(= inc-then-double (compose double inc))

!(inc-then-double 5)  ; → 12
```

---

## Type Equivalence

### Structural Equivalence

Types are equivalent if they have the same structure:

```metta
(List Number) ≡ (List Number)           ; Same
(Pair Number String) ≡ (Pair Number String)  ; Same
```

### Type Variable Renaming

Types are equivalent up to consistent renaming of type variables:

```metta
(-> $a $a) ≡ (-> $b $b)                 ; Alpha-equivalent
(-> $a $b (Pair $a $b)) ≡ (-> $x $y (Pair $x $y))  ; Alpha-equivalent
```

**Not Equivalent**:
```metta
(-> $a $a) ≢ (-> $a $b)                 ; Different structure
```

---

## Best Practices

### Naming Conventions

**Type Names**: Use CamelCase
```metta
(: Number Type)
(: List Type)
(: Maybe Type)
```

**Type Variables**: Use descriptive lowercase names with `$`
```metta
$t          ; Generic type
$elem       ; Element type
$key        ; Key type
$value      ; Value type
$a, $b, $c  ; Multiple type variables
```

**Function Names**: Use kebab-case
```metta
(: add-numbers (-> Number Number Number))
(: safe-div (-> Number Number (Maybe Number)))
```

### Type Annotation Guidelines

**Always Annotate**:
- Public API functions
- Complex polymorphic functions
- Data constructors

**Optional Annotation**:
- Local bindings
- Simple helper functions
- When type is obvious from context

**Example**:
```metta
; Public API - always annotate
(: process-data (-> (List Number) Number))
(= (process-data $data) ...)

; Local helper - annotation optional
(= (sum-helper $acc Nil) $acc)
(= (sum-helper $acc (Cons $x $xs)) (sum-helper (+ $acc $x) $xs))
```

---

## See Also

- **§00**: Overview of type system
- **§02**: Type checking and inference
- **§04**: Gradual typing with %Undefined%
- **§05**: Dependent types
- **§06**: Advanced features (higher-kinded types, meta-types)

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
