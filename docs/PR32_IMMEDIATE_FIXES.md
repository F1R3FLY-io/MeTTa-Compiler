# PR #32 Immediate Fixes - Quick Reference

## Changes Required Before Merge

Total Time: ~2 hours

---

## Fix 1: Change `NotReducible` to `Empty` Return Value

**File:** `src/backend/eval.rs`
**Line:** 489
**Time:** 5 minutes

### Current Code:
```rust
fn eval_switch_minimal(atom: MettaValue, cases: MettaValue, env: Environment) -> EvalResult {
    if let MettaValue::SExpr(cases_items) = cases {
        if cases_items.is_empty() {
            return (vec![MettaValue::Atom("NotReducible".to_string())], env);  // ❌ WRONG
        }
        // ... rest
    }
}
```

### Fixed Code:
```rust
fn eval_switch_minimal(atom: MettaValue, cases: MettaValue, env: Environment) -> EvalResult {
    if let MettaValue::SExpr(cases_items) = cases {
        if cases_items.is_empty() {
            return (vec![MettaValue::Atom("Empty".to_string())], env);  // ✅ CORRECT
        }
        // ... rest
    }
}
```

---

## Fix 2: Update Test Expectations

**File:** `src/backend/eval.rs`
**Line:** 3816
**Time:** 2 minutes

### Current Test:
```rust
#[test]
fn test_switch_no_match() {
    // ... setup ...
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Atom("NotReducible".to_string()));  // ❌ OLD
}
```

### Fixed Test:
```rust
#[test]
fn test_switch_no_match() {
    // ... setup ...
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Atom("Empty".to_string()));  // ✅ UPDATED
}
```

---

## Fix 3: Update Documentation

**File:** `docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md`
**Location:** After the `switch` and `case` entries (around line 60)
**Time:** 30 minutes

### Add This Section:

```markdown
### Implementation Notes for `switch` and `case`

**Status:** ✅ Basic implementation complete, ⚠️ Full compatibility pending

#### Known Limitations:

1. **Nondeterministic Results** - Does not fully handle expressions that evaluate
   to multiple results. Requires `collapse` and `superpose` functions which are
   not yet implemented in this compiler.

   ```metta
   ; This limitation affects cases like:
   (= (multi) 1)
   (= (multi) 2)
   !(case (multi) ((1 "one") (2 "two")))
   ; Expected: ["one", "two"]
   ; Actual: ["one"] (incomplete)
   ```

2. **List Deconstruction** - Uses Rust list slicing instead of MeTTa's `decons-atom`.
   May behave differently with improper lists or edge cases.

3. **Architecture** - Implemented as built-in special forms rather than composed
   MeTTa functions, so cannot be modified in MeTTa code.

#### What Works:

- ✅ Basic literal pattern matching
- ✅ Variable binding and substitution
- ✅ S-expression pattern matching
- ✅ Sequential case testing (first match wins)
- ✅ Empty handling in `case`
- ✅ Wildcard patterns
- ✅ Nested patterns
- ✅ Error handling

#### Future Work:

For full MeTTa stdlib compatibility, these functions need implementation:

- `collapse` - Collect nondeterministic results (CRITICAL)
- `superpose` - Distribute list elements as results (CRITICAL)
- `decons-atom` - Proper list deconstruction (MEDIUM)
- `unify` - Pattern matching with branches (MEDIUM)

See `docs/PR32_COMPLETION_ROADMAP.md` for complete implementation plan.
```

---

## Fix 4: Add Code Comments

**File:** `src/backend/eval.rs`
**Time:** 30 minutes

### Comment 1: In `case` handler (line 293)

```rust
// Subsequently tests multiple pattern-matching conditions (second argument) for the
// given value (first argument)
//
// IMPLEMENTATION NOTE: This is a simplified implementation that differs from the
// official MeTTa stdlib in the following ways:
//
// 1. Does not use collapse/superpose for nondeterministic results
//    Official: (let $c (collapse $atom)
//                (if (== (noeval $c) ())
//                  (id (switch-minimal Empty $cases))
//                  (chain (eval (superpose $c)) $e (id (switch-minimal $e $cases)))))
//
// 2. May not handle all nondeterministic evaluation cases correctly
//
// TODO: Implement collapse and superpose functions for full compatibility
"case" => {
    require_two_args!("case", items, env);
    // ... rest
}
```

### Comment 2: In `switch` handler (line 328)

