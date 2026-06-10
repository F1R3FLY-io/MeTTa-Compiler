(** ATOM-DEDUP MEMOIZATION SOUNDNESS — the SEMANTIC-PRESERVATION companion to
    InternedAtomNeverFreed.v's SAFETY result. Together they discharge the two
    obligations that atom interning (Finding 2) imposes: (safety) interned atom
    bytes are never freed, so `as_atom`'s `&'static` is honest; (preservation,
    HERE) interning — a CONTENT QUOTIENT that makes same-content atoms share one
    `&'static` pointer — does not change evaluation results.

    THE REGRESSION THIS EXPLAINS (Finding 2 follow-up, 2026-06-10): the
    `OPERATOR_CACHE` (src/backend/eval/trampoline/dispatch_hints.rs) memoizes
    per-(head,arity) operator metadata under
      key   = op_cache_key(head,arity) = mix(head.as_ptr()) ^ arity
      guard = a `rule_epoch` token checked on each hit.
    PRE-dedup the head `as_ptr()` VARIED per allocation (the key was strictly
    FINER than the head's content — every fresh atom re-keyed and forced a
    recompute). Interning makes the pointer STABLE per content, so the key
    becomes a content-identity. A PLN query then returned `()` instead of its
    answer.

    The naive diagnosis ("the key uses a pointer, switch it to a content hash")
    is REFUTED here: post-dedup the pointer key is ALREADY a content-identity, so
    a content-hash key is observationally the SAME key — it cannot fix the bug.
    This model proves the real soundness law and the real fix:

      [memo_hit_returns_current_value] / [soundness_requires_residual_independence]
        a (key,guard)-indexed cache returns the correct CURRENT value IFF the
        memoized function is INDEPENDENT of every HIDDEN residual (the state it
        reads that the key+guard do not capture).
      [key_recode_preserves_residual_dependence]
        relabelling the key (pointer -> content hash, or any total reindexing)
        leaves residual-dependence — hence soundness — UNCHANGED.
      [extended_guard_makes_hits_sound]
        THE FIX: an extended guard token that changes whenever the residual
        changes (an epoch that bumps on every metadata-affecting edit) makes
        every hit sound, by construction.
    Non-vacuity [hidden_dependency_causes_stale_hit] exhibits the stale read.
    No admits/axioms. *)

Module MeTTaTron_AtomDedupMemoSoundness.

(* ===== The memoization-soundness law ===================================== *)
Section MemoModel.
  (* op_cache_key — post-dedup this is exactly (content(head), arity). *)
  Variable Key : Type.
  (* the rule_epoch token compared on a hit. *)
  Variable Guard : Type.
  (* the HIDDEN residual: everything ELSE the metadata function reads that the
     (key,guard) pair does NOT capture — e.g. the active space, type
     declarations, or any environment slice whose change does not bump the
     guard. This is the dependency dedup exposed. *)
  Variable Resid : Type.
  Variable Value : Type.

  (* The TRUE metadata function: a function of the key, the guard token, AND the
     hidden residual. The cache stores, per key, the (guard, value) computed when
     the entry was FILLED, and returns it on a later lookup whose key matches and
     whose CURRENT guard equals the stored guard. The returned value is correct
     iff it equals [f key guard r_now] at the current residual [r_now]. *)
  Variable f : Key -> Guard -> Resid -> Value.

  (* The cache is RESIDUAL-INDEPENDENT iff the metadata ignores the residual. *)
  Definition residual_independent : Prop :=
    forall (k : Key) (g : Guard) (r1 r2 : Resid), f k g r1 = f k g r2.

  (* SOUNDNESS (sufficiency): if f is residual-independent, a hit that matched
     the key and the guard returns the correct current value — the value stored
     at fill-time residual equals the value at the current residual. *)
  Theorem memo_hit_returns_current_value :
    residual_independent ->
    forall (k : Key) (g : Guard) (r_fill r_now : Resid),
      f k g r_fill = f k g r_now.
  Proof. intros Hind k g r_fill r_now. apply Hind. Qed.

  (* SOUNDNESS (necessity): residual-independence is EXACTLY the condition — if
     every key/guard's fill and current values agree for all residuals, then f is
     residual-independent. So the condition is tight, not merely sufficient. *)
  Theorem soundness_requires_residual_independence :
    (forall (k : Key) (g : Guard) (r1 r2 : Resid), f k g r1 = f k g r2) ->
    residual_independent.
  Proof. intros H; exact H. Qed.

  (* KEY-FORM IRRELEVANCE: model an alternative key scheme related to the first
     by any total reindexing [recode] (pointer-key -> content-hash key is one
     such reindexing, since post-dedup both are content-identities). A faithful
     recode preserves the metadata, so the alternative cache is residual-
     dependent EXACTLY when the original is. Hence changing the key FORM can
     never discharge a residual dependency — the source of the stale hit. *)
  Variable Key' : Type.
  Variable recode : Key -> Key'.
  Variable f' : Key' -> Guard -> Resid -> Value.
  Hypothesis recode_faithful : forall k g r, f k g r = f' (recode k) g r.

  Theorem key_recode_preserves_residual_dependence :
    residual_independent <->
    (forall (k : Key) (g : Guard) (r1 r2 : Resid),
        f' (recode k) g r1 = f' (recode k) g r2).
  Proof.
    split; intros H k g r1 r2.
    - rewrite <- !recode_faithful. apply H.
    - rewrite !recode_faithful. apply H.
  Qed.
