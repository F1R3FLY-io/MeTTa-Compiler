; MORK rulify Meta-Programming Demonstration
; Syntax: (rulify $name (, $p0) (, $t0 ...) <antecedent> <consequent>)
;
; rulify generates exec rules from coalgebra definitions.
; It matches on template arity to create appropriate rules.

; ============================================================================
; 1. Single Template - One Result Rule
; ============================================================================
; Generate rule for coalgebra with one template
!(rulify lift-tree
         (, (tree $t))            ; Pattern (unary conjunction)
         (, (ctx $t nil))         ; Single template (unary conjunction)
         (, (tmp (tree $t)))      ; Antecedent: match temp data
         (O (- (tmp (tree $t)))   ; Remove temp
            (+ (ctx $t nil))      ; Add result
            (+ (has changed))))   ; Signal change

; This creates a rule that:
; 1. Matches (tmp (tree $t)) in space
; 2. Removes it
; 3. Adds (ctx $t nil)
; 4. Marks space as changed

; ============================================================================
; 2. Binary Template - Two Result Rule
; ============================================================================
; Generate rule for coalgebra with two templates (unfold)
!(rulify explode-tree
         (, (ctx (branch $left $right) $path))   ; Pattern
         (, (ctx $left (cons $path L))           ; First template
            (ctx $right (cons $path R)))         ; Second template
         (, (tmp (ctx (branch $left $right) $path)))  ; Antecedent
         (O (- (tmp (ctx (branch $left $right) $path)))  ; Remove temp
            (+ (ctx $left (cons $path L)))               ; Add left result
            (+ (ctx $right (cons $path R)))              ; Add right result
            (+ (has changed))))                          ; Signal change

; This creates a rule that:
; 1. Matches branching context in temp
; 2. Removes it
; 3. Adds both child contexts
; 4. Marks space as changed

; ============================================================================
; 3. N-ary Template - Multiple Result Rule
; ============================================================================
; Generate rule for coalgebra with N templates
!(rulify split-triple
         (, (triple $a $b $c))                 ; Pattern
         (, (first $a) (second $b) (third $c)) ; Three templates
         (, (tmp (triple $a $b $c)))           ; Antecedent
         (O (- (tmp (triple $a $b $c)))        ; Remove temp
            (+ (first $a))                     ; Add first
            (+ (second $b))                    ; Add second
            (+ (third $c))                     ; Add third
            (+ (has changed))))                ; Signal change

; ============================================================================
; 4. Pattern Matching on Template Arity
; ============================================================================
; rulify can distinguish template count

; Unary template: (, $t0)
; - Generates one-result rule

; Binary template: (, $t0 $t1)
; - Generates two-result rule

; N-ary template: (, $t0 $t1 ... $tn)
; - Generates n-result rule

; ============================================================================
; 5. Complete Tree-to-Space Meta-Program
; ============================================================================
; Define all three coalgebra transformations via rulify

; Lift: tree → ctx
!(rulify lift
         (, (tree $t))
         (, (ctx $t nil))
         (, (tmp (tree $t)))
         (O (- (tmp (tree $t))) (+ (ctx $t nil)) (+ (has changed))))

; Explode: branch → left + right
!(rulify explode
         (, (ctx (branch $l $r) $p))
         (, (ctx $l (cons $p L)) (ctx $r (cons $p R)))
         (, (tmp (ctx (branch $l $r) $p)))
         (O (- (tmp (ctx (branch $l $r) $p)))
            (+ (ctx $l (cons $p L)))
            (+ (ctx $r (cons $p R)))
            (+ (has changed))))

; Drop: leaf → value
!(rulify drop
         (, (ctx (leaf $v) $p))
         (, (value $p $v))
         (, (tmp (ctx (leaf $v) $p)))
         (O (- (tmp (ctx (leaf $v) $p))) (+ (value $p $v)) (+ (has changed))))

; ============================================================================
; 6. Variable Capture in Meta-Programming
; ============================================================================
; rulify captures variables from the template

; Variables in pattern: $p0, $p1, ...
; Variables in templates: $t0, $t1, ...
; Variables in antecedent/consequent: can reference pattern/template vars

; Example:
!(rulify example
         (, (pattern $x $y))           ; Captures $x, $y
         (, (result $x) (result $y))   ; Uses $x, $y in templates
         (, (tmp (pattern $x $y)))     ; References $x, $y
         (O (- (tmp (pattern $x $y)))
            (+ (result $x))
            (+ (result $y))
            (+ (has changed))))

; ============================================================================
; 7. Dynamic Rule Generation
; ============================================================================
; rulify enables dynamic code generation at runtime

; Given: Coalgebra definition
; Generate: Executable exec rule
; Result: Space transformation system

; This is the essence of meta-programming:
; - Code that generates code
; - Templates that expand to rules
; - Structural pattern matching on conjunctions
