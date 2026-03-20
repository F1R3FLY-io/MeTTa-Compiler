# MeTTaTron Technical Cheat Sheet

Interview-ready talking points for MeTTaTron — a high-performance MeTTa evaluator in pure Rust.

---

## 1. Project Identity & Mission

- **Name**: MeTTaTron
- **What**: A high-performance MeTTa language evaluator implemented in pure Rust
- **MeTTa**: LISP-like S-expression language for symbolic reasoning, pattern matching, type assertions, rules, and grounded functions. Programs are sets of rewrite rules applied nondeterministically — multiple rules can match, producing multiple results.
- **Integration target**: Rholang (blockchain-native concurrent language) via direct Rust linking (no FFI, no IPC). MeTTa expressions evaluate inside Rholang smart contracts, enabling symbolic AI reasoning on-chain.
- **Relationship to MeTTa HE** (hyperon-experimental): Alternative implementation. HE prioritizes correctness and language completeness; MeTTaTron targets sub-millisecond evaluation latency, deep nesting (252+ depth), million-fact rule sets, and direct Rholang integration — requirements that demand fundamentally different data structures (NaN-boxing, lock-free slab allocator) and execution strategies (JIT compilation, parallel nondeterministic branching).

---

## 2. Evaluation Pipeline

```
MeTTa Source → Tree-Sitter Parser → MettaExpr IR → Compilation (ValueEmitter) → MettaValue → Trampoline Evaluator → Results
```

- **ValueEmitter / zero-conversion design**: Traditional evaluators convert between representation types at each stage (parse→IR→value→eval), paying 2-3 marshalling passes per expression. MeTTaTron's `ValueEmitter` emits `MettaValue` directly during parsing — no intermediate types, keeping the hot path allocation-free. All pipeline stages share the same `MettaValue` type via `MettaValueTrait` + `MettaValueFactory` generics.
- **Iterative trampoline**: A work-stack + continuations architecture that replaces recursion. Exists because Tokio worker stacks are ~2MB (vs ~8MB OS threads) and knowledge graphs routinely nest 252+ levels deep — recursion would overflow. The trampoline keeps continuation state on the heap where it can grow unboundedly, enables tail call optimization, and is async-safe.

---

## 3. Core Data Model — MettaValue

**8 bytes, Copy-semantic** — fits in a register, passed by value everywhere. This is the universal currency of the evaluator: every expression, result, and intermediate value is a `MettaValue`.

### Two-Tier Memory Encoding

The core challenge: MeTTa values range from simple booleans to deeply nested S-expressions. A uniform representation must handle both without penalizing the common case (primitives).

- **NaN-boxed inline**: Booleans, small integers (48-bit signed), Unit, Empty — zero allocation, zero GC pressure. Uses IEEE 754 quiet NaN tag bits (bits [63:48] ≥ 0x7FF8) to distinguish types within the 64-bit word. Since 99%+ of values in typical workloads are primitives, this keeps the vast majority in registers with no heap interaction.
- **Slab-allocated**: Atoms, strings, S-expressions, errors, types, and other compound structures — tagged pointer with low-bit flags. 16-byte aligned for SIMD/cache efficiency. The slab allocator (§4) handles these without fragmenting the system allocator.

### Key Design Decisions

- **FLAG_HAS_VARIABLES** (bit 0 of pointer): O(1) variable presence check without tree walk. Exists because `apply_bindings` is the #1 CPU cost in pattern-heavy workloads — the flag lets the evaluator skip the O(tree) walk entirely for ground (variable-free) expressions, which are the common case.
- **16 variants** covering atoms, ground types, compound structures (S-expressions, conjunctions), evaluation state (quoted, spanned, memo), and space/state handles
- **ValueView**: A discriminated view for hot-path dispatch. Avoids materializing the inner enum on hot paths where only a discriminant check is needed. Auto-strips source-position (`Spanned`) wrappers so pattern matching code doesn't need to handle them.
- **Zero-conversion generics**: `MettaValueTrait` + `MettaValueFactory` — all pipeline stages share the same type, no conversions needed. Different backends (heap, arena, JIT-optimized) can provide different implementations without changing evaluation logic.

### Two-Tier Hash Cache

S-expression hashing is recursive — O(tree_size) for deeply nested expressions. In knowledge graphs with 252+ depth, this becomes a bottleneck when the same sub-expression is hashed repeatedly (e.g., during pattern matching, memoization, and rule indexing).

Thread-local cache eliminates this. L1 is a 1024-entry direct-mapped array (~2ns hit); L2 is a HashMap with Fibonacci pointer hashing. After first hash, subsequent lookups are O(1). Cleared at GC safepoints to prevent ABA issues (stale pointers matching new allocations).

### Hash-Consing

Knowledge graphs contain millions of structurally identical sub-expressions (e.g., the same type annotation on thousands of atoms). Hash-consing is thread-local content-addressed deduplication for ground (variable-free) S-expressions. It collapses structural equality to a single pointer comparison — O(1) instead of O(tree). Cleared at GC safepoints.

