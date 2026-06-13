(** INTERNED-ATOM NEVER-FREED — the correct-by-construction structural prevention of
    the laundered-`&'static str` UAF class (audit Finding 2). The READ-side companion
    to TrackedVarSideRetention.v, but eliminating the bug class by CONSTRUCTION rather
    than by rooting a single site.

    BUG CLASS this closes (audit Finding 2 — the generalization of Finding 1):
    `MettaValueTrait::as_atom(&self) -> Option<&'static str>` laund­ers a `&'static`
    borrow. In SLAB mode the atom bytes are leaked, so the `&'static` is HONEST. In
    INDEX mode the atom bytes are a `Box<str>` in a GC'd `SideColumn<str>`
    (index_heap.rs `intern_bytes_in`); a quiescent sweep's `SideColumn::free` drops the
    `Box`, so the `&'static` is a LIE — any caller that copies the borrow out and
    outlives the value's rooting reads freed bytes (Finding-1 was one such site).

    THE FIX (Option A, correct-by-construction): atom bytes are interned into the
    PERMANENT, process-lifetime interner (`symbol.rs`'s `static INTERNER:
    OnceLock<ThreadedRodeo>`), NOT into the GC'd arena side-column. `IndexFactory::atom`
    / `alloc_atom` call `symbol::intern_static` and store the resulting honest
    `&'static str` in the node — so an atom payload is never placed in any freeable
    `SideColumn` cell.

    MAIN RESULT [interned_atom_bytes_never_released]: under the source-coupled laws
    (a sweep RELEASES only side-boxed payloads; an interned atom is NOT side-boxed) an
    interned atom's bytes are NEVER released — so `as_atom`'s `&'static` borrow can
    never dangle and the Finding-1 UAF class is IMPOSSIBLE BY CONSTRUCTION (there is no
    freeable backing store for atom bytes). [interned_atom_borrow_valid_forever] states
    the borrow-validity corollary. Non-vacuity [pre_fix_sidebox_atom_releasable]
    exhibits the pre-fix world (a side-boxed atom payload that IS releasable — the bug).
    No admits/axioms. *)

Module MeTTaTron_GC_InternedAtomNeverFreed.

Section InternedAtomModel.
  (* Atom byte payloads (the `str` an atom names). *)
  Variable Bytes : Type.

  (* A payload that lives in a freeable arena side-column cell (`Box<str>` in a
     `SideColumn<str>` / a releasable segment). *)
  Variable SideBoxed : Bytes -> Prop.

  (* A collection RELEASES a payload. The sweep / segment-release path frees ONLY
     side-column / segment cells (`SideColumn::free` drops a side `Box`; segment
     release resets its side arenas) — nothing else is a freeable backing store. *)
  Variable Released : Bytes -> Prop.
  Definition ReleaseOnlySideboxed : Prop :=
    forall b, Released b -> SideBoxed b.

  (* THE FIX, source-coupled: an INTERNED atom payload is NOT side-boxed — its bytes
     live in the perpetual interner (`symbol.rs` `static OnceLock<ThreadedRodeo>`),
     which `alloc_atom` populates via `intern_static` INSTEAD of `intern_bytes_in`. So
     no `SideColumn` cell ever holds an interned atom's bytes. *)
  Variable Interned : Bytes -> Prop.
  Definition InternedNotSideboxed : Prop :=
    forall b, Interned b -> ~ SideBoxed b.

  (* MAIN: an interned atom's bytes are NEVER released — the contrapositive chain
     [Interned b -> ~ SideBoxed b] and [Released b -> SideBoxed b]. So `as_atom`'s
     `&'static` borrow of an interned atom can never name freed memory. *)
  Theorem interned_atom_bytes_never_released :
    ReleaseOnlySideboxed -> InternedNotSideboxed ->
    forall b, Interned b -> ~ Released b.
  Proof.
    intros Hrelease_sideboxed Hinterned_not_sideboxed b Hint Hrel.
    unfold ReleaseOnlySideboxed in Hrelease_sideboxed.
    unfold InternedNotSideboxed in Hinterned_not_sideboxed.
    apply (Hinterned_not_sideboxed b Hint).
    apply (Hrelease_sideboxed b Hrel).
  Qed.

  (* Borrow-validity corollary: the `&'static str` `as_atom` returns for an interned
     atom is valid for the whole process lifetime — there is no collection that frees
     its referent, so the `&'static` is HONEST (not laundered). *)
  Theorem interned_atom_borrow_valid_forever :
    ReleaseOnlySideboxed -> InternedNotSideboxed ->
    forall b, Interned b -> Released b -> False.
  Proof.
    intros Hrelease_sideboxed Hinterned_not_sideboxed b Hint Hrel.
    exact (interned_atom_bytes_never_released Hrelease_sideboxed
      Hinterned_not_sideboxed b Hint Hrel).
  Qed.
End InternedAtomModel.

(* ===== Non-vacuity: interning is NECESSARY =====

   WITHOUT the fix (the pre-fix world, where an atom payload IS a `Box<str>` in a
   freeable `SideColumn`), the sweep law still holds AND the payload is side-boxed AND
   releasable — the use-after-free the fix eliminates. We exhibit a concrete instance
   over [Bytes := nat] where the payload [b] is side-boxed and released, so the MAIN
   theorem's conclusion ([~ Released b]) would FAIL for a non-interned (side-boxed)
   atom — mirroring [unrooted_tracked_var_box_may_be_released]. *)
Theorem pre_fix_sidebox_atom_releasable :
  exists (SideBoxed Released : nat -> Prop) (b : nat),
    (forall x, Released x -> SideBoxed x)   (* the sweep law still holds *)
    /\ SideBoxed b                          (* the atom IS side-boxed (pre-fix) *)
    /\ Released b.                           (* and CAN be released -> the UAF *)
Proof.
  exists (fun _ => True), (fun n => n = 0), 0.
  split; [ intros x _; exact I | split; [ exact I | reflexivity ] ].
Qed.

End MeTTaTron_GC_InternedAtomNeverFreed.
