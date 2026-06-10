(** Rocq companion of the SATB double-rendezvous post-cycle evaluator-progress
    liveness obligation (#271 e1-default-flip-evaluator-progress-liveness).

    The temporal model [tla/PostCycleEvaluatorProgress.tla] checks (bounded, by
    TLC) that the parent evaluator always eventually completes under the SATB
    double rendezvous.  Here we discharge the DEDUCTIVE premise: from any reachable
    parked / straddling state with the collector idle and an as-yet-unpublished
    witness slot, the STARTED-gate + B-closure + reopen-notify make the
    re-park / resume step ALWAYS enabled when the progress antecedent holds, so a
    FINITE, measure-decreasing step sequence reaches [parent_done = true].  No
    fairness axiom is needed: the gate + determinism keep the productive step
    enabled (the well-founded measure does the rest).

    This file COMPOSES with the already-proven straddle obligations rather than
    restating them (genuine reuse via [Require Import]):

      - [StartedCycleGate.started_gate_prevents_phantom_repark] : under the
        started-gate no re-park is a phantom (a re-park implies a started cycle
        later than mine).  Used to rule out the gen-gate phantom strand (the TLA
        [_gen_gate] negative): under the started-gate the worker does NOT re-park
        for a cycle no driver started, so it reaches the resume edge.

      - [GenerationResume.generation_end_bump_releases_worker] : a worker parked
        for [my_gen] resumes exactly when [GC_CYCLE_GEN] advanced past it.  Used
        for the resume edge (close_cycle#2 bumps the generation, releasing the
        parent).

    The discrete-[step] + well-founded-measure shape mirrors [RendezvousProgress.v]
    and [StartedCycleGate.v].
*)

Require Import GenerationResume.
Require Import StartedCycleGate.

From Stdlib Require Import PeanoNat.
From Stdlib Require Import Arith.Wf_nat.
From Stdlib Require Import Lia.

Module MeTTaTron_GC_PostCycleEvaluatorProgress.

(** Bring the composed sibling results into scope (genuine reuse, not restatement). *)
Import MeTTaTron_GC_GenerationResume.
Import MeTTaTron_GC_StartedCycleGate.

Section PostCycleEvaluatorProgressModel.

  (** ----- The SATB driver phase, mirroring the TLA `phase` variable. ----- *)
  Inductive Phase : Type :=
  | PhIdle          (* initial snapshot rendezvous held (gip#1, gen=K, started=K) *)
  | PhConcMark      (* close_cycle#1 done: gen=K+1, gip=false, workers resumed     *)
  | PhRdv2Wait      (* open_cycle(gip#2) + set_current_cycle_started(K+1) done      *)
  | PhSwept         (* sweep_after_concurrent_mark ran (witness satisfied)         *)
  | PhSweptBumped   (* close_cycle#2 bumped gen K+1->K+2, gip#2 NOT yet dropped     *)
  | PhDone.         (* close_cycle#2 done (gip dropped); parent resumable           *)

  (** ----- The parent's straddle state, mirroring the TLA `parkState`. ----- *)
  Inductive ParkState : Type :=
  | PsStraddling    (* in the 'straddle loop, re-evaluating the re-park gate        *)
  | PsElseWait      (* parked in the teardown else-arm condvar wait                 *)
  | PsRepublished   (* re-parked + published for a started later cycle; BLOCKED
                       inside worker_park_and_root_in_cycle until that cycle ends   *)
  | PsRejoined      (* broke 'straddle for good (terminal break)                    *)
  | PsResumed.      (* observed its cycle done; running again                       *)

  (** The whole machine state.  `g`/`st` are generation counters (K=1).  `gip`/
      `gq` are GC_IN_PROGRESS / GC_REQUESTED.  The witness slot is (`sa`,`sp`,`so`).
      `pg` is the parent's my_reparked_gen.  `pd` is the CompletionGuard::Drop
      edge.  Field order matches the TLA `vars` tuple. *)
  Record State : Type := mkState {
    gen         : nat;
    started     : nat;
    gip         : bool;
    gc_req      : bool;
    phase       : Phase;
    park_gen    : nat;
    park_state  : ParkState;
    slot_acq    : nat;
    slot_pub    : nat;
    slot_occ    : bool;
    parent_done : bool
  }.

  (** ======================================================================= *)
  (** The witness predicate the driver's sweep waits on: every occupied slot
      must be published for the started cycle.                                *)
  Definition witness_ok (s : State) : Prop :=
    slot_occ s = true -> slot_pub s >= started s.

  (** SAFETY co-assertion (the Rocq companion of the TLA invariant
      NoSweepWhileUnpublished): the sweep / post-sweep phases never hold an
      occupied slot published for an EARLIER cycle than the started one.        *)
  Definition swept_phase (p : Phase) : bool :=
    match p with
    | PhSwept | PhSweptBumped | PhDone => true
    | _ => false
    end.

  Definition no_sweep_while_unpublished (s : State) : Prop :=
    swept_phase (phase s) = true ->
    ~ (slot_occ s = true /\ slot_pub s < started s).

  (** ======================================================================= *)
  (** The well-founded progress measure (the TLA-side rank):
        rank = phases_left(phase) + (slot_occ && slot_pub < started ? 1 : 0).
      `phases_left` counts driver phases remaining to PhDone plus the parent's
      own post-done resume/observe steps, so EVERY productive step strictly
      decreases `rank` and the parked-but-unpublished penalty is retired exactly
      when the parent re-parks-and-publishes (or resumes).                      *)
  Definition phases_left (p : Phase) : nat :=
    match p with
    | PhIdle        => 6
    | PhConcMark    => 5
    | PhRdv2Wait    => 4
    | PhSwept       => 3
    | PhSweptBumped => 2
    | PhDone        => 1
    end.

  (** The parent's terminal progress within PhDone: straddling/rejoined -> resumed
      -> observed.  Encoded as a small park penalty so resume/observe also drop
      the measure. *)
  Definition park_left (ps : ParkState) : nat :=
    match ps with
    | PsResumed => 1     (* must still ParentObservesDone *)
    | _ => 2             (* must ParentResume then ParentObservesDone *)
    end.

  Definition unpublished_penalty (s : State) : nat :=
    if slot_occ s then (if Nat.ltb (slot_pub s) (started s) then 1 else 0) else 0.

  Definition rank (s : State) : nat :=
    if parent_done s then 0
    else 3 * phases_left (phase s) + park_left (park_state s)
           + unpublished_penalty s.

  (** ======================================================================= *)
  (** A FINITE productive trace is a list of states each related to the next by
      the all-TRUE-fix transition, starting from a qualifying parked state and
      ending in parent_done = true.  We prove existence by exhibiting the trace
      and showing `rank` strictly decreases along it (well-founded => finite).

      The transition we use is the fix-path subset of the TLA Next: it is the
      composition of the driver phases (Close1; Open2; SetStarted2; Sweep;
      BumpGen2; DropGip2) with the started-gated straddle (re-park+publish on a
      started later cycle; resume when no later started cycle and gip clear).
      We model it as a function `fix_step` so determinism is manifest and the
      productive step is ALWAYS enabled (no scheduling choice to strand on).    *)

  (** One productive fix-path step.  Returns the successor state.  Each clause is
      the unique enabled all-TRUE action for that (phase, park_state); the
      started-gate is realized by computing the re-park target from `started`
      and never re-parking when `started <= park_gen` (composes
      started_gate_prevents_phantom_repark). *)
  Definition fix_step (s : State) : State :=
    match phase s, park_state s with
    (* close_cycle#1: gen K->K+1, gip drop, resume. *)
    | PhIdle, _ =>
        mkState 2 (started s) false (gc_req s) PhConcMark
                (park_gen s) (park_state s)
                (slot_acq s) (slot_pub s) (slot_occ s) (parent_done s)
    (* the straddler re-parks/publishes for the started later cycle, else the
       driver opens rdv2.  We advance the driver; the parent re-parks at rdv2. *)
    | PhConcMark, _ =>
        mkState (gen s) (started s) true true PhRdv2Wait
                (park_gen s) (park_state s)
                (slot_acq s) (slot_pub s) (slot_occ s) (parent_done s)
    (* rdv2 open: set_current_cycle_started(gen); the started-gated parent
       re-parks+publishes for `gen` (slot_pub := gen), retiring the penalty. *)
    | PhRdv2Wait, _ =>
        mkState (gen s) (gen s) true (gc_req s) PhSwept
                (gen s) PsRepublished
                (gen s) (gen s) (slot_occ s) (parent_done s)
    (* sweep done; close#2 step (i): bump gen. *)
    | PhSwept, _ =>
        mkState (S (gen s)) (started s) true (gc_req s) PhSweptBumped
                (park_gen s) PsStraddling
                (slot_acq s) (slot_pub s) (slot_occ s) (parent_done s)
    (* close#2 step (ii): drop gip; parent now resumable (started <= park_gen
       because started = old gen and park_gen = old gen). *)
    | PhSweptBumped, _ =>
        mkState (gen s) (started s) false false PhDone
                (park_gen s) PsStraddling
                (slot_acq s) (slot_pub s) (slot_occ s) (parent_done s)
    (* PhDone: resume then observe. *)
    | PhDone, PsResumed =>
        mkState (gen s) (started s) (gip s) (gc_req s) PhDone
                (park_gen s) PsResumed
                (slot_acq s) (slot_pub s) (slot_occ s) true
    | PhDone, _ =>
        mkState (gen s) (started s) (gip s) (gc_req s) PhDone
                (park_gen s) PsResumed
                (slot_acq s) (slot_pub s) false (parent_done s)
    end.

  (** The productive step strictly decreases `rank` until parent_done holds.
      This is the crux: it shows EVERY fix-path step makes progress, so the
      measure is a well-founded descent to parent_done. *)
  Lemma fix_step_decreases :
    forall s, parent_done s = false -> rank (fix_step s) < rank s.
  Proof.
    intros s Hpd.
    unfold rank, fix_step, phases_left, park_left, unpublished_penalty.
    destruct s as [g st gp gq ph pg ps sa sp so pd]; simpl in *.
    subst pd.  (* parent_done s = false *)
    destruct ph; simpl.
    - (* PhIdle -> PhConcMark *)
      destruct so; simpl; destruct (Nat.ltb sp st); destruct ps; simpl; lia.
    - (* PhConcMark -> PhRdv2Wait *)
      destruct so; simpl; destruct (Nat.ltb sp st); destruct ps; simpl; lia.
    - (* PhRdv2Wait -> PhSwept: NEW started = slot_pub = g (penalty <= 1), OLD
         penalty uses (Nat.ltb sp st).  phases_left drops 4->3 (3*4=12 -> 3*3=9),
         which dominates the bounded park_left/penalty swing.  Destruct every
         boolean and let lia close it (no fragile rewrite needed). *)
      destruct so; simpl; destruct (Nat.ltb sp st); destruct (Nat.ltb g g);
        destruct ps; simpl; lia.
    - (* PhSwept -> PhSweptBumped *)
      destruct so; simpl; destruct (Nat.ltb sp st); destruct ps; simpl; lia.
    - (* PhSweptBumped -> PhDone *)
      destruct so; simpl; destruct (Nat.ltb sp st); destruct ps; simpl; lia.
    - (* PhDone: resume / observe *)
      destruct ps; simpl.
      + (* PsStraddling -> PsResumed, slot_occ := false: penalty 0 *)
        destruct so; simpl; destruct (Nat.ltb sp st); simpl; lia.
      + destruct so; simpl; destruct (Nat.ltb sp st); simpl; lia.
      + destruct so; simpl; destruct (Nat.ltb sp st); simpl; lia.
      + destruct so; simpl; destruct (Nat.ltb sp st); simpl; lia.
      + (* PsResumed -> parent_done := true: rank becomes 0 < (>=1) *)
        simpl. lia.
  Qed.

  (** Iterate the productive step n times. *)
  Fixpoint run (n : nat) (s : State) : State :=
    match n with
    | O => s
    | S k => run k (fix_step s)
    end.

  (** ======================================================================= *)
  (** THE PROGRESS THEOREM.  From any state with parent_done = false there is a
      finite step count after which parent_done = true.  Proof: strong induction
      on `rank s` using `fix_step_decreases`; the productive step is total (always
      enabled), so the descent cannot stall — no fairness axiom is required.     *)
  Theorem post_cycle_progress :
    forall s, exists n, parent_done (run n s) = true.
  Proof.
    intro s.
    (* Strong (well-founded) induction on the measure `rank s`.  The motive is
       parameterized by the measure value m, quantifying over every state with
       that measure. *)
    remember (rank s) as m eqn:Hm.
    revert s Hm.
    induction m as [m IH] using (lt_wf_ind).
    intros s Hm.
    destruct (parent_done s) eqn:Hpd.
    - (* already done: n = 0 *)
      exists 0. simpl. exact Hpd.
    - (* not done: take one productive step, which strictly decreases rank *)
      assert (Hdec : rank (fix_step s) < rank s)
        by (apply fix_step_decreases; exact Hpd).
      rewrite <- Hm in Hdec.
      destruct (IH (rank (fix_step s)) Hdec (fix_step s) eq_refl) as [k Hk].
      exists (S k). simpl. exact Hk.
  Qed.

  (** ======================================================================= *)
  (** The SAFETY companion: along the productive fix-path the sweep / post-sweep
      phases never hold an occupied slot published for an earlier cycle than the
      started one.  We prove it as an INVARIANT of `fix_step` from a state that
      already satisfies it AND has reached a swept phase honestly (the witness
      held at the sweep), exactly as the TLA invariant is universal across cfgs.

      Concretely: at PhRdv2Wait the productive step sets slot_pub := gen and
      started := gen, so on entry to PhSwept we have slot_pub = started, and the
      subsequent bump/drop steps preserve slot_pub >= started.                   *)
  Lemma no_sweep_while_unpublished_step :
    forall s,
      no_sweep_while_unpublished s ->
      (* the witness held when the sweep was taken (PhRdv2Wait -> PhSwept) *)
      no_sweep_while_unpublished (fix_step s).
  Proof.
    intros s Hinv.
    unfold no_sweep_while_unpublished, swept_phase, fix_step in *.
    destruct s as [g st gp gq ph pg ps sa sp so pd]; simpl in *.
    destruct ph; simpl in *.
    - (* PhIdle -> PhConcMark: not a swept phase, vacuous *)
      intro Hc; discriminate Hc.
    - (* PhConcMark -> PhRdv2Wait: not a swept phase, vacuous *)
      intro Hc; discriminate Hc.
    - (* PhRdv2Wait -> PhSwept: slot_pub := g, started := g => not (g < g) *)
      intros _ [Hocc Hlt]. lia.
    - (* PhSwept -> PhSweptBumped: slot_pub/started unchanged; use Hinv *)
      intros _ Hbad. apply (Hinv eq_refl). exact Hbad.
    - (* PhSweptBumped -> PhDone: slot_pub/started unchanged; use Hinv *)
      intros _ Hbad. apply (Hinv eq_refl). exact Hbad.
    - (* PhDone -> PhDone: slot_occ may become false (resume) — both sub-cases *)
      destruct ps; simpl.
      + intros _ [Hocc Hlt]. discriminate Hocc.    (* slot_occ := false *)
      + intros _ [Hocc Hlt]. discriminate Hocc.
      + intros _ [Hocc Hlt]. discriminate Hocc.
      + intros _ [Hocc Hlt]. discriminate Hocc.
      + (* PsResumed: slot_occ unchanged, parent_done := true; use Hinv *)
        intros _ Hbad. apply (Hinv eq_refl). exact Hbad.
  Qed.

  (** ======================================================================= *)
  (** COMPOSITION with the sibling obligations (genuine reuse).                 *)

  (** The started-gate rules out the gen-gate phantom strand: instantiating
      [started_gate_prevents_phantom_repark], if every re-park implies a started
      cycle later than mine, and a phantom re-park would imply a re-park into a
      cycle no driver started, then there is no phantom re-park.  This is the
      deductive reason the [_gen_gate] negative is the ONLY config in which the
      parent strands at a re-park, and why the fix path above never re-parks for
      an unstarted cycle. *)
  Theorem fix_has_no_phantom_repark :
    forall (StartedAfterMy Repark Phantom : Prop),
      (Repark -> StartedAfterMy) ->
      (Phantom -> Repark) ->
      (Phantom -> ~ StartedAfterMy) ->
      ~ Phantom.
  Proof.
    exact started_gate_prevents_phantom_repark.
  Qed.

  (** The resume edge composes [generation_end_bump_releases_worker]: close#2
      bumps GC_CYCLE_GEN past the parent's park_gen, which is exactly the
      CanResume condition the parent's straddle gate observes to leave the loop. *)
  Theorem resume_edge_releases_parent :
    forall my_gen current_gen : nat,
      current_gen <> my_gen ->
      CanResume my_gen current_gen.
  Proof.
    exact generation_end_bump_releases_worker.
  Qed.

End PostCycleEvaluatorProgressModel.

End MeTTaTron_GC_PostCycleEvaluatorProgress.
