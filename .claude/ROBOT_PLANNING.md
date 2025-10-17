# Robot Planning System

A Prolog-style robot planning and navigation system implemented in MeTTa with comprehensive Rholang contract interfaces.

## Overview

This system demonstrates how to use MeTTa as a logic programming language (similar to Prolog) for robot planning tasks, integrated with Rholang contracts for a clean, composable API.

### Key Features

- **Path Finding**: Query reachability and shortest paths between rooms
- **Object Tracking**: Track object locations and robot state
- **Action Planning**: Plan sequences of movements and manipulations
- **State Management**: Maintain consistent world state across operations
- **Contract-Based API**: Clean Rholang interfaces for all operations

## Files

- `robot_planning.metta` - Complete MeTTa knowledge base with rules and queries
- `robot_planning.rho` - Full Rholang contract API (comprehensive demo)
- `robot_planning_test.rho` - Basic integration test (quick validation)
- `robot_planning_simple.metta` - Minimal MeTTa test (unit testing)

## Architecture

### MeTTa Knowledge Base

The MeTTa component defines:

1. **Environment Facts**
   - Room connectivity graph
   - Object locations
   - Robot state (position, carrying)

2. **Inference Rules**
   - Path finding (direct and transitive)
   - Distance calculation
   - Reachability queries

3. **Action Predicates**
   - Movement validation and execution
   - Object pickup/putdown
   - High-level planning

### Rholang Contracts

The Rholang layer provides:

1. **Initialization**: `robotAPI!("init", *state)`
2. **Queries**: `can_reach`, `locate`, `distance`
3. **Actions**: `move`, `pickup`, `putdown`
4. **Planning**: `plan_navigate`, `plan_transport`

## Environment Model

```
    room_a ---- room_b ---- room_c
      |                       |
      |                       |
    room_e --------------  room_d
```

### Initial State

**Objects:**
- `box1` in `room_a`
- `box2` in `room_b`
- `ball1` in `room_c`
- `key1` in `room_d`

**Robot:**
- Position: `room_a`
- Carrying: `nothing`

## Usage Examples

### Running MeTTa Directly

```bash
# Test basic connectivity
./target/release/mettatron examples/robot_planning_simple.metta

# Run full planning queries
./target/release/mettatron examples/robot_planning.metta
```

### Running with Rholang

```bash
# Basic integration test (fast)
/path/to/rholang-cli examples/robot_planning_test.rho

# Full contract demo (comprehensive)
/path/to/rholang-cli examples/robot_planning.rho
```

## MeTTa Query Examples

### Connectivity Queries

```lisp
// Direct connection check
!(connected room_a room_b)  // => [true]

// Path existence (transitive)
!(path room_a room_d)       // => [room_a] (path exists)

// Distance calculation
!(distance_to room_d)       // => [2] (when robot at room_a)
```

### Object Queries

```lisp
// Find object location
!(locate box1)              // => [room_a]

// List objects in a room
!(objects_in_room room_b)   // => [box2]
```

### Action Examples

```lisp
// Move to adjacent room
!(move_to room_b)           // Updates robot_at state

// Pick up object in current room
!(pickup box2)              // Updates robot_carrying state

// Put down carried object
!(putdown)                  // Places object in current room
```

### Planning Examples

```lisp
// Plan navigation to target
!(plan_navigate_to room_d)  // Executes movement sequence

// Complete transport plan
!(plan_transport box1 room_d)  // Full pickup-move-putdown sequence
```

## Rholang Contract API

### Initialization

```rholang
new state in {
  robotAPI!("init", *state) |
  for (@initialState <- state) {
    // Use initialState for queries and actions
  }
}
```

### Query: Can Reach

```rholang
new result in {
  robotAPI!("can_reach", "room_c", *result) |
  for (@canReach <- result) {
    // canReach contains true/false
  }
}
```

### Query: Locate Object

```rholang
new result in {
  robotAPI!("locate", "box1", *result) |
  for (@location <- result) {
    // location contains room name
  }
}
```

### Query: Distance

```rholang
new result in {
  robotAPI!("distance", "room_a", "room_d", *result) |
  for (@dist <- result) {
    // dist contains step count
  }
}
```

### Action: Move Robot

```rholang
new newState in {
  robotAPI!("move", currentState, "room_b", *newState) |
  for (@updatedState <- newState) {
    // updatedState has robot at room_b
  }
}
```

### Action: Pick Up Object

```rholang
new newState in {
  robotAPI!("pickup", currentState, "box1", *newState) |
  for (@updatedState <- newState) {
    // updatedState has robot carrying box1
  }
}
```

### Action: Put Down Object

```rholang
new newState in {
  robotAPI!("putdown", currentState, *newState) |
  for (@updatedState <- newState) {
    // updatedState has object placed in current room
  }
}
```

