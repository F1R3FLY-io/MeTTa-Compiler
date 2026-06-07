-------------------------- MODULE TierLeafExtraRoots --------------------------
(***************************************************************************)
(* Tier-leaf VM/JIT register-file roots published as `extra` before a       *)
(* cooperative CESK rendezvous park.                                        *)
(***************************************************************************)

CONSTANTS
    IncludeVmChunks,
    IncludeVmValueStack,
    IncludeVmLocals,
    IncludeVmCurrentBindings,
    IncludeVmBindingStack,
    IncludeVmCallFrames,
    IncludeVmChoicePoints,
    IncludeVmCollapseFrames,
    IncludeVmCollapseBindFrames,
    IncludeVmPerResultBindings,
    IncludeVmDispatchMemo,
    IncludeVmTrail,
    IncludeVmExpectedType,
    IncludeJitHot,
    IncludeJitLiteralPool,
    IncludeJitArenaLiteralPool,
    IncludeJitCurrentChunk,
    IncludeJitValueStack,
    IncludeJitResults,
    IncludeJitSavedStack,
    IncludeJitChoicePoints,
    IncludeJitBindingFrames,
    IncludeJitTemplateResults,
    IncludeJitStateCache

VARIABLES
    built,
    vmChunksLive,
    vmValueStackLive,
    vmLocalsLive,
    vmCurrentBindingsLive,
    vmBindingStackLive,
    vmCallFramesLive,
    vmChoicePointsLive,
    vmCollapseFramesLive,
    vmCollapseBindFramesLive,
    vmPerResultBindingsLive,
    vmDispatchMemoLive,
    vmTrailLive,
    vmExpectedTypeLive,
    jitHotLive,
    jitLiteralPoolLive,
    jitArenaLiteralPoolLive,
    jitCurrentChunkLive,
    jitValueStackLive,
    jitResultsLive,
    jitSavedStackLive,
    jitChoicePointsLive,
    jitBindingFramesLive,
    jitTemplateResultsLive,
    jitStateCacheLive,
    vmChunksRooted,
    vmValueStackRooted,
    vmLocalsRooted,
    vmCurrentBindingsRooted,
    vmBindingStackRooted,
    vmCallFramesRooted,
    vmChoicePointsRooted,
    vmCollapseFramesRooted,
    vmCollapseBindFramesRooted,
    vmPerResultBindingsRooted,
    vmDispatchMemoRooted,
    vmTrailRooted,
    vmExpectedTypeRooted,
    jitHotRooted,
    jitLiteralPoolRooted,
    jitArenaLiteralPoolRooted,
    jitCurrentChunkRooted,
    jitValueStackRooted,
    jitResultsRooted,
    jitSavedStackRooted,
    jitChoicePointsRooted,
    jitBindingFramesRooted,
    jitTemplateResultsRooted,
    jitStateCacheRooted,
    swept

vars ==
    <<built,
      vmChunksLive, vmValueStackLive, vmLocalsLive, vmCurrentBindingsLive,
      vmBindingStackLive, vmCallFramesLive, vmChoicePointsLive,
      vmCollapseFramesLive, vmCollapseBindFramesLive, vmPerResultBindingsLive,
      vmDispatchMemoLive, vmTrailLive, vmExpectedTypeLive,
      jitHotLive, jitLiteralPoolLive, jitArenaLiteralPoolLive,
      jitCurrentChunkLive, jitValueStackLive, jitResultsLive,
      jitSavedStackLive, jitChoicePointsLive, jitBindingFramesLive,
      jitTemplateResultsLive, jitStateCacheLive,
      vmChunksRooted, vmValueStackRooted, vmLocalsRooted,
      vmCurrentBindingsRooted, vmBindingStackRooted, vmCallFramesRooted,
      vmChoicePointsRooted, vmCollapseFramesRooted,
      vmCollapseBindFramesRooted, vmPerResultBindingsRooted,
      vmDispatchMemoRooted, vmTrailRooted, vmExpectedTypeRooted,
      jitHotRooted, jitLiteralPoolRooted, jitArenaLiteralPoolRooted,
      jitCurrentChunkRooted, jitValueStackRooted, jitResultsRooted,
      jitSavedStackRooted, jitChoicePointsRooted, jitBindingFramesRooted,
      jitTemplateResultsRooted, jitStateCacheRooted, swept>>

