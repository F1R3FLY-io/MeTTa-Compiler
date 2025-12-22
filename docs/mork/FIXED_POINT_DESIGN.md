# Fixed-Point Evaluation Design for MORK

This document describes the design and implementation strategy for the three remaining features needed for full ancestor.mm2 support:

1. Priority Ordering
2. Fixed-Point Evaluation
3. Dynamic Exec Generation

## Overview

These three features work together to enable meta-programming in MORK, where rules can create new rules that are then executed until no more changes occur (reaching a fixed point).

## Feature 1: Priority Ordering

### Purpose

Control the execution order of exec rules based on their priority values. Rules with lower priorities execute before rules with higher priorities.

### Priority Types

ancestor.mm2 uses two types of priority values:

1. **Tuple Priorities**: `(0 0)`, `(0 1)`, `(1 Z)`, `(2 0)`, `(2 1)`, `(2 2)`
2. **Mixed with Peano**: Tuples can contain Peano numbers like `Z`, `(S Z)`, etc.

### Priority Comparison Rules

```
(0 0) < (0 1) < (1 Z) < (1 (S Z)) < (2 0) < (2 1) < (2 2)
```

Lexicographic ordering:
- Compare first elements
- If equal, compare second elements
- Peano numbers: `Z < (S Z) < (S (S Z)) < ...`

### Implementation Strategy

**Location**: `src/backend/eval/priority.rs` (new file)

**Key Function**:
```rust
pub fn compare_priorities(p1: &MettaValue, p2: &MettaValue) -> Ordering
```

**Algorithm**:
1. Handle atomic values (numbers, Peano constructors)
2. Handle tuples (lexicographic comparison)
3. Recursively compare Peano numbers by counting S constructors
4. Return `Ordering::{Less, Equal, Greater}`

**Test Cases**:
```rust
assert!(compare_priorities(&(0 0), &(0 1)) == Less);
assert!(compare_priorities(&(0 1), &(1 Z)) == Less);
assert!(compare_priorities(&Z, &(S Z)) == Less);
assert!(compare_priorities(&(1 Z), &(1 (S Z))) == Less);
```

## Feature 2: Fixed-Point Evaluation

### Purpose

Execute all pending exec rules repeatedly until no new facts are generated (reaching a fixed point).

### Algorithm

```
1. Initialize fact_space with initial facts
2. Initialize rule_queue with all exec rules, sorted by priority
3. Repeat until fixed point:
   a. For each rule in rule_queue (in priority order):
      - Try to fire rule with current facts
      - If fires, add generated facts to fact_space
   b. If no new facts were generated, reached fixed point
   c. Safety: limit iterations to prevent infinite loops
4. Return final fact_space
```

### Implementation Strategy

**Location**: `src/backend/eval/fixed_point.rs` (new file)

**Key Function**:
```rust
pub fn eval_fixed_point(
    rules: Vec<ExecRule>,
    initial_facts: Vec<MettaValue>,
    max_iterations: usize
) -> (Vec<MettaValue>, usize)
```

**Data Structures**:
```rust
struct ExecRule {
    priority: MettaValue,
    antecedent: MettaValue,  // Conjunction
    consequent: MettaValue,  // Conjunction or Operation
}

struct FixedPointState {
    facts: HashSet<MettaValue>,  // Current fact space
    rules: Vec<ExecRule>,         // Sorted by priority
    iteration: usize,
    changed: bool,
}
```

**Safety Features**:
- Maximum iteration limit (default: 1000)
- Duplicate fact detection (use HashSet)
- Progress tracking (log facts added per iteration)

### Integration with Environment

Modify `Environment` to support fixed-point evaluation:

```rust
impl Environment {
    /// Execute all pending exec rules until fixed point
    pub fn eval_to_fixed_point(&mut self, max_iterations: usize) -> Result<usize, String> {
        // Extract exec rules from space
        // Sort by priority
        // Run fixed-point loop
        // Update space with final facts
    }
}
```

## Feature 3: Dynamic Exec Generation

### Purpose

Allow exec rules to be matched and generated dynamically. This enables meta-programming where rules create new rules.

### The Critical Pattern (ancestor.mm2 lines 33-36)

```metta
(exec (1 Z) (, (exec (1 $l) $ps $ts)
               (generation $l $c $p) (child $p $gp) )
            (, (exec (1 (S $l)) $ps $ts)
               (generation (S $l) $c $gp) ))
```

**What it does**:
1. **Antecedent**: Matches existing exec rules with priority `(1 $l)`
2. **Consequent**: Generates new exec rules with priority `(1 (S $l))`
3. **Effect**: Creates generation-tracking rules dynamically (0→1, 1→2, 2→3, etc.)

### Implementation Strategy

#### Step 1: Store exec rules as facts

When an exec is evaluated, store it in the fact space:

```rust
fn eval_exec(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let priority = &items[1];
    let antecedent = &items[2];
    let consequent = &items[3];

    // Create exec fact and store it
    let exec_fact = MettaValue::SExpr(items.clone());
    env.add_to_space(&exec_fact);

    // Then execute the rule normally...
}
```

#### Step 2: Match exec rules in antecedents

When evaluating an antecedent like `(exec (1 $l) $ps $ts)`:
- Use pattern matching against stored exec facts
- Bind variables: `$l`, `$ps`, `$ts`
- Return bindings to thread through conjunction

#### Step 3: Generate exec rules in consequents

