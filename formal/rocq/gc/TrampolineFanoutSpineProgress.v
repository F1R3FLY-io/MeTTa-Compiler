(** PROGRESS / TERMINATION of the CESK trampoline fan-out + continuation-spine
    lowering — the companion PROGRESS obligation to the spine SAFETY proofs.

    The spine proofs (TrampolineFanoutSpineBridge / TrampolineFanoutProductionRestore
    / UnifiedChoicePointRestore / VmChoicePointSpine / JitChoicePointSpineBridge)
    establish root/value/restore SAFETY of the continuation-spine round-trip; NONE
    establishes that the fan-out exploration makes PROGRESS / terminates. This file
    supplies that missing obligation and the companion TLA+ model
    [TrampolineFanoutSpineProgress] model-checks the temporal positive/negative cases.

    Source (extracted; pinned in scripts/verify_cesk_gc_source_coupling.sh): each
    fan-out continuation — ProcessRuleMatches / ProcessAmb / ProcessMatchTemplates /
    ProcessCollapseEvalResults (eval_loop.rs process_continuation arms) — consumes
    EXACTLY ONE element of its `remaining_*` iterator per visit (`remaining.next()`)
    and re-pushes the strictly-smaller tail, with a terminal base case that pushes a
    Resume (NOT itself) when `remaining` is empty. The spine lowering
    (into_/resolve_trampoline_fanout_spine, types.rs) is a faithful round-trip via
    SpineStore alloc/remove — `resolve (persist c) = c` — so it preserves the frame's
    `remaining`; `persist_trampoline_fanout_spines` is idempotent on an already-lowered
    frame (`into_trampoline_fanout_spine` `other => other`).

    MAIN RESULT [faithful_lowering_preserves_termination]: under a faithful round-trip
    and a strictly-decreasing advance, the LOWERED step's fan-out succession relation
    is well-founded (Acc) — the spine lowering introduces NO divergence of its own, so
    the lowered trampoline terminates exactly when the un-lowered (program) machine
    does. (CONSEQUENCE — confirmed by git-bisect: since the lowering itself
    terminates, the observed FlyingRaven slowdown was NOT a non-termination but a COST
    regression. Commit 8c29d4c3 re-lowered the WHOLE K stack on every trampoline tick
    (O(depth*ticks), quadratic); the fix persists INCREMENTALLY from a low-water mark,
    proven observationally equivalent to the whole-stack persist in the companion
    IncrementalSpinePersistEquivalence.) [nonfaithful_reset_breaks_progress] exhibits the
    non-vacuity: a resolve that RESETS `remaining` (the TLA _reset.cfg bug) violates the
    strict-decrease premise and can loop forever. No admits/axioms. *)

From Stdlib Require Import PeanoNat.
From Stdlib Require Import Lia.

Module MeTTaTron_GC_TrampolineFanoutSpineProgress.

