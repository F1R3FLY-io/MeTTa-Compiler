(** EQUIVALENCE of INCREMENTAL (low-water-mark) trampoline fan-out spine
    persistence to the WHOLE-STACK persist it replaced — the CORRECTNESS
    obligation of the fix for the quadratic-persist regression
    (commit 8c29d4c3 -> 8b74952e).

    PROBLEM. 8c29d4c3 ("Verify production trampoline fanout spine restore")
    called persist_trampoline_fanout_spines(&mut continuations) at the top of
    EVERY trampoline tick, re-lowering the ENTIRE K stack each iteration —
    O(depth) work per tick, hence O(depth*ticks) = quadratic on deep
    nondeterministic evaluations (e.g. FlyingRaven PLN). The companion PROGRESS
    proof (TrampolineFanoutSpineProgress) already established that the lowering
    TERMINATES, so the observed FlyingRaven slowdown was a COST regression, not a
    non-termination (the diagnosis git-bisect then pinned to 8c29d4c3). The fix
    (eval_loop.rs) persists INCREMENTALLY from a `spine_persisted_len` low-water
    mark: each tick lowers only the frames pushed since the previous tick.

    OBLIGATION (this file). The incremental persist must be OBSERVABLY EQUIVALENT
    to the whole-stack persist it replaced — i.e. it must leave the SAME lowered
    K stack at every loop top — otherwise the speedup would come at the cost of a
    behavioural divergence. We prove exactly that.

    Source (extracted; pinned in scripts/verify_cesk_gc_source_coupling.sh):
      * [lower] = Continuation::into_trampoline_fanout_spine (types.rs ~2202): it
        maps a re-enterable fan-out frame to a stored-spine handle and is the
        identity (`other => other`) on an already-lowered or non-fan-out frame —
        hence IDEMPOTENT ([LowerIdempotent]).
      * [persist_from s from] = persist_trampoline_fanout_spines_from(stack, from)
        (types.rs ~2254): `let from = from.min(len);` then each slot `c` in
        `stack[from..]` is replaced by `lower c` — leaves the prefix `[0,from)`
        untouched, lowers the suffix `[from,len)`. Modelled exactly as
        `firstn from s ++ map lower
        (skipn from s)` (firstn/skipn saturate at len, matching `.min(len)`).
      * The trampoline loop (eval_loop.rs ~4200): each tick runs
        persist_from(stack, spine_persisted_len) then sets spine_persisted_len =
        stack.len() ([loop_top]); process_continuation only PUSHES ([op_push]);
        the Resume arm is the SOLE pop site and clamps spine_persisted_len =
        spine_persisted_len.min(continuations.len()) ([op_pop]).

    MAIN RESULT [incremental_eq_full_at_every_loop_top]: along EVERY reachable
    execution (any sequence of pushes and pops from any initial stack), at every
    loop top the incremental persist equals the whole-stack persist
    [persist_full]. Proved from a preserved invariant [Inv] (the persisted prefix
    is always lowered and the watermark never exceeds the stack length).

    NON-VACUITY [clamp_is_necessary]/[clamp_restores_equivalence]: dropping the
    Resume-arm clamp lets a frame pushed after a pop be SKIPPED by an over-large
    watermark and left un-lowered, breaking the equivalence; the clamp restores
    it. So the clamp (the part of the fix authored in eval_loop.rs) is
    load-bearing, not incidental. No admits/axioms. *)

From Stdlib Require Import List.
From Stdlib Require Import PeanoNat.
From Stdlib Require Import Lia.
Import ListNotations.

Module MeTTaTron_GC_IncrementalSpinePersistEquivalence.

