# PR #32 Completion Roadmap: `switch` and `case` Implementation

## Document Overview

This document outlines the work required to make PR #32's `switch` and `case` implementation fully compatible with the official MeTTa stdlib. It provides a prioritized roadmap understanding that `collapse` and `superpose` are not yet implemented in this compiler.

**Status:** PR #32 provides a working basic implementation but is **NOT fully compatible** with the official MeTTa stdlib.

**Last Updated:** 2025-10-27

---

## Executive Summary

The current implementation handles basic pattern matching correctly but has the following gaps:

1. ❌ **Returns wrong value on no match** (`NotReducible` instead of `Empty`)
2. ❌ **Cannot handle nondeterministic results properly** (requires `collapse`/`superpose`)
3. ❌ **Missing `decons-atom` semantics** (uses Rust slicing instead)
4. ❌ **Missing `unify` grounded function** (uses different pattern matching)
5. ⚠️ **Different architectural approach** (built-in vs. composed functions)

---

## Phase 1: Critical Fixes (Can Be Done NOW)

These fixes can be implemented immediately without `collapse`/`superpose`.

### 1.1 Fix `NotReducible` to `Empty` Conversion

**Priority:** 🔴 **CRITICAL**
**Effort:** Low (< 1 hour)
**File:** `src/backend/eval.rs`

#### Current Behavior (WRONG):
```rust
// Line 489 in eval_switch_minimal
if cases_items.is_empty() {
    return (vec![MettaValue::Atom("NotReducible".to_string())], env);
}
```

#### Required Fix:
```rust
// In eval_switch_minimal, when no cases left:
if cases_items.is_empty() {
    return (vec![MettaValue::Atom("Empty".to_string())], env);
}
```

#### Also Fix in `eval_switch_internal`:
When recursion returns `NotReducible`, it should be converted to `Empty` at the top level.

```rust
// Add this helper function:
fn convert_not_reducible_to_empty(results: Vec<MettaValue>) -> Vec<MettaValue> {
    results
        .into_iter()
        .map(|v| match v {
            MettaValue::Atom(ref s) if s == "NotReducible" => {
                MettaValue::Atom("Empty".to_string())
            }
            other => other,
        })
        .collect()
}

// Then in eval_switch_minimal, before returning:
let final_results = convert_not_reducible_to_empty(results);
return (final_results, env);
```

#### Test Changes Required:
```rust
// Line 3816 in tests - CHANGE THIS:
assert_eq!(results[0], MettaValue::Atom("NotReducible".to_string()));

// TO THIS:
assert_eq!(results[0], MettaValue::Atom("Empty".to_string()));
```

#### Official Reference:
```metta
; stdlib.metta:351-353
(function (chain (decons-atom $cases) $list
  (chain (eval (switch-internal $atom $list)) $res
    (chain (eval (if-equal $res NotReducible Empty $res)) $x (return $x)) )))
```

---

### 1.2 Add Documentation for Current Limitations

**Priority:** 🔴 **CRITICAL**
**Effort:** Low (< 1 hour)
**File:** `docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md`

#### Add This Section:

```markdown
### Implementation Notes for `switch` and `case`

#### Current Implementation Status

The `switch` and `case` functions are implemented as built-in special forms in Rust,
providing basic pattern matching functionality. However, they differ from the official
MeTTa stdlib implementation in the following ways:

##### Known Limitations:

1. **Nondeterministic Results** - Currently does not properly handle nondeterministic
   evaluation results. When the first argument to `case` evaluates to multiple results
   (e.g., from multiple matching rules), only the first result is fully processed.

   - **Required:** `collapse` and `superpose` functions (not yet implemented)
   - **Impact:** Multi-valued expressions in `case` may produce incomplete results

2. **Missing `decons-atom` Semantics** - Uses Rust list slicing instead of the MeTTa
   `decons-atom` function for deconstructing the cases list.

   - **Impact:** May behave differently with improper lists or special structures
   - **Workaround:** Ensure cases are always proper lists

3. **Built-in vs. Composed** - Implemented as built-in special forms rather than
   composed MeTTa functions using `function`, `chain`, and `return`.

   - **Impact:** Cannot be modified or extended in MeTTa code
   - **Benefit:** Faster execution, no external dependencies

##### What Works Correctly:

- ✅ Basic literal pattern matching
- ✅ Variable binding and substitution
- ✅ S-expression pattern matching
- ✅ Sequential case testing (first match wins)
- ✅ Empty handling in `case`
- ✅ Wildcard patterns (`_`)
- ✅ Nested pattern matching with variable consistency
- ✅ Error handling for malformed inputs

##### Examples That Work:

```metta
; Basic matching
!(switch 42 ((42 "found") (43 "not found")))  ; Returns: "found"