When consequent contains `(exec (1 (S $l)) $ps $ts)`:
- Apply bindings to substitute variables
- Create new exec fact with instantiated values
- Add to fact space (will be executed in next iteration)

#### Step 4: Special handling for exec in consequent

```rust
match consequent {
    MettaValue::SExpr(items) if is_exec_form(&items) => {
        // Don't execute immediately - just add as fact
        let exec_fact = apply_bindings(consequent, &bindings);
        env.add_to_space(&exec_fact);
    }
    MettaValue::Conjunction(goals) => {
        // Check each goal for exec forms
        for goal in goals {
            if is_exec_form(goal) {
                let exec_fact = apply_bindings(goal, &bindings);
                env.add_to_space(&exec_fact);
            } else {
                // Normal evaluation
                eval_with_depth(goal, env, depth + 1);
            }
        }
    }
    // ... other cases
}
```

### Key Insight: Two Phases

1. **Rule Collection Phase**:
   - Exec expressions are added as facts
   - Pattern matching can query these facts

2. **Rule Execution Phase**:
   - Fixed-point loop fires all collected exec rules
   - New exec rules generated in consequents are added as facts
   - Loop continues until no new facts (including exec rules) are generated

## Implementation Order

### Phase 1: Priority Ordering (1 day)

1. Create `src/backend/eval/priority.rs`
2. Implement `compare_priorities()` with Peano support
3. Add unit tests for all priority patterns in ancestor.mm2
4. Add helper: `fn sort_rules_by_priority(rules: &mut [ExecRule])`

**Success Criteria**:
- All priority comparison tests pass
- Can correctly order: `(0 0) < (0 1) < (1 Z) < (1 (S Z)) < (2 0) < (2 1) < (2 2)`

### Phase 2: Fixed-Point Evaluation (1-2 days)

1. Create `src/backend/eval/fixed_point.rs`
2. Implement `ExecRule` struct and parsing
3. Implement fixed-point loop with iteration limit
4. Add tests for simple fixed-point scenarios
5. Integrate with `Environment::eval_to_fixed_point()`

**Success Criteria**:
- Can execute rules until convergence
- Respects priority ordering
- Handles iteration limits correctly
- Tests show proper fixed-point detection

### Phase 3: Dynamic Exec Generation (2-3 days)

1. Modify `eval_exec()` to store exec as fact
2. Add special handling for exec in consequent
3. Enable pattern matching on exec facts
4. Test meta-programming patterns
5. Full ancestor.mm2 integration test

**Success Criteria**:
- Can match exec rules in antecedents
- Can generate new exec rules in consequents
- Generated rules execute in subsequent iterations
- ancestor.mm2 lines 33-36 work correctly

## Testing Strategy

### Unit Tests

Each feature gets its own test file:
- `tests/priority_ordering.rs` - 10+ tests
- `tests/fixed_point.rs` - 15+ tests
- `tests/dynamic_exec.rs` - 20+ tests

### Integration Test

Create `tests/ancestor_mm2.rs`:
- Load ancestor.mm2 facts
- Execute to fixed point
- Verify expected inferences:
  - `(ancestor Ann Bob)`
  - `(ancestor Ann Pam)`
  - `(ancestor Ann Tom)`
  - `(incest Uru Vic)` after adding parents
  - Self-referential incest removal

### Example File

Create `examples/ancestor_demo.metta`:
- Simplified version of ancestor.mm2
- Demonstrates all three features
- Clear comments explaining each step

## Performance Considerations

### Priority Ordering
- O(n log n) sorting of rules
- O(m) comparison for tuples of length m
- One-time cost at start of fixed-point loop

### Fixed-Point Evaluation
- O(i × r × f) where:
  - i = iterations to fixed point
  - r = number of rules
  - f = number of facts
- Mitigated by:
  - Early termination on convergence
  - Efficient pattern matching (PathMap)
  - Duplicate fact detection (HashSet)

### Dynamic Exec Generation
- Minimal overhead (just storing exec as fact)
- Pattern matching on exec facts is O(m) with PathMap
- Scales well with number of generated rules

## Open Questions

1. **Should exec rules be visible in query results?**
   - Option A: Keep them separate (internal only)
   - Option B: Allow querying with `(match &self (exec $p $a $c) ...)`
   - Recommendation: Option B for full MORK compatibility

2. **How to handle infinite generation?**
   - Current: Iteration limit (1000)
   - Alternative: Detect cycles in generated rules
   - Recommendation: Start with iteration limit, add cycle detection later

3. **Priority comparison for non-standard values?**
   - What if priority is a symbol or string?
   - Recommendation: Error on non-comparable priorities, document tuple/number/Peano only

## Success Metrics

After implementation:
- ✅ All 20+ priority tests pass
- ✅ All 15+ fixed-point tests pass
- ✅ All 20+ dynamic exec tests pass
- ✅ ancestor.mm2 runs successfully
- ✅ All expected inferences generated
- ✅ Performance: ancestor.mm2 completes in <1 second
- ✅ All 569+ existing tests still pass (no regressions)

## References

- ancestor.mm2: `/home/dylon/Workspace/f1r3fly.io/MORK/kernel/resources/ancestor.mm2`
- MORK paper: Describes fixed-point semantics and meta-programming
- Existing exec implementation: `src/backend/eval/mork_forms.rs`
- Pattern matching: `src/backend/eval/mod.rs`

---

**Document Status**: Design Complete
**Implementation Status**: Not Started
**Estimated Total Effort**: 4-6 days
