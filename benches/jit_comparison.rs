//! Benchmark comparing JIT vs Bytecode VM vs Tree-Walking Interpreter
//!
//! This benchmark measures the performance of the three-tier evaluation system:
//! - Tier 0: Tree-walking interpreter (baseline)
//! - Tier 1: Bytecode VM
//! - Tier 2: Cranelift JIT (Stage 1 primitives + Stage 2 runtime calls)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mettatron::backend::bytecode::{compile, BytecodeVM};
use mettatron::backend::eval::eval;
use mettatron::backend::{Environment, MettaValue};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "jit")]
use mettatron::backend::bytecode::jit::{JitCompiler, JitContext, JitValue};

/// Helper to create an atom
fn atom(name: &str) -> MettaValue {
    MettaValue::Atom(name.to_string())
}

/// Helper to create an s-expression
fn sexpr(items: Vec<MettaValue>) -> MettaValue {
    MettaValue::SExpr(items)
}

/// Evaluate expression via tree-walking interpreter
fn eval_tree_walker(expr: &MettaValue) -> Vec<MettaValue> {
    let env = Environment::new();
    let (results, _env) = eval(expr.clone(), env);
    results
}

/// Evaluate expression via bytecode VM
fn eval_bytecode(expr: &MettaValue) -> Vec<MettaValue> {
    let chunk = compile("bench", expr).expect("compilation failed");
    let mut vm = BytecodeVM::new(Arc::new(chunk));
    vm.run().expect("VM execution failed")
}

/// Evaluate pre-compiled bytecode chunk (no compilation overhead)
fn eval_bytecode_precompiled(chunk: &Arc<mettatron::backend::bytecode::BytecodeChunk>) -> Vec<MettaValue> {
    let mut vm = BytecodeVM::new(Arc::clone(chunk));
    vm.run().expect("VM execution failed")
}

/// Execute JIT-compiled code directly
#[cfg(feature = "jit")]
unsafe fn exec_jit_code(code_ptr: *const (), constants: &[MettaValue]) -> i64 {
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx = JitContext::new(
        stack.as_mut_ptr(),
        64,
        constants.as_ptr(),
        constants.len(),
    );

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        std::mem::transmute(code_ptr);
    native_fn(&mut ctx as *mut JitContext)
}

// ============================================================================
// Benchmark 1: Pure Arithmetic (JIT-compilable)
// ============================================================================

/// Build nested arithmetic expression: ((((1 + 0) + 1) + 2) + ...)
/// Using only small constants to stay within PushLongSmall range
fn build_jit_arithmetic(depth: usize) -> MettaValue {
    let mut expr = MettaValue::Long(1);
    for i in 0..depth {
        let val = (i % 100) as i64; // Keep values small
        expr = sexpr(vec![atom("+"), expr, MettaValue::Long(val)]);
    }
    expr
}

fn bench_jit_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_arithmetic");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for depth in [10, 50, 100, 200].iter() {
        let expr = build_jit_arithmetic(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        // Tree-walker baseline
        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        // Bytecode VM (compile + execute)
        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        // Bytecode VM (execute only, pre-compiled)
        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        // JIT compiled execution
        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let constants = chunk.constants().to_vec();

                group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                    b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                });
            }
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 2: Boolean Logic (JIT-compilable)
// ============================================================================

/// Build nested boolean expression using and/or/not
fn build_jit_boolean(depth: usize) -> MettaValue {
    let mut expr = MettaValue::Bool(true);
    for i in 0..depth {
        expr = match i % 3 {
            0 => sexpr(vec![atom("not"), expr]),
            1 => sexpr(vec![atom("or"), expr, MettaValue::Bool(false)]),
            _ => sexpr(vec![atom("and"), expr, MettaValue::Bool(true)]),
        };
    }
    expr
}

fn bench_jit_boolean(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_boolean");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for depth in [10, 50, 100, 200].iter() {
        let expr = build_jit_boolean(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let constants = chunk.constants().to_vec();

                group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                    b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                });
            }
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 3: Mixed Arithmetic + Comparisons (JIT-compilable)
// ============================================================================

/// Build expression with arithmetic then final comparison: ((a + b - c * d) < limit)
/// This avoids the issue of mixing Bool results back into arithmetic chains
fn build_jit_mixed(depth: usize) -> MettaValue {
    // Build an arithmetic chain
    let mut expr = MettaValue::Long(1);
    for i in 0..depth {
        let val = ((i % 50) + 1) as i64;
        expr = match i % 3 {
            0 => sexpr(vec![atom("+"), expr, MettaValue::Long(val)]),
            1 => sexpr(vec![atom("-"), expr, MettaValue::Long(val / 2)]),
            _ => sexpr(vec![atom("*"), expr, MettaValue::Long(2)]),
        };
    }
    // Final comparison at the end (result is Bool, but no further ops)
    sexpr(vec![atom("<"), expr, MettaValue::Long(1_000_000)])
}

