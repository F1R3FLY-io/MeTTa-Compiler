# MeTTaTron Threading Model

This document describes the current MeTTaTron threading path used by Rholang
integration and parallel MeTTa evaluation. It is tied to the formal proof lane in
`formal/rocq/gc/` and `tla/`; when the implementation changes, the proof,
source-coupling checks, and this document must move together.

## Overview

MeTTaTron separates async coordination from CPU-bound MeTTa evaluation:

1. Rholang owns the async runtime and calls into MeTTa through synchronous or
   async integration entry points.
2. MeTTaTron batches independent eval expressions in `run_state_async()`.
3. CPU-bound eval work is submitted to the global eval `WorkPool`.
4. WorkPool workers execute `eval_trampoline()` and publish results through a
   scatter-gather barrier.
5. CESK/index-GC safety is maintained with structural roots, `EvalGuard`,
   worker-spawn latches, and persistent result-root handles where results cross
   non-participant async boundaries.

The current eval path does not use Tokio `spawn_blocking` for MeTTa eval tasks.
The live CPU-bound path is the unified WorkPool in
`src/backend/models/work_pool.rs`.

## Runtime Shape

```text
Rholang async runtime
  |
  +-- run_state_async()
      |
      +-- batch consecutive independent eval expressions
      |
      +-- global eval WorkPool
          |
          +-- PriorityQueue
          |   +-- age-refreshed scoring
          |   +-- FIFO tie-break by sequence
          |   +-- condition-variable wakeup after enqueue
          |
          +-- WorkPool workers
              +-- EvalGuard::enter()
              +-- eval_trampoline()
              +-- result publication
```

The WorkPool is allocated lazily through `GLOBAL_EVAL_POOL`. Allocation creates
the priority queue, worker park records, CPU-state records, and empty worker
slots. `global_eval_pool()` then calls `start_init()`, which runs
`spawn_all_workers()` exactly once before returning the pool reference. This
first-access startup rule is source-coupled in
`scripts/verify_cesk_gc_source_coupling.sh`.

## Priority-Queue Fairness Contract

WorkPool queue ordering must not starve older work behind newer high-priority
work:

```text
queued task ages
  -> score is recomputed before dequeue
  -> age can make old work outrank newer work
equal recomputed scores
  -> lower sequence number wins
```

This is formally modeled by:

- `formal/rocq/gc/SchedulerPriorityFairness.v`
- `tla/PriorityQueueAging.tla`
- `tla/MC_PriorityQueueAging_refresh.cfg`
- `tla/MC_PriorityQueueAging_stale.cfg`

Rocq proves that increased age strictly improves a task's recomputed priority
and that equal recomputed scores fall back to FIFO sequence order. TLC preserves
`OldPopsAfterAging` when the score is refreshed at pop time; the stale-score
negative model violates `OldPopsAfterAging` by popping newer high-priority work
after an older task has aged enough to run first.

The source-coupling check pins the implementation to that contract:

- `PriorityQueue::pop()` refreshes queued scores before selection.
- `ScoredTask::cmp()` uses the sequence number as the FIFO tie-break.

## Lifecycle-Accounting Contract

WorkPool capacity accounting is tied to worker park-state transitions:

```text
try_unpark(active worker)
  -> no aggregate active-count change
try_unpark(parked worker)
  -> active_count increments once
try_park(parked worker)
  -> no aggregate active-count change
try_park(active worker above min)
  -> active_count decrements once
respawn parked/dead replacement unparked
  -> active_count increments once
```

The invariant is:

```text
active_count + parked_count == max_workers
```

This is formally modeled by:

- `formal/rocq/gc/WorkPoolLifecycle.v`
- `tla/WorkPoolLifecycle.tla`
- `tla/MC_WorkPoolLifecycle_fixed.cfg`
- `tla/MC_WorkPoolLifecycle_double_unpark_bug.cfg`
- `tla/MC_WorkPoolLifecycle_respawn_bug.cfg`

