# Quick Reference: Reading eval_outputs from PathMap

## The PathMap Structure

MeTTa state is always returned as:
```rholang
{|("pending_exprs", []), ("environment", ...), ("eval_outputs", [results])|}
```

**Look for the `("eval_outputs", [...])` tuple to find your results.**

## Reading Results

| Output | Result |
|--------|--------|
| `("eval_outputs", [true])` | `true` |
| `("eval_outputs", ["room_a"])` | `"room_a"` |
| `("eval_outputs", [2])` | `2` |
| `("eval_outputs", [(navigate ...)])` | `(navigate ...)` |
| `("eval_outputs", [1, 2, 3])` | `1`, `2`, `3` |

## Why Not Extract Programmatically?

**Rholang does not support PathMap field extraction.**

These don't work:
```rholang
// ❌ Pattern matching doesn't work
match state { {| ("eval_outputs", outputs) |} => { ... } }

// ❌ No .get() method
state.get("eval_outputs")
```

## Current Approach

Print the full PathMap and visually find the `eval_outputs` field:

```rholang
for (@state <- result) {
  stdoutAck!("Result: ", *ack) |
  for (_ <- ack) {
    stdoutAck!(state, *ack)  // Full PathMap printed
  }
}
```

Output shows:
```
Result: {|("pending_exprs", []), ("environment", ...), ("eval_outputs", [true])|}
                                                          ^^^^^^^^^^^^^^^^
                                                          Your answer!
```

## Future Solution

**Option: Modify Rust integration** to return only `eval_outputs` as a list instead of full PathMap.

See `EXTRACT_OUTPUTS_GUIDE.md` for implementation details.

## See Also

- `EXTRACT_OUTPUTS_GUIDE.md` - Full explanation and solutions
- `EXTRACT_OUTPUTS_SOLUTION.md` - Problem analysis
- `robot_planning.rho` - Example with guidance text
