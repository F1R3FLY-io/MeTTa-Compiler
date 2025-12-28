;; Metamath Proof Verification for demo0.mm
;; Adapted for MeTTaTron from mmverify.py
;;
;; This script verifies the proof of theorem th1 (t = t) from the demo0.mm
;; Metamath database, which is the introductory example from Chapter 2 of
;; the Metamath book.

;; Initialize spaces and state
!(bind! &stack (new-space))
!(bind! &kb (new-space))
!(bind! &sp (new-state -1))

;; NOTE: mmverify-utils.metta is inlined before this file for benchmarking

;; ============================================================================
;; demo0.mm Database Contents
;; ============================================================================

;; Declare constants: 0 + = -> ( ) term wff |-
!(add_c "0")
!(add_c "+")
!(add_c "=")
!(add_c "->")
!(add_c "(")
!(add_c ")")
!(add_c "term")
!(add_c "wff")
!(add_c "|-")

;; Declare metavariables: t r s P Q (at frame depth 1)
!(add_v "t" 1)
!(add_v "r" 1)
!(add_v "s" 1)
!(add_v "P" 1)
!(add_v "Q" 1)

;; Floating hypotheses (type declarations for metavariables)
!(add_f "tt" "term" "t" 1)
!(add_f "tr" "term" "r" 1)
!(add_f "ts" "term" "s" 1)
!(add_f "wp" "wff" "P" 1)
!(add_f "wq" "wff" "Q" 1)

;; Axiom: tze - term 0 (0 is a term)
!(add_a "tze" ("term" "0"))

;; Axiom: tpl - term ( t + r ) (sum of terms is a term)
!(add_a "tpl" ("term" "(" "t" "+" "r" ")"))

;; Axiom: weq - wff t = r (equality of terms is a wff)
!(add_a "weq" ("wff" "t" "=" "r"))

;; Axiom: wim - wff ( P -> Q ) (implication is a wff)
!(add_a "wim" ("wff" "(" "P" "->" "Q" ")"))

;; Axiom a1: |- ( t = r -> ( t = s -> r = s ) )
;; Transitivity of equality
!(add_a "a1" ("|-" "(" "t" "=" "r" "->" "(" "t" "=" "s" "->" "r" "=" "s" ")" ")"))

;; Axiom a2: |- ( t + 0 ) = t
;; Right identity of addition
!(add_a "a2" ("|-" "(" "t" "+" "0" ")" "=" "t"))

;; Essential hypotheses for modus ponens (at frame depth 2)
!(add_e "min" ("|-" "P") 2)
!(add_e "maj" ("|-" "(" "P" "->" "Q" ")") 2)

;; Axiom mp: |- Q (modus ponens inference rule)
!(add_a "mp" ("|-" "Q"))

;; Clean up frame depth 2 after modus ponens definition
!(remove-pattern &kb (EList (FSDepth 2) $elist))
!(remove-pattern &kb (FList (FSDepth 2) $flist))
!(remove-pattern &kb ($1 $2 (FSDepth 2) $Data))

;; ============================================================================
;; Theorem th1: |- t = t (reflexivity of equality)
;; ============================================================================
;;
;; The proof uses modus ponens twice with axioms a1 and a2:
;; 1. From a2: |- (t + 0) = t
;; 2. From a1 with substitutions: |- ((t + 0) = t -> ((t + 0) = t -> t = t))
;; 3. Modus ponens gives: |- ((t + 0) = t -> t = t)
;; 4. Modus ponens again with a2 gives: |- t = t

!(add_p "th1"
  ("|-" "t" "=" "t")
  ("tt" "tze" "tpl" "tt" "weq" "tt" "tt" "weq" "tt" "a2"
   "tt" "tze" "tpl" "tt" "weq" "tt" "tze" "tpl" "tt" "weq"
   "tt" "tt" "weq" "wim" "tt" "a2" "tt" "tze" "tpl" "tt" "tt" "a1"
   "mp" "mp")
  True)

;; Clean up frame depth 1 after theorem
!(remove-pattern &kb (EList (FSDepth 1) $elist))
!(remove-pattern &kb (FList (FSDepth 1) $flist))
!(remove-pattern &kb ($1 $2 (FSDepth 1) $Data))
