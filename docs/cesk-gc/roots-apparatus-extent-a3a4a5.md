# CESK GC Roots Apparatus Inventory: Precise Code Extent Map (A3/A4/A5)

**Branch**: `feature/petta-semantics` (HEAD ~9cbb2ea)  
**Date**: 2026-05-29  
**Scope**: Exact file:line citations + comprehensive call-site enumeration for A3 (env factorization), A4 (VM-as-K-leaf + machine reader), A5 (apparatus deletion)

---

## A3: Environment in Continuations (E₀ vs E_local factorization)

### SharedEnv Type Definition
- **File**: `src/backend/eval/trampoline/context.rs`
- **Line**: (single line definition)
```rust
pub type SharedEnv = std::sync::Arc<MettaEnvironment>;
```

### Continuation Enum: env Field Count & Cardinality

**File**: `src/backend/eval/trampoline/types.rs`  
**Lines**: ~50–3230 (enum definition)

**Total Continuation variants**: **78** (enumerated at line ~3000+)

**env: SharedEnv field count**: **73 occurrences** (all at field level)

All env fields appear to be **the SAME shared Arc** through normal function parameters (fork_for_nondeterminism uses Arc::clone). Evidence:
- Env is passed from caller to called rule-dispatch, then clone/propagated to child WorkItems
- No CoW or per-frame variants observed (env fork happens via global Arc::clone in rule_management)
- **Conclusion for A3**: The env IS already effectively E₀ (single shared Arc); A3 refactor would merely extract it as a formal persistent root, not change behavior

### Continuation.collect_values() env Handling

**File**: `src/backend/eval/trampoline/types.rs`  
**Lines**: 1817–2300+ (impl Continuation, method at ~1817)

**Key finding**: The `env: SharedEnv` fields in Continuation variants are **NOT walked by `collect_values()`**. 
- Example pattern at line 1835+:
  ```rust
  Self::CollectSExpr {
      remaining, collected, outer_carrying, ..
  } => {
      out.extend(remaining.as_slice().iter().copied());
      for (vals, _env) in collected { ... }  // _env ignored
      collect_bindings_values(outer_carrying, out);
  }
  ```
- Env roots are ONLY collected via `RootProvider` registry (GenericEnvironmentShared<MettaValue>::collect_roots)

**Implication**: A3 does NOT require changing Continuation.collect_values; env is already invisible to the S/C/K root walk (rooted externally via registry).

---

## A4: VM Root Surface (VM-as-K-leaf + Structural Reader)

### VM collect_roots_into() Definition & Signature

**File**: `src/backend/bytecode/vm/mod.rs`  
**Lines**: 1040–1150+ (method definition)

```rust
pub(crate) fn collect_roots_into(&self, out: &mut Vec<V>) {
    // Walks 16 major V-bearing field groups:
    // 1. chunk constant pools
    // 2. value_stack: Vec<V>
    // 3. locals: Vec<V>
    // 4. results: Vec<V>
    // 5. expected_type: Option<V>
    // 6. current_bindings: GenericBindings<V>
    // 7. bindings_stack: Vec<GenericBindingFrame<V>>
    // 8. call_stack[].{return_chunk, saved_bindings}
    // 9. choice_points[].{chunk, alternatives}
    // 10. collapse_bind_frames[].{chunks, saved_results}
    // 11. per_result_bindings: Vec<GenericBindings<V>>
    // 12. dispatch_memo: HashMap<...>
    // 13. trail: Rebinding.old_value
    // ... (+ 3 more minor groups for GroundedState, etc.)
}
```

**Deliberately SKIPS** (lines 1045–1048 docstring):
- `env`: registered separately via `try_register_env_roots` (RootProvider)
- `native_registry` / `external_registry`: function pointers, no V values
- `memo_cache`: registered separately as `MemoCacheRoots` (RootProvider)

### with_vm_roots_frame() Implementation

**File**: `src/backend/bytecode/vm/mod.rs`  
**Lines**: 1181–1210

```rust
fn with_vm_roots_frame(&self) -> Option<EvalFrameGuard> {
    // TypeId gate: only for V == MettaValue
    if TypeId::of::<V>() != TypeId::of::<MettaValue>() {
        return None;
    }
    // Push custom frame with vm_roots_collector function
    Some(unsafe {
        EvalFrameGuard::push_custom(
            FrameLabel::BytecodeVm,
            self as *const Self as *const (),
            vm_roots_collector,  // Type-erased closure
        )
    })
}
```

