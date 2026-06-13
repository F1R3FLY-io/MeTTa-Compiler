(** Shared Inner-column read refinement.

    exp46 replaces the per-thread INNER_SHADOW materialization cache with one
    arena-coresident Inner column.  The live Rust reader has exactly two
    sources:

      * POD MettaValueInner variants read the shared column cell for their Addr.
      * Space/Memo are Arc-backed and read the append-only id store instead.

    The only stale-read hazard is a POD Addr reuse whose column cell was not
    rewritten before the new handle escaped.  The source code couples every mint
    path to populate_column post-alloc/pre-escape; this file records the abstract
    refinement obligation with no axioms or admits.
*)

Module MeTTaTron_GC_InnerColumnReadRefinement.

Inductive Variant : Type :=
| VAtom : Variant
| VLong : Variant
| VSExpr : Variant
| VSpanned : Variant
| VSpace : Variant
| VMemo : Variant.

Inductive ReadSource : Type :=
| SharedColumn : ReadSource
| IdStore : ReadSource.

Inductive CellVersion : Type :=
| OldCell : CellVersion
| NewCell : CellVersion.

Inductive DebugMark : Type :=
| Unwritten : DebugMark
| Written : DebugMark.

Definition is_space_memo (v : Variant) : bool :=
  match v with
  | VSpace | VMemo => true
  | _ => false
  end.

Definition read_source (v : Variant) : ReadSource :=
  if is_space_memo v then IdStore else SharedColumn.

Definition no_stale_read (v : Variant) (cell : CellVersion) : Prop :=
  match read_source v with
  | IdStore => True
  | SharedColumn => cell = NewCell
  end.

Theorem space_memo_reads_id_store :
  forall v,
    is_space_memo v = true ->
    read_source v = IdStore.
Proof.
  intros v Hspace.
  unfold read_source.
  rewrite Hspace.
  reflexivity.
Qed.

Theorem non_space_memo_reads_shared_column :
  forall v,
    is_space_memo v = false ->
    read_source v = SharedColumn.
Proof.
  intros v Hpod.
  unfold read_source.
  rewrite Hpod.
  reflexivity.
Qed.

Theorem pod_rewrite_before_escape_prevents_stale_read :
  forall v cell,
    is_space_memo v = false ->
    cell = NewCell ->
    no_stale_read v cell.
Proof.
  intros v cell Hpod Hnew.
  unfold no_stale_read.
  rewrite non_space_memo_reads_shared_column by exact Hpod.
  exact Hnew.
Qed.

Theorem space_memo_id_store_needs_no_column_rewrite :
  forall v cell,
    is_space_memo v = true ->
    no_stale_read v cell.
Proof.
  intros v cell Hspace.
  unfold no_stale_read.
  rewrite space_memo_reads_id_store by exact Hspace.
  exact I.
Qed.

Theorem missing_pod_rewrite_has_stale_counterexample :
  exists v cell,
    is_space_memo v = false /\
    cell = OldCell /\
    ~ no_stale_read v cell.
Proof.
  exists VAtom, OldCell.
  split; [reflexivity |].
  split; [reflexivity |].
  unfold no_stale_read, read_source, is_space_memo.
  discriminate.
Qed.

Definition debug_read_precondition (m : DebugMark) : Prop :=
  m = Written.

Theorem debug_mark_after_write_allows_read :
  debug_read_precondition Written.
Proof.
  unfold debug_read_precondition.
  reflexivity.
Qed.

Theorem debug_tripwire_rejects_unwritten_cell :
  ~ debug_read_precondition Unwritten.
Proof.
  unfold debug_read_precondition.
  discriminate.
Qed.

End MeTTaTron_GC_InnerColumnReadRefinement.