; Variable binding
!(switch 42 (($x (+ $x 10))))  ; Returns: 52

; Pattern matching
!(switch (foo 10 20) (((foo $x $y) (+ $x $y))))  ; Returns: 30

; Empty handling
!(case () ((Empty "was empty") (42 "was number")))  ; Returns: "was empty"
```

##### Examples That May Not Work As Expected:

```metta
; Nondeterministic evaluation (LIMITATION)
(= (multi) 1)
(= (multi) 2)
(= (multi) 3)
!(case (multi) ((1 "one") (2 "two") (3 "three")))
; Expected: ["one", "two", "three"]
; Actual: ["one"] (only first result processed fully)
```

##### Future Work:

To achieve full compatibility with the official MeTTa stdlib, the following
functions need to be implemented:

1. `collapse` - Collects nondeterministic results into a list
2. `superpose` - Distributes list elements as separate results
3. `decons-atom` - Properly deconstructs expressions
4. `unify` - Pattern matching with success/failure branches

See `docs/PR32_COMPLETION_ROADMAP.md` for the complete implementation plan.
```

---

### 1.3 Add Code Comments for Future Improvements

**Priority:** 🟡 **MEDIUM**
**Effort:** Low (< 30 minutes)
**File:** `src/backend/eval.rs`

#### Add These Comments:

```rust
// Line 293, in "case" handler:
"case" => {
    require_two_args!("case", items, env);

    // TODO: Full MeTTa compatibility requires:
    // 1. Implement (collapse $atom) to collect all nondeterministic results
    // 2. Implement (superpose $c) to distribute each result for matching
    // 3. Replace this loop with: collapse -> superpose -> switch-minimal pattern
    //
    // Current limitation: Only processes first few results, doesn't properly
    // handle nondeterministic evaluation as the official stdlib does.
    //
    // Official pattern:
    //   (let $c (collapse $atom)
    //     (if (== (noeval $c) ())
    //       (id (switch-minimal Empty $cases))
    //       (chain (eval (superpose $c)) $e (id (switch-minimal $e $cases)))))

    let atom = items[1].clone();
    let cases = items[2].clone();
    // ... rest of implementation
}

// Line 486, in eval_switch_minimal:
fn eval_switch_minimal(atom: MettaValue, cases: MettaValue, env: Environment) -> EvalResult {
    // TODO: Full MeTTa compatibility requires:
    // 1. Implement (decons-atom $cases) instead of direct list slicing
    // 2. Use (if-equal $res NotReducible Empty $res) for proper conversion
    //
    // Current implementation uses Rust list operations which may differ
    // from decons-atom behavior with improper lists.

    if let MettaValue::SExpr(cases_items) = cases {
        // ... rest of implementation
    }
}

// Line 513, in eval_switch_internal:
fn eval_switch_internal(atom: MettaValue, cases_data: MettaValue, env: Environment) -> EvalResult {
    // TODO: Full MeTTa compatibility requires:
    // 1. Implement (unify $atom $pattern success-expr failure-expr)
    // 2. Use (return $template) instead of immediate evaluation
    //
    // Current implementation evaluates the template immediately, while the
    // official version uses function/return for controlled evaluation.
    //
    // Official pattern:
    //   (function (unify $atom $pattern
    //     (return $template)
    //     (chain (eval (switch-minimal $atom $tail)) $ret (return $ret))))

    if let MettaValue::SExpr(cases_items) = cases_data {
        // ... rest of implementation
    }
}
```

---

## Phase 2: Implement Missing Grounded Functions

These functions are required for full compatibility but are independent implementations.

### 2.1 Implement `decons-atom` Grounded Function

**Priority:** 🟡 **MEDIUM**
**Effort:** Medium (2-4 hours)
**Dependencies:** None
**File:** `src/backend/eval.rs`

