; MORK lookup Special Form Demonstration
; Syntax: (lookup <pattern> <success-goals> <failure-goals>)
;
; lookup conditionally executes based on pattern presence in space.
; Both branches are always conjunctions.

; ============================================================================
; 1. Simple Success Branch
; ============================================================================
; If pattern found, execute success conjunction
!(lookup existing-fact (, T) (, F))

; If 'existing-fact' is in space → execute (, T) → return T
; Otherwise → execute (, F) → return F

; ============================================================================
; 2. Simple Failure Branch
; ============================================================================
; Variable patterns typically not found (unless bound)
!(lookup $unbound-var (, (found $unbound-var)) (, (not-found)))

; Variable $unbound-var not in space → execute failure branch

; ============================================================================
; 3. Pattern with Variable Binding
; ============================================================================
; Define some facts
(= (data foo 42))
(= (data bar 99))

; Lookup with variable binding
!(lookup (data foo $value)
         (, (found-foo-value $value))
         (, (foo-not-found)))

; If (data foo $value) matches → $value = 42 → (found-foo-value 42)

; ============================================================================
; 4. Nested Lookup in Success Branch
; ============================================================================
; Success branch can contain another lookup
!(lookup $primary
         (, (lookup $secondary
                    (, (both-found))
                    (, (only-primary-found))))
         (, (neither-found)))

; Evaluation:
; 1. If $primary found → check $secondary
;    - If $secondary also found → (both-found)
;    - If $secondary not found → (only-primary-found)
; 2. If $primary not found → (neither-found)

; ============================================================================
; 5. exec in Failure Branch
; ============================================================================
; Generate rule if pattern not found
!(lookup $pattern
         (, (pattern-exists))
         (, (exec (0 $pattern) (,) (, (created-from-pattern $pattern)))))

; If $pattern not in space → create exec rule to handle it

; ============================================================================
; 6. Multiple Goals in Success Branch
; ============================================================================
; Success conjunction with multiple goals
!(lookup (condition met)
         (, (action1) (action2) (action3))
         (, (default-action)))

; If condition met → execute all three actions sequentially

; ============================================================================
; 7. Multiple Goals in Failure Branch
; ============================================================================
; Failure conjunction with multiple fallback actions
!(lookup (primary-source)
         (, (use-primary))
         (, (try-secondary) (use-fallback)))

; If primary not found → try secondary, then use fallback

; ============================================================================
; 8. Conditional Chain
; ============================================================================
; Chain multiple lookups for priority-based execution
!(lookup first-priority
         (, (execute-first))
         (, (lookup second-priority
                    (, (execute-second))
                    (, (lookup third-priority
                              (, (execute-third))
                              (, (execute-default)))))))

; Priority chain: first → second → third → default

; ============================================================================
; 9. Variable Binding with Success Continuation
; ============================================================================
!(lookup (config $key $value)
         (, (use-config $key $value) (validate $value))
         (, (use-default-config)))

; If config found → bind variables and use them in success branch
