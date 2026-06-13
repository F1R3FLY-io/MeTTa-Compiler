; R-FL forced-GC oracle fixture (deterministic free-list-duplicate exerciser).
;
; PURPOSE. Ordinary conformance fixtures do ZERO GC cycles (per-fixture young allocation
; stays under the runtime young budget), so they never exercise `sweep_range`'s
; free-list push. This fixture deliberately allocates+discards MANY MiB of
; transient young S-expressions in a sequential loop so that:
;   • young_alloc_bytes crosses the young budget MANY times      → many MINORS;
;   • with a low METTATRON_INDEX_GC_MIN_BYTES the major floor is crossed too, and
;     after MAJOR_CADENCE (16) minors a MAJOR fires                → major→minor
;     AND minor→minor sweep sequences.
;
; THE BUG IT FORCES (R-FL). The current bump segment is ALWAYS young
; (`promote_young` sets `young_floor = current_seg()` but the current segment
; stays young — it is the live allocation target), so EVERY collection re-sweeps
; it. `sweep_range`'s pre-fix free-list push is non-idempotent: a dead-not-reused
; slot in the current segment is pushed by collection N and pushed AGAIN by
; collection N+1 ⇒ a DUPLICATE free-list entry ⇒ two `pop_young_free_slot` pops
; hand the SAME slot to two MettaValues ⇒ the first is clobbered. The detector
; (`METTATRON_INDEX_GC_FREELIST_CHECK=1`) panics `DUPLICATE FREE-LIST PUSH` on
; the second push — deterministically, on the pre-fix binary.
;
; WHY A PARTIALLY-LIVE CURRENT SEGMENT IS GUARANTEED. The loop threads a SINGLE
; long-lived accumulator (`$acc`, a small nested counter that survives every
; iteration and is rebuilt — its previous version dies) ALONGSIDE a large
; per-iteration transient (`wrap`/`unwrap` of a deep peano list, all of which
; dies at the end of the iteration). So at any collection the current segment
; holds BOTH live slots (the live accumulator + the loop/`let*` continuation
; spine) AND dead slots (last iteration's transient + last accumulator) — the
; partial-live segment that gets re-swept every cycle, the precise R-FL trigger.
;
; FANOUT=0 (no superpose/collapse ⇒ no eval worker spawned) and FANOUT>0
; (dedicated index-GC rendezvous) run the SAME `sweep_range` — the bug is
; mode-independent.
; Deterministic: the final result is always `done`.

; --- per-iteration transient: a deep peano wrapper built then folded to a scalar
;     (every (s ...) cell is young + dies once the iteration's `let*` returns) ---
(= (wrap 0) z)
(= (wrap $n) (s (wrap (- $n 1))))
(= (unwrap z) 0)
(= (unwrap (s $r)) (+ 1 (unwrap $r)))

; --- a small long-lived accumulator structure rebuilt each iteration: the
;     PREVIOUS (tag ...) node dies (young-dead) while the new one lives, keeping
;     the current segment partially-live across collections. Kept intentionally
;     shallow so it survives in `cur_seg` (not promoted out before reuse). ---
(= (bump-acc (tag $a $b $c)) (tag $b $c (+ $a 1)))

; --- driver: each round builds a deep transient (forces hundreds of young
;     allocations), folds it (full traversal — materializes the structure),
;     verifies it, AND rebuilds the live accumulator. The accumulator is threaded
;     through the recursion so it stays reachable across every collection. ---
(= (loop 0 $depth $acc) done)
(= (loop $k $depth $acc)
   (let* (($w   (unwrap (wrap $depth)))      ; deep transient young allocation
          ($ok  (== $w $depth))              ; force the fold + a comparison
          ($acc2 (bump-acc $acc)))           ; rebuild the live accumulator
     (if $ok
         (loop (- $k 1) $depth $acc2)
         (Error mismatch $k))))

; 12000 iterations × depth 220 ≈ 2.6M transient (s ...) cells + the +/- machinery
; + 12000 accumulator rebuilds. At ~24 B/node this is tens of MiB of young churn
; ⇒ young_alloc crosses the runtime young budget many times (many minors), and with a
; low MIN_BYTES the major floor + MAJOR_CADENCE=16 are crossed ⇒ major→minor and
; minor→minor sequences both occur, both re-sweeping the partially-live current
; segment.
!(loop 12000 220 (tag 0 0 0))
