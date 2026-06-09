---------------------- MODULE TrampolineFanoutSpineProgress ----------------------
(***************************************************************************)
(* PROGRESS / TERMINATION of the CESK trampoline fan-out + continuation-    *)
(* spine lowering — the missing companion to the spine SAFETY proofs        *)
(* (TrampolineFanoutSpineBridge / TrampolineFanoutProductionRestore /       *)
(* UnifiedChoicePointRestore prove root/value/restore SAFETY; NONE prove    *)
(* progress).                                                               *)
(*                                                                          *)
(* SOURCE (verified by extraction, file:line in the source-coupling gate):  *)
(*   eval_trampoline_inner (eval_loop.rs) drains a work stack. A fan-out     *)
(*   continuation — ProcessRuleMatches / ProcessAmb / ProcessMatchTemplates *)
(*   / ProcessCollapseEvalResults — explores `remaining` branches, consuming *)
(*   EXACTLY ONE per visit (`remaining.next()`) and re-pushing the strictly- *)
(*   smaller tail, with a terminal base case that does NOT re-push when      *)
(*   `remaining` is empty (push Resume instead). Every tick the K stack is   *)
(*   PERSISTED into the spine store (into_trampoline_fanout_spine, a NO-OP   *)
(*   `other => other` on an already-lowered frame ⇒ idempotent) and a frame  *)
(*   is RESOLVED back before processing (resolve_trampoline_fanout_spine).   *)
(*   The lowering is a faithful round-trip via SpineStore alloc/remove —     *)
(*   `resolve(persist(c)) = c` — so it preserves the frame's `remaining`.    *)
(*                                                                          *)
(* MODELED OBLIGATION: the spine lowering is PROGRESS-PRESERVING. A faithful *)
(* round-trip keeps `remaining` strictly decreasing per visit, so the       *)
(* fan-out exploration TERMINATES (<>done). The BUG config                  *)
(* (LoweringFaithful = FALSE: resolve RESETS `remaining`) loses the progress *)
(* the un-lowered machine would make and the exploration never terminates — *)
(* proving the property is non-vacuous and that the faithful round-trip is  *)
(* exactly what guarantees termination. Consequently the trampoline         *)
(* terminates IFF the program's rewrite is well-founded: the spine lowering  *)
(* introduces NO divergence of its own (so an observed hang is a workload    *)
(* non-termination, not a lowering/machine defect).                         *)
(*                                                                          *)
(* TLC: EventuallyDone HOLDS for _faithful.cfg, is VIOLATED for _reset.cfg   *)
(* ("Temporal properties were violated").                                   *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    N,                 \* initial fan-out breadth (branches to explore)
    LoweringFaithful   \* TRUE  = resolve restores the decremented `remaining`
                       \* FALSE = resolve RESETS `remaining` to N (lost progress)

VARIABLES
    remaining,   \* branches left to explore in the fan-out frame (0..N)
    lowered,     \* TRUE iff the frame is currently persisted in the spine store
    done         \* the fan-out frame has been fully drained (terminal base case)

vars == <<remaining, lowered, done>>

TypeOK ==
    /\ remaining \in 0..N
    /\ lowered \in BOOLEAN
    /\ done \in BOOLEAN

Init ==
    /\ remaining = N
    /\ lowered = FALSE
    /\ done = FALSE

(* persist_trampoline_fanout_spines: lower the live fan-out frame into the      *)
(* spine store. Guarded by ~lowered so re-persisting an already-lowered frame   *)
(* is the idempotent no-op (into_trampoline_fanout_spine `other => other`).     *)
Persist ==
    /\ ~done
    /\ ~lowered
    /\ lowered' = TRUE
    /\ UNCHANGED <<remaining, done>>

(* resolve_trampoline_fanout_spine + advance one branch. A FAITHFUL round-trip  *)
(* restores the frame and the advance consumes one branch (remaining - 1); the  *)
(* BUG resets `remaining` to N (re-exploration of the whole fan-out).           *)
ResolveAdvance ==
    /\ ~done
    /\ lowered
    /\ remaining > 0
    /\ lowered' = FALSE
    /\ remaining' = IF LoweringFaithful THEN remaining - 1 ELSE N
    /\ UNCHANGED done

(* Terminal base case: `remaining` exhausted ⇒ finalize (push Resume), no       *)
(* re-push of the fan-out frame.                                                *)
Finish ==
    /\ ~done
    /\ lowered
    /\ remaining = 0
    /\ done' = TRUE
    /\ UNCHANGED <<remaining, lowered>>

(* Stutter once drained so a terminal state is not flagged a deadlock — the     *)
(* LIVENESS property EventuallyDone is the discriminator.                       *)
Terminating ==
    /\ done
    /\ UNCHANGED vars

Next ==
    \/ Persist
    \/ ResolveAdvance
    \/ Finish
    \/ Terminating

(* Weak fairness: the trampoline keeps ticking — it persists, resolves+advances *)
(* a live frame, and finalizes an exhausted one. Termination must hold under    *)
(* this real scheduling.                                                        *)
Fairness ==
    /\ WF_vars(Persist)
    /\ WF_vars(ResolveAdvance)
    /\ WF_vars(Finish)

Spec == Init /\ [][Next]_vars /\ Fairness

(* Safety: `done` is set only after the fan-out is fully explored (no early      *)
(* finalize that would drop branches).                                          *)
Inv == (done => remaining = 0)

(* THE progress/termination property: the fan-out frame is always eventually     *)
(* drained. HOLDS for LoweringFaithful = TRUE (remaining strictly decreases to   *)
(* 0, then Finish); VIOLATED for FALSE (resolve resets remaining ⇒ Finish never  *)
(* enabled ⇒ the never-done lasso).                                              *)
EventuallyDone == <>done

=============================================================================
