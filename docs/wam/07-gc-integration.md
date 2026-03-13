# 7. GC Integration

This chapter describes how the WAM engine integrates with MeTTaTron's garbage
collector, covering root collection from WAM state, safepoint protocol, and
trail entry reachability.

## Background: MeTTaTron GC Architecture

MeTTaTron uses a **snapshot-based mark-sweep** garbage collector with:

- **Slab allocator**: All `MettaValue` inner data is slab-allocated with
  `'static` lifetime. Values are never moved.
- **Quiescent-state protocol**: GC can only proceed when all evaluators have
  reached a quiescent state (not mutating the object graph).
- **Root registry**: Each component that holds `MettaValue` references registers
  a `RootProvider` that enumerates its live values.
- **Epoch filtering**: The GC traces only values allocated since the last
  collection, using epoch-based filtering.

The WAM engine creates temporary state (`WamState`) during rule dispatch that
holds `MettaValue` references in registers, binding frames, trail entries,
choice points, and match results. These references must be visible to the GC
if a collection occurs during or after WAM execution.

## Root Collection Interface

Every WAM component implements a `collect_gc_roots(&self, out: &mut Vec<MettaValue>)`
method that enumerates its live `MettaValue` references:

### WamState Root Collection

The top-level `WamState::collect_gc_roots()` delegates to each component:

```rust
impl WamState {
    pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
        self.registers.collect_gc_roots(out);
        self.frame.collect_gc_roots(out);
        self.trail.collect_gc_roots(out);
        for cp in &self.choice_points {
            cp.collect_gc_roots(out);
        }
        for result in &self.match_results {
            out.push(result.rhs_info.template);
            if let Some(rhs_type) = result.rhs_info.rhs_type {
                out.push(rhs_type);
            }
            for (_, v) in result.bindings.iter() {
                out.push(*v);
            }
        }
    }
}
```

### Component-Level Root Collection

Each component reports its live references:

**WamRegisters**:

```rust
pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
    for i in 0..self.arity as usize {
        let val = self.args[i];
        if val != MettaValue::inline_unit()
            && val != MettaValue::inline_empty()
        {
            out.push(val);
        }
    }
    if self.scratch != MettaValue::inline_unit()
        && self.scratch != MettaValue::inline_empty()
    {
        out.push(self.scratch);
    }
}
```

Only registers up to `self.arity` are scanned. Inline values (`Unit`, `Empty`)
are excluded because they do not reference slab-allocated data.

**WamBindingFrame**:

```rust
pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
    let unbound = Self::unbound();
    for &slot in &self.slots {
        if slot != unbound {
            out.push(slot);
        }
    }
}
```

Only bound slots are reported. The `UNBOUND` sentinel (`MettaValue::inline_empty()`)
is excluded.

**Trail**:

```rust
pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
    for entry in &self.entries {
        out.push(entry.previous);
    }
}
```

All trail entries report their `previous` value, even if it is the UNBOUND
sentinel. This is conservative: the GC will simply ignore inline values during
marking.

**WamChoicePoint**:

```rust
pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
    // Accumulated results
    out.extend(self.results.iter().copied());
    // Alternative RHS templates
    for alt in &self.alternatives {
        out.push(alt.rhs_template);
        if let Some(rhs_type) = alt.rhs_type {
            out.push(rhs_type);
        }
    }
}
```

Choice points report two categories:
1. **Results**: Values accumulated from previously-explored branches
2. **Alternative RHS templates**: Templates for not-yet-explored alternatives

## Root Reachability Analysis

The following diagram shows which `MettaValue` references are reachable from
the `WamState` at any point during execution:

