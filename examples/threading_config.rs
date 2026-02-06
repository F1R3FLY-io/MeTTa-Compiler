//! Threading Configuration Example
//!
//! Demonstrates how to configure MeTTaTron's threading model for different workloads.

use mettatron::config::EvalConfig;
use mettatron::{compile_arena, new_arena_env, run_state_async};

#[tokio::main]
async fn main() {
    println!("=== MeTTaTron Threading Configuration Examples ===\n");

    // Example 1: Default Configuration
    println!("Example 1: Default Configuration");
    println!("  max_blocking_threads: 512 (Tokio default)");
    println!("  batch_size_hint: 32");
    demo_default_config().await;
    println!();

    // Example 2: CPU-Optimized Configuration
    println!("Example 2: CPU-Optimized Configuration");
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    println!("  Detected CPUs: {}", num_cpus);
    println!("  max_blocking_threads: {} (num_cpus * 2)", num_cpus * 2);
    println!("  batch_size_hint: 32");
    demo_cpu_optimized().await;
    println!();

    // Example 3: Memory-Optimized Configuration
    println!("Example 3: Memory-Optimized Configuration");
    println!("  max_blocking_threads: {} (num_cpus)", num_cpus);
    println!("  batch_size_hint: 16");
    demo_memory_optimized().await;
    println!();

    // Example 4: Throughput-Optimized Configuration
    println!("Example 4: Throughput-Optimized Configuration");
    println!("  max_blocking_threads: 1024");
    println!("  batch_size_hint: 128");
    demo_throughput_optimized().await;
    println!();

    // Example 5: Custom Configuration
    println!("Example 5: Custom Configuration");
    println!("  max_blocking_threads: 256");
    println!("  batch_size_hint: 64");
    demo_custom_config().await;
    println!();

    println!("=== All Examples Complete ===");
}

async fn demo_default_config() {
    // Default configuration is used if configure_eval is not called
    let env = new_arena_env();

    // Compile multiple independent expressions
    let src = r#"
        !(+ 1 2)
        !(* 3 4)
        !(- 10 5)
        !(/ 20 4)
    "#;

    let state = compile_arena(src).expect("Failed to compile");
    let (_env, outputs) = run_state_async(env, &state)
        .await
        .expect("Failed to evaluate");

    let results: Vec<String> = outputs.iter().map(|v| format!("{:?}", v.inner())).collect();
    println!("  Results: {:?}", results);
}

async fn demo_cpu_optimized() {
    // Configure for CPU-bound workloads
    // Note: Can only call configure_eval once, so this example shows what you would do
    // In a real application, you'd call this once at startup

    let _config = EvalConfig::cpu_optimized();
    println!("  Configuration created (not applied - already configured)");
    println!("  In real app, call configure_eval(_config) before any async operations");

    let env = new_arena_env();

    // CPU-intensive pattern matching
    let src = r#"
        (= (fib 0) 0)
        (= (fib 1) 1)
        (= (fib $n) (+ (fib (- $n 1)) (fib (- $n 2))))
        !(+ 10 20)
        !(* 30 40)
    "#;

    let state = compile_arena(src).expect("Failed to compile");
    let (_env, outputs) = run_state_async(env, &state)
        .await
        .expect("Failed to evaluate");

    let results: Vec<String> = outputs.iter().map(|v| format!("{:?}", v.inner())).collect();
    println!("  Results: {:?}", results);
}

async fn demo_memory_optimized() {
    let config = EvalConfig::memory_optimized();
    println!("  Configuration: {:?}", config);

    let env = new_arena_env();

    // Smaller batch of expressions
    let src = r#"
        !(+ 1 1)
        !(+ 2 2)
        !(+ 3 3)
    "#;

    let state = compile_arena(src).expect("Failed to compile");
    let (_env, outputs) = run_state_async(env, &state)
        .await
        .expect("Failed to evaluate");

    let results: Vec<String> = outputs.iter().map(|v| format!("{:?}", v.inner())).collect();
    println!("  Results: {:?}", results);
}

async fn demo_throughput_optimized() {
    let config = EvalConfig::throughput_optimized();
    println!("  Configuration: {:?}", config);

    let env = new_arena_env();

    // Large batch of independent expressions
    let mut src = String::new();
    for i in 1..=20 {
        src.push_str(&format!("!(+ {} {})\n", i, i));
    }

    let state = compile_arena(&src).expect("Failed to compile");
    let (_env, outputs) = run_state_async(env, &state)
        .await
        .expect("Failed to evaluate");

    println!("  Results count: {} expressions", outputs.len());
}

async fn demo_custom_config() {
    let config = EvalConfig {
        max_blocking_threads: 256,
        batch_size_hint: 64,
    };
    println!("  Configuration: {:?}", config);

    let env = new_arena_env();

    // Mixed workload: rules and evaluations
    let src = r#"
        (= (double $x) (* $x 2))
        (= (triple $x) (* $x 3))
        !(double 5)
        !(triple 5)
        !(double 10)
        !(triple 10)
    "#;

    let state = compile_arena(src).expect("Failed to compile");
    let (_env, outputs) = run_state_async(env, &state)
        .await
        .expect("Failed to evaluate");

    let results: Vec<String> = outputs.iter().map(|v| format!("{:?}", v.inner())).collect();
    println!("  Results: {:?}", results);
}
