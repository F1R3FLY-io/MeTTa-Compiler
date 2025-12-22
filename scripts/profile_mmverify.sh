#!/bin/bash
# Comprehensive mmverify profiling with Linux perf
#
# This script profiles the Metamath proof verifier (mmverify) demo to identify
# optimization opportunities in MeTTaTron. The mmverify demo verifies theorem
# th1 (t = t) from the demo0.mm Metamath database.
#
# Features:
# - CPU affinity and frequency locking for reproducibility
# - CPU cycle profiling with DWARF call graphs
# - Cache miss analysis (L1, LLC)
# - Branch misprediction analysis
# - IPC (Instructions Per Cycle) analysis
# - Flamegraph generation
#
# Usage: ./scripts/profile_mmverify.sh [duration_secs]
#   duration_secs: Optional duration for perf profiling (default: 120)
#
# Examples:
#   ./scripts/profile_mmverify.sh         # Profile for default 120 seconds
#   ./scripts/profile_mmverify.sh 60      # Profile for 60 seconds

set -e

# Configuration
BENCH_NAME="mmverify_benchmark"
DURATION="${1:-120}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTPUT_DIR="docs/optimization/mmverify_${TIMESTAMP}"

# CPU configuration for Intel Xeon E5-2699 v3
CPU_CORES="0-17"        # Socket 1 cores (avoid NUMA effects)
BASE_FREQ="2300000"     # 2.3 GHz base clock
MAX_FREQ="3570000"      # 3.57 GHz max turbo

echo "=== MeTTaTron mmverify Profiling Suite ==="
echo "Benchmark: $BENCH_NAME"
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

# Run baseline benchmarks first
echo "==> Running baseline benchmarks (30s warmup, 100 samples, 30s measurement)..."
taskset -c $CPU_CORES cargo bench --bench $BENCH_NAME 2>&1 | tee "$OUTPUT_DIR/criterion_baseline.txt"

# Extract baseline metrics
echo ""
echo "==> Extracting baseline metrics..."
if grep -q "verify_demo0_complete" "$OUTPUT_DIR/criterion_baseline.txt"; then
    grep -A 4 "verify_demo0_complete" "$OUTPUT_DIR/criterion_baseline.txt" | head -5 | tee "$OUTPUT_DIR/baseline_summary.txt"
fi

# Perf profiling
if [ -z "$SKIP_PERF" ]; then
    echo ""
    echo "==> Profiling with perf (${DURATION}s)..."

    # CPU cycle profiling with call graph (main profiling pass)
    echo "  -> Recording CPU cycles with call graphs..."
    taskset -c $CPU_CORES perf record \
        --freq=997 \
        --call-graph=dwarf,32768 \
        --output="$OUTPUT_DIR/perf_cycles.data" \
        -- cargo bench --bench $BENCH_NAME -- --profile-time $DURATION 2>&1 | tail -20

    # Generate text report
    echo ""
    echo "==> Generating perf report..."
    perf report \
        --input="$OUTPUT_DIR/perf_cycles.data" \
        --stdio \
        --sort=dso,symbol \
        --max-stack=20 \
        > "$OUTPUT_DIR/perf_report.txt" 2>&1

    # Top hotspots summary
    echo ""
    echo "==> Top 30 hotspots:"
    perf report \
        --input="$OUTPUT_DIR/perf_cycles.data" \
        --stdio \
        --no-children \
        --percent-limit=0.5 \
        2>/dev/null | head -60 | tee "$OUTPUT_DIR/top_hotspots.txt"

    # Detailed cache miss analysis
    echo ""
    echo "==> Cache miss analysis (30s)..."
    taskset -c $CPU_CORES perf stat \
        -e cache-references,cache-misses,L1-dcache-load-misses,L1-dcache-loads,L1-dcache-store-misses,L1-dcache-stores,LLC-load-misses,LLC-loads,LLC-store-misses,LLC-stores \
        -- cargo bench --bench $BENCH_NAME -- --profile-time 30 \
        2>&1 | tee "$OUTPUT_DIR/cache_stats.txt"

    # Branch prediction analysis
    echo ""
    echo "==> Branch prediction analysis (30s)..."
    taskset -c $CPU_CORES perf stat \
        -e branches,branch-misses \
        -- cargo bench --bench $BENCH_NAME -- --profile-time 30 \
        2>&1 | tee "$OUTPUT_DIR/branch_stats.txt"

    # IPC analysis
    echo ""
    echo "==> IPC analysis (30s)..."
    taskset -c $CPU_CORES perf stat \
        -e cycles,instructions,cpu-clock,task-clock,ref-cycles,bus-cycles \
        -- cargo bench --bench $BENCH_NAME -- --profile-time 30 \
        2>&1 | tee "$OUTPUT_DIR/ipc_stats.txt"

    # Memory allocation analysis
    echo ""
    echo "==> Memory allocation analysis (30s)..."
    taskset -c $CPU_CORES perf stat \
        -e page-faults,minor-faults,major-faults,context-switches,cpu-migrations \
        -- cargo bench --bench $BENCH_NAME -- --profile-time 30 \
        2>&1 | tee "$OUTPUT_DIR/memory_stats.txt"

    # Annotate top functions
    echo ""
    echo "==> Annotating top 5 hotspot functions..."
    TOP_FUNCS=$(perf report --input="$OUTPUT_DIR/perf_cycles.data" --stdio --no-children --percent-limit=2.0 2>/dev/null | grep -E "^\s+[0-9]" | head -5 | awk '{print $NF}')
    for func in $TOP_FUNCS; do
        echo "  -> Annotating: $func"
        perf annotate --input="$OUTPUT_DIR/perf_cycles.data" --symbol="$func" --stdio > "$OUTPUT_DIR/annotate_${func}.txt" 2>/dev/null || true
    done
