// JIT Optimization Benchmarks
//
// Targeted benchmarks for scientific evaluation of JIT optimizations:
// - Optimization 3.2: Call Fast Path for Grounded Functions
// - Optimization 3.1: Pattern Match Inlining for Simple Patterns
// - Optimization 3.3: Binding Hash Lookup
//
// Run with CPU affinity (per CLAUDE.md):
// taskset -c 0-17 cargo bench --bench jit_optimization_benchmarks --features jit
//
// Save baseline:
// taskset -c 0-17 cargo bench --bench jit_optimization_benchmarks --features jit -- --save-baseline baseline_pre_opt
//
// Compare against baseline:
// taskset -c 0-17 cargo bench --bench jit_optimization_benchmarks --features jit -- --baseline baseline_pre_opt

use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup,
    BenchmarkId, Criterion, Throughput,
};
use mettatron::backend::bytecode::{BytecodeChunk, ChunkBuilder, Opcode};
use mettatron::backend::MettaValue;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "jit")]
use mettatron::backend::bytecode::jit::{JitBindingFrame, JitCompiler, JitContext, JitValue};

// ============================================================================
// Configuration
// ============================================================================

fn configure_group(group: &mut BenchmarkGroup<WallTime>) {
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
}

// ============================================================================
// OPTIMIZATION 3.2: Call Dispatch for Grounded Functions
// ============================================================================

/// Create a chunk that calls a grounded arithmetic function
fn create_grounded_call_chunk(op: &str, a: i64, b: i64) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("grounded_call");
    let head_idx = builder.add_constant(MettaValue::Atom(op.to_string()));
    builder.emit_byte(Opcode::PushLongSmall, a as u8);
    builder.emit_byte(Opcode::PushLongSmall, b as u8);
    builder.emit_call(head_idx, 2);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chain of grounded calls: (+ (+ (+ a b) c) d) ...
