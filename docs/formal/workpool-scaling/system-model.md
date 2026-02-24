# System Model: Universal Scalability Law (USL)

## Motivation

The USL, developed by Neil Gunther, extends Amdahl's Law by adding a **coherence
penalty** term. Amdahl's Law captures the serial bottleneck (σ) that limits
speedup, but it assumes zero overhead for inter-thread coordination. In practice,
cache invalidation, lock contention, and memory bus traffic impose an O(N²) cost
that eventually causes throughput to **decrease** beyond a peak thread count. The
USL captures both effects in a single closed-form expression.

For WorkPool scaling, the USL provides two critical pieces of information:

- **N★**: the thread count that maximizes throughput (the peak)
- **Monotonicity**: below N★ throughput is increasing; above it, decreasing

The hill climber uses these properties to decide whether adding or removing a
thread is beneficial.

## Parameters

| Symbol | Rocq Name | Rust Name | Range | Physical Meaning |
|--------|-----------|-----------|-------|------------------|
| T₁ | `T1` | -- | > 0 | Single-thread throughput (evals/s) |
| σ | `sigma` | -- | (0, 1) | Serial fraction (Amdahl component) |
| κ | `kappa` | -- | > 0 | Coherence penalty coefficient |
| λ | `lambda` | -- | > 0 | Task arrival rate (evals/s) |

T₁ and λ are workload-dependent and not hardcoded in Rust; they are observed
at runtime through the EMA-smoothed throughput signal. σ and κ are implicit --
they manifest through the shape of the throughput curve that the hill climber
discovers empirically.

### Parameter Axioms (Prelude.v)

```
Axiom T1_pos    : T1 > 0.
Axiom sigma_pos : 0 < sigma.
Axiom sigma_lt_1: sigma < 1.
Axiom kappa_pos : kappa > 0.
Axiom lambda_pos: lambda > 0.
```

