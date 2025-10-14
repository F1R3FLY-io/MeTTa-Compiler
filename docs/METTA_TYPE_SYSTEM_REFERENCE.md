# MeTTa Type System: Reference from Official Implementation

This document describes MeTTa's actual type system based on the official `hyperon-experimental` implementation at https://github.com/trueagi-io/hyperon-experimental

## Overview

MeTTa's type system is **optional but powerful**, supporting everything from simple type assertions to dependent types with length-indexed data structures.

### Key Characteristics

1. **Optional**: Types can be specified or inferred, but are not required
2. **Gradual**: Can mix typed and untyped code
3. **Dependent**: Types can depend on values (e.g., `Vec n` - vector of length n)
4. **Inferred**: Automatic type inference with `!(pragma! type-check auto)`
5. **Pattern-based**: Type checking uses pattern matching

## Type System Components

### 1. Type Assertions

**Syntax**: `(: expr Type)`

**Examples**:
```lisp
; Basic type assertions
(: Socrates Entity)
(: 5 Number)
(: "hello" String)

; Function type assertions
(: add (-> Number Number Number))
(: map (-> (-> $a $b) (List $a) (List $b)))
```

**In Python tests**:
```python
# Adding type assertions to space
space.add_atom(E(S(":"), S("a"), S("A")))
space.add_atom(E(S(":"), S("foo"), E(S("->"), S("A"), S("B"))))
```

### 2. Built-in Type Functions

#### `get-type`

Returns the type(s) of an expression.

```lisp
!(get-type (Cons 5 (Cons 6 Nil)))
; Returns: (List Number)

!(get-type (LeftP 5))
; Returns: (EitherP Number)

!(get-type 42)
; Returns: Number
```

**Pattern matching with get-type**:
```lisp
; Extract type parameter
(let (List $t) (get-type (Cons 5 (Cons 6 Nil))) $t)
; Returns: Number
```

#### `check-type`

Validates if an atom has a specific type.

```python
# Python API
check_type(space, S("a"), S("A"))  # Check if 'a' has type 'A'
check_type(space, S("a"), AtomType.UNDEFINED)  # Check if undefined
```

#### `validate-atom`

Validates an atom based on type constraints.

```python
# Python API
validate_atom(space, E(S("foo"), S("a")))
# Validates 'foo' applied to 'a' based on type signatures
```

#### `get-atom-types`

Retrieves all types associated with an atom.

```python
# Python API
get_atom_types(space, S("foo"))
# Returns: [E(S("->"), S("A"), S("B"))]

get_atom_types(space, E(S("foo"), S("a")))
# Returns: [S("B")]  (return type after application)
```

### 3. Automatic Type Checking

**Enable**:
```lisp
!(pragma! type-check auto)
```

**Effects**:
- Automatic type inference on all expressions
- Type errors on mismatched types
- Type checking for function applications

**Examples**:
```lisp
!(pragma! type-check auto)

; Type error - mixing Number and String
!(+ 5 "S")
; Error: Expected Number, got String

; Type error - comparison between different types
!(== 5 "S")
; Error: Type mismatch

; OK - compatible types
!(+ 5 3)
; Returns: 8
```

**Limitations**:
- `match` does not perform strict type checking
- `let` bindings can create type-incompatible expressions
- Some operations bypass type checking

### 4. Arrow Types (Function Types)

**Syntax**: `(-> ArgType1 ArgType2 ... ReturnType)`

**Examples**:
```lisp
; Simple function
(: add (-> Number Number Number))

; Polymorphic function
(: id (-> $t $t))

; Higher-order function
(: map (-> (-> $a $b) (List $a) (List $b)))

; Multi-argument function
(: foo (-> A B C D))  ; Takes A, B, C, returns D
```

**Grounded operations with types**:
```python
# Python registration
metta.register_atom("id_num",
    OperationAtom("id_num", lambda x: x, ['Number', 'Number']))

# Polymorphic
metta.register_atom("id_poly",
    OperationAtom("id_poly", lambda x: [x], ['$t', '$t']))
```

### 5. Parameterized Types

Type constructors that take type parameters.

**Examples**:
```lisp
; List type constructor
(: List (-> Type Type))
(: Nil (List $t))
(: Cons (-> $t (List $t) (List $t)))

; Either type constructor
(: EitherP (-> Type Type))
(: LeftP (-> $t (EitherP $t)))
(: RightP (-> $t (EitherP $t)))

; Usage
!(get-type (Cons 5 (Cons 6 Nil)))
; Returns: (List Number)

!(get-type (LeftP "hello"))
; Returns: (EitherP String)
```

