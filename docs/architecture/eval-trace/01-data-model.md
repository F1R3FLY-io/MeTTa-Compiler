# 01 — Trace Event Data Model

All types are defined in `trace-format/src/lib.rs` and derive
`serde::{Serialize, Deserialize}`. They are serialized with
[postcard](https://docs.rs/postcard) for compact binary encoding.

## Type Hierarchy

```
TraceEvent
├── seq: u64                  Monotonic sequence number (per-thread)
├── thread_id: u32            Thread that produced this event
├── timestamp_ns: u64         Nanoseconds since trace start
├── tier: TraceTier           Which evaluation tier produced the event
├── depth: u32                Trampoline evaluation depth
├── input: TraceValue         Expression BEFORE rewrite
├── outputs: Vec<TraceValue>  Expression(s) AFTER rewrite
├── expr_span: Option<TraceSpan>  Source location of the expression
└── kind: TraceEventKind      What performed the rewrite
```

## TraceEvent

**Source**: `trace-format/src/lib.rs:279`

The fundamental unit of the trace log. Each event captures a single
evaluation step: the input expression, the output expression(s) after
rewriting, and metadata about what kind of rewrite occurred.

```rust
pub struct TraceEvent {
    pub seq: u64,
    pub thread_id: u32,
    pub timestamp_ns: u64,
    pub tier: TraceTier,
    pub depth: u32,
    pub input: TraceValue,
    pub outputs: Vec<TraceValue>,
    pub expr_span: Option<TraceSpan>,
    pub kind: TraceEventKind,
}
```

| Field | Description |
|-------|-------------|
| `seq` | Per-thread monotonic sequence number. Unique within a thread. |
| `thread_id` | Assigned by `TraceCollector::next_thread_id` on first event from each thread. |
| `timestamp_ns` | Nanoseconds elapsed since the `TraceCollector` was created (`Instant::now()`). |
| `tier` | Which evaluation tier produced this event. |
| `depth` | Current trampoline nesting depth. `0` = top-level `!()` expression. |
| `input` | The expression that was being rewritten. |
| `outputs` | The result(s) of the rewrite. Empty for lifecycle events like `EvalStart`. |
| `expr_span` | Source location of the expression, if available. |
| `kind` | Discriminated union describing the specific rewrite action. |

## TraceValue

**Source**: `trace-format/src/lib.rs:46`

A fully-owned deep copy of a `MettaValue`. Contains no slab references or
lifetimes. Created at trace-emission time so that the GC can freely reclaim
the original `MettaValue` afterward.

```rust
pub enum TraceValue {
    Atom(String),
    Bool(bool),
    Long(i64),
    Float(f64),
    String(String),
    SExpr(Vec<TraceValue>),
    Unit,
    Error(String, Box<TraceValue>),
    Type(Box<TraceValue>),
    Empty,
    Quoted(Box<TraceValue>),
}
```

| Variant | Maps to `MettaValueInner` | Description |
|---------|--------------------------|-------------|
| `Atom(String)` | `Atom(&'static str)` | Named atom (cloned from slab-interned string) |
| `Bool(bool)` | `Bool(bool)` | Boolean value (`True` / `False`) |
| `Long(i64)` | `Long(i64)` | 64-bit integer |
| `Float(f64)` | `Float(f64)` | 64-bit floating point |
| `String(String)` | `String(&'static str)` | String literal (cloned from slab) |
| `SExpr(Vec<TraceValue>)` | `SExpr(&'static [MettaValue])` | S-expression (recursive, deep-copied) |
| `Unit` | `Unit` | Unit value `()` |
| `Error(msg, details)` | `Error(&'static str, MettaValue)` | Error with message and details |
| `Type(inner)` | `Type(MettaValue)` | Type wrapper |
| `Empty` | `Empty` | Zero-result value (`%void%`) |
| `Quoted(inner)` | `Quoted(MettaValue)` | Quoted (unevaluated) expression |

### Special mappings for non-serializable types

| `MettaValueInner` variant | `TraceValue` representation |
|--------------------------|----------------------------|
| `Space(_)` | `Atom("<space>")` |
| `State(id)` | `Atom("<state:{id}>")` |
| `Memo(_)` | `Atom("<memo>")` |
| `Conjunction(items)` | `SExpr([ Atom(","), ...items ])` |
| `Spanned(v, _)` | Recursively unwrapped to inner value |

### Display format

`TraceValue` implements `Display` with MeTTa-like syntax:

- `Atom("foo")` → `foo`
- `Bool(true)` → `True`
- `Long(42)` → `42`
- `String("hi")` → `"hi"`
- `Unit` → `()`
- `Empty` → `%void%`
- `SExpr([Atom("+"), Long(1), Long(2)])` → `(+ 1 2)`
- `Error("oops", Unit)` → `(Error oops ())`
- `Quoted(Atom("x"))` → `(quote x)`

## TraceTier

**Source**: `trace-format/src/lib.rs:94`

Discriminates which evaluation tier produced an event. Stored as `repr(u8)`
for compact serialization.

```rust
#[repr(u8)]
pub enum TraceTier {
    TreeWalker  = 0,
    BytecodeVM  = 1,
    JitStage1   = 2,
    JitStage2   = 3,
}
```

| Variant | Description |
|---------|-------------|
| `TreeWalker` | The trampoline-based tree-walking evaluator. Produces the majority of events. |
| `BytecodeVM` | The `GenericBytecodeVM` opcode interpreter. Hot-path expressions. |
| `JitStage1` | First JIT compilation stage (basic native code). Very hot expressions. |
| `JitStage2` | Second JIT compilation stage (optimized native code). Extremely hot expressions. |

## TraceEventKind

**Source**: `trace-format/src/lib.rs:116`

Discriminated union of 23 event kind variants, organized into 9 categories.

### Rule Application

| Variant | Fields | Description |
|---------|--------|-------------|
| `RuleApplication` | `rule_lhs`, `rule_rhs`, `bindings`, `rule_span` | A user-defined rule `(= lhs rhs)` was applied with the given variable bindings. |
| `RuleMatchSet` | `match_count`, `matches` | All matching rules found for an expression. Each entry is `(rule_lhs, optional_span)`. |

### Grounded Operations

| Variant | Fields | Description |
|---------|--------|-------------|
| `GroundedOp` | `op_name`, `args` | A built-in operation (`+`, `-`, `*`, `/`, `<`, `==`, etc.) was executed successfully. |

### Special Forms

| Variant | Fields | Description |
|---------|--------|-------------|
| `SpecialForm` | `form_name`, `phase` | A special form (`if`, `let`, `chain`, `match`, `eval`, `quote`, etc.) was dispatched or progressed. `phase` distinguishes sub-steps (e.g., `"dispatch"`, `"condition-eval"`, `"then-branch"`). |

### Pattern Matching

| Variant | Fields | Description |
|---------|--------|-------------|
| `PatternMatch` | `pattern`, `value`, `success`, `bindings` | A pattern match was attempted against a value, recording whether it succeeded and the resulting bindings. |

### Type System

| Variant | Fields | Description |
|---------|--------|-------------|
| `TypeOperation` | `op`, `subject`, `result_type` | A type system operation (`"get-type"`, `"check-type"`, `"infer"`, `"validate-grounded-arg"`). |
| `ApplicativePreEval` | `operator`, `arg_indices`, `source` | Arguments were pre-evaluated before rule matching. `source` is `"type-driven"` or `"bloom-filter"`. |
| `BranchPrune` | `expected_type`, `pruned_count`, `surviving_count`, `pruned_types` | Type-driven branch pruning after rule matching. `pruned_types` records the rhs_type of each pruned match (`None` if the match had no rhs_type annotation). |

### Error/Exception Handling

| Variant | Fields | Description |
|---------|--------|-------------|
| `ErrorCreated` | `message`, `details` | An error value was created. |
| `ErrorCaught` | `error`, `handler`, `default_used` | An error was caught. `handler` identifies the mechanism (`"catch"`, `"is-error check"`, `"grounded-op fallback"`). |
| `ErrorPropagated` | `error`, `context` | An error propagated through a higher-order operation (`"map-atom"`, `"filter-atom"`, `"foldl-atom"`, etc.). |
| `GroundedOpError` | `op_name`, `error_kind`, `message`, `args` | A grounded operation failed. `error_kind` is `"NoReduce"`, `"Runtime"`, `"Arithmetic"`, or `"IncorrectArgument"`. |

### Nondeterminism

| Variant | Fields | Description |
|---------|--------|-------------|
| `NondeterministicFork` | `branch_count` | A nondeterministic fork occurred (multiple rule matches or superpose). |
| `BranchStart` | `branch_index`, `total_branches` | A nondeterministic branch started evaluation. |
| `BranchEnd` | `branch_index`, `result_count` | A nondeterministic branch completed with the given number of results. |

### Tier Transitions

| Variant | Fields | Description |
|---------|--------|-------------|
| `TierDispatch` | `expression_hash`, `selected_tier`, `execution_count` | An expression was dispatched to a specific evaluation tier based on its execution count. |
| `BytecodeCompilation` | `expression_hash`, `execution_count` | A bytecode chunk was compiled for an expression. |
| `JitCompilation` | `expression_hash`, `stage`, `execution_count` | A JIT compilation was triggered (`stage` = 1 or 2). |

### JIT Bailout

| Variant | Fields | Description |
|---------|--------|-------------|
| `JitBailout` | `bailout_ip`, `reason`, `fallback_tier` | JIT execution bailed out. `fallback_tier` is `"bytecode"` or `"tree-walker"`. |
| `BytecodeHalt` | `ip`, `reason` | Bytecode VM halted and fell back to tree-walker. |

### Evaluation Lifecycle

| Variant | Fields | Description |
|---------|--------|-------------|
| `EvalStart` | — | Top-level evaluation started. |
| `EvalEnd` | `result_count` | Top-level evaluation completed with the given number of results. |

### GC

| Variant | Fields | Description |
|---------|--------|-------------|
| `GcSafepoint` | `root_count`, `allocation_delta_bytes` | A GC safepoint was reached during evaluation. |

## TraceSpan

**Source**: `trace-format/src/lib.rs:30`

Compact source location for serialization. Uses a `u16` file ID that
indexes into the file table (written in the trace file footer).

```rust
pub struct TraceSpan {
    pub file_id: u16,
    pub start_row: u32,
    pub start_col: u32,
    pub end_row: u32,
    pub end_col: u32,
}
```

The `file_id` is assigned by the `FileTable` in `convert.rs`. At
trace-read time, the `TraceReader` resolves `file_id` to a path string
via the footer's file table.

## TraceHeader

**Source**: `trace-format/src/lib.rs:302`

Written once at the start of the trace file. Contains metadata about the
evaluation session.

```rust
pub struct TraceHeader {
    pub source_file: String,
    pub start_time_ns: u64,
    pub mettatron_version: String,
    pub cpu_count: u32,
    pub file_table: Vec<String>,
}
```

| Field | Description |
|-------|-------------|
| `source_file` | Path to the `.metta` file being evaluated. |
| `start_time_ns` | Wall-clock nanoseconds since Unix epoch at trace start. |
| `mettatron_version` | MeTTaTron version string (from `CARGO_PKG_VERSION`). |
| `cpu_count` | Number of logical CPUs (`num_cpus::get()`). |
| `file_table` | Empty in header (populated in footer for streaming writes). |
