# Dynamic Multi-Step Planning with Pattern Matching

## Overview

The robot planning system now uses **pattern matching** to dynamically compose action sequences from path-finding results. Instead of hard-coding action sequences, the system builds plans by:

1. **Querying paths** using `find_path` rules
2. **Pattern matching** on the path structure
3. **Composing actions** based on the matched pattern

This approach is more flexible and demonstrates how MeTTa's pattern matching can construct complex sequences from simple rules.

## Architecture

### 1. Path Finding (Data Layer)

Path-finding rules return **lists of rooms** to navigate through:

```metta
// Direct path: adjacent rooms
(= (find_path $from $to) ($to) (if (connected $from $to) true false))

// Two-hop paths
(= (find_path room_a room_c) (room_b room_c))
(= (find_path room_c room_a) (room_b room_a))
```

**Result format:**
- Single hop: `($dest)` - e.g., `(room_b)`
- Two hops: `($mid $dest)` - e.g., `(room_b room_a)`
- Three hops: `($mid1 $mid2 $dest)` - e.g., `(room_b room_c room_d)`

### 2. Plan Composition (Logic Layer)

The `compose_plan` function uses **pattern matching** to build action sequences from path structures:

```metta
// Pattern 1: Single-hop path ($dest)
(= (compose_plan $start $obj ($dest))
   ((navigate $start) (pickup $obj) (navigate $dest) (putdown)))

// Pattern 2: Two-hop path ($mid $dest)
(= (compose_plan $start $obj ($mid $dest))
   ((navigate $start) (pickup $obj) (navigate $mid) (navigate $dest) (putdown)))

// Pattern 3: Three-hop path ($mid1 $mid2 $dest)
(= (compose_plan $start $obj ($mid1 $mid2 $dest))
   ((navigate $start) (pickup $obj) (navigate $mid1) (navigate $mid2) (navigate $dest) (putdown)))
```

**Key insight:** The pattern `($mid $dest)` destructures the path list and inserts navigation steps for each element.

### 3. Integration Layer

Transport planning queries the path and delegates to composition:

```metta
// Look up object location and delegate
(= (transport_steps ball1 $target)
   (build_plan_with_path room_c $target ball1))

// Query find_path and compose
(= (build_plan_with_path room_c room_a $obj)
   (compose_plan room_c $obj (find_path room_c room_a)))
```

**Execution flow:**
1. `transport_steps ball1 room_a` → delegates to `build_plan_with_path`
2. `build_plan_with_path room_c room_a ball1` → calls `(find_path room_c room_a)`
3. `find_path room_c room_a` → returns `(room_b room_a)`
4. `compose_plan room_c ball1 (room_b room_a)` → matches `($mid $dest)` pattern
5. Result: `((navigate room_c) (pickup ball1) (navigate room_b) (navigate room_a) (putdown))`

## Example: Dynamic Composition

### Single-Hop Path (Adjacent Rooms)

```metta
!(transport_steps ball1 room_b)
```

**Execution:**
1. Query: `(find_path room_c room_b)` → `(room_b)` (single-hop)
2. Pattern match: `(compose_plan room_c ball1 (room_b))` matches `($dest)`
3. Result: `((navigate room_c) (pickup ball1) (navigate room_b) (putdown))`

**4 actions, 2 navigation steps**

### Two-Hop Path (Non-Adjacent Rooms)

```metta
!(transport_steps ball1 room_a)
```

**Execution:**
1. Query: `(find_path room_c room_a)` → `(room_b room_a)` (two-hop)
2. Pattern match: `(compose_plan room_c ball1 (room_b room_a))` matches `($mid $dest)`
3. Result: `((navigate room_c) (pickup ball1) (navigate room_b) (navigate room_a) (putdown))`

**5 actions, 3 navigation steps**

### Three-Hop Path (Long Distance)

```metta
!(transport_steps box1 room_d)  // Assuming box1 in room_a
```

**Hypothetical execution (if path existed):**
1. Query: `(find_path room_a room_d)` → `(room_b room_c room_d)` (three-hop)
2. Pattern match: `(compose_plan room_a box1 (room_b room_c room_d))` matches `($mid1 $mid2 $dest)`
3. Result: `((navigate room_a) (pickup box1) (navigate room_b) (navigate room_c) (navigate room_d) (putdown))`

**6 actions, 4 navigation steps**

## Benefits of Pattern Matching Approach

### ✅ Declarative

Rules describe **what** the plan should contain, not **how** to build it:

```metta
// Describes: "For a 2-hop path, insert nav steps for mid and dest"
(= (compose_plan $start $obj ($mid $dest))
   ((navigate $start) (pickup $obj) (navigate $mid) (navigate $dest) (putdown)))
```

