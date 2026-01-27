#!/bin/bash
# debug_mmverify_sigill.sh - Debug SIGILL in mmverify_benchmark
#
# This script builds the benchmark with debug symbols and runs it under GDB
# to catch and diagnose SIGILL (Illegal Instruction) crashes.
#
# Usage: ./scripts/debug_mmverify_sigill.sh
#
# Output files:
#   /tmp/sigill_debug.log     - GDB logging output
#   /tmp/sigill_gdb_output.txt - Full GDB session output

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Configuration
NUM_THREADS="${METTATRON_NUM_THREADS:-18}"
CPU_AFFINITY="${CPU_AFFINITY:-0-17}"
LOG_FILE="/tmp/sigill_debug.log"
OUTPUT_FILE="/tmp/sigill_gdb_output.txt"

echo "=== SIGILL Debug Script for mmverify_benchmark ==="
echo "Project directory: $PROJECT_DIR"
echo "Threads: $NUM_THREADS"
echo "CPU affinity: $CPU_AFFINITY"
echo ""

# Build with debug symbols
echo "Building with release-with-debug profile..."
cargo build --profile release-with-debug --bench mmverify_benchmark

# Find benchmark binary
BENCH_BIN=$(find target/release-with-debug/deps -name 'mmverify_benchmark-*' -executable -type f 2>/dev/null | head -1)
if [ -z "$BENCH_BIN" ]; then
    echo "ERROR: Could not find benchmark binary in target/release-with-debug/deps/"
    echo "Looking for any executable matching pattern..."
    find target/ -name 'mmverify_benchmark*' -type f 2>/dev/null || true
    exit 1
fi
echo "Benchmark binary: $BENCH_BIN"
echo ""

# Create GDB script
GDB_SCRIPT=$(mktemp --suffix=.gdb)
cat > "$GDB_SCRIPT" << 'GDBEOF'
# debug_sigill.gdb - Catch SIGILL and dump full diagnostics
set pagination off
set logging file /tmp/sigill_debug.log
set logging overwrite on
set logging enabled on

# Don't let GDB handle SIGILL - we want to catch it
handle SIGILL stop print nopass

# Catch the signal
catch signal SIGILL
commands
  printf "\n"
  printf "================================================================================\n"
  printf "=== SIGILL (Illegal Instruction) CAUGHT ===\n"
  printf "================================================================================\n"

  printf "\n--- Faulting Program Counter ---\n"
  printf "PC = %p\n", $pc

  printf "\n--- Disassembly around fault (20 bytes before, 16 after) ---\n"
  x/5i $pc-20
  x/5i $pc-10
  printf ">>> FAULT: "
  x/i $pc
  x/4i $pc+1

  printf "\n--- Stack Trace (full) ---\n"
  bt full

  printf "\n--- Registers ---\n"
  info registers

  printf "\n--- All Threads Backtraces ---\n"
  thread apply all bt

  printf "\n--- Current Frame Info ---\n"
  info frame

  printf "\n--- Local Variables (if available) ---\n"
  info locals

  printf "\n--- Function Arguments (if available) ---\n"
  info args

  printf "\n--- Memory Mappings (to identify JIT vs static code) ---\n"
  info proc mappings

  printf "\n--- Shared Libraries ---\n"
  info sharedlibrary

  printf "\n--- Extended disassembly of faulting function ---\n"
  disassemble

  printf "\n================================================================================\n"
  printf "=== END SIGILL DIAGNOSTICS ===\n"
  printf "================================================================================\n"

  quit 1
end

# Also catch SIGSEGV in case that's involved
handle SIGSEGV stop print nopass
catch signal SIGSEGV
commands
  printf "\n=== SIGSEGV CAUGHT (might be related to SIGILL) ===\n"
  printf "PC = %p\n", $pc
  bt full
  info registers
  quit 1
end

# Run the program
run --bench
GDBEOF

echo "GDB script created at: $GDB_SCRIPT"
echo ""

# Run under GDB
echo "Running under GDB with SIGILL trap..."
echo "Output will be saved to:"
echo "  - $LOG_FILE (GDB log)"
echo "  - $OUTPUT_FILE (full output)"
echo ""
echo "Starting benchmark..."
echo ""

# Set environment and run
METTATRON_NUM_THREADS="$NUM_THREADS" \
taskset -c "$CPU_AFFINITY" \
gdb -batch -x "$GDB_SCRIPT" "$BENCH_BIN" 2>&1 | tee "$OUTPUT_FILE"

EXIT_CODE=${PIPESTATUS[0]}

# Cleanup
rm -f "$GDB_SCRIPT"

echo ""
echo "=== Debug session complete ==="
echo "Exit code: $EXIT_CODE"
echo ""
echo "Output files:"
echo "  - GDB log: $LOG_FILE"
echo "  - Full output: $OUTPUT_FILE"
echo ""

if [ -f "$LOG_FILE" ]; then
    echo "=== Key info from log ==="
    # Extract the faulting PC and instruction if present
    if grep -q "SIGILL" "$LOG_FILE"; then
        echo "SIGILL was caught. Key details:"
        grep -A2 "Faulting Program Counter" "$LOG_FILE" 2>/dev/null || true
        grep -A3 ">>> FAULT" "$LOG_FILE" 2>/dev/null || true
    else
        echo "No SIGILL was caught - benchmark may have completed successfully or failed differently."
    fi
fi

exit $EXIT_CODE
