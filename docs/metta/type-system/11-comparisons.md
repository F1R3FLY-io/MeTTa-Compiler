# Type System Comparisons

## Abstract

Comparison of MeTTa's type system with other typed languages.

## Comparison Table

| Feature | MeTTa | Haskell | TypeScript | Python+mypy | Idris |
|---------|-------|---------|------------|-------------|-------|
| **Checking Time** | Runtime | Compile | Compile | Static Analysis | Compile |
| **Optional Types** | Yes | No | Yes | Yes | No |
| **Gradual Typing** | Yes | No | Yes | Yes | No |
| **Dependent Types** | Yes | No | No | No | Yes |
| **Type Inference** | Yes | Yes | Yes | Partial | Yes |
| **Higher-Kinded** | Yes | Yes | No | No | Yes |
| **Soundness** | Partial | Yes | No | Partial | Yes |

## Haskell

**Similarities**:
- Strong type system
- Higher-kinded types
- Polymorphism

**Differences**:
- Haskell: Compile-time checking, MeTTa: Runtime
- Haskell: No dependent types, MeTTa: Yes
- Haskell: Required types, MeTTa: Optional

## TypeScript

**Similarities**:
- Gradual typing
- Optional type annotations
- Flexible escape hatches

**Differences**:
- TypeScript: Structural typing, MeTTa: Nominal + structural
- TypeScript: No dependent types, MeTTa: Yes
- TypeScript: Compile-time, MeTTa: Runtime

## Idris

**Similarities**:
- Dependent types
- Type-level computation
- Precise specifications

**Differences**:
- Idris: Totality checking, MeTTa: No
- Idris: Required types, MeTTa: Optional
- Idris: Compile-time proofs, MeTTa: Runtime

## Python with mypy

**Similarities**:
- Optional type hints
- Gradual adoption
- Dynamic by default

**Differences**:
- Python: No runtime checking (mypy), MeTTa: Runtime checking
- Python: No dependent types, MeTTa: Yes
- Python: Static analysis tool, MeTTa: Integrated

## See Also

- **§00**: Overview with quick comparisons
- **§04**: Gradual typing

---

**Version**: 1.0
**Last Updated**: 2025-11-13