### vm_roots_collector() Function

**File**: `src/backend/bytecode/vm/mod.rs`  
**Line**: 144

```rust
unsafe fn vm_roots_collector(data: *const (), out: &mut Vec<MettaValue>) {
    let vm = unsafe { &*(data as *const GenericBytecodeVM<MettaValue, ActiveFactory>) };
    vm.collect_roots_into(out);
}
```

### with_vm_roots_frame Call Sites

1. **eval_loop.rs:8564** — Main VM dispatch from trampoline
   ```rust
   let _vm_roots_guard = self.with_vm_roots_frame();
   eval_trampoline(...)
   ```

2. **eval_loop.rs:8660** — Secondary VM call site
   ```rust
   let _vm_roots_guard = self.with_vm_roots_frame();
   ```

3. **vm/mod.rs:4049, 4184** — Internal VM nested dispatch (push_vec for intermediate chunks)

### VM Chunk Constant Roots

**File**: `src/backend/bytecode/cache.rs`  
Function `collect_generic_chunk_constants(chunk: &GenericChunk<V>, out: &mut Vec<V>)`

Called from `collect_roots_into` at line ~1055:
```rust
super::cache::collect_generic_chunk_constants(&self.chunk, out);
```

Walks chunk's immutable constant pool (contains literal MettaValue atoms/strings from compilation).

---

## A5: frame_chain Apparatus (Full Deletion Inventory)

### Core API Functions & Types

**File**: `src/backend/eval/frame_chain.rs` (489 lines)

| Item | Line | Type | Purpose |
|------|------|------|---------|
| `RootCollectorFn` | ~50 | Type alias | `unsafe fn(*const (), &mut Vec<MettaValue>)` |
| `struct EvalFrame` | ~65 | Type | {parent, root_data, root_collector, label} on heap |
| `enum FrameLabel` | ~90 | Enum | Include, Import, AssertEqual, Eval, BytecodeVm, Custom |
| `FRAME_CHAIN_HEAD` | ~140 | Thread-local | `Cell<*const EvalFrame>` (chain head pointer) |
| `struct EvalFrameGuard` | ~155 | RAII | Moves frame to/from heap; Push/Drop cycle |
| `EvalFrameGuard::push_vec()` | ~165 | Method | Generic type bridge; calls `push_custom` |
| `EvalFrameGuard::push_custom()` | ~178 | Method | Allocate Box<EvalFrame>, set as chain head |
| `collect_vec_roots()` | ~220 | Function | Type-erased root collector for `Vec<MettaValue>` |
| `maybe_push_frame<C>()` | ~230 | Function | Generic wrapper; always returns `Some(...)` for MettaValue |
| `collect_frame_chain_roots()` | **251** | **Public** | **Walk chain, invoke each collector** |
| `capture_stack_trace()` | ~275 | Function | Collect FrameLabels for error messages |
| `format_stack_trace()` | ~295 | Function | Human-readable trace string |

### EvalFrameGuard::push_vec Call Sites (18 total)

**Pattern**: `let _guard = unsafe { EvalFrameGuard::push_vec(label, &vec_ptr) };`

#### In frame_chain.rs (tests + internal):
1. Line 327 — test_single_frame_collects_roots()
2. Line 328 — collect within test guard scope
3. Line 338 — post-guard verification
4. Line 364 — test_custom_frame_collects_roots (custom frame)
5. Line 370 — collect within custom frame
6. Line 388 — test_nested_frames (outer_values)
7. Line 391 — test_nested_frames (inner_values)
8. Line 392 — collect between guards
9. Line 396 — post-inner-guard verification
10. Line 417 — test_deeply_nested (v1 Eval)
11. Line 419 — test_deeply_nested (v2 Include)
12. Line 421 — test_deeply_nested (v3 Import)
13. Line 450 — test_frame_labels (v1)
14. Line 451 — test_frame_labels (v2)
15. Line 469 — collect after frames

#### In vm/mod.rs (bytecode VM chunk rootings):
16. Line 4049 — `FrameLabel::Chunk` in nested VM call
17. Line 4184 — `FrameLabel::Chunk` in another VM path

#### In eval_loop.rs (trampoline + parallel dispatch):
18. Line 2364 — ParallelDispatchRootProvider frame (custom collector)
19. Line 2850 — ParallelCollapseRootProvider frame (custom collector)
20. Line 3329 — TrampolineFrameRoots frame (custom collector) in eval_trampoline_inner

