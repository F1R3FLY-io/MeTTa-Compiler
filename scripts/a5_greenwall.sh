#!/usr/bin/env bash
# A5 per-sub-step green-wall (CESK GC migration, Phase A5).
#
#   Usage: scripts/a5_greenwall.sh <step-label> [--with-oracle]
#
# Runs (all heavy ops capped under systemd-run, MemorySwapMax=0, FOREGROUND
# within this script — launch the SCRIPT itself in the background):
#   1. SLAB  nextest (release)              → expect 4325 pass / 0 fail
#   2. INDEX nextest (release, index-gc)    → expect 4177 pass / 0 fail
#   3. INDEX conformance bin build (release)
#   4. INDEX conformance (release, all 744) → expect 744 pass (483+221+40), cycles>0
#   --with-oracle (for A5.1+ steps touching the collection path):
#   5. INDEX conformance DEBUG, MIN_BYTES=1 → machine-equivalence oracle fires
#      every collection; expect 0 oracle panics, 744 pass.
#
# Baselines (390b743 / A4.4): slab 4325, index 4177, conformance 744.
set -uo pipefail
LABEL="${1:?usage: a5_greenwall.sh <label> [--with-oracle]}"
WITH_ORACLE=0; [[ "${2:-}" == "--with-oracle" ]] && WITH_ORACLE=1
REPO=/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
CONF_DIR="${CONFORMANCE_DIR:-/home/dylon/Workspace/f1r3fly.io/mettatron-specification/conformance}"
cd "$REPO"
CAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600%)
RUNBIN=(systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 -p CPUQuota=1600%)
P=/tmp/a5_${LABEL}
echo "===== A5 GREEN-WALL [$LABEL] ====="; date

echo "### 1 SLAB nextest (expect 4325/0)"
"${CAP[@]}" cargo nextest run --release > ${P}_slab_nextest.log 2>&1; echo "slab_rc=$?"
grep -E "Summary|tests run|^ *FAIL|TIMEOUT" ${P}_slab_nextest.log | tail -4

echo "### 2 INDEX nextest (expect 4177/0)"
"${CAP[@]}" cargo nextest run --release --features index-gc > ${P}_index_nextest.log 2>&1; echo "index_rc=$?"
grep -E "Summary|tests run|^ *FAIL|TIMEOUT" ${P}_index_nextest.log | tail -4

echo "### 3 INDEX conformance bin build (release)"
"${CAP[@]}" cargo build --release --features index-gc --bin mtt-conformance > ${P}_confbuild.log 2>&1; echo "confbuild_rc=$?"
tail -2 ${P}_confbuild.log

echo "### 4 INDEX conformance (release; expect 744 pass = 483+221+40, cycles>0)"
FANOUT_DEPTH=0 METTATRON_INDEX_GC_REPORT=1 "${RUNBIN[@]}" \
  "$REPO/target/release/mtt-conformance" --conformance-dir "$CONF_DIR" --strict > ${P}_conf.log 2>&1
echo "conf_rc=$?"
grep -E "^Found |^Summary:" ${P}_conf.log
grep INDEX_GC_CYCLES ${P}_conf.log
echo "fails+errors: $(grep -cE ': (FAIL|ERROR)' ${P}_conf.log)"
echo "per-module PASS counts:"
grep ': PASS' ${P}_conf.log | sed -E 's#/.*##' | sort | uniq -c

if [[ "$WITH_ORACLE" == "1" ]]; then
  echo "### 5 INDEX conformance DEBUG (machine-equivalence oracle, MIN_BYTES=1; expect 0 panics, 744 pass)"
  "${CAP[@]}" cargo build --features index-gc --bin mtt-conformance > ${P}_confbuild_debug.log 2>&1; echo "confbuild_debug_rc=$?"
  tail -2 ${P}_confbuild_debug.log
  FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=1 "${RUNBIN[@]}" \
    "$REPO/target/debug/mtt-conformance" --conformance-dir "$CONF_DIR" --strict > ${P}_conf_debug.log 2>&1
  echo "conf_debug_rc=$?"
  echo "debug Summary: $(grep -E '^Summary:' ${P}_conf_debug.log)"
  echo "debug oracle panics: $(grep -ciE 'panic|OLD .*NOT|superset' ${P}_conf_debug.log)"
  echo "debug fails+errors: $(grep -cE ': (FAIL|ERROR)' ${P}_conf_debug.log)"
fi
echo "===== A5 GREEN-WALL [$LABEL] DONE ====="; date
