# PathMap Par Integration Design

## Current Issue

The `compile` function variants exposed via the Rholang registry currently return JSON strings instead of PathMap data structures. The design requires that `MettaState` be wrapped in a PathMap object and returned as a `Par` type.

## Background

### What is a PathMap in Rholang?

- **PathMap** is a trie-based collection structure in Rholang (from the `pathmap` crate)
- **Type**: `BytesTrieMap<Par>` - maps byte sequences to Par values
- **Purpose**: Efficient storage and retrieval of hierarchical data
- **Usage**: f1r3node has `RholangPathMap` type alias and integration layer in `models/src/rust/pathmap_integration.rs`

### What is Par?

- **Par** is the primary data type in Rholang (from protobuf models)
- Represents Rholang processes and data
- Has multiple expression types via `ExprInstance` enum
- Can contain ground values, S-expressions, lists, tuples, and collections

## Required Changes

### 1. Create PathMap from MettaState

We need a function that converts `MettaState` into a `RholangPathMap`:

```rust
pub fn metta_state_to_pathmap(state: &MettaState) -> RholangPathMap {
    let mut map = RholangPathMap::new();

    // Store pending_exprs at path ["pending_exprs"]
    let pending_key = create_path_key(&["pending_exprs"]);
    let pending_par = metta_values_to_par_list(&state.pending_exprs);
    map.insert(pending_key, pending_par);

    // Store environment at path ["environment"]
    let env_key = create_path_key(&["environment"]);
    let env_par = environment_to_par(&state.environment);
    map.insert(env_key, env_par);

    // Store eval_outputs at path ["eval_outputs"]
    let outputs_key = create_path_key(&["eval_outputs"]);
    let outputs_par = metta_values_to_par_list(&state.eval_outputs);
    map.insert(outputs_key, outputs_par);

    map
}
```

### 2. Convert PathMap to Par

The PathMap needs to be wrapped in a Par object that can be returned from system process handlers:

```rust
pub fn pathmap_to_par(map: RholangPathMap) -> Par {
    // Need to understand how to create EPathMap ExprInstance
    // This may require:
    // 1. Converting PathMap entries to KeyValuePair protobuf messages
    // 2. Creating EPathMap with sorted_list of entries
    // 3. Wrapping in Expr and Par

    // Placeholder - needs implementation based on Rholang internals
    todo!("Convert PathMap to Par with EPathMap ExprInstance")
}
```

### 3. Convert MettaValue to Par

We need functions to convert MettaValue types to Rholang Par types:

```rust
pub fn metta_value_to_par(value: &MettaValue) -> Par {
    match value {
        MettaValue::Atom(s) => RhoString::create_par(s.clone()),
        MettaValue::Bool(b) => RhoBoolean::create_par(*b),
        MettaValue::Long(n) => RhoNumber::create_par(*n),
        MettaValue::String(s) => RhoString::create_par(s.clone()),
        MettaValue::Uri(s) => RhoUri::create_par(s.clone()),
        MettaValue::Nil => Par::default(),
        MettaValue::SExpr(items) => {
            let item_pars: Vec<Par> = items.iter()
                .map(|v| metta_value_to_par(v))
                .collect();
            create_list_par(item_pars)
        }
        MettaValue::Error(msg, details) => {
            // Create tuple (msg, details)
            create_tuple_par(vec![
                RhoString::create_par(msg.clone()),
                metta_value_to_par(details)
            ])
        }
        MettaValue::Type(t) => {
            // Wrap type in a tagged structure
            metta_value_to_par(t)
        }
    }
}
```

### 4. Update System Process Handlers

Update `metta_compile`, `metta_compile_sync`, and `metta_run` to return PathMap Pars instead of JSON strings:

```rust
pub async fn metta_compile(&mut self, contract_args: ...) -> Result<Vec<Par>, InterpreterError> {
    // ... extract arguments ...

    let src = self.pretty_printer.build_string_from_message(source);

    // Compile to MettaState
    let state = match mettatron::backend::compile::compile(&src) {
        Ok(s) => s,
        Err(e) => {
            // Return error as Par
            let error_par = RhoString::create_par(format!(r#"{{"error":"{}"}}"#, e));
            produce(&vec![error_par], return_channel).await?;
            return Ok(vec![error_par]);
        }
    };

    // Convert to PathMap and then to Par
    let pathmap = metta_state_to_pathmap(&state);
    let result_par = pathmap_to_par(pathmap);

    produce(&vec![result_par], return_channel).await?;
    Ok(vec![result_par])
}
```

