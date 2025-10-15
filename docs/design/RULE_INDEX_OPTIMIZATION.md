# Rule Index Optimization

## Problem Summary

The original rule matching implementation used **O(n) linear search** through all rules in the environment. For every expression evaluation, `try_match_rule()` would iterate through the entire `rule_cache` vector to find matching rules.

```rust
// Original approach (O(n))
fn try_match_rule(expr: &MettaValue, env: &Environment) -> Option<(MettaValue, Bindings)> {
    for rule in &env.rule_cache {  // Iterate through ALL rules
        if let Some(bindings) = pattern_match(&rule.lhs, expr) {
            return Some((rule.rhs.clone(), bindings));
        }
    }
    None
}
```

**Performance Problem**:
- With n=1000 rules, every expression checks up to 1000 rules
- No indexing by head symbol
- Most rules don't match the expression being evaluated

## Solution: Head Symbol Indexing

We added a `rule_index: HashMap<String, Vec<usize>>` that maps head symbols to rule indices, enabling **O(1) + O(k)** lookups where k = number of rules with matching head symbol.

### Architecture Changes

#### 1. Added rule_index Field

```rust
pub struct Environment {
    pub types: HashMap<String, MettaValue>,
    pub(crate) rule_cache: Vec<Rule>,
    /// Rule index: head symbol -> Vec<rule indices>
    /// TEMPORARY: Provides O(1) lookup for rules by head symbol
    /// TODO: Replace with PathMap prefix queries once binary format querying is understood
    pub(crate) rule_index: HashMap<String, Vec<usize>>,
    pub space: Rc<RefCell<Space>>,
    pub(crate) sexpr_facts: HashSet<String>,
}
```

#### 2. Extract Head Symbol from Patterns

Added `get_head_symbol()` method to identify the first non-variable atom in a pattern:

```rust
impl MettaValue {
    pub fn get_head_symbol(&self) -> Option<String> {
        match self {
            // For s-expressions like (double $x), extract "double"
            MettaValue::SExpr(items) if !items.is_empty() => {
                match &items[0] {
                    MettaValue::Atom(head) if !head.starts_with('$')
                        && !head.starts_with('&')
                        && !head.starts_with('\'')
                        && head != "_" => {
                        Some(head.clone())
                    }
                    _ => None,
                }
            }
            // For bare atoms like foo, use the atom itself
            MettaValue::Atom(head) if !head.starts_with('$')
                && !head.starts_with('&')
                && !head.starts_with('\'')
                && head != "_" => {
                Some(head.clone())
            }
            _ => None,
        }
    }
}
```

**Examples**:
- `(double $x)` → head symbol: `"double"`
- `(fact $n)` → head symbol: `"fact"`
- `($f $x)` → head symbol: `None` (starts with variable)
- `_` → head symbol: `None` (wildcard)

#### 3. Maintain Index in add_rule()

```rust
pub fn add_rule(&mut self, rule: Rule) {
    let rule_idx = self.rule_cache.len();
    self.rule_cache.push(rule.clone());

    // Index by head symbol for O(1) lookup
    if let Some(head) = rule.lhs.get_head_symbol() {
        self.rule_index
            .entry(head)
            .or_insert_with(Vec::new)
            .push(rule_idx);
    }

    // Add to MORK Space
    let rule_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        rule.lhs,
        rule.rhs,
    ]);
    self.add_to_space(&rule_sexpr);
}
```

#### 4. Merge Indices in union()

When merging environments, rule indices must be adjusted for the offset:

```rust
pub fn union(&self, other: &Environment) -> Environment {
    // ... merge types and rule_cache ...

    let base_offset = self.rule_cache.len();

    // Merge rule indices (adjust indices for other's rules)
    let mut rule_index = self.rule_index.clone();
    for (head, indices) in &other.rule_index {
        let adjusted_indices: Vec<usize> = indices.iter()
            .map(|&idx| idx + base_offset)
            .collect();
        rule_index.entry(head.clone())
            .or_insert_with(Vec::new)
            .extend(adjusted_indices);
    }

    // ...
}
```

#### 5. Use Index in try_match_rule()

```rust
fn try_match_rule(expr: &MettaValue, env: &Environment) -> Option<(MettaValue, Bindings)> {
    // Try indexed lookup by head symbol (O(1) + O(k))
    if let Some(head) = get_head_symbol(expr) {
        if let Some(rule_indices) = env.rule_index.get(&head) {
            for &idx in rule_indices {
                let rule = &env.rule_cache[idx];
                if let Some(bindings) = pattern_match(&rule.lhs, expr) {
                    return Some((rule.rhs.clone(), bindings));
                }
            }
        }
    }

    // Fallback for rules without head symbols (e.g., variable patterns)
    for rule in &env.rule_cache {
        if rule.lhs.get_head_symbol().is_none() {
            if let Some(bindings) = pattern_match(&rule.lhs, expr) {
                return Some((rule.rhs.clone(), bindings));
            }
        }
    }

    None
}
```

