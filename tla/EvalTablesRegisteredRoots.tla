------------------------ MODULE EvalTablesRegisteredRoots ------------------------
(***************************************************************************)
(* Thread-local eval-table structural-root discriminator.                  *)
(*                                                                         *)
(* EVAL_MEMO and MATCH_RESULT_CACHE are thread-local persistent value       *)
(* tables read by collect_global_anchors through collect_eval_memo_roots    *)
(* and collect_match_result_roots.  If either registered table is omitted   *)
(* from the scan, a later sweep can free a cached value that remains live   *)
(* in that thread-local table.                                             *)
(***************************************************************************)

CONSTANTS
    ScanEvalMemo,
    ScanMatchResult

VARIABLES
    phase,
    evalMemoRegistered,
    matchResultRegistered,
    evalMemoScanned,
    matchResultScanned,
    freed

vars ==
    <<phase, evalMemoRegistered, matchResultRegistered,
      evalMemoScanned, matchResultScanned, freed>>

TypeOK ==
    /\ phase \in {"start", "registered", "scanned", "swept", "done"}
    /\ ScanEvalMemo \in BOOLEAN
    /\ ScanMatchResult \in BOOLEAN
    /\ evalMemoRegistered \in BOOLEAN
    /\ matchResultRegistered \in BOOLEAN
    /\ evalMemoScanned \in BOOLEAN
    /\ matchResultScanned \in BOOLEAN
    /\ freed \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ evalMemoRegistered = FALSE
    /\ matchResultRegistered = FALSE
    /\ evalMemoScanned = FALSE
    /\ matchResultScanned = FALSE
    /\ freed = FALSE

RegisterTables ==
    /\ phase = "start"
    /\ phase' = "registered"
    /\ evalMemoRegistered' = TRUE
    /\ matchResultRegistered' = TRUE
    /\ UNCHANGED <<evalMemoScanned, matchResultScanned, freed>>

ScanRoots ==
    /\ phase = "registered"
    /\ phase' = "scanned"
    /\ evalMemoScanned' = (evalMemoRegistered /\ ScanEvalMemo)
    /\ matchResultScanned' = (matchResultRegistered /\ ScanMatchResult)
    /\ UNCHANGED <<evalMemoRegistered, matchResultRegistered, freed>>

Sweep ==
    /\ phase = "scanned"
    /\ phase' = "swept"
    /\ freed' =
        ((evalMemoRegistered /\ ~evalMemoScanned) \/
         (matchResultRegistered /\ ~matchResultScanned))
    /\ UNCHANGED <<evalMemoRegistered, matchResultRegistered,
                  evalMemoScanned, matchResultScanned>>

Finish ==
    /\ phase = "swept"
    /\ phase' = "done"
    /\ UNCHANGED <<evalMemoRegistered, matchResultRegistered,
                  evalMemoScanned, matchResultScanned, freed>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ RegisterTables
    \/ ScanRoots
    \/ Sweep
    \/ Finish
    \/ Done

Spec == Init /\ [][Next]_vars

NoEvalTableValueFreed ==
    ~freed

=============================================================================
