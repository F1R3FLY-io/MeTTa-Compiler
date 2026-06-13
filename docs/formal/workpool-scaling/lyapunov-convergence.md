# Lyapunov Convergence Analysis

## Motivation

The gradient theorems in [objective-function.md](objective-function.md) establish
that the hill climber makes correct **directional** decisions: it scales down when
overprovisioned and scales up when underprovisioned. But do the decisions
accumulate to reach the optimum? Could the system oscillate indefinitely, or get
stuck in a limit cycle?

Lyapunov stability theory answers these questions. By constructing a function V(n)
that strictly decreases at every effective step, we prove that the system converges
to the optimum in bounded time.

## Lyapunov Function

### Definition

```
V(n) = (n − N_opt)² / 2
```

where n is the current active thread count and N_opt is the optimal thread count
(memory-constrained USL optimum).

**Rocq definition** (`V`):

```coq
Definition V (N_opt n : nat) : R :=
  let x := INR n - INR N_opt in x * x / 2.
```

### Properties

| Property | Rocq Name | Statement | Interpretation |
|----------|-----------|-----------|----------------|
| Zero at optimum | `V_zero` | V N_opt N_opt = 0 | No energy at target |
| Positive definite | `V_pos_def` | n ≠ N_opt → V N_opt n > 0 | Energy everywhere else |

**Proof of `V_pos_def`**: If n ≠ N_opt, then INR(n) ≠ INR(N_opt) (by `INR_eq`),
so x = INR(n) − INR(N_opt) ≠ 0, and x² > 0 by `nra` (nonlinear real arithmetic).

## Control Signal

The hill climber produces one of three control actions:

```coq
Inductive control : Set :=
  | Park    (* n → n − 1 *)
  | Hold    (* n → n     *)
  | Unpark  (* n → n + 1 *)
.
```

The next-state function applies the control:

```coq
Definition next_state (n : nat) (u : control) : nat :=
  match u with
  | Park   => (n - 1)%nat
  | Hold   => n
  | Unpark => (n + 1)%nat
  end.
```

This models the WorkPool's `park_one()` and `unpark_one()` operations, which
adjust the active count by exactly ±1.

## Overprovisioned Decrease

**Theorem** (`lyapunov_decrease_overprovisioned`): For n > N_opt and n ≥ 2,

```
V(n − 1) < V(n)
```

*Proof*. Set x = INR(n) − INR(N_opt). Since n > N_opt, we have x ≥ 1.

```
V(n−1) − V(n) = ((x − 1)² − x²) / 2
              = (x² − 2x + 1 − x²) / 2
              = (1 − 2x) / 2
```

Since x ≥ 1, we have 1 − 2x ≤ −1 < 0, so V(n−1) − V(n) < 0.

The Rocq proof uses `minus_INR` to rewrite INR(n−1) = INR(n) − 1, normalizes
the subtraction via `replace ... by ring`, and closes with `nra`.

## Underprovisioned Decrease

**Theorem** (`lyapunov_decrease_underprovisioned`): For n < N_opt,

```
V(n + 1) < V(n)
```

*Proof*. Set x = INR(n) − INR(N_opt). Since n < N_opt, we have x ≤ −1.

```
V(n+1) − V(n) = ((x + 1)² − x²) / 2
              = (x² + 2x + 1 − x²) / 2
              = (2x + 1) / 2
```

Since x ≤ −1, we have 2x + 1 ≤ −1 < 0, so V(n+1) − V(n) < 0.

The Rocq proof uses `plus_INR` to rewrite INR(n+1) = INR(n) + 1, and closes
with `nra`.

## Hold at Optimum

**Theorem** (`lyapunov_stable_at_optimum`):

```
V(next_state N_opt Hold) = V(N_opt)
```

When at the optimum, Hold preserves V = 0. This is trivially `reflexivity` since
`next_state N_opt Hold = N_opt`.

## Convergence Diagram

```
V(n)
 ▲
 │ ●                             V(n₀) = (n₀ − N_opt)² / 2
 │   ╲
 │     ●                         V decreases by ≥ 1/2 per step
 │       ╲
 │         ●
 │           ╲
 │             ●
 │               ╲
 │                 ● ← V = 0    at N_opt
 └──────────────────────────▶ effective steps
 n₀  n₁  n₂  n₃ ... N_opt
```

