# PathMap State Integration

## Overview

This integration adds the `rho:metta:run` system process to f1r3node, enabling PathMap-based REPL functionality for MeTTa.

## System Process

**URN**: `rho:metta:run`
**Channel**: 202
**Arity**: 3 (accumulated_state, compiled_state, return_channel)

### Usage

```rholang
new mettaRun(\`rho:metta:run\`), result in {
    mettaRun!(accumulatedState, compiledState, *result)
}
```

### Parameters

1. **accumulated_state** - JSON string representing previous state with environment and outputs
2. **compiled_state** - JSON string from `rho:metta:compile:sync`
3. **return_channel** - Channel to receive the new accumulated state

### Returns

JSON object with new accumulated state:
```json
{
  "pending_exprs": [],
  "environment": {"facts_count": N},
  "eval_outputs": [...]
}
```

## Testing

Run the test contract:

```bash
cd /home/dylon/Workspace/f1r3fly.io/f1r3node/rholang
RUSTFLAGS="-C target-cpu=native" cargo build --release --bin rholang-cli

/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli \
  /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration/test_pathmap_state.rho
```

## Files Modified

1. `f1r3node/rholang/src/rust/interpreter/system_processes.rs` - Added `metta_run` handler
2. `f1r3node/models/src/rust/fixed_channels.rs` - Added channel 202
3. `f1r3node/models/src/rust/body_refs.rs` - Added METTA_RUN constant
4. `MeTTa-Compiler/integration/test_pathmap_state.rho` - Test contract

## Next Steps

To complete the integration:

1. Implement JSON deserialization in `run_state_json()` (currently a stub)
2. Update the handler to call the real implementation
3. Test the complete REPL workflow

## Rollback

If you need to rollback the changes:

```bash
# Restore backups (created with timestamp)
ls -lt /home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs.pre-pathmap-*
# Copy the backup you want to restore
```
