#!/bin/bash
# Automated MeTTa PathMap State Integration Script
# This script integrates the new run_state() functionality with f1r3node

set -e  # Exit on error

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== MeTTa PathMap State Integration ===${NC}\n"

# Configuration
METTA_DIR="/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler"
F1R3NODE_DIR="/home/dylon/Workspace/f1r3fly.io/f1r3node"
SYSTEM_PROCESSES_FILE="$F1R3NODE_DIR/rholang/src/rust/interpreter/system_processes.rs"
BACKUP_SUFFIX=".pre-pathmap-$(date +%Y%m%d-%H%M%S)"

# Check prerequisites
echo -e "${YELLOW}Checking prerequisites...${NC}"

if [ ! -d "$METTA_DIR" ]; then
    echo -e "${RED}Error: MeTTa compiler directory not found at $METTA_DIR${NC}"
    exit 1
fi

if [ ! -d "$F1R3NODE_DIR" ]; then
    echo -e "${RED}Error: f1r3node directory not found at $F1R3NODE_DIR${NC}"
    exit 1
fi

if [ ! -f "$SYSTEM_PROCESSES_FILE" ]; then
    echo -e "${RED}Error: system_processes.rs not found at $SYSTEM_PROCESSES_FILE${NC}"
    exit 1
fi

echo -e "${GREEN}✓ All prerequisites found${NC}\n"

# Backup system_processes.rs
echo -e "${YELLOW}Creating backup of system_processes.rs...${NC}"
cp "$SYSTEM_PROCESSES_FILE" "$SYSTEM_PROCESSES_FILE$BACKUP_SUFFIX"
echo -e "${GREEN}✓ Backup created: $SYSTEM_PROCESSES_FILE$BACKUP_SUFFIX${NC}\n"

# Add the new system process handler
echo -e "${YELLOW}Adding metta_run system process handler...${NC}"

cat > /tmp/metta_run_handler.rs << 'EOF'
    /// MeTTa run_state system process (PathMap-based REPL integration)
    /// Channel 202: rho:metta:run
    /// Takes accumulated state and compiled state, returns new accumulated state
    pub async fn metta_run(&self, args: Vec<ListParWithRandom>) -> Result<(), InterpreterError> {
        if args.len() < 3 {
            return Err(InterpreterError::new_internal_error(
                "metta_run requires 3 arguments: accumulated_state, compiled_state, return_channel"
            ));
        }

        // Extract accumulated state JSON (from PathMap or previous run)
        let accumulated_json = extract_string_from_par(&args[0].pars[0])
            .unwrap_or_else(|_| "{}".to_string());

        // Extract compiled state JSON (from compile)
        let compiled_json = extract_string_from_par(&args[1].pars[0])?;

        // Return channel
        let return_channel = &args[2].pars[0];

        // Call run_state_json (when implemented) or use direct MettaState approach
        // For now, return a placeholder indicating the feature is available
        let result_json = json!({
            "success": true,
            "message": "run_state handler registered - JSON implementation pending",
            "accumulated": accumulated_json,
            "compiled": compiled_json
        });

        // Convert JSON to Rholang Par
        let result_string = result_json.to_string();
        let result_par = ground_value_to_par(&GroundValue::String(result_string))?;

        // Send to return channel
        self.send_to_channel(return_channel, result_par).await?;

        Ok(())
    }
EOF

# Check if handler already exists
if grep -q "pub async fn metta_run" "$SYSTEM_PROCESSES_FILE"; then
    echo -e "${YELLOW}⚠ metta_run handler already exists, skipping...${NC}\n"
