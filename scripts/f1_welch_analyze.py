#!/usr/bin/env python3
"""F1 Welch analyzer for experiment #6.

Inputs are CSV files emitted by ``scripts/f1_welch_bench.sh``. Each CSV holds
warmup and measured rows for one workload and both arms:

    workload,phase,arm,rep,order_pos,wall_ms,peak_rss_mib,out_log,err_log

The analyzer reports the migration gate in the same spirit as the original
script: reject only when the index arm is statistically and materially worse.
It also writes ``f1_pgmcp_measurements.json`` next to the first CSV so the raw
per-replicate samples can be submitted to pgmcp without scraping console text.
"""

from __future__ import annotations

import csv
import json
import math
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path

from scipy import stats

ALPHA = 0.05
TOLERANCE = 0.10
EXPERIMENT_ID = 6
HYPOTHESIS_ID = 6


@dataclass(frozen=True)
class Series:
    workload: str
    metric: str
    unit: str
    slab: list[float]
    index: list[float]


def fmt_ms(x: float) -> str:
    return f"{x:.1f}ms" if x < 10_000 else f"{x / 1000.0:.3f}s"


def cohen_d(control: list[float], treatment: list[float]) -> float:
    n1, n2 = len(control), len(treatment)
    if n1 < 2 or n2 < 2:
        return 0.0
    v1 = statistics.variance(control)
    v2 = statistics.variance(treatment)
    pooled = math.sqrt(((n1 - 1) * v1 + (n2 - 1) * v2) / (n1 + n2 - 2))
    return (statistics.mean(treatment) - statistics.mean(control)) / pooled if pooled else 0.0


def welch_p_treatment_greater(control: list[float], treatment: list[float]) -> float:
    try:
        return float(stats.ttest_ind(treatment, control, equal_var=False, alternative="greater").pvalue)
    except TypeError:
        result = stats.ttest_ind(treatment, control, equal_var=False)
        two_sided = float(getattr(result, "pvalue"))
        if statistics.mean(treatment) > statistics.mean(control):
            return two_sided / 2.0
        return 1.0 - (two_sided / 2.0)


def metric_prefix(workload: str) -> str:
    wl = workload.lower()
    if wl == "robot":
        return "pln_robot"
    if wl == "mmverify":
        return "mmverify"
    return "pln_" + "".join(ch if ch.isalnum() else "_" for ch in wl).strip("_")


def csv_to_series(path: Path) -> list[Series]:
    by_arm: dict[str, dict[str, list[float]]] = {}
    workload = path.stem.replace("_samples", "")
    with path.open(newline="") as handle:
        for row in csv.DictReader(handle):
            if row.get("phase") != "measure":
                continue
            workload = row["workload"]
            arm = row["arm"]
            by_arm.setdefault(arm, {"wall_ms": [], "peak_rss_mib": []})
            by_arm[arm]["wall_ms"].append(float(row["wall_ms"]))
            by_arm[arm]["peak_rss_mib"].append(float(row["peak_rss_mib"]))
    if "slab" not in by_arm or "index" not in by_arm:
        raise SystemExit(f"{path}: expected measured slab and index rows")
    prefix = metric_prefix(workload)
    return [
        Series(workload, f"{prefix}_wall_ms", "ms", by_arm["slab"]["wall_ms"], by_arm["index"]["wall_ms"]),
        Series(
            workload,
            f"{prefix}_peak_rss_mib",
            "MiB",
            by_arm["slab"]["peak_rss_mib"],
            by_arm["index"]["peak_rss_mib"],
        ),
    ]


def write_pgmcp_payload(series: list[Series], out_path: Path) -> None:
    records = []
    for item in series:
        for arm_kind, arm_label, samples in (
            ("control", "control", item.slab),
            ("treatment", "treatment", item.index),
        ):
            records.append(
                {
                    "experiment_id": EXPERIMENT_ID,
                    "hypothesis_id": HYPOTHESIS_ID,
                    "arm_kind": arm_kind,
                    "arm_label": arm_label,
                    "metric": item.metric,
                    "unit": item.unit,
                    "source": "external_benchmark",
                    "samples": samples,
                    "command_spec": {"harness": "scripts/f1_welch_bench.sh", "workload": item.workload},
                }
            )
    out_path.write_text(json.dumps({"measurements": records}, indent=2, sort_keys=True) + "\n")


def main(argv: list[str]) -> int:
    if not argv:
        print("Usage: f1_welch_analyze.py <workload_samples.csv> ...", file=sys.stderr)
        return 2

    series: list[Series] = []
    for arg in argv:
        series.extend(csv_to_series(Path(arg)))

    reject = False
    wall_rows = [item for item in series if item.metric.endswith("_wall_ms")]
    rss_rows = [item for item in series if item.metric.endswith("_peak_rss_mib")]

    width = max((len(item.workload) for item in series), default=8)
    print(f"\n{'workload':<{width}}  {'metric':<12}  {'slab mean':>12}  {'index mean':>12}  {'ratio i/s':>9}  {'p(index>)':>10}  {'d':>8}  verdict")
    print("-" * (width + 83))
    for item in wall_rows + rss_rows:
        slab_mean = statistics.mean(item.slab)
        index_mean = statistics.mean(item.index)
        ratio = index_mean / slab_mean if slab_mean else float("nan")
        p = welch_p_treatment_greater(item.slab, item.index)
        d = cohen_d(item.slab, item.index)
        significant = p < ALPHA
        worse = ratio > 1.0
        large_effect = d >= 0.8
        if item.metric.endswith("_wall_ms"):
            material = ratio > (1.0 + TOLERANCE)
            budget_breach = item.metric.startswith("pln_robot_") and index_mean > 12_000.0
        else:
            material = ratio > 1.25
            budget_breach = (index_mean - slab_mean) > 8192.0
        item_reject = significant and worse and (large_effect or material or budget_breach)
        reject = reject or item_reject
        if item.unit == "ms":
            slab_text, index_text = fmt_ms(slab_mean), fmt_ms(index_mean)
        else:
            slab_text, index_text = f"{slab_mean:.1f}MiB", f"{index_mean:.1f}MiB"
        verdict = "REJECT" if item_reject else ("ACCEPT*" if significant and worse else "ACCEPT")
        print(
            f"{item.workload:<{width}}  {item.metric:<12}  {slab_text:>12}  {index_text:>12}  "
            f"{ratio:>8.3f}x  {p:>10.4g}  {d:>8.3f}  {verdict}"
        )

    print("-" * (width + 83))
    print(
        f"ALPHA={ALPHA}  WALL_TOLERANCE={TOLERANCE:.0%}  "
        "(REJECT = significant and worse with d>=0.8, tolerance breach, or budget breach)"
    )
    print(f"\nEXPERIMENT VERDICT: {'REJECT -- index regresses materially' if reject else 'ACCEPT -- index is perf-ready to default'}")

    payload_path = Path(argv[0]).resolve().parent / "f1_pgmcp_measurements.json"
    write_pgmcp_payload(series, payload_path)
    print(f"PGMCP_MEASUREMENTS_JSON={payload_path}")
    return 1 if reject else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
