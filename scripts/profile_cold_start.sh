#!/bin/bash
# Automated Cold Start Profiling for MeTTaTron
#
# Runs four profiling tools and collates results into a single timestamped
# report directory under docs/optimization/.
#
# Tools used:
#   1. hyperfine  — baseline warm/cold cache latency
#   2. perf stat  — hardware counters (cycles, cache misses, page faults, ...)
#   3. strace -c  — syscall summary (clone, mmap, futex, ...)
#   4. --startup-timing — per-phase wall-clock breakdown (built into binary)
#
# Usage: ./scripts/profile_cold_start.sh [input_file] [runs]
#   input_file : MeTTa file to profile  (default: examples/simple.metta)
#   runs       : number of repetitions  (default: 20)
#
# Examples:
#   ./scripts/profile_cold_start.sh
#   ./scripts/profile_cold_start.sh examples/mmverify/demo0/verify_demo0.metta 10
#   ./scripts/profile_cold_start.sh examples/simple.metta 50

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────
INPUT="${1:-examples/simple.metta}"
RUNS="${2:-20}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTPUT_DIR="docs/optimization/cold_start_${TIMESTAMP}"
BINARY="./target/release/mettatron"

# CPU configuration for Intel Xeon E5-2699 v3
CPU_CORES="0-17"

echo "=== MeTTaTron Cold Start Profiler ==="
echo "Input:   $INPUT"
echo "Runs:    $RUNS"
echo "Binary:  $BINARY"
echo "Output:  $OUTPUT_DIR"
echo ""

# ── Preflight checks ─────────────────────────────────────────────────────────
if [ ! -f "$INPUT" ]; then
    echo "Error: Input file not found: $INPUT"
    exit 1
fi

if [ ! -x "$BINARY" ]; then
    echo "Binary not found or not executable. Building release..."
    cargo build --release
fi

mkdir -p "$OUTPUT_DIR"

# Detect available tools
HAVE_HYPERFINE=true
HAVE_PERF=true
HAVE_STRACE=true

command -v hyperfine &>/dev/null || { echo "Warning: hyperfine not found. Skipping baseline."; HAVE_HYPERFINE=false; }
command -v perf      &>/dev/null || { echo "Warning: perf not found. Skipping HW counters.";    HAVE_PERF=false; }
command -v strace    &>/dev/null || { echo "Warning: strace not found. Skipping syscall summary."; HAVE_STRACE=false; }
echo ""

# Record binary size
BINARY_SIZE=$(stat --printf="%s" "$BINARY" 2>/dev/null || stat -f "%z" "$BINARY" 2>/dev/null || echo "unknown")
INPUT_SIZE=$(stat --printf="%s" "$INPUT" 2>/dev/null || stat -f "%z" "$INPUT" 2>/dev/null || echo "unknown")

# ── 1. Baseline with hyperfine ───────────────────────────────────────────────
if $HAVE_HYPERFINE; then
    echo "==> [1/4] Baseline latency (hyperfine, $RUNS runs, warm cache)..."
    taskset -c $CPU_CORES hyperfine \
        --warmup 5 \
        --runs "$RUNS" \
        --export-json "$OUTPUT_DIR/hyperfine.json" \
        "$BINARY $INPUT" \
        "$BINARY --help" \
        2>&1 | tee "$OUTPUT_DIR/hyperfine.txt"
    echo ""
fi

# ── 2. Hardware counters with perf stat ──────────────────────────────────────
if $HAVE_PERF; then
    echo "==> [2/4] Hardware counters (perf stat, $RUNS runs)..."
    taskset -c $CPU_CORES perf stat \
        -r "$RUNS" \
        -e task-clock,cycles,instructions,cache-references,cache-misses,page-faults,minor-faults,major-faults,context-switches,cpu-migrations,branch-misses,L1-dcache-load-misses,LLC-load-misses,dTLB-load-misses \
        -- "$BINARY" "$INPUT" \
        2>&1 | tee "$OUTPUT_DIR/perf_stat.txt"
    echo ""
fi

# ── 3. Syscall summary with strace ──────────────────────────────────────────
if $HAVE_STRACE; then
    echo "==> [3/4] Syscall summary (strace -c)..."
    strace -c -f -- "$BINARY" "$INPUT" \
        2>&1 | tee "$OUTPUT_DIR/strace_summary.txt"
    echo ""
