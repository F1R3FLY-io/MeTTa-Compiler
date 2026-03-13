# Profile-Guided Rule Specialization

MeTTaTron's JIT Stage 2 uses runtime profile data collected during the JIT
Stage 1 profiling window to generate specialized native code for hot rule
dispatch sites. This document describes the profile collection infrastructure,
the specialization analysis engine, the Cranelift code generation strategy, and
the deoptimization mechanism that guarantees correctness when the rule set
changes at runtime.

---

## 1. Tiered Compilation Context

Rule specialization is the distinguishing feature of JIT Stage 2 (Tier 3). It
sits at the top of MeTTaTron's four-tier execution hierarchy:

```
Tier 0: Tree-Walker Interpreter   (0 executions)
          │
          │  count >= 5
          ▼
Tier 1: Bytecode VM               (5+ executions)
          │
          │  count >= 200
          ▼
Tier 2: JIT Stage 1               (200+ executions)
        Generic Cranelift code     Collects RuntimeTypeProfile
          │
          │  count >= 2000
          ▼
Tier 3: JIT Stage 2               (2000+ executions)
        Specialized Cranelift code Profile-guided optimizations
```

The thresholds are defined in `src/backend/bytecode/tiered_cache.rs`:

```rust
pub const BYTECODE_THRESHOLD: u32 = 5;     // Tier 0 -> Tier 1
pub const JIT1_THRESHOLD:     u32 = 200;   // Tier 1 -> Tier 2
pub const JIT2_THRESHOLD:     u32 = 2_000; // Tier 2 -> Tier 3
```

JIT Stage 2 has a hard prerequisite: JIT Stage 1 must be `Ready`. This
ensures that 1,800 executions of profiling data (200 to 2,000) have been
collected before the optimizing compiler makes speculative decisions. This is
analogous to V8's requirement that Maglev must have run before Turbofan.

---

## 2. Profile Collection During JIT Stage 1

### 2.1 The Profiling Window

Between execution counts 200 and 2,000, the expression runs through JIT Stage 1
code that uses `jit_runtime_dispatch_rules_profiling` instead of the standard
`jit_runtime_dispatch_rules`. The profiling variant performs the same rule
dispatch but additionally records match statistics into the
`RuntimeTypeProfile`.

```
Execution Count:
0          5         200                   2000
├──────────┤─────────┤─────────────────────┤──────────────────
  Tree-      Bytecode   JIT Stage 1            JIT Stage 2
  Walker     VM         + Profiling            (Specialized)
                        ◄─── 1800 execs ──►
                          Profile data
                          collected here
```

### 2.2 What Gets Profiled

The profiling variant (`jit_runtime_dispatch_rules_profiling` in
`src/backend/bytecode/jit/runtime/rule_dispatch.rs`) records:

1. **Site identification**: Computes a `site_hash` from the expression's head
   symbol and arity using xxh3.

2. **Per-rule match counts**: For each rule that matches at a dispatch site,
   increments the rule's hit counter and records identity hashes (LHS and RHS)
   for staleness detection.

From `rule_dispatch.rs`:

```rust
unsafe fn record_dispatch_profile(
    ctx_ref: &JitContext,
    expr: &MettaValue,
    rules: &[CompiledRule],
) {
    // Compute site_hash from expression head + arity
    let (head_name, arity) = match expr.as_sexpr() {
        Some(items) if !items.is_empty() => {
            let head = items[0].as_atom().unwrap_or("");
            (head, items.len() as u16)
        }
        _ => return,
    };
    let site_hash = compute_site_hash(head_name, arity);

    // Record each matching rule
    let mut profile = profile_arc.lock();
    let site = find_or_create_dispatch_site(&mut profile, site_hash, head_name, arity);
    for (idx, rule) in rules.iter().enumerate() {
        let lhs_hash = hash_metta_value_quick(&rule.lhs);
        let rhs_hash = /* ... */;
        site.record_match(idx as u16, lhs_hash, rhs_hash, rhs_has_variables);
    }
}
```

The profile data is stored in `ExprCompilationState.runtime_profile`
(`Arc<parking_lot::Mutex<RuntimeTypeProfile>>`) and continues to accumulate
during the entire profiling window.

---

## 3. RuntimeTypeProfile

The `RuntimeTypeProfile` (defined in `src/backend/bytecode/runtime_profile.rs`)
is the central data structure that captures all feedback from lower-tier
execution. It is the MeTTaTron equivalent of V8's `FeedbackVector` or
HotSpot's `MethodData`.

### 3.1 Structure

```rust
pub struct RuntimeTypeProfile {
    pub branch_frequencies: SmallVec<[BranchFeedback; 8]>,
    pub arg_type_feedback:  SmallVec<[ArgTypeFeedback; 8]>,
    pub rule_match_hits:    SmallVec<[RuleMatchFeedback; 8]>,
    pub guard_outcomes:     SmallVec<[GuardFeedback; 4]>,
    pub dispatch_sites:     SmallVec<[RuleDispatchSite; 4]>,
    pub sample_count:       u32,
}
```

