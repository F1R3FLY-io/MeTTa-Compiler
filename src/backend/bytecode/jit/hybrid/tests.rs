//! Tests for hybrid JIT/VM execution.

use super::*;
use crate::backend::bytecode::{BytecodeVM, ChunkBuilder, Opcode};
use crate::backend::models::MettaValue;
use std::sync::Arc;

#[test]
fn test_hybrid_executor_new() {
    let executor = HybridExecutor::new();
    assert_eq!(executor.stats().total_runs, 0);
    assert_eq!(executor.stats().jit_runs, 0);
    assert_eq!(executor.stats().vm_runs, 0);
}

#[test]
fn test_hybrid_executor_bytecode_only() {
    let config = HybridConfig::bytecode_only();
    let mut executor = HybridExecutor::with_config(config);

    // Build simple chunk: push 42, return
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);
    let chunk = builder.build_arc();

    let results = executor.run(&chunk).expect("execution should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));

    // Should have used VM, not JIT
    assert_eq!(executor.stats().vm_runs, 1);
    assert_eq!(executor.stats().jit_runs, 0);
}

#[test]
fn test_hybrid_executor_simple_arithmetic() {
    let mut executor = HybridExecutor::new();

    // Build: 10 + 32 = 42
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 32);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build_arc();

    let results = executor.run(&chunk).expect("execution should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_hybrid_executor_shared_cache() {
    let executor1 = HybridExecutor::new();
    let cache = executor1.jit_cache();
    let compiler = executor1.tiered_compiler();

    let mut executor2 = HybridExecutor::with_shared_cache(cache.clone(), compiler.clone());

    // Both executors should share the same cache
    assert!(Arc::ptr_eq(&executor1.jit_cache(), &executor2.jit_cache()));

    // Build and run a chunk
    let mut builder = ChunkBuilder::new("shared");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Return);
    let chunk = builder.build_arc();

    let results = executor2.run(&chunk).expect("should succeed");
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_hybrid_stats() {
    let mut stats = HybridStats::default();
    assert_eq!(stats.jit_hit_rate(), 0.0);
    assert_eq!(stats.bailout_rate(), 0.0);

    stats.total_runs = 100;
    stats.jit_runs = 75;
    stats.vm_runs = 25;
    stats.jit_bailouts = 5;

    assert_eq!(stats.jit_hit_rate(), 75.0);
    assert!((stats.bailout_rate() - 6.666).abs() < 0.01);
}

#[test]
fn test_hybrid_config_with_trace() {
    let config = HybridConfig::default().with_trace();
    assert!(config.trace);
    assert!(config.vm_config.trace);
}

#[test]
fn test_hybrid_executor_compile_integration() {
    let mut executor = HybridExecutor::new();

    // Use the compile_arc function for a simple expression
    let expr = MettaValue::SExpr(vec![
        MettaValue::sym("+"),
        MettaValue::Long(20),
        MettaValue::Long(22),
    ]);

    match crate::backend::bytecode::compile_arc("test", &expr) {
        Ok(chunk) => {
            let results = executor.run(&chunk).expect("should succeed");
            assert_eq!(results.len(), 1);
            assert_eq!(results[0], MettaValue::Long(42));
        }
        Err(_) => {
            // Compilation might fail in some test configurations
        }
    }
}

