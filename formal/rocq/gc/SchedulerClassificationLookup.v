(** Rocq model of the scheduler classifier L2 range invariant.

    SchedulerAutomaton stores each head hash as a contiguous range in the flat
    L2 table. Inserting an additional entry for a head must extend that head's
    range and shift every later range. Otherwise lookup may scan another head's
    entry or miss the new entry entirely.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_SchedulerClassificationLookup.

Section L2RangeModel.
  Definition in_range (start count idx : nat) : Prop :=
    start <= idx < start + count.

  Definition disjoint (start_a count_a start_b count_b : nat) : Prop :=
    forall idx, in_range start_a count_a idx -> in_range start_b count_b idx -> False.

  Theorem insert_at_range_end_contains_new :
    forall start count,
      in_range start (S count) (start + count).
  Proof.
    intros start count.
    unfold in_range.
    lia.
  Qed.

  Theorem insert_preserves_existing_target_entries :
    forall start count idx,
      in_range start count idx ->
      in_range start (S count) idx.
  Proof.
    intros start count idx Hin.
    unfold in_range in *.
    lia.
  Qed.

  Theorem shift_later_start_preserves_later_membership :
    forall insert_at start count idx,
      insert_at <= start ->
      in_range start count idx ->
      in_range (S start) count (S idx).
  Proof.
    intros insert_at start count idx _ Hin.
    unfold in_range in *.
    lia.
  Qed.

  Theorem shifted_adjacent_later_range_is_disjoint :
    forall start count later_count,
      disjoint start (S count) (S (start + count)) later_count.
  Proof.
    intros start count later_count idx Hin_target Hin_later.
    unfold in_range in *.
    lia.
  Qed.
End L2RangeModel.

End MeTTaTron_GC_SchedulerClassificationLookup.
