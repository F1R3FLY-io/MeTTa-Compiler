# Composite Objective Function

## Motivation

The hill climber needs a single scalar to minimize at each tick. Throughput alone
is blind to memory pressure -- the system would keep adding threads even as the
process approaches OOM. Queue depth alone ignores whether added threads actually
improve throughput. The four-term composite objective combines all relevant signals
with carefully chosen weights so the hill climber makes correct directional
decisions in every operating regime.

## Weight Constants

| Weight | Rocq Name | Rust Constant | Value | Role |
|--------|-----------|---------------|-------|------|
| w_tp | `w_tp` | `THROUGHPUT_WEIGHT` | 1.0 | Maximize throughput (negative coefficient) |
| w_qd | `w_qd` | `QUEUE_DEPTH_WEIGHT` | 0.5 | Minimize queue depth (positive coefficient) |
| w_mp | `w_mp` | `MEMORY_PRESSURE_WEIGHT` | 5.0 | Minimize slab pressure (positive coefficient) |
| w_rss | `w_rss` | `RSS_PRESSURE_WEIGHT` | 8.0 | Minimize RSS pressure (positive coefficient) |

The negative sign on w_tp means that increasing throughput **decreases** the
objective (good), while increasing pressure **increases** it (bad). The hill
climber minimizes J, so it naturally maximizes throughput and minimizes all
pressure signals.

## Objective Formula

```
J(N) = −w_tp · T(N) + w_qd · Q(N) + w_mp · M(N) + w_rss · R(N)
```

**Rocq definition** (`objective`):

```coq
Definition objective (p : WorkPoolParams) (s : WorkPoolSignals p) (N : R) : R :=
  - w_tp * USL_throughput p N
  + w_qd * Q p s N
  + w_mp * M p s N
  + w_rss * Rp p s N.
```

## Signal Contracts

Each signal is packaged in `WorkPoolSignals` with range constraints and
monotonicity properties that reflect the physical system. The Rocq theorems are
parametric over that record, so these properties are theorem inputs rather than
trusted global declarations.

### Memory Pressure M(N)

| Contract Field | Rocq Name | Statement |
|-------|-----------|-----------|
| Non-negativity | `M_nonneg` | ∀N: M(N) ≥ 0 |
| Upper bound | `M_upper` | ∀N: M(N) ≤ 3 |
| Monotonicity | `M_nondecreasing` | N₁ ≤ N₂ → M(N₁) ≤ M(N₂) |

**Physical basis**: M(N) maps the slab allocator's 4-level backpressure signal
(0, 1, 2, 3) to a continuous value. More active threads produce more allocations,
driving higher pressure. The Rust implementation `slab_pressure()` returns
`backpressure_level() as f64` (work_pool.rs:611).

### RSS Pressure R(N)

| Contract Field | Rocq Name | Statement |
|-------|-----------|-----------|
| Non-negativity | `Rp_nonneg` | ∀N: R(N) ≥ 0 |
| Upper bound | `Rp_upper` | ∀N: R(N) ≤ 3 |
| Monotonicity | `Rp_nondecreasing` | N₁ ≤ N₂ → R(N₁) ≤ R(N₂) |

**Physical basis**: R(N) is a linear ramp from 50% to 125% of the RSS limit,
clamped to [0, 3]. The Rust implementation `rss_pressure()` uses
`((ratio - 0.5) * 4.0).clamp(0.0, 3.0)` (work_pool.rs:634-635).

### Queue Depth Q(N)

| Contract Field | Rocq Name | Statement |
|-------|-----------|-----------|
| Non-negativity | `Q_nonneg` | ∀N: Q(N) ≥ 0 |
| Vanishing | `Q_zero_when_sufficient` | T(N) ≥ λ → Q(N) = 0 |
| Decreasing below peak | `Q_decreasing_below_peak` | N₁ < N₂ ≤ N★ → Q(N₂) ≤ Q(N₁) |

**Physical basis**: When throughput exceeds the arrival rate, the queue drains to
zero. Below the USL peak, adding threads increases throughput and drains the queue.

## Gradient Expansion

The objective gradient ΔJ = J(N+1) − J(N) decomposes term-by-term:

**Lemma** (`obj_delta_expand`):

```
ΔJ = −w_tp · ΔT + w_qd · (Q(N+1) − Q(N)) + w_mp · (M(N+1) − M(N)) + w_rss · (R(N+1) − R(N))
```

Proven by `ring` (algebraic expansion of the objective difference).

## Scale-Down Theorem

**Theorem** (`obj_scale_down_past_peak`): When N ≥ N★ with pressure present, ΔJ ≥ 0.

### Preconditions

1. N ≥ 1 (at least one thread)
2. N ≥ N★ (past the USL peak)
3. −w_tp · ΔT ≥ 0 (throughput is flat or falling)
4. Q(N+1) ≥ Q(N) (queue is non-decreasing past peak)
5. M(N) + R(N) > 0 (some pressure exists)

