; C1.b generational-minor soundness crux: a `change-state!`-stored FRESH (young)
; value MUST survive a minor GC.
;
; The State cell lives in E₀ (the environment's `states` map) — a STRUCTURAL root —
; so the C1.b full mark (the minor marks the full reachable set, NOT a young-only
; mark) marks the stored young value, and `sweep_young` must NOT reclaim it. A bug
; in the young/old boundary or the young hash-cons retain (reclaiming a live young
; slot) surfaces here as a heap-use-after-free at `get-state` under ASAN.
;
; STRUCTURE: mirrors the PROVEN-bounded `stress_alloc.metta` `loop` (a single inline
; tail call, with ALL work in the `let*`/`if` conditions so only `$state`/`$k` are
; live at the tail call ⇒ TCO'd ⇒ bounded live set). A function-call boundary between
; the `let` and the tail recursion DEFEATS TCO (then prior iters' churn accumulates
; and dominates the root set, masking the test) — so everything is inlined here.
;
; Run under: METTATRON_INDEX_GC_MIDLOOP=1 (force mid-eval minors while the value is
; live) + a low METTATRON_INDEX_GC_YOUNG_MIN_BYTES (e.g. 4096) + ASAN.

; A fresh young structured value, distinct per iteration (so each minor sees a NEWLY
; young payload, before promotion reclassifies its segment as old).
(= (make-payload $k)
   (Payload $k (tag alpha beta gamma) (nested (deep $k) (list $k $k $k))))

; Transient young churn — allocates+drops a depth-d Peano wrapper so young live bytes
; climb past the young watermark and trigger a minor WHILE the State value lives.
(= (wrap 0) z)
(= (wrap $n) (s (wrap (- $n 1))))
(= (unwrap z) 0)
(= (unwrap (s $r)) (+ 1 (unwrap $r)))

; Each iter (single inline tail call, TCO'd): store a fresh young payload, churn (~200
; transient young cells → trips a minor under a low young floor), read the State back,
; and verify it is intact. `get-state` + the structural `==` force a full traversal of
; the read-back value → a heap-use-after-free if a minor freed it. Only `$state`/`$k`
; survive to the tail call, so the iteration's churn is reclaimable each round.
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

; 80 store->churn->read cycles. ~16000 transient young cells total drive several
; minors under a low YOUNG_MIN floor; each verifies the live State value survived.
!(bind! &cell (new-state (Payload 0 (tag a) (nested z (list 0 0 0)))))
!(loop 80 &cell)
