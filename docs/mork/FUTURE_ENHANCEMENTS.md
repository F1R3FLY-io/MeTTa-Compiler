# MORK Future Enhancements

**Status**: Planning Document
**Version**: 1.0
**Date**: 2025-11-26

## Overview

This document outlines potential enhancements to MeTTaTron's MORK evaluation engine beyond the current ancestor.mm2 feature set. All features in ancestor.mm2 are **already fully implemented and tested** (17/17 tests passing). These enhancements represent optimizations and advanced features for future development.

## Current Implementation Status

✅ **Complete (ancestor.mm2 verified)**:
- Fixed-point evaluation with convergence detection
- Variable binding threading across conjunction goals
- Priority ordering (integer, Peano, tuple, mixed)
- Dynamic exec generation (meta-programming)
- Conjunction patterns (empty, unary, N-ary)
- Operation forms (addition, removal)
- Pattern matching with unification
- Exec rule storage and retrieval
- Two-pass consequent evaluation

## Enhancement Categories

### 1. Performance Optimizations
### 2. Language Features
### 3. Developer Tools
### 4. Advanced Semantics

---

## 1. Performance Optimizations

### 1.1 Incremental Evaluation

**Description**: Track dependencies between facts and rules to avoid re-evaluating unchanged portions of the space during fixed-point iteration.

**Current Behavior**:
- Every iteration re-evaluates all exec rules against the entire space
- Even rules whose preconditions cannot fire due to no relevant facts changing

**Proposed Enhancement**:
```rust
struct RuleDependency {
    rule: MettaValue,
    depends_on_patterns: Vec<Pattern>,
}

struct IncrementalEvaluator {
    rules: Vec<RuleDependency>,
    fact_index: HashMap<Pattern, HashSet<FactId>>,
    dirty_rules: HashSet<RuleId>,
}

impl IncrementalEvaluator {
    fn mark_dirty(&mut self, new_fact: &MettaValue) {
        // Mark rules whose antecedents might match new_fact
        for (rule_id, rule_dep) in self.rules.iter().enumerate() {
            if rule_dep.could_match(&new_fact) {
                self.dirty_rules.insert(rule_id);
            }
        }
    }

    fn eval_iteration(&mut self, env: Environment) -> (Environment, bool) {
        let mut changed = false;
        for rule_id in self.dirty_rules.drain() {
            let (new_facts, new_env) = eval_rule(self.rules[rule_id], env);
            if !new_facts.is_empty() {
                changed = true;
                for fact in new_facts {
                    self.mark_dirty(&fact);
                }
            }
            env = new_env;
        }
        (env, changed)
    }
}
```

**Use Case**: Large MORK programs with hundreds of rules where only a small subset fire each iteration.

**Complexity**: Medium - requires dependency analysis and fact indexing

**Priority**: High - significant performance improvement for large programs

**Estimated Speedup**: 5-10× for programs with >50 rules

---

### 1.2 Parallel Rule Execution

**Description**: Execute rules at the same priority level concurrently using thread pools or async tasks.

**Current Behavior**:
- Rules executed sequentially within each priority level
- Single-threaded evaluation

**Proposed Enhancement**:
```rust
use rayon::prelude::*;

fn eval_priority_level_parallel(
    rules: Vec<ExecRule>,
    env: Environment,
) -> (Vec<MettaValue>, Environment) {
    // Rules at same priority are independent
    let results: Vec<(Vec<MettaValue>, FactSet)> = rules
        .par_iter()
        .map(|rule| {
            let local_env = env.clone();
            let (facts, _) = eval_exec_rule(rule, local_env);
            (facts, facts.iter().cloned().collect())
        })
        .collect();

    // Merge results
    let mut final_env = env;
    let mut all_facts = Vec::new();
    for (facts, fact_set) in results {
        all_facts.extend(facts);
        final_env = final_env.add_facts(&fact_set);
    }

    (all_facts, final_env)
}
```

