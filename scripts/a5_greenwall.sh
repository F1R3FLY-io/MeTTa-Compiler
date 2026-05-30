#!/usr/bin/env bash
# A5 per-sub-step green-wall (CESK GC migration, Phase A5).
#
#   Usage: scripts/a5_greenwall.sh <step-label> [--with-oracle]
#
# Runs (all heavy ops capped under systemd-run, MemorySwapMax=0, FOREGROUND
# within this script — launch the SCRIPT itself in the background):
#   1. SLAB  nextest (release)              → expect 4324 pass / 0 fail
#   2. INDEX nextest (release, index-gc)    → expect 4167 pass / 0 fail
#   3. INDEX conformance bin build (release)
#   4. INDEX conformance (release, all 483) → expect 483 pass (base 222 +
#      M11-pt 221 + M11-he 40, the latter two SUBSETS of 483), cycles>0 (~840).
#   --with-oracle (for steps touching the collection path):
#   5. INDEX conformance DEBUG, MIN_BYTES=1 → machine-equivalence oracle fires
#      every collection; expect 0 oracle panics, 483 pass.
#
# Baselines (dbc6fa8 / A5.7, Phase A complete): slab 4324, index 4167, conf 483.
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

echo "### 1 SLAB nextest (expect 4324/0)"
"${CAP[@]}" cargo nextest run --release > ${P}_slab_nextest.log 2>&1; echo "slab_rc=$?"
grep -E "Summary|tests run|^ *FAIL|TIMEOUT" ${P}_slab_nextest.log | tail -4

echo "### 2 INDEX nextest (expect 4167/0)"
"${CAP[@]}" cargo nextest run --release --features index-gc > ${P}_index_nextest.log 2>&1; echo "index_rc=$?"
grep -E "Summary|tests run|^ *FAIL|TIMEOUT" ${P}_index_nextest.log | tail -4

echo "### 3 INDEX conformance bin build (release)"
"${CAP[@]}" cargo build --release --features index-gc --bin mtt-conformance > ${P}_confbuild.log 2>&1; echo "confbuild_rc=$?"
tail -2 ${P}_confbuild.log

echo "### 4 INDEX conformance (release, all 483; cycles>0 ~840). FANOUT_DEPTH=0 forces"
echo "    single-threaded so worker_ever_spawned never latches; MIN_BYTES=128KiB so the"
echo "    quiescence collector fires often (non-vacuous)."
METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1 "${RUNBIN[@]}" \
  "$REPO/target/release/mtt-conformance" --conformance-dir "$CONF_DIR" --strict > ${P}_conf.log 2>&1
echo "conf_rc=$?"
grep -E "^Found |^Summary:" ${P}_conf.log
grep INDEX_GC_CYCLES ${P}_conf.log
echo "fails+errors: $(grep -cE ': (FAIL|ERROR)' ${P}_conf.log)"
echo "per-module PASS counts:"
grep ': PASS' ${P}_conf.log | sed -E 's#/.*##' | sort | uniq -c

echo "### 4b WARNINGS — mettatron lib (0-new-warnings gate; A5 baseline = 49 BOTH builds)."
echo "    (nextest logs report '(lib test)' which is noisy; cargo check gives the clean '(lib)' count.)"
"${CAP[@]}" cargo check > ${P}_wcheck_slab.log 2>&1
echo "slab lib:  $(grep -oE 'mettatron. \(lib\) generated [0-9]+ warning' ${P}_wcheck_slab.log | grep -oE '[0-9]+' | head -1) (expect 49)"
"${CAP[@]}" cargo check --features index-gc > ${P}_wcheck_index.log 2>&1
echo "index lib: $(grep -oE 'mettatron. \(lib\) generated [0-9]+ warning' ${P}_wcheck_index.log | grep -oE '[0-9]+' | head -1) (expect 49)"

if [[ "$WITH_ORACLE" == "1" ]]; then
  echo "### 5 INDEX conformance DEBUG (machine-equivalence oracle, MIN_BYTES=1; expect 0 panics, 483 pass)"
  "${CAP[@]}" cargo build --features index-gc --bin mtt-conformance > ${P}_confbuild_debug.log 2>&1; echo "confbuild_debug_rc=$?"
  tail -2 ${P}_confbuild_debug.log
  METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1 "${RUNBIN[@]}" \
    "$REPO/target/debug/mtt-conformance" --conformance-dir "$CONF_DIR" --strict > ${P}_conf_debug.log 2>&1
  echo "conf_debug_rc=$?"
  echo "debug Summary: $(grep -E '^Summary:' ${P}_conf_debug.log)"
  echo "debug oracle panics: $(grep -ciE 'panic|OLD .*NOT|superset' ${P}_conf_debug.log)"
  echo "debug fails+errors: $(grep -cE ': (FAIL|ERROR)' ${P}_conf_debug.log)"
fi
echo "===== A5 GREEN-WALL [$LABEL] DONE ====="; date
