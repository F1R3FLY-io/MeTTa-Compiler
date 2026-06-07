-------------------------- MODULE MajorWatermarkRearm --------------------------
EXTENDS Integers
(***************************************************************************)
(* B.4 old-live major watermark rearm model.                             *)
(*                                                                        *)
(* Production rearm uses post-promote old live bytes and GROWTH = 2:      *)
(* watermark := max(old_live_after * 2, min_threshold).  The live-growth   *)
(* major trigger is old_live > max(watermark, min_threshold).              *)
(*                                                                        *)
(* UseOldLiveRearm=FALSE => watermark is not rearmed from old_live_after.  *)
(* UseGrowth2=FALSE      => watermark omits the factor-2 growth.           *)
(***************************************************************************)

CONSTANTS UseOldLiveRearm, UseGrowth2

VARIABLES
    phase,
    oldLiveAfter,
    minThreshold,
    oldLiveNext,
    watermark,
    immediateLiveMajor,
    futureLiveMajor

vars == <<phase, oldLiveAfter, minThreshold, oldLiveNext, watermark,
          immediateLiveMajor, futureLiveMajor>>

Max(a, b) == IF a >= b THEN a ELSE b

DoubledOldLive == oldLiveAfter + oldLiveAfter

RearmValue ==
    IF UseOldLiveRearm THEN
        IF UseGrowth2 THEN Max(DoubledOldLive, minThreshold)
        ELSE oldLiveAfter
    ELSE 0

TypeOK ==
    /\ UseOldLiveRearm \in BOOLEAN
    /\ UseGrowth2 \in BOOLEAN
    /\ phase \in {"start", "chosen", "checked"}
    /\ oldLiveAfter \in 0..4
    /\ minThreshold \in 0..4
    /\ oldLiveNext \in 0..8
    /\ watermark \in 0..8
    /\ immediateLiveMajor \in BOOLEAN
    /\ futureLiveMajor \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ oldLiveAfter = 0
    /\ minThreshold = 0
    /\ oldLiveNext = 0
    /\ watermark = 0
    /\ immediateLiveMajor = FALSE
    /\ futureLiveMajor = FALSE

PickInputs ==
    /\ phase = "start"
    /\ oldLiveAfter' \in 0..4
    /\ minThreshold' \in 0..4
    /\ oldLiveNext' \in 0..8
    /\ phase' = "chosen"
    /\ UNCHANGED <<watermark, immediateLiveMajor, futureLiveMajor>>

RearmAndCheck ==
    /\ phase = "chosen"
    /\ watermark' = RearmValue
    /\ immediateLiveMajor' = (oldLiveAfter > Max(watermark', minThreshold))
    /\ futureLiveMajor' = (oldLiveNext > Max(watermark', minThreshold))
    /\ phase' = "checked"
    /\ UNCHANGED <<oldLiveAfter, minThreshold, oldLiveNext>>

Done ==
    /\ phase = "checked"
    /\ UNCHANGED vars

Next ==
    \/ PickInputs
    \/ RearmAndCheck
    \/ Done

Spec == Init /\ [][Next]_vars

ImmediateOldLiveDoesNotRefire ==
    ~(phase = "checked" /\ immediateLiveMajor)

FutureRefireRequiresDoubledGrowth ==
    ~(phase = "checked" /\ futureLiveMajor /\ oldLiveNext <= DoubledOldLive)

FutureRefireRequiresMinThresholdGrowth ==
    ~(phase = "checked" /\ futureLiveMajor /\ oldLiveNext <= minThreshold)

WatermarkAtLeastMinThreshold ==
    ~(phase = "checked" /\ watermark < minThreshold)

=============================================================================