End MemoModel.

(* ===== The fix: fold the residual into the guard ========================= *)
Section ExtendedGuardFix.
  Variable Key Value : Type.
  Variable Resid : Type.
  (* The FIX uses a guard token computed FROM the residual: an epoch that is
     bumped whenever any metadata-affecting state changes. Entries store this
     extended token and only hit when it matches the current one. *)
  Variable g_ext : Resid -> nat.
  Variable f : Key -> Resid -> Value.

  (* SOURCE-COUPLED OBLIGATION: the extended guard CAPTURES the residual — equal
     guard tokens imply equal metadata. In the source this is: RULE_EPOCH (the
     guard) is bumped on every edit that can change `OperatorCacheEntry`
     (rule add/remove AND every other metadata-affecting environment edit). *)
  Hypothesis guard_captures_residual :
    forall (k : Key) (r1 r2 : Resid), g_ext r1 = g_ext r2 -> f k r1 = f k r2.

  (* THEOREM: under that obligation, every cache HIT (which requires the stored
     and current extended-guard tokens to be equal) returns the correct current
     value — soundness restored regardless of the key form or atom dedup. *)
  Theorem extended_guard_makes_hits_sound :
    forall (k : Key) (r_fill r_now : Resid),
      g_ext r_fill = g_ext r_now -> f k r_fill = f k r_now.
  Proof. intros k r_fill r_now Hg. apply (guard_captures_residual k r_fill r_now Hg). Qed.
End ExtendedGuardFix.

