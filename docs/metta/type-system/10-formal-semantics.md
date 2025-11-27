# Formal Type Semantics

## Abstract

Formal specification of MeTTa's type system using inference rules and judgments.

## Type Judgments

### Notation

```
Γ ⊢ e : T    Expression e has type T in context Γ
Γ ⊢ T <: S   Type T is a subtype of S in context Γ
```

## Typing Rules

### Variable

```
         $x : T ∈ Γ
(VAR)    ──────────────
         Γ ⊢ $x : T
```

### Application

```
         Γ ⊢ f : T₁ → T₂    Γ ⊢ e : T₁
(APP)    ─────────────────────────────────
         Γ ⊢ (f e) : T₂
```

### Subsumption

```
         Γ ⊢ e : T    Γ ⊢ T <: S
(SUB)    ──────────────────────────
         Γ ⊢ e : S
```

### Gradual Typing

```
         Γ ⊢ e : T
(GRAD)   ────────────────────────
         Γ ⊢ e : %Undefined%
```

### Dependent Function

```
         Γ, x : T₁ ⊢ e : T₂
(ΠI)     ──────────────────────────
         Γ ⊢ (λ x. e) : (Π x:T₁. T₂)
```

## Subtyping Rules

### Reflexivity

```
(S-REFL)  ──────────
          T <: T
```

### Transitivity

```
          T <: S    S <: U
(S-TRANS) ────────────────
          T <: U
```

### %Undefined% Universal

```
(S-UNDEF) ────────────────────
          %Undefined% <: T
          
(S-UNDEF) ────────────────────
          T <: %Undefined%
```

## Soundness

**Theorem (Type Safety)**: Well-typed expressions don't produce type errors when type checking is enabled.

**Formally**:
```
If ⊢ e : T and type-check is enabled,
then evaluation of e either:
  1. Produces value v : T, or
  2. Diverges, or
  3. Produces (Error e <error-details>)
```

**Note**: MeTTa's gradual typing means full soundness doesn't hold due to `%Undefined%`.

## See Also

- **§01**: Type fundamentals
- **§02**: Type checking
- Pierce, B. C. (2002). *Types and Programming Languages*

---

**Version**: 1.0
**Last Updated**: 2025-11-13
