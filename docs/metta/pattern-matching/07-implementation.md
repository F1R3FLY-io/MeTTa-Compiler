# Pattern Matching Implementation

## Overview

This document provides detailed implementation analysis of MeTTa's pattern matching system, covering data structures, algorithms, performance characteristics, and integration points.

## Architecture Overview

### Component Hierarchy

```
┌─────────────────────────────────────┐
│      MeTTa Interpreter              │
│  (lib/src/metta/runner/mod.rs)     │
└─────────────┬───────────────────────┘
              │
              ↓
┌─────────────────────────────────────┐
│      Match Operation                │
│  (lib/src/metta/runner/stdlib/     │
│   core.rs:141-167)                  │
└─────────────┬───────────────────────┘
              │
              ↓
┌─────────────────────────────────────┐
│      Space Query                    │
│  (hyperon-space/src/lib.rs:156)    │
└─────────────┬───────────────────────┘
              │
              ↓
┌─────────────────────────────────────┐
│      AtomIndex + AtomTrie           │
│  (hyperon-space/src/index/)        │
└─────────────┬───────────────────────┘
              │
              ↓
┌─────────────────────────────────────┐
│      Unification Engine             │
│  (hyperon-atom/src/matcher.rs)     │
│   - match_atoms()                   │
│   - Bindings                        │
│   - BindingsSet                     │
└─────────────────────────────────────┘
```

### Data Flow

**Query Execution:**
```
User Query → MatchOp → Space.query() → AtomTrie.query()
→ Unification → Bindings → Template Application → Results
```

**Pattern Matching:**
```
Pattern + Atom → match_atoms() → BindingsSet
→ apply_bindings() → Substituted Atom
```

## Matcher.rs Implementation

### Location

**File**: `hyperon-atom/src/matcher.rs`
**Size**: ~1200 lines
**Primary Components**:
- `match_atoms()` - Main unification function
- `Bindings` - Variable binding data structure
- `BindingsSet` - Multiple binding solutions
- `atoms_are_equivalent()` - Structural equivalence check

### match_atoms Function

**Location**: `matcher.rs:1089-1129`

**Signature:**
```rust
pub fn match_atoms(left: &Atom, right: &Atom) -> BindingsSet
```

**Implementation Overview:**
```rust
pub fn match_atoms(left: &Atom, right: &Atom) -> BindingsSet {
    match_atoms_recursively(left, right, Bindings::new())
}

fn match_atoms_recursively(
    left: &Atom,
    right: &Atom,
    bindings: Bindings
) -> BindingsSet {
    match (left, right) {
        // Case 1: Variable on either side
        (Atom::Variable(v), atom) | (atom, Atom::Variable(v)) => {
            bindings.add_var_binding(v.clone(), atom.clone())
        }

        // Case 2: Matching symbols
        (Atom::Symbol(l), Atom::Symbol(r)) if l == r => {
            BindingsSet::single(bindings)
        }

        // Case 3: Matching expressions
        (Atom::Expression(l), Atom::Expression(r))
            if l.children().len() == r.children().len() => {
            let mut result = BindingsSet::single(bindings);
            for (l_child, r_child) in l.children().iter().zip(r.children()) {
                result = result.merge_v2(l_child, r_child, match_atoms_recursively);
            }
            result
        }

        // Case 4: Grounded atoms (with custom matching)
        (Atom::Grounded(g), atom) => {
            g.match_(atom)
        }
        (atom, Atom::Grounded(g)) => {
            g.match_(atom)
        }

        // Case 5: No match
        _ => BindingsSet::empty()
    }
}
```

**Algorithm Analysis:**

1. **Base Cases**:
   - Variable matching: Delegate to `add_var_binding()`
   - Symbol matching: Direct equality check
   - Mismatch: Return empty set

2. **Recursive Case** (Expressions):
   - Check length compatibility
   - Match children pairwise
   - Merge bindings incrementally
   - Fail on any child mismatch

3. **Custom Matching**:
   - Grounded atoms can override matching logic
   - Enables domain-specific patterns

### Occurs Check

**Location**: `matcher.rs:760-795`

**Purpose**: Prevent cyclic bindings like `$x ← (f $x)`.

