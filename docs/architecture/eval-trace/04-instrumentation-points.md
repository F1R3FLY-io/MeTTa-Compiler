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
| 1312 | `EvalEnd` | Top-level evaluation completed. Emitted after the trampoline loop exits with the final result count. |

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
RuleApplication                     ●
NondeterministicFork                ●
TierDispatch            ●
BytecodeHalt                        ●
JitBailout                                      ●
GcSafepoint             ●

PatternMatch            (reserved — not currently emitted)
TypeOperation           (reserved — not currently emitted)
BranchPrune             (reserved — not currently emitted)
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
