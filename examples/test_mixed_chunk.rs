// Quick test for mixed chunk
use std::sync::Arc;

fn main() {
    use mettatron::backend::bytecode::{BytecodeChunk, ChunkBuilder, Opcode};
    use mettatron::backend::bytecode::vm::{BytecodeVM, VmConfig};
    use mettatron::backend::Environment;

    let ops = 10;
    let mut builder = ChunkBuilder::new("state_mixed");

    // Create initial state
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::NewState);

    // Mixed operations
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
    let chunk = builder.build();

    println!("Chunk built, running...");
    let chunk = Arc::new(chunk);
    let mut vm = BytecodeVM::with_config_and_env(
        Arc::clone(&chunk),
        VmConfig::default(),
        Environment::new(),
    );
    
    match vm.run() {
        Ok(result) => println!("Result: {:?}", result),
        Err(e) => println!("Error: {:?}", e),
    }
}
