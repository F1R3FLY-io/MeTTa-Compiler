;; Test variable substitution in match templates
!(bind! &test (new-space))
!(bind! &results (new-space))
!(bind! &kb (new-space))

;; Define matchc like mmverify
(= (matchc $space $pattern $query) (collapse (match $space $pattern $query)))

;; Add test atom
!(add-atom &test (Foo bar baz))
!(println! "Test space contents:")
!(println! (matchc &test $x $x))

;; Test 1: Simple variable in template
!(println! "")
!(println! "Test 1: (match &test (Foo $a $b) (Got $a and $b))")
!(println! (match &test (Foo $a $b) (Got $a and $b)))

;; Test 2: Use variable in add-atom template
!(println! "")
!(println! "Test 2: (match &test (Foo $a $b) (add-atom &results (Result $a)))")
!(match &test (Foo $a $b) (add-atom &results (Result $a)))
!(println! "Results space after Test 2:")
!(println! (matchc &results $x $x))

;; Test 3: matchc version
!(println! "")
!(println! "Test 3: (matchc &test (Foo $a $b) (Captured $a))")
!(println! (matchc &test (Foo $a $b) (Captured $a)))

;; Test 4: collapse version (should be equivalent to matchc)
!(println! "")
!(println! "Test 4: (collapse (match &test (Foo $a $b) (Captured $a)))")
!(println! (collapse (match &test (Foo $a $b) (Captured $a))))

;; Test 5: mmverify-style pattern - add to &kb based on match
!(println! "")
!(println! "Test 5: mmverify-style - add ActiveHyp based on matched Label")
!(add-atom &kb ((Label "tt") FHyp (FSDepth 1) SomeData))
!(println! "Before matchc - &kb contents:")
!(println! (matchc &kb $x $x))
!(let $_result (matchc &kb ((Label $l) FHyp $d $data) (add-atom &kb (ActiveHyp $l)))
  (println! ("matchc with add-atom returned:" $_result)))
!(println! "After matchc - &kb contents:")
!(println! (matchc &kb $x $x))

;; Test 6: Direct test of add-atom inside match template
!(println! "")
!(println! "Test 6: Direct match with add-atom")
!(add-atom &test (Item one))
!(match &test (Item $x) (add-atom &results (Saved $x)))
!(println! "Results space:")
!(println! (matchc &results $x $x))