## Implementation Challenges

### Challenge 1: EPathMap Construction

**Problem**: We need to understand how to properly create `EPathMap` ExprInstance from a `RholangPathMap`.

**Research Needed**:
- How does the Rholang normalizer create EPathMap instances?
- What is the structure of the protobuf EPathMap message?
- Do we need to serialize the PathMap or can we use it directly?

**Potential Approach**:
Look at `rholang/src/rust/interpreter/compiler/normalizer/collection_normalize_matcher.rs` for examples of how EPathMap is constructed during normalization.

### Challenge 2: MettaValue to Par Conversion

**Problem**: Some MettaValue types don't have direct Par equivalents.

**Solutions**:
- **Atom**: Use `RhoString` with a tag prefix like `"atom:name"`
- **SExpr**: Use Rholang List (`EListBody`)
- **Error**: Use Rholang Tuple with `("error", msg, details)` structure
- **Type**: Use tagged string or nested structure

### Challenge 3: Environment Serialization

**Problem**: The `Environment` contains a `rule_cache` (PathMap-based MORK Space) which is complex.

**Solutions**:
- **Option 1**: Serialize to nested PathMap structure
- **Option 2**: Just store facts_count as metadata (current approach in JSON)
- **Option 3**: Serialize full MORK Space rules for complete state transfer

**Recommendation**: Start with Option 2 (metadata only) for initial implementation.

### Challenge 4: Deserialization

**Problem**: `run_state` handler needs to deserialize PathMap Par back to MettaState.

**Solution**: Create inverse functions:
```rust
pub fn par_to_metta_state(par: &Par) -> Result<MettaState, String>
pub fn par_to_pathmap(par: &Par) -> Result<RholangPathMap, String>
pub fn pathmap_to_metta_state(map: &RholangPathMap) -> Result<MettaState, String>
pub fn par_to_metta_value(par: &Par) -> Result<MettaValue, String>
```

## Alternative Approach: DataPath Instead of PathMap

**Consideration**: The design mentions "DataPath" as well as PathMap. These may be related concepts.

**Investigation Needed**:
- Is DataPath a wrapper around PathMap?
- Does Rholang have a specific DataPath type?
- Should we use a simpler structure like a Map/Dictionary Par?

## Recommended Implementation Plan

1. **Phase 1: Research** (Current)
   - Understand EPathMap protobuf structure
   - Find examples of EPathMap creation in f1r3node codebase
   - Determine if we should use EPathMap or simpler structure

2. **Phase 2: Basic Conversion**
   - Implement `metta_value_to_par()` for all MettaValue types
   - Implement helper functions for creating List, Tuple Pars
   - Test conversion with simple examples

3. **Phase 3: PathMap Integration**
   - Implement `metta_state_to_pathmap()`
   - Implement `pathmap_to_par()` (or alternative structure)
   - Test with round-trip serialization

4. **Phase 4: Handler Updates**
   - Update `metta_compile` and `metta_compile_sync` handlers
   - Update `metta_run` handler with deserialization
   - Test end-to-end with Rholang contracts

5. **Phase 5: Full MORK Serialization** (Optional Enhancement)
   - Implement full Environment serialization if needed
   - Support complete state transfer including all rules

## Questions for User

1. Should we use EPathMap specifically, or is a simpler Par structure acceptable (like a Map/Dictionary)?

2. For the Environment field, is metadata sufficient, or do we need full MORK Space serialization?

3. Are there existing examples in f1r3node of system processes that return PathMap structures we can reference?

4. Is the term "DataPath" in the design requirement referring to a specific type, or is it synonymous with PathMap?

## Next Steps

Based on user feedback, proceed with Phase 1 research to understand the exact Par structure required for PathMap representation.
