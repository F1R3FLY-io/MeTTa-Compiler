#!/bin/bash
# Profile set_operations benchmarks with perf and flamegraph
#
# Usage: ./scripts/profile_set_operations.sh [bench_filter]
#   bench_filter: Optional, filters benchmark names (e.g., "intersection", "subtraction")
#
# Example:
#   ./scripts/profile_set_operations.sh                    # Run all benchmarks
#   ./scripts/profile_set_operations.sh intersection       # Run only intersection benchmarks
#   ./scripts/profile_set_operations.sh "scaling/hashmap"  # Filter to specific variant
#
# Prerequisites:
#   - perf: sudo apt-get install linux-tools-common linux-tools-generic
#   - flamegraph: cargo install flamegraph
#   - cpupower: sudo apt-get install linux-tools-common (optional, for CPU freq locking)

set -e

# Configuration
BENCH_NAME="set_operations"
FILTER="${1:-}"
PROFILE_TIME=30          # Seconds to profile
CPU_CORES="0-17"         # Socket 1 cores (NUMA node 0)
BASE_FREQ="2300000"      # 2.3 GHz base clock
MAX_FREQ="3570000"       # 3.57 GHz max turbo
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="docs/benchmarks/set_operations/results/${TIMESTAMP}"

echo "=== Set Operations Profiling Script ==="
echo "Benchmark: $BENCH_NAME"
echo "Filter: ${FILTER:-all}"
echo "Profile time: ${PROFILE_TIME}s"
echo "CPU Cores: $CPU_CORES"
echo "Results: $RESULTS_DIR"
echo ""

# Create results directory
mkdir -p "$RESULTS_DIR"

# Save configuration
cat > "$RESULTS_DIR/config.txt" << EOF
Timestamp: $(date)
Commit: $(git rev-parse HEAD)
Branch: $(git branch --show-current)
Filter: ${FILTER:-all}
Profile Time: ${PROFILE_TIME}s
CPU Cores: $CPU_CORES
EOF

# Try to lock CPU frequency (if cpupower available)
RESTORE_FREQ=0
if command -v cpupower &> /dev/null; then
    echo "==> Setting CPU governor to performance..."
    if sudo cpupower frequency-set -g performance > /dev/null 2>&1; then
        echo "==> Locking CPU frequency to base clock (2.3 GHz)..."
        if sudo cpupower frequency-set -d $BASE_FREQ -u $BASE_FREQ > /dev/null 2>&1; then
            echo "==> CPU frequency locked."
            RESTORE_FREQ=1
        else
            echo "Warning: Could not lock CPU frequency. Results may have higher variance."
        fi
    else
        echo "Warning: Could not set CPU governor. Results may have higher variance."
    fi
else
    echo "Warning: cpupower not found. CPU frequency not locked."
    echo "Install with: sudo apt-get install linux-tools-common"
fi

# Build with profiling profile
echo ""
echo "==> Building with profiling profile..."
cargo build --profile=profiling --benches 2>&1 | tail -5

# Prepare filter argument
FILTER_ARG=""
if [ -n "$FILTER" ]; then
    FILTER_ARG="$FILTER"
fi

# Step 1: Run Criterion benchmarks (baseline measurement)
echo ""
echo "==> Running Criterion benchmarks..."
if [ -n "$FILTER_ARG" ]; then
    taskset -c $CPU_CORES cargo bench --bench $BENCH_NAME -- "$FILTER_ARG" 2>&1 | tee "$RESULTS_DIR/criterion.txt"
else
    taskset -c $CPU_CORES cargo bench --bench $BENCH_NAME 2>&1 | tee "$RESULTS_DIR/criterion.txt"
fi

# Step 2: Perf stat for cache/branch metrics
echo ""
echo "==> Collecting perf stat metrics..."
if command -v perf &> /dev/null; then
    if [ -n "$FILTER_ARG" ]; then
        taskset -c $CPU_CORES perf stat \
            -e cycles,instructions,cache-references,cache-misses \
            -e branches,branch-misses \
            -e L1-dcache-loads,L1-dcache-load-misses \
            -e LLC-loads,LLC-load-misses \
            cargo bench --bench $BENCH_NAME -- "$FILTER_ARG" --profile-time $PROFILE_TIME \
            2>&1 | tee "$RESULTS_DIR/perf_stat.txt"
    else
        taskset -c $CPU_CORES perf stat \
            -e cycles,instructions,cache-references,cache-misses \
            -e branches,branch-misses \
            -e L1-dcache-loads,L1-dcache-load-misses \
            -e LLC-loads,LLC-load-misses \
            cargo bench --bench $BENCH_NAME -- --profile-time $PROFILE_TIME \
            2>&1 | tee "$RESULTS_DIR/perf_stat.txt"
    fi
