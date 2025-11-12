# MeTTaTron PathMap Usage Patterns and Best Practices

**Date**: 2025-01-10
**Related**: `metta_pathmap_optimization_proposal.md`, Rholang LSP `docs/architecture/mork_pathmap_integration.md`

## Table of Contents

1. [PathMap Indexing Strategies](#indexing-strategies)
2. [WriteZipper vs ReadZipper](#zipper-apis)
3. [Thread Safety Considerations](#thread-safety)
4. [Common Patterns](#common-patterns)
5. [Performance Trade-offs](#performance)
6. [Comparison with HashMap](#comparison)
7. [Rholang LSP Learnings](#rholang-learnings)

---

<a name="indexing-strategies"></a>
## PathMap Indexing Strategies for MeTTaTron

### Strategy 1: Head Symbol + Arity Indexing (Rules)

**Use Case**: Fast rule matching by function name and argument count

**Path Structure**:
```
["rule", <head_symbol_bytes>, <arity_bytes>] → Vec<Rule>
```

**Example**:
```rust
// Index this rule: (fibonacci $n)
let rule = Rule {
    lhs: MettaValue::SExpr(vec![
        MettaValue::Atom("fibonacci".to_string()),
        MettaValue::Atom("$n".to_string()),
    ]),
    // ...
};

// Path: ["rule", "fibonacci", "1"]
let path = vec![
    b"rule".to_vec(),
    b"fibonacci".to_vec(),
    b"1".to_vec(),  // arity
];

// Insert
let mut wz = pathmap.write_zipper();
wz.descend_to(b"rule");
wz.descend_to(b"fibonacci");
wz.descend_to(b"1");
wz.set_val(vec![rule]);
```

**Query**:
```rust
// Find all rules for (fibonacci ...)
let mut rz = pathmap.read_zipper();
if rz.descend_to_check(b"rule")
    && rz.descend_to_check(b"fibonacci")
    && rz.descend_to_check(b"1") {
    if let Some(rules) = rz.val() {
        // Found matching rules!
    }
}
```

**Benefits**:
- ✅ O(3) lookup vs O(n) scan
- ✅ Prefix sharing (all "fibonacci" rules share path prefix)
- ✅ Easy to extend (add more path levels for refinement)

---

### Strategy 2: Type Assertion Head Symbol (Facts)

**Use Case**: Fast fact lookup by head symbol (e.g., all type assertions)

**Path Structure**:
```
[<head_symbol_bytes>, <rest_of_path>] → MorkExpr
```

**Example**:
```rust
// Index fact: (: foo Int)
// Path starts with head symbol ":"
let path = vec![
    b":".to_vec(),
    b"foo".to_vec(),
];

// Space already stores facts this way!
// No additional indexing needed for prefix navigation
```

**Query (has_sexpr_fact with prefix)**:
```rust
// Find all type assertions (head = ":")
let mut rz = space.btm.read_zipper();
if rz.descend_to_check(b":") {
    // Only iterate facts in ": " subtree
    while rz.to_next_val() {
        // Check if this is the fact we want
    }
}
```

**Benefits**:
- ✅ Filter facts by category (type assertions vs rules vs others)
- ✅ Leverages existing Space structure
- ✅ No secondary index needed

---

### Strategy 3: Wildcard Path for Patterns

**Use Case**: Store rules/facts with variable heads separately

**Path Structure**:
```
["rule", "_wildcard_"] → Vec<Rule>  // Rules with variable heads
```

**Example**:
```rust
// Rule with variable head: ($op $a $b)
let rule = Rule {
    lhs: MettaValue::SExpr(vec![
        MettaValue::Atom("$op".to_string()),  // Variable!
        // ...
    ]),
    // ...
};

// Store at special "_wildcard_" path
let mut wz = pathmap.write_zipper();
wz.descend_to(b"rule");
wz.descend_to(b"_wildcard_");  // Special marker
wz.set_val(vec![rule]);
```

**Query (always include wildcards)**:
```rust
// Query for specific + wildcard rules
let mut results = Vec::new();

// 1. Get specific rules
let mut rz = pathmap.read_zipper();
if rz.descend_to_check(b"rule") && rz.descend_to_check(b"fibonacci") {
    if let Some(specific_rules) = rz.val() {
        results.extend_from_slice(specific_rules);
    }
}

// 2. Always get wildcard rules
let mut rz = pathmap.read_zipper();
if rz.descend_to_check(b"rule") && rz.descend_to_check(b"_wildcard_") {
    if let Some(wildcard_rules) = rz.val() {
        results.extend_from_slice(wildcard_rules);
    }
}
```

**Benefits**:
- ✅ Separates fixed-head from variable-head rules
- ✅ Ensures pattern rules always checked
- ✅ Maintains correct semantics

---

<a name="zipper-apis"></a>
## WriteZipper vs ReadZipper

### WriteZipper API (Mutation)

**Use for**: Inserting, updating, deleting values

**Key Methods**:
```rust
pub struct WriteZipper<'a, V> {
    // Navigate down the trie
    pub fn descend_to(&mut self, key: &[u8]) -> &mut Self;

    // Set value at current position
    pub fn set_val(&mut self, val: V);

    // Delete value at current position
    pub fn delete_val(&mut self);

    // Get current value (for read-modify-write)
    pub fn val(&self) -> Option<&V>;
}
```

**Example - Insert**:
```rust
let mut wz = pathmap.write_zipper();

// Navigate to path
wz.descend_to(b"rule");
wz.descend_to(b"fibonacci");
wz.descend_to(b"1");

// Insert value
wz.set_val(vec![rule]);
```

**Example - Update (Read-Modify-Write)**:
```rust
let mut wz = pathmap.write_zipper();
wz.descend_to(b"rule");
wz.descend_to(b"fibonacci");
wz.descend_to(b"1");

// Get existing value
let mut rules = wz.val().cloned().unwrap_or_default();

// Modify
rules.push(new_rule);

// Write back
wz.set_val(rules);
```

**Important**: `descend_to()` **creates** path if it doesn't exist!
- No need to check if path exists
- Safe to navigate unconditionally

---

### ReadZipper API (Querying)

**Use for**: Querying, iterating values

**Key Methods**:
```rust
pub struct ReadZipper<'a, V> {
    // Navigate down (returns bool: path exists?)
    pub fn descend_to_check(&mut self, key: &[u8]) -> bool;

    // Get value at current position
    pub fn val(&self) -> Option<&V>;

    // Iterate to next value in trie
    pub fn to_next_val(&mut self) -> bool;

    // Navigate up one level
    pub fn ascend(&mut self) -> bool;
}
```

**Example - Query**:
```rust
let mut rz = pathmap.read_zipper();

// Check if path exists
if rz.descend_to_check(b"rule")
    && rz.descend_to_check(b"fibonacci")
    && rz.descend_to_check(b"1") {

    // Path exists, get value
    if let Some(rules) = rz.val() {
        // Use rules
    }
} else {
    // Path doesn't exist
}
```

**Example - Iterate Subtree**:
```rust
let mut rz = pathmap.read_zipper();

// Navigate to subtree root
if rz.descend_to_check(b":") {
    // Iterate all values in this subtree
    while rz.to_next_val() {
        if let Some(fact) = rz.val() {
            // Process each fact
        }
    }
}
```

**Important**: `descend_to_check()` **does NOT create** path!
- Returns `false` if path doesn't exist
- Must check return value before calling `val()`

---

<a name="thread-safety"></a>
## Thread Safety Considerations

### MORK Space is NOT Send + Sync

**Problem**: `Space` contains `Cell<u64>` internally
- `Cell` is not `Send + Sync`
- Cannot share `Space` across threads directly

**Solution**: Wrap in `Arc<Mutex<>>`

```rust
pub struct Environment {
    // CORRECT: Thread-safe
    pub space: Arc<Mutex<Space>>,
}
```

**Why this works**:
- `Mutex` provides exclusive access (no concurrent writes)
- `Arc` allows shared ownership
- Only one thread can hold lock at a time

---

### PathMap IS Thread-Safe (with Mutex)

**PathMap itself**:
- Immutable data structure (persistent trie)
- Safe to share across threads
- But mutation requires exclusive access

**Recommendation**: Wrap in `Arc<Mutex<>>` for consistency

```rust
pub struct Environment {
    pub space: Arc<Mutex<Space>>,

    // NEW indexes - also wrapped for consistency
    rule_index: Arc<Mutex<PathMap<Vec<Rule>>>>,
    type_index: Arc<Mutex<HashMap<String, MettaValue>>>,
}
```

---

### Locking Strategy

**Read-Heavy Workloads** (typical for queries):

```rust
// Good: Hold lock briefly
pub fn query_rules_by_head(&self, head: &str, arity: usize) -> Vec<Rule> {
    let index = self.rule_index.lock().unwrap();  // Acquire
    let mut rz = index.read_zipper();

    // Query quickly
    let results = if rz.descend_to_check(b"rule") {
        rz.val().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };

    drop(index);  // Release (implicit)
    results
}
```

**Write Operations** (rare):

```rust
// Good: Batch writes
pub fn add_multiple_rules(&mut self, rules: Vec<Rule>) {
    let mut index = self.rule_index.lock().unwrap();  // Acquire once

    for rule in rules {
        let mut wz = index.write_zipper();
        // ... insert rule ...
    }

    drop(index);  // Release once
}
```

**Bad Practice**:
```rust
// BAD: Lock per operation in loop
for rule in rules {
    let mut index = self.rule_index.lock().unwrap();  // Lock N times!
    // ... insert ...
    drop(index);
}
```

---

<a name="common-patterns"></a>
## Common Patterns

### Pattern 1: Prefix Navigation + Iteration

**Use Case**: Find all facts with specific head symbol

```rust
pub fn find_all_type_assertions(&self) -> Vec<(String, MettaValue)> {
    let space = self.space.lock().unwrap();
    let mut rz = space.btm.read_zipper();
    let mut results = Vec::new();

    // Navigate to type assertions (head = ":")
    if rz.descend_to_check(b":") {
        // Iterate all facts in this subtree
        while rz.to_next_val() {
            if let Some(fact) = rz.val() {
                if let Ok(metta_val) = mork_to_metta(fact, &space) {
                    // Extract atom and type from (: atom type)
                    if let MettaValue::SExpr(items) = metta_val {
                        if items.len() == 3 {
                            results.push((/* atom */, /* type */));
                        }
                    }
                }
            }
        }
    }

    results
}
```

---

### Pattern 2: Multi-Level Indexing

**Use Case**: Index by multiple criteria (head + arity + parameters)

```rust
// Path: ["rule", <head>, <arity>, <param0_pattern>, ...]
pub fn index_rule_with_patterns(&mut self, rule: &Rule) {
    let mut wz = self.rule_index.lock().unwrap().write_zipper();

    // Level 1: "rule"
    wz.descend_to(b"rule");

    // Level 2: Head symbol
    let head = extract_head_symbol(&rule.lhs);
    wz.descend_to(head.as_bytes());

    // Level 3: Arity
    let arity = extract_arity(&rule.lhs);
    wz.descend_to(arity.to_string().as_bytes());

    // Level 4: Parameter patterns (MORK bytes)
    for param in extract_parameters(&rule.lhs) {
        let param_bytes = param_to_mork_bytes(&param);
        wz.descend_to(&param_bytes);
    }

    // Insert rule
    wz.set_val(vec![rule.clone()]);
}
```

**Benefits**:
- ✅ Very specific queries (exact match on all levels)
- ✅ Graceful degradation (query fewer levels for broader matches)

**Trade-off**:
- ❌ Deeper trie = more memory
- ❌ More complex indexing logic

---

### Pattern 3: Read-Modify-Write for Appending

**Use Case**: Add rule to existing Vec at path

```rust
pub fn add_rule_to_index(&mut self, rule: Rule, path: &[Vec<u8>]) {
    let mut wz = self.rule_index.lock().unwrap().write_zipper();

    // Navigate to path
    for segment in path {
        wz.descend_to(segment);
    }

    // Read existing value
    let mut rules = wz.val().cloned().unwrap_or_default();

    // Modify
    rules.push(rule);

    // Write back
    wz.set_val(rules);
}
```

**Important**: Always use `unwrap_or_default()` to handle missing path!
- First insert: `val()` returns `None` → use empty Vec
- Subsequent inserts: `val()` returns existing Vec → append

---

### Pattern 4: Graceful Fallback

**Use Case**: Try optimized path first, fall back to full scan

```rust
pub fn find_matching_rules(&self, expr: &MettaValue) -> Vec<Rule> {
    // Try indexed lookup first
    if let Some(head) = extract_head_symbol(expr) {
        let arity = extract_arity(expr);
        let indexed_results = self.query_rules_by_head(&head, arity);

        if !indexed_results.is_empty() {
            return indexed_results;  // Fast path!
        }
    }

    // Fallback: Full scan (for complex patterns)
    self.iter_all_rules().collect()
}
```

**Benefits**:
- ✅ Fast for common cases (fixed head)
- ✅ Correct for all cases (fallback)
- ✅ No special cases needed

---

<a name="performance"></a>
## Performance Trade-offs

### PathMap vs Linear Scan

**PathMap Prefix Navigation**:
```
Complexity: O(k) where k = path depth
Example: ["rule", "fibonacci", "1"] → O(3)

Pros:
✅ Constant-time navigation (doesn't depend on total entries)
✅ Scales with path depth, not data size
✅ Prefix sharing saves memory

Cons:
❌ Setup cost: O(n) to build index
❌ Memory overhead for trie structure
```

**Linear Scan**:
```
Complexity: O(n) where n = total entries
Example: Iterate 1000 rules → O(1000)

Pros:
✅ No setup cost
✅ No extra memory
✅ Simple implementation

Cons:
❌ Scales linearly with data size
❌ Slow for large datasets
```

**When to Use PathMap**:
- ✅ Data size > 100 entries
- ✅ Frequent queries (amortize setup cost)
- ✅ Prefix-based categorization

**When to Use Linear Scan**:
- ✅ Data size < 100 entries
- ✅ Rare queries
- ✅ No natural prefix structure

---

### Memory Overhead

**PathMap Memory**:
```
Size = O(total_path_bytes + overhead_per_node)

Example:
- 1000 rules
- Average path: ["rule", "funcname", "2"] = 3 levels
- Average funcname: 10 bytes
- Total: ~1000 × (4 + 10 + 1) + overhead = ~20KB

Typical overhead: 1-2% of total data
```

**When Memory Matters**:
- Use shorter path segments
- Share common prefixes aggressively
- Consider single HashMap for small datasets

---

<a name="comparison"></a>
## Comparison with HashMap

### When to Use PathMap

**Use PathMap for**:
1. **Prefix-based queries**: "Find all rules with head 'fibonacci'"
2. **Hierarchical data**: Multi-level categorization
3. **Range queries**: "Find all rules with arity 2-4"
4. **Structural sharing**: Many entries share prefixes

**Example - Rule Indexing**:
```rust
// PathMap: Natural hierarchical structure
["rule", "fibonacci", "1"] → Rule
["rule", "fibonacci", "2"] → Rule
["rule", "eval", "1"] → Rule

// Can query by prefix:
// - All rules: ["rule"]
// - All fibonacci rules: ["rule", "fibonacci"]
// - Specific: ["rule", "fibonacci", "1"]
```

---

### When to Use HashMap

**Use HashMap for**:
1. **Exact lookups only**: "Get type of 'foo'"
2. **Flat data**: No hierarchy
3. **Simple keys**: String or number keys
4. **Small datasets**: < 100 entries

**Example - Type Index**:
```rust
// HashMap: Direct key-value
type_index: HashMap<String, MettaValue>
  "foo" → Type(Int)
  "bar" → Type(String)

// O(1) lookup by exact key
type_index.get("foo")
```

---

### Hybrid Approach (Recommended)

**Use both where appropriate**:

```rust
pub struct Environment {
    // PathMap for hierarchical rule indexing
    rule_index: Arc<Mutex<PathMap<Vec<Rule>>>>,

    // HashMap for flat type lookups
    type_index: Arc<Mutex<HashMap<String, MettaValue>>>,
}
```

**Benefits**:
- ✅ Best of both worlds
- ✅ Optimize each use case individually
- ✅ Clear separation of concerns

---

<a name="rholang-learnings"></a>
## Rholang LSP Learnings

### Lesson 1: Always Use Correct Zipper API

**Rholang LSP Bug (Fixed)**:
```rust
// WRONG: Used method that doesn't exist
self.patterns.insert(&path, metadata)?;

// CORRECT: Use WriteZipper
let mut wz = self.patterns.write_zipper();
for segment in &path {
    wz.descend_to(segment);
}
wz.set_val(metadata);
```

**Takeaway**: PathMap has no `insert()` method - always use WriteZipper!

---

### Lesson 2: descend_to_check() vs descend_to()

**For ReadZipper** (query):
```rust
// CORRECT: Check if path exists
if rz.descend_to_check(b"rule") {
    // Path exists
    if let Some(val) = rz.val() {
        // Use value
    }
}
```

**For WriteZipper** (insert):
```rust
// CORRECT: Create path if doesn't exist
wz.descend_to(b"rule");  // Always succeeds, creates if needed
wz.set_val(value);
```

**Takeaway**: ReadZipper checks existence, WriteZipper creates paths

---

### Lesson 3: Thread Safety with Space

**Rholang LSP Pattern**:
```rust
// Always wrap Space in Arc<Mutex<>>
pub struct GlobalSymbolIndex {
    space: Arc<Mutex<Space>>,  // Not Send+Sync without Mutex!
}
```

**Reason**: MORK's `Space` contains `Cell<u64>`:
- `Cell` is NOT `Send + Sync`
- Must wrap in `Mutex` for thread safety

**Takeaway**: Same applies to MeTTaTron's Environment

---

### Lesson 4: Pattern Index Structure

**Rholang LSP Strategy**:
```
Path: ["contract", <name_bytes>, <param0_mork>, <param1_mork>, ...]

Example:
["contract", "processUser", <@{name:n}_mork>, <ret_mork>]
```

**Benefits**:
- ✅ Exact match on contract signature
- ✅ Handles overloads (same name, different params)
- ✅ MORK bytes ensure structural matching

**Applicable to MeTTa**:
```
Path: ["rule", <head>, <arity>, <param0_pattern_mork>, ...]

Example:
["rule", "fibonacci", "1", <$n_mork>]
```

---

### Lesson 5: Graceful Degradation

**Rholang LSP Pattern**:
```rust
// Try pattern match first (optimized)
if let Some(matches) = pattern_index.query(...) {
    if !matches.is_empty() {
        return matches;
    }
}

// Fall back to lexical scope (always works)
lexical_scope_lookup(...)
```

**Applicable to MeTTa**:
```rust
// Try indexed lookup first
if let Some(head) = extract_head_symbol(expr) {
    let results = rule_index.query(head, arity);
    if !results.is_empty() {
        return results;
    }
}

// Fall back to full scan
iter_all_rules().filter(...)
```

**Takeaway**: Always have a fallback for complex cases!

---

## Summary

### PathMap Best Practices

1. ✅ Use WriteZipper for all mutations (no `insert()` method!)
2. ✅ Use ReadZipper with `descend_to_check()` for queries
3. ✅ Wrap `Space` and PathMap in `Arc<Mutex<>>` for thread safety
4. ✅ Hierarchical indexing: ["category", "subcategory", ...]
5. ✅ Wildcard paths for pattern/variable heads
6. ✅ Graceful fallback for complex queries
7. ✅ Read-modify-write pattern for appending to Vecs
8. ✅ Hold locks briefly (acquire, query, release)

### When to Use PathMap in MeTTaTron

**Yes**:
- ✅ Rule indexing by head + arity (Optimization #1)
- ✅ Fact filtering by head symbol (Optimization #2, #6)
- ✅ Hierarchical categorization

**No (use HashMap instead)**:
- ❌ Type lookups (flat key-value, Optimization #7)
- ❌ Small datasets (< 100 entries)
- ❌ Exact-key-only queries

**See Also**:
- `metta_pathmap_optimization_proposal.md` - Specific optimizations
- `metta_optimization_architecture.md` - Integration architecture
- Rholang LSP `docs/architecture/mork_pathmap_integration.md` - Reference implementation