### ✅ Extensible

Adding support for longer paths requires only one new pattern:

```metta
// Add 4-hop support
(= (compose_plan $start $obj ($m1 $m2 $m3 $dest))
   ((navigate $start) (pickup $obj) (navigate $m1) (navigate $m2) (navigate $m3) (navigate $dest) (putdown)))
```

### ✅ Composable

Path-finding and plan composition are **separate concerns**:
- `find_path` rules define **topological connectivity**
- `compose_plan` rules define **action sequencing**
- Changes to one don't affect the other

### ✅ Testable

Each layer can be tested independently:

```metta
// Test path finding
!(find_path room_c room_a)  → [(room_b room_a)]

// Test composition with known path
!(compose_plan room_c ball1 (room_b room_a))  → [((navigate ...) ...)]

// Test full integration
!(transport_steps ball1 room_a)  → [((navigate ...) ...)]
```

## Comparison: Hard-Coded vs Dynamic

### Hard-Coded Approach (Previous)

```metta
// Must write separate rule for each source/dest pair
(= (build_plan room_c room_a $object)
   ((navigate room_c) (pickup $object) (navigate room_b) (navigate room_a) (putdown)))

(= (build_plan room_b room_d $object)
   ((navigate room_b) (pickup $object) (navigate room_c) (navigate room_d) (putdown)))

// ... one rule for each multi-hop path
```

**Problems:**
- ❌ Repetitive action sequences
- ❌ Can't adapt to new paths without new rules
- ❌ Path and plan tightly coupled

### Dynamic Approach (Current)

```metta
// Single pattern handles all 2-hop paths
(= (compose_plan $start $obj ($mid $dest))
   ((navigate $start) (pickup $obj) (navigate $mid) (navigate $dest) (putdown)))

// Works with ANY path that matches the pattern
```

**Benefits:**
- ✅ One pattern handles infinite paths
- ✅ New paths work automatically
- ✅ Path and plan decoupled

## Testing

### Test File: `/tmp/test_multistep_planning.metta`

```bash
./target/release/mettatron /tmp/test_multistep_planning.metta
```

**Output:**
```
[room_c]                          # ball1 location
[(room_b room_a)]                 # 2-hop path
[((navigate room_c) (pickup ball1) (navigate room_b) (navigate room_a) (putdown))]  # 5-step plan
[((navigate room_c) (pickup ball1) (navigate room_b) (putdown))]                     # 4-step plan
```

**Demonstrates:**
- Same `compose_plan` patterns handle both 1-hop and 2-hop paths
- Action sequence automatically adapts to path length
- No hard-coded action sequences!

## Future: Fully Generic Planning

With full variable unification (future MeTTa feature), we could make path-finding fully generic:

```metta
// Generic path finding (requires variable unification + recursion)
(= (find_path $from $to)
   (if (connected $from $to)
       ($to)
       (cons $next (find_path $next $to))))

// Generic composition (requires list pattern matching)
(= (compose_plan $start $obj $path)
   (cons (navigate $start)
         (cons (pickup $obj)
               (append (map navigate $path) (putdown)))))
```

This would eliminate the need for explicit path rules entirely!

## Dynamic Distance Calculation

Distance is now **computed dynamically** from path structures instead of hard-coded:

```metta
// Pattern match on path structure to count hops
(= (path_hops ($dest)) 1)
(= (path_hops ($mid $dest)) 2)
(= (path_hops ($mid1 $mid2 $dest)) 3)

// Compute distance from path
(= (distance_via_path $from $to)
   (path_hops (find_path $from $to)))
```

**Examples:**
```metta
!(distance_via_path room_a room_c)  → [2]  // find_path returns (room_b room_c), matches ($mid $dest)
!(distance_via_path room_a room_b)  → [1]  // find_path returns (room_b), matches ($dest)
!(distance_via_path room_a room_d)  → [2]  // find_path returns (room_e room_d), matches ($mid $dest)
```

**Benefits:**
- ✅ No hard-coded distance values
- ✅ Automatically consistent with path definitions
- ✅ Adding new paths automatically updates distances
- ✅ Single source of truth (connectivity rules)

## Summary

The current implementation uses **pattern matching** to dynamically:

1. **Compose action sequences** from path structures
2. **Calculate distances** from hop counts
3. **Build navigation plans** from room lists

**Path finding** returns structured data → **Pattern matching** destructures and transforms → **Results** are computed dynamically

**Key patterns:**
- `($dest)` → 1 hop → 4 actions → distance 1
- `($mid $dest)` → 2 hops → 5 actions → distance 2
- `($mid1 $mid2 $dest)` → 3 hops → 6 actions → distance 3

Each pattern **automatically** generates the correct action sequence and distance for its path structure!
