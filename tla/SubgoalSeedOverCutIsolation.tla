----------------- MODULE SubgoalSeedOverCutIsolation -----------------
(***************************************************************************)
(* #309/#266 frisbee-drop — formal model of the IMPLEMENTED fix:           *)
(* PRECISE SEEDING of the cross-thread fixpoint active-set (commit         *)
(* 2ecdf677, confirmed 2026-06-18).                                        *)
(*                                                                         *)
(* BACKGROUND.  A fanned-out worker starts a FRESH trampoline with an      *)
(* empty thread-local active-evaluation set, so a subgoal that was active  *)
(* on the FORKING thread's lineage is not seen as a cycle by the worker.   *)
(* To make a cross-thread recursive re-entry cut to the fixpoint EMPTY     *)
(* exactly as the single-threaded inline evaluation would, the worker      *)
(* inherits a SEED of the forking thread's active subgoal hashes           *)
(* (SEEDED_ACTIVE_SET); is_actively_evaluating consults it.                *)
(*                                                                         *)
(* THE BUG.  The seed carried EVERY active subgoal, including              *)
(* NON-recursive ones.  A worker then cut its FIRST occurrence of a        *)
(* CONSTANT subgoal (e.g. (kb)) to EMPTY — an INVALID result, because a    *)
(* constant has exactly ONE valid value and EMPTY is not it.  The          *)
(* SubgoalTable froze that transient EMPTY and served it to a later        *)
(* identical lookup, dropping a detection (the "frisbee-drop").            *)
(*                                                                         *)
(* THE FIX (precise seeding).  Seed ONLY genuinely RECURSIVE subgoals      *)
(* (a fixpoint head that re-enters itself, detected dynamically via        *)
(* HEAD_ACTIVE_SET: a subgoal pushed while its head is already active).    *)
(* A non-recursive constant is therefore NEVER seeded, NEVER cut, and      *)
(* derives its full answer set.  A genuine recursion is STILL seeded and   *)
(* STILL cut to its (correct) EMPTY fixpoint base, so the original         *)
(* root-cause-#1 cross-thread cycle fix is preserved.                      *)
(*                                                                         *)
(* MODEL.  One subgoal S whose recursiveness is chosen nondeterministically*)
(* at Init (sIsRecursive \in BOOLEAN, so TLC explores BOTH a constant and  *)
(* a recursion).  Seed membership is the precise-seeding predicate:        *)
(*   - bug  (PreciseSeeding = FALSE): seed EVERY active subgoal  => seeded *)
(*   - fix  (PreciseSeeding = TRUE) : seed only if recursive     => seeded *)
(*                                     == sIsRecursive                      *)
(* A seeded subgoal is cut to EMPTY; an unseeded one derives `Answers`.    *)
(* The CORRECT answer is EMPTY for a recursion (its fixpoint base) and     *)
(* `Answers` for a constant.                                               *)
(*                                                                         *)
(*   MC_SubgoalSeedOverCutIsolation_bug.cfg                                *)
(*     : PreciseSeeding = FALSE -> the CONSTANT state is over-cut to EMPTY *)
(*       -> NoOverCut VIOLATED (the frisbee-drop, deterministically).      *)
(*   MC_SubgoalSeedOverCutIsolation_fixed.cfg                             *)
(*     : PreciseSeeding = TRUE  -> constant derives full, recursion still  *)
(*       cuts -> NoOverCut HOLDS for both states.                          *)
(*                                                                         *)
(* This is the IMPLEMENTED-fix companion to SubgoalQueryGenIsolation.tla,  *)
(* which models the more abstract freeze-class (a partial table entry      *)
(* frozen + served) and an equivalent serve-point guard (QueryGenGuard, a  *)
(* candidate that was NOT the route taken).                                *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Answers,        \* the subgoal's full (non-empty) derivation when not cut
          PreciseSeeding  \* TRUE = fix (seed only recursive); FALSE = bug (seed all active)

ASSUME Answers # {}

VARIABLES sIsRecursive,  \* is S a genuine recursion (fixpoint base = EMPTY) or a constant?
          seeded,        \* is S present in this worker's cross-thread seed?
          result         \* S's finalized result: [present, vals]

vars == <<sIsRecursive, seeded, result>>

NoResult == [present |-> FALSE, vals |-> {}]

(* Init picks S's nature nondeterministically and derives seed membership  *)
(* from the precise-seeding predicate, so ONE cfg checks both the constant *)
(* (sIsRecursive = FALSE) and the recursion (sIsRecursive = TRUE) state.   *)
Init ==
  /\ sIsRecursive \in BOOLEAN
  /\ seeded = (IF PreciseSeeding THEN sIsRecursive ELSE TRUE)
  /\ result = NoResult

(* The single valid answer for S: EMPTY iff it is a genuine recursion      *)
(* (its fixpoint base), else its full constant derivation `Answers`.       *)
Correct == IF sIsRecursive THEN {} ELSE Answers

(* The worker finalizes S: a seeded subgoal is detected as an active cycle *)
(* and cut to EMPTY; an unseeded subgoal derives its full answer set.      *)
Finalize ==
  /\ ~result.present
  /\ result' = [present |-> TRUE, vals |-> IF seeded THEN {} ELSE Answers]
  /\ UNCHANGED <<sIsRecursive, seeded>>

(* Terminal self-loop so TLC does not flag the legitimate end state as a   *)
(* deadlock during safety checking.                                        *)
Done == result.present /\ UNCHANGED vars

Next == Finalize \/ Done

Spec == Init /\ [][Next]_vars

(* SAFETY: a finalized S must hold its sole valid answer — never the       *)
(* over-cut EMPTY for a constant (the frisbee-drop), and still EMPTY for   *)
(* a genuine recursion (precise seeding must not regress root-cause-#1).   *)
NoOverCut == result.present => (result.vals = Correct)
=======================================================================