**Use Case**: Programs with many rules at the same priority level (e.g., 20+ rules at priority (1 0)).

**Complexity**: Medium - requires careful environment merging and race condition handling

**Priority**: Medium - good for throughput but adds complexity

**Estimated Speedup**: Near-linear with core count for rule-heavy programs

**Considerations**:
- Requires thread-safe Environment implementation
- PathMap already supports structural sharing (good for cloning)
- Need careful fact merging to maintain consistency

---

### 1.3 Binding Indexing

**Description**: Cache pattern matching results and binding sets to avoid redundant unification.

**Current Behavior**:
- Every goal match rescans entire space
- No caching of successful unifications

**Proposed Enhancement**:
```rust
struct BindingCache {
    // Cache: (pattern, fact) → bindings
    cache: HashMap<(PatternHash, FactHash), Option<Bindings>>,
}

impl BindingCache {
    fn get_or_compute(
        &mut self,
        pattern: &MettaValue,
        fact: &MettaValue,
    ) -> Option<Bindings> {
        let key = (hash(pattern), hash(fact));
        self.cache.entry(key).or_insert_with(|| {
            pattern_match(pattern, fact, &Bindings::new())
        }).clone()
    }
}

fn thread_bindings_with_cache(
    goals: &[MettaValue],
    bindings: Vec<Bindings>,
    env: &Environment,
    cache: &mut BindingCache,
) -> Vec<Bindings> {
    // Same algorithm but use cache for pattern_match calls
    // ...
}
```

**Use Case**: Rules with repeated patterns that match the same facts multiple times.

**Complexity**: Low - straightforward caching layer

**Priority**: Low - only helps with specific patterns

**Estimated Speedup**: 2-3× for programs with repetitive pattern matching

---

### 1.4 Fact Indexing

**Description**: Index facts by functor/arity for faster pattern matching.

**Current Behavior**:
- Linear scan through all facts for every goal match
- O(F × G) complexity per rule

**Proposed Enhancement**:
```rust
struct FactIndex {
    // Index by functor: "parent" → [(parent Alice Bob), (parent Bob Carol), ...]
    by_functor: HashMap<String, Vec<MettaValue>>,

    // Index by arity: 2 → [all binary facts]
    by_arity: HashMap<usize, Vec<MettaValue>>,

    // Combined index: ("parent", 2) → [parent facts]
    by_signature: HashMap<(String, usize), Vec<MettaValue>>,
}

impl FactIndex {
    fn query_candidates(&self, pattern: &MettaValue) -> Vec<&MettaValue> {
        match pattern {
            MettaValue::SExpr(items) if !items.is_empty() => {
                if let MettaValue::Atom(functor) = &items[0] {
                    // Use functor+arity index
                    self.by_signature
                        .get(&(functor.clone(), items.len()))
                        .map(|v| v.iter().collect())
                        .unwrap_or_default()
                } else {
                    // Fall back to arity index
                    self.by_arity
                        .get(&items.len())
                        .map(|v| v.iter().collect())
                        .unwrap_or_default()
                }
            }
            _ => self.all_facts(), // No optimization possible
        }
    }
}
```

**Use Case**: Large fact spaces (>1000 facts) where pattern matching dominates runtime.

**Complexity**: Medium - requires index maintenance

**Priority**: High - fundamental optimization for scaling

**Estimated Speedup**: 10-100× for large fact spaces

---

## 2. Language Features

### 2.1 Negation as Failure

**Description**: Support negation in antecedents using closed-world assumption.

**Proposed Syntax**:
```metta
;; Rule: If X is a parent but not a grandparent, X is a leaf parent
(exec (1 0)
    (, (parent $x $y)
       (not (parent $y $_)))
    (, (leaf-parent $x)))
```

