# Copy-on-Write Environment Design Specification

**Version**: 1.0
**Date**: 2025-11-13
**Status**: Design Phase
**Author**: MeTTaTron Development Team

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Problem Statement](#problem-statement)
3. [Current Architecture Analysis](#current-architecture-analysis)
4. [Proposed Solution](#proposed-solution)
5. [Detailed Design](#detailed-design)
6. [Performance Analysis](#performance-analysis)
7. [Implementation Phases](#implementation-phases)
8. [Testing Strategy](#testing-strategy)
9. [Migration Path](#migration-path)
10. [Risks and Mitigations](#risks-and-mitigations)
11. [Alternatives Considered](#alternatives-considered)
12. [References](#references)

---

## Executive Summary

### Objective

Implement Copy-on-Write (CoW) semantics for the `Environment` structure to enable safe dynamic rule/fact definition during parallel sub-evaluation with minimal performance impact on read-only workloads.

### Key Requirements

1. **Safety**: Eliminate race conditions in parallel rule definition
2. **Isolation**: Each evaluation path has independent environment view
3. **Performance**: < 1% overhead for read-only workloads (most common case)
4. **Correctness**: Proper environment merging via `union()` operation
5. **Backward Compatibility**: Existing code continues to work

### Success Metrics

- ✅ All 403+ existing tests pass
- ✅ New CoW-specific tests achieve 100% coverage
- ✅ Read-only evaluation performance degradation < 1%
- ✅ Concurrent read performance improves (via RwLock)
- ✅ Memory overhead acceptable (< 2× for modified environments)

### Implementation Scope

**Estimated Effort**: 24-32 hours (3-4 working days)

**Primary Changes**:
- `src/backend/environment.rs`: ~300-400 LOC modifications
- `src/backend/eval/mod.rs`: ~50 LOC modifications (union call sites)
- Test coverage: ~500-600 LOC new tests
- Benchmarks: ~200-300 LOC
- Documentation: ~1000-1500 LOC

**Total Impact**: ~2000-2800 LOC (including tests and docs)

---

## Problem Statement

### Current Behavior: Arc-Sharing with Race Conditions

The current `Environment` implementation uses `Arc<Mutex<T>>` for all mutable state:

```rust
#[derive(Clone)]
pub struct Environment {
    btm: Arc<Mutex<PathMap<()>>>,
    rule_index: Arc<Mutex<HashMap<(String, usize), Vec<Rule>>>>,
    wildcard_rules: Arc<Mutex<Vec<Rule>>>,
    multiplicities: Arc<Mutex<HashMap<String, usize>>>,
    // ... other fields ...
}
```

**Cloning behavior**:
```rust
let env2 = env1.clone();  // Shallow clone - both share Arc pointers
```

**Consequence**: All clones **share the same underlying data**.

### The Race Condition

When parallel sub-expressions modify the environment:

```rust
// Parallel evaluation of (expr1 expr2 expr3 expr4)
items.par_iter()
    .map(|item| {
        let env_clone = env.clone();  // Shares Arc pointers!
        eval_with_depth(item.clone(), env_clone, depth + 1)
    })
    .collect()
```

**Scenario**: If `expr2` defines a rule via `(= (new-rule) value)`:

1. Thread 2 evaluates `expr2` and calls `env.add_rule(rule)`
2. Thread 2 acquires `rule_index.lock()` and adds rule
3. **Rule is immediately visible to all threads** (shared Arc)
4. Thread 1, 3, 4 may or may not see the rule (timing-dependent)
5. **Non-deterministic behavior**: Results vary between runs

### Current union() is a No-Op

```rust
pub fn union(&self, _other: &Environment) -> Environment {
    // Returns self, completely ignores _other!
    let shared_mapping = self.shared_mapping.clone();
    let btm = self.btm.clone();
    // ... just clones self's Arc pointers ...
    Environment { shared_mapping, btm, /* ... */ }
}
```

**Problem**: The `union()` function doesn't actually merge anything because all environments already share the same data via Arc pointers.

### Why This Is Broken

1. **Non-determinism**: Different runs produce different results
2. **Heisenbugs**: Bugs appear/disappear based on thread scheduling
3. **Unsafe semantics**: No isolation between parallel branches
4. **Debugging nightmare**: Cannot reproduce issues reliably
5. **Violates expectations**: Functional programming expects immutability

### Why It "Works" Today

The current implementation **accidentally works** because:

1. No existing MeTTa code defines rules during evaluation
2. All rule definitions are at top-level (before evaluation)
3. Shared mutation is "fine" if everyone reads the same data
4. Arc-sharing is intentional for the single-threaded case

**But**: The moment someone defines rules during parallel evaluation, the system breaks.

---

## Current Architecture Analysis

### Environment Structure (As-Is)

**File**: `src/backend/environment.rs` (lines 19-68)

```rust
#[derive(Clone)]
pub struct Environment {
    // MORK storage (interned strings and fact trie)
    shared_mapping: SharedMappingHandle,      // Arc<Mutex<...>> internally
    btm: Arc<Mutex<PathMap<()>>>,             // Facts stored in MORK trie

    // Rule indices (for O(k) lookup by head symbol)
    rule_index: Arc<Mutex<HashMap<(String, usize), Vec<Rule>>>>,
    wildcard_rules: Arc<Mutex<Vec<Rule>>>,    // Rules with no head symbol
    multiplicities: Arc<Mutex<HashMap<String, usize>>>,  // Rule counts

    // Caches (performance optimization)
    pattern_cache: Arc<Mutex<LruCache<MettaValue, Vec<u8>>>>,  // MORK bytes

    // Fuzzy matching (for "Did you mean?" suggestions)
    fuzzy_matcher: FuzzyMatcher,              // Arc<Mutex<...>> internally

    // Type system
    type_index: Arc<Mutex<Option<PathMap<()>>>>,  // Subtrie for type queries
    type_index_dirty: Arc<Mutex<bool>>,           // Rebuild flag
}
```

### Clone Semantics (As-Is)

**Automatic derive** via `#[derive(Clone)]`:
- All `Arc<T>` fields: Increment reference count (O(1))
- All other fields: Deep clone (varies)

**Cost**: ~10ns (7-8 atomic reference count increments)

**Sharing**: All clones share the **same underlying data** (via Arc).

### Mutation Path: add_rule()

**File**: `src/backend/environment.rs` (lines 618-658)

```rust
pub fn add_rule(&mut self, rule: Rule) {
    // 1. Create rule s-expression
    let rule_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        rule.lhs.clone(),
        rule.rhs.clone(),
    ]);

    // 2. Generate key for multiplicity tracking
    let rule_key = rule_sexpr.to_mork_string();

    // 3. Update multiplicity count (LOCK #1)
    {
        let mut counts = self.multiplicities.lock().unwrap();
        let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
        counts.insert(rule_key.clone(), new_count);
    }

    // 4. Add to rule index (LOCK #2 or #3)
    if let Some(head) = rule.lhs.get_head_symbol() {
        let arity = rule.lhs.get_arity();
        let mut index = self.rule_index.lock().unwrap();
        index.entry((head.clone(), arity))
             .or_insert_with(Vec::new)
             .push(rule);
    } else {
        let mut wildcards = self.wildcard_rules.lock().unwrap();
        wildcards.push(rule);
    }

    // 5. Add to MORK Space (LOCK #4)
    self.add_to_space(&rule_sexpr);
}
```

**Lock Pattern**: 4 separate lock acquisitions (not atomic overall)

**Visibility**: Changes immediately visible to all Environment clones (shared Arc).

### Read Path: get_matching_rules()

**File**: `src/backend/environment.rs` (lines 1138-1168)

```rust
pub fn get_matching_rules(&self, head: &str, arity: usize) -> Vec<Rule> {
    // Capacity calculation (LOCK #1 and #2)
    let (indexed_len, wildcard_len) = {
        let index = self.rule_index.lock().unwrap();
        let wildcards = self.wildcard_rules.lock().unwrap();
        (
            index.get(&(head.to_string(), arity)).map_or(0, |r| r.len()),
            wildcards.len()
        )
    };

    let mut matching_rules = Vec::with_capacity(indexed_len + wildcard_len);

    // Get indexed rules (LOCK #3)
    {
        let index = self.rule_index.lock().unwrap();
        if let Some(rules) = index.get(&(head.to_string(), arity)) {
            matching_rules.extend(rules.clone());
        }
    }

    // Get wildcard rules (LOCK #4)
    {
        let wildcards = self.wildcard_rules.lock().unwrap();
        matching_rules.extend(wildcards.clone());
    }

    matching_rules
}
```

**Lock Pattern**: 4 lock acquisitions (2 for sizing, 2 for data)

**Atomicity**: NOT atomic across both indices - can see inconsistent state.

### Evaluation Flow

**File**: `src/backend/eval/mod.rs` (lines 192-223)

```rust
fn eval_sexpr(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalResult {
    // ... special forms handling ...

    // Parallel evaluation decision
    let eval_results_and_envs: Vec<(Vec<MettaValue>, Environment)> =
        if items.len() >= PARALLEL_EVAL_THRESHOLD {
            items.par_iter()
                 .map(|item| eval_with_depth(item.clone(), env.clone(), depth + 1))
                 .collect()
        } else {
            items.iter()
                 .map(|item| eval_with_depth(item.clone(), env.clone(), depth + 1))
                 .collect()
        };

    // Environment unification
    let (eval_results, envs): (Vec<_>, Vec<_>) = eval_results_and_envs.into_iter().unzip();

    let mut unified_env = env.clone();
    for e in envs {
        unified_env = unified_env.union(&e);  // No-op in current implementation!
    }

    // ... rule matching and result construction ...

    (all_final_results, unified_env)
}
```

**Issue**: `union()` doesn't merge environments, it just returns self.

### Performance Baseline

**From benchmarks and code analysis**:

| Operation | Latency | Notes |
|-----------|---------|-------|
| Environment::clone() | ~10ns | 7-8 Arc increments |
| Mutex::lock() (uncontended) | ~20-30ns | Single-threaded |
| Mutex::lock() (4 threads) | ~120ns | Serialized access |
| add_rule() | ~1µs | 4 lock acquisitions + data updates |
| get_matching_rules() | ~100ns | 4 lock acquisitions + cloning |
| union() (current) | ~10ns | No-op (just clone self) |
| eval_sexpr() (typical) | ~15-30µs | Full evaluation with rules |

---

## Proposed Solution

### Copy-on-Write (CoW) Semantics

**Core Principle**:
- **Cloning is cheap** (O(1) - just Arc increments)
- **Reading is cheap** (no copying)
- **First write triggers copy** (lazy deep copy of modified structures)
- **Isolation by default** (each clone has independent view after write)
- **Explicit merging** (via union() operation)

### Design Philosophy

1. **Optimize the common case**: Read-only clones are free
2. **Pay for what you use**: Only copy on write
3. **Functional semantics**: Immutable unless explicitly modified
4. **Thread safety**: RwLock for concurrent readers
5. **Explicit control**: union() gives programmer merge control

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Environment (Owner)                       │
├─────────────────────────────────────────────────────────────┤
│  owns_data: true                                            │
│  modified: false → true (on first write)                    │
│                                                             │
│  ┌────────────────────────────────────────────────┐        │
│  │ Arc<RwLock<rule_index>>  (owned exclusively)   │        │
│  │ Arc<RwLock<btm>>          (owned exclusively)   │        │
│  │ Arc<RwLock<wildcards>>    (owned exclusively)   │        │
│  └────────────────────────────────────────────────┘        │
└─────────────────────────────────────────────────────────────┘
                            │
                    .clone() │ (O(1) - Arc increments)
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                Environment (Clone - Shared)                  │
├─────────────────────────────────────────────────────────────┤
│  owns_data: false                                           │
│  modified: false                                            │
│                                                             │
│  ┌────────────────────────────────────────────────┐        │
│  │ Arc<RwLock<rule_index>>  (shared via Arc)      │        │
│  │ Arc<RwLock<btm>>          (shared via Arc)      │        │
│  │ Arc<RwLock<wildcards>>    (shared via Arc)      │        │
│  └────────────────────────────────────────────────┘        │
└─────────────────────────────────────────────────────────────┘
                            │
              .add_rule()   │ (triggers make_owned())
                            ▼
┌─────────────────────────────────────────────────────────────┐
│           Environment (Clone - Now Owns Copy)                │
├─────────────────────────────────────────────────────────────┤
│  owns_data: true                                            │
│  modified: true                                             │
│                                                             │
│  ┌────────────────────────────────────────────────┐        │
│  │ Arc<RwLock<rule_index>>  (NEW Arc - deep copy) │        │
│  │ Arc<RwLock<btm>>          (NEW Arc - O(1) CoW)  │        │
│  │ Arc<RwLock<wildcards>>    (NEW Arc - deep copy) │        │
│  └────────────────────────────────────────────────┘        │
└─────────────────────────────────────────────────────────────┘
```

### Key Operations

#### Clone (Read-Only Path)

```rust
let env2 = env1.clone();
// Cost: ~20ns (Arc increments + AtomicBool allocation)
// Memory: 0 bytes (shares data via Arc)
// owns_data: false
// modified: false
```

#### Write Path (Triggers CoW)

```rust
let mut env2 = env1.clone();
env2.add_rule(rule);
// First time:
//   - Triggers make_owned() (~100µs for deep copy)
//   - owns_data: false → true
//   - modified: false → true
// Subsequent writes:
//   - Normal add_rule cost (~1µs)
```

#### Union (Merge Environments)

```rust
let unified = env1.union(&env2);
// Case 1: Neither modified → ~20ns (fast path)
// Case 2: One modified → ~20ns (return modified one)
// Case 3: Both modified → ~100µs (deep merge)
```

### Mutex → RwLock Migration

**Current**:
```rust
rule_index: Arc<Mutex<HashMap<...>>>
```
- Single lock for reads AND writes
- Concurrent reads BLOCKED (serialized)

**Proposed**:
```rust
rule_index: Arc<RwLock<HashMap<...>>>
```
- Multiple concurrent readers allowed
- Single exclusive writer
- **Concurrent read performance: 4× improvement** (for 4 threads)

---

## Detailed Design

### 5.1 Environment Structure Changes

**File**: `src/backend/environment.rs`

#### New Fields

```rust
#[derive(Clone)]
pub struct Environment {
    // EXISTING FIELDS (Mutex → RwLock)
    shared_mapping: SharedMappingHandle,
    btm: Arc<RwLock<PathMap<()>>>,                        // Changed: Mutex → RwLock
    rule_index: Arc<RwLock<HashMap<(String, usize), Vec<Rule>>>>,  // Changed
    wildcard_rules: Arc<RwLock<Vec<Rule>>>,               // Changed
    multiplicities: Arc<RwLock<HashMap<String, usize>>>,  // Changed
    pattern_cache: Arc<RwLock<LruCache<MettaValue, Vec<u8>>>>,  // Changed
    type_index: Arc<RwLock<Option<PathMap<()>>>>,         // Changed
    type_index_dirty: Arc<RwLock<bool>>,                  // Changed
    fuzzy_matcher: FuzzyMatcher,                          // Unchanged (internal RwLock)

    // NEW FIELDS (Copy-on-Write tracking)

    /// Ownership flag: true if this environment owns its data (can modify in-place)
    /// false if this environment shares data (must copy before modification)
    owns_data: bool,

    /// Modification tracking: true if any write operations performed
    /// NOT shared via Arc - each clone gets fresh AtomicBool
    /// Used for fast-path union() optimization
    modified: Arc<AtomicBool>,
}
```

#### Field Semantics

**`owns_data: bool`**
- **Purpose**: Track whether this environment can modify data in-place
- **Initial value**: `true` for newly created environments
- **Clone value**: `false` (clones don't own parent's data)
- **After make_owned()**: `true` (now owns independent copy)

**`modified: Arc<AtomicBool>`**
- **Purpose**: Track if environment has been modified (for union fast path)
- **Initial value**: `false`
- **Clone behavior**: **NEW Arc** (not shared with parent!)
- **After write**: `true`
- **Thread-safe**: AtomicBool for concurrent access

### 5.2 Clone Implementation

```rust
impl Clone for Environment {
    fn clone(&self) -> Self {
        Environment {
            // Shallow Arc clones (O(1) - just increment ref count)
            shared_mapping: self.shared_mapping.clone(),
            btm: Arc::clone(&self.btm),
            rule_index: Arc::clone(&self.rule_index),
            wildcard_rules: Arc::clone(&self.wildcard_rules),
            multiplicities: Arc::clone(&self.multiplicities),
            pattern_cache: Arc::clone(&self.pattern_cache),
            fuzzy_matcher: self.fuzzy_matcher.clone(),
            type_index: Arc::clone(&self.type_index),
            type_index_dirty: Arc::clone(&self.type_index_dirty),

            // CoW tracking
            owns_data: false,  // Clone does NOT own parent's data

            // CRITICAL: Fresh AtomicBool, NOT shared with parent
            modified: Arc::new(AtomicBool::new(false)),
        }
    }
}
```

**Performance**:
- Arc increments: ~8 fields × 2-3ns = 16-24ns
- AtomicBool allocation: ~5-10ns
- **Total: ~20-35ns** (vs current ~10ns, acceptable overhead)

**Memory**:
- Arc increments: 0 bytes (just ref counts)
- AtomicBool: 8 bytes + allocation overhead (~16 bytes)
- **Total: ~24 bytes per clone** (negligible)

### 5.3 make_owned() Implementation

**Purpose**: Deep copy shared data structures to gain exclusive ownership.

```rust
impl Environment {
    /// Ensure this environment owns its data (can modify in-place).
    /// If already owned, this is a no-op.
    /// If shared, performs deep copy of all mutable structures.
    fn make_owned(&mut self) {
        if self.owns_data {
            return;  // Already own the data, nothing to do
        }

        // Deep copy rule_index (O(n) where n = number of rules)
        self.rule_index = Arc::new(RwLock::new({
            let index = self.rule_index.read().unwrap();
            index.clone()  // HashMap::clone() - O(n)
        }));

        // Deep copy wildcard_rules (O(m) where m = number of wildcards)
        self.wildcard_rules = Arc::new(RwLock::new({
            let wildcards = self.wildcard_rules.read().unwrap();
            wildcards.clone()  // Vec::clone() - O(m)
        }));

        // Deep copy multiplicities (O(n))
        self.multiplicities = Arc::new(RwLock::new({
            let counts = self.multiplicities.read().unwrap();
            counts.clone()  // HashMap::clone() - O(n)
        }));

        // PathMap clone (O(1) due to structural sharing via MORK)
        self.btm = Arc::new(RwLock::new({
            let btm = self.btm.read().unwrap();
            btm.clone()  // PathMap implements structural sharing
        }));

        // Type index (if present)
        self.type_index = Arc::new(RwLock::new({
            let idx = self.type_index.read().unwrap();
            idx.clone()  // Option<PathMap<()>>
        }));

        // Type index dirty flag
        self.type_index_dirty = Arc::new(RwLock::new({
            let dirty = self.type_index_dirty.read().unwrap();
            *dirty
        }));

        // Pattern cache (LRU cache - O(k) where k = cache size, max 1000)
        self.pattern_cache = Arc::new(RwLock::new({
            let cache = self.pattern_cache.read().unwrap();
            cache.clone()
        }));

        // Fuzzy matcher (contains PathMapDictionary)
        self.fuzzy_matcher = self.fuzzy_matcher.clone();

        // Mark as owner and modified
        self.owns_data = true;
        self.modified.store(true, Ordering::Release);
    }
}
```

**Performance Analysis**:

For environment with 10,000 rules:
```
rule_index clone:     ~50µs  (HashMap with 10K entries)
wildcard_rules clone: ~10µs  (Vec with ~100 wildcards)
multiplicities clone: ~50µs  (HashMap with 10K entries)
btm clone:            ~1µs   (PathMap O(1) structural sharing)
type_index clone:     ~1µs   (PathMap O(1) structural sharing)
pattern_cache clone:  ~10µs  (LRU with 1000 entries)
Arc allocations:      ~1µs   (7-8 Arc::new calls)
---------------------------------------------------
Total:               ~120µs
```

**Amortization**: Paid once per clone that performs writes. Subsequent writes are normal cost (~1µs per rule).

### 5.4 Mutation Methods Update

All methods that modify environment must call `make_owned()` first:

#### add_rule()

```rust
pub fn add_rule(&mut self, rule: Rule) {
    // Ensure we own the data before mutation
    self.make_owned();

    // EXISTING LOGIC (now operates on owned copy)
    let rule_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        rule.lhs.clone(),
        rule.rhs.clone(),
    ]);

    let rule_key = rule_sexpr.to_mork_string();

    // Update multiplicity (write lock)
    {
        let mut counts = self.multiplicities.write().unwrap();  // Changed: write()
        let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
        counts.insert(rule_key.clone(), new_count);
    }

    // Add to rule index (write lock)
    if let Some(head) = rule.lhs.get_head_symbol() {
        let arity = rule.lhs.get_arity();
        let mut index = self.rule_index.write().unwrap();  // Changed: write()
        index.entry((head.clone(), arity))
             .or_insert_with(Vec::new)
             .push(rule);
    } else {
        let mut wildcards = self.wildcard_rules.write().unwrap();  // Changed: write()
        wildcards.push(rule);
    }

    // Add to MORK Space
    self.add_to_space(&rule_sexpr);

    // Mark as modified
    self.modified.store(true, Ordering::Release);
}
```

#### add_to_space()

```rust
pub fn add_to_space(&mut self, value: &MettaValue) {
    self.make_owned();  // NEW: Ensure ownership

    // EXISTING LOGIC
    let space = self.create_space();
    let mut ctx = ConversionContext::new();

    if let Ok(bytes) = metta_to_mork_bytes(value, &space, &mut ctx) {
        let mut btm = self.btm.write().unwrap();  // Changed: write()
        if let Ok(expr) = mork_expr_from_bytes(&bytes) {
            btm.insert(expr);
        }
    }

    self.modified.store(true, Ordering::Release);  // NEW: Mark modified
}
```

#### add_type()

```rust
pub fn add_type(&mut self, name: String, typ: MettaValue) {
    self.make_owned();  // NEW: Ensure ownership

    // EXISTING LOGIC
    let type_assertion = MettaValue::SExpr(vec![
        MettaValue::Atom(":".to_string()),
        MettaValue::Atom(name),
        typ,
    ]);
    self.add_to_space(&type_assertion);

    // Invalidate type index
    *self.type_index_dirty.write().unwrap() = true;  // Changed: write()

    self.modified.store(true, Ordering::Release);  // NEW: Mark modified
}
```

**Pattern**: All mutation methods follow same template:
1. Call `make_owned()` first
2. Perform modification using `.write()` locks
3. Set `modified` flag to true

### 5.5 Read Methods Update

All read methods change `lock()` → `read()`:

#### get_matching_rules()

```rust
pub fn get_matching_rules(&self, head: &str, arity: usize) -> Vec<Rule> {
    // Capacity calculation (read locks - concurrent OK!)
    let (indexed_len, wildcard_len) = {
        let index = self.rule_index.read().unwrap();       // Changed: read()
        let wildcards = self.wildcard_rules.read().unwrap(); // Changed: read()
        (
            index.get(&(head.to_string(), arity)).map_or(0, |r| r.len()),
            wildcards.len()
        )
    };

    let mut matching_rules = Vec::with_capacity(indexed_len + wildcard_len);

    // Get indexed rules (read lock)
    {
        let index = self.rule_index.read().unwrap();  // Changed: read()
        if let Some(rules) = index.get(&(head.to_string(), arity)) {
            matching_rules.extend(rules.clone());
        }
    }

    // Get wildcard rules (read lock)
    {
        let wildcards = self.wildcard_rules.read().unwrap();  // Changed: read()
        matching_rules.extend(wildcards.clone());
    }

    matching_rules
}
```

**Benefit**: Multiple threads can call `get_matching_rules()` concurrently without blocking each other.

#### get_rule_count()

```rust
pub fn get_rule_count(&self, rule: &Rule) -> usize {
    let rule_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        rule.lhs.clone(),
        rule.rhs.clone(),
    ]);
    let rule_key = rule_sexpr.to_mork_string();

    let counts = self.multiplicities.read().unwrap();  // Changed: read()
    *counts.get(&rule_key).unwrap_or(&1)
}
```

### 5.6 union() Implementation

**File**: `src/backend/environment.rs`

Replace the current no-op union with proper merging logic:

```rust
impl Environment {
    /// Merge two environments, combining their modifications.
    ///
    /// Fast paths (O(1)):
    /// - If neither modified: return self
    /// - If only other modified: return other
    /// - If only self modified: return self
    ///
    /// Slow path (O(n) where n = total rules):
    /// - Both modified: deep merge all structures
    pub fn union(&self, other: &Environment) -> Environment {
        let self_modified = self.modified.load(Ordering::Acquire);
        let other_modified = other.modified.load(Ordering::Acquire);

        // Fast path 1: Neither modified (common case)
        if !self_modified && !other_modified {
            return self.clone();  // ~20ns
        }

        // Fast path 2: Only other modified
        if !self_modified && other_modified {
            return other.clone();  // ~20ns
        }

        // Fast path 3: Only self modified
        if self_modified && !other_modified {
            return self.clone();  // ~20ns
        }

        // Slow path: Both modified - must deep merge
        self.deep_merge(other)
    }

    /// Deep merge two environments (both have modifications).
    /// Called only when both environments have been modified.
    fn deep_merge(&self, other: &Environment) -> Environment {
        // Start with clone of self
        let mut result = self.clone();
        result.make_owned();  // Ensure we own the data

        // Merge rule_index
        {
            let other_index = other.rule_index.read().unwrap();
            let mut result_index = result.rule_index.write().unwrap();

            for ((head, arity), rules) in other_index.iter() {
                result_index
                    .entry((head.clone(), *arity))
                    .or_insert_with(Vec::new)
                    .extend(rules.clone());
            }
        }

        // Merge wildcard_rules
        {
            let other_wildcards = other.wildcard_rules.read().unwrap();
            let mut result_wildcards = result.wildcard_rules.write().unwrap();
            result_wildcards.extend(other_wildcards.clone());
        }

        // Merge multiplicities (sum counts)
        {
            let other_counts = other.multiplicities.read().unwrap();
            let mut result_counts = result.multiplicities.write().unwrap();

            for (key, count) in other_counts.iter() {
                *result_counts.entry(key.clone()).or_insert(0) += count;
            }
        }

        // Merge PathMap using join operation
        {
            let other_btm = other.btm.read().unwrap();
            let mut result_btm = result.btm.write().unwrap();
            *result_btm = result_btm.join(&*other_btm);
        }

        // Merge type indices (if both have them)
        {
            let other_type_index = other.type_index.read().unwrap();
            let mut result_type_index = result.type_index.write().unwrap();

            match (&*result_type_index, &*other_type_index) {
                (Some(result_idx), Some(other_idx)) => {
                    *result_type_index = Some(result_idx.join(other_idx));
                }
                (None, Some(other_idx)) => {
                    *result_type_index = Some(other_idx.clone());
                }
                _ => {}  // Keep result's index (or None)
            }
        }

        // Invalidate type index if either is dirty
        {
            let self_dirty = *self.type_index_dirty.read().unwrap();
            let other_dirty = *other.type_index_dirty.read().unwrap();
            *result.type_index_dirty.write().unwrap() = self_dirty || other_dirty;
        }

        // Pattern cache: Keep result's cache (arbitrary choice)
        // Could merge, but LRU semantics make this complex

        result
    }
}
```

**Performance**:
```
Fast path (unmodified):  ~20-30ns  (2 atomic loads + clone)
Fast path (one modified): ~20-30ns (2 atomic loads + clone)
Slow path (both modified): ~100-200µs for 10K rules
  - rule_index merge: ~50µs
  - wildcard merge: ~10µs
  - multiplicities merge: ~50µs
  - PathMap join: ~10µs
  - type index merge: ~10µs
  - Overhead: ~20µs
```

### 5.7 Constructor Update

```rust
impl Environment {
    pub fn new() -> Self {
        Environment {
            shared_mapping: SharedMappingHandle::new(),
            btm: Arc::new(RwLock::new(PathMap::new())),
            rule_index: Arc::new(RwLock::new(HashMap::new())),
            wildcard_rules: Arc::new(RwLock::new(Vec::new())),
            multiplicities: Arc::new(RwLock::new(HashMap::new())),
            pattern_cache: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(1000).unwrap()
            ))),
            fuzzy_matcher: FuzzyMatcher::new(),
            type_index: Arc::new(RwLock::new(None)),
            type_index_dirty: Arc::new(RwLock::new(false)),

            // CoW tracking
            owns_data: true,  // New environments own their data
            modified: Arc::new(AtomicBool::new(false)),
        }
    }
}
```

---

## Performance Analysis

### 6.1 Read-Only Workload (Common Case)

**Scenario**: Evaluate 1000 expressions, no rule definitions during eval.

**Operations per evaluation**:
- 1 clone: 20ns
- 4 rule lookups (parallel): 30ns each (concurrent via RwLock)
- 4 union calls: 30ns each (fast path)

**Current implementation**:
```
Clone:       10ns
Lookups:     4 × 30ns = 120ns (serialized via Mutex)
Union:       4 × 10ns = 40ns
---
Total:       170ns per evaluation
```

**CoW implementation**:
```
Clone:       20ns (+10ns)
Lookups:     30ns (concurrent - 4 threads in parallel, not 120ns!)
Union:       4 × 30ns = 120ns (+80ns)
---
Total:       170ns per evaluation
```

**Analysis**:
- Absolute overhead: +90ns
- Typical eval time: ~20,000ns (20µs)
- Percentage impact: 90/20000 = **0.45%**

**Conclusion**: Essentially unmeasurable in practice.

### 6.2 Write-Heavy Workload (Rare Case)

**Scenario**: Define 100 rules during parallel evaluation (4 branches, 25 rules each).

**Operations**:
- 4 clones: 4 × 20ns = 80ns
- 4 make_owned() calls: 4 × 100µs = 400µs
- 100 add_rule() calls: 100 × 1µs = 100µs
- 4 deep_merge() calls: ~200µs (merging 25+25+25+25 rules)

**Current (broken, but hypothetical)**:
```
Clones:      4 × 10ns = 40ns
Rules:       100 × 1µs = 100µs
Merges:      4 × 10ns = 40ns (no-op)
---
Total:       ~100µs
```

**CoW implementation**:
```
Clones:      80ns
make_owned:  400µs
Rules:       100µs
Merges:      200µs
---
Total:       ~700µs
```

**Overhead**: 600µs for 100 rules = **6µs per rule definition**

**Analysis**: This is a rare case (dynamic rule definition), and 6µs overhead per rule is acceptable for correctness.

### 6.3 Concurrent Read Benefit

**Scenario**: 4 threads performing rule lookups simultaneously.

**Current (Mutex)**:
```
Thread 1: lock() → read → unlock (30ns)
Thread 2: lock() → WAIT → read → unlock (30ns)
Thread 3: lock() → WAIT → WAIT → read → unlock (30ns)
Thread 4: lock() → WAIT → WAIT → WAIT → read → unlock (30ns)
---
Total wall time: 120ns (serialized)
```

**CoW (RwLock)**:
```
Thread 1: read_lock() → read → unlock (30ns) ┐
Thread 2: read_lock() → read → unlock (30ns) ├─ All concurrent
Thread 3: read_lock() → read → unlock (30ns) │
Thread 4: read_lock() → read → unlock (30ns) ┘
---
Total wall time: 30ns (parallel)
```

**Improvement**: **4× speedup** for concurrent reads!

### 6.4 Memory Overhead

**Baseline (current)**:
```
Environment size (10,000 rules):
  - rule_index: ~500KB (HashMap with 10K entries)
  - wildcard_rules: ~50KB (Vec with ~100 wildcards)
  - multiplicities: ~500KB (HashMap with 10K entries)
  - btm: ~1MB (PathMap trie)
  - Caches: ~100KB
  ---
  Total: ~2MB per environment (all shared via Arc)

Memory for 4 clones: 2MB (shared)
```

**CoW implementation**:
```
Unmodified clones: 2MB (shared)

Modified clone (after make_owned()):
  - Original: 2MB (shared with unmodified clones)
  - Copy: +2MB (owned by modified clone)
  ---
  Total: 4MB (2MB shared + 2MB owned)

4 clones (all modified): 8MB (4 × 2MB)
```

**Worst case**: 4 parallel threads all modify → 4 copies → 8MB

**Typical case**: 0-1 threads modify → 2-4MB

**Acceptable**: Memory is cheap, correctness is priceless.

### 6.5 Summary Performance Table

| Metric | Current | CoW | Difference | Impact |
|--------|---------|-----|------------|--------|
| Clone (O(1)) | 10ns | 20ns | +10ns | Negligible |
| Read (single-thread) | 30ns | 30ns | 0ns | None |
| Read (4 threads) | 120ns | 30ns | **-90ns** | **4× faster** ✅ |
| Write (first) | 1µs | ~100µs | +99µs | Rare case |
| Write (subsequent) | 1µs | 1µs | 0ns | None |
| Union (unmodified) | 10ns | 30ns | +20ns | Negligible |
| Union (both modified) | 10ns | ~200µs | +200µs | Rare case |
| Full eval (read-only) | 20µs | 20.09µs | +0.09µs | **0.45%** |
| Memory (unmodified) | 2MB | 2MB | 0MB | None |
| Memory (all modified) | 2MB | 8MB | +6MB | Acceptable |

**Key Takeaway**: < 1% overhead for common case (read-only), significant safety improvement.

---

## Implementation Phases

### Phase 1: Core CoW Infrastructure (CRITICAL)

**Goal**: Implement Copy-on-Write semantics with proper isolation and merging.

**Estimated Effort**: 8-10 hours

**Tasks**:
1. ✅ Add `owns_data` and `modified` fields to Environment
2. ✅ Replace `Mutex` with `RwLock` throughout
3. ✅ Implement `make_owned()` method
4. ✅ Update `Clone` trait implementation
5. ✅ Update all mutation methods (add_rule, add_to_space, add_type, etc.)
6. ✅ Update all read methods (get_matching_rules, etc.)
7. ✅ Implement proper `union()` and `deep_merge()`
8. ✅ Update constructor

**Files Modified**:
- `src/backend/environment.rs`: ~300-400 LOC changes

**Deliverables**:
- Working CoW implementation
- All existing tests pass
- No performance regression on read-only workloads

**Success Criteria**:
- ✅ Isolated environments after clone + write
- ✅ Proper union merges modifications
- ✅ No race conditions under parallel evaluation
- ✅ 403+ existing tests pass

---

### Phase 2: Epoch-Based Cache Invalidation (OPTIMIZATION)

**Goal**: Add version tracking for cache invalidation.

**Estimated Effort**: 3-4 hours

**Tasks**:
1. ✅ Add `epoch: Arc<AtomicU64>` field to Environment
2. ✅ Increment epoch on all write operations
3. ✅ Update `metta_to_mork_bytes_cached()` to validate epoch
4. ✅ Update pattern cache to store (epoch, bytes) tuples

**Files Modified**:
- `src/backend/environment.rs`: ~50-100 LOC changes

**Deliverables**:
- Epoch-aware caching
- Automatic cache invalidation on writes

**Success Criteria**:
- ✅ Caches invalidated when environment modified
- ✅ No stale cache reads
- ✅ Minimal overhead (< 10ns per cache check)

---

### Phase 3: Read-Mostly Optimization (OPTIONAL)

**Goal**: Avoid unnecessary CoW copies when possible.

**Estimated Effort**: 2-3 hours

**Tasks**:
1. ✅ Check `Arc::strong_count()` in `make_owned()`
2. ✅ Skip deep copy if strong_count == 1 (sole owner)
3. ✅ Claim ownership in-place

**Files Modified**:
- `src/backend/environment.rs`: ~20-30 LOC changes

**Deliverables**:
- Optimized make_owned() for single-reference case

**Success Criteria**:
- ✅ Avoids copy when sole reference
- ✅ Maintains correctness
- ✅ ~90% reduction in make_owned() calls for typical workloads

**Note**: This is optional - only implement if benchmarks show need.

---

### Phase 4: Comprehensive Testing (CRITICAL)

**Goal**: Ensure correctness through exhaustive test coverage.

**Estimated Effort**: 6-8 hours

**Test Categories**:

#### 4.1 Unit Tests (environment.rs)
```rust
#[cfg(test)]
mod cow_tests {
    // Basic CoW behavior
    test_clone_is_cheap()              // Clone < 100ns
    test_clone_doesnt_own_data()       // owns_data == false
    test_make_owned_creates_copy()     // Arc pointers differ after make_owned

    // Isolation tests
    test_write_isolates_environments() // env1 write doesn't affect env2
    test_parallel_writes_isolated()    // 10 threads, each write different rule

    // Union tests
    test_union_unmodified_fast()       // Union < 100ns if unmodified
    test_union_one_modified()          // Returns modified one
    test_union_both_modified()         // Deep merges correctly

    // Correctness tests
    test_all_rules_merged()            // Union contains all rules
    test_multiplicities_summed()       // Counts added correctly
    test_pathmap_joined()              // Facts merged properly
}
```

#### 4.2 Integration Tests (eval/mod.rs)
```rust
#[cfg(test)]
mod parallel_cow_tests {
    // Parallel evaluation with dynamic rules
    test_dynamic_rule_in_branch()      // Define rule in if-branch, use it
    test_parallel_branches_isolated()  // Branch A rules not in Branch B
    test_union_after_parallel_eval()   // Merged env has all rules

    // Edge cases
    test_nested_parallel_evaluation()  // Recursive parallel evals
    test_rule_visibility_timing()      // When are rules visible?
}
```

#### 4.3 Property-Based Tests
```rust
use proptest::prelude::*;

proptest! {
    // Forall environments e1, e2:
    // e1.union(e2) contains all rules from both
    #[test]
    fn prop_union_includes_all_rules(
        rules1: Vec<Rule>,
        rules2: Vec<Rule>
    ) {
        let mut env1 = Environment::new();
        for r in &rules1 { env1.add_rule(r.clone()); }

        let mut env2 = Environment::new();
        for r in &rules2 { env2.add_rule(r.clone()); }

        let unified = env1.union(&env2);

        // All rules from env1 and env2 should be in unified
        // ...
    }
}
```

#### 4.4 Stress Tests
```rust
#[test]
#[ignore]  // Run with --ignored
fn stress_100_threads_parallel_modification() {
    // 100 threads, 1000 rules each
    // Verify isolation and correctness
}

#[test]
#[ignore]
fn stress_deep_merge_10k_rules() {
    // Two environments with 10K rules each
    // Merge and verify all present
}
```

**Files Added**:
- `src/backend/environment.rs`: ~300-400 LOC (in #[cfg(test)])
- `src/backend/eval/mod.rs`: ~100-150 LOC (in #[cfg(test)])

**Success Criteria**:
- ✅ 100% code coverage for CoW paths
- ✅ All property tests pass
- ✅ Stress tests pass (--ignored)
- ✅ No flaky tests

---

### Phase 5: Performance Benchmarking (VALIDATION)

**Goal**: Measure and validate performance impact.

**Estimated Effort**: 2-3 hours

**Benchmark Suite**:

```rust
// benches/cow_environment.rs

use criterion::{black_box, criterion_group, criterion_main, Criterion};

// Clone benchmarks
fn bench_clone_unmodified(c: &mut Criterion) { /* ... */ }
fn bench_clone_modified(c: &mut Criterion) { /* ... */ }

// Write benchmarks
fn bench_first_write(c: &mut Criterion) { /* ... */ }
fn bench_subsequent_writes(c: &mut Criterion) { /* ... */ }

// Union benchmarks
fn bench_union_unmodified(c: &mut Criterion) { /* ... */ }
fn bench_union_both_modified(c: &mut Criterion) { /* ... */ }

// Read benchmarks
fn bench_concurrent_reads(c: &mut Criterion) {
    // Compare Mutex (current) vs RwLock (CoW)
}

// Full evaluation benchmarks
fn bench_eval_read_only(c: &mut Criterion) { /* ... */ }
fn bench_eval_with_writes(c: &mut Criterion) { /* ... */ }

criterion_group!(
    benches,
    bench_clone_unmodified,
    bench_clone_modified,
    bench_first_write,
    bench_subsequent_writes,
    bench_union_unmodified,
    bench_union_both_modified,
    bench_concurrent_reads,
    bench_eval_read_only,
    bench_eval_with_writes
);
criterion_main!(benches);
```

**Files Added**:
- `benches/cow_environment.rs`: ~200-300 LOC

**Metrics to Track**:
- Clone latency (ns)
- make_owned latency (µs)
- Union latency (ns or µs)
- Concurrent read throughput (ops/sec)
- Full evaluation latency (µs)
- Memory usage (MB)

**Acceptance Criteria**:
- ✅ Read-only eval: < 1% regression
- ✅ Concurrent reads: ≥ 2× improvement
- ✅ Memory overhead: < 3× worst case

---

### Phase 6: Documentation (COMMUNICATION)

**Goal**: Document CoW semantics, usage, and best practices.

**Estimated Effort**: 3-4 hours

**Documents to Create/Update**:

#### 6.1 Design Documentation
- ✅ `docs/design/COW_ENVIRONMENT_DESIGN.md` (this file)
- ✅ `docs/design/COW_IMPLEMENTATION_GUIDE.md` (implementation reference)

#### 6.2 Threading Model Update
- ✅ `docs/THREADING_MODEL.md`: Add CoW section

#### 6.3 User-Facing Documentation
- ✅ `CLAUDE.md`: Add dynamic rule definition section
- ✅ `README.md`: Note CoW semantics (if relevant)

#### 6.4 Code Examples
- ✅ `examples/dynamic_rules.metta`: MeTTa example
- ✅ `examples/cow_usage.rs`: Rust backend example

**Success Criteria**:
- ✅ Clear explanation of CoW semantics
- ✅ Performance characteristics documented
- ✅ Examples for common use cases
- ✅ Best practices guide

---

## Migration Path

### 9.1 Backward Compatibility

**Goal**: Existing code should continue to work without modification.

**Guarantee**: All 403+ existing tests pass without changes.

**Why it works**:
- Clone signature unchanged: `fn clone(&self) -> Self`
- Read methods unchanged (aside from Mutex → RwLock, transparent)
- Write methods unchanged (aside from internal make_owned() call)
- union() signature unchanged: `fn union(&self, other: &Environment) -> Environment`

**Breaking changes**: NONE (internal implementation only)

### 9.2 Incremental Rollout

**Step 1**: Implement Phase 1 (CoW core) in feature branch

**Step 2**: Run full test suite
```bash
cargo test --all
```
Expected: All tests pass (403+)

**Step 3**: Run benchmarks, compare against baseline
```bash
cargo bench --bench cow_environment > results_cow.txt
git checkout main
cargo bench --bench expression_parallelism > results_baseline.txt
# Compare results
```

**Step 4**: Review performance impact
- If < 5% regression: Proceed to merge
- If 5-10% regression: Investigate, optimize (Phase 3)
- If > 10% regression: Re-evaluate approach

**Step 5**: Merge to main if acceptance criteria met

**Step 6**: Monitor production usage (if applicable)

### 9.3 Rollback Plan

**If performance unacceptable**:

1. **Immediate**: Revert CoW commit
```bash
git revert <cow-commit-hash>
```

2. **Short-term**: Add runtime assertion
```rust
pub fn add_rule(&mut self, rule: Rule) {
    debug_assert!(
        !is_parallel_context(),
        "Rule definition during parallel evaluation is not supported"
    );
    // ... existing logic ...
}
```

3. **Long-term**: Document limitation
```markdown
## Known Limitations

Dynamic rule definition during parallel evaluation has undefined behavior.
Always define rules at top-level before evaluation.
```

4. **Alternative**: Consider simpler approaches (e.g., just epoch tracking without CoW)

---

## Risks and Mitigations

### 10.1 Performance Regression

**Risk**: CoW overhead too high for production workloads.

**Likelihood**: Low (analysis shows < 1% impact)

**Impact**: Medium (slower evaluation)

**Mitigation**:
- Comprehensive benchmarks before merge
- Performance acceptance criteria (< 5% regression)
- Phase 3 optimizations (Arc::strong_count check)
- Rollback plan ready

**Detection**:
- Automated benchmarks in CI
- Performance monitoring in production

---

### 10.2 Complex Merge Logic Bugs

**Risk**: deep_merge() implementation has subtle bugs.

**Likelihood**: Medium (complex state, many data structures)

**Impact**: High (incorrect results, data loss)

**Mitigation**:
- Exhaustive test coverage (unit, integration, property-based)
- Stress tests with large environments
- Code review with focus on merge logic
- Incremental implementation (one structure at a time)

**Detection**:
- Test failures
- Assertion failures in debug builds
- User reports of incorrect evaluation

---

### 10.3 Memory Overhead

**Risk**: Many modified clones cause excessive memory usage.

**Likelihood**: Low (writes rare in typical workloads)

**Impact**: Medium (OOM in extreme cases)

**Mitigation**:
- Memory profiling during benchmarks
- Stress tests with many parallel modifications
- Documentation of memory characteristics
- Phase 3 optimization (avoid copies when sole reference)

**Detection**:
- Memory monitoring
- OOM errors
- Benchmark memory metrics

---

### 10.4 Subtle Concurrency Bugs

**Risk**: RwLock usage introduces deadlocks or race conditions.

**Likelihood**: Low (RwLock well-understood, standard library)

**Impact**: High (deadlocks, data races)

**Mitigation**:
- Lock ordering discipline (always acquire in same order)
- Minimize lock hold times (clone and release pattern)
- Thorough testing with thread sanitizer
- Code review with concurrency expert

**Detection**:
- Thread sanitizer in CI
- Deadlock detection tools
- Stress tests under high concurrency

---

### 10.5 Breaking Existing Code

**Risk**: CoW changes break existing code in subtle ways.

**Likelihood**: Low (backward compatible API)

**Impact**: High (requires user code changes)

**Mitigation**:
- Run all 403+ existing tests
- Test with real-world MeTTa programs
- Beta testing period before stable release
- Clear migration guide (even though none needed)

**Detection**:
- Test failures
- User bug reports
- Integration test failures

---

## Alternatives Considered

### 11.1 Epoch-Based Synchronization (Rejected)

**Approach**: Track environment versions, invalidate stale reads.

**Pros**:
- ✅ Minimal overhead (atomic increment/load)
- ✅ Detects stale reads

**Cons**:
- ❌ Doesn't prevent concurrent modifications
- ❌ Doesn't solve union() problem
- ❌ Cache invalidation on every write (even unrelated)

**Verdict**: Useful as optimization (Phase 2), not as primary solution.

---

### 11.2 Read-Write Locks Only (Rejected)

**Approach**: Replace Mutex with RwLock, keep Arc-sharing.

**Pros**:
- ✅ Concurrent reads (performance win)
- ✅ Simple implementation

**Cons**:
- ❌ Doesn't provide isolation
- ❌ Still has race conditions on writes
- ❌ union() still broken

**Verdict**: Good for performance (included in CoW), insufficient alone.

---

### 11.3 Batch + Flush (Rejected)

**Approach**: Buffer writes, apply in batch at synchronization points.

**Pros**:
- ✅ Amortizes lock overhead
- ✅ Enables bulk optimizations

**Cons**:
- ❌ Delayed visibility (complex semantics)
- ❌ Easy to forget flush (correctness risk)
- ❌ Still need proper union()

**Verdict**: Too error-prone, doesn't solve core problem.

---

### 11.4 MVCC/Snapshots (Rejected)

**Approach**: Every write creates new snapshot, readers see consistent view.

**Pros**:
- ✅ Lock-free reads
- ✅ Clean transactional semantics

**Cons**:
- ❌ **Very expensive**: Every write clones entire environment
- ❌ High memory overhead (multiple snapshots)
- ❌ Write amplification (every small write = full clone)

**Verdict**: Too expensive for this use case (frequent rule additions).

---

### 11.5 Message Passing (Rejected)

**Approach**: Central environment manager, threads send write messages.

**Pros**:
- ✅ No shared mutable state
- ✅ Clear ownership

**Cons**:
- ❌ Requires complete redesign
- ❌ High latency (message passing overhead)
- ❌ Complex synchronization
- ❌ Doesn't fit functional paradigm

**Verdict**: Too invasive, poor fit for functional evaluation model.

---

### 11.6 Why Copy-on-Write?

**CoW is the best fit because**:

1. **Correctness**: Provides isolation without race conditions
2. **Performance**: Optimizes common case (read-only)
3. **Semantics**: Matches functional programming expectations
4. **Pragmatic**: Moderate implementation complexity
5. **Incremental**: Can optimize further (Phase 3)
6. **Proven**: Well-understood pattern (e.g., std::borrow::Cow, Git)

**Trade-offs accepted**:
- First write penalty (~100µs) acceptable for rare case
- Memory overhead (2-4× worst case) acceptable for correctness
- Implementation complexity (300 LOC) manageable

---

## References

### Internal Documentation

- `docs/THREADING_MODEL.md`: Current parallelism architecture
- `docs/optimization/OPTIMIZATION_4_REJECTED.md`: Rejected parallel bulk operations
- `docs/optimization/EXPRESSION_PARALLELISM_THRESHOLD_TUNING_PLAN.md`: Parallel eval tuning
- `src/backend/environment.rs`: Current environment implementation
- `src/backend/eval/mod.rs`: Evaluation engine

### External Resources

- [Copy-on-Write (Wikipedia)](https://en.wikipedia.org/wiki/Copy-on-write)
- [Rust std::borrow::Cow](https://doc.rust-lang.org/std/borrow/enum.Cow.html)
- [Arc vs Rc in Rust](https://doc.rust-lang.org/std/sync/struct.Arc.html)
- [RwLock documentation](https://doc.rust-lang.org/std/sync/struct.RwLock.html)
- [Atomic operations](https://doc.rust-lang.org/std/sync/atomic/)

### Academic Papers

- "The Art of Multiprocessor Programming" (Herlihy & Shavit) - Chapter 8: Spin Locks and Contention
- "Persistent Data Structures" (Okasaki) - Functional data structures with structural sharing

---

## Appendix

### A. Performance Measurement Methodology

**Hardware**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads)

**Benchmarking**:
```bash
# CPU affinity for consistency
taskset -c 0-17 cargo bench --bench cow_environment

# Perf profiling
taskset -c 0-17 perf record -g cargo bench
perf report
```

**Metrics**:
- Latency: Median, p95, p99
- Throughput: Operations per second
- Memory: Peak RSS, allocations
- Concurrency: Speedup vs single-threaded

### B. Code Review Checklist

**Correctness**:
- [ ] All mutations call make_owned() first
- [ ] All reads use .read(), all writes use .write()
- [ ] union() handles all cases (unmodified, one modified, both modified)
- [ ] deep_merge() includes all data structures
- [ ] Clone creates fresh modified flag (not shared)

**Performance**:
- [ ] Fast paths hit for common cases
- [ ] Locks held minimally
- [ ] No unnecessary allocations
- [ ] Benchmarks show < 1% regression

**Safety**:
- [ ] No data races (checked with thread sanitizer)
- [ ] No deadlocks (lock ordering consistent)
- [ ] No memory leaks (checked with valgrind)
- [ ] Proper error handling

**Testing**:
- [ ] All existing tests pass
- [ ] New CoW tests cover all paths
- [ ] Property tests validate invariants
- [ ] Stress tests pass

**Documentation**:
- [ ] CoW semantics explained
- [ ] Performance characteristics documented
- [ ] Examples provided
- [ ] Migration guide (if needed)

### C. Glossary

**CoW (Copy-on-Write)**: Optimization technique where data is shared until modification, at which point a copy is made.

**Arc**: Atomic Reference Counted pointer - enables shared ownership across threads.

**RwLock**: Read-Write lock - allows multiple concurrent readers OR single writer.

**Mutex**: Mutual exclusion lock - allows only one accessor (reader or writer) at a time.

**Epoch**: Version number used to detect stale data.

**make_owned()**: Method that triggers deep copy to gain exclusive ownership.

**union()**: Method that merges two environments, combining their modifications.

**Isolation**: Property where modifications in one environment don't affect others.

---

**End of Design Specification**
