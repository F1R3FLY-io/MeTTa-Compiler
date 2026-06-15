#!/usr/bin/env bash
# A5 ASAN (index store) — the legacy slab arm was removed at F4 R2 (slab
# decommissioned), so this now runs the INDEX ASAN gate only.
#
#   Usage: scripts/a5_asan_both.sh <label>
#
# INDEX ASAN: M11-bisimilarity-pt (221, oracle live) — NOT stress_multidir-CLI
#             (pre-existing CLI driver-C gap trips the oracle; that's A5.4).
# -Zbuild-std nightly ASAN, capped 32G build / 24G run, FOREGROUND, serial.
set -uo pipefail
LABEL="${1:?usage: a5_asan_both.sh <label>}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
CONF="${CONFORMANCE_DIR:-$REPO_PARENT/mettatron-specification/conformance}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/debug/mtt-conformance"
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "a5_${SAFE_LABEL}_asan.XXXXXXXX")}"
P="$LOG_DIR/a5_${SAFE_LABEL}_asan"
build_asan() {  # $1... = extra cargo args (e.g. legacy-slab opt-out)
  systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 \
    env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
    cargo +nightly build -Zbuild-std --target x86_64-unknown-linux-gnu --bin mtt-conformance -j4 "$@"
}
echo "===== A5 ASAN BOTH [$LABEL] ====="; date
echo "repo=$REPO"
echo "conformance_dir=$CONF"
echo "logs=$LOG_DIR"

echo "### build DEFAULT-INDEX ASAN bin"
build_asan > "${P}_index_build.log" 2>&1; echo "index_build_rc=$?"; tail -2 "${P}_index_build.log"
echo "### INDEX M11-bisimilarity-pt ASAN (default mid-loop gate, oracle live)"
METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1 \
  ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=800% \
  "$BIN" --conformance-dir "$CONF" --module M11-bisimilarity-pt --strict > "${P}_index_m11pt.log" 2>&1
echo "index_m11pt_rc=$?"; grep -E "^Summary:|INDEX_GC_CYCLES_RUN" "${P}_index_m11pt.log"

echo "===== ASAN VERDICT [$LABEL] ====="
echo "index ASAN UAF lines: $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison' "${P}_index_m11pt.log")"
echo "index oracle panics:  $(grep -cE 'oracle FAILED|machine-equivalence' "${P}_index_m11pt.log")"
echo "index m11pt fails: $(grep -cE ': (FAIL|ERROR)' "${P}_index_m11pt.log")"
echo "===== A5 ASAN BOTH [$LABEL] DONE ====="; date
