# Unification Algorithm

## Overview

Unification is the core algorithm underlying pattern matching in MeTTa. Unlike simple pattern matching where one side is a pattern and the other is ground data, MeTTa's unification is **bidirectional** - both sides can contain variables and are treated symmetrically.

## What is Unification?

### Specification

**Unification** determines if two atoms can be made equal by substituting variables with atoms, and if so, produces the most general unifier.

**Formal Definition:**
```
unify(atom₁, atom₂) → σ | ⊥

where:
  σ: Variables → Atoms  (substitution/bindings)
  ⊥: failure (atoms cannot unify)

Property: atom₁[σ] = atom₂[σ]  (atoms equal after substitution)
```

**Key Properties:**
1. **Symmetry**: `unify(A, B) = unify(B, A)`
2. **Most General**: Returns least restrictive bindings
3. **Idempotent**: Applying σ to σ doesn't change it

### Implementation

**Location**: `hyperon-atom/src/matcher.rs:1089-1129`

**Entry Point**:
```rust
pub fn match_atoms(left: &Atom, right: &Atom) -> MatchResultIter {
    let result = match_atoms_recursively(left, right, Bindings::new());
    Box::new(result.into_iter().filter(|b| !has_loops(b)))
}
```

**Returns**: Iterator over `Bindings` (multiple solutions possible)

## Unification Rules

### Rule 1: Variable-Atom

**Specification:**

A variable unifies with any atom, binding the variable to that atom.

**Formal Rule:**
```
$x is a variable, a is any atom
─────────────────────────────────
unify($x, a) = {$x ← a}
```

**Implementation** - `matcher.rs:1105-1107`:
```rust
(Atom::Variable(v), atom) | (atom, Atom::Variable(v)) => {
    bindings.add_var_binding(v.clone(), atom.clone())
}
```

**Examples:**
```metta
unify($x, Socrates)     → {$x ← Socrates}
unify($x, 42)           → {$x ← 42}
unify($x, (foo bar))    → {$x ← (foo bar)}
unify(Socrates, $y)     → {$y ← Socrates}  (symmetric)
```

**Note**: Bidirectional - doesn't matter which side has the variable.

### Rule 2: Variable-Variable

**Specification:**

Two variables unify by creating an equality constraint.

**Formal Rule:**
```
$x, $y are variables
────────────────────
unify($x, $y) = {$x = $y}
```

**Implementation** - `matcher.rs:1105-1107` (variable binding logic):
```rust
(Atom::Variable(v1), Atom::Variable(v2)) => {
    bindings.add_var_binding(v1.clone(), Atom::Variable(v2.clone()))
    // Or equivalently: add_var_equality(v1, v2)
}
```

**Examples:**
```metta
unify($x, $y)    → {$x = $y}
unify($a, $b)    → {$a = $b}

; Later bindings resolve transitively:
{$x = $y, $y ← Socrates} → {$x ← Socrates, $y ← Socrates}
```

**Transitive Resolution**: If `$x = $y` and `$y ← A`, then `$x ← A`.

### Rule 3: Symbol-Symbol

**Specification:**

Symbols unify if they have the same name.

**Formal Rule:**
```
sym₁.name = sym₂.name
──────────────────────
unify(sym₁, sym₂) = {}
```

**Implementation** - `matcher.rs:1110-1112`:
```rust
(Atom::Symbol(l), Atom::Symbol(r)) if l == r => {
    BindingsSet::single(bindings)  // No new bindings
}
```

**Examples:**
```metta
unify(Socrates, Socrates)  → {}      (success, no bindings)
unify(Human, Human)        → {}
unify(A, B)                → ⊥       (failure)
```

**Case Sensitivity**: Symbols are case-sensitive.
```metta
unify(Human, human)        → ⊥       (failure)
```

### Rule 4: Number-Number

**Specification:**

Numbers unify if they have the same value and type.

**Formal Rule:**
```
n₁ = n₂  (value and type equality)
────────────────────────────────
unify(n₁, n₂) = {}
```

**Examples:**
```metta
unify(42, 42)         → {}
unify(3.14, 3.14)     → {}
unify(42, 43)         → ⊥
unify(42, 42.0)       → ⊥  (different types)
```

### Rule 5: String-String

**Specification:**

Strings unify if they have identical content.

**Formal Rule:**
```
s₁ = s₂  (character-wise equality)
────────────────────────────────
unify(s₁, s₂) = {}
```

**Examples:**
```metta
unify("hello", "hello")    → {}
unify("", "")              → {}
unify("Hello", "hello")    → ⊥  (case-sensitive)
```

