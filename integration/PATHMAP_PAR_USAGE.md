# PathMap Par Integration - Usage Guide

## Overview

The MeTTa compiler integration with Rholang now uses **PathMap Par** structures (specifically `EPathMap`) instead of JSON strings. This provides type-safe, efficient state management for MeTTa evaluation in Rholang contracts.

## System Processes

### `rho:metta:compile` - Compile MeTTa Source

**Channel:** 200
**Arity:** 2 (source code, return channel)
**Returns:** EPathMap Par containing MettaState

```rholang
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@statePar <- result) {
    // statePar is an EPathMap with structure:
    // - Element 0: ("pending_exprs", <list of expressions>)
    // - Element 1: ("environment", <metadata>)
    // - Element 2: ("eval_outputs", <empty list>)
    ...
  }
}
```

### `rho:metta:compile:sync` - Synchronous Compile

**Channel:** 201
**Arity:** 2 (source code, ack channel)
**Returns:** EPathMap Par containing MettaState

```rholang
new result in {
  @"rho:metta:compile:sync"!("(= (double $x) (* $x 2))", *result) |
  for (@statePar <- result) {
    // Same structure as compile
    ...
  }
}
```

### `rho:metta:run` - Evaluate MettaState

**Channel:** 202
**Arity:** 3 (accumulated_state, compiled_state, return channel)
**Returns:** EPathMap Par with updated accumulated state

```rholang
new result in {
  @"rho:metta:run"!(accumulatedState, compiledState, *result) |
  for (@newState <- result) {
    // newState contains:
    // - Empty pending_exprs (all evaluated)
    // - Updated environment (with new rules)
    // - Extended eval_outputs (accumulated results)
    ...
  }
}
```

## PathMap Par Structure

### EPathMap Format

```
EPathMap {
  ps: [
    ETuple("pending_exprs", EList([expr1, expr2, ...])),
    ETuple("environment", ETuple("environment", GInt(facts_count))),
    ETuple("eval_outputs", EList([result1, result2, ...]))
  ],
  locally_free: [],
  connective_used: false,
  remainder: None
}
```

### MettaValue to Par Mappings

| MettaValue Type | Par Representation |
|----------------|-------------------|
| `Atom("foo")` | `GString("atom:foo")` |
| `Bool(true)` | `GBool(true)` |
| `Long(42)` | `GInt(42)` |
| `String("hello")` | `GString("hello")` |
| `Uri("example")` | `GUri("example")` |
| `Nil` | Empty Par |
| `SExpr([...])` | `EListBody([...])` |
| `Error(msg, details)` | `ETuple("error", msg, details)` |
| `Type(inner)` | `ETuple("type", inner)` |

## Error Handling

When compilation or evaluation fails, an error Par is returned:

```rholang
new result in {
  @"rho:metta:compile"!("(+ 1 2", *result) |  // Syntax error
  for (@errorPar <- result) {
    // errorPar is ETuple("error", <error message>)
    ...
  }
}
```

## REPL Pattern

To implement a REPL-like workflow where state accumulates across evaluations:

```rholang
new mettaCompile(`rho:metta:compile`),
    mettaRun(`rho:metta:run`),
    stdoutAck(`rho:io:stdoutAck`),
    ack in {

  // Step 1: Start with empty state
  new state1, compiled1 in {
    mettaCompile!("(= (double $x) (* $x 2))", *compiled1) |
    for (@compiled <- compiled1) {
      // Step 2: Run against empty state (Nil)
      mettaRun!(Nil, compiled, *state1) |
      for (@accumulated1 <- state1) {
        stdoutAck!("Rule defined\n", *ack) |

        // Step 3: Use the rule
        for (_ <- ack) {
          new state2, compiled2 in {
            mettaCompile!("!(double 21)", *compiled2) |
            for (@compiled <- compiled2) {
              // Step 4: Run against accumulated state
              mettaRun!(accumulated1, compiled, *state2) |
              for (@accumulated2 <- state2) {
                stdoutAck!("Rule executed - result in eval_outputs\n", *ack)
              }
            }
          }
        }
      }
    }
  }
}
```

## Important Notes

### Printing PathMap Par Objects

PathMap Par objects are **stringifiable** and can be printed directly with `stdoutAck`:

**CORRECT - Direct printing:**
```rholang
for (@statePar <- result) {
  stdoutAck!("Compiled state: ", *ack) |
  for (_ <- ack) {
    stdoutAck!(statePar, *ack) |  // Prints the PathMap structure
    for (_ <- ack) {
      stdoutAck!("\n", *ack)
    }
  }
}
```

The output will show the PathMap structure in Rholang syntax: `{|...|}`

### ⚠️ Do NOT Use String Concatenation

**WRONG:**
```rholang
for (@statePar <- result) {
  stdoutAck!("Result: " ++ statePar ++ "\n", *ack)  // ERROR!
}
```

The `++` operator only works with strings, not PathMap Par objects. You must print them separately as shown above.

### Initial State for `mettaRun`

For the first `mettaRun` call (when there's no accumulated state), use `Nil`:

```rholang
mettaRun!(Nil, compiledState, *result)
```

For subsequent calls, pass the accumulated state from the previous run:

```rholang
mettaRun!(accumulatedState, compiledState, *result)
```

## Example: Complete Workflow

See `integration/test_metta_integration.rho` for a complete example demonstrating:

1. Basic compilation
2. Synchronous compilation
3. Error handling
4. Full REPL workflow with state accumulation

## Testing

Run the integration test:

```bash
cd /home/dylon/Workspace/f1r3fly.io/f1r3node
./target/release/rholang-cli /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration/test_metta_integration.rho
```

Or run the simple test:

```bash
./target/release/rholang-cli /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration/test_pathmap_simple.rho
```

## Migration from JSON

If you have existing code using the old JSON-based interface:

**Old (JSON):**
```rholang
for (@json <- result) {
  stdoutAck!("Result: " ++ json ++ "\n", *ack)
}
```

**New (PathMap Par):**
```rholang
for (@statePar <- result) {
  stdoutAck!("Received MettaState\n", *ack)
  // Use statePar with mettaRun or store it
}
```

The PathMap Par approach is more efficient and type-safe, allowing direct manipulation of MeTTa state without JSON serialization overhead.
