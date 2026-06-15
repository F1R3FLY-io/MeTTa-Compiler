#!/usr/bin/env bash
# E5 — ThreadSanitizer gate for the concurrent SATB collector (FANOUT>0).
#
# The loom models (loom_rendezvous / loom_straddle) cover the rendezvous +
# straddle handshake under a sequentially-consistent operational model, and the
# TLA+ / Rocq suite covers the temporal + deductive obligations. TSan is the
# remaining E5 rung: a RUNTIME data-race detector over the real relaxed-memory
# concurrent collector. The specific target (phase-de-concurrent-collector-
# design.md, E5 section) is the read-locked concurrent MARK racing the
# concurrent allocator `alloc_*_concurrent(&self)` — workers bump-allocate under
# `.read()` while the dedicated GC thread marks under `.read()` and sweeps under
# `.write()`. A missing Acquire/Release or an unsynchronised shared write shows
# up here as `ThreadSanitizer: data race`.
#
#   Usage: scripts/e5_satb_tsan.sh <label>
#
# Build: -Zsanitizer=thread -Zbuild-std nightly, release,.
# Runs : FANOUT=8 PLN workloads (Robot, FlyingRaven) with MIN_BYTES low so the
#        rendezvous collector actually fires WHILE workers are live (non-vacuous:
#        '[index_gc] rendezvous ... cycle' > 0). TSan is ~5-15x slower than
#        native, so each arm has a generous timeout; the workloads are the
#        moderate PLN fixtures (NOT the 96 KB stress_multidir, which would run
#        for many minutes under TSan).
#
# Resource: build capped 32G, runs capped 24G, MemorySwapMax=0, FOREGROUND,
#           serial. NEVER background an uncapped sanitizer -Zbuild-std build
#           (a prior uncapped ASAN+stress run OOM-crashed the box).
set -uo pipefail
LABEL="${1:?usage: e5_satb_tsan.sh <label>}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/release/mettatron"
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "e5_${SAFE_LABEL}_tsan.XXXXXXXX")}"
P="$LOG_DIR/e5_${SAFE_LABEL}_tsan"
MIN_BYTES="${MIN_BYTES:-131072}"   # 128 KiB floor — forces the rendezvous collector to fire
FANOUT="${FANOUT:-8}"
# halt_on_error=1: stop on the FIRST race (a real race is a hard fail). The two
# settings below keep the report readable + bounded.
TSAN_OPTS="halt_on_error=1:second_deadlock_stack=1:history_size=4"

echo "===== E5 SATB TSAN [$LABEL] (FANOUT=$FANOUT, default dedicated index GC) ====="; date; free -h | head -2
echo "repo=$REPO  pln=$PLN  logs=$LOG_DIR  MIN_BYTES=$MIN_BYTES"

if [ "${SKIP_BUILD:-0}" = "1" ] && [ -x "$BIN" ]; then
  echo "### SKIP_BUILD=1 — reusing existing TSAN binary $BIN"
else
  echo "### build mettatron index-gc TSAN (release, -Zbuild-std, -j4, capped 32G)"
  systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 --quiet \
    env RUSTFLAGS="-Zsanitizer=thread -Cdebuginfo=2 -Ctarget-cpu=native" \
    cargo +nightly build --release -Zbuild-std --target x86_64-unknown-linux-gnu \
      --bin mettatron -j4 > "${P}_build.log" 2>&1
  BUILD_RC=$?
  echo "build_rc=$BUILD_RC"; tail -4 "${P}_build.log"
  if [ "$BUILD_RC" -ne 0 ]; then echo "BUILD FAILED — aborting"; exit 1; fi
fi

failures=0
run_arm() {  # $1=label $2=fixture $3=timeout
  local label="$1" fixture="$2" timeout_budget="$3"
  local rc race_count warn_count rendezvous_count
  echo "### TSAN arm: $label (FANOUT=$FANOUT, MIN_BYTES=$MIN_BYTES, timeout=${timeout_budget}s)"
  # REPORT=2 prints the per-cycle '[index_gc] rendezvous|quiescence ... cycle'
  # lines (REPORT=1 prints only the INDEX_GC_CYCLES_RUN summary).
  env METTATRON_PARALLEL_FANOUT_DEPTH="$FANOUT" \
      METTATRON_INDEX_GC_MIN_BYTES="$MIN_BYTES" \
      METTATRON_INDEX_GC_REPORT=2 \
      TSAN_OPTIONS="$TSAN_OPTS" \
    systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600% --quiet \
      timeout --signal=TERM --kill-after=10s "$timeout_budget" \
      "$BIN" "$fixture" > "${P}_${label}.log" 2>&1
  rc=$?
  race_count=$(grep -cE "ThreadSanitizer: data race" "${P}_${label}.log")
  warn_count=$(grep -cE "WARNING: ThreadSanitizer" "${P}_${label}.log")
  rendezvous_count=$(grep -cE "^\[index_gc\] rendezvous .* cycle" "${P}_${label}.log")
  # Robust non-vacuity fallback: the always-printed summary line.
  cycles_run=$(grep -oE "INDEX_GC_CYCLES_RUN=[0-9]+" "${P}_${label}.log" | tail -1 | cut -d= -f2)
  cycles_run="${cycles_run:-0}"
  echo "  rc=$rc  data_race=$race_count  tsan_warnings=$warn_count  rendezvous_cycles=$rendezvous_count  cycles_run=$cycles_run"
  # A real race/warning => fail. Non-vacuity: the concurrent collector must have
  # actually run at FANOUT>0 (a rendezvous cycle fired while workers were live;
  # if REPORT granularity hid the per-cycle label, fall back to cycles_run>0,
  # which at FANOUT>0 still means the GC thread ran concurrently with workers).
  if [ "$race_count" -ne 0 ] || [ "$warn_count" -ne 0 ]; then
    echo "  ARM FAIL: ThreadSanitizer reported a race/warning"; failures=$((failures + 1))
  elif [ "$rendezvous_count" -eq 0 ] && [ "$cycles_run" -eq 0 ]; then
    echo "  ARM FAIL (VACUOUS): no GC cycle fired — concurrent path not exercised"; failures=$((failures + 1))
  else
    echo "  ARM PASS: 0 races, rendezvous=$rendezvous_count cycles_run=$cycles_run (non-vacuous)"
  fi
}

# Robot (canonical PLN forward-chaining) fires ~90 rendezvous cycles at FANOUT=8.
# stress_alloc (small, alloc-heavy) is a fast second arm under TSan's ~5-15x slow-
# down. FlyingRaven is intentionally NOT used: it exceeds a practical TSan timeout.
run_arm robot_f8       "$PLN/examples/Robot.metta"            1200 || true
run_arm stress_alloc_f8 "$REPO/examples/cesk-gc/stress_alloc.metta" 600 || true

echo "===== E5 SATB TSAN VERDICT [$LABEL] ====="
echo "total arm failures: $failures"
echo "===== E5 SATB TSAN [$LABEL] DONE ====="; date
exit "$failures"