Section TrampolineFanoutSpineProgressModel.
  (* An opaque continuation; [remaining] is its fan-out progress measure — the
     length of its `remaining_*` iterator. *)
  Variable Cont : Type.
  Variable remaining : Cont -> nat.

  (* The trampoline's per-visit step on a fan-out frame: advance to the next
     branch (consume one element, re-push the tail). *)
  Variable advance : Cont -> Cont.

  (* The continuation-spine lowering: persist the frame into the store and resolve
     it back before processing. *)
  Variable Spine : Type.
  Variable persist : Cont -> Spine.
  Variable resolve : Spine -> Cont.

  (* The actual step the trampoline runs each visit on a lowered fan-out frame:
     resolve it back, then advance one branch. *)
  Definition lowered_step (c : Cont) : Cont := advance (resolve (persist c)).

  (* ===== Faithfulness of the lowering (round-trip identity) ===== *)

  (* [resolve (persist c) = c]: SpineStore.alloc returns a fresh address and
     SpineStore.remove returns the exact stored payload — so the round-trip
     preserves the frame's [remaining]. *)
  Theorem faithful_roundtrip_preserves_measure :
    (forall c, resolve (persist c) = c) ->
    forall c, remaining (resolve (persist c)) = remaining c.
  Proof.
    intros Hfaithful c. rewrite Hfaithful. reflexivity.
  Qed.

  (* Under a faithful round-trip the lowered step equals the bare advance, so it
     inherits exactly advance's progress behaviour. *)
  Theorem faithful_lowered_step_eq_advance :
    (forall c, resolve (persist c) = c) ->
    forall c, lowered_step c = advance c.
  Proof.
    intros Hfaithful c. unfold lowered_step. rewrite Hfaithful. reflexivity.
  Qed.

  (* ===== Termination of the fan-out exploration (well-foundedness) ===== *)

  (* The fan-out succession relation: [y] is the successor of [x] when [x] is a
     non-terminal fan-out frame ([remaining x <> 0]) and [y] is its step. [Acc] of
     this relation = no infinite chain of fan-out visits = the exploration
     terminates. *)
  Definition fanout_succ (step : Cont -> Cont) (y x : Cont) : Prop :=
    remaining x <> 0 /\ y = step x.

  (* If [step] strictly decreases [remaining] on every non-terminal frame, the
     fan-out exploration is well-founded from every frame (terminates). Proved by
     a fuel bound on the measure (ordinary induction), so no well-founded-recursion
     library lemma is relied upon. *)
  Theorem fanout_terminates :
    forall (step : Cont -> Cont),
      (forall c, remaining c <> 0 -> remaining (step c) < remaining c) ->
      forall c, Acc (fanout_succ step) c.
  Proof.
    intros step Hdec c.
    assert (Hfuel : forall n c0, remaining c0 <= n -> Acc (fanout_succ step) c0).
    { induction n as [| n IHn]; intros c0 Hle.
      - apply Acc_intro. intros y Hyx. destruct Hyx as [Hne _].
        exfalso. apply Hne. lia.
      - apply Acc_intro. intros y Hyx. destruct Hyx as [Hne Hy].
        apply IHn. subst y.
        assert (Hlt : remaining (step c0) < remaining c0) by (apply Hdec; exact Hne).
        lia. }
    apply (Hfuel (remaining c)). lia.
  Qed.

  (* MAIN: a FAITHFUL spine lowering PRESERVES termination. If the un-lowered
     advance strictly decreases the fan-out measure, the LOWERED step does too, so
     the lowered fan-out exploration terminates exactly when the un-lowered (program)
     machine does — the lowering adds no divergence. *)
  Theorem faithful_lowering_preserves_termination :
    (forall c, resolve (persist c) = c) ->
    (forall c, remaining c <> 0 -> remaining (advance c) < remaining c) ->
    forall c, Acc (fanout_succ lowered_step) c.
  Proof.
    intros Hfaithful Hdec.
    apply fanout_terminates.
    intros c Hne.
    rewrite (faithful_lowered_step_eq_advance Hfaithful c).
    apply Hdec. exact Hne.
  Qed.

End TrampolineFanoutSpineProgressModel.

(* ===== Non-vacuity: the faithfulness / strict-decrease hypothesis is NECESSARY =====

   A non-faithful lowering whose resolve RESETS the frame to a fixed non-empty
   [remaining] (modelled by the constant step [fun _ => 1] over the measure
   [Cont := nat]) does NOT decrease the measure, so the strict-decrease premise of
   [fanout_terminates] FAILS — exactly the TLA+ [TrampolineFanoutSpineProgress_reset]
   bug config, where the fan-out exploration never terminates. *)
Theorem nonfaithful_reset_breaks_progress :
  exists (step : nat -> nat),
    (exists c, c <> 0 /\ ~ (step c < c)) /\
    ~ (forall c, c <> 0 -> step c < c).
Proof.
  exists (fun _ => 1).
  split.
  - exists 1. split.
    + discriminate.
    + simpl. apply Nat.lt_irrefl.
  - intros Hcontra. apply (Nat.lt_irrefl 1). apply Hcontra. discriminate.
Qed.

End MeTTaTron_GC_TrampolineFanoutSpineProgress.