---

## 4. Slab Allocator & GC

### Lock-Free Bump Allocator

MeTTaTron allocates millions of small, immutable values during pattern matching and rule application. System malloc returns memory to the heap but never to the OS, causing monotonic RSS growth. Bumpalo (arena allocator) was tried but its non-Sync interior mutability caused SIGSEGV in multithreaded evaluation (observed in 2/20 runs).

The custom slab allocator solves both problems:

- **Treiber stack** with 128-bit ABA-safe CAS (`portable-atomic`) for lock-free free-list management — no mutexes on the allocation hot path, only atomic CAS operations
- **9 power-of-2 size classes** (16B–4KB) with mmap-based 256KB pages — fixed-size slots eliminate external fragmentation; `munmap` guarantees OS memory release when pages empty (RSS actually decreases)
- **Thread-local free caches** per size class — threads allocate from their own cache first, falling back to the global Treiber stack only on cache miss, minimizing contention

#### Three-Tier Allocation Fallback

Each allocation attempt follows three tiers of decreasing speed:

1. **Tier 1 — Thread-local cache** (O(1), no atomics): 64-slot `ThreadFreeCache` per thread. Each `CachedSlot` stores the raw pointer, a `*const ValuePage`, and a pre-computed `slot_idx` — eliminating both `pages.read()` RwLock acquisition and O(P) page scan on cache hit. A generation counter (`CACHE_GENERATION`, `AtomicU64`) detects stale entries: if the global generation has advanced (pages were munmapped), the entire cache is discarded before any stale pointer is dereferenced. For data allocations, 9 per-size-class caches of 64 pointers each (~4.5KB/thread).

2. **Tier 2 — Global Treiber stack batch pop**: On cache miss, 32 slots are popped from the Treiber stack in a single `pages.read()` scope. The first slot is returned; the remaining 31 are cached (pre-validated with page pointers and slot indices). This amortizes the RwLock cost over 32 allocations.

3. **Tier 3 — Atomic bump allocation**: If the Treiber stack is empty, a `compare_exchange_weak` loop on the page's `bump_count` claims the next virgin slot. If the page is full, a new 256KB page is mmap'd under `pages` write lock.

#### Atomic Write Protocol

Slab slots double as Treiber stack `FreeNode` entries when freed — the first 16 bytes hold an `AtomicU128` chain pointer. When a slot is re-allocated, the first 16 bytes must be written atomically (Release ordering) to pair with `pop()`'s Acquire load — preventing a concurrent speculative reader from observing a torn FreeNode/MettaValueInner hybrid. Compiles to `CMPXCHG16B` on x86-64, `LDXP/STXP` on ARM64. The quiescent GC protocol guarantees `push()` and `write_slot_bytes()` never target the same slot concurrently.

#### Batch Push & ASAN Integration

During GC sweep, hundreds to thousands of dead slots are freed per cycle. **Batch push** builds the FreeNode chain locally (zero contention), then splices the entire chain onto the Treiber stack head with a single CAS — reducing contention from O(D) to O(1) operations.

**ASAN poisoning** supports three modes: **standard** (poisons after the 16-byte FreeNode header — Treiber traversal still works), **full** (entire slot — catches any stale dereference including discriminant reads), and **quarantine** (`METTA_GC_QUARANTINE` env var — dead slots diverted to a quarantine list, fully poisoned, held for N GC cycles before reuse, giving ASAN maximum temporal sensitivity).

### Background Snapshot-Based Mark-Sweep GC

The GC must run concurrently with evaluation without stop-the-world pauses. The challenge: marking while evaluation threads allocate causes data races (Vec reallocation, free-list corruption, inconsistent bump counts).

The snapshot approach solves this: at a safepoint, the evaluation thread builds a `GcSnapshot` (owned copy of page data pointers, bump counts, roots) and sends it to the GC thread via MPSC channel. The GC operates exclusively on this owned snapshot — evaluation threads allocate freely during collection. New allocations go to slots not in the snapshot, invisible to GC.

- Threshold starts at 4MB, grows by 2× after each cycle
- **3 backpressure levels**: Graduated throttling proportional to memory pressure — level 0 (below threshold) has zero overhead (single relaxed atomic load); at maximum, evaluation blocks until GC completes. Formally verified in TLA+ to eventually relax.
- **`SessionGuard`**: Arena lifecycle tied to evaluation sessions — results must survive GC until formatted for output; SessionGuard prevents premature collection
- **Safepoint root handles** for quiescent-state transitions — all pointer-keyed caches cleared on GC epoch advance. Epoch filtering prevents TOCTOU use-after-free when slots are re-allocated after the snapshot was taken.

#### Mark-Sweep Mechanics

The `GcSnapshot` contains per-page `PageSnapshot` (data pointer, bump count, epoch vector), slot size, snapshot epoch, root set, and empty mark bitmaps. Built under `pages.read()` lock (sub-millisecond), then transferred to the GC pool via crossbeam channel — the GC thread never touches live allocator state.