**Implementation**:
```rust
fn eval_negation(goal: &MettaValue, env: &Environment) -> bool {
    match goal {
        MettaValue::SExpr(items) if items.len() == 2 => {
            if let MettaValue::Atom(op) = &items[0] {
                if op == "not" {
                    let pattern = &items[1];
                    // Negation succeeds if no matches found
                    return env.match_space(pattern, pattern).is_empty();
                }
            }
        }
        _ => {}
    }
    false
}
```

**Use Case**: Queries like "find people with no children" or "find rules that never fire".

**Complexity**: Low - straightforward to implement

**Priority**: High - common Datalog feature

**Considerations**:
- Requires stratification to avoid unstratified negation
- Must evaluate negative goals after positive goals converge
- Closed-world assumption (absence of fact = falsity)

---

### 2.2 Constraint Support

**Description**: Support inequality and comparison constraints in patterns.

**Proposed Syntax**:
```metta
;; Rule: Adults are people over 18
(exec (1 0)
    (, (person $name $age)
       (!= $age $age2)  ; $age and $age2 are different
       (> $age 18))     ; $age greater than 18
    (, (adult $name)))

;; Rule: Find ancestors from different generations
(exec (2 0)
    (, (generation $level1 $p $a1)
       (generation $level2 $p $a2)
       (!= $level1 $level2))  ; Different generation levels
    (, (multi-generation-ancestor $p)))
```

**Implementation**:
```rust
fn eval_constraint(constraint: &MettaValue, bindings: &Bindings) -> bool {
    match constraint {
        MettaValue::SExpr(items) if items.len() == 3 => {
            let op = &items[0];
            let left = apply_bindings(&items[1], bindings);
            let right = apply_bindings(&items[2], bindings);

            match op {
                MettaValue::Atom(s) if s == "!=" => left != right,
                MettaValue::Atom(s) if s == ">" => compare_gt(&left, &right),
                MettaValue::Atom(s) if s == "<" => compare_lt(&left, &right),
                MettaValue::Atom(s) if s == ">=" => compare_gte(&left, &right),
                MettaValue::Atom(s) if s == "<=" => compare_lte(&left, &right),
                _ => false,
            }
        }
        _ => false,
    }
}
```

**Use Case**: Rules requiring value comparisons (age checks, numeric thresholds, inequality constraints).

**Complexity**: Medium - requires constraint evaluation and propagation

**Priority**: Medium - useful but not critical

---

### 2.3 Aggregation

**Description**: Support aggregation operators (count, sum, max, min, etc.) in rules.

**Proposed Syntax**:
```metta
;; Count number of children per parent
(exec (1 0)
    (, (parent $p $_))
    (, (child-count $p (count (parent $p $_)))))

;; Find oldest person
(exec (2 0)
    (, (person $_ $age))
    (, (max-age (max $age))))

;; Sum of all ages
(exec (2 1)
    (, (person $_ $age))
    (, (total-age (sum $age))))
```

**Implementation**:
```rust
enum Aggregator {
    Count,
    Sum,
    Max,
    Min,
    Avg,
}

fn eval_aggregation(
    agg: Aggregator,
    pattern: &MettaValue,
    env: &Environment,
) -> MettaValue {
    let matches = env.match_space(pattern, pattern);

    match agg {
        Aggregator::Count => MettaValue::Long(matches.len() as i64),
        Aggregator::Sum => {
            let values: Vec<i64> = matches.iter()
                .filter_map(|m| extract_long(m))
                .collect();
            MettaValue::Long(values.iter().sum())
        }
        Aggregator::Max => {
            matches.iter()
                .filter_map(|m| extract_long(m))
                .max()
                .map(MettaValue::Long)
                .unwrap_or(MettaValue::Nil)
        }
        // ... Min, Avg implementations ...
    }
}
```

**Use Case**: Statistical queries, counting facts, finding extrema.

**Complexity**: High - requires grouping semantics and result handling

**Priority**: Low - nice-to-have for analytics

---

### 2.4 Stratification

