; Test module for export functionality
; Demonstrates symbol export control

; Export these symbols
!(export! public-fn)
!(export! PublicType)

; Public function (exported)
(= (public-fn $x)
   (+ $x 1))

; Public type (exported)
(: PublicType Type)

; Private function (not exported)
(= (private-fn $x)
   (- $x 1))

; Private type (not exported)
(: PrivateType Type)

; Helper function (not exported)
(= (internal-helper $x)
   (* $x 2))
