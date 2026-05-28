; Inc-6 single-threaded index GC stress workload.
;
; Allocation-heavy, deterministic, fully sequential (no superpose/collapse, so
; no eval worker is ever spawned → the single-threaded GC gate stays open).
;
; `build-list n` constructs a fresh n-element nested S-expression and `sum-list`
; folds it back to a scalar. Each `(loop k)` iteration builds + sums a 200-deep
; list (≈hundreds of transient SExpr allocations) and recurses. Across thousands
; of iterations this allocates millions of transient nodes whose live set stays
; bounded (only the current iteration's list is reachable), so a working
; collector keeps committed bytes bounded; without collection the index arena
; grows monotonically.

; Build a depth-n nested Peano wrapper (s (s ... z)) — pure SExpr allocation.
(= (wrap 0) z)
(= (wrap $n) (s (wrap (- $n 1))))

; Fold the wrapper back to a scalar (forces a full traversal of the structure).
(= (unwrap z) 0)
(= (unwrap (s $r)) (+ 1 (unwrap $r)))

; Drive `iters` iterations, each building+unwrapping a depth-`depth` wrapper.
(= (loop 0 $depth) done)
(= (loop $k $depth)
   (let $w (unwrap (wrap $depth))
        (if (== $w $depth)
            (loop (- $k 1) $depth)
            (Error mismatch))))

; 8000 iterations × depth 200 ≈ 1.6M transient (s ...) cells + the +/- machinery.
!(loop 8000 200)
