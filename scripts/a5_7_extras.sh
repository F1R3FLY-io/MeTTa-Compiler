#!/usr/bin/env bash
# A5.7 final-gate EXTRAS (beyond the per-step green-wall + ASAN): the parts of the
# master-plan per-rung gate not in a5_greenwall.sh —
#   mmverify "Correct proof" + PLN budgets (Robot ≤12s, FlyingRaven ≤25s, RSS ≤8G via
#   the 8G cap) + 20-run conformance result-determinism — all on the INDEX build.
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
CONF="${CONFORMANCE_DIR:-$REPO_PARENT/mettatron-specification/conformance}"
cd "$REPO"
CAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600%)
PLNRUN=(systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400%)
MTT="$REPO/target/release/mettatron"
CONFBIN="$REPO/target/release/mtt-conformance"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "a5_7_extras.XXXXXXXX")}"
echo "===== A5.7 EXTRAS (mmverify + PLN budgets + 20-run determinism, INDEX build) ====="; date
echo "repo=$REPO"
echo "pln=$PLN"
echo "conformance_dir=$CONF"
echo "logs=$LOG_DIR"

echo "### build INDEX mettatron + mtt-conformance (release)"
"${CAP[@]}" cargo build --release --bin mettatron --bin mtt-conformance > "$LOG_DIR/a57_build.log" 2>&1
echo "build_rc=$?"; tail -2 "$LOG_DIR/a57_build.log"

echo "### mmverify demo0 (expect 'Correct proof')"
"${CAP[@]}" "$MTT" examples/mmverify/demo0/verify_demo0.metta > "$LOG_DIR/a57_mmverify.log" 2>&1; echo "mmverify_rc=$?"
echo "  Correct-proof present: $(grep -ciE 'Correct proof' "$LOG_DIR/a57_mmverify.log")  (expect >=1)"
tail -3 "$LOG_DIR/a57_mmverify.log"

echo "### PLN Robot (budget <=12s wall; RSS <=8G enforced by the cap — OOM => rc!=0)"
s=$(date +%s); "${PLNRUN[@]}" "$MTT" "$PLN/examples/Robot.metta" > "$LOG_DIR/a57_robot.log" 2>&1; rc=$?; e=$(( $(date +%s) - s ))
echo "  robot_rc=$rc elapsed=${e}s (budget 12s); output tail:"; tail -2 "$LOG_DIR/a57_robot.log"

echo "### PLN FlyingRaven (budget <=25s wall)"
s=$(date +%s); "${PLNRUN[@]}" "$MTT" "$PLN/examples/FlyingRaven.metta" > "$LOG_DIR/a57_raven.log" 2>&1; rc=$?; e=$(( $(date +%s) - s ))
echo "  raven_rc=$rc elapsed=${e}s (budget 25s); output tail:"; tail -2 "$LOG_DIR/a57_raven.log"

echo "### 20-run index conformance RESULT determinism (default fanout — exercises parallel paths; expect 1 distinct hash)"
HASHES="$LOG_DIR/a57_hashes.txt"
: > "$HASHES"
for i in $(seq 1 20); do
  "${CAP[@]}" "$CONFBIN" --conformance-dir "$CONF" 2>/dev/null \
    | grep -E ': (PASS|FAIL|ERROR|SKIP|XFAIL|XPASS)' | sort | sha256sum | cut -d' ' -f1 >> "$HASHES"
done
echo "  runs recorded: $(wc -l < "$HASHES")/20; distinct result-hashes: $(sort -u "$HASHES" | wc -l) (expect 1)"
echo "===== A5.7 EXTRAS DONE ====="; date
