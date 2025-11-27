# Implementation Notes

## Abstract

This document provides detailed implementation notes from the hyperon-experimental reference implementation, including data structures, algorithms, and implementation-specific behaviors that may differ from the specification. These notes are essential for compiler implementers who need to understand the reference implementation or maintain compatibility with it.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Data Structures](#data-structures)
3. [Interpreter Implementation](#interpreter-implementation)
4. [Space Implementation](#space-implementation)
5. [Performance Characteristics](#performance-characteristics)
6. [Implementation Limitations](#implementation-limitations)
7. [Reference](#reference)

---

## Architecture Overview

### Module Structure

The hyperon-experimental implementation is organized into several key modules:

```
hyperon-experimental/
├── lib/src/
│   ├── metta/
│   │   ├── interpreter.rs       # Core interpreter
│   │   ├── runner/
│   │   │   └── stdlib/
│   │   │       ├── core.rs      # Core operations
│   │   │       └── space.rs     # Space operations
│   │   └── text.rs              # Parsing
│   ├── space/
│   │   └── grounding/
│   │       └── mod.rs           # Space implementation
│   ├── atom/
│   │   └── mod.rs               # Atom data structure
│   └── common/
│       └── mod.rs               # Common utilities
└── python/                      # Python bindings
```

### Language Implementation

**Host Language**: Rust

**Why Rust**:
- Memory safety without garbage collection
- Strong type system
- Good FFI for Python bindings
- Performance competitive with C/C++

### Python Bindings

The Python API provides:
- Easy experimentation
- Integration with ML libraries
- Prototyping new features

**Location**: `hyperon-experimental/python/hyperon/`

---

## Data Structures

### Atom

**Location**: `lib/src/atom/mod.rs`

**Definition** (simplified):
```rust
pub enum Atom {
    Symbol(SymbolAtom),
    Variable(VariableAtom),
    Expression(ExpressionAtom),
    Grounded(GroundedAtom),
}
```

**Components**:

1. **Symbol**: Atomic symbol (e.g., `foo`, `bar`)
   ```rust
   pub struct SymbolAtom {
       name: String,
   }
   ```

2. **Variable**: Variable with name (e.g., `$x`, `$y`)
   ```rust
   pub struct VariableAtom {
       name: String,
   }
   ```

3. **Expression**: List of atoms (e.g., `(foo bar baz)`)
   ```rust
   pub struct ExpressionAtom {
       children: Vec<Atom>,
   }
   ```

4. **Grounded**: Embedded Rust value (e.g., numbers, spaces, custom types)
   ```rust
   pub struct GroundedAtom {
       value: Box<dyn Grounded>,
   }
   ```

**Memory Layout**: Enum with pointer-sized discriminant + data

**Size**: Approximately 24-32 bytes per atom (implementation-specific)

### Bindings

**Purpose**: Map variables to values

**Definition** (simplified):
```rust
pub struct Bindings {
    map: HashMap<VariableAtom, Atom>,
}
```

**Operations**:
- `resolve(var) -> Option<Atom>`: Look up variable binding
- `merge(other) -> Option<Bindings>`: Combine two binding sets (fails if inconsistent)
- `has_loops() -> bool`: Check for circular bindings

### Stack

**Purpose**: Represents computation to be performed

**Definition**:
```rust
pub enum Stack {
    Empty,
    Atom(Atom, Rc<RefCell<Stack>>),
    // ...
}
```

**Used For**: Continuation-style evaluation

### InterpretedAtom

**Location**: `interpreter.rs`

**Definition**:
```rust
pub struct InterpretedAtom(Stack, Bindings);
```

**Purpose**: Represents one alternative in the evaluation plan
- `Stack`: What remains to be evaluated
- `Bindings`: Variable bindings for this alternative

### InterpreterState

**Location**: `interpreter.rs`:172-183

**Definition**:
```rust
pub struct InterpreterState {
    /// List of the alternatives to evaluate further.
    plan: Vec<InterpretedAtom>,
    /// List of the completely evaluated results to be returned.
    finished: Vec<Atom>,
    /// Evaluation context.
    context: InterpreterContext,
    /// Maximum stack depth
    max_stack_depth: usize,
}
```

**Key Fields**:
- `plan`: Work queue (alternatives to process)
- `finished`: Completed results
- `max_stack_depth`: Prevents infinite recursion

### AtomIndex

**Location**: `lib/src/space/grounding/mod.rs`

**Purpose**: Efficient indexing of atoms for pattern matching

**Structure**: Trie-based index
- Organizes atoms by symbol prefix
- Enables efficient pattern queries
- O(pattern depth) lookup complexity

**Note**: Internal structure is implementation-dependent

---

## Interpreter Implementation

### Main Evaluation Loop

**Location**: `interpreter.rs`:269-277

**Algorithm**:
```rust
pub fn interpret_step(mut state: InterpreterState) -> InterpreterState {
    let interpreted_atom = state.pop().unwrap();  // LIFO
    log::debug!("interpret_step:\n{}", interpreted_atom);
    let InterpretedAtom(stack, bindings) = interpreted_atom;
    for result in interpret_stack(&state.context, stack, bindings, state.max_stack_depth) {
        state.push(result);
    }
    state
}
```

**Steps**:
1. Pop one alternative from plan (LIFO)
2. Interpret the stack with current bindings
3. Push all resulting alternatives back onto plan
4. Repeat until plan is empty

**Termination Condition**: `plan.is_empty()`

### Stack Interpretation

**Function**: `interpret_stack()`

**Purpose**: Evaluate one level of the stack

**Returns**: Iterator of `InterpretedAtom` (new alternatives)

**Key Logic**:
1. If stack is empty, return finished result
2. If stack head is atom:
   - Try to evaluate/reduce
   - Query space for reduction rules
   - Return alternatives
3. Otherwise, continue processing stack

### Query Function

**Location**: `interpreter.rs`:604-638

**Purpose**: Find all atoms matching a pattern

**Algorithm**:
```rust
fn query(space: &DynSpace, prev: Option<Rc<RefCell<Stack>>>,
         to_eval: Atom, bindings: Bindings, vars: Variables)
         -> Vec<InterpretedAtom> {
    let var_x = &VariableAtom::new("X").make_unique();
    let query = Atom::expr([EQUAL_SYMBOL, to_eval.clone(),
                           Atom::Variable(var_x.clone())]);
    let results = space.borrow().query(&query);

    let results: Vec<InterpretedAtom> = results.into_iter()
        .flat_map(|b| b.merge(&bindings).into_iter())
        .filter_map(move |b| {
            if b.has_loops() {
                None
            } else {
                Some(result(res, b))
            }
        })
        .collect();

    // ... handle no results case ...
}
```

**Key Steps**:
1. Create pattern: `(= <expr> $X)` with fresh variable
2. Query space for matches
3. Merge with existing bindings
4. Filter circular bindings
5. Return all alternatives

### Depth Limit

**Purpose**: Prevent stack overflow from infinite recursion

**Implementation**:
```rust
if depth > max_stack_depth {
    return error or stop evaluation
}
```

**Default**: Configurable (typically 1000-10000)

**Trade-off**: Higher limit allows deeper recursion but risks stack overflow

---

## Space Implementation

### GroundingSpace

**Location**: `lib/src/space/grounding/mod.rs`

**Structure**:
```rust
pub struct GroundingSpace {
    index: AtomIndex,
    common: SpaceCommon,
}

pub struct SpaceCommon {
    observers: Vec<Box<dyn SpaceObserver>>,
}
```

**Key Components**:
- `index`: Trie-based atom storage and query
- `observers`: List of space observers

### Add Operation

**Implementation** (`mod.rs`:70-79):
```rust
pub fn add(&mut self, atom: Atom) {
    log::debug!("GroundingSpace::add: {}, atom: {}", self, atom);
    self.index.insert(atom.clone());
    self.common.notify_all_observers(&SpaceEvent::Add(atom));
}
```

**Steps**:
1. Insert into index
2. Notify observers synchronously
3. No transaction support

**Complexity**: O(atom depth) for trie insertion

### Remove Operation

**Implementation** (`mod.rs`:81-88):
```rust
pub fn remove(&mut self, atom: &Atom) -> bool {
    log::debug!("GroundingSpace::remove: {}, atom: {}", self, atom);
    let is_removed = self.index.remove(atom);
    if is_removed {
        self.common.notify_all_observers(&SpaceEvent::Remove(atom.clone()));
    }
    is_removed
}
```

**Returns**: `bool` indicating success

**Complexity**: O(atom depth) for trie removal

### Query Operation

**Purpose**: Find all atoms matching pattern

**Algorithm** (conceptual):
```
function query(pattern):
    traverse trie based on pattern structure
    collect all matching atoms
    return as bindings iterator
```

**Complexity**: O(pattern depth + number of matches)

**Iterator**: Returns lazy iterator (not all matches pre-collected)

### RefCell Usage

**Pattern**: `Rc<RefCell<Space>>`

**Why**:
- `Rc`: Shared ownership (multiple references)
- `RefCell`: Interior mutability (mutation through shared reference)

**Runtime Checking**:
```rust
let space_ref = space.borrow_mut();  // Panics if already borrowed
```

**Limitations**:
- Not thread-safe
- Runtime overhead for borrow checking
- Panics on concurrent borrow violations

---

## Performance Characteristics

### Time Complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Atom creation | O(1) | Constant time |
| Pattern matching | O(d + m) | d = depth, m = matches |
| Space add | O(d) | d = atom depth |
| Space remove | O(d) | d = atom depth |
| Space query | O(d + m) | d = pattern depth, m = matches |
| Alternative creation | O(1) | Push to plan vector |
| Alternative processing | O(n) | n = plan size |
| Binding lookup | O(1) average | HashMap lookup |
| Binding merge | O(n) | n = binding count |

### Space Complexity

| Structure | Size | Notes |
|-----------|------|-------|
| Atom | ~24-32 bytes | Enum + data |
| Bindings | O(n) | n = number of variables |
| Stack | O(d) | d = depth |
| Plan | O(a) | a = number of alternatives |
| Space | O(n × d) | n = atoms, d = average depth |

### Bottlenecks

**Common Performance Issues**:

1. **Large Number of Alternatives**:
   - Problem: Plan grows exponentially with non-determinism
   - Mitigation: Limit alternatives, use lazy evaluation

2. **Deep Recursion**:
   - Problem: Stack depth limit reached
   - Mitigation: Increase limit, use iteration instead of recursion

3. **Expensive Pattern Matching**:
   - Problem: Many atoms in space, complex patterns
   - Mitigation: Better indexing, pattern optimization

4. **Binding Merges**:
   - Problem: Many variables, frequent merges
   - Mitigation: Persistent data structures, structural sharing

---

## Implementation Limitations

### Thread Safety

**Limitation**: Not thread-safe

**Reason**: Uses `RefCell` for interior mutability

**Impact**: Cannot safely share spaces across threads

**Workaround**:
- Use separate spaces per thread
- Implement thread-safe space variant with `Mutex`

### Atomicity

**Limitation**: No transactional operations

**Impact**: Cannot rollback failed multi-step mutations

**Workaround**:
- Implement application-level transactions
- Add transaction support in space implementation

### Memory Management

**Limitation**: Reference counting (not garbage collected)

**Impact**:
- Circular references cause memory leaks
- `has_loops()` check prevents some but not all cycles

**Workaround**:
- Careful design to avoid cycles
- Weak references where appropriate

### Stack Depth

**Limitation**: Fixed maximum depth

**Impact**: Deep recursions fail

**Workaround**:
- Increase limit (risks actual stack overflow)
- Rewrite recursive algorithms iteratively
- Use tail recursion optimization (not currently implemented)

### Pattern Matching Performance

**Limitation**: Trie index has fixed structure

**Impact**: Some query patterns are slower than others

**Example**:
```metta
; Fast: specific head symbol
!(match &space (foo $x $y) ...)

; Slower: variable head
!(match &space ($head $x $y) ...)
```

**Workaround**: Restructure queries to use specific symbols

### Deterministic Ordering

**Limitation**: No guaranteed ordering for:
- Query results
- Alternative processing (LIFO is implementation detail)

**Impact**: Cannot rely on specific ordering

**Workaround**: Use explicit ordering mechanisms (e.g., priority attributes)

---

## Reference

### Key Files

1. **`lib/src/metta/interpreter.rs`**
   - Core interpreter logic
   - `InterpreterState` structure
   - `interpret_step()` function
   - `query()` function

2. **`lib/src/metta/runner/stdlib/space.rs`**
   - `add-atom`, `remove-atom`, `replace-atom` operations
   - Space operation implementations

3. **`lib/src/space/grounding/mod.rs`**
   - `GroundingSpace` implementation
   - `add()`, `remove()`, `query()` methods
   - Observer pattern implementation

4. **`lib/src/atom/mod.rs`**
   - `Atom` enum definition
   - Atom operations and utilities

5. **`lib/src/metta/runner/stdlib/core.rs`**
   - Core operations (superpose, collapse, etc.)
   - Built-in operation implementations

### Code Metrics

Based on hyperon-experimental commit `164c22e9`:

- **Total Lines of Rust Code**: ~50,000+ (lib/src/)
- **Core Interpreter**: ~2,000 lines (interpreter.rs)
- **Space Implementation**: ~1,000 lines (space/grounding/)
- **Standard Library**: ~3,000 lines (stdlib/)

### Build Requirements

**Rust Version**: 1.70+ (or as specified in `rust-toolchain`)

**Dependencies**: See `Cargo.toml`
- `regex` for text processing
- `log` for logging
- `pyo3` for Python bindings (optional)

**Build Command**:
```bash
cargo build --release
```

### Testing

**Unit Tests**: Throughout codebase
```bash
cargo test
```

**Integration Tests**: In `tests/` directory

**MeTTa Test Files**: In `python/tests/` and `lib/tests/`

---

## Implementation Variations

### Alternative Implementations

Compiler implementers may choose different approaches:

1. **Alternative Processing**:
   - **Reference**: LIFO (depth-first)
   - **Alternative**: FIFO (breadth-first), priority queue, parallel

2. **Space Storage**:
   - **Reference**: Trie-based index
   - **Alternative**: Hash table, database, graph store

3. **Memory Management**:
   - **Reference**: Reference counting (`Rc`)
   - **Alternative**: Garbage collection, arena allocation

4. **Thread Safety**:
   - **Reference**: Single-threaded (`RefCell`)
   - **Alternative**: Thread-safe (`Mutex`, `RwLock`), lock-free

5. **Evaluation Strategy**:
   - **Reference**: Normal order, plan-based
   - **Alternative**: Lazy evaluation, eager evaluation, hybrid

### Compatibility Considerations

When implementing variations, consider:

1. **Semantic Compatibility**: Preserve MeTTa semantics (non-determinism, pattern matching)
2. **Behavioral Compatibility**: Produce same results (as sets, not necessarily same order)
3. **API Compatibility**: Support same operations and syntax
4. **Performance Trade-offs**: Document performance characteristics

---

## Debugging and Profiling

### Logging

The reference implementation uses Rust's `log` crate:

```rust
log::debug!("interpret_step:\n{}", interpreted_atom);
```

**Enable Logging**:
```bash
RUST_LOG=debug cargo run
```

**Log Levels**: `error`, `warn`, `info`, `debug`, `trace`

### Profiling

**Tools**:
- **perf**: Linux profiling tool
- **flamegraph**: Visualize performance
- **cargo-flamegraph**: Rust integration

**Example**:
```bash
cargo install flamegraph
cargo flamegraph --bin <your-binary>
```

### Debugging

**GDB/LLDB**: Standard Rust debugging

**Rust-specific Tools**:
- `rust-gdb`: GDB with Rust pretty-printers
- `rust-lldb`: LLDB with Rust support

---

## See Also

- **§01-05**: Semantic specifications (what should be implemented)
- **§07**: Formal proofs (properties to maintain)
- **§08**: Comparisons (alternative approaches)
- **hyperon-experimental README**: Build and setup instructions

---

**Version**: 1.0
**Last Updated**: 2025-11-13
**Based on**: hyperon-experimental commit `164c22e9`
