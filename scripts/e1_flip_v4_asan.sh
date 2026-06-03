#!/usr/bin/env bash
# E1-FLIP V4 — the load-bearing UAF gate for activating concurrent collection.
#
# After E1-FLIP Commit A, the dedicated-GC-thread rendezvous SWEEPS under FANOUT>0
# (gate_open_rendezvous + the "rendezvous" phase + the Part-3 panic finish-bump). This
# is the FIRST time the sweep runs while eval workers are live, so it is where the whole
# E1-c root-completeness machinery is finally exercised end-to-end.
#
# This gate forces the dedicated collector ON via the ENV OVERRIDE
# (METTATRON_INDEX_GC_DEDICATED=1) against the committed Commit-A code — so it validates
# the activation WITHOUT needing the default-flip edit (Commit B's one line). Only flip
# the default + commit B once this is green.
#
# Per arm asserts:
#   (a) 0 ASAN UAF/poison (root-set completeness under live parallelism);
#   (b) NON-VACUOUS: '[index_gc] rendezvous ... cycle' > 0 (the rendezvous collector
#       actually fired + swept while workers were alive) AND non-rendezvous index_gc
#       cycles == 0 (the ST collectors are gated off by worker_ever_spawned);
#   (c) correct result (no Error / StackOverflow).
#
# CAPPED build 24G / run 20G, MemorySwapMax=0, -j4, tee'd. Check `free -h` first (a
# sibling build may be live in /var/tmp/wt-fork-fix → drop to -j3). NEVER background an
# UNCAPPED ASAN build (a prior uncapped backgrounded ASAN+stress run OOM-crashed the box).
set -uo pipefail
REPO=/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
PLN=/home/dylon/Workspace/f1r3fly.io/PLN-main
cd "$REPO"
BIN="$REPO/target/x86_64-unknown-linux-gnu/release/mettatron"
P=/tmp/e1_flip_v4
MIN_BYTES=131072   # 128 KiB major floor — forces the rendezvous collector to fire

echo "===== E1-FLIP V4 ASAN (FANOUT>0 + DEDICATED=1, env-forced) ====="; date; free -h | head -2

echo "### build mettatron index-gc ASAN (release, -Zbuild-std, -j4, capped 24G)"
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1000% -p TasksMax=512 --quiet \
  env RUSTFLAGS="-Zsanitizer=address -Cdebuginfo=2 -Ctarget-cpu=native" \
  cargo +nightly build --release -Zbuild-std --target x86_64-unknown-linux-gnu \
    --bin mettatron -j4 --features index-gc 2>&1 | tee "${P}_build.log"
BUILD_RC=${PIPESTATUS[0]}
echo "build_rc=$BUILD_RC"
if [ "$BUILD_RC" -ne 0 ] || [ ! -x "$BIN" ]; then
  echo "BUILD FAILED — aborting V4 (binary not produced)"; exit 1
fi

run_arm() {  # $1=label $2=fixture $3=fanout
  local label="$1" fixture="$2" fanout="$3"
  echo "### ASAN arm: $label (FANOUT=$fanout, DEDICATED=1, MIN_BYTES=$MIN_BYTES)"
  env METTATRON_PARALLEL_FANOUT_DEPTH="$fanout" METTATRON_INDEX_GC_DEDICATED=1 \
      METTATRON_INDEX_GC_MIN_BYTES="$MIN_BYTES" METTATRON_INDEX_GC_REPORT=2 \
      ASAN_OPTIONS=detect_leaks=0:abort_on_error=1:halt_on_error=1 \
    systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000% --quiet \
    "$BIN" --gc index "$fixture" > "${P}_${label}.log" 2>&1
  local rc=$?
  echo "  ${label}_rc=$rc"
  echo "  (a) UAF/ASAN:            $(grep -cE 'AddressSanitizer|heap-use-after-free|use-after-poison|heap-buffer-overflow' "${P}_${label}.log")  (expect 0)"
  echo "  (b) rendezvous cycles:   $(grep -cE '^\[index_gc\] rendezvous .* cycle' "${P}_${label}.log")  (expect >0)"
  echo "      NON-rendezvous cyc:  $(grep -E '^\[index_gc\] .* cycle' "${P}_${label}.log" | grep -cvE 'rendezvous')  (expect 0)"
  echo "  (c) Error/StackOverflow: $(grep -cE 'Error|StackOverflow' "${P}_${label}.log")  (expect 0)"
}

run_arm robot_f8  "$PLN/examples/Robot.metta"                    8
run_arm raven_f8  "$PLN/examples/FlyingRaven.metta"              8
run_arm stress_f8 "$REPO/examples/cesk-gc/stress_multidir.metta" 8

echo "===== E1-FLIP V4 ASAN DONE ====="; date
