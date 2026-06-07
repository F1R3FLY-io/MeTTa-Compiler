/-!
Tier-leaf VM/JIT register roots for the CESK rendezvous collector.

VM and JIT code can reach the collector while live values are held in tier-local
register files rather than in the trampoline S/C/K vectors. The mutator must
publish those values as the `extra` part of its tier-leaf contribution before it
parks. These theorems state the non-TLC safety shape for that publication.
-/

namespace MeTTaTron.GC.TierLeafExtraRoots

variable {Addr : Type u}

inductive VmRegisterRoot
    (ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot BindingStackRoot
     CallFrameRoot ChoicePointRoot CollapseFrameRoot CollapseBindFrameRoot
     PerResultBindingRoot DispatchMemoRoot TrailRoot ExpectedTypeRoot :
        Addr -> Prop) : Addr -> Prop where
  | chunk {a : Addr} : ChunkRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | valueStack {a : Addr} : ValueStackRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | localSlot {a : Addr} : LocalRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | currentBinding {a : Addr} : CurrentBindingRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | bindingStack {a : Addr} : BindingStackRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | callFrame {a : Addr} : CallFrameRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | choicePoint {a : Addr} : ChoicePointRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | collapseFrame {a : Addr} : CollapseFrameRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | collapseBindFrame {a : Addr} : CollapseBindFrameRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | perResultBinding {a : Addr} : PerResultBindingRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | dispatchMemo {a : Addr} : DispatchMemoRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | trail {a : Addr} : TrailRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a
  | expectedType {a : Addr} : ExpectedTypeRoot a ->
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a

inductive JitRegisterRoot
    (HotRoot LiteralPoolRoot ArenaLiteralPoolRoot CurrentChunkRoot
     ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot BindingFrameRoot
     TemplateResultRoot StateCacheRoot : Addr -> Prop) : Addr -> Prop where
  | hot {a : Addr} : HotRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | literalPool {a : Addr} : LiteralPoolRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | arenaLiteralPool {a : Addr} : ArenaLiteralPoolRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | currentChunk {a : Addr} : CurrentChunkRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | valueStack {a : Addr} : ValueStackRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | result {a : Addr} : ResultRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | savedStack {a : Addr} : SavedStackRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | choicePoint {a : Addr} : ChoicePointRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | bindingFrame {a : Addr} : BindingFrameRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | templateResult {a : Addr} : TemplateResultRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a
  | stateCache {a : Addr} : StateCacheRoot a ->
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a

theorem vm_register_root_in_extra
    {ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot BindingStackRoot
     CallFrameRoot ChoicePointRoot CollapseFrameRoot CollapseBindFrameRoot
     PerResultBindingRoot DispatchMemoRoot TrailRoot ExpectedTypeRoot Extra :
        Addr -> Prop}
    (chunkIncluded : forall {a : Addr}, ChunkRoot a -> Extra a)
    (valueStackIncluded : forall {a : Addr}, ValueStackRoot a -> Extra a)
    (localIncluded : forall {a : Addr}, LocalRoot a -> Extra a)
    (currentBindingIncluded : forall {a : Addr}, CurrentBindingRoot a -> Extra a)
    (bindingStackIncluded : forall {a : Addr}, BindingStackRoot a -> Extra a)
    (callFrameIncluded : forall {a : Addr}, CallFrameRoot a -> Extra a)
    (choicePointIncluded : forall {a : Addr}, ChoicePointRoot a -> Extra a)
    (collapseFrameIncluded : forall {a : Addr}, CollapseFrameRoot a -> Extra a)
    (collapseBindFrameIncluded :
      forall {a : Addr}, CollapseBindFrameRoot a -> Extra a)
    (perResultBindingIncluded :
      forall {a : Addr}, PerResultBindingRoot a -> Extra a)
    (dispatchMemoIncluded : forall {a : Addr}, DispatchMemoRoot a -> Extra a)
    (trailIncluded : forall {a : Addr}, TrailRoot a -> Extra a)
    (expectedTypeIncluded : forall {a : Addr}, ExpectedTypeRoot a -> Extra a) :
    forall {a : Addr},
      VmRegisterRoot ChunkRoot ValueStackRoot LocalRoot CurrentBindingRoot
        BindingStackRoot CallFrameRoot ChoicePointRoot CollapseFrameRoot
        CollapseBindFrameRoot PerResultBindingRoot DispatchMemoRoot TrailRoot
        ExpectedTypeRoot a ->
      Extra a := by
  intro a hroot
  cases hroot with
  | chunk h => exact chunkIncluded h
  | valueStack h => exact valueStackIncluded h
  | localSlot h => exact localIncluded h
  | currentBinding h => exact currentBindingIncluded h
  | bindingStack h => exact bindingStackIncluded h
  | callFrame h => exact callFrameIncluded h
  | choicePoint h => exact choicePointIncluded h
  | collapseFrame h => exact collapseFrameIncluded h
  | collapseBindFrame h => exact collapseBindFrameIncluded h
  | perResultBinding h => exact perResultBindingIncluded h
  | dispatchMemo h => exact dispatchMemoIncluded h
  | trail h => exact trailIncluded h
  | expectedType h => exact expectedTypeIncluded h

