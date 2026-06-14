#!/usr/bin/env bash
# verify_cesk_gc_all.sh — THE WHOLE-SYSTEM VERIFICATION WALL (capstone, task #22).
#
# One entrypoint that runs EVERY standing gate for the CESK index GC + allocator,
# in dependency order, each stage hard-gating the next, ending in a single
# verdict table. This is the harness the composition theorem
# (formal/rocq/gc/CESKCollectorSafety.v, pgmcp #152) is paired with: the theorem
# composes the local obligations; this wall re-checks every obligation's
# mechanized artifact AND the runtime evidence on the live tree.
#
#   Usage: scripts/verify_cesk_gc_all.sh [label]
#
#   Stages (env SKIP_<NAME>=1 skips one on a re-run; the verdict marks it):
#     HYGIENE    proof-shortcut scan over every mandatory .v (+ Lean supplemental)
#     TLCHYGIENE every run_tlc entry well-formed; no orphan models/cfgs
#     COUPLING   source-coupling pins (the model↔source contract)
#     FORMAL     the full Rocq corpus + the TLC discriminator suite
#     GREENWALL  builds both stores + nextest both + conformance 483 release +
#                49/49 warnings + the DEBUG machine-equivalence oracle
#     E1ASAN     e1_flip_v4_asan: forced cycles under FANOUT=8, 3 arms, 0-UAF +
#                rendezvous non-vacuity
#     LOOM       the concurrency models (rendezvous/straddle/arena publish)
#     MMVERIFY   metamath demo0 "Correct proof!" on BOTH store binaries
#     DETERM     20-run Robot FANOUT=8 alpha-normalized sorted-content hash
#                with index-GC forced non-vacuous by DETERM_MIN_BYTES
#                (1 distinct, order-insensitive — the unsorted ORDER is
#                pre-existing nondeterministic at HEAD; generated freshening
#                epochs are alpha-renamed before hashing; semantic content must
#                be invariant)
#
# Resource discipline: every cargo/TLC/Rocq/runtime replay stage is internally
# capped by the stage scripts (systemd-run MemoryMax); the loom and replay
# stages are capped here. Run on a machine clear of sibling cargo builds
# (`ps`-check first) — a loaded box can flake the nextest work-pool spawn (see
# memory: once-flake-environmental).
set -uo pipefail
LABEL="${1:-allwall}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
PLN="${PLN:-$REPO_PARENT/PLN-main}"
cd "$REPO"
SAFE_LABEL="${LABEL//[^A-Za-z0-9_.-]/_}"
LOG_ROOT="${LOG_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="$(mktemp -d -p "$LOG_ROOT" "allwall_${SAFE_LABEL}.XXXXXXXX")"
RUN_MEM_MAX="${RUN_MEM_MAX:-24G}"
RUN_CPU_QUOTA="${RUN_CPU_QUOTA:-1600%}"
RUN_TIMEOUT_SECS="${RUN_TIMEOUT_SECS:-600}"
DETERM_MEM_MAX="${DETERM_MEM_MAX:-24G}"
DETERM_CPU_QUOTA="${DETERM_CPU_QUOTA:-800%}"
DETERM_TIMEOUT_SECS="${DETERM_TIMEOUT_SECS:-600}"
DETERM_MIN_BYTES="${DETERM_MIN_BYTES:-131072}"

declare -a STAGE_NAMES=() STAGE_RCS=() STAGE_NOTES=()
stage() {  # $1=NAME $2=human title; reads SKIP_<NAME>; runs "$@" from $3...
  local name="$1" title="$2"; shift 2
  local skip_var="SKIP_${name}"
  echo ""
  echo "━━━ ${name}: ${title} ━━━"; date
  if [[ "${!skip_var:-0}" == "1" ]]; then
    echo "    (skipped by ${skip_var}=1)"
    STAGE_NAMES+=("$name"); STAGE_RCS+=("SKIP"); STAGE_NOTES+=("skipped")
    return 0
  fi
  local log="$LOG_DIR/${name}.log"
  ( "$@" ) >"$log" 2>&1
  local rc=$?
  STAGE_NAMES+=("$name"); STAGE_RCS+=("$rc")
  if [[ $rc -ne 0 ]]; then
    STAGE_NOTES+=("FAILED — log: $log")
    echo "    rc=$rc  FAILED (tail below; full log: $log)"
    tail -25 "$log" | sed 's/^/    | /'
    return 1
  fi
  STAGE_NOTES+=("ok")
  echo "    rc=0  ok  (log: $log)"
  return 0
}

verdict() {
  echo ""
  echo "═══════════ WHOLE-SYSTEM VERIFICATION VERDICT [$LABEL] ═══════════"
  local failed=0
  for i in "${!STAGE_NAMES[@]}"; do
    printf "  %-11s %-5s %s\n" "${STAGE_NAMES[$i]}" "${STAGE_RCS[$i]}" "${STAGE_NOTES[$i]}"
    [[ "${STAGE_RCS[$i]}" != "0" && "${STAGE_RCS[$i]}" != "SKIP" ]] && failed=1
  done
  echo "  logs: $LOG_DIR"
  if [[ $failed -eq 0 ]]; then
    echo "  ✅ ALL GATES GREEN"
  else
    echo "  ❌ GATE FAILURE — the wall stops at the first red stage"
  fi
  date
  return $failed
}
trap verdict EXIT

