/-!
Lean model of the R-FL free-list lifecycle in the CESK index arena.

This file intentionally models only the load-bearing free-list fact proved by
`tla/StoreCentricGC_RFL.tla` and implemented in
`src/backend/eval/cesk/index_arena.rs`:

  * `push` appends an address only when its persistent free bit was clear.
  * `pop` clears the bit for the popped address before deciding whether to reuse
    or discard it.
  * `majorDrain` clears the whole list and all membership bits.
  * `drainReleasedSegment` removes all listed addresses in a released segment
    before that segment's bitmap is dropped.

The theorem shape is unbounded over address types with decidable equality and
uses only Lean's standard trusted base.
-/

namespace MeTTaTron.GC.FreeList

structure State (Addr : Type u) where
  freeList : List Addr
  freeBit : Addr -> Bool

def Valid [DecidableEq Addr] (st : State Addr) : Prop :=
  (forall a, st.freeBit a = true <-> a ∈ st.freeList) ∧ st.freeList.Nodup

def push [DecidableEq Addr] (a : Addr) (st : State Addr) : State Addr :=
  if st.freeBit a = true then
    st
  else
    { freeList := a :: st.freeList
      freeBit := fun x => if x = a then true else st.freeBit x }

def pop [DecidableEq Addr] (st : State Addr) : State Addr :=
  match st.freeList with
  | [] => st
  | a :: rest =>
      { freeList := rest
        freeBit := fun x => if x = a then false else st.freeBit x }

def majorDrain (_st : State Addr) : State Addr :=
  { freeList := []
    freeBit := fun _ => false }

def drainReleasedSegment [DecidableEq Addr] (released : Addr -> Bool) (st : State Addr) :
    State Addr :=
  { freeList := st.freeList.filter (fun a => !released a)
    freeBit := fun a => if released a = true then false else st.freeBit a }

theorem push_preserves_valid [DecidableEq Addr] {a : Addr} {st : State Addr} :
    Valid st -> Valid (push a st) := by
  intro h
  unfold Valid at h ⊢
  rcases h with ⟨hbit, hnodup⟩
  unfold push
  by_cases hfree : st.freeBit a = true
  · simp [hfree, hbit, hnodup]
  · constructor
    · intro x
      by_cases hx : x = a
      · subst hx
        simp [hfree]
      · simp [hfree, hx, hbit]
    · simpa [hfree] using
        (List.nodup_cons.mpr ⟨fun hin => hfree ((hbit a).2 hin), hnodup⟩)

theorem pop_preserves_valid [DecidableEq Addr] {st : State Addr} :
    Valid st -> Valid (pop st) := by
  intro h
  unfold Valid at h ⊢
  rcases h with ⟨hbit, hnodup⟩
  unfold pop
  cases hlist : st.freeList with
  | nil =>
      simpa [hlist] using And.intro hbit hnodup
  | cons head rest =>
      have hnodupCons : (head :: rest).Nodup := by
        simpa [hlist] using hnodup
      have hheadNotRest : head ∉ rest := by
        exact List.nodup_cons.mp hnodupCons |>.1
      have hrestNodup : rest.Nodup := by
        exact List.nodup_cons.mp hnodupCons |>.2
      constructor
      · intro x
        by_cases hx : x = head
        · subst hx
          constructor
          · intro hfalse
            simp at hfalse
          · intro hin
            exact False.elim (hheadNotRest hin)
        · constructor
          · intro hxb
            have hxb' : st.freeBit x = true := by
              simpa [hx] using hxb
            have hxmem : x ∈ st.freeList := (hbit x).1 hxb'
            simp [hlist, hx] at hxmem
            exact hxmem
          · intro hxmem
            have hstmem : x ∈ st.freeList := by
              simp [hlist, hx, hxmem]
            have hxb : st.freeBit x = true := (hbit x).2 hstmem
            simp [hx, hxb]
      · exact hrestNodup

theorem majorDrain_valid [DecidableEq Addr] (st : State Addr) :
    Valid (majorDrain st) := by
  unfold Valid majorDrain
  constructor
  · intro a
    simp
  · simp

theorem filter_not_mem_released [DecidableEq Addr] {released : Addr -> Bool}
    {a : Addr} {xs : List Addr} :
    a ∈ xs.filter (fun x => !released x) -> released a = false := by
  intro h
  have hkeep : (!released a) = true := (List.mem_filter.mp h).2
  cases hrel : released a <;> simp [hrel] at hkeep ⊢

theorem drainReleasedSegment_preserves_valid [DecidableEq Addr]
    {released : Addr -> Bool} {st : State Addr} :
    Valid st -> Valid (drainReleasedSegment released st) := by
  intro h
  unfold Valid at h ⊢
  rcases h with ⟨hbit, hnodup⟩
  unfold drainReleasedSegment
  constructor
  · intro a
    by_cases hrel : released a = true
    · constructor
      · intro hfalse
        simp [hrel] at hfalse
      · intro hin
        have hkeep := filter_not_mem_released (released := released) (a := a) hin
        simp [hrel] at hkeep
    · constructor
      · intro hb
        have hb' : st.freeBit a = true := by
          simpa [hrel] using hb
        have hmem : a ∈ st.freeList := (hbit a).1 hb'
        exact List.mem_filter.mpr ⟨hmem, by simp [hrel]⟩
      · intro hin
        have hmem : a ∈ st.freeList := (List.mem_filter.mp hin).1
        have hb : st.freeBit a = true := (hbit a).2 hmem
        simp [hrel, hb]
  · exact List.Sublist.nodup List.filter_sublist hnodup

end MeTTaTron.GC.FreeList