```rust
// Difference between `switch` and `case` is how they interpret `Empty` result.
// case evaluates first argument and checks if result is empty.
// switch does NOT evaluate first argument (takes it as-is).
//
// IMPLEMENTATION NOTE: This matches the official MeTTa behavior for basic cases,
// but see limitations noted in the `case` handler above.
"switch" => {
    require_two_args!("switch", items, env);
    // ... rest
}
```

### Comment 3: In `eval_switch_minimal` (line 486)

```rust
/// Helper function to implement switch-minimal logic
/// Handles the main switch logic by deconstructing cases and calling switch-internal
///
/// IMPLEMENTATION NOTE: Uses Rust list slicing instead of MeTTa's `decons-atom`.
/// This may behave differently with improper lists or special expression structures.
///
/// Official implementation uses:
///   (function (chain (decons-atom $cases) $list
///     (chain (eval (switch-internal $atom $list)) $res
///       (chain (eval (if-equal $res NotReducible Empty $res)) $x (return $x)))))
///
/// TODO: Optionally implement `decons-atom` for better compatibility
fn eval_switch_minimal(atom: MettaValue, cases: MettaValue, env: Environment) -> EvalResult {
    // ... rest
}
```

### Comment 4: In `eval_switch_internal` (line 513)

```rust
/// Helper function to implement switch-internal logic
/// Tests one case and recursively tries remaining cases if no match
///
/// IMPLEMENTATION NOTE: Evaluates the template immediately upon match,
/// while the official implementation uses `function/return` for controlled evaluation.
///
/// Official implementation:
///   (function (unify $atom $pattern
///     (return $template)
///     (chain (eval (switch-minimal $atom $tail)) $ret (return $ret))))
///
/// The current approach is functionally equivalent for most cases but may differ
/// with complex evaluation scenarios.
fn eval_switch_internal(atom: MettaValue, cases_data: MettaValue, env: Environment) -> EvalResult {
    // ... rest
}
```

---

## Fix 5: Add Limitation Tests (Optional but Recommended)

**File:** `src/backend/eval.rs`
**Location:** After existing switch/case tests (around line 4313)
**Time:** 30 minutes

```rust
#[test]
#[ignore] // Ignored because this is a known limitation
fn test_case_nondeterministic_limitation() {
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

    // KNOWN LIMITATION: Should return all three results
    // Currently only returns first result
    // TODO: Remove #[ignore] when collapse/superpose are implemented
    assert_eq!(results.len(), 3); // Will fail with current implementation
    assert!(results.contains(&MettaValue::String("one".to_string())));
    assert!(results.contains(&MettaValue::String("two".to_string())));
    assert!(results.contains(&MettaValue::String("three".to_string())));
}
```

---

## Verification Checklist

After applying fixes, verify:

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes (all non-ignored tests)
- [ ] `cargo clippy` has no warnings
- [ ] Documentation is updated
- [ ] Comments are added
- [ ] Test expectations match new behavior
- [ ] No regression in existing functionality

---

## Commands to Run

```bash
# 1. Make the code changes above

# 2. Build and test
cargo build
cargo test

# 3. Check for warnings
cargo clippy

# 4. Run specific switch/case tests
cargo test test_switch
cargo test test_case

# 5. Format code
cargo fmt

# 6. Check git diff
git diff src/backend/eval.rs
git diff docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md
```

---

## Expected Test Results After Fix

```
test test_switch_basic ... ok
test test_switch_with_variables ... ok
test test_switch_no_match ... ok          ← Should now expect "Empty"
test test_switch_with_sexpr_pattern ... ok
test test_case_basic ... ok
test test_case_with_evaluation ... ok
test test_case_with_empty_result ... ok
test test_case_nondeterministic_limitation ... ignored  ← Known limitation
```

---

## Summary of Changes

| File | Lines | Change | Time |
|------|-------|--------|------|
| `src/backend/eval.rs` | 489 | NotReducible → Empty | 5 min |
| `src/backend/eval.rs` | 3816 | Update test | 2 min |
| `src/backend/eval.rs` | 293, 328, 486, 513 | Add comments | 30 min |
| `src/backend/eval.rs` | After 4313 | Add ignored test | 30 min |
| `docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md` | After line 60 | Add limitations | 30 min |

**Total Time:** ~2 hours
**Complexity:** Low
**Risk:** Very low (mostly documentation and one simple fix)

---

## After Merge

Next steps (separate PRs):

1. Implement `decons-atom` (Phase 2.1)
2. Implement `unify` (Phase 2.2)
3. Research `collapse` and `superpose` (Phase 3)

See `docs/PR32_COMPLETION_ROADMAP.md` for complete plan.

---

**Quick Reference Version:** 1.0
**Date:** 2025-10-27
**For:** PR #32 - switch and case implementation
