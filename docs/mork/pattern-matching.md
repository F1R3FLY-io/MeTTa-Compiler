# Pattern Matching Implementation Guide for MORK

**Version**: 1.0
**Date**: 2025-11-13
**Target**: MeTTaTron Compiler
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Introduction](#introduction)
2. [MeTTa Pattern Matching Overview](#metta-pattern-matching-overview)
3. [MORK Pattern Matching Architecture](#mork-pattern-matching-architecture)
4. [Variable Representation Strategy](#variable-representation-strategy)
5. [Pattern Encoding](#pattern-encoding)
6. [Pattern Matching Algorithm](#pattern-matching-algorithm)
7. [Bindings Management](#bindings-management)
8. [Non-Deterministic Matching](#non-deterministic-matching)
9. [Performance Optimization](#performance-optimization)
10. [Edge Cases and Special Handling](#edge-cases-and-special-handling)
11. [Implementation Examples](#implementation-examples)
12. [Testing Strategy](#testing-strategy)
13. [Performance Benchmarks](#performance-benchmarks)

---

## Introduction

This document provides a comprehensive guide to implementing MeTTa pattern matching on top of MORK's hypergraph processing kernel. Pattern matching is fundamental to MeTTa's execution model, used for:

- **Query evaluation**: Finding atoms that match patterns
- **Rewrite rules**: Pattern-based term rewriting
- **Unification**: Bidirectional pattern matching
- **Type checking**: Matching atoms against type patterns

### Key Design Goals

1. **Efficiency**: Leverage MORK's structural sharing and prefix compression
2. **Correctness**: Handle all MeTTa pattern matching semantics
3. **Non-determinism**: Support multiple bindings for same pattern
4. **Composability**: Enable pattern composition and reuse
5. **Debuggability**: Provide clear error messages and traces

### Key Challenges

The primary challenge in implementing MeTTa pattern matching on MORK is the **variable representation mismatch**:

- **MeTTa**: Uses named variables (`$x`, `$foo`, `$_`)
- **MORK**: Uses positional De Bruijn levels (0, 1, 2, ...)

This guide presents a hybrid representation strategy that preserves MeTTa semantics while exploiting MORK's performance characteristics.

---

## MeTTa Pattern Matching Overview

### Pattern Language

MeTTa patterns are atoms with special semantics:

```metta
; Symbol pattern - matches exactly
parent

; Variable pattern - matches any atom
$x

; Anonymous variable - matches any, no binding
$_

; Expression pattern - recursive matching
(parent $x $y)

; Nested patterns
(grandparent $x $z (parent $x $y) (parent $y $z))

; Grounded patterns - custom matching logic
(: $x Number)
```

### Matching Semantics

**Symbol Matching**:
```metta
parent matches parent     ; ✓
parent matches child      ; ✗
```

**Variable Matching**:
```metta
$x matches foo           ; ✓ binds $x = foo
$x matches (bar baz)     ; ✓ binds $x = (bar baz)
$_ matches anything      ; ✓ no binding
```

**Expression Matching**:
```metta
(parent Alice Bob) matches (parent Alice Bob)
; ✓ exact match

(parent $x Bob) matches (parent Alice Bob)
; ✓ binds $x = Alice

(parent $x $x) matches (parent Alice Alice)
; ✓ binds $x = Alice

(parent $x $x) matches (parent Alice Bob)
; ✗ inconsistent bindings
```

### Binding Consistency

Variables must bind consistently within a single match:

```metta
; Pattern: (edge $x $x)
(edge A A)     ; ✓ binds $x = A
(edge A B)     ; ✗ $x cannot be both A and B
```

### Non-Deterministic Matching

A single pattern may match multiple atoms:

```metta
; Space contains:
(parent Alice Bob)
(parent Alice Carol)
(parent Dave Eve)

; Pattern: (parent Alice $x)
; Matches: $x = Bob, $x = Carol
```

### Grounded Atom Matching

Grounded atoms may have custom matching logic:

```metta
(: 42 Number)        ; ✓ type check
(: "hello" Number)   ; ✗ type mismatch
(< $x 10)            ; ✓ if $x is bound to number < 10
```

---

## MORK Pattern Matching Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     MeTTa Pattern Matcher                   │
├─────────────────────────────────────────────────────────────┤
│  PatternContext   │  BindingsSet   │  MatchIterator         │
│  (name↔level map) │  (results)     │  (lazy evaluation)     │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                      Pattern Encoder                        │
├─────────────────────────────────────────────────────────────┤
│  • MeTTa Atom → MORK Bytes                                  │
│  • Named Variables → De Bruijn Levels                       │
│  • Symbol Interning                                         │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                      MORK Query Engine                      │
├─────────────────────────────────────────────────────────────┤
│  BTMSource    │  ACTSource     │  PathMap::meet()           │
│  (byte query) │  (action query)│  (set intersection)        │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                    MORK Hypergraph Space                    │
├─────────────────────────────────────────────────────────────┤
│  PathMap<()>  │  SharedMapping │  GroundedRegistry          │
│  (atoms)      │  (symbols)     │  (grounded types)          │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

1. **Pattern Parsing**: MeTTa pattern → AST
2. **Context Building**: Extract variable names, assign De Bruijn levels
3. **Encoding**: MeTTa AST → MORK byte sequence (with NewVar/VarRef tags)
4. **Source Creation**: Byte sequence → BTMSource
5. **Query Execution**: BTMSource.meet(space) → matching paths
6. **Result Decoding**: MORK bytes → MeTTa atoms
7. **Bindings Extraction**: Reconstruct variable bindings from encoded positions
8. **Result Collection**: BindingsSet with all matches

---

## Variable Representation Strategy

### The Hybrid Approach

We use a **hybrid representation** that combines named variables (MeTTa layer) with De Bruijn levels (MORK layer):

```rust
pub struct PatternContext {
    /// Map: variable name → De Bruijn level
    name_to_level: HashMap<String, u8>,

    /// Map: De Bruijn level → variable name
    level_to_name: Vec<String>,

    /// Next available De Bruijn level
    next_level: u8,
}

impl PatternContext {
    pub fn new() -> Self {
        Self {
            name_to_level: HashMap::new(),
            level_to_name: Vec::new(),
            next_level: 0,
        }
    }

    /// Register a variable, return its De Bruijn level
    pub fn register_variable(&mut self, name: &str) -> u8 {
        if let Some(&level) = self.name_to_level.get(name) {
            // Already registered - return existing level
            level
        } else {
            // New variable - assign next level
            let level = self.next_level;
            self.name_to_level.insert(name.to_string(), level);
            self.level_to_name.push(name.to_string());
            self.next_level += 1;
            level
        }
    }

    /// Look up variable name by level
    pub fn get_name(&self, level: u8) -> Option<&str> {
        self.level_to_name.get(level as usize).map(|s| s.as_str())
    }

    /// Look up De Bruijn level by name
    pub fn get_level(&self, name: &str) -> Option<u8> {
        self.name_to_level.get(name).copied()
    }
}
```

### Example Variable Registration

```metta
; Pattern: (grandparent $x $z)
;   where grandparent($x, $z) ← parent($x, $y), parent($y, $z)

Variables encountered in order:
1. $x → level 0
2. $z → level 1
3. $y → level 2

Pattern encoding uses:
- VarRef(0) for $x
- VarRef(1) for $z
- VarRef(2) for $y
```

### Anonymous Variables

Anonymous variables (`$_`) require special handling:

**Option 1: Fresh Level Per Occurrence**
```rust
pub fn register_anonymous(&mut self) -> u8 {
    let level = self.next_level;
    // Don't add to name_to_level map
    self.level_to_name.push("$_".to_string());
    self.next_level += 1;
    level
}
```

Each `$_` gets a unique level, ensuring they don't unify with each other:

```metta
; Pattern: (edge $_ $_)
(edge A B)   ; ✓ matches (level 0 = A, level 1 = B)
```

**Option 2: Wildcard Tag**
```rust
// Add new tag to MORK encoding
const WILDCARD: u8 = 0xF5;

pub fn encode_anonymous() -> Vec<u8> {
    vec![WILDCARD]
}
```

MORK query engine treats `WILDCARD` as matching any value without binding.

**Recommendation**: Use Option 1 (fresh levels) for simpler implementation, Option 2 for better performance when many anonymous variables are used.

---

## Pattern Encoding

### Encoding Algorithm

```rust
use mork::expr::Tag;

/// Encode a MeTTa pattern atom into MORK byte sequence
pub fn encode_pattern(
    atom: &Atom,
    ctx: &mut PatternContext,
) -> Result<Vec<u8>, EncodingError> {
    match atom {
        Atom::Symbol(sym) => encode_symbol(sym),
        Atom::Variable(var) => encode_variable(var, ctx),
        Atom::Expression(expr) => encode_expression(expr, ctx),
        Atom::Grounded(g) => encode_grounded(g, ctx),
    }
}

/// Encode a symbol
fn encode_symbol(sym: &SymbolAtom) -> Result<Vec<u8>, EncodingError> {
    let name = sym.name();
    let len = name.len();

    if len == 0 {
        return Err(EncodingError::EmptySymbol);
    }

    if len <= 15 {
        // Inline symbol: 0xF0 | len, followed by UTF-8 bytes
        let mut bytes = vec![Tag::SymbolSize as u8 | (len as u8)];
        bytes.extend_from_slice(name.as_bytes());
        Ok(bytes)
    } else {
        // Interned symbol: 0xF1, followed by 4-byte symbol ID
        // (requires symbol table lookup)
        let symbol_id = intern_symbol(name)?;
        let mut bytes = vec![Tag::InternedSymbol as u8];
        bytes.extend_from_slice(&symbol_id.to_le_bytes());
        Ok(bytes)
    }
}

/// Encode a variable
fn encode_variable(
    var: &VariableAtom,
    ctx: &mut PatternContext,
) -> Result<Vec<u8>, EncodingError> {
    let name = var.name();

    if name == "$_" {
        // Anonymous variable
        let level = ctx.register_anonymous();
        Ok(vec![Tag::NewVar as u8])  // Or use WILDCARD tag
    } else {
        // Named variable
        let level = ctx.register_variable(name);

        if ctx.is_first_occurrence(name) {
            // First occurrence: NewVar
            Ok(vec![Tag::NewVar as u8])
        } else {
            // Subsequent occurrence: VarRef
            Ok(vec![Tag::VarRef as u8, level])
        }
    }
}

/// Encode an expression (recursive)
fn encode_expression(
    expr: &ExpressionAtom,
    ctx: &mut PatternContext,
) -> Result<Vec<u8>, EncodingError> {
    let children = expr.children();
    let arity = children.len();

    if arity > 255 {
        return Err(EncodingError::ArityTooLarge(arity));
    }

    let mut bytes = vec![Tag::Arity as u8, arity as u8];

    // Recursively encode each child
    for child in children {
        let child_bytes = encode_pattern(child, ctx)?;
        bytes.extend(child_bytes);
    }

    Ok(bytes)
}

/// Encode a grounded atom
fn encode_grounded(
    g: &Grounded,
    ctx: &mut PatternContext,
) -> Result<Vec<u8>, EncodingError> {
    // Get grounded type ID from registry
    let type_id = get_grounded_type_id(g)?;

    // Serialize grounded value
    let value_bytes = g.serialize()?;

    let mut bytes = vec![Tag::Grounded as u8];
    bytes.extend_from_slice(&type_id.to_le_bytes());
    bytes.extend_from_slice(&(value_bytes.len() as u32).to_le_bytes());
    bytes.extend(value_bytes);

    Ok(bytes)
}
```

### Example Encodings

**Simple Symbol Pattern**:
```metta
parent
```
```rust
encode_pattern(&atom!("parent"), &mut ctx)
// → [0xF0 | 6, b'p', b'a', b'r', b'e', b'n', b't']
// → [0xF6, b'p', b'a', b'r', b'e', b'n', b't']
```

**Variable Pattern (First Occurrence)**:
```metta
$x
```
```rust
encode_pattern(&var!("$x"), &mut ctx)
// First occurrence → NewVar
// → [0xF3]
```

**Variable Pattern (Subsequent Occurrence)**:
```metta
; Pattern: (edge $x $x)
```
```rust
let mut ctx = PatternContext::new();
// (edge ...): Arity 3
let mut bytes = vec![3];

// Symbol "edge"
bytes.extend(encode_symbol(&sym!("edge"))?);
// → [0xF4, b'e', b'd', b'g', b'e']

// $x (first occurrence): NewVar, registers level 0
bytes.extend(encode_variable(&var!("$x"), &mut ctx)?);
// → [0xF3]

// $x (second occurrence): VarRef(0)
bytes.extend(encode_variable(&var!("$x"), &mut ctx)?);
// → [0xF4, 0]

// Final: [3, 0xF4, b'e', b'd', b'g', b'e', 0xF3, 0xF4, 0]
```

**Expression Pattern**:
```metta
(parent Alice $x)
```
```rust
let mut ctx = PatternContext::new();

// Arity 3
let mut bytes = vec![3];

// Symbol "parent"
bytes.extend([0xF6, b'p', b'a', b'r', b'e', b'n', b't']);

// Symbol "Alice"
bytes.extend([0xF5, b'A', b'l', b'i', b'c', b'e']);

// Variable $x (first occurrence)
bytes.extend([0xF3]);

// Final: [3, 0xF6, ..., 0xF5, ..., 0xF3]
```

**Nested Expression Pattern**:
```metta
(grandparent $x $z)
; where grandparent is defined by rule, not a literal match
; But if matching literally:
```
```rust
// Arity 3
[3,
  // Symbol "grandparent" (11 chars)
  0xFB, b'g', b'r', b'a', b'n', b'd', b'p', b'a', b'r', b'e', b'n', b't',
  // Variable $x (first occurrence, level 0)
  0xF3,
  // Variable $z (first occurrence, level 1)
  0xF3
]
```

---

## Pattern Matching Algorithm

### Two-Phase Matching

MORK pattern matching uses a **two-phase** approach:

1. **Structural Matching**: Use MORK's `meet()` operation to find structurally compatible atoms
2. **Binding Extraction**: Decode matched atoms and extract variable bindings

### Phase 1: Structural Matching

```rust
pub fn structural_match(
    pattern: &Atom,
    space: &MorkSpace,
) -> Result<Vec<Vec<u8>>, MatchError> {
    // Encode pattern
    let mut ctx = PatternContext::new();
    let pattern_bytes = encode_pattern(pattern, &mut ctx)?;

    // Create BTM source from pattern
    let pattern_source = BTMSource::new(pattern_bytes);

    // Query space using meet operation
    let space_zipper = space.btm.read_zipper();
    let result = pattern_source.read_zipper().meet(&space_zipper, true);

    // Collect matching paths
    let mut matches = Vec::new();
    for path in result.iter_paths() {
        matches.push(path.to_vec());
    }

    Ok(matches)
}
```

**Key Insight**: MORK's `meet()` operation computes the intersection of pattern and space, yielding only atoms that are structurally compatible with the pattern.

### Phase 2: Binding Extraction

```rust
pub fn extract_bindings(
    matched_path: &[u8],
    ctx: &PatternContext,
) -> Result<Bindings, MatchError> {
    let mut bindings = Bindings::new();
    let mut pos = 0;

    // Walk pattern and matched path in parallel
    extract_bindings_recursive(
        matched_path,
        &mut pos,
        ctx,
        &mut bindings,
        0, // current De Bruijn level
    )?;

    Ok(bindings)
}

fn extract_bindings_recursive(
    bytes: &[u8],
    pos: &mut usize,
    ctx: &PatternContext,
    bindings: &mut Bindings,
    level_offset: u8,
) -> Result<(), MatchError> {
    if *pos >= bytes.len() {
        return Err(MatchError::UnexpectedEnd);
    }

    let tag = bytes[*pos];
    *pos += 1;

    match Tag::from_u8(tag)? {
        Tag::Arity => {
            let arity = bytes[*pos];
            *pos += 1;

            // Recursively extract from children
            for _ in 0..arity {
                extract_bindings_recursive(bytes, pos, ctx, bindings, level_offset)?;
            }
        }

        Tag::SymbolSize => {
            let len = (tag & 0x0F) as usize;
            *pos += len; // Skip symbol bytes
        }

        Tag::InternedSymbol => {
            *pos += 4; // Skip 4-byte symbol ID
        }

        Tag::NewVar => {
            // New variable binding - extract the actual value at this position
            // This requires decoding the value that matched here
            let value = decode_value(bytes, pos)?;
            let level = level_offset;

            if let Some(var_name) = ctx.get_name(level) {
                if var_name != "$_" {
                    bindings.add_binding(var_name, value);
                }
            }
        }

        Tag::VarRef => {
            let ref_level = bytes[*pos];
            *pos += 1;

            // Variable reference - verify consistency
            let value = decode_value(bytes, pos)?;
            let actual_level = level_offset + ref_level;

            if let Some(var_name) = ctx.get_name(actual_level) {
                if var_name != "$_" {
                    // Check consistency with existing binding
                    if let Some(existing) = bindings.get(var_name) {
                        if existing != &value {
                            return Err(MatchError::InconsistentBinding {
                                var: var_name.to_string(),
                                existing: existing.clone(),
                                new: value,
                            });
                        }
                    } else {
                        bindings.add_binding(var_name, value);
                    }
                }
            }
        }

        Tag::Grounded => {
            let type_id = u32::from_le_bytes([
                bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]
            ]);
            *pos += 4;

            let value_len = u32::from_le_bytes([
                bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]
            ]) as usize;
            *pos += 4;

            *pos += value_len; // Skip grounded value bytes
        }
    }

    Ok(())
}

fn decode_value(bytes: &[u8], pos: &mut usize) -> Result<Atom, MatchError> {
    // Decode the atom at current position
    // (same logic as extract_bindings_recursive, but constructs Atom)
    let tag = bytes[*pos];
    *pos += 1;

    match Tag::from_u8(tag)? {
        Tag::Arity => {
            let arity = bytes[*pos];
            *pos += 1;

            let mut children = Vec::with_capacity(arity as usize);
            for _ in 0..arity {
                children.push(decode_value(bytes, pos)?);
            }

            Ok(Atom::Expression(ExpressionAtom::new(children)))
        }

        Tag::SymbolSize => {
            let len = (tag & 0x0F) as usize;
            let name = std::str::from_utf8(&bytes[*pos..*pos + len])?;
            *pos += len;
            Ok(Atom::Symbol(SymbolAtom::new(name)))
        }

        Tag::InternedSymbol => {
            let symbol_id = u32::from_le_bytes([
                bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]
            ]);
            *pos += 4;

            let name = lookup_symbol(symbol_id)?;
            Ok(Atom::Symbol(SymbolAtom::new(&name)))
        }

        Tag::NewVar | Tag::VarRef => {
            // In matched data, variables are replaced by their values
            // This should not occur in well-formed matched output
            Err(MatchError::UnexpectedVariableInMatch)
        }

        Tag::Grounded => {
            let type_id = u32::from_le_bytes([
                bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]
            ]);
            *pos += 4;

            let value_len = u32::from_le_bytes([
                bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]
            ]) as usize;
            *pos += 4;

            let value_bytes = &bytes[*pos..*pos + value_len];
            *pos += value_len;

            let grounded = deserialize_grounded(type_id, value_bytes)?;
            Ok(Atom::Grounded(grounded))
        }
    }
}
```

### Complete Matching Pipeline

```rust
pub struct PatternMatcher {
    space: Arc<MorkSpace>,
}

impl PatternMatcher {
    pub fn new(space: Arc<MorkSpace>) -> Self {
        Self { space }
    }

    /// Match a pattern against the space, returning all bindings
    pub fn match_pattern(&self, pattern: &Atom) -> Result<BindingsSet, MatchError> {
        // Phase 1: Structural matching
        let mut ctx = PatternContext::new();
        let pattern_bytes = encode_pattern(pattern, &mut ctx)?;
        let pattern_source = BTMSource::new(pattern_bytes);

        let space_zipper = self.space.btm.read_zipper();
        let matches = pattern_source.read_zipper().meet(&space_zipper, true);

        // Phase 2: Binding extraction
        let mut bindings_set = BindingsSet::new();

        for path in matches.iter_paths() {
            match extract_bindings(path, &ctx) {
                Ok(bindings) => {
                    bindings_set.add(bindings);
                }
                Err(MatchError::InconsistentBinding { .. }) => {
                    // Skip inconsistent matches
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(bindings_set)
    }
}
```

---

## Bindings Management

### Bindings Data Structure

```rust
use std::collections::HashMap;

/// A single set of variable bindings
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    /// Map: variable name → bound value
    bindings: HashMap<String, Atom>,
}

impl Bindings {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Add a new binding
    pub fn add_binding(&mut self, var: &str, value: Atom) {
        self.bindings.insert(var.to_string(), value);
    }

    /// Get binding for variable
    pub fn get(&self, var: &str) -> Option<&Atom> {
        self.bindings.get(var)
    }

    /// Check if variable is bound
    pub fn is_bound(&self, var: &str) -> bool {
        self.bindings.contains_key(var)
    }

    /// Merge bindings (fails if inconsistent)
    pub fn merge(&mut self, other: &Bindings) -> Result<(), BindingError> {
        for (var, value) in &other.bindings {
            if let Some(existing) = self.bindings.get(var) {
                if existing != value {
                    return Err(BindingError::InconsistentMerge {
                        var: var.clone(),
                        existing: existing.clone(),
                        new: value.clone(),
                    });
                }
            } else {
                self.bindings.insert(var.clone(), value.clone());
            }
        }
        Ok(())
    }

    /// Apply bindings to an atom (substitute variables)
    pub fn apply(&self, atom: &Atom) -> Atom {
        match atom {
            Atom::Variable(var) => {
                if let Some(value) = self.get(var.name()) {
                    value.clone()
                } else {
                    atom.clone()
                }
            }
            Atom::Expression(expr) => {
                let children: Vec<Atom> = expr.children()
                    .iter()
                    .map(|child| self.apply(child))
                    .collect();
                Atom::Expression(ExpressionAtom::new(children))
            }
            _ => atom.clone(),
        }
    }

    /// Iterate over bindings
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Atom)> {
        self.bindings.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of bindings
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}
```

### BindingsSet for Non-Determinism

```rust
/// A set of alternative bindings (for non-deterministic matches)
#[derive(Debug, Clone)]
pub struct BindingsSet {
    /// All possible binding alternatives
    alternatives: Vec<Bindings>,
}

impl BindingsSet {
    pub fn new() -> Self {
        Self {
            alternatives: Vec::new(),
        }
    }

    /// Create from single bindings
    pub fn from_single(bindings: Bindings) -> Self {
        Self {
            alternatives: vec![bindings],
        }
    }

    /// Add alternative bindings
    pub fn add(&mut self, bindings: Bindings) {
        self.alternatives.push(bindings);
    }

    /// Get all alternatives
    pub fn alternatives(&self) -> &[Bindings] {
        &self.alternatives
    }

    /// Number of alternatives
    pub fn len(&self) -> usize {
        self.alternatives.len()
    }

    /// Check if empty (no matches)
    pub fn is_empty(&self) -> bool {
        self.alternatives.is_empty()
    }

    /// Cartesian product with another BindingsSet
    pub fn product(&self, other: &BindingsSet) -> Result<BindingsSet, BindingError> {
        let mut result = BindingsSet::new();

        for b1 in &self.alternatives {
            for b2 in &other.alternatives {
                let mut merged = b1.clone();
                merged.merge(b2)?;
                result.add(merged);
            }
        }

        Ok(result)
    }

    /// Union (concatenate alternatives)
    pub fn union(&mut self, other: BindingsSet) {
        self.alternatives.extend(other.alternatives);
    }

    /// Filter alternatives by predicate
    pub fn filter<F>(&mut self, predicate: F)
    where
        F: Fn(&Bindings) -> bool,
    {
        self.alternatives.retain(predicate);
    }

    /// Map over alternatives
    pub fn map<F>(&self, f: F) -> BindingsSet
    where
        F: Fn(&Bindings) -> Bindings,
    {
        BindingsSet {
            alternatives: self.alternatives.iter().map(f).collect(),
        }
    }
}

impl IntoIterator for BindingsSet {
    type Item = Bindings;
    type IntoIter = std::vec::IntoIter<Bindings>;

    fn into_iter(self) -> Self::IntoIter {
        self.alternatives.into_iter()
    }
}
```

### Binding Application Example

```rust
// Pattern: (parent $x $y)
// Bindings: {$x → Alice, $y → Bob}

let pattern = expr!(sym!("parent"), var!("$x"), var!("$y"));
let mut bindings = Bindings::new();
bindings.add_binding("$x", atom!("Alice"));
bindings.add_binding("$y", atom!("Bob"));

let result = bindings.apply(&pattern);
// Result: (parent Alice Bob)
```

---

## Non-Deterministic Matching

### Multiple Matches Example

```metta
; Space:
(parent Alice Bob)
(parent Alice Carol)
(parent Dave Eve)

; Pattern:
(parent Alice $x)

; Expected matches:
; 1. {$x → Bob}
; 2. {$x → Carol}
```

```rust
let pattern = expr!(sym!("parent"), sym!("Alice"), var!("$x"));
let matcher = PatternMatcher::new(space);
let bindings_set = matcher.match_pattern(&pattern)?;

assert_eq!(bindings_set.len(), 2);

for bindings in bindings_set.alternatives() {
    let x = bindings.get("$x").unwrap();
    // x is either "Bob" or "Carol"
}
```

### Cartesian Product for Conjunctions

```metta
; Pattern: both conditions must hold
(and (parent $x Bob) (parent Alice $x))

; Space:
(parent Alice Bob)
(parent Dave Bob)
(parent Alice Carol)

; Match process:
; 1. Match (parent $x Bob)     → {$x → Alice}, {$x → Dave}
; 2. Match (parent Alice $x)   → {$x → Bob}, {$x → Carol}
; 3. Cartesian product and merge:
;    - {$x → Alice} ∪ {$x → Bob}    → inconsistent (skip)
;    - {$x → Alice} ∪ {$x → Carol}  → inconsistent (skip)
;    - {$x → Dave} ∪ {$x → Bob}     → inconsistent (skip)
;    - {$x → Dave} ∪ {$x → Carol}   → inconsistent (skip)
; Result: No matches
```

```rust
pub fn match_conjunction(
    patterns: &[Atom],
    space: &MorkSpace,
) -> Result<BindingsSet, MatchError> {
    let matcher = PatternMatcher::new(Arc::new(space.clone()));

    let mut result = BindingsSet::from_single(Bindings::new());

    for pattern in patterns {
        let pattern_bindings = matcher.match_pattern(pattern)?;
        result = result.product(&pattern_bindings)?;
    }

    Ok(result)
}
```

### Union for Disjunctions

```metta
; Pattern: either condition holds
(or (parent Alice $x) (parent $x Bob))

; Space:
(parent Alice Bob)
(parent Alice Carol)
(parent Dave Bob)

; Match process:
; 1. Match (parent Alice $x)   → {$x → Bob}, {$x → Carol}
; 2. Match (parent $x Bob)     → {$x → Alice}, {$x → Dave}
; 3. Union:
;    Result: {$x → Bob}, {$x → Carol}, {$x → Alice}, {$x → Dave}
```

```rust
pub fn match_disjunction(
    patterns: &[Atom],
    space: &MorkSpace,
) -> Result<BindingsSet, MatchError> {
    let matcher = PatternMatcher::new(Arc::new(space.clone()));

    let mut result = BindingsSet::new();

    for pattern in patterns {
        let pattern_bindings = matcher.match_pattern(pattern)?;
        result.union(pattern_bindings);
    }

    Ok(result)
}
```

---

## Performance Optimization

### Lazy Evaluation

For large result sets, use lazy evaluation to avoid materializing all matches:

```rust
pub struct MatchIterator<'a> {
    paths: PathIterator<'a>,
    ctx: PatternContext,
}

impl<'a> Iterator for MatchIterator<'a> {
    type Item = Result<Bindings, MatchError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.paths.next().map(|path| {
            extract_bindings(path, &self.ctx)
        })
    }
}

impl PatternMatcher {
    pub fn match_pattern_lazy(&self, pattern: &Atom) -> Result<MatchIterator, MatchError> {
        let mut ctx = PatternContext::new();
        let pattern_bytes = encode_pattern(pattern, &mut ctx)?;
        let pattern_source = BTMSource::new(pattern_bytes);

        let space_zipper = self.space.btm.read_zipper();
        let matches = pattern_source.read_zipper().meet(&space_zipper, true);

        Ok(MatchIterator {
            paths: matches.iter_paths(),
            ctx,
        })
    }
}
```

### Prefix Optimization

MORK's prefix compression means patterns with common prefixes are very efficient:

```metta
; These patterns share prefix and benefit from structural sharing:
(parent Alice $x)
(parent Alice Bob)
(parent Alice Carol)

; MORK representation:
parent/Alice/Bob
parent/Alice/Carol
          ^^^^ shared prefix
```

### Batched Queries

For multiple related queries, batch them:

```rust
pub fn match_patterns_batched(
    patterns: &[Atom],
    space: &MorkSpace,
) -> Result<Vec<BindingsSet>, MatchError> {
    let matcher = PatternMatcher::new(Arc::new(space.clone()));

    // Encode all patterns first (reuse symbol interning)
    let encoded: Vec<_> = patterns.iter()
        .map(|p| {
            let mut ctx = PatternContext::new();
            encode_pattern(p, &mut ctx).map(|bytes| (bytes, ctx))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Execute queries in parallel (if enabled)
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        encoded.par_iter()
            .map(|(bytes, ctx)| {
                let source = BTMSource::new(bytes.clone());
                let matches = source.read_zipper().meet(&space.btm.read_zipper(), true);

                let mut bindings_set = BindingsSet::new();
                for path in matches.iter_paths() {
                    if let Ok(bindings) = extract_bindings(path, ctx) {
                        bindings_set.add(bindings);
                    }
                }
                Ok(bindings_set)
            })
            .collect()
    }

    #[cfg(not(feature = "parallel"))]
    {
        encoded.iter()
            .map(|(bytes, ctx)| {
                let source = BTMSource::new(bytes.clone());
                let matches = source.read_zipper().meet(&space.btm.read_zipper(), true);

                let mut bindings_set = BindingsSet::new();
                for path in matches.iter_paths() {
                    if let Ok(bindings) = extract_bindings(path, ctx) {
                        bindings_set.add(bindings);
                    }
                }
                Ok(bindings_set)
            })
            .collect()
    }
}
```

### Grounded Atom Caching

Grounded atoms with expensive matching logic should be cached:

```rust
use std::collections::HashMap;
use std::sync::RwLock;

pub struct GroundedMatchCache {
    cache: RwLock<HashMap<(u32, Vec<u8>), bool>>,
}

impl GroundedMatchCache {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_or_compute<F>(
        &self,
        type_id: u32,
        value: &[u8],
        compute: F,
    ) -> bool
    where
        F: FnOnce() -> bool,
    {
        let key = (type_id, value.to_vec());

        // Try read lock first
        if let Some(&result) = self.cache.read().unwrap().get(&key) {
            return result;
        }

        // Compute and cache
        let result = compute();
        self.cache.write().unwrap().insert(key, result);
        result
    }
}
```

### Hardware-Specific Optimizations

**For Intel Xeon E5-2699 v3 (36 cores, 72 threads)**:

```rust
// Configure thread pool for pattern matching
pub fn configure_pattern_matching() {
    #[cfg(feature = "parallel")]
    {
        use rayon::ThreadPoolBuilder;

        // Use physical cores only for better cache locality
        ThreadPoolBuilder::new()
            .num_threads(36)
            .build_global()
            .unwrap();
    }
}
```

**NUMA-aware allocation**:
```rust
// Pin threads to NUMA nodes for better memory locality
pub fn set_numa_affinity(thread_id: usize) {
    #[cfg(target_os = "linux")]
    {
        use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};
        use std::mem;

        unsafe {
            let mut cpuset: cpu_set_t = mem::zeroed();
            CPU_ZERO(&mut cpuset);

            // Distribute threads across NUMA nodes
            // 36 cores: 0-8 (node 1), 9-17 (node 2), 18-26 (node 3), 27-35 (node 4)
            let core = thread_id % 36;
            CPU_SET(core, &mut cpuset);

            sched_setaffinity(0, mem::size_of::<cpu_set_t>(), &cpuset);
        }
    }
}
```

---

## Edge Cases and Special Handling

### Empty Expressions

```metta
; Pattern: ()
; Should match: only ()
```

```rust
// Encoding: [0] (arity 0)
pub fn encode_empty_expression() -> Vec<u8> {
    vec![Tag::Arity as u8, 0]
}
```

### Variable-Only Patterns

```metta
; Pattern: $x
; Should match: any atom
```

```rust
// Encoding: [0xF3] (NewVar)
// Matches: all atoms in space
pub fn match_any(space: &MorkSpace) -> Result<Vec<Atom>, MatchError> {
    // Special case: iterate all atoms
    let all_paths = space.btm.iter_paths();
    all_paths.map(|path| decode_atom(path)).collect()
}
```

### Deeply Nested Expressions

```metta
; Pattern: (a (b (c (d (e ...)))))
```

**Issue**: Deep recursion may overflow stack.

**Solution**: Use iterative encoding with explicit stack:

```rust
pub fn encode_pattern_iterative(atom: &Atom) -> Result<Vec<u8>, EncodingError> {
    let mut ctx = PatternContext::new();
    let mut bytes = Vec::new();
    let mut stack = vec![atom];

    while let Some(current) = stack.pop() {
        match current {
            Atom::Symbol(sym) => {
                bytes.extend(encode_symbol(sym)?);
            }
            Atom::Variable(var) => {
                bytes.extend(encode_variable(var, &mut ctx)?);
            }
            Atom::Expression(expr) => {
                let children = expr.children();
                bytes.push(Tag::Arity as u8);
                bytes.push(children.len() as u8);

                // Push children in reverse order
                for child in children.iter().rev() {
                    stack.push(child);
                }
            }
            Atom::Grounded(g) => {
                bytes.extend(encode_grounded(g, &mut ctx)?);
            }
        }
    }

    Ok(bytes)
}
```

### Circular References (Grounded Atoms)

Grounded atoms may contain circular references:

```rust
pub struct GraphNode {
    id: u32,
    neighbors: Arc<Vec<u32>>,  // May reference self
}
```

**Solution**: Use reference IDs instead of inline encoding:

```rust
pub fn encode_grounded_with_refs(
    g: &Grounded,
    ref_map: &mut HashMap<usize, u32>,
) -> Result<Vec<u8>, EncodingError> {
    let ptr = g as *const _ as usize;

    if let Some(&ref_id) = ref_map.get(&ptr) {
        // Already encoded - use reference
        let mut bytes = vec![Tag::GroundedRef as u8];
        bytes.extend_from_slice(&ref_id.to_le_bytes());
        Ok(bytes)
    } else {
        // First occurrence - assign ID and encode
        let ref_id = ref_map.len() as u32;
        ref_map.insert(ptr, ref_id);

        let mut bytes = vec![Tag::Grounded as u8];
        bytes.extend_from_slice(&ref_id.to_le_bytes());
        bytes.extend(g.serialize()?);
        Ok(bytes)
    }
}
```

### Unicode Symbols

```metta
; Pattern: (Σ α β)  ; Greek letters
```

```rust
// UTF-8 encoding handles Unicode correctly
pub fn encode_unicode_symbol(name: &str) -> Vec<u8> {
    let utf8_bytes = name.as_bytes();
    let len = utf8_bytes.len();

    if len <= 15 {
        let mut bytes = vec![Tag::SymbolSize as u8 | (len as u8)];
        bytes.extend_from_slice(utf8_bytes);
        bytes
    } else {
        // Intern long Unicode symbols
        let symbol_id = intern_symbol(name).unwrap();
        let mut bytes = vec![Tag::InternedSymbol as u8];
        bytes.extend_from_slice(&symbol_id.to_le_bytes());
        bytes
    }
}
```

**Test Case**:
```rust
#[test]
fn test_unicode_patterns() {
    let pattern = expr!(sym!("Σ"), var!("α"), var!("β"));
    let mut ctx = PatternContext::new();
    let encoded = encode_pattern(&pattern, &mut ctx).unwrap();

    // Verify correct UTF-8 encoding
    assert!(String::from_utf8(encoded.clone()).is_ok());
}
```

### Variable Shadowing

```metta
; Pattern: ((λ $x $x) $x)
; Inner $x should not shadow outer $x in pattern matching context
```

**Issue**: MeTTa uses lexical scoping, MORK uses De Bruijn levels.

**Solution**: Maintain scope stack during encoding:

```rust
pub struct ScopedContext {
    scopes: Vec<HashMap<String, u8>>,
    level_to_name: Vec<String>,
    next_level: u8,
}

impl ScopedContext {
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn register_variable(&mut self, name: &str) -> u8 {
        // Check current scope first
        if let Some(scope) = self.scopes.last_mut() {
            if let Some(&level) = scope.get(name) {
                return level;
            }
        }

        // Check outer scopes
        for scope in self.scopes.iter().rev().skip(1) {
            if let Some(&level) = scope.get(name) {
                return level;
            }
        }

        // New variable - register in current scope
        let level = self.next_level;
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), level);
        }
        self.level_to_name.push(name.to_string());
        self.next_level += 1;
        level
    }
}
```

---

## Implementation Examples

### Complete Pattern Matching Example

```rust
use mork_pattern::{PatternMatcher, PatternContext, encode_pattern};
use mork_space::MorkSpace;
use metta_atom::{Atom, atom, expr, sym, var};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create space
    let mut space = MorkSpace::new();

    // Add facts
    space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Bob")))?;
    space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Carol")))?;
    space.add(&expr!(sym!("parent"), sym!("Dave"), sym!("Eve")))?;

    // Create pattern
    let pattern = expr!(sym!("parent"), sym!("Alice"), var!("$x"));

    // Match pattern
    let matcher = PatternMatcher::new(Arc::new(space));
    let bindings_set = matcher.match_pattern(&pattern)?;

    // Print results
    println!("Matches: {}", bindings_set.len());
    for (i, bindings) in bindings_set.alternatives().iter().enumerate() {
        println!("  {}:", i + 1);
        for (var, value) in bindings.iter() {
            println!("    {} = {}", var, value);
        }
    }

    // Expected output:
    // Matches: 2
    //   1:
    //     $x = Bob
    //   2:
    //     $x = Carol

    Ok(())
}
```

### Recursive Pattern Example

```rust
// Pattern: (grandparent $x $z) defined by rule
// Rule: (grandparent $x $z) ← (parent $x $y) ∧ (parent $y $z)

pub fn find_grandparents(
    space: &MorkSpace,
    x: Option<&str>,
    z: Option<&str>,
) -> Result<BindingsSet, MatchError> {
    // Build patterns based on provided arguments
    let pattern1 = match x {
        Some(name) => expr!(sym!("parent"), sym!(name), var!("$y")),
        None => expr!(sym!("parent"), var!("$x"), var!("$y")),
    };

    let pattern2 = match z {
        Some(name) => expr!(sym!("parent"), var!("$y"), sym!(name)),
        None => expr!(sym!("parent"), var!("$y"), var!("$z")),
    };

    // Match both patterns and compute Cartesian product
    let matcher = PatternMatcher::new(Arc::new(space.clone()));
    let bindings1 = matcher.match_pattern(&pattern1)?;
    let bindings2 = matcher.match_pattern(&pattern2)?;

    let result = bindings1.product(&bindings2)?;
    Ok(result)
}

// Usage:
let space = create_family_tree();
let grandparents = find_grandparents(&space, Some("Alice"), None)?;

for bindings in grandparents.alternatives() {
    let y = bindings.get("$y").unwrap();
    let z = bindings.get("$z").unwrap();
    println!("Alice → {} → {}", y, z);
}
```

### Type Pattern Matching

```rust
pub fn match_type_pattern(
    atom: &Atom,
    type_pattern: &Atom,
    space: &MorkSpace,
) -> Result<bool, MatchError> {
    // Type pattern: (: <value> <type>)
    match type_pattern {
        Atom::Expression(expr) if expr.children().len() == 3 => {
            let type_op = &expr.children()[0];
            let value_pattern = &expr.children()[1];
            let type_atom = &expr.children()[2];

            if let Atom::Symbol(sym) = type_op {
                if sym.name() == ":" {
                    // Match value against pattern
                    let matcher = PatternMatcher::new(Arc::new(space.clone()));
                    let bindings_set = matcher.match_pattern(value_pattern)?;

                    if bindings_set.is_empty() {
                        return Ok(false);
                    }

                    // Check type constraint
                    for bindings in bindings_set.alternatives() {
                        let instantiated = bindings.apply(value_pattern);
                        if !check_type(&instantiated, type_atom, space)? {
                            return Ok(false);
                        }
                    }

                    return Ok(true);
                }
            }
        }
        _ => {}
    }

    Err(MatchError::InvalidTypePattern)
}

fn check_type(
    value: &Atom,
    type_atom: &Atom,
    space: &MorkSpace,
) -> Result<bool, MatchError> {
    // Query space for type assertions
    let type_query = expr!(sym!(":"), value.clone(), type_atom.clone());
    let matcher = PatternMatcher::new(Arc::new(space.clone()));
    let matches = matcher.match_pattern(&type_query)?;

    Ok(!matches.is_empty())
}
```

---

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_matching() {
        let mut space = MorkSpace::new();
        space.add(&atom!("foo")).unwrap();
        space.add(&atom!("bar")).unwrap();

        let pattern = atom!("foo");
        let matcher = PatternMatcher::new(Arc::new(space));
        let matches = matcher.match_pattern(&pattern).unwrap();

        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_variable_matching() {
        let mut space = MorkSpace::new();
        space.add(&atom!("foo")).unwrap();
        space.add(&atom!("bar")).unwrap();

        let pattern = var!("$x");
        let matcher = PatternMatcher::new(Arc::new(space));
        let matches = matcher.match_pattern(&pattern).unwrap();

        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_expression_matching() {
        let mut space = MorkSpace::new();
        space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Bob"))).unwrap();
        space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Carol"))).unwrap();

        let pattern = expr!(sym!("parent"), sym!("Alice"), var!("$x"));
        let matcher = PatternMatcher::new(Arc::new(space));
        let matches = matcher.match_pattern(&pattern).unwrap();

        assert_eq!(matches.len(), 2);

        for bindings in matches.alternatives() {
            let x = bindings.get("$x").unwrap();
            assert!(x == &atom!("Bob") || x == &atom!("Carol"));
        }
    }

    #[test]
    fn test_consistent_binding() {
        let mut space = MorkSpace::new();
        space.add(&expr!(sym!("edge"), sym!("A"), sym!("A"))).unwrap();
        space.add(&expr!(sym!("edge"), sym!("A"), sym!("B"))).unwrap();

        let pattern = expr!(sym!("edge"), var!("$x"), var!("$x"));
        let matcher = PatternMatcher::new(Arc::new(space));
        let matches = matcher.match_pattern(&pattern).unwrap();

        assert_eq!(matches.len(), 1);

        let bindings = &matches.alternatives()[0];
        assert_eq!(bindings.get("$x").unwrap(), &atom!("A"));
    }

    #[test]
    fn test_anonymous_variable() {
        let mut space = MorkSpace::new();
        space.add(&expr!(sym!("edge"), sym!("A"), sym!("B"))).unwrap();
        space.add(&expr!(sym!("edge"), sym!("C"), sym!("D"))).unwrap();

        let pattern = expr!(sym!("edge"), var!("$_"), var!("$_"));
        let matcher = PatternMatcher::new(Arc::new(space));
        let matches = matcher.match_pattern(&pattern).unwrap();

        assert_eq!(matches.len(), 2);

        // Anonymous variables should not appear in bindings
        for bindings in matches.alternatives() {
            assert!(!bindings.is_bound("$_"));
        }
    }

    #[test]
    fn test_nested_expression() {
        let mut space = MorkSpace::new();
        space.add(&expr!(
            sym!("edge"),
            expr!(sym!("node"), sym!("A")),
            expr!(sym!("node"), sym!("B"))
        )).unwrap();

        let pattern = expr!(
            sym!("edge"),
            expr!(sym!("node"), var!("$x")),
            expr!(sym!("node"), var!("$y"))
        );

        let matcher = PatternMatcher::new(Arc::new(space));
        let matches = matcher.match_pattern(&pattern).unwrap();

        assert_eq!(matches.len(), 1);

        let bindings = &matches.alternatives()[0];
        assert_eq!(bindings.get("$x").unwrap(), &atom!("A"));
        assert_eq!(bindings.get("$y").unwrap(), &atom!("B"));
    }
}
```

### Property-Based Tests

```rust
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_symbol() -> impl Strategy<Value = Atom> {
        "[a-z]{1,10}".prop_map(|s| atom!(s))
    }

    fn arb_atom(depth: u32) -> impl Strategy<Value = Atom> {
        let leaf = prop_oneof![
            arb_symbol(),
            Just(var!("$x")),
            Just(var!("$y")),
        ];

        leaf.prop_recursive(depth, 256, 10, |inner| {
            prop::collection::vec(inner, 0..5)
                .prop_map(|children| {
                    if children.is_empty() {
                        atom!("empty")
                    } else {
                        Atom::Expression(ExpressionAtom::new(children))
                    }
                })
        })
    }

    proptest! {
        #[test]
        fn test_roundtrip_encoding(atom in arb_atom(3)) {
            let mut ctx = PatternContext::new();
            let encoded = encode_pattern(&atom, &mut ctx).unwrap();
            let decoded = decode_atom(&encoded).unwrap();

            // Note: Variables are normalized to De Bruijn levels,
            // so exact equality may not hold. Check structural equivalence.
            assert_eq!(atom.children().len(), decoded.children().len());
        }

        #[test]
        fn test_match_is_subset(atom in arb_atom(2)) {
            let mut space = MorkSpace::new();
            space.add(&atom).unwrap();

            // Pattern with variable should match
            let pattern = var!("$x");
            let matcher = PatternMatcher::new(Arc::new(space));
            let matches = matcher.match_pattern(&pattern).unwrap();

            assert!(!matches.is_empty());
        }
    }
}
```

### Integration Tests

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_family_tree() {
        let mut space = MorkSpace::new();

        // Add facts
        space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Bob"))).unwrap();
        space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Carol"))).unwrap();
        space.add(&expr!(sym!("parent"), sym!("Bob"), sym!("Dave"))).unwrap();
        space.add(&expr!(sym!("parent"), sym!("Bob"), sym!("Eve"))).unwrap();

        // Query: grandchildren of Alice
        let grandchildren = find_grandparents(&space, Some("Alice"), None).unwrap();

        assert_eq!(grandchildren.len(), 2);

        let names: Vec<_> = grandchildren.alternatives()
            .iter()
            .map(|b| b.get("$z").unwrap().clone())
            .collect();

        assert!(names.contains(&atom!("Dave")));
        assert!(names.contains(&atom!("Eve")));
    }

    #[test]
    fn test_transitive_closure() {
        let mut space = MorkSpace::new();

        // Add edges: A→B, B→C, C→D
        space.add(&expr!(sym!("edge"), sym!("A"), sym!("B"))).unwrap();
        space.add(&expr!(sym!("edge"), sym!("B"), sym!("C"))).unwrap();
        space.add(&expr!(sym!("edge"), sym!("C"), sym!("D"))).unwrap();

        // Compute transitive closure (up to 3 hops)
        let reachable = compute_reachable(&space, "A", 3).unwrap();

        assert_eq!(reachable.len(), 3);  // B, C, D
        assert!(reachable.contains(&atom!("B")));
        assert!(reachable.contains(&atom!("C")));
        assert!(reachable.contains(&atom!("D")));
    }
}

fn compute_reachable(
    space: &MorkSpace,
    start: &str,
    max_hops: usize,
) -> Result<Vec<Atom>, MatchError> {
    let mut visited = HashSet::new();
    let mut frontier = vec![atom!(start)];

    for _ in 0..max_hops {
        let mut next_frontier = Vec::new();

        for node in &frontier {
            if visited.contains(node) {
                continue;
            }
            visited.insert(node.clone());

            // Find successors
            let pattern = expr!(sym!("edge"), node.clone(), var!("$next"));
            let matcher = PatternMatcher::new(Arc::new(space.clone()));
            let matches = matcher.match_pattern(&pattern)?;

            for bindings in matches.alternatives() {
                if let Some(next) = bindings.get("$next") {
                    next_frontier.push(next.clone());
                }
            }
        }

        frontier = next_frontier;
        if frontier.is_empty() {
            break;
        }
    }

    visited.remove(&atom!(start));
    Ok(visited.into_iter().collect())
}
```

---

## Performance Benchmarks

### Benchmark Suite

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_pattern_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_matching");

    // Vary space size
    for size in [100, 1_000, 10_000, 100_000].iter() {
        let mut space = MorkSpace::new();

        for i in 0..*size {
            space.add(&expr!(
                sym!("fact"),
                sym!(format!("arg_{}", i)),
                sym!(format!("val_{}", i % 100))
            )).unwrap();
        }

        let pattern = expr!(sym!("fact"), var!("$x"), sym!("val_42"));

        group.bench_with_input(
            BenchmarkId::new("simple_pattern", size),
            &space,
            |b, space| {
                let matcher = PatternMatcher::new(Arc::new(space.clone()));
                b.iter(|| {
                    let matches = matcher.match_pattern(black_box(&pattern)).unwrap();
                    black_box(matches.len())
                });
            },
        );
    }

    group.finish();
}

fn bench_variable_binding(c: &mut Criterion) {
    let mut group = c.benchmark_group("variable_binding");

    let mut space = MorkSpace::new();
    for i in 0..1000 {
        space.add(&expr!(
            sym!("triple"),
            sym!(format!("s_{}", i)),
            sym!("pred"),
            sym!(format!("o_{}", i))
        )).unwrap();
    }

    let pattern = expr!(sym!("triple"), var!("$s"), sym!("pred"), var!("$o"));

    group.bench_function("extract_bindings", |b| {
        let matcher = PatternMatcher::new(Arc::new(space.clone()));
        b.iter(|| {
            let matches = matcher.match_pattern(black_box(&pattern)).unwrap();
            black_box(matches.len())
        });
    });

    group.finish();
}

fn bench_nested_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_patterns");

    for depth in [1, 2, 3, 4, 5].iter() {
        let mut space = MorkSpace::new();

        // Create deeply nested expressions
        let mut expr = atom!("leaf");
        for i in 0..*depth {
            expr = Atom::Expression(ExpressionAtom::new(vec![
                sym!(format!("level_{}", i)),
                expr,
            ]));
        }
        space.add(&expr).unwrap();

        // Pattern with variables at each level
        let mut pattern = var!("$leaf");
        for i in (0..*depth).rev() {
            pattern = Atom::Expression(ExpressionAtom::new(vec![
                sym!(format!("level_{}", i)),
                pattern,
            ]));
        }

        group.bench_with_input(
            BenchmarkId::new("depth", depth),
            &space,
            |b, space| {
                let matcher = PatternMatcher::new(Arc::new(space.clone()));
                b.iter(|| {
                    let matches = matcher.match_pattern(black_box(&pattern)).unwrap();
                    black_box(matches.len())
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_pattern_matching,
    bench_variable_binding,
    bench_nested_patterns
);
criterion_main!(benches);
```

### Expected Performance Characteristics

Based on MORK's design and the Intel Xeon E5-2699 v3 hardware:

| Operation | Complexity | Expected Performance (1M atoms) |
|-----------|------------|--------------------------------|
| Simple symbol match | O(log N) | ~100 ns |
| Variable match (any) | O(N) | ~1-10 ms |
| Expression match (depth 3) | O(N × D) | ~10-50 ms |
| Consistent binding check | O(V) | ~10-100 ns per binding |
| Bindings merge | O(V) | ~100 ns per variable |

**Where**:
- N = number of atoms in space
- D = depth of expression
- V = number of variables

### Profiling Commands

```bash
# CPU profiling with perf
perf record --call-graph=dwarf \
    target/release/pattern_matching_bench \
    --bench simple_pattern

# Generate flamegraph
perf script | stackcollapse-perf.pl | flamegraph.pl > pattern_matching.svg

# Memory profiling with heaptrack
heaptrack target/release/pattern_matching_bench \
    --bench variable_binding

# Cache analysis with cachegrind
valgrind --tool=cachegrind \
    target/release/pattern_matching_bench \
    --bench nested_patterns
```

### Optimization Targets

1. **L3 cache hit rate**: > 95% for typical queries
2. **Memory bandwidth utilization**: < 50% of peak (66.4 GB/s for DDR4-2133)
3. **Thread scaling**: Linear up to 36 threads (physical cores)
4. **Latency**: < 1 ms for simple patterns, < 100 ms for complex patterns (1M atoms)

---

## Summary

This pattern matching implementation guide provides:

1. **Hybrid Variable Representation**: Seamlessly maps MeTTa named variables to MORK De Bruijn levels
2. **Two-Phase Matching**: Structural matching via MORK, binding extraction via custom logic
3. **Non-Deterministic Support**: BindingsSet with Cartesian product and union operations
4. **Performance Optimization**: Lazy evaluation, prefix optimization, batched queries, hardware-specific tuning
5. **Comprehensive Testing**: Unit, property-based, and integration tests
6. **Benchmark Suite**: Systematic performance measurement

### Key Takeaways

- **Use BTMSource.meet() for structural matching** - leverages MORK's prefix compression
- **Extract bindings in a separate pass** - cleaner separation of concerns
- **Cache grounded atom matching** - avoid repeated expensive operations
- **Batch related queries** - amortize symbol interning costs
- **Profile before optimizing** - measure actual bottlenecks

### Next Steps

1. Implement `PatternContext` and `Bindings` data structures
2. Implement `encode_pattern()` and `extract_bindings()` functions
3. Implement `PatternMatcher` with `match_pattern()` method
4. Add comprehensive test suite
5. Run benchmarks and optimize hot paths
6. Integrate with MeTTaTron evaluation engine

---

**Document Version**: 1.0
**Last Updated**: 2025-11-13
**Next Review**: After initial implementation and benchmarking
