# Common Debugging Workflows

This guide covers practical debugging scenarios using the eval-trace system.

## Finding Why an Expression Returns Unexpected Results

**Symptom**: `!(my-function 5)` returns `(my-function 5)` instead of the
expected result.

**Workflow:**

1. Search for the function name:
   ```bash
   trace-analyzer search /tmp/trace.mtrace "my-function"
   ```

2. Look at the `RuleMatchSet` event for the expression:
   ```
   [#42 T0 D1 TreeWalker]
     (my-function 5)
     RuleMatchSet { match_count: 0 }
   ```

3. `match_count: 0` means no rules matched. This happens when:
   - The rule was never defined (missing `(= (my-function $x) ...)`)
   - The rule pattern doesn't match the arguments
   - The rule was defined in a module that wasn't imported

4. If `match_count > 0`, follow the subsequent events to see which
   rule was applied and how the result was computed.

## Finding Type Errors

**Symptom**: A grounded operation fails with a type error.

**Workflow:**

1. List all error events:
   ```bash
   trace-analyzer errors /tmp/trace.mtrace
   ```

2. Look for `GROUNDED OP ERROR` events with `kind: "IncorrectArgument"`:
   ```
   [#100 T0 D3 TreeWalker] GROUNDED OP ERROR
     op: "+"
     kind: "IncorrectArgument"
     message: "expected Number, got: foo"
     args: [foo, 42]
   ```

3. The `args` field shows what was actually passed. Trace back by
   finding events with lower sequence numbers at the same depth to
   understand how the incorrect argument was produced.

## Finding Infinite Loops / Divergence

**Symptom**: Evaluation hangs or takes extremely long.

**Workflow:**

1. Run with a timeout and capture whatever trace is written:
   ```bash
   timeout 10 ./target/release/mettatron --trace /tmp/trace.mtrace input.metta
   ```

2. Check the statistics:
   ```bash
   trace-analyzer stats /tmp/trace.mtrace
   ```

3. Look for:
   - **Unusually high max depth** (e.g., D1000+) — indicates deep or
     infinite recursion
   - **Very high event count at a single depth** — indicates a
     repeating loop at that depth

4. Search for the repeating expression:
   ```bash
   trace-analyzer dump /tmp/trace.mtrace --limit 1000 | tail -100
   ```

   Look for the same expression appearing repeatedly at increasing
   depth, e.g.:
   ```
   D10: (fact -1)
   D11: (fact -2)
   D12: (fact -3)
   ```

   This pattern indicates a recursive rule without a proper base case
   guard.

5. **Common cause**: Overlapping rules without guards. For example:
   ```metta
   (= (fact 0) 1)
   (= (fact $n) (* $n (fact (- $n 1))))
   ```

   Without a specificity filter, both rules match at `$n = 0`. The
   recursive rule computes `(fact -1)`, `(fact -2)`, etc. Fix by using
   a guarded rule:
   ```metta
   (= (fact $n) (if (== $n 0) 1 (* $n (fact (- $n 1)))))
   ```

## Finding JIT Bailouts and Performance Issues

**Symptom**: An expression runs slower than expected, or the `--tier-stats`
flag shows low JIT hit rates.

**Workflow:**

1. Check bailout summary:
   ```bash
   trace-analyzer bailouts /tmp/trace.mtrace
   ```

2. Review the reason histogram:
   ```
   --- Bailout Reasons ---
         45x  unsupported opcode
          3x  BytecodeHalt: complex expression
   ```

3. Correlate bailouts with expressions:
   ```bash
   trace-analyzer search /tmp/trace.mtrace "unsupported opcode"
   ```

4. If many bailouts occur for the same expression, the JIT may not
   support a pattern used in that expression. The tree-walker fallback
   handles it correctly but without JIT speedup.

## Finding GC Pressure

**Symptom**: High memory usage or frequent GC pauses.

**Workflow:**

1. Check safepoint statistics:
   ```bash
   trace-analyzer stats /tmp/trace.mtrace
   ```

   Look at `GC safepoints` count. More than a handful indicates
   significant allocation pressure.

2. Search for safepoint details:
   ```bash
   trace-analyzer search /tmp/trace.mtrace "GcSafepoint"
   ```

3. Examine each safepoint's `allocation_delta_bytes`:
   ```
   GcSafepoint { roots: 42, alloc_delta: 104857600 bytes }
   ```

   `104857600 bytes = 100 MB` — this safepoint was triggered by 100 MB
   of allocation since the last check.

4. Look at the surrounding events (same thread, nearby sequence numbers)
   to identify which expressions are allocating heavily.

## Finding Which Tier Executes a Function

**Symptom**: You want to know if a frequently-called function is being
JIT-compiled.

**Workflow:**

1. Search for the function:
   ```bash
   trace-analyzer search /tmp/trace.mtrace "my-hot-function"
   ```

2. Observe the `Tier` field in the event headers:
   ```
   [#100 T0 D1 TreeWalker]
     (my-hot-function 1) => [...]

   [#500 T0 D1 BytecodeVM]
     (my-hot-function 50) => [...]

   [#1000 T0 D1 JitStage1]
     (my-hot-function 100) => [...]
   ```

3. Look for `TierDispatch` events:
   ```bash
   trace-analyzer dump /tmp/trace.mtrace --json \
       | jq 'select(.kind.TierDispatch) | .kind.TierDispatch'
   ```

   This shows the expression hash, selected tier, and execution count
   at each tier transition.

## Following Error Propagation

**Symptom**: An error appears at the top level but you need to find
where it was originally created.

**Workflow:**

1. List all errors:
   ```bash
   trace-analyzer errors /tmp/trace.mtrace
   ```

2. Find the `ERROR CREATED` event (lowest sequence number for that
   error message):
   ```
   [#42 T0 D5 TreeWalker] ERROR CREATED
     message: "Math"
     details: (/ 1 0)
   ```

3. Follow `ERROR PROPAGATED` events (higher sequence numbers):
   ```
   [#43 T0 D4 TreeWalker] ERROR PROPAGATED
     error: (Error Math (/ 1 0))
     context: "map-atom"

   [#44 T0 D3 TreeWalker] ERROR PROPAGATED
     error: (Error Math (/ 1 0))
     context: "let"
   ```

4. Look for `ERROR CAUGHT`:
   ```
   [#50 T0 D2 TreeWalker] ERROR CAUGHT
     error: (Error Math (/ 1 0))
     handler: "catch"
     default: 0
   ```

   This shows the error was caught by a `catch` form with default
   value `0`.

## Tips

- **Start with `stats`**: Get an overview before diving into individual
  events. High error counts, unusual depths, or many bailouts guide
  where to look.

- **Use `--limit`**: Large traces can produce thousands of lines. Start
  with `dump --limit 100` to get a feel for the trace structure.

- **Use `--json` with `jq`**: For complex queries, pipe JSON output
  through `jq` for filtering, aggregation, and transformation.

- **Compare traces**: Run the same program with and without a change,
  then compare `stats` output to see if event counts, depths, or tier
  distributions changed.
