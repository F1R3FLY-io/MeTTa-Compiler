/-!
Epoch-protected cache coherence for reused index addresses.

The CESK index collector can recycle an `Addr` after sweep. A worker-local cache
whose key or payload can encode that address is safe only if every lookup first
observes the global sweep epoch and either keeps a current cache or removes old
entries. This file proves the shared shape used by VALUE_HASH_CACHE, MORK ground
fragments, hash-cons, eval/match memo tables, and OPERATOR_CACHE.
-/

namespace MeTTaTron.GC.EpochProtectedCaches

inductive CacheKind where
  | valueHash
  | morkGround
  | hashCons
  | evalMemo
  | matchResult
  | operator

def ValidatePost
    {Entry : Type u} {Epoch : Type v}
    (heapEpoch localEpoch : Epoch)
    (CacheBefore CacheAfter : Entry -> Prop)
    (Stale : Epoch -> Entry -> Prop) : Prop :=
  (localEpoch = heapEpoch ∧ (∀ e, CacheAfter e -> CacheBefore e)) ∨
  (localEpoch ≠ heapEpoch ∧
    (∀ e, CacheAfter e -> CacheBefore e) ∧
    (∀ e, CacheAfter e -> ¬ Stale heapEpoch e))

def ExplicitClearPost
    {Entry : Type u} {Epoch : Type v}
    (heapEpoch clearedLocalEpoch : Epoch)
    (CacheAfter : Entry -> Prop) : Prop :=
  (∀ e, ¬ CacheAfter e) ∧ clearedLocalEpoch = heapEpoch

theorem lookup_after_validate_not_stale
    {Entry : Type u} {Epoch : Type v}
    {heapEpoch localEpoch : Epoch}
    {CacheBefore CacheAfter Returned : Entry -> Prop}
    {Stale : Epoch -> Entry -> Prop}
    (currentSafe :
      localEpoch = heapEpoch -> ∀ e, CacheBefore e -> ¬ Stale heapEpoch e)
    (validated : ValidatePost heapEpoch localEpoch CacheBefore CacheAfter Stale)
    (lookup : ∀ e, Returned e -> CacheAfter e) :
    ∀ e, Returned e -> ¬ Stale heapEpoch e := by
  intro e hreturned
  cases validated with
  | inl hcurrent =>
      exact currentSafe hcurrent.left e (hcurrent.right e (lookup e hreturned))
  | inr hstale =>
      exact hstale.right.right e (lookup e hreturned)

theorem stale_local_old_entries_miss_after_validate
    {Entry : Type u} {Epoch : Type v}
    {heapEpoch localEpoch : Epoch}
    {CacheBefore CacheAfter Returned : Entry -> Prop}
    {Stale : Epoch -> Entry -> Prop}
    (staleLocal : localEpoch ≠ heapEpoch)
    (oldEntriesStale : ∀ e, CacheBefore e -> Stale heapEpoch e)
    (validated : ValidatePost heapEpoch localEpoch CacheBefore CacheAfter Stale)
    (lookup : ∀ e, Returned e -> CacheAfter e) :
    ∀ e, ¬ Returned e := by
  intro e hreturned
  cases validated with
  | inl hcurrent =>
      exact staleLocal hcurrent.left
  | inr hstale =>
      have hafter := lookup e hreturned
      have hbefore := hstale.right.left e hafter
      exact hstale.right.right e hafter (oldEntriesStale e hbefore)

theorem explicit_clear_leaves_empty_current_cache
    {Entry : Type u} {Epoch : Type v}
    {heapEpoch clearedLocalEpoch : Epoch}
    {CacheAfter : Entry -> Prop}
    (clearPost : ExplicitClearPost heapEpoch clearedLocalEpoch CacheAfter) :
    clearedLocalEpoch = heapEpoch ∧ ∀ e, ¬ CacheAfter e := by
  exact And.intro clearPost.right clearPost.left

theorem explicit_clear_lookup_misses
    {Entry : Type u} {Epoch : Type v}
    {heapEpoch clearedLocalEpoch : Epoch}
    {CacheAfter Returned : Entry -> Prop}
    (clearPost : ExplicitClearPost heapEpoch clearedLocalEpoch CacheAfter)
    (lookup : ∀ e, Returned e -> CacheAfter e) :
    ∀ e, ¬ Returned e := by
  intro e hreturned
  exact clearPost.left e (lookup e hreturned)

end MeTTaTron.GC.EpochProtectedCaches
