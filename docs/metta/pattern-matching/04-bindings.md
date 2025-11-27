# Variable Bindings

## Overview

**Bindings** (also called substitutions) are the core data structure in MeTTa's pattern matching system. They represent the mapping from variables to atoms (or to other variables) that makes a pattern match succeed.

## What are Bindings?

### Specification

**Definition**: A set of variable-to-atom mappings and variable equalities.

**Formal Definition:**
```
Bindings = {
    Assignments: {$v₁ ← a₁, $v₂ ← a₂, ..., $vₙ ← aₙ}
    Equalities:  {$x₁ = $x₂ = ... = $xₘ}
}
```

**Purpose**: Represent the solution to a unification problem.

### Two Types of Bindings

**1. Variable Assignments:**
- Variable bound to concrete atom
- Format: `$x ← atom`
- Example: `{$x ← Socrates}`

**2. Variable Equalities:**
- Variables that must be equal
- Format: `$x = $y = $z`
- Example: `{$x = $y}`

### Why Two Types?

**Reason**: Symmetric unification - both sides can have variables.

**Example:**
```metta
; Unify ($a $b) with ($x $y)
; Cannot immediately assign concrete values
; Create equalities: {$a = $x, $b = $y}

; Later, if $x is bound:
; {$a = $x, $x ← Socrates}
; Resolve: {$a ← Socrates, $x ← Socrates}
```

## Bindings Implementation

### Data Structure

**Location**: `hyperon-atom/src/matcher.rs:140-765`

**Structure:**
```rust
pub struct Bindings {
    // Map variables to binding group IDs
    values: HashMap<VariableAtom, usize>,

    // Binding groups (holes for variable-only groups)
    bindings: HoleyVec<Binding>,
}
```

**Two-Level Structure:**
1. **Variable Map**: Maps each variable to a binding group ID
2. **Binding Groups**: Actual bindings indexed by group ID

### Binding Enum

**Location**: `matcher.rs:100-110`

```rust
pub enum Binding {
    Empty,                      // No variables in group
    Var(VariableAtom),          // Single variable (equality)
    Link(usize),                // Points to another group
    Atom(Atom, usize),          // Bound to atom (with generation)
}
```

**Variants:**
- **Empty**: Placeholder (no variables)
- **Var**: Represents variable equality chain
- **Link**: Indirection to merged groups
- **Atom**: Concrete binding with generation counter

### Generation Counter

**Purpose**: Track binding changes for incremental updates.

**Usage**: `Atom(atom, generation)`
- Generation increments when binding changes
- Allows detecting stale bindings

## Creating Bindings

### Empty Bindings

**Creation:**
```rust
let bindings = Bindings::new();
```

**Represents**: No variables bound yet.

### Adding Variable Bindings

**Method**: `add_var_binding()` - `matcher.rs:398-450`

**Signature:**
```rust
pub fn add_var_binding(&self, var: VariableAtom, atom: Atom) -> BindingsSet
```

**Process:**
1. Check if variable already bound
2. If bound, check consistency
3. If consistent or unbound, create new binding
4. May return multiple binding sets (split)

**Example:**
```rust
let bindings = Bindings::new();
let bindings = bindings.add_var_binding(var_x, atom_socrates);
// Result: {$x ← Socrates}
```

### Adding Variable Equalities

**Method**: `add_var_equality()` - `matcher.rs:452-480`

**Signature:**
```rust
pub fn add_var_equality(&self, left: VariableAtom, right: VariableAtom) -> BindingsSet
```

**Process:**
1. Check if either variable already bound
2. Merge binding groups
3. Maintain transitivity

**Example:**
```rust
let bindings = bindings.add_var_equality(var_x, var_y);
// Result: {$x = $y}
```

## Resolving Bindings

### Variable Resolution

**Method**: `resolve()` - `matcher.rs:484-520`

**Signature:**
```rust
pub fn resolve(&self, var: &VariableAtom) -> Option<&Atom>
```

