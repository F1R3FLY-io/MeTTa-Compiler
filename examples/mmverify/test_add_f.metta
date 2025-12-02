; Test mmverify add_f function in isolation

; Load mmverify-utils
!(import! &self mmverify-utils)

; Create fresh kb
!(bind! &kb (new-space))

; Step 1: Add constant "term"
!(println! "=== Step 1: add_c term ===")
!(add_c "term")

; Check kb
!(println! "=== KB after add_c ===")
!(get-atoms &kb)

; Step 2: Add variable "t" 
!(println! "=== Step 2: add_v t 1 ===")
!(add_v "t" 1)

; Check kb
!(println! "=== KB after add_v ===")
!(get-atoms &kb)

; Step 3: Add floating hypothesis
!(println! "=== Step 3: add_f tt term t 1 ===")
!(add_f "tt" "term" "t" 1)

; Check kb
!(println! "=== KB after add_f ===")
!(get-atoms &kb)
