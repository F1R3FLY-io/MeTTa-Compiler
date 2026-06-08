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
#      M11-pt 221 + M11-he 40, the latter two SUBSETS of 483), cycles>0.
#   --with-oracle (for steps touching the collection path):
#   5. INDEX conformance DEBUG, MAX_BYTES=1MiB → machine-equivalence oracle fires
#      every collection; expect 0 oracle panics, 483 pass.
#
# Baselines (dbc6fa8 / A5.7, Phase A complete): slab 4324, index 4167, conf 483.
set -euo pipefail
LABEL="${1:?usage: a5_greenwall.sh <label> [--with-oracle]}"
WITH_ORACLE=0; [[ "${2:-}" == "--with-oracle" ]] && WITH_ORACLE=1
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
CONF_DIR="${CONFORMANCE_DIR:-$REPO_PARENT/mettatron-specification/conformance}"
cd "$REPO"
CAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600%)
RUNBIN=(systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 -p CPUQuota=1600%)
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "a5_${SAFE_LABEL}.XXXXXXXX")}"
P="$LOG_DIR/a5_${SAFE_LABEL}"

fail_with_log() {
  local label="$1" log="$2" detail="$3"
  echo "ERROR: $label: $detail" >&2
  if [[ -f "$log" ]]; then
    echo "---- tail $log ----" >&2
    tail -120 "$log" >&2
  fi
  exit 1
}

run_logged() {
  local label="$1" log="$2"
  shift 2
  set +e
  "$@" > "$log" 2>&1
  local rc=$?
  set -e
  echo "${label}_rc=$rc"
  if [[ "$rc" -ne 0 ]]; then
    fail_with_log "$label" "$log" "command exited rc=$rc"
  fi
}

require_summary_483() {
  local label="$1" log="$2" summary
  summary="$(grep -E '^Summary:' "$log" | tail -1 || true)"
  if [[ ! "$summary" =~ ^Summary:\ 483\ pass,\ 0\ fail,\ 0\ error,\ 0\ skipped$ ]]; then
    fail_with_log "$label" "$log" "unexpected conformance summary: ${summary:-missing}"
  fi
}

require_no_failures() {
  local label="$1" log="$2" count
  count="$(grep -cE ': (FAIL|ERROR)' "$log" || true)"
  echo "fails+errors: $count"
  if [[ "$count" -ne 0 ]]; then
    fail_with_log "$label" "$log" "reported $count conformance failures/errors"
  fi
}

require_warning_count() {
  local label="$1" log="$2" expected="$3" count
  count="$(grep -oE 'mettatron. \(lib\) generated [0-9]+ warnings?' "$log" | grep -oE '[0-9]+' | head -1 || true)"
  echo "$label: ${count:-missing} (expect $expected)"
  if [[ "${count:-}" != "$expected" ]]; then
    fail_with_log "$label" "$log" "warning count ${count:-missing}, expected $expected"
  fi
}

echo "===== A5 GREEN-WALL [$LABEL] ====="; date
echo "repo=$REPO"
echo "conformance_dir=$CONF_DIR"
echo "logs=$LOG_DIR"

echo "### 1 SLAB nextest (expect 4324/0)"
run_logged slab "${P}_slab_nextest.log" "${CAP[@]}" cargo nextest run --release
grep -E "Summary|tests run|^ *FAIL|TIMEOUT" "${P}_slab_nextest.log" | tail -4 || true

echo "### 2 INDEX nextest (expect 4167/0)"
run_logged index "${P}_index_nextest.log" "${CAP[@]}" cargo nextest run --release --features index-gc
grep -E "Summary|tests run|^ *FAIL|TIMEOUT" "${P}_index_nextest.log" | tail -4 || true

echo "### 3 INDEX conformance bin build (release)"
run_logged confbuild "${P}_confbuild.log" "${CAP[@]}" cargo build --release --features index-gc --bin mtt-conformance
tail -2 "${P}_confbuild.log"

echo "### 4 INDEX conformance (release, all 483; cycles>0). FANOUT_DEPTH=0 forces"
echo "    single-threaded so worker_ever_spawned never latches; MAX_BYTES=1MiB"
echo "    trips the committed-cap trigger on this corpus (non-vacuous)."
run_logged conf "${P}_conf.log" env \
  METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MAX_BYTES=1048576 METTATRON_INDEX_GC_REPORT=1 \
  "${RUNBIN[@]}" "$REPO/target/release/mtt-conformance" --conformance-dir "$CONF_DIR" --strict
grep -E "^Found |^Summary:" "${P}_conf.log"
grep INDEX_GC_CYCLES "${P}_conf.log"
CYCLES=$(grep -oE 'INDEX_GC_CYCLES_RUN=[0-9]+' "${P}_conf.log" | tail -1 | cut -d= -f2)
if [[ -z "$CYCLES" || "$CYCLES" -le 0 ]]; then
  echo "ERROR: index conformance was GC-vacuous (INDEX_GC_CYCLES_RUN=${CYCLES:-missing})"
  exit 1
fi
require_summary_483 conf "${P}_conf.log"
require_no_failures conf "${P}_conf.log"
echo "per-module PASS counts:"
grep ': PASS' "${P}_conf.log" | sed -E 's#/.*##' | sort | uniq -c

echo "### 4b WARNINGS — mettatron lib (0-new-warnings gate; A5 baseline = 49 BOTH builds)."
echo "    (nextest logs report '(lib test)' which is noisy; cargo check gives the clean '(lib)' count.)"
run_logged wcheck_slab "${P}_wcheck_slab.log" "${CAP[@]}" cargo check
require_warning_count "slab lib" "${P}_wcheck_slab.log" 49
run_logged wcheck_index "${P}_wcheck_index.log" "${CAP[@]}" cargo check --features index-gc
require_warning_count "index lib" "${P}_wcheck_index.log" 49

if [[ "$WITH_ORACLE" == "1" ]]; then
  echo "### 5 INDEX conformance DEBUG (machine-equivalence oracle, MAX_BYTES=1MiB; expect 0 panics, 483 pass)"
  run_logged confbuild_debug "${P}_confbuild_debug.log" "${CAP[@]}" cargo build --features index-gc --bin mtt-conformance
  tail -2 "${P}_confbuild_debug.log"
  run_logged conf_debug "${P}_conf_debug.log" env \
    METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MAX_BYTES=1048576 METTATRON_INDEX_GC_REPORT=1 \
    "${RUNBIN[@]}" "$REPO/target/debug/mtt-conformance" --conformance-dir "$CONF_DIR" --strict
  echo "debug Summary: $(grep -E '^Summary:' "${P}_conf_debug.log")"
  require_summary_483 conf_debug "${P}_conf_debug.log"
  oracle_panics="$(grep -ciE 'panic|OLD .*NOT|superset' "${P}_conf_debug.log" || true)"
  echo "debug oracle panics: $oracle_panics"
  if [[ "$oracle_panics" -ne 0 ]]; then
    fail_with_log conf_debug "${P}_conf_debug.log" "machine-equivalence oracle reported $oracle_panics panic/superset lines"
  fi
  require_no_failures conf_debug "${P}_conf_debug.log"
fi
echo "===== A5 GREEN-WALL [$LABEL] DONE ====="; date
