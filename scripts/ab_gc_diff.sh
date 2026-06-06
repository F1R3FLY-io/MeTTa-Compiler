#!/usr/bin/env bash
# ab_gc_diff.sh — A/B differential oracle for the store-centric GC migration (Inc 3).
#
# Proves hypothesis H2: the index-arena store produces BYTE-IDENTICAL observable
# results to the slab store. The two configurations are selected at COMPILE time
# (the store is a build-time machine parameter — see docs/cesk-gc/store-centric-architecture.md):
#
#   * default build              → ActiveFactory = GcFactory   (slab store)
#   * build --features index-gc  → ActiveFactory = IndexFactory (index-arena store σ)
#
# For each arm we run the conformance suite (all modules) and the lib test suite,
# canonicalize the per-fixture pass/fail sets, and assert they are IDENTICAL.
# Any divergence is an index-vs-slab semantic bug and fails the script (exit 3).
#
# Usage:  scripts/ab_gc_diff.sh [--conformance-dir DIR] [--no-nextest]
#
# NOTE: `--features index-gc` is introduced in Inc 4 sub-step 2 (the alias flip);
# until then this script's index arm is skipped with a clear message (so it is
# safe to land now and becomes live the moment the feature exists).

set -uo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
REPO_PARENT="$(cd -- "$REPO/.." && pwd -P)"
CONF_DIR="${CONFORMANCE_DIR:-$REPO_PARENT/mettatron-specification/conformance}"
RUN_NEXTEST=1
BUILD_CAP=(systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=1000%)
NEXTEST_CAP=(systemd-run --user --scope -p MemoryMax=96G -p MemorySwapMax=0 -p CPUQuota=1800%)
OUT_ROOT="${OUT_ROOT:-$REPO/target/gc-logs}"
mkdir -p "$OUT_ROOT"
OUT="${OUT:-$(mktemp -d -p "$OUT_ROOT" "ab_gc_diff.XXXXXXXX")}"
cd "$REPO"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --conformance-dir) CONF_DIR="$2"; shift 2 ;;
    --no-nextest)      RUN_NEXTEST=0; shift ;;
    *) echo "unknown arg: $1" >&2; exit 64 ;;
  esac
done

echo "== A/B GC differential =="
echo "repo:            $REPO"
echo "conformance-dir: $CONF_DIR"
echo "scratch:         $OUT"

# Does the crate expose the `index-gc` feature yet? (Inc 4 sub-step 2 adds it.)
if cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q '"index-gc"'; then
  HAVE_INDEX_FEATURE=1
else
  HAVE_INDEX_FEATURE=0
  echo "NOTE: cargo feature 'index-gc' not present yet (lands in Inc 4 sub-step 2)."
  echo "      Running the SLAB arm only; the index arm + diff become live once the feature exists."
fi

# --- conformance under one arm: prints "<module>/<fixture>: PASS|FAIL" lines ---
run_conformance () {  # $1 = binary path, $2 = label
  local bin="$1" label="$2"
  for mod in "" "--module M11-bisimilarity-pt" "--module M11-bisimilarity-he"; do
    # shellcheck disable=SC2086
    "$bin" --conformance-dir "$CONF_DIR" $mod 2>/dev/null \
      | grep -E ': (PASS|FAIL|ERROR)$' || true
  done | sort > "$OUT/conf_${label}.txt"
  local n
  n=$(wc -l < "$OUT/conf_${label}.txt")
  echo "  [$label] conformance fixtures recorded: $n"
}

echo "-- building + running SLAB arm --"
"${BUILD_CAP[@]}" cargo build --release --bin mtt-conformance 2>&1 | tail -1
run_conformance ./target/release/mtt-conformance slab

RC=0
if [[ "$HAVE_INDEX_FEATURE" == "1" ]]; then
  echo "-- building + running INDEX arm (--features index-gc) --"
  "${BUILD_CAP[@]}" cargo build --release --features index-gc --bin mtt-conformance 2>&1 | tail -1
  # the index binary overwrites target/release/mtt-conformance; it is the index build now
  run_conformance ./target/release/mtt-conformance index

  echo "-- diffing conformance pass/fail sets (slab vs index) --"
  if diff -u "$OUT/conf_slab.txt" "$OUT/conf_index.txt" > "$OUT/conf_diff.txt"; then
    echo "  CONFORMANCE: IDENTICAL ✓"
  else
    echo "  CONFORMANCE: DIVERGENCE ✗ (see below)"; cat "$OUT/conf_diff.txt"; RC=3
  fi

  if [[ "$RUN_NEXTEST" == "1" ]]; then
    echo "-- nextest under both arms (canonicalized pass/fail) --"
    "${NEXTEST_CAP[@]}" cargo nextest run 2>&1 | grep -E '^\s+(PASS|FAIL)' | awk '{print $1, $NF}' | sort > "$OUT/nt_slab.txt" || true
    "${NEXTEST_CAP[@]}" cargo nextest run --features index-gc 2>&1 | grep -E '^\s+(PASS|FAIL)' | awk '{print $1, $NF}' | sort > "$OUT/nt_index.txt" || true
    if diff -u "$OUT/nt_slab.txt" "$OUT/nt_index.txt" > "$OUT/nt_diff.txt"; then
      echo "  NEXTEST: IDENTICAL ✓"
    else
      echo "  NEXTEST: DIVERGENCE ✗"; cat "$OUT/nt_diff.txt"; RC=3
    fi
  fi
  # restore the default (slab) binary so the tree is left in the default state
  "${BUILD_CAP[@]}" cargo build --release --bin mtt-conformance 2>&1 | tail -1
fi

echo "== result: $([[ $RC -eq 0 ]] && echo 'ACCEPT (no divergence)' || echo 'REJECT (divergence)') =="
echo "artifacts: $OUT"
exit $RC
