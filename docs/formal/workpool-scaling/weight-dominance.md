# Weight Dominance Verification

## Motivation

The proofs in [objective-function.md](objective-function.md) are **parametric** --
they hold for any weight values satisfying certain inequalities. But do our
specific weights (w_tp = 1.0, w_qd = 0.5, w_mp = 5.0, w_rss = 8.0) actually
satisfy those inequalities?

`WeightDominance.v` instantiates the parametric theorems with concrete numeric
values and machine-checks all five dominance conditions simultaneously. This
closes the gap between "the math works in general" and "our code uses correct
values."

## EMA Gain Derivation

The EMA smoothing factor α = 0.15 produces a gain factor used in several
conditions:

```
ema_gain = α / (1 − α) = 0.15 / 0.85 = 15/85 ≈ 0.176
```

This represents the steady-state ratio of a unit step response after one EMA
update. Throughput deltas are multiplied by this factor before reaching the
objective, dampening transient spikes.

**Rocq verification** (`ema_gain_value`): `ema_gain = 15 / 85`, proven by
`field_simplify; lra`.

## Condition 1: Memory Overrides Zero Gradient at Peak

**Statement**: At N = N★, the throughput gradient is exactly zero (USL_peak).
Any positive memory pressure must produce a positive ΔJ to trigger parking.

```
w_mp · 1 > 0   ⟹   5.0 > 0  ✓
```

**Rocq theorem** (`weight_dominance_at_peak`): `w_mp * 1 > 0`.

**Physical interpretation**: At the USL peak, throughput is flat. Even the mildest
slab pressure (backpressure level 1) immediately dominates, because the throughput
term contributes exactly zero to the gradient. The hill climber correctly parks.

**Corollary** (`memory_dominates_zero_gradient`): For any ε > 0, w_mp · ε > 0.
This generalizes to arbitrarily small pressure signals.

## Condition 2: Memory Overrides Near-Peak Throughput

**Statement**: Near (but not exactly at) the peak, throughput has a small nonzero
gradient. The memory weight must still dominate after EMA smoothing.

```
∀ ΔT_max ≤ 28:  w_mp · 1 > w_tp · ΔT_max · ema_gain
                 5.0 > 1.0 × ΔT_max × (15/85)
```

**Boundary check**: At ΔT_max = 28 (the maximum proven bound):

```
w_tp × 28 × (15/85) = 28 × 15 / 85 = 420/85 ≈ 4.94
5.0 > 4.94  ✓
```

**Typical instantiation** (ΔT_max = 9, realistic near-peak gradient):

```
w_tp × 9 × (15/85) = 9 × 15 / 85 = 135/85 ≈ 1.59
5.0 > 1.59  ✓✓  (3.1× margin)
```

**Rocq theorems**:
- `weight_dominance_near_peak`: Parametric for ΔT_max ∈ [0, 28]
- `weight_dominance_typical`: Concrete instantiation at ΔT_max = 9

**Proof technique**: Multiply both sides by (1 − α) > 0 to clear the fraction,
then `field_simplify; lra` closes the arithmetic.

**Physical interpretation**: Even when throughput is still slightly increasing
near the peak, moderate slab pressure (M = 1) overrides the throughput signal.
This prevents the hill climber from chasing marginal throughput gains into the
memory-constrained region.

## Condition 3: RSS Dominates Slab Pressure

**Statement**: The RSS weight is strictly greater than the slab weight.

```
w_rss > w_mp   ⟹   8.0 > 5.0  ✓
Ratio: w_rss / w_mp = 8/5 = 1.6×
```

**Rocq theorems**:
- `rss_dominates_slab`: `w_rss > w_mp`
- `rss_slab_ratio`: `w_rss / w_mp = 8 / 5`

**Physical interpretation**: OOM kills are **unrecoverable** -- the kernel
terminates the process immediately with no chance for graceful degradation.
Slab pressure, by contrast, can be mitigated by the garbage collector. The
60% premium on w_rss ensures that when the system is under RSS pressure,
the hill climber prioritizes reducing thread count even if slab pressure
alone would not trigger action.

## Condition 4: Queue Sensitivity Matches Threshold

**Statement**: At mild queue buildup (Q = 0.1), the queue contribution to
the objective matches the hill climber's improvement threshold.

```
w_qd · 0.1 ≥ threshold   ⟹   0.5 × 0.1 = 0.05 ≥ 0.05  ✓
```

**Rocq theorem** (`queue_threshold_sensitivity`): `w_qd * (1/10) >= threshold`.

**Complementary bound** (`queue_not_too_aggressive`): At moderate buildup
(Q = 0.5):

```
w_qd · 0.5 = 0.25 < w_mp · 1 = 5.0
```

This ensures queue depth never overrides memory pressure.

**Physical interpretation**: The queue weight is calibrated so that even a small
queue (0.1 pending tasks) is sufficient to nudge the hill climber toward unparking,
while remaining subordinate to the memory pressure signals.

