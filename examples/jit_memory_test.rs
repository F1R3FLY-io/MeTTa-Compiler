//! Memory leak test for JIT compiler
//! Run with: valgrind --leak-check=full ./target/release/examples/jit_memory_test

use mettatron::backend::bytecode::{ChunkBuilder, Opcode};
use mettatron::backend::MettaValue;
use std::sync::Arc;

#[cfg(feature = "jit")]
use mettatron::backend::bytecode::jit::{JitCompiler, JitContext, JitValue};

fn create_simple_chunk() -> mettatron::backend::bytecode::BytecodeChunk {
    let mut builder = ChunkBuilder::new("test");
    let head_idx = builder.add_constant(MettaValue::Atom("+".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 40);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_call(head_idx, 2);
    builder.emit(Opcode::Return);
    builder.build()
}

#[cfg(feature = "jit")]
fn run_jit_iterations(iterations: usize) {
    println!("Running {} JIT compile/execute iterations...", iterations);

    for i in 0..iterations {
        // Create a fresh compiler each iteration to test for leaks
        let chunk = create_simple_chunk();
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        if let Ok(code_ptr) = compiler.compile(&chunk) {
            let constants = chunk.constants();
            let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

            // Execute the compiled code
            let mut ctx = unsafe {
                JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
            };

            let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
                unsafe { std::mem::transmute(code_ptr) };
            let result = unsafe { native_fn(&mut ctx as *mut JitContext) };

            if i == 0 || (i + 1) % 100 == 0 {
                println!("  Iteration {}: result = {}", i + 1, result);
            }
        }

        // Compiler should be dropped here, releasing JITModule memory
    }

    println!("Completed {} iterations", iterations);
}

#[cfg(not(feature = "jit"))]
fn run_jit_iterations(_iterations: usize) {
    println!("JIT feature not enabled");
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    run_jit_iterations(iterations);

    println!("Done. Check valgrind output for memory leaks.");
}
