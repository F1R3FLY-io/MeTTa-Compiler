# XQuery-Like Query Language for MeTTa Test Outputs

This document describes the XQuery-inspired query language for testing MeTTa s-expression outputs in Rholang integration tests.

## Overview

The query language provides path-based navigation through MeTTa s-expressions with type coercion, predicates, and filtering - similar to XQuery for XML/XPath.

## Core Concepts

### QueryResult
Results can be:
- `QueryResult::Single(value)` - One result
- `QueryResult::Multiple(vec)` - Multiple results
- `QueryResult::Empty` - No results

### PathMapQuery Trait
Main interface for querying PathMap outputs. Provides XQuery-like methods.

## Path Expressions

### Direct Queries

```rust
use common::PathMapQuery;

// Find first s-expression with specific head
let result = pathmap.query_sexpr("plan");
// Matches: (plan ...)

// Find all s-expressions with specific head
let results = pathmap.query_all_sexpr("navigate");
// Matches all: (navigate room_a), (navigate room_b), ...
```

### Descendant Queries (Recursive)

```rust
// Find first descendant at any depth
let result = pathmap.query_descendant("objective");
// Recursively searches: (plan ... (objective ...))

// Find all descendants at any depth
let results = pathmap.query_all_descendants("navigate");
// Finds all (navigate ...) expressions anywhere in the tree
```

### Path Navigation

```rust
// Navigate through nested s-expressions
let result = pathmap.query_path(&["plan", "route", "waypoints"]);
// Navigates: (plan ... (route ... (waypoints ...)))
```

## Filtering and Predicates

### Text-Based Filtering

```rust
// Filter outputs containing specific text
let results = pathmap.filter_contains("ball1");
// Finds all outputs mentioning "ball1" (recursive text search)
```

### Predicate Filtering

```rust
// Filter with custom predicate
let results = pathmap.query_outputs_where(|v| {
    matches!(v, MettaValue::Long(n) if *n > 0)
});

// Check if any result matches predicate
let has_match = results.exists(|v| {
    v.to_display_string().contains("multihop")
});
```

## Type Extraction

### From QueryResult

```rust
let result = pathmap.query_sexpr("plan");

// Extract typed values
let atom = result.as_atom();       // Option<String>
let string = result.as_string();   // Option<String>
let i64_val = result.as_i64();     // Option<i64>
let bool_val = result.as_bool();   // Option<bool>
let sexpr = result.as_sexpr();     // Option<Vec<MettaValue>>
```

### From PathMapQuery

```rust
// Direct type coercion from output index
let num = pathmap.output_as_i64(0);      // Option<i64>
let flag = pathmap.output_as_bool(1);    // Option<bool>
let text = pathmap.output_as_string(2);  // Option<String>
```

## Aggregation

```rust
// Count results
let count = results.count();  // usize

// Check if empty
if results.is_empty() { /* ... */ }

// Convert to vector
let vec = results.as_vec();  // Vec<MettaValue>
```

## Complete Example: Robot Planning Test

```rust
use common::{PathMapQuery, OutputMatcher};

// Find paths from room_c to room_a
let demo3_paths = pathmaps.iter().find(|pm| {
    let path_results = pm.query_all_sexpr("path");
    !path_results.is_empty() && path_results.exists(|v| {
        let s = v.to_display_string();
        s.contains("room_c") && s.contains("room_b") && s.contains("room_a")
    })
});

// Find outputs mentioning ball1 with plans
let demo4_plan = pathmaps.iter().find(|pm| {
    let ball1_outputs = pm.filter_contains("ball1");
    !ball1_outputs.is_empty() && ball1_outputs.exists(|v| {
        let s = v.to_display_string();
        s.contains("build_plan") && s.contains("plan")
    })
});

// Find path_hop_count at any depth with specific values
let demo5_hop_counts = pathmaps.iter().find(|pm| {
    let hop_count_expr = pm.query_descendant("path_hop_count");
    !hop_count_expr.is_empty() && pm.output.iter().any(|v| {
        matches!(v, MettaValue::Long(2) | MettaValue::Long(3))
    })
});

// Exact structure matching with OutputMatcher
let ball1_steps = vec![
    vec!["navigate", "room_c"],
    vec!["pickup", "ball1"],
    vec!["navigate", "room_b"],
    vec!["navigate", "room_a"],
    vec!["putdown"],
];

let has_ball1_steps = pathmaps.iter().any(|pm| {
    OutputMatcher::new(pm).match_steps_sequence(&ball1_steps)
});
```

## XQuery Comparison

| XQuery/XPath | MeTTa Query Language |
|--------------|---------------------|
| `/plan` | `query_sexpr("plan")` |
| `//objective` | `query_descendant("objective")` |
| `/plan/route/waypoints` | `query_path(&["plan", "route", "waypoints"])` |
| `//navigate` | `query_all_descendants("navigate")` |
| `contains(., "ball1")` | `filter_contains("ball1")` |
| `count(//plan)` | `query_all_descendants("plan").count()` |
| `exists(//error)` | `query_descendant("error").exists(\|_\| true)` |

## API Reference

### PathMapQuery Methods

```rust
// Direct queries
fn query_output(&self, index: usize) -> QueryResult
fn query_all_outputs(&self) -> QueryResult
fn query_outputs_where<F>(&self, predicate: F) -> QueryResult

// XQuery-like path expressions
fn query_sexpr(&self, head: &str) -> QueryResult
fn query_all_sexpr(&self, head: &str) -> QueryResult
fn query_descendant(&self, head: &str) -> QueryResult
fn query_all_descendants(&self, head: &str) -> QueryResult
fn query_path(&self, path: &[&str]) -> QueryResult
fn filter_contains(&self, text: &str) -> QueryResult

// Type extraction
fn output_as_i64(&self, index: usize) -> Option<i64>
fn output_as_bool(&self, index: usize) -> Option<bool>
fn output_as_string(&self, index: usize) -> Option<String>
fn outputs_as_i64_seq(&self) -> Vec<i64>
```

### QueryResult Methods

```rust
// Type extraction
fn as_atom(&self) -> Option<String>
fn as_string(&self) -> Option<String>
fn as_i64(&self) -> Option<i64>
fn as_bool(&self) -> Option<bool>
fn as_sexpr(&self) -> Option<Vec<MettaValue>>

// Filtering and predicates
fn filter<F>(&self, predicate: F) -> QueryResult
fn exists<F>(&self, predicate: F) -> bool
fn count(&self) -> usize
fn is_empty(&self) -> bool

// Conversion
fn as_single(&self) -> Option<&MettaValue>
fn as_vec(&self) -> Vec<MettaValue>
```

## Best Practices

1. **Use specific queries**: Prefer `query_sexpr("plan")` over string matching
2. **Handle multiple results**: Use `query_all_*` when expecting multiple matches
3. **Type safety**: Extract types with `as_i64()`, `as_bool()`, etc.
4. **Composition**: Chain queries with `filter()` and `exists()`
5. **Deep searches**: Use `query_descendant()` for nested structures

## See Also

- `tests/common/query.rs` - Implementation
- `tests/rholang_integration.rs` - Usage examples
- `tests/common/output_parser.rs` - S-expression parsing