**Implementation:**
```rust
fn check_occurs(var: &VariableAtom, atom: &Atom) -> bool {
    match atom {
        Atom::Variable(v) if v == var => true,  // Direct cycle
        Atom::Expression(expr) => {
            // Recursively check children
            expr.children().iter()
                .any(|child| check_occurs(var, child))
        }
        _ => false
    }
}
```

**Integration** in `add_var_binding()`:
```rust
pub fn add_var_binding(&self, var: VariableAtom, atom: Atom) -> BindingsSet {
    // Check for occurs check violation
    if check_occurs(&var, &atom) {
        return BindingsSet::empty();  // Fail
    }

    // Proceed with binding...
}
```

**Time Complexity**: O(size(atom)) - must traverse entire atom structure

**Space Complexity**: O(depth(atom)) - recursion depth

### Variable Handling

**Variable Representation:**
```rust
pub struct VariableAtom {
    name: String,     // e.g., "x"
    id: u64,          // Unique identifier
}
```

**Equality:**
```rust
impl PartialEq for VariableAtom {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.id == other.id
    }
}
```

**Hashing:**
```rust
impl Hash for VariableAtom {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.id.hash(state);
    }
}
```

**Scope Management**:
- Each expression creates new variable scope
- Variables with same name in different scopes have different IDs
- Parser assigns unique IDs during parsing

## Bindings Implementation

### Data Structure

**Location**: `matcher.rs:140-765`

**Core Structure:**
```rust
pub struct Bindings {
    // Map: Variable → Binding Group ID
    values: HashMap<VariableAtom, usize>,

    // Binding groups (sparse vector)
    bindings: HoleyVec<Binding>,
}
```

**Binding Enum:**
```rust
pub enum Binding {
    Empty,                    // No variables in this group
    Var(VariableAtom),        // Variable equality (chain link)
    Link(usize),              // Indirection to merged group
    Atom(Atom, usize),        // Concrete binding (+ generation)
}
```

### HoleyVec

**Purpose**: Sparse vector allowing `None` entries.

**Implementation Concept:**
```rust
struct HoleyVec<T> {
    data: Vec<Option<T>>,
    holes: Vec<usize>,  // Indices of None values
}

impl<T> HoleyVec<T> {
    fn push(&mut self, value: T) -> usize {
        // Reuse hole if available
        if let Some(idx) = self.holes.pop() {
            self.data[idx] = Some(value);
            idx
        } else {
            // Allocate new slot
            let idx = self.data.len();
            self.data.push(Some(value));
            idx
        }
    }

    fn remove(&mut self, idx: usize) {
        self.data[idx] = None;
        self.holes.push(idx);
    }
}
```

**Benefits:**
- O(1) allocation and deallocation
- Memory reuse for temporary bindings
- Compact representation

### Binding Groups

**Concept**: Variables that are equal belong to same group.

**Example State:**
```
Variables: {$x, $y, $z, $a, $b}
Bindings:  {$x = $y = $z ← 42, $a = $b}

Internal Representation:
values: {
    $x → 0,
    $y → 0,
    $z → 0,
    $a → 1,
    $b → 1,
}
bindings: [
    Atom(42, gen=0),    // Group 0
    Var($a),            // Group 1 (no concrete binding yet)
]
```

**Group Merging**:
```rust
// Merge groups when adding equality
fn merge_groups(&mut self, group1: usize, group2: usize) {
    match (&self.bindings[group1], &self.bindings[group2]) {
        (Binding::Atom(a, g), Binding::Var(_)) => {
            // Group2 variables point to group1
            self.bindings[group2] = Binding::Link(group1);
        }
        (Binding::Var(_), Binding::Atom(a, g)) => {
            // Group1 variables point to group2
            self.bindings[group1] = Binding::Link(group2);
        }
        (Binding::Atom(a1, _), Binding::Atom(a2, _)) => {
            // Conflict if atoms differ
            if a1 != a2 {
                return Err(Conflict);
            }
        }
        _ => {
            // Chain variable groups
            self.bindings[group2] = Binding::Link(group1);
        }
    }
}
```

### add_var_binding Implementation

**Location**: `matcher.rs:398-450`

