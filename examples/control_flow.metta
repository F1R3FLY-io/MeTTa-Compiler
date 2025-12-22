; Test file for control flow operations (Phase 2: mmverify compatibility)

; Test let* - sequential bindings
!(assertEqual (let* (($x 1) ($y 2)) (+ $x $y)) 3)
!(assertEqual (let* (($x 1) ($y (+ $x 1))) $y) 2)
!(assertEqual (let* (($a 10) ($b (* $a 2)) ($c (+ $b $a))) $c) 30)

; Test let* with empty bindings
!(assertEqual (let* () 42) 42)

; Test let* with single binding
!(assertEqual (let* (($x 5)) (* $x $x)) 25)

; Test unify - successful unification with variables
!(assertEqual (unify $x 5 $x fail) 5)
!(assertEqual (unify (pair $a $b) (pair 1 2) (+ $a $b) fail) 3)

; Test unify - failed unification
!(assertEqual (unify (foo $x) (bar $y) success fail) fail)
!(assertEqual (unify 1 2 success fail) fail)

; Test unify - structural matching
!(assertEqual (unify (triple $x $y $z) (triple a b c) $x fail) a)
!(assertEqual (unify (triple $x $y $z) (triple a b c) (+ 1 2) fail) 3)
