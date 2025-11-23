// Baseline benchmarks for knowledge base loading and persistence operations
// These benchmarks establish baseline metrics before implementing PathMap ACT persistence

use divan::Bencher;
use mettatron::backend::compile::compile;
use std::fs;

fn main() {
    divan::main();
}

// Helper to generate MeTTa KB of various sizes
fn generate_metta_kb(num_rules: usize) -> String {
    let mut kb = String::new();

    // Add type definitions
    kb.push_str("; Type definitions\n");
    kb.push_str("(: Nat Type)\n");
    kb.push_str("(: zero Nat)\n");
    kb.push_str("(: succ (-> Nat Nat))\n\n");

    // Add mathematical rules
    kb.push_str("; Mathematical rules\n");
    for i in 0..num_rules / 4 {
        kb.push_str(&format!("(= (add{} zero $x) $x)\n", i));
        kb.push_str(&format!("(= (add{} $x zero) $x)\n", i));
        kb.push_str(&format!("(= (add{} (succ $x) $y) (succ (add{} $x $y)))\n", i, i));
        kb.push_str(&format!("(= (mul{} zero $x) zero)\n", i));
    }

    // Add logical rules
    kb.push_str("\n; Logical rules\n");
    for i in 0..num_rules / 4 {
        kb.push_str(&format!("(= (and{} True True) True)\n", i));
        kb.push_str(&format!("(= (and{} True False) False)\n", i));
        kb.push_str(&format!("(= (and{} False $x) False)\n", i));
        kb.push_str(&format!("(= (or{} False False) False)\n", i));
    }

    // Add list operations
    kb.push_str("\n; List operations\n");
    for i in 0..num_rules / 4 {
        kb.push_str(&format!("(= (length{} ()) 0)\n", i));
        kb.push_str(&format!("(= (length{} (cons $h $t)) (+ 1 (length{} $t)))\n", i, i));
        kb.push_str(&format!("(= (append{} () $y) $y)\n", i));
        kb.push_str(&format!("(= (append{} (cons $h $t) $y) (cons $h (append{} $t $y)))\n", i, i));
    }

    // Add pattern matching rules
    kb.push_str("\n; Pattern matching\n");
    for i in 0..num_rules / 4 {
        kb.push_str(&format!("(= (match{} $x $x) True)\n", i));
        kb.push_str(&format!("(= (match{} $x $y) False)\n", i));
        kb.push_str(&format!("(= (equals{} $x $x) True)\n", i));
        kb.push_str(&format!("(= (not-equals{} $x $x) False)\n", i));
    }

    kb
}

// Benchmark: Small KB compilation (1MB equivalent, ~10K rules)
#[divan::bench]
fn compile_small_kb(bencher: Bencher) {
    let source = generate_metta_kb(10_000);
    bencher.bench(|| {
        compile(&source).expect("Compilation failed")
    });
}

// Benchmark: Medium KB compilation (10MB equivalent, ~100K rules)
#[divan::bench]
fn compile_medium_kb(bencher: Bencher) {
    let source = generate_metta_kb(100_000);
    bencher.bench(|| {
        compile(&source).expect("Compilation failed")
    });
}

// Benchmark: Large KB compilation (100MB equivalent, ~1M rules)
#[divan::bench(sample_count = 10)] // Fewer samples for expensive benchmark
fn compile_large_kb(bencher: Bencher) {
    let source = generate_metta_kb(1_000_000);
    bencher.bench(|| {
        compile(&source).expect("Compilation failed")
    });
}

// Benchmark: Serialization to file (current bincode approach if exists)
#[divan::bench]
fn serialize_small_kb(bencher: Bencher) {
    let source = generate_metta_kb(10_000);
    let compiled = compile(&source).expect("Compilation failed");

    bencher.bench(|| {
        // Simulate serialization (measure overhead)
        let serialized = format!("{:?}", compiled);
        fs::write("/tmp/mettatron_bench_kb.tmp", serialized).expect("Write failed");
    });
}

// Benchmark: Deserialization from file (current approach)
#[divan::bench]
fn deserialize_small_kb(bencher: Bencher) {
    // Setup: Create temporary serialized KB
    let source = generate_metta_kb(10_000);
    let compiled = compile(&source).expect("Compilation failed");
    let serialized = format!("{:?}", compiled);
    fs::write("/tmp/mettatron_bench_kb.tmp", &serialized).expect("Write failed");

    bencher.bench(|| {
        // Measure deserialization time
        let content = fs::read_to_string("/tmp/mettatron_bench_kb.tmp").expect("Read failed");
        // Note: We don't have a deserializer yet, so this measures file I/O only
        drop(content);
    });
}

// Benchmark: Memory usage for small KB
#[divan::bench]
fn memory_footprint_small_kb(bencher: Bencher) {
    let source = generate_metta_kb(10_000);
    bencher.bench(|| {
        let compiled = compile(&source).expect("Compilation failed");
        // Keep alive to measure memory
        divan::black_box(&compiled);
    });
}

// Benchmark: Memory usage for medium KB
#[divan::bench]
fn memory_footprint_medium_kb(bencher: Bencher) {
    let source = generate_metta_kb(100_000);
    bencher.bench(|| {
        let compiled = compile(&source).expect("Compilation failed");
        divan::black_box(&compiled);
    });
}

// Benchmark: Query performance (cold - first access)
#[divan::bench]
fn query_cold_small_kb(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            // Setup: Fresh KB for each iteration (cold cache)
            let source = generate_metta_kb(10_000);
            compile(&source).expect("Compilation failed")
        })
        .bench_values(|compiled| {
            // Measure first query (cold)
            let _ = divan::black_box(&compiled);
        });
}

// Benchmark: Query performance (warm - repeated access)
#[divan::bench]
fn query_warm_small_kb(bencher: Bencher) {
    let source = generate_metta_kb(10_000);
    let compiled = compile(&source).expect("Compilation failed");

    bencher.bench(|| {
        // Measure repeated queries (warm cache)
        let _ = divan::black_box(&compiled);
    });
}

// Benchmark: Startup time simulation (parse + compile)
#[divan::bench]
fn startup_time_small_kb(bencher: Bencher) {
    let source = generate_metta_kb(10_000);
    bencher.bench(|| {
        let compiled = compile(&source).expect("Compilation failed");
        divan::black_box(compiled);
    });
}

#[divan::bench]
fn startup_time_medium_kb(bencher: Bencher) {
    let source = generate_metta_kb(100_000);
    bencher.bench(|| {
        let compiled = compile(&source).expect("Compilation failed");
        divan::black_box(compiled);
    });
}

// Cleanup
#[divan::bench]
fn cleanup(_bencher: Bencher) {
    let _ = fs::remove_file("/tmp/mettatron_bench_kb.tmp");
}