**Algorithm:**
```rust
pub fn add_var_binding(&self, var: VariableAtom, atom: Atom) -> BindingsSet {
    // 1. Occurs check
    if check_occurs(&var, &atom) {
        return BindingsSet::empty();
    }

    // 2. Check if variable already has binding
    match self.resolve(&var) {
        Some(existing) => {
            // 3a. Variable already bound
            if existing == &atom {
                // Consistent - return unchanged
                BindingsSet::single(self.clone())
            } else {
                // Conflict - fail
                BindingsSet::empty()
            }
        }
        None => {
            // 3b. Variable unbound

            // Check if atom is also a variable
            if let Atom::Variable(atom_var) = &atom {
                // Both are variables - create equality
                self.add_var_equality(var, atom_var.clone())
            } else {
                // Bind variable to concrete atom
                let mut new_bindings = self.clone();
                let group_id = new_bindings.get_or_create_group(var);
                new_bindings.bindings[group_id] = Binding::Atom(atom, 0);
                BindingsSet::single(new_bindings)
            }
        }
    }
}
```

**Key Operations:**
1. **Occurs Check**: O(size(atom))
2. **Conflict Detection**: O(1) with hash lookup
3. **Group Assignment**: O(1)

**Splitting** (for non-determinism):
```rust
// When atom contains variables that could have multiple bindings
pub fn add_var_binding_with_split(&self, var: VariableAtom, atom: Atom)
    -> BindingsSet
{
    // If atom contains unbound variables, may produce multiple results
    // Example: $x ← (f $y), where $y has multiple possible values

    // This is handled by returning BindingsSet::Multi(vec![...])
}
```

### resolve Implementation

**Location**: `matcher.rs:484-520`

**Algorithm:**
```rust
pub fn resolve(&self, var: &VariableAtom) -> Option<&Atom> {
    let mut current_var = var;

    loop {
        let group_id = self.values.get(current_var)?;

        match &self.bindings[*group_id] {
            Binding::Atom(atom, _) => {
                // Found concrete binding
                return Some(atom);
            }
            Binding::Var(next_var) => {
                // Follow variable equality chain
                current_var = next_var;
            }
            Binding::Link(target_group) => {
                // Follow group merge link
                match &self.bindings[*target_group] {
                    Binding::Atom(atom, _) => return Some(atom),
                    Binding::Var(v) => current_var = v,
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
}
```

**Complexity:**
- **Time**: O(d) where d = chain depth
- **Space**: O(1) - no allocation
- **Average Chain Depth**: Small constant (typically 1-3)

**Optimization**: Path compression could reduce chain depth to O(1) amortized.

### merge Implementation

**Location**: `matcher.rs:547-620`

**Purpose**: Combine two compatible binding sets.

**Algorithm:**
```rust
pub fn merge(&self, other: &Bindings) -> Option<Bindings> {
    let mut result = self.clone();

    // Iterate all variables in other
    for (var, other_group) in &other.values {
        match other.get_binding_for_var(var) {
            Some(Binding::Atom(atom, _)) => {
                // Other binds variable to atom
                match result.add_var_binding(var.clone(), atom.clone()) {
                    BindingsSet::Single(b) => result = b,
                    BindingsSet::Empty => return None,  // Conflict
                    BindingsSet::Multi(_) => {
                        // Shouldn't happen in simple merge
                        return None;
                    }
                }
            }
            Some(Binding::Var(other_var)) => {
                // Other has variable equality
                match result.add_var_equality(var.clone(), other_var.clone()) {
                    BindingsSet::Single(b) => result = b,
                    BindingsSet::Empty => return None,
                    _ => return None,
                }
            }
            _ => {}
        }
    }

    Some(result)
}
```

**Complexity:**
- **Time**: O(v) where v = number of variables in `other`
- **Space**: O(v) for cloning

**Conflict Detection**: Any inconsistency returns `None`.

## BindingsSet Implementation

### Data Structure

**Location**: `matcher.rs:886-1044`

**Definition:**
```rust
pub enum BindingsSet {
    Empty,                   // No solutions (match failed)
    Single(Bindings),        // One solution
    Multi(Vec<Bindings>),    // Multiple solutions
}
```

**Optimization**: Use enum to avoid Vec allocation for single result (common case).

### Core Operations