### Planning: Navigate

```rholang
new plan in {
  robotAPI!("plan_navigate", currentState, "room_d", *plan) |
  for (@navigationPlan <- plan) {
    // navigationPlan contains sequence of rooms
  }
}
```

### Planning: Transport Object

```rholang
new plan in {
  robotAPI!("plan_transport", "box1", "room_d", *plan) |
  for (@fullPlan <- plan) {
    // fullPlan contains complete action sequence
  }
}
```

## Design Patterns

### 1. Fact-Based Knowledge Representation

MeTTa uses equality rules to represent facts:

```lisp
(= (connected room_a room_b) true)
(= (object_at box1 room_a) true)
```

### 2. Rule-Based Inference

Complex queries use pattern matching and recursion:

```lisp
// Transitive path finding
(= (path $from $to)
   (if (connected $from $intermediate)
       (if (path $intermediate $to)
           $from
           nothing)
       nothing))
```

### 3. Conditional Actions

Actions validate preconditions before execution:

```lisp
(= (can_pickup $object)
   (if (robot_at $room)
       (if (object_at $object $room)
           (robot_carrying nothing)
           false)
       false))
```

### 4. State Composition

Rholang contracts chain state transformations:

```rholang
robotAPI!("init", *s0) |
for (@state0 <- s0) {
  robotAPI!("move", state0, "room_b", *s1) |
  for (@state1 <- s1) {
    robotAPI!("pickup", state1, "box2", *s2)
  }
}
```

### 5. Contract Abstraction

High-level operations hide implementation details:

```rholang
contract robotAPI(@"plan_transport", @object, @targetRoom, ret) = {
  // Complex multi-step planning hidden behind simple interface
  ...
}
```

## Composability Properties

The system satisfies these composability properties:

1. **Identity**: Empty state + compiled = initial state
2. **Sequential Composition**: state.run(a).run(b) accumulates both results
3. **Rule Persistence**: Rules defined in earlier runs remain available
4. **Rule Chaining**: Rules can reference other previously defined rules
5. **State Independence**: Same compiled code can run against different states
6. **Monotonic Accumulation**: Output count never decreases
7. **Error Resilience**: Errors don't break subsequent runs
8. **No Cross-Contamination**: Independent chains don't affect each other

## Testing

### Unit Tests (MeTTa Only)

```bash
./target/release/mettatron examples/robot_planning_simple.metta
```

Expected output:
```
[true]
[true]
[true]
```

### Integration Tests (Rholang + MeTTa)

```bash
/path/to/rholang-cli examples/robot_planning_test.rho
```

Expected output shows:
- Room connections defined
- Symmetry verification
- Object location queries
- State composition

### Full Demo

```bash
/path/to/rholang-cli examples/robot_planning.rho
```

Demonstrates:
- All query types
- All action types
- Sequential state updates
- Complete planning scenarios

## Extending the System

### Adding New Rooms

In MeTTa:
```lisp
(= (connected room_d room_f) true)
```

### Adding New Objects

In MeTTa:
```lisp
(= (object_at tool1 room_e) true)
```

### Adding New Predicates

In MeTTa:
```lisp
(= (is_tool $object)
   (if (object_at $object $_)
       (if (== $object tool1) true false)
       false))
```

In Rholang:
```rholang
contract robotAPI(@"is_tool", @objectName, ret) = {
  new queryCode, compiled, result in {
    queryCode!("!(is_tool " ++ objectName ++ ")") |
    for (@code <- queryCode) {
      mettaCompile!(code, *compiled) |
      for (@compiledQuery <- compiled) {
        result!(currentState.run(compiledQuery)) |
        for (@res <- result) { ret!(res) }
      }
    }
  }
}
```

### Adding New Actions

Follow the pattern:
1. Define validation predicate (`can_action`)
2. Define execution rule (`action`)
3. Add Rholang contract wrapper

## Performance Characteristics

- **Compilation**: O(n) where n = source code length
- **Query Execution**: Depends on rule complexity
  - Direct facts: O(1)
  - Transitive paths: O(e) where e = edges in graph
  - Planning: O(e × d) where d = path depth
- **State Updates**: O(1) for PathMap operations

## Comparison with Prolog

### Similarities

- Pattern matching and unification
- Rule-based inference
- Logical queries
- Backtracking (implicit in evaluation)

### Differences

- MeTTa uses S-expressions instead of Prolog syntax
- Lazy evaluation by default
- Integration with concurrent Rholang processes
- Persistent state via PathMap structures

## References

- MeTTa Language Guide: See `CLAUDE.md`
- Rholang Specification: See `/path/to/rholang-rs/`
- Integration Examples: See `integration/` directory
- Original Research: See MeTTaTron documentation

## License

Same as parent MeTTa-Compiler project (Apache 2.0).
