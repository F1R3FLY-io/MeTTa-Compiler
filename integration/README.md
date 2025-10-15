# Rholang Integration

This directory contains templates and documentation for integrating the MeTTa compiler with Rholang.

## Directory Structure

```
integration/
├── README.md              # This file
├── templates/             # Current integration templates (Direct Rust Linking)
│   ├── rholang_handler.rs    # Handler methods for SystemProcesses
│   └── rholang_registry.rs   # Service registration and Definition structs
└── archive/               # Legacy FFI-based approaches (deprecated)
    ├── rholang_handler_v1_ffi.rs
    ├── rholang_handler_v2_ffi.rs
    ├── rholang_registry_v1_ffi.rs
    └── rholang_registry_v2_ffi.rs
```

## Current Integration (Direct Rust Linking with PathMap Par)

The templates in `integration/templates/` implement **direct Rust linking** - no FFI required!

### Integration Status

✅ **Successfully Deployed** to `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/`
✅ **Updated to PathMap Par** - Now uses EPathMap structures instead of JSON

### Files Modified in Rholang

1. **Cargo.toml** - Added mettatron dependency
2. **src/rust/interpreter/system_processes.rs** - Added handlers and registry
3. **src/lib.rs** - Registered MeTTa contracts at runtime

### Services Available

- `rho:metta:compile` (arity 2, channel 200) - Compile MeTTa to PathMap Par
- `rho:metta:compile:sync` (arity 2, channel 201) - Synchronous compile
- `rho:metta:run` (arity 3, channel 202) - Evaluate with state accumulation

### Usage from Rholang

**⚠️ Important:** Services now return **PathMap Par** structures, not JSON strings!

PathMaps are **printable** - use `stdoutAck` to display them:

```rholang
// Compile MeTTa code and print result
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@statePar <- result) {
    // statePar is an EPathMap containing MettaState
    stdoutAck!("Compiled state: ", *ack) |
    for (_ <- ack) {
      stdoutAck!(statePar, *ack) |  // Prints PathMap as {|...|}
      for (_ <- ack) {
        stdoutAck!("\n", *ack)
      }
    }
  }
}

// REPL workflow with state accumulation
new compiled, evaluated in {
  @"rho:metta:compile"!("(+ 10 5)", *compiled) |
  for (@compiledState <- compiled) {
    @"rho:metta:run"!(Nil, compiledState, *evaluated) |
    for (@accumulatedState <- evaluated) {
      stdoutAck!("Result: ", *ack) |
      for (_ <- ack) {
        stdoutAck!(accumulatedState, *ack)
      }
    }
  }
}
```

**Note:** Do NOT use `++` operator on PathMap Par (use separate `stdoutAck` calls).

See **`PATHMAP_PAR_USAGE.md`** for complete usage guide and examples.

### PathMap Par Integration

Functions used:
- `mettatron::metta_state_to_pathmap_par(&MettaState) -> Par` - Convert state to EPathMap
- `mettatron::pathmap_par_to_metta_state(&Par) -> Result<MettaState>` - Deserialize
- `mettatron::run_state(accumulated, compiled) -> Result<MettaState>` - Evaluate

Returns EPathMap Par with structure:
- Element 0: `("pending_exprs", EList([...]))`
- Element 1: `("environment", metadata)`
- Element 2: `("eval_outputs", EList([...]))`

## Documentation

Detailed guides in this directory:

### Quick Start
- `DIRECT_RUST_INTEGRATION.md` - Step-by-step deployment guide
- `DIRECT_RUST_SUMMARY.md` - Quick technical summary
- `DEPLOYMENT_GUIDE.md` - Deployment procedures
- `DEPLOYMENT_CHECKLIST.md` - Pre-deployment checklist

### Technical Details
- `RHOLANG_INTEGRATION_SUMMARY.md` - Technical overview
- `RHOLANG_REGISTRY_PATTERN.md` - Service registration pattern
- `RHOLANG_SYNC_GUIDE.md` - Synchronous operation guide
- `SYNC_OPERATOR_SUMMARY.md` - Understanding the !? operator
- `FFI_VS_DIRECT_COMPARISON.md` - Why we chose direct linking over FFI

### Index
- `RHOLANG_INTEGRATION_INDEX.md` - Complete documentation index

## Archive

The `archive/` directory contains earlier FFI-based approaches. These are kept for reference but are **no longer recommended** as direct Rust linking provides:

- Better type safety
- No unsafe code
- Simpler build process
- No C ABI concerns
- Better performance

---

For the latest documentation, see: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/README.md`
