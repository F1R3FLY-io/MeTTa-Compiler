; Create separate knowledge spaces
!(bind! &facts (new-space))
!(bind! &rules (new-space))

; Load facts into dedicated space - using ASCII arrows
!(add-atom &facts (implies A B))
!(add-atom &facts (implies B C))
!(add-atom &facts (implies C D))
!(add-atom &facts A)
!(add-atom &facts (implies D E))

; Load inference rules into rule space
!(add-atom &rules (rule (implies $p $q) $p $q))
!(add-atom &rules (rule (implies $p $q) (implies $q $r) (implies $p $r)))

; Simple forward chainer - less complex than backward chaining
(= (can-prove $goal)
    (match &facts $goal True))

(= (can-prove $goal)
    (match &facts (implies $premise $goal)
      (match &facts $premise True)))

(= (can-prove $goal)
    (match &facts (implies $p1 $p2)
      (match &facts (implies $p2 $goal)
          (can-prove $p1))))

; Test reasoning chains
!(can-prove A)
!(can-prove B)
!(can-prove C)
!(can-prove D)
!(can-prove E)