The positive model preserves `CapacityConsistent`. The double-unpark negative
model violates `CapacityConsistent` when a caller counts a single unpark
transition twice. The respawn negative model violates `CapacityConsistent` when
a parked replacement is respawned unparked without incrementing the aggregate
active count.

The source-coupling check pins the corresponding implementation facts:

- WorkPool and AdaptiveGcPool expose transition-returning `try_park()` and
  `try_unpark()` operations.
- Scaling paths hold `scale_lock` before park/unpark transition checks.
- `unpark_n()`, `park_n()`, and `check_and_respawn_workers()` update aggregate
  active counts only under successful transition-return checks.

## Startup-Drain Contract

The WorkPool must preserve eval tasks submitted during the startup window:

```text
submit task before workers are ready
  -> task is retained in PriorityQueue
  -> workers start
  -> workers pop retained tasks
  -> every retained eval task completes or reports its own panic path
```

This is formally modeled by:

- `formal/rocq/gc/WorkPoolStartupDrain.v`
- `tla/WorkPoolStartupDrain.tla`
- `tla/WorkPoolStartupDrain_all.cfg`
- `tla/WorkPoolStartupDrain_no_start.cfg`
- `tla/WorkPoolStartupDrain_lossy_enqueue.cfg`

The positive model proves retained startup tasks drain after workers start. The
negative no-start model violates the eventual-drain property. The negative
lossy-enqueue model violates `AllSubmittedComplete`.

The source-coupling check pins the corresponding implementation facts:

- `PriorityQueue::push()` pushes before `notify_one()`.
- `WorkPool::spawn_eval()` enqueues eval tasks without a drop/backpressure path.
- `WorkPool::start_init()` calls `spawn_all_workers()` under `WORK_POOL_INIT`.
- `global_eval_pool()` calls `start_init()` before the scaling monitor starts.
- `work_pool_worker_loop()` waits for unpark and then pops from the queue.
- `test_async_init_tasks_drain()` remains present as an implementation smoke
  test for the abstract proof.

## Panic-Isolation Contract

WorkPool workers must keep draining the queue after a task panic or an internal
accounting panic:

```text
queued task panics
  -> PriorityTask::execute catches it
  -> runtime is reported as zero
  -> runtime/WFST accounting is skipped
  -> worker heartbeat is still published
  -> worker loops to the next queued task

runtime tracking or WFST accounting panics
  -> worker-loop outer catch handles it
  -> worker loops to the next queued task
```

This is formally modeled by:

- `formal/rocq/gc/WorkPoolPanicIsolation.v`
- `tla/WorkPoolPanicIsolation.tla`
- `tla/WorkPoolPanicIsolation_task_caught.cfg`
- `tla/WorkPoolPanicIsolation_accounting_caught.cfg`
- `tla/WorkPoolPanicIsolation_no_inner.cfg`
- `tla/WorkPoolPanicIsolation_no_outer.cfg`

The missing-inner-catch negative model violates
`TaskPanicPublishesHeartbeat`. The missing-outer-catch negative model violates
eventual completion of the next queued task.

## Parallel Eval Batching

`src/rholang_integration.rs` batches consecutive independent eval expressions.
Rule definitions and ground facts force synchronization points because they may
change the environment used by later evals.

For each batch item:

1. `evaluate_batch_parallel_arena()` clones the environment needed by the item.
2. It submits a WorkPool eval task with `TaskTypeId::Eval(0)` and normal
   priority.
3. The worker enters `EvalGuard`.
4. The worker runs `eval_trampoline()`.
5. The worker stores its `BatchOutcome` in its preallocated result slot.
6. A `BatchCompletionGuard` decrements the shared completion counter on every
   exit path, including panic unwind.
7. The caller waits on the completion condvar and then drains every slot.

The batch-completion obligation is modeled by:

- `formal/rocq/gc/RholangBatchCompletion.v`
- `tla/RholangBatchCompletion.tla`

The negative panic model shows that a tail-position decrement can strand the
parent if `eval_trampoline()` panics before the decrement. The live source
therefore uses the RAII completion guard.

