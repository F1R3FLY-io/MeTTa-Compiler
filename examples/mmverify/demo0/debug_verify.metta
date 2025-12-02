;; Debug test for verify function
;; This file traces through the verify function to find where the let* chain breaks

;; Initialize spaces and state
!(bind! &stack (new-space))
!(bind! &kb (new-space))
!(bind! &sp (new-state -1))

;; Include the mmverify utilities
(include "../mmverify-utils.metta")

;; ============================================================================
;; Setup same as verify_demo0.metta
;; ============================================================================

;; Declare constants
!(add_c "0")
!(add_c "+")
!(add_c "=")
!(add_c "->")
!(add_c "(")
!(add_c ")")
!(add_c "term")
!(add_c "wff")
!(add_c "|-")

;; Declare metavariables (at frame depth 1)
!(add_v "t" 1)
!(add_v "r" 1)
!(add_v "s" 1)
!(add_v "P" 1)
!(add_v "Q" 1)

;; Floating hypotheses
!(add_f "tt" "term" "t" 1)
!(add_f "tr" "term" "r" 1)
!(add_f "ts" "term" "s" 1)
!(add_f "wp" "wff" "P" 1)
!(add_f "wq" "wff" "Q" 1)

;; Axioms
!(add_a "tze" ("term" "0"))
!(add_a "tpl" ("term" "(" "t" "+" "r" ")"))
!(add_a "weq" ("wff" "t" "=" "r"))
!(add_a "wim" ("wff" "(" "P" "->" "Q" ")"))
!(add_a "a1" ("|-" "(" "t" "=" "r" "->" "(" "t" "=" "s" "->" "r" "=" "s" ")" ")"))
!(add_a "a2" ("|-" "(" "t" "+" "0" ")" "=" "t"))

;; Essential hypotheses for modus ponens
!(add_e "min" ("|-" "P") 2)
!(add_e "maj" ("|-" "(" "P" "->" "Q" ")") 2)

;; MP axiom
!(add_a "mp" ("|-" "Q"))

;; Clean up frame depth 2
!(remove-pattern &kb (EList (FSDepth 2) $elist))
!(remove-pattern &kb (FList (FSDepth 2) $flist))
!(remove-pattern &kb ($1 $2 (FSDepth 2) $Data))

;; ============================================================================
;; Debug verify - step by step tracing
;; ============================================================================

;; The proof sequence for th1
!(println! "=== Starting debug verification ===")

;; Step 1: Run treat_normal_proof
!(println! "Step 1: Running treat_normal_proof...")
!(let $_0 (treat_normal_proof ("tt" "tze" "tpl" "tt" "weq" "tt" "tt" "weq" "tt" "a2"
   "tt" "tze" "tpl" "tt" "weq" "tt" "tze" "tpl" "tt" "weq"
   "tt" "tt" "weq" "wim" "tt" "a2" "tt" "tze" "tpl" "tt" "tt" "a1"
   "mp" "mp"))
  (println! ("treat_normal_proof completed. Result:" $_0)))

;; Step 2: Check stack contents with matchc
!(println! "")
!(println! "Step 2: Checking stack with matchc...")
!(let $all_stack (matchc &stack $x $x)
  (println! ("All stack atoms:" $all_stack)))

!(let $stack_expr (matchc &stack ( (Num $n) $f) $f)
  (println! ("matchc result (stack_expr):" $stack_expr)))

;; Step 3: Check size-atom behavior
!(println! "")
!(println! "Step 3: Checking size-atom...")
!(let $stack_expr (matchc &stack ( (Num $n) $f) $f)
  (let $size_result (size-atom $stack_expr)
    (println! ("size-atom result:" $size_result))))

;; Step 4: Check empty comparison
!(println! "")
!(println! "Step 4: Checking empty comparison...")
!(let $stack_expr (matchc &stack ( (Num $n) $f) $f)
  (let $empty_check (== () $stack_expr)
    (println! ("(== () stack_expr):" $empty_check))))

;; Step 5: Check car-atom
!(println! "")
!(println! "Step 5: Checking car-atom...")
!(let $stack_expr (matchc &stack ( (Num $n) $f) $f)
  (let $stack_top (car-atom $stack_expr)
    (println! ("car-atom result:" $stack_top))))

;; Step 6: Full verify function - will it print "Correct proof!"?
!(println! "")
!(println! "=== Running full verify function ===")
!(verify ("tt" "tze" "tpl" "tt" "weq" "tt" "tt" "weq" "tt" "a2"
   "tt" "tze" "tpl" "tt" "weq" "tt" "tze" "tpl" "tt" "weq"
   "tt" "tt" "weq" "wim" "tt" "a2" "tt" "tze" "tpl" "tt" "tt" "a1"
   "mp" "mp") ("|-" "t" "=" "t"))

!(println! "=== Verification complete ===")
