#!/usr/bin/env bash
# D-RLOCK.2 gate driver (concurrent IndexFactory allocation). All heavy ops capped
# at MemoryMax=20G MemorySwapMax=0 (shared machine), FOREGROUND. Each step writes a
# log under a temporary directory so it runs ONCE. Pass a step name as $1.
#
#   nextest_slab | nextest_index | conf_f0 | conf_oracle | mmverify
set -uo pipefail
STEP="${1:?usage: drlock_gate.sh <step>}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
CONF_DIR="${CONFORMANCE_DIR:-$REPO_PARENT/mettatron-specification/conformance}"
cd "$REPO"
CAP=(systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000%)
SAFE_STEP="${STEP//[^A-Za-z0-9_.-]/_}"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t "drlock_${SAFE_STEP}.XXXXXXXX")}"
P="$LOG_DIR/drlock_${SAFE_STEP}"
echo "repo=$REPO"
echo "conformance_dir=$CONF_DIR"
echo "logs=$LOG_DIR"

case "$STEP" in
  nextest_slab)
    echo "### SLAB nextest (expect 4343/0)"; date
    "${CAP[@]}" cargo nextest run --release > "${P}_slab_nextest.log" 2>&1; echo "slab_rc=$?"
    grep -E "Summary|tests run|^ *FAIL|TIMEOUT|Starting" "${P}_slab_nextest.log" | tail -6
    ;;
  nextest_index)
    echo "### INDEX nextest (expect ~4186/0)"; date
    "${CAP[@]}" cargo nextest run --release --features index-gc > "${P}_index_nextest.log" 2>&1; echo "index_rc=$?"
    grep -E "Summary|tests run|^ *FAIL|TIMEOUT|Starting" "${P}_index_nextest.log" | tail -6
    ;;
  conf_build)
    echo "### INDEX conformance bin build (release)"; date
    "${CAP[@]}" cargo build --release --features index-gc --bin mtt-conformance > "${P}_confbuild.log" 2>&1; echo "confbuild_rc=$?"
    tail -3 "${P}_confbuild.log"
    ;;
  conf_f0)
    echo "### INDEX conformance FANOUT=0 (expect 483/0, cycles>0 ~840)"; date
    METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1 "${CAP[@]}" \
      "$REPO/target/release/mtt-conformance" --conformance-dir "$CONF_DIR" --strict > "${P}_conf_f0.log" 2>&1
    echo "conf_rc=$?"
    grep -E "^Found |^Summary:" "${P}_conf_f0.log"
    grep INDEX_GC_CYCLES "${P}_conf_f0.log" | tail -1
    echo "fails+errors: $(grep -cE ': (FAIL|ERROR)' "${P}_conf_f0.log")"
    ;;
  conf_oracle)
    echo "### INDEX conformance DEBUG oracle (MIN_BYTES=131072; expect 0 panics, 483 pass)"; date
    "${CAP[@]}" cargo build --features index-gc --bin mtt-conformance > "${P}_confbuild_debug.log" 2>&1; echo "confbuild_debug_rc=$?"
    tail -2 "${P}_confbuild_debug.log"
    METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1 "${CAP[@]}" \
      "$REPO/target/debug/mtt-conformance" --conformance-dir "$CONF_DIR" --strict > "${P}_conf_oracle.log" 2>&1
    echo "conf_debug_rc=$?"
    echo "debug Summary: $(grep -E '^Summary:' "${P}_conf_oracle.log")"
    echo "debug oracle panics: $(grep -ciE 'panic|OLD .*NOT|superset' "${P}_conf_oracle.log")"
    echo "debug fails+errors: $(grep -cE ': (FAIL|ERROR)' "${P}_conf_oracle.log")"
    ;;
  *) echo "unknown step: $STEP"; exit 2;;
esac
echo "### DONE $STEP"; date