**Mark phase**: Worklist-based DFS from roots. Each root's inner pointer is pushed; the loop pops, matches on `MettaValueInner` variant (SExpr, Conjunction, Error, Type, Quoted, Space, Spanned), and pushes reachable children. Slots are skipped if freed (epoch == `u64::MAX`) or allocated after snapshot (`slot_epoch > snapshot_epoch`). Marks are set in per-page bitmaps (bit `i` in word `i/64`).

**Sweep phase**: Iterates all committed slots. Unmarked, non-freed, pre-snapshot slots go into the dead set. Dead data collection is deferred to response processing — reading dead content during sweep races with concurrent session releases.

#### Five-Phase Response Processing

When the evaluation thread receives a `GcResponse`, it processes under `GcInProgressGuard`:

- **Phase 0 — Safepoint rescue**: Builds the transitive closure of currently registered safepoint roots (trampoline work_stack + continuations). Values dead per the previous cycle but reachable from current safepoint roots are rescued — prevents use-after-free when the trampoline resumes.
- **Phase 1 — Epoch + safepoint filter**: For each dead pointer, binary-search the sorted page index (O(log P)), check `slot_epoch > snapshot_epoch` (skip re-allocated), check safepoint live set (skip rescued). Pre-resolves `(page_idx, slot_idx)` so Phase 3 needs zero page lookups.
- **Phase 2 — Collect dead data**: Reads variant-specific data (strings, slices, spans) from still-valid slot content before Phase 3 invalidates it.
- **Phase 2.5 — Flush exec_counts**: Non-zero execution counters from dead slots merged into global `TieredCache` (under `COUNTER_FLUSH_LOCK`), preserving JIT eligibility data.
- **Phase 3 — Free value slots**: Sets sentinel epoch (`u64::MAX`), clears compilation hash, decrements `live_count`, then `push_batch` (single CAS). In quarantine mode, slots are poisoned and diverted instead.
- **Phase 4 — Free data slots**: Batched by size class, each with its own `DataPageIndex`.
- **Phase 5 — Release empty pages**: Write-locks `PAGE_LIFECYCLE_LOCK`, drains Treiber stack atomically, filters out pointers to empty pages, rebuilds the stack, bumps `CACHE_GENERATION` **before** munmap, then drops pages (triggering `munmap`). `GC_SWEEP_EPOCH` is incremented so thread-local caches (`EVAL_MEMO`, `NORMAL_FORM_BLOOM`, `HASH_CONS_TABLE`) self-invalidate.

#### Backpressure Details

The cron monitor (100ms poll) computes backpressure from `committed_bytes / gc_threshold`:

| Level | Condition | Tier 1 (every 256 trampoline iters) | Tier 2 (between top-level exprs) |
|-------|-----------|-------------------------------------|-----------------------------------|
| 0 | `< threshold` | no-op (single Relaxed load) | no-op |
| 1 | `≥ threshold` | `yield_now()` | no-op |
| 2 | `≥ 1.5× threshold` | `sleep(10μs)` | no-op |
| 3 | `≥ 2× threshold` | `sleep(100μs)` | block on `GC_CYCLE_CONDVAR` until cycle completes |

Tier 1 is gated on `gc_cycle_in_flight()` — if no GC is running, sleeping is pointless. `GC_REACHABLE_COUNTER` heartbeat prevents permanent throttling: if the counter hasn't advanced between polls (GC lifecycle unreachable in library/test code), backpressure stays at 0. Formally verified in TLA+ (`BackpressureEventuallyRelaxes` liveness property).

### Coordination & Integration Layer

**Quiescent-state protocol**: Four atomics (`ACTIVE_EVALUATORS: AtomicU32`, `GC_IN_PROGRESS: AtomicBool`, `GC_REQUESTED: AtomicBool`, `GC_CYCLE_IN_FLIGHT: AtomicBool`) coordinate GC with evaluation. `EvalGuard` (RAII) increments `ACTIVE_EVALUATORS` on entry and blocks on a condvar if `GC_IN_PROGRESS` is set. `GcInProgressGuard` uses CAS-based `try_enter()`. GC snapshot building requires `ACTIVE_EVALUATORS == 0` (quiescent state). Evaluators wait sub-millisecond for snapshot capture — no stop-the-world pauses.

**PAGE_LIFECYCLE_LOCK (RwLock)**: Mark/sweep hold the read lock; `release_empty_pages()` holds the write lock during munmap. Without this, a GC worker could dereference pointers into pages another worker just unmapped. Discovered as Bug 4 in TLA+ model `SlabGC_Pages`.

**Generation-based cache invalidation**: `CACHE_GENERATION` (values) and `DATA_CACHE_GENERATION` (data) are `AtomicU64` counters incremented **before** munmap. Thread-local caches compare their stored generation on every access; stale caches are silently discarded (stale pointers are never pushed back — they point to unmapped memory).

