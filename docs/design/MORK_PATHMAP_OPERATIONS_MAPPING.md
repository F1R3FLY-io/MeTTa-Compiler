# MORK and PathMap Operations Mapping for Built-in Functions

This document maps available operations in MORK and PathMap to pending MeTTa built-in functions, identifying optimization opportunities and implementation strategies.

## MORK Expression Operations

### Available in `mork_expr::Expr`

| Operation | Location | Description | Time Complexity |
|-----------|----------|-------------|----------------|
| `size()` | expr/src/lib.rs:280 | Count total nodes in expression | O(n) |
| `leaves()` | expr/src/lib.rs:284 | Count leaf nodes | O(n) |
| `expressions()` | expr/src/lib.rs:288 | Count sub-expressions | O(n) |
| `symbols()` | expr/src/lib.rs:292 | Count symbols | O(n) |
| `newvars()` | expr/src/lib.rs:296 | Count new variables | O(n) |
| `references()` | expr/src/lib.rs:300 | Count variable references | O(n) |
| `variables()` | expr/src/lib.rs:309 | Count all variables | O(n) |
| `max_arity()` | expr/src/lib.rs:313 | Find maximum arity | O(n) |
| `has_unbound()` | expr/src/lib.rs:317 | Check for unbound variables | O(n) |
| `difference(Expr)` | expr/src/lib.rs:328 | Find first difference location | O(n) |
| `substitute(&[Expr])` | expr/src/lib.rs:367 | Apply variable substitutions | O(n) |
| `unify(Vec<(ExprEnv, ExprEnv)>)` | expr/src/lib.rs:1927 | Unification with occurs check | O(n*m) |
| `apply()` | expr/src/lib.rs:1714 | Apply unification bindings | O(n) |
| `span()` | expr/src/lib.rs:268 | Get byte span of expression | O(n) |
| `symbol()` | expr/src/lib.rs:261 | Extract symbol if present | O(1) |

## PathMap Operations

### Core Trie Operations

| Operation | Description | Time Complexity |
|-----------|-------------|----------------|
| `set_val_at(&[u8], T)` | Insert value at path | O(k) where k=path length |
| `get_val_at(&[u8])` | Retrieve value at path | O(k) |
| `contains(&[u8])` | Check if path exists | O(k) |
| `val_count()` | Count values in trie | O(1) |
| `iter()` | Iterate over (path, value) pairs | O(n) |
| `is_empty()` | Check if trie is empty | O(1) |

### Ring/Algebraic Operations

| Operation | Description | Time Complexity | Use Cases |
|-----------|-------------|----------------|-----------|
| `join(&PathMap)` | Union of two tries | O(n+m) | Set union, space merge |
| `meet(&PathMap)` | Intersection of two tries | O(n+m) | Set intersection, common facts |
| `subtract(&PathMap)` | Set difference | O(n+m) | Remove facts, difference |
| `restrict(&PathMap)` | Prefix restriction | O(n) | Filter by prefix |

### Zipper Operations (Cursors)

| Operation | Description | Use Cases |
|-----------|-------------|-----------|
| `read_zipper()` | Create read-only cursor | Efficient traversal |
| `write_zipper()` | Create read-write cursor | Modifications |
| `descend_to(&[u8])` | Navigate to path | Path-based queries |
| `ascend()` | Move up in trie | Backtracking |
| `child_mask()` | Get available children | Branch exploration |
| `path()` | Get current path | Location tracking |

## MORK Space Operations

### Data Management

| Operation | Location | Description |
|-----------|----------|-------------|
| `add_all_sexpr(&[u8])` | space.rs:818 | Add S-expressions to space |
| `remove_all_sexpr(&[u8])` | space.rs:819 | Remove S-expressions from space |
| `add_sexpr(pattern, template)` | space.rs:843 | Add with pattern/template |
| `remove_sexpr(pattern, template)` | space.rs:844 | Remove with pattern/template |
| `load_json(&[u8])` | space.rs:536 | Load JSON data |
| `load_jsonl(&[u8])` | space.rs:601 | Load JSON Lines |

### Query Operations

| Operation | Location | Description | Performance |
|-----------|----------|-------------|------------|
| `query_multi(pattern, effect)` | space.rs:988 | Pattern matching with unification | O(m) matches |
| `transform_multi_multi_(p, t, a)` | space.rs:1151 | Transform with pattern/template | O(m) transforms |

### Persistence