fi

# ── 4. Per-phase breakdown with --startup-timing ─────────────────────────────
echo "==> [4/4] Per-phase startup timing..."
"$BINARY" --startup-timing "$INPUT" 2>"$OUTPUT_DIR/startup_timing.txt" >/dev/null
cat "$OUTPUT_DIR/startup_timing.txt"
echo ""

# ── 5. Collated summary report ──────────────────────────────────────────────
REPORT="$OUTPUT_DIR/cold_start_report.txt"
{
    echo "=== MeTTaTron Cold Start Profile ==="
    echo "Date:   $(date '+%Y-%m-%d %H:%M:%S')"
    echo "Binary: $BINARY ($((BINARY_SIZE / 1024)) KB)"
    echo "Input:  $INPUT ($((INPUT_SIZE / 1024)) KB)"
    echo ""

    if $HAVE_HYPERFINE && [ -f "$OUTPUT_DIR/hyperfine.json" ]; then
        echo "--- Baseline (hyperfine, $RUNS runs) ---"
        # Extract mean and stddev from JSON using grep/sed (no jq dependency)
        if command -v jq &>/dev/null; then
            WARM_MEAN=$(jq -r '.results[0].mean * 1000 | . * 100 | round / 100' "$OUTPUT_DIR/hyperfine.json" 2>/dev/null || echo "?")
            WARM_STDDEV=$(jq -r '.results[0].stddev * 1000 | . * 100 | round / 100' "$OUTPUT_DIR/hyperfine.json" 2>/dev/null || echo "?")
            HELP_MEAN=$(jq -r '.results[1].mean * 1000 | . * 100 | round / 100' "$OUTPUT_DIR/hyperfine.json" 2>/dev/null || echo "?")
            HELP_STDDEV=$(jq -r '.results[1].stddev * 1000 | . * 100 | round / 100' "$OUTPUT_DIR/hyperfine.json" 2>/dev/null || echo "?")
            echo "  Warm cache:     ${WARM_MEAN} +/- ${WARM_STDDEV} ms"
            echo "  --help only:    ${HELP_MEAN} +/- ${HELP_STDDEV} ms"
        else
            echo "  (install jq for parsed summary; raw data in hyperfine.json)"
        fi
        # Always include the raw text output
        echo ""
        cat "$OUTPUT_DIR/hyperfine.txt"
        echo ""
    fi

    if $HAVE_PERF && [ -f "$OUTPUT_DIR/perf_stat.txt" ]; then
        echo "--- Hardware Counters (perf stat, $RUNS runs) ---"
        cat "$OUTPUT_DIR/perf_stat.txt"
        echo ""
    fi

    if $HAVE_STRACE && [ -f "$OUTPUT_DIR/strace_summary.txt" ]; then
        echo "--- Syscall Summary (strace -c) ---"
        cat "$OUTPUT_DIR/strace_summary.txt"
        echo ""
    fi

    if [ -f "$OUTPUT_DIR/startup_timing.txt" ]; then
        echo "--- Phase Breakdown (--startup-timing) ---"
        cat "$OUTPUT_DIR/startup_timing.txt"
        echo ""
    fi
} > "$REPORT"

# ── Final summary ────────────────────────────────────────────────────────────
echo "=== Profiling Complete ==="
echo ""
echo "Results directory: $OUTPUT_DIR/"
echo ""
echo "Files generated:"
[ -f "$OUTPUT_DIR/hyperfine.json" ]     && echo "  - hyperfine.json       (raw latency data)"
[ -f "$OUTPUT_DIR/hyperfine.txt" ]      && echo "  - hyperfine.txt        (hyperfine console output)"
[ -f "$OUTPUT_DIR/perf_stat.txt" ]      && echo "  - perf_stat.txt        (hardware counters)"
[ -f "$OUTPUT_DIR/strace_summary.txt" ] && echo "  - strace_summary.txt   (syscall summary)"
[ -f "$OUTPUT_DIR/startup_timing.txt" ] && echo "  - startup_timing.txt   (per-phase timing)"
[ -f "$REPORT" ]                        && echo "  - cold_start_report.txt (collated report)"
echo ""
echo "Quick review:"
echo "  cat $REPORT"
