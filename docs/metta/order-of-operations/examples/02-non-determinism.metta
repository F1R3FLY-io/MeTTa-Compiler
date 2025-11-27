; Example: Non-Deterministic Evaluation
; Demonstrates that all matching rules are explored simultaneously

; Define multiple rules for the same function
(= (color) red)
(= (color) green)
(= (color) blue)

; This should return ALL three colors
!(color)
; Expected result: {red, green, blue}

; Multiple shapes
(= (shape) circle)
(= (shape) square)
(= (shape) triangle)

!(shape)
; Expected result: {circle, square, triangle}

; Cartesian product: combining non-deterministic results
; Each combination of color and shape should be produced
!(pair (color) (shape))
; Expected result: {
;   (pair red circle), (pair red square), (pair red triangle),
;   (pair green circle), (pair green square), (pair green triangle),
;   (pair blue circle), (pair blue square), (pair blue triangle)
; }

; Nested non-determinism
(= (outer) (inner 1))
(= (outer) (inner 2))
(= (inner $x) (* $x 10))
(= (inner $x) (+ $x 100))

!(outer)
; Expected result: {10, 101, 20, 102}
; Explanation:
;   outer → inner 1 → {10, 101}
;   outer → inner 2 → {20, 102}

; Using superpose explicitly
!(superpose (A B C D))
; Expected result: {A, B, C, D}

; Collapse collects all alternatives into a single expression
!(collapse (color))
; Expected result: (red green blue)