```
  WamState
  +-- registers
  |     +-- args[0..arity]         live values being matched
  |     +-- scratch                temporary computation value
  |
  +-- frame
  |     +-- slots[0..num_slots]    bound variable values (excl. UNBOUND)
  |
  +-- trail
  |     +-- entries[0..len]
  |           +-- previous         pre-binding values (for undo)
  |
  +-- choice_points[0..N]
  |     +-- results[]              values from explored branches
  |     +-- alternatives[]
  |           +-- rhs_template     RHS template MettaValues
  |           +-- rhs_type         optional type values
  |
  +-- match_results[0..M]
  |     +-- rhs_info.template      matched RHS templates
  |     +-- rhs_info.rhs_type      matched RHS types
  |     +-- bindings               variable binding values
  |
  +-- code (Arc<WamCode>)
        +-- rhs_templates[]
              +-- template         all rule RHS templates
              +-- rhs_type         all rule RHS types
```

### Trail Entry Reachability

Trail entries deserve special attention because they hold *previous* values
(the values that were in binding frame slots *before* the current bindings).
These previous values must be kept alive because:

1. **Backtracking restores them**: When `trail.unwind_to()` executes, trail
   entries' `previous` values are written back to frame slots. If the GC
   collected these values, the restored slots would contain dangling references.

2. **Transitive reachability**: A previous value may be the only reference to a
   slab-allocated term. Consider:

   ```
   Step 1: frame[0] = UNBOUND
   Step 2: BindSlot(0, A) -> trail: [(0, UNBOUND)], frame[0] = A
   Step 3: ... (A was only in frame[0]; now trail holds UNBOUND, frame holds A)
   Step 4: backtrack -> unwind -> frame[0] = UNBOUND (A still in match_results)
   ```

   If between steps 3 and 4 a GC occurs, A is reachable via `frame.slots[0]`.
   After step 4, A is reachable via `match_results` (if the match succeeded).
   The previous value `UNBOUND` is an inline value and needs no special handling.

   For the more interesting case where a slot is rebound:

   ```
   Step 1: frame[0] = UNBOUND
   Step 2: BindSlot(0, A) -> trail: [(0, UNBOUND)], frame[0] = A
   Step 3: BindSlot(0, B) -> trail: [(0, UNBOUND), (0, A)], frame[0] = B
   ```

   Now A is reachable *only* through the trail. If the trail did not report GC
   roots, A could be collected, causing a use-after-free when backtracking
   restores it.

### Choice Point Result Reachability

Choice point results are values from previously-explored alternatives that have
not yet been returned to the caller. They are live until `wam_dispatch_rules()`
returns.

In the current implementation, choice points in the engine's `choice_points` Vec
use a simplified structure where `results` and `alternatives` are empty (the
engine uses the instruction-pointer-based layout instead of the alternative-list
layout). However, the `collect_gc_roots()` method handles both cases for forward
compatibility.

### Arc<WamCode> Reachability

The `WamCode` structure is reference-counted via `Arc`. The `WamState` holds an
`Arc<WamCode>` clone, ensuring the compiled instructions and RHS templates remain
alive for the duration of execution. The RHS templates within `WamCode` are
`MettaValue` references to slab-allocated data -- these are kept alive by the
`Arc`'s reference to the `WamCode` struct.

## Safepoint Protocol

MeTTaTron uses **cooperative safepoints** for GC coordination. The trampoline
evaluation loop checks for safepoints every 256 iterations:

```
  Trampoline loop:
    iteration_count += 1
    if iteration_count % 256 == 0:
        if should_safepoint():
            // 1. Collect roots from work_stack + continuations
            // 2. Register temporary roots with GC
            // 3. Drop EvalGuard (allow GC to proceed)
            // 4. Wait for GC completion
            // 5. Re-acquire EvalGuard
```

### WAM Execution and Safepoints

The WAM execution loop (`execute_wam()`) runs within a single call from the
trampoline. It does **not** have internal safepoints -- the entire dispatch
completes without yielding to the GC.

This is safe because:

1. **WAM dispatch is fast**: A typical rule match involves 10-20 instructions,
   each executing in O(1). Even with multi-rule backtracking, total execution
   time is sub-microsecond.

2. **No allocation within the loop**: The WAM engine does not allocate new
   `MettaValue` instances during instruction dispatch. All values are existing
   slab-allocated references. The only allocations are:
   - `Vec` pushes for `match_results` (Vec metadata, not MettaValues)
   - `SmallVec` operations in `to_generic_bindings()` (at TailEval time)

