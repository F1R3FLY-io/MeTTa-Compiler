# Metamath Proof Verifier in MeTTa

> **Status**: Work in Progress - Requires further HE compatibility work

This example demonstrates a Metamath proof verifier implemented in MeTTa, adapted from the mmverify.py project. The verifier is **not yet fully functional** on MeTTaTron due to semantic differences between HE (Hyperon Experimental) and MeTTaTron.

## What is Metamath?

[Metamath](https://us.metamath.org/) is a formal language and associated computer program for archiving, verifying, and studying mathematical proofs. Created by Norman Dwight Megill (1950-2021), it provides a minimal but powerful foundation for formal mathematics.

Metamath databases (`.mm` files) contain:
- **Constants** (`$c`): Symbols used in the formal language
- **Variables** (`$v`): Metavariables that can be substituted
- **Floating hypotheses** (`$f`): Type declarations for variables
- **Essential hypotheses** (`$e`): Logical assumptions
- **Axioms** (`$a`): Axiomatic assertions
- **Proofs** (`$p`): Proven theorems with their proof sequences

## How the Verifier Works

The verifier implements the core mmverify.py algorithm in pure MeTTa:

1. **Stack-based proof verification**: Proofs are sequences of labels that manipulate a stack
2. **Substitution**: Floating hypotheses create substitutions from stack entries
3. **Disjoint variable checking**: Ensures variable distinctness is preserved through substitution
4. **Assertion verification**: Applies substitutions to essential hypotheses and the conclusion

### Key Components

- **`&kb` space**: Knowledge base storing constants, variables, hypotheses, and assertions
- **`&stack` space**: Proof stack for intermediate results
- **`&sp` state**: Stack pointer for proof verification

## Files

```
examples/mmverify/
├── README.md               # This file
├── mmverify-utils.metta    # Core verification logic (~510 lines)
└── demo0/
    ├── demo0.mm            # Original Metamath database
    └── verify_demo0.metta  # Verification script for demo0.mm
```

## Running the Example

```bash
# From the MeTTa-Compiler directory
./target/release/mettatron examples/mmverify/demo0/verify_demo0.metta
```

Expected output will show the verification steps and conclude with:
```
Correct proof!
```

## The demo0.mm Example

The `demo0.mm` database is the introductory example from Chapter 2 of the Metamath book. It defines:

### Constants
- `0` - zero
- `+` - addition operator
- `=` - equality
- `->` - implication
- `( )` - parentheses
- `term` - type for terms
- `wff` - type for well-formed formulas
- `|-` - provability assertion

### Axioms
- **tze**: `term 0` (zero is a term)
- **tpl**: `term ( t + r )` (sum of terms is a term)
- **weq**: `wff t = r` (equality of terms is a wff)
- **wim**: `wff ( P -> Q )` (implication is a wff)
- **a1**: `|- ( t = r -> ( t = s -> r = s ) )` (transitivity of equality)
- **a2**: `|- ( t + 0 ) = t` (right identity of addition)
- **mp**: Modus ponens inference rule

### Theorem th1
The theorem `|- t = t` (reflexivity of equality) is proved using axioms a1, a2, and modus ponens.

## MeTTaTron Compatibility

### Completed Adaptations

1. **String comparison**: Replaced Python interop (`py-dot`) with native MeTTaTron comparison:
   ```metta
   ;; Original (HE with Python interop)
   (= (string< $x $y)
      ((py-dot $x __lt__) $y))

   ;; MeTTaTron (native comparison)
   (= (string< $x $y)
      (< $x $y))
   ```

2. **Error form**: MeTTaTron now supports HE's `(Error details msg)` form in addition to its native `(error msg details)`.

3. **match-atom**: Replaced deprecated `if-decons-expr` with `chain`/`decons-atom`/`unify`.

### Known Compatibility Issues

The following HE features have semantic differences in MeTTaTron that prevent full verification:

1. **Pattern matching with `()`**: HE's `let*` allows `()` as a discard pattern, but MeTTaTron expects `Nil`:
   ```metta
   ;; HE: (() (println! "debug"))
   ;; MeTTaTron expects: ($_ (println! "debug")) or explicit Unit pattern
   ```

2. **Space operations**: Some space operations may have subtle differences in behavior.

3. **Nondeterministic evaluation**: The verifier relies heavily on HE's nondeterministic rule matching.

### Future Work

To achieve full compatibility, MeTTaTron needs:
- Support for `()` as a wildcard/discard pattern in `let*`
- Verification of space operation semantics match HE
- Testing of nondeterministic evaluation patterns

## Generating Verification Scripts

The verification scripts are generated from `.mm` files using the `mmverify.py` Python program. To generate a new verification script:

```bash
cd /path/to/mmverify.py
python mmverify.py examples/your_database.mm --metta > verify_your_database.metta
```

## Features Demonstrated

This example showcases several MeTTaTron capabilities:

- **Space operations**: `new-space`, `add-atom`, `remove-atom`, `match`
- **State operations**: `new-state`, `get-state`, `change-state!`
- **List operations**: `car-atom`, `cdr-atom`, `cons-atom`, `decons-atom`
- **Control flow**: `if`, `let`, `let*`, `case`, `unify`, `chain`
- **String operations**: `repr`, string comparison with `<`
- **Higher-order functions**: `map-atom`, `filter-atom`, `collapse`

## References

- [Metamath Home Page](https://us.metamath.org/)
- [Metamath Book](https://us.metamath.org/downloads/metamath.pdf)
- [set.mm Database](https://github.com/metamath/set.mm)
- [mmverify.py](https://github.com/david-a-wheeler/mmverify.py)
