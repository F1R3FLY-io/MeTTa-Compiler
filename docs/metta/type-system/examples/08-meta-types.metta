; Example: Meta-Types and Evaluation
; Demonstrates how Atom meta-type prevents evaluation

; Normal function (evaluates argument)
(: double-eval (-> Number Number))
(= (double-eval $x) (* $x 2))

!(double-eval (+ 1 2))  ; → 6
; Argument (+ 1 2) is evaluated to 3, then passed

; Meta-programming function (doesn't evaluate argument)
(: quote (-> Atom Atom))
(= (quote $expr) $expr)

!(quote (+ 1 2))  ; → (+ 1 2)
; Argument (+ 1 2) is NOT evaluated, passed as-is

; Macro-like behavior
(: unless (-> Atom Atom Atom %Undefined%))
(= (unless $cond $then)
   (if (not $cond) $then ()))

; Both arguments receive unevaluated
!(unless False (print "Condition was false"))
