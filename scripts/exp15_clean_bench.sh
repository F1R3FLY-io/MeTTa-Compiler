#!/usr/bin/env bash
# Experiment #15 CLEAN re-measurement — release-build UnsafeCell shadow directory
# (slug: release-build-unsafecell-shadow-directory-drop-the-refcell-borrow-flag-rmw-from-the-hit-path-f1)
#
# The first measurement was CONTAMINATED: the #309 wedge-validation farm ran
# concurrently on the same taskset cores. This rig re-runs the LOCKED protocol
# on a quiet machine:
#   metric   toothbrush_default_index_wall_s  (Toothbrush, DEFAULT env — no
#            METTATRON_* overrides; the shipped default)
#   stats    Welch one-tailed (treat < control), alpha=0.05, Cohen's d >= 0.5
#   n        3 warmup + 51 reps per arm, taskset 8-15, performance governor
#   control  clean HEAD checkout (sibling worktree; HEAD differs from the
#            working tree ONLY by the exp15 metta_value.rs cfg-split)
#   treat    working-tree index build (HEAD + exp15 diff)
#
# PRECONDITIONS (the script refuses to run otherwise):
#   - no other cargo/rustc/mettatron/hyperfine processes (quiet machine)
#   - both binaries already staged (build them BEFORE invoking; see below)
#
# Staging the binaries (run on a quiet machine):
#   treat:   snapshot the greenwall's working-tree index build:
#              cp target/release/mettatron "$OUT/mtt-treat"   (built --features index-gc)
#            or rebuild: systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 --quiet \
#              cargo build --release --bin mettatron --features index-gc
#   control: git worktree add --detach /tmp/exp15-ctrl <HEAD-sha>; then in it:
#              systemd-run ... cargo +nightly build --release --bin mettatron --features index-gc
#            (worktrees do NOT inherit the rustup dir override — use +nightly
#             explicitly; see memory: worktree builds need `cargo +nightly`)
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
REPS="${REPS:-51}"
mkdir -p "$OUT"

echo "===== EXP15 CLEAN BENCH ====="; date
echo "fixture=$FX  control=$CTRL  treat=$TREAT  affinity=$AFFINITY warmup=$WARMUP reps=$REPS"

# ── Quiet-machine gate (the contamination lesson: locked criterion says NO concurrent load) ──
BUSY="$(ps aux | grep -E 'cargo build|rustc |mettatron |hyperfine' | grep -v grep | grep -v exp15_clean_bench || true)"
if [ -n "$BUSY" ]; then
  echo "REFUSING: machine not quiet:"; echo "$BUSY"; exit 2
fi
GOV="$(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor 2>/dev/null || echo unknown)"
echo "governor(cpu8)=$GOV"
[ "$GOV" = "performance" ] || echo "WARNING: governor is '$GOV', locked protocol assumes performance"
[ -x "$CTRL" ] || { echo "REFUSING: control binary missing: $CTRL"; exit 2; }
[ -x "$TREAT" ] || { echo "REFUSING: treat binary missing: $TREAT"; exit 2; }
[ -f "$FX" ] || { echo "REFUSING: fixture missing: $FX"; exit 2; }

# Provenance: record binary hashes + git state.
{ echo "head=$(git -C "$REPO" rev-parse HEAD)"; git -C "$REPO" status --short;
  sha256sum "$CTRL" "$TREAT"; } | tee "$OUT/provenance.txt"

# ── Measure (DEFAULT env: no METTATRON_* overrides) ──
JSON="$OUT/toothbrush_default.json"
hyperfine --warmup "$WARMUP" --runs "$REPS" --export-json "$JSON" \
  -n control "taskset -c $AFFINITY $CTRL $FX" \
  -n treat   "taskset -c $AFFINITY $TREAT $FX" \
  | tee "$OUT/toothbrush_hyperfine.txt"

# ── Welch one-tailed (treat < control) + Cohen's d against the locked criterion ──
python3 - "$JSON" <<'PY'
import json, sys, math
d = json.load(open(sys.argv[1]))
# hyperfine preserves -n order: results[0]=control, results[1]=treat
ctrl = d["results"][0]["times"]; treat = d["results"][1]["times"]
n1, n2 = len(ctrl), len(treat)
m1 = sum(ctrl)/n1; m2 = sum(treat)/n2
v1 = sum((x-m1)**2 for x in ctrl)/(n1-1); v2 = sum((x-m2)**2 for x in treat)/(n2-1)
se = math.sqrt(v1/n1 + v2/n2)
t = (m1 - m2) / se   # >0 when treat is FASTER (one-tailed: treat < control)
df = (v1/n1 + v2/n2)**2 / ((v1/n1)**2/(n1-1) + (v2/n2)**2/(n2-1))
sp = math.sqrt(((n1-1)*v1 + (n2-1)*v2) / (n1+n2-2))
cd = (m1 - m2) / sp
# one-tailed p via the survival function of t (normal approx is NOT acceptable at this df? df~100 -> fine, but do exact via betainc)
def t_sf(t, df):
    # survival P(T > t) using the regularized incomplete beta
    x = df / (df + t*t)
    a, b = df/2.0, 0.5
    # continued-fraction betainc (Lentz) — adequate precision for the report
    def betacf(a, b, x):
        MAXIT, EPS, FPMIN = 200, 3e-9, 1e-30
        qab, qap, qam = a+b, a+1.0, a-1.0
        c, dd = 1.0, max(1.0 - qab*x/qap, FPMIN); dd = 1.0/dd; h = dd
        for m in range(1, MAXIT+1):
            m2 = 2*m
            aa = m*(b-m)*x/((qam+m2)*(a+m2))
            dd = max(1.0+aa*dd, FPMIN); c = max(1.0+aa/c, FPMIN); dd = 1.0/dd; h *= dd*c
            aa = -(a+m)*(qab+m)*x/((a+m2)*(qap+m2))
            dd = max(1.0+aa*dd, FPMIN); c = max(1.0+aa/c, FPMIN); dd = 1.0/dd
            de = dd*c; h *= de
            if abs(de-1.0) < EPS: break
        return h
    def betai(a, b, x):
        if x <= 0: return 0.0
        if x >= 1: return 1.0
        bt = math.exp(math.lgamma(a+b)-math.lgamma(a)-math.lgamma(b)+a*math.log(x)+b*math.log(1.0-x))
        return bt*betacf(a,b,x)/a if x < (a+1.0)/(a+b+2.0) else 1.0-bt*betacf(b,a,1.0-x)/b
    p2 = betai(a, b, x)          # two-tailed
    return p2/2.0 if t > 0 else 1.0 - p2/2.0
p = t_sf(t, df)
print(f"control: n={n1} mean={m1:.4f}s sd={math.sqrt(v1):.4f}")
print(f"treat:   n={n2} mean={m2:.4f}s sd={math.sqrt(v2):.4f}")
print(f"Welch t={t:.3f} df={df:.1f} one-tailed p={p:.2e} (H1: treat < control)")
print(f"Cohen's d={cd:.3f} (criterion: d >= 0.5)")
print(f"delta = {m1-m2:+.4f}s ({100*(m1-m2)/m1:+.2f}%)")
verdict = "ACCEPT" if (p < 0.05 and cd >= 0.5) else "REJECT"
print(f"LOCKED-CRITERION VERDICT: {verdict} (alpha=0.05 one-tailed AND d>=0.5)")
PY
RC=$?
echo "===== EXP15 CLEAN BENCH DONE (rc=$RC) ====="; date
