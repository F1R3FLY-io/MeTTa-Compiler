# 04 — Instrumentation Points

This chapter catalogs every location in the evaluator that emits trace
events, organized by evaluation tier.

## Tree-Walker Instrumentation

The tree-walker is the most instrumented tier because it is the primary
evaluation path and handles the full MeTTa semantics.

### Trampoline Engine (`src/backend/eval/trampoline/generic_trampoline.rs`)

| Line | Event Kind | Description |
|------|-----------|-------------|
| 91 | `EvalStart` | Top-level evaluation started. Emitted once before the trampoline loop begins. |
| 152–156 | `GcSafepoint` | GC safepoint reached during evaluation. Emitted after `perform_safepoint()` with root count and allocation delta. |
| 253 | `GroundedOp` | Grounded operation succeeded. Emitted when a `ProcessGroundedArgs` continuation completes with `GenericGroundedWork::Results`. |
| 301 | `GroundedOpError` | Grounded operation failed. Emitted when a `ProcessGroundedArgs` continuation completes with `GenericGroundedWork::Error`. Error kind is extracted from the error type (`"NoReduce"`, `"Runtime"`, `"Arithmetic"`, `"IncorrectArgument"`). |
| 418 | `BranchPrune` | Type-driven branch pruning. Emitted after `matches.retain()` in `EvalRuleMatchesLazy` when at least one match was pruned due to rhs_type incompatibility with expected_type. Records pruned count, surviving count, and the rhs_type of each pruned match. |
| 480 | `RuleApplication` | Rule applied (first match). Emitted after `apply_bindings_generic` in `EvalRuleMatchesLazy`, recording the RHS template, instantiated body, and variable bindings. |
| 1494 | `RuleApplication` | Rule applied (subsequent match). Same event emitted in the `ProcessRuleMatches` continuation for second and later matches. |
| 1336 | `EvalEnd` | Top-level evaluation completed. Emitted after the trampoline loop exits with the final result count. |

### Phase-Progression Events (`src/backend/eval/trampoline/generic_trampoline.rs`)

Branching continuation handlers emit phase-progression events that record
control-flow decisions after the initial `"dispatch"` event. All use
`SpecialForm { form_name, phase }` with `TraceTier::TreeWalker`.

| Continuation | Phase String | Description |
|-------------|-------------|-------------|
| `ProcessIfCondition` | `"condition-result"` | Emitted after condition evaluation completes, before branch decision. Input: evaluated condition value. |
| `ProcessIfCondition` | `"then-branch"` | Condition was `Bool(true)` — then-branch selected. Output: the then-branch expression. |
| `ProcessIfCondition` | `"else-branch"` | Condition was `Bool(false)` — else-branch selected. Output: the else-branch expression. |
| `ProcessIfCondition` | `"non-boolean"` | Condition was not a boolean — returns unreduced `(if cond then else)`. |
| `ProcessLet` | `"value-result"` | Value expression evaluation completed. Input: pattern. Outputs: evaluated value(s). |
| `ProcessLet` | `"pattern-match"` | Pattern matched a value — body will be evaluated. Input: pattern. Output: matched value. |
| `ProcessLet` | `"pattern-no-match"` | Pattern did not match a value — skipping. Input: pattern. |
| `ProcessChainExpr` | `"expr-result"` | Chain expression evaluation completed with result(s). Input: chain variable. Outputs: result(s). |
| `ProcessChainExpr` | `"expr-empty"` | Chain expression evaluation produced zero results (branch annihilation). Input: chain variable. |
| `ProcessCaseAtom` | `"scrutinee-result"` | Scrutinee evaluation completed. Input: cases. Outputs: evaluated scrutinee value(s). |
| `ProcessCaseEvalScrutineeResults` | `"case-match"` | A scrutinee value matched a case pattern. Input: scrutinee value. Output: matched template. |
| `ProcessCaseEvalScrutineeResults` | `"case-no-match"` | No case pattern matched the scrutinee value. Input: scrutinee value. |
| `ProcessMatchSpace` | `"space-result"` | Space query completed. Input: match pattern. Outputs: instantiated template(s). |
| `ProcessMatchOrSpace` | `"default-branch"` | No space matches found — default branch taken. Input: match pattern. Output: default expression. |
| `ProcessIfReducible` | `"reduced"` | Expression changed after evaluation — then-branch taken. Input: original expression. Outputs: eval result(s). |
| `ProcessIfReducible` | `"irreducible"` | Expression unchanged after evaluation — else-branch taken. Input: original expression. Outputs: eval result(s). |
| `ProcessCollapseEvalResults` | `"collapse-result"` | All nondeterministic results evaluated and assembled into final tuple. Input: assembled tuple. |

### S-Expression Step (`src/backend/eval/step/generic_sexpr.rs`)