**Note**: Actually **20 sites**, not 18. Three involve custom collectors (ParallelDispatchRootProvider, ParallelCollapseRootProvider, TrampolineFrameRoots).

### EvalFrameGuard::push_custom Call Sites (explicit custom collectors)

**Pattern**: `EvalFrameGuard::push_custom(label, data_ptr, custom_collector_fn);`

1. **eval_loop.rs:2364** — `ParallelDispatchRootProvider::collect_roots`
2. **eval_loop.rs:2850** — `ParallelCollapseRootProvider::collect_roots`
3. **eval_loop.rs:3329** — `collect_trampoline_frame_roots` (TrampolineFrameRoots)
4. **vm/mod.rs:1199** — `vm_roots_collector` (indirect via with_vm_roots_frame)

### collect_frame_chain_roots() Call Sites (11 total)

**Pattern**: `collect_frame_chain_roots(&mut roots_vec);`

#### In gc_allocator.rs (safepoint collection):
1. **Line 3687 (collect_all_roots)** — After provider snapshots; calls `collect_safepoint_roots` which calls this
2. **Line 3707 (collect_all_roots_readonly)** — Read-only provider variant

#### In eval_loop.rs (cooperative GC + safepoints):
3. **Line 184** — `worker_cooperative_safepoint()`: collect parent-class roots from frame chain
4. **Line 2651** — ProcessReturn fan-out: collect parent roots for resumption
5. **Line 2752** — ProcessFunction fan-out: collect parent roots
6. **Line 3504** — `eval_trampoline_inner` default mid-loop safepoint

#### In frame_chain.rs tests:
7. Line 316, 328, 338, 370, 392, 396, 417–421, 450, 451, 469, 477 (12 test calls)

**Total actual production call sites**: 6 (gc_allocator + eval_loop)

### collect_safepoint_roots() Definition

**File**: `src/backend/models/gc_allocator.rs`  
**Lines**: 3827–3839

```rust
fn collect_safepoint_roots(roots: &mut Vec<MettaValue>) {
    if let Some(registry) = SAFEPOINT_ROOTS.get() {
        let guard = registry.lock();
        for root_set in guard.iter().flatten() {
            roots.extend(root_set.iter().copied());
        }
    }
}
```

Called from:
- `collect_all_roots()` line 3714
- `collect_all_roots_readonly()` line 3749

---

## ROOT_REGISTRY Apparatus (A5 Deletion Target)

### Core Definitions

**File**: `src/backend/models/gc_allocator.rs`

| Component | Line | Definition |
|-----------|------|-----------|
| `pub trait RootProvider` | **3637** | `fn collect_roots(&self, roots: &mut Vec<MettaValue>);` |
| `static ROOT_REGISTRY` | **3651** | `OnceLock<RwLock<Vec<Weak<dyn RootProvider>>>>` |
| `fn root_registry()` | **3656** | Lazy init accessor for ROOT_REGISTRY |
| `pub fn register_root_provider()` | **3663** | Write-lock registry, push Weak ref |
| `pub fn collect_all_roots()` | **3686** | **Phase 1**: Snapshot providers; **Phase 2**: collect without lock |
| `fn collect_all_roots_readonly()` | **3729** | Read-lock variant (no pruning) |

### RootProvider Implementations (11 types)

**File** | **Line** | **Type** | **What It Roots** |
|--------|---------|---------|------------------|
| environment/core.rs | ~2158 | `GenericEnvironmentShared<MettaValue>` | Space, named_spaces, bindings, memoization |
| models/metta_state.rs | ~300+ | `MettaStateGcRoots` | MettaState.output, .result_bindings |
| bytecode/cache.rs | ~800+ | `BytecodeCacheRoots` | Global bytecode chunk constant pool |
| bytecode/space_registry.rs | ~200+ | `SpaceRegistryRoots` | Interned SpaceHandle atoms/URIs |
| bytecode/memo_cache.rs | ~400+ | `MemoCacheRoots` | Per-env memo cache entries |
| eval/trampoline/types.rs | ~2050 | `ParallelDispatchRootProvider` | nondeterministic_dispatch_results bindings |
| eval/trampoline/types.rs | ~2150 | `ParallelCollapseRootProvider` | collapse_results + fold accumulators |
| eval/trampoline/current_iter_root.rs | ~50+ | `CurrentIterRootProvider` | Per-thread active iteration roots |
| bytecode/compiler/iterative.rs | ~1200+ | `CompilerAtomRoots` | Compiled atom interning table |
| bytecode/tiered_cache.rs | ~2000+ | `TieredCacheRoots` | T0/T1/T2/T3 bytecode cache entries |
| models/gc_allocator.rs | ~3600+ | (via `try_register_env_roots`) | Environment clones for forking |

