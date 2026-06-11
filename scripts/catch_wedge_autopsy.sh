#!/usr/bin/env bash
# Bug #309 — autonomous wedge catcher + gdb autopsy (no sudo needed).
#
# Spawns the (symbolized) binary DIRECTLY as this shell's child — yama
# ptrace_scope=1 permits tracing DESCENDANTS, so on a stall we can gdb -p the
# wedged child in place and capture full backtraces WITH frame arguments,
# then chase each pump_parallel_collapse_wait frame's handle state
# (remaining / done / cancel) — the decisive #309 discriminator.
#
# NO /usr/bin/time wrapper (the resilient-bench farm's wrapper ate the USR1 and
# orphaned its children — that is how the original specimen was born); wall time
# comes from bash SECONDS.
#
#   Usage: scripts/catch_wedge_autopsy.sh <binary-with-symbols> <fixture> <out-dir>
#   Env:   MAX_REPS (200), STALL_S (60), LOAD (1 → run a background CPU-load
#          generator on the same cores to recreate the contention that raises
#          the wedge rate to ~5%), AFFINITY (8-15)
set -uo pipefail
BIN="${1:?usage: catch_wedge_autopsy.sh <bin> <fixture> <out>}"
FX="${2:?fixture}"
OUT="${3:?out-dir}"
MAX_REPS="${MAX_REPS:-200}"
STALL_S="${STALL_S:-60}"
LOAD="${LOAD:-1}"
AFFINITY="${AFFINITY:-8-15}"
mkdir -p "$OUT"

# Optional contention generator: spin half the pinned cores at low priority —
# the farm's accidental discovery was that co-load amplifies the wedge race.
LOADPIDS=()
if [[ "$LOAD" == "1" ]]; then
  for c in 8 10 12 14; do
    nice -n 19 taskset -c "$c" bash -c 'while :; do :; done' &
    LOADPIDS+=($!)
  done
  echo "load generators: ${LOADPIDS[*]} (cores 8/10/12/14, nice 19)"
fi
cleanup() { for p in "${LOADPIDS[@]:-}"; do kill -9 "$p" 2>/dev/null; done; }
trap cleanup EXIT

for rep in $(seq 1 "$MAX_REPS"); do
  err="$OUT/rep_${rep}.err"
  SECONDS=0
  taskset -c "$AFFINITY" "$BIN" "$FX" > /dev/null 2> "$err" &
  pid=$!
  hung=0
  while kill -0 "$pid" 2>/dev/null; do
    sleep 1
    if [[ "$SECONDS" -ge "$STALL_S" ]]; then hung=1; break; fi
  done
  if [[ "$hung" -eq 0 ]]; then
    wait "$pid" 2>/dev/null
    rm -f "$err"
    echo "rep $rep ok (${SECONDS}s)"
    continue
  fi

  echo "rep $rep WEDGED at ${SECONDS}s — autopsy of pid $pid"
  # 1) the in-band diagnostics dump (the binary's SIGUSR1 → diag-watcher, 50ms poll)
  kill -USR1 "$pid" 2>/dev/null; sleep 2
  # 2) the gdb autopsy: full backtraces + every pump frame's handle state.
  #    The child is OUR descendant ⇒ ptrace allowed at yama scope 1.
  gdb -p "$pid" -batch \
    -ex 'set pagination off' \
    -ex 'echo \n===== THREADS =====\n' -ex 'info threads' \
    -ex 'echo \n===== BT FULL =====\n' -ex 'thread apply all bt full 25' \
    > "$OUT/rep_${rep}_autopsy.txt" 2>&1
  # 3) chase handle state from every pump frame (a second focused pass)
  gdb -p "$pid" -batch \
    -ex 'set pagination off' \
    -ex 'echo \n===== PUMP HANDLES =====\n' \
    -ex 'thread apply all frame apply all -q info frame' \
    > "$OUT/rep_${rep}_frames.txt" 2>&1
  kill -9 "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  echo "WEDGE CAPTURED: $OUT/rep_${rep}_autopsy.txt (+ .err dump $(stat -c%s "$err" 2>/dev/null) bytes)"
  exit 0
done
echo "no wedge in $MAX_REPS reps"
exit 1
