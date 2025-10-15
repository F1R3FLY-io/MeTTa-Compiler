# Pattern Matching Optimization: MORK/PathMap Integration Analysis

## Executive Summary

Current implementation uses inefficient iteration over all rules with O(n) complexity per match attempt. This document proposes optimizations using MORK's PathMap and query_multi for O(m) pattern matching where m is the pattern length.

## Current Implementation Analysis

### Problems Identified

#### 1. **Inefficient Rule Retrieval** (`try_match_rule` in eval.rs:576-614)

**Current approach:**
```rust
fn try_match_rule(expr: &MettaValue, env: &Environment) -> Option<(MettaValue, Bindings)> {
    // Iterates through ALL rules twice
    for rule in env.iter_rules() {  // O(n) where n = total rules
        if let Some(rule_head) = rule.lhs.get_head_symbol() {
            if &rule_head == head {
                matching_rules.push(rule);
            }
        }
    }

    // Second pass for rules without head symbols
    for rule in env.iter_rules() {
        if rule.lhs.get_head_symbol().is_none() {
            matching_rules.push(rule);
        }
    }

    // Sort and try each rule
    matching_rules.sort_by_key(|rule| pattern_specificity(&rule.lhs));
    for rule in matching_rules {
        if let Some(bindings) = pattern_match(&rule.lhs, expr) {
            return Some((rule.rhs.clone(), bindings));
        }
    }
}
```

**Issues:**
- **O(n) iteration**: Scans all rules even when only a few match the head symbol
- **Multiple passes**: Iterates twice over all rules
- **No indexing**: Does not leverage MORK's prefix-based trie structure
- **Dump + parse overhead**: `iter_rules()` dumps entire Space to strings, then parses back

#### 2. **Inefficient `iter_rules()` Implementation** (types.rs:80-126)

```rust
pub fn iter_rules(&self) -> impl Iterator<Item = Rule> {
    // Dumps ENTIRE Space to bytes
    let mut sexprs_bytes = Vec::new();
    space.dump_all_sexpr(&mut sexprs_bytes).is_err()

    // Parses EVERY line back to MettaValue
    let rules: Vec<Rule> = sexprs_str
        .lines()
        .filter_map(|line| {
            if let Ok(state) = compile(line) {
                // Parse each line as MeTTa source
                // Extract rules from parsed expressions
            }
        })
        .collect();
}
```

**Issues:**
- **O(n) dump**: Serializes all facts/rules to string format
- **O(n) parse**: Parses every rule back from MORK format
- **Memory overhead**: Creates temporary string buffer for entire Space
- **CPU waste**: Parsing is expensive, done repeatedly

#### 3. **Inefficient `has_sexpr_fact()` Implementation** (types.rs:167-199)

```rust
pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
    // Dumps entire Space
    space.dump_all_sexpr(&mut sexprs_bytes)

    // Parses every line and compares
    for line in sexprs_str.lines() {
        if let Ok(state) = compile(line) {
            for stored_value in state.pending_exprs {
                if sexpr.structurally_equivalent(&stored_value) {
                    return true;
                }
            }
        }
    }
}
```

**Issues:**
- Same O(n) dump + parse overhead
- No use of PathMap's O(m) prefix querying

## MORK/PathMap Capabilities

### Available Operations

#### 1. **query_multi** (space.rs:961)

```rust
pub fn query_multi<F>(
    btm: &PathMap<()>,
    pat_expr: Expr,
    mut effect: F
) -> usize
where F: FnMut(Result<&[u32], (BTreeMap<(u8, u8), ExprEnv>, u8, u8, &[(u8, u8)])>, Expr) -> bool
```

**Capabilities:**
- **O(m) pattern matching**: Walks trie based on pattern structure
- **Unification**: Returns bindings for variables
- **Lazy evaluation**: Callback-based, can short-circuit
- **Direct trie access**: No serialization overhead

**Usage:**
```rust
Space::query_multi(&env.space.borrow().btm, pattern_expr, |result, matched| {
    match result {
        Ok(refs) => {
            // Exact match (no variables)
            true  // Continue searching
        }
        Err((bindings, oi, ni, assignments)) => {
            // Pattern with variables matched
            // bindings: variable assignments
            false  // Stop searching
        }
    }
});
```

