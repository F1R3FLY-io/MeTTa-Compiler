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

## Current Integration (Direct Rust Linking)

The templates in `integration/templates/` implement **direct Rust linking** - no FFI required!

### Integration Status

✅ **Successfully Deployed** to `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/`

### Files Modified in Rholang

1. **Cargo.toml** - Added mettatron dependency
2. **src/rust/interpreter/system_processes.rs** - Added handlers and registry
3. **src/lib.rs** - Registered MeTTa contracts at runtime

### Services Available

- `rho:metta:compile` (arity 2) - Traditional pattern with explicit return channel
- `rho:metta:compile:sync` (arity 2) - Synchronous pattern with implicit return

### Usage from Rholang

```rholang
// Traditional Pattern
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@json <- result) {
    stdoutAck!(json, *ack)
  }
}

// Synchronous Pattern with !?
@"rho:metta:compile:sync" !? ("(+ 1 2)", *ack) ; {
  stdoutAck!("Compilation complete", *ack)
}
```

### Compilation Function

Both services use: `mettatron::rholang_integration::compile_safe(&str) -> String`

Returns JSON with compiled AST or error message.

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
