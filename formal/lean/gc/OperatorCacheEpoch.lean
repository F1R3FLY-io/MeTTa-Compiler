/-!
Operator-cache sweep-epoch coherence obligation.

The trampoline operator cache is keyed partly by an interned atom pointer. Under
the index collector, a sweep can recycle addresses while another worker still
owns a thread-local cache entry. The live source therefore checks
`gc_sweep_epoch` before every operator-cache lookup, clears the cache when the
local epoch is stale, and records the current epoch after explicit clears. This
proof captures the abstract cache-coherence shape behind that source coupling.
-/

namespace MeTTaTron.GC.OperatorCacheEpoch

def EnsurePost
    {Entry : Type u} {Epoch : Type v}
    (heapEpoch localEpoch : Epoch)
    (CacheBefore CacheAfter : Entry -> Prop) : Prop :=
  (localEpoch = heapEpoch ∧ (∀ e, CacheAfter e -> CacheBefore e)) ∨
  (localEpoch ≠ heapEpoch ∧ (∀ e, ¬ CacheAfter e))

def ExplicitClearPost
    {Entry : Type u} {Epoch : Type v}
    (heapEpoch clearedLocalEpoch : Epoch)
    (CacheAfter : Entry -> Prop) : Prop :=
  (∀ e, ¬ CacheAfter e) ∧ clearedLocalEpoch = heapEpoch

theorem returned_entry_has_current_epoch_after_ensure
    {Entry : Type u} {Epoch : Type v}
    {entryEpoch : Entry -> Epoch}
    {heapEpoch localEpoch : Epoch}
    {CacheBefore CacheAfter Returned : Entry -> Prop}
    (stamped : ∀ e, CacheBefore e -> entryEpoch e = localEpoch)
    (ensurePost : EnsurePost heapEpoch localEpoch CacheBefore CacheAfter)
    (lookup : ∀ e, Returned e -> CacheAfter e) :
    ∀ e, Returned e -> entryEpoch e = heapEpoch := by
  intro e hreturned
  cases ensurePost with
  | inl hcurrent =>
      exact Eq.trans (stamped e (hcurrent.right e (lookup e hreturned))) hcurrent.left
  | inr hstale =>
      exact False.elim (hstale.right e (lookup e hreturned))

theorem stale_epoch_lookup_misses_after_ensure
    {Entry : Type u} {Epoch : Type v}
    {heapEpoch localEpoch : Epoch}
    {CacheBefore CacheAfter Returned : Entry -> Prop}
    (stale : localEpoch ≠ heapEpoch)
    (ensurePost : EnsurePost heapEpoch localEpoch CacheBefore CacheAfter)
    (lookup : ∀ e, Returned e -> CacheAfter e) :
    ∀ e, ¬ Returned e := by
  intro e hreturned
  cases ensurePost with
  | inl hcurrent =>
      exact stale hcurrent.left
  | inr hstale =>
      exact hstale.right e (lookup e hreturned)

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

end MeTTaTron.GC.OperatorCacheEpoch
