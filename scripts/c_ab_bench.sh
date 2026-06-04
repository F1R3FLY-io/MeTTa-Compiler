#!/usr/bin/env bash
# Phase C — git-version A/B benchmark (replaces the stale env-switch c1b_fanout0_bench.sh,
# which set the now-DELETED METTATRON_INDEX_GC_YOUNG_MIN_BYTES). The C1.c reworks deleted
# the on/off switch, so A/B must compare two BUILDS, not one binary under two envs.
#
#   Usage: scripts/c_ab_bench.sh <A_REF> <B_REF> [reps]
#     e.g. scripts/c_ab_bench.sh d7dbbac HEAD 3      # Increment A: side-free off vs on
#          scripts/c_ab_bench.sh b8d96f1 HEAD 3      # cumulative C: foundation vs HEAD
#
# Builds release index-gc `mettatron` at A_REF and B_REF in git worktrees (the working
# tree is untouched), sharing one CARGO_TARGET_DIR so deps build once. Runs each binary
# on the side-free RSS exerciser + PLN Robot at FANOUT=0 with the collector ON (minors
# firing: MIN_BYTES high so committed never preempts), ×reps, capturing wall (s) and peak
# RSS (/usr/bin/time -v Maximum resident set size) + the REPORT=2 minor/major split.
#
# The side-free's win is PEAK RSS (payloads freed), NOT committed (the spine never shrinks
# under no-recycle) — so RSS is the headline metric.
set -uo pipefail
A_REF="${1:?usage: c_ab_bench.sh <A_REF> <B_REF> [reps]}"
B_REF="${2:?need B_REF}"
REPS="${3:-3}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
ROBOT="${ROBOT:-$PLN/examples/Robot.metta}"
cd "$REPO"
GIB=$((1024*1024*1024))
LOG_DIR="${LOG_DIR:-$(mktemp -d -t "cab_bench.XXXXXXXX")}"
BIN_DIR="${BIN_DIR:-$(mktemp -d -t "cab_bins.XXXXXXXX")}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(mktemp -d -t "cab_target.XXXXXXXX")}"
BUILDCAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600%)
RUNCAP=(systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400%)

safe_name() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9_.-' '_'
}

make_worktree_path() {
  local safe="$1" wt
  wt="$(mktemp -d -p "$REPO_PARENT" "wt-cab-${safe}.XXXXXXXX")"
  rmdir "$wt"
  printf '%s\n' "$wt"
}

WORKLOADS=("$REPO/examples/cesk-gc/side_free_minor.metta")
[ -f "$ROBOT" ] && WORKLOADS+=("$ROBOT")

SAFE_A="$(safe_name "$A_REF")"
SAFE_B="$(safe_name "$B_REF")"
WT_A="${WT_A:-$(make_worktree_path "$SAFE_A")}"
WT_B="${WT_B:-$(make_worktree_path "$SAFE_B")}"
A_BIN="$BIN_DIR/cab_${SAFE_A}_mettatron"
B_BIN="$BIN_DIR/cab_${SAFE_B}_mettatron"
WT_A_ADDED=0
WT_B_ADDED=0
cleanup() {
  if [ "$WT_A_ADDED" -eq 1 ]; then git -C "$REPO" worktree remove --force "$WT_A" 2>/dev/null || true; fi
  if [ "$WT_B_ADDED" -eq 1 ]; then git -C "$REPO" worktree remove --force "$WT_B" 2>/dev/null || true; fi
}
trap cleanup EXIT

build_ref() {  # $1=ref $2=worktree $3=output-bin $4=log
  local ref="$1" wt="$2" out="$3" log="$4"
  # The worktree MUST be a sibling of the repo: Cargo.toml has relative path deps
  # (../MORK/kernel, ../f1r3node-rust/models, ../PathMap, …) that only resolve from the
  # real parent dir; arbitrary temp worktrees can break those sibling lookups.
  if [ -e "$wt" ]; then
    echo "worktree path already exists: $wt" >&2
    return 1
  fi
  git -C "$REPO" worktree add --detach "$wt" "$ref" >"$log" 2>&1
  # +nightly: the repo's nightly toolchain is a rustup DIRECTORY OVERRIDE (not a tracked
  # rust-toolchain file), so a fresh worktree defaults to stable and fails E0554 on
  # fast-slice-utils' #![feature]. .cargo/config.toml (target-cpu=native) IS tracked, so
  # it ships with the worktree.
  ( cd "$wt" && "${BUILDCAP[@]}" cargo +nightly build --release --features index-gc --bin mettatron ) \
    >>"$log" 2>&1
  local build_rc=$?
  if [ "$build_rc" -ne 0 ]; then
    return "$build_rc"
  fi
  cp "$CARGO_TARGET_DIR/release/mettatron" "$out"
}

