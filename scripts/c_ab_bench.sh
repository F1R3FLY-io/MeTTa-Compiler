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
REPO=/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
ROBOT=/home/dylon/Workspace/f1r3fly.io/PLN-main/examples/Robot.metta
cd "$REPO"
GIB=$((1024*1024*1024))
export CARGO_TARGET_DIR=/tmp/cab-target
BUILDCAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600%)
RUNCAP=(systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=400%)

WORKLOADS=("$REPO/examples/cesk-gc/side_free_minor.metta")
[ -f "$ROBOT" ] && WORKLOADS+=("$ROBOT")

build_ref() {  # $1=ref -> echoes saved binary path
  # The worktree MUST be a sibling of MeTTa-Compiler: Cargo.toml has relative path deps
  # (../MORK/kernel, ../f1r3node-rust/models, ../PathMap, …) that only resolve from the
  # real parent dir — a /tmp worktree resolves ../MORK to /tmp/MORK (nonexistent).
  local ref="$1" wt="/home/dylon/Workspace/f1r3fly.io/wt-cab-$1" out="/tmp/cab_${1}_mettatron"
  git worktree add --detach "$wt" "$ref" >/tmp/cab_wt_$1.log 2>&1 || true
  # +nightly: the repo's nightly toolchain is a rustup DIRECTORY OVERRIDE (not a tracked
  # rust-toolchain file), so a fresh worktree defaults to stable and fails E0554 on
  # fast-slice-utils' #![feature]. .cargo/config.toml (target-cpu=native) IS tracked, so
  # it ships with the worktree.
  ( cd "$wt" && "${BUILDCAP[@]}" cargo +nightly build --release --features index-gc --bin mettatron ) \
    >/tmp/cab_build_$1.log 2>&1
  cp "$CARGO_TARGET_DIR/release/mettatron" "$out"
  echo "$out"
}

bench() {  # $1=label $2=bin
  local label="$1" bin="$2"
  for wl in "${WORKLOADS[@]}"; do
    local name; name=$(basename "$wl" .metta)
    for r in $(seq 1 "$REPS"); do
      # /usr/bin/time -v must run INSIDE the systemd scope so it measures the BINARY's
      # RSS (its child), not the systemd-run wrapper's. env sets the GC knobs in the scope.
      "${RUNCAP[@]}" env METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=$GIB \
        METTATRON_INDEX_GC_REPORT=2 /usr/bin/time -v "$bin" "$wl" \
        >"/tmp/cab_${label}_${name}_${r}.out" 2>"/tmp/cab_${label}_${name}_${r}.err"
      local rss wall minors majors
      rss=$(grep -oE 'Maximum resident set size \(kbytes\): [0-9]+' "/tmp/cab_${label}_${name}_${r}.err" | grep -oE '[0-9]+$')
      wall=$(grep -oE 'Elapsed \(wall clock\) time.*' "/tmp/cab_${label}_${name}_${r}.err" | sed 's/.*: //')
      minors=$(grep -cE 'minor cycle' "/tmp/cab_${label}_${name}_${r}.err" "/tmp/cab_${label}_${name}_${r}.out" 2>/dev/null | paste -sd+ | bc 2>/dev/null || echo 0)
      majors=$(grep -cE 'major cycle' "/tmp/cab_${label}_${name}_${r}.err" "/tmp/cab_${label}_${name}_${r}.out" 2>/dev/null | paste -sd+ | bc 2>/dev/null || echo 0)
      printf '%-8s %-20s rep%s  RSS=%9s KB  wall=%-12s minor/major=%s/%s\n' \
        "$label" "$name" "$r" "${rss:-?}" "${wall:-?}" "${minors:-?}" "${majors:-?}"
    done
  done
}

echo "===== C A/B BENCH  A=$A_REF  B=$B_REF  reps=$REPS ====="; date
echo "### build A ($A_REF)"; A_BIN=$(build_ref "$A_REF"); echo "A_bin=$A_BIN ($(tail -1 /tmp/cab_build_$A_REF.log))"
echo "### build B ($B_REF)"; B_BIN=$(build_ref "$B_REF"); echo "B_bin=$B_BIN ($(tail -1 /tmp/cab_build_$B_REF.log))"
echo "### bench A"; bench "A($A_REF)" "$A_BIN"
echo "### bench B"; bench "B($B_REF)" "$B_BIN"
echo "===== cleanup worktrees ====="
git worktree remove --force "/home/dylon/Workspace/f1r3fly.io/wt-cab-$A_REF" 2>/dev/null || true
git worktree remove --force "/home/dylon/Workspace/f1r3fly.io/wt-cab-$B_REF" 2>/dev/null || true
echo "===== C A/B BENCH DONE (RSS is the side-free headline metric) ====="; date
