#!/bin/bash
# Comprehensive eval profiling with Linux perf
#
# This script profiles lazy evaluation, trampolining, TCO, and Cartesian products
# with comprehensive perf instrumentation for identifying performance bottlenecks.
#
# Features:
# - CPU affinity and frequency locking for reproducibility
# - CPU cycle profiling with DWARF call graphs
# - Cache miss analysis (L1, LLC)
# - Branch misprediction analysis
# - IPC (Instructions Per Cycle) analysis
# - Flamegraph generation
#
# Usage: ./scripts/profile_eval.sh [benchmark_filter] [duration_secs]
#   benchmark_filter: Optional regex to filter benchmarks (e.g., "cartesian")
#   duration_secs: Optional duration for perf profiling (default: 60)
#
# Examples:
#   ./scripts/profile_eval.sh                     # Run all benchmarks
#   ./scripts/profile_eval.sh cartesian 30        # Profile cartesian tests for 30s
#   ./scripts/profile_eval.sh trampoline          # Profile trampoline tests

set -e

# Configuration
BENCH_NAME="eval_profiling"
BENCH_FILTER="${1:-}"
DURATION="${2:-60}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTPUT_DIR="docs/optimization/eval_${TIMESTAMP}"

# CPU configuration for Intel Xeon E5-2699 v3
CPU_CORES="0-17"        # Socket 1 cores (avoid NUMA effects)
BASE_FREQ="2300000"     # 2.3 GHz base clock
MAX_FREQ="3570000"      # 3.57 GHz max turbo

echo "=== MeTTaTron Eval Profiling Suite ==="
echo "Benchmark: $BENCH_NAME"
echo "Filter: ${BENCH_FILTER:-<all>}"
echo "Duration: ${DURATION}s"
echo "CPU Cores: $CPU_CORES"
echo "Output: $OUTPUT_DIR"
echo ""

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Check dependencies
SKIP_CPUPOWER=""
SKIP_PERF=""
SKIP_FLAMEGRAPH=""

if ! command -v cpupower &> /dev/null; then
    echo "Warning: cpupower not found. Skipping CPU frequency locking."
    echo "Install with: sudo pacman -S linux-tools"
    SKIP_CPUPOWER=1
fi

if ! command -v perf &> /dev/null; then
    echo "Warning: perf not found. Skipping perf profiling."
    echo "Install with: sudo pacman -S perf"
    SKIP_PERF=1
fi

if ! command -v cargo-flamegraph &> /dev/null; then
    echo "Warning: cargo-flamegraph not found. Skipping flamegraph generation."
    echo "Install with: cargo install flamegraph"
    SKIP_FLAMEGRAPH=1
fi

echo ""

# Set CPU governor and frequency
if [ -z "$SKIP_CPUPOWER" ]; then
    echo "==> Setting CPU governor to performance..."
    sudo cpupower frequency-set -g performance > /dev/null 2>&1

    echo "==> Locking CPU frequency to base clock (2.3 GHz)..."
    sudo cpupower frequency-set -d $BASE_FREQ -u $BASE_FREQ > /dev/null 2>&1

    echo "==> CPU frequency locked. Current settings:"
    cpupower frequency-info | grep -E "current CPU frequency|current policy" || true
    echo ""
fi

# Build with profiling profile
echo "==> Building with profiling profile..."
cargo build --profile=profiling --benches 2>&1 | tail -5

# Find benchmark binary
BENCH_BIN=$(find target/profiling/deps -name "${BENCH_NAME}*" -type f -executable 2>/dev/null | head -1)
if [ -z "$BENCH_BIN" ]; then
    echo "Error: Could not find benchmark binary"
    exit 1
fi
echo "Binary: $BENCH_BIN"
echo ""

# Run baseline benchmarks
echo "==> Running baseline benchmarks..."
if [ -n "$BENCH_FILTER" ]; then
    taskset -c $CPU_CORES cargo bench --bench $BENCH_NAME -- "$BENCH_FILTER" 2>&1 | tee "$OUTPUT_DIR/criterion_baseline.txt"
else
    taskset -c $CPU_CORES cargo bench --bench $BENCH_NAME 2>&1 | tee "$OUTPUT_DIR/criterion_baseline.txt"
fi

