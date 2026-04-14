;; PeTTa lib_import compatibility placeholder.
;;
;; PeTTa's lib_import.metta registers Prolog primitives via
;; `import_prolog_function`. MeTTaTron provides all of these primitives
;; as native built-ins (in src/backend/eval/list_ops/ops.rs,
;; src/backend/eval/set_ops.rs, src/backend/eval/testing_ops.rs, and
;; src/backend/eval/step/sexpr.rs):
;;
;;   - is-member, append, length, exclude-item, msort, foldl-atom (3-arg
;;     PeTTa form), cut — grounded ops in list_ops/ops.rs
;;   - alpha-unique-atom (alpha-equivalence dedup) — set_ops.rs
;;   - test (PeTTa-style assertion) — testing_ops.rs
;;   - progn (sequential evaluation) and reduce (force evaluation) —
;;     desugared to (let $_ a b) and (eval x) in step/sexpr.rs, so they
;;     are true built-ins available without any import
;;
;; This file exists so that `!(import! &self (library lib_import))`
;; from PeTTa-style example code resolves successfully without
;; introducing any actual definitions that would shadow MeTTaTron's
;; native implementations.
;;
;; PLN-specific overrides (=> and ?) are defined in the PLN example
;; files themselves (e.g., Direct.metta) rather than here, to avoid
;; polluting the namespace for other PLN examples.