**Process:**
1. Follow variable equality chains
2. Return final concrete binding
3. Return `None` if unbound

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
            Some(Binding::Link(target)) => {
                // Follow link to merged group
                current_var = self.get_var_from_group(target);
            }
            _ => return None  // Unbound
        }
    }
}
```

**Example:**
```rust
// Bindings: {$a = $b, $b = $c, $c ← Socrates}
bindings.resolve(&var_a);
// Follows: $a → $b → $c → Socrates
// Returns: Some(Socrates)
```

### Resolve All

**Method**: `resolve_and_subs()` - `matcher.rs:522-545`

**Purpose**: Apply bindings to entire atom (recursive substitution).

**Signature:**
```rust
pub fn resolve_and_subs(&self, atom: &Atom) -> Atom
```

**Process:**
1. If atom is variable, resolve it
2. If atom is expression, recursively substitute children
3. Return substituted atom

**Example:**
```rust
// Bindings: {$x ← Socrates, $y ← 70}
// Atom: (age $x $y)
bindings.resolve_and_subs(&atom);
// Returns: (age Socrates 70)
```

## Binding Operations

### Merging Bindings

**Method**: `merge()` - `matcher.rs:547-620`

**Purpose**: Combine two compatible binding sets.

**Signature:**
```rust
pub fn merge(&self, other: &Bindings) -> Option<Bindings>
```

**Process:**
1. Check compatibility (no conflicts)
2. Combine variable mappings
3. Merge binding groups
4. Return unified bindings or None if incompatible

**Example:**
```rust
// b1: {$x ← A}
// b2: {$y ← B}
b1.merge(&b2);
// Result: Some({$x ← A, $y ← B})

// b1: {$x ← A}
// b2: {$x ← B}  (conflict!)
b1.merge(&b2);
// Result: None
```

### Narrowing Bindings

**Method**: `narrow_vars()` - `matcher.rs:622-660`

**Purpose**: Extract bindings for specific variables.

**Signature:**
```rust
pub fn narrow_vars(&self, vars: &VariableSet) -> Bindings
```

**Use Case**: Return only bindings for variables in template.

**Example:**
```rust
// Bindings: {$x ← A, $y ← B, $z ← C}
// Keep only: {$x, $y}
bindings.narrow_vars(&var_set);
// Result: {$x ← A, $y ← B}
```

## BindingsSet

### What is a BindingsSet?

**Definition**: Represents multiple possible binding solutions.

**Location**: `matcher.rs:886-1044`

**Structure:**
```rust
pub enum BindingsSet {
    Single(Bindings),              // One solution
    Multi(Vec<Bindings>),          // Multiple solutions
    Empty,                         // No solutions (failure)
}
```

**Usage**: Unification can produce multiple results.

### Creating BindingsSets

**Empty** (no matches):
```rust
BindingsSet::empty()
```

**Single** (one match):
```rust
BindingsSet::single(bindings)
```

**Multiple** (many matches):
```rust
BindingsSet::from_vec(vec![b1, b2, b3])
```

### BindingsSet Operations

**union** - `matcher.rs:950-970`:
```rust
pub fn union(self, other: BindingsSet) -> BindingsSet
```
Combines two sets (concatenates solutions).

**merge_v2** - `matcher.rs:1003-1023`:
```rust
pub fn merge_v2<F>(self, left: &Atom, right: &Atom, f: F) -> Self
where F: Fn(&Atom, &Atom, Bindings) -> BindingsSet
```
Applies unification function to each binding.

**Example:**
```rust
// Start: {$x ← A}
// Match: ($x $y) with (A B)
// Result: {$x ← A, $y ← B}
```

## Binding Consistency

### Conflict Detection

**Definition**: Two bindings conflict if they bind the same variable to different atoms.

**Check Location**: `add_var_binding()` - `matcher.rs:410-430`

**Algorithm:**
```rust
if let Some(existing) = self.resolve(var) {
    if existing != atom {
        return BindingsSet::empty();  // Conflict!
    }
}
```

**Example:**
```metta
; Pattern: ($x foo $x)
; Atom: (A foo B)
; Step 1: $x ← A
; Step 2: $x ← B (conflict with step 1)
; Result: BindingsSet::empty() (no match)
```

### Consistency Enforcement

**Invariant**: All bindings in a `Bindings` must be consistent.

**Maintained by**:
- `add_var_binding()` checks before adding
- `merge()` checks before combining
- Unification algorithm ensures consistency

## Binding Examples

### Simple Binding

```rust
// Pattern: $x
// Atom: Socrates
let bindings = Bindings::new()
    .add_var_binding(var_x, atom_socrates);