**Epoch advancement**: The global epoch increments on every free-list reuse; each slot's epoch is set at allocation time. GC snapshot captures the current epoch. Sweep skips slots where `slot_epoch > snapshot_epoch` (allocated after snapshot). Freed slots get sentinel `u64::MAX`. This prevents TOCTOU use-after-free (Bug 1 in TLA+ model `SlabGC`).

**Hash cache coordination**: `GC_SWEEP_EPOCH` increments after freeing values. Thread-local `EVAL_MEMO`, `NORMAL_FORM_BLOOM`, `HASH_CONS_TABLE` store a local epoch and self-invalidate when stale — preventing access to freed/reused slots.

**Adaptive GC Pool**: 1–4 workers (hill climber, 700ms cooldown, EMA α=0.2). Two crossbeam channels: HIGH priority for `Collect(GcSnapshot)`, LOW for `SessionRelease`. Workers dequeue HIGH first (non-blocking), then LOW with 500ms timeout. Dead workers detected and respawned.

**TLA+ verification**: 4 models — `SlabGC` (base TOCTOU), `SlabGC_Reactive` (snapshot isolation), `SlabGC_Pages` (page-level UAF), `SlabGC_Quiescent` (full protocol with sessions, safepoints, backpressure). The final model verifies 25 safety invariants + 7 liveness properties (8.8M–73.5M states). The models discovered 4 bugs before production: (1) TOCTOU use-after-free from re-allocated slots, (2) data race from GC touching live allocator state, (3) watermark-based sweep missing values above high-water mark, (4) page-level UAF from concurrent mark + munmap.

---

## 5. Pattern Matching & Rule Indexing

### Iterative Work-Stack Algorithm

Pattern matching must compare two arbitrary S-expressions structurally. No recursion — pairs are pushed onto a stack and processed iteratively. Same stack-overflow motivation as the trampoline: MeTTa patterns can be arbitrarily deep, and Tokio stacks are limited.

### Variable Types & Matching

MeTTa has 4 variable types, each with distinct matching semantics:
- `$x` (pattern variable) — binds to any value during matching
- `&name` (space reference) — references a named atom space
- `'z` (quoted variable) — matches literally, not as a binding target
- `_` (wildcard) — matches anything, discards the value

Matching supports structural + quoted matching, cross-type compatibility (Unit ↔ empty S-expr), and exact-match for ground types.

### Adaptive Bindings (GenericBindings)

During pattern matching, variable bindings are accumulated. Profiling showed 90%+ of bindings have ≤1 entry, so heap allocation is wasteful for the common case.

3-tier storage adapts: Empty (zero-cost) → Single (inline, one key-value pair) → SmallVec\<8\> (stack-allocated up to 8 entries). Interned keys (`&'static str`) eliminate string allocation. Only bindings with 9+ entries touch the heap.

### SG1: Binding-Aware Matching

Profiling showed `apply_bindings` was the #1 CPU hotspot — for every match attempt, it materialized an O(tree) deep copy with all variables substituted. SG1 eliminates this.

WAM-style lazy variable resolution — matches against template + outer bindings **without materializing** the fully-substituted expression. When a variable is encountered during structural comparison, it's looked up in bindings on the fly and the bound value is compared directly. Combined with `has_variables_fast()` (O(1) FLAG_HAS_VARIABLES check), binding application is skipped entirely for ground expressions.

### Two-Level Rule Index

When evaluating `(f arg1 arg2 ...)`, the evaluator must find all rules whose LHS matches. A linear scan of all rules is O(n) — too slow for large rule sets.

First level: `(head_symbol, arity)` → O(1) HashMap lookup. Second level: first argument's head symbol → narrows candidates further. PLN workloads have many rules sharing the same head symbol but differing in first argument; the second level exploits this clustering to narrow candidates by 3–10×. Wildcard rules (non-S-expression LHS) are included in all queries.

---

## 6. Evaluation Engine

### Trampoline Architecture

The evaluation engine replaces a recursive evaluator with an explicit work-stack and continuations — a **CEK machine** (Control + Environment + Kontinuation). Structurally isomorphic to a pushdown automaton: finite control (3 work item types × 60+ continuation states), explicit LIFO stack, and finite alphabet of stack symbol kinds. The key difference from a classical PDA: each continuation carries rich data (environments, bindings, partial results), making the effective stack alphabet infinite — the machine computes result sets, not merely accepts/rejects. Each "continuation" captures the state of a partially-completed evaluation step — what to do when a sub-result comes back.

3 work item types (Eval, EvalWithBindings, Resume) and **60+ continuation variants** across 14 categories — full call-stack semantics without recursion. Continuations cover core dispatch, grounded ops, control flow, iteration, error handling, unification, nondeterminism, space operations, memoization, state, and type/format operations. This enables TCO, parallel dispatch (fork work items to different threads), and async-safe execution without recursive frames.

### Evaluation Semantics

