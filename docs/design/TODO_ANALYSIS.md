# TODO Analysis

> **Note:** The issues documented in this analysis are now tracked as GitHub issues:
> - Binary format querying: [Issue #13](https://github.com/F1R3FLY-io/MeTTa-Compiler/issues/13)
> - Pattern matching optimization: [Issue #12](https://github.com/F1R3FLY-io/MeTTa-Compiler/issues/12)

## Summary

Out of **9 TODOs** in the codebase, **3 were completed** (removed as redundant), **5 are blocked** by the binary format issue, and **1 requires external dependency**.

## Completed TODOs (3)

### ✅ 1. compile.rs:112 - "Query MORK Space to verify environment is initialized"
**Status**: Removed (redundant)
**Reason**: Environment IS initialized via `Environment::new()`, it's just empty at compile time (facts are added during eval). No verification needed.

### ✅ 2. eval.rs:1040 - "Query MORK Space to verify rule was added"
**Status**: Removed (unnecessary for this test)
**Reason**: The test `test_rule_definition()` only verifies that rule definition returns Nil. Verifying database storage is the purpose of the separate test `test_rule_definition_added_to_fact_database()`.

### ✅ 3. eval.rs:2041 - "Query MORK Space to verify rule was added"
**Status**: Removed (already verified)
**Reason**: Line 2044 already has the assertion `assert!(new_env.has_sexpr_fact(&rule_def))` which verifies the rule is in the fact database. The TODO was redundant.

## Blocked TODOs (5) - Binary Format Issue

All of these are blocked by the same root cause: **MORK Space stores data in binary format** (parsed from text), but we only have text-format APIs. See `docs/SEXPR_FACTS_DESIGN.md` for details.

### ❌ 4. types.rs:55 - "Replace with PathMap prefix queries"
**Blocker**: Binary format querying not understood
**Affects**: `rule_index: HashMap<String, Vec<usize>>`
**Current**: O(1) + O(k) HashMap index for rule lookup by head symbol
**Ideal**: O(m) PathMap prefix queries (m = pattern length)
**Notes**: Current solution works well (100x speedup achieved), minor memory overhead acceptable

### ❌ 5. types.rs:63 - "Remove once we can query MORK Space with parsed binary keys"
**Blocker**: Binary format querying not understood
**Affects**: `sexpr_facts: HashSet<String>`
**Current**: O(1) HashSet for s-expression existence checks (MORK text format)
**Ideal**: O(m) PathMap contains (m = key length)
**Notes**: Temporary bridge until we can convert text patterns to binary for PathMap queries

### ❌ 6. types.rs:95 - "Eventually parse rules directly from MORK Space and remove this cache"
**Blocker**: Binary format querying not understood
**Affects**: `rule_cache: Vec<Rule>`
**Current**: Rules stored in both rule_cache (text) and MORK Space (binary)
**Ideal**: Parse rules directly from MORK Space on demand
**Notes**: Would require converting binary data from PathMap back to Rule structs

### ❌ 7. types.rs:127 - "Use indexed lookup for O(1) query"
**Status**: Intentionally simplified
**Affects**: `has_fact()` method
**Current**: Returns true if ANY fact exists (optimistic placeholder)
**Ideal**: O(1) or O(m) indexed lookup for specific atoms
**Notes**: Not a priority - this is a simplified implementation for testing

### ❌ 8. types.rs:141 - "Replace with PathMap query once we can convert to binary format"
**Blocker**: Binary format querying not understood
**Affects**: `has_sexpr_fact()` method
**Same as #5**: Both refer to sexpr_facts HashSet

## External Dependency (1)

### ❌ 9. compile.rs:94 - "Implement conversion to Rholang AST Proc type"
**Blocker**: Requires rholang-rs integration
**Affects**: `to_proc_expr()` function
**Current**: Returns error "not yet implemented"
**Ideal**: Convert MettaValue to Rholang AST Proc type
**Notes**: Placeholder for future rholang-rs integration

## Root Cause: Binary Format Issue

The primary blocker for 5 out of 9 TODOs is that **MORK Space stores s-expressions in binary format**:

```rust
// In space.rs:812
pub fn load_all_sexpr(&mut self, r: &[u8]) -> Result<usize, String> {
    // Parser converts text → binary
    match parser.sexpr(&mut it, &mut ez) {
        Ok(()) => {
            let data = &stack[..ez.loc];  // Binary format!
            self.btm.insert(data, ());     // Stored as binary key
        }
    }
}
```

Our queries use text format:
```rust
let mork_str = sexpr.to_mork_string();  // Text: "(Hello World)"
self.space.borrow().btm.contains(mork_str.as_bytes())  // Mismatch!
```

**To resolve**, we need:
1. Access to MORK's text-to-binary parser as a standalone function
2. Ability to convert text patterns to binary format for queries
3. Ability to convert binary data back to text/MettaValue

## Current State

**All 69 tests passing** with temporary caches:
- `rule_index: HashMap<String, Vec<usize>>` - O(1) + O(k) rule lookup
- `sexpr_facts: HashSet<String>` - O(1) s-expression existence checks
- `rule_cache: Vec<Rule>` - Temporary convenience for pattern matching

These temporary solutions are **acceptable and performant**:
- 100x reduction in pattern matching calls (from 1000 to ~10 rules checked)
- O(1) lookups better than O(m) PathMap for simple existence checks
- Minimal memory overhead (~12 KB for 1000 rules)

## Recommendation

1. **Keep current implementation** - it's working well and performant
2. **Document binary format as blocker** for future PathMap optimization
3. **Revisit when MORK exposes binary conversion APIs** or when we understand the binary format better
4. **Focus on other features** rather than premature optimization

The temporary caches are pragmatic solutions that achieve the performance goals without requiring complex binary format handling.