#### Specification:
```metta
(@doc decons-atom
  (@desc "Works as a reverse to cons-atom function. It gets Expression as an input
          and returns it splitted to head and tail")
  (@params (
    (@param "Expression to deconstruct")))
  (@return "Pair of head and tail"))
(: decons-atom (-> Expression Atom))

; Examples:
!(decons-atom (A B C))        ; Returns: (A (B C))
!(decons-atom (A))            ; Returns: (A ())
!(decons-atom ())             ; Returns: Error or Empty
```

#### Implementation Approach:

```rust
// Add to eval_grounded or create new grounded function section

"decons-atom" => {
    if args.len() != 1 {
        let err = MettaValue::Error(
            "decons-atom requires exactly 1 argument".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return Some(err);
    }

    match &args[0] {
        MettaValue::SExpr(items) if items.is_empty() => {
            // Empty expression - return error or Empty
            Some(MettaValue::Error(
                "decons-atom: cannot deconstruct empty expression".to_string(),
                Box::new(MettaValue::SExpr(vec![])),
            ))
        }
        MettaValue::SExpr(items) if items.len() == 1 => {
            // Single element: (head ())
            Some(MettaValue::SExpr(vec![
                items[0].clone(),
                MettaValue::SExpr(vec![]),
            ]))
        }
        MettaValue::SExpr(items) => {
            // Multiple elements: (head (tail...))
            let head = items[0].clone();
            let tail = MettaValue::SExpr(items[1..].to_vec());
            Some(MettaValue::SExpr(vec![head, tail]))
        }
        other => {
            // Not an expression - return error
            Some(MettaValue::Error(
                "decons-atom expects an Expression".to_string(),
                Box::new(other.clone()),
            ))
        }
    }
}
```

#### Tests to Add:

```rust
#[test]
fn test_decons_atom_basic() {
    let env = Environment::new();

    // (decons-atom (A B C)) -> (A (B C))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("decons-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("A".to_string()),
            MettaValue::Atom("B".to_string()),
            MettaValue::Atom("C".to_string()),
        ]),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);

    if let MettaValue::SExpr(pair) = &results[0] {
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[0], MettaValue::Atom("A".to_string()));
        if let MettaValue::SExpr(tail) = &pair[1] {
            assert_eq!(tail.len(), 2);
            assert_eq!(tail[0], MettaValue::Atom("B".to_string()));
            assert_eq!(tail[1], MettaValue::Atom("C".to_string()));
        } else {
            panic!("Expected tail to be SExpr");
        }
    } else {
        panic!("Expected result to be SExpr pair");
    }
}

#[test]
fn test_decons_atom_single() {
    let env = Environment::new();

    // (decons-atom (A)) -> (A ())
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("decons-atom".to_string()),
        MettaValue::SExpr(vec![MettaValue::Atom("A".to_string())]),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);

    if let MettaValue::SExpr(pair) = &results[0] {
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[0], MettaValue::Atom("A".to_string()));
        assert_eq!(pair[1], MettaValue::SExpr(vec![]));
    } else {
        panic!("Expected result to be SExpr pair");
    }
}

#[test]
fn test_decons_atom_empty() {
    let env = Environment::new();

    // (decons-atom ()) -> Error
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("decons-atom".to_string()),
        MettaValue::SExpr(vec![]),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(..)));
}
```

#### Update `switch-minimal` to Use `decons-atom`:

Once implemented, optionally update `eval_switch_minimal`:

```rust
// OPTIONAL: Use decons-atom for better compatibility
fn eval_switch_minimal(atom: MettaValue, cases: MettaValue, env: Environment) -> EvalResult {
    // Use decons-atom instead of direct slicing
    let decons_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("decons-atom".to_string()),
        cases,
    ]);

    let (decons_results, decons_env) = eval(decons_expr, env);

    if decons_results.is_empty() {
        return (vec![MettaValue::Atom("Empty".to_string())], decons_env);
    }

    if let MettaValue::Error(..) = decons_results[0] {
        // Empty cases list
        return (vec![MettaValue::Atom("Empty".to_string())], decons_env);
    }

    // Extract (head tail) pair
    if let MettaValue::SExpr(pair) = &decons_results[0] {
        if pair.len() != 2 {
            let err = MettaValue::Error(
                "decons-atom should return pair".to_string(),
                Box::new(decons_results[0].clone()),
            );
            return (vec![err], decons_env);
        }

        let first_case = pair[0].clone();
        let remaining_cases = pair[1].clone();
        let cases_list = MettaValue::SExpr(vec![first_case, remaining_cases]);

        return eval_switch_internal(atom, cases_list, decons_env);
    }

    let err = MettaValue::Error(
        "decons-atom should return expression".to_string(),
        Box::new(decons_results[0].clone()),
    );
    (vec![err], decons_env)
}
```