## Condition 5: Scale-Up Dominates Mild Pressure at Low N

**Statement**: At low thread counts, the throughput gain from adding a thread
dominates mild memory pressure (M = 1).

```
∀ T₁ ≥ 32:  w_tp · (T₁ × 0.9) · ema_gain > w_mp · 1
```

**Boundary check** at T₁ = 32:

```
1.0 × (32 × 0.9) × (15/85) = 28.8 × 15/85 = 432/85 ≈ 5.08
5.08 > 5.0  ✓
```

**High-throughput check** at T₁ = 400 (realistic single-thread throughput):

```
1.0 × (400 × 0.9) × (15/85) = 360 × 15/85 = 5400/85 ≈ 63.5
63.5 > 5.0  ✓✓  (12.7× margin)
```

**Rocq theorems**:
- `scaleup_dominates_pressure`: Parametric for T₁ ≥ 32
- `scaleup_dominates_max_pressure`: Even at max pressure (M = 3), T₁ = 400:
  `1.0 × 400 × (15/85) = 70.6 > 15.0 = 5.0 × 3`

**Proof technique**: Same as C2: multiply both sides by (1 − α), then
`field_simplify; lra`.

**Physical interpretation**: At low thread counts (N = 1 or 2), the throughput
gain from parallelism is enormous -- nearly T₁ per added thread. Even if there
is mild slab pressure, the throughput benefit justifies adding a thread. This
prevents the system from being stuck at minimum threads under transient pressure.

## Summary Theorem

**Theorem** (`all_weights_valid`): All five conditions hold simultaneously.

```coq
Theorem all_weights_valid :
  w_mp > 0 /\
  w_mp * 1 > w_tp * 9 * ema_gain /\
  w_rss > w_mp /\
  w_qd * (1 / 10) >= threshold /\
  (forall T1_val, T1_val >= 32 ->
    w_tp * (T1_val * (9 / 10)) * ema_gain > w_mp * 1).
```

Proven by `repeat split` and invoking the individual theorems.

## Weight Regime Diagram

The following diagram shows which signal dominates the objective in each
operating region:

```
       Memory Pressure
  3  ┤ ██████████████████████████  ← M/R always dominates (C1, C2)
     │ ██████████████████████████
  2  ┤ ██████ Emergency Park █████  ← bp ≥ 2 bypasses hill climber
     │ ██████████████████████████
  1  ┤ █████┆╱╱╱╱╱╱╱╱╱╱╱╱╱╱█████  ← M dominates near peak (C2)
     │ ╱╱╱╱╱┆╱╱ Throughput ╱╱████     boundary: ΔT_max ≈ 28
  0  ┤ ╱╱╱╱╱┆╱╱ dominates ╱╱╱╱╱╱  ← Scale-up dominates (C5)
     └──────┼────────────────────▶
       1   N★              N_max
                Thread Count

  Legend:  ██ = Memory/RSS dominates → Park
           ╱╱ = Throughput dominates → Unpark
           ┆  = USL peak boundary
```

- **Left of N★, M = 0**: Throughput dominates (C5). Hill climber unparks.
- **Left of N★, M ≥ 1**: Memory dominates only if ΔT is small (C2 boundary).
  For large ΔT (low N), throughput still wins (C5).
- **At N★**: Zero throughput gradient. Any M > 0 dominates (C1).
- **Right of N★**: Throughput is falling. All terms reinforce parking.
- **M ≥ 2 anywhere**: Emergency override bypasses hill climber entirely.

## Rocq Artifacts Reference

| Name | Kind | Statement |
|------|------|-----------|
| `weight_dominance_at_peak` | Theorem | w_mp · 1 > 0 (C1) |
| `memory_dominates_zero_gradient` | Corollary | ∀ε > 0: w_mp · ε > 0 |
| `weight_dominance_near_peak` | Theorem | ΔT ≤ 28 → w_mp > w_tp · ΔT · gain (C2) |
| `weight_dominance_typical` | Corollary | w_mp > w_tp · 9 · gain (C2 at ΔT=9) |
| `rss_dominates_slab` | Theorem | w_rss > w_mp (C3) |
| `rss_slab_ratio` | Lemma | w_rss / w_mp = 8/5 |
| `queue_threshold_sensitivity` | Theorem | w_qd · 0.1 ≥ threshold (C4) |
| `queue_not_too_aggressive` | Lemma | w_qd · 0.5 < w_mp · 1 |
| `scaleup_dominates_pressure` | Theorem | T₁ ≥ 32 → throughput > pressure (C5) |
| `scaleup_dominates_max_pressure` | Theorem | T₁ = 400, M = 3: throughput wins |
| `all_weights_valid` | Theorem | C1 ∧ C2 ∧ C3 ∧ C4 ∧ C5 |
