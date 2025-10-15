# PathMap State Design for MeTTa/Rholang Integration

## Overview

This document describes the PathMap-based state management design for integrating the MeTTa compiler with the Rholang runtime. The design enables REPL-style incremental compilation and evaluation where state accumulates across multiple interactions.

## State Structure

### MettaState

The `MettaState` structure represents the complete state of a MeTTa computation session:

```rust
pub struct MettaState {
    /// Pending s-expressions to be evaluated
    pub pending_exprs: Vec<MettaValue>,
    /// The atom space (MORK fact database) containing rules and facts
    pub environment: Environment,
    /// Results from previous evaluations
    pub eval_outputs: Vec<MettaValue>,
}
```

### Two Types of States

1. **Compiled State** (fresh from `compile`):
   - `pending_exprs`: S-expressions parsed from source code
   - `environment`: Empty (no facts/rules yet)
   - `eval_outputs`: Empty (no evaluations performed)

2. **Accumulated State** (built over multiple REPL iterations):
   - `pending_exprs`: Empty (already evaluated)
   - `environment`: Accumulated atom space with rules and facts from previous evaluations
   - `eval_outputs`: Results from all previous evaluations

## API Design

### compile(source: String) -> PathMap

Compiles MeTTa source code and returns a PathMap containing the compiled state.

**Pseudocode:**
```
compile(source):
  exprs = parse(source)                  # Parse source into s-expressions
  state = MettaState {
    pending_exprs: exprs,
    environment: empty,
    eval_outputs: []
  }
  return PathMap.from_state(state)      # Wrap in PathMap: {| state |}
```

**JSON Format:**
```json
{
  "pending_exprs": [
    {"type":"sexpr","items":[...]}
  ],
  "environment": {"facts_count": 0},
  "eval_outputs": []
}
```

### run(accumulated_state: PathMap, compiled_state: PathMap) -> PathMap

Evaluates pending expressions from compiled state against the accumulated environment.

**Pseudocode:**
```
run(accumulated_state, compiled_state):
  # Extract states from PathMaps
  accum = accumulated_state.extract()
  comp = compiled_state.extract()

  # Start with accumulated environment
  env = accum.environment
  outputs = accum.eval_outputs.clone()

  # Evaluate each pending expression
  for expr in comp.pending_exprs:
    (results, new_env) = eval(expr, env)
    env = new_env
    outputs.extend(results)

  # Create new accumulated state
  new_state = MettaState {
    pending_exprs: [],          # Empty (everything evaluated)
    environment: env,            # Updated environment
    eval_outputs: outputs        # Accumulated outputs
  }

  return PathMap.from_state(new_state)
```

## Rholang Integration

### Usage Pattern

```rholang
new stdout(`rho:io:stdout`),
    compile(`rho:metta:compile`),
    runMeTTa,
    src
in {
  // Contract to compile and run against accumulated state
  contract runMeTTa(ret, src, accumulated) = {
    for (@code <= src) {
      // Compile source to get compiled state PathMap
      for (@compiled_pm <- compile!?(code)) {
        // Run compiled state against accumulated state
        // This evaluates pending expressions and returns new accumulated state
        ret!(accumulated.run(compiled_pm))
      }
    }
  } |

  // Test harness that builds up state over multiple compilations
  contract testHarness(src) = {
    // Start with empty accumulated state
    new accumulated in {
      accumulated!({||}) |  // Empty PathMap state

      // Compile and run first source
      src!(MeTTaCode1) |
      for (@rslt1 <= runMeTTa!?(src, *accumulated)) {
        accumulated!(rslt1) |  // Update accumulated state
        stdout!([\"Result 1:\", rslt1]) |

        // Compile and run second source (uses state from first)
        src!(MeTTaCode2) |
        for (@rslt2 <= runMeTTa!?(src, *accumulated)) {
          accumulated!(rslt2) |
          stdout!([\"Result 2:\", rslt2])
        }
      }
    }
  }
}
```

### System Processes

Two Rust system processes are implemented:

1. **`rho:metta:compile`** (channel 200)
   - Input: MeTTa source code string
   - Output: PathMap containing compiled state
   - Usage: `compile!(source, *result)`

2. **`rho:metta:run`** (channel 202) - **TO BE IMPLEMENTED**
   - Input: Accumulated state PathMap, Compiled state PathMap
   - Output: New accumulated state PathMap
   - Usage: `mettaRun!(accum_state, compiled_state, *result)`

Alternatively, the Rholang team may implement `run` as a method on PathMap itself:
```rholang
for (@new_state <- accumulated_pm.run(compiled_pm)) {
  // new_state is updated accumulated state
}
```

## Implementation Status

### ✅ Completed

1. **MettaState structure** (`src/backend/types.rs`)
   - Defined with all required fields
   - Constructor methods for different state types

2. **JSON serialization** (`src/rholang_integration.rs`)
   - `metta_state_to_json()`: Converts MettaState to JSON
   - `compile_to_state_json()`: Compiles and returns JSON state
   - `compile_to_state_safe()`: Error-safe wrapper