bench() {  # $1=label $2=bin
  local label="$1" bin="$2"
  local safe_label; safe_label="$(safe_name "$label")"
  for wl in "${WORKLOADS[@]}"; do
    local name; name=$(basename "$wl" .metta)
    for r in $(seq 1 "$REPS"); do
      local out_log="$LOG_DIR/cab_${safe_label}_${name}_${r}.out"
      local err_log="$LOG_DIR/cab_${safe_label}_${name}_${r}.err"
      # /usr/bin/time -v must run INSIDE the systemd scope so it measures the BINARY's
      # RSS (its child), not the systemd-run wrapper's. env sets the GC knobs in the scope.
      "${RUNCAP[@]}" env METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=$GIB \
        METTATRON_INDEX_GC_REPORT=2 /usr/bin/time -v "$bin" "$wl" \
        >"$out_log" 2>"$err_log"
      local rss wall minors majors
      rss=$(grep -oE 'Maximum resident set size \(kbytes\): [0-9]+' "$err_log" | grep -oE '[0-9]+$')
      wall=$(grep -oE 'Elapsed \(wall clock\) time.*' "$err_log" | sed 's/.*: //')
      minors=$(grep -h -cE 'minor cycle' "$err_log" "$out_log" 2>/dev/null | paste -sd+ | bc 2>/dev/null || echo 0)
      majors=$(grep -h -cE 'major cycle' "$err_log" "$out_log" 2>/dev/null | paste -sd+ | bc 2>/dev/null || echo 0)
      printf '%-8s %-20s rep%s  RSS=%9s KB  wall=%-12s minor/major=%s/%s\n' \
        "$label" "$name" "$r" "${rss:-?}" "${wall:-?}" "${minors:-?}" "${majors:-?}"
    done
  done
}

echo "===== C A/B BENCH  A=$A_REF  B=$B_REF  reps=$REPS ====="; date
echo "repo=$REPO"
echo "pln=$PLN"
echo "logs=$LOG_DIR"
echo "bins=$BIN_DIR"
echo "target=$CARGO_TARGET_DIR"
echo "### build A ($A_REF)"
WT_A_ADDED=1
if ! build_ref "$A_REF" "$WT_A" "$A_BIN" "$LOG_DIR/cab_build_${SAFE_A}.log"; then
  echo "A build failed; see $LOG_DIR/cab_build_${SAFE_A}.log" >&2
  exit 1
fi
echo "A_bin=$A_BIN ($(tail -1 "$LOG_DIR/cab_build_${SAFE_A}.log"))"
echo "### build B ($B_REF)"
WT_B_ADDED=1
if ! build_ref "$B_REF" "$WT_B" "$B_BIN" "$LOG_DIR/cab_build_${SAFE_B}.log"; then
  echo "B build failed; see $LOG_DIR/cab_build_${SAFE_B}.log" >&2
  exit 1
fi
echo "B_bin=$B_BIN ($(tail -1 "$LOG_DIR/cab_build_${SAFE_B}.log"))"
echo "### bench A"; bench "A($A_REF)" "$A_BIN"
echo "### bench B"; bench "B($B_REF)" "$B_BIN"
echo "===== cleanup worktrees ====="
git -C "$REPO" worktree remove --force "$WT_A" 2>/dev/null || true; WT_A_ADDED=0
git -C "$REPO" worktree remove --force "$WT_B" 2>/dev/null || true; WT_B_ADDED=0
echo "===== C A/B BENCH DONE (RSS is the side-free headline metric) ====="; date
