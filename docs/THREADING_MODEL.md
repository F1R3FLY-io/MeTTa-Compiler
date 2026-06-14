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

The scheduler maximizes safe parallelism through a staged analysis pipeline:

1. Classification marks known pure, impure, and dynamic-eval heads.
2. The WFST transducer maps cost classes and descriptors to scheduling actions.
3. Wavefront construction groups independent tasks into earliest legal waves.
4. Fanout admission uses the transducer degree and runtime budget gates.
5. Queue pressure and depth quota prevent over-parallelization.

The formal lane covers the main scheduler obligations:

- `SchedulerClassificationLookup.v` and `SchedulerClassificationLookup.tla`
  keep insertion into the classification tables index-safe and ensure
  state-mutating heads are not treated as known pure.
- `SchedulerDynamicEvalGate.v` and `SchedulerDynamicEvalGate.tla` keep dynamic
  eval forms out of static-pure parallel classes.
- `SchedulerWavefrontParallelism.v` and `SchedulerWavefrontParallelism.tla`
  prove same-wave independence and earliest-ready admission.
- `SchedulerTransducerParallelism.v` and
  `SchedulerTransducerParallelism.tla` prove zero-cap safety and prevent
  underutilized branch-parallel actions.
- `SchedulerFanoutAdmissionCompleteness.v` and
  `SchedulerFanoutAdmissionCompleteness.tla` prove the requested degree is
  admitted when the runtime gates allow it.

These proofs are mandatory in `scripts/verify_cesk_gc_formal.sh`.

The wavefront reordering contract is:

```text
task can enter wave k
  -> every dependency is in a wave < k
task is ready for wave k and not already assigned
  -> task is assigned to wave k, not deferred
all tasks independent
  -> all tasks share wave 0
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

The diamond and all-independent positive models preserve dependency safety and
maximal ready-set admission. The cyclic same-wave negative model violates
`SameWaveIndependent`. The deferred-ready negative model preserves dependency
safety but violates `NoReadyTaskDeferred`, which is the optimal-parallelism side
of the proof.

The source-coupling check pins the corresponding Kahn-loop facts:

- Initial in-degree-zero tasks are pushed into the current wave.
- Dependents whose in-degree falls to zero are pushed into the next wave.
- Malformed task indices/dependencies and cyclic unresolved suffixes degrade to
  sequential waves.

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

The positive model preserves both non-overlap and stop-before-redispatch. The
no-stop negative model violates `StopPreventsRedispatch`, matching a worker that
clears `in_flight` without first publishing terminal state. The no-claim
negative model violates `NoOverlapDispatch`, matching a due placeholder that
submits a second worker before the first recurring worker finishes.

The source-coupling check pins the corresponding implementation facts:

- `dispatch_to_pool()` checks `stop_requested` before claiming work.
- The `compare_exchange(false, true, ...)` claim occurs before `pool.spawn_eval`.
- The failed-claim path requeues only and returns before any worker submission.
- Worker completion updates `stop_requested` before clearing `in_flight`.

## CESK And GC Interaction

The index-GC architecture is structural-root based. WorkPool and scheduler code
must not reintroduce a root registry or a cross-thread thread-local discovery
side channel.

The relevant runtime rules are:

- Per-worker roots come from that worker's reified machine state.
- `EvalGuard` marks active evaluator participation.
- Worker-spawn latches close non-rendezvous mid-loop collection once parallel
  workers can observe the heap.
- Batch outputs that cross the async gather boundary carry persistent
  safepoint-root handles until the caller copies them into `MettaState.output`.
- Dispatch/collapse fanout uses registered dispatch roots for live branches and
  result slots.

The scheduler/thread-pool/GC boundary is modeled by:

- `SchedulerGcBoundary.v` and `SchedulerGcBoundary.tla`
- `SchedulerSpawnLatch.v` and `SchedulerSpawnLatch.tla`
- `BatchHandoff.v` and `BatchHandoff.tla`
- `DriverRootUnion.v` and `DriverRootUnion.tla`
- `CESKCollectorSafety.v`

The source-coupling script pins these proof obligations to the implementation.

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