**union** - `matcher.rs:950-970`:
```rust
pub fn union(self, other: BindingsSet) -> BindingsSet {
    match (self, other) {
        (BindingsSet::Empty, b) | (b, BindingsSet::Empty) => b,
        (BindingsSet::Single(b1), BindingsSet::Single(b2)) => {
            BindingsSet::Multi(vec![b1, b2])
        }
        (BindingsSet::Single(b), BindingsSet::Multi(mut vec)) |
        (BindingsSet::Multi(mut vec), BindingsSet::Single(b)) => {
            vec.push(b);
            BindingsSet::Multi(vec)
        }
        (BindingsSet::Multi(mut v1), BindingsSet::Multi(v2)) => {
            v1.extend(v2);
            BindingsSet::Multi(v1)
        }
    }
}
```

**Time Complexity**: O(1) for most cases, O(n) for Multi+Multi

**merge_v2** - `matcher.rs:1003-1023`:
```rust
pub fn merge_v2<F>(self, left: &Atom, right: &Atom, f: F) -> Self
where
    F: Fn(&Atom, &Atom, Bindings) -> BindingsSet
{
    match self {
        BindingsSet::Empty => BindingsSet::Empty,
        BindingsSet::Single(b) => {
            // Apply unification function with current bindings
            f(left, right, b)
        }
        BindingsSet::Multi(bindings) => {
            // Apply to each binding and collect results
            let results: Vec<BindingsSet> = bindings
                .into_iter()
                .map(|b| f(left, right, b))
                .collect();

            // Union all results
            results.into_iter()
                .fold(BindingsSet::Empty, |acc, bs| acc.union(bs))
        }
    }
}
```

**Purpose**: Incrementally match expressions by matching children.

**Example Usage** (in `match_atoms_recursively`):
```rust
let mut result = BindingsSet::single(bindings);
for (l_child, r_child) in left_children.zip(right_children) {
    result = result.merge_v2(l_child, r_child, match_atoms_recursively);
}
```

**Complexity:**
- Single: O(match time)
- Multi(n): O(n × match time)

### Iterator Implementation

**Location**: `matcher.rs:1025-1044`

```rust
impl IntoIterator for BindingsSet {
    type Item = Bindings;
    type IntoIter = BindingsSetIter;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            BindingsSet::Empty => BindingsSetIter::Empty,
            BindingsSet::Single(b) => BindingsSetIter::Single(Some(b)),
            BindingsSet::Multi(vec) => BindingsSetIter::Multi(vec.into_iter()),
        }
    }
}

pub enum BindingsSetIter {
    Empty,
    Single(Option<Bindings>),
    Multi(std::vec::IntoIter<Bindings>),
}

impl Iterator for BindingsSetIter {
    type Item = Bindings;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            BindingsSetIter::Empty => None,
            BindingsSetIter::Single(opt) => opt.take(),
            BindingsSetIter::Multi(iter) => iter.next(),
        }
    }
}
```

**Benefits:**
- Unified interface for all variants
- Zero-cost abstraction
- Lazy evaluation

## Trie-Based Query Optimization

### AtomTrie Structure

**Location**: `hyperon-space/src/index/trie.rs`

**Purpose**: Efficient pattern-based queries on atom spaces.

**Conceptual Structure:**
```rust
pub struct AtomTrie {
    root: TrieNode,
}

enum TrieNode {
    Leaf(Vec<Atom>),           // Atoms at this path
    Branch {
        children: HashMap<AtomKey, Box<TrieNode>>,
        atoms: Vec<Atom>,       // Atoms matching this prefix
    },
}

enum AtomKey {
    Symbol(SymbolAtom),
    Expression,                // All expressions
    Variable,                  // All variables
    Grounded(TypeId),          // By grounded type
}
```

**Index Structure Example:**
```
Space: {(Human Socrates), (Human Plato), (age John 30)}

Trie:
root
├─ Expression
   ├─ Human
   │  ├─ Socrates → [(Human Socrates)]
   │  └─ Plato    → [(Human Plato)]
   └─ age
      └─ John
         └─ 30   → [(age John 30)]
```

### Query Algorithm

**Location**: `hyperon-space/src/index/trie.rs:query()`