---

### 2.2 Implement `unify` Grounded Function

**Priority:** 🟡 **MEDIUM**
**Effort:** Medium (3-5 hours)
**Dependencies:** None
**File:** `src/backend/eval.rs`

#### Specification:
```metta
(@doc unify
  (@desc "Matches atom against pattern. If matching is successful then success
          expression is evaluated. Otherwise alternative is returned")
  (@params (
    (@param "Atom to match")
    (@param "Pattern to match with")
    (@param "Result returned if matching is successful")
    (@param "Result returned if matching is failed")))
  (@return "Result depending on the pattern matching result"))
(: unify (-> Atom Atom Atom Atom %Undefined%))

; Examples:
!(unify 42 42 success fail)           ; Returns: success
!(unify 42 43 success fail)           ; Returns: fail
!(unify 42 $x success fail)           ; Returns: success (binds $x=42)
!(unify (foo 10) (foo $x) $x fail)    ; Returns: 10
```

#### Implementation Approach:

```rust
// Add to eval_grounded or eval_special_form

"unify" => {
    if args.len() != 4 {
        let err = MettaValue::Error(
            "unify requires exactly 4 arguments: atom, pattern, success-expr, fail-expr".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return Some(err);
    }

    let atom = &args[0];
    let pattern = &args[1];
    let success_expr = &args[2];
    let fail_expr = &args[3];

    // Try to pattern match
    if let Some(bindings) = pattern_match(pattern, atom) {
        // Success: apply bindings to success expression
        let instantiated = apply_bindings(success_expr, &bindings);
        Some(instantiated)
    } else {
        // Failure: return fail expression as-is (unevaluated)
        Some(fail_expr.clone())
    }
}
```

**Note:** The above implementation should be added as a **grounded function**, not a special form, if you want it to evaluate arguments first. Check the official MeTTa implementation to determine the correct approach.

#### Alternative: As a Special Form (Arguments NOT Evaluated)

```rust
// In eval_sexpr, before the main evaluation loop:

"unify" => {
    if items.len() != 5 {  // Including "unify" itself
        let err = MettaValue::Error(
            "unify requires exactly 4 arguments".to_string(),
            Box::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let atom = items[1].clone();
    let pattern = items[2].clone();
    let success_expr = items[3].clone();
    let fail_expr = items[4].clone();

    // Evaluate atom (first argument)
    let (atom_results, atom_env) = eval(atom, env);

    let mut final_results = Vec::new();

    for atom_result in atom_results {
        // Try to pattern match
        if let Some(bindings) = pattern_match(&pattern, &atom_result) {
            // Success: apply bindings and evaluate success expression
            let instantiated = apply_bindings(&success_expr, &bindings);
            let (success_results, _) = eval(instantiated, atom_env.clone());
            final_results.extend(success_results);
        } else {
            // Failure: evaluate fail expression
            let (fail_results, _) = eval(fail_expr.clone(), atom_env.clone());
            final_results.extend(fail_results);
        }
    }

    return (final_results, atom_env);
}
```

#### Tests to Add:

```rust
#[test]
fn test_unify_success_literal() {
    let env = Environment::new();

    // (unify 42 42 success fail)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("unify".to_string()),
        MettaValue::Long(42),
        MettaValue::Long(42),
        MettaValue::Atom("success".to_string()),
        MettaValue::Atom("fail".to_string()),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Atom("success".to_string()));
}

#[test]
fn test_unify_failure_literal() {
    let env = Environment::new();

    // (unify 42 43 success fail)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("unify".to_string()),
        MettaValue::Long(42),
        MettaValue::Long(43),
        MettaValue::Atom("success".to_string()),
        MettaValue::Atom("fail".to_string()),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Atom("fail".to_string()));
}

#[test]
fn test_unify_with_variable() {
    let env = Environment::new();

    // (unify 42 $x $x fail)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("unify".to_string()),
        MettaValue::Long(42),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),  // Returns bound value
        MettaValue::Atom("fail".to_string()),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_unify_with_pattern() {
    let env = Environment::new();

    // (unify (foo 10 20) (foo $x $y) (+ $x $y) fail)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("unify".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
        MettaValue::Atom("fail".to_string()),
    ]);

    let (results, _) = eval(expr, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(30));
}
```

