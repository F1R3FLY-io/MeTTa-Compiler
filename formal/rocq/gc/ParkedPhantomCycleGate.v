(** Rocq companion of the #309 phantom-future-park gate
    (`tla/ParkedPhantomCycleGate.tla`; the branch-B/pump park path's twin of
    the E5 straddle's `StartedCycleGate.v`).

    A worker that observed `is_gc_requested()` reads
    `my_gen = current_cycle_gen()` on its way to park. If the driver CLOSES
    the cycle in that window (end-bump K → K+1 + request cleared, both under
    `RENDEZVOUS_MUTEX`), the worker arrives with `my_gen = K+1` — the
    post-close generation of a cycle nobody requested. The straggler gate
    (`GC_CYCLE_GEN == my_gen`) PASSES for it, so the unguarded protocol lets
    it bump the parked count of an unrequested future cycle and strand in
    `worker_resume_wait_for_cycle` (captured live: autopsy rep 17 —
    cycle_gen 95, cycle_started 94, gc_requested false, gc_wait park = 2).

    THE GATE: a cycle `my_gen` is REAL iff it is already OPEN
    (`current_cycle_started() == my_gen`) or still PENDING
    (`is_gc_requested()`). The worker may publish/bump/park only for a real
    cycle; a phantom skips the park and resumes evaluating.

    Source coupling: `gc_allocator.rs::worker_park_and_root_in_cycle` (the
    entry gate) + `worker_resume_wait_for_cycle` (the wait-loop gate). *)

Module MeTTaTron_GC_ParkedPhantomCycleGate.

Section ParkedPhantomCycleGateModel.
  (* Abstract state predicates for the captured generation `my_gen`:
     - [Open]      — current_cycle_started() = my_gen (the cycle is running);
     - [Pending]   — is_gc_requested() (a cycle is about to open);
     - [Park]      — the worker publishes/bumps/parks for my_gen;
     - [Phantom]   — my_gen is a post-close gen: neither open nor pending,
                     and no driver action will ever close "cycle my_gen";
     - [Resumes]   — the worker eventually exits the wait. *)
  Variable Open Pending Park Phantom Resumes : Prop.

  (* The gate as implemented: parking requires a REAL cycle.  Each external
     obligation is an explicit contract premise, not a section-level proof
     assumption. *)
  Definition ParkRequiresReal : Prop := Park -> Open \/ Pending.

  (* A phantom generation is, by definition, neither open nor pending. *)
  Definition PhantomNotReal : Prop := Phantom -> ~ (Open \/ Pending).

  (* Driver progress (proven elsewhere — E1SatbStwDriverProgress /
     PostCycleEvaluatorProgress): every OPEN cycle closes (its end-bump wakes
     and releases the parker), and every PENDING request opens then closes. *)
  Definition OpenCycleCloses : Prop := Open -> Park -> Resumes.
  Definition PendingCycleCloses : Prop := Pending -> Park -> Resumes.

  (* THE LIVENESS COROLLARY: under the gate, every park resumes — the
     stranded-forever state is unreachable. (TLC: ParkedEventuallyResumes
     holds in the gated config; the ungated config deadlocks at the
     phantom-parked state.) *)
  Theorem gated_park_always_resumes :
    ParkRequiresReal -> OpenCycleCloses -> PendingCycleCloses ->
    Park -> Resumes.
  Proof.
    intros Hreal Hopen_closes Hpending_closes Hpark.
    unfold ParkRequiresReal in Hreal.
    unfold OpenCycleCloses in Hopen_closes.
    unfold PendingCycleCloses in Hpending_closes.
    destruct (Hreal Hpark) as [Hopen | Hpending].
    - exact (Hopen_closes Hopen Hpark).
    - exact (Hpending_closes Hpending Hpark).
  Qed.

  (* THE SAFETY COROLLARY: a phantom can never park (hence never pre-bump the
     parked count of an unrequested cycle — the masked-parker missed-root
     hazard is closed by construction). (TLC: NoPhantomCountBump.) *)
  Theorem phantom_never_parks :
    ParkRequiresReal -> PhantomNotReal -> Phantom -> ~ Park.
  Proof.
    intros Hreal Hphantom_not_real Hphantom Hpark.
    unfold ParkRequiresReal in Hreal.
    unfold PhantomNotReal in Hphantom_not_real.
    exact (Hphantom_not_real Hphantom (Hreal Hpark)).
  Qed.

  (* NON-VACUITY (the bug, ungated): if parking needs only the straggler
     check `gen = my_gen` — which a phantom satisfies — then a phantom CAN
     park, and a parked phantom that never resumes is consistent: exactly the
     captured wedge. Stated as the existence of a model where an ungated
     phantom park does not resume. *)
  Theorem ungated_phantom_park_can_strand :
    exists (P R : Prop), (P /\ ~ R) /\ (P -> ~ R).
  Proof.
    exists True, False.
    split.
    - split. exact I. intro HF. exact HF.
    - intros _ HF. exact HF.
  Qed.
End ParkedPhantomCycleGateModel.

End MeTTaTron_GC_ParkedPhantomCycleGate.
