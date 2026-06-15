#!/usr/bin/env bash
# Phase E4 — serializable continuations ASAN (the dynamic UAF discharge).
#
#   Usage: scripts/e4_serializable_asan.sh
#
# `restored_future_touch_not_freed` (formal/rocq/gc/SerializableContinuationSlice.v)
# proves a restored suspended CESK state cannot touch a freed address PROVIDED the
# slice is store-closed (Hclosed) and restore re-interns to fresh, non-freed slots.
# This script is the EMPIRICAL discharge of that obligation on the implementation:
# it runs the `continuation_slice` tests under -Zsanitizer=address, INCLUDING
# `restored_future_touch_not_freed_after_source_swept`, which:
#   1. captures a slice (COPYING the closure),
#   2. FORCES a major mark+sweep that reclaims the source closure's σ slots
#      (the source value is dropped + unrooted),
#   3. restores (FRESH re-intern over `slice.bytes`/`children`) + READS the
#      restored value.
# A restore that dereferenced a freed source slot = heap-use-after-free. Expect:
# 0 UAF, all tests pass.
#
# Builds the `continuation_slice` test binary under index-gc ASAN (-Zbuild-std
# nightly, capped, FOREGROUND) and runs it via nextest with abort_on_error.
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
cd "$REPO"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "e4_serializable_asan.XXXXXXXX")}"
P="$LOG_DIR/e4_serializable_asan"
echo "===== E4 SERIALIZABLE CONTINUATIONS ASAN ====="; date
echo "repo=$REPO"
echo "logs=$LOG_DIR"

# ASAN nextest run (build + run in one capped invocation). `-Zbuild-std` rebuilds
# std with the sanitizer; the nextest run then executes the cfg(all(test,
# index-gc)) tests in `continuation_slice`. detect_leaks=0 (the global heap arena
# intentionally outlives the process); abort_on_error surfaces any UAF as a
# nonzero exit.
echo "### build+run continuation_slice tests under index-gc ASAN"
set +e
systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 \
  env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
      ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
  cargo +nightly nextest run -Zbuild-std --target x86_64-unknown-linux-gnu \
    continuation_slice \
  > "${P}.log" 2>&1
rc=$?
set -e
echo "asan_rc=$rc"

UAF="$(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|use-after-free' "${P}.log" || true)"
PASS="$(grep -oE '[0-9]+ tests? run: [0-9]+ passed' "${P}.log" | tail -1 || true)"
FAILS="$(grep -cE 'FAIL \[' "${P}.log" || true)"
echo "  UAF lines:   $UAF (expect 0)"
echo "  summary:     ${PASS:-missing}"
echo "  test fails:  $FAILS (expect 0)"
echo "---- tail ----"
tail -25 "${P}.log"

echo "===== E4 SERIALIZABLE CONTINUATIONS ASAN VERDICT ====="
if [[ "$rc" -ne 0 || "$UAF" -ne 0 || "$FAILS" -ne 0 ]]; then
  echo "RESULT: FAIL (rc=$rc uaf=$UAF fails=$FAILS)"
  exit 1
fi
echo "RESULT: PASS (0 UAF, all continuation_slice tests green under ASAN)"
date
