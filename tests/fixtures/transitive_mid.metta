; Middle module: imports base, defines own function using base
!(import! &self "transitive_base.metta")
(= (mid-add $x) (+ (base-add $x) 50))