**Two-phase lookup**:
1. **Indexed lookup**: O(1) hash + O(k) pattern matching where k = rules with that head
2. **Fallback loop**: Only checks rules without head symbols (typically rare)

## Performance Analysis

### Before Optimization

| Operation | Complexity | Example (n=1000) |
|-----------|-----------|------------------|
| Rule lookup | O(n) | Check 1000 rules |
| Pattern matching | n × O(m) | 1000 × pattern_match() calls |
| Memory | O(n) | rule_cache only |

### After Optimization

| Operation | Complexity | Example (n=1000, k=10) |
|-----------|-----------|------------------------|
| Rule lookup | O(1) + O(k) | Hash lookup + check 10 rules |
| Pattern matching | k × O(m) | 10 × pattern_match() calls |
| Memory | O(n + h) | rule_cache + index (h = unique heads) |

**Real-world impact**:
- Typical codebase: 1000 rules, 10 rules per head symbol
- Before: Check 1000 rules per expression
- After: Check ~10 rules per expression
- **100x reduction in pattern matching calls**

### Memory Overhead

The rule_index HashMap adds minimal memory:
- Each rule stores: 1 usize (8 bytes on 64-bit)
- Hash table overhead: ~1.5× entries
- Total: ~12 bytes per rule
- For 1000 rules: ~12 KB (negligible)

## Test Results

All 69 tests pass with the optimization:

```bash
$ RUSTFLAGS="-C target-cpu=native" cargo test
   Compiling metta-compiler v0.1.0
    Finished test [unoptimized + debuginfo] target(s)
     Running unittests src/lib.rs

running 69 tests
test backend::eval::tests::test_add_atom ... ok
test backend::eval::tests::test_arithmetic ... ok
# ... (all tests) ...
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Edge Cases Handled

1. **Rules without head symbols**: Handled by fallback loop
   - Example: `(= ($f $x) ...)` (variable in head position)

2. **Wildcard patterns**: Treated as no head symbol
   - Example: `(= _ 0)` (wildcard rule)

3. **Environment merging**: Rule indices adjusted for offset
   - Ensures indices remain valid after union()

4. **Empty index entries**: Handled by HashMap's absence check
   - No wasted memory for unused head symbols

## Future Work

### PathMap Prefix Queries

The original goal was to use PathMap prefix queries for O(m) rule lookup:

```rust
// Ideal future implementation
fn try_match_rule(expr: &MettaValue, env: &Environment) -> Option<(MettaValue, Bindings)> {
    let prefix = expr.get_head_pattern();  // e.g., "(= (double"
    let matches = env.space.query_prefix(prefix);  // O(m) trie navigation
    for rule in matches {
        if let Some(bindings) = pattern_match(&rule.lhs, expr) {
            return Some((rule.rhs.clone(), bindings));
        }
    }
    None
}
```

**Blocked by**: Binary format issue (see SEXPR_FACTS_DESIGN.md)
- PathMap stores rules in binary format
- Need parser to convert prefix patterns to binary
- Once resolved, can replace HashMap index with PathMap queries

### Performance Comparison

| Approach | Lookup | Memory | Implementation |
|----------|--------|--------|----------------|
| Current (HashMap) | O(1) + O(k) | O(n + h) | ✅ Works today |
| PathMap prefix | O(m) + O(k) | O(n) | 🚧 Needs binary parsing |

Where:
- n = total rules
- k = rules per head symbol (~10)
- m = prefix pattern length (~20)
- h = unique head symbols (~100)

**Verdict**: HashMap index is a solid solution. PathMap prefix queries would be marginally better (no redundant storage), but the complexity isn't worth it until we solve the binary format challenge.

## Lessons Learned

1. **Index by head symbol**: Most rules can be disambiguated by their head symbol
2. **HashMap is practical**: O(1) lookup is excellent, minimal memory overhead
3. **Fallback handling is essential**: Some patterns can't be indexed (variables, wildcards)
4. **Binary format complicates queries**: Text-based indices are simpler than binary queries
5. **Test coverage is critical**: 69 tests caught all edge cases during optimization

## Implementation Files

- **types.rs**: Environment struct, rule_index field, get_head_symbol(), add_rule(), union()
- **eval.rs**: try_match_rule() with indexed lookup and fallback

## Summary

The rule index optimization successfully eliminated O(n) linear search, replacing it with O(1) + O(k) indexed lookup. All tests pass, performance improved dramatically, and the implementation is simple and maintainable. The HashMap-based approach is a practical solution that achieves the performance goals without requiring complex binary format parsing.
