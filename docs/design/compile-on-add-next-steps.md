# Sub-1s PLN Robot: Next Steps

## Current State: 1.4s ± 0.1s (target <1.0s)

### What's Done
- Compile-on-add: rule RHS bodies pre-compiled at `add_rule` time → `RuleEntry.compiled_rhs`
- VM compiled RHS execution via call frame switching in `op_dispatch_rules`
- Jump-based case compilation in generic compiler (Dup/MatchBind/JumpIfFalse/Jump)
- Native collapse with zero-overhead `in_collapse_scope` compiler flag
- Pre-compiled built-in bytecode registry (25 operations in `builtin_chunks.rs`)
- Conditional subgoal/thunk table dirty flags
- `add-atom` whitelisted in `can_compile_with_env`
- TieredCache caching for environment-aware bytecode
- WFST-gated parallelism, speculative matching fix, structural sharing
- VM nondeterminism infrastructure: yield_on_return, saved_unreduced, resume_alternatives (disabled — see Path A)
- `inferred_fn_types` DashMap → `Arc<DashMap>` for O(1) fork (never written during evaluation)
- Literal Bool `if` condition inline in trampoline (skip continuation + work item)

### Performance History

| Change | PLN Robot |
|--------|-----------|
| Bloom filter + freshening + Unit fix | ∞ → 4.17s |
| FxHashMap + SmallVec trie | 4.17s → 3.70s |
| Deep WFST + env fix + speculative fix | 3.70s → 1.33s |
| Bytecode tier + compile-on-add | 1.33s → 1.45s (infrastructure) |
| Jump-based case + native collapse + built-in registry | 1.45s → 1.35s |
| yield_on_top_return + can_compile_with_env widening + Arc<DashMap> fork + Bool if inline | 1.35s → 1.37s (lower variance, -5% CPU) |

---

## Critical Profiling Findings (2026-03-25)

### DTrace Profile: `/tmp/dtrace_robot.out` (109K lines)
### Trace: `/var/tmp/Robot.mtrace` (930MB, analyzed with `target/release/trace-analyzer`)

### Finding 1: Bytecode VM is 0.004% utilized
```
TreeWalker: 922,144 events (99.996%)
BytecodeVM: 38 events (0.004%)
```

**Root cause**: The bytecode VM IS tried eagerly for every expression passing `can_compile_with_env` (lines 294/496 in `eval/mod.rs`). However, **nondeterministic rule dispatch** causes `has_choices=true` in the VM result, triggering mandatory fallback to the tree-walker. The `has_choices` guard CANNOT be dropped — doing so breaks 8 nondeterminism tests.

The 38 successful bytecode events are expressions where DispatchRules found exactly 0 or 1 matching rule (no choice points created).

### Finding 2: DTrace CPU Hotspots (42K total samples)

| Function | Samples | % | Category |
|----------|---------|---|----------|
| eval_trampoline_generic (ParallelBranch) | 7,405 | 18% | Trampoline loop |
| eval_trampoline_inner (ParallelBranch) | 7,279 | 17% | Trampoline loop |
| SmallVec::drop (MettaTrie keys) | 3,072 | 7% | Memory mgmt |
| Arc::drop_slow (MettaTrieNode) | 3,052 | 7% | Memory mgmt |
| apply_bindings_generic | 2,979 | 7% | Binding application |
| apply_bindings_generic_inner | 2,490 | 6% | Binding application |
| process_continuation_generic (Session+Branch) | 3,738 | 9% | Continuation processing |
| dispatch_rule_matches | 1,464 | 3.5% | Rule dispatch |
| hash_value_cached_inner | 1,029 | 2.5% | Expression hashing |
| drop_in_place (DiscriminationTree) | 1,012 | 2.4% | Memory mgmt |
| eval_sexpr_step_generic_inner | 617 | 1.5% | S-expr evaluation |
| add_to_space | 689 | 1.6% | Space operations |
| SlabAllocator::alloc_value | 465 | 1.1% | Allocation |

### Finding 3: Amdahl's Law Limit
```
Sequential fraction: 54%
Parallel fraction:   46%
Amdahl's limit:      1.8x speedup with unlimited cores
Thread 0:            11.2s (54% of total compute)
```

### Finding 4: Trace Event Breakdown
```
SpecialForm:           543,205 (58.9%)  — dominated by let* (~192K), if (~13.4K)
RuleApplication:       227,349 (24.7%)
RuleMatchSet:           49,653 (5.4%)
NondeterministicFork:   12,829 (1.4%)
Max eval depth:             248
Fork nesting depth:     up to 299
Empty branches:          50% (935/1870)
```

