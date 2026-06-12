#!/usr/bin/env bash
# Experiment #15 measurement — INTERLEAVED PAIRED ROUNDS (pre-registered amendment).
#
# ── PRE-REGISTERED PROTOCOL AMENDMENT (committed BEFORE any sample is taken) ──
# Attempt #1 (sequential hyperfine, per #14) was discarded as contaminated and
# recorded ZERO samples. The sequential design (all 51 control reps, then all
# 51 treatment reps) is drift-vulnerable: another session's continuously
# iterating unpinned builds (a 30-min quiet-watcher timed out) can overlap one
# arm's ~15-min window only, biasing the between-arm comparison — the same
# drift class that invalidated attempt #1.
#
# AMENDMENT (run-scheduling ONLY; the frozen decision rule is UNTOUCHED):
#   51 rounds, each = one control + one treatment run back-to-back; the
#   within-round order ALTERNATES by round parity ((C,T) even, (T,C) odd) to
#   neutralize within-round trends; 3 alternating warmup pairs precede.
#   Minutes-scale external load waves hit both arms symmetrically because a
#   round (tens of seconds) is much shorter than a wave; per-round external
#   rustc counts are logged for a post-hoc symmetric-exposure audit.
#
# FROZEN CRITERION (unchanged): metric toothbrush_default_index_wall_s;
#   Welch one-tailed (treat < control), alpha=0.05; Cohen's d >= 0.5;
#   n=51 per arm; taskset -c 8-15; default env (no METTATRON_* overrides);
#   performance governor. The analysis is the identical UNPAIRED Welch t on
#   the same 51+51 samples (pairing is exploited only for drift symmetry —
#   statistically conservative).
#
# ARMS: control  = clean HEAD 552886b9 build (sibling worktree ../exp15-ctrl)
#       treatment = HEAD + the uncommitted metta_value.rs cfg-split
#       (both binaries include the #309 fixes; smoke: rc=0 both, sorted
#        outputs identical, hashes differ)
set -uo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
PLN="${PLN:-$(cd -- "$REPO/.." && pwd -P)/PLN-main}"
FX="$PLN/examples/Toothbrush.metta"
OUT="${OUT:-$REPO/target/gc-logs/exp15_clean}"
CTRL="${CTRL:-$OUT/mtt-control}"
TREAT="${TREAT:-$OUT/mtt-treat}"
AFFINITY="${AFFINITY:-8-15}"
WARMUP="${WARMUP:-3}"
ROUNDS="${ROUNDS:-51}"
CSV="$OUT/interleaved_samples.csv"
mkdir -p "$OUT"

echo "===== EXP15 INTERLEAVED BENCH ====="; date
echo "fixture=$FX rounds=$ROUNDS warmup=$WARMUP affinity=$AFFINITY"
[ -x "$CTRL" ]  || { echo "REFUSING: control binary missing: $CTRL"; exit 2; }
[ -x "$TREAT" ] || { echo "REFUSING: treat binary missing: $TREAT"; exit 2; }
[ -f "$FX" ]    || { echo "REFUSING: fixture missing: $FX"; exit 2; }
GOV="$(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor 2>/dev/null || echo unknown)"
echo "governor(cpu8)=$GOV"
[ "$GOV" = "performance" ] || echo "WARNING: governor '$GOV' (locked protocol assumes performance)"

# Provenance.
{ echo "head=$(git -C "$REPO" rev-parse HEAD)"; git -C "$REPO" status --short;
  sha256sum "$CTRL" "$TREAT"; date; } > "$OUT/interleaved_provenance.txt"

ext_load() { pgrep -c -f 'rustc --crate-name' 2>/dev/null || echo 0; }

# one_run <binary> -> echoes wall seconds; nonzero rc aborts the experiment.
one_run() {
  local bin="$1" t0 t1 rc
  t0=$(date +%s.%N)
  taskset -c "$AFFINITY" "$bin" "$FX" > /dev/null 2>&1
  rc=$?
  t1=$(date +%s.%N)
  if [ "$rc" -ne 0 ]; then echo "ABORT: $bin rc=$rc" >&2; exit 3; fi
  echo "$t0 $t1" | awk '{printf "%.6f", $2-$1}'
}

echo "round,arm,order_pos,wall_s,ext_rustc" > "$CSV"
echo "── warmup ($WARMUP alternating pairs) ──"
for ((w=1; w<=WARMUP; w++)); do
  one_run "$CTRL" > /dev/null
  one_run "$TREAT" > /dev/null