#### 2. **ReadZipper** (zipper.rs)

```rust
pub struct ReadZipperUntracked<'a, 'path, V, A: Allocator> {
    // Efficient trie navigation
}

// Key methods:
rz.descend_first_byte()      // Go to first child
rz.descend_to_existing(byte) // Go to specific child
rz.to_next_step()            // Sibling traversal
rz.to_next_val()             // Next value in trie
rz.ascend()                  // Go back up
```

**Capabilities:**
- **O(m) prefix navigation**: Follow path based on expression structure
- **Structural queries**: Check for existence without full parse
- **Memory efficient**: No allocations for navigation

## Proposed Optimizations

### Phase 1: Use query_multi for Pattern Matching

#### Implementation Plan

**1. Add MORK conversion for MettaValue** (types.rs)

```rust
impl MettaValue {
    /// Convert to MORK Expr for query_multi
    pub fn to_mork_expr(&self, space: &Space) -> Result<Expr, String> {
        let mork_str = self.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        // Parse using MORK's parser
        let mut temp_space = Space::new();
        temp_space.load_all_sexpr(mork_bytes)?;

        // Extract the Expr
        // ... (implementation details)
    }
}
```

**2. Replace try_match_rule with query_multi-based matching** (eval.rs)

```rust
fn try_match_rule_optimized(expr: &MettaValue, env: &Environment) -> Option<(MettaValue, Bindings)> {
    // Convert expr to MORK pattern
    let pattern_expr = match convert_to_query_pattern(expr, &env.space.borrow()) {
        Ok(p) => p,
        Err(_) => return try_match_rule_fallback(expr, env),
    };

    let space = env.space.borrow();
    let mut matches: Vec<(MettaValue, Bindings, usize)> = Vec::new();

    // Use query_multi for O(m) matching
    Space::query_multi(&space.btm, pattern_expr, |result, matched_expr| {
        if let Err((bindings, _oi, _ni, _assignments)) = result {
            // Parse matched_expr to MettaValue
            let rule = parse_matched_rule(matched_expr);

            // Convert bindings to our format
            let our_bindings = convert_bindings(bindings);

            // Compute specificity
            let specificity = pattern_specificity(&rule.lhs);

            matches.push((rule.rhs, our_bindings, specificity));
        }
        true  // Continue searching for all matches
    });

    // Sort by specificity and return best match
    matches.sort_by_key(|(_, _, spec)| *spec);
    matches.into_iter().next().map(|(rhs, bindings, _)| (rhs, bindings))
}
```

**Benefits:**
- **O(m) vs O(n)**: Pattern matching scales with pattern size, not rule count
- **No dump/parse**: Direct trie access
- **Indexed lookup**: PathMap trie structure provides natural indexing

### Phase 2: Optimize iter_rules with Zipper

#### Implementation Plan

**1. Add zipper-based rule iteration** (types.rs)

```rust
impl Environment {
    /// Iterate rules using zipper for O(n) but without parse overhead
    pub fn iter_rules_optimized(&self) -> impl Iterator<Item = Rule> {
        let space = self.space.borrow();
        let mut rz = space.btm.read_zipper();
        let mut rules = Vec::new();

        // Use zipper to walk trie
        while rz.to_next_val() {
            // Check if this is a rule (= lhs rhs)
            if is_rule_pattern(&rz) {
                // Extract rule directly from zipper position
                if let Some(rule) = extract_rule_from_zipper(&rz) {
                    rules.push(rule);
                }
            }
        }

        rules.into_iter()
    }
}
```

**Benefits:**
- **No string serialization**: Direct trie traversal
- **No parsing**: Extract structure directly
- **Lower memory**: No temporary string buffers

### Phase 3: Optimize has_sexpr_fact with Prefix Query

#### Implementation Plan

**1. Use zipper for O(m) existence checks** (types.rs)

```rust
impl Environment {
    pub fn has_sexpr_fact_optimized(&self, sexpr: &MettaValue) -> bool {
        let mork_str = sexpr.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        let space = self.space.borrow();
        let mut rz = space.btm.read_zipper();

        // Walk trie following the pattern
        for &byte in mork_bytes {
            if !rz.descend_to_existing(byte) {
                return false;  // Path doesn't exist
            }
        }

        // Check if we're at a value (fact exists)
        rz.has_val()
    }
}
```

