#!/bin/bash
# Profile-Guided Optimization (PGO) Build Script
#
# This script builds MeTTaTron with PGO for maximum performance.
# PGO provides 20%+ speedup on heavy workloads by optimizing:
# - Code layout for better instruction cache locality
# - Branch prediction based on actual execution patterns
# - Function inlining based on hot call paths
#
# Usage: ./scripts/build_pgo.sh [workload_file]
#   workload_file: MeTTa file to use for profiling (default: mmverify)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
PGO_DIR="/tmp/pgo-data-$$"  # Use PID for unique directory

# Default workload for profiling
WORKLOAD="${1:-$PROJECT_DIR/examples/mmverify/demo0/verify_demo0.metta}"

echo "=== MeTTaTron PGO Build ==="
echo "Project: $PROJECT_DIR"
echo "Workload: $WORKLOAD"
echo ""

# Verify workload exists
if [[ ! -f "$WORKLOAD" ]]; then
    echo "ERROR: Workload file not found: $WORKLOAD"
    exit 1
fi

# Step 1: Clean and build instrumented binary
echo "Step 1/4: Building instrumented binary..."
mkdir -p "$PGO_DIR"
cd "$PROJECT_DIR"
cargo clean
RUSTFLAGS="-Cprofile-generate=$PGO_DIR -Ctarget-cpu=native" cargo build --release 2>&1 | tail -5

# Step 2: Collect profile data
echo ""
echo "Step 2/4: Collecting profile data..."
./target/release/mettatron "$WORKLOAD" > /dev/null 2>&1
echo "Profile data collected: $(ls -1 $PGO_DIR/*.profraw 2>/dev/null | wc -l) files"

# Step 3: Merge profile data
echo ""
echo "Step 3/4: Merging profile data..."
llvm-profdata merge -o "$PGO_DIR/merged.profdata" "$PGO_DIR"/*.profraw
echo "Merged profile: $(du -h $PGO_DIR/merged.profdata | cut -f1)"

# Step 4: Build optimized binary
echo ""
echo "Step 4/4: Building PGO-optimized binary..."
cargo clean
RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata -Ctarget-cpu=native" cargo build --release 2>&1 | tail -5

# Show binary size comparison
echo ""
echo "=== Build Complete ==="
BINARY="$PROJECT_DIR/target/release/mettatron"
echo "Binary: $BINARY"
echo "Size: $(du -h $BINARY | cut -f1)"

# Cleanup
rm -rf "$PGO_DIR"

echo ""
echo "PGO build complete. Run benchmarks to verify performance:"
echo "  taskset -c 0 time ./target/release/mettatron $WORKLOAD"
