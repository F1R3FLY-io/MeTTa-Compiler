# S-Expression Fact Tracking Design

## Problem Summary

After removing redundant HashMap/HashSet caches (rule_index, sexpr_cache), we attempted to replace `sexpr_facts: HashSet<String>` with direct PathMap queries using `contains()`. However, **all tests failed** when checking for s-expression existence.

## Root Cause

**MORK Space stores s-expressions in binary format, not text format.**

### How Data is Stored

When `add_to_space()` is called:

1. MettaValue is converted to MORK text format: `"(Hello World)"`
2. This text is passed to `space.load_all_sexpr(mork_bytes)`
3. MORK's parser converts the text to **binary format** (tags + symbols)
4. The **binary data** is inserted into PathMap at line 812 in space.rs:
   ```rust
   let data = &stack[..ez.loc];  // Binary parsed format
   self.btm.insert(data, ());     // Stored as binary key
   ```

### How We Were Querying

In the failed implementation:

```rust
pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
    let mork_str = sexpr.to_mork_string();  // "(Hello World)" as text
    self.space.borrow().btm.contains(mork_str.as_bytes())  // Query with text bytes
}
```

**Mismatch**: PathMap contains binary keys, but we're querying with text keys!

## Current Solution

We restored the `sexpr_facts: HashSet<String>` as a **temporary bridge**:

```rust
pub struct Environment {
    pub space: Rc<RefCell<Space>>,
    /// TEMPORARY: PathMap stores s-expressions in binary format (from parse), but
    /// has_sexpr_fact() needs to check MORK text format. This HashSet bridges that gap.
    /// TODO: Remove once we can query MORK Space with parsed binary keys
    pub(crate) sexpr_facts: HashSet<String>,
}
```

The HashSet tracks MORK text strings (like `"(Hello World)"`) for O(1) existence checks.

## Why This Works

- **add_to_space()**: Stores text in sexpr_facts AND binary in PathMap
- **has_sexpr_fact()**: Queries the text-based sexpr_facts HashSet
- **Pattern matching**: Uses PathMap's binary format for efficient queries

## Future Work

To remove the sexpr_facts HashSet, we need to:

1. **Parse MettaValue to binary format** before querying:
   ```rust
   pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
       let mork_str = sexpr.to_mork_string();
       // TODO: Parse mork_str into binary format using MORK's parser
       let binary_key = parse_to_binary(mork_str);  // Need this function!
       self.space.borrow().btm.contains(&binary_key)
   }
   ```

2. **Investigate PathMap query API** for s-expression queries:
   - Can we query with partially-parsed data?
   - Is there a text-to-binary conversion helper?
   - Should we cache binary keys instead of text strings?

## Performance Impact

**Current (with HashSet)**:
- S-expression existence check: O(1) HashSet lookup
- Memory: O(n) where n = unique s-expressions (text strings)

**Ideal (PathMap only)**:
- S-expression existence check: O(m) PathMap contains (m = key length)
- Memory: O(n) in PathMap only (no redundant storage)

**Verdict**: The HashSet is acceptable as a temporary solution. The memory overhead is small compared to PathMap's trie, and O(1) lookups are actually better than O(m) PathMap lookups for simple existence checks.

## Lessons Learned

1. **PathMap stores data, not text**: Keys in PathMap are the actual parsed binary structures
2. **Text format is for humans**: MORK text format is just the input/output representation
3. **Don't assume formats match**: Always check how data is stored vs. how it's queried
4. **Temporary caches are OK**: Sometimes a small cache is better than complex queries

## Test Results

After restoring sexpr_facts: **All 69 tests pass** ✅

```
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
