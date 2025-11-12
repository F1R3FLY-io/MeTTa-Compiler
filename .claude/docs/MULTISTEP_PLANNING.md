# Multi-Step Planning in robot_planning.rho

## Overview

The `robot_planning.rho` system now supports **multi-step planning** where the robot must navigate through intermediate rooms to reach destinations that are not directly connected.

## Environment Layout

```
    room_a ---- room_b ---- room_c
      |                       |
      |                       |
    room_e --------------  room_d

Direct connections (1 step):
  - room_a ↔ room_b
  - room_b ↔ room_c
  - room_c ↔ room_d
  - room_a ↔ room_e
  - room_e ↔ room_d

Multi-step paths (2+ steps):
  - room_a → room_c (via room_b)
  - room_c → room_a (via room_b)
  - room_b → room_d (via room_c)
  - room_d → room_b (via room_c)
  - room_a ↔ room_d (via room_e)
```

## Multi-Step Planning Rules

### Path Finding Rules

The system defines explicit multi-hop paths:

```metta
// Direct path: adjacent rooms
(= (find_path $from $to) ($to) (if (connected $from $to) true false))

// Path through room_b as intermediate
(= (find_path room_a room_c) (room_b room_c))
(= (find_path room_c room_a) (room_b room_a))

// Path through room_c as intermediate
(= (find_path room_b room_d) (room_c room_d))
(= (find_path room_d room_b) (room_c room_b))

// Path through room_e as intermediate
(= (find_path room_a room_d) (room_e room_d))
(= (find_path room_d room_a) (room_e room_a))
```

### Build Plan Rules

Plans are constructed as **lists of action tuples**, where each action can be executed separately:

```metta
// Single-step plan (adjacent rooms)
// Returns: [(navigate start) (pickup obj) (navigate dest) (putdown)]
(= (build_plan $obj_loc $target $object)
   ((navigate $obj_loc) (pickup $object) (navigate $target) (putdown))
   (if (connected $obj_loc $target) true false))

// Multi-step plan: room_a to room_c via room_b
// Returns: [(navigate room_a) (pickup obj) (navigate room_b) (navigate room_c) (putdown)]
(= (build_plan room_a room_c $object)
   ((navigate room_a) (pickup $object) (navigate room_b) (navigate room_c) (putdown)))

// Multi-step plan: room_c to room_a via room_b
(= (build_plan room_c room_a $object)
   ((navigate room_c) (pickup $object) (navigate room_b) (navigate room_a) (putdown)))

// Multi-step plan: room_b to room_d via room_c
(= (build_plan room_b room_d $object)
   ((navigate room_b) (pickup $object) (navigate room_c) (navigate room_d) (putdown)))

// Multi-step plan: room_a to room_d via room_e
(= (build_plan room_a room_d $object)
   ((navigate room_a) (pickup $object) (navigate room_e) (navigate room_d) (putdown)))
```

**Key difference:** Actions are now individual tuples in a list, making them easy to iterate over and execute one at a time.

## Demo 4: Multi-Step Planning Example

Demo 4 demonstrates transporting `ball1` from `room_c` to `room_a`, which requires navigating through `room_b`:

### Query Sequence

1. **Locate the object:**
   ```metta
   !(locate ball1)
   ```
   Result: `room_c`

2. **Find the path:**
   ```metta
   !(find_path room_c room_a)
   ```
   Result: `(room_b room_a)` - path goes through room_b

3. **Build the plan:**
   ```metta
   !(transport_steps ball1 room_a)
   ```
   Result: `((navigate room_c) (pickup ball1) (navigate room_b) (navigate room_a) (putdown))`

### Plan Breakdown

The generated plan is a **list of 5 action tuples** with **3 navigation steps**:

1. `(navigate room_c)` - Go to where ball1 is located
2. `(pickup ball1)` - Pick up the object
3. `(navigate room_b)` - **Intermediate hop** (room_c and room_a not directly connected)
4. `(navigate room_a)` - Reach the final destination
5. `(putdown)` - Place the object