## Scheduler Parallelism

The production scheduler maximizes safe fanout parallelism through the active
dispatch pipeline:

1. Classification marks known pure, impure, and dynamic-eval heads.
2. The WFST transducer maps cost classes and descriptors to scheduling actions.
3. Purity/dynamic-eval gates reject side-effecting or code-evaluating branches.
4. Fanout admission uses the transducer degree and runtime budget gates.
5. Queue pressure and depth quota prevent over-parallelization.

Current production fanout guarantee: admitted pure fanout dispatches every
branch/item slot directly through `parallel_dispatch()` or
`parallel_collapse_dispatch()`. `compute_wavefront()` is verified as a
scheduler-library primitive for dependency-DAG grouping, but the 2026-06-14
activation audit found no production caller; it is not currently an active
production fanout stage.

The formal lane covers the main scheduler obligations:

- `SchedulerClassificationLookup.v` and `SchedulerClassificationLookup.tla`
  keep insertion into the classification tables index-safe and ensure
  state-mutating heads are not treated as known pure.
- `SchedulerDynamicEvalGate.v` and `SchedulerDynamicEvalGate.tla` keep dynamic
  eval forms out of static-pure parallel classes.
- `SchedulerWavefrontParallelism.v` and `SchedulerWavefrontParallelism.tla`
  prove same-wave independence and earliest-ready admission for the
  scheduler-library wavefront primitive.
- `SchedulerEffectConflictCompleteness.v` and
  `SchedulerEffectConflictCompleteness.tla` prove that dependency-DAG grouping
  is same-wave conflict-free only when callers encode every data/effect conflict
  as a dependency edge.
- `SchedulerDirectFanoutWavefrontRefinement.v` and
  `SchedulerDirectFanoutWavefrontRefinement.tla` prove that production direct
  rule-match fanout refines the wavefront model only for the all-independent
  single-wave case; dependency-bearing DAGs require the general wavefront
  builder.
- `SchedulerTransducerParallelism.v` and
  `SchedulerTransducerParallelism.tla` prove zero-cap safety and prevent
  underutilized branch-parallel actions.
- `SchedulerFanoutAdmissionCompleteness.v` and
  `SchedulerFanoutAdmissionCompleteness.tla` prove the requested degree is
  admitted when the runtime gates allow it.
- `SchedulerActiveFanoutGate.v` and `SchedulerActiveFanoutGate.tla` compose
  the active production rule-match fanout gates: branch threshold, WFST degree,
  purity/dynamic-eval blocking, depth, pool availability, and budget.
- `CollapseFanoutAdmissionCompleteness.v` and
  `CollapseFanoutAdmissionCompleteness.tla` prove the same input-completeness
  contract for `collapse` and `collapse-bind`.
- `ThreadingEndToEndInterleaving.v` and
  `ThreadingEndToEndInterleaving.tla` compose scheduler dependency waves,
  direct-fanout maximality, active-worker GC roots, closed worker admission, and
  recurring-cron in-flight claims into one small interleaving envelope.  The
  envelope also includes cron startup delivery, so a task submitted through the
  returned handle after the ready receiver observes startup is rejected if the
  cron event loop has no `CheckEvents` or `DrainChannel` polling path.  The same
  envelope composes WorkPool startup drain and panic isolation: startup
  submissions must be retained until workers drain them, task panics must
  publish the heartbeat path, and accounting panics must not kill the worker
  needed for subsequent queued work.  It also composes the GC-facing scheduler
  boundary for live-dispatch and async batch roots, so sweep rejects missing
  dispatch or batch root publication the same way it rejects missing active
  worker roots.  Its active direct-fanout obligation requires branch threshold,
  WFST degree, purity/dynamic-eval, depth, pool, budget, and complete-dispatch
  gates before `DirectFanout` can contribute to maximal same-wave parallelism.

These proofs are mandatory in `scripts/verify_cesk_gc_formal.sh`.

