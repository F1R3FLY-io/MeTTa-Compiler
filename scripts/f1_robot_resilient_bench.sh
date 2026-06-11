#!/usr/bin/env bash
# F1 SATB-young lever — hang-resilient Robot A/B (experiment #12 secondaries).
#
# Bug #309: HEAD's default-env Robot intermittently wedges (all threads
# futex-parked). hyperfine cannot survive that, so this loop measures each rep
# with a stall watchdog: stalled (>STALL_S with ~0 CPU growth) ⇒ SIGUSR1 (the
# diagnostics dump lands in the rep's captured stderr — the dump farm), then
# SIGKILL, counted as a hang and excluded from timing (retried up to RETRIES).
# Arms are interleaved per rep to control drift; per-rep wall comes from
# /usr/bin/time -f '%e' -o (kept separate from the program's stderr).
#
#   Usage: scripts/f1_robot_resilient_bench.sh <control-bin> <treatment-bin> <slab-bin> <out-dir>
#   Env:   REPS (30), WARMUP (2), AFFINITY (8-15), STALL_S (90), RETRIES (3), PLN
set -uo pipefail
CTRL="${1:?control binary}"; TREAT="${2:?treatment binary}"; SLAB="${3:?slab binary}"; OUT="${4:?out dir}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="${REPO:-$(cd -- "$SCRIPT_DIR/.." && pwd -P)}"
PLN="${PLN:-$(cd -- "$REPO/.." && pwd -P)/PLN-main}"
FX="$PLN/examples/Robot.metta"
REPS="${REPS:-30}"; WARMUP="${WARMUP:-2}"; AFFINITY="${AFFINITY:-8-15}"
STALL_S="${STALL_S:-90}"; RETRIES="${RETRIES:-3}"
mkdir -p "$OUT"
CSV="$OUT/robot_samples.csv"
echo "arm,rep,attempt,wall_s,rc,hang" > "$CSV"

# run_rep <arm> <bin> <rep> -> appends CSV; returns 0 if a timing was recorded
run_rep() {
  local arm="$1" bin="$2" rep="$3" attempt rc wall hung
  for attempt in $(seq 1 "$RETRIES"); do
    local err="$OUT/err_${arm}_${rep}_${attempt}.log" twall="$OUT/twall_${arm}_${rep}_${attempt}.txt"
    # NO /usr/bin/time wrapper for the signaled process: GNU time does NOT
    # forward signals — SIGUSR1 killed the WRAPPER and orphaned the wedged
    # mettatron alive (that is how the long-lived #309 specimens were created,
    # and why every farmed dump was empty). Wall time now comes from bash's
    # SECONDS at kill/exit; the binary is the direct child so signals land.
    env -u METTATRON_PARALLEL_FANOUT_DEPTH \
      taskset -c "$AFFINITY" "$bin" "$FX" > /dev/null 2> "$err" &
    local tpid=$!
    SECONDS=0
    hung=0
    local waited=0
    while kill -0 "$tpid" 2>/dev/null; do
      sleep 1; waited=$((waited+1))
      if [ "$waited" -ge "$STALL_S" ]; then
        hung=1
        echo "[hang] arm=$arm rep=$rep attempt=$attempt pid=$tpid — SIGUSR1 dump then kill" >&2
        kill -USR1 "$tpid" 2>/dev/null; sleep 3
        kill -9 "$tpid" 2>/dev/null
        break
      fi
    done
    wait "$tpid" 2>/dev/null; rc=$?
    if [ "$hung" -eq 1 ]; then
      echo "$arm,$rep,$attempt,," >> "$CSV"; sed -i '$ s/$/1/' "$CSV"
      continue  # retry
    fi
    wall="$SECONDS"  # (integer seconds; the time-wrapper was removed — see above)
    echo "$arm,$rep,$attempt,$wall,$rc,0" >> "$CSV"
    rm -f "$err" "$twall"   # keep stderr only for hangs (the dump farm)
    return 0
  done
  return 1
}

echo "===== F1 Robot resilient A/B  reps=$REPS warmup=$WARMUP stall=${STALL_S}s ====="; date
for w in $(seq 1 "$WARMUP"); do
  run_rep warmup-ctrl "$CTRL" "w$w" || true
  run_rep warmup-treat "$TREAT" "w$w" || true
done
# Warmup rows stay in the CSV labeled warmup-*; analysis filters them out.
for i in $(seq 1 "$REPS"); do
  run_rep control "$CTRL" "$i" || echo "[give-up] control rep $i after $RETRIES hangs" >&2
  run_rep treatment "$TREAT" "$i" || echo "[give-up] treatment rep $i after $RETRIES hangs" >&2
  run_rep slab "$SLAB" "$i" || echo "[give-up] slab rep $i after $RETRIES hangs" >&2
done
echo "===== done ====="; date
echo "--- hang census ---"
awk -F, 'NR>1 && $6==1 {h[$1]++} END {for (a in h) print a, h[a]; if (length(h)==0) print "no hangs"}' "$CSV"
