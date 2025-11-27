# MeTTaTron Optimization Implementation Roadmap

**Date**: 2025-01-10
**Related**: `metta_pathmap_optimization_proposal.md`, `metta_optimization_architecture.md`

## Table of Contents

1. [Implementation Phases](#phases)
2. [Phase 1: Quick Wins](#phase-1)
3. [Phase 2: High-Impact Indexing](#phase-2)
4. [Phase 3: Advanced Features](#phase-3)
5. [Phase 4: Testing & Documentation](#phase-4)
6. [Timeline](#timeline)
7. [Success Criteria](#success-criteria)

---

<a name="phases"></a>
## Implementation Phases Overview

### 4-Phase Strategy

| Phase | Focus | Duration | Optimizations | Value |
|-------|-------|----------|---------------|-------|
| **1** | Quick Wins | 1 week | #3, #4, #2 | Fix bugs, immediate performance boost |
| **2** | High-Impact Indexing | 1 week | #1, #7 | 20-50x faster rule matching |
| **3** | Advanced Features | 1 week | #5, #6 | Fuzzy completion, pattern-guided matching |
| **4** | Testing & Docs | 1 week | All | Validation, benchmarks, documentation |

**Total Timeline**: 4 weeks (3 weeks implementation + 1 week validation)

---

<a name="phase-1"></a>
## Phase 1: Quick Wins (Week 1)

**Goal**: Fix correctness issues and gain immediate performance improvements with low-risk changes

### Task 1.1: Fix `has_fact()` Correctness (Optimization #3)

**Effort**: 1-2 hours
**Risk**: Very Low
**Priority**: 🔥 CRITICAL (Semantic Bug)

**Current Issue**: `has_fact()` returns `true` if ANY fact exists (wrong!)

**Implementation Steps**:

1. **Update function signature** (no changes needed):
   ```rust
   pub fn has_fact(&self, atom: &str) -> bool
   ```

2. **Replace implementation** in `src/backend/environment.rs:445-464`:
   ```rust
   pub fn has_fact(&self, atom: &str) -> bool {
       let space = self.space.lock().unwrap();
       let mut rz = space.btm.read_zipper();
       let atom_bytes = atom.as_bytes();

       // Navigate to exact atom path (O(k))
       if !rz.descend_to_check(atom_bytes) {
           return false;  // Path doesn't exist
       }

       // Check if complete term (has value)
       rz.val().is_some()
   }
   ```

3. **Add tests** in `tests/environment_tests.rs` (new file):
   ```rust
   #[test]
   fn test_has_fact_correctness() {
       let mut env = Environment::new();
       env.add_fact(MettaValue::Atom("foo".to_string()));
       env.add_fact(MettaValue::Atom("bar".to_string()));

       assert!(env.has_fact("foo"));      // Should be true
       assert!(env.has_fact("bar"));      // Should be true
       assert!(!env.has_fact("baz"));     // Should be false (FIXED!)
   }
   ```

4. **Run regression tests**: Ensure all existing tests pass

**Deliverables**:
- ✅ `has_fact()` returns correct results
- ✅ Unit tests for all cases
- ✅ No breaking changes

---

### Task 1.2: Add Completion Cache (Optimization #4)

**Effort**: 1-2 hours
**Risk**: Very Low
**Priority**: ⚡ HIGH (UX Improvement)

**Implementation Steps**:

1. **Update `MettaHelper` struct** in `src/repl/helper.rs:15-30`:
   ```rust
   pub struct MettaHelper {
       // Existing fields
       defined_functions: Vec<String>,
       defined_variables: Vec<String>,

       // NEW: Completion cache
       cached_completions: Arc<Mutex<Vec<String>>>,
       dirty: Arc<Mutex<bool>>,
   }
   ```

2. **Add `rebuild_completion_cache()` method**:
   ```rust
   fn rebuild_completion_cache(&mut self) {
       let mut all = Vec::new();

       all.extend(GROUNDED_FUNCTIONS.iter().map(|s| s.to_string()));
       all.extend(SPECIAL_FORMS.iter().map(|s| s.to_string()));
       all.extend(TYPE_OPERATIONS.iter().map(|s| s.to_string()));
       all.extend(CONTROL_FLOW.iter().map(|s| s.to_string()));
       all.extend(self.defined_functions.iter().cloned());
       all.extend(self.defined_variables.iter().cloned());

       all.sort();
       all.dedup();

       *self.cached_completions.lock().unwrap() = all;
       *self.dirty.lock().unwrap() = false;
   }
   ```

3. **Update `update_from_environment()` to rebuild cache**:
   ```rust
   pub fn update_from_environment(&mut self, env: &crate::backend::Environment) {
       // ... existing function extraction code ...

       // Rebuild cache
       self.rebuild_completion_cache();
   }
   ```

4. **Update `complete()` to use cache**:
   ```rust
   fn complete(&self, line: &str, pos: usize, ctx: &Context<'_>)
       -> Result<(usize, Vec<Pair>)> {

       let partial = extract_partial_token(line, pos);
       let cached = self.cached_completions.lock().unwrap();

       // Binary search for first match
       let start_idx = cached.binary_search_by(|probe| {
           if probe.starts_with(&partial) {
               std::cmp::Ordering::Equal
           } else if probe < &partial {
               std::cmp::Ordering::Less
           } else {
               std::cmp::Ordering::Greater
           }
       }).unwrap_or_else(|x| x);

       // Collect matching range
       let mut matches = Vec::new();
       for completion in cached[start_idx..].iter() {
           if !completion.starts_with(&partial) {
               break;
           }
           matches.push(Pair {
               display: completion.clone(),
               replacement: completion.clone(),
           });
       }

       Ok((start, matches))
   }
   ```

5. **Benchmark before/after** (see benchmarking plan)

**Deliverables**:
- ✅ Cached completion list
- ✅ 50x faster per-keystroke completion
- ✅ Benchmark showing improvement

---

### Task 1.3: Optimize `has_sexpr_fact()` (Optimization #2)

**Effort**: 2-3 hours
**Risk**: Low
**Priority**: 🔥 CRITICAL (Performance)

**Implementation Steps**:

1. **Add helper function** `extract_head_symbol()`:
   ```rust
   fn extract_head_symbol(sexpr: &MettaValue) -> Option<Vec<u8>> {
       match sexpr {
           MettaValue::SExpr(items) if !items.is_empty() => {
               match &items[0] {
                   MettaValue::Atom(name) => Some(name.as_bytes().to_vec()),
                   _ => None,
               }
           }
           _ => None,
       }
   }
   ```

2. **Update `has_sexpr_fact()`** in `src/backend/environment.rs:471-495`:
   ```rust
   pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
       let space = self.space.lock().unwrap();

       // Try prefix-guided search first
       if let Some(head) = Self::extract_head_symbol(sexpr) {
           let mut rz = space.btm.read_zipper();

           if !rz.descend_to_check(&head) {
               return false;  // No facts with this head
           }

           // Only iterate facts in this subtree
           while rz.to_next_val() {
               if let Ok(stored_value) = Self::mork_expr_to_metta_value(&expr, &space) {
                   if sexpr.structurally_equivalent(&stored_value) {
                       return true;
                   }
               }
           }

           false
       } else {
           // Fallback: full scan for complex patterns
           self.has_sexpr_fact_full_scan(sexpr)
       }
   }

   fn has_sexpr_fact_full_scan(&self, sexpr: &MettaValue) -> bool {
       // Original implementation for fallback
       // ... existing code ...
   }
   ```

3. **Add tests**:
   ```rust
   #[test]
   fn test_has_sexpr_fact_performance() {
       let mut env = Environment::new();

       // Add 10,000 facts with various heads
       for i in 0..10000 {
           env.add_fact(type_assertion(format!("var{}", i), "Int"));
       }

       // Add one fact with different head
       env.add_fact(rule_fact("special", "..."));

       // Should find it quickly (not checking all 10,000)
       assert!(env.has_sexpr_fact(&rule_fact("special", "...")));
   }
   ```

**Deliverables**:
- ✅ Prefix-guided fact searching
- ✅ 10-90x faster (depending on distribution)
- ✅ Graceful fallback for complex patterns

---

**Phase 1 Summary**:
- **Total Effort**: 4-7 hours
- **Deliverables**: 3 optimizations implemented and tested
- **Value**: Bug fix + immediate performance gains
- **Risk**: Very Low

---

<a name="phase-2"></a>
## Phase 2: High-Impact Indexing (Week 2)

**Goal**: Implement PathMap indexing for maximum performance gains

### Task 2.1: Head Symbol Rule Index (Optimization #1)

**Effort**: 4-6 hours
**Risk**: Medium
**Priority**: 🔥 CRITICAL (Highest Performance Impact)

**Implementation Steps**:

1. **Add `rule_index` field** to `Environment` in `src/backend/environment.rs:10-20`:
   ```rust
   pub struct Environment {
       pub space: Arc<Mutex<Space>>,
       multiplicities: Arc<Mutex<HashMap<String, usize>>>,

       // NEW: Rule index by head symbol + arity
       rule_index: Arc<Mutex<PathMap<Vec<Rule>>>>,
   }
   ```

2. **Update `Environment::new()` to initialize index**:
   ```rust
   pub fn new() -> Self {
       Self {
           space: Arc::new(Mutex::new(Space::new())),
           multiplicities: Arc::new(Mutex::new(HashMap::new())),
           rule_index: Arc::new(Mutex::new(PathMap::new())),
       }
   }
   ```

3. **Create `index_rule()` helper**:
   ```rust
   fn index_rule(&self, rule: &Rule) {
       let mut index = self.rule_index.lock().unwrap();
       let mut wz = index.write_zipper();

       // Extract head symbol and arity
       let (head, arity) = match &rule.lhs {
           MettaValue::SExpr(items) if !items.is_empty() => {
               match &items[0] {
                   MettaValue::Atom(name) => (Some(name.clone()), items.len()),
                   _ => (None, items.len()),
               }
           }
           _ => (None, 0),
       };

       // Build path: ["rule", <head>, <arity>]
       wz.descend_to(b"rule");
       if let Some(head_str) = head {
           wz.descend_to(head_str.as_bytes());
           wz.descend_to(arity.to_string().as_bytes());
       } else {
           wz.descend_to(b"_wildcard_");
       }

       // Add rule to vector at this path
       let mut rules = wz.val().cloned().unwrap_or_default();
       rules.push(rule.clone());
       wz.set_val(rules);
   }
   ```

4. **Update `add_rule()` to index**:
   ```rust
   pub fn add_rule(&mut self, rule: Rule) {
       // Existing logic to add to Space
       // ...

       // NEW: Also index the rule
       self.index_rule(&rule);
   }
   ```

5. **Create `query_rules_by_head()` method**:
   ```rust
   pub fn query_rules_by_head(&self, head: &str, arity: usize) -> Vec<Rule> {
       let index = self.rule_index.lock().unwrap();
       let mut results = Vec::new();

       // Query specific head
       let mut rz = index.read_zipper();
       if rz.descend_to_check(b"rule")
           && rz.descend_to_check(head.as_bytes())
           && rz.descend_to_check(arity.to_string().as_bytes()) {
           if let Some(rules) = rz.val() {
               results.extend_from_slice(rules);
           }
       }

       // Always include wildcard rules
       let mut rz = index.read_zipper();
       if rz.descend_to_check(b"rule")
           && rz.descend_to_check(b"_wildcard_") {
           if let Some(wildcard_rules) = rz.val() {
               results.extend_from_slice(wildcard_rules);
           }
       }

       results
   }
   ```

6. **Update `try_match_all_rules_iterative()`** in `src/backend/eval/mod.rs:617-643`:
   ```rust
   fn try_match_all_rules_iterative(
       expr: &MettaValue,
       env: &Environment,
   ) -> Vec<(MettaValue, Bindings)> {
       let mut matching_rules = Vec::new();

       // Extract head symbol and arity
       if let MettaValue::SExpr(items) = expr {
           if let Some(MettaValue::Atom(head)) = items.get(0) {
               // Use indexed lookup!
               matching_rules = env.query_rules_by_head(head, items.len());
           }
       }

       // Fallback if no head
       if matching_rules.is_empty() {
           matching_rules = env.query_wildcard_rules();
       }

       // Rest of matching logic...
   }
   ```

7. **Comprehensive testing** (see benchmarking plan)

**Deliverables**:
- ✅ Rule index by head symbol + arity
- ✅ 20-50x faster rule matching
- ✅ Maintains exact semantics

---

### Task 2.2: Type Assertion Index (Optimization #7)

**Effort**: 2 hours
**Risk**: Low
**Priority**: 📋 LOW (Nice to Have)

**Implementation Steps**:

1. **Add `type_index` field** to `Environment`:
   ```rust
   pub struct Environment {
       pub space: Arc<Mutex<Space>>,
       multiplicities: Arc<Mutex<HashMap<String, usize>>>,
       rule_index: Arc<Mutex<PathMap<Vec<Rule>>>>,

       // NEW: Type assertion index
       type_index: Arc<Mutex<HashMap<String, MettaValue>>>,
   }
   ```

2. **Create `add_type_assertion()` method**:
   ```rust
   pub fn add_type_assertion(&self, atom: &str, typ: MettaValue) {
       // Add to Space
       let type_fact = MettaValue::SExpr(vec![
           MettaValue::Atom(":".to_string()),
           MettaValue::Atom(atom.to_string()),
           typ.clone(),
       ]);
       self.add_fact(type_fact);

       // Update index
       self.type_index.lock().unwrap().insert(atom.to_string(), typ);
   }
   ```

3. **Create `get_type()` and `has_type()` methods**:
   ```rust
   pub fn get_type(&self, atom: &str) -> Option<MettaValue> {
       self.type_index.lock().unwrap().get(atom).cloned()
   }

   pub fn has_type(&self, atom: &str) -> bool {
       self.type_index.lock().unwrap().contains_key(atom)
   }
   ```

**Deliverables**:
- ✅ O(1) type lookups
- ✅ Foundation for type checking

---

**Phase 2 Summary**:
- **Total Effort**: 6-8 hours
- **Deliverables**: 2 major indexes implemented
- **Value**: 20-50x faster rule matching, instant type lookups
- **Risk**: Medium (requires careful testing)

---

<a name="phase-3"></a>
## Phase 3: Advanced Features (Week 3)

**Goal**: Add fuzzy completion and pattern-guided matching

### Task 3.1: liblevenshtein FuzzyCache (Optimization #5)

**Effort**: 3-4 hours
**Risk**: Low
**Priority**: ⚡ HIGH (Enhanced UX)

**Implementation Steps**:

1. **Add dependency** to `Cargo.toml`:
   ```toml
   [dependencies]
   liblevenshtein = "0.6"
   ```

2. **Update `MettaHelper` struct**:
   ```rust
   use liblevenshtein::cache::FuzzyCache;

   pub struct MettaHelper {
       defined_functions: Vec<String>,
       defined_variables: Vec<String>,

       // Replace cached_completions with fuzzy_cache
       fuzzy_cache: Arc<Mutex<FuzzyCache<String>>>,
   }
   ```

3. **Update `rebuild_completion_cache()`**:
   ```rust
   fn rebuild_completion_cache(&mut self) {
       let mut all = Vec::new();
       // ... collect all completions ...

       // Build fuzzy cache
       let mut cache = FuzzyCache::new();
       cache.build(all);

       *self.fuzzy_cache.lock().unwrap() = cache;
   }
   ```

4. **Update `complete()` to use fuzzy search**:
   ```rust
   fn complete(&self, line: &str, pos: usize, ctx: &Context<'_>)
       -> Result<(usize, Vec<Pair>)> {

       let partial = extract_partial_token(line, pos);
       let fuzzy_cache = self.fuzzy_cache.lock().unwrap();

       // Fuzzy search with edit distance 1
       let matches: Vec<Pair> = fuzzy_cache
           .fuzzy_search(&partial, 1)
           .map(|completion| Pair {
               display: completion.clone(),
               replacement: completion.clone(),
           })
           .collect();

       Ok((start, matches))
   }
   ```

5. **Test typo tolerance**:
   ```rust
   #[test]
   fn test_fuzzy_completion_typos() {
       let helper = setup_helper_with_completions();

       // Typo: "evl" should match "eval"
       let matches = helper.complete("(evl", 4, &ctx).unwrap();
       assert!(matches.1.iter().any(|p| p.display == "eval"));

       // Typo: "procesUser" should match "processUser"
       let matches = helper.complete("(procesUser", 11, &ctx).unwrap();
       assert!(matches.1.iter().any(|p| p.display == "processUser"));
   }
   ```

**Deliverables**:
- ✅ Fuzzy completion with typo tolerance
- ✅ 24x faster queries
- ✅ Better user experience

---

### Task 3.2: Pattern-Guided `match_space()` (Optimization #6)

**Effort**: 3-5 hours
**Risk**: Medium
**Priority**: 📊 MEDIUM (Specialized)

**Implementation Steps**:

1. **Create `extract_pattern_prefix()` helper**:
   ```rust
   fn extract_pattern_prefix(pattern: &MettaValue) -> Option<Vec<u8>> {
       match pattern {
           MettaValue::SExpr(items) if !items.is_empty() => {
               match &items[0] {
                   MettaValue::Atom(head) => Some(head.as_bytes().to_vec()),
                   _ => None,
               }
           }
           _ => None,
       }
   }
   ```

2. **Update `match_space()`** in `src/backend/environment.rs:361-390`:
   ```rust
   pub fn match_space(&self, pattern: &MettaValue, template: &MettaValue)
       -> Vec<MettaValue> {

       let space = self.space.lock().unwrap();
       let mut results = Vec::new();

       if let Some(prefix) = extract_pattern_prefix(pattern) {
           // Prefix-guided search
           let mut rz = space.btm.read_zipper();

           if rz.descend_to_check(&prefix) {
               while rz.to_next_val() {
                   if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                       if let Some(bindings) = pattern_match(pattern, &atom) {
                           let instantiated = apply_bindings(template, &bindings);
                           results.push(instantiated);
                       }
                   }
               }
           }
       } else {
           // Fallback: full scan
           // ... original implementation ...
       }

       results
   }
   ```

**Deliverables**:
- ✅ Pattern-guided fact matching
- ✅ 20-97x faster for common patterns
- ✅ Graceful fallback

---

**Phase 3 Summary**:
- **Total Effort**: 6-9 hours
- **Deliverables**: Fuzzy completion + pattern-guided matching
- **Value**: Enhanced UX + specialized performance gains
- **Risk**: Low-Medium

---

<a name="phase-4"></a>
## Phase 4: Testing & Documentation (Week 4)

**Goal**: Comprehensive validation and documentation

### Task 4.1: Comprehensive Testing

**Effort**: 8-12 hours

**Test Categories**:

1. **Unit Tests** (per optimization):
   - Correctness verification
   - Edge cases (empty data, complex patterns)
   - Error handling

2. **Integration Tests**:
   - End-to-end workflows
   - REPL sessions
   - Evaluation pipelines

3. **Regression Tests**:
   - All existing tests must pass
   - No behavioral changes

4. **Performance Benchmarks** (see `metta_benchmarking_plan.md`):
   - Before/after measurements
   - Speedup validation
   - Performance regression detection

### Task 4.2: Documentation

**Effort**: 4-6 hours

**Deliverables**:
- ✅ Architecture documentation updates
- ✅ Performance results published
- ✅ Usage examples
- ✅ Migration guide (if needed)

---

**Phase 4 Summary**:
- **Total Effort**: 12-18 hours
- **Deliverables**: Complete test suite + documentation
- **Value**: Confidence in correctness and performance
- **Risk**: Low

---

<a name="timeline"></a>
## Timeline

### Week-by-Week Breakdown

**Week 1: Quick Wins**
- Day 1-2: Optimization #3 (has_fact fix)
- Day 2-3: Optimization #4 (completion cache)
- Day 3-5: Optimization #2 (has_sexpr_fact prefix nav)

**Week 2: High-Impact Indexing**
- Day 1-3: Optimization #1 (rule index)
- Day 4-5: Optimization #7 (type index)

**Week 3: Advanced Features**
- Day 1-2: Optimization #5 (FuzzyCache)
- Day 3-5: Optimization #6 (pattern-guided match_space)

**Week 4: Testing & Documentation**
- Day 1-2: Unit tests for all optimizations
- Day 3: Integration tests
- Day 4: Benchmarking and performance validation
- Day 5: Documentation updates

---

<a name="success-criteria"></a>
## Success Criteria

### Per-Optimization Criteria

| Optimization | Success Metric |
|-------------|----------------|
| #1: Rule Index | 20-50x faster rule matching (benchmarked) |
| #2: has_sexpr_fact | 10-90x faster fact checks (benchmarked) |
| #3: has_fact | Correct semantics (all tests pass) |
| #4: Completion Cache | 50x faster per-keystroke (benchmarked) |
| #5: FuzzyCache | Typo tolerance + 24x faster (tested) |
| #6: match_space | 20-97x faster for common patterns (benchmarked) |
| #7: Type Index | O(1) lookups verified |

### Overall Success Criteria

1. ✅ All existing tests pass (no regressions)
2. ✅ All benchmarks show expected speedups (±20%)
3. ✅ No breaking API changes
4. ✅ Thread safety maintained
5. ✅ Documentation complete and accurate

---

## Risk Mitigation

**For Each Optimization**:
1. Implement in isolated branch
2. Run full test suite
3. Benchmark before/after
4. Code review before merge
5. Gradual rollout if possible

**Rollback Plan**:
- Keep original implementations as fallback
- Feature flags for new optimizations
- Easy revert if issues found

---

## Next Steps After Approval

1. Create feature branch `feature/metta-optimizations`
2. Start Phase 1 (Week 1)
3. Daily progress updates
4. Code reviews after each phase
5. Merge to main after Phase 4 validation

**See Also**:
- `metta_pathmap_optimization_proposal.md` - Detailed specs
- `metta_benchmarking_plan.md` - Performance testing strategy
- `metta_optimization_architecture.md` - Architecture integration
