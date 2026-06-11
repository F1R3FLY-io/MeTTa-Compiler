#!/usr/bin/env python3
"""F1 Welch analyzer — decide ACCEPT/REJECT for the index-vs-slab GC migration.

Reads one or more hyperfine JSON exports (each produced for a single workload
with exactly two commands, in order: [0]=slab+JIT baseline, [1]=index+JIT). For
each workload it runs a Welch's t-test (unequal variances) on the per-run wall
times and reports the mean ratio (index/slab) with its significance.

Decision (experiment gc-substrate #6): the index collector is ACCEPTED as the
default if, across all workloads, it is never *significantly slower* than slab
beyond a tolerance band. "Significantly slower" means the Welch test rejects
equality (p < ALPHA) AND the index mean exceeds the slab mean by more than
TOLERANCE. A faster-or-equal or within-noise result ACCEPTS. The migration's
value is the collector's structural properties (serializable continuations,
unified backtracking, abstract-GC, concurrent collection); F1 only requires that
it not impose a material throughput regression.

Usage: f1_welch_analyze.py <wl1.json> <wl2.json> ...
Exit 0 = ACCEPT (index ready to default), 1 = REJECT (a workload regresses).
"""
import sys
import json
import statistics
from scipy import stats

ALPHA = 0.05          # significance threshold for the Welch test
TOLERANCE = 0.10      # index may be up to 10% slower (mean) and still ACCEPT

def fmt(x: float) -> str:
    return f"{x:.4f}s" if x < 10 else f"{x:.3f}s"

rows = []
reject = False
for path in sys.argv[1:]:
    with open(path) as f:
        data = json.load(f)
    name = data.get("_workload") or path.rsplit("/", 1)[-1].replace(".json", "")
    res = data["results"]
    if len(res) != 2:
        print(f"!! {name}: expected 2 commands (slab,index), got {len(res)} — skipping")
        continue
    slab, index = res[0], res[1]
    s, i = slab["times"], index["times"]
    s_mean, i_mean = statistics.mean(s), statistics.mean(i)
    s_sd = statistics.pstdev(s) if len(s) > 1 else 0.0
    i_sd = statistics.pstdev(i) if len(i) > 1 else 0.0
    # Welch's t-test (unequal variance). Two-sided p; we care about direction.
    # (getattr: scipy's TtestResult exposes .pvalue at runtime but its type stub
    # omits it; getattr returns Any so float() is well-typed.)
    p = float(getattr(stats.ttest_ind(i, s, equal_var=False), "pvalue"))
    ratio = i_mean / s_mean if s_mean else float("nan")
    slower = ratio > 1.0
    significant = p < ALPHA
    # ACCEPT unless index is significantly slower beyond the tolerance band.
    workload_reject = significant and slower and (ratio - 1.0) > TOLERANCE
    if workload_reject:
        reject = True
    verdict = "REJECT" if workload_reject else ("ACCEPT" if not slower or not significant else "ACCEPT*")
    rows.append((name, s_mean, s_sd, i_mean, i_sd, ratio, p, verdict))

w = max((len(r[0]) for r in rows), default=8)
print(f"\n{'workload':<{w}}  {'slab mean':>11}  {'index mean':>11}  {'ratio i/s':>9}  {'p(welch)':>9}  verdict")
print("-" * (w + 62))
for name, sm, ssd, im, isd, ratio, p, verdict in rows:
    print(f"{name:<{w}}  {fmt(sm):>11}  {fmt(im):>11}  {ratio:>8.3f}x  {p:>9.4f}  {verdict}")
print("-" * (w + 62))
print(f"ALPHA={ALPHA}  TOLERANCE={TOLERANCE:.0%}  (ACCEPT* = slower but within noise/tolerance)")
print(f"\nEXPERIMENT VERDICT: {'REJECT — a workload regresses materially' if reject else 'ACCEPT — index is perf-ready to default'}")
sys.exit(1 if reject else 0)