// Result: {$x ← Socrates}
```

### Multiple Bindings

```rust
// Pattern: ($x $y)
// Atom: (A B)
let bindings = Bindings::new()
    .add_var_binding(var_x, atom_a)
    .add_var_binding(var_y, atom_b);
// Result: {$x ← A, $y ← B}
```

### Variable Equality

```rust
// Pattern: ($a $b)
// Atom: ($x $y)
let bindings = Bindings::new()
    .add_var_equality(var_a, var_x)
    .add_var_equality(var_b, var_y);
// Result: {$a = $x, $b = $y}
```

### Transitive Binding

```rust
// Create chain: $a = $b, $b ← Socrates
let bindings = Bindings::new()
    .add_var_equality(var_a, var_b)
    .add_var_binding(var_b, atom_socrates);

// Resolve $a
bindings.resolve(&var_a);
// Returns: Some(Socrates) (follows $a → $b → Socrates)
```

### Shared Variables

```rust
// Pattern: (same $x $x)
// Atom: (same A A)
let bindings = Bindings::new()
    .add_var_binding(var_x, atom_a);  // First $x
    .add_var_binding(var_x, atom_a);  // Second $x (consistent)
// Result: {$x ← A}

// Pattern: (same $x $x)
// Atom: (same A B)
let bindings = Bindings::new()
    .add_var_binding(var_x, atom_a);  // First $x
    .add_var_binding(var_x, atom_b);  // Second $x (conflict!)