#### Optional: Update `switch-internal` to Use `unify`:

```rust
// This is OPTIONAL and may not improve functionality significantly
// The current pattern_match approach is equivalent for switch/case purposes
```

---

### 2.3 Implement Helper Functions (`if-equal`, `noeval`, `id`)

**Priority:** 🟢 **LOW**
**Effort:** Low (1-2 hours each)
**Dependencies:** None
**File:** `src/backend/eval.rs`

#### 2.3.1 `if-equal` Function

```rust
// Add as special form or MeTTa function

"if-equal" => {
    if items.len() != 5 {
        let err = MettaValue::Error(
            "if-equal requires 4 arguments: val1, val2, then-expr, else-expr".to_string(),
            Box::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let val1 = items[1].clone();
    let val2 = items[2].clone();
    let then_expr = items[3].clone();
    let else_expr = items[4].clone();

    // Evaluate both values
    let (results1, env1) = eval(val1, env);
    let (results2, env2) = eval(val2, env1);

    // Check if any combination is equal
    let mut any_equal = false;
    for r1 in &results1 {
        for r2 in &results2 {
            if values_equal(r1, r2) {
                any_equal = true;
                break;
            }
        }
        if any_equal {
            break;
        }
    }

    if any_equal {
        eval(then_expr, env2)
    } else {
        eval(else_expr, env2)
    }
}

// Helper function for value equality
fn values_equal(v1: &MettaValue, v2: &MettaValue) -> bool {
    match (v1, v2) {
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::SExpr(a), MettaValue::SExpr(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        (MettaValue::Nil, MettaValue::Nil) => true,
        _ => false,
    }
}
```

#### 2.3.2 `noeval` Function

```rust
// Add as special form (prevents evaluation)

"noeval" => {
    if items.len() != 2 {
        let err = MettaValue::Error(
            "noeval requires exactly 1 argument".to_string(),
            Box::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    // Return argument without evaluation
    return (vec![items[1].clone()], env);
}
```

#### 2.3.3 `id` Function

```rust
// Add as regular function or rule
// Can be defined as a MeTTa rule:
// (= (id $x) $x)

// Or as special form:
"id" => {
    if items.len() != 2 {
        let err = MettaValue::Error(
            "id requires exactly 1 argument".to_string(),
            Box::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    // Evaluate argument and return it
    return eval(items[1].clone(), env);
}
```

---

## Phase 3: Implement `collapse` and `superpose` (BLOCKED)

These functions are **critical** for full `case` compatibility but are currently **NOT implemented** in the compiler.

### 3.1 Implement `collapse` Function

**Priority:** 🔴 **CRITICAL** (for full compatibility)
**Effort:** High (1-2 days)
**Dependencies:**
- `collapse-bind` (grounded function)
- `foldl-atom` (stdlib function)
- Understanding of nondeterministic evaluation
- Possibly context/space management

**Status:** ⛔ **BLOCKED** - Not yet implemented in compiler

#### Specification:
```metta
; Official stdlib doesn't show collapse definition directly in stdlib.metta
; It's likely a grounded function or defined elsewhere

; Behavior:
; Takes an expression that may evaluate to multiple results
; Returns a list containing all those results

; Example:
(= (multi) 1)
(= (multi) 2)
(= (multi) 3)

!(collapse (multi))  ; Returns: (1 2 3)
!(collapse 42)       ; Returns: (42)
!(collapse ())       ; Returns: ()
```

#### Implementation Requirements:

1. **Evaluate the expression fully** to get all nondeterministic results
2. **Collect all results** into a single list
3. **Handle empty results** (return empty list)
4. **Preserve evaluation environment**

#### Pseudo-Implementation:

```rust
"collapse" => {
    if items.len() != 2 {
        let err = MettaValue::Error(
            "collapse requires exactly 1 argument".to_string(),
            Box::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let expr = items[1].clone();

    // Evaluate expression to get ALL nondeterministic results
    let (results, new_env) = eval(expr, env);

    // Collect all results into a single list
    let collapsed = MettaValue::SExpr(results);

    return (vec![collapsed], new_env);
}
```

**Note:** This is a simplified version. The official implementation may have more complex semantics around handling `Empty`, errors, and bindings.

#### Research Required:

1. **Find the official implementation** of `collapse` in the hyperon-experimental repo
2. **Understand `collapse-bind`** and how it differs from `collapse`
3. **Test behavior** with the official MeTTa interpreter
4. **Determine handling** of:
   - Empty results
   - Error results
   - Mixed result types
   - Environment bindings

#### Files to Check:
```bash
# Search in the official repo
grep -r "collapse" /home/dylon/Workspace/f1r3fly.io/hyperon-experimental/lib/src/metta/runner/stdlib/
grep -r "collapse" /home/dylon/Workspace/f1r3fly.io/hyperon-experimental/python/hyperon/
```

---

### 3.2 Implement `superpose` Function

**Priority:** 🔴 **CRITICAL** (for full compatibility)
**Effort:** Medium (3-6 hours)
**Dependencies:**
- Understanding of nondeterministic results
- Possibly `superpose-bind`

**Status:** ⛔ **BLOCKED** - Not yet implemented in compiler

#### Specification:
```metta
(@doc superpose-bind
  (@desc "Complement to the collapse-bind. It takes result of collapse-bind
          (first argument) and returns only result atoms without bindings")
  (@params (
    (@param "Expression")))
  (@return "One of elements from a passed expression"))
(: superpose-bind (-> Expression Atom))

; Behavior:
; Takes a list/expression and returns each element as a separate result
; Opposite of collapse

; Example:
!(superpose (1 2 3))     ; Returns: 1, 2, 3 (three separate results)
!(superpose (42))        ; Returns: 42
!(superpose ())          ; Returns: Empty or no results
```

#### Implementation Requirements:

1. **Take a list expression**
2. **Return each element as a separate result** (multiple results from one call)
3. **Handle empty lists** (return no results or Empty)
4. **Distribute results** properly in nondeterministic evaluation

#### Pseudo-Implementation:

```rust
"superpose" => {
    if items.len() != 2 {
        let err = MettaValue::Error(
            "superpose requires exactly 1 argument".to_string(),
            Box::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let expr = items[1].clone();

    // Evaluate the expression
    let (results, new_env) = eval(expr, env);

    // If result is a list, distribute each element
    let mut distributed = Vec::new();
    for result in results {
        match result {
            MettaValue::SExpr(elements) if !elements.is_empty() => {
                // Return each element as a separate result
                distributed.extend(elements);
            }
            MettaValue::SExpr(elements) if elements.is_empty() => {
                // Empty list - return Empty or nothing
                // Check official behavior
            }
            other => {
                // Not a list - return as-is
                distributed.push(other);
            }
        }
    }

    return (distributed, new_env);
}
```

#### Research Required:

1. **Verify behavior** with official MeTTa interpreter
2. **Understand `superpose-bind`** vs. `superpose`
3. **Test edge cases**:
   - Empty lists
   - Nested lists
   - Non-list values
   - Error propagation

---

### 3.3 Update `case` Implementation to Use `collapse` and `superpose`

**Priority:** 🔴 **CRITICAL** (after 3.1 and 3.2)
**Effort:** Medium (2-3 hours)
**Dependencies:** `collapse`, `superpose`, `noeval`, `if-equal`

**Status:** ⛔ **BLOCKED** - Waiting for `collapse` and `superpose`

#### Target Implementation:

```rust
"case" => {
    require_two_args!("case", items, env);

    let atom = items[1].clone();
    let cases = items[2].clone();

    // Step 1: Collapse the atom to collect all nondeterministic results
    let collapse_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("collapse".to_string()),
        atom.clone(),
    ]);
    let (collapsed_results, collapsed_env) = eval(collapse_expr, env);

    if collapsed_results.is_empty() {
        // No results - treat as Empty
        return eval_switch_minimal(
            MettaValue::Atom("Empty".to_string()),
            cases,
            collapsed_env,
        );
    }

    let collapsed = &collapsed_results[0];

    // Step 2: Check if collapsed result is empty using noeval
    let noeval_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("noeval".to_string()),
        collapsed.clone(),
    ]);
    let (noeval_results, noeval_env) = eval(noeval_expr, collapsed_env);

    let is_empty = if let Some(result) = noeval_results.first() {
        matches!(result, MettaValue::SExpr(items) if items.is_empty())
    } else {
        false
    };

    if is_empty {
        // Empty tuple - use Empty atom
        return eval_switch_minimal(
            MettaValue::Atom("Empty".to_string()),
            cases,
            noeval_env,
        );
    }

    // Step 3: Superpose the collapsed results
    let superpose_expr = MettaValue::SExpr(vec![
        MettaValue::Atom("superpose".to_string()),
        collapsed.clone(),
    ]);
    let (superposed_results, superposed_env) = eval(superpose_expr, noeval_env);

    // Step 4: Apply switch-minimal to each superposed result
    let mut final_results = Vec::new();
    for result in superposed_results {
        let (switch_results, _) = eval_switch_minimal(
            result,
            cases.clone(),
            superposed_env.clone(),
        );
        final_results.extend(switch_results);
    }

    return (final_results, superposed_env);
}
```

