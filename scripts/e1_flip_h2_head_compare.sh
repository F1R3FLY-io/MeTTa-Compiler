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
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
TARGET_REF="${TARGET_REF:-8070c78}"
PLN_DIR="${PLN_DIR:-$REPO_PARENT/PLN-main}"
RB="${ROBOT:-$PLN_DIR/examples/Robot.metta}"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t "e1_h2.XXXXXXXX")}"
if [ -z "${WT:-}" ]; then
  WT="$(mktemp -d -p "$REPO_PARENT" "mtt-head-${TARGET_REF}.XXXXXXXX")"
  rmdir "$WT"
fi
WORKTREE_ADDED=0
cleanup() {
  if [ "$WORKTREE_ADDED" -eq 1 ]; then
    git -C "$REPO" worktree remove --force "$WT" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "===== H2: HEAD $TARGET_REF DEDICATED=1 robot @ FANOUT=8 ====="; date
echo "repo=$REPO"
echo "worktree=$WT"
echo "robot=$RB"
echo "logs=$LOG_DIR"

if [ -e "$WT" ]; then
  echo "worktree path already exists: $WT" >&2
  exit 1
fi
git -C "$REPO" worktree add --force --detach "$WT" "$TARGET_REF" 2>&1 | tail -2 || { echo "worktree add FAILED"; exit 1; }
WORKTREE_ADDED=1

cd "$WT"
echo "--- build index-gc release at 8070c78 (capped 24G, separate target/) ---"
BUILD_LOG="$LOG_DIR/h2_build.log"
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=2400% --quiet \
  cargo build --release --features index-gc > "$BUILD_LOG" 2>&1
echo "build_rc=$?"
grep -E "Compiling mettatron|^error|Finished" "$BUILD_LOG" | tail -5
HEADBIN="$WT/target/release/mettatron"
if [ ! -x "$HEADBIN" ]; then echo "HEAD BUILD FAILED — aborting"; exit 1; fi

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

cd "$REPO"
git -C "$REPO" worktree remove --force "$WT" 2>&1 | tail -1
WORKTREE_ADDED=0
echo "===== H2 DONE — interpretation: HEAD-FAIL ⇒ bug pre-dates my edits; HEAD-PASS/cleaner ⇒ my ①a/①c regressed it ====="; date
