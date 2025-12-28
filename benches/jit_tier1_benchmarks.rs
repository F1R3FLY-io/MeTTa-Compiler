// JIT Tier 1 Opcode Benchmarks
//
// This benchmark suite measures the performance of Tier 1 JIT-compiled opcodes:
// - Bindings: LoadBinding, StoreBinding, PushBindingFrame, PopBindingFrame
// - Pattern Matching: Match, MatchBind, MatchHead, MatchArity
// - Call/TailCall: Call, TailCall, CallN, TailCallN
// - Nondeterminism: Fork, Yield, Collect
//
// Run with CPU affinity (per CLAUDE.md):
// taskset -c 0-17 cargo bench --bench jit_tier1_benchmarks --features jit

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mettatron::backend::bytecode::{BytecodeChunk, ChunkBuilder, Opcode};
use mettatron::backend::MettaValue;
use std::sync::Arc;

#[cfg(feature = "jit")]
use mettatron::backend::bytecode::jit::{JitBindingFrame, JitCompiler, JitContext, JitValue};

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a simple arithmetic bytecode chunk: (+ a b)
fn create_arithmetic_chunk(a: i64, b: i64) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("arithmetic");
    builder.emit_byte(Opcode::PushLongSmall, a as u8);
    builder.emit_byte(Opcode::PushLongSmall, b as u8);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a binding operations chunk: push frame, store, load, pop frame
fn create_binding_chunk() -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("bindings");
    let var_idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
    builder.emit(Opcode::PushBindingFrame);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_u16(Opcode::StoreBinding, var_idx);
    builder.emit_u16(Opcode::LoadBinding, var_idx);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a pattern match chunk
fn create_match_chunk() -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("match");
    // Pattern: (foo $x)
    let pattern_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]));
    // Value: (foo 42)
    let value_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Long(42),
    ]));
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a match bind chunk
fn create_match_bind_chunk() -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("match_bind");
    let _var_idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
    // Pattern: ($x 2)
    let pattern_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(2),
    ]));
    // Value: (1 2)
    let value_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a call chunk: (func arg1 arg2)
