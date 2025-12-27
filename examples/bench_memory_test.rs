//! Memory test that mimics the benchmark pattern
//! Run with: valgrind --tool=massif ./target/release/examples/bench_memory_test

use mettatron::backend::bytecode::vm::BytecodeVM;
use mettatron::backend::bytecode::{BytecodeChunk, ChunkBuilder, Opcode};
use mettatron::backend::MettaValue;
use std::sync::Arc;

#[cfg(feature = "jit")]
use mettatron::backend::bytecode::jit::{JitCompiler, JitContext, JitValue};

fn create_grounded_call_chunk(op: &str, a: i64, b: i64) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("grounded_call");
    let head_idx = builder.add_constant(MettaValue::Atom(op.to_string()));
    builder.emit_byte(Opcode::PushLongSmall, a as u8);
    builder.emit_byte(Opcode::PushLongSmall, b as u8);
    builder.emit_call(head_idx, 2);
    builder.emit(Opcode::Return);
    builder.build()
}

fn run_bytecode_benchmark(iterations: usize) {
    println!("Running {} bytecode iterations...", iterations);
    let chunk = Arc::new(create_grounded_call_chunk("+", 40, 2));

    for i in 0..iterations {
        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let _result = vm.run();

        if (i + 1) % 10000 == 0 {
            println!("  Bytecode iteration {}", i + 1);
        }
    }
}

#[cfg(feature = "jit")]
fn run_jit_benchmark(iterations: usize) {
    println!("Running {} JIT iterations...", iterations);
    let chunk = create_grounded_call_chunk("+", 40, 2);
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    if let Ok(code_ptr) = compiler.compile(&chunk) {
        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        for i in 0..iterations {
            let mut ctx = unsafe {
                JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
            };

            let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                unsafe { std::mem::transmute(code_ptr) };
            let _result = unsafe { native_fn(&mut ctx as *mut JitContext) };

            if (i + 1) % 10000 == 0 {
                println!("  JIT iteration {}", i + 1);
            }
        }
    }
}

#[cfg(not(feature = "jit"))]
fn run_jit_benchmark(_iterations: usize) {
    println!("JIT feature not enabled");
}

fn run_multi_compile(num_compilers: usize) {
    println!(
        "Creating {} JIT compilers to test memory growth...",
        num_compilers
    );

    #[cfg(feature = "jit")]
    {
        let mut compilers = Vec::new();

        for i in 0..num_compilers {
            let chunk = create_grounded_call_chunk("+", 40, 2);
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let _ = compiler.compile(&chunk);
            compilers.push(compiler);

            if (i + 1) % 10 == 0 {
                println!("  Created {} compilers", i + 1);
            }
        }

        println!("Holding {} compilers in memory...", compilers.len());
        std::thread::sleep(std::time::Duration::from_secs(1));

        println!("Dropping all compilers...");
        drop(compilers);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("all");
    let iterations: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100000);

    match mode {
        "bytecode" => run_bytecode_benchmark(iterations),
        "jit" => run_jit_benchmark(iterations),
        "multi" => run_multi_compile(iterations.min(100)),
        _ => {
            run_bytecode_benchmark(iterations);
            run_jit_benchmark(iterations);
            run_multi_compile(50);
        }
    }

    println!("Done");
}
