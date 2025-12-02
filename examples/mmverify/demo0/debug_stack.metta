;; Focused debug test for stack operations
;; Tests just a few proof steps to see if stack is being populated

!(bind! &stack (new-space))
!(bind! &kb (new-space))
!(bind! &sp (new-state -1))

(include "../mmverify-utils.metta")

;; Minimal setup - DON'T add "t" as constant! It's a variable
!(add_c "term")
!(println! "After add_c term:")
!(println! (matchc &kb (Constant "term" $x) (Constant "term" $x)))

!(add_v "t" 1)
!(println! "After add_v t:")
!(println! (matchc &kb (Var "t" $x $y) (Var "t" $x $y)))

!(add_f "tt" "term" "t" 1)
!(println! "After add_f tt:")
!(println! (matchc &kb ((Label "tt") $type $depth $data) ((Label "tt") $type $depth $data)))

!(println! "")
!(println! "=== All atoms in &kb ===")
!(println! (matchc &kb $x $x))

!(println! "")
!(println! "=== Testing stack operations ===")

;; Check initial stack
!(println! "Initial stack:")
!(println! (matchc &stack $x $x))

;; Check if tt FHyp is in knowledge base (3-element vs 4-element pattern)
!(println! "")
!(println! "Checking &kb for tt FHyp (3-element pattern - wrong):")
!(println! (matchc &kb ((Label "tt") $type $data) ($type $data)))

!(println! "")
!(println! "Checking &kb for tt FHyp (4-element pattern - correct):")
!(println! (matchc &kb ((Label "tt") $type $depth $data) ($type $depth $data)))

;; Check if tt is an ActiveHyp
!(println! "")
!(println! "Checking for ActiveHyp tt:")
!(println! (matchc &kb (ActiveHyp "tt") (ActiveHyp "tt")))

;; Test treat_step for tt
!(println! "")
!(println! "=== Calling treat_step for tt ===")
!(treat_step "tt")

!(println! "")
!(println! "Stack after treat_step tt:")
!(let $stack_contents (matchc &stack $x $x)
  (println! $stack_contents))

;; Direct add-atom test
!(println! "")
!(println! "=== Direct add-atom test ===")
!(add-atom &stack (test_atom 42))
!(println! "After direct add-atom:")
!(println! (matchc &stack $x $x))