These are physically justified: throughput and arrival rate must be positive, the
serial fraction must be strictly between 0 and 1 (pure serial or pure parallel are
degenerate cases), and the coherence penalty must be positive (zero coherence cost
would make USL reduce to Amdahl's Law).

## USL Throughput Formula

The USL models throughput T(N) for N threads as:

```
              T₁ · N
T(N) = ─────────────────────────────
        1 + σ(N − 1) + κN(N − 1)
```

### Denominator Positivity

**Theorem** (`USL_denom_pos`): For all N ≥ 1, the denominator D(N) > 0.

*Proof sketch*. Define D(N) = 1 + σ(N − 1) + κN(N − 1). For N ≥ 1:

- The constant term 1 > 0
- σ(N − 1) ≥ 0 because σ > 0 and N − 1 ≥ 0
- κN(N − 1) ≥ 0 because κ > 0, N ≥ 1, and N − 1 ≥ 0
- Sum of a positive number and two non-negative numbers is positive

This guarantees the throughput function is well-defined for all thread counts ≥ 1.

### Boundary Value

**Lemma** (`USL_at_one`): T(1) = T₁.

At N = 1 the denominator simplifies to 1 + 0 + 0 = 1, recovering the
single-thread throughput. This serves as a sanity check on the formula.

## Derivative Analysis

To find the peak, we analyze the sign of the derivative. Applying the quotient
rule to T(N) = T₁ · N / D(N), the numerator of the derivative is:

```
T₁ · [D(N) − N · D′(N)]
```

where D′(N) = σ + κ(2N − 1). The sign-determining function is:

```
S(N) = D(N) − N · D′(N) = 1 − σ − κN²
```

**Lemma** (`USL_deriv_numerator`): For all N,

```
D(N) − N · (σ + κ(2N − 1)) = 1 − σ − κN²
```

This is proven by `ring` (algebraic simplification). The sign function S(N) is a
downward-opening parabola in N, which crosses zero exactly once for positive N.

## Peak Thread Count

### Definition

The optimal thread count (USL peak) is:

```
N★ = √((1 − σ) / κ)
```

### Key Properties

**Lemma** (`N_star_pos`): N★ > 0.

Since 1 − σ > 0 (from σ < 1) and κ > 0, the quotient (1 − σ)/κ > 0,
and its square root is positive. Proven via Coquelicot's `sqrt_lt_R0`.

**Lemma** (`N_star_sq`): N★² = (1 − σ) / κ.

The fundamental identity relating N★ to the parameters. Proven via
`sqrt_sqrt` from Coquelicot.

**Lemma** (`kappa_N_star_sq`): κ · N★² = 1 − σ.

Immediate from N★² = (1 − σ)/κ by field simplification. This identity is
the key tool for proving the peak and monotonicity theorems.

### Peak Theorem

**Theorem** (`USL_peak`): S(N★) = 0.

```
S(N★) = 1 − σ − κ · N★²
      = 1 − σ − (1 − σ)         (by kappa_N_star_sq)
      = 0
```

### Worked Example

For σ = 0.05, κ = 0.001:

```
N★ = √((1 − 0.05) / 0.001)
   = √(0.95 / 0.001)
   = √950
   ≈ 30.8 threads
```

## Monotonicity

### Increasing Below Peak

**Theorem** (`USL_increasing`): For 1 ≤ N < N★, S(N) > 0.

*Proof*. Since N < N★ and both are non-negative:

1. By `sq_strict_mono`: N² < N★² (squaring preserves strict order for non-negatives)
2. By `Rmult_lt_compat_l` with κ > 0: κN² < κN★²
3. By `kappa_N_star_sq`: κN² < 1 − σ
4. Therefore S(N) = 1 − σ − κN² > 0

A positive sign function means the derivative is positive, so throughput is
increasing.

### Decreasing Above Peak

**Theorem** (`USL_decreasing`): For N > N★, S(N) < 0.

*Proof*. Symmetric to the increasing case, with the inequality reversed.

### Monotonicity Diagram

```
T(N)
 ▲
 │        ╱╲
 │      ╱    ╲
 │    ╱        ╲
 │  ╱            ╲
 │╱                ╲───
 └──────────────────────▶ N
 1    N★
     ◀──▶ ◀─────▶
   S(N)>0  S(N)<0
  increasing decreasing
```

## Uniqueness of Peak

**Theorem** (`USL_peak_unique`): If N₁ > 0, N₂ > 0, S(N₁) = 0, and S(N₂) = 0,
then N₁ = N₂.

*Proof*.

1. From S(Nᵢ) = 0: κNᵢ² = 1 − σ for i ∈ {1, 2}
2. Dividing: κN₁² = κN₂², so N₁² = N₂² (since κ ≠ 0)
3. Factor: (N₁ − N₂)(N₁ + N₂) = 0
4. Since N₁ > 0 and N₂ > 0: N₁ + N₂ > 0
5. Therefore N₁ − N₂ = 0, i.e., N₁ = N₂

This ensures N★ is the **unique** peak, ruling out multiple local optima.

## Discrete Throughput Delta

For the composite objective, we need the discrete difference:

```
ΔT(N) = T(N + 1) − T(N)
```

**Definition** (`throughput_delta`): `T(N + 1) - T(N)`.

The sign of ΔT(N) inherits from the continuous derivative analysis:
- ΔT(N) > 0 when N + 1 ≤ N★ (throughput still increasing)
- ΔT(N) < 0 when N > N★ (throughput decreasing)

## Proof Tactics Used

| Tactic | Purpose |
|--------|---------|
| `ring` | Algebraic identities (denominator expansion, sign function) |
| `lra` | Linear real arithmetic (parameter range inequalities) |
| `sqrt_sqrt`, `sqrt_lt_R0` | Coquelicot lemmas for √ properties |
| `Rmult_lt_compat_l` | Monotonicity of multiplication by positive constant |
| `field` | Field simplification (USL_at_one, kappa_N_star_sq) |

## Rocq Artifacts Reference

| Name | Kind | Statement |
|------|------|-----------|
| `USL_denom` | Definition | D(N) = 1 + σ(N−1) + κN(N−1) |
| `USL_denom_pos` | Lemma | N ≥ 1 → D(N) > 0 |
| `USL_throughput` | Definition | T(N) = T₁·N / D(N) |
| `USL_at_one` | Lemma | T(1) = T₁ |
| `USL_deriv_numerator` | Lemma | D(N) − N·D′(N) = 1 − σ − κN² |
| `USL_sign` | Definition | S(N) = 1 − σ − κN² |
| `N_star` | Definition | N★ = √((1−σ)/κ) |
| `N_star_pos` | Lemma | N★ > 0 |
| `N_star_sq` | Lemma | N★² = (1−σ)/κ |
| `kappa_N_star_sq` | Lemma | κ·N★² = 1 − σ |
| `USL_peak` | Theorem | S(N★) = 0 |
| `sq_strict_mono` | Lemma | 0 ≤ a < b → a² < b² |
| `USL_increasing` | Theorem | 1 ≤ N < N★ → S(N) > 0 |
| `USL_decreasing` | Theorem | N > N★ → S(N) < 0 |
| `USL_peak_unique` | Theorem | N₁,N₂ > 0, S(Nᵢ)=0 → N₁ = N₂ |
| `throughput_delta` | Definition | ΔT(N) = T(N+1) − T(N) |
