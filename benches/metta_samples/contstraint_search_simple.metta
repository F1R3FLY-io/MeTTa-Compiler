; Smaller choice space to avoid explosion
(= (small-digit) 1)
(= (small-digit) 2)
(= (small-digit) 3)

; Simplified constraint checking
(= (not-equal $x $y) (if (== $x $y) (empty) True))

; Simple 2-variable constraint problem
(= (solve-pair)
    (let $x (small-digit)
      (let $y (small-digit)
          (if (not-equal $x $y)
              (pair $x $y)
              (empty)))))

; Simple 3-variable with early pruning
(= (solve-triple)
    (let $x (small-digit)
      (if (== $x 1)
          (let $y (small-digit)
            (if (not-equal $x $y)
                (let $z (small-digit)
                  (if (and (not-equal $x $z) (not-equal $y $z))
                      (triple $x $y $z)
                      (empty)))
                (empty)))
          (let $y (small-digit)
            (if (not-equal $x $y)
                (triple $x $y 1)
                (empty))))))

; Test the constraint solver
!(solve-pair)
!(solve-triple)
!(solve-pair)