---

## Two Optimization Paths for Sub-1s

### Path A: VM-Internal Nondeterministic Evaluation

**Status: INVESTIGATED — runtime approaches produce regressions. Requires compiler-level changes.**

**Goal**: Make the bytecode VM exhaust all choice points before returning, so `has_choices` is always `false` and the tree-walker fallback is eliminated.

#### Approaches Investigated (2026-03-25)

**Approach A1: yield_on_return call frame flag**
- Added `yield_on_return: bool` to `GenericCallFrame`. When set, `op_return` yields the result to `self.results` and calls `op_fail` to backtrack.
- **Problem**: `call_stack.is_empty()` cannot distinguish outermost vs intermediate DispatchRules within a single chunk. Intermediate dispatches (e.g., `(a)` inside `(+ (a) (b))`) incorrectly yield results, short-circuiting the rest of the chunk.
- **Result**: 6 nondeterminism tests fail — intermediate dispatch results are yielded instead of flowing back to the calling chunk.

**Approach A2: resume_alternatives() loop in eval_inner**
- After `vm.run()` returns with `has_choices=true`, repeatedly call `vm.op_fail()` + `vm.run()` to exhaust remaining alternatives. Each resume re-executes the full chunk from the dispatch point.
- **Correctness**: All 3681 tests pass, including Cartesian product and nested nondeterminism.
- **Performance**: 1.557s ± 0.040s vs 1.35s baseline = **~15% regression**. The resume loop re-executes shared computation (e.g., sub-expression evaluation) for each alternative.

**Approach A3: Compiled RHS in multi-match (RuleMatch alternatives)**
- Downcast `compiled_rhs` for each match and store as `RuleMatch { chunk, bindings }`.
- **Problem**: The per-match `Arc::clone().downcast()` + `bindings.clone()` adds measurable overhead even when falling back to tree-walker.
- **Result**: 1.525s vs 1.35s = **~13% regression** just from the all_compiled check.

#### Root Cause Analysis

The tree-walker's continuation-based nondeterminism is fundamentally more efficient than the VM's backtracking approach for expressions with shared sub-computations:

- **Tree-walker**: Forks evaluation at the nondeterminism point, each branch carries a continuation (rest of the computation). Shared sub-expressions before the fork are evaluated once.
- **VM resume loop**: Re-executes the entire chunk from the dispatch point for each alternative, redundantly re-evaluating shared sub-expressions.

#### What's Needed: Compiler-Level Nondeterminism

To make VM nondeterminism faster than the tree-walker, the **bytecode compiler** must emit inline Fork/Yield/Fail sequences that:
1. Evaluate shared sub-expressions ONCE (before the fork point)
2. Fork into per-alternative sub-chunks for the divergent parts
3. Collect results via Yield without re-executing shared code

This is analogous to how `superpose` already works — it's compiled into Fork/Yield/Fail opcodes. The compiler would need to recognize multi-match DispatchRules at compile time and emit similar sequences.

#### Infrastructure Preserved

The following infrastructure was added and is available for future compiler-level work:
- `GenericCallFrame.yield_on_return: bool` — auto-yield on frame return
- `GenericChoicePoint.saved_unreduced: bool` — prevents flag pollution across alternatives
- `GenericBytecodeVM::resume_alternatives()` — exhausts remaining choice points via op_fail+run loop
- `GenericAlternative::RuleMatch { chunk, bindings }` — compiled RHS alternatives in op_fail

**Key files**:
- `src/backend/bytecode/vm/types.rs` — yield_on_return, saved_unreduced fields
- `src/backend/bytecode/vm/mod.rs` — resume_alternatives(), op_fail RuleMatch handler with call frames
- `src/backend/eval/mod.rs` — has_choices guard (unchanged; resume loop available but disabled)

---

### Path B: Tree-Walker Hot Loop Optimization

**Goal**: Make the tree-walker itself faster for the expressions that must remain in tree-walker mode.

**Hotspot 1: MettaTrie/Arc drops (14% of CPU)**

`Arc::drop_slow` (3,052 samples) + `SmallVec::drop` for trie keys (3,072 samples) + `drop_in_place(DiscriminationTree)` (1,012 samples) = 7,136 samples total.

This suggests heavy allocation/deallocation of trie data structures during evaluation. Potential causes:
- CoW environment cloning triggers deep copies of the discrimination tree
- Rule matching creates temporary trie structures that are dropped after each match
- `add_to_space` (689 samples) decomposes values into trie keys

