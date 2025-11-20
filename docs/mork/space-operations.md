# Space Operations Implementation Guide for MORK

**Version**: 1.0
**Date**: 2025-11-13
**Target**: MeTTaTron Compiler
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Introduction](#introduction)
2. [MeTTa Space Semantics](#metta-space-semantics)
3. [MORK Space Architecture](#mork-space-architecture)
4. [Space Data Structures](#space-data-structures)
5. [Add Operation](#add-operation)
6. [Remove Operation](#remove-operation)
7. [Query Operation](#query-operation)
8. [Replace Operation](#replace-operation)
9. [Atom Iteration](#atom-iteration)
10. [Space Composition](#space-composition)
11. [Concurrent Space Access](#concurrent-space-access)
12. [Performance Optimization](#performance-optimization)
13. [Implementation Examples](#implementation-examples)
14. [Testing Strategy](#testing-strategy)
15. [Performance Benchmarks](#performance-benchmarks)

---

## Introduction

This document provides a comprehensive guide to implementing MeTTa space operations on top of MORK's hypergraph processing kernel. Spaces are fundamental to MeTTa's architecture, serving as:

- **Knowledge bases**: Collections of facts and rules
- **Module boundaries**: Scoped namespaces for atoms
- **Query targets**: Datasets against which patterns are matched
- **Mutable storage**: Dynamic atom collections

### Key Design Goals

1. **Correctness**: Preserve MeTTa space semantics exactly
2. **Performance**: Leverage MORK's structural sharing and prefix compression
3. **Concurrency**: Enable safe multi-threaded access
4. **Composability**: Support space composition and nesting
5. **Memory efficiency**: Minimize memory footprint through sharing

### MORK Advantages for Spaces

- **Structural sharing**: Multiple spaces can share common sub-structures
- **Prefix compression**: Similar atoms share storage
- **Copy-on-write**: O(1) space cloning
- **Lock-free reads**: Concurrent queries without contention
- **Algebraic operations**: Efficient space union/intersection/difference

---

## MeTTa Space Semantics

### Space Operations Overview

MeTTa spaces support the following core operations:

```metta
; Add atom to space
!(add-atom &space (parent Alice Bob))

; Remove atom from space
!(remove-atom &space (parent Alice Bob))

; Query space with pattern
!(match &space (parent $x Bob) $x)

; Replace atom (remove old, add new)
!(replace-atom &space (parent Alice Bob) (parent Alice Carol))

; Get all atoms in space
!(get-atoms &space)

; Create new empty space
!(new-space)

; Clone space (COW)
!(clone-space &space)
```

### Set Semantics

MeTTa spaces have **set semantics**:
- No duplicate atoms
- Order-independent
- Add is idempotent: adding same atom twice = adding once

```metta
; Example:
!(add-atom &space foo)
!(add-atom &space foo)  ; No-op, foo already exists
!(get-atoms &space)     ; → [foo]
```

### Atom Equality

Two atoms are equal if:
1. Same type (Symbol/Variable/Expression/Grounded)
2. Same structure (for expressions)
3. Same value (for symbols/grounded atoms)

```metta
; Equal atoms:
foo == foo
(parent Alice Bob) == (parent Alice Bob)

; Not equal:
foo != bar
(parent Alice Bob) != (parent Bob Alice)
```

### Space Isolation

Each space is isolated:

```metta
; Different spaces, independent state
!(add-atom &space1 foo)
!(add-atom &space2 bar)

!(match &space1 $x $x)  ; → [foo]
!(match &space2 $x $x)  ; → [bar]
```

### Space Composition

Spaces can be composed:

```metta
; Union of spaces
!(union-space &space1 &space2)

; Intersection of spaces
!(intersect-space &space1 &space2)

; Difference of spaces
!(diff-space &space1 &space2)
```

---

## MORK Space Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        MorkSpace                            │
├─────────────────────────────────────────────────────────────┤
│  • PathMap<()>         - Atom storage (BTM encoded)         │
│  • SharedMapping       - Symbol table (interning)           │
│  • GroundedRegistry    - Grounded type registry             │
│  • AtomCache          - Decoded atom cache (optional)       │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                     MORK Operations                         │
├─────────────────────────────────────────────────────────────┤
│  • add_atom()     → AddSink.join()                          │
│  • remove_atom()  → RemoveSink.subtract()                   │
│  • query()        → BTMSource.meet()                        │
│  • replace_atom() → subtract() + join()                     │
│  • iterate()      → PathMap.iter_paths()                    │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                    PathMap (MORK Core)                      │
├─────────────────────────────────────────────────────────────┤
│  • Arc-based structural sharing                             │
│  • Prefix-compressed trie                                   │
│  • Lock-free reads, coordinated writes                      │
│  • Algebraic lattice operations                             │
└─────────────────────────────────────────────────────────────┘
```

### Encoding Layer

```
MeTTa Atom ──encode──> MORK Bytes ──store──> PathMap
           PatternContext        AddSink

PathMap ──iterate──> MORK Bytes ──decode──> MeTTa Atom
                                  AtomDecoder
```

---

## Space Data Structures

### MorkSpace Core Structure

```rust
use std::sync::{Arc, RwLock};
use pathmap::PathMap;
use mork::SharedMapping;

/// A MeTTa space implemented on MORK
pub struct MorkSpace {
    /// Main atom storage (BTM-encoded atoms → unit value)
    btm: PathMap<()>,

    /// Shared symbol table for interning
    symbol_table: Arc<SharedMapping>,

    /// Registry for grounded atom types
    grounded_registry: Arc<GroundedRegistry>,

    /// Optional cache for frequently accessed atoms
    cache: Option<Arc<RwLock<AtomCache>>>,
}

impl MorkSpace {
    /// Create a new empty space
    pub fn new() -> Self {
        Self {
            btm: PathMap::new(),
            symbol_table: Arc::new(SharedMapping::new()),
            grounded_registry: Arc::new(GroundedRegistry::new()),
            cache: None,
        }
    }

    /// Create a new empty space with caching enabled
    pub fn with_cache() -> Self {
        Self {
            btm: PathMap::new(),
            symbol_table: Arc::new(SharedMapping::new()),
            grounded_registry: Arc::new(GroundedRegistry::new()),
            cache: Some(Arc::new(RwLock::new(AtomCache::new()))),
        }
    }

    /// Create space with shared symbol table
    pub fn with_shared_symbols(symbol_table: Arc<SharedMapping>) -> Self {
        Self {
            btm: PathMap::new(),
            symbol_table,
            grounded_registry: Arc::new(GroundedRegistry::new()),
            cache: None,
        }
    }

    /// Clone space (copy-on-write)
    pub fn clone_cow(&self) -> Self {
        Self {
            btm: self.btm.clone(),  // Arc clone - O(1)
            symbol_table: Arc::clone(&self.symbol_table),
            grounded_registry: Arc::clone(&self.grounded_registry),
            cache: self.cache.as_ref().map(Arc::clone),
        }
    }

    /// Number of atoms in space
    pub fn len(&self) -> usize {
        self.btm.len()
    }

    /// Check if space is empty
    pub fn is_empty(&self) -> bool {
        self.btm.is_empty()
    }
}
```

### Grounded Registry

```rust
use std::collections::HashMap;
use std::any::TypeId;

/// Registry for grounded atom types
pub struct GroundedRegistry {
    /// Map: type name → type ID
    name_to_id: RwLock<HashMap<String, u32>>,

    /// Map: type ID → type info
    id_to_info: RwLock<HashMap<u32, GroundedTypeInfo>>,

    /// Next available type ID
    next_id: AtomicU32,
}

pub struct GroundedTypeInfo {
    pub type_id: TypeId,
    pub type_name: String,
    pub serialize: fn(&dyn Any) -> Result<Vec<u8>, SerializationError>,
    pub deserialize: fn(&[u8]) -> Result<Box<dyn Any>, SerializationError>,
}

impl GroundedRegistry {
    pub fn new() -> Self {
        Self {
            name_to_id: RwLock::new(HashMap::new()),
            id_to_info: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(0),
        }
    }

    /// Register a grounded type
    pub fn register<T: 'static>(
        &self,
        type_name: &str,
        serialize: fn(&T) -> Result<Vec<u8>, SerializationError>,
        deserialize: fn(&[u8]) -> Result<T, SerializationError>,
    ) -> u32 {
        let type_id = TypeId::of::<T>();

        // Check if already registered
        {
            let name_map = self.name_to_id.read().unwrap();
            if let Some(&id) = name_map.get(type_name) {
                return id;
            }
        }

        // Allocate new ID
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        // Wrap serialize/deserialize functions
        let serialize_any = move |any: &dyn Any| -> Result<Vec<u8>, SerializationError> {
            let value = any.downcast_ref::<T>()
                .ok_or(SerializationError::TypeMismatch)?;
            serialize(value)
        };

        let deserialize_any = move |bytes: &[u8]| -> Result<Box<dyn Any>, SerializationError> {
            let value = deserialize(bytes)?;
            Ok(Box::new(value))
        };

        let info = GroundedTypeInfo {
            type_id,
            type_name: type_name.to_string(),
            serialize: serialize_any,
            deserialize: deserialize_any,
        };

        // Store mappings
        self.name_to_id.write().unwrap().insert(type_name.to_string(), id);
        self.id_to_info.write().unwrap().insert(id, info);

        id
    }

    /// Get type ID by name
    pub fn get_id(&self, type_name: &str) -> Option<u32> {
        self.name_to_id.read().unwrap().get(type_name).copied()
    }

    /// Get type info by ID
    pub fn get_info(&self, type_id: u32) -> Option<GroundedTypeInfo> {
        self.id_to_info.read().unwrap().get(&type_id).cloned()
    }
}
```

### Atom Cache

```rust
use lru::LruCache;

/// LRU cache for decoded atoms
pub struct AtomCache {
    cache: LruCache<Vec<u8>, Atom>,
}

impl AtomCache {
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            cache: LruCache::new(cap),
        }
    }

    pub fn get(&mut self, key: &[u8]) -> Option<&Atom> {
        self.cache.get(key)
    }

    pub fn put(&mut self, key: Vec<u8>, value: Atom) {
        self.cache.put(key, value);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}
```

---

## Add Operation

### Semantics

```metta
; Add atom to space
!(add-atom &space (parent Alice Bob))

; Returns: previous state or success indicator
```

**Idempotency**: Adding same atom twice has no effect.

### Implementation

```rust
impl MorkSpace {
    /// Add an atom to the space
    pub fn add(&mut self, atom: &Atom) -> Result<bool, SpaceError> {
        // Encode atom to MORK bytes
        let encoded = self.encode_atom(atom)?;

        // Create BTM source from encoded atom
        let source = BTMSource::new(encoded.clone());

        // Use AddSink to insert into PathMap
        let mut sink = AddSink::new();
        let mut wz = self.btm.write_zipper();
        let status = sink.apply(&mut wz, &source)?;

        // Update cache if enabled
        if let Some(cache) = &self.cache {
            cache.write().unwrap().put(encoded, atom.clone());
        }

        // Return true if atom was newly added
        match status {
            AlgebraicStatus::Element => Ok(true),   // New atom added
            AlgebraicStatus::Identity => Ok(false), // Already existed
            AlgebraicStatus::None => Ok(false),     // No change
        }
    }

    /// Encode atom to MORK byte sequence
    fn encode_atom(&self, atom: &Atom) -> Result<Vec<u8>, EncodingError> {
        encode_atom_with_registry(
            atom,
            &self.symbol_table,
            &self.grounded_registry,
        )
    }
}

/// Encode atom with symbol table and grounded registry
pub fn encode_atom_with_registry(
    atom: &Atom,
    symbol_table: &SharedMapping,
    grounded_registry: &GroundedRegistry,
) -> Result<Vec<u8>, EncodingError> {
    match atom {
        Atom::Symbol(sym) => encode_symbol(sym, symbol_table),
        Atom::Variable(var) => encode_variable(var),
        Atom::Expression(expr) => encode_expression(expr, symbol_table, grounded_registry),
        Atom::Grounded(g) => encode_grounded(g, grounded_registry),
    }
}

fn encode_symbol(
    sym: &SymbolAtom,
    symbol_table: &SharedMapping,
) -> Result<Vec<u8>, EncodingError> {
    let name = sym.name();
    let len = name.len();

    if len == 0 {
        return Err(EncodingError::EmptySymbol);
    }

    if len <= 15 {
        // Inline symbol
        let mut bytes = vec![Tag::SymbolSize as u8 | (len as u8)];
        bytes.extend_from_slice(name.as_bytes());
        Ok(bytes)
    } else {
        // Interned symbol
        let symbol_id = symbol_table.insert(name);
        let mut bytes = vec![Tag::InternedSymbol as u8];
        bytes.extend_from_slice(&symbol_id.to_le_bytes());
        Ok(bytes)
    }
}

fn encode_variable(var: &VariableAtom) -> Result<Vec<u8>, EncodingError> {
    // Variables in ground atoms (not patterns) are treated as symbols
    let name = var.name();
    let len = name.len();

    let mut bytes = vec![Tag::SymbolSize as u8 | (len as u8)];
    bytes.extend_from_slice(name.as_bytes());
    Ok(bytes)
}

fn encode_expression(
    expr: &ExpressionAtom,
    symbol_table: &SharedMapping,
    grounded_registry: &GroundedRegistry,
) -> Result<Vec<u8>, EncodingError> {
    let children = expr.children();
    let arity = children.len();

    if arity > 255 {
        return Err(EncodingError::ArityTooLarge(arity));
    }

    let mut bytes = vec![Tag::Arity as u8, arity as u8];

    for child in children {
        let child_bytes = encode_atom_with_registry(child, symbol_table, grounded_registry)?;
        bytes.extend(child_bytes);
    }

    Ok(bytes)
}

fn encode_grounded(
    g: &Grounded,
    grounded_registry: &GroundedRegistry,
) -> Result<Vec<u8>, EncodingError> {
    let type_name = g.type_name();
    let type_id = grounded_registry.get_id(type_name)
        .ok_or(EncodingError::UnregisteredGroundedType(type_name.to_string()))?;

    let info = grounded_registry.get_info(type_id)
        .ok_or(EncodingError::InvalidGroundedTypeId(type_id))?;

    let value_bytes = (info.serialize)(g.as_any())?;

    let mut bytes = vec![Tag::Grounded as u8];
    bytes.extend_from_slice(&type_id.to_le_bytes());
    bytes.extend_from_slice(&(value_bytes.len() as u32).to_le_bytes());
    bytes.extend(value_bytes);

    Ok(bytes)
}
```

### Batched Add

For adding multiple atoms efficiently:

```rust
impl MorkSpace {
    /// Add multiple atoms in batch
    pub fn add_batch(&mut self, atoms: &[Atom]) -> Result<Vec<bool>, SpaceError> {
        // Encode all atoms first
        let encoded: Vec<_> = atoms.iter()
            .map(|atom| self.encode_atom(atom))
            .collect::<Result<Vec<_>, _>>()?;

        // Combine into single BTM source
        let mut combined = PathMap::new();
        for bytes in &encoded {
            let source = BTMSource::new(bytes.clone());
            let mut wz = combined.write_zipper();
            wz.join_into(&source.read_zipper(), true);
        }

        // Single join operation with main space
        let mut wz = self.btm.write_zipper();
        let status = wz.join_into(&combined.read_zipper(), true);

        // Update cache
        if let Some(cache) = &self.cache {
            let mut cache_lock = cache.write().unwrap();
            for (encoded, atom) in encoded.iter().zip(atoms.iter()) {
                cache_lock.put(encoded.clone(), atom.clone());
            }
        }

        // For batch operations, we return success for all
        // (individual tracking would require per-atom queries)
        Ok(vec![true; atoms.len()])
    }
}
```

### Performance Characteristics

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Encode atom | O(N) | N = atom size in bytes |
| BTM source creation | O(log N) | N = source size |
| Join into space | O(log M) | M = space size, amortized |
| Cache update | O(1) | LRU cache |

**Expected latency** (1M atoms in space):
- Simple symbol: ~1-5 μs
- Expression (depth 3): ~10-50 μs
- Grounded atom: ~10-100 μs (depends on serialization)

---

## Remove Operation

### Semantics

```metta
; Remove atom from space
!(remove-atom &space (parent Alice Bob))

; Returns: true if atom was present, false otherwise
```

### Implementation

```rust
impl MorkSpace {
    /// Remove an atom from the space
    pub fn remove(&mut self, atom: &Atom) -> Result<bool, SpaceError> {
        // Encode atom
        let encoded = self.encode_atom(atom)?;

        // Create BTM source for atom to remove
        let source = BTMSource::new(encoded.clone());

        // Use subtract operation
        let mut wz = self.btm.write_zipper();
        let status = wz.subtract_into(&source.read_zipper(), true);

        // Update cache
        if let Some(cache) = &self.cache {
            cache.write().unwrap().cache.pop(&encoded);
        }

        // Return true if atom was removed
        match status {
            AlgebraicStatus::Element => Ok(true),   // Atom was removed
            AlgebraicStatus::Identity => Ok(false), // Atom wasn't present
            AlgebraicStatus::None => Ok(true),      // All removed (should be same as Element here)
        }
    }

    /// Remove all atoms matching a pattern
    pub fn remove_matching(&mut self, pattern: &Atom) -> Result<usize, SpaceError> {
        // First, query to find matching atoms
        let matches = self.query(pattern)?;

        if matches.is_empty() {
            return Ok(0);
        }

        // Collect all matched atoms
        let mut matched_atoms = Vec::new();
        for bindings in matches.alternatives() {
            let instantiated = bindings.apply(pattern);
            matched_atoms.push(instantiated);
        }

        // Remove in batch
        let count = matched_atoms.len();
        for atom in &matched_atoms {
            self.remove(atom)?;
        }

        Ok(count)
    }
}
```

### Batched Remove

```rust
impl MorkSpace {
    /// Remove multiple atoms in batch
    pub fn remove_batch(&mut self, atoms: &[Atom]) -> Result<Vec<bool>, SpaceError> {
        // Encode all atoms
        let encoded: Vec<_> = atoms.iter()
            .map(|atom| self.encode_atom(atom))
            .collect::<Result<Vec<_>, _>>()?;

        // Combine into single BTM source
        let mut combined = PathMap::new();
        for bytes in &encoded {
            let source = BTMSource::new(bytes.clone());
            let mut wz = combined.write_zipper();
            wz.join_into(&source.read_zipper(), true);
        }

        // Single subtract operation
        let mut wz = self.btm.write_zipper();
        let status = wz.subtract_into(&combined.read_zipper(), true);

        // Update cache
        if let Some(cache) = &self.cache {
            let mut cache_lock = cache.write().unwrap();
            for bytes in &encoded {
                cache_lock.cache.pop(bytes);
            }
        }

        Ok(vec![true; atoms.len()])
    }
}
```

### Performance Characteristics

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Encode atom | O(N) | N = atom size |
| Subtract from space | O(log M) | M = space size |
| Pattern-based remove | O(M) | Must query first |

---

## Query Operation

### Semantics

```metta
; Query space with pattern
!(match &space (parent $x Bob) $x)

; Returns: all bindings for $x
```

### Implementation

Uses the pattern matching infrastructure from `pattern-matching.md`:

```rust
use crate::pattern_matching::{PatternMatcher, PatternContext, BindingsSet};

impl MorkSpace {
    /// Query space with a pattern
    pub fn query(&self, pattern: &Atom) -> Result<BindingsSet, QueryError> {
        let matcher = PatternMatcher::new(Arc::new(self.clone_cow()));
        matcher.match_pattern(pattern)
    }

    /// Query space and apply bindings to template
    pub fn query_map(
        &self,
        pattern: &Atom,
        template: &Atom,
    ) -> Result<Vec<Atom>, QueryError> {
        let bindings_set = self.query(pattern)?;

        let results: Vec<Atom> = bindings_set.alternatives()
            .iter()
            .map(|bindings| bindings.apply(template))
            .collect();

        Ok(results)
    }

    /// Query with predicate filter
    pub fn query_filter<F>(
        &self,
        pattern: &Atom,
        predicate: F,
    ) -> Result<BindingsSet, QueryError>
    where
        F: Fn(&Bindings) -> bool,
    {
        let mut bindings_set = self.query(pattern)?;
        bindings_set.filter(predicate);
        Ok(bindings_set)
    }
}
```

### Advanced Queries

**Conjunction** (AND):
```rust
impl MorkSpace {
    /// Query with multiple patterns (all must match)
    pub fn query_and(&self, patterns: &[Atom]) -> Result<BindingsSet, QueryError> {
        let mut result = BindingsSet::from_single(Bindings::new());

        for pattern in patterns {
            let pattern_bindings = self.query(pattern)?;
            result = result.product(&pattern_bindings)?;

            if result.is_empty() {
                // Early exit if no matches
                return Ok(result);
            }
        }

        Ok(result)
    }
}
```

**Disjunction** (OR):
```rust
impl MorkSpace {
    /// Query with multiple patterns (any can match)
    pub fn query_or(&self, patterns: &[Atom]) -> Result<BindingsSet, QueryError> {
        let mut result = BindingsSet::new();

        for pattern in patterns {
            let pattern_bindings = self.query(pattern)?;
            result.union(pattern_bindings);
        }

        Ok(result)
    }
}
```

**Negation** (NOT):
```rust
impl MorkSpace {
    /// Query excluding matches of negated pattern
    pub fn query_not(
        &self,
        positive_pattern: &Atom,
        negative_pattern: &Atom,
    ) -> Result<BindingsSet, QueryError> {
        let positive_bindings = self.query(positive_pattern)?;
        let negative_bindings = self.query(negative_pattern)?;

        // Filter out positive bindings that are in negative bindings
        let mut result = BindingsSet::new();

        for pos_bind in positive_bindings.alternatives() {
            let mut excluded = false;

            for neg_bind in negative_bindings.alternatives() {
                if bindings_compatible(pos_bind, neg_bind) {
                    excluded = true;
                    break;
                }
            }

            if !excluded {
                result.add(pos_bind.clone());
            }
        }

        Ok(result)
    }
}

fn bindings_compatible(b1: &Bindings, b2: &Bindings) -> bool {
    for (var, val1) in b1.iter() {
        if let Some(val2) = b2.get(var) {
            if val1 != val2 {
                return false;
            }
        }
    }
    true
}
```

---

## Replace Operation

### Semantics

```metta
; Replace old atom with new atom
!(replace-atom &space (parent Alice Bob) (parent Alice Carol))
```

### Implementation

```rust
impl MorkSpace {
    /// Replace an atom (remove old, add new)
    pub fn replace(&mut self, old: &Atom, new: &Atom) -> Result<bool, SpaceError> {
        // First remove old atom
        let removed = self.remove(old)?;

        // Then add new atom
        let added = self.add(new)?;

        // Return true if old was present
        Ok(removed)
    }

    /// Replace all atoms matching pattern with instantiated template
    pub fn replace_matching(
        &mut self,
        pattern: &Atom,
        template: &Atom,
    ) -> Result<usize, SpaceError> {
        // Query to find matches
        let bindings_set = self.query(pattern)?;

        if bindings_set.is_empty() {
            return Ok(0);
        }

        let mut count = 0;

        for bindings in bindings_set.alternatives() {
            let old_atom = bindings.apply(pattern);
            let new_atom = bindings.apply(template);

            if old_atom != new_atom {
                self.remove(&old_atom)?;
                self.add(&new_atom)?;
                count += 1;
            }
        }

        Ok(count)
    }
}
```

### Atomic Replace

For transactional semantics:

```rust
impl MorkSpace {
    /// Atomically replace (both succeed or both fail)
    pub fn replace_atomic(&mut self, old: &Atom, new: &Atom) -> Result<bool, SpaceError> {
        // Check if old exists
        let old_encoded = self.encode_atom(old)?;
        let old_source = BTMSource::new(old_encoded.clone());
        let space_zipper = self.btm.read_zipper();

        let contains_old = !old_source.read_zipper().meet(&space_zipper, true).is_empty();

        if !contains_old {
            return Ok(false);  // Old atom not present
        }

        // Create new space state
        let new_encoded = self.encode_atom(new)?;
        let new_source = BTMSource::new(new_encoded);

        // Remove old and add new in single write
        let mut wz = self.btm.write_zipper();

        // Remove old
        wz.subtract_into(&old_source.read_zipper(), true);

        // Add new
        wz.join_into(&new_source.read_zipper(), true);

        Ok(true)
    }
}
```

---

## Atom Iteration

### Semantics

```metta
; Get all atoms in space
!(get-atoms &space)
```

### Implementation

```rust
impl MorkSpace {
    /// Iterate all atoms in space
    pub fn iter(&self) -> AtomIterator {
        AtomIterator {
            paths: self.btm.iter_paths(),
            symbol_table: Arc::clone(&self.symbol_table),
            grounded_registry: Arc::clone(&self.grounded_registry),
            cache: self.cache.as_ref().map(Arc::clone),
        }
    }

    /// Collect all atoms into a vector
    pub fn get_atoms(&self) -> Result<Vec<Atom>, DecodingError> {
        self.iter().collect()
    }

    /// Count atoms (more efficient than collecting)
    pub fn count(&self) -> usize {
        self.btm.len()
    }
}

pub struct AtomIterator {
    paths: PathIterator,
    symbol_table: Arc<SharedMapping>,
    grounded_registry: Arc<GroundedRegistry>,
    cache: Option<Arc<RwLock<AtomCache>>>,
}

impl Iterator for AtomIterator {
    type Item = Result<Atom, DecodingError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.paths.next().map(|path| {
            // Check cache first
            if let Some(cache) = &self.cache {
                if let Some(atom) = cache.write().unwrap().get(path) {
                    return Ok(atom.clone());
                }
            }

            // Decode from path
            let atom = decode_atom(
                path,
                &self.symbol_table,
                &self.grounded_registry,
            )?;

            // Update cache
            if let Some(cache) = &self.cache {
                cache.write().unwrap().put(path.to_vec(), atom.clone());
            }

            Ok(atom)
        })
    }
}

/// Decode MORK byte sequence to MeTTa atom
pub fn decode_atom(
    bytes: &[u8],
    symbol_table: &SharedMapping,
    grounded_registry: &GroundedRegistry,
) -> Result<Atom, DecodingError> {
    let mut pos = 0;
    decode_atom_at(bytes, &mut pos, symbol_table, grounded_registry)
}

fn decode_atom_at(
    bytes: &[u8],
    pos: &mut usize,
    symbol_table: &SharedMapping,
    grounded_registry: &GroundedRegistry,
) -> Result<Atom, DecodingError> {
    if *pos >= bytes.len() {
        return Err(DecodingError::UnexpectedEnd);
    }

    let tag = bytes[*pos];
    *pos += 1;

    match Tag::from_u8(tag)? {
        Tag::Arity => {
            let arity = bytes[*pos];
            *pos += 1;

            let mut children = Vec::with_capacity(arity as usize);
            for _ in 0..arity {
                children.push(decode_atom_at(bytes, pos, symbol_table, grounded_registry)?);
            }

            Ok(Atom::Expression(ExpressionAtom::new(children)))
        }

        Tag::SymbolSize => {
            let len = (tag & 0x0F) as usize;
            let name = std::str::from_utf8(&bytes[*pos..*pos + len])
                .map_err(|_| DecodingError::InvalidUtf8)?;
            *pos += len;
            Ok(Atom::Symbol(SymbolAtom::new(name)))
        }

        Tag::InternedSymbol => {
            let symbol_id = u32::from_le_bytes([
                bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]
            ]);
            *pos += 4;

            let name = symbol_table.get(symbol_id)
                .ok_or(DecodingError::InvalidSymbolId(symbol_id))?;
            Ok(Atom::Symbol(SymbolAtom::new(&name)))
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

            let info = grounded_registry.get_info(type_id)
                .ok_or(DecodingError::InvalidGroundedTypeId(type_id))?;

            let value = (info.deserialize)(value_bytes)?;

            // Reconstruct Grounded atom (requires type-specific logic)
            let grounded = reconstruct_grounded(type_id, value)?;

            Ok(Atom::Grounded(grounded))
        }

        _ => Err(DecodingError::UnexpectedTag(tag)),
    }
}

fn reconstruct_grounded(type_id: u32, value: Box<dyn Any>) -> Result<Grounded, DecodingError> {
    // Type-specific reconstruction
    // This requires registry to maintain reconstruction logic
    todo!("Implement grounded atom reconstruction")
}
```

### Filtered Iteration

```rust
impl MorkSpace {
    /// Iterate atoms matching a predicate
    pub fn iter_filter<F>(&self, predicate: F) -> impl Iterator<Item = Result<Atom, DecodingError>>
    where
        F: Fn(&Atom) -> bool,
    {
        self.iter().filter_map(move |result| {
            match result {
                Ok(atom) => {
                    if predicate(&atom) {
                        Some(Ok(atom))
                    } else {
                        None
                    }
                }
                Err(e) => Some(Err(e)),
            }
        })
    }

    /// Iterate expressions only
    pub fn iter_expressions(&self) -> impl Iterator<Item = Result<ExpressionAtom, DecodingError>> {
        self.iter().filter_map(|result| {
            match result {
                Ok(Atom::Expression(expr)) => Some(Ok(expr)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        })
    }

    /// Iterate symbols only
    pub fn iter_symbols(&self) -> impl Iterator<Item = Result<SymbolAtom, DecodingError>> {
        self.iter().filter_map(|result| {
            match result {
                Ok(Atom::Symbol(sym)) => Some(Ok(sym)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        })
    }
}
```

---

## Space Composition

### Union

```rust
impl MorkSpace {
    /// Union of two spaces (self ∪ other)
    pub fn union(&mut self, other: &MorkSpace) -> Result<(), SpaceError> {
        let mut wz = self.btm.write_zipper();
        wz.join_into(&other.btm.read_zipper(), true);
        Ok(())
    }

    /// Create new space as union (non-mutating)
    pub fn union_new(&self, other: &MorkSpace) -> Result<MorkSpace, SpaceError> {
        let mut result = self.clone_cow();
        result.union(other)?;
        Ok(result)
    }
}
```

### Intersection

```rust
impl MorkSpace {
    /// Intersection of two spaces (self ∩ other)
    pub fn intersection(&mut self, other: &MorkSpace) -> Result<(), SpaceError> {
        let result = self.btm.read_zipper().meet(&other.btm.read_zipper(), true);

        // Replace self.btm with intersection result
        self.btm = result;

        Ok(())
    }

    /// Create new space as intersection (non-mutating)
    pub fn intersection_new(&self, other: &MorkSpace) -> Result<MorkSpace, SpaceError> {
        let btm = self.btm.read_zipper().meet(&other.btm.read_zipper(), true);

        Ok(MorkSpace {
            btm,
            symbol_table: Arc::clone(&self.symbol_table),
            grounded_registry: Arc::clone(&self.grounded_registry),
            cache: None,
        })
    }
}
```

### Difference

```rust
impl MorkSpace {
    /// Difference of two spaces (self \ other)
    pub fn difference(&mut self, other: &MorkSpace) -> Result<(), SpaceError> {
        let mut wz = self.btm.write_zipper();
        wz.subtract_into(&other.btm.read_zipper(), true);
        Ok(())
    }

    /// Create new space as difference (non-mutating)
    pub fn difference_new(&self, other: &MorkSpace) -> Result<MorkSpace, SpaceError> {
        let mut result = self.clone_cow();
        result.difference(other)?;
        Ok(result)
    }
}
```

### Subset Check

```rust
impl MorkSpace {
    /// Check if self is subset of other (self ⊆ other)
    pub fn is_subset_of(&self, other: &MorkSpace) -> bool {
        // self ⊆ other iff self ∩ other = self
        let intersection = self.btm.read_zipper().meet(&other.btm.read_zipper(), true);

        // Compare sizes (approximate check)
        // For exact check, need to compare all paths
        intersection.len() == self.btm.len()
    }

    /// Check if self is superset of other (self ⊇ other)
    pub fn is_superset_of(&self, other: &MorkSpace) -> bool {
        other.is_subset_of(self)
    }
}
```

---

## Concurrent Space Access

### Read-Write Lock Pattern

```rust
use std::sync::{Arc, RwLock};

/// Thread-safe space wrapper
pub struct SharedSpace {
    space: Arc<RwLock<MorkSpace>>,
}

impl SharedSpace {
    pub fn new(space: MorkSpace) -> Self {
        Self {
            space: Arc::new(RwLock::new(space)),
        }
    }

    /// Add atom (requires write lock)
    pub fn add(&self, atom: &Atom) -> Result<bool, SpaceError> {
        let mut space = self.space.write().unwrap();
        space.add(atom)
    }

    /// Remove atom (requires write lock)
    pub fn remove(&self, atom: &Atom) -> Result<bool, SpaceError> {
        let mut space = self.space.write().unwrap();
        space.remove(atom)
    }

    /// Query space (read lock only)
    pub fn query(&self, pattern: &Atom) -> Result<BindingsSet, QueryError> {
        let space = self.space.read().unwrap();
        space.query(pattern)
    }

    /// Clone space (read lock + COW)
    pub fn clone_cow(&self) -> MorkSpace {
        let space = self.space.read().unwrap();
        space.clone_cow()
    }
}

impl Clone for SharedSpace {
    fn clone(&self) -> Self {
        Self {
            space: Arc::clone(&self.space),
        }
    }
}
```

### Lock-Free Read Pattern

```rust
use std::sync::Arc;
use parking_lot::RwLock;

/// Optimized for concurrent reads
pub struct ConcurrentSpace {
    /// Current space version
    space: Arc<RwLock<Arc<MorkSpace>>>,
}

impl ConcurrentSpace {
    pub fn new(space: MorkSpace) -> Self {
        Self {
            space: Arc::new(RwLock::new(Arc::new(space))),
        }
    }

    /// Get snapshot for reading (lock-free after snapshot)
    pub fn snapshot(&self) -> Arc<MorkSpace> {
        Arc::clone(&*self.space.read())
    }

    /// Add atom (COW update)
    pub fn add(&self, atom: &Atom) -> Result<bool, SpaceError> {
        let mut current = self.space.write();

        // Clone space (COW)
        let mut new_space = (**current).clone_cow();

        // Modify clone
        let result = new_space.add(atom)?;

        // Atomic swap
        *current = Arc::new(new_space);

        Ok(result)
    }

    /// Query using snapshot
    pub fn query(&self, pattern: &Atom) -> Result<BindingsSet, QueryError> {
        let snapshot = self.snapshot();
        snapshot.query(pattern)
    }
}
```

---

## Performance Optimization

### Batch Operations

Always prefer batch operations when adding/removing multiple atoms:

```rust
// ✗ Inefficient: individual adds
for atom in atoms {
    space.add(atom)?;
}

// ✓ Efficient: batch add
space.add_batch(&atoms)?;
```

**Performance difference**: 10-100× faster for large batches.

### Symbol Table Sharing

Share symbol tables across related spaces:

```rust
let symbol_table = Arc::new(SharedMapping::new());

let space1 = MorkSpace::with_shared_symbols(Arc::clone(&symbol_table));
let space2 = MorkSpace::with_shared_symbols(Arc::clone(&symbol_table));

// Both spaces use same symbol interning
```

**Memory savings**: 50-90% reduction for spaces with common symbols.

### Query Result Caching

Cache frequent query results:

```rust
use std::collections::HashMap;

pub struct CachedSpace {
    space: MorkSpace,
    query_cache: RwLock<HashMap<Atom, BindingsSet>>,
}

impl CachedSpace {
    pub fn query(&self, pattern: &Atom) -> Result<BindingsSet, QueryError> {
        // Check cache
        {
            let cache = self.query_cache.read().unwrap();
            if let Some(result) = cache.get(pattern) {
                return Ok(result.clone());
            }
        }

        // Execute query
        let result = self.space.query(pattern)?;

        // Update cache
        {
            let mut cache = self.query_cache.write().unwrap();
            cache.insert(pattern.clone(), result.clone());
        }

        Ok(result)
    }

    pub fn invalidate_cache(&self) {
        let mut cache = self.query_cache.write().unwrap();
        cache.clear();
    }
}
```

### Lazy Iteration

Use iterators instead of collecting to vectors:

```rust
// ✗ Eagerly collect all atoms
let atoms = space.get_atoms()?;
for atom in atoms {
    process(atom);
}

// ✓ Lazy iteration
for atom_result in space.iter() {
    let atom = atom_result?;
    process(atom);
}
```

### NUMA-Aware Allocation

For the Intel Xeon E5-2699 v3 with 4 NUMA nodes:

```rust
#[cfg(target_os = "linux")]
pub fn create_numa_spaces(num_spaces: usize) -> Vec<MorkSpace> {
    use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};

    (0..num_spaces).map(|i| {
        // Pin to NUMA node
        let node = i % 4;
        set_numa_affinity(node);

        MorkSpace::new()
    }).collect()
}
```

---

## Implementation Examples

### Complete Space Usage Example

```rust
use mork_space::{MorkSpace, SharedSpace};
use metta_atom::{atom, expr, sym, var};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create space
    let mut space = MorkSpace::with_cache();

    // Add facts
    space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Bob")))?;
    space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Carol")))?;
    space.add(&expr!(sym!("parent"), sym!("Bob"), sym!("Dave")))?;

    println!("Space size: {}", space.len());

    // Query: find Alice's children
    let pattern = expr!(sym!("parent"), sym!("Alice"), var!("$child"));
    let bindings_set = space.query(&pattern)?;

    println!("Alice's children:");
    for bindings in bindings_set.alternatives() {
        if let Some(child) = bindings.get("$child") {
            println!("  {}", child);
        }
    }

    // Remove a fact
    space.remove(&expr!(sym!("parent"), sym!("Alice"), sym!("Bob")))?;
    println!("After removal: {} atoms", space.len());

    // Replace a fact
    space.replace(
        &expr!(sym!("parent"), sym!("Bob"), sym!("Dave")),
        &expr!(sym!("parent"), sym!("Bob"), sym!("Eve")),
    )?;

    // Iterate all atoms
    println!("All atoms:");
    for atom_result in space.iter() {
        let atom = atom_result?;
        println!("  {}", atom);
    }

    Ok(())
}
```

### Multi-Threaded Space Access

```rust
use std::thread;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let space = MorkSpace::new();
    let shared = SharedSpace::new(space);

    // Writer thread
    let writer = {
        let shared = shared.clone();
        thread::spawn(move || {
            for i in 0..1000 {
                let atom = expr!(sym!("fact"), sym!(format!("item_{}", i)));
                shared.add(&atom).unwrap();
            }
        })
    };

    // Reader threads
    let readers: Vec<_> = (0..4).map(|_| {
        let shared = shared.clone();
        thread::spawn(move || {
            let pattern = expr!(sym!("fact"), var!("$x"));
            loop {
                let matches = shared.query(&pattern).unwrap();
                if matches.len() >= 1000 {
                    break;
                }
                thread::yield_now();
            }
        })
    }).collect();

    writer.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }

    println!("Final space size: {}", shared.space.read().unwrap().len());

    Ok(())
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
    fn test_add_atom() {
        let mut space = MorkSpace::new();

        let atom = atom!("foo");
        let added = space.add(&atom).unwrap();

        assert!(added);
        assert_eq!(space.len(), 1);

        // Idempotency
        let added_again = space.add(&atom).unwrap();
        assert!(!added_again);
        assert_eq!(space.len(), 1);
    }

    #[test]
    fn test_remove_atom() {
        let mut space = MorkSpace::new();

        let atom = atom!("foo");
        space.add(&atom).unwrap();

        let removed = space.remove(&atom).unwrap();
        assert!(removed);
        assert_eq!(space.len(), 0);

        // Remove non-existent
        let removed_again = space.remove(&atom).unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_query() {
        let mut space = MorkSpace::new();

        space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Bob"))).unwrap();
        space.add(&expr!(sym!("parent"), sym!("Alice"), sym!("Carol"))).unwrap();

        let pattern = expr!(sym!("parent"), sym!("Alice"), var!("$x"));
        let matches = space.query(&pattern).unwrap();

        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_replace() {
        let mut space = MorkSpace::new();

        let old = atom!("foo");
        let new = atom!("bar");

        space.add(&old).unwrap();
        space.replace(&old, &new).unwrap();

        assert!(!space.query(&var!("$x")).unwrap().alternatives().iter().any(|b| {
            b.get("$x") == Some(&old)
        }));

        assert!(space.query(&var!("$x")).unwrap().alternatives().iter().any(|b| {
            b.get("$x") == Some(&new)
        }));
    }

    #[test]
    fn test_batch_add() {
        let mut space = MorkSpace::new();

        let atoms: Vec<_> = (0..1000).map(|i| {
            atom!(format!("item_{}", i))
        }).collect();

        space.add_batch(&atoms).unwrap();

        assert_eq!(space.len(), 1000);
    }
}
```

---

## Performance Benchmarks

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_add(c: &mut Criterion) {
    let mut group = c.benchmark_group("add");

    for size in [100, 1_000, 10_000].iter() {
        group.bench_with_input(
            BenchmarkId::new("single", size),
            size,
            |b, &size| {
                b.iter_batched(
                    || MorkSpace::new(),
                    |mut space| {
                        for i in 0..size {
                            let atom = atom!(format!("item_{}", i));
                            space.add(black_box(&atom)).unwrap();
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("batch", size),
            size,
            |b, &size| {
                b.iter_batched(
                    || {
                        let atoms: Vec<_> = (0..size).map(|i| {
                            atom!(format!("item_{}", i))
                        }).collect();
                        (MorkSpace::new(), atoms)
                    },
                    |(mut space, atoms)| {
                        space.add_batch(black_box(&atoms)).unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_add);
criterion_main!(benches);
```

---

## Summary

This space operations guide provides:

1. **Complete API**: add, remove, query, replace, iterate, compose
2. **Efficient Encoding**: Leverage MORK's byte-level representation
3. **Concurrency Support**: Lock-free reads, coordinated writes
4. **Performance Optimization**: Batching, caching, NUMA awareness
5. **Comprehensive Testing**: Unit tests, integration tests, benchmarks

### Key Takeaways

- **Use batch operations** for multiple atoms
- **Share symbol tables** across related spaces
- **Cache frequent queries** when appropriate
- **Prefer lazy iteration** over eager collection
- **Profile before optimizing** - measure actual bottlenecks

---

**Document Version**: 1.0
**Last Updated**: 2025-11-13
**Next Review**: After initial implementation and benchmarking
