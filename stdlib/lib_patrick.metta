;; lib_patrick — pure-MeTTa shim for MeTTaTron compatibility.
;;
;; Mirrors PeTTa's `<PeTTa>/lib/lib_patrick.metta` but without the
;; PeTTa-specific `add-translator-rule!` line for `for`. The function
;; bodies are pure MeTTa and work directly under MeTTaTron's evaluator.
;;
;; Used by `examples/RavenInduction.metta` (and any other PeTTa-style
;; code that imports `(library lib_patrick)`).

;; General function composition
;;
;; Usage: `(compose (cons f g h) (a b))` -> `(f (g (h a b)))`
(: compose (-> Expression Atom Atom))
(= (compose (cons $f $rest) $args)
   (if (== $rest ())
       (if (== (length $args) 1)
           ;; Single arg: avoid `reduce` for direct function call
           (let ($arg) $args ($f $arg))
           ;; Multi arg: reduce a constructed call expression
           (let $func (cons $f $args) (reduce $func)))
       ($f (compose $rest $args))))

;; Reverse function matching: `(@ $a $b)` binds $a to $b and returns $a
(= (@ $a $b) (let $a $b $a))

;; For loop. PeTTa originally registered this as a translator rule via
;; `add-translator-rule!`, which is a PeTTa-specific extension that
;; expands the form at translation time. MeTTaTron has no equivalent,
;; so the function definition runs at evaluation time instead.
(: for (-> Expression Atom Expression Expression))
(= (for $var $collection $body)
   (quote (let $var (superpose $collection) $body)))

;; Iteration with function: call ($step $i $state) for i = 0 .. n-1
(= (iterate $i $n $state $step)
   (if (== $n 0)
       $state
       (iterate (+ $i 1)
                (- $n 1)
                ($step $i $state)
                $step)))
