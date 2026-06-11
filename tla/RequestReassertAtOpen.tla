----------------------- MODULE RequestReassertAtOpen -----------------------
(***************************************************************************)
(* Bug #309 — second mechanism: the COALESCED-REQUEST witness starvation.  *)
(*                                                                         *)
(* `request_concurrent_collection` = GC_REQUESTED.store(true) + a          *)
(* CollectRendezvous channel message (two effects). Two coalesced requests *)
(* leave TWO messages. Cycle 1 opens with the flag true; its close         *)
(* (`resume_workers`) clears the flag; cycle 2 then opens FROM THE SECOND  *)
(* MESSAGE with the flag FALSE. Mutators park + stamp their witness slots  *)
(* only at safepoints that observe `is_gc_requested()`, so under the       *)
(* ungated protocol nobody stamps for cycle 2 and the driver's witness     *)
(* wait (`requestor_wait_for_all_reified_parked`) starves forever          *)
(* (captured live: validate rep 36 — cycle_started == cycle_gen,           *)
(* gc_requested FALSE, witness_ok FALSE, occupied_unpublished 4).          *)
(*                                                                         *)
(* THE FIX (ReassertAtOpen = TRUE): the driver re-asserts GC_REQUESTED at  *)
(* every rendezvous OPEN — idempotent on the normal path; the close        *)
(* clears it exactly as before.                                            *)
(***************************************************************************)

EXTENDS Naturals

CONSTANTS ReassertAtOpen        \* TRUE = the fixed protocol

VARIABLES
    msgs,       \* queued CollectRendezvous messages
    requested,  \* GC_REQUESTED
    cycle,      \* "idle" | "open"
    stamped,    \* the (sole, abstracted) mutator stamped this open cycle
    closes      \* completed cycles (domain-capped)

vars == <<msgs, requested, cycle, stamped, closes>>

TypeOK ==
    /\ msgs \in 0..2
    /\ requested \in BOOLEAN
    /\ cycle \in {"idle", "open"}
    /\ stamped \in BOOLEAN
    /\ closes \in 0..2

Init ==
    \* Two coalesced requests: both stores landed (flag true once), two messages.
    /\ msgs = 2
    /\ requested = TRUE
    /\ cycle = "idle"
    /\ stamped = FALSE
    /\ closes = 0

(* The driver dequeues a message and OPENS a rendezvous cycle
   (set_current_cycle_started). The fix re-asserts the request here. *)
DriverOpen ==
    /\ cycle = "idle" /\ msgs > 0
    /\ msgs' = msgs - 1
    /\ cycle' = "open"
    /\ stamped' = FALSE
    /\ requested' = (requested \/ ReassertAtOpen)
    /\ UNCHANGED closes

(* The mutator reaches a safepoint: it parks + stamps ONLY if it observes
   the request flag. (The witness predicate needs every occupied slot
   stamped for the open cycle.) *)
MutatorStamp ==
    /\ cycle = "open" /\ requested /\ ~stamped
    /\ stamped' = TRUE
    /\ UNCHANGED <<msgs, requested, cycle, closes>>

(* The driver's witness wait is satisfied only once stamped; the close
   (end-bump + resume_workers) clears the request flag. *)
DriverClose ==
    /\ cycle = "open" /\ stamped
    /\ cycle' = "idle"
    /\ requested' = FALSE
    /\ stamped' = FALSE
    /\ closes' = closes + 1
    /\ UNCHANGED msgs

(* Terminal stutter: all messages serviced. *)
Terminating ==
    /\ cycle = "idle" /\ msgs = 0
    /\ UNCHANGED vars

Next ==
    \/ DriverOpen
    \/ MutatorStamp
    \/ DriverClose
    \/ Terminating

Spec ==
    Init /\ [][Next]_vars
         /\ WF_vars(DriverOpen) /\ WF_vars(MutatorStamp) /\ WF_vars(DriverClose)

(* LIVENESS: every dequeued message's cycle eventually closes — i.e. both
   queued requests are fully serviced. The BUG config (ReassertAtOpen =
   FALSE) violates it: cycle 2 opens with requested = FALSE, MutatorStamp is
   never enabled, DriverClose never fires — TLC halts in the starved-open
   deadlock (the captured wedge). *)
EveryCycleCloses == <>(closes = 2)

(* SAFETY: an open cycle always has the request visible (so safepoints can
   see it) — the invariant the re-assert restores. *)
OpenImpliesRequested == (cycle = "open") => requested

=============================================================================