The classifier and dynamic-eval gate contract is:

```text
insert head into L2 classification table
  -> extend that head's contiguous range
  -> shift every later L1 range start
state-mutating or random head
  -> not in known-pure head set
dynamic eval head (`eval`, `!`, `evalc`)
  -> not in known-pure head set
  -> blocks no-budget parallel dispatch before strict-I/O filtering
```

This contract is checked by:

- `formal/rocq/gc/SchedulerClassificationLookup.v`
- `tla/SchedulerClassificationLookup.tla`
- `tla/MC_SchedulerClassificationLookup_shift.cfg`
- `tla/MC_SchedulerClassificationLookup_no_shift.cfg`
- `formal/rocq/gc/SchedulerDynamicEvalGate.v`
- `tla/SchedulerDynamicEvalGate.tla`
- `tla/MC_SchedulerDynamicEvalGate_fixed.cfg`
- `tla/MC_SchedulerDynamicEvalGate_missing.cfg`

The classifier positive model preserves disjoint L2 ranges after insertion; the
no-shift negative model violates `RangesDisjoint`. The dynamic-eval positive
model preserves `NoDynamicEvalParallelBypass`; the missing-gate negative model
violates it by admitting a hidden dynamic eval body into the no-budget parallel
path.

The source-coupling check pins the corresponding implementation facts:

- `SchedulerAutomaton` inserts a new L2 entry and shifts later L1 starts.
- `random-int`, `random-float`, `eval`, and `!` are absent from known-pure
  classifications.
- `DYNAMIC_EVAL_HEADS` contains `eval`, `!`, and `evalc`.
- `body_blocks_parallel_dispatch()` checks state mutation, then dynamic eval,
  then strict-print-order I/O.
- CESK branch analysis marks `eval`, `!`, and `evalc` as impure/sequential.

The wavefront reordering contract is:

```text
caller constructs dependency graph
  -> every data/effect conflict is represented as an edge
task can enter wave k
  -> every dependency is in a wave < k
task is ready for wave k and not already assigned
  -> task is assigned to wave k, not deferred
all tasks independent
  -> all tasks share wave 0
production direct rule-match fanout
  -> valid wavefront refinement only for the all-independent single-wave case
dependency-bearing instruction DAG
  -> must use complete dependency/effect edges before claiming wavefront reorder
malformed dependency graph or unresolved cycle
  -> fallback is sequential, not one unsafe same-wave batch
```

This contract is checked by:

- `formal/rocq/gc/SchedulerWavefrontParallelism.v`
- `tla/SchedulerWavefrontParallelism.tla`
- `tla/MC_SchedulerWavefrontParallelism_diamond.cfg`
- `tla/MC_SchedulerWavefrontParallelism_independent.cfg`
- `tla/MC_SchedulerWavefrontParallelism_cycle.cfg`
- `tla/MC_SchedulerWavefrontParallelism_deferred.cfg`
- `formal/rocq/gc/SchedulerEffectConflictCompleteness.v`
- `tla/SchedulerEffectConflictCompleteness.tla`
- `tla/MC_SchedulerEffectConflictCompleteness_complete.cfg`
- `tla/MC_SchedulerEffectConflictCompleteness_no_conflicts.cfg`
- `tla/MC_SchedulerEffectConflictCompleteness_missing_edge.cfg`
- `formal/rocq/gc/SchedulerDirectFanoutWavefrontRefinement.v`
- `tla/SchedulerDirectFanoutWavefrontRefinement.tla`
- `tla/MC_SchedulerDirectFanoutWavefrontRefinement_independent.cfg`
- `tla/MC_SchedulerDirectFanoutWavefrontRefinement_dependent_missing_gate.cfg`
- `tla/MC_SchedulerDirectFanoutWavefrontRefinement_partial_dispatch.cfg`

