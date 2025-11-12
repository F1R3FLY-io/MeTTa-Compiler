# MeTTaTron Optimization Architecture

**Date**: 2025-01-10
**Related**: `metta_pathmap_optimization_proposal.md`

## Table of Contents

1. [Integration Architecture](#integration-architecture)
2. [Data Structure Diagrams](#data-structures)
3. [Evaluation Pipeline](#evaluation-pipeline)
4. [REPL Completion Architecture](#repl-completion)
5. [Space Operations Workflow](#space-operations)
6. [Thread Safety Considerations](#thread-safety)

---

<a name="integration-architecture"></a>
## Integration Architecture

### How All 7 Optimizations Work Together

```
┌─────────────────────────────────────────────────────────────────┐
│ MeTTaTron Evaluation Pipeline                                   │
└─────────────────────────────────────────────────────────────────┘
                           │
                    eval(expr, env)
                           │
         ┌─────────────────▼────────────────┐
         │ query_multi() (MORK)             │
         │ Already optimized! ✅            │
         │ O(k) pattern matching            │
         └─────────────────┬────────────────┘
                           │
                    No exact match ↓
   ┌───────────────────────────────────────────────────┐
   │ try_match_all_rules_iterative()                   │
   │ [OPTIMIZATION #1: Head Symbol PathMap Index]      │
   │                                                   │
   │ BEFORE: O(2n) - two full iterations               │
   │ AFTER:  O(k + m) - indexed by head symbol         │
   │                                                   │
   │ Implementation:                                   │
   │ • Index: ["rule", <head>, <arity>] → Vec<Rule>   │
   │ • Query by head symbol: O(3) path navigation      │
   │ • Iterate only matching rules: O(m)               │
   │ • Plus wildcard rules: ["rule", "_wildcard_"]     │
   └───────────────────────┬───────────────────────────┘
                           │
                  For each matching rule
         ┌─────────────────▼────────────────┐
         │ pattern_match(rule.lhs, expr)    │
         │ apply_bindings(rule.rhs, bindings│
         └──────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ Space Operations (Fact Database)                                │
└─────────────────────────────────────────────────────────────────┘

  ┌───────────────────────────────────┐
  │ has_sexpr_fact(sexpr)             │
  │ [OPTIMIZATION #2: Prefix Nav]     │
  │                                   │
  │ BEFORE: O(n) - check all facts    │
  │ AFTER:  O(k + m) - prefix filter  │
  │                                   │
  │ Implementation:                   │
  │ • Extract head symbol from sexpr  │
  │ • Navigate to facts with that head│
  │ • Only check facts in subtree     │
  │ • Fallback: full scan for complex │
  └───────────────────────────────────┘

  ┌───────────────────────────────────┐
  │ has_fact(atom)                    │
  │ [OPTIMIZATION #3: Fix + Prefix]   │
  │                                   │
  │ BEFORE: O(1) but WRONG!           │
  │ AFTER:  O(k) and CORRECT          │
  │                                   │
  │ Implementation:                   │
  │ • Navigate to exact atom path     │
  │ • Check if val exists (complete)  │
  │ • Uses PathMap descend_to_check() │
  └───────────────────────────────────┘

  ┌───────────────────────────────────┐
  │ match_space(pattern, template)    │
  │ [OPTIMIZATION #6: Pattern-Guided] │
  │                                   │
  │ BEFORE: O(n × p) - match all facts│
  │ AFTER:  O(k + m × p) - filter     │
  │                                   │
  │ Implementation:                   │
  │ • Extract pattern prefix          │
  │ • Navigate to matching facts      │
  │ • Pattern-match only relevant     │
  │ • Fallback: full scan for complex │
  └───────────────────────────────────┘

  ┌───────────────────────────────────┐
  │ get_type(atom)                    │
  │ [OPTIMIZATION #7: Type Index]     │
  │                                   │
  │ BEFORE: O(n) - scan all facts     │
  │ AFTER:  O(1) - HashMap lookup     │
  │                                   │
  │ Implementation:                   │
  │ • Secondary index: atom → type    │
  │ • Updated when (: atom type) added│
  │ • HashMap for instant access      │
  └───────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ REPL Completion System                                          │
└─────────────────────────────────────────────────────────────────┘

  ┌───────────────────────────────────────────┐
  │ update_from_environment(env)              │
  │ [OPTIMIZATION #4 & #5: Cache + Fuzzy]     │
  │                                           │
  │ Called when environment changes           │
  │                                           │
  │ BEFORE: No caching (rebuild per keystroke)│
  │ AFTER:  Build once, cache forever         │
  │                                           │
  │ Implementation:                           │
  │ • Extract all function names from rules   │
  │ • Combine with built-in functions         │
  │ • Build FuzzyCache (O(n log n) once)      │
  │ • Store in Arc<Mutex<>>                   │
  └───────────────────┬───────────────────────┘
                      │
                      │ Cache stored in MettaHelper
                      │
  ┌───────────────────▼───────────────────────┐
  │ complete(line, pos) - Per Keystroke       │
  │                                           │
  │ BEFORE: O(n) filter + O(m log m) sort     │
  │ AFTER:  O(k + m) fuzzy search             │
  │                                           │
  │ Implementation:                           │
  │ • Extract partial token at cursor         │
  │ • FuzzyCache.fuzzy_search(partial, 1)     │
  │   - O(k) prefix navigation                │
  │   - O(m) iterate matches                  │
  │   - Edit distance 1 (typo tolerance)      │
  │ • Results already sorted by relevance     │
  │ • No sort needed!                         │
  └───────────────────────────────────────────┘
```

---

<a name="data-structures"></a>
## Data Structure Diagrams

### Rule Index PathMap Trie (Optimization #1)

```
PathMap<Vec<Rule>> Structure:

root
│
└─ "rule" ──────────────────── (Level 1: All rules)
   │
   ├─ "fibonacci" ──────────── (Level 2: Head symbol)
   │  │
   │  ├─ "1" ─────────────────(Level 3: Arity)
   │  │  │
   │  │  └─ Vec<Rule> [
   │  │       Rule { lhs: (fibonacci $n), rhs: ... },
   │  │       Rule { lhs: (fibonacci @0), rhs: 1 },
   │  │     ]
   │  │
   │  └─ "2" ─────────────────(Level 3: Arity)
   │     │
   │     └─ Vec<Rule> [
   │          Rule { lhs: (fibonacci $n $m), rhs: ... },
   │        ]
   │
   ├─ "eval" ────────────────(Level 2: Head symbol)
   │  │
   │  ├─ "1" ─────────────────(Level 3: Arity)
   │  │  │
   │  │  └─ Vec<Rule> [ ... ]
   │  │
   │  └─ "2" ─────────────────(Level 3: Arity)
   │     │
   │     └─ Vec<Rule> [ ... ]
   │
   ├─ "+" ───────────────────(Level 2: Head symbol)
   │  │
   │  └─ "2" ─────────────────(Level 3: Arity)
   │     │
   │     └─ Vec<Rule> [
   │          Rule { lhs: (+ $a $b), rhs: ... },
   │        ]
   │
   └─ "_wildcard_" ──────────(Level 2: Special path for pattern rules)
      │
      └─ Vec<Rule> [
           Rule { lhs: ($op $a $b), rhs: ... },  // Variable head
           Rule { lhs: $x, rhs: ... },            // Pure variable
         ]

Query Process for (fibonacci 5):
1. Extract head: "fibonacci"
2. Extract arity: 1
3. Navigate: root → "rule" → "fibonacci" → "1"
4. Get Vec<Rule> at this path
5. Also get Vec<Rule> at "rule" → "_wildcard_"
6. Return combined list

Complexity: O(3) navigation + O(m) where m = matching rules
```

### Type Assertion Index (Optimization #7)

```
HashMap<String, MettaValue> Structure:

type_index: {
  "foo" → Type(Int),
  "bar" → Type(String),
  "processUser" → Type(Function([String, String], Unit)),
  "fibonacci" → Type(Function([Int], Int)),
  ...
}

Parallel Storage:
• Space (PathMap) contains (: atom type) facts for pattern matching
• type_index contains atom → type mappings for instant lookup

Add Type Assertion (: foo Int):
1. Insert into Space as fact: (: foo Int)
2. Update type_index["foo"] = Type(Int)

Lookup get_type("foo"):
1. type_index.get("foo") → O(1)
2. Return Type(Int)

No need to scan Space!
```

### FuzzyCache Structure (Optimization #5)

```
FuzzyCache<String> (from liblevenshtein):

Internal Structure (Levenshtein Automaton):
┌──────────────────────────────────────┐
│ Sorted Trie of Completions           │
│                                      │
│     root                             │
│      ├─ 'e'                          │
│      │  ├─ 'v'                       │
│      │  │  ├─ 'a'                    │
│      │  │  │  └─ 'l' → "eval"        │
│      │  │  └─ 'i'                    │
│      │  │     └─ 'l' → "evil" (typo) │
│      │  └─ ...                       │
│      ├─ 'f'                          │
│      │  ├─ 'i'                       │
│      │  │  └─ 'b' → "fib..."         │
│      │  └─ ...                       │
│      └─ 'p'                          │
│         ├─ 'r'                       │
│         │  └─ 'o' → "pro..."         │
│         └─ ...                       │
└──────────────────────────────────────┘

Fuzzy Search for "evl" (1 typo):
1. Build Levenshtein automaton for "evl" with distance 1
2. Traverse trie, accepting paths within edit distance
3. Matches:
   - "eval" (1 insertion: e[v]→l → eva[l])
   - "evil" (1 substitution: ev[l] → ev[i]l)
4. Return sorted by distance:
   - Distance 1: ["eval", "evil"]

Exact Match for "eval":
1. Direct prefix navigation (O(4) for 4 chars)
2. Return: ["eval"]

Performance:
• Build: O(n log n) one-time
• Query: O(k + m) where k=query length, m=matches
• Memory: O(n × average_length)
```

---

<a name="evaluation-pipeline"></a>
## Evaluation Pipeline with Optimizations

### Before Optimizations

```
eval(expr: (fibonacci 5), env) {
  │
  ├─ query_multi(expr, env.space) ────────────── O(k) ✅ Already fast
  │  ├─ Pattern match against Space
  │  └─ Return matches OR empty
  │
  ├─ If no matches:
  │  │
  │  └─ try_match_all_rules_iterative(expr, env)
  │     │
  │     ├─ FIRST PASS: Iterate ALL rules ──────── O(n)
  │     │  │
  │     │  └─ For each rule:
  │     │     ├─ Extract head symbol
  │     │     └─ If matches "fibonacci", add to list
  │     │
  │     ├─ SECOND PASS: Iterate ALL rules ──────── O(n)
  │     │  │
  │     │  └─ For each rule:
  │     │     ├─ Check if no head symbol (wildcard)
  │     │     └─ Add to list
  │     │
  │     └─ Total: O(2n) iterations
  │
  └─ For each matching rule:
     ├─ pattern_match(rule.lhs, expr) ──────────── O(p)
     ├─ apply_bindings(rule.rhs, bindings) ─────── O(b)
     └─ Return results
}

Total Complexity: O(k) + O(2n) + O(m × p)
Bottleneck: O(2n) rule iteration
```

### After Optimizations

```
eval(expr: (fibonacci 5), env) {
  │
  ├─ query_multi(expr, env.space) ────────────── O(k) ✅ Already fast
  │  ├─ Pattern match against Space
  │  └─ Return matches OR empty
  │
  ├─ If no matches:
  │  │
  │  └─ try_match_all_rules_iterative(expr, env)
  │     │
  │     ├─ Extract head symbol: "fibonacci" ──────── O(1)
  │     ├─ Extract arity: 1 ─────────────────────── O(1)
  │     │
  │     ├─ Query rule_index:
  │     │  ├─ Path: ["rule", "fibonacci", "1"] ──── O(3) navigation
  │     │  └─ Get Vec<Rule> ───────────────────────── O(1)
  │     │
  │     ├─ Query wildcard rules:
  │     │  ├─ Path: ["rule", "_wildcard_"] ────────── O(2) navigation
  │     │  └─ Get Vec<Rule> ───────────────────────── O(1)
  │     │
  │     └─ Total: O(3 + m) where m = matching rules
  │
  └─ For each matching rule (typically m << n):
     ├─ pattern_match(rule.lhs, expr) ──────────── O(p)
     ├─ apply_bindings(rule.rhs, bindings) ─────── O(b)
     └─ Return results
}

Total Complexity: O(k) + O(3 + m) + O(m × p)
Improvement: O(2n) → O(3 + m)

Example: 1000 rules, 5 matching "fibonacci"
Before: O(2000) iterations
After:  O(8) operations (3 nav + 5 rules)
Speedup: 250x faster!
```

---

<a name="repl-completion"></a>
## REPL Completion Architecture

### Before Optimizations

```
User types: "(fib" in REPL
            ↓
┌───────────────────────────────────────┐
│ complete(line: "(fib", pos: 4)        │
├───────────────────────────────────────┤
│ 1. get_all_completions()              │
│    ├─ Build Vec (PER KEYSTROKE!)      │
│    ├─ Add GROUNDED_FUNCTIONS          │
│    ├─ Add SPECIAL_FORMS               │
│    ├─ Add TYPE_OPERATIONS             │
│    ├─ Add CONTROL_FLOW                │
│    ├─ Add defined_functions           │
│    ├─ Add defined_variables           │
│    └─ Total: ~500 allocations ──────── O(n) build
│                                       │
│ 2. Extract partial: "fib"             │
│                                       │
│ 3. Filter all_completions             │
│    ├─ .iter()                         │
│    ├─ .filter(|c| c.starts_with("fib")│
│    └─ Total: ~500 comparisons ──────── O(n) filter
│                                       │
│ 4. Sort matches                       │
│    └─ matches.sort_by(...)  ──────────── O(m log m)
│                                       │
│ 5. Return matches                     │
└───────────────────────────────────────┘

Per Keystroke: O(n) + O(n) + O(m log m)
Total for typing "(fibonacci":
  11 keystrokes × O(500) = ~5500 operations!
```

### After Optimizations

```
Environment changes (new rule defined)
            ↓
┌───────────────────────────────────────┐
│ update_from_environment(env)          │
├───────────────────────────────────────┤
│ 1. Extract defined_functions          │
│    ├─ Iterate rules: O(n)             │
│    └─ Collect function names          │
│                                       │
│ 2. Build completion list              │
│    ├─ Combine all sources             │
│    └─ Sort + dedup ────────────────── O(n log n) ONCE
│                                       │
│ 3. Build FuzzyCache                   │
│    └─ fuzzy_cache.build(completions) ─ O(n log n) ONCE
│                                       │
│ 4. Store in cache                     │
│    └─ cached_completions = result     │
└───────────────────────────────────────┘

User types: "(fib" in REPL
            ↓
┌───────────────────────────────────────┐
│ complete(line: "(fib", pos: 4)        │
├───────────────────────────────────────┤
│ 1. Extract partial: "fib"             │
│                                       │
│ 2. fuzzy_cache.fuzzy_search("fib", 1) │
│    ├─ Prefix navigation: O(k) k=3    │
│    ├─ Iterate matches: O(m)          │
│    └─ Total: O(3 + m) ──────────────── ~10 ops
│                                       │
│ 3. Results already sorted! ✓          │
│                                       │
│ 4. Return matches                     │
└───────────────────────────────────────┘

Per Keystroke: O(k + m) where k=3, m=10
Total for typing "(fibonacci":
  11 keystrokes × O(10) = ~110 operations

Improvement: 5500 → 110 operations (50x faster!)
```

---

<a name="space-operations"></a>
## Space Operations Workflow

### Fact Storage in PathMap

```
Space.btm: PathMap<MorkExpr>

Example Facts:
  (: foo Int)
  (: bar String)
  (rule (fibonacci $n) (if ...))
  (rule (eval $expr) ...)

PathMap Structure (simplified):
root
├─ ":" (type assertions)
│  ├─ "foo" → MorkExpr((: foo Int))
│  └─ "bar" → MorkExpr((: bar String))
├─ "rule" (rule definitions)
│  ├─ <fibonacci_mork_bytes> → MorkExpr(...)
│  └─ <eval_mork_bytes> → MorkExpr(...)
└─ ...

Operations:
1. has_sexpr_fact((: foo Int))
   ├─ Extract head: ":"
   ├─ Navigate to ":" subtree
   ├─ Iterate only type assertions
   └─ Check structural equivalence

2. has_fact("foo")
   ├─ Navigate to "foo" path
   ├─ Check if value exists
   └─ Return boolean

3. match_space((rule $lhs $rhs), $template)
   ├─ Extract pattern prefix: "rule"
   ├─ Navigate to "rule" subtree
   ├─ Iterate only rule facts
   └─ Pattern match + apply template
```

---

<a name="thread-safety"></a>
## Thread Safety Considerations

### Current MeTTaTron Thread Safety

```rust
pub struct Environment {
    pub space: Arc<Mutex<Space>>,
    multiplicities: Arc<Mutex<HashMap<String, usize>>>,
    // ...
}
```

**Thread-Safe Components** ✅:
- `Space`: Wrapped in `Arc<Mutex<>>` - safe for concurrent access
- `multiplicities`: Wrapped in `Arc<Mutex<>>` - safe

**Important Note from MORK**:
- `Space` contains `Cell<u64>` - **NOT** `Send + Sync`
- Must be wrapped in `Mutex` (already done in MeTTaTron ✅)
- `SharedMappingHandle` is `Send + Sync` ✅

### Optimization Thread Safety

All proposed optimizations maintain thread safety:

```rust
// Optimization #1: Rule Index
pub struct Environment {
    rule_index: Arc<Mutex<PathMap<Vec<Rule>>>>,  // ✅ Thread-safe
}

// Optimization #4: Completion Cache
pub struct MettaHelper {
    cached_completions: Arc<Mutex<Vec<String>>>,  // ✅ Thread-safe
    dirty: Arc<Mutex<bool>>,                       // ✅ Thread-safe
}

// Optimization #5: FuzzyCache
pub struct MettaHelper {
    fuzzy_cache: Arc<Mutex<FuzzyCache<String>>>,  // ✅ Thread-safe
}

// Optimization #7: Type Index
pub struct Environment {
    type_index: Arc<Mutex<HashMap<String, MettaValue>>>,  // ✅ Thread-safe
}
```

**Locking Strategy**:
- Minimize lock duration (query quickly, release)
- Read-heavy operations (most queries)
- Writes only on environment changes (rare)

**No Data Races Possible**:
- All shared state behind `Mutex`
- Same pattern as existing MeTTaTron code
- Proven safe in Rholang LSP implementation

---

## Summary

All 7 optimizations integrate cleanly into MeTTaTron's existing architecture:

1. **Rule Index**: Extends Environment with PathMap-based rule index
2. **has_sexpr_fact**: Adds prefix navigation to existing Space operations
3. **has_fact**: Fixes correctness while using existing PathMap APIs
4. **Completion Cache**: Adds caching layer to MettaHelper
5. **FuzzyCache**: Replaces linear filter with liblevenshtein
6. **match_space**: Adds pattern-guided filtering to existing logic
7. **Type Index**: Adds secondary index alongside Space

**No Breaking Changes**:
- All public APIs remain the same
- Internal optimizations only
- Backward compatible
- Thread-safe

**See Also**:
- `metta_pathmap_optimization_proposal.md` - Detailed optimization specs
- `metta_implementation_roadmap.md` - Implementation phases
- `metta_liblevenshtein_integration.md` - liblevenshtein details
- `metta_pathmap_patterns.md` - PathMap usage patterns
