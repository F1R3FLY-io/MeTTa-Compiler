#!/usr/bin/env bash
# A5 ASAN both builds — for the sub-steps that touch SLAB registration cfg
# (A5.3 / A5.5 / A5.6). The #1 A5 risk is a cfg seam silently dropping a SLAB
# root provider → latent UAF release tests rarely surface; slab ASAN is the
# sensitive catch (alongside the slab debug oracle + byte-identical nextest).
#
#   Usage: scripts/a5_asan_both.sh <label>
#
# SLAB  ASAN: FULL conformance (483) — exercises every provider's collection at
#             session release (env/caches/MettaState); the env-provider drop is
#             the worst case (RuleEntry.lhs/rhs freed → UAF in match_rules).
# INDEX ASAN: M11-bisimilarity-pt (221, oracle live) — NOT stress_multidir-CLI
#             (pre-existing CLI driver-C gap trips the oracle; that's A5.4).
# Both -Zbuild-std nightly ASAN, capped 32G build / 24G run, FOREGROUND, serial.
set -uo pipefail
LABEL="${1:?usage: a5_asan_both.sh <label>}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
CONF="${CONFORMANCE_DIR:-$REPO_PARENT/mettatron-specification/conformance}"
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/debug/mtt-conformance"
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t "a5_${SAFE_LABEL}_asan.XXXXXXXX")}"
P="$LOG_DIR/a5_${SAFE_LABEL}_asan"
build_asan() {  # $1... = extra cargo args (e.g. --features index-gc)
  systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 \
    env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
    cargo +nightly build -Zbuild-std --target x86_64-unknown-linux-gnu --bin mtt-conformance -j4 "$@"
}
echo "===== A5 ASAN BOTH [$LABEL] ====="; date
echo "repo=$REPO"
echo "conformance_dir=$CONF"
echo "logs=$LOG_DIR"

echo "### build SLAB ASAN bin"
build_asan > "${P}_slab_build.log" 2>&1; echo "slab_build_rc=$?"; tail -2 "${P}_slab_build.log"
echo "### SLAB conformance ASAN (full 483 — catches a dropped slab provider)"
ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=800% \
  "$BIN" --conformance-dir "$CONF" --strict > "${P}_slab_conf.log" 2>&1
echo "slab_conf_rc=$?"; grep -E "^Summary:" "${P}_slab_conf.log"

echo "### build INDEX ASAN bin (--features index-gc; overwrites the bin)"
build_asan --features index-gc > "${P}_index_build.log" 2>&1; echo "index_build_rc=$?"; tail -2 "${P}_index_build.log"
echo "### INDEX M11-bisimilarity-pt ASAN (no MIDLOOP, oracle live)"
METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1 \
  ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=800% \
  "$BIN" --conformance-dir "$CONF" --module M11-bisimilarity-pt --strict > "${P}_index_m11pt.log" 2>&1
echo "index_m11pt_rc=$?"; grep -E "^Summary:|INDEX_GC_CYCLES_RUN" "${P}_index_m11pt.log"

echo "===== ASAN VERDICT [$LABEL] ====="
echo "slab ASAN UAF lines:  $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison' "${P}_slab_conf.log")"
echo "index ASAN UAF lines: $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison' "${P}_index_m11pt.log")"
echo "index oracle panics:  $(grep -cE 'oracle FAILED|machine-equivalence' "${P}_index_m11pt.log")"
echo "slab conf fails:   $(grep -cE ': (FAIL|ERROR)' "${P}_slab_conf.log")"
echo "index m11pt fails: $(grep -cE ': (FAIL|ERROR)' "${P}_index_m11pt.log")"
echo "===== A5 ASAN BOTH [$LABEL] DONE ====="; date
