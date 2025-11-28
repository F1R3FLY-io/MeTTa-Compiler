; Test file for list operations (Phase 1: mmverify compatibility)

; Test car-atom - get first element
!(assertEqual (car-atom (a b c)) a)
!(assertEqual (car-atom (1 2 3)) 1)
!(assertEqual (car-atom ((nested) items)) (nested))

; Test cdr-atom - get rest of list
!(assertEqual (cdr-atom (a b c)) (b c))
!(assertEqual (cdr-atom (1 2 3)) (2 3))
!(assertEqual (cdr-atom (single)) ())

; Test cons-atom - prepend element
!(assertEqual (cons-atom a (b c)) (a b c))
!(assertEqual (cons-atom 1 (2 3)) (1 2 3))
!(assertEqual (cons-atom x ()) (x))

; Test decons-atom - split into (head tail)
!(assertEqual (decons-atom (a b c)) (a (b c)))
!(assertEqual (decons-atom (1 2 3)) (1 (2 3)))
!(assertEqual (decons-atom (single)) (single ()))

; Test size-atom - get list length
!(assertEqual (size-atom (a b c)) 3)
!(assertEqual (size-atom ()) 0)
!(assertEqual (size-atom (1 2 3 4 5)) 5)

; Test max-atom - get maximum number
!(assertEqual (max-atom (1 5 3)) 5)
!(assertEqual (max-atom (10 2 8 4)) 10)
!(assertEqual (max-atom (-5 -2 -10)) -2)