### Rule 6: Expression-Expression

**Specification:**

Expressions unify if they have the same arity and all corresponding children unify.

**Formal Rule:**
```
len(e₁) = len(e₂)
∀i ∈ [0..len): unify(e₁[i], e₂[i]) = σᵢ
σ = merge(σ₁, σ₂, ..., σₙ)
σ is consistent (no conflicts)
─────────────────────────────────────
unify(e₁, e₂) = σ
```

**Implementation** - `matcher.rs:1115-1123`:
```rust
(Atom::Expression(l), Atom::Expression(r))
    if l.children().len() == r.children().len() =>
{
    let mut result = BindingsSet::single(bindings);
    for (l_child, r_child) in l.children().iter().zip(r.children()) {
        result = result.merge_v2(l_child, r_child, match_atoms_recursively);
    }
    result
}
```

**Examples:**
```metta
; Simple expression
unify((Human $x), (Human Socrates))
  → {$x ← Socrates}

; Nested expression
unify((parent Alice $child), (parent Alice Bob))
  → {$child ← Bob}

; Multiple variables
unify((edge $a $b), (edge X Y))
  → {$a ← X, $b ← Y}

; Arity mismatch
unify((f $x), (f $x $y))
  → ⊥  (different lengths)
```

### Rule 7: Grounded-Grounded

**Specification:**

Grounded atoms (custom Rust types) unify via their custom `match_()` implementation or equality.

**Implementation** - `matcher.rs:1125-1127`:
```rust
(Atom::Grounded(l), Atom::Grounded(r)) => {
    // Try custom matcher if available
    if let Some(result) = l.match_(r) {
        return result;
    }
    // Fall back to equality
    if l == r {
        BindingsSet::single(bindings)
    } else {
        BindingsSet::empty()
    }
}
```

**Custom Matching**: See `lib/examples/custom_match.rs`

### Rule 8: Type Mismatch

**Specification:**

Atoms of different types cannot unify.

**Formal Rule:**
```
type(a₁) ≠ type(a₂)
──────────────────
unify(a₁, a₂) = ⊥
```

**Examples:**
```metta
unify(Socrates, 42)         → ⊥  (symbol vs number)
unify("hello", hello)       → ⊥  (string vs symbol)
unify((a), a)               → ⊥  (expression vs symbol)
```

## Unification Algorithm

### Recursive Algorithm

**Location**: `matcher.rs:1101-1129`

**Pseudocode:**
```
function unify(left, right, bindings):
    // Variable cases
    if left is Variable:
        return bindings.add_var_binding(left, right)
    if right is Variable:
        return bindings.add_var_binding(right, left)

    // Symbol case
    if left is Symbol and right is Symbol:
        if left.name == right.name:
            return BindingsSet.single(bindings)
        else:
            return BindingsSet.empty()

    // Expression case
    if left is Expression and right is Expression:
        if left.len() != right.len():
            return BindingsSet.empty()

        result = BindingsSet.single(bindings)
        for i in 0..len:
            result = result.merge(left[i], right[i], unify)
        return result

    // Grounded case
    if left is Grounded and right is Grounded:
        return custom_match_or_equality(left, right, bindings)

    // Type mismatch
    return BindingsSet.empty()
```

**Complexity:**
- Time: O(size(left) + size(right))
- Space: O(depth) for recursion stack

### Binding Merging

**Process**: Combining bindings from sub-expressions

**Location**: `matcher.rs:886-1044` (BindingsSet)

**Method**: `merge_v2()` at `matcher.rs:1003-1023`

**Algorithm:**
```rust
fn merge_v2<F>(self, left: &Atom, right: &Atom, f: F) -> Self
where F: Fn(&Atom, &Atom, Bindings) -> BindingsSet
{
    let mut result = BindingsSet::empty();
    for bindings in self.into_iter() {
        let new_set = f(left, right, bindings);
        result = result.sum(new_set);
    }
    result
}
```

**Explanation:**
1. For each existing binding set
2. Match children with current bindings
3. Get new binding sets
4. Combine all results

**Example:**
```metta
; Unify ((a $x) (b $y)) with ((a 1) (b 2))

; Initial: bindings = {}
; Match (a $x) with (a 1):
;   bindings₁ = {$x ← 1}
; Match (b $y) with (b 2) given bindings₁:
;   bindings₂ = {$x ← 1, $y ← 2}
; Result: {$x ← 1, $y ← 2}
```

### Binding Consistency

**Requirement**: Bindings must be consistent (no conflicts).