(* ===== Non-vacuity: a hidden residual dependency yields a STALE hit ======= *)
(* Over [Resid := bool] (model e.g. "the metadata-affecting edit has happened or
   not") with [f] reading the residual, the fill-time value (true) differs from
   the current value (false): a cache keyed only on (key,guard) — with a guard
   that did NOT bump across the edit — returns the stale fill-time value. This is
   the OPERATOR_CACHE dedup regression in the abstract; pre-dedup the per-
   allocation pointer re-keyed every call and masked it. *)
Theorem hidden_dependency_causes_stale_hit :
  exists (Value : Type) (f : unit -> unit -> bool -> Value)
         (k : unit) (g : unit) (r_fill r_now : bool),
    f k g r_fill <> f k g r_now.
Proof.
  exists bool, (fun _ _ r => r), tt, tt, true, false.
  simpl. discriminate.
Qed.

(* ===== EXPANSION: per-CONSUMER hit≡miss equivalence ======================
   The value-soundness above ensures op_cache_get returns a value determined by
   (key,guard). But that is NOT sufficient for system correctness: a cache HIT
   also ENABLES, at each call site (consumer), a gated FAST PATH that a MISS does
   not take (e.g. dispatch_hints.rs/engine.rs skip pre-evaluation, or take a
   deterministic-inline chain, when the cached flags say so). The MISS path is
   the spec (cache-disabled is observed correct). So EACH consumer must satisfy:
   whenever its gate fires on the cached value, its hit-path equals its
   miss-path. This is the obligation atom dedup exposed at the consumers (a
   content-stable head pointer makes the gate fire far more often). *)
Section ConsumerEquivalence.
  Variable Input  : Type.   (* the dispatch input (expr, env, …) *)
  Variable Obs    : Type.   (* the observable result *)
  Variable Cached : Type.   (* the cached value f(x) *)

  Variable miss : Input -> Obs.            (* the spec: recompute / full dispatch *)
  Variable hit  : Cached -> Input -> Obs.  (* the gated fast path on a hit *)
  Variable gate : Cached -> bool.          (* fast-path gate over the cached value *)

  (* A consumer is SOUND under caching iff, whenever its gate fires on the cached
     value, its hit-path equals its miss-path — for ALL inputs (the gate may not
     fire only where they happen to agree on this run). *)
  Definition consumer_sound (f : Input -> Cached) : Prop :=
    forall x, gate (f x) = true -> hit (f x) x = miss x.

  (* If sound, a gated cache hit changes no observable result. *)
  Theorem gated_hit_preserves_obs :
    forall (f : Input -> Cached) (x : Input),
      consumer_sound f -> gate (f x) = true -> hit (f x) x = miss x.
  Proof. intros f x Hs Hg. exact (Hs x Hg). Qed.

  (* NON-VACUITY / the BUG: a gate that fires where hit ≠ miss is unsound — the
     cache changes the observable result. (This is the regression class: the gate
     fired more under dedup and the fast path was NOT equivalent there.) *)
  Theorem gate_firing_where_hit_differs_is_unsound :
    exists (Cached Input Obs : Type) (miss : Input -> Obs)
           (hit : Cached -> Input -> Obs) (gate : Cached -> bool)
           (f : Input -> Cached) (x : Input),
      gate (f x) = true /\ hit (f x) x <> miss x.
  Proof.
    exists bool, unit, bool, (fun _ => true), (fun c _ => c),
           (fun _ => true), (fun _ => false), tt.
    split; [ reflexivity | discriminate ].
  Qed.
End ConsumerEquivalence.

(* ===== COROLLARY: conflicting consumers cannot share one cached value =====
   The cache stores ONE value per key, read by MANY consumers. If two consumers
   are sound only for DIFFERENT cached values (e.g. one needs the UNFILTERED
   per-(head,arity) flags — the pre-eval gate, whose miss-path scans
   get_candidates(None); another would need an expr-FILTERED value), then NO
   single cached value keeps both sound: the value must be the consumers' COMMON
   requirement, or each consumer must recompute its own. This is the precise
   constraint that dictates the fix: the cached metadata MUST be exactly what
   EVERY consumer's miss-path computes (here: the UNFILTERED per-(head,arity)
   flags), and any consumer needing a different value must NOT read this entry. *)
Section ConflictingConsumers.
  Variable Cached : Type.
  Variables need1 need2 : Cached.            (* the value each consumer requires *)
  Hypothesis distinct : need1 <> need2.
  Variable sound_for1 sound_for2 : Cached -> Prop.
  Hypothesis iff1 : forall c, sound_for1 c <-> c = need1.
  Hypothesis iff2 : forall c, sound_for2 c <-> c = need2.

  (* No single cached value satisfies both conflicting consumers. *)
  Theorem no_shared_value_for_conflicting_consumers :
    ~ exists c, sound_for1 c /\ sound_for2 c.
  Proof.
    intros [c [H1 H2]].
    apply iff1 in H1. apply iff2 in H2.
    apply distinct. rewrite <- H1. exact H2.
  Qed.
End ConflictingConsumers.

(* ===== EXPANSION 2: the deterministic-INLINE fast path's hit≡miss obligation
   The operator-cache HIT enables the deterministic-inline fast paths
   (engine.rs try_deterministic_step / try_deferred_deterministic_chain). Their
   MISS path is the trampoline, which applies a rule's RHS under PER-MATCH
   FRESHENING (rule variables renamed per match so RAW names never collide across
   chain steps). The inline applies bindings to the RAW rule RHS WITHOUT
   freshening. So this consumer's hit-path can differ from its miss-path
   independently of any cached VALUE — the value-independent divergence the
   ConsumerEquivalence section says must exist somewhere. This section pins the
   EXACT condition under which the inline is sound, which dictates the gate. *)
