; Finding 1 UAF regression oracle — collapse-bind "tracked variable" use-after-free.
;
; THE BUG (pre-fix): a `collapse-bind` whose inner expression has FREE VARIABLES
; pushes a `BindingCaptureFrame` recording those variables. Pre-fix the frame held
; laundered `&'static str` NAMES obtained from `as_atom()`, which in the index-gc
; backend borrow an arena string side-`Box`. The variable ATOM itself was NOT in the
; GC root set. Under FANOUT>0 the parent shares the tracked-var set
; (`tracked_vars_hint`, an `Arc<SmallVec<..>>`) cross-thread into branch workers
; (`WorkerCaptureScope`). A rendezvous MAJOR (forced here by a small
; `METTATRON_INDEX_GC_MAX_BYTES`) running while the workers are PARKED can sweep /
; release the young segment that owns those arena strings, dropping the `Box`. A
; worker then dereferences the dangling `&str` at the projection / sidecar-encoding
; site (`project_*_for_consumer` reads each tracked-var NAME to build the
; `(value (Bindings ($var val) …))` sidecar) → heap-use-after-free / silent
; wrong-projection.
;
; THE FIX: capture the variable ATOM `MettaValue` handle (rootable) instead of the
; `&str`, and root the binding-capture stack via `collect_binding_capture_roots` ⟶
; `collect_global_anchors`. The segment is then retained while any capture frame is
; live, so the laundered `&str` stays valid.
;
; WHY THIS FIXTURE TRIGGERS IT:
;   • `collapse-bind (query $who)` has the FREE VARIABLE `$who`. `(query $who)`
;     resolves to `(father $who b)`, which matches FOUR `father` facts, binding `$who`
;     to a1/a2/a3/a4 per branch. ⇒ the BindingCaptureFrame records `$who`, and the
;     collapse fans out 4 ways; FANOUT>0 spawns workers that re-push `tracked_vars_hint`.
;   • The per-branch result KEEPS a non-trivial tracked-var binding (`{ $who <- a1 }`,
;     …) — so the projection sites genuinely READ the laundered `$who` name on a worker
;     (the exact pre-fix UAF dereference).
;   • Each matched `father` fact's body churns a transient PEANO chain (`(mk N)` ⟶
;     `(grind N)`), allocating young arena nodes that die immediately ⇒ the young
;     segment co-resident with the `$who` atom becomes dead and a small-budget
;     rendezvous MAJOR sweeps it mid-flight while the fan-out workers are parked.
;   • `$who` is RE-DERIVED fresh every outer `(probe)` (a fresh match against `father`),
;     NOT hoisted to a long-lived root — the only thing keeping its atom alive across a
;     parked-worker major is the (now-rooted) binding-capture stack.
;   • `(probe)` is DETERMINISTIC (collapse-bind collapses the 4-way fan-out into ONE
;     tuple) so the `(drive)` loop does NOT branch exponentially; it stays tail-recursive
;     at bounded depth and terminates quickly even under ASAN.
;
; EXPECTED:  pre-fix → ASAN heap-use-after-free (or a wrong / empty `$who` projection);
;            post-fix → 0 UAF, deterministic correct `{ $who <- aN }` projection.
;
; STANDALONE ASAN RUN (index-gc ASAN binary; committed-bytes > MAX_BYTES forces a
; MAJOR, FANOUT=8 spawns the workers):
;   METTATRON_PARALLEL_FANOUT_DEPTH=8 METTATRON_INDEX_GC_MAX_BYTES=65536 \
;   METTATRON_INDEX_GC_REPORT=2 \
;   ASAN_OPTIONS=detect_leaks=0:abort_on_error=1:halt_on_error=1 \
;     ./mettatron --gc index examples/cesk-gc/collapse_bind_tracked_uaf.metta
; Non-vacuity: expect '[index_gc] rendezvous satb-major cycle' > 0 (the FANOUT
; collector swept while workers were live) and a final 'done'.

; --- transient young-churn building block: a peano chain built then folded, leaving
;     immediately-dead young arena nodes per call. ---
(= (grind 0) z)
(= (grind $n) (s (grind (- $n 1))))
(= (ungrind z) 0)
(= (ungrind (s $r)) (+ 1 (ungrind $r)))

; `(mk $n)` forces a build+fold of an $n-deep peano chain (transient young churn) and
; returns the ground marker `ok` — so each matched `father` branch allocates a dead
; young segment WHILE the `$who` capture atom is live.
(= (mk $n) (if (== (ungrind (grind $n)) $n) ok (Error churn-mismatch $n)))

; --- the multi-clause relation the collapse-bind queries. FOUR `father` facts ⇒ the
;     `collapse-bind (query $who)` fans out 4 ways, each binding `$who` to a distinct
;     atom (a1..a4) that is RE-DERIVED fresh per probe. Each body churns young memory. ---
(= (father a1 b) (mk 400))
(= (father a2 b) (mk 400))
(= (father a3 b) (mk 400))
(= (father a4 b) (mk 400))

; `(query $who)` keeps `$who` FREE in the collapse-bind inner expression; rule
; resolution of `(father $who b)` binds it per branch (the tracked-var binding the
; sidecar projection reads on a worker).
(= (query $who) (father $who b))

; DETERMINISTIC probe: the collapse-bind collapses the 4-way fan-out into one tuple, so
; the driver does not branch. `$who` is a fresh atom each call.
(= (probe) (collapse-bind (query $who)))

; --- driver: tail-recursive, bounded depth, deterministic body. Each iteration pushes a
;     fresh `$who` capture frame + fans out (workers re-push `tracked_vars_hint`) + churns
;     enough young memory that committed crosses MAX_BYTES and a rendezvous MAJOR sweeps a
;     young segment while the workers are parked. The collapse tuple `$r` is fully
;     materialized (the launder that would dereference a dangling tracked `$who` `&str`
;     if its atom were wrongly reclaimed), then dropped. ---
(= (drive 0) done)
(= (drive $i) (let $r (probe) (drive (- $i 1))))

; 60 outer iterations: finishes well under the ASAN timeout (~1s release) yet fires a
; dozen rendezvous majors over young segments while the fan-out workers are parked.
!(drive 60)
