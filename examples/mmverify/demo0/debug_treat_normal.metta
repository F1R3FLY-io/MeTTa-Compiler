;; Debug test for treat_normal_proof
!(bind! &stack (new-space))
!(bind! &kb (new-space))
!(bind! &sp (new-state -1))

(include "../mmverify-utils.metta")

;; Minimal setup
!(add_c "term")
!(add_v "t" 1)
!(add_f "tt" "term" "t" 1)

!(println! "=== Initial state ===")
!(println! "")
!(println! "FHyps in &kb:")
!(println! (matchc &kb ((Label $l) FHyp $d $data) ($l $d $data)))

!(println! "")
!(println! "=== Manually adding ActiveHyps (what treat_normal_proof should do) ===")
!(println! "Adding ActiveHyp for all FHyps:")
!(let $result (matchc &kb ((Label $l) FHyp $d $data) (add-atom &kb (ActiveHyp $l)))
  (println! ("matchc result:" $result)))

!(println! "")
!(println! "ActiveHyps after manual add:")
!(println! (matchc &kb (ActiveHyp $l) (ActiveHyp $l)))

!(println! "")
!(println! "=== Direct call to treat_hypothesis ===")
!(let $stack_len (case (matchc &stack ( (Num $n) $s ) $n) ( ( () 0 ) ( $nums (+ 1 (max-atom $nums)))))
  (let ()
    (println! ("stack_len:" $stack_len))
    (println! ("Calling treat_hypothesis tt FHyp ..."))
    (treat_hypothesis "tt" FHyp ((Typecode term) (FVar t) (Type "$f")) $stack_len)))

!(println! "")
!(println! "Stack after direct treat_hypothesis:")
!(println! (matchc &stack $x $x))

!(println! "")
!(println! "=== Calling treat_normal_proof with (tt) ===")
!(treat_normal_proof ("tt"))

!(println! "")
!(println! "=== After treat_normal_proof ===")

!(println! "Stack contents:")
!(println! (matchc &stack $x $x))

!(println! "")
!(println! "ActiveHyps after proof (should be removed):")
!(println! (matchc &kb (ActiveHyp $l) (ActiveHyp $l)))