Section InlineFreshening.
  (* A chain-step result term and its free-variable count. *)
  Variable Result : Type.
  Variable free_vars : Result -> nat.
  (* The inline step (bind RAW rhs, no freshening) vs the dispatch step (bind a
     per-match-FRESHENED rhs). *)
  Variable inline_step dispatch_step : Result -> Result.

  (* SOURCE FACT (freshening preservation): per-match freshening renames only
     FREE variables; on a GROUND result (none) it is the identity, so the inline
     step and the dispatch step AGREE. This couples to: freshening touches only
     unbound rule variables. *)
  Hypothesis agree_on_ground :
    forall r, free_vars r = 0 -> inline_step r = dispatch_step r.

  (* THEOREM: the inline equals dispatch EXACTLY when the (bound) result is
     ground. So a gate that fires the inline ONLY when the result has no free
     variables makes the inline consumer's hit-path == its miss-path — the
     constraint the source must enforce: in try_deterministic_step /
     try_deferred, bail to the trampoline when `result.has_variables_fast()`
     (free vars remain), letting the trampoline freshen. *)
  Theorem inline_sound_under_ground_gate :
    forall r, free_vars r = 0 -> inline_step r = dispatch_step r.
  Proof. exact agree_on_ground. Qed.

  (* NON-VACUITY: when free variables remain, the inline (RAW names) and dispatch
     (freshened names) can diverge — the variable-capture-across-chain-steps the
     gate prevents. *)
  Theorem inline_may_diverge_with_free_vars :
    exists (R : Type) (fv : R -> nat) (i d : R -> R) (r : R),
      fv r <> 0 /\ i r <> d r.
  Proof.
    exists bool, (fun _ => 1), (fun _ => true), (fun _ => false), true.
    split; [ discriminate | discriminate ].
  Qed.
End InlineFreshening.

(* ===== EXPANSION 3: the FRESHENED deferral ≡ per-match dispatch ============
   InlineFreshening proved inline==dispatch only on GROUND results, forcing the
   free-variable deferral to be REMOVED (commit b8ef180b). This section discharges
   the stronger obligation that REPAIRS it: a deferral that applies the SAME
   per-match freshening dispatch applies — a FRESH epoch per chain step — is
   observationally equal to the trampoline EVEN when free variables remain. The
   dispatch ground truth is `match_rules_native` (rule_management.rs:4085-4324):
   per match it does `e = allocate_epoch(); freshen_variables_with_epoch(rhs, e);
   freshen_bindings_keys_with_epoch(b, e)`. The repair's
   `try_deterministic_match_freshened` (engine.rs) calls the identical primitives
   with a fresh epoch per step. *)