TypeOK ==
    /\ IncludeVmChunks \in BOOLEAN
    /\ IncludeVmValueStack \in BOOLEAN
    /\ IncludeVmLocals \in BOOLEAN
    /\ IncludeVmCurrentBindings \in BOOLEAN
    /\ IncludeVmBindingStack \in BOOLEAN
    /\ IncludeVmCallFrames \in BOOLEAN
    /\ IncludeVmChoicePoints \in BOOLEAN
    /\ IncludeVmCollapseFrames \in BOOLEAN
    /\ IncludeVmCollapseBindFrames \in BOOLEAN
    /\ IncludeVmPerResultBindings \in BOOLEAN
    /\ IncludeVmDispatchMemo \in BOOLEAN
    /\ IncludeVmTrail \in BOOLEAN
    /\ IncludeVmExpectedType \in BOOLEAN
    /\ IncludeJitHot \in BOOLEAN
    /\ IncludeJitLiteralPool \in BOOLEAN
    /\ IncludeJitArenaLiteralPool \in BOOLEAN
    /\ IncludeJitCurrentChunk \in BOOLEAN
    /\ IncludeJitValueStack \in BOOLEAN
    /\ IncludeJitResults \in BOOLEAN
    /\ IncludeJitSavedStack \in BOOLEAN
    /\ IncludeJitChoicePoints \in BOOLEAN
    /\ IncludeJitBindingFrames \in BOOLEAN
    /\ IncludeJitTemplateResults \in BOOLEAN
    /\ IncludeJitStateCache \in BOOLEAN
    /\ built \in BOOLEAN
    /\ swept \in BOOLEAN
    /\ vmChunksLive \in BOOLEAN
    /\ vmValueStackLive \in BOOLEAN
    /\ vmLocalsLive \in BOOLEAN
    /\ vmCurrentBindingsLive \in BOOLEAN
    /\ vmBindingStackLive \in BOOLEAN
    /\ vmCallFramesLive \in BOOLEAN
    /\ vmChoicePointsLive \in BOOLEAN
    /\ vmCollapseFramesLive \in BOOLEAN
    /\ vmCollapseBindFramesLive \in BOOLEAN
    /\ vmPerResultBindingsLive \in BOOLEAN
    /\ vmDispatchMemoLive \in BOOLEAN
    /\ vmTrailLive \in BOOLEAN
    /\ vmExpectedTypeLive \in BOOLEAN
    /\ jitHotLive \in BOOLEAN
    /\ jitLiteralPoolLive \in BOOLEAN
    /\ jitArenaLiteralPoolLive \in BOOLEAN
    /\ jitCurrentChunkLive \in BOOLEAN
    /\ jitValueStackLive \in BOOLEAN
    /\ jitResultsLive \in BOOLEAN
    /\ jitSavedStackLive \in BOOLEAN
    /\ jitChoicePointsLive \in BOOLEAN
    /\ jitBindingFramesLive \in BOOLEAN
    /\ jitTemplateResultsLive \in BOOLEAN
    /\ jitStateCacheLive \in BOOLEAN
    /\ vmChunksRooted \in BOOLEAN
    /\ vmValueStackRooted \in BOOLEAN
    /\ vmLocalsRooted \in BOOLEAN
    /\ vmCurrentBindingsRooted \in BOOLEAN
    /\ vmBindingStackRooted \in BOOLEAN
    /\ vmCallFramesRooted \in BOOLEAN
    /\ vmChoicePointsRooted \in BOOLEAN
    /\ vmCollapseFramesRooted \in BOOLEAN
    /\ vmCollapseBindFramesRooted \in BOOLEAN
    /\ vmPerResultBindingsRooted \in BOOLEAN
    /\ vmDispatchMemoRooted \in BOOLEAN
    /\ vmTrailRooted \in BOOLEAN
    /\ vmExpectedTypeRooted \in BOOLEAN
    /\ jitHotRooted \in BOOLEAN
    /\ jitLiteralPoolRooted \in BOOLEAN
    /\ jitArenaLiteralPoolRooted \in BOOLEAN
    /\ jitCurrentChunkRooted \in BOOLEAN
    /\ jitValueStackRooted \in BOOLEAN
    /\ jitResultsRooted \in BOOLEAN
    /\ jitSavedStackRooted \in BOOLEAN
    /\ jitChoicePointsRooted \in BOOLEAN
    /\ jitBindingFramesRooted \in BOOLEAN
    /\ jitTemplateResultsRooted \in BOOLEAN
    /\ jitStateCacheRooted \in BOOLEAN

