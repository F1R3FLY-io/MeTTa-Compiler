#!/usr/bin/env bash
# D-RLOCK.2 determinism gate. Runs a fixture N times and hashes ONLY stdout (the
# RESULT output) — the `[index_gc]` diagnostics + cycle counters go to stderr, so
# excluding stderr automatically drops them. Reports the count of UNIQUE hashes
# (expect 1). Capped 20G, FOREGROUND.
#
#   drlock_determinism.sh <label> <fanout> <runs> <fixture-relpath> [EXTRA_ENV...]
set -uo pipefail
LABEL="${1:?label}"; FANOUT="${2:?fanout}"; RUNS="${3:?runs}"; FIX="${4:?fixture}"; shift 4
REPO=/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler
BIN="$REPO/target/release/mettatron"
GIB=$((1024*1024*1024))
CAP=(systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p CPUQuota=1000% --quiet)
OUT=/tmp/drlock_det_${LABEL}
: > "${OUT}_hashes.txt"
echo "### determinism [$LABEL] fanout=$FANOUT runs=$RUNS fixture=$FIX extra=$*"; date
for i in $(seq 1 "$RUNS"); do
  env METTATRON_PARALLEL_FANOUT_DEPTH="$FANOUT" "$@" \
    "${CAP[@]}" "$BIN" "$REPO/$FIX" 2>/dev/null | sha256sum | awk '{print $1}' >> "${OUT}_hashes.txt"
done
echo "unique result hashes (expect 1):"
sort "${OUT}_hashes.txt" | uniq -c
echo "distinct count: $(sort -u "${OUT}_hashes.txt" | wc -l)"
echo "### DONE [$LABEL]"; date