Each effective step (Park when over, Unpark when under) strictly decreases V.
Hold steps at the optimum maintain V = 0.

## Distance Metric

The natural distance between the current state and the optimum is:

```coq
Definition nat_dist (a b : nat) : nat :=
  if (a <=? b)%nat then (b - a)%nat else (a - b)%nat.
```

### Distance Lemmas

**Lemma** (`nat_dist_park`): Parking when overprovisioned reduces distance by 1:

```
n > N_opt ∧ n ≥ 2  →  nat_dist(n−1, N_opt) = nat_dist(n, N_opt) − 1
```

**Lemma** (`nat_dist_unpark`): Unparking when underprovisioned reduces distance by 1:

```
n < N_opt  →  nat_dist(n+1, N_opt) = nat_dist(n, N_opt) − 1
```

**Lemma** (`nat_dist_zero`): Distance is zero iff at optimum:

```
nat_dist(n, N_opt) = 0  ↔  n = N_opt
```

All three are proven by case analysis on `Nat.leb_spec` followed by `lia`.

## Convergence Bound

**Theorem** (`convergence_steps`): The system reaches N_opt in at most
nat_dist(n₀, N_opt) effective steps.

Each effective step (Park or Unpark) decreases the distance by exactly 1.
Starting from distance d, after d steps the distance is 0, i.e., n = N_opt.

## Worst-Case Wall-Clock Time

The worst-case time includes cooldown periods between effective steps:

```
worst_case_ticks(N_opt, n₀) = nat_dist(n₀, N_opt) × cooldown_period
```

**Definition** (`worst_case_ticks`):

```coq
Definition worst_case_ticks (N_opt n0 : nat) : nat :=
  nat_dist n0 N_opt * cooldown_period.
```

Converting to wall-clock time with the monitor interval (200ms) and cooldown
period (5 ticks):

```
worst_case_time = |n₀ − N_opt| × 5 ticks × 200 ms/tick
```

### Worked Example

Starting with 35 threads, optimal at 18 (distance = 17):

```
17 steps × 5 ticks/step × 200 ms/tick = 17 seconds
```

This is an upper bound; in practice, convergence is faster because:

1. The cooldown period may be partially elapsed from a previous action
2. Emergency override (bp ≥ 2) bypasses cooldown entirely
3. The hill climber may combine multiple improvements

## Proof Tactics Summary

| Tactic | Purpose |
|--------|---------|
| `nra` | Nonlinear real arithmetic for quadratic inequalities |
| `lia` | Linear integer arithmetic for `nat_dist` case analysis |
| `replace ... by ring` | Normalize subtraction/addition order before `set` binding |
| `minus_INR`, `plus_INR` | Rewrite INR(n±1) = INR(n) ± 1 |
| `le_INR` | Convert ≤ on nat to ≤ on R for bound derivation |
| `INR_eq` | Injectivity of INR (equal reals → equal nats) |

## Rocq Artifacts Reference

| Name | Kind | Statement |
|------|------|-----------|
| `control` | Inductive | Park \| Hold \| Unpark |
| `control_value` | Definition | Park → −1, Hold → 0, Unpark → +1 |
| `next_state` | Definition | State transition function |
| `V` | Definition | V(N_opt,n) = (INR n − INR N_opt)² / 2 |
| `V_zero` | Lemma | V(N_opt) = 0 |
| `V_pos_def` | Lemma | n ≠ N_opt → V(n) > 0 |
| `lyapunov_decrease_overprovisioned` | Theorem | n > N_opt → V(n−1) < V(n) |
| `lyapunov_decrease_underprovisioned` | Theorem | n < N_opt → V(n+1) < V(n) |
| `lyapunov_stable_at_optimum` | Theorem | V(Hold(N_opt)) = V(N_opt) |
| `nat_dist` | Definition | Absolute distance on nat |
| `nat_dist_park` | Lemma | Parking reduces distance by 1 |
| `nat_dist_unpark` | Lemma | Unparking reduces distance by 1 |
| `nat_dist_zero` | Lemma | Distance zero ↔ at optimum |
| `convergence_steps` | Theorem | Convergence in ≤ nat_dist(n₀, N_opt) steps |
| `worst_case_ticks` | Definition | Distance × cooldown_period |
