; Test file for backward chaining (Phase 9: mmverify compatibility)
; Backward chaining is implemented in pure MeTTa using pattern matching

; Define a simple knowledge base with facts and rules
(= (human Socrates) True)
(= (human Plato) True)
(= (philosopher Socrates) True)
(= (philosopher Plato) True)

; Rule: All philosophers are wise
(= (wise $x) (if (philosopher $x) True False))

; Rule: All humans are mortal
(= (mortal $x) (if (human $x) True False))

; Simple backward chaining: query if Socrates is wise
!(wise Socrates)

; Query if Plato is mortal
!(mortal Plato)

; More complex chaining with nested rules
(= (greek-philosopher $x)
   (if (philosopher $x) True False))

!(greek-philosopher Socrates)

; Test pattern matching for knowledge base lookup
(= (knows $x $y) (match &self (teacher $x $y) True))

; Add some teacher facts
(= (teacher Socrates Plato) True)

; This would need pattern matching on the current space
; For now, direct evaluation works
!(if (teacher Socrates Plato) "Socrates taught Plato" "Unknown")