### register_root_provider() Call Sites (10 total)

All follow pattern: `register_root_provider(&Arc<ConcreteType>);`

1. **environment/core.rs** — GenericEnvironmentShared<MettaValue> constructor (multiple forks)
   - Line ~2200: Initial environment creation
   - Line ~2250: fork_for_nondeterminism
   - Line ~2300: fork_for_import
   - Line ~2350: fork_for_space_op
   - Line ~2400: fork_for_mutation

2. **models/metta_state.rs** — MettaStateGcRoots
   - Line ~330: register upon MettaState creation

3. **bytecode/cache.rs** — BytecodeCacheRoots
   - Line ~850: register global bytecode cache

4. **bytecode/space_registry.rs** — SpaceRegistryRoots
   - Line ~220: register space interning

5. **bytecode/memo_cache.rs** — MemoCacheRoots
   - Line ~450: register per-env memo cache

6. **eval/trampoline/types.rs** — ParallelDispatchRootProvider
   - Line ~2070: register on dispatch fork

7. **eval/trampoline/types.rs** — ParallelCollapseRootProvider
   - Line ~2170: register on collapse fork

8. **eval/trampoline/current_iter_root.rs** — CurrentIterRootProvider
   - Line ~100: register per-thread iteration roots

9. **bytecode/compiler/iterative.rs** — CompilerAtomRoots
   - Line ~1250: register atom table

10. **bytecode/tiered_cache.rs** — TieredCacheRoots
    - Line ~2050: register tiered cache

### register_temporary_roots() and SafepointRootHandle

**File**: `src/backend/models/gc_allocator.rs`

| Item | Line | Purpose |
|------|------|---------|
| `struct SafepointRootHandle` | **3781** | RAII guard; releases slot on drop |
| `impl Drop for SafepointRootHandle` | **3786** | Clears SAFEPOINT_ROOTS[idx] |
| `pub fn register_temporary_roots()` | **3807** | Claim slot in SAFEPOINT_ROOTS, return handle |
| `static SAFEPOINT_ROOTS` | (before 3781) | `Mutex<Vec<Option<Vec<MettaValue>>>>`; a preallocated "parking lot" for transient roots |

**Call sites** (13 total):

1. **bin/mtt_conformance.rs:189** — Hold conformance results
2. **main.rs:775** — Hold filtered eval results
3. **main.rs:1033** — Hold filtered results in REPL
4. **models/gc_allocator.rs** — 5 test calls (lines 7063, 7091, 7092, 7115, 7116, 7136, 7140, 7202)
5. **eval/mod.rs:124** — Hold eval_direct roots
6. **eval/trampoline/session_context.rs:237** — Hold session roots
7. **eval/trampoline/context.rs:79** — Hold evaluation context roots
8. **eval/trampoline/context.rs:462** — Hold result context roots
9. **eval/trampoline/eval_loop.rs:193** — Cooperative safepoint roots (worker_cooperative_safepoint)
10. **eval/trampoline/eval_loop.rs:2676** — ProcessReturn parent roots
11. **eval/trampoline/eval_loop.rs:2774** — ProcessFunction parent roots

---

## A4: Deferred Env Drop & Trampoline Safepoint (E₀ Registration)

### deferred_shared_drops Vector

**File**: `src/backend/eval/trampoline/eval_loop.rs`

| Component | Line | Purpose |
|-----------|------|---------|
| Declaration | **3394** | `Vec<Arc<GenericEnvironmentShared<MettaValue>>>` |
| Push site | **8281** | Rule dispatch: push cloned env into deferred vec |
| Pop site | **3464** | On Continuation pop: deferred drop batched later |
| Batch drain | **3623–3626** | Drain up to 32 entries per iteration; drop immediately |
| Root collection | **3521** | Collect from deferred envs into roots before GC |

