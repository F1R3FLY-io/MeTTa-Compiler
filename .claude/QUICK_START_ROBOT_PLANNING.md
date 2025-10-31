# Robot Planning - Quick Start Guide

## Run the Working Demo

```bash
# 1. Build the MeTTa compiler (if not already built)
cd /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
RUSTFLAGS="-C target-cpu=native" cargo build --release

# 2. Test MeTTa queries directly
./target/release/mettatron examples/robot_planning_fixed.metta

# 3. Run full Rholang integration
/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli \
  examples/robot_planning_working.rho
```

## Expected Output

### MeTTa Direct (`robot_planning_fixed.metta`)
```
[room_a]
[true]
[2]
[true]
```

### Rholang Integration (`robot_planning_working.rho`)
```
=== Robot Planning System - Working Demo ===

Demo 1: Are room_a and room_b connected?
  Result: {..., ("eval_outputs", [true])}

Demo 2: Where is box1 located?
  Location: {..., ("eval_outputs", ["atom:room_a"])}

Demo 3: Can robot reach room_c from room_a?
  Reachable: {..., ("eval_outputs", [true])}

Demo 4: Distance from room_a to room_d?
  Distance: {..., ("eval_outputs", [2])} steps

=== All Tests Passed! ===
```

## File Guide

| File | Purpose | Working? |
|------|---------|----------|
| `robot_planning_fixed.metta` | Simplified MeTTa KB | ✅ Yes |
| `robot_planning_working.rho` | Working Rholang contracts | ✅ Yes |
| `robot_planning_simple.metta` | Unit test | ✅ Yes |
| `robot_planning_test.rho` | Integration test | ✅ Yes |
| `robot_planning.metta` | Advanced (Prolog-style) | ⚠️ Educational |
| `robot_planning.rho` | Full contract API | ⚠️ Educational |

## Quick Examples

### MeTTa: Define a Room
```lisp
(= (connected room_x room_y) true)
```

### MeTTa: Query Connection
```lisp
!(connected room_a room_b)
```

### Rholang: Use Contract
```rholang
new result in {
  robotAPI!("locate", "box1", *result) |
  for (@location <- result) {
    stdoutAck!(location, *ack)
  }
}
```

## Environment

```
    room_a ---- room_b ---- room_c
      |                       |
      |                       |
    room_e --------------  room_d

Objects:
  - box1  → room_a
  - box2  → room_b
  - ball1 → room_c
  - key1  → room_d
```

## Contract API

```rholang
robotAPI!("init", *state)                      // Initialize
robotAPI!("connected", "room_a", "room_b", *r) // Check connection
robotAPI!("locate", "box1", *r)                // Find object
robotAPI!("can_reach", "room_c", *r)           // Check reachability
robotAPI!("distance", "room_d", *r)            // Get distance
```

## Documentation

- **Full Guide**: `ROBOT_PLANNING.md`
- **Summary**: `ROBOT_PLANNING_SUMMARY.md`
- **This Guide**: `QUICK_START.md`

## Troubleshooting

**Q: Getting unevaluated variables (`$room`, `$dist`)?**
A: Use `robot_planning_fixed.metta` and `robot_planning_working.rho` instead of the original versions.

**Q: `rholang-cli` not found?**
A: Build it from `/home/dylon/Workspace/f1r3fly.io/f1r3node/` with `cargo build --release`.

**Q: Want to add new rooms/objects?**
A: Edit the MeTTa facts in the `init()` contract or in the `.metta` file.
