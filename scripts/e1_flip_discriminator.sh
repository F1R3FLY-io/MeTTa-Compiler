#!/usr/bin/env bash
# E1-FLIP corruptness discriminator — default dedicated robot @ FANOUT=8 check.
#
# Re-confirms the converged coordination fix (①a+①c+②+③ + CEX-1): under the
# default dedicated GC thread, the FANOUT>0 rendezvous produces the expected PLN
# self-verdict. The bug signature was a VALID-BUT-WRONG SUBSET, so the robot
# harness' sorted self-check is the relevant metric.
#
#   Arm A: MIN=131072       → rendezvous fires + SWEEPS (reclaim>0)
#   Arm C: MIN=4294967295    → rendezvous fires, NO sweep pressure from low floor
#
# Pass = 0/N mismatch on BOTH arms, with arm A reclaim>0 (non-vacuous sweep).
# Runs the PREBUILT index binary directly (no cargo) → valid under sibling build load
# (output is CPU-independent; ample thread headroom rules out the work-pool EAGAIN flake).
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN_DIR="${PLN_DIR:-$REPO_PARENT/PLN-main}"
BIN="${BIN:-$REPO/target/release/mettatron}"
ROBOT="${ROBOT:-$PLN_DIR/examples/Robot.metta}"
N="${N:-16}"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
P="${P:-$(mktemp -d -p "$LOG_ROOT" "e1_disc.XXXXXXXX")}"
mkdir -p "$P"

# CORRECT METRIC (the prior "count ❌" was an ARTIFACT — a run that drops a live
# atom makes robot's harness ABORT on the first failed assertion, truncating output
# so it has ZERO ❌ and was falsely scored "clean"). robot is SELF-CHECKING: a
# correct run prints its assertion verdict `… ✅` and the full ~404-line output; a
# corrupt run prints `… ❌` AND/OR truncates (✅ absent). So a run PASSES iff it
# contains `✅` and NOT `❌` (freshvar nondeterminism in $__fr_N makes raw-diff
# useless; the verdict is freshvar-independent). HANG (timeout) and OOM are failures.
TIMEOUT="${TIMEOUT:-120}"
run_verdict() { # $1=min  → echoes "PASS"|"FAIL(reason)"
  local out rc bad ok ln
  out=$(timeout "$TIMEOUT" env METTATRON_PARALLEL_FANOUT_DEPTH=8 \
      METTATRON_INDEX_GC_MIN_BYTES="$1" "$BIN" --gc index "$ROBOT" 2>/dev/null); rc=$?
  if [ "$rc" -eq 124 ]; then echo "FAIL(HANG)"; return; fi
  if [ "$rc" -eq 137 ]; then echo "FAIL(OOM)"; return; fi
  bad=$(printf '%s' "$out" | grep -c '❌'); ok=$(printf '%s' "$out" | grep -c '✅')
  ln=$(printf '%s' "$out" | grep -vE '^\[index_gc\]' | grep -c .)
  if [ "$ok" -ge 1 ] && [ "$bad" -eq 0 ]; then echo "PASS(lines=$ln)"; else echo "FAIL(❌=$bad ✅=$ok lines=$ln)"; fi
}

echo "===== E1-FLIP DISCRIMINATOR (robot @ FANOUT=8 ×$N, default dedicated, 2 arms) ====="; date
echo "BIN=$BIN  metric=✅-present-and-no-❌ (truncation/HANG/OOM = FAIL)"
echo "robot=$ROBOT"
echo "scratch=$P"

declare -A MIN=( [A]=131072 [C]=4294967295 )
RESULT=""
for arm in A C; do
  fail=0
  for i in $(seq 1 "$N"); do
    v=$(run_verdict "${MIN[$arm]}")
    echo "  [$arm] run$i: $v"
    case "$v" in PASS*) : ;; *) fail=$((fail+1)) ;; esac
  done
  line="ARM $arm (MIN=${MIN[$arm]}): $fail/$N FAIL"
  echo "$line"; RESULT="$RESULT$line"$'\n'
done

# Non-vacuity: confirm arm A actually swept (reclaimed slots > 0).
echo "--- non-vacuity: arm A reclaim (REPORT=2) ---"
env METTATRON_PARALLEL_FANOUT_DEPTH=8 \
    METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=2 \
  "$BIN" --gc index "$ROBOT" 2>&1 | grep -E 'rendezvous|reclaim' | tail -6

echo "===== DISCRIMINATOR SUMMARY ====="
printf '%s' "$RESULT"
echo "PASS criterion: 0/$N on both arms (+ arm A reclaim>0)"
date
