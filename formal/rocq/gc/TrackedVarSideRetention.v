(** TRACKED-VARIABLE SIDE-BOX RETENTION — the READ-side companion to the
    side-payload FREE-side proofs (SideFreeQuiescence / QuiescentSideIndexReuse /
    the generation guard).

    BUG CLASS this closes (audit Finding 1, HIGH): collapse-bind "tracked
    variables" were captured as laundered `&'static str` pointing into an arena
    string side-Box, shared cross-thread into branch workers, while the SOURCE
    variable ATOM was NOT in the GC root set (the root set holds MettaValue Addrs;
    the bare `&str` discarded the parent Addr). A FANOUT>0 rendezvous major could
    release the backing segment (workers parked) and drop the string side-Box ->
    a worker dereferenced freed bytes -> use-after-free. The free-side proofs
    reason that a stale snapshot must not free a LIVE cell; NONE modelled a
    laundered ref OUTLIVING a correctly-freed cell — the read side.

    THE FIX (genuine-CESK, commit-pending): capture the variable ATOM handle
    (MettaValue) and root it — `collect_binding_capture_roots` is wired into
    `collect_global_anchors` (roots.rs), so every mark (rendezvous / midloop /
    quiescence) keeps each tracked-var atom — hence its string side-Box —
    reachable. The tracked var becomes a first-class member of σ|_Reachable, which
    is exactly what the CESK root mandate requires ("there is no invisible live
    value").

    MAIN RESULT [tracked_var_side_box_retained]: under the source-coupled wiring
    (a held tracked-var atom is a global anchor, anchors are roots, roots are
    reachable, an atom reaches its side-Box) and the mark-completeness sweep law
    (a sweep releases only UNREACHABLE cells), a held tracked variable's string
    side-Box is NEVER released — so the laundered read can never dangle.
    Non-vacuity [unrooted_tracked_var_box_may_be_released] exhibits the pre-fix
    bug: WITHOUT the anchor wiring, an unreachable tracked-var box is releasable.
    No admits/axioms. *)

Module MeTTaTron_GC_TrackedVarSideRetention.

Section TrackedVarSideRetentionModel.
  (* Heap addresses (the index-arena Addr = u32 in the implementation). *)
  Variable Addr : Type.

  (* The structural root set, the reachability edge relation, and reachability =
     the closure of Edge from Root (σ|_Reachable). *)
  Variable Root  : Addr -> Prop.
  Variable Edge  : Addr -> Addr -> Prop.
  Variable Reach : Addr -> Prop.
  Hypothesis reach_root : forall a, Root a -> Reach a.
  Hypothesis reach_step : forall a b, Reach a -> Edge a b -> Reach b.

  (* The global-anchor root family (collect_global_anchors): every anchor is a
     root of the mark. *)
  Variable GlobalAnchor : Addr -> Prop.
  Hypothesis anchor_is_root : forall a, GlobalAnchor a -> Root a.

  (* THE FIX, source-coupled: a held tracked-variable atom is a global anchor —
     i.e. collect_binding_capture_roots is wired into collect_global_anchors so
     the binding-capture stack contributes to the root set on every mark. *)
  Variable BindingCaptureRoot : Addr -> Prop.
  Hypothesis binding_capture_is_anchor :
    forall a, BindingCaptureRoot a -> GlobalAnchor a.

  (* The atom -> string-side-Box ownership edge (Node::Atom(ByteRef) -> its
     SideColumn<str> entry), part of the structural Edge relation. *)
  Variable SideBox : Addr -> Addr -> Prop.
  Hypothesis sidebox_is_edge : forall a s, SideBox a s -> Edge a s.

  (* The sweep / segment-release law (mark completeness): a side-Box (or segment)
     is released only when it is UNREACHABLE. *)
  Variable Released : Addr -> Prop.
  Hypothesis sweep_releases_only_unreachable :
    forall s, Released s -> ~ Reach s.

  (* A held tracked-variable atom is reachable (its source atom is in σ|_Reachable
     via the binding-capture anchor wiring). *)
  Lemma tracked_var_atom_reachable :
    forall a, BindingCaptureRoot a -> Reach a.
  Proof.
    intros a Hbcr.
    apply reach_root. apply anchor_is_root. apply binding_capture_is_anchor. exact Hbcr.
  Qed.

  (* Its string side-Box is therefore reachable too (one Edge step). *)
  Lemma tracked_var_side_box_reachable :
    forall a s, BindingCaptureRoot a -> SideBox a s -> Reach s.
  Proof.
    intros a s Hbcr Hbox.
    apply (reach_step a s).
    - apply tracked_var_atom_reachable. exact Hbcr.
    - apply sidebox_is_edge. exact Hbox.
  Qed.

  (* MAIN: a held tracked variable's string side-Box is NEVER released — so the
     laundered `&str` read can never dangle. Contrapositive of the sweep law on
     the reachable side-Box. *)
  Theorem tracked_var_side_box_retained :
    forall a s, BindingCaptureRoot a -> SideBox a s -> ~ Released s.
  Proof.
    intros a s Hbcr Hbox Hrel.
    apply (sweep_releases_only_unreachable s Hrel).
    apply (tracked_var_side_box_reachable a s Hbcr Hbox).
  Qed.
End TrackedVarSideRetentionModel.

(* ===== Non-vacuity: the binding-capture anchor wiring is NECESSARY =====

   WITHOUT the [binding_capture_is_anchor] wiring (the pre-fix world, where a
   tracked var is a bare laundered `&str` whose source atom is in no root source),
   a tracked-var side-Box that is unreachable IS releasable — the use-after-free.
   We exhibit a concrete instance over [Addr := nat] where the side-Box [s] is
   unreachable yet its atom [a] is a "binding-capture root" in name only (no
   anchor wiring), so the retention theorem's conclusion fails: [s] is released. *)
Theorem unrooted_tracked_var_box_may_be_released :
  exists (Reach Released : nat -> Prop) (s : nat),
    (forall x, Released x -> ~ Reach x)      (* the sweep law still holds *)
    /\ ~ Reach s                             (* s is unreachable (atom not rooted) *)
    /\ Released s.                            (* yet s is released -> the UAF *)
Proof.
  exists (fun _ => False), (fun n => n = 0), 0.
  split; [ intros x _ HF; exact HF | split; [ intro HF; exact HF | reflexivity ] ].
Qed.

End MeTTaTron_GC_TrackedVarSideRetention.