fn bench_jit_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_mixed");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for depth in [10, 50, 100].iter() {
        let expr = build_jit_mixed(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let constants = chunk.constants().to_vec();

                group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                    b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                });
            }
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 4: Pow with Runtime Calls (JIT Stage 2)
// ============================================================================

/// Build nested power expression: 2^2^2^2 (modular to avoid overflow)
/// This tests the Stage 2 runtime call infrastructure
fn build_jit_pow(depth: usize) -> MettaValue {
    // Chain of small power operations to test runtime calls
    // Each result is used as the exponent for the next, but we keep bases small
    let mut expr = MettaValue::Long(2);
    for _ in 0..depth {
        // 2^2 = 4, 4^2 = 16, etc. - keeps values reasonable for testing
        expr = sexpr(vec![atom("pow"), MettaValue::Long(2), expr]);
    }
    expr
}

fn bench_jit_pow(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_pow");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    // Use smaller depths because Pow creates large values quickly
    for depth in [1, 2, 3, 5].iter() {
        let expr = build_jit_pow(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let constants = chunk.constants().to_vec();

                group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                    b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                });
            }
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 5: JIT Compilation Overhead
// ============================================================================

#[cfg(feature = "jit")]
#[allow(dead_code)]
fn bench_jit_compilation_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_compilation_overhead");
    group.measurement_time(Duration::from_secs(10));

    for depth in [10, 50, 100, 200].iter() {
        let expr = build_jit_arithmetic(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        if !JitCompiler::can_compile_stage1(&chunk) {
            continue;
        }

        // Bytecode compilation only
        group.bench_with_input(
            BenchmarkId::new("bytecode-compile", depth),
            &expr,
            |b, expr| {
                b.iter(|| compile("bench", black_box(expr)))
            },
        );

        // JIT compilation only (from bytecode)
        group.bench_with_input(
            BenchmarkId::new("jit-compile", depth),
            &chunk,
            |b, chunk| {
                b.iter(|| {
                    let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                    compiler.compile(black_box(chunk))
                })
            },
        );

        // Full pipeline: bytecode + JIT compile + execute
        group.bench_with_input(
            BenchmarkId::new("full-pipeline", depth),
            &expr,
            |b, expr| {
                b.iter(|| {
                    let chunk = compile("bench", black_box(expr)).expect("compilation failed");
                    let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                    let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                    let constants = chunk.constants().to_vec();
                    unsafe { exec_jit_code(code_ptr, &constants) }
                })
            },
        );
    }

    group.finish();
}

#[cfg(not(feature = "jit"))]
fn bench_jit_compilation_overhead(_c: &mut Criterion) {
    // No-op when JIT is disabled
}

// ============================================================================
// Benchmark 6: Repeated Execution (amortized JIT benefit)
// ============================================================================

fn bench_repeated_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("repeated_execution");
    group.measurement_time(Duration::from_secs(10));

    let depth = 100;
    let iterations = 1000;
    let expr = build_jit_arithmetic(depth);
    let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

    group.throughput(Throughput::Elements(iterations as u64));

    // Tree-walker: N iterations
    group.bench_function("tree-walker-1000x", |b| {
        b.iter(|| {
            for _ in 0..iterations {
                black_box(eval_tree_walker(&expr));
            }
        })
    });

    // Bytecode VM: N iterations (with recompilation each time)
    group.bench_function("bytecode-recompile-1000x", |b| {
        b.iter(|| {
            for _ in 0..iterations {
                black_box(eval_bytecode(&expr));
            }
        })
    });

    // Bytecode VM: N iterations (pre-compiled)
    group.bench_function("bytecode-precompiled-1000x", |b| {
        b.iter(|| {
            for _ in 0..iterations {
                black_box(eval_bytecode_precompiled(&chunk));
            }
        })
    });

    // JIT: N iterations (pre-JIT-compiled)
    #[cfg(feature = "jit")]
    {
        if JitCompiler::can_compile_stage1(&chunk) {
            let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
            let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
            let constants = chunk.constants().to_vec();

            group.bench_function("jit-precompiled-1000x", |b| {
                b.iter(|| {
                    for _ in 0..iterations {
                        black_box(unsafe { exec_jit_code(code_ptr, &constants) });
                    }
                })
            });
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 7: Control Flow (if/then/else chains)
// ============================================================================

/// Build nested if/then/else chain testing native Phase E `if` optimization
fn build_jit_if_chain(depth: usize) -> MettaValue {
    // (if (< 5 10) (if (< 3 5) (if ... result1 result2) result2) result2)
    let mut expr = MettaValue::Long(42); // Final result
    for i in 0..depth {
        let a = ((i * 3) % 20) as i64;
        let b = ((i * 3 + 10) % 30) as i64;
        let condition = sexpr(vec![atom("<"), MettaValue::Long(a), MettaValue::Long(b)]);
        let else_branch = MettaValue::Long(-1);
        expr = sexpr(vec![atom("if"), condition, expr, else_branch]);
    }
    expr
}

fn bench_jit_if_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_if_chain");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for depth in [5, 10, 20, 50].iter() {
        let expr = build_jit_if_chain(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let constants = chunk.constants().to_vec();

                group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                    b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                });
            }
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 8: Let Bindings (let/let* chains)
// ============================================================================

