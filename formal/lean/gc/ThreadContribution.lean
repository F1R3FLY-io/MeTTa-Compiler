/-!
Per-mutator thread-contribution obligations for the CESK rendezvous collector.

The dedicated collector never reads another mutator's thread-local machine
registers directly. Each mutator publishes a structural contribution into the
worker-root buffer. These theorems state the safety shape: if the concrete reader
includes every component it claims, and publication/drain/mark/sweep preserve the
usual obligations, then every component root survives collection.
-/

namespace MeTTaTron.GC.ThreadContribution

variable {Addr : Type u}

def TrampolineContributionRoot
    (Extra SCK Env0 Global KSpine Deferred : Addr -> Prop)
    (a : Addr) : Prop :=
  Extra a ∨ SCK a ∨ Env0 a ∨ Global a ∨ KSpine a ∨ Deferred a

def TierLeafContributionRoot
    (Extra Global KSpine : Addr -> Prop)
    (a : Addr) : Prop :=
  Extra a ∨ Global a ∨ KSpine a

theorem trampoline_component_in_thread_root
    {Extra SCK Env0 Global KSpine Deferred ThreadRoot : Addr -> Prop}
    (extraIncluded : forall {a : Addr}, Extra a -> ThreadRoot a)
    (sckIncluded : forall {a : Addr}, SCK a -> ThreadRoot a)
    (env0Included : forall {a : Addr}, Env0 a -> ThreadRoot a)
    (globalIncluded : forall {a : Addr}, Global a -> ThreadRoot a)
    (kSpineIncluded : forall {a : Addr}, KSpine a -> ThreadRoot a)
    (deferredIncluded : forall {a : Addr}, Deferred a -> ThreadRoot a) :
    forall {a : Addr},
      TrampolineContributionRoot Extra SCK Env0 Global KSpine Deferred a ->
      ThreadRoot a := by
  intro a hroot
  cases hroot with
  | inl hextra => exact extraIncluded hextra
  | inr hrest =>
      cases hrest with
      | inl hsck => exact sckIncluded hsck
      | inr hrest =>
          cases hrest with
          | inl henv0 => exact env0Included henv0
          | inr hrest =>
              cases hrest with
              | inl hglobal => exact globalIncluded hglobal
              | inr hrest =>
                  cases hrest with
                  | inl hk => exact kSpineIncluded hk
                  | inr hdeferred => exact deferredIncluded hdeferred

theorem tier_leaf_component_in_thread_root
    {Extra Global KSpine ThreadRoot : Addr -> Prop}
    (extraIncluded : forall {a : Addr}, Extra a -> ThreadRoot a)
    (globalIncluded : forall {a : Addr}, Global a -> ThreadRoot a)
    (kSpineIncluded : forall {a : Addr}, KSpine a -> ThreadRoot a) :
    forall {a : Addr},
      TierLeafContributionRoot Extra Global KSpine a ->
      ThreadRoot a := by
  intro a hroot
  cases hroot with
  | inl hextra => exact extraIncluded hextra
  | inr hrest =>
      cases hrest with
      | inl hglobal => exact globalIncluded hglobal
      | inr hk => exact kSpineIncluded hk

theorem published_thread_contribution_survives_collection
    {ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop}
    (publicationBuffersThread :
      forall {a : Addr}, ThreadRoot a -> BufferRoot a)
    (driverDrainsBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, ThreadRoot a -> Not (Freed a) := by
  intro a hthread hfreed
  have hbuffer := publicationBuffersThread hthread
  have hdriver := driverDrainsBuffer hbuffer
  have hmarked := markFromDriverRoots hdriver
  exact sweepOnlyUnmarked hfreed hmarked

theorem trampoline_contribution_survives_collection
    {Extra SCK Env0 Global KSpine Deferred
      ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop}
    (extraIncluded : forall {a : Addr}, Extra a -> ThreadRoot a)
    (sckIncluded : forall {a : Addr}, SCK a -> ThreadRoot a)
    (env0Included : forall {a : Addr}, Env0 a -> ThreadRoot a)
    (globalIncluded : forall {a : Addr}, Global a -> ThreadRoot a)
    (kSpineIncluded : forall {a : Addr}, KSpine a -> ThreadRoot a)
    (deferredIncluded : forall {a : Addr}, Deferred a -> ThreadRoot a)
    (publicationBuffersThread :
      forall {a : Addr}, ThreadRoot a -> BufferRoot a)
    (driverDrainsBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr},
      TrampolineContributionRoot Extra SCK Env0 Global KSpine Deferred a ->
      Not (Freed a) := by
  intro a hcomponent
  have hthread :
      ThreadRoot a :=
    trampoline_component_in_thread_root
      (Extra := Extra)
      (SCK := SCK)
      (Env0 := Env0)
      (Global := Global)
      (KSpine := KSpine)
      (Deferred := Deferred)
      (ThreadRoot := ThreadRoot)
      extraIncluded
      sckIncluded
      env0Included
      globalIncluded
      kSpineIncluded
      deferredIncluded
      hcomponent
  exact
    published_thread_contribution_survives_collection
      (ThreadRoot := ThreadRoot)
      (BufferRoot := BufferRoot)
      (DriverRoot := DriverRoot)
      (Marked := Marked)
      (Freed := Freed)
      publicationBuffersThread
      driverDrainsBuffer
      markFromDriverRoots
      sweepOnlyUnmarked
      hthread

theorem tier_leaf_contribution_survives_collection
    {Extra Global KSpine
      ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop}
    (extraIncluded : forall {a : Addr}, Extra a -> ThreadRoot a)
    (globalIncluded : forall {a : Addr}, Global a -> ThreadRoot a)
    (kSpineIncluded : forall {a : Addr}, KSpine a -> ThreadRoot a)
    (publicationBuffersThread :
      forall {a : Addr}, ThreadRoot a -> BufferRoot a)
    (driverDrainsBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr},
      TierLeafContributionRoot Extra Global KSpine a ->
      Not (Freed a) := by
  intro a hcomponent
  have hthread :
      ThreadRoot a :=
    tier_leaf_component_in_thread_root
      (Extra := Extra)
      (Global := Global)
      (KSpine := KSpine)
      (ThreadRoot := ThreadRoot)
      extraIncluded
      globalIncluded
      kSpineIncluded
      hcomponent
  exact
    published_thread_contribution_survives_collection
      (ThreadRoot := ThreadRoot)
      (BufferRoot := BufferRoot)
      (DriverRoot := DriverRoot)
      (Marked := Marked)
      (Freed := Freed)
      publicationBuffersThread
      driverDrainsBuffer
      markFromDriverRoots
      sweepOnlyUnmarked
      hthread

end MeTTaTron.GC.ThreadContribution
