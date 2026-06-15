#!/usr/bin/env bash
# Phase E4 — rholang ship/resume COMPILE gate.
#
#   Usage: scripts/e4_rholang_resume_gate.sh
#
# The faithful directive-granularity ship/resume wrapper
# (rholang_integration::{run_state_async_resumable, resume_shipped}) lives behind
# `#[cfg(all(feature = "async", feature = "index-gc"))]` inside the
# `#[cfg(feature = "rholang")]` module. NO other gate compiles the
# `index-gc + rholang` feature combination (the greenwall / ASAN / formal harness all
# build `` WITHOUT rholang), so without this gate the ship path
# would bit-rot uncompiled.
#
# The rholang runtime is NOT empirically runnable in this environment, so the ship
# path is verified by (a) THIS compile-check and (b) the structural source-coupling
# pins (scripts/verify_cesk_gc_source_coupling.sh, the R1-R4 block) — NOT a
# behavioral round-trip test. The SAFETY of restore (no use-after-free over the
# re-interned σ) is already discharged by the committed core: the proof
# formal/rocq/gc/SerializableContinuationSlice.v + the ASAN run
# scripts/e4_serializable_asan.sh, both over capture_slice/restore_slice which the
# wrapper reuses verbatim.
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
cd "$REPO"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG="${LOG:-$(mktemp -p "$LOG_ROOT" "e4_rholang_gate.XXXXXXXX.log")}"
echo "===== E4 RHOLANG SHIP/RESUME COMPILE GATE ====="; date
echo "repo=$REPO"; echo "log=$LOG"

set +e
systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1200% --quiet \
  cargo check --features rholang > "$LOG" 2>&1
rc=$?
set -e

WARN="$(grep -aoE 'generated [0-9]+ warning' "$LOG" | tail -1 || true)"
echo "check_rc=$rc  (${WARN:-no warning summary})"
echo "---- tail ----"
tail -4 "$LOG"

echo "===== E4 RHOLANG SHIP/RESUME COMPILE GATE VERDICT ====="
if [[ "$rc" -ne 0 ]]; then
  echo "RESULT: FAIL (index-gc,rholang does not compile)"
  exit 1
fi
echo "RESULT: PASS (faithful rholang ship/resume compiles under --features rholang)"
date