3. **Bounded execution**: The number of instructions is bounded by the compiled
   code length (N rules x ~10 instructions each). There is no unbounded looping
   within the WAM engine.

If future phases add more complex evaluation within the WAM (e.g., WAM-native
special forms via `YieldToTrampoline`), safepoints may need to be added to the
execution loop.

### Root Visibility During Safepoint

When the trampoline reaches a safepoint, the `WamState` has already been dropped
(it is stack-allocated within `wam_dispatch_rules()` and returned before the
trampoline continues). The match results are in the trampoline's work stack as
`(rhs_template, bindings)` pairs, which are covered by the trampoline's own
root collection.

The temporal ordering is:

```
  1. Trampoline identifies rule dispatch needed
  2. wam_dispatch_rules() called
     2a. WamState created (stack)
     2b. execute_wam() runs to completion
     2c. Results extracted from WamState
     2d. WamState dropped (stack unwind)
  3. Results returned to trampoline as Vec<(MettaValue, GenericBindings, ...)>
  4. Trampoline pushes results to work stack / continuation stack
  5. ... later: safepoint check
     5a. Trampoline collects roots from work_stack + continuations
     5b. Results are reachable via trampoline roots
```

Between steps 2b and 2c, if a GC were somehow triggered (not possible in the
current single-threaded model since GC requires quiescence), the `WamState`'s
`collect_gc_roots()` would enumerate all live references.

## Binding Frame GC Interaction

The `WamBindingFrame` has a method `apply_to_template()` that performs binding
substitution directly from the frame. This method allocates new `MettaValue`
instances (via `factory.sexpr()`) for S-expressions with substituted children:

```rust
pub fn apply_to_template(
    &self,
    template: &MettaValue,
    factory: &GcFactory,
) -> MettaValue {
    // ... iterative postorder traversal ...
    // Allocates new S-expressions via factory.sexpr(children)
}
```

These new allocations go through the `GcFactory` which delegates to the global
slab allocator. They are immediately rooted by being pushed to the result stack
or returned as the substitution result. No intermediate dangling references are
possible because the iterative traversal maintains a `result_stack` Vec that
holds all intermediate values.

## Summary: GC Root Sources

| Source | Values Reported | When Live |
|--------|----------------|-----------|
| `WamRegisters.args[0..arity]` | Input and decomposed sub-expressions | During execute_wam() |
| `WamRegisters.scratch` | Temporary computation | During execute_wam() |
| `WamBindingFrame.slots[]` | Bound variable values | During execute_wam() |
| `Trail.entries[].previous` | Pre-binding slot values (for undo) | During execute_wam() |
| `WamChoicePoint.results[]` | Results from explored branches | During execute_wam() |
| `WamChoicePoint.alternatives[].rhs_template` | Untried RHS templates | During execute_wam() |
| `WamMatchResult.rhs_info.template` | Matched RHS templates | During + after execute_wam() |
| `WamMatchResult.bindings` | Matched variable values | During + after execute_wam() |
| `Arc<WamCode>.rhs_templates[].template` | All compiled RHS templates | Lifetime of WamCode |

## Invariants

The following invariants are maintained for GC correctness:

**INV-1**: Every `MettaValue` reachable from the WAM state is either:
- An inline value (Unit, Empty, Bool, Long) that does not reference slab memory, or
- A pointer to a slab-allocated `MettaValueInner` with `'static` lifetime

**INV-2**: The trail always contains sufficient entries to restore the binding
frame to any choice point's saved state. No trail entry is discarded while a
choice point that references its mark is still on the stack.

**INV-3**: The `WamState` is stack-allocated and has a strictly shorter lifetime
than the calling trampoline frame. When the trampoline reaches a safepoint, the
`WamState` has already been dropped and its results are reachable through the
trampoline's own root set.

**INV-4**: `Arc<WamCode>` ensures that compiled instructions and RHS templates
outlive all `WamState` instances that reference them.

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