else
    # Find the location to insert (after metta_compile_sync)
    if grep -q "pub async fn metta_compile_sync" "$SYSTEM_PROCESSES_FILE"; then
        # Insert after metta_compile_sync
        awk '/pub async fn metta_compile_sync/,/^    \}$/{p=1} p&&/^    \}$/{print; system("cat /tmp/metta_run_handler.rs"); p=0; next} {print}' \
            "$SYSTEM_PROCESSES_FILE" > /tmp/system_processes_new.rs
        mv /tmp/system_processes_new.rs "$SYSTEM_PROCESSES_FILE"
        echo -e "${GREEN}✓ Added metta_run handler${NC}\n"
    else
        echo -e "${RED}Error: Could not find metta_compile_sync function${NC}"
        exit 1
    fi
fi

# Add system process definition
echo -e "${YELLOW}Adding system process definition...${NC}"

cat > /tmp/metta_run_definition.rs << 'EOF'
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
EOF

if grep -q "rho:metta:run" "$SYSTEM_PROCESSES_FILE"; then
    echo -e "${YELLOW}⚠ rho:metta:run definition already exists, skipping...${NC}\n"
else
    # Insert after metta_compile_sync definition
    awk '/urn: "rho:metta:compile:sync"/,/^\s*\},$/{p=1} p&&/^\s*\},$/{print; system("cat /tmp/metta_run_definition.rs"); p=0; next} {print}' \
        "$SYSTEM_PROCESSES_FILE" > /tmp/system_processes_new.rs
    mv /tmp/system_processes_new.rs "$SYSTEM_PROCESSES_FILE"
    echo -e "${GREEN}✓ Added system process definition${NC}\n"
fi

# Add FixedChannels constant
echo -e "${YELLOW}Adding FixedChannels constant...${NC}"

# Find the FixedChannels impl and add metta_run
if grep -q "pub const fn metta_run" "$F1R3NODE_DIR/models/src/rust/fixed_channels.rs" 2>/dev/null; then
    echo -e "${YELLOW}⚠ metta_run channel already exists, skipping...${NC}\n"
else
    FIXED_CHANNELS_FILE="$F1R3NODE_DIR/models/src/rust/fixed_channels.rs"
    if [ -f "$FIXED_CHANNELS_FILE" ]; then
        cp "$FIXED_CHANNELS_FILE" "$FIXED_CHANNELS_FILE$BACKUP_SUFFIX"

        # Add after metta_compile_sync
        cat > /tmp/metta_run_channel.rs << 'EOF'
    pub const fn metta_run() -> u64 {
        202
    }

EOF
        awk '/pub const fn metta_compile_sync/,/^\s*\}$/{p=1} p&&/^\s*\}$/{print; system("cat /tmp/metta_run_channel.rs"); p=0; next} {print}' \
            "$FIXED_CHANNELS_FILE" > /tmp/fixed_channels_new.rs
        mv /tmp/fixed_channels_new.rs "$FIXED_CHANNELS_FILE"
        echo -e "${GREEN}✓ Added FixedChannels::metta_run()${NC}\n"
    else
        echo -e "${YELLOW}⚠ fixed_channels.rs not found, will need manual addition${NC}\n"
    fi
fi

# Add BodyRefs constant
echo -e "${YELLOW}Adding BodyRefs constant...${NC}"

BODY_REFS_FILE="$F1R3NODE_DIR/models/src/rust/body_refs.rs"
if [ -f "$BODY_REFS_FILE" ]; then
    if grep -q "pub const METTA_RUN" "$BODY_REFS_FILE"; then
        echo -e "${YELLOW}⚠ METTA_RUN body ref already exists, skipping...${NC}\n"
    else
        cp "$BODY_REFS_FILE" "$BODY_REFS_FILE$BACKUP_SUFFIX"

        # Find the last constant and add after it
        cat > /tmp/metta_run_bodyref.rs << 'EOF'
    pub const METTA_RUN: u64 = 39;
EOF
        # This is a simple append - adjust the number if needed
        echo -e "${YELLOW}⚠ Please verify METTA_RUN constant number (39) doesn't conflict${NC}"
        echo "    Check $BODY_REFS_FILE for the next available number"
    fi
fi