**Type checking prevents mixing**:
```lisp
!(get-type (Cons 5 (Cons "6" Nil)))
; Returns: Empty set (type error - mixed Number and String)
```

### 6. Dependent Types

Types that depend on **values**, not just other types.

#### Natural Number Indexing

```lisp
; Define natural numbers at type level
(: Nat Type)
(: Z Nat)
(: S (-> Nat Nat))

; Vector indexed by length
(: Vec (-> Type Nat Type))
(: Nil (Vec $t Z))
(: Cons (-> $t (Vec $t $n) (Vec $t (S $n))))

; Usage
!(get-type (Cons 0 (Cons 1 Nil)))
; Returns: (Vec Number (S (S Z)))

; Length-aware operations
(: drop (-> (Vec $t (S $n)) (Vec $t $n)))

!(drop (Cons 1 Nil))
; Returns: Nil
; Type: (Vec Number Z)
```

**Type safety**:
```lisp
; Cannot drop from empty vector
!(drop Nil)
; Type error: Expected (Vec $t (S $n)), got (Vec $t Z)
```

#### Numeric Length Indexing

```lisp
; Vector with numeric length
(: VecN (-> Type Number Type))
(: NilN (VecN $t 0))
(: ConsN (-> $t (VecN $t $n) (VecN $t (+ $n 1))))

; Usage
!(get-type (ConsN "1" (ConsN "2" NilN)))
; Returns: (VecN String 2)

; Length-changing operations
(: dropN (-> (VecN $t $n) (VecN $t (- $n 1))))

!(dropN (ConsN "1" NilN))
; Returns: NilN
; Type: (VecN String 0)
```

**Benefits**:
- Prevents index out of bounds at type level
- Encodes invariants in types
- Compiler can verify correctness

### 7. Type Variables

Variables in types start with `$`.

**Examples**:
```lisp
; Polymorphic identity
(: id (-> $t $t))

; Polymorphic list operations
(: map (-> (-> $a $b) (List $a) (List $b)))
(: filter (-> (-> $a Bool) (List $a) (List $a)))

; Constrained type variables
(: add (-> Number Number Number))  ; Specific type
(: append (-> (List $t) (List $t) (List $t)))  ; Polymorphic
```

### 8. Type Checking Behavior

#### Untyped Symbols

```lisp
; Untyped symbols prevent reduction but don't error
!(id_num untyp)
; Returns: (id_num untyp)  (unevaluated)
```

#### Typed Symbols

```lisp
; Typed symbols cause type errors
(: myAtom String)
!(id_num myAtom)
; Error: Expected Number, got String
```

#### Type Inference

```lisp
; Infer from usage
(= (double $x) (* $x 2))

; If * requires Numbers, double is inferred as (-> Number Number)
!(get-type double)
; Returns: (-> Number Number)
```

## Implementation Architecture

### Type Representation

From `atoms.py`:

```python
class AtomType:
    UNDEFINED = 0
    TYPE = 1
    ATOM = 2
    SYMBOL = 3
    VARIABLE = 4
    EXPRESSION = 5
    GROUNDED = 6
```

### Type Sugar Function

Converts various representations to type atoms:

```python
def _type_sugar(typ):
    """Convert type representation to Atom

    - None -> UNDEFINED
    - '$x' -> Variable
    - 'Atom' -> Symbol
    - [types...] -> Arrow type expression
    """
    if typ is None:
        return None
    if isinstance(typ, list):
        # Convert to arrow type: [A, B, C] -> (-> A B C)
        return E(S("->"), *[_type_sugar(t) for t in typ])
    if isinstance(typ, str):
        if typ[0] == '$':
            return V(typ)  # Variable
        else:
            return S(typ)  # Symbol
    return typ
```

### Grounded Types

Grounded atoms (implemented in Rust/Python) have types:

```python
# Create grounded atom with type
ValueAtom(5, "Number")

# Create grounded operation with type signature
OperationAtom("add", add_fn, ["Number", "Number", "Number"])

# Get type of grounded atom
atom.get_grounded_type()  # Returns type atom
```

## Type System Features Summary

| Feature | Difficulty | Supported | Example |
|---------|-----------|-----------|---------|
| **Type assertions** | Easy | ✅ Yes | `(: x Number)` |
| **Arrow types** | Easy | ✅ Yes | `(-> A B C)` |
| **Type variables** | Medium | ✅ Yes | `$t`, `$a` |
| **Polymorphism** | Medium | ✅ Yes | `(-> $t $t)` |
| **Type checking** | Medium | ✅ Yes | `check-type`, `validate-atom` |
| **Type inference** | Hard | ✅ Yes | `get-type`, auto-inference |
| **Parameterized types** | Hard | ✅ Yes | `(List $t)` |
| **Dependent types** | Very Hard | ✅ Yes | `(Vec $t $n)` |
| **GADTs** | Very Hard | ✅ Yes | With type constructors |