The diamond and all-independent positive models preserve dependency safety and
maximal ready-set admission. The cyclic same-wave negative model violates
`SameWaveIndependent`. The deferred-ready negative model preserves dependency
safety but violates `NoReadyTaskDeferred`, which is the optimal-parallelism side
of the proof. The effect-conflict positive models preserve same-wave conflict
freedom when conflict edges are complete; the missing-edge negative model
violates `ConflictEdgesCovered`. The direct-fanout refinement positive model
preserves maximal single-wave dispatch for independent branch sets; the
dependent-DAG negative violates `DirectOnlyForIndependentWavefront`, and the
partial-dispatch negative violates `DirectMatchesWavefrontMaxParallelism`.

The source-coupling check pins the corresponding Kahn-loop facts:

- `wavefront.rs` states that production direct fanout implements only the
  all-independent refinement without calling `compute_wavefront()`.
- Initial in-degree-zero tasks are pushed into the current wave.
- Dependents whose in-degree falls to zero are pushed into the next wave.
- Malformed task indices/dependencies and cyclic unresolved suffixes degrade to
  sequential waves.
- `WavefrontTask.dependencies` documents the caller obligation to include both
  data dependencies and effect-conflict edges.

The transducer/fanout admission contract is:

```text
WFST degree == 1
  -> branch set stays sequential
WFST degree > 1
  -> branch set may try purity/depth/pool/budget gates
active rule-match fanout dispatch
  -> branch threshold, WFST degree, purity, depth, pool, and budget gates pass
branch count <= safe cap
  -> branch-aware transduction uses every available branch
max_parallel == 0
  -> degree degrades to sequential 1, never invalid 0
admitted branch fanout
  -> every branch slot is represented in the dispatched range
admitted collapse fanout
  -> every collapse item is represented in the dispatched range
```

This contract is checked by:

- `formal/rocq/gc/SchedulerTransducerParallelism.v`
- `tla/SchedulerTransducerParallelism.tla`
- `tla/MC_SchedulerTransducerParallelism_zero_cap.cfg`
- `tla/MC_SchedulerTransducerParallelism_zero_cap_bug.cfg`
- `tla/MC_SchedulerTransducerParallelism_underutilized.cfg`
- `formal/rocq/gc/SchedulerFanoutAdmissionCompleteness.v`
- `tla/SchedulerFanoutAdmissionCompleteness.tla`
- `tla/MC_SchedulerFanoutAdmissionCompleteness_all.cfg`
- `tla/MC_SchedulerFanoutAdmissionCompleteness_partial.cfg`
- `tla/MC_SchedulerFanoutAdmissionCompleteness_missing_degree.cfg`
- `formal/rocq/gc/SchedulerActiveFanoutGate.v`
- `tla/SchedulerActiveFanoutGate.tla`
- `tla/MC_SchedulerActiveFanoutGate_all.cfg`
- `tla/MC_SchedulerActiveFanoutGate_missing_purity.cfg`
- `tla/MC_SchedulerActiveFanoutGate_missing_budget.cfg`
- `tla/MC_SchedulerActiveFanoutGate_partial_dispatch.cfg`
- `formal/rocq/gc/CollapseFanoutAdmissionCompleteness.v`
- `tla/CollapseFanoutAdmissionCompleteness.tla`
- `tla/MC_CollapseFanoutAdmissionCompleteness_all.cfg`
- `tla/MC_CollapseFanoutAdmissionCompleteness_partial.cfg`
- `tla/MC_CollapseFanoutAdmissionCompleteness_missing_threshold.cfg`

The transducer positive model preserves nonzero degree and maximal-before-cap
use of available branch parallelism. Its zero-cap negative violates
`NonZeroDegree`; its underutilized negative violates `MaximalBeforeCap`. The
branch fanout positive model preserves `CompleteAdmittedFanout`; the partial
spawn and missing-degree negatives violate `CompleteAdmittedFanout` and
`DegreeGateRequired`. The active fanout positive model preserves
`CompleteDispatch`; the missing-purity, missing-budget, and partial-dispatch
negatives violate `NoDispatchWithoutPurityGate`,
`NoDispatchWithoutBudgetGate`, and `CompleteDispatch`. The collapse fanout
positive model preserves `CompleteAdmittedCollapse`; the partial spawn and
missing-threshold negatives violate `CompleteAdmittedCollapse` and
`ThresholdGateRequired`.

