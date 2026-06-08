#!/usr/bin/env bash
# Phase D — D2.3 gate #5: the LOAD-BEARING UAF check for the parallel-collector
# RENDEZVOUS. Builds the `mettatron` CLI under index-gc ASAN (-Zsanitizer=address
# -Zbuild-std nightly) and runs PARALLEL PLN workloads at FANOUT=8 with the
# default dedicated rendezvous collector engaged and `MIN_BYTES` lowered so
# the RENDEZVOUS collector FIRES WHILE WORKERS ARE ALIVE. Asserts, per arm:
#
#   (a) 0 ASAN UAF — validates Risk R3 (deferred side-`Box` free: a parked worker
#       holds laundered `&'static` INNER_SHADOW refs into side `Box`es; the
#       rendezvous mark_sweep passes phase="rendezvous" != "quiescence" so it
#       reclaims NODE slots only and DEFERS the side `Box`es — index_heap.rs:1985)
#       AND union completeness at runtime (every parked worker self-rooted; the
#       requestor unions ⋃-workers ∪ requestor-machine ∪ E₀ ∪ driver-C, so no live
#       parked machine is unrooted ⇒ a fired mark frees nothing live).
#   (b) NON-VACUOUS: the RENDEZVOUS collector actually fired — `[index_gc]
#       rendezvous ... cycle` lines > 0 (REPORT=2) on the parallel PLN arms. The
#       shallow multi-directive stress arm is a true-quiescence witness instead:
#       fanout configuration no longer blocks `active==0 && n_threads()==0`
#       collection. Across all FANOUT arms, midloop non-rendezvous cycles must be 0.
#   (c) correct result (no Error/StackOverflow; PLN produces its answer).
#
# CAPPED build 24G / run 20G, MemorySwapMax=0, FOREGROUND, -j4 (RAM-mindful; the
# prior OOM was an UNCAPPED BACKGROUNDED ASAN build — see MEMORY.md).
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/release/mettatron"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "d2_3_rendezvous_asan.XXXXXXXX")}"
P="$LOG_DIR/d2_3_rendezvous_asan"
# Lower the MAJOR floor so the rendezvous collector fires early & often while
# workers are alive (the whole point — exercise the UAF window). 131072 = 128 KiB,
# the same low MIN_BYTES the C/D green-walls use to force frequent collection.
MIN_BYTES=131072
ARM_TIMEOUT="${ARM_TIMEOUT:-240s}"
STRESS_TIMEOUT="${STRESS_TIMEOUT:-1200s}"
ARM_KILL_AFTER="${ARM_KILL_AFTER:-20s}"
echo "===== D2.3 RENDEZVOUS ASAN (FANOUT>0 + default dedicated index GC) ====="; date
echo "repo=$REPO"
echo "pln=$PLN"
echo "logs=$LOG_DIR"
echo "arm_timeout=$ARM_TIMEOUT stress_timeout=$STRESS_TIMEOUT kill_after=$ARM_KILL_AFTER"

echo "### build mettatron index-gc ASAN (release, -Zbuild-std, -j4, capped 24G)"
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1000% -p TasksMax=256 --quiet \
  env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
  cargo +nightly build --release -Zbuild-std --target x86_64-unknown-linux-gnu \
    --bin mettatron -j4 --features index-gc > "${P}_build.log" 2>&1
BUILD_RC=$?
echo "build_rc=$BUILD_RC"; tail -3 "${P}_build.log"
if [ "$BUILD_RC" -ne 0 ] || [ ! -x "$BIN" ]; then
  echo "BUILD FAILED — aborting D2.3 rendezvous ASAN (binary not produced)"
  exit 1
fi

