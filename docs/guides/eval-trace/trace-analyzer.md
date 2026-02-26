# trace-analyzer Tool Reference

**Source**: `tools/trace-analyzer/`

The trace-analyzer is a standalone CLI tool for reading and analyzing
binary `.mtrace` files produced by `mettatron --trace`.

## Building

```bash
cd tools/trace-analyzer
cargo build --release
```

The binary is at `tools/trace-analyzer/target/release/trace-analyzer`.

## Subcommands

### `dump` — Sequential Event Dump

Prints all events in the trace file, either in human-readable or JSON
format.

```bash
trace-analyzer dump <file> [--json] [--limit N]
```

**Options:**

| Flag | Description |
|------|-------------|
| `--json` | Output events as one JSON object per line (NDJSON) |
| `--limit N` | Stop after N events |

**Human-readable output format:**

```
[#0 T0 D0 TreeWalker]
  (! (double 21))
  EvalStart

[#1 T0 D0 TreeWalker]
  (! (double 21)) => [42]
  SpecialForm { form: "!", phase: "dispatch" }

[#2 T0 D1 TreeWalker 1:0-1:15 example.metta]
  (double 21) => [42]
  GroundedOp { op: "*", args: [2, 21] }
```

Each event block shows:
- **Header line**: `[#seq Tthread_id Ddepth Tier span file]`
- **Input/Output**: `input => [output1, output2, ...]`
- **Kind details**: Event-specific fields

**JSON output format:**

```bash
trace-analyzer dump /tmp/trace.mtrace --json --limit 3
```

Produces one JSON object per line (NDJSON), suitable for piping to `jq`:

```bash
trace-analyzer dump /tmp/trace.mtrace --json | jq '.kind'
```

### `stats` — Summary Statistics

Prints aggregate statistics about the trace.

```bash
trace-analyzer stats <file>
```

**Output sections:**

1. **Header**: Source file, MeTTaTron version, total event count, max
   eval depth, error count, bailout count, GC safepoint count

2. **Events by Tier**: Count and percentage for each `TraceTier`
   variant (TreeWalker, BytecodeVM, JitStage1, JitStage2)

3. **Events by Kind**: Count and percentage for each `TraceEventKind`
   variant, sorted by frequency (top 20 shown)

4. **Depth Histogram**: Bar chart showing event count at each
   trampoline depth

**Example output:**

```
=== MeTTaTron Trace Statistics ===

Source: verify_demo0.metta
Version: 0.2.0
Total events: 145832
Max eval depth: 44
Errors: 12
Bailouts: 0
GC safepoints: 3

--- Events by Tier ---
  TreeWalker       142891  (98.0%)
  BytecodeVM         2941  (2.0%)

--- Events by Kind ---
  SpecialForm                       45231  (31.0%)
  RuleMatchSet                      32100  (22.0%)
  GroundedOp                        28450  (19.5%)
  ApplicativePreEval                15320  (10.5%)
  EvalStart                          8200  (5.6%)
  EvalEnd                            8200  (5.6%)
  NondeterministicFork               4100  (2.8%)
  GroundedOpError                      12  (0.0%)
  GcSafepoint                          3  (0.0%)

--- Depth Histogram ---
  D0        16400  ████████████████████████████████████████
  D1        28930  ████████████████████████████████████████████████
  D2        34200  ████████████████████████████████████████████████████████
  D3        22100  ████████████████████████████████████████████
  ...
```

### `search` — Pattern-Based Event Filtering

Searches for events where a pattern string appears in the input value,
any output value, or event-kind-specific fields.

```bash
trace-analyzer search <file> <pattern>
```

The pattern is a substring match (case-sensitive) applied to:

- **Input/Output values**: Atom names, string contents (recursive into S-expressions)
- **Event kind fields**: `op_name` (GroundedOp, GroundedOpError), `form_name`
  (SpecialForm), `message` (ErrorCreated, GroundedOpError), `error_kind`
  (GroundedOpError), `reason` (JitBailout, BytecodeHalt)

**Example:**

```bash
# Find all events involving the "double" function
trace-analyzer search /tmp/trace.mtrace "double"

# Find all events involving arithmetic
trace-analyzer search /tmp/trace.mtrace "+"

# Find all type errors
trace-analyzer search /tmp/trace.mtrace "IncorrectArgument"
```

**Output format:**

```
Searching for pattern: "double"

[#3 T0 D1 TreeWalker]
  (double 21) => [(* 2 21)]

[#4 T0 D1 TreeWalker]
  (double 21) => [42]

Found 2 matching events
```

### `errors` — Error Event Listing

Lists all error-related events: `ErrorCreated`, `GroundedOpError`,
`ErrorCaught`, and `ErrorPropagated`.

```bash
trace-analyzer errors <file>
```

**Output format:**

```
=== Error Events ===

[#42 T0 D5 TreeWalker] ERROR CREATED
  message: "Math"
  details: (/ 1 0)

[#43 T0 D4 TreeWalker] GROUNDED OP ERROR
  op: "/"
  kind: "Arithmetic"
  message: "division by zero"
  args: [1, 0]

[#50 T0 D2 TreeWalker] ERROR CAUGHT
  error: (Error Math (/ 1 0))
  handler: "catch"
  default: 0

Total error events: 3
```

### `bailouts` — JIT/Bytecode Bailout Summary

Lists all JIT bailout and bytecode halt events, with a summary histogram
of bailout reasons.

```bash
trace-analyzer bailouts <file>
```

**Output format (when bailouts exist):**

```
=== Bailout Summary ===

--- Bailout Reasons ---
      12x  unsupported opcode
       3x  BytecodeHalt: complex expression

--- JIT Bailouts (12) ---
  [#1023] ip=42, reason: unsupported opcode, fallback: tree-walker
  [#2048] ip=42, reason: unsupported opcode, fallback: tree-walker
  ...

--- Bytecode Halts (3) ---
  [#500] ip=15, reason: "complex expression"
  ...

Total: 12 JIT bailouts, 3 bytecode halts
```

**Output format (no bailouts):**

```
=== Bailout Summary ===

No bailouts recorded.
```

## JSON Output for Programmatic Consumption

The `dump --json` subcommand produces NDJSON (Newline-Delimited JSON):

```bash
# Extract all grounded op names
trace-analyzer dump file.mtrace --json \
    | jq -r 'select(.kind.GroundedOp) | .kind.GroundedOp.op_name' \
    | sort | uniq -c | sort -rn

# Count events per tier
trace-analyzer dump file.mtrace --json \
    | jq '.tier' | sort | uniq -c

# Find events at depth > 10
trace-analyzer dump file.mtrace --json \
    | jq 'select(.depth > 10)'
```

The JSON schema matches the `TraceEvent` Rust struct directly, serialized
via `serde_json`. All enum variants are represented as objects with the
variant name as key:

```json
{
  "seq": 42,
  "thread_id": 0,
  "timestamp_ns": 123456789,
  "tier": "TreeWalker",
  "depth": 3,
  "input": {"SExpr": [{"Atom": "+"}, {"Long": 1}, {"Long": 2}]},
  "outputs": [{"Long": 3}],
  "expr_span": {"file_id": 0, "start_row": 1, "start_col": 0, "end_row": 1, "end_col": 7},
  "kind": {"GroundedOp": {"op_name": "+", "args": [{"Long": 1}, {"Long": 2}]}}
}
```
