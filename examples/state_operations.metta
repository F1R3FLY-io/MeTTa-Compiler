; Test file for state operations (Phase 4: mmverify compatibility)

; Test new-state - create a new state cell
!(let $s (new-state 42) $s)

; Test get-state - retrieve value from state cell
!(let $s (new-state 42) (get-state $s))

; Test change-state! - modify state cell value
!(let* (($s (new-state 10))
        (_ (change-state! $s 20)))
    (get-state $s))

; Test state with complex value
!(let $s (new-state (Person Alice 25)) (get-state $s))

; Test bind! - bind a symbol to a value
; Note: using myval without & due to parser treating & as separate token
!(let* ((_ (bind! myval 123)))
    myval)

; Test state mutation sequence
!(let* (($counter (new-state 0))
        (_ (change-state! $counter 1))
        (_ (change-state! $counter 2))
        (_ (change-state! $counter 3)))
    (get-state $counter))

; Test get-state returns the actual value, not the state reference
!(let $s (new-state (list 1 2 3)) (get-state $s))
