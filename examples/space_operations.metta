; Test file for space operations (Phase 3: mmverify compatibility)

; Test new-space - create a new space
!(let* (($kb (new-space))) $kb)

; Test add-atom - add atoms to space
!(let* (($kb (new-space)))
    (let* ((_ (add-atom $kb (Person Alice)))
           (_ (add-atom $kb (Person Bob))))
        (collapse $kb)))

; Test remove-atom - remove atom from space
!(let* (($kb (new-space)))
    (let* ((_ (add-atom $kb (item1)))
           (_ (add-atom $kb (item2)))
           (_ (add-atom $kb (item3)))
           (_ (remove-atom $kb (item2))))
        (collapse $kb)))

; Test collapse on empty space
!(let* (($kb (new-space))) (collapse $kb))
