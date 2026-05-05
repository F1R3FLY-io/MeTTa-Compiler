# MeTTaTron-Specific Language Extensions

**Audience.** Engineers working at the boundary between MeTTaTron and the
MeTTa specification (`metta-specification` repo, derived from MeTTa HE).

**Scope.** This document enumerates intentional, MeTTaTron-only deviations
from MeTTa Hyperon-Experimental (HE). Each entry is preserved by user
directive; this is NOT a deprecation list.

**Non-goal.** This document is NOT part of the MeTTa language specification.
The metta-specification repo is derived from HE and remains the
authoritative reference for MeTTa-language behavior. These are
**implementation extensions**; programs that depend on them are not
portable to HE-conformant runtimes.

## How to read each entry

Every extension is listed with:
1. **Spec reference** — section of `metta-specification/spec/` that defines the HE behavior.
2. **MeTTaTron behavior** — what MeTTaTron does.
3. **HE behavior** — what HE does (the spec).
4. **Source location** — `file:line` in MeTTa-Compiler.
5. **Motivation** — when known from git or design context.
6. **Regression tests** — pinning the extension behavior.

## Compatibility

A program written against these extensions will not run on HE.
A program written against pure HE behavior will run on MeTTaTron.

The extensions form a strict superset of accepted programs;
observable result-multisets may differ on programs that exercise
non-determinism through arithmetic.

## Extensions

### Ext-1. `get-metatype` returns `Undefined` for empty input
**Spec:** [§12.10](../../../metta-specification/spec/12-stdlib-atoms.md) — `get-metatype` returns one of `Symbol | Variable | Expression | Grounded`.
**MeTTaTron:** When the argument evaluates to zero results (branch annihilation, `(empty)`, etc.), `get-metatype` returns the symbol `Undefined`. (NOT in HE's closed list.)
**Source:** `src/backend/eval/trampoline/eval_loop.rs:11057-11098` (the `if atom_results.is_empty()` branch).
**Motivation:** Lets deterministic-test predicates `(if (== (get-metatype $x) Undefined) ...)` work uniformly when `$x` was reached through a non-deterministic empty path.
**Tests:** `tests/mettatron_extensions_regression.rs::ext1_*`

### Ext-2. `get-metatype` of `Error` returns `Error`
**Spec:** §12.10 + [§C errors](../../../metta-specification/spec/C-errors.md).
**MeTTaTron:** `MettaValueInner::Error(..) → "Error"`.
**HE:** classifies `(Error ...)` as a grounded expression atom; returns `Expression`.
**Source:** `src/backend/eval/trampoline/eval_loop.rs:11077` (and spanned dispatch at `:11087`).
**Motivation:** Makes error introspection idiomatic: `(case (get-metatype $r) ((Error ...)))`. Without it, callers must structurally match `(Error $expr $msg)`.
**Tests:** `tests/mettatron_extensions_regression.rs::ext2_*`

### Ext-3. `Empty` sentinel branch annihilation in arithmetic and comparison
**Spec:** [§6.2 Empty](../../../metta-specification/spec/06-kernel-semantics.md), [§13.2 / §13.3](../../../metta-specification/spec/13-stdlib-arithmetic.md).
**MeTTaTron:** Inside the Cartesian-product loop, an `Empty` sentinel argument is silently skipped, contributing zero results to the cross-product. With no other args, the entire call yields `()` (zero results).
**HE:** has no `Empty` sentinel value at the runtime layer; `(empty)` produces zero results that the interpreter filters at result-collection — never propagated through grounded ops.
**Source:** `src/backend/grounded/arithmetic.rs` (8 sites) + `src/backend/grounded/comparison.rs` (2 sites). Search `is_empty()`.
**Motivation:** Belt-and-suspenders for cooperative branch cancellation (commit `8fd3a9a`). PLN constructs `Empty` sentinels in evaluation results that flow through arithmetic before reaching the result-collection filter.
**Tests:** `tests/mettatron_extensions_regression.rs::ext3_*`

### Ext-4. Unary `-` accepted as `(- 5) → -5`
**Spec:** §13.2 — `-` signature `(-> Number Number Number)`, arity 2.
**MeTTaTron:** `(- 5) → -5`, `(- 3.14) → -3.14`, `(- -7) → 7`. Two's-complement: `(- i64::MIN) → i64::MIN` (silent wrap).
**HE:** `MinusOp` is strictly binary; unary application is `IncorrectArgument` → `NotReducible`.
**Source:** `src/backend/grounded/arithmetic.rs:124-168` (the `state.step == 0 → 1 args → step = 10` arm and the `step = 10` evaluation block).
**Motivation:** Authored together with wrapping arithmetic (commit `8fd3a9a`); supports natural source like `(- (myFn))` without `(- 0 ...)` boilerplate.
**Tests:** `tests/mettatron_extensions_regression.rs::ext4_*`

### Ext-5. `<` / `<=` / `>=` / `>` accept strings (lexicographic fallback)
**Spec:** [§13.3 comparisons](../../../metta-specification/spec/13-stdlib-arithmetic.md) — signature `(-> Number Number Bool)`; non-`Number` argument is `IncorrectArgument`.
**MeTTaTron:** When both arguments are strings, compare lexicographically.
**HE:** rejects strings with `IncorrectArgument` → `NotReducible`.
**Source:** `src/backend/grounded/comparison.rs:191-200` (the `if let (Some(x), Some(y)) = (a.as_string(), b.as_string())` block).
**Motivation:** Convenience parity with the legacy interpreter (commit `3b9113c`). Allows simple string-keyed sorts.
**Tests:** `tests/mettatron_extensions_regression.rs::ext5_*`

### Ext-6. Cartesian product over non-deterministic arithmetic/comparison args
**Spec:** [§6.6 chain](../../../metta-specification/spec/06-kernel-semantics.md), §13.2 / §13.3 ("Non-determinism. Deterministic.").
**MeTTaTron:** When an argument evaluates to `n` results, the op produces an `n_a × n_b` Cartesian product. E.g., `(+ (superpose (1 2)) 10) → (11 12)`.
**HE:** Applies grounded ops to a single resolved atom; non-determinism is the caller's responsibility (typically lifted via `chain`).
**Source:** `src/backend/grounded/arithmetic.rs:73-103` (AddOp), recurring in SubOp/MulOp/DivOp/ModOp/Min/Max; `comparison.rs:174-209,263-275`.
**Motivation:** Co-introduced with wrapping arithmetic (commit `8fd3a9a`). Lets callers compose arithmetic with `superpose` without explicit `chain` boilerplate.
**Tests:** `tests/mettatron_extensions_regression.rs::ext6_*`

## Tier-specific divergence (informative)

The bytecode VM and JIT runtime paths at `src/backend/bytecode/jit/runtime/arithmetic.rs`, `…/handlers/arithmetic.rs`, and `…/handlers/comparison.rs` do NOT currently mirror Ext-3, Ext-5, or Ext-6 (no `is_empty()` guards, no string fallback, no Cartesian loop). When those paths encounter the relevant inputs, they bail with `VmError::TypeError` and fall through to the trampoline tier, where the extension behavior applies.

This is a **known internal divergence**; it is not part of the regression suite (which targets only the user-visible runner output and exercises whichever tier the runner picks).

## Maintenance contract

Any change to one of the source-code sites listed here MUST update this
file and the regression suite. Test names embed the extension identifier
so a future "fix to HE bisimilarity" PR cannot silently delete them —
deletion of a test or change of an `assert_eq!` in
`tests/mettatron_extensions_regression.rs` is a deliberate, intentional
removal of an extension and SHOULD trigger reviewer scrutiny.