**Description**: Automatic dependency analysis to ensure rules are evaluated in a safe order (stratified negation, aggregation).

**Proposed Enhancement**:
```rust
struct RuleStratum {
    level: usize,
    rules: Vec<ExecRule>,
}

struct Stratification {
    strata: Vec<RuleStratum>,
}

impl Stratification {
    fn compute(rules: &[ExecRule]) -> Result<Self, StratificationError> {
        let mut graph = DependencyGraph::new();

        // Build dependency graph
        for rule in rules {
            let produces = extract_predicates(&rule.consequent);
            let depends_on = extract_predicates(&rule.antecedent);
            let negated = extract_negated_predicates(&rule.antecedent);

            for dep in depends_on {
                graph.add_edge(dep, produces, EdgeType::Positive);
            }
            for neg in negated {
                graph.add_edge(neg, produces, EdgeType::Negative);
            }
        }

        // Detect cycles through negation (unstratified program)
        if graph.has_negative_cycle() {
            return Err(StratificationError::UnstratifiedNegation);
        }

        // Topological sort to assign strata
        let strata = graph.topological_sort_strata();
        Ok(Stratification { strata })
    }

    fn eval_stratified(&self, env: Environment) -> (Environment, FixedPointResult) {
        let mut current_env = env;
        let mut total_iterations = 0;

        // Evaluate each stratum to fixed point before moving to next
        for stratum in &self.strata {
            let (new_env, result) = eval_stratum_to_fixed_point(
                stratum.rules.clone(),
                current_env,
                1000,
            );
            current_env = new_env;
            total_iterations += result.iterations;
        }

        (current_env, FixedPointResult {
            converged: true,
            iterations: total_iterations,
            facts_added: current_env.space_size(),
        })
    }
}
```

**Use Case**: Programs with negation or aggregation requiring safe evaluation order.

**Complexity**: High - requires dependency analysis and graph algorithms

**Priority**: Medium - needed for safe negation/aggregation

**Estimated Performance**: Potentially faster than naive fixed-point for stratifiable programs

---

## 3. Developer Tools

### 3.1 Trace/Debug Mode

**Description**: Detailed execution logging for debugging rule execution.

**Proposed API**:
```rust
#[derive(Debug, Clone)]
pub struct ExecutionTrace {
    pub iteration: usize,
    pub rule: MettaValue,
    pub priority: Priority,
    pub antecedent_matches: Vec<(MettaValue, Bindings)>,
    pub consequent_facts: Vec<MettaValue>,
    pub time_elapsed: Duration,
}

pub struct TracingEvaluator {
    traces: Vec<ExecutionTrace>,
    trace_level: TraceLevel,
}

impl TracingEvaluator {
    pub fn eval_with_trace(
        &mut self,
        env: Environment,
        max_iterations: usize,
    ) -> (Environment, FixedPointResult, Vec<ExecutionTrace>) {
        // ... evaluation with detailed tracing ...
        (final_env, result, self.traces.clone())
    }

    pub fn print_trace(&self) {
        for trace in &self.traces {
            println!("Iteration {}: Rule {:?}", trace.iteration, trace.rule);
            println!("  Priority: {:?}", trace.priority);
            println!("  Matches: {}", trace.antecedent_matches.len());
            println!("  Facts Added: {}", trace.consequent_facts.len());
            println!("  Time: {:?}", trace.time_elapsed);
        }
    }
}
```

**Use Case**: Understanding why rules fire or don't fire, debugging convergence issues.

**Complexity**: Low - mostly instrumentation

**Priority**: High - essential for development

---

### 3.2 Visualization Tools

**Description**: Generate visual representations of evaluation execution.

**Proposed Features**:

