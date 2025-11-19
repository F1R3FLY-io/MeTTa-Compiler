; Test the new list operations

; Test map-atom
!(map-atom (1 2 3 4) $v (+ $v 1))

; Test filter-atom  
!(filter-atom (1 2 3 4 5 6) $v (> $v 3))

; Test foldl-atom
!(foldl-atom (1 2 3 4) 0 $acc $x (+ $acc $x))

; Test composition
!(foldl-atom (filter-atom (map-atom (1 2 3 4 5) $v (* $v 2)) $v (> $v 4)) 0 $acc $x (+ $acc $x))