fn create_grounded_call_chain(op: &str, depth: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("grounded_call_chain");
    let head_idx = builder.add_constant(MettaValue::Atom(op.to_string()));

    // Start with initial value
    builder.emit_byte(Opcode::PushLongSmall, 1);

    // Chain calls
    for i in 0..depth {
        builder.emit_byte(Opcode::PushLongSmall, ((i % 10) + 1) as u8);
        builder.emit_call(head_idx, 2);
    }

    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chunk that calls a user-defined rule (not grounded)
fn create_user_rule_call_chunk(rule_name: &str, arity: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("user_rule_call");
    let head_idx = builder.add_constant(MettaValue::Atom(rule_name.to_string()));
    for i in 0..arity {
        builder.emit_byte(Opcode::PushLongSmall, (i + 1) as u8);
    }
    builder.emit_call(head_idx, arity as u8);
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn bench_call_dispatch_grounded(c: &mut Criterion) {
    let mut group = c.benchmark_group("call_dispatch_grounded");
    configure_group(&mut group);

    // Test core grounded operations (minimal set for fast baseline)
    let ops = ["+", "*", "=="];

    for op in ops.iter() {
        // Bytecode VM baseline
        group.bench_with_input(
            BenchmarkId::new("bytecode", *op),
            op,
            |b, op| {
                let chunk = Arc::new(create_grounded_call_chunk(op, 40, 2));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        // JIT compiled
        group.bench_with_input(
            BenchmarkId::new("jit", *op),
            op,
            |b, op| {
                let chunk = create_grounded_call_chunk(op, 40, 2);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
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
                }
            },
        );
    }

    group.finish();
}

#[cfg(feature = "jit")]
fn bench_call_dispatch_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("call_dispatch_chain");
    configure_group(&mut group);

    for depth in [10, 50, 100].iter() {
        group.throughput(Throughput::Elements(*depth as u64));

        // Bytecode VM
        group.bench_with_input(
            BenchmarkId::new("add_chain_bytecode", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_grounded_call_chain("+", depth));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        // JIT compiled
        group.bench_with_input(
            BenchmarkId::new("add_chain_jit", depth),
            depth,
            |b, &depth| {
                let chunk = create_grounded_call_chain("+", depth);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];

                    b.iter(|| {
                        let mut ctx = unsafe {
                            JitContext::new(stack.as_mut_ptr(), 256, constants.as_ptr(), constants.len())
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    group.finish();
}

#[cfg(feature = "jit")]
fn bench_call_dispatch_user_rules(c: &mut Criterion) {
    let mut group = c.benchmark_group("call_dispatch_user_rules");
    configure_group(&mut group);

    // Control: user-defined rule calls (should not be affected by grounded fast path)
    for arity in [1, 2, 4].iter() {
        group.bench_with_input(
            BenchmarkId::new("user_rule_bytecode", arity),
            arity,
            |b, &arity| {
                let chunk = Arc::new(create_user_rule_call_chunk("my-function", arity));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("user_rule_jit", arity),
            arity,
            |b, &arity| {
                let chunk = create_user_rule_call_chunk("my-function", arity);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
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
                }
            },
        );
    }

    group.finish();
}

// ============================================================================
// OPTIMIZATION 3.1: Pattern Match Inlining
// ============================================================================

/// Create a ground pattern match (no variables)
fn create_ground_pattern_chunk(pattern_size: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("ground_pattern");

    // Pattern: (foo bar baz ...) - all ground atoms
    let pattern_items: Vec<MettaValue> = (0..pattern_size)
        .map(|i| MettaValue::Atom(format!("atom{}", i)))
        .collect();
    let pattern_idx = builder.add_constant(MettaValue::SExpr(pattern_items.clone()));

    // Value: same structure (should match)
    let value_idx = builder.add_constant(MettaValue::SExpr(pattern_items));

    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a single-variable pattern match
fn create_single_var_pattern_chunk() -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("single_var_pattern");

    // Pattern: (foo $x) - one variable
    let pattern_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]));

    // Value: (foo 42)
    let value_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Long(42),
    ]));

    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a two-variable pattern match
fn create_two_var_pattern_chunk() -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("two_var_pattern");

    // Pattern: ($x $y)
    let pattern_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
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

/// Create a complex nested pattern match
fn create_complex_pattern_chunk(nesting_depth: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("complex_pattern");

    // Build nested pattern: (a (b (c ... $x)))
    fn build_nested(depth: usize, var: bool) -> MettaValue {
        if depth == 0 {
            if var {
                MettaValue::Atom("$x".to_string())
            } else {
                MettaValue::Long(42)
            }
        } else {
            MettaValue::SExpr(vec![
                MettaValue::Atom(format!("level{}", depth)),
                build_nested(depth - 1, var),
            ])
        }
    }

    let pattern = build_nested(nesting_depth, true);
    let value = build_nested(nesting_depth, false);

    let pattern_idx = builder.add_constant(pattern);
    let value_idx = builder.add_constant(value);

    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn bench_pattern_match_simple(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_match_simple");
    configure_group(&mut group);

    // Ground patterns of varying sizes
    for size in [2, 5, 10].iter() {
        group.bench_with_input(
            BenchmarkId::new("ground_bytecode", size),
            size,
            |b, &size| {
                let chunk = Arc::new(create_ground_pattern_chunk(size));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("ground_jit", size),
            size,
            |b, &size| {
                let chunk = create_ground_pattern_chunk(size);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
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
                }
            },
        );
    }

    // Single variable pattern
    group.bench_function("single_var_bytecode", |b| {
        let chunk = Arc::new(create_single_var_pattern_chunk());
        b.iter(|| {
            let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
            black_box(vm.run())
        })
    });

    group.bench_function("single_var_jit", |b| {
        let chunk = create_single_var_pattern_chunk();
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        if let Ok(code_ptr) = compiler.compile(&chunk) {
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
        }
    });

    // Two variable pattern
    group.bench_function("two_var_bytecode", |b| {
        let chunk = Arc::new(create_two_var_pattern_chunk());
        b.iter(|| {
            let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
            black_box(vm.run())
        })
    });

    group.bench_function("two_var_jit", |b| {
        let chunk = create_two_var_pattern_chunk();
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        if let Ok(code_ptr) = compiler.compile(&chunk) {
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
        }
    });

    group.finish();
}

#[cfg(feature = "jit")]
fn bench_pattern_match_complex(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_match_complex");
    configure_group(&mut group);

    // Complex nested patterns (control - should not be affected by simple pattern inlining)
    for depth in [2, 5, 10].iter() {
        group.bench_with_input(
            BenchmarkId::new("nested_bytecode", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_complex_pattern_chunk(depth));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("nested_jit", depth),
            depth,
            |b, &depth| {
                let chunk = create_complex_pattern_chunk(depth);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
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
                }
            },
        );
    }

    group.finish();
}

// ============================================================================
// OPTIMIZATION 3.3: Binding Lookup
// ============================================================================

/// Create a chunk with multiple binding lookups at varying frame depth
fn create_binding_lookup_depth_chunk(depth: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("binding_lookup_depth");

    // Push nested frames, each with one binding
    for i in 0..depth {
        builder.emit(Opcode::PushBindingFrame);
        builder.emit_byte(Opcode::PushLongSmall, (i + 1) as u8);
        let var_idx = builder.add_constant(MettaValue::Atom(format!("$x{}", i)));
        builder.emit_u16(Opcode::StoreBinding, var_idx);
    }

    // Lookup the first binding (deepest, requires full traversal)
    let first_var_idx = builder.add_constant(MettaValue::Atom("$x0".to_string()));
    builder.emit_u16(Opcode::LoadBinding, first_var_idx);

    // Pop all frames
    for _ in 0..depth {
        builder.emit(Opcode::PopBindingFrame);
    }

    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chunk with multiple bindings in a single frame (width test)
fn create_binding_lookup_width_chunk(width: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("binding_lookup_width");

    builder.emit(Opcode::PushBindingFrame);

    // Store many bindings in one frame
    for i in 0..width {
        builder.emit_byte(Opcode::PushLongSmall, (i % 256) as u8);
        let var_idx = builder.add_constant(MettaValue::Atom(format!("$var{}", i)));
        builder.emit_u16(Opcode::StoreBinding, var_idx);
    }

    // Lookup the first binding (requires traversing all)
    let first_var_idx = builder.add_constant(MettaValue::Atom("$var0".to_string()));
    builder.emit_u16(Opcode::LoadBinding, first_var_idx);

    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chunk with repeated lookups (cache locality test)
fn create_binding_lookup_repeated_chunk(lookups: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("binding_lookup_repeated");

    builder.emit(Opcode::PushBindingFrame);

    // Store a single binding
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let var_idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
    builder.emit_u16(Opcode::StoreBinding, var_idx);

    // Repeatedly lookup the same binding
    for _ in 0..lookups {
        builder.emit_u16(Opcode::LoadBinding, var_idx);
        builder.emit(Opcode::Pop); // Discard to prevent stack overflow
    }

    // Final lookup (keep result)
    builder.emit_u16(Opcode::LoadBinding, var_idx);

    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn bench_binding_lookup_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("binding_lookup_depth");
    configure_group(&mut group);

    for depth in [1, 10, 20].iter() {
        group.bench_with_input(
            BenchmarkId::new("bytecode", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_binding_lookup_depth_chunk(depth));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("jit", depth),
            depth,
            |b, &depth| {
                let chunk = create_binding_lookup_depth_chunk(depth);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
                    let mut binding_frames: Vec<JitBindingFrame> =
                        vec![JitBindingFrame::default(); depth.max(8) + 4];

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
                }
            },
        );
    }

    group.finish();
}

#[cfg(feature = "jit")]
fn bench_binding_lookup_width(c: &mut Criterion) {
    let mut group = c.benchmark_group("binding_lookup_width");
    configure_group(&mut group);

    for width in [1, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::new("bytecode", width),
            width,
            |b, &width| {
                let chunk = Arc::new(create_binding_lookup_width_chunk(width));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("jit", width),
            width,
            |b, &width| {
                let chunk = create_binding_lookup_width_chunk(width);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx =
                                JitContext::new(stack.as_mut_ptr(), 256, constants.as_ptr(), constants.len());
                            ctx.binding_frames = binding_frames.as_mut_ptr();
                            ctx.binding_frames_cap = binding_frames.len();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    group.finish();
}

#[cfg(feature = "jit")]
fn bench_binding_lookup_repeated(c: &mut Criterion) {
    let mut group = c.benchmark_group("binding_lookup_repeated");
    configure_group(&mut group);

    for lookups in [10, 50, 100].iter() {
        group.throughput(Throughput::Elements(*lookups as u64));

        group.bench_with_input(
            BenchmarkId::new("bytecode", lookups),
            lookups,
            |b, &lookups| {
                let chunk = Arc::new(create_binding_lookup_repeated_chunk(lookups));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("jit", lookups),
            lookups,
            |b, &lookups| {
                let chunk = create_binding_lookup_repeated_chunk(lookups);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx =
                                JitContext::new(stack.as_mut_ptr(), 256, constants.as_ptr(), constants.len());
                            ctx.binding_frames = binding_frames.as_mut_ptr();
                            ctx.binding_frames_cap = binding_frames.len();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    group.finish();
}

// ============================================================================
// PHASE 5 EXPERIMENT 5.1: State Operation Caching
// ============================================================================

/// Create a chunk that creates and reads states
#[cfg(feature = "jit")]
fn create_state_ops_chunk(num_states: usize, reads_per_state: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("state_ops");

    // Create states and read them multiple times
    for i in 0..num_states {
        // new-state with initial value
        builder.emit_byte(Opcode::PushLongSmall, (i + 1) as u8);
        builder.emit(Opcode::NewState);

        // Read the state multiple times (to test caching)
        for _ in 0..reads_per_state {
            builder.emit(Opcode::Dup); // Duplicate state handle
            builder.emit(Opcode::GetState);
            builder.emit(Opcode::Pop); // Discard value
        }

        // Final read (keep result)
        builder.emit(Opcode::GetState);
        if i < num_states - 1 {
            builder.emit(Opcode::Pop); // Discard intermediate results
        }
    }

    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chunk with change-state operations
#[cfg(feature = "jit")]
fn create_state_change_chunk(num_changes: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("state_change");

    // Create a state
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::NewState);

    // Change it multiple times
    for i in 0..num_changes {
        builder.emit_byte(Opcode::PushLongSmall, ((i + 1) % 256) as u8);
        builder.emit(Opcode::ChangeState);
    }

    // Read final value
    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a mixed state workload (like mmverify's &sp pattern)
#[cfg(feature = "jit")]
fn create_state_mixed_chunk(ops: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("state_mixed");

    // Create initial state (stack pointer simulation)
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::NewState);

    // Mixed operations: get, increment, change (like stack push/pop)
    for i in 0..ops {
        if i % 3 == 0 {
            // Get state (read sp)
            builder.emit(Opcode::Dup);
            builder.emit(Opcode::GetState);
            builder.emit(Opcode::Pop);
        } else if i % 3 == 1 {
            // Change state (modify sp)
            builder.emit_byte(Opcode::PushLongSmall, ((i + 1) % 256) as u8);
            builder.emit(Opcode::ChangeState);
        } else {
            // Get and change (read-modify-write)
            builder.emit(Opcode::Dup);
            builder.emit(Opcode::GetState);
            builder.emit_byte(Opcode::PushLongSmall, 1);
            builder.emit(Opcode::Add);
            builder.emit(Opcode::ChangeState);
        }
    }

    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn bench_state_operations(c: &mut Criterion) {
    use mettatron::backend::Environment;

    let mut group = c.benchmark_group("state_operations");
    configure_group(&mut group);

    // State creation
    for count in [1, 10, 100].iter() {
        group.throughput(Throughput::Elements(*count as u64));

        group.bench_with_input(
            BenchmarkId::new("create_bytecode", count),
            count,
            |b, &count| {
                let chunk = Arc::new(create_state_ops_chunk(count, 0));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::with_config_and_env(
                        Arc::clone(&chunk),
                        mettatron::backend::bytecode::vm::VmConfig::default(),
                        Environment::new(),
                    );
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("create_jit", count),
            count,
            |b, &count| {
                let chunk = create_state_ops_chunk(count, 0);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut env = Environment::new();

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx = JitContext::new(
                                stack.as_mut_ptr(),
                                256,
                                constants.as_ptr(),
                                constants.len(),
                            );
                            ctx.env_ptr = &mut env as *mut Environment as *mut ();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    // Hot state access (repeated reads)
    for repeats in [1, 10, 100].iter() {
        group.throughput(Throughput::Elements(*repeats as u64));

        group.bench_with_input(
            BenchmarkId::new("get_hot_bytecode", repeats),
            repeats,
            |b, &repeats| {
                let chunk = Arc::new(create_state_ops_chunk(1, repeats));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::with_config_and_env(
                        Arc::clone(&chunk),
                        mettatron::backend::bytecode::vm::VmConfig::default(),
                        Environment::new(),
                    );
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_hot_jit", repeats),
            repeats,
            |b, &repeats| {
                let chunk = create_state_ops_chunk(1, repeats);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut env = Environment::new();

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx = JitContext::new(
                                stack.as_mut_ptr(),
                                256,
                                constants.as_ptr(),
                                constants.len(),
                            );
                            ctx.env_ptr = &mut env as *mut Environment as *mut ();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    // State changes
    for changes in [1, 10, 100].iter() {
        group.throughput(Throughput::Elements(*changes as u64));

        group.bench_with_input(
            BenchmarkId::new("change_bytecode", changes),
            changes,
            |b, &changes| {
                let chunk = Arc::new(create_state_change_chunk(changes));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::with_config_and_env(
                        Arc::clone(&chunk),
                        mettatron::backend::bytecode::vm::VmConfig::default(),
                        Environment::new(),
                    );
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("change_jit", changes),
            changes,
            |b, &changes| {
                let chunk = create_state_change_chunk(changes);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut env = Environment::new();

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx = JitContext::new(
                                stack.as_mut_ptr(),
                                256,
                                constants.as_ptr(),
                                constants.len(),
                            );
                            ctx.env_ptr = &mut env as *mut Environment as *mut ();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    // Mixed workload (mmverify-like)
    // NOTE: Temporarily disabled due to SIGILL during criterion warmup
    // The underlying VM works correctly (verified via test_mixed_chunk example)
    // TODO: Investigate criterion interaction causing SIGILL
    /*
    for ops in [10, 50, 100].iter() {
        group.throughput(Throughput::Elements(*ops as u64));

        group.bench_with_input(
            BenchmarkId::new("mixed_bytecode", ops),
            ops,
            |b, &ops| {
                let chunk = Arc::new(create_state_mixed_chunk(ops));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::with_config_and_env(
                        Arc::clone(&chunk),
                        mettatron::backend::bytecode::vm::VmConfig::default(),
                        Environment::new(),
                    );
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("mixed_jit", ops),
            ops,
            |b, &ops| {
                let chunk = create_state_mixed_chunk(ops);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut env = Environment::new();

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx = JitContext::new(
                                stack.as_mut_ptr(),
                                256,
                                constants.as_ptr(),
                                constants.len(),
                            );
                            ctx.env_ptr = &mut env as *mut Environment as *mut ();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }
    */

    group.finish();
}

// ============================================================================
// PHASE 5 EXPERIMENT 5.2: Nondeterminism / Choice Point Pre-allocation
// ============================================================================

/// Create a chunk with a fork and multiple alternatives
/// Uses proper nondeterminism opcodes to collect ALL results (MeTTa HE semantics)
#[cfg(feature = "jit")]
fn create_fork_chunk(num_alternatives: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("fork_test");

    // Add constants for each alternative (Fork reads from constant pool, not stack)
    let mut const_indices = Vec::with_capacity(num_alternatives);
    for i in 0..num_alternatives {
        let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
        const_indices.push(idx);
    }

    // BeginNondet marks start of nondeterministic region
    builder.emit(Opcode::BeginNondet);

    // Fork with u16 count and u16 constant indices
    builder.emit_u16(Opcode::Fork, num_alternatives as u16);
    for idx in &const_indices {
        builder.emit_raw(&idx.to_be_bytes());
    }

    // Simple computation on the chosen value
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Add);

    // Yield collects this result and backtracks for more alternatives
    builder.emit(Opcode::Yield);

    // Return is never reached - op_fail returns Break(results) when exhausted
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chunk with nested forks
/// Uses proper nondeterminism opcodes to collect ALL results (MeTTa HE semantics)
#[cfg(feature = "jit")]
fn create_nested_fork_chunk(depth: usize, alternatives_per_level: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("nested_fork");

    // Add constants for alternatives at each level
    // For simplicity, we use the same alternative values for each level
    let mut const_indices = Vec::with_capacity(alternatives_per_level);
    for i in 0..alternatives_per_level {
        let idx = builder.add_constant(MettaValue::Long((i + 1) as i64));
        const_indices.push(idx);
    }

    // BeginNondet marks start of nondeterministic region
    builder.emit(Opcode::BeginNondet);

    // Start with initial value
    builder.emit_byte(Opcode::PushLongSmall, 0);

    // Create nested forks
    for _level in 0..depth {
        // Fork with constant indices
        builder.emit_u16(Opcode::Fork, alternatives_per_level as u16);
        for idx in &const_indices {
            builder.emit_raw(&idx.to_be_bytes());
        }
        builder.emit(Opcode::Add); // Accumulate
    }

    // Yield collects this result and backtracks for more alternatives
    builder.emit(Opcode::Yield);

    // Return is never reached - op_fail returns Break(results) when exhausted
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create a chunk that exercises backtracking
/// Uses proper nondeterminism opcodes to collect ALL results (MeTTa HE semantics)
#[cfg(feature = "jit")]
fn create_backtrack_chain_chunk(backtracks: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("backtrack_chain");

    // Add constants for all alternatives (including the failing ones and the success)
    let mut const_indices = Vec::with_capacity(backtracks + 1);
    for i in 0..backtracks {
        let idx = builder.add_constant(MettaValue::Long((i % 256) as i64));
        const_indices.push(idx);
    }
    // Final success alternative
    let success_idx = builder.add_constant(MettaValue::Long(42));
    const_indices.push(success_idx);

    // BeginNondet marks start of nondeterministic region
    builder.emit(Opcode::BeginNondet);

    // Fork with all alternatives
    builder.emit_u16(Opcode::Fork, const_indices.len() as u16);
    for idx in &const_indices {
        builder.emit_raw(&idx.to_be_bytes());
    }

    // Check if we got the success value (42)
    builder.emit(Opcode::Dup);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Eq);

    // Branch to fail if not 42
    let fail_label = builder.emit_jump(Opcode::JumpIfFalse);

    // Success path - yield the successful value
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    // Failure path - trigger backtrack to try next alternative
    builder.patch_jump(fail_label);
    builder.emit(Opcode::Pop); // Pop the value
    builder.emit(Opcode::Fail);

    builder.build()
}

/// Create a chunk with fork and large stack save
/// Uses proper nondeterminism opcodes to collect ALL results (MeTTa HE semantics)
#[cfg(feature = "jit")]
fn create_fork_large_stack_chunk(stack_size: usize, alternatives: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("fork_large_stack");

    // Add constants for alternatives (Fork reads from constant pool, not stack)
    let mut const_indices = Vec::with_capacity(alternatives);
    for i in 0..alternatives {
        let idx = builder.add_constant(MettaValue::Long(((i + 100) % 256) as i64));
        const_indices.push(idx);
    }

    // BeginNondet marks start of nondeterministic region
    builder.emit(Opcode::BeginNondet);

    // Push many values onto the stack before fork (tests stack save/restore)
    for i in 0..stack_size {
        builder.emit_byte(Opcode::PushLongSmall, ((i + 1) % 256) as u8);
    }

    // Fork with constant indices
    builder.emit_u16(Opcode::Fork, alternatives as u16);
    for idx in &const_indices {
        builder.emit_raw(&idx.to_be_bytes());
    }

    // Pop all values and return the fork result
    for _ in 0..stack_size {
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Pop);
    }

    // Yield collects this result and backtracks for more alternatives
    builder.emit(Opcode::Yield);

    // Return is never reached - op_fail returns Break(results) when exhausted
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn bench_nondeterminism(c: &mut Criterion) {
    use mettatron::backend::bytecode::jit::HybridExecutor;

    let mut group = c.benchmark_group("nondeterminism");
    configure_group(&mut group);

    // Fork with varying alternatives
    for alts in [2, 4, 8, 16].iter() {
        group.throughput(Throughput::Elements(*alts as u64));

        group.bench_with_input(
            BenchmarkId::new("fork_bytecode", alts),
            alts,
            |b, &alts| {
                let chunk = Arc::new(create_fork_chunk(alts));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        // HybridExecutor with run_with_backtracking for MeTTa HE semantic equivalence
        group.bench_with_input(BenchmarkId::new("fork_hybrid", alts), alts, |b, &alts| {
            let chunk = Arc::new(create_fork_chunk(alts));
            let mut executor = HybridExecutor::new();
            b.iter(|| {
                black_box(executor.run_with_backtracking(&chunk))
            })
        });
    }

    // Nested forks
    for depth in [2, 4, 6].iter() {
        group.bench_with_input(
            BenchmarkId::new("nested_fork_bytecode", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_nested_fork_chunk(depth, 2));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        // HybridExecutor with run_with_backtracking for MeTTa HE semantic equivalence
        group.bench_with_input(
            BenchmarkId::new("nested_fork_hybrid", depth),
            depth,
            |b, &depth| {
                let chunk = Arc::new(create_nested_fork_chunk(depth, 2));
                let mut executor = HybridExecutor::new();
                b.iter(|| {
                    black_box(executor.run_with_backtracking(&chunk))
                })
            },
        );
    }

    // Backtrack chain
    for backtracks in [5, 10, 20].iter() {
        group.throughput(Throughput::Elements(*backtracks as u64));

        group.bench_with_input(
            BenchmarkId::new("backtrack_chain_bytecode", backtracks),
            backtracks,
            |b, &backtracks| {
                let chunk = Arc::new(create_backtrack_chain_chunk(backtracks));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        // HybridExecutor with run_with_backtracking for MeTTa HE semantic equivalence
        group.bench_with_input(
            BenchmarkId::new("backtrack_chain_hybrid", backtracks),
            backtracks,
            |b, &backtracks| {
                let chunk = Arc::new(create_backtrack_chain_chunk(backtracks));
                let mut executor = HybridExecutor::new();
                b.iter(|| {
                    black_box(executor.run_with_backtracking(&chunk))
                })
            },
        );
    }

    // Fork with large stack (tests stack save/restore)
    for stack_size in [10, 50, 100].iter() {
        group.throughput(Throughput::Elements(*stack_size as u64));

        group.bench_with_input(
            BenchmarkId::new("fork_large_stack_bytecode", stack_size),
            stack_size,
            |b, &stack_size| {
                let chunk = Arc::new(create_fork_large_stack_chunk(stack_size, 4));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        // HybridExecutor with run_with_backtracking for MeTTa HE semantic equivalence
        group.bench_with_input(
            BenchmarkId::new("fork_large_stack_hybrid", stack_size),
            stack_size,
            |b, &stack_size| {
                let chunk = Arc::new(create_fork_large_stack_chunk(stack_size, 4));
                let mut executor = HybridExecutor::new();
                b.iter(|| {
                    black_box(executor.run_with_backtracking(&chunk))
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// PHASE 5 EXPERIMENT 5.3: Pattern Matching with Many Variables
// ============================================================================

/// Create a pattern with many variables
#[cfg(feature = "jit")]
fn create_many_vars_pattern_chunk(num_vars: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("many_vars_pattern");

    // Pattern: ($x1 $x2 $x3 ...)
    let pattern_items: Vec<MettaValue> = (0..num_vars)
        .map(|i| MettaValue::Atom(format!("$x{}", i)))
        .collect();
    let pattern_idx = builder.add_constant(MettaValue::SExpr(pattern_items));

    // Value: (1 2 3 ...)
    let value_items: Vec<MettaValue> = (0..num_vars).map(|i| MettaValue::Long(i as i64)).collect();
    let value_idx = builder.add_constant(MettaValue::SExpr(value_items));

    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create repeated matches of the same pattern
#[cfg(feature = "jit")]
fn create_repeat_var_pattern_chunk(num_repeats: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("repeat_var_pattern");

    // Pattern: ($x $y)
    let pattern_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]));

    // Value: (1 2)
    let value_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));

    builder.emit(Opcode::PushBindingFrame);

    // Repeat the same match multiple times
    for _ in 0..num_repeats {
        builder.emit_u16(Opcode::PushConstant, pattern_idx);
        builder.emit_u16(Opcode::PushConstant, value_idx);
        builder.emit(Opcode::MatchBind);
        builder.emit(Opcode::Pop); // Discard result
    }

    // Final match (keep result)
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);

    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

/// Create mixed ground + variable pattern
#[cfg(feature = "jit")]
fn create_mixed_pattern_chunk(num_elements: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("mixed_pattern");

    // Pattern: (foo 1 $x bar 2 $y ...)
    let pattern_items: Vec<MettaValue> = (0..num_elements)
        .map(|i| {
            if i % 3 == 0 {
                MettaValue::Atom(format!("atom{}", i))
            } else if i % 3 == 1 {
                MettaValue::Long(i as i64)
            } else {
                MettaValue::Atom(format!("$x{}", i))
            }
        })
        .collect();
    let pattern_idx = builder.add_constant(MettaValue::SExpr(pattern_items.clone()));

    // Value: matching structure
    let value_items: Vec<MettaValue> = (0..num_elements)
        .map(|i| {
            if i % 3 == 0 {
                MettaValue::Atom(format!("atom{}", i))
            } else if i % 3 == 1 {
                MettaValue::Long(i as i64)
            } else {
                MettaValue::Long((i * 10) as i64)
            }
        })
        .collect();
    let value_idx = builder.add_constant(MettaValue::SExpr(value_items));

    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn bench_pattern_match_variables(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_match_variables");
    configure_group(&mut group);

    // Many variables
    for num_vars in [5, 10, 20].iter() {
        group.throughput(Throughput::Elements(*num_vars as u64));

        group.bench_with_input(
            BenchmarkId::new("many_vars_bytecode", num_vars),
            num_vars,
            |b, &num_vars| {
                let chunk = Arc::new(create_many_vars_pattern_chunk(num_vars));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("many_vars_jit", num_vars),
            num_vars,
            |b, &num_vars| {
                let chunk = create_many_vars_pattern_chunk(num_vars);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
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
                }
            },
        );
    }

    // Repeated variable matching
    for repeats in [10, 50, 100].iter() {
        group.throughput(Throughput::Elements(*repeats as u64));

        group.bench_with_input(
            BenchmarkId::new("repeat_var_bytecode", repeats),
            repeats,
            |b, &repeats| {
                let chunk = Arc::new(create_repeat_var_pattern_chunk(repeats));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("repeat_var_jit", repeats),
            repeats,
            |b, &repeats| {
                let chunk = create_repeat_var_pattern_chunk(repeats);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants();
                    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 256];
                    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 8];

                    b.iter(|| {
                        let mut ctx = unsafe {
                            let mut ctx =
                                JitContext::new(stack.as_mut_ptr(), 256, constants.as_ptr(), constants.len());
                            ctx.binding_frames = binding_frames.as_mut_ptr();
                            ctx.binding_frames_cap = binding_frames.len();
                            ctx
                        };

                        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                            unsafe { std::mem::transmute(code_ptr) };
                        let result = unsafe { native_fn(&mut ctx as *mut JitContext) };
                        black_box(result)
                    })
                }
            },
        );
    }

    // Mixed patterns
    for elements in [6, 12, 24].iter() {
        group.throughput(Throughput::Elements(*elements as u64));

        group.bench_with_input(
            BenchmarkId::new("mixed_bytecode", elements),
            elements,
            |b, &elements| {
                let chunk = Arc::new(create_mixed_pattern_chunk(elements));
                b.iter(|| {
                    let mut vm = mettatron::backend::bytecode::vm::BytecodeVM::new(Arc::clone(&chunk));
                    black_box(vm.run())
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("mixed_jit", elements),
            elements,
            |b, &elements| {
                let chunk = create_mixed_pattern_chunk(elements);
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                if let Ok(code_ptr) = compiler.compile(&chunk) {
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
                }
            },
        );
    }

    group.finish();
}

// ============================================================================
// Placeholder for non-JIT builds
// ============================================================================

#[cfg(not(feature = "jit"))]
fn bench_no_jit_placeholder(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_optimization_benchmarks");
    group.bench_function("placeholder", |b| {
        b.iter(|| black_box(42))
    });
    group.finish();
}

// ============================================================================
// Criterion Groups and Main
// ============================================================================

#[cfg(feature = "jit")]
criterion_group!(
    call_dispatch_benches,
    bench_call_dispatch_grounded,
    bench_call_dispatch_chain,
    bench_call_dispatch_user_rules,
);

#[cfg(feature = "jit")]
criterion_group!(
    pattern_match_benches,
    bench_pattern_match_simple,
    bench_pattern_match_complex,
);

#[cfg(feature = "jit")]
criterion_group!(
    binding_lookup_benches,
    bench_binding_lookup_depth,
    bench_binding_lookup_width,
    bench_binding_lookup_repeated,
);

#[cfg(feature = "jit")]
criterion_group!(
    state_operations_benches,
    bench_state_operations,
);

#[cfg(feature = "jit")]
criterion_group!(
    nondeterminism_benches,
    bench_nondeterminism,
);

#[cfg(feature = "jit")]
criterion_group!(
    pattern_match_variables_benches,
    bench_pattern_match_variables,
);

#[cfg(feature = "jit")]
criterion_main!(
    call_dispatch_benches,
    pattern_match_benches,
    binding_lookup_benches,
    state_operations_benches,
    nondeterminism_benches,
    pattern_match_variables_benches
);

#[cfg(not(feature = "jit"))]
criterion_group!(no_jit_benches, bench_no_jit_placeholder);

#[cfg(not(feature = "jit"))]
criterion_main!(no_jit_benches);
