#!/usr/bin/env bash
# A5.7 final-gate EXTRAS (beyond the per-step green-wall + ASAN): the parts of the
# master-plan per-rung gate not in a5_greenwall.sh —
#   mmverify "Correct proof" + PLN budgets (Robot ≤12s, FlyingRaven ≤25s, RSS ≤8G via
#   the 8G cap) + 20-run conformance result-determinism — all on the INDEX build.
set -uo pipefail
REPO=/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
PLN=/home/dylon/Workspace/f1r3fly.io/PLN-main
CONF="${CONFORMANCE_DIR:-/home/dylon/Workspace/f1r3fly.io/mettatron-specification/conformance}"
cd "$REPO"
CAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600%)
PLNRUN=(systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400%)
MTT="$REPO/target/release/mettatron"
CONFBIN="$REPO/target/release/mtt-conformance"
echo "===== A5.7 EXTRAS (mmverify + PLN budgets + 20-run determinism, INDEX build) ====="; date

echo "### build INDEX mettatron + mtt-conformance (release)"
"${CAP[@]}" cargo build --release --features index-gc --bin mettatron --bin mtt-conformance > /tmp/a57_build.log 2>&1
echo "build_rc=$?"; tail -2 /tmp/a57_build.log

echo "### mmverify demo0 (expect 'Correct proof')"
"${CAP[@]}" "$MTT" examples/mmverify/demo0/verify_demo0.metta > /tmp/a57_mmverify.log 2>&1; echo "mmverify_rc=$?"
echo "  Correct-proof present: $(grep -ciE 'Correct proof' /tmp/a57_mmverify.log)  (expect >=1)"
tail -3 /tmp/a57_mmverify.log

echo "### PLN Robot (budget <=12s wall; RSS <=8G enforced by the cap — OOM => rc!=0)"
s=$(date +%s); "${PLNRUN[@]}" "$MTT" "$PLN/examples/Robot.metta" > /tmp/a57_robot.log 2>&1; rc=$?; e=$(( $(date +%s) - s ))
echo "  robot_rc=$rc elapsed=${e}s (budget 12s); output tail:"; tail -2 /tmp/a57_robot.log

echo "### PLN FlyingRaven (budget <=25s wall)"
s=$(date +%s); "${PLNRUN[@]}" "$MTT" "$PLN/examples/FlyingRaven.metta" > /tmp/a57_raven.log 2>&1; rc=$?; e=$(( $(date +%s) - s ))
echo "  raven_rc=$rc elapsed=${e}s (budget 25s); output tail:"; tail -2 /tmp/a57_raven.log

echo "### 20-run index conformance RESULT determinism (default fanout — exercises parallel paths; expect 1 distinct hash)"
: > /tmp/a57_hashes.txt
for i in $(seq 1 20); do
  "${CAP[@]}" "$CONFBIN" --conformance-dir "$CONF" 2>/dev/null \
    | grep -E ': (PASS|FAIL|ERROR|SKIP|XFAIL|XPASS)' | sort | sha256sum | cut -d' ' -f1 >> /tmp/a57_hashes.txt
done
echo "  runs recorded: $(wc -l < /tmp/a57_hashes.txt)/20; distinct result-hashes: $(sort -u /tmp/a57_hashes.txt | wc -l) (expect 1)"
echo "===== A5.7 EXTRAS DONE ====="; date