/// Build nested let bindings testing native Phase E `let` optimization
fn build_jit_let_chain(depth: usize) -> MettaValue {
    // (let $x 1 (let $y (+ $x 1) (let $z (+ $y 1) ...)))
    // Each binding builds on the previous
    let mut expr = MettaValue::Long(0);
    let mut counter = 0i64;

    for i in (0..depth).rev() {
        let var_name = format!("$v{}", i);
        let var = atom(&var_name);  // Variables are atoms with $ prefix

        // Each let binds a fresh value
        counter += 1;
        let value = MettaValue::Long(counter);

        // Build: (let $vN valueN body)
        expr = sexpr(vec![atom("let"), var, value, expr]);
    }
    expr
}

fn bench_jit_let_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_let_chain");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for depth in [5, 10, 20, 50].iter() {
        let expr = build_jit_let_chain(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                // Gracefully skip JIT if compilation fails (e.g., StackUnderflow)
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants().to_vec();

                    group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                        b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                    });
                }
            }
        }
    }

    group.finish();
}

// ============================================================================
// Benchmark 9: Hybrid Workload (mix of arithmetic, if, and let)
// ============================================================================

/// Build expression that exercises multiple JIT-optimized paths
fn build_jit_hybrid_workload(depth: usize) -> MettaValue {
    // (let $a (+ 1 2)
    //   (if (< $a 10)
    //     (let $b (* $a 2)
    //       (if (> $b 5)
    //         (+ $b 10)
    //         0))
    //     0))
    let mut expr = MettaValue::Long(1);

    for i in 0..depth {
        let val = (i % 10) as i64;
        let var_name = format!("$h{}", i);
        let var = atom(&var_name);  // Variables are atoms with $ prefix

        match i % 3 {
            0 => {
                // Arithmetic
                expr = sexpr(vec![atom("+"), expr, MettaValue::Long(val)]);
            }
            1 => {
                // If condition
                let cond = sexpr(vec![atom("<"), MettaValue::Long(val), MettaValue::Long(15)]);
                expr = sexpr(vec![atom("if"), cond, expr, MettaValue::Long(0)]);
            }
            _ => {
                // Let binding
                expr = sexpr(vec![atom("let"), var, MettaValue::Long(val), expr]);
            }
        }
    }
    expr
}

fn bench_jit_hybrid_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_hybrid_workload");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for depth in [10, 30, 60, 100].iter() {
        let expr = build_jit_hybrid_workload(*depth);
        let chunk = Arc::new(compile("bench", &expr).expect("compilation failed"));

        group.throughput(Throughput::Elements(*depth as u64));

        group.bench_with_input(BenchmarkId::new("tree-walker", depth), &expr, |b, expr| {
            b.iter(|| eval_tree_walker(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-full", depth), &expr, |b, expr| {
            b.iter(|| eval_bytecode(black_box(expr)))
        });

        group.bench_with_input(BenchmarkId::new("bytecode-exec", depth), &chunk, |b, chunk| {
            b.iter(|| eval_bytecode_precompiled(black_box(chunk)))
        });

        #[cfg(feature = "jit")]
        {
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
                // Gracefully skip JIT if compilation fails (e.g., StackUnderflow)
                if let Ok(code_ptr) = compiler.compile(&chunk) {
                    let constants = chunk.constants().to_vec();

                    group.bench_with_input(BenchmarkId::new("jit-exec", depth), &(), |b, _| {
                        b.iter(|| unsafe { exec_jit_code(black_box(code_ptr), &constants) })
                    });
                }
            }
        }
    }

    group.finish();
}

// ============================================================================
// Main
// ============================================================================

criterion_group!(
    benches,
    bench_jit_arithmetic,
    bench_jit_boolean,
    bench_jit_mixed,
    bench_jit_pow,
    bench_repeated_execution,
    bench_jit_if_chain,
    bench_jit_let_chain,
    bench_jit_hybrid_workload,
);

criterion_main!(benches);
