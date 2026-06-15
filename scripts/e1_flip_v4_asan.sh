#!/usr/bin/env bash
# E1-FLIP V4 — the load-bearing UAF gate for activating concurrent collection.
#
# After E1-FLIP Commit A, the dedicated-GC-thread rendezvous SWEEPS under FANOUT>0
# (gate_open_rendezvous + the "rendezvous" phase + the Part-3 panic finish-bump). This
# is the FIRST time the sweep runs while eval workers are live, so it is where the whole
# E1-c root-completeness machinery is finally exercised end-to-end.
#
# This gate validates the default dedicated index-GC activation directly. The
# old override split is retired.
#
# Per arm asserts:
#   (a) 0 ASAN UAF/poison (root-set completeness under live parallelism);
#   (b) NON-VACUOUS: robot/raven require '[index_gc] rendezvous ... cycle' > 0
#       (the rendezvous collector actually fired + swept while workers were live);
#       stress_multidir requires '[index_gc] quiescence ... cycle' > 0 (the
#       fanout-configured true-quiescence path actually fired). In all arms,
#       FANOUT midloop non-rendezvous cycles must be 0.
#   (c) correct result (no Error / StackOverflow).
#
# CAPPED build 24G / run 20G, MemorySwapMax=0, -j4. Check `free -h` first (a
# sibling build may be live in another worktree → drop to -j3). NEVER background an
# UNCAPPED ASAN build (a prior uncapped backgrounded ASAN+stress run OOM-crashed the box).
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/release/mettatron"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "e1_flip_v4.XXXXXXXX")}"
P="$LOG_DIR/e1_flip_v4"
MIN_BYTES=131072   # 128 KiB major floor — forces the rendezvous collector to fire
ARM_TIMEOUT="${ARM_TIMEOUT:-240s}"
STRESS_TIMEOUT="${STRESS_TIMEOUT:-1200s}"
ARM_KILL_AFTER="${ARM_KILL_AFTER:-20s}"

echo "===== E1-FLIP V4 ASAN (FANOUT>0 + default dedicated index GC) ====="; date; free -h | head -2
echo "repo=$REPO"
echo "pln=$PLN"
echo "logs=$LOG_DIR"
echo "arm_timeout=$ARM_TIMEOUT stress_timeout=$STRESS_TIMEOUT kill_after=$ARM_KILL_AFTER"

echo "### build mettatron index-gc ASAN (release, -Zbuild-std, -j4, capped 24G)"
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1000% -p TasksMax=512 --quiet \
  env RUSTFLAGS="-Zsanitizer=address -Cdebuginfo=2 -Ctarget-cpu=native" \
  cargo +nightly build --release -Zbuild-std --target x86_64-unknown-linux-gnu \
    --bin mettatron -j4 > "${P}_build.log" 2>&1
BUILD_RC=$?
echo "build_rc=$BUILD_RC"
tail -5 "${P}_build.log"
if [ "$BUILD_RC" -ne 0 ] || [ ! -x "$BIN" ]; then
  echo "BUILD FAILED — aborting V4 (binary not produced)"; exit 1
fi

run_arm() {  # $1=label $2=fixture $3=fanout $4=required_cycle_kind [$5=timeout]
  local label="$1" fixture="$2" fanout="$3" required_cycle_kind="$4" timeout_budget="${5:-$ARM_TIMEOUT}"
  local rc asan_count all_cycle_count rendezvous_count quiescence_count midloop_count unexpected_non_rendezvous_count error_count
  echo "### ASAN arm: $label (FANOUT=$fanout, default dedicated index GC, MIN_BYTES=$MIN_BYTES, timeout=$timeout_budget)"
  env METTATRON_PARALLEL_FANOUT_DEPTH="$fanout" \
      METTATRON_INDEX_GC_MIN_BYTES="$MIN_BYTES" METTATRON_INDEX_GC_REPORT=2 \
      ASAN_OPTIONS=detect_leaks=0:abort_on_error=1:halt_on_error=1 \
    systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000% --quiet \
    timeout --signal=USR1 --kill-after="$ARM_KILL_AFTER" "$timeout_budget" \
    "$BIN" --gc index "$fixture" > "${P}_${label}.log" 2>&1
  rc=$?
  echo "  ${label}_rc=$rc"
  asan_count=$(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|heap-buffer-overflow' "${P}_${label}.log" || true)
  all_cycle_count=$(grep -cE '^\[index_gc\] .* cycle' "${P}_${label}.log" || true)
  rendezvous_count=$(grep -cE '^\[index_gc\] rendezvous .* cycle' "${P}_${label}.log" || true)
  quiescence_count=$(grep -cE '^\[index_gc\] quiescence .* cycle' "${P}_${label}.log" || true)
  midloop_count=$(grep -cE '^\[index_gc\] midloop .* cycle' "${P}_${label}.log" || true)
  unexpected_non_rendezvous_count=$(grep -E '^\[index_gc\] .* cycle' "${P}_${label}.log" | grep -cvE '^\[index_gc\] (rendezvous|quiescence) ' || true)
  error_count=$(grep -cE 'Error|StackOverflow' "${P}_${label}.log" || true)
  echo "  (a) UAF/ASAN:            $asan_count  (expect 0)"
  echo "  (b) all index cycles:    $all_cycle_count  (expect >0)"
  echo "      rendezvous cycles:   $rendezvous_count"
  echo "      quiescence cycles:   $quiescence_count"
  echo "      midloop cycles:      $midloop_count  (expect 0 under FANOUT)"
  echo "      unexpected non-rdv:  $unexpected_non_rendezvous_count  (expect 0)"
  echo "  (c) Error/StackOverflow: $error_count  (expect 0)"
  if [ "$required_cycle_kind" = "rendezvous" ] && [ "$rendezvous_count" -eq 0 ]; then
    echo "  ${label}: FAIL (required rendezvous witness missing; log=${P}_${label}.log)"
    tail -80 "${P}_${label}.log"
    return 1
  fi
  if [ "$required_cycle_kind" = "quiescence" ] && [ "$quiescence_count" -eq 0 ]; then
    echo "  ${label}: FAIL (required quiescence witness missing; log=${P}_${label}.log)"
    tail -80 "${P}_${label}.log"
    return 1
  fi
  if [ "$rc" -ne 0 ] || [ "$asan_count" -ne 0 ] || [ "$all_cycle_count" -eq 0 ] || \
     [ "$midloop_count" -ne 0 ] || [ "$unexpected_non_rendezvous_count" -ne 0 ] || \
     [ "$error_count" -ne 0 ]; then
    echo "  ${label}: FAIL (log=${P}_${label}.log)"
    tail -80 "${P}_${label}.log"
    return 1
  fi
  echo "  ${label}: PASS"
}

failures=0
run_arm robot_f8  "$PLN/examples/Robot.metta"                    8 rendezvous || failures=$((failures + 1))
run_arm raven_f8  "$PLN/examples/FlyingRaven.metta"              8 rendezvous || failures=$((failures + 1))
run_arm stress_f8 "$REPO/examples/cesk-gc/stress_multidir.metta" 8 quiescence "$STRESS_TIMEOUT" || failures=$((failures + 1))

echo "===== E1-FLIP V4 ASAN DONE ====="; date
if [ "$failures" -ne 0 ]; then
  echo "V4 ASAN failures=$failures"
  exit 1
fi