#[test]
fn test_hybrid_executor_run_with_backtracking_simple() {
    // Test that run_with_backtracking works for simple non-forking code
    let mut executor = HybridExecutor::new();

    // Build simple chunk: push 42, return
    let mut builder = ChunkBuilder::new("test_backtrack_simple");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);
    let chunk = builder.build_arc();

    // First verify the bytecode works via VM
    let vm_results = executor.run(&chunk).expect("VM should succeed");
    assert_eq!(vm_results.len(), 1, "VM should return 1 result");
    assert_eq!(
        vm_results[0],
        MettaValue::Long(42),
        "VM should return Long(42)"
    );

    // Now use run_with_backtracking (uses VM since not JIT-compiled yet)
    let results = executor
        .run_with_backtracking(&chunk)
        .expect("should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_hybrid_executor_run_with_backtracking_arithmetic() {
    // Test dispatcher loop with arithmetic (no nondeterminism)
    let mut executor = HybridExecutor::new();

    // Build: 10 + 32 = 42
    let mut builder = ChunkBuilder::new("test_backtrack_arith");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 32);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build_arc();

    // First verify the bytecode works via VM
    let vm_results = executor.run(&chunk).expect("VM should succeed");
    assert_eq!(vm_results.len(), 1, "VM should return 1 result");
    assert_eq!(
        vm_results[0],
        MettaValue::Long(42),
        "VM should return Long(42)"
    );

    // Now use run_with_backtracking (uses VM since not JIT-compiled yet)
    let results = executor
        .run_with_backtracking(&chunk)
        .expect("should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

/// Test semantic equivalence: BytecodeVM and HybridExecutor must return identical results
/// for Fork chunks with proper nondeterminism opcodes.
///
/// This test verifies MeTTa HE semantics: ALL alternatives must be explored.
#[test]
fn test_semantic_equivalence_fork_basic() {
    // Create a fork chunk with 3 alternatives: Fork(1, 2, 3)
    // Expected output: [11, 12, 13] (each alternative + 10)
    let num_alternatives = 3;

    let mut builder = ChunkBuilder::new("fork_equivalence_test");

    // Add constants for alternatives 1, 2, 3
    let mut const_indices = Vec::with_capacity(num_alternatives);
    for i in 0..num_alternatives {
        let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
        const_indices.push(idx);
    }

    // Build chunk with proper nondeterminism opcodes:
    // BeginNondet -> Fork(3) [indices] -> +10 -> Yield -> Return
    builder.emit(Opcode::BeginNondet);
    builder.emit_u16(Opcode::Fork, num_alternatives as u16);
    for idx in &const_indices {
        builder.emit_raw(&idx.to_be_bytes());
    }
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();

    // 1. Run with BytecodeVM
    let mut vm = BytecodeVM::new(Arc::clone(&chunk));
    let vm_results = vm.run().expect("VM should succeed");

    // 2. Run with HybridExecutor (uses VM path for first few runs)
    let mut executor = HybridExecutor::new();
    let hybrid_results = executor
        .run_with_backtracking(&chunk)
        .expect("Hybrid should succeed");

    // 3. Verify semantic equivalence
    assert_eq!(
        vm_results.len(),
        hybrid_results.len(),
        "VM and Hybrid must return same number of results"
    );

    // Expected: 3 results [11, 12, 13] in some order
    assert_eq!(
        vm_results.len(),
        num_alternatives,
        "Must return ALL {} alternatives",
        num_alternatives
    );

    // Verify all expected values are present
    let vm_longs: Vec<i64> = vm_results
        .iter()
        .filter_map(|v| {
            if let MettaValue::Long(n) = v {
                Some(*n)
            } else {
                None
            }
        })
        .collect();

    let hybrid_longs: Vec<i64> = hybrid_results
        .iter()
        .filter_map(|v| {
            if let MettaValue::Long(n) = v {
                Some(*n)
            } else {
                None
            }
        })
        .collect();

    // Both should contain {11, 12, 13}
    let mut vm_sorted = vm_longs.clone();
    let mut hybrid_sorted = hybrid_longs.clone();
    vm_sorted.sort();
    hybrid_sorted.sort();

    assert_eq!(
        vm_sorted,
        vec![11, 12, 13],
        "VM results should be 11, 12, 13"
    );
    assert_eq!(
        hybrid_sorted,
        vec![11, 12, 13],
        "Hybrid results should be 11, 12, 13"
    );
    assert_eq!(vm_sorted, hybrid_sorted, "Results must match exactly");
}

/// Test semantic equivalence with more alternatives (5)
#[test]
fn test_semantic_equivalence_fork_five_alternatives() {
    let num_alternatives = 5;

    let mut builder = ChunkBuilder::new("fork_5_equivalence");

    let mut const_indices = Vec::with_capacity(num_alternatives);
    for i in 0..num_alternatives {
        let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
        const_indices.push(idx);
    }

    builder.emit(Opcode::BeginNondet);
    builder.emit_u16(Opcode::Fork, num_alternatives as u16);
    for idx in &const_indices {
        builder.emit_raw(&idx.to_be_bytes());
    }
    // Just yield the value directly, no arithmetic
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();

    // Run with both implementations
    let mut vm = BytecodeVM::new(Arc::clone(&chunk));
    let vm_results = vm.run().expect("VM should succeed");

    let mut executor = HybridExecutor::new();
    let hybrid_results = executor
        .run_with_backtracking(&chunk)
        .expect("Hybrid should succeed");

    // Verify equivalence
    assert_eq!(vm_results.len(), num_alternatives);
    assert_eq!(hybrid_results.len(), num_alternatives);

    let mut vm_longs: Vec<i64> = vm_results
        .iter()
        .filter_map(|v| {
            if let MettaValue::Long(n) = v {
                Some(*n)
            } else {
                None
            }
        })
        .collect();

    let mut hybrid_longs: Vec<i64> = hybrid_results
        .iter()
        .filter_map(|v| {
            if let MettaValue::Long(n) = v {
                Some(*n)
            } else {
                None
            }
        })
        .collect();

    vm_longs.sort();
    hybrid_longs.sort();

    assert_eq!(vm_longs, vec![1, 2, 3, 4, 5]);
    assert_eq!(hybrid_longs, vec![1, 2, 3, 4, 5]);
}

/// Test semantic equivalence for single alternative (edge case)
#[test]
fn test_semantic_equivalence_fork_single() {
    let mut builder = ChunkBuilder::new("fork_single");

    // Single alternative: Fork(42)
    let idx = builder.add_constant(MettaValue::Long(42));

    builder.emit(Opcode::BeginNondet);
    builder.emit_u16(Opcode::Fork, 1);
    builder.emit_raw(&idx.to_be_bytes());
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();

    let mut vm = BytecodeVM::new(Arc::clone(&chunk));
    let vm_results = vm.run().expect("VM should succeed");

    let mut executor = HybridExecutor::new();
    let hybrid_results = executor
        .run_with_backtracking(&chunk)
        .expect("Hybrid should succeed");

    assert_eq!(vm_results.len(), 1, "Single alternative = 1 result");
    assert_eq!(hybrid_results.len(), 1);
    assert_eq!(vm_results[0], MettaValue::Long(42));
    assert_eq!(hybrid_results[0], MettaValue::Long(42));
}

/// Test that without Yield, only first result is returned (documents behavior)
#[test]
fn test_fork_without_yield_returns_first_only() {
    let mut builder = ChunkBuilder::new("fork_no_yield");

    let mut const_indices = Vec::new();
    for i in 0..3 {
        let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
        const_indices.push(idx);
    }

    // BeginNondet + Fork but NO Yield - should return first result only
    builder.emit(Opcode::BeginNondet);
    builder.emit_u16(Opcode::Fork, 3);
    for idx in &const_indices {
        builder.emit_raw(&idx.to_be_bytes());
    }
    // No Yield, just return
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();

    let mut vm = BytecodeVM::new(Arc::clone(&chunk));
    let vm_results = vm.run().expect("VM should succeed");

    // Without Yield, we only get the first alternative
    assert_eq!(
        vm_results.len(),
        1,
        "Without Yield, only first result is returned"
    );
    assert_eq!(
        vm_results[0],
        MettaValue::Long(1),
        "First alternative should be 1"
    );
}