Init ==
    /\ built = FALSE
    /\ swept = FALSE
    /\ vmChunksLive = TRUE
    /\ vmValueStackLive = TRUE
    /\ vmLocalsLive = TRUE
    /\ vmCurrentBindingsLive = TRUE
    /\ vmBindingStackLive = TRUE
    /\ vmCallFramesLive = TRUE
    /\ vmChoicePointsLive = TRUE
    /\ vmCollapseFramesLive = TRUE
    /\ vmCollapseBindFramesLive = TRUE
    /\ vmPerResultBindingsLive = TRUE
    /\ vmDispatchMemoLive = TRUE
    /\ vmTrailLive = TRUE
    /\ vmExpectedTypeLive = TRUE
    /\ jitHotLive = TRUE
    /\ jitLiteralPoolLive = TRUE
    /\ jitArenaLiteralPoolLive = TRUE
    /\ jitCurrentChunkLive = TRUE
    /\ jitValueStackLive = TRUE
    /\ jitResultsLive = TRUE
    /\ jitSavedStackLive = TRUE
    /\ jitChoicePointsLive = TRUE
    /\ jitBindingFramesLive = TRUE
    /\ jitTemplateResultsLive = TRUE
    /\ jitStateCacheLive = TRUE
    /\ vmChunksRooted = FALSE
    /\ vmValueStackRooted = FALSE
    /\ vmLocalsRooted = FALSE
    /\ vmCurrentBindingsRooted = FALSE
    /\ vmBindingStackRooted = FALSE
    /\ vmCallFramesRooted = FALSE
    /\ vmChoicePointsRooted = FALSE
    /\ vmCollapseFramesRooted = FALSE
    /\ vmCollapseBindFramesRooted = FALSE
    /\ vmPerResultBindingsRooted = FALSE
    /\ vmDispatchMemoRooted = FALSE
    /\ vmTrailRooted = FALSE
    /\ vmExpectedTypeRooted = FALSE
    /\ jitHotRooted = FALSE
    /\ jitLiteralPoolRooted = FALSE
    /\ jitArenaLiteralPoolRooted = FALSE
    /\ jitCurrentChunkRooted = FALSE
    /\ jitValueStackRooted = FALSE
    /\ jitResultsRooted = FALSE
    /\ jitSavedStackRooted = FALSE
    /\ jitChoicePointsRooted = FALSE
    /\ jitBindingFramesRooted = FALSE
    /\ jitTemplateResultsRooted = FALSE
    /\ jitStateCacheRooted = FALSE

BuildExtraRoots ==
    /\ ~built
    /\ ~swept
    /\ built' = TRUE
    /\ vmChunksRooted' = IncludeVmChunks /\ vmChunksLive
    /\ vmValueStackRooted' = IncludeVmValueStack /\ vmValueStackLive
    /\ vmLocalsRooted' = IncludeVmLocals /\ vmLocalsLive
    /\ vmCurrentBindingsRooted' = IncludeVmCurrentBindings /\ vmCurrentBindingsLive
    /\ vmBindingStackRooted' = IncludeVmBindingStack /\ vmBindingStackLive
    /\ vmCallFramesRooted' = IncludeVmCallFrames /\ vmCallFramesLive
    /\ vmChoicePointsRooted' = IncludeVmChoicePoints /\ vmChoicePointsLive
    /\ vmCollapseFramesRooted' = IncludeVmCollapseFrames /\ vmCollapseFramesLive
    /\ vmCollapseBindFramesRooted' =
        IncludeVmCollapseBindFrames /\ vmCollapseBindFramesLive
    /\ vmPerResultBindingsRooted' =
        IncludeVmPerResultBindings /\ vmPerResultBindingsLive
    /\ vmDispatchMemoRooted' = IncludeVmDispatchMemo /\ vmDispatchMemoLive
    /\ vmTrailRooted' = IncludeVmTrail /\ vmTrailLive
    /\ vmExpectedTypeRooted' = IncludeVmExpectedType /\ vmExpectedTypeLive
    /\ jitHotRooted' = IncludeJitHot /\ jitHotLive
    /\ jitLiteralPoolRooted' = IncludeJitLiteralPool /\ jitLiteralPoolLive
    /\ jitArenaLiteralPoolRooted' =
        IncludeJitArenaLiteralPool /\ jitArenaLiteralPoolLive
    /\ jitCurrentChunkRooted' = IncludeJitCurrentChunk /\ jitCurrentChunkLive
    /\ jitValueStackRooted' = IncludeJitValueStack /\ jitValueStackLive
    /\ jitResultsRooted' = IncludeJitResults /\ jitResultsLive
    /\ jitSavedStackRooted' = IncludeJitSavedStack /\ jitSavedStackLive
    /\ jitChoicePointsRooted' = IncludeJitChoicePoints /\ jitChoicePointsLive
    /\ jitBindingFramesRooted' = IncludeJitBindingFrames /\ jitBindingFramesLive
    /\ jitTemplateResultsRooted' =
        IncludeJitTemplateResults /\ jitTemplateResultsLive
    /\ jitStateCacheRooted' = IncludeJitStateCache /\ jitStateCacheLive
    /\ UNCHANGED <<vmChunksLive, vmValueStackLive, vmLocalsLive,
                  vmCurrentBindingsLive, vmBindingStackLive, vmCallFramesLive,
                  vmChoicePointsLive, vmCollapseFramesLive,
                  vmCollapseBindFramesLive, vmPerResultBindingsLive,
                  vmDispatchMemoLive, vmTrailLive, vmExpectedTypeLive,
                  jitHotLive, jitLiteralPoolLive, jitArenaLiteralPoolLive,
                  jitCurrentChunkLive, jitValueStackLive, jitResultsLive,
                  jitSavedStackLive, jitChoicePointsLive, jitBindingFramesLive,
                  jitTemplateResultsLive, jitStateCacheLive, swept>>