- **Lazy**: `if` defers both branches — only the chosen branch is evaluated. This prevents exponential blowup in nondeterministic programs where unused branches would themselves produce multiple results.
- **Eager for grounded ops**: Arithmetic and comparisons evaluate arguments first because they need concrete values (`+ 1 2` can't defer).
- **Tail call optimization**: `is_tail_call` flag prevents depth increment — O(N) instead of O(N²) for nested `let*` chains. The trampoline naturally supports this by reusing the current work-stack slot.
- **Identity short-circuit**: `apply_bindings` returns the original value if no substitution occurred (detected via FLAG_HAS_VARIABLES), avoiding unnecessary allocation.

### Nondeterministic Semantics

MeTTa's core semantic: multiple rules for the same pattern produce an **unordered result set**, not a single value. This is fundamental to the language's use in symbolic reasoning, where a query may have multiple valid answers.

Nested nondeterminism yields Cartesian products (e.g., two 2-valued functions composed → 4 results). Key combinators: `superpose`/`amb` (explicit nondeterminism), `collapse` (collect all results into a list), `empty` (branch annihilation — prune this path), `guard`/`commit`/`backtrack` (control over which branches survive).

### Caching & Memoization

Multiple evaluation phases repeatedly encounter the same expressions. Rather than one large cache, MeTTaTron uses layered caches targeting different hot paths:

- **Normal-form bloom filter** (100K entries): O(1) test for "is this expression already in normal form?" — catches fixpoint loops early without expensive evaluation
- **Expression-level eval memo** (8,192 LRU): Full evaluation results for identical (expression, environment) pairs
- **Match result cache** (4,096 LRU): Pattern match results for the same pattern against the same rule set
- **Operator cache** (512 LRU): Skips hash computation and re-matching for repeated operator expressions in arithmetic-heavy code
- **MORK serialization cache**: MettaValue→bytes mapping for Rholang round-tripping
- **Generic DashMap-based memo cache**: User-controlled via `(memo-call ...)` for expensive computations

Impure operations are excluded from memoization. All pointer-keyed caches invalidated on GC epoch advance to prevent stale pointer matches.

---

## 7. Tiered JIT Compilation

MeTTa programs have highly variable function hotness: most functions execute once (knowledge base facts), but some execute millions of times (recursive rules, arithmetic helpers). The JIT invests compilation effort proportional to execution frequency.

4 execution tiers: tree-walker (0–9 execs) → bytecode VM (10+, V8 Ignition equivalent) → JIT Stage 1 (200+, Cranelift, V8 Maglev equivalent) → JIT Stage 2 (2,000+, Cranelift, V8 Turbofan equivalent).

- **Tree-walker** (Tier 0): Direct S-expression interpretation. Zero compilation cost, maximum flexibility. Handles cold code (most facts/rules execute 0-9 times).
- **Bytecode VM** (Tier 1): Stack-based bytecode compiled from S-expressions. ~1-2ms compilation cost, amortized over 10+ executions. Removes tree-walking overhead.
- **JIT Stage 1** (Tier 2): Cranelift native code generation with basic optimizations. ~5-10ms compilation cost, justified at 200+ executions.
- **JIT Stage 2** (Tier 3): Full Cranelift optimization with type specialization based on profiling data. Highest compilation cost, reserved for very hot functions (2,000+).

Key design points:
- **Background compilation**: Non-blocking — spawned on a separate 4-worker compile pool, never stalls evaluation. If the compile pool is saturated, tasks are dropped (speculative — safe to lose). Tier status: NotStarted → Compiling → Ready (or Failed → fallback to lower tier).
- **Thread-local execution counters** flushed periodically to a global DashMap — avoids atomic contention on every function call
- **Runtime type profiling**: Branch frequencies, argument type feedback (top-2 types), rule match hits — guides JIT optimization after ≥50 samples. Monomorphic call sites can be specialized, eliminating type dispatch overhead.

---

## 8. Environment & Type System

### Copy-on-Write Environment

The environment holds all defined rules, type assertions, and atom space contents. Nondeterministic evaluation creates many branches, each of which may define new rules — so the environment must be cheaply cloneable but isolated between branches.

O(1) clone via Arc reference count — read-only clones share data (the common case). First mutation triggers a deep copy (`make_owned`); subsequent mutations are in-place. This means branch creation is nearly free, and only branches that actually modify the environment pay the copy cost.

### Unified AtomSpace

The atom space stores all facts and rules. Ground atoms (no variables) dominate at 95%+, and MORK's trie-based directional search is fast for them. But MORK's De Bruijn encoding assumes concrete positions — it can't find stored variables at concrete query positions. So variable-containing atoms are stored separately in a Vec and included in all queries.

Ground atoms stored in **MORK PathMap** (trie-based, De Bruijn encoded) for fast pattern search. Multiplicity encoding tracks occurrence counts (same fact asserted N times → multiplicity N, expanded lazily on match).

### Bloom Filters

Before the expensive HashMap + MORK trie traversal, pattern matching checks a bloom filter. A negative means "definitely absent" — and in PLN workloads, 95%+ of lookups are negatives, making bloom rejection the primary code path.

**HeadArityBloomFilter** and **TypeBloomFilter** provide O(1) rejection for absent atoms/types using Kirsch-Mitzenmacher double hashing with xxh3 (SIMD-accelerated, 3-5× faster than SipHash). ~1% false positive rate. Lock-free `AtomicBloomFilter` (CAS bit-set via `fetch_or`) for inferred types, enabling wait-free reads and lock-free insertion.

### Operator Cache

When arithmetic-heavy code evaluates the same operator expression repeatedly (e.g., `(+ $x 1)` with different bindings), the evaluator must look up the operator's implementation each time. The operator cache avoids this.

Thread-local LRU (512 entries) with Fibonacci pointer hashing. Skips hash computation when all rule candidates have structural matchers (no need to re-index).

### Nondeterministic Type System

#### Design Philosophy

Nondeterministic by design for HE (Hyperon) semantic parity. Gradual typing: typed and untyped code mix seamlessly; `%Undefined%` matches any type, so unknown types never block valid programs. Conservative approach: absence of type information → `%Undefined%` (no false rejections).

#### Core Mechanics

- **Multiple types per atom**: `(: a Dog)` + `(: a Cat)` both valid simultaneously — `get-type` returns the full unordered set
- **Nondeterministic propagation**: Tuple/compound types compute the **Cartesian product** of element types (e.g., a pair of 2-typed atoms → 4 compound types)
- **Subtype relations** (`(:< Sub Super)`) with **BFS transitive closure**; arrow types support contravariant parameters / covariant return
- **TypeBloomFilter**: O(1) rejection via xxh3 SIMD hashing before expensive trie lookups (~1% false positive rate)

#### Multi-Phase Type Inference (ordered by priority)

- **Control-flow tracing**: Infers types through `if`/`let`/`case` branches structurally, collecting the union of branch result types
- **Arrow type synthesis from rules**: Bidirectional analysis extracts parameter constraints from RHS usage (e.g., `(= (double $x) (+ $x $x))` → `(-> Number Number)`)
- **Fixpoint iteration for mutually recursive functions**: Tarjan SCC for dependency analysis, state-based cycle detection prevents divergence, generation-based dirty tracking avoids redundant recomputation
- **Type variable matching**: Binding consistency for polymorphic types (`(-> $t $u)` matches `(-> Number Bool)`, binding `{$t: Number, $u: Bool}`)
- **Type variable freshening** (`$t` → `$t__0`) prevents cross-contamination between multiple arrow declarations

#### Why Not Hindley-Milner

HM requires unification-based constraint solving and produces a **single principal type**; MeTTaTron deliberately produces **multiple types** (unordered set) to match MeTTa's nondeterministic evaluation semantics. HM is planned as a future enhancement for polymorphic inference, but the current system prioritizes HE semantic parity.

---

## 9. Parallel Execution

### Parallel Nondeterministic Branching

MeTTa's unordered-set semantics make parallel execution semantics-preserving: since results are an unordered set, it doesn't matter which thread produces which result. When ≥2 rule matches exist, evaluation can fork into parallel branches.

The challenge: unbounded forking creates 2^n threads for depth n (e.g., `(or (or ...) (or ...))`). **Budget system**: global `AtomicU32` initialized to `num_cpus × 2`, with exponential depth decay (`n / 4^depth`) — root-level branches get full CPU attention while deep leaves serialize. Queue pressure backoff prevents oversubscription: if `queue_depth > active_workers × 2`, no budget is granted regardless of remaining capacity.

### Adaptive Work Pool (Hill Climber)

The optimal thread count changes at runtime — CPU-bound phases want more threads, memory-heavy phases want fewer. A static thread pool can't adapt.

Two pools: eval pool (adaptive 1–N workers) and compile pool (fixed 4 workers, tasks dropped under backpressure). The eval pool scales via a **4-term composite objective**: throughput (maximize) × queue depth (minimize) × slab memory pressure (minimize) × RSS pressure (minimize, weighted 1.6× slab to prevent OOM before slab limits trigger). Throughput alone would spawn unlimited threads; memory pressure alone would starve throughput — the weighted sum balances them. Weights verified in Rocq to prevent any single term from dominating.

EMA smoothing with 200ms monitor interval and 1-second cooldown. Worker CPU utilization tracked via `CLOCK_THREAD_CPUTIME_ID` on Linux (VDSO, ~25ns) with heartbeat fallback on other platforms. Cache-line padding prevents false sharing between the monitor thread and worker threads.

#### Geometric Scaling

The hill climber does not adjust by ±1 thread. Step size doubles on consecutive improvements in the same direction (1 → 2 → 4 → 8 → 16), capped at `max_threads / 4`. On direction reversal (objective worsened), step size resets to 1. This enables rapid ramp-up: on a 36-core machine starting at 18 active workers, full saturation takes 2 actions (18 → 27 → 36) instead of 18 with ±1 stepping.

Scaling decisions follow a 3-phase cycle per 200ms tick:
1. **Cooldown check** — after any scale action, wait 5 ticks (1s) for EMA to absorb the perturbation
2. **Objective evaluation** — compute `J = −w_tp·ēma_tp + w_qd·ēma_qd + w_mp·M + w_rss·R` (lower = better; weights: throughput 1.0, queue depth 2.0, slab pressure 5.0, RSS pressure 8.0)
3. **Decision** — if objective improved ≥ 5%: continue direction, double step; if worsened: reverse direction, reset step to 1; otherwise: hold

Workers are never OS-terminated — inactive workers **park** on a per-worker `Condvar` and are **unparked** by the monitor. Parked threads consume no CPU (kernel wait queue). Minimum floor: `METTATRON_MIN_WORK_THREADS` (default 1).

#### Overflow Workers

When core workers block (I/O, lock contention), effective parallelism drops. The monitor detects this via per-worker CPU-time tracking (`clock_gettime(CLOCK_THREAD_CPUTIME_ID)` on Linux, `task_info` on macOS): if `cpu_delta / wall_delta < 0.5` for a worker over a 200ms window, it is classified as blocked.

Overflow workers are spawned to compensate, pulling from the same priority queue. They are **self-draining**: after 2 consecutive 500ms idle timeouts (1s total inactivity), an overflow worker exits. Maximum overflow count equals `max_threads`, so peak thread count is `2 × max_threads` (core + overflow). Dead workers (panics) are detected and respawned automatically.

#### Scheduling Algorithm: Priority Queue with P² Runtime Estimation

The scheduler is a **centralized min-heap priority queue** — not work-stealing. All workers (core + overflow) pop from one shared `parking_lot::Mutex<BinaryHeap>`. This design keeps P² runtime estimates accurate (one global view) and simplifies fairness guarantees at the cost of a single contention point (acceptable because task granularity is coarse — milliseconds, not microseconds).

**Score formula** (lower = scheduled first):
```
score = base_priority + (P²_median_runtime / 1e9) × runtime_weight − age_seconds × decay_rate
```

- **Base priorities**: Interactive=0, Normal=5, BackgroundCompile=10, Low=20, Batch=50
- **P² runtime estimation** (Jain & Chlamtac 1985): O(1) space (5 markers), O(1) per observation. Maintains a running median of task runtimes per task-type hash. Shorter tasks get lower scores → SJF-like behavior within a priority band
- **Age decay** (rate 0.1/s): prevents starvation — a Batch task waiting 50s has its score reduced by 5.0, effectively promoting it to Normal priority
- **FIFO tie-breaking**: monotonic sequence counter ensures stable ordering when scores are equal

Workers block on a `Condvar` when the queue is empty; `push()` calls `notify_one()` to wake exactly one worker.

### Two-Pool Tokio Architecture

CPU-hungry JIT compilation would starve latency-sensitive evaluation if they shared a pool. Compile tasks are speculative (safe to drop); eval tasks are essential.

Both pools are coordinated by the same Tokio runtime (shared with Rholang). Async executor threads handle I/O/coordination; blocking thread pool handles CPU-intensive eval via `spawn_blocking`. `block_in_place` + `block_on` bridges the Rholang sync→async boundary.

---

## 10. Rholang Integration

### Direct Rust Linking

MeTTaTron is compiled as a Rust library and linked directly into the Rholang runtime — no FFI boundary, no serialization/deserialization, no IPC. Each integration call is ~0.5-1μs; FFI/IPC would add 10-100× overhead. Shared `Arc<Mutex<Environment>>` means changes are visible immediately with no sync protocol.

- **`compile_safe`**: Never fails — wraps errors as `(error "improved message")` with context + suggestions via fuzzy matching (liblevenshtein, 72 keywords). Rholang smart contracts must not crash on MeTTa compilation errors; errors become data values.
- **`run_state` / `run_state_async`**: REPL-style evaluation preserving environment across calls; async variant parallelizes independent `!` expressions
- **`eval_metta_session`**: Session-based with O(1) bulk arena deallocation — each session gets its own slab arena, freed in one `munmap` call

### MeTTa ↔ Rholang Conversion

All conversions use **EList** (not ETuple) to support `...rest` decomposition in Rholang pattern matching — MeTTa's variable-length patterns require cons-cell structure (head/tail split) that ETuple doesn't provide. Ground types map directly (Atom→GString, Bool→GBool, Long→GInt); compound types become tagged ELists (e.g., `["error", msg, details]`).

### MORK Byte-Level Serialization

Environments must round-trip through Rholang (serialize MeTTa state, store in Rholang tuplespace, deserialize later). Direct MettaValue→bytes conversion is ~10× faster than the string→parse path.

Binary formats (MTTS for regular expressions, MTTL for wide arity ≥64) with thread-local 256KB buffers, symbol ID caching, and ground fragment caching. Lenient deserialization tolerates version skew in long-running Rholang processes. Post-deserialize bloom filter rebuild ensures caches are valid.

---

## 11. MORK/PathMap Integration

MORK (Meta Operations for Rholang Kernel) provides the storage layer for MeTTa facts and rules. It bridges MeTTa's symbolic world with Rholang's byte-oriented tuplespace.

- **PathMap**: A trie-map for canonical fact/rule storage. Tries enable O(path_length) prefix queries — "find all facts matching `(person *)`" is a single trie walk vs O(n) scan with a hash table. De Bruijn encoding maps variables to positional indices for pattern matching. Byte-level pattern matching avoids deserialization during search.
- **MORK special forms**: Rule execution and space manipulation operations that operate directly on PathMap storage — avoiding the MettaValue→bytes→MettaValue round-trip for internal operations
- **`ExprZipper`**: Zero-copy cursor-based traversal of MORK-encoded expressions. Navigates the trie without allocating intermediate MettaValue objects.
- **Monotonic epoch counter** (`AtomicU64`): Prevents ABA cache invalidation — if an environment is dropped and a new one allocated at the same address, the epoch (never reused, only incremented) ensures all caches are invalidated. Without this, stale symbol IDs from a previous environment could silently corrupt lookups.

---

## 12. Language Surface Area

MeTTaTron implements the full MeTTa HE specification plus extensions for MORK integration and advanced control flow.

- **109 special forms** across 18 categories: rules & evaluation, conditionals, binding & sequencing, functions, error handling, type system, space operations, state operations, nondeterminism, list/atom operations, higher-order, tuple operations, set operations (multiset), string/I/O, memoization, module system, alpha equivalence, testing/assertions, and MORK forms. Special forms have lazy evaluation semantics (arguments evaluated on demand by the form's implementation).
- **42 grounded functions** (eager evaluation): arithmetic (9), math (15), trigonometry (6), float classification (2), comparison (6), boolean (4). Grounded functions are eager because they need concrete numeric/boolean values before computation — arguments are fully evaluated before the function is applied.

---

## 13. Key Design Patterns

Each pattern addresses a specific bottleneck identified through profiling or a semantic requirement of MeTTa's nondeterministic evaluation model.

| Pattern                       | Purpose                                                                                |
|-------------------------------|----------------------------------------------------------------------------------------|
| Zero-conversion generics      | Same `MettaValue` type across all pipeline stages — no marshalling                     |
| NaN-boxing + slab fallback    | Primitives inline in 64 bits; compounds in GC-managed arena                            |
| Copy-on-Write environments    | O(1) clone for nondeterministic branching; CoW on first mutation                       |
| Iterative trampoline (CEK)    | Work-stack + 60+ continuations replace recursion — PDA-like finite control + unbounded stack; enables TCO and parallel dispatch |
| Two-tier hash cache           | L1 direct-mapped + L2 HashMap — eliminates O(tree) hashing                             |
| Hash-consing                  | O(1) pointer equality for structurally identical ground expressions                    |
| Parallel budget + depth decay | `AtomicU32` budget, exponential decay `4^(-depth)`, queue pressure backoff             |
| SG1 binding-aware matching    | WAM-style lazy resolution — matches without materializing bound expressions            |
| Lock-free slab allocator      | Treiber stack (128-bit ABA-safe CAS) — no mutex contention on allocation hot path      |
| Session-based GC              | Arena lifecycle tied to evaluation sessions — prevents premature collection            |
| Three-tier alloc fallback     | Thread-local (0 atomics) → batch Treiber pop (1 RwLock) → CAS bump — amortizes contention |
| Epoch-based TOCTOU safety     | Per-slot monotonic epoch + snapshot epoch filtering — prevents freeing re-allocated slots   |
| Quiescent-state GC protocol   | ACTIVE_EVALUATORS + GC_IN_PROGRESS atomics — sub-ms snapshot without stop-the-world        |
| Four-term hill climber        | Throughput × queue depth × slab pressure × RSS pressure — adaptive thread pool scaling |
| P² priority scheduler         | Centralized min-heap, SJF-like via P² runtime medians, age decay prevents starvation  |
| Tiered JIT compilation        | 4 tiers (interpret → bytecode → JIT1 → JIT2) with non-blocking background compilation  |
| Bloom filter fast rejection   | O(1) rejection for absent atoms/types before expensive trie lookups                    |

---

## 14. Metamath Proof Verification

- **Location**: `examples/mmverify/` — MeTTa-based Metamath proof verifier
- Demonstrates MeTTa's pattern matching power for formal verification: Metamath proofs are verified by matching proof steps against axiom schemas, a natural fit for MeTTa's rule-based evaluation
- Used as **PGO benchmark workload** because it exercises pattern matching, rule application, deep nesting, and nondeterministic branching in realistic proportions — representative of production symbolic reasoning workloads. PGO yields 20%+ speedup.