3. **Evaluation engine** (`src/backend/eval.rs`)
   - Complete `eval()` function with pattern matching
   - Built-in operations (arithmetic, comparisons)
   - Control flow (if, catch, quote, eval)
   - Error handling and propagation
   - Type system (assertions, inference, checking)
   - MORK Space integration for fact storage

### 🔨 To Be Implemented

1. **`run_state()` function** (`src/rholang_integration.rs`)
   ```rust
   pub fn run_state(
       accumulated_json: &str,
       compiled_json: &str
   ) -> Result<String, String>
   ```
   - Parse both JSON states
   - Evaluate pending expressions from compiled state
   - Merge environments
   - Return new accumulated state as JSON

2. **`metta_run` Rust handler** (`f1r3node/rholang/src/rust/interpreter/system_processes.rs`)
   - Register at channel 202 with arity 3
   - Extract accumulated and compiled PathMap states
   - Call `run_state()`
   - Return result as PathMap

3. **PathMap `run` method** (Scala side - Rholang team)
   - Add stub method to PathMap class
   - Call into Rust `metta_run` handler
   - Or implement directly in Scala if simpler

4. **Test contract** (`integration/test_pathmap_state.rho`)
   - Demonstrate multi-step REPL workflow
   - Test state accumulation
   - Verify environment persistence

## Example Workflow

### Step 1: Initial Compilation

```
Input: "(= (double $x) (* $x 2))"

compile() returns PathMap:
{
  "pending_exprs": [
    {
      "type":"sexpr",
      "items":[
        {"type":"atom","value":"="},
        {"type":"sexpr","items":[
          {"type":"atom","value":"double"},
          {"type":"atom","value":"$x"}
        ]},
        {"type":"sexpr","items":[
          {"type":"atom","value":"mul"},
          {"type":"atom","value":"$x"},
          {"type":"number","value":2}
        ]}
      ]
    }
  ],
  "environment": {"facts_count": 0},
  "eval_outputs": []
}
```

### Step 2: First Execution

```
Input:
  accumulated_state: {||}  (empty)
  compiled_state: (from step 1)

run() evaluates:
  - Processes rule definition (= (double $x) (* $x 2))
  - Adds rule to environment
  - Returns Nil

Output PathMap:
{
  "pending_exprs": [],
  "environment": {"facts_count": 1},  # Rule added
  "eval_outputs": [{"type":"nil"}]
}
```

### Step 3: Second Compilation

```
Input: "(double 5)"

compile() returns PathMap:
{
  "pending_exprs": [
    {
      "type":"sexpr",
      "items":[
        {"type":"atom","value":"double"},
        {"type":"number","value":5}
      ]
    }
  ],
  "environment": {"facts_count": 0},
  "eval_outputs": []
}
```

### Step 4: Second Execution

```
Input:
  accumulated_state: (from step 2, has rule)
  compiled_state: (from step 3)

run() evaluates:
  - Matches (double 5) against rule (double $x)
  - Binds $x = 5
  - Evaluates (* 5 2)
  - Returns 10

Output PathMap:
{
  "pending_exprs": [],
  "environment": {"facts_count": 1},  # Still has rule
  "eval_outputs": [
    {"type":"nil"},           # From step 2
    {"type":"number","value":10}  # From step 4
  ]
}
```

## Key Design Decisions

### 1. PathMap as State Container

The design uses PathMap with a single element `{| state |}` to represent the state. This provides:
- Type safety at the Rholang level
- Composability with other PathMap operations
- Natural integration with Rholang's collection system

### 2. Stateless compile, Stateful run

- **compile**: Always returns a fresh state with no environment
- **run**: Accumulates state across invocations

This separation ensures:
- Compilation is deterministic and cacheable
- Evaluation can be repeated with different accumulated states
- State management is explicit and controllable

### 3. Direct Rust Function Calls

The integration uses direct Rust function calls (no FFI):
- MeTTa compiler crate is a dependency of rholang crate
- Type safety enforced at compile time
- Zero serialization overhead for internal calls
- Only serialize to JSON at Rholang boundary

### 4. Environment Merging Strategy

When merging environments in `run`:
- Use `Environment::union()` for monotonic merge
- MORK Space facts accumulate (PathMap union)
- Rule cache extends (indexed for O(1) lookup)
- Type assertions extend (HashMap merge)

## Testing Strategy

### Unit Tests (Rust)

1. Test `MettaState` construction
2. Test JSON serialization/deserialization
3. Test `run_state()` with various inputs
4. Test environment merging

### Integration Tests (Rholang)

1. Single compilation + evaluation
2. Multiple compilations with state accumulation
3. Rule definition followed by rule application
4. Type assertions persistence across evaluations
5. Error handling and recovery

### REPL Tests

1. Define rule, then use it
2. Add facts incrementally
3. Query accumulated knowledge
4. Type checking with accumulated types

## References

- **MeTTa Compiler**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/`
- **Rholang Runtime**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/`
- **PathMap Implementation**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/models/src/rust/path_map.rs`
- **System Processes**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs`
- **Integration Tests**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration/`

## Next Steps

1. Implement `run_state()` function
2. Add `metta_run` system process handler
3. Create comprehensive test suite
4. Update documentation with exact JSON schemas
5. Coordinate with Rholang team on PathMap `run` method implementation
