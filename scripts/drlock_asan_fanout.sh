#!/usr/bin/env bash
# D-RLOCK.2 gate #6: ASAN FANOUT>0. Builds the mettatron CLI under index-gc ASAN
# (-Zsanitizer=address -Zbuild-std nightly) and runs a PARALLEL workload at
# FANOUT=8 with --gc index → asserts 0 ASAN UAF. Validates that the concurrent
# (`.read()`+`&self`) bump path's side-arena APPENDS never move/free a published
# Box (only the collector frees, and it is `.write()`/gate-excluded), so a
# concurrent reader of a just-published node + side datum is sound.
#
# CAPPED build 24G / run 20G, MemorySwapMax=0, FOREGROUND, -j4 (RAM-mindful; the
# prior OOM was an UNCAPPED BACKGROUNDED ASAN build).
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/release/mettatron"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "drlock_asan.XXXXXXXX")}"
P="$LOG_DIR/drlock_asan"
echo "===== D-RLOCK.2 ASAN FANOUT>0 ====="; date
echo "repo=$REPO"
echo "pln=$PLN"
echo "logs=$LOG_DIR"

echo "### build mettatron index-gc ASAN (release, -Zbuild-std, -j4, capped 24G)"
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1000% -p TasksMax=256 --quiet \
  env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
  cargo +nightly build --release -Zbuild-std --target x86_64-unknown-linux-gnu \
    --bin mettatron -j4 --features index-gc > "${P}_build.log" 2>&1
echo "build_rc=$?"; tail -3 "${P}_build.log"

run_arm() {  # $1=label  $2=fixture-abs  $3=fanout
  local label="$1" fixture="$2" fanout="$3"
  echo "### ASAN arm: $label  (FANOUT=$fanout)  $fixture"
  env METTATRON_PARALLEL_FANOUT_DEPTH="$fanout" ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000% --quiet \
    "$BIN" --gc index "$fixture" > "${P}_${label}.log" 2>&1
  echo "  ${label}_rc=$?"
  echo "  UAF/ASAN lines: $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free|heap-buffer-overflow' "${P}_${label}.log")"
  echo "  result tail: $(grep -vE '^\[index_gc\]' "${P}_${label}.log" | tail -1)"
}

# Two parallel PLN workloads at FANOUT=8 (workers spawn ⇒ collector backs off ⇒
# the concurrent `.read()`+`&self` allocation path is the active path).
run_arm robot_f8 "$PLN/examples/Robot.metta" 8
run_arm raven_f8 "$PLN/examples/FlyingRaven.metta" 8
# Also a heavy alloc workload at FANOUT=8 to maximize concurrent-append pressure.
run_arm stress_f8 "$REPO/examples/cesk-gc/stress_multidir.metta" 8

echo "===== D-RLOCK.2 ASAN VERDICT ====="
TOTAL=$(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free|heap-buffer-overflow' "${P}_robot_f8.log" "${P}_raven_f8.log" "${P}_stress_f8.log" 2>/dev/null | awk -F: '{s+=$2} END{print s}')
echo "total UAF/ASAN lines across arms: $TOTAL (expect 0)"
echo "===== D-RLOCK.2 ASAN DONE ====="; date
