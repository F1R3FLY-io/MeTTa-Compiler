; Phase C Increment D (C2) — MIDLOOP cut+young abstract-GC-narrowing ASAN exerciser.
;
; Drives the THREE abstract-GC-narrowed K-frames onto the K-spine with a COMMITTED cut,
; then churns young allocation WHILE the narrowed frame is still live:
;   • ProcessRuleMatches  — the multi-clause `(pick)` (3 matches); `(cut)` commits to the
;                           first, pruning remaining {answer-2, answer-3}.
;   • ProcessAmb          — the `let*` value-expr fan-out the cut crosses (051/401 path).
;   • ProcessMatchTemplates — the `(match &items …)` template fan-out a `(cut)` in the
;                           template commits to the first matched template.
;
; Why this is the soundness ASAN (per docs/cesk-gc/phase-c-increment-d-design.md §C(3)):
; after the cut, `collect_live_values` SKIPS the pruned `remaining_*` field, so a mid-loop
; MINOR's `sweep_young` reclaims those (correctly) dead alternative NODES, and the
; POST-CUT churn below REUSES their slots (and triggers young-segment release — the
; side-free is quiescence-gated, OFF at midloop). With the correct narrowing the pruned
; alternatives are genuinely dead (the cut committed elsewhere) ⇒ 0 UAF. A wrong-narrow
; that dropped a LIVE field would surface as a heap-use-after-free once a reused slot is
; overwritten and a stale ref dereferenced.
;
; `pick`/`item` return COMPOUND payloads (real arena nodes, not inline immediates) so the
; pruned `remaining_*` hold genuine Addrs the minor actually reclaims.
;
; DEPTH NOTE (measured): the `(cut)` keeps `cut-pick`'s `let*` frame live, so `(loop)` is
; NOT tail-call-optimized (unlike side_free_minor.metta's cut-free loop). The loop depth and
; the (also non-tail) `wrap` recursion add JOINTLY — empirically loop+wrap must stay ≲ 800 or
; the run StackOverflows (loop 1500, or loop 500 × wrap 600, both overflow). So the run is
; sized to loop 500 × wrap 300 (≈ 4.8 KiB young/round, ~2.3 MiB total): young_alloc crosses
; YOUNG_BUDGET (2 MiB) exactly ONCE, firing ONE mid-loop MINOR near the end of the run that
; reclaims ~80k dead young slots WHILE a post-cut `cut-pick` frame is live — the non-vacuous
; collection-during-cut signal the ASAN needs. (The narrowing logic itself is proven for all
; three variants by the differential unit tests + the read-site coupling debug_asserts; this
; fixture is the dynamic UAF confirmation that one real minor during a live post-cut frame is
; clean.)
;
; Run arms (scripts/d_midloop_asan.sh):
;   midloop:  MIDLOOP=1 + MIN_BYTES high ⇒ mid-loop MINORS fire while the post-cut frame
;             is live → expect 0 UAF + (midloop minor cycle)>0 (non-vacuous).
;   control:  MIDLOOP unset ⇒ collection only at the directive boundary → SAME result
;             (proves the answer is independent of mid-loop collection).

; --- young-churn building block (peano wrap/unwrap; all transient young nodes) ---
(= (wrap 0) z)
(= (wrap $n) (s (wrap (- $n 1))))
(= (unwrap z) 0)
(= (unwrap (s $r)) (+ 1 (unwrap $r)))

; --- (1) ProcessRuleMatches + (2) ProcessAmb ---
; A multi-clause `pick` whose first answer is committed by a `(cut)` in a later `let*`
; pair; the remaining clauses {answer-2, answer-3} (compound arena nodes) are pruned and
; become young-dead while the frame stays on the K-spine through the post-cut churn.
(= (pick) (answer 1 (payload a a a)))
(= (pick) (answer 2 (payload b b b)))
(= (pick) (answer 3 (payload c c c)))

(= (cut-pick $k)
   (let* (($x  (pick))                 ; ProcessRuleMatches: 3 matches
          ($t  (cut))                  ; commit → prune remaining {answer-2, answer-3}
          ($w  (unwrap (wrap 300)))    ; POST-CUT young churn (reuses the pruned slots)
          ($ok (== $w 300)))
     (if $ok $x (Error churn $k))))

; --- (3) ProcessMatchTemplates ---
; A space with three matching atoms; a `(cut)` in the match template commits to the first
; template, pruning the remaining matched templates.
!(bind! &items (new-space))
!(add-atom &items (item 1 (payload p p p)))
!(add-atom &items (item 2 (payload q q q)))
!(add-atom &items (item 3 (payload r r r)))

(= (cut-match)
   (match &items (item $n $pl)
     (let* (($t (cut))) (matched $n))))

; --- driver: each round commits BOTH cut paths + churns young, then checks the committed
;     answers STRUCTURALLY (forces a full materialize of the live result — the launder
;     traversal that surfaces a UAF if a live result node were wrongly reclaimed). ---
(= (loop 0) done)
(= (loop $k)
   (let* (($a (cut-pick $k))
          ($b (cut-match)))
     (if (== $a (answer 1 (payload a a a)))
         (if (== $b (matched 1))
             (loop (- $k 1))
             (Error match-cut $k))
         (Error rule-cut $k))))

!(loop 500)
