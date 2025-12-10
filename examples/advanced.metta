; Advanced MeTTa features

; Type assertion with arrow type
(: $func (-> Long Long Bool))

; Nested rules with pattern matching
(= (double $x) (* $x 2))
(= (quadruple $x) (double (double $x)))

; Add concrete facts to space - using &self, the only defined space
!(add-atom &self (person alice 30))
!(add-atom &self (person bob 25))
!(add-atom &self (city "Boston"))
!(add-atom &self (city "Paris"))

; Query the space to see what we added
!(match &self (person $name $age) (found $name $age))

; Remove one person
!(remove-atom &self (person alice 30))

; Verify alice is gone but bob remains
!(match &self (person $name $age) (found $name $age))

; Define transform as shorthand for match - pattern matching wrapper
(= (transform $pattern $result) (match &self $pattern $result))

; Use transform to find all cities
!(transform (city $name) (city-found $name))

; Builtin operations
(+ 10 20)
(- 100 50)
(* 5 6)
(/ 100 4)
(< 5 10)
(<= 5 5)
(> 10 5)
(>= 10 10)
(== 42 42)
