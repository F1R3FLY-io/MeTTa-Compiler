# MeTTaTron Architecture Overview

**Date**: 2025-11-12
**Status**: Current
**Version**: 1.0

---

## Table of Contents

1. [System Overview](#system-overview)
2. [High-Level Architecture](#high-level-architecture)
3. [Core Components](#core-components)
4. [Data Flow](#data-flow)
5. [Threading Model](#threading-model)
6. [Storage Layer](#storage-layer)
7. [Integration Points](#integration-points)
8. [Performance Characteristics](#performance-characteristics)

---

## System Overview

MeTTaTron is a high-performance MeTTa language evaluator implemented in Rust with:
- **Lazy evaluation** with pattern matching and special forms
- **LISP-like S-expression syntax** with rules, control flow, and type assertions
- **Thread-safe environment** using `Arc<Mutex<T>>` and `Arc<RwLock<T>>`
- **PathMap-based storage** (MORK format) for efficient pattern matching
- **Rholang integration** via direct Rust linking (no FFI)

---

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     User Interface Layer                     │
│  ┌────────────┐  ┌──────────┐  ┌────────────────────────┐  │
│  │ CLI (main) │  │   REPL   │  │ Rholang Integration    │  │
│  └────────────┘  └──────────┘  └────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Compilation Layer                         │
│  ┌──────────────────────┐  ┌────────────────────────────┐  │
│  │ Tree-Sitter Parser   │  │ S-Expression Compiler      │  │
│  │ (grammar → SExpr)    │→ │ (SExpr → MettaValue)       │  │
│  └──────────────────────┘  └────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Evaluation Engine                         │
│  ┌────────────┐  ┌────────────┐  ┌────────────────────┐   │
│  │ Pattern    │  │ Rule       │  │ Special Forms      │   │
│  │ Matching   │  │ Application│  │ (if, quote, eval)  │   │
│  └────────────┘  └────────────┘  └────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Storage Layer (MORK)                      │
│  ┌────────────┐  ┌────────────┐  ┌────────────────────┐   │
│  │ Facts      │  │ Rules      │  │ Type Index         │   │
│  │ (PathMap)  │  │ (HashMap)  │  │ (cached PathMap)   │   │
│  └────────────┘  └────────────┘  └────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

---

## Core Components

### 1. Parsing Layer

**Tree-Sitter Parser** (`src/tree_sitter_parser.rs`)
- Converts MeTTa source code to S-expressions (`SExpr`)
- Handles comments: `//`, `/* */`, `;`
- Tracks source positions for error reporting
- Supports special operators: `!`, `?`, `<-`, `<=`

**S-Expression Compiler** (`src/backend/compile.rs`)
- Converts `SExpr` to `MettaValue` (AST)
- Validates syntax and structure
- Resolves symbols and literals

### 2. Evaluation Engine

**Pattern Matcher** (`src/backend/eval/`)
- Modular evaluation system split across specialized modules:
  - `evaluation.rs` - Main eval loop and rule application
  - `bindings.rs` - Variable binding and unification
  - `control_flow.rs` - `if`, `switch`, `case` special forms
  - `errors.rs` - Error handling and propagation
  - `list_ops.rs` - List operations (`cons`, `car`, `cdr`, etc.)
  - `quoting.rs` - `quote` and `eval` special forms
  - `space.rs` - Space operations and `match`
  - `types.rs` - Type inference and checking

**Rule Application** (`src/backend/eval/evaluation.rs`)
- Lazy evaluation with reduction limits
- Pattern matching against rules
- Variable substitution and binding
- Iterative deepening for complex expressions

### 3. Storage Layer

**Environment** (`src/backend/environment.rs`)
- Thread-safe container for facts, rules, and metadata
- Key structures:
  ```rust
  pub struct Environment {
      space: Arc<RwLock<Space>>,                              // MORK Space (facts)
      rule_index: Arc<Mutex<HashMap<(String, usize), Vec<Rule>>>>,  // Indexed rules
      wildcard_rules: Arc<Mutex<Vec<Rule>>>,                  // Wildcard rules
      type_index: Arc<Mutex<Option<PathMap<()>>>>,            // Cached type subtrie
      pattern_cache: Arc<Mutex<LruCache<MettaValue, Vec<u8>>>>,  // Pattern cache
      multiplicities: Arc<Mutex<HashMap<String, usize>>>,     // Arity tracking
  }
  ```

**MORK/PathMap Integration** (`src/backend/mork_convert.rs`)
- Converts `MettaValue` to MORK byte format
- Direct byte conversion (Variant C optimization):
  ```
  MettaValue → metta_to_mork_bytes() → Vec<u8> → PathMap::insert()
             ~500ns                              (no parsing!)
  ```
- 10× faster than string-based path (see `docs/optimization/experiments/VARIANT_C_RESULTS_2025-11-11.md`)

### 4. Type System

**Type Inference** (`src/backend/eval/types.rs`)
- Infers types from expressions
- Type assertions with `(: expr type)`
- Type checking with `get-type` and `check-type`

**Type Index Optimization** (242.9× median speedup)
- Lazy-initialized PathMap subtrie for type lookups
- See `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md`

---

## Data Flow

### Fact Insertion Flow

```
User Input: (add-to-space (person "Alice"))
    │
    ▼
Tree-Sitter Parse → SExpr::List([Symbol("person"), String("Alice")])
    │
    ▼
Compile → MettaValue::SExpr(vec![Symbol("person"), String("Alice")])
    │
    ▼
MORK Conversion → metta_to_mork_bytes() → Vec<u8> [0x02, 0x70, ...]
    │
    ▼
PathMap Insert → space.btm.insert(&bytes, ()) → Stored in trie
```

### Rule Application Flow

```
Expression: (fib 5)
    │
    ▼
Pattern Match:
  1. Look up rules by (head="fib", arity=1) in rule_index HashMap
  2. Try each matching rule pattern
  3. Bind variables ($n, etc.)
    │
    ▼
Rule Body Evaluation:
  - Substitute variables with bindings
  - Recursively evaluate sub-expressions
  - Apply special forms (if, etc.)
    │
    ▼
Return Result: Long(5)
```

---

## Threading Model

### Current Architecture (Phase 1-3 Complete)

**Read-Heavy Workload**:
- 95%+ reads (pattern matching, lookups)
- <5% writes (rule/fact insertion)

**Lock Strategy**:
- `Arc<RwLock<Space>>` - Concurrent reads, exclusive writes
- `Arc<Mutex<HashMap>>` - Rule index (short critical sections)
- `Arc<Mutex<LruCache>>` - Pattern cache

**Performance Characteristics**:
- Read lock acquisition: ~50-100ns
- Write lock acquisition: ~100-200ns (uncontended)
- Lock duration: <10μs for most operations

### Tokio Integration

**Two Thread Pools**:
1. **Async Executor** - Tokio default (~num_cpus threads)
   - Handles async coordination and I/O
2. **Blocking Pool** - Configurable (`max_blocking_threads`)
   - Handles CPU-intensive MeTTa evaluation
   - Configured via `config::configure_eval()`

See: `docs/THREADING_MODEL.md` for details

---

## Storage Layer

### MORK Format

**PathMap Trie Structure**:
```
MORK Encoding: 0x02 (SExpr) + 0x70 (symbol "person") + 0x41 ("Alice")
                     │
                     ▼
PathMap Trie:       root
                     │
                  ┌──┴──┐
                0x02   0x03
                  │
               ┌──┴──┐
             0x70  0x71
                │
              0x41 ← fact stored here
```

**Optimizations**:
1. **Type Index** (`.restrict()` subtrie) - 242.9× speedup
2. **Direct Byte Conversion** (skip parsing) - 10.3× speedup
3. **Prefix Navigation** - Already optimal O(p) lookups

### Fact Storage

- **Format**: MORK bytes stored in PathMap trie
- **Insertion**: O(p) where p = path length (~20-50 bytes)
- **Lookup**: O(p) prefix navigation
- **Pattern Match**: O(k) where k = matching facts

### Rule Storage

- **Index**: HashMap by `(head_symbol, arity)`
- **Insertion**: O(1) HashMap insert + O(1) Vec push
- **Lookup**: O(1) HashMap lookup + O(k) iteration (k = matching rules)
- **Speedup**: 1.6-1.8× over linear scan

---

## Integration Points

### Rholang Integration

**Direct Rust Linking** (Recommended):
```rust
use mettatron::{Environment, MettaValue};

let mut env = Environment::new();
env.add_to_space(&fact)?;
let results = env.eval(&expr)?;
```

**Synchronous API**:
- `evaluate_sync()` - Blocking evaluation
- Thread-safe with interior mutability

**Asynchronous API**:
- `evaluate_async()` - Tokio-based async evaluation
- Suitable for Rholang's async runtime

See: `integration/RHOLANG_INTEGRATION.md`

### PathMap Par Conversion

**Convert between PathMap and Rholang Par**:
```rust
use mettatron::pathmap_par_integration::{pathmap_to_par, par_to_pathmap};

let par: Par = pathmap_to_par(&pathmap)?;
let pathmap: PathMap<()> = par_to_pathmap(&par)?;
```

See: `integration/PATHMAP_INTEGRATION.md`

---

## Performance Characteristics

### Optimized Operations

| Operation | Complexity | Typical Time | Speedup Achieved |
|-----------|-----------|--------------|------------------|
| **Type lookup** (cached) | O(1) | ~950 ns | **242.9×** |
| **Fact insertion** | O(p) | ~0.95 μs | **9.5×** (direct bytes) |
| **Rule lookup** | O(1) + O(k) | ~5-10 μs | **1.6-1.8×** (indexed) |
| **Pattern match** | O(k × m) | Varies | - |
| **Prefix navigation** | O(p) | ~100-500 ns | Already optimal |

### Bottlenecks Addressed

1. ✅ **Type Index** - 242.9× speedup via lazy cached subtrie
2. ✅ **MORK Serialization** - 10.3× speedup via direct byte conversion
3. ✅ **Rule Indexing** - 1.6-1.8× speedup via HashMap index
4. ⏭️ **Parallelization** - Planned (expected 1.6-36× additional speedup)

See: `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md`

---

## Future Architecture

### Planned Optimizations (Phase 5+)

**Parallel Bulk Operations**:
- Rayon-based parallel fact/rule insertion
- Expected speedup: 1.6-36× (depending on batch size)
- Plan: `docs/optimization/sessions/OPTIMIZATION_2_PARALLEL_BULK_OPERATIONS_PLAN.md`

**Adaptive Heuristics**:
- Dynamic threshold for parallel vs sequential execution
- Based on workload size and available cores

---

## Related Documentation

- **Threading**: `docs/THREADING_MODEL.md`
- **Optimization**: `docs/optimization/PERFORMANCE_OPTIMIZATION_SUMMARY.md`
- **Design Docs**: `docs/design/`
- **Integration**: `integration/RHOLANG_INTEGRATION.md`
- **API Reference**: `docs/reference/BACKEND_API_REFERENCE.md`

---

**Status**: ✅ Production-ready architecture with significant optimizations applied

**Last Updated**: 2025-11-12