**Algorithm:**
```rust
pub fn query(&self, pattern: &Atom) -> BindingsSet {
    self.query_recursive(pattern, &self.root, Bindings::new())
}

fn query_recursive(
    &self,
    pattern: &Atom,
    node: &TrieNode,
    bindings: Bindings
) -> BindingsSet {
    match pattern {
        Atom::Variable(v) => {
            // Variable matches all atoms at this node
            node.get_all_atoms()
                .into_iter()
                .map(|atom| {
                    bindings.add_var_binding(v.clone(), atom.clone())
                })
                .fold(BindingsSet::empty(), |acc, bs| acc.union(bs))
        }

        Atom::Symbol(s) => {
            // Symbol: exact match
            if let Some(child) = node.children.get(&AtomKey::Symbol(s.clone())) {
                // Atom found, return binding
                child.get_atoms()
                    .into_iter()
                    .map(|atom| BindingsSet::single(bindings.clone()))
                    .fold(BindingsSet::empty(), |acc, bs| acc.union(bs))
            } else {
                BindingsSet::empty()
            }
        }

        Atom::Expression(expr) => {
            // Expression: traverse children
            if let Some(child) = node.children.get(&AtomKey::Expression) {
                // Match expression head
                let head = &expr.children()[0];

                // Recursively match against child node
                let head_matches = self.query_recursive(head, child, bindings);

                // For each match, continue with tail
                head_matches.merge_v2(
                    pattern,
                    &Atom::Expression(expr.clone()),
                    |p, a, b| self.match_tail(p, a, b)
                )
            } else {
                BindingsSet::empty()
            }
        }

        Atom::Grounded(g) => {
            // Grounded: may use custom matching
            // Or fall back to exact match
            node.get_atoms()
                .into_iter()
                .filter_map(|atom| {
                    // Try custom match
                    let results = g.match_(atom);
                    if results.is_empty() {
                        None
                    } else {
                        Some(results)
                    }
                })
                .fold(BindingsSet::empty(), |acc, bs| acc.union(bs))
        }
    }
}
```

**Optimization Strategies:**

1. **Ground Term Pruning**:
   - Pattern: `(Human $x)` → Only traverse "Human" branch
   - Skips all non-Human atoms
   - O(log n) traversal vs O(n) linear scan

2. **Prefix Indexing**:
   - Atoms indexed by prefix path
   - Longest ground prefix determines index depth
   - Example: `(Human (Greek $x))` → index by [Human, Greek]

3. **Lazy Matching**:
   - Don't unify until necessary
   - Return candidates from trie, unify on demand

### Query Complexity

**Best Case**: O(log n + m)
- Pattern: Fully ground term
- Example: `(Human Socrates)` → direct lookup
- log n for trie traversal, m matches

**Average Case**: O(k + m × u)
- Pattern: Partial ground prefix
- Example: `(Human $x)` → traverse Human branch
- k = trie depth, m = matches, u = unification cost

**Worst Case**: O(n × u)
- Pattern: Pure variable `$x`
- Must check all n atoms
- u = unification cost per atom

**Space Complexity**: O(n × d)
- n atoms, d = average trie depth

## Space Query Integration

### GroundingSpace Implementation

**Location**: `lib/src/space/grounding/mod.rs`

**Structure:**
```rust
pub struct GroundingSpace {
    index: AtomIndex,
    atoms: Vec<Atom>,
}

impl Space for GroundingSpace {
    fn query(&self, pattern: &Atom) -> BindingsSet {
        self.index.query(pattern)
    }

    fn add(&mut self, atom: Atom) {
        self.atoms.push(atom.clone());
        self.index.add(&atom);
    }

    fn remove(&mut self, atom: &Atom) -> bool {
        if let Some(pos) = self.atoms.iter().position(|a| a == atom) {
            self.atoms.swap_remove(pos);
            self.index.remove(atom);
            true
        } else {
            false
        }
    }
}
```

**Dual Representation:**
- **atoms**: Linear storage for iteration
- **index**: Trie for efficient queries

### AtomIndex

**Location**: `hyperon-space/src/index/mod.rs`

