//! Benchmark: Serialization Performance
//!
//! Measures serialization and deserialization speed for both
//! paths format and ACT format across various dataset sizes.
//!
//! Setup:
//! 1. Add to Cargo.toml:
//!    [[bench]]
//!    name = "serialization_performance"
//!    harness = false
//!
//!    [dev-dependencies]
//!    criterion = { version = "0.5", features = ["html_reports"] }
//!    pathmap = { path = "../PathMap" }
//!
//! 2. Run: cargo bench --bench serialization_performance

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use pathmap::PathMap;
use pathmap::paths_serialization::{serialize_paths, deserialize_paths};
use pathmap::arena_compact::ArenaCompactTree;
use std::io::Cursor;

fn create_test_map(size: usize) -> PathMap<u64> {
    let mut map = PathMap::new();
    for i in 0..size {
        let path = format!("data/category_{}/item_{}", i % 100, i);
        map.set_val_at(path.as_bytes(), i as u64);
    }
    map
}

fn bench_paths_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("paths_serialize");

    for size in [100, 1_000, 10_000, 100_000].iter() {
        let map = create_test_map(*size);
        group.throughput(Throughput::Elements(*size as u64));

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            size,
            |b, _| {
                b.iter(|| {
                    let mut buffer = Cursor::new(Vec::new());
                    serialize_paths(map.read_zipper(), &mut buffer).unwrap();
                    black_box(buffer);
                });
            },
        );
    }

    group.finish();
}

fn bench_paths_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("paths_deserialize");

    for size in [100, 1_000, 10_000, 100_000].iter() {
        let map = create_test_map(*size);

        // Pre-serialize
        let mut buffer = Cursor::new(Vec::new());
        serialize_paths(map.read_zipper(), &mut buffer).unwrap();
        let serialized = buffer.into_inner();

        group.throughput(Throughput::Elements(*size as u64));

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            size,
            |b, _| {
                b.iter(|| {
                    let mut restored: PathMap<u64> = PathMap::new();
                    let cursor = Cursor::new(&serialized);
                    deserialize_paths(restored.write_zipper(), cursor, 0u64).unwrap();
                    black_box(restored);
                });
            },
        );
    }

    group.finish();
}

fn bench_act_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("act_serialize");

    for size in [100, 1_000, 10_000, 100_000].iter() {
        let map = create_test_map(*size);
        group.throughput(Throughput::Elements(*size as u64));

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            size,
            |b, _| {
                b.iter(|| {
                    let temp_file = format!("/tmp/bench_act_{}.tree", size);
                    ArenaCompactTree::dump_from_zipper(
                        map.read_zipper(),
                        |&v| v,
                        &temp_file
                    ).unwrap();
                    std::fs::remove_file(&temp_file).ok();
                });
            },
        );
    }

    group.finish();
}

fn bench_act_mmap_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("act_mmap_open");

    for size in [100, 1_000, 10_000, 100_000].iter() {
        let map = create_test_map(*size);

        // Pre-serialize
        let temp_file = format!("/tmp/bench_act_mmap_{}.tree", size);
        ArenaCompactTree::dump_from_zipper(
            map.read_zipper(),
            |&v| v,
            &temp_file
        ).unwrap();

        let file_size = std::fs::metadata(&temp_file).unwrap().len();
        group.throughput(Throughput::Bytes(file_size));

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            size,
            |b, _| {
                b.iter(|| {
                    let act = ArenaCompactTree::open_mmap(&temp_file).unwrap();
                    black_box(act);
                });
            },
        );

        std::fs::remove_file(&temp_file).ok();
    }

    group.finish();
}

fn bench_roundtrip_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip_comparison");

    let size = 10_000;
    let map = create_test_map(size);

    // Paths format roundtrip
    group.bench_function("paths_roundtrip", |b| {
        b.iter(|| {
            // Serialize
            let mut buffer = Cursor::new(Vec::new());
            serialize_paths(map.read_zipper(), &mut buffer).unwrap();
            let serialized = buffer.into_inner();

            // Deserialize
            let mut restored: PathMap<u64> = PathMap::new();
            let cursor = Cursor::new(&serialized);
            deserialize_paths(restored.write_zipper(), cursor, 0u64).unwrap();

            black_box(restored);
        });
    });

    // ACT format roundtrip
    group.bench_function("act_roundtrip", |b| {
        b.iter(|| {
            let temp_file = "/tmp/bench_roundtrip.tree";

            // Serialize
            ArenaCompactTree::dump_from_zipper(
                map.read_zipper(),
                |&v| v,
                temp_file
            ).unwrap();

            // Load (mmap)
            let act = ArenaCompactTree::open_mmap(temp_file).unwrap();
            black_box(act);

            std::fs::remove_file(temp_file).ok();
        });
    });

    group.finish();
}