**Conflict**: Trying to bind same variable to different atoms.

**Example:**
```metta
; Unify ($x foo $x) with (A foo B)
; Step 1: $x ← A
; Step 2: foo matches foo
; Step 3: $x ← B  CONFLICT with $x ← A
; Result: ⊥ (failure)
```

**Implementation**: `add_var_binding` checks for conflicts - `matcher.rs:398-450`

## Occurs Check

### What is the Occurs Check?

**Problem**: Prevent creating cyclic bindings like `$x ← (f $x)`.

**Specification:**

The occurs check ensures that a variable does not occur in the atom it's being bound to.

**Formal Rule:**
```
$x occurs in atom
──────────────────────────
unify($x, atom) = ⊥
```

**Example:**
```metta
unify($x, (f $x))     → ⊥  (occurs check fails)
unify($x, (f $y))     → {$x ← (f $y)}  (OK, $x doesn't occur)
```

### Implementation

**Location**: `matcher.rs:596-629`

**Function**: `has_loops(bindings: &Bindings) -> bool`

**Algorithm:**
```rust
fn has_loops(bindings: &Bindings) -> bool {
    for var in bindings.variables() {
        if binding_has_loops(bindings, var, &mut BitSet::new()) {
            return true;
        }
    }
    false
}

fn binding_has_loops(bindings: &Bindings, var: &VariableAtom, visited: &mut BitSet) -> bool {
    if visited.contains(var.id) {
        return true;  // Cycle detected
    }

    visited.insert(var.id);

    if let Some(atom) = bindings.resolve(var) {
        for v in atom.iter_variables() {
            if binding_has_loops(bindings, v, visited) {
                return true;
            }
        }
    }

    visited.remove(var.id);
    false
}
```

**Detection Method:**
1. Use bitset to track visited variables
2. Traverse binding graph depth-first
3. If we revisit a variable in current path → cycle

**Filtering**: `match_atoms()` filters out bindings with loops - `matcher.rs:1092-1094`:
```rust
Box::new(result.into_iter().filter(|b| !has_loops(b)))
```

### Examples

**Direct Loop:**
```metta
unify($x, (f $x))
; Attempted binding: $x ← (f $x)
; Occurs check: $x occurs in (f $x) ✗
; Filtered out
```

**Indirect Loop:**
```metta
unify(($x $y), ((f $y) (g $x)))
; Attempted bindings: {$x ← (f $y), $y ← (g $x)}
; $x depends on $y, $y depends on $x → cycle ✗
; Filtered out
```

**Valid Complex Binding:**
```metta
unify(($x $y), ((f $z) (g $z)))
; Bindings: {$x ← (f $z), $y ← (g $z)}
; No cycle: $z is free ✓
```

## Variable Equality vs Assignment

### Variable Equalities

**Specification**: Variables that must have the same value.

**Representation**: `{$x = $y = $z}`

**Usage**: When unifying two variables.

**Example:**
```metta
unify($x, $y)
; Creates equality: {$x = $y}

; Later, if $y is bound:
{$x = $y, $y ← Socrates}
; Resolves to: {$x ← Socrates, $y ← Socrates}
```

### Variable Assignments

**Specification**: Variable bound to a concrete atom.

**Representation**: `{$x ← atom}`

**Usage**: When unifying variable with non-variable.

**Example:**
```metta
unify($x, Socrates)
; Creates assignment: {$x ← Socrates}
```

### Mixed Example

```metta
; Unify ($a $b $c) with ($x $y 42)
; Step 1: $a and $x → {$a = $x}
; Step 2: $b and $y → {$a = $x, $b = $y}
; Step 3: $c and 42 → {$a = $x, $b = $y, $c ← 42}
```

### Transitive Resolution

**Implementation**: `Bindings::resolve()` - `matcher.rs:484-520`

**Algorithm:**
```rust
pub fn resolve(&self, var: &VariableAtom) -> Option<&Atom> {
    let mut current_var = var;
    loop {
        match self.get_binding(current_var) {
            Some(Binding::Var(next_var)) => {
                current_var = next_var;  // Follow chain
            }
            Some(Binding::Atom(atom, _)) => {
                return Some(atom);  // Found concrete binding
            }
            _ => return None
        }
    }
}
```

**Example:**
```metta
; Bindings: {$a = $b, $b = $c, $c ← 42}
; resolve($a):
;   $a → $b (variable)
;   $b → $c (variable)
;   $c → 42 (atom)
;   return 42
```

## Most General Unifier

### Definition