### Term-by-Term Sign Analysis

```
N ≥ N★:   −w_tp · ΔT ≥ 0    throughput flat/falling past peak
          +w_qd · ΔQ ≥ 0    queue non-decreasing (precondition)
          +w_mp · ΔM ≥ 0    pressure non-decreasing (M_nondecreasing)
          +w_rss · ΔR ≥ 0   RSS non-decreasing (Rp_nondecreasing)
          ───────────────
          ΔJ ≥ 0  ⟹  Hill climber detects worsening → Park
```

### Worked Example

At N = 35 threads with σ = 0.05, κ = 0.001 (N★ ≈ 30.8):

- ΔT = T(36) − T(35) < 0 (past peak), so −1.0 × ΔT > 0
- Slab pressure M = 1 (moderate), M(36) ≥ M(35), so ΔM ≥ 0
- RSS pressure R = 0.5, R(36) ≥ R(35), so ΔR ≥ 0
- Queue Q = 2.0, Q(36) ≥ Q(35), so ΔQ ≥ 0
- All four terms are non-negative → ΔJ ≥ 0 → Park

## Scale-Up Theorem

**Theorem** (`obj_scale_up_below_peak`): When below peak with no pressure and
pending work, ΔJ < 0.

### Preconditions

1. 1 ≤ N (at least one thread)
2. N + 1 ≤ N★ (still below peak after adding a thread)
3. M(N) = 0 and M(N+1) = 0 (no slab pressure)
4. R(N) = 0 and R(N+1) = 0 (no RSS pressure)
5. Q(N) > 0 (pending work exists)
6. ΔT > 0 (throughput is increasing)

### Term-by-Term Sign Analysis

```
N+1 ≤ N★, M=R=0:   −w_tp · ΔT < 0    throughput rising (precondition)
                    +w_qd · ΔQ ≤ 0    queue draining (Q_decreasing_below_peak)
                    +w_mp · ΔM = 0    no pressure change (M = 0 both sides)
                    +w_rss · ΔR = 0   no pressure change (R = 0 both sides)
                    ────────────────
                    ΔJ < 0  ⟹  Hill climber detects improvement → Unpark
```

### Worked Example

At N = 3 threads with T₁ = 200 evals/s:

- ΔT = T(4) − T(3) > 0 (well below peak), so −1.0 × ΔT < 0
- No slab pressure (M = 0 at low thread counts)
- No RSS pressure (R = 0, low memory usage)
- Queue Q = 15 (pending work), Q(4) ≤ Q(3) by `Q_decreasing_below_peak`
- Throughput term dominates → ΔJ < 0 → Unpark

## Hold Corollary

**Lemma** (`obj_hold_at_optimum`): When all signals are zero:

```
ΔJ = −w_tp · ΔT
```

Near N★, the throughput curve is flat (ΔT ≈ 0), so ΔJ ≈ 0. The hill climber's
dead zone (±0.05 threshold) absorbs this near-zero gradient, producing a Hold
decision. The system remains at the optimum.

## Proof Tactics

| Tactic | Purpose |
|--------|---------|
| `ring` | Expand `obj_delta` into four terms |
| `lra` | Close inequality goals after unfolding weight definitions |
| `unfold ... in *` | Make weight values visible to `lra` in both goal and hypotheses |
| `M_nondecreasing`, `Rp_nondecreasing` | Establish non-negative pressure deltas |
| `Q_decreasing_below_peak` | Establish non-positive queue delta below peak |

## Rocq Artifacts Reference

| Name | Kind | Statement |
|------|------|-----------|
| `w_tp` | Definition | 1 |
| `w_qd` | Definition | 1/2 |
| `w_mp` | Definition | 5 |
| `w_rss` | Definition | 8 |
| `WorkPoolSignals` | Record | Pressure and queue signals plus proof fields |
| `M_nonneg`, `M_upper`, `M_nondecreasing` | Fields | Memory pressure properties |
| `Rp_nonneg`, `Rp_upper`, `Rp_nondecreasing` | Fields | RSS pressure properties |
| `Q_nonneg`, `Q_zero_when_sufficient`, `Q_decreasing_below_peak` | Fields | Queue depth properties |
| `objective` | Definition | J(N) = −w_tp·T + w_qd·Q + w_mp·M + w_rss·R |
| `obj_delta` | Definition | ΔJ = J(N+1) − J(N) |
| `obj_delta_expand` | Lemma | ΔJ decomposition into four terms |
| `obj_scale_down_past_peak` | Theorem | N ≥ N★ + pressure → ΔJ ≥ 0 |
| `obj_scale_up_below_peak` | Theorem | N+1 ≤ N★, no pressure, Q > 0 → ΔJ < 0 |
| `obj_hold_at_optimum` | Lemma | All signals zero → ΔJ = −w_tp·ΔT |