| Operation | Location | Description |
|-----------|----------|-------------|
| `backup_tree(path)` | space.rs:964 | Serialize Merkle tree |
| `restore_tree(path)` | space.rs:969 | Deserialize Merkle tree |
| `backup_paths(path)` | space.rs:978 | Serialize paths |
| `restore_paths(path)` | space.rs:983 | Deserialize paths |
| `backup_symbols(path)` | space.rs:945 | Serialize symbol table |
| `restore_symbols(path)` | space.rs:956 | Deserialize symbol table |

## Mapping to Pending Built-in Functions

### Expression Manipulation (#16)

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `size-atom` | `Expr::size()` | Direct use | **HIGH** ✅ |
| `cons-atom` | MORK expression construction | Build expr from head+tail | HIGH |
| `decons-atom` | MORK traversal | Extract head and tail | HIGH |
| `car-atom` | MORK traversal (first child) | Extract first element | HIGH |
| `cdr-atom` | MORK traversal (skip first) | Extract remaining elements | HIGH |
| `index-atom` | MORK traversal with counter | Navigate to index | MEDIUM |

**Implementation Notes:**
- `size-atom`: Call `metta_to_mork_expr(expr).size()`
- `cons-atom`, `decons-atom`, `car-atom`, `cdr-atom`: Use MORK's expression traversal macros
- All operations O(n) or better

### List Operations (#22)

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `map-atom` | `Expr::substitute()` + traversal | Traverse list, apply function, collect | **HIGH** |
| `filter-atom` | `Space::query_multi()` | Query with predicate, collect matches | **HIGH** |
| `foldl-atom` | MORK traversal + accumulator | Traverse left-to-right with fold | **HIGH** |

**Implementation Notes:**
- `map-atom`: Use `traverseh!` macro to traverse and transform
- `filter-atom`: Can use `query_multi` for pattern-based filtering
- `foldl-atom`: Implement as recursive evaluation with accumulator

### Non-deterministic Operations (#19)

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `superpose` | Native MeTTa semantics | Return multiple alternatives | **HIGH** |
| `collapse` | `Space::query_multi()` | Collect all alternatives into list | **HIGH** |
| `collapse-bind` | `Space::query_multi()` bindings | Return alternatives with bindings | MEDIUM |
| `superpose-bind` | Native + bindings | Inverse of collapse-bind | MEDIUM |

**Implementation Notes:**
- `collapse` is **critical** for robot planning issue #25 (shortest path selection)
- Use `query_multi` to collect all alternatives, then package as list
- `collapse-bind` can return `BTreeMap<(u8, u8), ExprEnv>` from unification

### Space Operations (#20)

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `add-atom` | `Space::add_all_sexpr()` | Direct use | **HIGH** ✅ |
| `remove-atom` | `Space::remove_all_sexpr()` | Direct use | **HIGH** ✅ |
| `get-atoms` | PathMap `iter()` | Iterate space, convert to list | **HIGH** ✅ |
| `new-space` | Create `Space::new()` | Allocate new MORK space | MEDIUM |
| `add-atoms` | Batch `add_sexpr` | Multiple insertions | MEDIUM |
| `add-reduct` | Evaluate then `add_sexpr` | Eval + insert | MEDIUM |

**Implementation Notes:**
- Space operations map **directly** to MORK
- `get-atoms` can use efficient PathMap iteration
- Multi-space support requires space management in MettaState

### Set Operations (Can optimize with PathMap)

| Function | PathMap Operation | Implementation Strategy | Priority |
|----------|-------------------|------------------------|----------|
| `unique-atom` | PathMap dedupe | Insert into PathMap, extract values | MEDIUM |
| `union-atom` | `PathMap::join()` | Direct use | MEDIUM ✅ |
| `intersection-atom` | `PathMap::meet()` | Direct use | MEDIUM ✅ |
| `subtraction-atom` | `PathMap::subtract()` | Direct use | MEDIUM ✅ |

**Implementation Notes:**
- PathMap's ring operations provide **O(n+m)** set operations
- Much faster than naive list-based implementations
- Requires conversion: List → PathMap → Operation → List

### Unification & Pattern Matching

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `unify` | `mork_expr::unify()` | Wrapper for 4-arg MeTTa API | **HIGH** ✅ |
| `match` | `Space::query_multi()` | Already used in MeTTaTron | **DONE** ✅ |

**Implementation Notes:**
- Issue #15 already documents `unify` implementation
- `query_multi` provides efficient pattern matching with O(m) matches
- Unification returns bindings for variable substitution

### Evaluation Control (#14)

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `chain` | `Expr::substitute()` | Eval first, bind, eval second | **HIGH** |
| `unquote` | MORK traversal | Remove quote wrapper | MEDIUM |
| `evalc` | `Space::query_multi()` context | Evaluate in specific space | MEDIUM |
| `function`/`return` | Native evaluation | Control flow mechanism | MEDIUM |

