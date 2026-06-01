; Phase C Increment A — side-free MINOR-path ASAN exerciser.
;
; Drives a generational MINOR (young_alloc > YOUNG_BUDGET = 2 MiB) at the directive
; boundary with a LIVE young `change-state!` cell, so the minor's `sweep_young`
; reclaims the dead transient churn and the (now quiescence-only) side-free drops
; those dead nodes' payload `Box`es — while the live State cell (an E₀ `states` root)
; is marked and MUST NOT be freed. The persistent cell is read back each round via
; `get-state` + a structural `==`, forcing a full materialize traversal (the launder
; path) — so a side-free that freed a live payload, or a dangling launder'd `&'static`,
; surfaces as a heap-use-after-free under ASAN.
;
; Sizing: loop 400 × wrap 200 ≈ 80k+ transient young SExpr nodes (>2.5 MiB young
; node-slab) accumulate with NO mid-loop collection (MIDLOOP unset), so the quiescence
; collector at the directive boundary fires a MINOR (young_alloc > 2 MiB) rather than a
; major (set MIN_BYTES high so committed never trips the major first). TCO'd single
; inline tail call (mirrors change_state_young.metta) so only $state/$k are live at the
; tail ⇒ the round's churn is reclaimable each minor.
;
; Run arms (scripts/c_A_asan.sh):
;   quiescence-minor: MIN_BYTES high + MIDLOOP unset ⇒ minor at the boundary, side-free
;                     runs (quiescence) → expect 0 UAF.
;   midloop:          MIDLOOP=1 ⇒ mid-loop minors fire while the value is live, but the
;                     side-free is GATED OFF (deferred) → expect 0 UAF (proves the gate).

(= (make-payload $k)
   (Payload $k (tag alpha beta gamma) (nested (deep $k) (list $k $k $k))))

(= (wrap 0) z)
(= (wrap $n) (s (wrap (- $n 1))))
(= (unwrap z) 0)
(= (unwrap (s $r)) (+ 1 (unwrap $r)))

(= (loop 0 $state) done)
(= (loop $k $state)
   (let* (($_ (change-state! $state (make-payload $k)))
          ($w (unwrap (wrap 200)))
          ($got (get-state $state)))
     (if (== $w 200)
         (if (== $got (make-payload $k))
             (loop (- $k 1) $state)
             (Error state-value-lost $k))
         (Error churn-mismatch $k))))

!(bind! &cell (new-state (Payload 0 (tag a) (nested z (list 0 0 0)))))
!(loop 400 &cell)
