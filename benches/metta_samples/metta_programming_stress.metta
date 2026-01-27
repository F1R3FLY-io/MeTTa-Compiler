; Meta-level function generation
(= (generate-accessor $field)
    (quote (= (get $field $obj)
            (match $obj ($field $val) $val))))

(= (generate-predicate $type)
    (quote (= (is $type $obj)
            (match (get-type $obj) $type True))))

; Dynamic rule creation and evaluation
(= (create-rule $pattern $body)
    (let $rule (quote (= $pattern $body))
      (add-atom &self $rule)))

(= (create-fact $fact)
    (add-atom &self $fact))

; Code analysis and transformation
(= (analyze-expression $expr)
    (case (get-metatype $expr)
      ((Symbol (symbol-info $expr))
      (Variable (variable-info $expr))
      (Expression (analyze-compound $expr))
      (Grounded (grounded-info $expr)))))

(= (analyze-compound $expr)
    (let $head (car-atom $expr)
      (let $tail (cdr-atom $expr)
          (compound-info $head (count-args $tail) $tail))))

(= (count-args ()) 0)
(= (count-args $list)
    (if (== () $list)
        0
        (let $rest (cdr-atom $list)
          (+ 1 (count-args $rest)))))

; Template instantiation system
(= (instantiate-template $template $bindings)
    (substitute-vars $template $bindings))

; Look up a variable in bindings list
; Bindings format: (($var1 $val1) ($var2 $val2) ...)
(= (lookup-var $var ())
    $var)
(= (lookup-var $var $bindings)
    (let $first (car-atom $bindings)
      (let $rest (cdr-atom $bindings)
        (let $key (car-atom $first)
          (let $first-tail (cdr-atom $first)
            (let $val (car-atom $first-tail)
              (if (== $key $var)
                  $val
                  (lookup-var $var $rest))))))))

; Substitute variables in an expression
; Empty bindings -> return expression unchanged
(= (substitute-vars $expr ())
    $expr)
; Non-empty bindings -> dispatch based on metatype
(= (substitute-vars $expr $bindings)
    (case (get-metatype $expr)
      ((Variable (lookup-var $expr $bindings))
       (Expression (map-substitute $expr $bindings))
       ($_ $expr))))

; Map substitution over expression elements
(= (map-substitute $expr $bindings)
    (if (== () $expr)
        ()
        (let $head (car-atom $expr)
          (let $tail (cdr-atom $expr)
            (chain (substitute-vars $head $bindings) $subst-head
              (chain (map-substitute $tail $bindings) $subst-tail
                (cons-atom $subst-head $subst-tail)))))))

; Dynamic fact and rule generation
!(create-fact (person Alice))
!(create-fact (person Bob))
!(create-fact (age Alice 25))
!(create-fact (age Bob 30))

; Generate and use meta-rules
!(analyze-expression (foo $x $y))
!(analyze-expression hello)
!(analyze-expression $var)
!(instantiate-template (likes $person food) (($person Alice)))
!(instantiate-template (older $x $y) (($x Bob) ($y Alice)))