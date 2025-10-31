# Collection-Aware Parser and Query System

## Overview

The PathMap parser has been enhanced with collection type awareness and an XQuery-inspired query system for integration testing. This allows tests to:

1. **Parse collection types** with proper semantics (sets, lists, tuples, maps)
2. **Query outputs** using path-based queries
3. **Match values** with automatic type coercion
4. **Assert sequences** maintaining order and types

## Collection Types

### Supported Collections

The parser recognizes and handles the following collection types:

- **Sets**: `{|a, b, c|}` - Par sets from Rholang
- **Lists**: `[a, b, c]` - Ordered sequences
- **Tuples/S-expressions**: `(a, b, c)` - Recursively parsed into `MettaValue::SExpr`
- **Maps**: `{k1: v1, k2: v2}` - Key-value pairs (planned)

### CollectionValue Type

```rust
pub enum CollectionValue {
    Set(Vec<MettaValue>),           // {|a, b, c|}
    List(Vec<MettaValue>),          // [a, b, c]
    Tuple(Vec<MettaValue>),         // (a, b, c)
    Map(Vec<(MettaValue, MettaValue)>),  // {k: v}
    Single(MettaValue),             // Individual value
}
```

## Literal Parsing

The parser uses direct nom combinators to parse literals into typed `MettaValue` variants:

```rust
// Integers → MettaValue::Long
parse_integer("[1, 2, 3]") → [Long(1), Long(2), Long(3)]

// Booleans → MettaValue::Bool
parse_boolean("true") → Bool(true)

// Strings → MettaValue::String
parse_string_literal("\"hello\"") → String("hello")

// Tuples → MettaValue::SExpr (recursive)
parse_tuple("(1, (2, 3))") → SExpr([Long(1), SExpr([Long(2), Long(3)])])
```

## Query System

### PathMapQuery Trait

The `PathMapQuery` trait provides XQuery-inspired methods for querying PathMap outputs:

```rust
use common::{PathMapQuery, OutputMatcher};

// Query specific output by index
let result = pathmap.query_output(0);  // Get first output

// Query all outputs
let all = pathmap.query_all_outputs();

// Query with predicate
let longs = pathmap.query_outputs_where(|v| matches!(v, MettaValue::Long(_)));

// Type-specific queries with coercion
let value: Option<i64> = pathmap.output_as_i64(0);
let value: Option<bool> = pathmap.output_as_bool(1);
let value: Option<String> = pathmap.output_as_string(2);

// Sequence queries
let seq: Vec<i64> = pathmap.outputs_as_i64_seq();
```

### QueryResult Type

Query results are wrapped in a `QueryResult` type:

```rust
pub enum QueryResult {
    Single(MettaValue),      // One result
    Multiple(Vec<MettaValue>), // Multiple results
    Empty,                   // No results
}

// Convert to common types
let value: Option<&MettaValue> = result.as_single();
let values: Vec<MettaValue> = result.as_vec();
let count: usize = result.len();
let is_empty: bool = result.is_empty();
```

## Matcher System

### OutputMatcher

The `OutputMatcher` provides type-coercing assertions:

```rust
use common::{OutputMatcher, ToMettaValue};

let matcher = OutputMatcher::new(&pathmap);

// Assert specific output with type coercion
assert!(matcher.assert_output_eq(0, 42i64));
assert!(matcher.assert_output_eq(1, true));
assert!(matcher.assert_output_eq(2, "hello"));

// Assert sequence (order matters)
assert!(matcher.assert_outputs_eq(&[1i64, 2i64, 3i64]));

// Assert contains (order doesn't matter)
assert!(matcher.assert_outputs_contain(42i64));
assert!(matcher.assert_outputs_contain_all(&[1i64, 2i64, 3i64]));

// Get count
assert_eq!(matcher.output_count(), 3);
```

### Type Coercion

The `ToMettaValue` trait automatically converts Rust types to `MettaValue`:

```rust
// Automatic conversions:
42i64    → MettaValue::Long(42)
42i32    → MettaValue::Long(42)
true     → MettaValue::Bool(true)
"hello"  → MettaValue::String("hello".to_string())

// Works with test assertions:
assert!(matcher.assert_output_eq(0, 42));  // i32 auto-converts to Long
```

## Integration Test Examples

### Example 1: Query-Based Test

```rust
#[test]
fn test_arithmetic_with_queries() {
    let (success, stdout, stderr) = run_rho_test("test_arithmetic.rho");
    let pathmaps = parse_pathmap(&stdout);

    assert!(!pathmaps.is_empty(), "No PathMap structures found");
    let pathmap = &pathmaps[0];

    // Query specific outputs by index
    assert_eq!(pathmap.output_as_i64(0), Some(3));   // (+ 1 2) → 3
    assert_eq!(pathmap.output_as_i64(1), Some(12));  // (* 3 4) → 12
    assert_eq!(pathmap.output_as_i64(2), Some(5));   // (- 10 5) → 5
}
```

