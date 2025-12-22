//! JIT profiling example - runs JIT code in a tight loop for perf analysis
//!
//! Usage:
//!   cargo run --example jit_profile --features jit --release          # JIT mode (default)
//!   cargo run --example jit_profile --features jit --release -- vm    # Bytecode VM mode

use mettatron::backend::bytecode::{compile, BytecodeVM};
use mettatron::backend::MettaValue;
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "jit")]
use mettatron::backend::bytecode::jit::{JitCompiler, JitContext, JitValue, JitBailoutReason};

fn atom(name: &str) -> MettaValue {
    MettaValue::Atom(name.to_string())
}

fn sexpr(items: Vec<MettaValue>) -> MettaValue {
    MettaValue::SExpr(items)
}

/// Build nested arithmetic expression
fn build_arithmetic(depth: usize) -> MettaValue {
    let mut expr = MettaValue::Long(1);
    for i in 0..depth {
        let val = (i % 100) as i64;
        expr = sexpr(vec![atom("+"), expr, MettaValue::Long(val)]);
    }
    expr
}

#[cfg(feature = "jit")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let use_vm = args.get(1).map(|s| s == "vm").unwrap_or(false);

    let depth = 100;
    let iterations: u64 = if use_vm { 10_000_000 } else { 100_000_000 };

    println!("Building expression with depth {}...", depth);
    let expr = build_arithmetic(depth);

    println!("Compiling to bytecode...");
    let chunk = Arc::new(compile("profile", &expr).expect("compilation failed"));

    if use_vm {
        println!("Running {} bytecode VM iterations...", iterations);
        let start = Instant::now();

        let mut result = Vec::new();
        for _ in 0..iterations {
            let mut vm = BytecodeVM::new(Arc::clone(&chunk));
            result = vm.run().expect("VM execution failed");
        }

        let elapsed = start.elapsed();
        let ns_per_iter = elapsed.as_nanos() as f64 / iterations as f64;

        println!("Result: {:?}", result);
        println!("Total time: {:?}", elapsed);
        println!("Per iteration: {:.2} ns", ns_per_iter);
        println!("Throughput: {:.2} Melem/s", (depth as f64 * iterations as f64) / elapsed.as_secs_f64() / 1e6);
    } else {
        if !JitCompiler::can_compile_stage1(&chunk) {
            eprintln!("Chunk is not JIT-compilable!");
            return;
        }

        println!("JIT compiling...");
        let mut compiler = JitCompiler::new().expect("JIT compiler creation failed");
        let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
        let constants = chunk.constants().to_vec();

        println!("Running {} JIT iterations...", iterations);
        let start = Instant::now();

        let mut result: i64 = 0;
        for _ in 0..iterations {
            result = unsafe { exec_jit_code(code_ptr, &constants) };
        }

        let elapsed = start.elapsed();
        let ns_per_iter = elapsed.as_nanos() as f64 / iterations as f64;

        println!("Result: {}", result);
        println!("Total time: {:?}", elapsed);
        println!("Per iteration: {:.2} ns", ns_per_iter);
        println!("Throughput: {:.2} Gelem/s", (depth as f64 * iterations as f64) / elapsed.as_secs_f64() / 1e9);
    }
}

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

#[cfg(not(feature = "jit"))]
fn main() {
    eprintln!("This example requires the 'jit' feature. Run with: cargo run --example jit_profile --features jit --release");
}