**Dependency Graph Visualization**:
```rust
pub fn generate_dependency_graph_dot(rules: &[ExecRule]) -> String {
    let mut dot = String::from("digraph MORK {\n");

    for rule in rules {
        let produces = extract_predicates(&rule.consequent);
        let depends = extract_predicates(&rule.antecedent);

        for pred in produces {
            for dep in &depends {
                dot.push_str(&format!("  \"{}\" -> \"{}\";\n", dep, pred));
            }
        }
    }

    dot.push_str("}\n");
    dot
}
```

**Execution Flame Graph**:
```rust
pub fn generate_execution_flamegraph(
    traces: &[ExecutionTrace],
) -> FlameGraph {
    let mut graph = FlameGraph::new();

    for trace in traces {
        let stack = vec![
            format!("iteration_{}", trace.iteration),
            format!("priority_{:?}", trace.priority),
            format!("rule_{}", rule_name(&trace.rule)),
        ];
        graph.add_sample(stack, trace.time_elapsed.as_nanos() as usize);
    }

    graph
}
```

**Fact Space Visualization**:
```rust
pub fn generate_fact_timeline(
    traces: &[ExecutionTrace],
) -> Timeline {
    let mut timeline = Timeline::new();

    for trace in traces {
        timeline.add_event(
            trace.iteration,
            Event::FactsAdded(trace.consequent_facts.len()),
        );
    }

    timeline
}
```

**Use Case**: Understanding program behavior, performance profiling, teaching/demos.

**Complexity**: Medium - requires graph generation libraries

**Priority**: Medium - useful but not critical

---

### 3.3 Property-Based Testing

**Description**: Automated test generation for MORK programs.

**Proposed Framework**:
```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_convergence(
        rules in vec(arbitrary_exec_rule(), 1..10),
        facts in vec(arbitrary_fact(), 10..100),
    ) {
        let mut env = Environment::new();
        for fact in facts {
            env.add_to_space(&fact);
        }
        for rule in rules {
            env.add_to_space(&rule);
        }

        let (_, result) = eval_env_to_fixed_point(env, 1000);

        // Property: All evaluations should converge
        prop_assert!(result.converged);

        // Property: Fact count should be monotonic
        prop_assert!(result.facts_added >= 0);
    }
}

fn arbitrary_exec_rule() -> impl Strategy<Value = MettaValue> {
    // Generate random well-formed exec rules
    (arbitrary_priority(), arbitrary_conjunction(), arbitrary_conjunction())
        .prop_map(|(p, a, c)| {
            make_exec_rule(p, a, c)
        })
}
```

**Use Case**: Finding edge cases, ensuring correctness properties hold.

**Complexity**: High - requires careful generator design

**Priority**: Low - good for robustness but requires significant effort

---

## 4. Advanced Semantics

### 4.1 Transactional Operations

**Description**: Atomic multi-fact operations with rollback on failure.

**Proposed Syntax**:
```metta
;; Transfer operation: remove from A, add to B atomically
(exec (1 0)
    (, (account $a $balance)
       (>= $balance $amount))
    (transaction
        (O (- (account $a $balance))
           (+ (account $a (- $balance $amount)))
           (+ (account $b (+ $balance2 $amount))))))
```

**Implementation**:
```rust
fn eval_transaction(ops: &[Operation], env: Environment) -> Result<Environment, RollbackError> {
    let mut temp_env = env.clone();
    let mut applied_ops = Vec::new();

    for op in ops {
        match apply_operation(op, temp_env.clone()) {
            Ok(new_env) => {
                temp_env = new_env;
                applied_ops.push(op.clone());
            }
            Err(e) => {
                // Rollback all applied operations
                return Err(RollbackError::Failed {
                    operation: op.clone(),
                    error: e,
                    rollback_ops: applied_ops,
                });
            }
        }
    }

    Ok(temp_env)
}
```

**Use Case**: Database-like operations requiring atomicity.

**Complexity**: High - requires transaction semantics and rollback

**Priority**: Low - advanced feature for specific use cases

---

### 4.2 Temporal Logic

**Description**: Time-based rules and fact expiration.

