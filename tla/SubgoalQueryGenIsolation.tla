-------------------- MODULE SubgoalQueryGenIsolation --------------------
(***************************************************************************)
(* #309/#266 frisbee-drop — formal model of the SubgoalTable freezing a    *)
(* PARTIAL subgoal completion under parallel fanout.                       *)
(*                                                                         *)
(* Mechanism (established by capture + static analysis, 2026-06-17):       *)
(*   A subgoal S's answer set is produced INCREMENTALLY as the parallel    *)
(*   derivation progresses (`derived` grows toward `Answers`). If a worker *)
(*   TABLES S before its derivation is complete, the partial snapshot      *)
(*   (often EMPTY -> result-hash 0x00000000, or Unit) is frozen and later  *)
(*   SERVED to the consumer, dropping a subset of results. Disabling the   *)
(*   cache removes the drop because the partial is then recomputed instead *)
(*   of frozen (fact-7); the drop is independent of GC-sweep frequency     *)
(*   because it is a tabling-logic race, not a sweep/UAF.                   *)
(*                                                                         *)
(* QueryGenGuard models the fix: every table entry is stamped with the     *)
(* generation at which it was tabled; a consumer reading at a LATER        *)
(* generation rejects the stale entry and recomputes against the current   *)
(* (complete) derivation, restoring the full answer set.                   *)
(*                                                                         *)
(*   MC_SubgoalQueryGenIsolation_bug.cfg   : QueryGenGuard = FALSE -> CEX   *)
(*   MC_SubgoalQueryGenIsolation_fixed.cfg : QueryGenGuard = TRUE  -> holds *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Answers,        \* the full answer set S must produce (e.g. {1, 2})
          QueryGenGuard   \* TRUE = fix (gen-isolated table); FALSE = bug

ASSUME Answers # {}

VARIABLES derived,  \* answers of S derived so far (grows monotonically toward Answers)
          gen,      \* generation counter: bumps each time `derived` grows
          table,    \* the tabled entry: [present, vals, g]
          result    \* the consumer's finalized result: [present, vals]

vars == <<derived, gen, table, result>>

NoEntry == [present |-> FALSE, vals |-> {}, g |-> 0]

Init ==
  /\ derived = {}
  /\ gen     = 0
  /\ table   = NoEntry
  /\ result  = [present |-> FALSE, vals |-> {}]

(* A parallel worker derives one more answer of S; the generation advances. *)
Derive ==
  /\ \E a \in (Answers \ derived):
        /\ derived' = derived \cup {a}
        /\ gen'     = gen + 1
  /\ UNCHANGED <<table, result>>

(* A worker TABLES S with the answers derived SO FAR — possibly PARTIAL (the bug seed). *)
TableNow ==
  /\ ~table.present
  /\ table' = [present |-> TRUE, vals |-> derived, g |-> gen]
  /\ UNCHANGED <<derived, gen, result>>

(* The consumer finalizes S only once its derivation is complete (the outer query     *)
(* collects every answer of S before using it). It reads the table if one is present. *)
Consume ==
  /\ ~result.present
  /\ derived = Answers
  /\ LET served ==
           IF ~table.present THEN derived
           ELSE IF QueryGenGuard /\ table.g # gen
                  THEN derived       \* stale generation -> reject + recompute (the fix)
                  ELSE table.vals    \* serve the tabled entry (possibly partial -> the bug)
     IN result' = [present |-> TRUE, vals |-> served]
  /\ UNCHANGED <<derived, gen, table>>

(* Terminal self-loop: once the consumer has finalized, the model has reached its    *)
(* normal end state. Modeled as an explicit stutter so TLC does not report the        *)
(* (legitimate) terminal state as a deadlock during safety checking.                  *)
Done == result.present /\ UNCHANGED vars

Next == Derive \/ TableNow \/ Consume \/ Done

Spec == Init /\ [][Next]_vars

(* SAFETY: a finalized consumer must hold the COMPLETE answer set — no dropped subset. *)
NoDroppedResult == result.present => (result.vals = Answers)
=========================================================================