### 3.2 Feedback Categories

| Field | V8 Equivalent | HotSpot Equivalent | Purpose |
|-------|---------------|--------------------|---------|
| `branch_frequencies` | BinaryOpIC | BranchData | Per-offset taken/not-taken counts |
| `arg_type_feedback` | CallIC / LoadIC | ReceiverTypeData | Top-2 type histogram per call site argument |
| `rule_match_hits` | (no equivalent) | VirtualCallData | Per-rule match frequency at dispatch sites |
| `guard_outcomes` | TypeGuard feedback | UncommonTrapData | Pass/fail counts for type guards |
| `dispatch_sites` | (no equivalent) | (no equivalent) | Detailed per-site rule dispatch profiles |

### 3.3 Type Tags

The `TypeTag` enum classifies MeTTa runtime values for profiling:

```rust
#[repr(u8)]
pub enum TypeTag {
    Unit   = 0,   Bool   = 1,   Long   = 2,
    Float  = 3,   String = 4,   Atom   = 5,
    SExpr  = 6,   Error  = 7,   Quoted = 8,
    Other  = 9,
}
```

Classification is performed by `TypeTag::from_inner()` which dispatches on
`MettaValueInner` variants in O(1), stripping `Spanned` wrappers.

### 3.4 Branch Feedback

Each `BranchFeedback` entry tracks taken/not-taken counts for a specific
bytecode jump offset using atomic counters:

```rust
pub struct BranchFeedback {
    pub offset: u32,
    pub taken: AtomicU32,
    pub not_taken: AtomicU32,
}
```

A branch is considered biased when one direction exceeds 90% of total
observations:

```rust
pub fn is_biased(&self) -> bool {
    let t = self.taken_count();
    let nt = self.not_taken_count();
    let total = t + nt;
    if total < 10 { return false; }
    t * 10 > total * 9 || nt * 10 > total * 9
}
```

### 3.5 Argument Type Feedback

`ArgTypeFeedback` tracks the top-2 types observed at each call-site argument
position, enabling monomorphic/polymorphic classification:

```rust
pub struct ArgTypeFeedback {
    pub head: String,
    pub arg_index: u8,
    pub primary_type: TypeTag,
    pub primary_count: u32,
    pub secondary_type: Option<TypeTag>,
    pub secondary_count: u32,
}
```

The `record()` method maintains the top-2 invariant: if a new type overtakes
the secondary in count, they are swapped. If it overtakes the primary, both
are promoted/demoted. A site is monomorphic when `secondary_type.is_none()` or
`secondary_count == 0`.

### 3.6 Rule Dispatch Site

The richest profile data is in `RuleDispatchSite`, which captures full
per-rule match frequency at each dispatch location:

```rust
pub struct RuleDispatchSite {
    pub site_hash: u64,
    pub head: String,
    pub arity: u16,
    pub rule_hits: Vec<RuleHit>,
    pub total_dispatches: u32,
}

pub struct RuleHit {
    pub rule_index: u16,
    pub match_count: u32,
    pub lhs_hash: u64,
    pub rhs_hash: u64,
    pub rhs_has_variables: bool,
}
```

A dispatch site has a dominant rule when a single rule handles > 80% of all
dispatches:

```rust
pub fn dominant_rule(&self) -> Option<&RuleHit> {
    self.rule_hits.first().filter(|h| {
        (h.match_count as f64 / self.total_dispatches as f64) > 0.80
    })
}
```

### 3.7 Maturity

A profile is considered mature when `sample_count >= min_samples` (default: 50)
and every branch site has at least `min_samples` total observations. Immature
profiles are rejected by the specializer to avoid premature optimization from
unrepresentative data.

---

## 4. SpecializationPlan

The `SpecializationPlan` (in `src/backend/bytecode/jit/compiler/specializer.rs`)
is the bridge between profiling data and code generation. It is produced by
`analyze_profile()` and consumed by `compile_specialized()`.

### 4.1 Plan Structure

```rust
pub struct SpecializationPlan {
    pub branch_biases:              HashMap<u32, BranchBias>,
    pub monomorphic_sites:          Vec<MonomorphicSite>,
    pub eliminable_guards:          Vec<u16>,
    pub hot_rules:                  Vec<HotRuleHint>,
    pub specialized_dispatch_sites: Vec<SpecializedDispatchSite>,
    pub rule_epoch:                 u64,
    pub quality_score:              f64,
}
```

### 4.2 The `analyze_profile()` Algorithm

The analysis function performs five passes over the profile data:

```
analyze_profile(profile, min_samples=50)
    │
    ├─── Pass 1: Branch Bias Analysis
    │    For each BranchFeedback:
    │      if is_biased() → BranchBias::MostlyTaken or MostlyNotTaken
    │
    ├─── Pass 2: Monomorphic Type Analysis
    │    For each ArgTypeFeedback:
    │      if is_monomorphic() AND primary_count >= 50 → MonomorphicSite
    │
    ├─── Pass 3: Guard Elimination Analysis
    │    For each GuardFeedback:
    │      if never_failed() AND pass_count >= 50 → eliminable guard
    │
    ├─── Pass 4: Hot Rule Analysis
    │    Group rule_match_hits by site_hash
    │    For each site with total >= 50:
    │      if dominant rule fraction > 0.80 → HotRuleHint
    │
    ├─── Pass 5: Dispatch Site Analysis
    │    For each RuleDispatchSite with total >= 50:
    │      if dominant_rule().is_some() → opportunity counted
    │
    └─── Compute quality_score = opportunities / total_sites
```

### 4.3 Quality Score and Worthwhileness

The quality score is a ratio in [0.0, 1.0]:

```
quality_score = min(1.0, opportunities / total_sites)
```

A plan is considered worthwhile if any of the following hold:

```rust
pub fn is_worthwhile(&self) -> bool {
    self.quality_score > 0.1
        || !self.branch_biases.is_empty()
        || !self.monomorphic_sites.is_empty()
        || !self.hot_rules.is_empty()
        || !self.specialized_dispatch_sites.is_empty()
}
```

If `is_worthwhile()` returns false, the tiered cache falls back to generic JIT
Stage 2 compilation (`compile()`) rather than `compile_specialized()`.

### 4.4 Rule Data Extraction

Before the background JIT2 task is spawned, `extract_rule_data_for_specialization()`
runs on the calling thread (which has environment access) to snapshot all rule
patterns and bodies needed by the specializer:

```
extract_rule_data_for_specialization(dispatch_sites, env, rule_epoch)
    │
    ├── For each RuleDispatchSite with total_dispatches >= 50:
    │     ├── Query RuleIndex for candidates at (head, arity)
    │     ├── Sort hits by match frequency (descending)
    │     ├── For each hit:
    │     │     ├── Verify rule identity via LHS hash
    │     │     ├── Extract StructuralMatcher → (checks, var_bindings)
    │     │     └── Package as SpecializedRuleInfo
    │     └── Build SpecializedDispatchSite with coverage fraction
    │
    └── Sort result by total_dispatches (hottest first)
```

Each `SpecializedRuleInfo` contains:

```rust
pub struct SpecializedRuleInfo {
    pub checks: Vec<InlinableCheck>,       // Structural pattern predicates
    pub var_bindings: Vec<InlinableVarOp>, // Variable extraction operations
    pub rhs: MettaValue,                   // Rule body for FFI fallback
    pub rhs_has_variables: bool,           // If false, skip apply_bindings
    pub lhs_hash: u64,                     // Identity tracking
    pub rhs_type: Option<TypeTag>,         // Return type hint
}
```

The LHS hash comparison guards against stale profiles. If a rule has been
modified via `add-atom`/`remove-atom` since the profile was collected, the hash
will mismatch and the stale entry is skipped.

---

## 5. InlinableCheck and InlinableVarOp

These types are the JIT-consumable translations of the tree-walker's
`StructuralMatcher`. They express pattern matching as a sequence of
predicates over S-expression paths that the Cranelift code generator can emit
as inline branch trees.

### 5.1 InlinableCheck

Each check is a predicate that must hold for a pattern to match. The `path`
field is a sequence of child indices that navigates from the root S-expression
to the element being tested.

```rust
pub enum InlinableCheck {
    /// Check that the value at `path` is an S-expression with the given arity.
    Arity { path: Vec<u8>, expected: u16 },

    /// Check that the value at `path` is the atom with the given name.
    Atom { path: Vec<u8>, expected: &'static str },

    /// Check that the value at `path` is the given integer.
    Long { path: Vec<u8>, expected: i64 },

    /// Check that the value at `path` is the given boolean.
    Bool { path: Vec<u8>, expected: bool },

    /// Check that the value at `path` is the given float (bitwise).
    Float { path: Vec<u8>, expected_bits: u64 },

    /// Check that the value at `path` is the given string.
    Str { path: Vec<u8>, expected: &'static str },
}
```

Path example: `[1, 0]` means "navigate to the second child of the root, then
to the first child of that child."

### 5.2 InlinableVarOp

Variable operations either extract a value from the expression tree and store
it in a binding slot, or verify that a repeated variable occurrence matches a
previously-bound value.

```rust
pub enum InlinableVarOp {
    /// Extract value at `path`, store in binding slot `slot_index`.
    Bind {
        path: Vec<u8>,
        name: &'static str,
        slot_index: u8,
    },

    /// Verify value at `path` equals the value in `bind_slot`.
    /// Used when the same variable appears multiple times in a pattern.
    EqualCheck { path: Vec<u8>, bind_slot: u8 },
}
```

### 5.3 Path Navigation in Cranelift IR

The `navigate_path()` helper on `CodegenContext` emits a chain of FFI calls to
`jit_runtime_get_element` that walks from the root expression to the target
sub-expression:

```
navigate_path(expr, [1, 0], fail_block, get_element_ref)
    │
    ├── call get_element_ref(ctx, expr, 1)  → child_1
    │     brif (child_1 != 0) → continue, fail_block
    │
    └── call get_element_ref(ctx, child_1, 0) → child_1_0
          brif (child_1_0 != 0) → continue, fail_block
          return child_1_0
```

If any navigation step fails (index out of bounds for the S-expression), the
generated code branches to `fail_block` (the next rule's entry block).

---

## 6. Specialized Code Generation

### 6.1 Trigger Path

The specialization trigger lives in `TieredCache::maybe_trigger_jit2()`:

```
TieredCache::maybe_trigger_jit2(state, count=2000)
    │
    ├── count >= JIT2_THRESHOLD?             → yes
    ├── bytecode_status == Ready?            → yes
    ├── jit1_status == Ready?                → yes  (ensures profiling data exists)
    ├── jit2_status == NotStarted?           → yes
    ├── CAS(NotStarted → Compiling) won?     → yes
    │
    ├── Snapshot RuntimeTypeProfile
    ├── analyze_profile(snapshot, 50) → plan
    ├── plan.is_worthwhile()?
    │     yes → compile_specialized(chunk, plan)
    │     no  → compile(chunk)    [generic JIT2]
    │
    └── Spawn background task via WorkPool
```

### 6.2 compile_specialized() Overview

`JitCompiler::compile_specialized()` (in `src/backend/bytecode/jit/compiler/mod.rs`)
generates a monolithic Cranelift function that replaces the generic opcode-by-opcode
translation with a specialized dispatch tree:

```
┌─────────────────────────────────────────────────────────────────┐
│  jit_specialized_N(ctx: *mut JitContext) -> i64                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────────────────────────┐                           │
│  │ Entry Block                      │                           │
│  │   Load RULE_EPOCH (atomic)       │                           │
│  │   Compare with expected_epoch    │                           │
│  │   brif match → main_body        │                           │
│  │             → deopt_block        │                           │
│  └─────────┬───────────┬────────────┘                           │
│            │           │                                        │
│   ┌────────▼───┐  ┌────▼──────────┐                             │
│   │ main_body  │  │ deopt_block   │                             │
│   │ pop(expr)  │  │ return        │                             │
│   │ → site_0   │  │ BAILOUT       │                             │
│   └────┬───────┘  └───────────────┘                             │
│        │                                                        │
│   ┌────▼─────────────────────────────────────────────────┐      │
│   │ Site 0: head == "f"? arity == 3?                     │      │
│   │   Rule 0 (95%): [Atom "f", Arity 3, Long @[2]=0]    │      │
│   │     checks pass → eval RHS → push → OK              │      │
│   │   Rule 1 (5%):  [Atom "f", Arity 3, Var $x]         │      │
│   │     checks pass → eval_with_bindings → push → OK    │      │
│   │   Site fallback: FFI dispatch_rules → BAILOUT        │      │
│   └────┬─────────────────────────────────────────────────┘      │
│        │                                                        │
│   ┌────▼─────────────────────────────────────────────────┐      │
│   │ Site 1: head == "g"? arity == 2?                     │      │
│   │   Rule 0 (88%): [Atom "g", Long @[1]=42]            │      │
│   │     checks pass → push constant RHS → OK            │      │
│   │   Site fallback: FFI dispatch_rules → BAILOUT        │      │
│   └────┬─────────────────────────────────────────────────┘      │
│        │                                                        │
│   ┌────▼──────────────────┐                                     │
│   │ Fallback Block        │   No specialized site matched       │
│   │ FFI dispatch_rules    │                                     │
│   │ → BAILOUT             │                                     │
│   └────┬──────────────────┘                                     │
│        │                                                        │
│   ┌────▼──────────────────┐                                     │
│   │ Merge Block           │                                     │
│   │ return signal         │   JIT_SIGNAL_OK or JIT_SIGNAL_BAILOUT│
│   └───────────────────────┘                                     │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 6.3 Deoptimization Guard (Entry Block)

The first thing the specialized function does is verify that the rule set has
not changed since compilation. It loads the global `RULE_EPOCH` and compares
it to the epoch baked into the function as an immediate constant:

```rust
// Load RULE_EPOCH from global atomic
let epoch_addr = &RULE_EPOCH as *const AtomicU64 as u64;
let epoch_ptr_val = builder.ins().iconst(types::I64, epoch_addr as i64);
let current_epoch = builder.ins().load(
    types::I64, MemFlags::trusted(), epoch_ptr_val, 0);
let expected_epoch = builder.ins().iconst(
    types::I64, plan.rule_epoch as i64);
let epoch_ok = builder.ins().icmp(
    IntCC::Equal, current_epoch, expected_epoch);
builder.ins().brif(epoch_ok, main_body, &[], deopt_block, &[]);
```

If the epochs do not match, the function immediately returns
`JIT_SIGNAL_BAILOUT`, causing the `HybridExecutor` to fall back to JIT Stage 1
or bytecode execution. This is a single load + compare + branch: approximately
5 cycles on cache hit.

### 6.4 Dispatch Site Chain

After the epoch guard passes, the generated code pops the expression from the
JIT context's value stack and enters a chain of dispatch site checks. Each
site is ordered by total dispatch frequency (hottest first) so the common case
hits the first site without traversing the chain.

For each site, two FFI calls verify the expression structure:

1. **Head check** (`jit_runtime_check_head`): Compares the expression's head
   atom against the expected symbol. Uses interned-pointer equality first
   (O(1)), falls back to string comparison if pointers differ.

2. **Arity check** (`jit_runtime_get_arity_fast`): Retrieves the S-expression
   length and compares against the expected arity.

If either check fails, execution falls through to the next site in the chain.

### 6.5 Inline Rule Matching

Within a dispatch site, each hot rule's pattern checks are emitted as inline
Cranelift IR, ordered by match frequency (hottest first as fall-through):

```
rules_block:
    │
    ├── Rule 0 (hottest):
    │     navigate_path(expr, [1]) → child_1
    │     check_atom_eq(child_1, "Impl")  → pass / fail → Rule 1
    │     navigate_path(expr, [2]) → child_2
    │     bind(child_2, $TV, slot 0)
    │     All checks passed → evaluate RHS
    │
    ├── Rule 1:
    │     navigate_path(expr, [1]) → child_1
    │     check_long_eq(child_1, 0)       → pass / fail → site_fallback
    │     All checks passed → push constant RHS
    │
    └── site_fallback:
          FFI dispatch_rules → BAILOUT
```

Each `InlinableCheck` variant generates specific Cranelift IR:

| Check Variant | Generated IR |
|---------------|-------------|
| `Arity { path, expected }` | `navigate_path` + `get_arity_fast` + `icmp(eq, arity, expected)` |
| `Atom { path, expected }` | `navigate_path` + `check_atom_eq` (pointer equality via `band + icmp`) |
| `Long { path, expected }` | `navigate_path` + `check_long_eq` (NaN-box tag + payload comparison) |
| `Bool { path, expected }` | `navigate_path` + `check_bool_eq` (NaN-box exact comparison) |
| `Float { path, expected_bits }` | `navigate_path` + `check_float_eq` (bitwise comparison) |
| `Str { path, expected }` | Not yet inlined -- jumps to `next_rule_block` |

### 6.6 Variable Binding Extraction

After all structural checks pass, variable bindings are extracted. Each
`InlinableVarOp::Bind` emits:

1. `navigate_path()` to reach the variable's position in the expression tree
2. Compute the variable name's xxh3 hash as an `iconst`
3. Store the `(name_hash, value)` pair into the `bind_slots` vector

`InlinableVarOp::EqualCheck` verifies that a repeated variable occurrence
matches the already-bound value using an `icmp(eq)`. On mismatch, the code
branches to the next rule.

### 6.7 RHS Evaluation

After pattern matching and binding extraction succeed, the RHS is evaluated.
There are three cases:

**Case 1: Variable-free RHS** -- The RHS is a constant value. The function
encodes the RHS's `inner_ptr()` as a `TAG_PTR` NaN-boxed value and pushes it
directly onto the JIT context's value stack. No FFI call is needed.

```rust
if !rule.rhs_has_variables {
    let inner_ptr = rule.rhs.inner_ptr() as u64;
    let jit_val = TAG_PTR | (inner_ptr & PAYLOAD_MASK);
    let result_val = builder.ins().iconst(types::I64, jit_val as i64);
    builder.ins().call(stack_push_ref, &[ctx_ptr, result_val]);
    // jump merge_block with JIT_SIGNAL_OK
}
```

**Case 2: RHS with variables, bindings extracted** -- The `(name_hash, value)`
pairs are spilled to a Cranelift stack slot and passed to
`jit_runtime_eval_with_bindings` via FFI. That runtime function reconstructs a
`Bindings` map, calls `apply_bindings()`, and returns the substituted result
as a NaN-boxed value.

```rust
else if !bind_slots.is_empty() {
    // Allocate Cranelift stack slot for binding pairs
    let slot_size = (num_bindings * 16) as u32;
    let stack_slot = builder.create_sized_stack_slot(/* ... */);

    // Store each (hash, value) pair
    for i in 0..num_bindings {
        builder.ins().stack_store(hash_val, stack_slot, offset);
        builder.ins().stack_store(val, stack_slot, offset + 8);
    }

    // Call eval_with_bindings(ctx, rhs_ptr, bindings_addr, count)
    let result = builder.ins().call(eval_with_bindings_ref, &[...]);
    builder.ins().call(stack_push_ref, &[ctx_ptr, result]);
    // jump merge_block with JIT_SIGNAL_OK
}
```

**Case 3: No bindings extracted** -- Falls through to the site-level FFI
fallback (`jit_runtime_dispatch_rules`).

### 6.8 Fallback Paths

Two fallback paths ensure correctness when specialization cannot handle an
expression:

1. **Site fallback**: When a dispatch site's inline rules all fail their pattern
   checks, the code pushes the expression back onto the stack and calls the
   generic `jit_runtime_dispatch_rules` FFI function. The function returns
   `JIT_SIGNAL_BAILOUT` to let the VM handle the matched rules.

2. **Global fallback**: When no specialized dispatch site matches (head/arity
   mismatch for all sites), the code similarly falls back to the FFI dispatch.

---

## 7. Runtime FFI Functions for Specialization

Four FFI functions were added specifically for JIT Stage 2 specialization. They
are registered via `RulesInit::register_rules_symbols()` and declared via
`RulesInit::declare_rules_funcs()`.

### 7.1 RulesFuncIds (Phase 9 Additions)

```rust
pub struct RulesFuncIds {
    // ... standard dispatch functions ...

    /// Profile-collecting variant of dispatch_rules
    pub dispatch_rules_profiling_func_id: FuncId,
    /// Fast head symbol check for specialized dispatch
    pub check_head_func_id: FuncId,
    /// Fast arity check for specialized dispatch
    pub get_arity_fast_func_id: FuncId,
    /// Evaluate rule body with pre-extracted bindings
    pub eval_with_bindings_func_id: FuncId,
}
```

### 7.2 Function Signatures

| Function | Signature | Purpose |
|----------|-----------|---------|
| `jit_runtime_dispatch_rules_profiling` | `fn(ctx, expr, ip) -> count` | Same as `dispatch_rules` but records profile data |
| `jit_runtime_check_head` | `fn(ctx, expr, name_ptr, name_len) -> 0/1` | Fast head symbol comparison |
| `jit_runtime_get_arity_fast` | `fn(ctx, expr) -> arity` | Fast S-expression length query |
| `jit_runtime_eval_with_bindings` | `fn(ctx, rhs_ptr, bindings_ptr, count) -> result` | Apply bindings and evaluate RHS |

### 7.3 check_head: Interned Pointer Optimization

`jit_runtime_check_head` exploits MeTTaTron's atom interning: if two atom
strings share the same address, they are guaranteed identical. The function
first compares data pointers and lengths. Only on pointer mismatch does it
fall back to byte-by-byte string comparison:

```rust
// Pointer equality first (interned atoms share addresses)
if std::ptr::eq(head_name.as_ptr(), expected.as_ptr())
    && head_name.len() == expected.len()
{
    return 1;
}
// Fall back to string comparison
if head_name == expected {
    return 1;
}
```

---

## 8. Deoptimization

### 8.1 RULE_EPOCH

The global rule epoch counter (`src/backend/environment/rule_management.rs`)
is the cornerstone of specialization safety:

```rust
pub static RULE_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn increment_rule_epoch() {
    RULE_EPOCH.fetch_add(1, Ordering::Release);
}
```

The epoch is incremented on every mutation that could change the rule set:
`add_rule()`, `add_type_generic()`, `remove_type_generic()`, and any
`add-atom`/`remove-atom` that affects the rule index.

### 8.2 DeoptimizationGuard

Each specialized function has an associated `DeoptimizationGuard` stored in
`ExprCompilationState`:

```rust
pub struct DeoptimizationGuard {
    pub expected_epoch: u64,
    pub chunk_hash: u64,
}

impl DeoptimizationGuard {
    #[inline]
    pub fn is_valid(&self) -> bool {
        RULE_EPOCH.load(Ordering::Acquire) == self.expected_epoch
    }
}
```

### 8.3 Dual-Layer Epoch Check

The epoch is checked at two levels:

**Layer 1: Before JIT2 dispatch (Rust side)** -- `TieredCache::get_best_tier()`
calls `state.check_deopt_guard()` before returning `JitStage2`. If the guard
fails, it calls `state.invalidate_jit2()` and returns `JitStage1` instead:

```rust
if state.jit2_status() == TierStatusKind::Ready {
    if state.check_deopt_guard() {
        return ExecutionTier::JitStage2;
    }
    // Deopt guard failed -- rules changed since JIT2 compilation.
    state.invalidate_jit2();
}
```

**Layer 2: Inside the specialized function (Cranelift IR)** -- The first basic
block loads `RULE_EPOCH` and compares it to the baked-in expected epoch. This
catches races where the epoch changes between the Rust-side check and function
entry.

### 8.4 invalidate_jit2()

When the deoptimization guard fails, `invalidate_jit2()` performs three
operations:

```rust
pub fn invalidate_jit2(&self) {
    // 1. Reset JIT2 status: CAS(Ready -> NotStarted)
    self.jit2_status.compare_exchange(
        TierStatusKind::Ready as u8,
        TierStatusKind::NotStarted as u8,
        Ordering::AcqRel,
        Ordering::Relaxed,
    ).ok();

    // 2. Clear the stale deopt guard
    *self.deopt_guard.lock() = None;

    // 3. Reset the runtime profile for fresh data collection
    let mut profile = self.runtime_profile.lock();
    *profile = RuntimeTypeProfile::new();
}
```

This allows the expression to re-enter the profiling window, collect fresh
data under the new rule configuration, and eventually trigger a new JIT Stage 2
compilation with an updated specialization plan.

### 8.5 Invalidation Lifecycle

```
                          RULE_EPOCH incremented
                          (add-atom / remove-atom)
                                  │
                                  ▼
┌─────────────────────┐    ┌──────────────────────────┐
│ JIT Stage 2 Running │    │ check_deopt_guard()      │
│ (specialized code)  │───►│ expected_epoch != current │
└─────────────────────┘    └──────────┬───────────────┘
                                      │
                                      ▼
                           ┌──────────────────────────┐
                           │ invalidate_jit2():        │
                           │   CAS(Ready → NotStarted) │
                           │   Clear deopt guard       │
                           │   Reset RuntimeTypeProfile │
                           └──────────┬───────────────┘
                                      │
                                      ▼
                           ┌──────────────────────────┐
                           │ Falls back to JIT Stage 1 │
                           │ (profile-collecting)      │
                           └──────────┬───────────────┘
                                      │
                                      │ 2000 more executions
                                      ▼
                           ┌──────────────────────────┐
                           │ New JIT Stage 2 compiled  │
                           │ with fresh profile data   │
                           │ and new rule_epoch        │
                           └──────────────────────────┘
```

---

## 9. Worked Example: PLN Rule Specialization

Consider a PLN (Probabilistic Logic Network) knowledge base with these rules:

```metta
(= (init-sentence ((Implication $A $B) $TV) $Y)
   (build-link "ImplicationLink" $A $B $TV $Y))

(= (init-sentence ($C $TV) $Y)
   (build-link "ConceptNode" $C $C $TV $Y))
```

During execution, the first rule matches 92% of dispatches and the second
matches 8%.

### Step 1: Profile Collection (executions 200-2000)

The profiling variant records:

```
RuleDispatchSite {
    site_hash: xxh3("init-sentence", arity=3),
    head: "init-sentence",
    arity: 3,
    rule_hits: [
        RuleHit { rule_index: 0, match_count: 1656, lhs_hash: 0xA1B2..., ... },
        RuleHit { rule_index: 1, match_count: 144,  lhs_hash: 0xC3D4..., ... },
    ],
    total_dispatches: 1800,
}
```

### Step 2: Profile Analysis (at execution 2000)

`analyze_profile()` identifies:
- 1 hot rule hint: rule 0 at site `xxh3("init-sentence", 3)` with
  match_fraction = 1656/1800 = 0.92 (> 0.80 threshold)

`extract_rule_data_for_specialization()` extracts rule 0's structural matcher:

```
SpecializedRuleInfo {
    checks: [
        Arity { path: [],   expected: 3 },     // (init-sentence _ _ _)
        Arity { path: [1],  expected: 2 },      // arg1 is a pair
        Arity { path: [1,0], expected: 3 },     // arg1[0] is (Implication $A $B)
        Atom  { path: [1,0,0], expected: "Implication" },
    ],
    var_bindings: [
        Bind { path: [1,0,1], name: "$A",  slot_index: 0 },
        Bind { path: [1,0,2], name: "$B",  slot_index: 1 },
        Bind { path: [1,1],   name: "$TV", slot_index: 2 },
        Bind { path: [2],     name: "$Y",  slot_index: 3 },
    ],
    rhs: (build-link "ImplicationLink" $A $B $TV $Y),
    rhs_has_variables: true,
    lhs_hash: 0xA1B2...,
    rhs_type: Some(TypeTag::SExpr),
}
```

Rule 1 also gets extracted (match_count 144 >= 50 threshold).

### Step 3: Specialization Plan

```
SpecializationPlan {
    specialized_dispatch_sites: [
        SpecializedDispatchSite {
            site_hash: 0x7F3A...,
            head: "init-sentence",
            arity: 3,
            inline_rules: [rule_0_info, rule_1_info],
            total_dispatches: 1800,
            coverage: 1.0,
        }
    ],
    rule_epoch: 47,
    quality_score: 1.0,
}
```

### Step 4: Cranelift Code Generation

`compile_specialized()` generates:

```
jit_specialized_42(ctx):
    ┌──────────────────────────────────────────────────┐
    │ ENTRY:                                           │
    │   current_epoch = load(&RULE_EPOCH)              │
    │   brif (current_epoch == 47) → MAIN, DEOPT      │
    └──────────────────────┬───────────────┬───────────┘
                           │               │
                 ┌─────────▼─────┐  ┌──────▼──────┐
                 │ MAIN:         │  │ DEOPT:      │
                 │ expr = pop()  │  │ return -3   │
                 │ → SITE_0      │  │ (BAILOUT)   │
                 └───────┬───────┘  └─────────────┘
                         │
                 ┌───────▼─────────────────────────────┐
                 │ SITE_0:                              │
                 │ check_head(expr, "init-sentence")==1?│
                 │   no → FALLBACK                     │
                 │ get_arity(expr) == 3?                │
                 │   no → FALLBACK                     │
                 └───────┬─────────────────────────────┘
                         │
                 ┌───────▼─────────────────────────────┐
                 │ RULE_0 (92%):                        │
                 │                                      │
                 │ // Check arg1 is a 2-element pair    │
                 │ child_1 = get_element(expr, 1)       │
                 │ get_arity(child_1) == 2?             │
                 │   no → RULE_1                        │
                 │                                      │
                 │ // Check arg1[0] is (Implication _ _) │
                 │ child_1_0 = get_element(child_1, 0)  │
                 │ get_arity(child_1_0) == 3?           │
                 │   no → RULE_1                        │
                 │ child_1_0_0 = get_element(child_1_0, 0)│
                 │ check_atom_eq(child_1_0_0, "Implication")│
                 │   no → RULE_1                        │
                 │                                      │
                 │ // Extract bindings                  │
                 │ $A  = get_element(child_1_0, 1)      │
                 │ $B  = get_element(child_1_0, 2)      │
                 │ $TV = get_element(child_1, 1)        │
                 │ $Y  = get_element(expr, 2)           │
                 │                                      │
                 │ // Store bindings to stack slot       │
                 │ stack_store(hash("$A"),  slot, 0)    │
                 │ stack_store($A,          slot, 8)    │
                 │ stack_store(hash("$B"),  slot, 16)   │
                 │ stack_store($B,          slot, 24)   │
                 │ stack_store(hash("$TV"), slot, 32)   │
                 │ stack_store($TV,         slot, 40)   │
                 │ stack_store(hash("$Y"),  slot, 48)   │
                 │ stack_store($Y,          slot, 56)   │
                 │                                      │
                 │ result = eval_with_bindings(ctx,      │
                 │           rhs_ptr, slot_addr, 4)     │
                 │ push(result)                          │
                 │ → MERGE(OK)                           │
                 └───────┬─────────────────────────────┘
                         │ (pattern check failed)
                 ┌───────▼─────────────────────────────┐
                 │ RULE_1 (8%):                         │
                 │ // Less specific: ($C $TV) pattern   │
                 │ child_1 = get_element(expr, 1)       │
                 │ get_arity(child_1) == 2?             │
                 │   no → SITE_FALLBACK                 │
                 │ $C  = get_element(child_1, 0)        │
                 │ $TV = get_element(child_1, 1)        │
                 │ $Y  = get_element(expr, 2)           │
                 │ result = eval_with_bindings(ctx,      │
                 │           rhs_ptr_1, slot_addr, 3)   │
                 │ push(result)                          │
                 │ → MERGE(OK)                           │
                 └───────┬─────────────────────────────┘
                         │ (pattern check failed)
                 ┌───────▼─────────────────────────────┐
                 │ SITE_FALLBACK:                       │
                 │ push(expr)                            │
                 │ dispatch_rules(ctx, expr, 0)         │
                 │ → MERGE(BAILOUT)                      │
                 └─────────────────────────────────────┘
```

### Step 5: Performance Impact

The specialized code eliminates the following overhead per dispatch:

| Operation | Generic JIT1 | Specialized JIT2 |
|-----------|-------------|-----------------|
| MORK trie traversal | O(n) per rule | Eliminated (inline checks) |
| Rule candidate collection | Vec allocation | Eliminated |
| Pattern matching | Full unification | 4 inline branch checks |
| Variable binding | HashMap operations | Direct SSA values |
| RHS lookup | Indirect via rule index | Baked-in `iconst` pointer |
| Deoptimization check | None | 1 load + 1 compare (~5 cycles) |

For the 92% hot path (Rule 0), the dominant cost becomes the 4
`get_element` FFI calls plus the `eval_with_bindings` call. The structural
checks (atom equality, arity comparison) are single-instruction operations
after the FFI returns.

---

## 10. Summary of Source Locations

| Component | File |
|-----------|------|
| Specialization engine | `src/backend/bytecode/jit/compiler/specializer.rs` |
| `compile_specialized()` | `src/backend/bytecode/jit/compiler/mod.rs` (line ~2464) |
| Runtime type profile | `src/backend/bytecode/runtime_profile.rs` |
| JIT2 trigger | `src/backend/bytecode/tiered_cache.rs` (`maybe_trigger_jit2()`, line ~1294) |
| Deoptimization guard check | `src/backend/bytecode/tiered_cache.rs` (`check_deopt_guard()`, `invalidate_jit2()`) |
| Rule dispatch FFI | `src/backend/bytecode/jit/runtime/rule_dispatch.rs` |
| Rule FFI declarations | `src/backend/bytecode/jit/compiler/init/rules.rs` |
| RULE_EPOCH | `src/backend/environment/rule_management.rs` |