The source-coupling check pins the corresponding implementation facts:

- `transduce_with_branches()` computes `safe_max_parallel = max_parallel.max(1)`
  and then uses `branch_count.min(safe_max_parallel)`.
- `SchedulingAction::parallelism_degree` is documented as an admission degree.
- Branch fanout budget requests use every extra branch:
  `(branch_count - 1)`.
- Active branch fanout reaches `try_acquire_budget()` only after the WFST
  degree/purity gate, depth gate, and active-worker pool gate have passed.
- `parallel_dispatch()` allocates result and completion slots for the full
  branch count before iterating every branch slot.
- Collapse fanout threshold is documented as an admission threshold, not a spawn
  cap.
- `parallel_collapse_dispatch()` allocates result and completion slots for the
  full item count before iterating every collapse item.

## Cron Manager

The cron manager is implemented by `src/backend/models/task_scheduler.rs`. It
uses a thread-local priority queue for due tasks and dispatches pooled work to
the WorkPool when appropriate.

Recurring pooled tasks are protected by an `in_flight` flag:

1. The cron thread claims `in_flight` before submitting pooled work.
2. If the recurring task is due while `in_flight` is already true, the cron
   thread requeues only the placeholder and does not dispatch overlap work.
3. If the worker returns `false` or panics, it stores the durable stop flag
   before clearing `in_flight`.
4. The worker clears `in_flight` on normal, stop, or panic paths.
5. A later due tick drops a stopped recurring task instead of redispatching it.

The formal lane covers this with:

- `formal/rocq/gc/CronRecurringDispatch.v`
- `tla/CronRecurringDispatch.tla`
- `tla/MC_CronRecurringDispatch_stop.cfg`
- `tla/MC_CronRecurringDispatch_no_stop.cfg`
- `tla/MC_CronRecurringDispatch_no_claim.cfg`
- `formal/rocq/gc/CronStartupDelivery.v`
- `tla/CronStartupDelivery.tla`
- `tla/MC_CronStartupDelivery_all.cfg`
- `tla/MC_CronStartupDelivery_no_ready_signal.cfg`
- `tla/MC_CronStartupDelivery_no_handle_sender.cfg`
- `tla/MC_CronStartupDelivery_no_poll_path.cfg`

The positive model preserves both non-overlap and stop-before-redispatch. The
no-stop negative model violates `StopPreventsRedispatch`, matching a worker that
clears `in_flight` without first publishing terminal state. The no-claim
negative model violates `NoOverlapDispatch`, matching a due placeholder that
submits a second worker before the first recurring worker finishes.

The startup-delivery model proves the ready-channel contract beyond endpoint
pairing. A task submitted through the returned `CronHandle` after the caller
observes the returned ready receiver is reachable by the event loop only if the
ready signal is sent from inside `CronStateMachine::run()` and the task channel
is polled by `CheckEvents` or `DrainChannel`. TLC rejects missing ready signal,
missing returned handle sender, and missing poll-path variants.

The source-coupling check pins the corresponding implementation facts:

- `dispatch_to_pool()` checks `stop_requested` before claiming work.
- The `compare_exchange(false, true, ...)` claim occurs before `pool.spawn_eval`.
- The failed-claim path requeues only and returns before any worker submission.
- Worker completion updates `stop_requested` before clearing `in_flight`.
- `spawn_cron_with_interval_name_and_pool()` creates the task and ready
  channels before spawning the cron thread, moves the receiver and ready sender
  into `CronStateMachine::new`, returns the handle sender and ready receiver,
  sends ready inside `run()` before the state-machine loop, and polls the task
  channel in both `CheckEvents` and `DrainChannel`.

## CESK And GC Interaction

