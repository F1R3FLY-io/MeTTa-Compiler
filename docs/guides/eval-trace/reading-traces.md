# Understanding Trace Events

This guide explains how to read and interpret trace events produced by
the eval-trace system.

## Anatomy of a Trace Event

Every event has these fields:

```
[#seq Tthread_id Ddepth Tier span file]
  input => [output1, output2, ...]
  EventKind { field1: value1, ... }
```

| Field | Meaning |
|-------|---------|
| `#seq` | Per-thread monotonic sequence number. Events from the same thread have increasing seq. |
| `Tthread_id` | Thread identifier (0 = main thread, 1+ = parallel eval threads). |
| `Ddepth` | Trampoline nesting depth. D0 = top-level `!()` expression. |
| `Tier` | Which evaluation tier produced this event: TreeWalker, BytecodeVM, JitStage1, JitStage2. |
| `span file` | Source location (row:col-row:col) and filename, if available. |
| `input` | The expression before the rewrite. |
| `outputs` | The expression(s) after the rewrite. Empty for lifecycle events. |
| `EventKind` | What kind of rewrite or action occurred. |

## Understanding Depth

The depth field tracks the trampoline evaluation nesting:

- **D0**: Top-level expressions — the `!(expr)` force-evaluation
- **D1**: First level of nesting — the expression inside `!()`
- **D2**: Second level — e.g., arguments being evaluated
- **D3+**: Deeper nesting from nested function calls

Example for `!(double 21)` where `(= (double $x) (* 2 $x))`:

```
D0: !(double 21)              ← top-level force-eval
D1:   (double 21)             ← function call
D2:     (* 2 21)              ← rule body after substitution
D3:       grounded * 2 21     ← built-in multiplication
```

Higher-than-expected depth may indicate deep recursion or divergence.
Use `stats` to check the max depth.

## Following a Reduction Chain

To understand how an expression was evaluated, follow events with
increasing sequence numbers at increasing depth, then watch the depth
decrease as results propagate back up.

### Example: `!(if (> 3 2) (+ 1 2) (* 4 5))`

```
[#0 T0 D0 TreeWalker]
  (! (if (> 3 2) (+ 1 2) (* 4 5)))
  EvalStart

[#1 T0 D0 TreeWalker]
  (! (if (> 3 2) (+ 1 2) (* 4 5)))
  SpecialForm { form: "!", phase: "dispatch" }

[#2 T0 D1 TreeWalker]
  (if (> 3 2) (+ 1 2) (* 4 5))
  SpecialForm { form: "if", phase: "dispatch" }

[#3 T0 D2 TreeWalker]
  (> 3 2) => [True]
  GroundedOp { op: ">", args: [3, 2] }

[#4 T0 D1 TreeWalker]
  True => []
  SpecialForm { form: "if", phase: "condition-result" }

[#5 T0 D1 TreeWalker]
  True => [(+ 1 2)]
  SpecialForm { form: "if", phase: "then-branch" }

[#6 T0 D2 TreeWalker]
  (+ 1 2) => [3]
  GroundedOp { op: "+", args: [1, 2] }

[#7 T0 D0 TreeWalker]
  (! (if (> 3 2) (+ 1 2) (* 4 5))) => [3]
  EvalEnd { results: 1 }
```

Reading this trace:

1. **#0**: Evaluation starts for the force-eval expression
2. **#1**: The `!` special form is dispatched
3. **#2**: Inside the `!`, the `if` special form is dispatched
4. **#3**: The condition `(> 3 2)` is evaluated — produces `True`
5. **#4**: The condition result `True` is recorded (phase-progression event)
6. **#5**: The `if` form takes the then-branch because condition was `True`
7. **#6**: The then-branch `(+ 1 2)` evaluates to `3`
8. **#7**: Evaluation completes with result `[3]`

Note that `(* 4 5)` was never evaluated — lazy evaluation means the
else-branch is skipped when the condition is `True`.

## Understanding Tiers

Most events will be `TreeWalker` — this is the default tier for all
expressions. As expressions become hot (executed many times), the tiered
compilation system promotes them:

| Tier | When Used | Event Volume |
|------|-----------|-------------|
| TreeWalker | Default for all expressions | Highest (typically 90%+) |
| BytecodeVM | After threshold executions (e.g., 10+) | Moderate |
| JitStage1 | After higher threshold (e.g., 100+) | Low |
| JitStage2 | After very high threshold (e.g., 1000+) | Rare |

A `TierDispatch` event is emitted whenever an expression is dispatched
to a tier, showing the expression hash, selected tier, and execution count.

## Event Kind Categories

### Rule Application Events

```
RuleApplication {
    lhs: (double $x)
    rhs: (* 2 $x)
    bindings: { $x = 21 }
}
```

A user-defined rule `(= lhs rhs)` was applied. The `bindings` show how
pattern variables were bound. The `rule_span` (if present) identifies
where the rule was defined in source. Both the tree-walker and bytecode
VM emit this event — one `RuleApplication` per matching rule, immediately
after binding substitution and before the instantiated body is evaluated.

```
RuleMatchSet { match_count: 3 }
```

All matching rules were found for an expression. If `match_count > 1`,
nondeterministic evaluation will explore all branches.

### Grounded Operation Events

```
GroundedOp { op: "+", args: [1, 2] }
```

A built-in operation was executed successfully. The `args` are the
evaluated arguments passed to the operation.

### Special Form Events

```
SpecialForm { form: "if", phase: "dispatch" }
SpecialForm { form: "if", phase: "condition-result" }
SpecialForm { form: "if", phase: "then-branch" }
```

Special forms produce events at different phases of their evaluation.
Every special form emits a `"dispatch"` event when first recognized, then
one or more phase-progression events as the continuation handler processes
results.