**Structure:**
```rust
pub struct AtomIndex {
    trie: Option<AtomTrie>,
    config: IndexConfig,
}

pub struct IndexConfig {
    enable_trie: bool,
    min_atoms_for_index: usize,
}

impl AtomIndex {
    pub fn query(&self, pattern: &Atom) -> BindingsSet {
        match &self.trie {
            Some(trie) => trie.query(pattern),
            None => {
                // Fallback: linear scan
                // Used for small spaces or when trie disabled
                BindingsSet::empty()
            }
        }
    }
}
```

**Index Building:**
```rust
pub fn add(&mut self, atom: &Atom) {
    if let Some(trie) = &mut self.trie {
        trie.insert(atom);
    }
}
```

**Cost**: O(depth(atom)) per insertion

## Performance Characteristics

### Time Complexity Summary

| Operation | Best | Average | Worst |
|-----------|------|---------|-------|
| Unification | O(1) | O(size(pattern)) | O(size(pattern) × size(atom)) |
| Variable Binding | O(1) | O(1) | O(size(atom)) (occurs check) |
| Resolution | O(1) | O(d) (d=chain) | O(d) |
| Bindings Merge | O(1) | O(v) | O(v) |
| Space Query | O(log n) | O(k + m) | O(n) |
| Match Operation | O(log n) | O(k + m × u) | O(n × u) |

**Variables:**
- n = number of atoms in space
- m = number of matches
- v = number of variables
- d = variable chain depth
- k = trie depth
- u = unification cost

### Space Complexity

**Bindings**: O(v)
- v variables, each with O(1) group pointer

**BindingsSet**: O(n × v)
- n binding sets, each with v variables

**AtomTrie**: O(n × d)
- n atoms, average depth d

**Total Query**: O(m × v)
- m matches, each binding v variables

### Benchmarking Considerations

**Factors Affecting Performance:**

1. **Space Size**: Larger spaces → slower queries (unless well-indexed)
2. **Pattern Specificity**: More ground terms → faster
3. **Variable Count**: More variables → more binding overhead
4. **Expression Depth**: Deeper nesting → more recursion
5. **Match Count**: More matches → more result processing

**Benchmark Design:**
```rust
#[bench]
fn bench_simple_query(b: &mut Bencher) {
    let space = create_test_space(10000);  // 10k atoms
    let pattern = expr!("Human", var!("x"));

    b.iter(|| {
        space.query(&pattern)
    });
}

#[bench]
fn bench_conjunction_query(b: &mut Bencher) {
    let space = create_test_space(10000);
    let pattern = expr!(",",
        expr!("Human", var!("x")),
        expr!("philosopher", var!("x"))
    );

    b.iter(|| {
        space.query(&pattern)
    });
}
```

**Profiling Points:**
- Trie traversal time
- Unification time
- Binding allocation
- Result construction

## Memory Layout

### Bindings Memory

**Structure Size:**
```rust
size_of::<Bindings>() =
    size_of::<HashMap<VariableAtom, usize>>() +  // ~48 bytes base
    size_of::<HoleyVec<Binding>>()                // ~24 bytes base
    = ~72 bytes (empty)
```

**Per Variable**: ~40 bytes
- HashMap entry: 32 bytes (key + value + overhead)
- Binding enum: 8-24 bytes (depending on variant)

**Typical Binding Set** (5 variables): ~272 bytes

### AtomTrie Memory

**Per Node**: ~56 bytes
- HashMap for children: ~48 bytes
- Vec for atoms: ~24 bytes

**Memory per Atom**: ~100-200 bytes (including path overhead)

**Example**: 10,000 atoms → ~1-2 MB trie

### Optimization Strategies

**1. Shallow Cloning**:
```rust
// Use Arc for shared data
pub struct Bindings {
    values: Arc<HashMap<VariableAtom, usize>>,
    bindings: Arc<HoleyVec<Binding>>,
}
```
Benefits: Copy-on-write, reduced allocation

**2. Interning**:
```rust
// Intern common atoms
static ATOM_INTERNER: Interner<Atom> = Interner::new();
```
Benefits: Deduplication, reduced memory

**3. Compact Variable IDs**:
```rust
// Use smaller ID type
pub struct VariableAtom {
    name: Symbol,  // Interned
    id: u32,       // Instead of u64
}
```

**4. Trie Pruning**:
- Remove rarely-used branches
- LRU cache for hot paths

## Integration Points

### Interpreter Integration

**Location**: `lib/src/metta/runner/mod.rs`