**Key insight**: Deferred drops create temporary Arc refcounts that must be visible to GC. `collect_all_roots()` **does NOT** walk deferred_shared_drops directly; they are rooted via `GenericEnvironmentShared<MettaValue>::RootProvider::collect_roots()` when the environment is still live.

### TrampolineFrameRoots (Mid-Execution Rooting, Index-GC Only)

**File**: `src/backend/eval/trampoline/eval_loop.rs`

| Component | Line | Definition |
|-----------|------|-----------|
| `struct TrampolineFrameRoots` | ~3300 | Raw pointers to work_stack and continuations |
| Declaration | **3309** | Local in eval_trampoline_inner |
| Guard creation | **3323–3345** | Conditional push_custom (gated on gc_mode_is_index()) |
| Collector | **collect_trampoline_frame_roots** | Walks work_stack/continuations, calls their collect_values |

**Gated feature**: Only active when `gc_mode_is_index() == true` (feature `index-gc`). In default (slab) build, no frame is pushed.

---

## Summary: Apparatus Extent for A5 Deletion

### Files to Delete/Refactor

1. **`src/backend/eval/frame_chain.rs`** (489 lines) — ENTIRE FILE
   - All frame chain logic, thread-local FRAME_CHAIN_HEAD, EvalFrameGuard
   - **Replacement**: Direct walk of reified machine (S/C/K + env)

2. **`src/backend/models/gc_allocator.rs`** (major refactor, ~1000+ lines affected)
   - **Delete**: ROOT_REGISTRY (lines 3651), register_root_provider (3663), trait RootProvider (3637)
   - **Delete**: All 11 RootProvider implementations across codebase
   - **Keep**: SafepointRootHandle + register_temporary_roots (those are transient roots, not providers)

### Call-Site Cleanup

| Category | Count | Action |
|----------|-------|--------|
| EvalFrameGuard::push_* | 20 | Delete push_vec, push_custom calls; replace with direct collection |
| collect_frame_chain_roots | 6 production | Replace with direct S/C/K walk |
| register_root_provider | 10 | Delete all calls (environment + cache roots inlined into env collector) |
| Continuation.env field | 73 | No change (still present, but collected via structural reader not registry) |

---

## A3: Environment Factorization Simplicity

**Current state**: All 73 Continuation env fields are **already the same Arc** (E₀) due to Arc::clone propagation through fork_for_nondeterminism.

**A3 refactor scope**: 
- Extract env from WorkItem/Continuation to a persistent **global E₀** or **per-trampoline E_local**
- OR: Keep env in each variant but register ONCE at eval_loop entry (not per-provider per-environment)

**Cost**: Low. The env is already invisible to S/C/K root walks (only rooted via registry). A3 is a formalization, not a behavioral change.

---

## A4: VM-as-K-Leaf (Structural Reader Implementation)

**Current surface**:
- `GenericBytecodeVM::collect_roots_into()` (lines 1040–1150): walks 16 field groups (13k bytes)
- `with_vm_roots_frame()` (lines 1181–1210): pushes frame with `vm_roots_collector` function
- `vm_roots_collector` (line 144): type-erased closure

**A4 scope**:
1. Keep `collect_roots_into()` as-is (no change to VM internals)
2. Replace `with_vm_roots_frame()` frame-chain registration with direct call in eval_loop
3. Make eval_loop aware of VM state via context parameter (already partially done via EvalContext trait)

**Machine-equivalence oracle**: Verify that `collect_trampoline_frame_roots` (which walks S/C/K at top level) + env + vm.collect_roots_into = exact same closure as current nested-frame walk.

---

## Key Citations for Design Review

| Concept | File | Lines |
|---------|------|-------|
| SharedEnv type | context.rs | ~1 line |
| Continuation variants (78 total) | types.rs | ~50–3230 |
| Continuation.collect_values (no env walk) | types.rs | ~1817–2300 |
| VM collect_roots_into | vm/mod.rs | 1040–1150 |
| with_vm_roots_frame | vm/mod.rs | 1181–1210 |
| frame_chain API (full) | frame_chain.rs | 1–489 |
| ROOT_REGISTRY apparatus | gc_allocator.rs | 3637–3800+ |
| collect_all_roots (2-phase) | gc_allocator.rs | 3686–3750 |
| TrampolineFrameRoots (index-gc only) | eval_loop.rs | ~3300–3345 |
| deferred_shared_drops | eval_loop.rs | 3394, 3464, 3521, 3623 |
