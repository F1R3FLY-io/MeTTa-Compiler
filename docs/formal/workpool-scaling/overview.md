# WorkPool Scaling: Formal Verification Overview

## Purpose

The WorkPool decides how many worker threads to keep active at any moment. A wrong
decision has severe consequences: too many threads under memory pressure triggers
OOM kills (unrecoverable), while too few threads wastes available throughput. The
hill climber's four-term objective function balances these competing concerns, but
how do we know the weights are correct?

Formal verification in Rocq (Coq) provides machine-checked guarantees that:

1. The hill climber scales **down** when past the USL throughput peak under memory pressure
2. The hill climber scales **up** when below the peak with no pressure and pending work
3. The hill climber **holds** at the optimum when all signals are quiescent
4. Convergence to the optimum takes at most |n₀ − N_opt| effective steps
5. The concrete weight values satisfy all five dominance conditions simultaneously

## Proof Architecture

```
┌──────────────────┐
│    Prelude.v      │  WorkPoolParams record: T₁, σ, κ, λ
│ (proof fields)    │  Constants: α, threshold, cooldown
└────────┬─────────┘
         │
    ┌────┴─────────────────────┐
    │                          │
    ▼                          ▼
┌─────────────┐  ┌──────────────────────┐
│   USL.v      │  │  LyapunovConvergence.v│
│  T(N), N★    │  │  V(n) = (n−N_opt)²/2 │
└──────┬──────┘  └──────────────────────┘
       │
       ▼
┌────────────────────┐
│ObjectiveFunction.v  │  J(N) = −w_tp·T + w_qd·Q + w_mp·M + w_rss·R
└────────┬───────────┘
         │
         ▼
┌────────────────────┐
│ WeightDominance.v   │  C1–C5 concrete verification
└────────────────────┘
```

## Proof File Summary

| File | What it proves | Key theorems |
|------|----------------|--------------|
| `Prelude.v` | Explicit model-input record, range evidence, utility lemmas | `WorkPoolParams`, `sigma_range`, `ema_gain_pos`, `ema_gain_value` |
| `USL.v` | USL throughput model, peak, monotonicity | `USL_peak`, `USL_increasing`, `USL_decreasing`, `USL_peak_unique` |
| `ObjectiveFunction.v` | 4-term objective, gradient theorems | `obj_scale_down_past_peak`, `obj_scale_up_below_peak`, `obj_hold_at_optimum` |
| `LyapunovConvergence.v` | Lyapunov stability + convergence bound | `lyapunov_decrease_overprovisioned`, `lyapunov_decrease_underprovisioned`, `convergence_steps` |
| `WeightDominance.v` | 5 concrete weight conditions | `weight_dominance_at_peak`, `weight_dominance_typical`, `rss_dominates_slab`, `queue_threshold_sensitivity`, `scaleup_dominates_pressure`, `all_weights_valid` |

## Recommended Reading Order

1. **Prelude.v** -- Establishes the parameter space and shared lemmas
2. **USL.v** -- Builds the throughput model atop the parameters
3. **ObjectiveFunction.v** -- Constructs the composite objective using USL throughput
4. **LyapunovConvergence.v** -- Proves convergence independently (only depends on Prelude)
5. **WeightDominance.v** -- Instantiates ObjectiveFunction with concrete weights

## How to Build

All proofs compile under Rocq 9.1 with Coquelicot. Use resource limits to prevent
system unresponsiveness:

```bash
cd formal/rocq/work_pool_stability
systemd-run --user --scope \
  -p MemoryMax=126G \
  -p CPUQuota=1800% \
  -p IOWeight=30 \
  -p TasksMax=200 \
  make -j1
```

## Companion Documents

| Document | Contents |
|----------|----------|
| [system-model.md](system-model.md) | USL throughput model derivation |
| [objective-function.md](objective-function.md) | 4-term composite objective + gradient theorems |
| [lyapunov-convergence.md](lyapunov-convergence.md) | Lyapunov stability analysis |
| [weight-dominance.md](weight-dominance.md) | 5 dominance conditions with numeric examples |
| [rust-proof-alignment.md](rust-proof-alignment.md) | Point-by-point Rust ↔ Rocq correspondence |