Sweep ==
    /\ built
    /\ ~swept
    /\ swept' = TRUE
    /\ UNCHANGED <<built,
                  vmChunksLive, vmValueStackLive, vmLocalsLive,
                  vmCurrentBindingsLive, vmBindingStackLive, vmCallFramesLive,
                  vmChoicePointsLive, vmCollapseFramesLive,
                  vmCollapseBindFramesLive, vmPerResultBindingsLive,
                  vmDispatchMemoLive, vmTrailLive, vmExpectedTypeLive,
                  jitHotLive, jitLiteralPoolLive, jitArenaLiteralPoolLive,
                  jitCurrentChunkLive, jitValueStackLive, jitResultsLive,
                  jitSavedStackLive, jitChoicePointsLive, jitBindingFramesLive,
                  jitTemplateResultsLive, jitStateCacheLive,
                  vmChunksRooted, vmValueStackRooted, vmLocalsRooted,
                  vmCurrentBindingsRooted, vmBindingStackRooted,
                  vmCallFramesRooted, vmChoicePointsRooted,
                  vmCollapseFramesRooted, vmCollapseBindFramesRooted,
                  vmPerResultBindingsRooted, vmDispatchMemoRooted,
                  vmTrailRooted, vmExpectedTypeRooted, jitHotRooted,
                  jitLiteralPoolRooted, jitArenaLiteralPoolRooted,
                  jitCurrentChunkRooted, jitValueStackRooted, jitResultsRooted,
                  jitSavedStackRooted, jitChoicePointsRooted,
                  jitBindingFramesRooted, jitTemplateResultsRooted,
                  jitStateCacheRooted>>

Done ==
    /\ swept
    /\ UNCHANGED vars

Next ==
    \/ BuildExtraRoots
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

TierLeafExtraRootsComplete ==
    swept =>
      /\ vmChunksLive => vmChunksRooted
      /\ vmValueStackLive => vmValueStackRooted
      /\ vmLocalsLive => vmLocalsRooted
      /\ vmCurrentBindingsLive => vmCurrentBindingsRooted
      /\ vmBindingStackLive => vmBindingStackRooted
      /\ vmCallFramesLive => vmCallFramesRooted
      /\ vmChoicePointsLive => vmChoicePointsRooted
      /\ vmCollapseFramesLive => vmCollapseFramesRooted
      /\ vmCollapseBindFramesLive => vmCollapseBindFramesRooted
      /\ vmPerResultBindingsLive => vmPerResultBindingsRooted
      /\ vmDispatchMemoLive => vmDispatchMemoRooted
      /\ vmTrailLive => vmTrailRooted
      /\ vmExpectedTypeLive => vmExpectedTypeRooted
      /\ jitHotLive => jitHotRooted
      /\ jitLiteralPoolLive => jitLiteralPoolRooted
      /\ jitArenaLiteralPoolLive => jitArenaLiteralPoolRooted
      /\ jitCurrentChunkLive => jitCurrentChunkRooted
      /\ jitValueStackLive => jitValueStackRooted
      /\ jitResultsLive => jitResultsRooted
      /\ jitSavedStackLive => jitSavedStackRooted
      /\ jitChoicePointsLive => jitChoicePointsRooted
      /\ jitBindingFramesLive => jitBindingFramesRooted
      /\ jitTemplateResultsLive => jitTemplateResultsRooted
      /\ jitStateCacheLive => jitStateCacheRooted

=============================================================================