done

echo "── measurement ($ROUNDS interleaved rounds) ──"
for ((r=1; r<=ROUNDS; r++)); do
  load=$(ext_load)
  if (( r % 2 == 0 )); then
    wc_=$(one_run "$CTRL");  echo "$r,control,1,$wc_,$load" >> "$CSV"
    wt_=$(one_run "$TREAT"); echo "$r,treat,2,$wt_,$load"   >> "$CSV"
  else
    wt_=$(one_run "$TREAT"); echo "$r,treat,1,$wt_,$load"   >> "$CSV"
    wc_=$(one_run "$CTRL");  echo "$r,control,2,$wc_,$load" >> "$CSV"
  fi
  printf "  round %02d/%d: C=%ss T=%ss ext=%s\n" "$r" "$ROUNDS" "$wc_" "$wt_" "$load"
done

echo "── Welch one-tailed (treat < control) + Cohen's d (frozen criterion) ──"
python3 - "$CSV" <<'PY'
import csv, sys, math
ctrl, treat, loads = [], [], []
for row in csv.DictReader(open(sys.argv[1])):
    (ctrl if row["arm"] == "control" else treat).append(float(row["wall_s"]))
    loads.append(int(row["ext_rustc"]))
n1, n2 = len(ctrl), len(treat)
m1 = sum(ctrl)/n1; m2 = sum(treat)/n2
v1 = sum((x-m1)**2 for x in ctrl)/(n1-1); v2 = sum((x-m2)**2 for x in treat)/(n2-1)
se = math.sqrt(v1/n1 + v2/n2)
t = (m1 - m2) / se   # >0 when treat is FASTER (H1: treat < control)
df = (v1/n1 + v2/n2)**2 / ((v1/n1)**2/(n1-1) + (v2/n2)**2/(n2-1))
sp = math.sqrt(((n1-1)*v1 + (n2-1)*v2) / (n1+n2-2))
cd = (m1 - m2) / sp
def betacf(a, b, x):
    MAXIT, EPS, FPMIN = 200, 3e-9, 1e-30
    qab, qap, qam = a+b, a+1.0, a-1.0
    c, dd = 1.0, max(1.0 - qab*x/qap, FPMIN); dd = 1.0/dd; h = dd
    for m in range(1, MAXIT+1):
        m2_ = 2*m
        aa = m*(b-m)*x/((qam+m2_)*(a+m2_))
        dd = max(1.0+aa*dd, FPMIN); c = max(1.0+aa/c, FPMIN); dd = 1.0/dd; h *= dd*c
        aa = -(a+m)*(qab+m)*x/((a+m2_)*(qap+m2_))
        dd = max(1.0+aa*dd, FPMIN); c = max(1.0+aa/c, FPMIN); dd = 1.0/dd
        de = dd*c; h *= de
        if abs(de-1.0) < EPS: break
    return h
def betai(a, b, x):
    if x <= 0: return 0.0
    if x >= 1: return 1.0
    bt = math.exp(math.lgamma(a+b)-math.lgamma(a)-math.lgamma(b)+a*math.log(x)+b*math.log(1.0-x))
    return bt*betacf(a,b,x)/a if x < (a+1.0)/(a+b+2.0) else 1.0-bt*betacf(b,a,1.0-x)/b
x = df / (df + t*t)
p2 = betai(df/2.0, 0.5, x)
p = p2/2.0 if t > 0 else 1.0 - p2/2.0
print(f"control: n={n1} mean={m1:.4f}s sd={math.sqrt(v1):.4f}")
print(f"treat:   n={n2} mean={m2:.4f}s sd={math.sqrt(v2):.4f}")
print(f"Welch t={t:.3f} df={df:.1f} one-tailed p={p:.2e} (H1: treat < control)")
print(f"Cohen's d={cd:.3f} (criterion: d >= 0.5)")
print(f"delta = {m1-m2:+.4f}s ({100*(m1-m2)/m1:+.2f}%)")
print(f"ext-load audit: rounds with rustc>0: {sum(1 for l in loads if l>0)}/{len(loads)} (max {max(loads)})")
verdict = "ACCEPT" if (p < 0.05 and cd >= 0.5) else "REJECT"
print(f"LOCKED-CRITERION VERDICT: {verdict} (alpha=0.05 one-tailed AND d>=0.5)")
PY
RC=$?
echo "===== EXP15 INTERLEAVED BENCH DONE (rc=$RC) ====="; date
exit "$RC"