### Example 2: Matcher-Based Test

```rust
#[test]
fn test_boolean_operations() {
    let (success, stdout, stderr) = run_rho_test("test_bool.rho");
    let pathmaps = parse_pathmap(&stdout);
    let pathmap = &pathmaps[0];

    let matcher = OutputMatcher::new(pathmap);

    // Assert specific positions
    assert!(matcher.assert_output_eq(0, true));   // (< 1 2) → true
    assert!(matcher.assert_output_eq(1, false));  // (< 3 2) → false

    // Assert entire sequence (maintains order)
    assert!(matcher.assert_outputs_eq(&[true, false, true]));
}
```

### Example 3: Sequence Assertion

```rust
#[test]
fn test_sequence_evaluation() {
    let (success, stdout, stderr) = run_rho_test("test_seq.rho");
    let pathmaps = parse_pathmap(&stdout);
    let pathmap = &pathmaps[0];

    // Assert outputs match sequence exactly (order and types)
    assert!(pathmap.outputs_match_sequence(&[3i64, 12i64, 5i64]));

    // Alternative: use matcher
    let matcher = OutputMatcher::new(pathmap);
    assert!(matcher.assert_outputs_eq(&[3, 12, 5]));  // i32 auto-converts
}
```

### Example 4: Flexible Matching

```rust
#[test]
fn test_rule_outputs() {
    let (success, stdout, stderr) = run_rho_test("test_rules.rho");
    let pathmaps = parse_pathmap(&stdout);
    let pathmap = &pathmaps[0];

    let matcher = OutputMatcher::new(pathmap);

    // Check that specific values exist (order doesn't matter)
    assert!(matcher.assert_outputs_contain(10i64));
    assert!(matcher.assert_outputs_contain(15i64));

    // Check all expected values present
    assert!(matcher.assert_outputs_contain_all(&[3, 10, 15]));

    // Check count
    assert_eq!(matcher.output_count(), 3);
}
```

## Migration Guide

### Old Style (Flat Map)

```rust
// ❌ Old approach: flat map over all outputs
let all_outputs: Vec<_> = pathmaps.iter()
    .flat_map(|pm| pm.output.clone())
    .collect();

for expected in &["3", "12", "5"] {
    assert!(
        all_outputs.iter().any(|v| v.matches_str(expected)),
        "Expected output '{}' not found",
        expected
    );
}
```

### New Style (Sequence with Types)

```rust
// ✅ New approach: query and match with types
use common::{PathMapQuery, OutputMatcher};

let pathmap = &pathmaps[0];
let matcher = OutputMatcher::new(pathmap);

// Assert exact sequence (maintains order)
assert!(matcher.assert_outputs_eq(&[3i64, 12i64, 5i64]));

// Or query specific positions
assert_eq!(pathmap.output_as_i64(0), Some(3));
assert_eq!(pathmap.output_as_i64(1), Some(12));
assert_eq!(pathmap.output_as_i64(2), Some(5));

// Or check containment (order-independent)
assert!(matcher.assert_outputs_contain_all(&[3, 12, 5]));
```

## Benefits

1. **Type Safety**: Literal types are preserved (`Long`, `Bool`, `String`)
2. **Order Awareness**: Sequence assertions maintain output order
3. **Collection Semantics**: Sets, lists, tuples, maps have proper handling
4. **Type Coercion**: Automatic conversion from Rust types to `MettaValue`
5. **Flexible Queries**: XQuery-inspired path-based queries
6. **Better Errors**: Type mismatches caught at compile time

## Technical Details

### Recursive Tuple Parsing

Tuples/S-expressions are parsed recursively:

```rust
fn parse_tuple(input: &str) -> IResult<&str, MettaValue> {
    map(
        delimited(
            char('('),
            separated_list0(ws(char(',')), ws(parse_metta_value_recursive)),
            char(')')
        ),
        |elements| {
            if elements.is_empty() {
                MettaValue::Nil
            } else {
                MettaValue::SExpr(elements)
            }
        }
    )(input)
}
```

This allows nested structures like `(1, (2, 3), ((4, 5), 6))` to be fully parsed.

### Direct Literal Parsing

All literals are parsed directly into typed `MettaValue` variants using nom combinators:

```rust
// Integer parsing
fn parse_integer(input: &str) -> IResult<&str, MettaValue> {
    map_res(
        recognize(pair(opt(char('-')), digit1)),
        |s: &str| s.parse::<i64>().map(MettaValue::Long)
    )(input)
}

// Boolean parsing
fn parse_boolean(input: &str) -> IResult<&str, MettaValue> {
    alt((
        value(MettaValue::Bool(true), tag("true")),
        value(MettaValue::Bool(false), tag("false")),
    ))(input)
}
```

No string post-processing is performed.