# ── the loom stage body (the only one without its own script) ────────────────
run_loom() {
  systemd-run --user --scope -p MemoryMax=22G -p MemorySwapMax=0 -p CPUQuota=1600% --quiet \
    env RUSTFLAGS="--cfg loom -C target-cpu=native" LOOM_MAX_PREEMPTIONS=2 \
    cargo test --release --lib loom_ -- --nocapture
}

run_with_scope() {
  local mem="$1" cpu_quota="$2" timeout_secs="$3"
  shift 3
  systemd-run --user --scope \
    -p "MemoryMax=$mem" \
    -p MemorySwapMax=0 \
    -p "CPUQuota=$cpu_quota" \
    -p TasksMax=512 \
    --quiet \
    timeout --signal=TERM --kill-after=10s "$timeout_secs" "$@"
}

# ── mmverify on both store binaries (greenwall leaves the default-index release
#    built; the legacy-slab arm rebuilds quickly thanks to the cache) ─────────
run_mmverify_both() {
  set -e
  local out_index out_slab
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600% --quiet \
    cargo build --release --bin mettatron
  cp target/release/mettatron "$LOG_DIR/mtt-index"
  out_index="$(run_with_scope "$RUN_MEM_MAX" "$RUN_CPU_QUOTA" "$RUN_TIMEOUT_SECS" \
    "$LOG_DIR/mtt-index" examples/mmverify/demo0/verify_demo0.metta 2>&1)"
  out_index="$(printf '%s\n' "$out_index" | grep -c 'Correct proof!' || true)"
  [[ "$out_index" -ge 1 ]] || { echo "index mmverify did not print 'Correct proof!'"; exit 1; }
  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1600% --quiet \
    cargo build --release --no-default-features --features legacy-slab-gc --bin mettatron
  cp target/release/mettatron "$LOG_DIR/mtt-slab"
  out_slab="$(run_with_scope "$RUN_MEM_MAX" "$RUN_CPU_QUOTA" "$RUN_TIMEOUT_SECS" \
    "$LOG_DIR/mtt-slab" examples/mmverify/demo0/verify_demo0.metta 2>&1)"
  out_slab="$(printf '%s\n' "$out_slab" | grep -c 'Correct proof!' || true)"
  [[ "$out_slab" -ge 1 ]] || { echo "slab mmverify did not print 'Correct proof!'"; exit 1; }
  echo "mmverify Correct on both arms"
}

# ── 20-run alpha-normalized sorted-content determinism (Robot, FANOUT=8, the
#    INDEX binary). `$__fr_<epoch>_` names are per-invocation alpha-renaming
#    artifacts minted by the parallel rule matcher; the epoch number is not
#    semantic content, and it can vary with harmless worker interleavings.
normalize_determinism_output() {
  sed -E 's/\$__fr_[0-9]+_/\$__fr_E_/g'
}

run_determinism() {
  set -e
  [[ -x "$LOG_DIR/mtt-index" ]] || { echo "index binary missing (MMVERIFY stage builds it)"; exit 1; }
  echo "determinism cap: MemoryMax=$DETERM_MEM_MAX MemorySwapMax=0 CPUQuota=$DETERM_CPU_QUOTA timeout=${DETERM_TIMEOUT_SECS}s MIN_BYTES=$DETERM_MIN_BYTES"
  local hashes
  hashes="$(for i in $(seq 1 20); do
    run_with_scope "$DETERM_MEM_MAX" "$DETERM_CPU_QUOTA" "$DETERM_TIMEOUT_SECS" \
      env METTATRON_PARALLEL_FANOUT_DEPTH=8 METTATRON_INDEX_GC_MIN_BYTES="$DETERM_MIN_BYTES" \
      "$LOG_DIR/mtt-index" "$PLN/examples/Robot.metta" 2>/dev/null \
      | normalize_determinism_output | sort | sha256sum | cut -d' ' -f1
  done | sort -u | wc -l)"
  echo "distinct alpha-normalized sorted-content hashes: $hashes (expect 1)"
  [[ "$hashes" == "1" ]]
}

echo "═══════════ WHOLE-SYSTEM VERIFICATION WALL [$LABEL] ═══════════"
echo "repo=$REPO  logs=$LOG_DIR"
free -h | head -2
echo "sibling cargo/rustc: $(ps aux | grep -E 'cargo|rustc' | grep -v grep | wc -l) (should be 0)"

stage HYGIENE    "proof-shortcut hygiene"          bash scripts/verify_cesk_gc_proof_hygiene.sh   || exit 1
stage TLCHYGIENE "TLC harness hygiene"             bash scripts/verify_cesk_gc_tlc_hygiene.sh     || exit 1
stage COUPLING   "source-coupling pins"            bash scripts/verify_cesk_gc_source_coupling.sh || exit 1
stage FORMAL     "Rocq corpus + TLC discriminators" bash scripts/verify_cesk_gc_formal.sh         || exit 1
stage GREENWALL  "builds + nextest + conformance + oracle" bash scripts/a5_greenwall.sh "allwall-${SAFE_LABEL}" --with-oracle || exit 1
stage E1ASAN     "forced-cycle ASAN (FANOUT=8, 3 arms)"    bash scripts/e1_flip_v4_asan.sh        || exit 1
stage LOOM       "loom concurrency models"         run_loom                                       || exit 1
stage MMVERIFY   "mmverify Correct (both stores)"  run_mmverify_both                              || exit 1
stage DETERM     "20-run alpha-normalized sorted-content determinism" run_determinism             || exit 1
# the trap prints the verdict; its rc is the script's rc