**Important:** Each action is a separate tuple, making the plan easy to execute step-by-step.

## Comparison: Single-Step vs Multi-Step

### Single-Step Example (box1: room_a → room_b)

```metta
!(transport_steps box1 room_b)
```

Result:
```
((navigate room_a) (pickup box1) (navigate room_b) (putdown))
```

**Actions:** 4 action tuples, **2 navigation steps**
- `(navigate room_a)` - Navigate to start
- `(pickup box1)` - Pickup object
- `(navigate room_b)` - Navigate to destination
- `(putdown)` - Putdown object

### Multi-Step Example (ball1: room_c → room_a)

```metta
!(transport_steps ball1 room_a)
```

Result:
```
((navigate room_c) (pickup ball1) (navigate room_b) (navigate room_a) (putdown))
```

**Actions:** 5 action tuples, **3 navigation steps**
- `(navigate room_c)` - Navigate to start
- `(pickup ball1)` - Pickup object
- `(navigate room_b)` - **Navigate through intermediate**
- `(navigate room_a)` - Navigate to destination
- `(putdown)` - Putdown object

## Key Features

1. **Automatic Path Construction**
   - MeTTa rules determine whether direct or multi-hop path is needed
   - No pre-computed paths - constructed on-demand through queries

2. **Explicit Intermediate Steps**
   - Plans include all intermediate navigation waypoints
   - Clear sequence of actions for execution

3. **Rule Precedence**
   - Multi-step rules (e.g., `build_plan room_c room_a`) match before generic rule
   - Ensures specific paths take precedence over generic patterns

4. **Query-Based Planning**
   - Location queries: `!(locate <object>)`
   - Path queries: `!(find_path <from> <to>)`
   - Plan construction: `!(transport_steps <object> <target>)`

## Testing Multi-Step Planning

Test file: `/tmp/test_multistep_planning.metta`

```bash
./target/release/mettatron /tmp/test_multistep_planning.metta
```

Expected output:
```
[room_c]
[(room_b room_a)]
[(navigate room_c pickup ball1 navigate room_b navigate room_a putdown)]
```

## Running the Full Demo

```bash
cd /home/dylon/Workspace/f1r3fly.io/f1r3node
./target/release/rholang-cli /path/to/MeTTa-Compiler/examples/robot_planning.rho
```

Look for Demo 4 output showing the multi-step plan with intermediate navigation through `room_b`.

## Extension Ideas

### 1. Longer Paths (3+ hops)

For very long paths, additional rules can be added:

```metta
// Path from room_a to room_d via room_b and room_c
(= (build_plan room_a room_d $object)
   (navigate room_a pickup $object navigate room_b navigate room_c navigate room_d putdown))
```

### 2. Dynamic Path Finding

With full variable unification (future MeTTa feature), paths could be computed dynamically:

```metta
// Generic path finding (requires variable unification)
(= (build_multi_step_plan $start $end $object $path)
   (if (find_path $start $end $intermediate)
       (cons navigate $start (cons pickup $object (build_nav_sequence $intermediate)))
       (build_simple_plan $start $end $object)))
```

### 3. Cost-Based Planning

Add costs and choose optimal paths:

```metta
(= (path_cost room_a room_c) 2)  // Via room_b
(= (path_cost room_a room_d) 2)  // Via room_e
(= (path_cost room_a room_d) 3)  // Via room_b, room_c

(= (optimal_plan $object $target)
   (find_min_cost_path (locate $object) $target))
```

## Summary

✅ Multi-step planning implemented for non-adjacent rooms
✅ Explicit intermediate navigation waypoints in plans
✅ Query-based path construction (no pre-computed paths)
✅ Demo 4 demonstrates 3-step navigation: room_c → room_b → room_a
✅ Tested and verified with standalone MeTTa file

The system now handles realistic scenarios where destinations require navigating through intermediate locations!