#### Tests to Add After Implementation:

```rust
#[test]
fn test_case_with_nondeterministic_full() {
    let mut env = Environment::new();

    // Define multiple rules for multi
    let rule1 = Rule {
        lhs: MettaValue::SExpr(vec![MettaValue::Atom("multi".to_string())]),
        rhs: MettaValue::Long(1),
    };
    let rule2 = Rule {
        lhs: MettaValue::SExpr(vec![MettaValue::Atom("multi".to_string())]),
        rhs: MettaValue::Long(2),
    };
    let rule3 = Rule {
        lhs: MettaValue::SExpr(vec![MettaValue::Atom("multi".to_string())]),
        rhs: MettaValue::Long(3),
    };
    env.add_rule(rule1);
    env.add_rule(rule2);
    env.add_rule(rule3);

    // (case (multi) ((1 "one") (2 "two") (3 "three")))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("case".to_string()),
        MettaValue::SExpr(vec![MettaValue::Atom("multi".to_string())]),
        MettaValue::SExpr(vec![
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::String("one".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Long(2),
                MettaValue::String("two".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Long(3),
                MettaValue::String("three".to_string()),
            ]),
        ]),
    ]);

    let (results, _) = eval(expr, env);

    // Should return all three results
    assert_eq!(results.len(), 3);
    assert!(results.contains(&MettaValue::String("one".to_string())));
    assert!(results.contains(&MettaValue::String("two".to_string())));
    assert!(results.contains(&MettaValue::String("three".to_string())));
}
```

---

## Phase 4: Additional Improvements (Optional)

### 4.1 Add Integration Tests with Other Stdlib Functions

**Priority:** 🟢 **LOW**
**Effort:** Medium
**Dependencies:** Other stdlib functions

#### Test Composition:

```rust
#[test]
fn test_switch_with_let_binding() {
    // (let $x 10 (switch $x ((10 "ten") (20 "twenty"))))
}

#[test]
fn test_case_with_if_condition() {
    // (case (if true 1 2) ((1 "was true") (2 "was false")))
}

#[test]
fn test_nested_switch_case() {
    // (switch X ((A (case Y ...)) (B ...)))
}
```

---

### 4.2 Performance Optimization

**Priority:** 🟢 **LOW**
**Effort:** Variable

#### Potential Optimizations:

1. **Memoization** of pattern matching results
2. **Early exit** when first match found (already done)
3. **Compile-time pattern analysis** for simple cases
4. **Reduce cloning** of environments and values

---

### 4.3 Better Error Messages

**Priority:** 🟢 **LOW**
**Effort:** Low

#### Improvements:

```rust
// Current:
"switch requires exactly 2 arguments"

// Improved:
"switch requires exactly 2 arguments: atom and cases-list, but got X arguments"

// With context:
"switch requires exactly 2 arguments
  Expected: (switch <atom> <cases>)
  Got: (switch ...)"
```

---

## Summary of Phases

| Phase | Priority | Can Do Now? | Blocking On |
|-------|----------|-------------|-------------|
| **Phase 1: Critical Fixes** | 🔴 CRITICAL | ✅ **YES** | None |
| 1.1 Fix NotReducible→Empty | 🔴 CRITICAL | ✅ YES | None |
| 1.2 Add documentation | 🔴 CRITICAL | ✅ YES | None |
| 1.3 Add code comments | 🟡 MEDIUM | ✅ YES | None |
| **Phase 2: Grounded Functions** | 🟡 MEDIUM | ✅ **YES** | None |
| 2.1 Implement decons-atom | 🟡 MEDIUM | ✅ YES | None |
| 2.2 Implement unify | 🟡 MEDIUM | ✅ YES | None |
| 2.3 Implement helpers | 🟢 LOW | ✅ YES | None |
| **Phase 3: Collapse/Superpose** | 🔴 CRITICAL | ❌ **NO** | Research needed |
| 3.1 Implement collapse | 🔴 CRITICAL | ❌ NO | Research/design |
| 3.2 Implement superpose | 🔴 CRITICAL | ❌ NO | Research/design |
| 3.3 Update case | 🔴 CRITICAL | ❌ NO | Phase 3.1, 3.2 |
| **Phase 4: Improvements** | 🟢 LOW | ✅ **YES** | None |