fi

# Flamegraph generation
if [ -z "$SKIP_FLAMEGRAPH" ]; then
    echo ""
    echo "==> Generating flamegraph..."

    taskset -c $CPU_CORES cargo flamegraph \
        --bench $BENCH_NAME \
        --profile=profiling \
        --output="$OUTPUT_DIR/flamegraph.svg" \
        -- --bench 2>&1 | tail -10

    echo "  -> Flamegraph: $OUTPUT_DIR/flamegraph.svg"
fi

# Restore CPU frequency
if [ -z "$SKIP_CPUPOWER" ]; then
    echo ""
    echo "==> Restoring CPU frequency settings..."
    sudo cpupower frequency-set -d $BASE_FREQ -u $MAX_FREQ > /dev/null 2>&1
fi

# Generate comprehensive summary
echo ""
echo "=== Profiling Complete! ==="
echo ""
echo "Results directory: $OUTPUT_DIR/"
echo ""
echo "Files generated:"
[ -f "$OUTPUT_DIR/criterion_baseline.txt" ] && echo "  - criterion_baseline.txt (Criterion benchmark output)"
[ -f "$OUTPUT_DIR/baseline_summary.txt" ] && echo "  - baseline_summary.txt (baseline timing summary)"
[ -f "$OUTPUT_DIR/perf_report.txt" ] && echo "  - perf_report.txt (CPU cycle hotspots)"
[ -f "$OUTPUT_DIR/top_hotspots.txt" ] && echo "  - top_hotspots.txt (top CPU consumers)"
[ -f "$OUTPUT_DIR/perf_cycles.data" ] && echo "  - perf_cycles.data (raw perf data for analysis)"
[ -f "$OUTPUT_DIR/cache_stats.txt" ] && echo "  - cache_stats.txt (L1/LLC cache statistics)"
[ -f "$OUTPUT_DIR/branch_stats.txt" ] && echo "  - branch_stats.txt (branch prediction stats)"
[ -f "$OUTPUT_DIR/ipc_stats.txt" ] && echo "  - ipc_stats.txt (instructions per cycle)"
[ -f "$OUTPUT_DIR/memory_stats.txt" ] && echo "  - memory_stats.txt (page faults and context switches)"
[ -f "$OUTPUT_DIR/flamegraph.svg" ] && echo "  - flamegraph.svg (visual call graph)"
echo ""
echo "Next steps:"
echo "1. Open flamegraph: firefox $OUTPUT_DIR/flamegraph.svg"
echo "2. Review hotspots: less $OUTPUT_DIR/top_hotspots.txt"
echo "3. Check cache efficiency: cat $OUTPUT_DIR/cache_stats.txt"
echo "4. Identify optimization targets (functions >5% CPU)"
echo "5. Document findings in mmverify_optimization_journal.md"
echo ""
echo "For interactive perf analysis:"
echo "  perf report -i $OUTPUT_DIR/perf_cycles.data"
echo ""
echo "To generate optimization journal entry:"
echo "  cat $OUTPUT_DIR/top_hotspots.txt >> docs/optimization/mmverify_optimization_journal.md"
