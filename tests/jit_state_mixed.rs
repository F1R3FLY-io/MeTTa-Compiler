#![cfg(feature = "jit")]

use std::sync::Arc;

use mettatron::backend::bytecode::jit::{JitCompiler, JitContext, JitValue};
use mettatron::backend::bytecode::vm::BytecodeVM;
use mettatron::backend::bytecode::{BytecodeChunk, ChunkBuilder, Opcode};
use mettatron::backend::{MettaEnvironment, MettaValue};

fn create_state_mixed_chunk(ops: usize) -> BytecodeChunk {
    let mut builder = ChunkBuilder::new("state_mixed");

    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::NewState);

    for i in 0..ops {
        if i % 3 == 0 {
            builder.emit(Opcode::Dup);
            builder.emit(Opcode::GetState);
            builder.emit(Opcode::Pop);
        } else if i % 3 == 1 {
            builder.emit_byte(Opcode::PushLongSmall, ((i + 1) % 256) as u8);
            builder.emit(Opcode::ChangeState);
        } else {
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

fn run_mixed_bytecode(ops: usize) -> MettaValue {
    let chunk = Arc::new(create_state_mixed_chunk(ops));
    let env = MettaEnvironment::default();
    let mut vm = BytecodeVM::with_env(Arc::clone(&chunk), env);
    let results = vm.run().expect("mixed state bytecode should execute");

    assert_eq!(
        results.len(),
        1,
        "mixed state bytecode should return one value for {ops} ops"
    );
    results[0].clone()
}

fn run_mixed_jit(ops: usize) -> MettaValue {
    let chunk = create_state_mixed_chunk(ops);
    let mut compiler = JitCompiler::new().expect("JIT compiler should initialize");
    let code_ptr = compiler
        .compile(&chunk)
        .expect("mixed state chunk should compile to JIT");
    let constants = chunk.constants();
    let mut stack = vec![JitValue::unit(); 256];
    let mut env = MettaEnvironment::default();

    let mut ctx = unsafe {
        let mut ctx = JitContext::new(
            stack.as_mut_ptr(),
            stack.len(),
            constants.as_ptr(),
            constants.len(),
        );
        ctx.env_ptr = &mut env as *mut MettaEnvironment as *mut ();
        ctx
    };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let raw = unsafe { native_fn(&mut ctx as *mut JitContext) } as u64;
    let value = JitValue::from_raw(raw);

    assert!(
        value.is_valid_tag(),
        "mixed state JIT returned invalid value bits {raw:#x} for {ops} ops"
    );
    assert!(
        !value.is_error(),
        "mixed state JIT returned error bits {raw:#x} for {ops} ops"
    );

    unsafe { value.to_metta() }
}

#[test]
fn mixed_state_workload_matches_bytecode_under_direct_jit() {
    for ops in [0, 1, 2, 3, 10, 50, 100] {
        let bytecode = run_mixed_bytecode(ops);
        let jit = run_mixed_jit(ops);
        assert_eq!(
            jit, bytecode,
            "mixed state JIT result diverged from bytecode for {ops} ops"
        );
    }
}