---

## Recommended Action Plan

### Immediate Actions (Can Merge PR with These):

1. ✅ **Apply Phase 1.1** - Fix `NotReducible` to `Empty` conversion
2. ✅ **Apply Phase 1.2** - Add documentation about limitations
3. ✅ **Apply Phase 1.3** - Add code comments for future work

**Outcome:** PR #32 can be merged with clear documentation of limitations.

### Short-term Actions (Next 1-2 weeks):

4. ✅ **Apply Phase 2.1** - Implement `decons-atom`
5. ✅ **Apply Phase 2.2** - Implement `unify`
6. ✅ **Apply Phase 2.3** - Implement helper functions

**Outcome:** Better compatibility, easier testing against official MeTTa.

### Long-term Actions (Requires research):

7. ⏳ **Research `collapse` implementation** in official repo
8. ⏳ **Research `superpose` implementation** in official repo
9. ⏳ **Design nondeterministic handling** for this compiler
10. ⏳ **Apply Phase 3** once design is complete

**Outcome:** Full compatibility with official MeTTa stdlib.

---

## Testing Strategy

### Current Test Status:
- ✅ Basic pattern matching
- ✅ Variable binding
- ✅ S-expression patterns
- ✅ Empty handling
- ✅ Error cases
- ❌ Nondeterministic results
- ❌ collapse/superpose semantics
- ❌ Integration with stdlib functions

### Recommended Test Additions:

1. **After Phase 1.1:**
   - Update existing tests to expect `Empty` instead of `NotReducible`
   - Add explicit test for `NotReducible` → `Empty` conversion

2. **After Phase 2.1 (decons-atom):**
   - Add `decons-atom` unit tests
   - Add edge case tests (empty lists, single elements, improper lists)

3. **After Phase 2.2 (unify):**
   - Add `unify` unit tests
   - Add pattern matching edge cases
   - Test integration with `switch`

4. **After Phase 3 (collapse/superpose):**
   - Add nondeterministic evaluation tests
   - Add multi-valued expression tests
   - Add integration tests with `case`

---

## Conclusion

**Can PR #32 be merged now?**

**YES**, with the following conditions:

1. ✅ **Apply Phase 1.1** fix for `NotReducible` → `Empty`
2. ✅ **Apply Phase 1.2** documentation of limitations
3. ✅ **Merge with clear labels**: "Partial implementation", "MeTTa compatibility: Basic"

**What should be the next priority?**

After merge, prioritize in this order:
1. **Phase 2.1** - `decons-atom` (standalone, useful for other functions)
2. **Phase 2.2** - `unify` (standalone, useful for other functions)
3. **Research Phase 3** - Understanding `collapse`/`superpose` design
4. **Phase 3** - Full nondeterministic handling (major feature)

**Timeline Estimate:**
- Phase 1: 1-2 hours ✅ **Can merge today**
- Phase 2: 1-2 days ✅ **Can complete this week**
- Phase 3: 1-2 weeks ⏳ **Requires research and design**

---

## References

### Official MeTTa Implementations:
- `switch`: `/home/dylon/Workspace/f1r3fly.io/hyperon-experimental/lib/src/metta/runner/stdlib/stdlib.metta` lines 339-365
- `case`: Same file, lines 1193-1204
- `decons-atom`: Same file, line 98-103
- `unify`: Grounded function (check Rust implementation)
- `collapse`/`superpose`: Research needed

### PR #32 Files:
- Implementation: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/src/backend/eval.rs` lines 291-349, 486-557
- Tests: Same file, lines 3740-4313
- Documentation: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md`

### Related Documentation:
- This roadmap: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/docs/PR32_COMPLETION_ROADMAP.md`

---

**Document Version:** 1.0
**Last Updated:** 2025-10-27
**Maintainer:** Claude Code
**Status:** Living document - update as implementation progresses
