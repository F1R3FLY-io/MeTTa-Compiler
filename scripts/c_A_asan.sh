#!/usr/bin/env bash
# Phase C Increment A — side-free ASAN (the load-bearing safety discharge).
#
#   Usage: scripts/c_A_asan.sh
#
# Builds the `mettatron` CLI under index-gc ASAN (-Zbuild-std nightly, capped 32G
# build / 24G run, FOREGROUND) and exercises the re-enabled (quiescence-only,
# no-recycle) side-free on THREE arms, each asserting 0 ASAN UAF:
#
#   1. quiescence-MAJOR : change_state_young.metta @ MIN_BYTES=131072 (committed trips
#                         a MAJOR at the directive boundary) → side-free on `sweep`.
#   2. quiescence-MINOR : side_free_minor.metta @ MIN_BYTES=1 GiB (no major) + heavy
#                         young churn ⇒ a MINOR fires (young_alloc > 2 MiB) → side-free
#                         on `sweep_young` (the young-only path).
#   3. midloop-GATE     : side_free_minor.metta ⇒ default-on mid-loop minors fire
#                         while the State value is live, but the side-free is GATED
#                         OFF (deferred) → 0 UAF, proving the gate prevents the
#                         launder UAF.
#
# All FANOUT_DEPTH=0 (single-threaded quiescence collector). REPORT=2 prints the
# minor/major split so we can confirm the intended collection type fired (non-vacuous).
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/debug/mettatron"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "c_A_asan.XXXXXXXX")}"
P="$LOG_DIR/c_A_asan"
GIB=$((1024*1024*1024))
echo "===== C #A SIDE-FREE ASAN ====="; date
echo "repo=$REPO"
echo "logs=$LOG_DIR"

echo "### build mettatron index-gc ASAN bin"
systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 \
  env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
  cargo +nightly build -Zbuild-std --target x86_64-unknown-linux-gnu --bin mettatron -j4 --features index-gc \
  > "${P}_build.log" 2>&1
echo "build_rc=$?"; tail -3 "${P}_build.log"

run_arm() {  # $1=label  $2=fixture  $3...=env assignments
  local label="$1"; local fixture="$2"; shift 2
  echo "### arm: $label  ($fixture)  env: $*"
  env "$@" ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=800% \
    "$BIN" "examples/cesk-gc/$fixture" > "${P}_${label}.log" 2>&1
  echo "${label}_rc=$?"
  echo "  UAF lines:       $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free' "${P}_${label}.log")"
  echo "  GC cycles (minor/major): $(grep -cE 'minor cycle' "${P}_${label}.log") / $(grep -cE 'major cycle' "${P}_${label}.log")"
  echo "  result tail: $(grep -vE '^\[index_gc\]' "${P}_${label}.log" | tail -2 | tr '\n' ' ')"
  echo "  Error/lost?:     $(grep -cE 'Error|state-value-lost|churn-mismatch' "${P}_${label}.log")"
}

run_arm quiescence_major change_state_young.metta \
  METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=2
run_arm quiescence_minor side_free_minor.metta \
  METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=$GIB METTATRON_INDEX_GC_REPORT=2
run_arm midloop_gate side_free_minor.metta \
  METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=$GIB METTATRON_INDEX_GC_REPORT=2

echo "===== C #A SIDE-FREE ASAN VERDICT ====="
TOTAL_UAF=$(grep -lcE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free' "${P}_quiescence_major.log" "${P}_quiescence_minor.log" "${P}_midloop_gate.log" 2>/dev/null | grep -v ':0' | wc -l)
echo "arms with UAF: $TOTAL_UAF (expect 0)"
echo "===== C #A SIDE-FREE ASAN DONE ====="; date