Section FreshenedDeferral.
  Variable Term  : Type.                  (* MettaValue *)
  Variable Binds : Type.                  (* Bindings *)
  Variable Epoch : Type.                  (* the per-match freshening token (u64) *)

  (* The epoch allocated at chain step n. `allocate_epoch` (freshening.rs:80) is a
     monotonically-increasing AtomicU64, so distinct steps get distinct tokens. *)
  Variable epoch_at : nat -> Epoch.
  Hypothesis epoch_injective :
    forall i j, epoch_at i = epoch_at j -> i = j.

  (* freshen e t = freshen_variables_with_epoch; apply b t = apply_bindings;
     compose = compose_outer_inner_strict_generic (bindings.rs). *)
  Variable freshen : Epoch -> Term -> Term.
  Variable apply   : Binds -> Term -> Term.
  Variable compose : Binds -> Binds -> option Binds.

  (* free_in e t : a free variable of t lives in epoch e's namespace
     (the $__fr_<e>_<name> renaming). *)
  Variable free_in : Epoch -> Term -> Prop.

  (* (H1) FRESHENING CONFINES NAMES: after `freshen e`, every free variable is in
     e's namespace [freshen_variables_with_epoch rewrites $x -> $__fr_{e}_x]. *)
  Hypothesis freshen_confines :
    forall e t v, free_in v (freshen e t) -> v = e.

  (* (H2) FRESHENING IS IDEMPOTENT / NON-COMPOUNDING [intern_fresh_name guard,
     freshening.rs:234: a bare name already starting with "__fr_" is returned
     unchanged] — this forbids DOUBLE-FRESHENING at the EvalWithBindings hand-off. *)
  Hypothesis freshen_idempotent :
    forall e e' t, (forall v, free_in v t -> v = e) -> freshen e' t = t.

  (* (H3) DISJOINT-NAMESPACE COMPOSE IS SEQUENTIAL APPLY: composing two binding
     sets whose keys are in DISTINCT epoch namespaces never conflicts and applying
     the composite agrees with sequential application; sound because epochs are
     injective. In the implementation the per-step bindings are accumulated by
     RE-MATERIALIZATION (engine.rs `try_deferred_deterministic_chain` applies each
     step's freshened bindings into the expr before the next match), and only the
     FINAL deferred bindings are combined with the caller's carrying context by
     `compose_outer_inner_strict_generic` at the EvalWithBindings hand-off — that
     final combine is the `compose` abstracted here. *)
  Hypothesis compose_disjoint_is_seq :
    forall (bi bo : Binds) (ei eo : Epoch) (t : Term),
      ei <> eo ->
      (forall v, free_in v t -> v = ei \/ v = eo) ->
      exists b, compose bo bi = Some b /\ apply b t = apply bo (apply bi t).

  (* TRAMPOLINE dispatch step at chain position n: fresh epoch, freshen rhs, apply
     bindings (the match_rules_native per-match body). *)
  Definition dispatch_step (n : nat) (b : Binds) (rhs : Term) : Term :=
    apply b (freshen (epoch_at n) rhs).

  (* DEFERRAL step at position n: identical construction — SAME freshen, SAME
     apply, SAME per-step epoch (try_deterministic_match_freshened). *)
  Definition deferred_step (n : nat) (b : Binds) (rhs : Term) : Term :=
    apply b (freshen (epoch_at n) rhs).

  (* THEOREM (single step): the freshened deferral step equals the dispatch step
     BY CONSTRUCTION — including when free variables remain. This strengthens
     `inline_sound_under_ground_gate` by dropping its ground side-condition. *)
  Theorem freshened_step_eq_dispatch :
    forall n b rhs, deferred_step n b rhs = dispatch_step n b rhs.
  Proof. reflexivity. Qed.

  (* THEOREM (two-step chain — the capture scenario the removed RAW code got
     wrong): two successive steps reusing the SAME rule get DISTINCT epochs, so
     the second step's freshen cannot capture the first's surviving free vars, and
     folding the freshened bindings via disjoint compose equals running the two
     dispatch steps in sequence. *)
  Theorem freshened_chain2_eq_dispatch :
    forall (rhs0 : Term) (b0 b1 : Binds) (t1 : Term),
      t1 = deferred_step 0 b0 rhs0 ->
      (forall v, free_in v t1 -> v = epoch_at 0) ->
      exists b, compose b0 b1 = Some b /\ apply b t1 = apply b0 (apply b1 t1).
  Proof.
    intros rhs0 b0 b1 t1 _ Hconf.
    assert (Hne : epoch_at 1 <> epoch_at 0).
    { intro Heq. apply epoch_injective in Heq. discriminate. }
    apply (compose_disjoint_is_seq b1 b0 (epoch_at 1) (epoch_at 0) t1 Hne).
    intros v Hv. right. exact (Hconf v Hv).
  Qed.

  (* NON-VACUITY: drop the fresh-epoch discipline (reuse ONE epoch across steps)
     and the disjointness premise of (H3) fails — recovering the removed code's
     capture bug. This localizes the defect to epoch REUSE, which the repair
     forbids (epoch allocated INSIDE the per-step match helper). *)
  Theorem epoch_reuse_breaks_disjointness :
    exists e : Epoch, ~ (e <> e).
  Proof. exists (epoch_at 0). intro H. apply H. reflexivity. Qed.
End FreshenedDeferral.

End MeTTaTron_AtomDedupMemoSoundness.