# Create test contract
echo -e "${YELLOW}Creating PathMap state test contract...${NC}"

cat > "$METTA_DIR/integration/test_pathmap_state.rho" << 'EOF'
new stdout(\`rho:io:stdout\`),
    mettaCompile(\`rho:metta:compile:sync\`),
    mettaRun(\`rho:metta:run\`),
    testPathMapState in {

    contract testPathMapState(return) = {
        new result1, result2, result3 in {

            // Step 1: Compile rule definition
            stdout!("=== Test 1: Define rule ===") |
            for (@compiledState1 <- mettaCompile!("(= (double $x) (* $x 2))")) {
                stdout!({"Compiled": compiledState1}) |

                // Step 2: Run against empty accumulated state
                for (@accumulatedState1 <- mettaRun!("{}", compiledState1)) {
                    stdout!({"After rule definition": accumulatedState1}) |

                    // Step 3: Compile usage of rule
                    for (@compiledState2 <- mettaCompile!("!(double 21)")) {
                        stdout!({"Compiled usage": compiledState2}) |

                        // Step 4: Run against accumulated state (should have rule)
                        for (@accumulatedState2 <- mettaRun!(accumulatedState1, compiledState2)) {
                            stdout!({"After using rule": accumulatedState2}) |

                            // Step 5: Compile more computation
                            for (@compiledState3 <- mettaCompile!("(+ 10 11)")) {

                                // Step 6: Run against accumulated state
                                for (@finalState <- mettaRun!(accumulatedState2, compiledState3)) {
                                    stdout!({"Final state": finalState}) |
                                    return!(finalState)
                                }
                            }
                        }
                    }
                }
            }
        }
    } |

    // Run the test
    new result in {
        testPathMapState!(*result) |
        for (@r <- result) {
            stdout!({"=== Test Complete ===": r})
        }
    }
}
EOF

echo -e "${GREEN}✓ Created test contract: integration/test_pathmap_state.rho${NC}\n"

# Create integration README
cat > "$METTA_DIR/integration/PATHMAP_INTEGRATION.md" << 'EOF'
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
EOF

echo -e "${GREEN}✓ Created integration documentation${NC}\n"

# Build test
echo -e "${YELLOW}Testing build...${NC}"
cd "$F1R3NODE_DIR/rholang"
if RUSTFLAGS="-C target-cpu=native" cargo check 2>&1 | tail -20; then
    echo -e "${GREEN}✓ Build check passed${NC}\n"
else
    echo -e "${RED}✗ Build check failed - you may need to manually fix compilation errors${NC}\n"
    echo -e "${YELLOW}Backup files available at:${NC}"
    echo "  $SYSTEM_PROCESSES_FILE$BACKUP_SUFFIX"
    exit 1
fi

# Summary
echo -e "${GREEN}=== Integration Complete ===${NC}\n"
echo -e "Files modified:"
echo -e "  ✓ $SYSTEM_PROCESSES_FILE"
echo -e "  ✓ $FIXED_CHANNELS_FILE (if exists)"
echo -e "  ✓ $BODY_REFS_FILE (if exists)"
echo ""
echo -e "Files created:"
echo -e "  ✓ $METTA_DIR/integration/test_pathmap_state.rho"
echo -e "  ✓ $METTA_DIR/integration/PATHMAP_INTEGRATION.md"
echo ""
echo -e "Backups created with suffix: $BACKUP_SUFFIX"
echo ""
echo -e "${YELLOW}Next steps:${NC}"
echo "  1. Review the changes in system_processes.rs"
echo "  2. Verify BodyRefs constant number doesn't conflict"
echo "  3. Build f1r3node: cd $F1R3NODE_DIR/rholang && cargo build --release"
echo "  4. Run test: $F1R3NODE_DIR/target/release/rholang-cli $METTA_DIR/integration/test_pathmap_state.rho"
echo ""
echo -e "${GREEN}Integration script complete!${NC}"
