---------------------------- MODULE DedicatedSingleRegime ----------------------------
(***************************************************************************)
(* E1 dedicated-GC single-regime discriminator.                            *)
(*                                                                        *)
(* Under dedicated GC, legacy cooperative producers must not set           *)
(* GC_REQUESTED without posting a dedicated rendezvous driver request.     *)
(***************************************************************************)

CONSTANTS
    GateDefault,
    GateSession,
    GateParallel,
    GateCron

VARIABLES
    phase,
    producer,
    gcRequested,
    driverPosted

vars == <<phase, producer, gcRequested, driverPosted>>

Producers == {"default", "session", "parallel", "cron", "dedicated"}

ProducerGated(p) ==
    CASE p = "default" -> GateDefault
      [] p = "session" -> GateSession
      [] p = "parallel" -> GateParallel
      [] p = "cron" -> GateCron
      [] OTHER -> TRUE

TypeOK ==
    /\ GateDefault \in BOOLEAN
    /\ GateSession \in BOOLEAN
    /\ GateParallel \in BOOLEAN
    /\ GateCron \in BOOLEAN
    /\ phase \in {"start", "chosen", "requested"}
    /\ producer \in Producers
    /\ gcRequested \in BOOLEAN
    /\ driverPosted \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ producer = "dedicated"
    /\ gcRequested = FALSE
    /\ driverPosted = FALSE

ChooseProducer ==
    /\ phase = "start"
    /\ producer' \in Producers
    /\ phase' = "chosen"
    /\ UNCHANGED <<gcRequested, driverPosted>>

Request ==
    /\ phase = "chosen"
    /\ IF producer = "dedicated" THEN
          /\ gcRequested' = TRUE
          /\ driverPosted' = TRUE
       ELSE IF ProducerGated(producer) THEN
          /\ gcRequested' = FALSE
          /\ driverPosted' = FALSE
       ELSE
          /\ gcRequested' = TRUE
          /\ driverPosted' = FALSE
    /\ phase' = "requested"
    /\ UNCHANGED producer

Done ==
    /\ phase = "requested"
    /\ UNCHANGED vars

Next ==
    \/ ChooseProducer
    \/ Request
    \/ Done

Spec == Init /\ [][Next]_vars

NoDriverlessRequest ==
    gcRequested => driverPosted

LegacySuppressed ==
    phase = "requested" /\ producer # "dedicated" => ~gcRequested

DedicatedRequestPostsDriver ==
    phase = "requested" /\ producer = "dedicated" /\ gcRequested => driverPosted

=============================================================================
