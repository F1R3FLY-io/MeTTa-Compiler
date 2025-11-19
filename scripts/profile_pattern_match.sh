#!/bin/bash
# Profile pattern_match with perf and flamegraph
#
# This script provides comprehensive profiling of pattern matching performance:
# - Sets CPU affinity to Socket 1 cores for consistent NUMA access
# - Locks CPU frequency to base clock (2.3 GHz) for reproducible results
# - Generates perf data, flamegraphs, and criterion reports
#
# Usage: ./scripts/profile_pattern_match.sh [bench_name]
#   bench_name: Optional, defaults to "pattern_match"

set -e

# Configuration
BENCH_NAME="${1:-pattern_match}"
DURATION=60  # seconds for profiling run
CPU_CORES="0-17"  # Socket 1 cores (NUMA node 0)
BASE_FREQ="2300000"  # 2.3 GHz base clock
MAX_FREQ="3570000"   # 3.57 GHz max turbo (for restore)

echo "=== Pattern Match Profiling Script ==="
echo "Benchmark: $BENCH_NAME"
echo "Duration: ${DURATION}s"
echo "CPU Cores: $CPU_CORES"
echo ""

# Check if running as root for cpupower
if ! command -v cpupower &> /dev/null; then
    echo "Warning: cpupower not found. Skipping CPU frequency locking."
    echo "Install with: sudo apt-get install linux-tools-common linux-tools-generic"
    SKIP_CPUPOWER=1
fi

# Set CPU governor and frequency
if [ -z "$SKIP_CPUPOWER" ]; then
    echo "==> Setting CPU governor to performance..."
    sudo cpupower frequency-set -g performance > /dev/null 2>&1

    echo "==> Locking CPU frequency to base clock (2.3 GHz)..."
    sudo cpupower frequency-set -d $BASE_FREQ -u $BASE_FREQ > /dev/null 2>&1

    echo "==> CPU frequency locked. Current settings:"
    cpupower frequency-info | grep "current CPU frequency"
    echo ""
fi

# Build optimized binary with debug symbols
echo "==> Building optimized binary with debug symbols..."
cargo build --profile=profiling --benches

# Run baseline benchmark
echo "==> Running baseline benchmark..."
cargo bench --bench $BENCH_NAME -- --save-baseline before-opt

# Profile with perf (if available)
if command -v perf &> /dev/null; then
    echo "==> Profiling with perf (${DURATION}s)..."
    taskset -c $CPU_CORES perf record \
        --freq=997 \
        --call-graph=dwarf \
        --output=perf.data \
        cargo bench --bench $BENCH_NAME -- --profile-time $DURATION

    echo "==> Generating perf report..."
    perf report --stdio > docs/optimization/pattern_match_perf_report.txt
    echo "  → Perf report: docs/optimization/pattern_match_perf_report.txt"
else
    echo "Warning: perf not found. Skipping perf profiling."
    echo "Install with: sudo apt-get install linux-tools-common linux-tools-generic"
fi

# Generate flamegraph (if cargo-flamegraph is installed)
if command -v cargo-flamegraph &> /dev/null; then
    echo "==> Generating flamegraph..."
    taskset -c $CPU_CORES cargo flamegraph \
        --bench $BENCH_NAME \
        --output=docs/optimization/pattern_match_flamegraph.svg \
        -- --bench
    echo "  → Flamegraph: docs/optimization/pattern_match_flamegraph.svg"
else
    echo "Warning: cargo-flamegraph not found. Skipping flamegraph generation."
    echo "Install with: cargo install flamegraph"
fi

# Restore CPU frequency
if [ -z "$SKIP_CPUPOWER" ]; then
    echo "==> Restoring CPU frequency settings..."
    sudo cpupower frequency-set -d $BASE_FREQ -u $MAX_FREQ > /dev/null 2>&1
fi

echo ""
echo "=== Profiling Complete! ==="
echo ""
echo "Results:"
echo "  - Baseline: target/criterion/$BENCH_NAME/before-opt/"
echo "  - Criterion report: target/criterion/report/index.html"
if [ -f docs/optimization/pattern_match_perf_report.txt ]; then
    echo "  - Perf report: docs/optimization/pattern_match_perf_report.txt"
fi
if [ -f docs/optimization/pattern_match_flamegraph.svg ]; then
    echo "  - Flamegraph: docs/optimization/pattern_match_flamegraph.svg"
fi
echo ""
echo "Next steps:"
echo "1. Open flamegraph in browser to identify bottlenecks"
echo "2. Review perf report for detailed CPU usage"
echo "3. Implement optimizations targeting functions >10% CPU"
echo "4. Re-run this script to compare results"
