#!/usr/bin/env bash
# F3 default-store soak.
#
# Rebuilds fresh binaries and exercises the observable F3 contract:
#   * default features compile the index store;
#   * default-env CLI and REPL sessions assert/report index and evaluate work;
#   * a slab request against the default-index binary hard-errors, reporting the
#     slab store is decommissioned (F4 R2 removed the legacy slab build).
#
# Usage: scripts/f3_default_store_soak.sh [label]
set -euo pipefail

LABEL="${1:-f3-default-store}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
cd "$REPO"

SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "f3_${SAFE_LABEL}.XXXXXXXX")}"
P="$LOG_DIR/f3_${SAFE_LABEL}"
WORKLOAD="$LOG_DIR/f3_smoke.metta"
INDEX_BIN="$LOG_DIR/mettatron-default-index"

BUILD_CAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1200% --quiet timeout --signal=TERM --kill-after=10s 600s)
RUN_CAP=(systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400% --quiet timeout --signal=TERM --kill-after=5s 60s)

cat > "$WORKLOAD" <<'METTA'
!(+ 1 2)
METTA

fail_with_log() {
  local label="$1" log="$2" detail="$3"
  echo "ERROR: $label: $detail" >&2
  if [[ -f "$log" ]]; then
    echo "---- $log ----" >&2
    sed -n '1,160p' "$log" >&2
  fi
  exit 1
}

assert_contains() {
  local label="$1" log="$2" needle="$3"
  if ! grep -F -- "$needle" "$log" >/dev/null; then
    fail_with_log "$label" "$log" "missing expected text: $needle"
  fi
}

assert_not_contains() {
  local label="$1" log="$2" needle="$3"
  if grep -F -- "$needle" "$log" >/dev/null; then
    fail_with_log "$label" "$log" "unexpected text: $needle"
  fi
}

run_logged() {
  local label="$1" log="$2"
  shift 2
  set +e
  "$@" >"$log" 2>&1
  local rc=$?
  set -e
  echo "${label}_rc=$rc"
  if [[ "$rc" -ne 0 ]]; then
    fail_with_log "$label" "$log" "command exited rc=$rc"
  fi
}

echo "===== F3 DEFAULT-STORE SOAK [$LABEL] ====="; date
echo "repo=$REPO"
echo "logs=$LOG_DIR"
echo "head=$(git rev-parse --short HEAD)"

echo "### build default-index mettatron"
run_logged build_index "${P}_build_index.log" "${BUILD_CAP[@]}" cargo build --release --bin mettatron
cp target/release/mettatron "$INDEX_BIN"

echo "### default-env CLI stdin asserts index and evaluates"
run_logged cli_index "${P}_cli_index.log" \
  env -u MTT_GC -u METTATRON_PARALLEL_FANOUT_DEPTH -u METTATRON_INDEX_GC_DISABLE \
  bash -c 'printf "%s\n" "!(+ 1 2)" | "$@"' bash "${RUN_CAP[@]}" "$INDEX_BIN" --gc index -
assert_contains cli_index "${P}_cli_index.log" "[mettatron] GC store = index"
assert_contains cli_index "${P}_cli_index.log" "[3]"

echo "### default-env REPL asserts index, evaluates, and exits"
run_logged repl_index "${P}_repl_index.log" \
  env -u MTT_GC -u METTATRON_PARALLEL_FANOUT_DEPTH -u METTATRON_INDEX_GC_DISABLE \
  bash -c 'printf "%s\n%s\n" "!(+ 1 2)" "quit" | "$@"' bash "${RUN_CAP[@]}" "$INDEX_BIN" --gc index --repl
assert_contains repl_index "${P}_repl_index.log" "[mettatron] GC store = index"
assert_contains repl_index "${P}_repl_index.log" "MeTTaTron REPL"
assert_contains repl_index "${P}_repl_index.log" "[3]"
assert_contains repl_index "${P}_repl_index.log" "Goodbye!"

echo "### default-index binary rejects slab requests (slab store decommissioned)"
set +e
env -u METTATRON_PARALLEL_FANOUT_DEPTH -u METTATRON_INDEX_GC_DISABLE MTT_GC=slab \
  "${RUN_CAP[@]}" "$INDEX_BIN" "$WORKLOAD" >"${P}_mismatch_slab.log" 2>&1
mismatch_rc=$?
set -e
echo "mismatch_slab_rc=$mismatch_rc"
if [[ "$mismatch_rc" -eq 0 ]]; then
  fail_with_log mismatch_slab "${P}_mismatch_slab.log" "slab request unexpectedly succeeded against default-index binary"
fi
assert_contains mismatch_slab "${P}_mismatch_slab.log" "compiled with the 'index' GC store"
assert_contains mismatch_slab "${P}_mismatch_slab.log" "the slab store has been decommissioned"
assert_not_contains mismatch_slab "${P}_mismatch_slab.log" 'without `index-gc`'

echo "===== F3 DEFAULT-STORE SOAK [$LABEL] DONE ====="; date
echo "artifacts=$LOG_DIR"