else
    echo "Warning: perf not found. Skipping perf stat."
    echo "Install with: sudo apt-get install linux-tools-common linux-tools-generic"
fi

# Step 3: Perf record for flamegraph
echo ""
echo "==> Recording perf data for flamegraph..."
if command -v perf &> /dev/null; then
    if [ -n "$FILTER_ARG" ]; then
        taskset -c $CPU_CORES perf record \
            --freq=997 \
            --call-graph=dwarf,65528 \
            --output="$RESULTS_DIR/perf.data" \
            cargo bench --bench $BENCH_NAME -- "$FILTER_ARG" --profile-time $PROFILE_TIME \
            2>&1 | tee -a "$RESULTS_DIR/perf_record.log"
    else
        taskset -c $CPU_CORES perf record \
            --freq=997 \
            --call-graph=dwarf,65528 \
            --output="$RESULTS_DIR/perf.data" \
            cargo bench --bench $BENCH_NAME -- --profile-time $PROFILE_TIME \
            2>&1 | tee -a "$RESULTS_DIR/perf_record.log"
    fi

    # Generate perf report
    echo ""
    echo "==> Generating perf report..."
    perf report -i "$RESULTS_DIR/perf.data" --stdio > "$RESULTS_DIR/perf_report.txt" 2>&1

    # Generate flamegraph if available
    if command -v flamegraph &> /dev/null; then
        echo "==> Generating flamegraph..."
        perf script -i "$RESULTS_DIR/perf.data" 2>/dev/null | flamegraph --title "Set Operations Profile ($TIMESTAMP)" > "$RESULTS_DIR/flamegraph.svg" 2>&1 || echo "Warning: flamegraph generation failed"
    else
        echo "Warning: flamegraph not found. Install with: cargo install flamegraph"
    fi
else
    echo "Warning: perf not found. Skipping perf record."
fi

# Restore CPU frequency
if [ "$RESTORE_FREQ" = "1" ]; then
    echo ""
    echo "==> Restoring CPU frequency settings..."
    sudo cpupower frequency-set -d $BASE_FREQ -u $MAX_FREQ > /dev/null 2>&1 || true
fi

# Generate summary
echo ""
echo "==> Generating summary..."
cat > "$RESULTS_DIR/summary.md" << EOF
# Set Operations Benchmark Results

**Date**: $(date)
**Commit**: $(git rev-parse HEAD)
**Branch**: $(git branch --show-current)
**Filter**: ${FILTER:-all}

## System Configuration

- **CPU**: $(cat /proc/cpuinfo | grep "model name" | head -1 | cut -d: -f2 | xargs)
- **Cores Used**: $CPU_CORES
- **Frequency Locked**: $([ "$RESTORE_FREQ" = "1" ] && echo "Yes (${BASE_FREQ} Hz)" || echo "No")

## Files Generated

- \`criterion.txt\`: Raw Criterion benchmark results
- \`perf_stat.txt\`: CPU performance counters
- \`perf_report.txt\`: Detailed CPU profile
- \`flamegraph.svg\`: Visual CPU flamegraph (if generated)

## Quick Results

### Criterion Output (first 50 lines)
\`\`\`
$(head -50 "$RESULTS_DIR/criterion.txt" 2>/dev/null || echo "Not available")
\`\`\`

### Perf Stat Summary
\`\`\`
$(tail -30 "$RESULTS_DIR/perf_stat.txt" 2>/dev/null || echo "Not available")
\`\`\`

### Top Functions (from perf report)
\`\`\`
$(head -50 "$RESULTS_DIR/perf_report.txt" 2>/dev/null | grep -E "^\s+[0-9]" | head -20 || echo "Not available")
\`\`\`
EOF

echo ""
echo "=== Profiling Complete! ==="
echo ""
echo "Results saved to: $RESULTS_DIR/"
echo ""
echo "Files:"
ls -la "$RESULTS_DIR/"
echo ""
echo "View summary: cat $RESULTS_DIR/summary.md"
if [ -f "$RESULTS_DIR/flamegraph.svg" ]; then
    echo "View flamegraph: firefox $RESULTS_DIR/flamegraph.svg"
fi
