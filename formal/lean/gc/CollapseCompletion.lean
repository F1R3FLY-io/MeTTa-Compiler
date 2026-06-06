/-!
Lean model of the E1 parallel completion-guard obligation.

The parallel dispatch and collapse workers signal completion through a single
RAII guard. The guard's destructor is the only worker decrement and runs on both
normal return and panic-unwind. TLC checks the temporal fairness/liveness
discriminator in `tla/CollapseCompletion.tla`; these theorems prove the
compositional premise used by the source.
-/

namespace MeTTaTron.GC.CollapseCompletion

inductive ExitPath where
  | normal : ExitPath
  | panic : ExitPath

variable {Worker : Type u}

def WorkerExited
    (Exit : Worker -> ExitPath -> Prop)
    (w : Worker) : Prop :=
  Exit w ExitPath.normal ∨ Exit w ExitPath.panic

def ParentWaitStranded
    (Spawned Dropped : Worker -> Prop) : Prop :=
  exists w, Spawned w ∧ Not (Dropped w)

theorem guard_drop_covers_worker_exit
    {Dropped : Worker -> Prop}
    {Exit : Worker -> ExitPath -> Prop}
    (dropOnNormal : forall {w : Worker}, Exit w ExitPath.normal -> Dropped w)
    (dropOnPanic : forall {w : Worker}, Exit w ExitPath.panic -> Dropped w) :
    forall {w : Worker}, WorkerExited Exit w -> Dropped w := by
  intro w hexited
  cases hexited with
  | inl hnormal => exact dropOnNormal hnormal
  | inr hpanic => exact dropOnPanic hpanic

theorem every_spawned_worker_drops_on_exit
    {Spawned Dropped : Worker -> Prop}
    {Exit : Worker -> ExitPath -> Prop}
    (everyWorkerExits : forall {w : Worker}, Spawned w -> WorkerExited Exit w)
    (dropOnNormal : forall {w : Worker}, Exit w ExitPath.normal -> Dropped w)
    (dropOnPanic : forall {w : Worker}, Exit w ExitPath.panic -> Dropped w) :
    forall {w : Worker}, Spawned w -> Dropped w := by
  intro w hspawned
  exact guard_drop_covers_worker_exit
    (Dropped := Dropped)
    (Exit := Exit)
    dropOnNormal
    dropOnPanic
    (everyWorkerExits hspawned)

theorem no_stranded_parent_wait_when_all_workers_drop
    {Spawned Dropped : Worker -> Prop}
    (allDropped : forall {w : Worker}, Spawned w -> Dropped w) :
    Not (ParentWaitStranded Spawned Dropped) := by
  intro hstranded
  cases hstranded with
  | intro w hw =>
      exact hw.right (allDropped hw.left)

theorem completion_guard_prevents_panic_strand
    {Spawned Dropped : Worker -> Prop}
    {Exit : Worker -> ExitPath -> Prop}
    (everyWorkerExits : forall {w : Worker}, Spawned w -> WorkerExited Exit w)
    (dropOnNormal : forall {w : Worker}, Exit w ExitPath.normal -> Dropped w)
    (dropOnPanic : forall {w : Worker}, Exit w ExitPath.panic -> Dropped w) :
    Not (ParentWaitStranded Spawned Dropped) := by
  exact no_stranded_parent_wait_when_all_workers_drop
    (Spawned := Spawned)
    (Dropped := Dropped)
    (every_spawned_worker_drops_on_exit
      (Spawned := Spawned)
      (Dropped := Dropped)
      (Exit := Exit)
      everyWorkerExits
      dropOnNormal
      dropOnPanic)

theorem panic_skip_completion_can_strand_parent_observation
    {Spawned Dropped : Worker -> Prop}
    {Exit : Worker -> ExitPath -> Prop}
    {ParentObserved : Prop}
    {w : Worker}
    (spawned : Spawned w)
    (panicExit : Exit w ExitPath.panic)
    (notDropped : Not (Dropped w))
    (parentRequiresComplete :
      ParentObserved -> forall {u : Worker}, Spawned u -> Exit u ExitPath.panic -> Dropped u) :
    Not ParentObserved := by
  intro observed
  have dropped : Dropped w := parentRequiresComplete observed spawned panicExit
  exact notDropped dropped

end MeTTaTron.GC.CollapseCompletion
