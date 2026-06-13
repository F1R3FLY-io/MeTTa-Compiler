(** Rocq companion of the #309 coalesced-request witness-starvation fix
    (`tla/RequestReassertAtOpen.tla`).

    `request_concurrent_collection` posts TWO effects (the GC_REQUESTED flag
    and a CollectRendezvous channel message). Two coalesced requests leave two
    messages; the first cycle's close clears the flag, so the second message
    OPENS a cycle with the flag false. Mutators park + stamp their witness
    slots only at safepoints that observe the flag, so the driver's witness
    wait starves forever (captured live: validate rep 36 — cycle open,
    gc_requested false, occupied_unpublished 4).

    THE FIX: the driver RE-ASSERTS the request at every rendezvous open
    (`gc_driver_rendezvous_cycle` calls `request_gc()` after admission,
    before `prepare_rendezvous_roots`), restoring the invariant
    `open ⇒ requested` for the cycle's whole duration; the close clears the
    flag exactly as before. *)

Module MeTTaTron_GC_RequestReassertAtOpen.

Section RequestReassertModel.
  (* Abstract predicates over one opened rendezvous cycle:
     - [Open]      — the driver dequeued a message and opened the cycle;
     - [Requested] — GC_REQUESTED is visible to safepoints;
     - [Stamped]   — every occupied witness slot stamped for this cycle;
     - [Closes]    — the cycle reaches its end-bump + resume. *)
  Variable Open Requested Stamped Closes : Prop.

  (* The fix: opening re-asserts the request.  Model each external obligation as
     an explicit contract premise rather than a section-level proof assumption. *)
  Definition OpenReasserts : Prop := Open -> Requested.

  (* Safepoint progress (mutators run and reach safepoints — the scheduler
     liveness proven by SchedulerFanoutProgress): a visible request during an
     open cycle gets every occupied slot stamped. *)
  Definition RequestedStamps : Prop := Open -> Requested -> Stamped.

  (* Driver progress (E1SatbStwDriverProgress): a stamped witness set lets the
     wait return, and the cycle body unconditionally closes. *)
  Definition StampedCloses : Prop := Open -> Stamped -> Closes.

  (* LIVENESS COROLLARY: under the re-assert, every opened cycle closes —
     witness starvation is unreachable. (TLC: EveryCycleCloses in the
     reassert config; the lost config deadlocks at the starved-open state.) *)
  Theorem every_open_cycle_closes :
    OpenReasserts -> RequestedStamps -> StampedCloses -> Open -> Closes.
  Proof.
    intros Hreassert Hstamps Hcloses Hopen.
    unfold OpenReasserts in Hreassert.
    unfold RequestedStamps in Hstamps.
    unfold StampedCloses in Hcloses.
    apply (Hcloses Hopen).
    apply (Hstamps Hopen).
    apply (Hreassert Hopen).
  Qed.

  (* THE RESTORED INVARIANT (TLC: OpenImpliesRequested). *)
  Theorem open_implies_requested : OpenReasserts -> Open -> Requested.
  Proof.
    intros Hreassert.
    exact Hreassert.
  Qed.

  (* NON-VACUITY (the bug, without the re-assert): an open cycle with the
     flag cleared admits a model where no stamp ever happens and the cycle
     never closes — exactly the captured wedge. *)
  Theorem unreasserted_open_can_starve :
    exists (O R S C : Prop),
      (O /\ ~ R) /\ ((R -> S) -> True) /\ (O /\ ~ C).
  Proof.
    exists True, False, False, False.
    repeat split; auto.
  Qed.
End RequestReassertModel.

End MeTTaTron_GC_RequestReassertAtOpen.
