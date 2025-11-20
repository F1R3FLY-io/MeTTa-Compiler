# MeTTa Language Overview for MORK Implementation

**Version**: 1.0
**Last Updated**: 2025-11-13
**Purpose**: Reference guide for implementing MeTTa semantics in MORK
**Target Audience**: MeTTaTron compiler developers

## Table of Contents

1. [Introduction](#introduction)
2. [MeTTa Atom Types](#metta-atom-types)
3. [Pattern Matching System](#pattern-matching-system)
4. [Minimal MeTTa Instruction Set](#minimal-metta-instruction-set)
5. [Space Operations](#space-operations)
6. [Type System](#type-system)
7. [Module System](#module-system)
8. [Grounded Atoms](#grounded-atoms)
9. [Evaluation Model](#evaluation-model)
10. [Standard Library](#standard-library)

---

## Introduction

### What is MeTTa?

**MeTTa** (Meta Type Talk) is a meta-language designed for probabilistic logic programming and neural-symbolic AI. It combines:
- **Symbolic reasoning**: Pattern matching, unification, logical inference
- **Sub-symbolic grounding**: Integration with neural networks, numeric computation
- **Type safety**: Optional static typing with inference
- **Non-determinism**: Multiple evaluation branches, probabilistic outcomes

### MeTTa in the Hyperon Architecture

**Location**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/`

**Architecture Layers**:
```
┌─────────────────────────────────────┐
│   MeTTa Syntax & Parser             │  User-facing language
├─────────────────────────────────────┤
│   Atom Types & Operations           │  Core data structures
├─────────────────────────────────────┤
│   Pattern Matching & Unification    │  Query engine
├─────────────────────────────────────┤
│   Evaluation & Reduction            │  Execution engine
├─────────────────────────────────────┤
│   Spaces (Atomspaces)               │  Knowledge bases
├─────────────────────────────────────┤
│   Grounded Atoms & Foreign Funcs    │  External integration
└─────────────────────────────────────┘
```

### Why This Matters for MORK

**MORK** is a high-performance backend for MeTTa that uses:
- Byte-level encoding of atoms
- Trie-based storage (PathMap)
- Zipper-based queries
- Source/sink architecture

**This document** maps MeTTa's high-level semantics to concepts that can be implemented in MORK's low-level infrastructure.

---

## MeTTa Atom Types

### Core Type Hierarchy

**Location**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/hyperon-atom/src/lib.rs`

```rust
pub enum Atom {
    Symbol(SymbolAtom),
    Variable(VariableAtom),
    Expression(ExpressionAtom),
    Grounded(Grounded),
}
```

### 1. Symbol Atoms

**Definition**: Immutable identifiers representing concepts.

**Implementation** (`SymbolAtom`):
```rust
pub struct SymbolAtom(Shared<str>);  // Internally uses UniqueString

impl SymbolAtom {
    pub fn new<T: AsRef<str>>(name: T) -> Self;
    pub fn name(&self) -> &str;
}
```

**Properties**:
- Interned strings (shared storage for identical symbols)
- Case-sensitive
- Valid characters: alphanumeric, `-`, `_`, `@`, etc.
- Cannot contain whitespace or special delimiters `()[]`

**Examples**:
```metta
foo
parent
+
Number
CamelCase
kebab-case
snake_case
@grounded
```

**Equality**:
- Structural: Compare string content
- Identity: Pointer equality for interned strings (optimization)

**MORK Mapping Consideration**:
- Symbols will be interned via MORK's `SharedMapping`
- Byte encoding will use `Tag::SymbolSize(n)` followed by UTF-8 bytes

### 2. Variable Atoms

**Definition**: Placeholders for pattern matching and binding.

**Implementation** (`VariableAtom`):
```rust
pub struct VariableAtom {
    name: Shared<str>,
    id: usize,  // Unique ID for scoping
}

impl VariableAtom {
    pub fn new(name: impl AsRef<str>) -> Self;
    pub fn name(&self) -> &str;
    pub fn id(&self) -> usize;
}
```

**Properties**:
- Prefixed with `$` in syntax: `$x`, `$var`, `$CamelCase`
- Each variable has unique ID for alpha-renaming
- Variables with same name but different IDs are distinct
- Used in patterns and templates

**Examples**:
```metta
$x
$my_var
$Type
$_  ; wildcard variable
```

**Scoping**:
```metta
; Different scopes, different IDs
(= (foo $x) $x)      ; $x #1
(= (bar $x $y) $x)   ; $x #2, $y #3
```

**Equality**:
- Structural: Compare name AND ID
- Variables with same name but different IDs are NOT equal

**MORK Mapping Consideration**:
- Variable names will be converted to De Bruijn levels
- Scoping will require careful tracking of variable introductions
- MORK uses `Tag::NewVar` and `Tag::VarRef(k)` for encoding

### 3. Expression Atoms

**Definition**: Composite structures containing child atoms (S-expressions).

**Implementation** (`ExpressionAtom`):
```rust
pub struct ExpressionAtom {
    children: Shared<[Atom]>,
}

impl ExpressionAtom {
    pub fn new(children: Vec<Atom>) -> Self;
    pub fn children(&self) -> &[Atom];
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

**Properties**:
- Arbitrary arity (0 to N children)
- Nested composition (expressions can contain expressions)
- Order-sensitive (different order = different expression)
- Shared storage for identical expressions

**Examples**:
```metta
(parent Alice Bob)           ; 3 children
(+ 1 2)                      ; 3 children
(if $condition $then $else)  ; 4 children
(foo)                        ; 1 child (just 'foo')
()                           ; 0 children (empty expression)
((nested (deeply)) here)     ; Nested expressions
```

**Equality**:
- Structural: Recursively compare all children
- Order matters: `(a b)` ≠ `(b a)`
- Arity matters: `(a)` ≠ `(a a)`

**MORK Mapping Consideration**:
- Expressions will use `Tag::Arity(n)` encoding
- Recursive encoding of children
- Structural sharing in PathMap will deduplicate common subexpressions

### 4. Grounded Atoms

**Definition**: Atoms representing sub-symbolic data with custom behavior.

**Implementation** (`Grounded`):
```rust
pub struct Grounded(Box<dyn GroundedAtom>);

pub trait GroundedAtom: Display + Debug + Send + Sync {
    fn type_(&self) -> Atom;
    fn as_any(&self) -> &dyn Any;
    fn clone_grounded(&self) -> Grounded;
    fn eq_grounded(&self, other: &Grounded) -> bool;
    fn match_(&self, other: &Atom) -> matcher::MatchResultIter;
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError>;
}
```

**Properties**:
- Encapsulate foreign data types (numbers, strings, functions, etc.)
- Custom equality semantics
- Custom matching logic
- Can execute as functions
- Type information available via `type_()`

**Standard Grounded Types**:

**Numbers**:
```metta
42        ; Integer
3.14      ; Float
-17       ; Negative
```

**Strings**:
```metta
"hello"
"multi\nline"
"with \"quotes\""
```

**Functions** (callable):
```metta
(+ 1 2)   ; Calls grounded '+' function → 3
(* 3 4)   ; Calls grounded '*' function → 12
```

**Custom Types**:
```rust
struct MyCustomType { data: Vec<u8> }

impl GroundedAtom for MyCustomType {
    fn type_(&self) -> Atom { sym!("MyCustomType") }
    // ... implement other methods
}
```

**Equality**:
- Delegated to `eq_grounded()` implementation
- Can be value-based or identity-based
- Numbers: value equality (`42 == 42`)
- Strings: content equality (`"foo" == "foo"`)
- Custom types: type-specific logic

**Matching**:
- Delegated to `match_()` implementation
- Can implement fuzzy matching, pattern extraction, etc.
- Example: regex grounded type matches strings

**Execution**:
- Delegated to `execute()` implementation
- Arguments passed as atoms
- Returns multiple results (non-deterministic)
- Can fail with `ExecError`

**MORK Mapping Consideration**:
- Grounded atoms need special encoding scheme
- Registry pattern to map encoded bytes ↔ grounded implementations
- WASMSink integration for grounded execution
- Custom matching requires callback mechanism

### Atom Trait Methods

**All atoms implement**:
```rust
pub trait AtomLike {
    fn as_atom(&self) -> &Atom;
    fn is_symbol(&self) -> bool;
    fn is_variable(&self) -> bool;
    fn is_expression(&self) -> bool;
    fn is_grounded(&self) -> bool;
}

impl Atom {
    pub fn clone(&self) -> Atom;
    pub fn eq(&self, other: &Atom) -> bool;
    pub fn display(&self) -> String;
}
```

**Type Refinement**:
```rust
impl Atom {
    pub fn as_symbol(&self) -> Option<&SymbolAtom>;
    pub fn as_variable(&self) -> Option<&VariableAtom>;
    pub fn as_expression(&self) -> Option<&ExpressionAtom>;
    pub fn as_grounded(&self) -> Option<&Grounded>;
}
```

---

## Pattern Matching System

### Overview

**Location**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/hyperon-atom/src/matcher.rs`

**Purpose**: Unify patterns with atoms, producing variable bindings.

**Key Insight**: MeTTa pattern matching is **symmetric** - both sides can contain variables.

### Bindings Structure

```rust
pub struct Bindings {
    binding_by_var: HashMap<VariableAtom, usize>,
    bindings: HoleyVec<Binding>,
}

enum Binding {
    var(VariableAtom),
    atom(Atom),
}
```

**Properties**:
- Maps variables to either atoms or other variables
- Supports variable aliasing: `$x = $y`
- Circular bindings detected and excluded
- Efficient representation using indices

**Operations**:
```rust
impl Bindings {
    pub fn new() -> Self;
    pub fn is_empty(&self) -> bool;

    pub fn add_var_binding(&mut self, var: VariableAtom, atom: Atom) -> bool;
    pub fn add_var_equality(&mut self, var1: VariableAtom, var2: VariableAtom) -> bool;

    pub fn resolve(&self, var: &VariableAtom) -> Option<&Atom>;
    pub fn iter(&self) -> impl Iterator<Item = (&VariableAtom, &Atom)>;

    pub fn merge_v2(&self, other: &Bindings) -> BindingsSet;
    pub fn narrow_vars(&self, vars: &VariableSet) -> Bindings;
}
```

### BindingsSet

```rust
pub struct BindingsSet {
    vec: Vec<Bindings>,
}
```

**Purpose**: Represent multiple alternative binding sets (non-determinism).

**Operations**:
```rust
impl BindingsSet {
    pub fn empty() -> Self;
    pub fn single(bindings: Bindings) -> Self;
    pub fn from_bindings(vec: Vec<Bindings>) -> Self;

    pub fn is_empty(&self) -> bool;
    pub fn is_single(&self) -> bool;
    pub fn len(&self) -> usize;

    pub fn push(&mut self, bindings: Bindings);
    pub fn extend(&mut self, other: BindingsSet);

    pub fn iter(&self) -> impl Iterator<Item = &Bindings>;
}
```

### Matching Algorithm

**Function Signature**:
```rust
pub fn match_atoms(left: &Atom, right: &Atom) -> BindingsSet
```

**Algorithm** (simplified):
```rust
fn match_atoms(left: &Atom, right: &Atom) -> BindingsSet {
    match (left, right) {
        // Variable on left
        (Atom::Variable(v), atom) => {
            let mut bindings = Bindings::new();
            bindings.add_var_binding(v.clone(), atom.clone());
            BindingsSet::single(bindings)
        }

        // Variable on right
        (atom, Atom::Variable(v)) => {
            // Symmetric case
            match_atoms(right, left)
        }

        // Symbol matching
        (Atom::Symbol(s1), Atom::Symbol(s2)) => {
            if s1 == s2 {
                BindingsSet::single(Bindings::new())  // Empty bindings (match)
            } else {
                BindingsSet::empty()  // No match
            }
        }

        // Expression matching
        (Atom::Expression(e1), Atom::Expression(e2)) => {
            if e1.len() != e2.len() {
                return BindingsSet::empty();
            }

            // Match children recursively and merge bindings
            let mut result = BindingsSet::single(Bindings::new());
            for (child1, child2) in e1.children().iter().zip(e2.children()) {
                let child_matches = match_atoms(child1, child2);
                result = merge_all_bindings(result, child_matches);
            }
            result
        }

        // Grounded matching (custom logic)
        (Atom::Grounded(g), atom) => {
            g.match_(atom)  // Delegate to custom implementation
        }

        // Mismatched types
        _ => BindingsSet::empty()
    }
}
```

**Example Matches**:
```metta
; Simple variable binding
(foo $x) ~ (foo bar)  →  $x = bar

; Variable aliasing
(foo $x $x) ~ (foo $y $z)  →  $x = $y, $y = $z  (or: $x = $y = $z)

; Nested expressions
(parent $x (child $y)) ~ (parent Alice (child Bob))
  →  $x = Alice, $y = Bob

; No match (different symbols)
(foo $x) ~ (bar $x)  →  {}  (empty)

; No match (different arity)
(foo $x $y) ~ (foo 1)  →  {}  (empty)
```

### Binding Resolution

**Purpose**: Follow variable chains to get final value.

**Algorithm**:
```rust
fn resolve(bindings: &Bindings, var: &VariableAtom) -> Option<Atom> {
    let mut current = var;
    let mut visited = HashSet::new();

    loop {
        if visited.contains(current) {
            return None;  // Circular binding
        }
        visited.insert(current.clone());

        match bindings.get(current) {
            Some(Binding::Atom(atom)) => return Some(atom.clone()),
            Some(Binding::Var(next_var)) => current = next_var,
            None => return None,  // Unbound
        }
    }
}
```

**Example**:
```metta
; Bindings: $x = $y, $y = $z, $z = 42
resolve($x) → 42
resolve($y) → 42
resolve($z) → 42

; Circular: $a = $b, $b = $a
resolve($a) → None  (circular)
```

### Binding Merge

**Purpose**: Combine bindings from multiple match operations.

**Challenge**: Merging may produce multiple alternative binding sets.

**Algorithm** (conceptual):
```rust
fn merge_bindings(b1: &Bindings, b2: &Bindings) -> BindingsSet {
    let mut result = b1.clone();

    for (var, value2) in b2.iter() {
        match result.resolve(var) {
            None => {
                // Variable unbound in result, add binding
                result.add_var_binding(var.clone(), value2.clone());
            }
            Some(value1) => {
                // Variable already bound, must match
                let sub_matches = match_atoms(value1, value2);

                if sub_matches.is_empty() {
                    return BindingsSet::empty();  // Conflict
                }

                // May split into multiple alternatives
                if sub_matches.len() > 1 {
                    return merge_all_alternatives(result, sub_matches);
                }

                // Merge sub-bindings
                for (sub_var, sub_value) in sub_matches.single().iter() {
                    result.add_var_binding(sub_var.clone(), sub_value.clone());
                }
            }
        }
    }

    BindingsSet::single(result)
}
```

**Example**:
```metta
; Merge: {$x = 1} ∪ {$y = 2}  →  {$x = 1, $y = 2}

; Merge: {$x = 1} ∪ {$x = 1}  →  {$x = 1}

; Merge: {$x = 1} ∪ {$x = 2}  →  {}  (conflict)

; Merge with variable alias: {$x = $y} ∪ {$y = 42}  →  {$x = 42, $y = 42}
```

### MORK Mapping Considerations

**Challenges**:
1. **Named Variables**: MeTTa uses named variables with IDs; MORK uses De Bruijn levels
2. **Symmetric Matching**: Both sides can have variables; MORK patterns are typically one-sided
3. **Multiple Results**: BindingsSet can have many alternatives; MORK returns paths

**Solutions**:
1. **Variable Conversion Layer**: Map MeTTa variables ↔ De Bruijn levels
2. **Post-Processing**: Convert MORK query results into BindingsSet
3. **Custom Source**: Implement symmetric matching via custom source logic

---

## Minimal MeTTa Instruction Set

### Overview

**Location**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/docs/minimal-metta.md`

**Purpose**: Core operations that define MeTTa semantics. All higher-level constructs reduce to these.

**Evaluation Model**:
- Input: List of `(atom, bindings)` pairs
- Output: List of `(atom, bindings)` pairs
- Non-deterministic: Multiple branches tracked in parallel
- Normal order: Arguments not evaluated before application

### Core Operations

#### 1. eval

**Signature**: `(eval <atom>)` → `<result>`

**Semantics**: Single-step evaluation.

**Behavior**:
1. If `<atom>` is a grounded function: execute it
2. If `<atom>` matches `(= <pattern> <template>)` in space: apply rewrite
3. Otherwise: return `<atom>` unchanged

**Examples**:
```metta
; Grounded function
(eval (+ 1 2))  →  3

; Rewrite rule: (= (double $x) (+ $x $x))
(eval (double 5))  →  (+ 5 5)

; No reduction
(eval foo)  →  foo
```

**Multiple Results**:
```metta
; Multiple matching rules:
; (= (color apple) red)
; (= (color apple) green)

(eval (color apple))  →  {red, green}
```

**MORK Implementation**:
- Query space for `(= <pattern> $result)`
- Use BTMSource with pattern matching
- Collect all matching results
- For grounded: dispatch via WASMSink or native function

#### 2. chain

**Signature**: `(chain <atom> <variable> <template>)` → `<result>`

**Semantics**: Sequential composition with variable binding.

**Behavior**:
1. Evaluate `<atom>` → get results with bindings
2. For each result: substitute `<variable>` in `<template>`
3. Evaluate substituted templates → collect results

**Examples**:
```metta
; Simple chain
(chain (+ 1 2) $x (+ $x 10))
  →  (eval (+ 1 2)) → 3
  →  substitute $x = 3 in (+ $x 10) → (+ 3 10)
  →  (eval (+ 3 10)) → 13

; Multiple results
; (= (number) {1, 2, 3})
(chain (number) $x (* $x $x))
  →  (eval (number)) → {1, 2, 3}
  →  substitute: {(* 1 1), (* 2 2), (* 3 3)}
  →  (eval ...) → {1, 4, 9}
```

**MORK Implementation**:
- Evaluate first atom using eval
- For each result, perform De Bruijn substitution
- Evaluate substituted atoms
- Collect all branches

#### 3. unify

**Signature**: `(unify <atom> <pattern> <then> <else>)` → `<result>`

**Semantics**: Conditional branching based on pattern match.

**Behavior**:
1. Match `<atom>` against `<pattern>`
2. If match succeeds: evaluate `<then>` with bindings
3. If match fails: evaluate `<else>`

**Examples**:
```metta
; Successful match
(unify (foo bar) (foo $x) $x error)
  →  match (foo bar) ~ (foo $x) → {$x = bar}
  →  eval $x with bindings → bar

; Failed match
(unify (foo bar) (baz $x) $x error)
  →  match (foo bar) ~ (baz $x) → {}
  →  eval error → error

; Multiple match results
(unify $x $x success failure)
  →  match $x ~ $x → {$x = $x}  (variable equality)
  →  eval success → success
```

**MORK Implementation**:
- Use pattern matching engine
- Build Bindings from match
- Evaluate then/else branch with context

#### 4. cons-atom / decons-atom

**Signatures**:
- `(cons-atom <head> <tail>)` → `(<head> <tail>)`
- `(decons-atom (<head> <tail>))` → `(<head> <tail>)`

**Semantics**: Expression construction and deconstruction.

**cons-atom Behavior**:
- Takes atom and expression
- Constructs new expression with atom as first child

**decons-atom Behavior**:
- Takes expression
- Returns pair: (head, tail)
- Head: first child
- Tail: expression of remaining children

**Examples**:
```metta
; Construction
(cons-atom foo (bar baz))  →  (foo bar baz)
(cons-atom 1 (2 3 4))      →  (1 2 3 4)

; Deconstruction
(decons-atom (a b c))  →  (a (b c))
(decons-atom (single))  →  (single ())

; Empty expression
(decons-atom ())  →  Error
```

**MORK Implementation**:
- Direct byte-level manipulation
- cons-atom: increment arity, prepend child encoding
- decons-atom: decrement arity, split off first child

#### 5. function / return

**Signatures**:
- `(function <body>)` → `<closure>`
- `(return <atom>)` → terminates function

**Semantics**: Create evaluation loops (functions).

**function Behavior**:
- Creates closure capturing current bindings
- Returns unevaluated

**return Behavior**:
- Exits innermost function
- Returns value

**Example**:
```metta
(= (factorial $n)
   (function
     (unify $n 0
       (return 1)
       (chain (eval (- $n 1)) $n_minus_1
         (chain (factorial $n_minus_1) $factorial_n_minus_1
           (return (* $n $factorial_n_minus_1)))))))

(eval (factorial 5))  →  120
```

**MORK Implementation**:
- Function: create closure structure
- Return: special marker for evaluation loop exit

#### 6. collapse-bind / superpose-bind

**Signatures**:
- `(collapse-bind <bindings_list>)` → merges bindings
- `(superpose-bind <bindings_set>)` → explodes bindings

**Semantics**: Control non-deterministic inference.

**collapse-bind**:
- Combines multiple binding alternatives into single set
- Intersection of variables

**superpose-bind**:
- Splits binding set into individual alternatives
- Enables branching exploration

**Examples**:
```metta
; Collapse multiple results
(collapse-bind ((= $x 1) (= $x 2)))
  →  {($x = 1), ($x = 2)}

; Superpose for branching
(superpose-bind {$x = {1, 2, 3}})
  →  {($x = 1), ($x = 2), ($x = 3)}
```

**MORK Implementation**:
- Track binding alternatives as separate paths
- Collapse: collect all paths
- Superpose: split into multiple evaluation branches

### Derived Operations

Higher-level MeTTa constructs compile to minimal operations:

**let / let***:
```metta
(let $x <value> <body>)
  ≡ (chain <value> $x <body>)

(let* (($x <val1>) ($y <val2>)) <body>)
  ≡ (chain <val1> $x (chain <val2> $y <body>))
```

**if**:
```metta
(if $condition $then $else)
  ≡ (unify $condition True $then $else)
```

**match**:
```metta
(match <atom>
  (<pattern1> <result1>)
  (<pattern2> <result2>))

  ≡ (unify <atom> <pattern1> <result1>
      (unify <atom> <pattern2> <result2> Error))
```

**case**:
```metta
(case (<val1> <val2>)
  ((<pat1> <pat2>) <result>))

  ≡ (unify <val1> <pat1>
      (unify <val2> <pat2> <result> Error)
      Error)
```

### MORK Implementation Strategy

**Goal**: Map each minimal operation to MORK primitives.

**Approach**:
1. **eval**: Query space + grounded dispatch
2. **chain**: Substitution + recursive eval
3. **unify**: Pattern matching + conditional
4. **cons/decons**: Byte-level expression manipulation
5. **function/return**: Control flow markers
6. **collapse/superpose**: Path management

**Key Challenge**: Tracking non-deterministic evaluation state through MORK's zipper-based architecture.

---

## Space Operations

### Space Concept

**Space** (Atomspace): Collection of atoms that can be queried and modified.

**Properties**:
- Unordered set of atoms (with optional deduplication)
- Supports pattern-based queries
- Can be observed for changes
- Multiple spaces can coexist

### Space Types

#### GroundingSpace

**Location**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/hyperon-atom/src/space.rs`

```rust
pub struct GroundingSpace {
    content: Vec<Atom>,
    dedup: bool,
}
```

**Operations**:
```rust
impl GroundingSpace {
    pub fn new() -> Self;
    pub fn with_dedup(dedup: bool) -> Self;

    pub fn add(&mut self, atom: Atom);
    pub fn remove(&mut self, atom: &Atom) -> bool;
    pub fn replace(&mut self, from: &Atom, to: Atom) -> bool;

    pub fn query(&self, pattern: &Atom) -> BindingsSet;
    pub fn atoms(&self) -> impl Iterator<Item = &Atom>;

    pub fn observe(&mut self, observer: impl Observer);
}
```

#### ModuleSpace

**Purpose**: Namespace isolation for modules.

**Properties**:
- Wraps another space
- Adds namespace prefix to all atoms
- Import/export controls

#### DynSpace

**Purpose**: Dynamic dispatch to any space implementation.

```rust
pub struct DynSpace(Box<dyn Space>);
```

### Core Space Operations

#### add!

**Signature**: `(add-atom &<space> <atom>)`

**Semantics**: Insert atom into space.

**Examples**:
```metta
(add-atom &space (parent Alice Bob))
(add-atom &space (= (foo $x) (bar $x)))
```

**MORK Implementation**:
- Encode atom to bytes
- Use AddSink to insert into PathMap
- Update space observation callbacks

#### remove!

**Signature**: `(remove-atom &<space> <atom>)`

**Semantics**: Remove atom from space.

**Examples**:
```metta
(remove-atom &space (parent Alice Bob))
```

**MORK Implementation**:
- Encode atom to bytes
- Use RemoveSink to delete from PathMap
- Update space observation callbacks

#### match

**Signature**: `(match &<space> <pattern> <template>)`

**Semantics**: Query space with pattern, apply template to results.

**Examples**:
```metta
; Find all parents
(match &space (parent $x $y) ($x is parent of $y))

; Find specific parent
(match &space (parent Alice $child) $child)
```

**MORK Implementation**:
- Convert pattern to BTMSource query
- Execute query to get matching paths
- Convert paths to Bindings
- Substitute bindings in template

#### get-atoms

**Signature**: `(get-atoms &<space>)` → `<list of atoms>`

**Semantics**: Return all atoms in space.

**MORK Implementation**:
- Iterate PathMap
- Decode bytes to atoms
- Collect into list

### Space Observation

**Purpose**: React to space changes.

**Observer Interface**:
```rust
pub trait Observer {
    fn on_add(&mut self, atom: &Atom);
    fn on_remove(&mut self, atom: &Atom);
}
```

**Use Cases**:
- Incremental indexing
- Change propagation
- Consistency maintenance
- Logging/debugging

**MORK Implementation**:
- Register observers with space wrapper
- Trigger callbacks on AddSink/RemoveSink finalize
- Pass atom and operation type

### Multi-Space Management

**Capability**: Multiple independent spaces.

**Example**:
```metta
(= &facts (new-space))
(= &rules (new-space))

(add-atom &facts (parent Alice Bob))
(add-atom &rules (= (ancestor $x $y) (parent $x $y)))

(match &facts (parent $x $y) (add-atom &derived (ancestor $x $y)))
```

**MORK Implementation**:
- Each space = separate PathMap
- Space references encoded as symbols
- Namespace prefixes for isolation

---

## Type System

### Type Representation

**Types as Atoms**: Types are first-class values.

**Type Constructors**:
```metta
Number              ; Primitive type
String              ; Primitive type
(-> $arg $ret)      ; Function type
($a -> $b)          ; Infix notation
(List $T)           ; Parameterized type
```

### Type Declarations

**Syntax**: `(: <atom> <type>)`

**Examples**:
```metta
(: 42 Number)
(: "hello" String)
(: + (-> Number Number Number))
(: map (-> (-> $a $b) (List $a) (List $b)))
```

### Type Checking

**Location**: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/lib/src/metta/types.rs`

**check_type Function**:
```rust
pub fn check_type(
    space: &GroundingSpace,
    atom: &Atom,
    expected_type: &Atom,
) -> Result<(), String>
```

**Algorithm**:
1. Query space for type declaration `(: <atom> <type>)`
2. Unify found type with expected type
3. Recursive checking for expressions

**Example**:
```metta
; Declarations
(: foo (-> Number Number))
(: 5 Number)

; Check
(foo 5)  ; type checks: foo expects Number, 5 is Number
(foo "x")  ; type error: foo expects Number, "x" is String
```

### Type Inference

**Capability**: Infer types from usage.

**Algorithm**:
- Collect type constraints from expressions
- Solve constraints via unification
- Propagate inferred types

**Example**:
```metta
; Infer type of $f
(let $f (λ $x (+ $x 1))
  ($f 5))

; Infers: $f : (-> Number Number)
```

### Special Types

**Type**: Type of types
```metta
(: Number Type)
(: String Type)
(: (-> $a $b) Type)
```

**ErrorType**: Represents type errors
```metta
(: (foo "wrong") ErrorType)  ; Type mismatch
```

**SpaceType**: Type of spaces
```metta
(: &space SpaceType)
```

### MORK Implementation

**Type Storage**:
- Store type declarations as regular atoms in space
- Encode type atoms like any other atom

**Type Checking**:
- Query space for `: <atom> <type>` using pattern matching
- Unify types using standard unification
- Cache inferred types for performance

**Type Inference**:
- Collect constraints during evaluation
- Solve via repeated unification queries
- Update space with inferred types

---

## Module System

### Module Concept

**Module**: Named collection of atoms with import/export controls.

**Properties**:
- Namespace isolation
- Explicit imports
- Lazy loading
- Git-based distribution (optional)

### Module Definition

**Syntax**:
```metta
!(module <name>
   (export <symbol1> <symbol2> ...)
   <atom1>
   <atom2>
   ...)
```

**Example**:
```metta
!(module math
   (export + - * / sqrt)
   (= (+ $x $y) (<grounded-plus> $x $y))
   (= (sqrt $x) (<grounded-sqrt> $x)))
```

### Module Import

**Syntax**: `!(import &<space> <module>)`

**Examples**:
```metta
!(import &self math)        ; Import from local
!(import &self git:org/repo)  ; Import from git
```

### Module Spaces

**ModuleSpace Wrapper**:
```rust
pub struct ModuleSpace {
    space: Box<dyn Space>,
    namespace: String,
}
```

**Namespace Prefixing**:
```metta
; In module "math"
(= (sqrt $x) ...)

; Imported as
(= (math.sqrt $x) ...)
```

### MORK Implementation

**Module Storage**:
- Each module = separate PathMap
- Namespace prefix in byte encoding

**Import Resolution**:
- Copy atoms from module space to target space
- Apply namespace transformation

**Git Loading**:
- Fetch module from git
- Parse MeTTa source
- Load into module space

---

## Grounded Atoms

### Grounded Atom Interface

**Trait Definition**:
```rust
pub trait GroundedAtom: Display + Debug + Send + Sync {
    fn type_(&self) -> Atom;
    fn as_any(&self) -> &dyn Any;
    fn clone_grounded(&self) -> Grounded;
    fn eq_grounded(&self, other: &Grounded) -> bool;
    fn match_(&self, other: &Atom) -> matcher::MatchResultIter;
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError>;
}
```

### Standard Grounded Types

#### Numbers

**Implementation**:
```rust
pub enum Number {
    Integer(i64),
    Float(f64),
}

impl GroundedAtom for Number {
    fn type_(&self) -> Atom { sym!("Number") }

    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        Err(ExecError::NotExecutable)  // Numbers don't execute
    }

    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Value-based matching
        if let Some(other_num) = other.as_grounded::<Number>() {
            if self == other_num {
                return MatchResultIter::single(Bindings::new());
            }
        }
        MatchResultIter::empty()
    }
}
```

#### Strings

**Implementation**:
```rust
pub struct Str(String);

impl GroundedAtom for Str {
    fn type_(&self) -> Atom { sym!("String") }

    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Content-based matching
        if let Some(other_str) = other.as_grounded::<Str>() {
            if self.0 == other_str.0 {
                return MatchResultIter::single(Bindings::new());
            }
        }
        MatchResultIter::empty()
    }
}
```

#### Functions

**Implementation**:
```rust
pub struct GroundedFunc {
    name: String,
    func: fn(&[Atom]) -> Result<Vec<Atom>, ExecError>,
}

impl GroundedAtom for GroundedFunc {
    fn type_(&self) -> Atom {
        // Return function type based on name/signature
        sym!("Function")
    }

    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        (self.func)(args)
    }
}
```

**Example**:
```rust
fn plus(args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
    let n1 = args[0].as_grounded::<Number>()?;
    let n2 = args[1].as_grounded::<Number>()?;
    Ok(vec![Atom::gnd(n1 + n2)])
}

let plus_atom = Atom::gnd(GroundedFunc {
    name: "+".to_string(),
    func: plus,
});
```

### Custom Grounded Types

**Example**: Regex matching

```rust
pub struct Regex(regex::Regex);

impl GroundedAtom for Regex {
    fn type_(&self) -> Atom { sym!("Regex") }

    fn match_(&self, other: &Atom) -> MatchResultIter {
        if let Some(s) = other.as_grounded::<Str>() {
            if self.0.is_match(&s.0) {
                // Extract capture groups as bindings
                let captures = self.0.captures(&s.0)?;
                let mut bindings = Bindings::new();
                for (i, cap) in captures.iter().enumerate() {
                    let var = var!(format!("${}", i));
                    bindings.add_var_binding(var, Atom::gnd(Str(cap.as_str().to_string())));
                }
                return MatchResultIter::single(bindings);
            }
        }
        MatchResultIter::empty()
    }
}
```

**Usage**:
```metta
(match "hello123" <regex:"[a-z]+(\d+)"> $1)  →  "123"
```

### MORK Integration

**Challenge**: MORK works with bytes; grounded atoms have custom logic.

**Solution**: Registry pattern

```rust
pub struct GroundedRegistry {
    by_type: HashMap<String, Box<dyn GroundedAdapter>>,
    instances: Vec<Box<dyn GroundedAtom>>,
}

pub trait GroundedAdapter {
    fn encode(&self, atom: &dyn GroundedAtom) -> Vec<u8>;
    fn decode(&self, bytes: &[u8]) -> Box<dyn GroundedAtom>;
    fn match_custom(&self, left_bytes: &[u8], right_bytes: &[u8]) -> MatchResultIter;
    fn execute(&self, bytes: &[u8], args: &[Atom]) -> Result<Vec<Atom>, ExecError>;
}
```

**Flow**:
1. Grounded atom → Encode to bytes (type tag + data)
2. Store bytes in PathMap
3. Query retrieves bytes
4. Decode bytes → Grounded atom
5. Delegate matching/execution to adapter

---

## Evaluation Model

### Evaluation Pipeline

**Input**: Atom to evaluate

**Output**: List of `(result_atom, bindings)` pairs

**Stages**:
1. **Check if reducible**: Can atom be evaluated further?
2. **Find reductions**: Query space for `(= <pattern> <template>)` rules
3. **Apply grounding**: If grounded function, execute it
4. **Substitute bindings**: Apply variable substitutions
5. **Recursive evaluation**: Evaluate sub-expressions
6. **Collect results**: Gather all non-deterministic branches

### Reduction Strategies

#### Normal Order

**Rule**: Don't evaluate arguments before function application.

**Example**:
```metta
(= (const $x $y) $x)

(const 42 (infinite-loop))
  → evaluate (const 42 (infinite-loop))
  → match pattern (const $x $y)
  → substitute: $x = 42, $y = (infinite-loop)
  → return 42
  ; (infinite-loop) never evaluated!
```

**Benefit**: Enables lazy evaluation, handles non-terminating expressions.

#### Eager Evaluation (Optional)

**Rule**: Evaluate arguments first.

**Example**:
```metta
(+ (fib 10) (fib 11))
  → evaluate (fib 10) → 55
  → evaluate (fib 11) → 89
  → evaluate (+ 55 89) → 144
```

**MORK**: Support both via evaluation strategy configuration.

### Non-Determinism

**Source**: Multiple rewrite rules, grounded functions returning multiple values.

**Handling**:
```rust
pub struct EvalState {
    branches: Vec<(Atom, Bindings)>,
}

impl EvalState {
    pub fn eval_step(&mut self, space: &Space) {
        let mut new_branches = Vec::new();

        for (atom, bindings) in self.branches.drain(..) {
            let results = eval_atom(&atom, space, &bindings);
            new_branches.extend(results);
        }

        self.branches = new_branches;
    }
}
```

**Example**:
```metta
; Multiple rules
(= (color) red)
(= (color) blue)

(eval (color))  →  {red, blue}
```

### Error Handling

**Error Atom**: Special atom representing evaluation failure.

```rust
pub struct ErrorAtom(String);

impl GroundedAtom for ErrorAtom { ... }
```

**Usage**:
```metta
(if (condition-that-fails) success error)
  → Error "condition evaluation failed"
```

**NotReducible**: Marker for atoms that can't be evaluated further.

**Empty**: Represents no results.

### Evaluation Termination

**Stop Conditions**:
1. No more reductions possible
2. Explicit `return` statement
3. Error encountered
4. Resource limit reached (timeout, depth)

**Depth Limit**:
```rust
pub fn eval_with_limit(atom: &Atom, space: &Space, max_depth: usize) -> Vec<Atom> {
    if max_depth == 0 {
        return vec![atom.clone()];
    }

    let results = eval_step(atom, space);
    if results.is_empty() {
        return vec![atom.clone()];
    }

    results.into_iter()
        .flat_map(|r| eval_with_limit(&r, space, max_depth - 1))
        .collect()
}
```

### MORK Evaluation Architecture

**Components**:
1. **Rewrite Engine**: Query space for `(= pattern template)` rules
2. **Grounded Executor**: Dispatch grounded function calls
3. **Substitution Engine**: Apply variable bindings (De Bruijn substitution)
4. **Branch Manager**: Track non-deterministic evaluation state
5. **Error Handler**: Propagate and report errors

**Flow**:
```
Input Atom
    ↓
Check if grounded function
    ↓ (yes)              ↓ (no)
Execute via WASM    Query space for rewrite rules
    ↓                    ↓
Return results      Apply substitutions
    ↓                    ↓
    └────────→  Collect all branches ←────────┘
                     ↓
                Return (atom, bindings) list
```

---

## Standard Library

### Arithmetic Operations

```metta
+, -, *, /       ; Binary operations
mod, %           ; Modulo
<, >, <=, >=, == ; Comparisons
```

### List Operations

```metta
cons, car, cdr   ; List construction/deconstruction
length           ; List length
append           ; List concatenation
map, filter      ; Higher-order functions
```

### Logical Operations

```metta
and, or, not     ; Boolean logic
True, False      ; Boolean values
```

### Control Flow

```metta
if               ; Conditional
let, let*        ; Variable binding
match, case      ; Pattern matching
```

### String Operations

```metta
concat           ; String concatenation
substring        ; Extract substring
strlen           ; String length
```

### Space Operations

```metta
add-atom         ; Insert atom
remove-atom      ; Remove atom
match            ; Query with pattern
get-atoms        ; Get all atoms
new-space        ; Create new space
```

### Type Operations

```metta
get-type         ; Get type of atom
check-type       ; Type check
infer-type       ; Type inference
```

### I/O Operations

```metta
print            ; Print to stdout
read             ; Read from stdin
read-file        ; Read file
write-file       ; Write file
```

---

## Conclusion

This document provides a comprehensive overview of MeTTa language features relevant for MORK implementation. Key takeaways:

**Atom Types**: 4 core types (Symbol, Variable, Expression, Grounded) with distinct semantics

**Pattern Matching**: Symmetric matching with variable binding, support for non-determinism

**Minimal Operations**: 8 core operations that define all MeTTa semantics

**Spaces**: Atomspaces with add/remove/query operations

**Types**: First-class types with checking and inference

**Grounded Atoms**: Extensible foreign function interface

**Evaluation**: Non-deterministic, normal-order evaluation with multiple branches

**Next Steps**: See companion documents for detailed MORK implementation strategies:
- `encoding-strategy.md`: Byte-level atom encoding
- `pattern-matching.md`: Pattern matching implementation
- `space-operations.md`: Space operations
- `evaluation-engine.md`: Evaluation engine
- `implementation-roadmap.md`: Step-by-step guide
- `challenges-solutions.md`: Common problems and solutions
