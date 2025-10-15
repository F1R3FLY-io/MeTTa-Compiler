# PathMap State Integration - Quick Start

## Automated Integration

Run the automated integration script to add PathMap state support to f1r3node:

```bash
cd /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration
./integrate.sh
```

The script will:
- ✅ Backup existing files with timestamps
- ✅ Add `metta_run` system process handler
- ✅ Register `rho:metta:run` at channel 202
- ✅ Create test contract for PathMap state workflow
- ✅ Generate documentation
- ✅ Verify the build compiles

## What Gets Added

### 1. System Process Handler

**Location**: `f1r3node/rholang/src/rust/interpreter/system_processes.rs`

```rust
pub async fn metta_run(&self, args: Vec<ListParWithRandom>) -> Result<(), InterpreterError>
```

- **Channel**: 202
- **URN**: `rho:metta:run`
- **Arity**: 3 (accumulated_state, compiled_state, return_channel)

### 2. System Process Registration

Adds the `rho:metta:run` service alongside existing `rho:metta:compile` services.

### 3. Test Contract

**Location**: `MeTTa-Compiler/integration/test_pathmap_state.rho`

Demonstrates full REPL workflow:
1. Compile rule definition
2. Run against empty state
3. Compile rule usage
4. Run against accumulated state (rule should work)
5. Compile more computation
6. Run against accumulated state (all outputs preserved)

## Manual Integration

If you prefer manual integration, follow these steps:

### Step 1: Add Handler Function

In `f1r3node/rholang/src/rust/interpreter/system_processes.rs`, add after `metta_compile_sync`:

```rust
pub async fn metta_run(&self, args: Vec<ListParWithRandom>) -> Result<(), InterpreterError> {
    if args.len() < 3 {
        return Err(InterpreterError::new_internal_error(
            "metta_run requires 3 arguments"
        ));
    }

    let accumulated_json = extract_string_from_par(&args[0].pars[0])
        .unwrap_or_else(|_| "{}".to_string());
    let compiled_json = extract_string_from_par(&args[1].pars[0])?;
    let return_channel = &args[2].pars[0];

    // TODO: Call mettatron::run_state_json when JSON deserialization is implemented
    let result_json = json!({
        "success": true,
        "message": "run_state handler active",
        "accumulated": accumulated_json,
        "compiled": compiled_json
    });

    let result_string = result_json.to_string();
    let result_par = ground_value_to_par(&GroundValue::String(result_string))?;
    self.send_to_channel(return_channel, result_par).await?;

    Ok(())
}
```

### Step 2: Register System Process

In the same file, add to the `definitions` list:

```rust
Definition {
    urn: "rho:metta:run".to_string(),
    fixed_channel: FixedChannels::metta_run(),
    arity: 3,
    body_ref: BodyRefs::METTA_RUN,
    handler: Box::new(|ctx| {
        Box::new(move |args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                ctx.system_processes.clone().metta_run(args).await
            })
        })
    }),
    remainder: None,
},
```

### Step 3: Add Fixed Channel

In `f1r3node/models/src/rust/fixed_channels.rs`:

```rust
pub const fn metta_run() -> u64 {
    202
}
```

### Step 4: Add Body Reference

In `f1r3node/models/src/rust/body_refs.rs`:

```rust
pub const METTA_RUN: u64 = 39;  // Adjust number as needed
```

## Testing

### Build

```bash
cd /home/dylon/Workspace/f1r3fly.io/f1r3node/rholang
RUSTFLAGS="-C target-cpu=native" cargo build --release --bin rholang-cli
```

### Run Test Contract

```bash
/home/dylon/Workspace/f1r3fly.io/f1r3node/target/release/rholang-cli \
  /home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration/test_pathmap_state.rho
```

### Expected Output

```
=== Test 1: Define rule ===
{"Compiled": {"success": true, "exprs": [...]}}
{"After rule definition": {"success": true, "message": "..."}}
{"Compiled usage": {"success": true, "exprs": [...]}}
{"After using rule": {"success": true, "message": "..."}}
{"Final state": {"success": true, "message": "..."}}
=== Test Complete ===
```

## Usage in Rholang

```rholang
new stdout(\`rho:io:stdout\`),
    mettaCompile(\`rho:metta:compile:sync\`),
    mettaRun(\`rho:metta:run\`) in {

    // Initialize REPL state
    new replState in {
        replState!("{}") |  // Empty initial state

        // User input 1: Define rule
        for (@currentState <- replState) {
            for (@compiled <- mettaCompile!("(= (double $x) (* $x 2))")) {
                for (@newState <- mettaRun!(currentState, compiled)) {
                    replState!(newState) |

                    // User input 2: Use rule
                    for (@currentState2 <- replState) {
                        for (@compiled2 <- mettaCompile!("!(double 21)")) {
                            for (@result <- mettaRun!(currentState2, compiled2)) {
                                stdout!(result)  // Should show 42 in outputs
                            }
                        }
                    }
                }
            }
        }
    }
}
```

## Rollback

Backups are created with timestamp suffix:

```bash
# List backups
ls -lt /home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs.pre-pathmap-*

# Restore a backup
cp system_processes.rs.pre-pathmap-YYYYMMDD-HHMMSS system_processes.rs
```

## Next Steps

1. ✅ Run automated integration script
2. ✅ Build f1r3node
3. ✅ Run test contract
4. 🔄 Implement JSON deserialization in `run_state_json()`
5. 🔄 Update handler to use real implementation
6. 🔄 Test complete REPL workflow with state persistence

## Architecture

The integration follows the same pattern as existing MeTTa services:

```
Rholang Contract
    ↓
\`rho:metta:run\` (channel 202)
    ↓
system_processes.metta_run()
    ↓
mettatron::run_state() [Rust native call]
    ↓
Return accumulated state as JSON
```

## Documentation

- **Design**: `docs/design/PATHMAP_STATE_DESIGN.md`
- **Integration Status**: `integration/INTEGRATION_STATUS.md`
- **Test Contract**: `integration/test_pathmap_state.rho`

## Support

For issues or questions:
1. Check build errors in cargo output
2. Review backup files if integration fails
3. Verify channel numbers don't conflict
4. Check INTEGRATION_STATUS.md for existing integration details