**Proposed Syntax**:
```metta
;; Fact valid for 100 iterations
(exec (1 0)
    (, (event $e $data))
    (, (temporary 100 (cached $e $data))))

;; Rule fires only after iteration 50
(exec (1 0)
    (, (after 50)
       (trigger $x))
    (, (delayed-action $x)))
```

**Use Case**: Event processing, cache expiration, time-based reasoning.

**Complexity**: High - requires temporal semantics and state management

**Priority**: Low - niche feature

---

## Implementation Roadmap

### Phase 1: Essential Performance (High Priority)
1. Fact indexing (functor/arity) - **High impact, medium complexity**
2. Incremental evaluation - **High impact, medium complexity**
3. Trace/debug mode - **Essential for development**

**Estimated Effort**: 2-3 weeks
**Expected Speedup**: 10-50× for large programs

### Phase 2: Language Features (Medium Priority)
1. Negation as failure - **Common Datalog feature**
2. Constraint support - **Useful for many programs**
3. Stratification - **Required for safe negation**

**Estimated Effort**: 2-3 weeks
**Impact**: Enables new classes of programs

### Phase 3: Developer Experience (Medium Priority)
1. Visualization tools - **Graphviz/DOT integration**
2. Parallel rule execution - **Good for throughput**
3. Binding cache - **Specific optimization**

**Estimated Effort**: 2-3 weeks
**Impact**: Better development workflow

### Phase 4: Advanced Features (Low Priority)
1. Aggregation - **Nice-to-have analytics**
2. Property-based testing - **Robustness testing**
3. Transactional operations - **Advanced use case**
4. Temporal logic - **Niche feature**

**Estimated Effort**: 4-6 weeks
**Impact**: Specialized capabilities

---

## Benchmarking Strategy

### Current Baseline
- ancestor.mm2: ~15ms for full family tree
- 20 facts + 4 rules + 10 iterations
- <10MB memory peak

### Target Benchmarks
```rust
#[bench]
fn bench_large_fact_space(b: &mut Bencher) {
    // 10,000 facts, 50 rules, measure throughput
}

#[bench]
fn bench_deep_recursion(b: &mut Bencher) {
    // 100-level recursive rules, measure convergence time
}

#[bench]
fn bench_complex_patterns(b: &mut Bencher) {
    // 10-goal conjunctions, measure pattern matching overhead
}
```

### Performance Goals
- **100,000 facts**: <500ms per iteration
- **1,000 rules**: <100ms per iteration
- **50 iterations**: <5 seconds total
- **Memory**: <100MB for 100K facts

---

## References

### Implementation
- Current MORK code: `src/backend/eval/mork_forms.rs`
- Fixed-point loop: `src/backend/eval/fixed_point.rs`
- Tests: `tests/dynamic_exec.rs`, `tests/ancestor_mm2_full.rs`

### Documentation
- Features: `docs/mork/MORK_FEATURES_SUPPORT.md`
- Completion: `docs/mork/conjunction-pattern/COMPLETION_SUMMARY.md`
- Implementation: `docs/mork/conjunction-pattern/IMPLEMENTATION.md`

### External References
- Datalog stratification: Ullman, "Principles of Database and Knowledge-Base Systems"
- Negation as failure: Lloyd, "Foundations of Logic Programming"
- Incremental evaluation: Ceri et al., "What You Always Wanted to Know About Datalog"
- Parallel Datalog: Shkapsky et al., "Big Data Analytics with Datalog Queries on Spark"

---

## Conclusion

All features from **ancestor.mm2 are fully implemented and tested** (17/17 tests passing). These enhancements represent future optimization and expansion opportunities:

**High Priority**: Fact indexing, incremental evaluation, negation support
**Medium Priority**: Constraints, visualization tools, parallel execution
**Low Priority**: Aggregation, temporal logic, transactional semantics

The current implementation is **production-ready** for MORK evaluation workloads matching the ancestor.mm2 feature set.
