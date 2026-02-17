; Circular import test: B imports A
!(import! &self "cycle_a.metta")
(= (from-b) 99)
