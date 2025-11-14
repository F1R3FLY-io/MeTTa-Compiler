; Create a tree structure for pattern matching
(tree (node A (leaf 1) (leaf 2)))
(tree (node B (node C (leaf 3) (leaf 4)) (leaf 5)))
(tree (node D (leaf 6) (node E (leaf 7) (leaf 8))))
(tree (node F (node G (leaf 9) (leaf 10)) (node H (leaf 11) (leaf 12))))

; Complex pattern matching with multiple variables
(= (find-leaves $tree)
    (match &self (tree $tree)
      (collect-leaves $tree ())))

(= (collect-leaves (leaf $n) $acc)
    (cons $n $acc))
(= (collect-leaves (node $name $left $right) $acc)
    (collect-leaves $left (collect-leaves $right $acc)))

; Pattern matching with constraints
(= (find-subtrees $pattern)
    (match &self (tree $tree)
      (match-pattern $tree $pattern)))

(= (match-pattern $tree $pattern)
    (unify-trees $tree $pattern))

(= (unify-trees (node $n $l $r) (node $n $_ $_))
    (subtree-found $n))
(= (unify-trees (node $n $l $r) $pattern)
    (match-pattern $l $pattern))
(= (unify-trees (node $n $l $r) $pattern)
    (match-pattern $r $pattern))
(= (unify-trees (leaf $_) $pattern)
    (empty))

; Stress the pattern matcher
!(find-leaves (node A (leaf 1) (leaf 2)))
!(find-subtrees (node C $x $y))
!(find-subtrees (node $name (leaf $_) (leaf $_)))
!(find-leaves (node F (node G (leaf 9) (leaf 10)) (node H (leaf 11) (leaf 12))))
!(find-subtrees (node $n $l $r))