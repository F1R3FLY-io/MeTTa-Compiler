# Guide: Understanding eval_outputs in MeTTa State PathMaps

## Overview

When you run MeTTa code via the `rho:metta:compile` system process and the `.run()` method, you receive a PathMap structure representing the MeTTa state:

```rholang
{|
  ("pending_exprs", []),
  ("environment", ({|...|}, [])),
  ("eval_outputs", [results])
|}
```

The `eval_outputs` field contains the actual results of evaluating MeTTa expressions.

## The Challenge

**Rholang does not currently support PathMap field extraction** via pattern matching or accessor methods. This means you cannot programmatically extract just the `eval_outputs` field in Rholang code.

## Current Solution: Visual Inspection

The full PathMap is printed with all fields visible. You can identify your results by looking for the `("eval_outputs", [...])` tuple.

### Example Outputs

**Boolean result:**
```
{|("pending_exprs", []), ("environment", ({|...|}, [])), ("eval_outputs", [true])|}
```
Answer: `true`

**String result:**
```
{|("pending_exprs", []), ("environment", ({|...|}, [])), ("eval_outputs", ["room_a"])|}
```
Answer: `"room_a"`

**Number result:**
```
{|("pending_exprs", []), ("environment", ({|...|}, [])), ("eval_outputs", [2])|}
```
Answer: `2`

**S-expression result:**
```
{|("pending_exprs", []), ("environment", ({|...|}, [])), ("eval_outputs", [("navigate", "room_a", "pickup", "box1", "navigate", "room_d", "putdown")])|}
```
Answer: `("navigate", "room_a", "pickup", "box1", "navigate", "room_d", "putdown")`

**Multiple results:**
```
{|("pending_exprs", []), ("environment", ({|...|}, [])), ("eval_outputs", [1, 2, 3])|}
```
Answers: `1`, `2`, `3`

## Why Pattern Matching Doesn't Work

Rholang's pattern matching has limitations for PathMaps. These patterns **do not work**:

```rholang
// DOES NOT WORK
match state {
  {| ("eval_outputs", outputs) |} => { ... }
}

// DOES NOT WORK
match state {
  {| (_, _), (_, _), ("eval_outputs", outputs) |} => { ... }
}

// DOES NOT WORK
state.get("eval_outputs")
```

These are current language limitations in Rholang.

## Future Solutions

### Option 1: Modify Rust Integration

Modify `src/pathmap_par_integration.rs` to return only `eval_outputs` instead of the full state:

```rust
// Instead of returning full PathMap with all three fields,
// return just the eval_outputs as a Rholang list

pub fn metta_to_pathmap_par(state: &MettaState) -> Result<Par, String> {
    // Only serialize eval_outputs
    let outputs_par = state.eval_outputs
        .iter()
        .map(|v| metta_value_to_par(v))
        .collect();

    Ok(Par::default().with_exprs(vec![Expr {
        expr_instance: Some(ExprInstance::EListBody(EList {
            ps: outputs_par,
            ...
        })),
    }]))
}
```

This would change the return type from PathMap to List, making results directly usable.

### Option 2: Add System Process

Create a new Rholang system process `rho:metta:get_outputs`:

```rholang
new mettaGetOutputs(`rho:metta:get_outputs`) in {
  // Takes a state PathMap, returns just eval_outputs
  mettaGetOutputs!(state, *outputs) |
  for (@evalOutputs <- outputs) {
    // evalOutputs is now just [results], not full PathMap
    stdoutAck!(evalOutputs, *ack)
  }
}
```

This would require implementation in the Rholang runtime.

### Option 3: PathMap Methods

Add accessor methods to PathMap Par objects:

```rholang
// If PathMap supported .get() method
state.get("eval_outputs", *outputs) |
for (@evalOutputs <- outputs) {
  // Use evalOutputs
}
```

This would require changes to the Rholang language implementation.

## Using robot_planning.rho

The `robot_planning.rho` example includes guidance text in all output:

```rholang
stdoutAck!("  Result (look for eval_outputs field): ", *ack) |
for (_ <- ack) {
  stdoutAck!(res, *ack)
}
```

This reminds you to look for the `("eval_outputs", [...])` tuple in the output.

## Recommendations

### For Development/Testing

Read the `eval_outputs` field visually from the printed PathMap. The structure is always:
```
{|..., ("eval_outputs", [your_results])|}
```

### For Production Use

Consider implementing Option 1 (Modify Rust Integration) to return only `eval_outputs`:

**Pros:**
- Simple to implement
- No Rholang language changes needed
- Direct access to results
- Smaller data transfer

**Cons:**
- Loses access to `environment` and `pending_exprs` (usually not needed)
- Breaking change for existing code

**Implementation steps:**
1. Modify `metta_to_pathmap_par()` in `src/pathmap_par_integration.rs`
2. Return `EListBody` instead of `EPathMap`
3. Rebuild and test

This change would make results directly usable:
```rholang
for (@results <- compiled) {
  // results is now [1, 2, 3] instead of full PathMap
  stdoutAck!(results, *ack)
}
```

## See Also

- `EXTRACT_OUTPUTS_SOLUTION.md` - Complete explanation of the limitation and solutions
- `QUICK_REFERENCE_EXTRACTION.md` - Quick reference for reading results
- `robot_planning.rho` - Example usage with guidance text
- `PATHMAP_PAR_USAGE.md` - PathMap Par integration documentation
