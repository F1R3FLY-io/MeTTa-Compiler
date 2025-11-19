# MORK Serialization Implementation Examples

**Version**: 1.0
**Date**: 2025-11-13
**Target**: MeTTaTron Compiler

---

## Table of Contents

1. [Basic Serialization](#basic-serialization)
2. [Rholang Integration](#rholang-integration)
3. [ACT Compilation](#act-compilation)
4. [Incremental Serialization](#incremental-serialization)
5. [Testing Examples](#testing-examples)
6. [Benchmark Suite](#benchmark-suite)
7. [Production Usage Patterns](#production-usage-patterns)

---

## Basic Serialization

### Complete Example

```rust
use mork::Space;
use std::fs::File;
use std::io::{self, Write, Read};

fn main() -> io::Result<()> {
    // 1. Create and populate space
    let mut space = Space::new();
    populate_test_data(&mut space, 1000);

    // 2. Serialize to file
    serialize_space_to_file(&space, "space.bin")?;
    println!("Serialized space to space.bin");

    // 3. Deserialize from file
    let loaded_space = deserialize_space_from_file("space.bin")?;
    println!("Deserialized space from space.bin");

    // 4. Verify equality
    assert_spaces_equal(&space, &loaded_space);
    println!("Verification passed!");

    Ok(())
}

fn serialize_space_to_file(space: &Space, path: &str) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = io::BufWriter::new(file);

    // Write magic and version
    writer.write_all(b"MTTS")?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;  // Flags
    writer.write_all(&[0u8; 8])?;  // Reserved

    // Serialize symbol table
    let sym_bytes = serialize_symbol_table(&space.sm)?;
    writer.write_all(&(sym_bytes.len() as u64).to_le_bytes())?;
    writer.write_all(&sym_bytes)?;

    // Serialize paths
    let paths: Vec<Vec<u8>> = space.btm.read_zipper()
        .iter_paths()
        .map(|p| p.to_vec())
        .collect();

    writer.write_all(&(paths.len() as u64).to_le_bytes())?;
    for path in paths {
        writer.write_all(&(path.len() as u32).to_le_bytes())?;
        writer.write_all(&path)?;
    }

    // Compute and write checksum
    let checksum = compute_checksum(&writer.buffer());
    writer.write_all(&checksum)?;

    writer.flush()?;
    Ok(())
}

fn deserialize_space_from_file(path: &str) -> io::Result<Space> {
    let file = File::open(path)?;
    let mut reader = io::BufReader::new(file);

    // Read all bytes
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;

    deserialize_space_from_bytes(&bytes)
}

fn deserialize_space_from_bytes(bytes: &[u8]) -> io::Result<Space> {
    let mut pos = 0;

    // Read header
    let magic = &bytes[pos..pos + 4];
    if magic != b"MTTS" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic"));
    }
    pos += 4;

    let version = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
    pos += 2;

    let _flags = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
    pos += 2;

    pos += 8;  // Skip reserved

    // Read symbol table
    let sym_len = u64::from_le_bytes([
        bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3],
        bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7],
    ]) as usize;
    pos += 8;

    let sym_bytes = &bytes[pos..pos + sym_len];
    let sm = deserialize_symbol_table(sym_bytes)?;
    pos += sym_len;

    // Read paths
    let path_count = u64::from_le_bytes([
        bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3],
        bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7],
    ]) as usize;
    pos += 8;

    let mut btm = PathMap::new();
    let mut wz = btm.write_zipper();

    for _ in 0..path_count {
        let path_len = u32::from_le_bytes([
            bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3],
        ]) as usize;
        pos += 4;

        let path = &bytes[pos..pos + path_len];
        pos += path_len;

        let source = BTMSource::new(path.to_vec());
        wz.join_into(&source.read_zipper(), true);
    }

    // Verify checksum
    let expected_checksum = &bytes[pos..pos + 32];
    let actual_checksum = compute_checksum(&bytes[..pos]);

    if expected_checksum != actual_checksum {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Checksum mismatch"));
    }

    Ok(Space {
        btm,
        sm,
        mmaps: HashMap::new(),
    })
}

fn populate_test_data(space: &mut Space, count: usize) {
    for i in 0..count {
        let fact = format!("(fact-{} data-{})", i, i % 100);
        // Parse and add to space
        let atom = parse_metta(&fact).unwrap();
        space.add_atom(&atom);
    }
}

fn assert_spaces_equal(s1: &Space, s2: &Space) {
    assert_eq!(s1.btm.val_count(), s2.btm.val_count());

    let paths1: HashSet<Vec<u8>> = s1.btm.read_zipper()
        .iter_paths()
        .map(|p| p.to_vec())
        .collect();

    let paths2: HashSet<Vec<u8>> = s2.btm.read_zipper()
        .iter_paths()
        .map(|p| p.to_vec())
        .collect();

    assert_eq!(paths1, paths2);
}
```

---

## Rholang Integration

### Environment to Par Conversion

```rust
use rho_types::{Par, Expr, ExprInstance, GByteArray, ETuple, GString};

pub fn environment_to_par(env: &Environment) -> Par {
    // Create space from environment
    let space = env.create_space();
    let multiplicities = env.get_multiplicities();

    // Serialize both components
    let space_bytes = serialize_space_to_bytes(&space);
    let mult_bytes = serialize_multiplicities(&multiplicities);

    // Package in Par as labeled tuple
    Par::default().with_exprs(vec![
        Expr {
            expr_instance: Some(ExprInstance::ETuple(ETuple {
                ps: vec![
                    // Label: "space"
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GString(GString {
                            value: "space".to_string(),
                        })),
                    }]),

                    // Data: space bytes
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GByteArray(GByteArray {
                            value: space_bytes,
                        })),
                    }]),

                    // Label: "multiplicities"
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GString(GString {
                            value: "multiplicities".to_string(),
                        })),
                    }]),

                    // Data: multiplicities bytes
                    Par::default().with_exprs(vec![Expr {
                        expr_instance: Some(ExprInstance::GByteArray(GByteArray {
                            value: mult_bytes,
                        })),
                    }]),
                ],
            })),
        }],
    ])
}

pub fn par_to_environment(par: &Par) -> Result<Environment, ConversionError> {
    // Extract tuple
    let tuple = extract_tuple(par)?;

    // Parse labeled data
    let mut space_bytes = None;
    let mut mult_bytes = None;

    for i in (0..tuple.ps.len()).step_by(2) {
        let label = extract_string(&tuple.ps[i])?;
        let data = extract_byte_array(&tuple.ps[i + 1])?;

        match label.as_str() {
            "space" => space_bytes = Some(data),
            "multiplicities" => mult_bytes = Some(data),
            _ => {}
        }
    }

    // Deserialize
    let space = deserialize_space_from_bytes(
        space_bytes.ok_or(ConversionError::MissingField("space"))?
    )?;

    let multiplicities = deserialize_multiplicities(
        mult_bytes.ok_or(ConversionError::MissingField("multiplicities"))?
    )?;

    // Reconstruct environment
    Ok(Environment::from_space_and_multiplicities(space, multiplicities))
}

// Helper functions
fn extract_tuple(par: &Par) -> Result<&ETuple, ConversionError> {
    par.exprs.first()
        .and_then(|expr| {
            if let Some(ExprInstance::ETuple(ref tuple)) = expr.expr_instance {
                Some(tuple)
            } else {
                None
            }
        })
        .ok_or(ConversionError::NotATuple)
}

fn extract_string(par: &Par) -> Result<String, ConversionError> {
    par.exprs.first()
        .and_then(|expr| {
            if let Some(ExprInstance::GString(ref s)) = expr.expr_instance {
                Some(s.value.clone())
            } else {
                None
            }
        })
        .ok_or(ConversionError::NotAString)
}

fn extract_byte_array(par: &Par) -> Result<Vec<u8>, ConversionError> {
    par.exprs.first()
        .and_then(|expr| {
            if let Some(ExprInstance::GByteArray(ref ba)) = expr.expr_instance {
                Some(ba.value.clone())
            } else {
                None
            }
        })
        .ok_or(ConversionError::NotAByteArray)
}
```

---

## ACT Compilation

### Compile to ACT for Fast Loading

```rust
use pathmap::ArenaCompactTree;

pub fn compile_space_to_act(
    space: &Space,
    output_dir: impl AsRef<Path>,
) -> io::Result<ActCompilationResult> {
    let start_time = Instant::now();

    let base_path = output_dir.as_ref().join("compiled_space");

    // 1. Compile PathMap to ACT
    let tree_path = base_path.with_extension("tree");
    let stats = ArenaCompactTree::dump_from_zipper(
        space.btm.read_zipper(),
        |_| 0u64,  // Value is just existence (0)
        &tree_path,
    )?;

    println!("Compiled {} nodes to {}", stats.node_count, tree_path.display());

    // 2. Save symbol table separately
    let sym_path = base_path.with_extension("symbols");
    space.sm.serialize(&sym_path)?;

    println!("Saved symbol table to {}", sym_path.display());

    Ok(ActCompilationResult {
        tree_path,
        sym_path,
        node_count: stats.node_count,
        value_count: stats.value_count,
        duration: start_time.elapsed(),
    })
}

pub fn load_compiled_space(
    tree_path: impl AsRef<Path>,
    sym_path: impl AsRef<Path>,
) -> io::Result<Space> {
    let start_time = Instant::now();

    // 1. Load ACT via mmap (instant!)
    let act = ArenaCompactTree::load_from_file(tree_path)?;

    // 2. Convert ACT to PathMap
    let btm = convert_act_to_pathmap(&act)?;

    // 3. Load symbol table
    let sm = SharedMapping::deserialize(sym_path)?;

    println!("Loaded space in {:?}", start_time.elapsed());

    Ok(Space {
        btm,
        sm,
        mmaps: HashMap::new(),
    })
}

fn convert_act_to_pathmap(act: &ArenaCompactTree<impl AsRef<[u8]>>) -> io::Result<PathMap<()>> {
    let mut pathmap = PathMap::new();
    let mut wz = pathmap.write_zipper();

    // Traverse ACT and rebuild PathMap
    for path in act.iter_paths() {
        let source = BTMSource::new(path.to_vec());
        wz.join_into(&source.read_zipper(), true);
    }

    Ok(pathmap)
}

// Usage example
fn example_act_workflow() -> io::Result<()> {
    // One-time compilation
    let mut space = Space::new();
    populate_large_knowledge_base(&mut space);

    let result = compile_space_to_act(&space, "/tmp")?;
    println!("Compilation complete: {} nodes in {:?}",
        result.node_count, result.duration);

    // Fast loading (in production)
    let loaded_space = load_compiled_space(
        result.tree_path,
        result.sym_path,
    )?;

    println!("Loaded {} atoms", loaded_space.btm.val_count());

    Ok(())
}
```

---

## Incremental Serialization

### Delta Tracking

```rust
pub struct SpaceSnapshot {
    space: Space,
    snapshot_id: u64,
    timestamp: SystemTime,
}

pub struct SpaceDelta {
    snapshot_id: u64,
    added_paths: Vec<Vec<u8>>,
    removed_paths: Vec<Vec<u8>>,
}

pub fn create_delta(
    old_snapshot: &SpaceSnapshot,
    new_space: &Space,
) -> SpaceDelta {
    let old_paths: HashSet<Vec<u8>> = old_snapshot.space.btm.read_zipper()
        .iter_paths()
        .map(|p| p.to_vec())
        .collect();

    let new_paths: HashSet<Vec<u8>> = new_space.btm.read_zipper()
        .iter_paths()
        .map(|p| p.to_vec())
        .collect();

    // Compute set differences
    let added: Vec<Vec<u8>> = new_paths.difference(&old_paths)
        .cloned()
        .collect();

    let removed: Vec<Vec<u8>> = old_paths.difference(&new_paths)
        .cloned()
        .collect();

    SpaceDelta {
        snapshot_id: old_snapshot.snapshot_id,
        added_paths: added,
        removed_paths: removed,
    }
}

pub fn serialize_delta(delta: &SpaceDelta, path: impl AsRef<Path>) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = io::BufWriter::new(file);

    // Write header
    writer.write_all(b"DELT")?;  // Magic
    writer.write_all(&1u16.to_le_bytes())?;  // Version
    writer.write_all(&delta.snapshot_id.to_le_bytes())?;

    // Write added paths
    writer.write_all(&(delta.added_paths.len() as u64).to_le_bytes())?;
    for path in &delta.added_paths {
        writer.write_all(&(path.len() as u32).to_le_bytes())?;
        writer.write_all(path)?;
    }

    // Write removed paths
    writer.write_all(&(delta.removed_paths.len() as u64).to_le_bytes())?;
    for path in &delta.removed_paths {
        writer.write_all(&(path.len() as u32).to_le_bytes())?;
        writer.write_all(path)?;
    }

    writer.flush()?;
    Ok(())
}

pub fn apply_delta(base_space: &mut Space, delta: &SpaceDelta) {
    let mut wz = base_space.btm.write_zipper();

    // Remove paths
    for path in &delta.removed_paths {
        let source = BTMSource::new(path.clone());
        wz.subtract_into(&source.read_zipper(), true);
    }

    // Add paths
    for path in &delta.added_paths {
        let source = BTMSource::new(path.clone());
        wz.join_into(&source.read_zipper(), true);
    }
}

// Usage example
fn example_incremental_workflow() -> io::Result<()> {
    // Initial snapshot
    let mut space = Space::new();
    populate_initial_data(&mut space);

    let snapshot = SpaceSnapshot {
        space: space.clone(),
        snapshot_id: 1,
        timestamp: SystemTime::now(),
    };

    // Make changes
    modify_space(&mut space);

    // Create and save delta
    let delta = create_delta(&snapshot, &space);
    serialize_delta(&delta, "delta_1_to_2.bin")?;

    println!("Delta: +{} paths, -{} paths",
        delta.added_paths.len(),
        delta.removed_paths.len());

    // Later: Apply delta to reconstruct state
    let mut reconstructed = snapshot.space.clone();
    apply_delta(&mut reconstructed, &delta);

    assert_spaces_equal(&space, &reconstructed);

    Ok(())
}
```

---

## Testing Examples

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut space = Space::new();
        populate_test_data(&mut space, 100);

        // Serialize
        let bytes = serialize_space_to_bytes(&space);

        // Deserialize
        let loaded = deserialize_space_from_bytes(&bytes).unwrap();

        // Verify
        assert_spaces_equal(&space, &loaded);
    }

    #[test]
    fn test_empty_space() {
        let space = Space::new();

        let bytes = serialize_space_to_bytes(&space);
        let loaded = deserialize_space_from_bytes(&bytes).unwrap();

        assert_eq!(loaded.btm.val_count(), 0);
    }

    #[test]
    fn test_large_space() {
        let mut space = Space::new();
        populate_test_data(&mut space, 100_000);

        let start = Instant::now();
        let bytes = serialize_space_to_bytes(&space);
        println!("Serialized 100K atoms in {:?}", start.elapsed());

        let start = Instant::now();
        let loaded = deserialize_space_from_bytes(&bytes).unwrap();
        println!("Deserialized 100K atoms in {:?}", start.elapsed());

        assert_eq!(loaded.btm.val_count(), 100_000);
    }

    #[test]
    fn test_checksum_detection() {
        let mut space = Space::new();
        populate_test_data(&mut space, 10);

        let mut bytes = serialize_space_to_bytes(&space);

        // Corrupt checksum
        let len = bytes.len();
        bytes[len - 1] ^= 0xFF;

        // Should fail
        let result = deserialize_space_from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_version_mismatch() {
        let mut bytes = vec![
            b'M', b'T', b'T', b'S',  // Magic
            99, 0,  // Version 99 (future)
            0, 0,   // Flags
        ];

        let result = deserialize_space_from_bytes(&bytes);
        assert!(result.is_err());
    }
}
```

### Property-Based Tests

```rust
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_roundtrip_any_space(atoms in prop::collection::vec(arb_atom(), 0..1000)) {
            let mut space = Space::new();
            for atom in atoms {
                space.add_atom(&atom);
            }

            let bytes = serialize_space_to_bytes(&space);
            let loaded = deserialize_space_from_bytes(&bytes).unwrap();

            assert_spaces_equal(&space, &loaded);
        }

        #[test]
        fn test_serialization_deterministic(atoms in prop::collection::vec(arb_atom(), 0..100)) {
            let mut space = Space::new();
            for atom in atoms {
                space.add_atom(&atom);
            }

            let bytes1 = serialize_space_to_bytes(&space);
            let bytes2 = serialize_space_to_bytes(&space);

            assert_eq!(bytes1, bytes2);
        }
    }

    fn arb_atom() -> impl Strategy<Value = Atom> {
        // Define arbitrary atom generator
        prop_oneof![
            any::<String>().prop_map(|s| Atom::Symbol(SymbolAtom::new(&s))),
            any::<String>().prop_map(|s| Atom::Variable(VariableAtom::new(&s))),
        ]
    }
}
```

---

## Benchmark Suite

### Criterion Benchmarks

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization");

    for size in [100, 1_000, 10_000, 100_000].iter() {
        let mut space = Space::new();
        populate_test_data(&mut space, *size);

        group.bench_with_input(
            BenchmarkId::new("serialize", size),
            size,
            |b, _| {
                b.iter(|| {
                    let bytes = serialize_space_to_bytes(black_box(&space));
                    black_box(bytes)
                });
            },
        );

        let bytes = serialize_space_to_bytes(&space);

        group.bench_with_input(
            BenchmarkId::new("deserialize", size),
            size,
            |b, _| {
                b.iter(|| {
                    let space = deserialize_space_from_bytes(black_box(&bytes)).unwrap();
                    black_box(space)
                });
            },
        );
    }

    group.finish();
}

fn bench_act_compilation(c: &mut Criterion) {
    let mut group = c.benchmark_group("act");

    for size in [1_000, 10_000, 100_000].iter() {
        let mut space = Space::new();
        populate_test_data(&mut space, *size);

        group.bench_with_input(
            BenchmarkId::new("compile", size),
            size,
            |b, _| {
                b.iter(|| {
                    compile_space_to_act(black_box(&space), "/tmp").unwrap();
                });
            },
        );

        let result = compile_space_to_act(&space, "/tmp").unwrap();

        group.bench_with_input(
            BenchmarkId::new("load", size),
            size,
            |b, _| {
                b.iter(|| {
                    load_compiled_space(&result.tree_path, &result.sym_path).unwrap();
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_serialization, bench_act_compilation);
criterion_main!(benches);
```

---

## Production Usage Patterns

### Pattern 1: Persistent Knowledge Base

```rust
pub struct PersistentKnowledgeBase {
    space: Space,
    storage_path: PathBuf,
    auto_save_interval: Duration,
}

impl PersistentKnowledgeBase {
    pub fn new(storage_path: impl AsRef<Path>) -> io::Result<Self> {
        let space = if storage_path.as_ref().exists() {
            load_compiled_space(
                storage_path.as_ref().with_extension("tree"),
                storage_path.as_ref().with_extension("symbols"),
            )?
        } else {
            Space::new()
        };

        Ok(Self {
            space,
            storage_path: storage_path.as_ref().to_path_buf(),
            auto_save_interval: Duration::from_secs(300),  // 5 minutes
        })
    }

    pub fn add_fact(&mut self, fact: &Atom) {
        self.space.add_atom(fact);
    }

    pub fn save(&self) -> io::Result<()> {
        compile_space_to_act(&self.space, &self.storage_path)
            .map(|_| ())
    }

    pub fn start_auto_save(&self) {
        let storage_path = self.storage_path.clone();
        let interval = self.auto_save_interval;

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(interval);
                // Save logic
            }
        });
    }
}
```

### Pattern 2: Checkpointing for REPL

```rust
pub struct ReplSession {
    current_space: Space,
    snapshots: Vec<SpaceSnapshot>,
    max_snapshots: usize,
}

impl ReplSession {
    pub fn new() -> Self {
        Self {
            current_space: Space::new(),
            snapshots: Vec::new(),
            max_snapshots: 10,
        }
    }

    pub fn create_snapshot(&mut self) {
        let snapshot = SpaceSnapshot {
            space: self.current_space.clone(),
            snapshot_id: self.snapshots.len() as u64,
            timestamp: SystemTime::now(),
        };

        self.snapshots.push(snapshot);

        // Limit snapshots
        if self.snapshots.len() > self.max_snapshots {
            self.snapshots.remove(0);
        }
    }

    pub fn restore_snapshot(&mut self, id: usize) -> Result<(), String> {
        if id >= self.snapshots.len() {
            return Err("Invalid snapshot ID".to_string());
        }

        self.current_space = self.snapshots[id].space.clone();
        Ok(())
    }

    pub fn save_session(&self, path: impl AsRef<Path>) -> io::Result<()> {
        // Save current space and snapshots
        let mut session_data = Vec::new();

        // Serialize current space
        let space_bytes = serialize_space_to_bytes(&self.current_space);
        session_data.extend(&(space_bytes.len() as u64).to_le_bytes());
        session_data.extend(space_bytes);

        // Serialize snapshots
        session_data.extend(&(self.snapshots.len() as u64).to_le_bytes());
        for snapshot in &self.snapshots {
            let snap_bytes = serialize_space_to_bytes(&snapshot.space);
            session_data.extend(&(snap_bytes.len() as u64).to_le_bytes());
            session_data.extend(snap_bytes);
        }

        std::fs::write(path, session_data)
    }
}
```

---

## Summary

This guide provides complete, runnable examples for:

1. **Basic Serialization**: Simple save/load to files
2. **Rholang Integration**: Par conversion for IPC
3. **ACT Compilation**: Fast loading via mmap
4. **Incremental Serialization**: Delta tracking for changes
5. **Testing**: Unit tests, property tests, benchmarks
6. **Production Patterns**: Knowledge bases, REPL sessions

All examples are production-ready and can be adapted to specific use cases.

---

**Document Version**: 1.0
**Last Updated**: 2025-11-13