The **most general unifier (MGU)** is the least restrictive set of bindings that makes two atoms equal.

**Formal Property:**
```
If σ = MGU(a₁, a₂), then:
  ∀σ' where a₁[σ'] = a₂[σ']:
    ∃θ such that σ' = σ ∘ θ
```

**Meaning**: Any other unifier is an instance of the MGU.

### Examples

**Example 1:**
```metta
unify((f $x), (f 42))
; MGU: {$x ← 42}
; More specific (not general): {$x ← 42, $y ← 0} (extra binding)
```

**Example 2:**
```metta
unify(($x $y), ($a $a))
; MGU: {$x = $a, $y = $a}  (or {$x ← $a, $y ← $a})
; More specific: {$x ← 42, $y ← 42, $a ← 42}
```

**Property**: MeTTa's unification always returns MGU (when it exists).

## Unification Complexity

### Time Complexity

**Per Unification:**
- O(n) where n = max(size(left), size(right))
- Linear in size of atoms being unified

**Breakdown:**
- Variable binding: O(1)
- Symbol comparison: O(1)
- Expression: O(n) recursive traversal
- Binding merge: O(variables)

### Space Complexity

**Recursion Stack:**
- O(depth) where depth = max nesting level
- Typically small for reasonable expressions

**Bindings Storage:**
- O(v) where v = number of variables
- Plus overhead for equality chains

### Occurs Check Complexity

**Time:** O(v × d) where:
- v = number of variables
- d = average depth of binding chains

**Optimization**: Bitset tracking reduces constant factor

## Unification vs Pattern Matching

### Traditional Pattern Matching

**Definition**: Pattern has variables, data is ground.

**Direction**: One-way (pattern → data)

**Example (traditional):**
```
pattern: (Human $x)
data: (Human Socrates)
result: {$x ← Socrates}
```

### MeTTa Unification

**Definition**: Both sides can have variables.

**Direction**: Bidirectional (symmetric)

**Example (MeTTa):**
```metta
unify(($x $y), ($a $b))
; Result: {$x = $a, $y = $b}
; Both sides have variables!
```

### Practical Implications

**In Queries:**
```metta
; Traditional pattern matching feel:
!(match &self (Human $x) $x)
; Space has ground data, pattern has variable

; But unification allows:
!(match &self ($pred Socrates) $pred)
; Both variable and ground on pattern side
```

**In Rule Matching:**
```metta
; Rule: (= (mortal $x) (Human $x))
; Query: (mortal Socrates)
; Unify: (mortal $x) with (mortal Socrates)
; Result: {$x ← Socrates}
```

## Unification Properties

### Soundness

**Property**: If unification succeeds with σ, then atoms are equal after substitution.

**Formal:**
```
unify(a₁, a₂) = σ ⟹ a₁[σ] = a₂[σ]
```

### Completeness

**Property**: If atoms can be made equal, unification finds bindings.

**Formal:**
```
∃σ: a₁[σ] = a₂[σ] ⟹ unify(a₁, a₂) ≠ ⊥
```

### Symmetry

**Property**: Order doesn't matter.

**Formal:**
```
unify(a₁, a₂) ≡ unify(a₂, a₁)
```

**Implementation ensures**: Same bindings regardless of argument order.

### Idempotence

**Property**: Applying substitution to itself is stable.

**Formal:**
```
σ = unify(a₁, a₂)
─────────────────
σ[σ] = σ
```

## Related Operations

**match**: Uses unification for queries
- See: [03-match-operation.md](03-match-operation.md)

**Bindings**: Data structure for substitutions
- See: [04-bindings.md](04-bindings.md)

**Space Queries**: Apply unification over atom sets
- See: `../atom-space/05-space-operations.md`

## Summary

**Unification Algorithm:**
- **Bidirectional**: Both sides can have variables
- **Recursive**: Processes structure recursively
- **Symmetric**: Order-independent
- **MGU**: Returns most general unifier

**Key Rules:**
- Variable-Atom: Bind variable
- Variable-Variable: Create equality
- Symbol-Symbol: Must match exactly
- Expression-Expression: Recursive, same arity

**Occurs Check:**
- Prevents cyclic bindings
- Detects loops via graph traversal
- Filters invalid bindings

**Properties:**
- Sound and complete
- Symmetric and idempotent
- Linear time complexity
- Produces MGU

**Implementation:**
- Location: `hyperon-atom/src/matcher.rs`
- Entry: `match_atoms()`
- Recursive: `match_atoms_recursively()`
- Filtering: `has_loops()`

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
