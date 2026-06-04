#!/usr/bin/env bash
# Phase C Increment D (C2) — MIDLOOP abstract-GC-narrowing ASAN (the dynamic UAF discharge).
#
#   Usage: scripts/d_midloop_asan.sh
#
# The soundness of `collect_live_values`'s post-cut narrowing (skip the dead `remaining_*`
# K-frame iterator once the cut fired) is PROVEN by the differential unit tests (the SKIP
# side: skip ⟺ cut fired) ∧ the read-site coupling debug_asserts (the READ side: the advance
# arm reads `remaining_*` only when !cut) — see docs/cesk-gc/phase-c-increment-d-design.md §C.
# This script is the DYNAMIC confirmation: a REAL mid-loop MINOR firing while a post-cut frame
# is live, reclaiming the (correctly) pruned-dead alternative NODES, has 0 UAF.
#
# Builds the `mettatron` CLI under index-gc ASAN (-Zbuild-std nightly, capped, FOREGROUND)
# and runs `cut_young.metta` (3 narrowed K-frames committed by a cut, then post-cut young
# churn) on two arms, each asserting 0 ASAN UAF:
#
#   1. midloop : MIDLOOP=1 + MIN_BYTES high ⇒ a mid-loop MINOR fires (young_alloc > 2 MiB)
#                WHILE a post-cut `cut-pick` frame is on the K-spine; `collect_machine_roots_
#                live` narrows that frame, so the minor's `sweep_young` reclaims the pruned
#                {answer-2, answer-3} nodes and the post-cut churn reuses their slots /
#                triggers young-segment release (side-free is quiescence-gated, OFF here).
#                A wrong-narrow (dropping a LIVE field) ⇒ heap-use-after-free. Expect: 0 UAF,
#                (minor cycle)>0 (non-vacuous), result == [done].
#   2. control : MIDLOOP unset ⇒ collection only at the directive boundary → 0 UAF AND the
#                SAME [done] result (proves the answer is independent of mid-loop collection).
#
# FANOUT_DEPTH=0 (single-threaded). REPORT=2 prints the minor/major split for non-vacuity.
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/debug/mettatron"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t "d_midloop_asan.XXXXXXXX")}"
P="$LOG_DIR/d_midloop_asan"
GIB=$((1024*1024*1024))
echo "===== C #D-2 MIDLOOP NARROWING ASAN ====="; date
echo "repo=$REPO"
echo "logs=$LOG_DIR"

echo "### build mettatron index-gc ASAN bin"
systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 \
  env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
  cargo +nightly build -Zbuild-std --target x86_64-unknown-linux-gnu --bin mettatron -j4 --features index-gc \
  > "${P}_build.log" 2>&1
echo "build_rc=$?"; tail -3 "${P}_build.log"

run_arm() {  # $1=label  $2...=env assignments
  local label="$1"; shift
  echo "### arm: $label  (cut_young.metta)  env: $*"
  env "$@" ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=800% \
    "$BIN" "examples/cesk-gc/cut_young.metta" > "${P}_${label}.log" 2>&1
  echo "${label}_rc=$?"
  echo "  UAF lines:               $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free' "${P}_${label}.log")"
  echo "  GC cycles (minor/major): $(grep -cE 'minor cycle' "${P}_${label}.log") / $(grep -cE 'major cycle' "${P}_${label}.log")"
  echo "  read-site assert panics: $(grep -cE 'D-2:|panicked|assertion failed' "${P}_${label}.log")"
  echo "  result tail:             $(grep -vE '^\[index_gc\]|^Running as' "${P}_${label}.log" | grep -vE '^\s*$' | tail -1)"
  echo "  Error/StackOverflow?:    $(grep -cE 'Error|StackOverflow' "${P}_${label}.log")"
}

run_arm midloop METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIDLOOP=1 METTATRON_INDEX_GC_MIN_BYTES=$GIB METTATRON_INDEX_GC_REPORT=2
run_arm control METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=$GIB METTATRON_INDEX_GC_REPORT=2

echo "===== C #D-2 MIDLOOP NARROWING ASAN VERDICT ====="
ARMS_WITH_UAF=$(grep -lE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free' "${P}_midloop.log" "${P}_control.log" 2>/dev/null | wc -l)
echo "arms with UAF: ${ARMS_WITH_UAF} (expect 0)"
echo "midloop minors (expect >0, non-vacuous): $(grep -cE 'minor cycle' "${P}_midloop.log")"
echo "===== C #D-2 MIDLOOP NARROWING ASAN DONE ====="; date