## Implementation Roadmap for Our Evaluator

### Phase 1: Basic Type Assertions (1 week)

**What to implement**:
```rust
// Add Type variant
enum MettaValue {
    // ... existing
    Type(String),
}

// Store type assertions
struct Environment {
    rules: Vec<Rule>,
    type_assertions: HashMap<String, MettaValue>,
}

// Special form: (: expr Type)
":" => {
    if items.len() >= 3 {
        let name = extract_name(&items[1]);
        let typ = items[2].clone();
        env.add_type(name, typ);
        return (vec![MettaValue::Nil], env);
    }
}
```

**Add built-in `get-type`**:
```rust
"get-type" => {
    if items.len() >= 2 {
        let expr = &items[1];
        let typ = infer_type(expr, &env)?;
        return (vec![typ], env);
    }
}
```

### Phase 2: Arrow Types (1 week)

**What to implement**:
```rust
enum MettaType {
    Named(String),           // Number, Bool, etc.
    Var(String),            // $t, $a, etc.
    Arrow(Vec<MettaType>),  // (-> A B C)
    App(Box<MettaType>, Vec<MettaType>), // (List Number)
}

// Parse arrow types
fn parse_arrow_type(sexpr: &MettaValue) -> Result<MettaType> {
    match sexpr {
        MettaValue::SExpr(items) if items[0] == Atom("->") => {
            let types = items[1..].map(parse_type).collect();
            Ok(MettaType::Arrow(types))
        }
        // ...
    }
}

// Check function application
fn check_application(func_type: &MettaType, args: &[MettaValue]) -> Result<MettaType>
```

### Phase 3: Type Checking (2 weeks)

**What to implement**:
```rust
// Check if expression has expected type
fn check_type(expr: &MettaValue, expected: &MettaType, env: &Environment)
    -> Result<(), TypeError>

// Infer type of expression
fn infer_type(expr: &MettaValue, env: &Environment)
    -> Result<MettaType, TypeError>

// Unification for type variables
fn unify(t1: &MettaType, t2: &MettaType)
    -> Result<Substitution, TypeError>
```

### Phase 4: Dependent Types (4-8 weeks)

**What to implement**:
- Normalize type expressions (evaluate in type context)
- Conversion checking (are two types equal?)
- Bidirectional checking (inference + checking modes)
- Type-level computation

**This requires expert knowledge** - defer unless needed.

## Key Insights from Official Implementation

### 1. Types Are Optional

You can:
- Write completely untyped code
- Add types gradually
- Mix typed and untyped code

### 2. Types Are First-Class

Types are atoms, just like values:
```lisp
(: Type Type)  ; Type has type Type
(: Number Type)  ; Number is a Type
```

### 3. Pattern Matching Does Type Checking

Type checking uses the same pattern matching engine:
```lisp
; Check if x has type (List Number)
(let (List Number) (get-type $x) ...)
```

### 4. Pragmatic Approach

- Start with simple type assertions
- Add `get-type` for inspection
- Optionally enable auto-checking
- Dependent types for when you need them

### 5. Error Messages

Type errors show:
- Expected type
- Actual type
- Location of mismatch

## Recommendations

### For MVP: Minimal Types ✅

**Implement**:
1. Type assertions: `(: x Type)`
2. `get-type` function
3. Basic type checking in eval

**Skip** (for now):
- Automatic inference
- Dependent types
- Complex unification

**Time**: 1-2 weeks

### For Production: Add Inference

**Implement**:
1. Everything in MVP
2. `check-type` validation
3. Optional `!(pragma! type-check auto)`
4. Arrow type checking

**Time**: 4-6 weeks total

### For Research: Full System

**Implement**:
1. Everything above
2. Dependent types (Vec n)
3. Type-level computation
4. Proof capabilities

**Time**: 3-6 months
**Requires**: Type theory expert

## Conclusion

The official MeTTa implementation shows that:

1. **Type system is optional** - Start simple, add complexity as needed
2. **Types are atoms** - Reuse existing pattern matching
3. **Gradual typing** - Mix typed and untyped code
4. **Pragmatic design** - Simple things are simple, complex things are possible

**Recommendation**: Start with Phase 1-2 (basic types + arrow types) in 2 weeks. This gives 80% of the value with 20% of the complexity. Defer dependent types unless there's a specific need.

The existing evaluator's pattern matching and variable binding already provides most of what's needed for basic types!