# Perf profiling
if [ -z "$SKIP_PERF" ]; then
    echo ""
    echo "==> Profiling with perf (${DURATION}s)..."

    # Build filter args for cargo bench
    FILTER_ARGS=""
    if [ -n "$BENCH_FILTER" ]; then
        FILTER_ARGS="-- $BENCH_FILTER"
    fi

    # CPU cycle profiling with call graph
    taskset -c $CPU_CORES perf record \
        --freq=997 \
        --call-graph=dwarf,32768 \
        --output="$OUTPUT_DIR/perf_cycles.data" \
        -- cargo bench --bench $BENCH_NAME $FILTER_ARGS --profile-time $DURATION 2>&1 | tail -20

    # Generate text report
    echo "==> Generating perf report..."
    perf report \
        --input="$OUTPUT_DIR/perf_cycles.data" \
        --stdio \
        --sort=dso,symbol \
        --max-stack=20 \
        > "$OUTPUT_DIR/perf_report.txt" 2>&1

    # Top hotspots summary
    echo "==> Top 20 hotspots:"
    perf report \
        --input="$OUTPUT_DIR/perf_cycles.data" \
        --stdio \
        --no-children \
        --percent-limit=1.0 \
        2>/dev/null | head -40

    echo ""
    echo "==> Cache miss analysis..."
    taskset -c $CPU_CORES perf stat \
        -e cache-references,cache-misses,L1-dcache-load-misses,L1-dcache-loads,LLC-load-misses,LLC-loads \
        -- cargo bench --bench $BENCH_NAME $FILTER_ARGS --profile-time 10 \
        2>&1 | tee "$OUTPUT_DIR/cache_stats.txt"

    echo ""
    echo "==> Branch prediction analysis..."
    taskset -c $CPU_CORES perf stat \
        -e branches,branch-misses \
        -- cargo bench --bench $BENCH_NAME $FILTER_ARGS --profile-time 10 \
        2>&1 | tee "$OUTPUT_DIR/branch_stats.txt"

    echo ""
    echo "==> IPC analysis..."
    taskset -c $CPU_CORES perf stat \
        -e cycles,instructions,cpu-clock,task-clock \
        -- cargo bench --bench $BENCH_NAME $FILTER_ARGS --profile-time 10 \
        2>&1 | tee "$OUTPUT_DIR/ipc_stats.txt"

    echo ""
    echo "==> Memory allocation analysis..."
    taskset -c $CPU_CORES perf stat \
        -e page-faults,minor-faults,major-faults \
        -- cargo bench --bench $BENCH_NAME $FILTER_ARGS --profile-time 10 \
        2>&1 | tee "$OUTPUT_DIR/memory_stats.txt"
fi

# Flamegraph generation
if [ -z "$SKIP_FLAMEGRAPH" ]; then
    echo ""
    echo "==> Generating flamegraph..."

    # Build flamegraph args
    FLAMEGRAPH_ARGS=""
    if [ -n "$BENCH_FILTER" ]; then
        FLAMEGRAPH_ARGS="-- --bench $BENCH_FILTER"
    else
        FLAMEGRAPH_ARGS="-- --bench"
    fi

    taskset -c $CPU_CORES cargo flamegraph \
        --bench $BENCH_NAME \
        --profile=profiling \
        --output="$OUTPUT_DIR/flamegraph.svg" \
        $FLAMEGRAPH_ARGS 2>&1 | tail -10

    echo "  -> Flamegraph: $OUTPUT_DIR/flamegraph.svg"
fi

# Restore CPU frequency
if [ -z "$SKIP_CPUPOWER" ]; then
    echo ""
    echo "==> Restoring CPU frequency settings..."
    sudo cpupower frequency-set -d $BASE_FREQ -u $MAX_FREQ > /dev/null 2>&1
fi

# Generate summary
echo ""
echo "=== Profiling Complete! ==="
echo ""
echo "Results directory: $OUTPUT_DIR/"
echo ""
echo "Files generated:"
[ -f "$OUTPUT_DIR/criterion_baseline.txt" ] && echo "  - criterion_baseline.txt (benchmark timings)"
[ -f "$OUTPUT_DIR/perf_report.txt" ] && echo "  - perf_report.txt (CPU cycle hotspots)"
[ -f "$OUTPUT_DIR/perf_cycles.data" ] && echo "  - perf_cycles.data (raw perf data for analysis)"
[ -f "$OUTPUT_DIR/cache_stats.txt" ] && echo "  - cache_stats.txt (L1/LLC cache statistics)"
[ -f "$OUTPUT_DIR/branch_stats.txt" ] && echo "  - branch_stats.txt (branch prediction stats)"
[ -f "$OUTPUT_DIR/ipc_stats.txt" ] && echo "  - ipc_stats.txt (instructions per cycle)"
[ -f "$OUTPUT_DIR/memory_stats.txt" ] && echo "  - memory_stats.txt (page fault statistics)"
[ -f "$OUTPUT_DIR/flamegraph.svg" ] && echo "  - flamegraph.svg (visual call graph)"
echo ""
echo "Next steps:"
echo "1. Open flamegraph: firefox $OUTPUT_DIR/flamegraph.svg"
echo "2. Review hotspots: less $OUTPUT_DIR/perf_report.txt"
echo "3. Check cache efficiency: cat $OUTPUT_DIR/cache_stats.txt"
echo "4. Identify optimization targets (functions >5% CPU)"
echo "5. Document findings in scientific journal"
echo ""
echo "For interactive perf analysis:"
echo "  perf report -i $OUTPUT_DIR/perf_cycles.data"