// Result: BindingsSet::empty()
```

## Applying Bindings

### To Single Atom

**Function**: `apply_bindings_to_atom_move()` - `matcher.rs:1131-1179`

**Signature:**
```rust
pub fn apply_bindings_to_atom_move(atom: Atom, bindings: &Bindings) -> Atom
```

**Process:**
1. If variable, resolve and return bound value
2. If expression, recursively apply to children
3. Otherwise, return atom unchanged

**Example:**
```rust
// Bindings: {$x ← Socrates}
// Atom: (Human $x)
apply_bindings_to_atom_move(atom, &bindings);
// Returns: (Human Socrates)
```

### To Expression

**Recursive Application:**
```rust
match atom {
    Atom::Variable(v) => {
        bindings.resolve(&v)
            .cloned()
            .unwrap_or(Atom::Variable(v))
    }
    Atom::Expression(expr) => {
        let children = expr.children()
            .iter()
            .map(|child| apply_bindings_to_atom_move(child.clone(), bindings))
            .collect();
        Atom::Expression(ExpressionAtom::new(children))
    }
    other => other
}
```

## Binding Internals

### HoleyVec

**Purpose**: Vector that can have holes (None values).

**Usage**: Store binding groups with possible gaps.

**Benefits:**
- Efficient for sparse data
- Allows reusing IDs
- Compact representation

### Variable IDs

**Purpose**: Distinguish variables with same name in different scopes.

**Format**: `<name>[#<id>]`

**Example:**
- User writes: `$x`
- Internal: `$x#42` (unique ID)

**Equality**: Variables equal only if name AND ID match.

### Binding Groups

**Concept**: Variables that are equal belong to same group.

**Example:**
```
{$a = $b = $c} → all in group 0
{$x = $y}      → all in group 1
{$z ← 42}      → group 2 with atom binding
```

**Links**: Groups can be merged via Link bindings.

## Performance Considerations

### Time Complexity

**Operations:**
- `add_var_binding()`: O(1) average, O(d) worst (d = chain depth)
- `add_var_equality()`: O(1) average
- `resolve()`: O(d) where d = equality chain length
- `merge()`: O(v) where v = number of variables
- `apply_bindings()`: O(size(atom))

### Space Complexity

**Per Bindings:**
- O(v) where v = number of variables
- Plus O(g) where g = number of groups

**BindingsSet:**
- O(n × v) where n = number of binding sets

### Optimization Strategies

**1. Shallow Chains:**
- Keep variable equality chains short
- Compress chains during resolution

**2. Efficient Merging:**
- Check conflicts early
- Avoid unnecessary copying

**3. Lazy Resolution:**
- Don't resolve until needed
- Cache resolved values

## Common Patterns

### Building Incrementally

```rust
let mut bindings = Bindings::new();
bindings = bindings.add_var_binding(var_x, atom_a)?;
bindings = bindings.add_var_binding(var_y, atom_b)?;
bindings = bindings.add_var_binding(var_z, atom_c)?;
```

### Checking Consistency

```rust
match bindings.add_var_binding(var, atom) {
    BindingsSet::Single(b) => {
        // Success, use b
    }
    BindingsSet::Empty => {
        // Conflict, binding failed
    }
    BindingsSet::Multi(bs) => {
        // Multiple solutions
    }
}
```

### Extracting Results

```rust
for bindings in bindings_set.into_iter() {
    let result = apply_bindings_to_atom_move(template.clone(), &bindings);
    results.push(result);
}
```

## Debugging Bindings

### Display Implementation

**Format**: `{$x ← Atom, $y = $z}`

**Example Output:**
```
{$x ← Socrates, $y ← 70}
{$a = $b, $c ← 42}
{}  (empty bindings)
```

### Inspecting Bindings

```rust
// Check if variable is bound
if let Some(atom) = bindings.resolve(&var) {
    println!("Variable bound to: {:?}", atom);
} else {
    println!("Variable unbound");
}

// Iterate all variables
for var in bindings.variables() {
    println!("Variable: {:?}", var);
}
```

## Best Practices

### 1. Check for Conflicts

```rust
match bindings.add_var_binding(var, atom) {
    BindingsSet::Empty => {
        // Handle conflict
    }
    result => {
        // Use result
    }
}
```

### 2. Use Appropriate Type

```rust
// For single solution
Bindings

// For multiple solutions
BindingsSet
```

### 3. Resolve Early

```rust
// Resolve once, use many times
let resolved = bindings.resolve(&var);
// Use resolved value multiple times
```

### 4. Minimize Copying

```rust
// Use references where possible
fn process(bindings: &Bindings) { ... }

// Clone only when necessary
let copy = bindings.clone();
```

## Related Concepts

**Unification**: [02-unification.md](02-unification.md) - Produces bindings
**Match Operation**: [03-match-operation.md](03-match-operation.md) - Uses bindings
**Non-Determinism**: [08-non-determinism.md](08-non-determinism.md) - BindingsSet

## Summary

**Bindings:**
- **Purpose**: Map variables to atoms
- **Types**: Assignments ($x ← atom) and Equalities ($x = $y)
- **Implementation**: Two-level structure with groups
- **Resolution**: Follow chains to get final values

**BindingsSet:**
- **Purpose**: Represent multiple solutions
- **Variants**: Empty, Single, Multi
- **Operations**: union, merge, filter

**Key Operations:**
- `add_var_binding()` - Bind variable to atom
- `add_var_equality()` - Assert variable equality
- `resolve()` - Get bound value
- `merge()` - Combine bindings
- `apply_bindings()` - Substitute in atoms

**Properties:**
- Consistency enforced
- Transitive resolution
- Conflict detection
- Efficient representation

**Implementation:**
- Location: `hyperon-atom/src/matcher.rs:140-765`
- Complexity: O(v) space, O(d) resolution time
- Optimized: Shallow chains, lazy evaluation

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
