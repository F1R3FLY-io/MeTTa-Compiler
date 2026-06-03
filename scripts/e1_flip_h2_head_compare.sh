#!/usr/bin/env bash
# H2 experiment: does the COMMITTED baseline HEAD 8070c78 (the dormant Commit A,
# WITHOUT the uncommitted ①a/①c/② coordination edits + CEX-1) ALSO break at
# DEDICATED=1? If HEAD is clean/less-broken → my coordination edits REGRESSED it
# (gating OFF the legacy coop+cron collection left the dedicated thread as sole
# collector → drops roots / OOMs). If HEAD is equally broken → the bug pre-dates my
# edits (Commit A's rendezvous itself drops a live value).
#
# NON-DESTRUCTIVE: uses a git WORKTREE at 8070c78 (my working tree is untouched —
# this is NOT a stash/reset/checkout of my uncommitted edits). Worktree removed at end.
# Capped; correct metric (✅-present + no-❌; truncation/HANG/OOM = FAIL).
set -uo pipefail
REPO=/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
WT=/tmp/mtt-head-8070c78
RB=/home/dylon/Workspace/f1r3fly.io/PLN-main/examples/Robot.metta
echo "===== H2: HEAD 8070c78 DEDICATED=1 robot @ FANOUT=8 ====="; date

git -C "$REPO" worktree remove --force "$WT" 2>/dev/null || true
git -C "$REPO" worktree add --force --detach "$WT" 8070c78 2>&1 | tail -2 || { echo "worktree add FAILED"; exit 1; }

cd "$WT"
echo "--- build index-gc release at 8070c78 (capped 24G, separate target/) ---"
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=2400% --quiet \
  cargo build --release --features index-gc 2>&1 | tee /tmp/h2_build.log | grep -E "Compiling mettatron|^error|Finished" | tail -5
HEADBIN="$WT/target/release/mettatron"
if [ ! -x "$HEADBIN" ]; then echo "HEAD BUILD FAILED — aborting"; cd "$REPO"; git worktree remove --force "$WT" 2>/dev/null; exit 1; fi

echo "--- HEAD 8070c78 DEDICATED=1 (the FANOUT=8 broken config) ×4 ---"
for i in 1 2 3 4; do
  out=$(timeout 120 env METTATRON_PARALLEL_FANOUT_DEPTH=8 METTATRON_INDEX_GC_DEDICATED=1 METTATRON_INDEX_GC_MIN_BYTES=131072 \
    systemd-run --user --scope -p MemoryMax=18G -p MemorySwapMax=0 -p CPUQuota=2400% --quiet \
    "$HEADBIN" --gc index "$RB" 2>/dev/null); rc=$?
  if [ "$rc" -eq 124 ]; then echo "  HEAD run$i: HANG"; continue; fi
  if [ "$rc" -eq 137 ]; then echo "  HEAD run$i: OOM(>18G)"; continue; fi
  bad=$(printf '%s' "$out" | grep -c '❌'); ok=$(printf '%s' "$out" | grep -c '✅')
  ln=$(printf '%s' "$out" | grep -vE '^\[index_gc\]' | grep -c .)
  if [ "$ok" -ge 1 ] && [ "$bad" -eq 0 ]; then echo "  HEAD run$i: PASS (✅, lines=$ln)"; else echo "  HEAD run$i: FAIL (❌=$bad ✅=$ok lines=$ln)"; fi
done

echo "--- HEAD DEDICATED=0 sanity ×1 (expect PASS 404) ---"
out=$(timeout 120 env METTATRON_PARALLEL_FANOUT_DEPTH=8 METTATRON_INDEX_GC_DEDICATED=0 METTATRON_INDEX_GC_MIN_BYTES=131072 \
  systemd-run --user --scope -p MemoryMax=18G -p MemorySwapMax=0 -p CPUQuota=2400% --quiet \
  "$HEADBIN" --gc index "$RB" 2>/dev/null)
echo "  HEAD D0: ❌=$(printf '%s' "$out"|grep -c '❌') ✅=$(printf '%s' "$out"|grep -c '✅') lines=$(printf '%s' "$out"|grep -vE '^\[index_gc\]'|grep -c .)"

cd "$REPO"; git worktree remove --force "$WT" 2>&1 | tail -1
echo "===== H2 DONE — interpretation: HEAD-FAIL ⇒ bug pre-dates my edits; HEAD-PASS/cleaner ⇒ my ①a/①c regressed it ====="; date