run_arm() {  # $1=label  $2=fixture-abs  $3=fanout  $4=required-cycle-kind [$5=timeout]
  local label="$1" fixture="$2" fanout="$3" required_cycle_kind="$4" timeout_budget="${5:-$ARM_TIMEOUT}"
  local rc asan_count rendezvous_count quiescence_count midloop_count total_cycles unexpected_non_rendezvous_count error_count result_tail
  echo "### ASAN arm: $label  (FANOUT=$fanout, default dedicated index GC, MIN_BYTES=$MIN_BYTES, timeout=$timeout_budget)  $fixture"
  env METTATRON_PARALLEL_FANOUT_DEPTH="$fanout" \
      METTATRON_INDEX_GC_MIN_BYTES="$MIN_BYTES" \
      METTATRON_INDEX_GC_REPORT=2 \
      ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000% --quiet \
    timeout --signal=USR1 --kill-after="$ARM_KILL_AFTER" "$timeout_budget" \
    "$BIN" --gc index "$fixture" > "${P}_${label}.log" 2>&1
  rc=$?
  asan_count=$(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free|heap-buffer-overflow' "${P}_${label}.log" || true)
  rendezvous_count=$(grep -cE '^\[index_gc\] rendezvous .* cycle' "${P}_${label}.log" || true)
  quiescence_count=$(grep -cE '^\[index_gc\] quiescence .* cycle' "${P}_${label}.log" || true)
  midloop_count=$(grep -cE '^\[index_gc\] midloop .* cycle' "${P}_${label}.log" || true)
  total_cycles=$(grep -cE '^\[index_gc\] .* cycle' "${P}_${label}.log" || true)
  unexpected_non_rendezvous_count=$(grep -E '^\[index_gc\] .* cycle' "${P}_${label}.log" | grep -cvE '^\[index_gc\] (rendezvous|quiescence) ' || true)
  error_count=$(grep -cE 'Error|StackOverflow' "${P}_${label}.log" || true)
  result_tail=$(grep -vE '^\[index_gc\]|^Running as|^\s*$' "${P}_${label}.log" | tail -1 || true)
  echo "  ${label}_rc=$rc"
  echo "  (a) UAF/ASAN lines:        $asan_count  (expect 0)"
  echo "  (b) rendezvous cycles:     $rendezvous_count"
  echo "      quiescence cycles:     $quiescence_count"
  echo "      midloop cycles:        $midloop_count  (expect 0 under FANOUT)"
  echo "      total index_gc cycles: $total_cycles"
  echo "      unexpected non-rdv:    $unexpected_non_rendezvous_count  (expect 0)"
  echo "  (c) Error/StackOverflow?:  $error_count"
  echo "      result tail:           $result_tail"
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
  if [ "$rc" -ne 0 ] || [ "$asan_count" -ne 0 ] || [ "$total_cycles" -eq 0 ] || \
     [ "$midloop_count" -ne 0 ] || [ "$unexpected_non_rendezvous_count" -ne 0 ] || \
     [ "$error_count" -ne 0 ]; then
    echo "  ${label}: FAIL (log=${P}_${label}.log)"
    tail -80 "${P}_${label}.log"
    return 1
  fi
  echo "  ${label}: PASS"
}

# Parallel PLN workloads at FANOUT=8 witness rendezvous collection. The shallow
# stress workload witnesses fanout-configured true-quiescence collection.
failures=0
run_arm robot_f8 "$PLN/examples/Robot.metta" 8 rendezvous || failures=$((failures + 1))
run_arm raven_f8 "$PLN/examples/FlyingRaven.metta" 8 rendezvous || failures=$((failures + 1))
# Heavy alloc workload at FANOUT=8 to maximize concurrent-append + collection pressure.
run_arm stress_f8 "$REPO/examples/cesk-gc/stress_multidir.metta" 8 quiescence "$STRESS_TIMEOUT" || failures=$((failures + 1))

echo "===== D2.3 RENDEZVOUS ASAN VERDICT ====="
logs=("${P}_robot_f8.log" "${P}_raven_f8.log" "${P}_stress_f8.log")
TOTAL_UAF=$(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free|heap-buffer-overflow' "${logs[@]}" 2>/dev/null | awk -F: '{s+=$2} END{print s}')
TOTAL_RDV=$(grep -cE '^\[index_gc\] rendezvous .* cycle' "${logs[@]}" 2>/dev/null | awk -F: '{s+=$2} END{print s}')
echo "total UAF/ASAN lines across arms:  ${TOTAL_UAF} (expect 0)"
echo "total rendezvous cycles across arms: ${TOTAL_RDV} (expect >0 — the collector FIRED while workers alive)"
echo "===== D2.3 RENDEZVOUS ASAN DONE ====="; date
if [ "$failures" -ne 0 ]; then
  echo "D2.3 ASAN failures=$failures"
  exit 1
fi