| Line | Event Kind | Description |
|------|-----------|-------------|
| 84 | `SpecialForm` | Special form dispatch. Emitted when the head of an S-expression matches a special form (`if`, `let`, `chain`, `match`, `eval`, `quote`, `case`, `switch`, `superpose`, `collapse`, etc.). Phase is `"dispatch"`. |
| 1562 | `ApplicativePreEval` | Type-driven applicative pre-evaluation. Emitted when the type system identifies argument indices that should be pre-evaluated before rule matching. Source is `"type-driven"`. |
| 1594 | `ApplicativePreEval` | Bloom-filter applicative pre-evaluation. Emitted when the bloom filter identifies argument positions whose head atom has user-defined rules. Source is `"bloom-filter"`. |
| 1638 | `RuleMatchSet` | All matching rules found. Emitted after rule matching completes, recording the match count and each rule's LHS and definition span. |

### Tier Dispatch (`src/backend/eval/mod.rs`)

| Line | Event Kind | Description |
|------|-----------|-------------|
| 178 | `TierDispatch` | Expression dispatched to JIT Stage 2. Emitted when a JIT Stage 2 compiled function is ready and selected. |
| 204 | `TierDispatch` | Expression dispatched to JIT Stage 1. Emitted when a JIT Stage 1 compiled function is ready and selected. |
| (bytecode path) | `TierDispatch` | Expression dispatched to Bytecode VM. Emitted when a bytecode chunk is ready and selected. |
| (tree-walker path) | `TierDispatch` | Expression dispatched to Tree-Walker. Emitted as fallback when no compiled tier is available. |

All `TierDispatch` events include the expression hash, selected tier, and
execution count.

## Bytecode VM Instrumentation

The bytecode VM emits events from within opcode handlers, using the
thread-local trace collector pattern.

### VM Opcode Handlers (`src/backend/bytecode/vm/mod.rs`)

| Line | Event Kind | Description |
|------|-----------|-------------|
| 1236 | `BytecodeHalt` | VM `Halt` opcode executed. Emitted when the VM halts and falls back to the tree-walker. Includes the instruction pointer and reason. |
| 2775 | `NondeterministicFork` | Fork opcode executed. Emitted when the VM encounters multiple alternatives after rule dispatch. Records the branch count. |
| 3035 | `GroundedOp` | Native function call succeeded. Emitted when `CallNative` opcode completes successfully. Records op name and arguments. |
| 3075 | `GroundedOpError` | Native function call failed. Emitted when `CallNative` opcode returns an error. Records op name, error kind, and message. |
| 3311 | `RuleMatchSet` | Rule dispatch completed. Emitted after the VM's rule matching phase. Records match count. |
| 3341 | `RuleApplication` | Single rule match applied. Emitted when exactly one rule matches and is applied. Records LHS, RHS, and bindings. |
| 3392 | `NondeterministicFork` | Multiple rule matches. Emitted when multiple rules match, creating nondeterministic branches. |

All bytecode VM events use `TraceTier::BytecodeVM` and access the trace
collector via:

```rust
#[cfg(feature = "eval-trace")]
{
    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
    with_thread_trace_collector(|tc| {
        tc.emit_converted(
            trace_format::TraceTier::BytecodeVM, 0,
            /* ... */
        );
    });
}
```

## JIT Instrumentation

The JIT tier emits events only at bailout points — when JIT execution
cannot complete and must fall back to a lower tier.

### JIT Arena (`src/backend/bytecode/jit/hybrid/arena.rs`)

| Line | Event Kind | Description |
|------|-----------|-------------|
| 158 | `JitBailout` | First bailout point in `execute_jit_arena`. Emitted when JIT execution bails out. Records the bailout instruction pointer, reason, and fallback tier (`"tree-walker"`). Uses `TraceTier::JitStage1`. |
| 324 | `JitBailout` | Second bailout point in `execute_jit_arena_with_env`. Same as above but in the environment-aware execution path. |

## Event Kind → Tier Emission Matrix

This matrix shows which `TraceEventKind` variants can be emitted by which
tier:

```
                        TreeWalker  BytecodeVM  JIT
                        ──────────  ──────────  ───
EvalStart               ●
EvalEnd                 ●
GroundedOp              ●           ●
GroundedOpError         ●           ●
SpecialForm             ●
ApplicativePreEval      ●
RuleMatchSet            ●           ●
RuleApplication         ●           ●
NondeterministicFork                ●
TierDispatch            ●
BytecodeHalt                        ●
JitBailout                                      ●
BranchPrune             ●
GcSafepoint             ●

PatternMatch            (reserved — not currently emitted)
TypeOperation           (reserved — not currently emitted)
ErrorCreated            (reserved — not currently emitted)
ErrorCaught             (reserved — not currently emitted)
ErrorPropagated         (reserved — not currently emitted)
BranchStart             (reserved — not currently emitted)
BranchEnd               (reserved — not currently emitted)
BytecodeCompilation     (reserved — not currently emitted)
JitCompilation          (reserved — not currently emitted)
```

The "reserved" variants have their data model defined in `TraceEventKind`
but are not yet emitted by any instrumentation point. They exist to
support future instrumentation expansion without a format version bump.