The index-GC architecture is structural-root based. WorkPool and scheduler code
must not reintroduce a root registry or a cross-thread thread-local discovery
side channel.

The scheduler/thread-pool/GC boundary contract is:

```text
active eval worker
  -> publishes structural roots from its own reified CESK machine registers
live dispatch/collapse fanout
  -> is represented by live-dispatch anchors for branch inputs and result slots
async batch result not yet copied to output
  -> is protected by a persistent safepoint/driver-C root handle
driver collection root set
  -> contains worker publications, safepoint roots, live env anchors,
     and live dispatch anchors
driver begins participant snapshot
  -> closes worker admission before taking the snapshot
eval worker handoff
  -> stores the spawn latch before the worker can enter eval
```

This contract forbids a collector from treating scheduler-held values as dead
while any WorkPool, fanout, or async handoff path can still publish them. It
also forbids a newly admitted worker from appearing after the driver has closed
admission and before the collection snapshot is complete.

The boundary is modeled by:

- `formal/rocq/gc/SchedulerGcBoundary.v`
- `tla/SchedulerGcBoundary.tla`
- `tla/MC_SchedulerGcBoundary_all.cfg`
- `tla/MC_SchedulerGcBoundary_missing_worker.cfg`
- `tla/MC_SchedulerGcBoundary_missing_dispatch.cfg`
- `tla/MC_SchedulerGcBoundary_missing_batch.cfg`

The composed end-to-end envelope is modeled by:

- `formal/rocq/gc/ThreadingEndToEndInterleaving.v`
- `tla/ThreadingEndToEndInterleaving.tla`
- `tla/MC_ThreadingEndToEndInterleaving_safe.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_independent.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_active_fanout_missing_purity.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_active_fanout_missing_budget.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_active_fanout_partial_dispatch.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_missing_dependency.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_missing_worker_root.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_missing_dispatch_root.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_missing_batch_root.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_open_admission.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_unclaimed_cron.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_startup_no_poll.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_work_pool_lossy_enqueue.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_work_pool_no_inner_catch.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_work_pool_no_outer_catch.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_work_pool_uncapped_overflow.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_work_pool_double_unpark_bug.cfg`
- `tla/MC_ThreadingEndToEndInterleaving_work_pool_respawn_bug.cfg`

The positive dependency-bearing and independent configs preserve
`EndToEndSafe`. The negative configs violate it when dependency edges are
omitted, active direct fanout skips purity/budget/complete-dispatch gates,
active worker roots are omitted, dispatch or async batch roots are omitted,
worker admission stays open across the root snapshot, or recurring cron
dispatch submits without claiming `in_flight`.
The composed WorkPool discriminators additionally reject overflow spawning
that bypasses the live-worker cap, double-unpark accounting that counts one
parked worker twice, and respawning a parked replacement without incrementing
the aggregate active count.
- `tla/MC_SchedulerGcBoundary_admission_open.cfg`
- `formal/rocq/gc/SchedulerSpawnLatch.v`
- `tla/SchedulerSpawnLatch.tla`
- `tla/MC_SchedulerSpawnLatch_all.cfg`
- `tla/MC_SchedulerSpawnLatch_spawn_before_latch.cfg`
- `tla/MC_SchedulerSpawnLatch_missing_latch.cfg`
- `formal/rocq/gc/DriverRootUnion.v`
- `tla/DriverRootUnion.tla`
- `tla/MC_DriverRootUnion.tla`
- `tla/MC_DriverRootUnion_all.cfg`
- `tla/MC_DriverRootUnion_missing_env.cfg`
- `tla/MC_DriverRootUnion_missing_dispatch.cfg`
- `formal/rocq/gc/BatchHandoff.v`
- `tla/BatchHandoff.tla`
- `tla/MC_BatchHandoff_handle.cfg`
- `tla/MC_BatchHandoff_no_handle.cfg`
- `tla/MC_BatchHandoff_drop_before_copy.cfg`
- `formal/rocq/gc/CESKCollectorSafety.v`