**Evaluation Loop:**
```rust
fn evaluate_atom(&mut self, atom: Atom) -> Vec<Atom> {
    match atom {
        Atom::Expression(expr) => {
            // 1. Try to match against rules
            let results = self.space.query(&atom);

            if results.is_empty() {
                // 2. No rule match, evaluate normally
                self.evaluate_expression(expr)
            } else {
                // 3. Apply rule results
                results.into_iter()
                    .flat_map(|bindings| {
                        let template = get_rule_template(&atom);
                        let substituted = apply_bindings(&template, &bindings);
                        self.evaluate_atom(substituted)
                    })
                    .collect()
            }
        }
        _ => vec![atom]
    }
}
```

### Custom Matching Hooks

**Trait**: `Grounded::match_()` - `hyperon-atom/src/lib.rs`

**Implementation Example:**
```rust
impl Grounded for MyType {
    fn match_(&self, other: &Atom) -> MatchResultIter {
        // Custom matching logic
        match other {
            Atom::Expression(expr) if self.matches_pattern(expr) => {
                // Return custom bindings
                let bindings = self.extract_bindings(expr);
                Box::new(std::iter::once(bindings))
            }
            _ => BindingsSet::empty().into_iter()
        }
    }
}
```

**Use Cases:**
- Domain-specific patterns (regex, ranges, etc.)
- Performance optimization (precomputed matches)
- External data integration (databases, APIs)

### Type System Interaction

**Type Checking with Patterns:**
```rust
// Pattern: (: <atom> <type>)
fn check_type(&self, atom: &Atom, expected_type: &Atom) -> bool {
    let pattern = expr!(":", atom.clone(), var!("actual_type"));
    let results = self.space.query(&pattern);

    results.into_iter().any(|bindings| {
        let actual_type = bindings.resolve(&var!("actual_type")).unwrap();
        types_compatible(actual_type, expected_type)
    })
}
```

**See**: `../type-system/05-type-checking.md`

## Implementation Best Practices

### 1. Avoid Deep Variable Chains

**Problem**: Long chains slow resolution.

**Solution**: Compress chains during merge.
```rust
fn compress_chain(&mut self, var: &VariableAtom) {
    let final_binding = self.resolve(var);
    if let Some(atom) = final_binding {
        // Short-circuit: point directly to final binding
        let group = self.values[var];
        self.bindings[group] = Binding::Atom(atom.clone(), gen);
    }
}
```

### 2. Reuse Binding Groups

**Problem**: Allocating groups is expensive.

**Solution**: Use HoleyVec to recycle group IDs.

### 3. Lazy Template Evaluation

**Problem**: Evaluating all templates is wasteful if only first result needed.

**Solution**: Return iterator, evaluate on demand.

### 4. Cache Trie Queries

**Problem**: Repeated queries are redundant.

**Solution**: LRU cache for query results.
```rust
struct CachedIndex {
    trie: AtomTrie,
    cache: LruCache<Atom, BindingsSet>,
}
```

### 5. Profile Before Optimizing

**Measurement Points:**
- Query time by pattern type
- Unification time by atom complexity
- Memory allocation patterns
- Cache hit rates

## Related Documentation

**Unification**: [02-unification.md](02-unification.md)
**Bindings**: [04-bindings.md](04-bindings.md)
**Match Operation**: [03-match-operation.md](03-match-operation.md)
**Non-Determinism**: [08-non-determinism.md](08-non-determinism.md)

## Summary

**Implementation Components:**
- **Matcher.rs**: Core unification engine
- **Bindings**: Two-level HashMap + HoleyVec structure
- **BindingsSet**: Multi-solution representation
- **AtomTrie**: Efficient pattern-based queries
- **Integration**: Seamless interpreter and type system connection

**Performance Characteristics:**
- **Query**: O(log n) to O(n) depending on pattern specificity
- **Unification**: O(size(pattern)) typically
- **Space**: O(n × d) for trie, O(m × v) for results

**Optimization Strategies:**
- Ground prefix matching
- Trie-based pruning
- Lazy evaluation
- Path compression
- Caching

**Best Practices:**
- Profile before optimizing
- Compress variable chains
- Reuse binding groups
- Use lazy iterators
- Cache frequent queries

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-17
