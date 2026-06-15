#!/usr/bin/env bash
# Historical R-FL bite gate.
#
# Checks out the parent of the free-bit fix into a temporary sibling worktree,
# injects a unit test that asserts a dead current-segment slot is listed at most
# once across repeated young sweeps, and requires that test to fail with the
# expected duplicate-listing assertion. This is the source-level pre-fix bite:
# the duplicate detector was introduced with the fix, so the older commit cannot
# be exercised by the detector-backed post-fix harness without patching it first.
#
# Usage:
#   scripts/rfl_pre_fix_bite.sh
#
# Environment:
#   PRE_FIX_REF   commit/ref to test (default: add0585^)
#   REPO          checkout root (default: derived from this script)
#   LOG_DIR       log directory (default: mktemp)
#   CARGO_TARGET_DIR, MEMORY_MAX, CPU_QUOTA
set -uo pipefail

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PRE_FIX_REF="${PRE_FIX_REF:-add0585^}"
SAFE_REF="$(printf '%s' "$PRE_FIX_REF" | tr -c 'A-Za-z0-9_.-' '_')"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="${LOG_DIR:-$(mktemp -d -p "$LOG_ROOT" "rfl_pre_fix_bite.${SAFE_REF}.XXXXXXXX")}"
if [ -z "${WT:-}" ]; then
  WT="$(mktemp -d -p "$REPO_PARENT" "rfl-pre-fix-bite.${SAFE_REF}.XXXXXXXX")" \
    || fail "could not allocate temporary worktree path under: $REPO_PARENT"
  rmdir "$WT" || fail "could not clear temporary worktree path: $WT"
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target/rfl-pre-fix-bite}"
CAP=(
  systemd-run --user --scope
  -p MemoryMax="${MEMORY_MAX:-16G}"
  -p MemorySwapMax=0
  -p CPUQuota="${CPU_QUOTA:-300%}"
  --quiet
)

worktree_added=0

cleanup() {
  if [ "$worktree_added" -eq 1 ]; then
    git -C "$REPO" worktree remove --force "$WT" >/dev/null 2>&1 || true
  fi
  rmdir "$WT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$LOG_DIR" || fail "could not create log directory: $LOG_DIR"

echo "===== R-FL PRE-FIX BITE ====="
date
echo "repo=$REPO"
echo "pre_fix_ref=$PRE_FIX_REF"
echo "worktree=$WT"
echo "target=$TARGET_DIR"
echo "logs=$LOG_DIR"

git -C "$REPO" rev-parse --is-inside-work-tree >"$LOG_DIR/rev_parse.log" 2>&1 \
  || fail "REPO is not a git checkout: $REPO"

git -C "$REPO" rev-parse --verify "$PRE_FIX_REF^{commit}" >"$LOG_DIR/ref.log" 2>&1 \
  || fail "PRE_FIX_REF does not resolve to a commit: $PRE_FIX_REF"

git -C "$REPO" worktree add --detach "$WT" "$PRE_FIX_REF" >"$LOG_DIR/worktree_add.log" 2>&1 \
  || fail "could not create pre-fix worktree; see $LOG_DIR/worktree_add.log"
worktree_added=1

if ! git -C "$WT" apply >"$LOG_DIR/inject.patch.log" 2>&1 <<'PATCH'
diff --git a/src/backend/eval/cesk/index_arena.rs b/src/backend/eval/cesk/index_arena.rs
--- a/src/backend/eval/cesk/index_arena.rs
+++ b/src/backend/eval/cesk/index_arena.rs
@@ -1798,6 +1798,28 @@ mod tests {
         assert_eq!(
             arena.young_committed_node_bytes(),
             64 * std::mem::size_of::<u64>()
         );
     }
+
+    #[test]
+    fn pre_fix_current_segment_minor_repushes_listed_free_slot() {
+        let mut arena: IndexArena<u64> = IndexArena::with_segment_capacity(8);
+        let dead = arena.alloc(1);
+        let live = arena.alloc(2);
+
+        arena.mark(live);
+        let first = arena.sweep_young();
+        assert_eq!(first.reclaimed_to_free_list, 1);
+        assert_eq!(arena.free_list, vec![dead]);
+
+        arena.mark(live);
+        let _second = arena.sweep_young();
+        let occurrences = arena.free_list.iter().filter(|&&addr| addr == dead).count();
+        assert!(
+            occurrences <= 1,
+            "pre-fix bite: duplicate free-list entry for {:?}: {:?}",
+            dead,
+            arena.free_list
+        );
+    }
 }
PATCH
then
  fail "could not inject pre-fix bite test; see $LOG_DIR/inject.patch.log"
fi

log="$LOG_DIR/cargo_test.log"
env CARGO_TARGET_DIR="$TARGET_DIR" "${CAP[@]}" \
  cargo test -q --lib pre_fix_current_segment_minor_repushes_listed_free_slot \
    --manifest-path "$WT/Cargo.toml" \
    \
    -- --nocapture >"$log" 2>&1
rc=$?

if [ "$rc" -eq 0 ]; then
  echo "FAIL: pre-fix invariant test unexpectedly passed; log=$log" >&2
  tail -80 "$log" >&2
  exit 1
fi

if ! grep -q "pre-fix bite: duplicate free-list entry" "$log"; then
  echo "FAIL: pre-fix test failed for an unexpected reason; log=$log" >&2
  tail -120 "$log" >&2
  exit 1
fi

echo "PASS: pre-fix duplicate free-list behavior reproduced"
echo "log=$log"
