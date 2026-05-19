; MTT stdlib helpers ported verbatim from HE stdlib.metta to close
; bisim-spec gaps (Phase 5 of bisim-closure plan, 2026-05-19).
;
; Source: hyperon-experimental/lib/src/metta/runner/stdlib/stdlib.metta
; (HE commit 3f76dc46, lines 285-329, 691-698, 966-967).
;
; Per superset invariant: these helpers add to MTT — they do not replace
; or shadow any existing MTT rule or grounded op. Shadowing audit was
; performed pre-port; all 5 helpers in this file are NO_CONFLICT.
;
; The HE `assert` rule (stdlib.metta:700-704) is GROUNDED-DIVERGENT and
; is NOT ported here — MTT's grounded `assert` (bytecode/native_registry.rs:348)
; is preserved; the HE rule could be added separately as `he-assert` if
; Phase 5+ work needs it.

; ----- if-decons-expr (HE stdlib.metta:293-298) -----
(: if-decons-expr (-> Expression Variable Variable Atom Atom %Undefined%))
(= (if-decons-expr $atom $head $tail $then $else)
   (function (eval (if-equal $atom ()
     (return $else)
     (chain (decons-atom $atom) $list
       (unify $list ($head $tail) (return $then) (return $else)))))))

; ----- if-error (HE stdlib.metta:307-317) — dependency of return-on-error -----
(: if-error (-> Atom Atom Atom %Undefined%))
(= (if-error $atom $then $else)
   (function (chain (eval (get-metatype $atom)) $meta
     (eval (if-equal $meta Expression
       (eval (if-equal $atom ()
         (return $else)
         (chain (decons-atom $atom) $list
           (unify $list ($head $tail)
             (eval (if-equal $head Error (return $then) (return $else)))
             (return $else)))))
       (return $else))))))

; ----- return-on-error (HE stdlib.metta:325-329) -----
(: return-on-error (-> Atom Atom %Undefined%))
(= (return-on-error $atom $then)
   (function (eval (if-equal $atom Empty (return (return Empty))
     (eval (if-error $atom (return (return $atom))
       (return $then)))))))

; ----- assertIncludes (HE stdlib.metta:691-698) -----
(: assertIncludes (-> Atom Expression (->)))
(= (assertIncludes $atom $content)
   (let $eval_atom (collapse $atom)
     (let $diff (subtraction-atom $content $eval_atom)
       (if (== $diff ())
           ()
           (Error (assertIncludes $atom $content)
             (assertIncludes error: $diff not included in result: $eval_atom))))))

; ----- noreduce-eq (HE stdlib.metta:966-967) -----
(: noreduce-eq (-> Atom Atom Bool))
(= (noreduce-eq $a $b) (== (quote $a) (quote $b)))
