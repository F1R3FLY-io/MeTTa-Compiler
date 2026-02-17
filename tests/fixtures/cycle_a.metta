; Circular import test: A imports B
!(import! &self "cycle_b.metta")
(= (from-a) 42)