The positive boundary model preserves `SchedulerBoundaryComplete`. Its missing
worker, missing dispatch, missing batch, and admission-open discriminator models
violate `SchedulerBoundaryComplete`, which means each root class and the closed
admission gate are independently necessary.

The driver-root-union model preserves `RootUnionComplete`. Its missing-env and
missing-dispatch discriminator models violate `RootUnionComplete`, which means
environment anchors and dispatch anchors are not optional implementation
details.

The batch-handoff model preserves `NoPublishedBatchResultFreed` when the async
handoff carries a persistent handle until output copy. The no-handle and
drop-before-copy discriminator models violate `NoPublishedBatchResultFreed`.

The spawn-latch model preserves `NoWorkerWithMidloopGateOpen`. The
spawn-before-latch and missing-latch discriminator models violate
`NoWorkerWithMidloopGateOpen`, matching the forbidden execution where a worker
can observe the heap before the non-rendezvous mid-loop collection gate is
closed.

The source-coupling script pins these proof obligations to the implementation:

- Public eval and `eval_with_tier` register driver-C roots before entering the
  eval guard.
- Live environments are registered through `register_live_env()` and collected
  by the environment root collectors.
- Live dispatch/collapse fanouts are registered through `register_live_dispatch`
  and collected by the dispatch anchor collectors.
- `BatchOutcome` carries a `_root_handle: Option<SafepointRootHandle>`, result
  vectors are registered before publication into gather slots, and the handle
  rides through sort/drain until after output copy.
- The driver closes worker admission before taking the participant snapshot.
- `EvalGuard` backs out when `GC_IN_PROGRESS` is observed.
- Dispatch and collapse workers wait for their dedicated rendezvous before
  entering `EvalGuard`.
- Every eval-worker handoff calls `note_worker_spawned()` before submitting the
  worker to the pool.

## Configuration

WorkPool worker counts are controlled by environment variables read by
`get_work_thread_config()`:

- `METTATRON_MIN_WORK_THREADS`: minimum active workers, default `1`.
- `METTATRON_MAX_WORK_THREADS`: maximum core workers, default `num_cpus * 2`.

The WorkPool may also spawn bounded overflow workers when the queue is pressured
and core workers are blocked. Overflow spawning is separately capped and modeled
by `WorkPoolOverflowCap.v` and `WorkPoolOverflowCap.tla`.

`EvalConfig::max_blocking_threads` is retained for callers that use
`apply_to_runtime_builder()` to configure a Tokio runtime builder. It is not the
WorkPool eval-worker cap.

## Verification Commands

Focused startup-drain checks:

```bash
systemd-run --user --scope -p MemoryMax=4G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
  rocq c -q -Q formal/rocq/gc "" formal/rocq/gc/WorkPoolStartupDrain.v
systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
  tlc -config tla/WorkPoolStartupDrain_all.cfg tla/WorkPoolStartupDrain.tla
systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
  tlc -config tla/WorkPoolStartupDrain_no_start.cfg tla/WorkPoolStartupDrain.tla
systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=200% --quiet \
  tlc -config tla/WorkPoolStartupDrain_lossy_enqueue.cfg tla/WorkPoolStartupDrain.tla
```

Full formal gate:

```bash
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=600% --quiet \
  bash scripts/verify_cesk_gc_formal.sh
```

The full harness also caps each TLC subprocess internally.

## Source Map

- `src/backend/models/work_pool.rs`: global eval WorkPool, worker startup,
  priority enqueue, scaling, overflow workers.
- `src/backend/priority_scheduler.rs`: priority queue ordering, score refresh,
  and condition-variable wakeups.
- `src/backend/models/task_scheduler.rs`: cron scheduler and pooled recurring
  dispatch.
- `src/rholang_integration.rs`: async Rholang batch evaluation and batch result
  handoff.
- `src/backend/scheduler/`: classification, transducer, wavefront, and
  admission logic.
- `docs/cesk-gc/formal-verification-ledger.md`: current proof ledger and
  verification history.