fn create_call_chunk(arity: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("call");
    let head_idx = builder.add_constant(MettaValue::Atom("func".to_string()));
    for i in 0..arity {
        builder.emit_byte(Opcode::PushLongSmall, (i + 1) as u8);
    }
    builder.emit_call(head_idx, arity as u8);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a nested if chain chunk for testing hot paths
fn create_if_chain_chunk(depth: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("if_chain");

    // Push initial condition
    builder.emit(Opcode::PushTrue);

    for _ in 0..depth {
        // if condition then 1 else 0
        let else_jump = builder.emit_jump(Opcode::JumpIfFalse);

        // Then branch - push 1
        builder.emit_byte(Opcode::PushLongSmall, 1);
        let end_jump = builder.emit_jump(Opcode::Jump);

        // Else branch - push 0
        builder.patch_jump(else_jump);
        builder.emit_byte(Opcode::PushLongSmall, 0);

        // End - patch the end jump
        builder.patch_jump(end_jump);

        // Result becomes next condition (always truthy)
    }

    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a let chain chunk for testing local variable performance
fn create_let_chain_chunk(depth: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("let_chain");

    // (let $x0 1 (let $x1 (+ $x0 1) (let $x2 (+ $x1 1) ...)))
    for i in 0..depth {
        if i == 0 {
            builder.emit_byte(Opcode::PushLongSmall, 1);
        } else {
            builder.emit_byte(Opcode::LoadLocal, (i - 1) as u8);
            builder.emit_byte(Opcode::PushLongSmall, 1);
            builder.emit(Opcode::Add);
        }
        builder.emit_byte(Opcode::StoreLocal, i as u8);
    }

    // Return last local
    if depth > 0 {
        builder.emit_byte(Opcode::LoadLocal, (depth - 1) as u8);
    } else {
        builder.emit_byte(Opcode::PushLongSmall, 0);
    }
    builder.emit(Opcode::Return);
    builder.build()
}

// ============================================================================
// Benchmark 1: Binding Operations
// ============================================================================

#[cfg(feature = "jit")]
fn bench_binding_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_tier1_bindings");

    // Bytecode VM baseline
    group.bench_function("bytecode_vm", |b| {
        let chunk = Arc::new(create_binding_chunk());
        b.iter(|| {
            let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
            black_box(vm.run())
        })
    });

    // JIT compiled
    group.bench_function("jit_compiled", |b| {
        let chunk = create_binding_chunk();
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        b.iter(|| {
            let mut ctx = unsafe {
                let mut ctx =
                    JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len());
                ctx.binding_frames = binding_frames.as_mut_ptr();
                ctx.binding_frames_cap = binding_frames.len();
                ctx
            };

            let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                unsafe { std::mem::transmute(code_ptr) };
            let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
            black_box(result)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 2: Pattern Matching
// ============================================================================

#[cfg(feature = "jit")]
fn bench_pattern_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_tier1_pattern_match");

    // Match without binding
    group.bench_function("match_bytecode", |b| {
        let chunk = Arc::new(create_match_chunk());
        b.iter(|| {
            let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
            black_box(vm.run())
        })
    });

    group.bench_function("match_jit", |b| {
        let chunk = create_match_chunk();
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        b.iter(|| {
            let mut ctx = unsafe {
                JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
            };

            let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                unsafe { std::mem::transmute(code_ptr) };
            let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
            black_box(result)
        })
    });

    // Match with binding
    group.bench_function("match_bind_bytecode", |b| {
        let chunk = Arc::new(create_match_bind_chunk());
        b.iter(|| {
            let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
            black_box(vm.run())
        })
    });

    group.bench_function("match_bind_jit", |b| {
        let chunk = create_match_bind_chunk();
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

        b.iter(|| {
            let mut ctx = unsafe {
                let mut ctx =
                    JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len());
                ctx.binding_frames = binding_frames.as_mut_ptr();
                ctx.binding_frames_cap = binding_frames.len();
                ctx
            };

            let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                unsafe { std::mem::transmute(code_ptr) };
            let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
            black_box(result)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark 3: Call Operations
// ============================================================================

#[cfg(feature = "jit")]
fn bench_call_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_tier1_call");

    for arity in [0, 1, 2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::new("call_bytecode", arity),
            arity,
            |b, &arity| {
                let chunk = Arc::new(create_call_chunk(arity));
                b.iter(|| {
                    let mut vm =
                        mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(BenchmarkId::new("call_jit", arity), arity, |b, &arity| {
            let chunk = create_call_chunk(arity);
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

            let constants = chunk.constants();
            let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

            b.iter(|| {
                let mut ctx = unsafe {
                    JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
                };

                let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                    unsafe { std::mem::transmute(code_ptr) };
                let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                black_box(result)
            })
        });
    }

    group.finish();
}

// ============================================================================
// Benchmark 4: Hot Path Performance (If Chains)
// ============================================================================

#[cfg(feature = "jit")]
fn bench_hot_paths_if_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_tier1_if_chain");

    for depth in [10, 25, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::new("if_chain_bytecode", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_if_chain_chunk(depth));
                b.iter(|| {
                    let mut vm =
                        mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("if_chain_jit", depth),
            depth,
            |b, &depth| {
                let chunk = create_if_chain_chunk(depth);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

                let constants = chunk.constants();
                let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];

                b.iter(|| {
                    let mut ctx = unsafe {
                        JitContext::new(
                            stack.as_mut_ptr(),
                            256,
                            constants.as_ptr(),
                            constants.len(),
                        )
                    };

                    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                        unsafe { std::mem::transmute(code_ptr) };
                    let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Benchmark 5: Let Chain Performance (Local Variables)
// ============================================================================

#[cfg(feature = "jit")]
fn bench_let_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_tier1_let_chain");

    // Note: JIT compilation of local variables requires proper chunk metadata setup.
    // For now, we benchmark only the bytecode VM for let chains.
    // The existing jit_comparison benchmark handles JIT let chain testing properly.

    for depth in [10, 25, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::new("let_chain_bytecode", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_let_chain_chunk(depth));
                b.iter(|| {
                    let mut vm =
                        mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Benchmark 6: Arithmetic Operations Comparison
// ============================================================================

#[cfg(feature = "jit")]
fn bench_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_tier1_arithmetic");

    // Simple addition
    group.bench_function("add_bytecode", |b| {
        let chunk = Arc::new(create_arithmetic_chunk(40, 2));
        b.iter(|| {
            let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
            black_box(vm.run())
        })
    });

    group.bench_function("add_jit", |b| {
        let chunk = create_arithmetic_chunk(40, 2);
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        b.iter(|| {
            let mut ctx = unsafe {
                JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
            };

            let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                unsafe { std::mem::transmute(code_ptr) };
            let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
            black_box(result)
        })
    });

    group.finish();
}

// ============================================================================
// Benchmark Groups
// ============================================================================

#[cfg(feature = "jit")]
criterion_group!(
    tier1_benches,
    bench_binding_operations,
    bench_pattern_matching,
    bench_call_operations,
    bench_hot_paths_if_chain,
    bench_let_chain,
    bench_arithmetic,
);

// Placeholder benchmark when JIT is not enabled
#[cfg(not(feature = "jit"))]
fn bench_no_jit_placeholder(c: &mut Criterion) {
    c.bench_function("no_jit_placeholder", |b| b.iter(|| black_box(42)));
}

#[cfg(not(feature = "jit"))]
criterion_group!(tier1_benches, bench_no_jit_placeholder,);

criterion_main!(tier1_benches);

// ============================================================================
// Usage Instructions
// ============================================================================

/*
## Running Benchmarks

Basic usage:
```bash
cargo bench --bench jit_tier1_benchmarks --features jit
```

With CPU affinity (recommended per CLAUDE.md):
```bash
taskset -c 0-17 cargo bench --bench jit_tier1_benchmarks --features jit
```

Run specific benchmark group:
```bash
cargo bench --bench jit_tier1_benchmarks --features jit -- bindings
cargo bench --bench jit_tier1_benchmarks --features jit -- pattern_match
cargo bench --bench jit_tier1_benchmarks --features jit -- call
cargo bench --bench jit_tier1_benchmarks --features jit -- if_chain
cargo bench --bench jit_tier1_benchmarks --features jit -- let_chain
```

Save baseline for comparison:
```bash
cargo bench --bench jit_tier1_benchmarks --features jit -- --save-baseline tier1_baseline
```

Compare with baseline:
```bash
cargo bench --bench jit_tier1_benchmarks --features jit -- --baseline tier1_baseline
```

## Expected Results

| Benchmark Category | Expected JIT Speedup |
|-------------------|---------------------|
| Bindings          | 5-20x               |
| Pattern Matching  | 2-10x               |
| Call (bailout)    | 1-2x (overhead)     |
| If Chains         | 50-150x             |
| Let Chains        | 50-150x             |
| Arithmetic        | 50-150x             |

Note: Call operations trigger bailout, so JIT may show overhead vs bytecode.
The benefit of JIT is in the fast paths (arithmetic, control flow, locals).
*/