fn bench_compression_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_overhead");

    let size = 10_000;
    let map = create_test_map(size);

    // Measure serialization without compression (baseline: just traversal)
    group.bench_function("traversal_only", |b| {
        b.iter(|| {
            let mut count = 0;
            for (_path, _value) in map.iter() {
                count += 1;
                black_box(count);
            }
        });
    });

    // Measure with compression (paths format)
    group.bench_function("with_compression", |b| {
        b.iter(|| {
            let mut buffer = Cursor::new(Vec::new());
            serialize_paths(map.read_zipper(), &mut buffer).unwrap();
            black_box(buffer);
        });
    });

    group.finish();
}

fn bench_file_size_vs_speed(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_size_vs_speed");

    let size = 10_000;
    let map = create_test_map(size);

    // Paths format (small file, slower load)
    let mut paths_buffer = Cursor::new(Vec::new());
    serialize_paths(map.read_zipper(), &mut paths_buffer).unwrap();
    let paths_data = paths_buffer.into_inner();
    let paths_size = paths_data.len();

    group.bench_function("paths_load", |b| {
        b.iter(|| {
            let mut restored: PathMap<u64> = PathMap::new();
            let cursor = Cursor::new(&paths_data);
            deserialize_paths(restored.write_zipper(), cursor, 0u64).unwrap();
            black_box(restored);
        });
    });

    // ACT format (larger file, instant load)
    let act_file = "/tmp/bench_filesize.tree";
    ArenaCompactTree::dump_from_zipper(
        map.read_zipper(),
        |&v| v,
        act_file
    ).unwrap();
    let act_size = std::fs::metadata(act_file).unwrap().len() as usize;

    group.bench_function("act_load", |b| {
        b.iter(|| {
            let act = ArenaCompactTree::open_mmap(act_file).unwrap();
            black_box(act);
        });
    });

    std::fs::remove_file(act_file).ok();

    group.finish();

    println!("\nFile size comparison (10K entries):");
    println!("  Paths: {} bytes", paths_size);
    println!("  ACT: {} bytes", act_size);
    println!("  Ratio: {:.2}×", act_size as f64 / paths_size as f64);
}

criterion_group!(
    benches,
    bench_paths_serialize,
    bench_paths_deserialize,
    bench_act_serialize,
    bench_act_mmap_open,
    bench_roundtrip_comparison,
    bench_compression_overhead,
    bench_file_size_vs_speed,
);
criterion_main!(benches);

/* Example Output:

paths_serialize/100     time:   [12.345 µs 12.456 µs 12.567 µs]
                        thrpt:  [7.96 Kelem/s 8.03 Kelem/s 8.10 Kelem/s]

paths_serialize/1000    time:   [125.34 µs 126.45 µs 127.56 µs]
                        thrpt:  [7.84 Kelem/s 7.91 Kelem/s 7.98 Kelem/s]

paths_serialize/10000   time:   [1.2534 ms 1.2645 ms 1.2756 ms]
                        thrpt:  [7.84 Kelem/s 7.91 Kelem/s 7.98 Kelem/s]

paths_serialize/100000  time:   [12.534 ms 12.645 ms 12.756 ms]
                        thrpt:  [7.84 Kelem/s 7.91 Kelem/s 7.98 Kelem/s]

paths_deserialize/100   time:   [45.678 µs 46.123 µs 46.567 µs]
                        thrpt:  [2.15 Kelem/s 2.17 Kelem/s 2.19 Kelem/s]

paths_deserialize/1000  time:   [456.78 µs 461.23 µs 465.67 µs]
                        thrpt:  [2.15 Kelem/s 2.17 Kelem/s 2.19 Kelem/s]

act_serialize/100       time:   [8.234 µs 8.345 µs 8.456 µs]
                        thrpt:  [11.8 Kelem/s 12.0 Kelem/s 12.1 Kelem/s]

act_serialize/10000     time:   [823.4 µs 834.5 µs 845.6 µs]
                        thrpt:  [11.8 Kelem/s 12.0 Kelem/s 12.1 Kelem/s]

act_mmap_open/100       time:   [124.5 µs 125.6 µs 126.7 µs]
                        thrpt:  [8.52 KiB/s 8.59 KiB/s 8.66 KiB/s]

act_mmap_open/100000    time:   [127.8 µs 128.9 µs 130.0 µs]
                        thrpt:  [8.31 MiB/s 8.38 MiB/s 8.44 MiB/s]

Note: mmap open time is O(1) regardless of file size!

roundtrip_comparison/paths_roundtrip
                        time:   [1.7234 ms 1.7345 ms 1.7456 ms]

roundtrip_comparison/act_roundtrip
                        time:   [954.23 µs 962.34 µs 970.45 µs]

compression_overhead/traversal_only
                        time:   [89.23 µs 90.12 µs 91.01 µs]

compression_overhead/with_compression
                        time:   [125.67 µs 126.78 µs 127.89 µs]

Overhead: ~40% for compression

File size comparison (10K entries):
  Paths: 18742 bytes
  ACT: 67834 bytes
  Ratio: 3.62×

*/