**Implementation Notes:**
- `chain` can use `substitute()` for variable binding
- `evalc` requires space-aware evaluation

### Control Flow (#15)

| Function | MORK/PathMap Operation | Implementation Strategy | Priority |
|----------|------------------------|------------------------|----------|
| `case` | Pattern matching logic | Match patterns sequentially | **HIGH** |
| `switch` | Similar to case | Handle Empty specially | HIGH |
| `let*` | Sequential `substitute()` | Chain bindings | MEDIUM |

**Implementation Notes:**
- Pure evaluation logic, no direct MORK operations
- Can use `unify` for pattern matching in `case`/`switch`

### Logical Operations (#18)

No MORK/PathMap operations needed - pure boolean logic in MeTTa evaluation.

### Math Operations (#17)

No MORK/PathMap operations needed - pure Rust arithmetic operations.

## Optimization Opportunities

### High-Impact MORK/PathMap Optimizations

1. **Pattern Matching** (Issue #12)
   - Current: O(n*m) manual iteration in `match_space()`
   - Optimized: Use `Space::query_multi()` for O(m) performance
   - **Impact: 10-1000x speedup**

2. **Rule Counting** (Issue #12)
   - Current: O(n*m) via `iter_rules().count()`
   - Optimized: Use `PathMap::val_count()` for O(1)
   - **Impact: 1000x speedup**

3. **Type Queries** (Issue #12)
   - Current: O(n) iteration in `get_type()`
   - Optimized: Use PathMap prefix queries
   - **Impact: 10-100x speedup**

4. **Set Operations** (New optimization)
   - Current: O(n*m) list operations
   - Optimized: PathMap `join`/`meet`/`subtract` O(n+m)
   - **Impact: 10-100x speedup for large sets**

5. **Expression Analysis** (New capability)
   - Use MORK `Expr::size()`, `leaves()`, `symbols()` directly
   - Enables efficient implementation of `size-atom`, structural queries

### Implementation Priority

**Phase 1: High-Impact Space Operations** (Issues #20, #12)
- Implement `add-atom`, `remove-atom`, `get-atoms`
- Optimize `match` with `query_multi`
- Optimize `rule_count` with `val_count`

**Phase 2: Expression Manipulation** (Issue #16)
- Implement `cons-atom`, `decons-atom`, `car-atom`, `cdr-atom`
- Implement `size-atom` using MORK `Expr::size()`
- Implement `index-atom`

**Phase 3: Non-deterministic Operations** (Issue #19)
- Implement `collapse` using `query_multi` (**critical for #25**)
- Implement `superpose`
- Implement `collapse-bind`, `superpose-bind`

**Phase 4: List Operations** (Issue #22)
- Implement `map-atom`, `filter-atom`, `foldl-atom`
- Use MORK traversal for efficiency

**Phase 5: Set Operations** (New issue needed)
- Implement `unique-atom`, `union-atom`, `intersection-atom`, `subtraction-atom`
- Use PathMap ring operations

## Performance Impact Summary

| Operation Category | Current | With MORK/PathMap | Speedup |
|-------------------|---------|-------------------|---------|
| Pattern matching | O(n*m) | O(m) | **10-1000x** |
| Rule counting | O(n*m) | O(1) | **1000x** |
| Type queries | O(n) | O(k) | **10-100x** |
| Set operations | O(n*m) | O(n+m) | **10-100x** |
| Expression size | O(n) | O(n) | Direct API |
| Space operations | Manual | Native | **Correct** |

## Dependencies

### Type Conversion (Issue #17 - Type Conversion Utilities)

All MORK/PathMap operations require conversion between MettaValue and MORK representations:
- `metta_to_mork_expr(MettaValue) -> Expr`
- `mork_expr_to_metta(Expr) -> MettaValue`
- `metta_to_pathmap_path(MettaValue) -> Vec<u8>`
- `pathmap_path_to_metta(Vec<u8>) -> MettaValue`

**This is the critical dependency** blocking many implementations.

## References

- **MORK Expression API**: `MORK/expr/src/lib.rs`
- **MORK Space API**: `MORK/kernel/src/space.rs`
- **PathMap API**: `PathMap/src/lib.rs`
- **PathMap Ring Operations**: `PathMap/src/ring.rs`
- **MeTTa Built-in Functions**: `docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md`
- **Pattern Matching Issue**: #12
- **Space Operations Issue**: #20
- **Expression Manipulation Issue**: #16
- **Non-deterministic Operations Issue**: #19
- **List Operations Issue**: #22
- **Robot Planning (needs collapse)**: #25
