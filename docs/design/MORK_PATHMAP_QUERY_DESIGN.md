# MORK PathMap Query Design

## Current Problem

The current implementation maintains redundant caches:
- `rule_cache: Vec<Rule>` - O(n) linear search through all rules
- `rule_index: HashMap<String, Vec<usize>>` - O(1) hash lookup + O(k) pattern matching
- `sexpr_cache: HashSet<String>` - O(1) existence checking

**But MORK Space with PathMap IS the index!**

## PathMap/Trie Architecture

PathMap is a trie-map where:
- **Insertion**: O(m) where m = key length
- **Lookup**: O(m) where m = query length
- **Prefix Query**: O(m + k) where k = number of matching entries

## Proper Design

### Rule Storage
```
MORK Space (PathMap trie)
  (= (double $x) (mul $x 2))
  (= (double $x $y) (mul (add $x $y) 2))
  (= (fact 0) 1)
  (= (fact $n) (mul $n (fact (sub $n 1))))
```

### Rule Lookup by Head Symbol

**Query**: Find all rules with head `double`
```
Pattern: (= (double $ ...) ...)
Result: O(m) trie navigation to prefix + O(k) iteration over matches
```

where:
- m = length of prefix pattern `(= (double`
- k = number of rules with head `double`

**Much better than**:
- HashMap: O(1) hash + O(k) - but requires maintaining separate index
- Linear: O(n) - iterates through all rules

### Fact Existence

**Query**: Check if `(Hello World)` exists
```
Pattern: (Hello World)
Result: O(m) trie lookup where m = pattern length
```

**Current approach**: HashSet cache O(1) - redundant!

## Required PathMap Query API

To implement this properly, we need from MORK/PathMap:

1. **Prefix Query**
   ```rust
   space.query_prefix(pattern: &str) -> Iterator<Entry>
   ```
   Navigate trie to prefix, return all matches

2. **Exact Match**
   ```rust
   space.contains(key: &str) -> bool
   ```
   Check if exact key exists in trie

3. **Pattern Match with Variables**
   ```rust
   space.query_pattern(pattern: &str) -> Iterator<Entry>
   ```
   Query with wildcards: `(= (double $ ...) ...)`

## TODO: Investigate MORK API

Current code uses:
- `space.load_all_sexpr(bytes)` - insertion ✓
- `space.btm.read_zipper()` - navigation ?
- `rz.to_next_val()` - iteration (O(n) - wrong!)

Need to find/implement:
- Zipper navigation to specific prefix
- Efficient prefix-based queries
- Pattern matching with variables

## Refactoring Plan

1. **Remove redundant caches**
   - Remove `rule_index: HashMap`
   - Remove `sexpr_cache: HashSet`
   - Keep `rule_cache: Vec<Rule>` temporarily for backward compat

2. **Implement PathMap queries**
   - Query rules by head symbol using trie prefix
   - Query facts using trie exact match
   - Use zipper for efficient navigation

3. **Optimize further**
   - Eventually remove `rule_cache` if we can parse rules from MORK
   - Keep only `types: HashMap` for type assertions (not in MORK)
   - MORK Space becomes THE ONLY cache/index

## Expected Performance

| Operation | Current (HashMap) | Proper (PathMap) |
|-----------|------------------|------------------|
| Rule lookup by head | O(1) + O(k) | O(m) + O(k) |
| Fact existence | O(1) | O(m) |
| Memory overhead | 3 separate caches | 1 trie |

where m = pattern length (typically small, e.g., 10-50 bytes)

For typical use:
- m ≈ 20 (pattern length)
- n ≈ 1000 (total rules)
- k ≈ 10 (rules per head symbol)

PathMap O(20) is comparable to HashMap O(1) but with:
- No redundant storage
- No cache synchronization
- Native pattern matching support