**Benefits:**
- **O(m) complexity**: Scales with pattern size, not database size
- **Early termination**: Stops as soon as mismatch found
- **No parsing**: Direct trie navigation

## Performance Expectations

### Before Optimization

- **try_match_rule**: O(n * m) where n = rules, m = pattern size
  - Iterates all rules: O(n)
  - Dumps entire Space: O(n)
  - Parses all rules: O(n * m)
  - Pattern matches each: O(n * m)

- **iter_rules**: O(n * m)
  - Dump all: O(n)
  - Parse all: O(n * m)

- **has_sexpr_fact**: O(n * m)
  - Dump all: O(n)
  - Parse and compare all: O(n * m)

### After Optimization

- **try_match_rule_optimized**: O(k * m) where k = matching rules (k << n)
  - query_multi trie walk: O(m)
  - Process k matches: O(k * m)
  - Since k << n (only rules with matching head), major speedup

- **iter_rules_optimized**: O(n)
  - Direct zipper traversal: O(n)
  - No parsing overhead

- **has_sexpr_fact_optimized**: O(m)
  - Direct prefix query: O(m)
  - Early termination on mismatch

### Expected Speedup

For typical workloads:
- **Pattern matching**: 10-100x faster (depends on n/k ratio)
- **Rule iteration**: 5-10x faster (no parsing)
- **Fact checking**: 100-1000x faster (O(m) vs O(n*m))

## Implementation Challenges

### 1. MORK Expr Conversion

**Challenge:** Converting MettaValue to MORK Expr requires understanding MORK's internal representation.

**Solution:**
- Study MORK's parser implementation
- Create bidirectional converters
- Add comprehensive tests

### 2. De Bruijn Variable Names

**Challenge:** MORK uses De Bruijn indices, changing variable names.

**Solution:**
- Already handled by `structurally_equivalent()`
- Ensure binding conversion accounts for this
- Use structural comparison throughout

### 3. Callback-Based API

**Challenge:** query_multi uses callbacks, not iterators.

**Solution:**
- Collect results in callback
- Convert to iterator if needed
- Or redesign to use callback-based flow

## Migration Strategy

### Step 1: Parallel Implementation

- Keep existing implementations
- Add `_optimized` versions
- Compare outputs for correctness

### Step 2: Feature Flag

```rust
#[cfg(feature = "mork-optimized")]
pub use optimized::*;
#[cfg(not(feature = "mork-optimized"))]
pub use legacy::*;
```

### Step 3: Comprehensive Testing

- Run all existing tests with both versions
- Add performance benchmarks
- Verify identical semantics

### Step 4: Gradual Rollout

1. Enable for `has_sexpr_fact` first (lowest risk)
2. Enable for `iter_rules` (medium risk)
3. Enable for `try_match_rule` (highest complexity)

## Next Steps

1. **Prototype query_multi integration**
   - Create simple example
   - Understand binding format
   - Verify performance gains

2. **Implement MORK Expr converters**
   - MettaValue → Expr
   - Bindings conversion
   - Round-trip tests

3. **Benchmark current implementation**
   - Measure baseline performance
   - Identify bottlenecks
   - Set performance targets

4. **Implement Phase 1**
   - Add optimized pattern matching
   - Test correctness
   - Measure speedup

5. **Iterate through remaining phases**
   - Optimize one component at a time
   - Validate at each step
   - Document lessons learned

## References

- **MORK Space**: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/src/space.rs`
- **PathMap Zipper**: `/home/dylon/Workspace/f1r3fly.io/PathMap/src/zipper.rs`
- **Current Implementation**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/eval.rs`
- **Type System**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/types.rs`

## Conclusion

The current pattern matching implementation is functional but suboptimal. By leveraging MORK's PathMap trie structure and query_multi, we can achieve:

- **10-100x speedup** for pattern matching
- **O(m) complexity** instead of O(n*m)
- **Lower memory usage** (no dumps/parses)
- **Better scalability** as rule count grows

The optimization is worth pursuing, especially for real-world applications with large rule sets.
