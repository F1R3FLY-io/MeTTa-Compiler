------------------------ MODULE ThunkChannelSeed ------------------------
(***************************************************************************)
(* #309/#266 root cause #1 (THUNK channel) — the H1 namespace-collision    *)
(* hazard of the cross-thread thunk seed, and its domain-tag fix.          *)
(*                                                                         *)
(* The subgoal seed and the thunk seed SHARE one thread-local set           *)
(* (`SEEDED_ACTIVE_SET`). A subgoal seed key is the raw subgoal hash; a     *)
(* thunk seed key is the raw thunk hash. If a live subgoal hash and a live  *)
(* thunk hash collide as u64 (`H_s == H_t`), then:                          *)
(*   - a worker that GENUINELY (non-cyclically) re-enters thunk `H_t` would  *)
(*     probe the shared seed, hit the colliding SUBGOAL seed, and be        *)
(*     SPURIOUSLY cut to Blackhole -> a dropped/wrong bag.                   *)
(*                                                                         *)
(* FIX (`DomainTag`): domain-tag thunk seed keys (`thunk_hash ^             *)
(* THUNK_SEED_DOMAIN`), applied SYMMETRICALLY at the producer               *)
(* (`push_blackhole_seed_keys`) and the consumer (`lookup` Absent probe).   *)
(* A tagged thunk key (domain THK) can never equal an untagged subgoal key  *)
(* (domain SUB), so the spurious cut is impossible — while a LEGITIMATE     *)
(* cyclic thunk re-entry (whose tagged seed the parent really planted)      *)
(* still matches and cuts. This model is what `CrossThreadCycleDetection`    *)
(* cannot express: it has no notion of two hash namespaces sharing a set.   *)
(*                                                                         *)
(* TLC: bug cfg (`DomainTag=FALSE`) violates `NoWrong` via the spurious     *)
(* cut; fixed cfg (`DomainTag=TRUE`) holds for BOTH scenarios — proving the  *)
(* tag kills the spurious cut WITHOUT breaking the legitimate one.          *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS DomainTag   \* FIX toggle: tag thunk seed keys disjoint from subgoal keys

(* A seed key is <<base, domain>>; equality is structural on both fields.    *)
SUB   == 0            \* subgoal namespace
THK   == 1            \* thunk namespace
Hbase == 7            \* the adversarial COLLIDING base: live subgoal hash == thunk hash

(* Producer side: the spurious scenario seeds a SUBGOAL (always untagged);   *)
(* the legitimate scenario seeds the THUNK (tagged iff the fix is on).        *)
SubgoalSeed == <<Hbase, SUB>>
ThunkSeed   == IF DomainTag THEN <<Hbase, THK>> ELSE <<Hbase, SUB>>
(* Consumer side: the worker's thunk probe key for Hbase (tagged iff fix).    *)
ThunkProbe  == IF DomainTag THEN <<Hbase, THK>> ELSE <<Hbase, SUB>>

VARIABLES
  scenario,   \* "spurious" (genuine non-cyclic re-use) | "legitimate" (cyclic re-entry)
  seeded,     \* the shared SEEDED_ACTIVE_SET contents visible to the worker
  result      \* "None" | "Correct" | "Wrong"

vars == <<scenario, seeded, result>>

TypeOK ==
  /\ scenario \in {"spurious", "legitimate"}
  /\ result \in {"None", "Correct", "Wrong"}

Init ==
  /\ scenario \in {"spurious", "legitimate"}
  /\ seeded = (IF scenario = "spurious" THEN {SubgoalSeed} ELSE {ThunkSeed})
  /\ result = "None"

(* The worker re-enters thunk Hbase and probes the shared seed.              *)
(*   spurious  : a GENUINE non-cyclic re-use -> it must NOT cut (cut = Wrong) *)
(*   legitimate: a cyclic re-entry           -> it MUST cut   (no cut = Wrong)*)
Probe ==
  /\ result = "None"
  /\ LET cut == ThunkProbe \in seeded
     IN result' = CASE scenario = "spurious"   -> (IF cut THEN "Wrong" ELSE "Correct")
                    [] scenario = "legitimate" -> (IF cut THEN "Correct" ELSE "Wrong")
  /\ UNCHANGED <<scenario, seeded>>

Next == Probe \/ (result # "None" /\ UNCHANGED vars)

Spec == Init /\ [][Next]_vars

(* Neither a spurious cut of a genuine thunk NOR a missed legitimate cut.     *)
NoWrong == result # "Wrong"
=========================================================================