#### Phase Reference Table

| `form_name` | Phase | When Emitted |
|------------|-------|-------------|
| `"if"` | `"dispatch"` | `if` expression recognized |
| `"if"` | `"condition-result"` | Condition evaluation completed |
| `"if"` | `"then-branch"` | Condition was `True` — then-branch selected |
| `"if"` | `"else-branch"` | Condition was `False` — else-branch selected |
| `"if"` | `"non-boolean"` | Condition was not boolean — unreduced |
| `"let"` | `"dispatch"` | `let` expression recognized |
| `"let"` | `"value-result"` | Value expression evaluation completed |
| `"let"` | `"pattern-match"` | Pattern matched a value |
| `"let"` | `"pattern-no-match"` | Pattern did not match a value |
| `"chain"` | `"dispatch"` | `chain` expression recognized |
| `"chain"` | `"expr-result"` | Chain expr evaluated to result(s) |
| `"chain"` | `"expr-empty"` | Chain expr evaluated to zero results |
| `"case"` | `"dispatch"` | `case` expression recognized |
| `"case"` | `"scrutinee-result"` | Scrutinee evaluation completed |
| `"case"` | `"case-match"` | Scrutinee matched a case pattern |
| `"case"` | `"case-no-match"` | No case pattern matched |
| `"match"` | `"dispatch"` | `match` expression recognized |
| `"match"` | `"space-result"` | Space query completed with template(s) |
| `"match-or"` | `"dispatch"` | `match-or` expression recognized |
| `"match-or"` | `"default-branch"` | No matches — default branch taken |
| `"if-reducible"` | `"dispatch"` | `if-reducible` expression recognized |
| `"if-reducible"` | `"reduced"` | Expression changed — then-branch taken |
| `"if-reducible"` | `"irreducible"` | Expression unchanged — else-branch taken |
| `"collapse"` | `"dispatch"` | `collapse` expression recognized |
| `"collapse"` | `"collapse-result"` | All results evaluated, tuple assembled |

### Error Events

```
ErrorCreated { msg: "Math", details: (/ 1 0) }
```

An error value was created. Track its propagation through subsequent
`ErrorPropagated` events, and its resolution through `ErrorCaught`.

```
GroundedOpError { op: "/", kind: "Arithmetic", msg: "division by zero" }
```

A grounded operation failed. The `error_kind` categorizes the failure:
- `"NoReduce"` — the operation cannot reduce the given arguments
- `"Runtime"` — a runtime error occurred
- `"Arithmetic"` — an arithmetic error (e.g., division by zero)
- `"IncorrectArgument"` — wrong argument type

### Nondeterministic Fork Events

```
NondeterministicFork { branches: 3 }
```

Multiple rules matched, creating nondeterministic branches. Each branch
is evaluated independently, and results are collected. In the bytecode
VM, you may also see `BranchStart` and `BranchEnd` events for individual
branches.

### Tier Dispatch Events

```
TierDispatch { hash: 0x1a2b3c4d5e6f7890, tier: BytecodeVM, exec_count: 15 }
```

An expression was dispatched to a specific tier based on its execution
count. The `hash` identifies the expression. Use this to understand tier
promotion behavior.

### JIT Bailout Events

```
JIT BAILOUT @ ip=42, reason: unsupported opcode, fallback: tree-walker
```

The JIT-compiled code could not handle the expression and fell back to
a lower tier. `ip` is the instruction pointer where the bailout occurred.
Frequent bailouts for the same reason may indicate a JIT coverage gap.

```
BytecodeHalt @ ip=15, reason: "complex expression"
```

The bytecode VM halted and fell back to the tree-walker. Similar to JIT
bailouts but at the bytecode level.

### GC Safepoint Events

```
GcSafepoint { roots: 42, alloc_delta: 104857600 bytes }
```

A garbage collection safepoint was reached. `roots` is the number of
live values registered as GC roots. `alloc_delta` is the allocation
pressure (bytes allocated since last safepoint check) that triggered
the safepoint.

### BranchPrune Events

```
BranchPrune { expected: Number, pruned: 2, surviving: 1 }
    pruned[0]: rhs_type=String
    pruned[1]: rhs_type=Bool
```

Type-driven branch pruning removed rule matches whose declared return
type (`rhs_type`) is incompatible with the caller's `expected_type`.
This event is emitted in the tree-walker's `EvalRuleMatchesLazy` handler
after rule matching but before rule application.

| Field | Meaning |
|-------|---------|
| `expected` | The type expected by the calling context (propagated via `expected_type`). |
| `pruned` | Number of matches removed. |
| `surviving` | Number of matches that passed the type filter. |
| `pruned[i]: rhs_type=...` | The declared return type of each pruned match. `None` means the match had no type annotation (these are never pruned — they pass conservatively). |

**Diagnostic use**: If a `RuleMatchSet` reports `match_count=N` but fewer
than `N` `RuleApplication` events follow, a `BranchPrune` event between
them explains how many matches were filtered by the type system and what
types they had. If there is no `BranchPrune` event, the loss occurred
elsewhere (e.g., pattern match failure during binding instantiation).

## Multi-Threaded Traces

When parallel evaluation is active, events from different threads are
interleaved. Use the `Tthread_id` field to separate per-thread reduction
chains:

```
[#0  T0 D0 TreeWalker]  ← Thread 0 starts eval of expr A
[#0  T1 D0 TreeWalker]  ← Thread 1 starts eval of expr B (seq resets per thread)
[#1  T0 D1 TreeWalker]  ← Thread 0 continues with expr A
[#1  T1 D1 TreeWalker]  ← Thread 1 continues with expr B
```

Within a single thread, `seq` values are strictly monotonic. Across
threads, events are interleaved in buffer-flush order.