theorem jit_register_root_in_extra
    {HotRoot LiteralPoolRoot ArenaLiteralPoolRoot CurrentChunkRoot
     ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot BindingFrameRoot
     TemplateResultRoot StateCacheRoot Extra : Addr -> Prop}
    (hotIncluded : forall {a : Addr}, HotRoot a -> Extra a)
    (literalPoolIncluded : forall {a : Addr}, LiteralPoolRoot a -> Extra a)
    (arenaLiteralPoolIncluded :
      forall {a : Addr}, ArenaLiteralPoolRoot a -> Extra a)
    (currentChunkIncluded : forall {a : Addr}, CurrentChunkRoot a -> Extra a)
    (valueStackIncluded : forall {a : Addr}, ValueStackRoot a -> Extra a)
    (resultIncluded : forall {a : Addr}, ResultRoot a -> Extra a)
    (savedStackIncluded : forall {a : Addr}, SavedStackRoot a -> Extra a)
    (choicePointIncluded : forall {a : Addr}, ChoicePointRoot a -> Extra a)
    (bindingFrameIncluded : forall {a : Addr}, BindingFrameRoot a -> Extra a)
    (templateResultIncluded :
      forall {a : Addr}, TemplateResultRoot a -> Extra a)
    (stateCacheIncluded : forall {a : Addr}, StateCacheRoot a -> Extra a) :
    forall {a : Addr},
      JitRegisterRoot HotRoot LiteralPoolRoot ArenaLiteralPoolRoot
        CurrentChunkRoot ValueStackRoot ResultRoot SavedStackRoot ChoicePointRoot
        BindingFrameRoot TemplateResultRoot StateCacheRoot a ->
      Extra a := by
  intro a hroot
  cases hroot with
  | hot h => exact hotIncluded h
  | literalPool h => exact literalPoolIncluded h
  | arenaLiteralPool h => exact arenaLiteralPoolIncluded h
  | currentChunk h => exact currentChunkIncluded h
  | valueStack h => exact valueStackIncluded h
  | result h => exact resultIncluded h
  | savedStack h => exact savedStackIncluded h
  | choicePoint h => exact choicePointIncluded h
  | bindingFrame h => exact bindingFrameIncluded h
  | templateResult h => exact templateResultIncluded h
  | stateCache h => exact stateCacheIncluded h

theorem extra_root_survives_collection
    {Extra ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop}
    (extraIncluded : forall {a : Addr}, Extra a -> ThreadRoot a)
    (publicationBuffersThread :
      forall {a : Addr}, ThreadRoot a -> BufferRoot a)
    (driverDrainsBuffer : forall {a : Addr}, BufferRoot a -> DriverRoot a)
    (markFromDriverRoots : forall {a : Addr}, DriverRoot a -> Marked a)
    (sweepOnlyUnmarked : forall {a : Addr}, Freed a -> Not (Marked a)) :
    forall {a : Addr}, Extra a -> Not (Freed a) := by
  intro a hextra hfreed
  have hthread := extraIncluded hextra
  have hbuffer := publicationBuffersThread hthread
  have hdriver := driverDrainsBuffer hbuffer
  have hmarked := markFromDriverRoots hdriver
  exact sweepOnlyUnmarked hfreed hmarked

end MeTTaTron.GC.TierLeafExtraRoots