Section IncrementalSpinePersistModel.
  (* A K-stack frame. [lower] = into_trampoline_fanout_spine. *)
  Variable Frame : Type.
  Variable lower : Frame -> Frame.

  (* IDEMPOTENCE of the lowering: lowering an already-lowered / non-fan-out frame
     is a no-op — exactly the `other => other` arm of into_trampoline_fanout_spine
     on a TrampolineFanoutSpine (or any non-fan-out) frame. *)
  Definition LowerIdempotent : Prop :=
    forall f, lower (lower f) = lower f.

  (* A frame is "lowered" when it is a fixpoint of [lower] (already a spine
     handle, or a non-fan-out frame the lowering leaves alone). *)
  Definition lowered (f : Frame) : Prop := lower f = f.

  (* Whole-stack persist (the 8c29d4c3 form): lower EVERY frame. *)
  Definition persist_full (s : list Frame) : list Frame := map lower s.

  (* Incremental persist from a low-water mark: leave the prefix [0,from)
     untouched, lower only the suffix [from,len). *)
  Definition persist_from (s : list Frame) (from : nat) : list Frame :=
    firstn from s ++ map lower (skipn from s).

  (* ===== Self-contained list helpers (proved by induction; no fragile
     stdlib-name dependencies beyond the ancient [firstn_skipn] / [map_app]) ===== *)

  Lemma len_app : forall (l1 l2 : list Frame),
    length (l1 ++ l2) = length l1 + length l2.
  Proof. induction l1 as [| x xs IH]; intro l2; simpl; [reflexivity | rewrite IH; reflexivity]. Qed.

  Lemma len_map_lower : forall l, length (map lower l) = length l.
  Proof. induction l as [| x xs IH]; simpl; [reflexivity | rewrite IH; reflexivity]. Qed.

  Lemma firstn_app_exact : forall (l1 l2 : list Frame),
    firstn (length l1) (l1 ++ l2) = l1.
  Proof. induction l1 as [| x xs IH]; intro l2; simpl; [reflexivity | rewrite IH; reflexivity]. Qed.

  Lemma len_firstn_le : forall n (l : list Frame),
    n <= length l -> length (firstn n l) = n.
  Proof.
    induction n as [| n IH]; intros l Hle; simpl.
    - reflexivity.
    - destruct l as [| x xs]; simpl in *.
      + lia.
      + rewrite IH; [reflexivity | lia].
  Qed.

  Lemma forall_app_intro : forall (P : Frame -> Prop) l1 l2,
    Forall P l1 -> Forall P l2 -> Forall P (l1 ++ l2).
  Proof.
    intros P l1 l2 H1 H2. induction l1 as [| x xs IH]; simpl.
    - exact H2.
    - inversion H1; subst. constructor; [assumption | apply IH; assumption].
  Qed.

  Lemma forall_firstn : forall (P : Frame -> Prop) n l,
    Forall P l -> Forall P (firstn n l).
  Proof.
    intros P. induction n as [| n IH]; intros l H; simpl.
    - constructor.
    - destruct l as [| x xs]; simpl.
      + constructor.
      + inversion H; subst. constructor; [assumption | apply IH; assumption].
  Qed.

  (* Every frame [lower] produces is lowered (idempotence). *)
  Lemma forall_lowered_map : LowerIdempotent -> forall l, Forall lowered (map lower l).
  Proof.
    intro Hlower_idem.
    unfold LowerIdempotent in Hlower_idem.
    induction l as [| x xs IH]; simpl.
    - constructor.
    - constructor; [unfold lowered; apply Hlower_idem | exact IH].
  Qed.

  (* On an already-lowered list, [map lower] is the identity. *)
  Lemma map_lower_id : forall l, Forall lowered l -> map lower l = l.
  Proof.
    induction l as [| x xs IH]; intro H; simpl.
    - reflexivity.
    - inversion H as [| ? ? Hx Hxs]; subst.
      unfold lowered in Hx. rewrite Hx, (IH Hxs). reflexivity.
  Qed.

  (* ===== The equivalence ===== *)

  (* If the persisted prefix is already lowered, the INCREMENTAL persist equals
     the WHOLE-STACK persist — the core observational-equivalence fact. *)
  Theorem incremental_eq_full : forall s from,
    Forall lowered (firstn from s) ->
    persist_from s from = persist_full s.
  Proof.
    intros s from Hpre. unfold persist_from, persist_full.
    transitivity (map lower (firstn from s) ++ map lower (skipn from s)).
    - rewrite (map_lower_id (firstn from s) Hpre). reflexivity.
    - rewrite <- map_app, firstn_skipn. reflexivity.
  Qed.

  (* persist_from at mark 0 is precisely the whole-stack persist. *)
  Lemma persist_from_0 : forall s, persist_from s 0 = persist_full s.
  Proof. intro s. unfold persist_from, persist_full. reflexivity. Qed.

  (* persist preserves the stack length (it rewrites frames in place). *)
  Lemma len_persist_from : forall s from, length (persist_from s from) = length s.
  Proof.
    intros s from. unfold persist_from.
    rewrite len_app, len_map_lower, <- len_app, firstn_skipn. reflexivity.
  Qed.

  (* After a persist whose prefix was lowered, the WHOLE stack is lowered. *)
  Theorem persist_from_all_lowered : forall s from,
    LowerIdempotent ->
    Forall lowered (firstn from s) ->
    Forall lowered (persist_from s from).
  Proof.
    intros s from Hlower_idem Hpre. rewrite (incremental_eq_full s from Hpre).
    apply forall_lowered_map. exact Hlower_idem.
  Qed.

  (* ===== The trampoline loop as a state machine over (stack, watermark) ===== *)

  (* Loop top (eval_loop.rs ~4200-4201): persist from the watermark, then set the
     watermark to the (length-preserving) new stack length. *)
  Definition loop_top (st : list Frame * nat) : list Frame * nat :=
    (persist_from (fst st) (snd st), length (persist_from (fst st) (snd st))).

  (* process_continuation push: append raw frames, watermark unchanged. *)
  Definition op_push (r : list Frame) (st : list Frame * nat) : list Frame * nat :=
    (fst st ++ r, snd st).

  (* Resume-arm pop + clamp (eval_loop.rs ~8731/8736): drop the top frame and
     clamp the watermark to the new length. *)
  Definition op_pop (st : list Frame * nat) : list Frame * nat :=
    (firstn (length (fst st) - 1) (fst st), Nat.min (snd st) (length (fst st) - 1)).

  Inductive Op : Type := Push (r : list Frame) | Pop.

  Definition step (st : list Frame * nat) (o : Op) : list Frame * nat :=
    match o with
    | Push r => op_push r (loop_top st)
    | Pop    => op_pop (loop_top st)
    end.

  (* Loop invariant: the watermark never exceeds the stack, and the persisted
     prefix is always lowered. This is the precondition [incremental_eq_full]
     needs at every loop top. *)
  Definition Inv (st : list Frame * nat) : Prop :=
    snd st <= length (fst st) /\ Forall lowered (firstn (snd st) (fst st)).

  (* After a loop-top persist the stack is FULLY lowered and the watermark sits
     at its end. *)
  Lemma loop_top_post : LowerIdempotent -> forall s wm, Inv (s, wm) ->
    loop_top (s, wm) = (persist_full s, length s) /\ Forall lowered (persist_full s).
  Proof.
    intros Hlower_idem s wm [Hle Hpre]. simpl in *.
    assert (Hpf : persist_from s wm = persist_full s)
      by (apply incremental_eq_full; exact Hpre).
    unfold loop_top; simpl. rewrite Hpf.
    split.
    - f_equal. unfold persist_full. apply len_map_lower.
    - apply forall_lowered_map. exact Hlower_idem.
  Qed.

  (* The invariant is preserved by a loop top followed by either a push or a
     clamped pop. *)
  Theorem inv_preserved : LowerIdempotent -> forall st o, Inv st -> Inv (step st o).
  Proof.
    intros Hlower_idem [s wm] o HInv.
    destruct (loop_top_post Hlower_idem s wm HInv) as [Hlt Hall].
    assert (Hlen : length (persist_full s) = length s)
      by (unfold persist_full; apply len_map_lower).
    destruct o as [r |].
    - (* Push r: state becomes (persist_full s ++ r, length s). *)
      unfold step, op_push. rewrite Hlt; simpl.
      unfold Inv; simpl. split.
      + rewrite len_app. lia.
      + replace (length s) with (length (persist_full s)) by exact Hlen.
        rewrite firstn_app_exact. exact Hall.
    - (* Pop: state becomes (firstn (len-1) (persist_full s), min (len) (len-1)). *)
      unfold step, op_pop. rewrite Hlt; simpl.
      unfold Inv; simpl. split.
      + rewrite len_firstn_le; rewrite Hlen; lia.
      + apply forall_firstn, forall_firstn. exact Hall.
  Qed.

  (* Hence the invariant holds along any sequence of loop iterations. *)
  Lemma inv_fold : LowerIdempotent -> forall ops st, Inv st -> Inv (fold_left step ops st).
  Proof.
    intros Hlower_idem.
    induction ops as [| o os IH]; intros st HInv; simpl.
    - exact HInv.
    - apply IH, inv_preserved; assumption.
  Qed.

  Definition init (s0 : list Frame) : list Frame * nat := (s0, 0).

  Lemma inv_init : forall s0, Inv (init s0).
  Proof. intro s0. unfold Inv, init; simpl. split; [lia | constructor]. Qed.

  Theorem reachable_inv : LowerIdempotent -> forall ops s0, Inv (fold_left step ops (init s0)).
  Proof. intros Hlower_idem ops s0. apply inv_fold; [exact Hlower_idem | apply inv_init]. Qed.

  (* MAIN: at EVERY loop top of EVERY reachable execution, the incremental persist
     equals the whole-stack persist it replaced — observational equivalence, so
     the fix changes only COST, never the lowered K-stack state. *)
  Theorem incremental_eq_full_at_every_loop_top : LowerIdempotent -> forall ops s0,
    persist_from (fst (fold_left step ops (init s0)))
                 (snd (fold_left step ops (init s0)))
    = persist_full (fst (fold_left step ops (init s0))).
  Proof.
    intros Hlower_idem ops s0.
    destruct (reachable_inv Hlower_idem ops s0) as [_ Hpre].
    apply incremental_eq_full. exact Hpre.
  Qed.