**Optimization approaches**:
1. Arc-wrap `DiscriminationTree` so cloning is O(1) ref-count bump
2. Use deferred/batched drops outside the hot evaluation loop
3. Arena-allocate trie keys instead of SmallVec heap allocation

**Key files**:
- `src/backend/eval/cesk/discrimination_tree.rs` — DiscriminationTree struct
- `src/backend/environment/generic.rs` — `make_owned()` CoW deep-copy (line 341)
- MettaTrie in the external `metta-trie` crate

**Hotspot 2: apply_bindings_generic (13% of CPU)**

Already well-optimized with:
- O(1) `has_variables_fast()` skip (line 116)
- Per-child fast path (line 138)
- Identity short-circuit (line 150)
- SmallVec<[V; 8]> stack allocation (line 132)

The 13% comes from sheer volume (227K rule applications). Remaining opportunity:
- Avoid iterating children when bindings and value's variable set don't overlap
- Pre-compute a variable bitmask on the RHS at rule insertion time, compare against binding keys

**Hotspot 3: Trampoline loop overhead (44% of CPU)**

`eval_trampoline_inner` + `process_continuation_generic` dominate CPU. These are the core evaluation loop — each step is a work item popped from a Vec stack.

**Optimization approaches**:
1. Inline common `let*` single-binding case to avoid work stack push/pop
2. Inline `if` with Bool condition to avoid continuation creation
3. Pre-allocate trampoline stacks via thread-local pooling (generic type complicates this)
4. Reduce continuation chain depth for tail-call patterns

**Key files**:
- `src/backend/eval/trampoline/generic_trampoline.rs` — main loop (line 1288), continuation processing
- `src/backend/eval/trampoline/generic_engine.rs` — `apply_bindings_generic_inner` (line 92)

---

## Invalidated Approaches (DO NOT RETRY)

| Approach | Result | Why |
|----------|--------|-----|
| Normal-form skip for bytecode results | +6-16% regression | PLN results are mostly reducible S-exprs; check cost > savings |
| DispatchRules `is_normal_form_bounded` early exit | Infinite loop | `may_have_rules_for` bloom filter and `rule_index` disagree |
| Drop `has_choices` guard in eval_inner | 8 test failures | Nondeterministic evaluation requires all alternatives |
| `if-reducible` whitelisting | 3 test failures | Lazy argument semantics incompatible with eager bytecode compilation |
| Modifying `op_fail` for collapse barriers | +90% regression | Per-call overhead in hot backtracking path |
| yield_on_return call frame flag | 6 test failures | `call_stack.is_empty()` cannot distinguish outermost vs intermediate DispatchRules in a chunk |
| resume_alternatives() loop in eval_inner | +15% regression | Re-executes full chunk per alternative; tree-walker continuations avoid redundant work |
| Compiled RHS multi-match (RuleMatch alt) | +13% regression | Arc::clone().downcast() + bindings.clone() per match exceeds savings |

---

## Profiling Commands

```bash
# Rebuild with trace support
cargo build --profile release-traced --features eval-trace

# Run with tracing
METTA_MODULE_PATH=../PLN/src/ ./target/release-traced/mettatron ../PLN/examples/Robot.metta \
  --trace /var/tmp/Robot.mtrace

# Analyze trace
target/release/trace-analyzer stats /var/tmp/Robot.mtrace
target/release/trace-analyzer hotpath /var/tmp/Robot.mtrace
target/release/trace-analyzer bottlenecks /var/tmp/Robot.mtrace
target/release/trace-analyzer critical-path /var/tmp/Robot.mtrace
target/release/trace-analyzer lint /var/tmp/Robot.mtrace
target/release/trace-analyzer fanout /var/tmp/Robot.mtrace
target/release/trace-analyzer redundancy /var/tmp/Robot.mtrace

# DTrace CPU profile (macOS)
sudo dtrace -x ustackframes=100 -n 'profile-997 /execname == "mettatron"/ { @[ustack()] = count(); }' \
  -c 'env METTA_MODULE_PATH=../PLN/src/ ./target/release/mettatron ../PLN/examples/Robot.metta' \
  -o /tmp/dtrace_robot.out

# Benchmark
hyperfine --warmup 3 --runs 10 \
  "METTA_MODULE_PATH=../PLN/src/ ./target/release/mettatron ../PLN/examples/Robot.metta"
```