End IncrementalSpinePersistModel.

(* ===== Non-vacuity: the Resume-arm CLAMP is necessary for the equivalence =====

   Instantiate the model concretely with [lower := fun _ => true] over [bool]
   ([false] = a raw fan-out frame, [true] = its lowered form; [lower] is
   idempotent). Consider the stack reached by popping [true] (firstn 0 -> [])
   then pushing the raw frame [false]: the stack is [false] and the WHOLE-STACK
   persist would lower it to [true].

   With the CLAMP the watermark is min 1 (length []) = 0, so persist_from [false]
   0 lowers [false] -> [true] = whole-stack persist ([clamp_restores_equivalence]).
   WITHOUT the clamp the stale watermark 1 (the popped stack's old length) makes
   persist_from [false] 1 leave [false] un-lowered ([false] <> [true]), DIVERGING
   from the whole-stack persist ([clamp_is_necessary]). Hence the clamp authored
   at eval_loop.rs ~8736 is load-bearing. *)

Theorem clamp_is_necessary :
  firstn 1 ([] ++ [false]) ++ map (fun _ : bool => true) (skipn 1 ([] ++ [false]))
  <> map (fun _ : bool => true) ([] ++ [false]).
Proof. simpl. intro H. discriminate. Qed.

Theorem clamp_restores_equivalence :
  firstn 0 ([] ++ [false]) ++ map (fun _ : bool => true) (skipn 0 ([] ++ [false]))
  = map (fun _ : bool => true) ([] ++ [false]).
Proof. simpl. reflexivity. Qed.

End MeTTaTron_GC_IncrementalSpinePersistEquivalence.
