use super::*;
use crate::backend::bytecode::ChunkBuilder;

#[test]
fn test_can_compile_stage1_arithmetic() {
    let mut builder = ChunkBuilder::new("test_arith");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));
}

#[test]
fn test_can_compile_stage1_boolean() {
    let mut builder = ChunkBuilder::new("test_bool");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Or);
    builder.emit(Opcode::Not);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));
}

#[test]
fn test_can_compile_calls_with_bailout() {
    // Phase 3: Call is now compilable with bailout semantics
    let mut builder = ChunkBuilder::new("test_call");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_call(0, 1); // head_index=0, arity=1
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Call IS compilable in Stage 1 (with bailout)
    assert!(JitCompiler::can_compile_stage1(&chunk));
}

#[test]
fn test_can_compile_fork_with_bailout() {
    // Phase 9: Fork is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_nondet");
    builder.emit_byte(Opcode::Fork, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Fork is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Fork chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_compile_simple_addition() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("test_add");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let result = compiler.compile(&chunk);
    assert!(result.is_ok(), "Compilation failed: {:?}", result.err());
}

// =========================================================================
// End-to-End JIT Execution Tests
// =========================================================================

#[test]
fn test_jit_execute_addition() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 10 + 20 = 30
    let mut builder = ChunkBuilder::new("e2e_add");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Compile to native code
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    // Set up JIT context (needed for bailout signaling, constants, etc.)
    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    // Execute native code - returns NaN-boxed result directly
    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Verify result
    assert!(!ctx.bailout, "JIT execution bailed out unexpectedly");

    // Interpret the return value as a JitValue
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 30, "Expected 10 + 20 = 30");
}

#[test]
fn test_jit_execute_arithmetic_chain() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: ((10 + 20) * 3) - 5 = 85
    let mut builder = ChunkBuilder::new("e2e_chain");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Add); // 30
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit(Opcode::Mul); // 90
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Sub); // 85
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert_eq!(result.as_long(), 85);
}

#[test]
fn test_jit_execute_boolean_logic() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: (True or False) and (not False) = True
    let mut builder = ChunkBuilder::new("e2e_bool");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Or); // True
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Not); // True
    builder.emit(Opcode::And); // True
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_bool(), "Expected Bool result");
    assert!(result.as_bool(), "Expected True");
}

#[test]
fn test_jit_execute_comparison() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 5 < 10 = True
    let mut builder = ChunkBuilder::new("e2e_cmp");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Lt);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_bool(), "Expected Bool result");
    assert!(result.as_bool(), "Expected 5 < 10 = True");
}

#[test]
fn test_jit_execute_division() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 100 / 4 = 25
    let mut builder = ChunkBuilder::new("e2e_div");
    builder.emit_byte(Opcode::PushLongSmall, 100);
    builder.emit_byte(Opcode::PushLongSmall, 4);
    builder.emit(Opcode::Div);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 25, "Expected 100 / 4 = 25");
}

#[test]
fn test_jit_execute_modulo() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 17 % 5 = 2
    let mut builder = ChunkBuilder::new("e2e_mod");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 2, "Expected 17 % 5 = 2");
}

// =========================================================================
// Stage 2: Pow (Runtime Call) Tests
// =========================================================================

#[test]
fn test_can_compile_pow() {
    // Test that Pow is now compilable (Stage 2)
    let mut builder = ChunkBuilder::new("test_pow_compilable");
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Pow should now be Stage 2 compilable"
    );
}

#[test]
fn test_jit_execute_pow() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 2^10 = 1024
    let mut builder = ChunkBuilder::new("e2e_pow");
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Pow should be compilable"
    );

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 1024, "Expected 2^10 = 1024");
}

#[test]
fn test_jit_execute_pow_zero_exponent() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 5^0 = 1
    let mut builder = ChunkBuilder::new("e2e_pow_zero");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 1, "Expected 5^0 = 1");
}

// =========================================================================
// Stage 2: PushConstant (Runtime Call) Tests
// =========================================================================

#[test]
fn test_can_compile_push_constant() {
    use crate::backend::MettaValue;

    // Test that PushConstant is now compilable (Stage 2)
    let mut builder = ChunkBuilder::new("test_const_compilable");
    let idx = builder.add_constant(MettaValue::Long(1_000_000));
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PushConstant should now be Stage 2 compilable"
    );
}

#[test]
fn test_jit_execute_push_constant() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: load constant 1_000_000
    let mut builder = ChunkBuilder::new("e2e_const");
    let idx = builder.add_constant(MettaValue::Long(1_000_000));
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 1_000_000, "Expected constant 1_000_000");
}

#[test]
fn test_jit_execute_push_constant_arithmetic() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 1_000_000 + 500_000 = 1_500_000
    let mut builder = ChunkBuilder::new("e2e_const_arith");
    let idx1 = builder.add_constant(MettaValue::Long(1_000_000));
    let idx2 = builder.add_constant(MettaValue::Long(500_000));
    builder.emit_u16(Opcode::PushConstant, idx1);
    builder.emit_u16(Opcode::PushConstant, idx2);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout);

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        1_500_000,
        "Expected 1_000_000 + 500_000 = 1_500_000"
    );
}

// =========================================================================
// Integration Tests: MeTTa Expression → Bytecode → JIT
// =========================================================================

/// Helper to execute JIT code and return the result

fn exec_jit(
    code_ptr: *const (),
    constants: &[crate::backend::MettaValue],
) -> crate::backend::bytecode::jit::JitValue {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};

    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution bailed out unexpectedly");
    JitValue::from_raw(result_bits as u64)
}

#[test]
fn test_jit_integration_simple_arithmetic() {
    use crate::backend::bytecode::compile;
    use crate::backend::MettaValue;

    // Build MeTTa expression: (+ 10 20) = 30
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(20),
    ]);

    let chunk = compile("test", &expr).expect("Compilation failed");

    if !JitCompiler::can_compile_stage1(&chunk) {
        // Skip if not JIT-compilable (e.g., uses non-primitive ops)
        return;
    }

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");
    let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 30, "Expected (+ 10 20) = 30");
}

#[test]
fn test_jit_integration_nested_arithmetic() {
    use crate::backend::bytecode::compile;
    use crate::backend::MettaValue;

    // Build MeTTa expression: (+ (- 100 50) (* 5 3)) = 50 + 15 = 65
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("-".to_string()),
            MettaValue::Long(100),
            MettaValue::Long(50),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(3),
        ]),
    ]);

    let chunk = compile("test", &expr).expect("Compilation failed");

    if !JitCompiler::can_compile_stage1(&chunk) {
        return;
    }

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");
    let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 65, "Expected (+ (- 100 50) (* 5 3)) = 65");
}

#[test]
fn test_jit_integration_comparison_chain() {
    use crate::backend::bytecode::compile;
    use crate::backend::MettaValue;

    // Build MeTTa expression: (< (+ 5 5) 20) = True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
        ]),
        MettaValue::Long(20),
    ]);

    let chunk = compile("test", &expr).expect("Compilation failed");

    if !JitCompiler::can_compile_stage1(&chunk) {
        return;
    }

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");
    let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert!(result.as_bool(), "Expected (< (+ 5 5) 20) = True");
}

#[test]
fn test_jit_integration_pow() {
    use crate::backend::bytecode::compile;
    use crate::backend::MettaValue;

    // Build MeTTa expression: (pow 2 8) = 256
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(8),
    ]);

    let chunk = compile("test", &expr).expect("Compilation failed");

    if !JitCompiler::can_compile_stage1(&chunk) {
        return;
    }

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");
    let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 256, "Expected (pow 2 8) = 256");
}

// =========================================================================
// JIT vs VM Equivalence Tests
// =========================================================================

#[test]
fn test_jit_vm_equivalence_arithmetic() {
    use crate::backend::bytecode::{compile, BytecodeVM};
    use crate::backend::MettaValue;
    use std::sync::Arc;

    // Test various arithmetic expressions
    let test_cases = vec![
        (vec!["+", "10", "20"], 30i64),
        (vec!["-", "100", "45"], 55i64),
        (vec!["*", "7", "8"], 56i64),
        (vec!["/", "100", "4"], 25i64),
        (vec!["%", "17", "5"], 2i64),
    ];

    for (ops, expected) in test_cases {
        let op = ops[0];
        let a: i64 = ops[1].parse().unwrap();
        let b: i64 = ops[2].parse().unwrap();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom(op.to_string()),
            MettaValue::Long(a),
            MettaValue::Long(b),
        ]);

        let chunk = Arc::new(compile("equiv", &expr).expect("Compilation failed"));

        // VM result
        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let vm_results = vm.run().expect("VM execution failed");

        // JIT result (if compilable)
        if JitCompiler::can_compile_stage1(&chunk) {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
            let jit_result = exec_jit(code_ptr, chunk.constants());

            assert!(
                jit_result.is_long(),
                "JIT: Expected Long for ({} {} {})",
                op,
                a,
                b
            );
            let jit_val = jit_result.as_long();

            // Compare with VM
            assert_eq!(vm_results.len(), 1, "VM should return single value");
            if let MettaValue::Long(vm_val) = &vm_results[0] {
                assert_eq!(
                    jit_val, *vm_val,
                    "JIT vs VM mismatch for ({} {} {})",
                    op, a, b
                );
                assert_eq!(
                    jit_val, expected,
                    "Expected {} for ({} {} {})",
                    expected, op, a, b
                );
            } else {
                panic!("VM returned non-Long value");
            }
        }
    }
}

#[test]
fn test_jit_vm_equivalence_comparisons() {
    use crate::backend::bytecode::{compile, BytecodeVM};
    use crate::backend::MettaValue;
    use std::sync::Arc;

    // Test comparison operators
    let test_cases = vec![
        ("<", 5, 10, true),
        ("<", 10, 5, false),
        ("<=", 5, 5, true),
        ("<=", 6, 5, false),
        (">", 10, 5, true),
        (">", 5, 10, false),
        (">=", 5, 5, true),
        (">=", 4, 5, false),
        ("==", 42, 42, true),
        ("==", 42, 43, false),
    ];

    for (op, a, b, expected) in test_cases {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom(op.to_string()),
            MettaValue::Long(a),
            MettaValue::Long(b),
        ]);

        let chunk = Arc::new(compile("equiv_cmp", &expr).expect("Compilation failed"));

        // VM result
        let mut vm = BytecodeVM::new(Arc::clone(&chunk));
        let vm_results = vm.run().expect("VM execution failed");

        // JIT result
        if JitCompiler::can_compile_stage1(&chunk) {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
            let jit_result = exec_jit(code_ptr, chunk.constants());

            assert!(
                jit_result.is_bool(),
                "JIT: Expected Bool for ({} {} {})",
                op,
                a,
                b
            );
            let jit_val = jit_result.as_bool();

            assert_eq!(vm_results.len(), 1);
            if let MettaValue::Bool(vm_val) = &vm_results[0] {
                assert_eq!(
                    jit_val, *vm_val,
                    "JIT vs VM mismatch for ({} {} {})",
                    op, a, b
                );
                assert_eq!(
                    jit_val, expected,
                    "Expected {} for ({} {} {})",
                    expected, op, a, b
                );
            } else {
                panic!("VM returned non-Bool value");
            }
        }
    }
}

// =========================================================================
// Edge Cases and Boundary Tests
// =========================================================================

#[test]
fn test_jit_execute_negative_numbers() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: -10 + 5 = -5
    // PushLongSmall operand is i8, cast to u8 for emit_byte
    let mut builder = ChunkBuilder::new("e2e_neg");
    builder.emit_byte(Opcode::PushLongSmall, (-10i8) as u8);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), -5, "Expected -10 + 5 = -5");
}

#[test]
fn test_jit_execute_all_comparisons() {
    let test_cases = vec![
        (Opcode::Lt, 5, 10, true),   // 5 < 10
        (Opcode::Lt, 10, 5, false),  // 10 < 5
        (Opcode::Le, 5, 5, true),    // 5 <= 5
        (Opcode::Le, 6, 5, false),   // 6 <= 5
        (Opcode::Gt, 10, 5, true),   // 10 > 5
        (Opcode::Gt, 5, 10, false),  // 5 > 10
        (Opcode::Ge, 5, 5, true),    // 5 >= 5
        (Opcode::Ge, 4, 5, false),   // 4 >= 5
        (Opcode::Eq, 42, 42, true),  // 42 == 42
        (Opcode::Eq, 42, 43, false), // 42 == 43
        (Opcode::Ne, 42, 43, true),  // 42 != 43
        (Opcode::Ne, 42, 42, false), // 42 != 42
    ];

    for (op, a, b, expected) in test_cases {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("cmp_test");
        builder.emit_byte(Opcode::PushLongSmall, a);
        builder.emit_byte(Opcode::PushLongSmall, b);
        builder.emit(op);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool for {:?}({}, {})", op, a, b);
        assert_eq!(
            result.as_bool(),
            expected,
            "{:?}({}, {}) should be {}",
            op,
            a,
            b,
            expected
        );
    }
}

#[test]
fn test_jit_execute_stack_operations() {
    // Test Dup: duplicate top of stack
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("dup_test");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Add); // 42 + 42 = 84
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert_eq!(result.as_long(), 84, "Dup: 42 + 42 = 84");
    }

    // Test Swap: swap top two values
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("swap_test");
        builder.emit_byte(Opcode::PushLongSmall, 10); // bottom
        builder.emit_byte(Opcode::PushLongSmall, 3); // top
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Sub); // 3 - 10 = -7
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert_eq!(result.as_long(), -7, "Swap: 3 - 10 = -7");
    }

    // Test Over: copy second value to top
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("over_test");
        builder.emit_byte(Opcode::PushLongSmall, 5); // bottom
        builder.emit_byte(Opcode::PushLongSmall, 10); // top
        builder.emit(Opcode::Over); // copies 5 to top: [5, 10, 5]
        builder.emit(Opcode::Add); // 10 + 5 = 15: [5, 15]
        builder.emit(Opcode::Add); // 5 + 15 = 20: [20]
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert_eq!(result.as_long(), 20, "Over: 5 + (10 + 5) = 20");
    }
}

#[test]
fn test_jit_execute_special_values() {
    // Test Nil
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("nil_test");
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_nil(), "Expected Nil");
    }

    // Test Unit
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("unit_test");
        builder.emit(Opcode::PushUnit);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_unit(), "Expected Unit");
    }

    // Test True and False
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("bool_test");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::And); // True and False = False
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool");
        assert!(!result.as_bool(), "True and False = False");
    }
}

#[test]
fn test_jit_execute_floor_div() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: 17 // 5 = 3 (floor division)
    let mut builder = ChunkBuilder::new("floor_div_test");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::FloorDiv);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 3, "Expected 17 // 5 = 3");
}

#[test]
fn test_jit_execute_neg_and_abs() {
    // Test Neg
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("neg_test");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Neg);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert_eq!(result.as_long(), -42, "Neg(42) = -42");
    }

    // Test Abs of negative
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("abs_neg_test");
        builder.emit_byte(Opcode::PushLongSmall, (-42i8) as u8);
        builder.emit(Opcode::Abs);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert_eq!(result.as_long(), 42, "Abs(-42) = 42");
    }

    // Test Abs of positive (unchanged)
    {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let mut builder = ChunkBuilder::new("abs_pos_test");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Abs);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert_eq!(result.as_long(), 42, "Abs(42) = 42");
    }
}

#[test]
fn test_jit_execute_pow_chain() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: pow(pow(2, 3), 2) = pow(8, 2) = 64
    let mut builder = ChunkBuilder::new("pow_chain_test");
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit(Opcode::Pow); // 2^3 = 8
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Pow); // 8^2 = 64
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 64, "Expected pow(pow(2,3),2) = 64");
}

#[test]
fn test_jit_execute_deep_expression() {
    use crate::backend::bytecode::compile;
    use crate::backend::MettaValue;

    // Build a deeply nested expression: ((((1 + 2) + 3) + 4) + 5) = 15
    fn build_nested_add(depth: usize) -> MettaValue {
        let mut expr = MettaValue::Long(1);
        for i in 2..=depth {
            expr = MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                expr,
                MettaValue::Long(i as i64),
            ]);
        }
        expr
    }

    let expr = build_nested_add(5); // 1 + 2 + 3 + 4 + 5 = 15
    let chunk = compile("deep", &expr).expect("Compilation failed");

    if !JitCompiler::can_compile_stage1(&chunk) {
        return;
    }

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");
    let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 15, "Expected 1+2+3+4+5 = 15");
}

#[test]
fn test_jit_execute_large_constant_arithmetic() {
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode with large constants: 1_000_000 * 1000 = 1_000_000_000
    let mut builder = ChunkBuilder::new("large_const_arith");
    let idx1 = builder.add_constant(MettaValue::Long(1_000_000));
    let idx2 = builder.add_constant(MettaValue::Long(1000));
    builder.emit_u16(Opcode::PushConstant, idx1);
    builder.emit_u16(Opcode::PushConstant, idx2);
    builder.emit(Opcode::Mul);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 1_000_000_000, "Expected 1M * 1K = 1B");
}

// =========================================================================
// JIT Bailout Tests for Non-Determinism Opcodes (Phase 4)
// =========================================================================

#[test]
fn test_can_compile_fail() {
    // Phase 9: Fail is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_fail");
    builder.emit(Opcode::Fail);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Fail is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Fail chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_cut() {
    // Phase 9: Cut is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_cut");
    builder.emit(Opcode::Cut);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Cut is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Cut chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_collect_with_bailout() {
    // Phase 9: Collect is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_collect");
    builder.emit(Opcode::Collect);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Collect is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Collect chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_collect_n() {
    // Phase 9: CollectN is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_collect_n");
    builder.emit_byte(Opcode::CollectN, 5);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // CollectN is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "CollectN chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_yield_with_bailout() {
    // Phase 9: Yield is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_yield");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Yield is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Yield chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_begin_nondet() {
    // Phase 9: BeginNondet is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_begin_nondet");
    builder.emit(Opcode::BeginNondet);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::EndNondet);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // BeginNondet is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "BeginNondet chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_end_nondet() {
    // Phase 9: EndNondet is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_end_nondet");
    builder.emit(Opcode::EndNondet);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // EndNondet is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "EndNondet chunks should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_can_compile_call_n() {
    // Phase 1.2: CallN is compilable (stack-based head + bailout)
    let mut builder = ChunkBuilder::new("test_call_n");
    builder.emit_u16(Opcode::PushConstant, 0); // Push head
    builder.emit_u16(Opcode::PushConstant, 1); // Push arg
    builder.emit_byte(Opcode::CallN, 1); // CallN with arity = 1
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "CallN opcode should be JIT compilable (Phase 1.2)"
    );
}

#[test]
fn test_can_compile_tail_call_n() {
    // Phase 1.2: TailCallN is compilable (stack-based head + bailout + TCO)
    let mut builder = ChunkBuilder::new("test_tail_call_n");
    builder.emit_u16(Opcode::PushConstant, 0); // Push head
    builder.emit_u16(Opcode::PushConstant, 1); // Push arg1
    builder.emit_u16(Opcode::PushConstant, 2); // Push arg2
    builder.emit_byte(Opcode::TailCallN, 2); // TailCallN with arity = 2
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "TailCallN opcode should be JIT compilable (Phase 1.2)"
    );
}

#[test]
fn test_can_compile_call_n_zero_arity() {
    // Phase 1.2: CallN with zero arity (just head, no args)
    let mut builder = ChunkBuilder::new("test_call_n_zero_arity");
    builder.emit_u16(Opcode::PushConstant, 0); // Push head
    builder.emit_byte(Opcode::CallN, 0); // CallN with arity = 0
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "CallN with arity 0 should be JIT compilable (Phase 1.2)"
    );
}

#[test]
fn test_can_compile_fork_in_middle_with_bailout() {
    // Phase 9: Fork anywhere in chunk is detected as nondeterminism
    let mut builder = ChunkBuilder::new("test_fork_middle");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Add);
    builder.emit_byte(Opcode::Fork, 2); // Fork in the middle
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Fork anywhere triggers nondeterminism detection - routes to bytecode tier
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Fork in chunk should not be JIT compilable (Phase 9 optimization)"
    );
}

#[test]
fn test_jit_bailout_reason_nondeterminism_exists() {
    // Verify that JitBailoutReason::NonDeterminism exists and has correct value
    use crate::backend::bytecode::jit::JitBailoutReason;

    let reason = JitBailoutReason::NonDeterminism;
    assert_eq!(reason as u8, 8, "NonDeterminism should have discriminant 8");
}

#[test]
fn test_jit_bailout_reason_phase4_values() {
    // Verify Phase 4 bailout reasons have correct discriminant values
    use crate::backend::bytecode::jit::JitBailoutReason;

    assert_eq!(
        JitBailoutReason::Fork as u8,
        11,
        "Fork should have discriminant 11"
    );
    assert_eq!(
        JitBailoutReason::Yield as u8,
        12,
        "Yield should have discriminant 12"
    );
    assert_eq!(
        JitBailoutReason::Collect as u8,
        13,
        "Collect should have discriminant 13"
    );
}

// =========================================================================
// Phase 4: Fork/Yield/Collect Execution Tests (Native Semantics)
// =========================================================================

#[test]
fn test_jit_fork_creates_choice_points_native() {
    // Phase 9: Fork chunks are now detected statically and routed to bytecode tier
    // This test verifies that Fork chunks are properly rejected by can_compile_stage1()
    use crate::backend::MettaValue;

    // Create chunk with Fork opcode
    // Format: Fork count:u16 idx0:u16 idx1:u16
    let mut builder = ChunkBuilder::new("test_fork_native");
    let idx0 = builder.add_constant(MettaValue::Long(1));
    let idx1 = builder.add_constant(MettaValue::Long(2));
    builder.emit_u16(Opcode::Fork, 2); // Fork with 2 alternatives
    builder.emit_raw(&idx0.to_be_bytes()); // index for alternative 0
    builder.emit_raw(&idx1.to_be_bytes()); // index for alternative 1
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Verify nondeterminism is detected
    assert!(
        chunk.has_nondeterminism(),
        "Fork should be detected as nondeterminism"
    );

    // Verify JIT compilation is rejected
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Fork chunks should not be JIT compilable (Phase 9: static nondeterminism routing)"
    );
}

#[test]
fn test_jit_yield_signals_bailout() {
    // Stage 2 JIT: Yield now returns JIT_SIGNAL_YIELD to dispatcher instead of bailout
    // Note: This test verifies that Yield stores result and returns signal
    use crate::backend::bytecode::jit::runtime::jit_runtime_yield_native;
    use crate::backend::bytecode::jit::{JitChoicePoint, JitContext, JitValue, JIT_SIGNAL_YIELD};

    // Test the runtime function directly instead of JIT code generation
    // (JIT code gen for Yield returns immediately, which breaks block filling)
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
    let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];
    let mut ctx = unsafe {
        JitContext::with_nondet(
            stack.as_mut_ptr(),
            stack.len(),
            std::ptr::null(),
            0,
            choice_points.as_mut_ptr(),
            choice_points.len(),
            results.as_mut_ptr(),
            results.len(),
        )
    };

    // Yield value 42
    let value = JitValue::from_long(42).0; // Get raw u64
    let signal = unsafe { jit_runtime_yield_native(&mut ctx, value, 0) };

    // Stage 2: Yield stores result and returns JIT_SIGNAL_YIELD
    assert_eq!(
        signal, JIT_SIGNAL_YIELD,
        "Yield should return JIT_SIGNAL_YIELD"
    );
    assert_eq!(ctx.results_count, 1, "Yield should have stored one result");

    // Verify the stored result
    let stored_result = unsafe { *ctx.results };
    assert_eq!(stored_result.as_long(), 42, "Yield should have stored 42");
}

#[test]
fn test_jit_collect_signals_bailout() {
    // Stage 2 JIT: Collect now uses native function and pushes result to stack
    // Note: The return value is the NaN-boxed SExpr result, not a signal
    use crate::backend::bytecode::jit::runtime::jit_runtime_collect_native;
    use crate::backend::bytecode::jit::{JitChoicePoint, JitContext, JitValue};

    // Test the runtime function directly
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
    let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];
    let mut ctx = unsafe {
        JitContext::with_nondet(
            stack.as_mut_ptr(),
            stack.len(),
            std::ptr::null(),
            0,
            choice_points.as_mut_ptr(),
            choice_points.len(),
            results.as_mut_ptr(),
            results.len(),
        )
    };

    // Store some results to collect
    unsafe {
        *ctx.results.add(0) = JitValue::from_long(1);
        *ctx.results.add(1) = JitValue::from_long(2);
        *ctx.results.add(2) = JitValue::from_long(3);
    }
    ctx.results_count = 3;

    // Collect the results
    let result = unsafe { jit_runtime_collect_native(&mut ctx) };

    // Stage 2: Collect returns NaN-boxed SExpr with collected results
    // The result should be a heap pointer (TAG_HEAP)
    let jv = JitValue::from_raw(result);
    assert!(jv.is_heap(), "Collect should return a heap pointer (SExpr)");

    // Verify the collected SExpr (should be (1 2 3))
    let metta = unsafe { jv.to_metta() };
    if let crate::backend::models::MettaValue::SExpr(items) = metta {
        assert_eq!(items.len(), 3, "Collected SExpr should have 3 items");
    } else {
        panic!("Collect should return SExpr");
    }
}

// =========================================================================
// Stage 3: Jump Instructions Tests
// =========================================================================

#[test]
fn test_can_compile_jump() {
    // Test that Jump opcode is now compilable (Stage 3)
    let mut builder = ChunkBuilder::new("test_jump");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    // Jump forward past the next instruction
    builder.emit_u16(Opcode::Jump, 4); // Jump over the next instruction
    builder.emit_byte(Opcode::PushLongSmall, 0); // Skipped
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Jump should be Stage 3 compilable"
    );
}

#[test]
fn test_can_compile_jump_if_false() {
    // Test that JumpIfFalse opcode is now compilable (Stage 3)
    let mut builder = ChunkBuilder::new("test_jump_if_false");
    builder.emit(Opcode::PushTrue);
    builder.emit_u16(Opcode::JumpIfFalse, 5); // Skip if false
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "JumpIfFalse should be Stage 3 compilable"
    );
}

#[test]
fn test_can_compile_jump_if_true() {
    // Test that JumpIfTrue opcode is now compilable (Stage 3)
    let mut builder = ChunkBuilder::new("test_jump_if_true");
    builder.emit(Opcode::PushFalse);
    builder.emit_u16(Opcode::JumpIfTrue, 5); // Skip if true
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "JumpIfTrue should be Stage 3 compilable"
    );
}

#[test]
fn test_can_compile_jump_short() {
    // Test that JumpShort opcode is now compilable (Stage 3)
    let mut builder = ChunkBuilder::new("test_jump_short");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::JumpShort, 3); // Short jump
    builder.emit_byte(Opcode::PushLongSmall, 0); // Skipped
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "JumpShort should be Stage 3 compilable"
    );
}

#[test]
fn test_jit_execute_conditional_jump_true_path() {
    // Test conditional jump: if True then 42 else 0
    // When condition is true, JumpIfFalse should NOT jump (fallthrough to then branch)
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: if True then 42 else 0
    // Layout (instruction sizes):
    // 0: PushTrue (1 byte)
    // 1: JumpIfFalse +5 (3 bytes: 1 opcode + 2 i16) -> next_ip=4, target=4+5=9 (else branch)
    // 4: PushLongSmall 42 (2 bytes: then branch)
    // 6: Jump +2 (3 bytes: 1 opcode + 2 i16) -> next_ip=9, target=9+2=11 (Return, skip else)
    // 9: PushLongSmall 0 (2 bytes: else branch)
    // 11: Return (1 byte)

    let mut builder = ChunkBuilder::new("e2e_cond_true");
    builder.emit(Opcode::PushTrue); // offset 0
    builder.emit_u16(Opcode::JumpIfFalse, 5); // offset 1, jumps to 9 if false
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 4, then branch
    builder.emit_u16(Opcode::Jump, 2); // offset 6, skip else -> target 11
    builder.emit_byte(Opcode::PushLongSmall, 0); // offset 9, else branch
    builder.emit(Opcode::Return); // offset 11
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 42, "Expected true branch result 42");
}

#[test]
fn test_jit_execute_conditional_jump_false_path() {
    // Test conditional jump: if False then 42 else 99
    // When condition is false, JumpIfFalse should jump to else branch
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Layout (same as true path but with PushFalse):
    // 0: PushFalse (1 byte)
    // 1: JumpIfFalse +5 (3 bytes) -> next_ip=4, target=4+5=9 (else branch)
    // 4: PushLongSmall 42 (2 bytes: then branch, skipped)
    // 6: Jump +2 (3 bytes) -> next_ip=9, target=9+2=11 (Return, skip else)
    // 9: PushLongSmall 99 (2 bytes: else branch)
    // 11: Return (1 byte)

    let mut builder = ChunkBuilder::new("e2e_cond_false");
    builder.emit(Opcode::PushFalse); // offset 0
    builder.emit_u16(Opcode::JumpIfFalse, 5); // offset 1, jumps to 9 if false
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 4, then branch (skipped)
    builder.emit_u16(Opcode::Jump, 2); // offset 6, skip else (skipped)
    builder.emit_byte(Opcode::PushLongSmall, 99); // offset 9, else branch
    builder.emit(Opcode::Return); // offset 11
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 99, "Expected false branch result 99");
}

#[test]
fn test_jit_execute_comparison_with_jump() {
    // Test: if 10 < 20 then 1 else 0
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Layout:
    // 0: PushLongSmall 10 (2 bytes)
    // 2: PushLongSmall 20 (2 bytes)
    // 4: Lt (1 byte) -> stack: [true]
    // 5: JumpIfFalse +5 (3 bytes) -> next_ip=8, target=8+5=13 (else branch)
    // 8: PushLongSmall 1 (2 bytes: then branch)
    // 10: Jump +2 (3 bytes) -> next_ip=13, target=13+2=15 (Return, skip else)
    // 13: PushLongSmall 0 (2 bytes: else branch)
    // 15: Return (1 byte)

    let mut builder = ChunkBuilder::new("e2e_cmp_jump");
    builder.emit_byte(Opcode::PushLongSmall, 10); // offset 0
    builder.emit_byte(Opcode::PushLongSmall, 20); // offset 2
    builder.emit(Opcode::Lt); // offset 4, stack: [true]
    builder.emit_u16(Opcode::JumpIfFalse, 5); // offset 5, jump to 13 if false
    builder.emit_byte(Opcode::PushLongSmall, 1); // offset 8, then branch
    builder.emit_u16(Opcode::Jump, 2); // offset 10, skip else -> target 15
    builder.emit_byte(Opcode::PushLongSmall, 0); // offset 13, else branch
    builder.emit(Opcode::Return); // offset 15
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 1, "Expected 10 < 20 = true, result 1");
}

#[test]
fn test_jit_execute_jump_if_true() {
    // Test JumpIfTrue: if True then jump to return, keeping 99 on stack
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Layout:
    // 0: PushLongSmall 99 (2 bytes) - push result first
    // 2: PushTrue (1 byte)
    // 3: JumpIfTrue +3 (3 bytes) -> next_ip=6, target=6+3=9 (Return)
    // 6: Pop (1 byte) - pop 99 (not executed if jump taken)
    // 7: PushLongSmall 0 (2 bytes) - push 0 (not executed if jump taken)
    // 9: Return (1 byte)
    //
    // When true: jump to 9, stack=[99], return 99
    // When false: pop 99, push 0, stack=[0], return 0

    let mut builder = ChunkBuilder::new("e2e_jump_if_true");
    builder.emit_byte(Opcode::PushLongSmall, 99); // offset 0, push result first
    builder.emit(Opcode::PushTrue); // offset 2
    builder.emit_u16(Opcode::JumpIfTrue, 3); // offset 3, jumps to 9 if true
    builder.emit(Opcode::Pop); // offset 6, pop 99 (not executed)
    builder.emit_byte(Opcode::PushLongSmall, 0); // offset 7, push 0 (not executed)
    builder.emit(Opcode::Return); // offset 9
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 99, "Expected jump taken, return 99");
}

// =========================================================================
// Stage 4: Local Variable Tests
// =========================================================================

#[test]
fn test_can_compile_stage4_local_variables() {
    let mut builder = ChunkBuilder::new("test_locals");
    builder.set_local_count(2);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::StoreLocal, 0); // Store 42 in local 0
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::StoreLocal, 1); // Store 10 in local 1
    builder.emit_byte(Opcode::LoadLocal, 0); // Load local 0 (42)
    builder.emit_byte(Opcode::LoadLocal, 1); // Load local 1 (10)
    builder.emit(Opcode::Add); // 42 + 10 = 52
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));
}

#[test]
fn test_can_compile_halt_opcode() {
    // Phase 1.10: Halt is now compilable (returns HALT signal)
    let mut builder = ChunkBuilder::new("test_halt");
    builder.emit_byte(Opcode::LoadLocal, 0);
    builder.emit(Opcode::Halt); // Halt is now compilable
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Halt opcode should be JIT compilable"
    );
}

// =====================================================================
// Phase G: Advanced Nondeterminism Tests
// =====================================================================

#[test]
fn test_can_compile_phase_g_cut() {
    // Phase 9: Cut is detected as nondeterminism and routed to bytecode tier
    let mut builder = ChunkBuilder::new("test_phase_g_cut");
    builder.emit(Opcode::Cut);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Cut is NOT compilable - static nondeterminism detection routes to bytecode
    assert!(
        !JitCompiler::can_compile_stage1(&chunk),
        "Phase 9: Cut chunks should not be JIT compilable (static nondeterminism detection)"
    );
}

// =====================================================================
// Phase H: MORK Bridge Tests
// =====================================================================

#[test]
fn test_can_compile_phase_h_mork_lookup() {
    // Phase H: MorkLookup opcode is compilable
    let mut builder = ChunkBuilder::new("test_phase_h_mork_lookup");
    builder.emit(Opcode::PushNil); // path placeholder
    builder.emit(Opcode::MorkLookup);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Phase H: MorkLookup should be JIT compilable"
    );
}

#[test]
fn test_can_compile_phase_h_mork_match() {
    // Phase H: MorkMatch opcode is compilable
    let mut builder = ChunkBuilder::new("test_phase_h_mork_match");
    builder.emit(Opcode::PushNil); // path placeholder
    builder.emit(Opcode::PushNil); // pattern placeholder
    builder.emit(Opcode::MorkMatch);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Phase H: MorkMatch should be JIT compilable"
    );
}

#[test]
fn test_can_compile_phase_h_mork_insert() {
    // Phase H: MorkInsert opcode is compilable
    let mut builder = ChunkBuilder::new("test_phase_h_mork_insert");
    builder.emit(Opcode::PushNil); // path placeholder
    builder.emit(Opcode::PushNil); // value placeholder
    builder.emit(Opcode::MorkInsert);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Phase H: MorkInsert should be JIT compilable"
    );
}

#[test]
fn test_can_compile_phase_h_mork_delete() {
    // Phase H: MorkDelete opcode is compilable
    let mut builder = ChunkBuilder::new("test_phase_h_mork_delete");
    builder.emit(Opcode::PushNil); // path placeholder
    builder.emit(Opcode::MorkDelete);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Phase H: MorkDelete should be JIT compilable"
    );
}

// =====================================================================
// Phase I: Debug/Meta Tests
// =====================================================================

#[test]
fn test_can_compile_phase_i_trace() {
    // Phase I: Trace opcode is compilable
    let mut builder = ChunkBuilder::new("test_phase_i_trace");
    builder.emit_byte(Opcode::PushLongSmall, 42); // value to trace
    builder.emit_u16(Opcode::Trace, 0); // msg_idx = 0
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Phase I: Trace should be JIT compilable"
    );
}

#[test]
fn test_can_compile_phase_i_breakpoint() {
    // Phase I: Breakpoint opcode is compilable
    let mut builder = ChunkBuilder::new("test_phase_i_breakpoint");
    builder.emit_u16(Opcode::Breakpoint, 1); // bp_id = 1
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Phase I: Breakpoint should be JIT compilable"
    );
}

#[test]
fn test_jit_execute_local_store_load() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Store 42 in local 0, then load and return it
    let mut builder = ChunkBuilder::new("e2e_locals_basic");
    builder.set_local_count(1);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::StoreLocal, 0);
    builder.emit_byte(Opcode::LoadLocal, 0);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 42, "Expected local variable value 42");
}

#[test]
fn test_jit_execute_local_arithmetic() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Store values in two locals, load them, and perform arithmetic
    // local 0 = 100, local 1 = 58
    // result = local0 - local1 = 42
    let mut builder = ChunkBuilder::new("e2e_locals_arith");
    builder.set_local_count(2);
    builder.emit_byte(Opcode::PushLongSmall, 100);
    builder.emit_byte(Opcode::StoreLocal, 0);
    builder.emit_byte(Opcode::PushLongSmall, 58);
    builder.emit_byte(Opcode::StoreLocal, 1);
    builder.emit_byte(Opcode::LoadLocal, 0);
    builder.emit_byte(Opcode::LoadLocal, 1);
    builder.emit(Opcode::Sub);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 42, "Expected 100 - 58 = 42");
}

#[test]
fn test_jit_execute_local_overwrite() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Test that storing to a local twice overwrites the value
    // local 0 = 10, then local 0 = 99
    let mut builder = ChunkBuilder::new("e2e_locals_overwrite");
    builder.set_local_count(1);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::StoreLocal, 0);
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit_byte(Opcode::StoreLocal, 0); // Overwrite
    builder.emit_byte(Opcode::LoadLocal, 0);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 99, "Expected overwritten value 99");
}

#[test]
fn test_jit_execute_local_with_control_flow() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Store a value, then use conditional jump
    // if true, load the local and return; else load 0 and return
    // local 0 = 77, condition = true, should return 77
    //
    // Layout:
    // 0-1: PushLongSmall 77 (2 bytes)
    // 2-3: StoreLocal 0 (2 bytes)
    // 4: PushTrue (1 byte)
    // 5-7: JumpIfFalse +5 (3 bytes) -> next_ip=8, target=8+5=13
    // 8-9: LoadLocal 0 (2 bytes) - true path
    // 10: Return (1 byte) - true path exits
    // 11-12: PushLongSmall 0 (2 bytes) - false path
    // 13: Return (1 byte)
    //
    // When true: load local (77), return
    // When false: jump to 13, but we actually want to push 0 first

    let mut builder = ChunkBuilder::new("e2e_locals_control");
    builder.set_local_count(1);
    builder.emit_byte(Opcode::PushLongSmall, 77); // offset 0-1
    builder.emit_byte(Opcode::StoreLocal, 0); // offset 2-3
    builder.emit(Opcode::PushTrue); // offset 4
    builder.emit_u16(Opcode::JumpIfFalse, 3); // offset 5-7, jump to offset 11 (8+3)
    builder.emit_byte(Opcode::LoadLocal, 0); // offset 8-9, true: load 77
    builder.emit(Opcode::Return); // offset 10, true: return 77
    builder.emit_byte(Opcode::PushLongSmall, 0); // offset 11-12, false: push 0
    builder.emit(Opcode::Return); // offset 13, false: return 0
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        77,
        "Expected local value from true branch"
    );
}

#[test]
fn test_jit_execute_many_locals() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Test with multiple locals (up to 8)
    let mut builder = ChunkBuilder::new("e2e_many_locals");
    builder.set_local_count(8);

    // Store values 0-7 in locals 0-7
    for i in 0..8u8 {
        builder.emit_byte(Opcode::PushLongSmall, i);
        builder.emit_byte(Opcode::StoreLocal, i);
    }

    // Sum all locals: 0 + 1 + 2 + 3 + 4 + 5 + 6 + 7 = 28
    builder.emit_byte(Opcode::LoadLocal, 0);
    for i in 1..8u8 {
        builder.emit_byte(Opcode::LoadLocal, i);
        builder.emit(Opcode::Add);
    }
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 28, "Expected sum 0+1+2+3+4+5+6+7 = 28");
}

// =========================================================================
// Stage 5: JumpIfNil and JumpIfError Tests
// =========================================================================

#[test]
fn test_jit_can_compile_jump_if_nil() {
    // JumpIfNil should be compilable in Stage 5
    let mut builder = ChunkBuilder::new("can_compile_jump_if_nil");
    builder.emit(Opcode::PushNil);
    builder.emit_u16(Opcode::JumpIfNil, 2); // Jump forward 2 bytes
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));
}

#[test]
fn test_jit_can_compile_jump_if_error() {
    // JumpIfError should be compilable in Stage 5
    let mut builder = ChunkBuilder::new("can_compile_jump_if_error");
    builder.emit(Opcode::PushNil); // Use nil as placeholder (no PushError opcode)
    builder.emit_u16(Opcode::JumpIfError, 2); // Jump forward 2 bytes
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));
}

#[test]
fn test_jit_execute_jump_if_nil_takes_jump() {
    // When value is nil, JumpIfNil should take the jump
    // Layout:
    // 0: PushNil (1 byte)
    // 1-3: JumpIfNil +5 (3 bytes) -> next_ip=4, target=4+5=9
    // 4-5: PushLongSmall 42 (2 bytes) - fallthrough path (skipped)
    // 6-8: Jump +2 (3 bytes) -> skip else
    // 9-10: PushLongSmall 99 (2 bytes) - jump target
    // 11: Return (1 byte)
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_jump_if_nil_takes");
    builder.emit(Opcode::PushNil); // offset 0
    builder.emit_u16(Opcode::JumpIfNil, 5); // offset 1-3, jump to 9 if nil
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 4-5, not nil path (skipped)
    builder.emit_u16(Opcode::Jump, 2); // offset 6-8, skip else
    builder.emit_byte(Opcode::PushLongSmall, 99); // offset 9-10, nil path
    builder.emit(Opcode::Return); // offset 11
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 99, "Expected nil branch result 99");
}

#[test]
fn test_jit_execute_jump_if_nil_fallthrough() {
    // When value is not nil, JumpIfNil should NOT jump (fallthrough)
    // Same layout but with non-nil value
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_jump_if_nil_fallthrough");
    builder.emit_byte(Opcode::PushLongSmall, 1); // offset 0-1, not nil
    builder.emit_u16(Opcode::JumpIfNil, 5); // offset 2-4, jump to 10 if nil
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 5-6, not nil path
    builder.emit_u16(Opcode::Jump, 2); // offset 7-9, skip else
    builder.emit_byte(Opcode::PushLongSmall, 99); // offset 10-11, nil path (skipped)
    builder.emit(Opcode::Return); // offset 12
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 42, "Expected non-nil branch result 42");
}

#[test]
fn test_jit_execute_jump_if_nil_with_bool_false() {
    // False is NOT nil, so should fallthrough
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_jump_if_nil_bool_false");
    builder.emit(Opcode::PushFalse); // offset 0, False (not nil)
    builder.emit_u16(Opcode::JumpIfNil, 5); // offset 1-3, jump to 9 if nil
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 4-5, not nil path
    builder.emit_u16(Opcode::Jump, 2); // offset 6-8, skip else
    builder.emit_byte(Opcode::PushLongSmall, 99); // offset 9-10, nil path (skipped)
    builder.emit(Opcode::Return); // offset 11
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        42,
        "False is not nil, expected fallthrough to 42"
    );
}

#[test]
fn test_jit_execute_jump_if_error_no_error() {
    // When value is not an error, JumpIfError should NOT jump (fallthrough)
    // Note: JumpIfError PEEKS, doesn't pop
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Simple test: push non-error, JumpIfError peeks and continues (value stays on stack)
    let mut builder = ChunkBuilder::new("e2e_jump_if_error_no_error");
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 0-1, not an error
    builder.emit_u16(Opcode::JumpIfError, 1); // offset 2-4, peek: stack still has 42
    builder.emit(Opcode::Return); // offset 5
    builder.emit(Opcode::Return); // offset 6 (jump target, should not reach)
    let chunk = builder.build();

    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    // The value should still be on stack (peek doesn't pop)
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        42,
        "JumpIfError should peek, not pop; stack should have 42"
    );
}

#[test]
fn test_jit_execute_jump_if_nil_pops_value() {
    // JumpIfNil should POP the value being tested
    // Test: push 42, push nil, JumpIfNil takes jump (pops nil), return 42
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Layout:
    // 0-1: PushLongSmall 42 (2 bytes)
    // 2: PushNil (1 byte)
    // 3-5: JumpIfNil +2 (3 bytes) -> next_ip=6, target=8 (Jump over the not-nil-path)
    // 6-7: PushLongSmall 99 (2 bytes) - not reached since nil
    // 8: Return (1 byte) -> returns 42 (nil was popped, 42 stays on stack)
    let mut builder = ChunkBuilder::new("e2e_jump_if_nil_pops");
    builder.emit_byte(Opcode::PushLongSmall, 42); // offset 0-1
    builder.emit(Opcode::PushNil); // offset 2
    builder.emit_u16(Opcode::JumpIfNil, 2); // offset 3-5, pops nil, jumps to 8
    builder.emit_byte(Opcode::PushLongSmall, 99); // offset 6-7, not-nil path (skipped)
    builder.emit(Opcode::Return); // offset 8
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    // Stack should be: [42] after nil is popped by JumpIfNil
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        42,
        "JumpIfNil should pop; result should be 42"
    );
}

// =========================================================================
// Stage 6: Type Predicate Tests
// =========================================================================

#[test]
fn test_jit_can_compile_is_variable() {
    let mut builder = ChunkBuilder::new("test_is_variable");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "IsVariable should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_is_sexpr() {
    let mut builder = ChunkBuilder::new("test_is_sexpr");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "IsSExpr should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_is_symbol() {
    let mut builder = ChunkBuilder::new("test_is_symbol");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "IsSymbol should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_execute_is_variable_false_for_long() {
    // IsVariable(42) should return false
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_is_variable_long");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert!(!result.as_bool(), "IsVariable(Long) should return false");
}

#[test]
fn test_jit_execute_is_sexpr_false_for_long() {
    // IsSExpr(42) should return false
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_is_sexpr_long");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert!(!result.as_bool(), "IsSExpr(Long) should return false");
}

#[test]
fn test_jit_execute_is_symbol_false_for_long() {
    // IsSymbol(42) should return false
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_is_symbol_long");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert!(!result.as_bool(), "IsSymbol(Long) should return false");
}

#[test]
fn test_jit_execute_is_variable_false_for_bool() {
    // IsVariable(true) should return false
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_is_variable_bool");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert!(!result.as_bool(), "IsVariable(Bool) should return false");
}

#[test]
fn test_jit_execute_is_sexpr_false_for_nil() {
    // IsSExpr(nil) should return false
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_is_sexpr_nil");
    builder.emit(Opcode::PushNil);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert!(!result.as_bool(), "IsSExpr(Nil) should return false");
}

#[test]
fn test_jit_execute_is_symbol_false_for_unit() {
    // IsSymbol(unit) should return false
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_is_symbol_unit");
    builder.emit(Opcode::PushUnit);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert!(!result.as_bool(), "IsSymbol(Unit) should return false");
}

// =========================================================================
// Stage 7: Stack Operations and Negation Tests
// =========================================================================

#[test]
fn test_jit_can_compile_pop() {
    let mut builder = ChunkBuilder::new("test_pop");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Pop);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Pop should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_dup() {
    let mut builder = ChunkBuilder::new("test_dup");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Dup);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Dup should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_swap() {
    let mut builder = ChunkBuilder::new("test_swap");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Sub);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Swap should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_neg() {
    let mut builder = ChunkBuilder::new("test_neg");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Neg);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Neg should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_dup_n() {
    let mut builder = ChunkBuilder::new("test_dup_n");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::DupN, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "DupN should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_pop_n() {
    let mut builder = ChunkBuilder::new("test_pop_n");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::PopN, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PopN should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_execute_pop() {
    // Push 42, push 10, pop -> result should be 42
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_pop");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Pop);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 42, "Pop should discard 10, leaving 42");
}

#[test]
fn test_jit_execute_dup() {
    // Push 21, dup, add -> result should be 42
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_dup");
    builder.emit_byte(Opcode::PushLongSmall, 21);
    builder.emit(Opcode::Dup);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        42,
        "Dup should duplicate 21, then add: 21+21=42"
    );
}

#[test]
fn test_jit_execute_swap() {
    // Push 10, push 5, swap, sub -> 5 - 10 = -5
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_swap");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Sub);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    // Stack after push: [10, 5]
    // Stack after swap: [5, 10]
    // Sub pops b=10, a=5, computes a-b = 5-10 = -5
    assert_eq!(result.as_long(), -5, "Swap should swap, then sub: 5-10=-5");
}

#[test]
fn test_jit_execute_neg_positive() {
    // Neg(42) = -42
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_neg_positive");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Neg);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), -42, "Neg(42) should be -42");
}

#[test]
fn test_jit_execute_neg_negative() {
    // Neg(-10) = 10
    // We can't push negative with PushLongSmall, so use 10 - 20 = -10, then negate
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_neg_negative");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Sub); // 10 - 20 = -10
    builder.emit(Opcode::Neg); // Neg(-10) = 10
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 10, "Neg(-10) should be 10");
}

#[test]
fn test_jit_execute_dup_n() {
    // Push 1, push 2, DupN 2 -> [1, 2, 1, 2], add (2+1=3), add (3+2=5), add (5+1=6)
    // Actually let's do simpler: push 3, push 4, DupN 2 -> [3, 4, 3, 4]
    // Then add top two: [3, 4, 7], then return 7
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_dup_n");
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::PushLongSmall, 4);
    builder.emit_byte(Opcode::DupN, 2); // Stack: [3, 4, 3, 4]
    builder.emit(Opcode::Add); // Stack: [3, 4, 7]
    builder.emit(Opcode::Return); // Returns 7
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(
        result.as_long(),
        7,
        "DupN should duplicate top 2, then add: 3+4=7"
    );
}

#[test]
fn test_jit_execute_pop_n() {
    // Push 100, push 1, push 2, push 3, PopN 3 -> [100], return 100
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_pop_n");
    builder.emit_byte(Opcode::PushLongSmall, 100);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::PopN, 3); // Pop 3, 2, 1 -> [100]
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 100, "PopN 3 should leave 100");
}

#[test]
fn test_jit_execute_neg_zero() {
    // Neg(0) = 0
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_neg_zero");
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Neg);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 0, "Neg(0) should be 0");
}

// =========================================================================
// Stage 8: More Arithmetic and Stack Operations Tests
// =========================================================================

#[test]
fn test_jit_can_compile_abs() {
    let mut builder = ChunkBuilder::new("test_abs");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Abs);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Abs should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_mod() {
    let mut builder = ChunkBuilder::new("test_mod");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Mod should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_floor_div() {
    let mut builder = ChunkBuilder::new("test_floor_div");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::FloorDiv);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "FloorDiv should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_rot3() {
    let mut builder = ChunkBuilder::new("test_rot3");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit(Opcode::Rot3);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Rot3 should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_can_compile_over() {
    let mut builder = ChunkBuilder::new("test_over");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Over);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Over should be Stage 1 compilable"
    );
}

#[test]
fn test_jit_execute_abs_positive() {
    // Abs(42) = 42
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_abs_positive");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Abs);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 42, "Abs(42) should be 42");
}

#[test]
fn test_jit_execute_abs_negative() {
    // Abs(-10) = 10
    // Create -10 via subtraction: 5 - 15 = -10
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_abs_negative");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 15);
    builder.emit(Opcode::Sub); // 5 - 15 = -10
    builder.emit(Opcode::Abs); // Abs(-10) = 10
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 10, "Abs(-10) should be 10");
}

#[test]
fn test_jit_execute_mod() {
    // 17 % 5 = 2
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_mod");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 2, "17 % 5 should be 2");
}

#[test]
fn test_jit_execute_floor_div_stage8() {
    // 17 / 5 = 3 (floor)
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_floor_div_s8");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::FloorDiv);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 3, "17 / 5 (floor) should be 3");
}

#[test]
fn test_jit_execute_rot3() {
    // Stack: [1, 2, 3] -> Rot3 -> [3, 1, 2]
    // Return top: 2
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_rot3");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit(Opcode::Rot3); // [3, 1, 2]
    builder.emit(Opcode::Return); // Returns 2
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 2, "Rot3([1,2,3]) top should be 2");
}

#[test]
fn test_jit_execute_rot3_verify_order() {
    // Verify the full rotation by adding: [a,b,c] -> [c,a,b]
    // Push 10, 20, 30 -> Rot3 -> [30, 10, 20]
    // Then: 20 - 10 = 10 (top - second), pop the 10, return 30
    // Actually let's do: sub twice to verify order
    // [30, 10, 20] -> sub(10, 20) = 10-20 = -10 -> [30, -10] -> return -10
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_rot3_verify");
    builder.emit_byte(Opcode::PushLongSmall, 10); // a
    builder.emit_byte(Opcode::PushLongSmall, 20); // b
    builder.emit_byte(Opcode::PushLongSmall, 30); // c
    builder.emit(Opcode::Rot3); // [c=30, a=10, b=20]
    builder.emit(Opcode::Sub); // a - b = 10 - 20 = -10 -> [30, -10]
    builder.emit(Opcode::Return); // Returns -10
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), -10, "Rot3 should make a-b = 10-20 = -10");
}

#[test]
fn test_jit_execute_over() {
    // Stack: [1, 2] -> Over -> [1, 2, 1]
    // Add: 1 + 2 = 3 -> [1, 3] -> Return 3
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_over");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Over); // [1, 2, 1]
    builder.emit(Opcode::Add); // [1, 3]
    builder.emit(Opcode::Return); // Returns 3
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 3, "Over should copy 1, then add: 2+1=3");
}

#[test]
fn test_jit_execute_abs_zero() {
    // Abs(0) = 0
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_abs_zero");
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Abs);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 0, "Abs(0) should be 0");
}

#[test]
fn test_jit_execute_mod_exact() {
    // 15 % 5 = 0
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_mod_exact");
    builder.emit_byte(Opcode::PushLongSmall, 15);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 0, "15 % 5 should be 0");
}

// =========================================================================
// Stage 9: Boolean Logic Tests
// =========================================================================

#[test]
fn test_jit_can_compile_boolean_ops_stage9() {
    // And
    let mut builder = ChunkBuilder::new("bool_and");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);
    assert!(
        JitCompiler::can_compile_stage1(&builder.build()),
        "And should be compilable"
    );

    // Or
    let mut builder = ChunkBuilder::new("bool_or");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Or);
    builder.emit(Opcode::Return);
    assert!(
        JitCompiler::can_compile_stage1(&builder.build()),
        "Or should be compilable"
    );

    // Not
    let mut builder = ChunkBuilder::new("bool_not");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Not);
    builder.emit(Opcode::Return);
    assert!(
        JitCompiler::can_compile_stage1(&builder.build()),
        "Not should be compilable"
    );

    // Xor
    let mut builder = ChunkBuilder::new("bool_xor");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Xor);
    builder.emit(Opcode::Return);
    assert!(
        JitCompiler::can_compile_stage1(&builder.build()),
        "Xor should be compilable"
    );
}

#[test]
fn test_jit_execute_and() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // True AND True = True
    let mut builder = ChunkBuilder::new("and_tt");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "True AND True should be True");

    // True AND False = False
    let mut builder = ChunkBuilder::new("and_tf");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "True AND False should be False");

    // False AND False = False
    let mut builder = ChunkBuilder::new("and_ff");
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "False AND False should be False");
}

#[test]
fn test_jit_execute_or() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // True OR False = True
    let mut builder = ChunkBuilder::new("or_tf");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Or);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "True OR False should be True");

    // False OR False = False
    let mut builder = ChunkBuilder::new("or_ff");
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Or);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "False OR False should be False");
}

#[test]
fn test_jit_execute_not() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // NOT True = False
    let mut builder = ChunkBuilder::new("not_t");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Not);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "NOT True should be False");

    // NOT False = True
    let mut builder = ChunkBuilder::new("not_f");
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Not);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "NOT False should be True");
}

#[test]
fn test_jit_execute_xor() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // True XOR True = False
    let mut builder = ChunkBuilder::new("xor_tt");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Xor);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "True XOR True should be False");

    // True XOR False = True
    let mut builder = ChunkBuilder::new("xor_tf");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Xor);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "True XOR False should be True");

    // False XOR False = False
    let mut builder = ChunkBuilder::new("xor_ff");
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Xor);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "False XOR False should be False");
}

#[test]
fn test_jit_execute_boolean_chain() {
    // Complex: (True AND False) OR (NOT False) = False OR True = True
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("bool_chain");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::And); // False
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Not); // True
    builder.emit(Opcode::Or); // False OR True = True
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(
        result.as_bool(),
        true,
        "(T AND F) OR (NOT F) should be True"
    );
}

// =========================================================================
// Stage 11: StructEq Tests
// =========================================================================

#[test]
fn test_jit_can_compile_struct_eq() {
    let mut builder = ChunkBuilder::new("struct_eq");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    assert!(
        JitCompiler::can_compile_stage1(&builder.build()),
        "StructEq should be compilable"
    );
}

#[test]
fn test_jit_execute_struct_eq_equal_longs() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // 42 == 42 structurally
    let mut builder = ChunkBuilder::new("struct_eq_longs");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "42 == 42 should be true");
}

#[test]
fn test_jit_execute_struct_eq_different_longs() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // 42 == 43 structurally
    let mut builder = ChunkBuilder::new("struct_eq_diff");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 43);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "42 == 43 should be false");
}

#[test]
fn test_jit_execute_struct_eq_bools() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // True == True
    let mut builder = ChunkBuilder::new("struct_eq_true");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "True == True should be true");

    // True == False
    let mut builder = ChunkBuilder::new("struct_eq_tf");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    let chunk = builder.build();
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());
    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), false, "True == False should be false");
}

#[test]
fn test_jit_execute_struct_eq_nil() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Nil == Nil
    let mut builder = ChunkBuilder::new("struct_eq_nil");
    builder.emit(Opcode::PushNil);
    builder.emit(Opcode::PushNil);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(result.as_bool(), true, "Nil == Nil should be true");
}

#[test]
fn test_jit_execute_struct_eq_different_types() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // 1 == True (different types)
    let mut builder = ChunkBuilder::new("struct_eq_diff_types");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_bool(), "Expected Bool result");
    assert_eq!(
        result.as_bool(),
        false,
        "Long(1) == Bool(True) should be false"
    );
}

// =========================================================================
// Stage 10: More Pow Tests (extend existing tests)
// =========================================================================

#[test]
fn test_jit_execute_pow_zero_exp() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // 5^0 = 1
    let mut builder = ChunkBuilder::new("pow_5_0");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 1, "5^0 should be 1");
}

#[test]
fn test_jit_execute_pow_one_exp() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // 7^1 = 7
    let mut builder = ChunkBuilder::new("pow_7_1");
    builder.emit_byte(Opcode::PushLongSmall, 7);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 7, "7^1 should be 7");
}

#[test]
fn test_jit_execute_pow_small() {
    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // 3^4 = 81
    let mut builder = ChunkBuilder::new("pow_3_4");
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::PushLongSmall, 4);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
    let result = exec_jit(code_ptr, chunk.constants());

    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 81, "3^4 should be 81");
}

// =========================================================================
// Stage 13: Value Creation Tests
// =========================================================================

#[test]
fn test_jit_can_compile_push_empty() {
    let mut builder = ChunkBuilder::new("push_empty");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PushEmpty should be Stage 13 compilable"
    );
}

#[test]
fn test_jit_can_compile_push_atom() {
    use crate::backend::MettaValue;

    let mut builder = ChunkBuilder::new("push_atom");
    let idx = builder.add_constant(MettaValue::Atom("foo".to_string()));
    builder.emit_u16(Opcode::PushAtom, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PushAtom should be Stage 13 compilable"
    );
}

#[test]
fn test_jit_can_compile_push_string() {
    use crate::backend::MettaValue;

    let mut builder = ChunkBuilder::new("push_string");
    let idx = builder.add_constant(MettaValue::String("hello".to_string()));
    builder.emit_u16(Opcode::PushString, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PushString should be Stage 13 compilable"
    );
}

#[test]
fn test_jit_can_compile_push_variable() {
    use crate::backend::MettaValue;

    let mut builder = ChunkBuilder::new("push_variable");
    let idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
    builder.emit_u16(Opcode::PushVariable, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PushVariable should be Stage 13 compilable"
    );
}

#[test]
fn test_jit_execute_push_empty() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_push_empty");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // PushEmpty returns a heap pointer (TAG_HEAP) to an empty S-expression
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_heap(),
        "Expected Heap result for empty S-expr, got: {:?}",
        result
    );
}

#[test]
fn test_jit_execute_push_atom() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_push_atom");
    let idx = builder.add_constant(MettaValue::Atom("foo".to_string()));
    builder.emit_u16(Opcode::PushAtom, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // PushAtom returns a heap pointer to the Atom
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_heap(),
        "Expected Heap result for atom, got: {:?}",
        result
    );
}

#[test]
fn test_jit_execute_push_string() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_push_string");
    let idx = builder.add_constant(MettaValue::String("hello world".to_string()));
    builder.emit_u16(Opcode::PushString, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // PushString returns a heap pointer to the String
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_heap(),
        "Expected Heap result for string, got: {:?}",
        result
    );
}

#[test]
fn test_jit_execute_push_variable() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("e2e_push_variable");
    let idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
    builder.emit_u16(Opcode::PushVariable, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // PushVariable returns a heap pointer to the Variable (which is an Atom starting with $)
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_heap(),
        "Expected Heap result for variable, got: {:?}",
        result
    );
}

// =========================================================================
// Stage 14: S-Expression Operations Tests
// =========================================================================

#[test]
fn test_jit_can_compile_get_head() {
    let mut builder = ChunkBuilder::new("get_head");
    builder.emit(Opcode::PushEmpty); // Dummy S-expr - real test needs heap value
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "GetHead should be Stage 14 compilable"
    );
}

#[test]
fn test_jit_can_compile_get_tail() {
    let mut builder = ChunkBuilder::new("get_tail");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "GetTail should be Stage 14 compilable"
    );
}

#[test]
fn test_jit_can_compile_get_arity() {
    let mut builder = ChunkBuilder::new("get_arity");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::GetArity);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "GetArity should be Stage 14 compilable"
    );
}

#[test]
fn test_jit_execute_get_arity_empty() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk that pushes an empty S-expr and gets its arity (should be 0)
    let mut builder = ChunkBuilder::new("e2e_get_arity_empty");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::GetArity);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetArity on empty S-expr should return 0
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_long(),
        "Expected Long result for arity, got: {:?}",
        result
    );
    assert_eq!(result.as_long(), 0, "Arity of empty S-expr should be 0");
}

#[test]
fn test_jit_execute_get_arity_nonempty() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk with a 3-element S-expr in constant pool
    let mut builder = ChunkBuilder::new("e2e_get_arity_nonempty");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::GetArity);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetArity on 3-element S-expr should return 3
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_long(),
        "Expected Long result for arity, got: {:?}",
        result
    );
    assert_eq!(result.as_long(), 3, "Arity of (1 2 3) should be 3");
}

#[test]
fn test_jit_execute_get_head() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk with a 3-element S-expr where head is Long(42)
    let mut builder = ChunkBuilder::new("e2e_get_head");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(42),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetHead on (42 2 3) should return 42
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_long(),
        "Expected Long result for head, got: {:?}",
        result
    );
    assert_eq!(result.as_long(), 42, "Head of (42 2 3) should be 42");
}

#[test]
fn test_jit_execute_get_tail() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk with a 3-element S-expr, then get tail
    let mut builder = ChunkBuilder::new("e2e_get_tail");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::GetArity); // Get arity of tail to verify it's (2 3) = length 2
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetTail on (1 2 3) gives (2 3) which has arity 2
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_long(),
        "Expected Long result for arity of tail, got: {:?}",
        result
    );
    assert_eq!(result.as_long(), 2, "Arity of tail of (1 2 3) should be 2");
}

#[test]
fn test_jit_execute_get_tail_get_head() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: get head of tail of (1 2 3) = 2
    let mut builder = ChunkBuilder::new("e2e_get_tail_get_head");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::GetTail); // (2 3)
    builder.emit(Opcode::GetHead); // 2
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // Head of tail of (1 2 3) should be 2
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(result.as_long(), 2, "Head of tail of (1 2 3) should be 2");
}

// =========================================================================
// Stage 14b: GetElement Tests
// =========================================================================

#[test]
fn test_jit_can_compile_get_element() {
    use crate::backend::MettaValue;

    let mut builder = ChunkBuilder::new("get_element");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(10),
        MettaValue::Long(20),
        MettaValue::Long(30),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1
    builder.emit(Opcode::Return);

    let chunk = builder.build();
    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "GetElement should be compilable"
    );
}

#[test]
fn test_jit_execute_get_element_first() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: get element at index 0 of (10 20 30) = 10
    let mut builder = ChunkBuilder::new("e2e_get_element_first");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(10),
        MettaValue::Long(20),
        MettaValue::Long(30),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit_byte(Opcode::GetElement, 0); // Get element at index 0
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // Element at index 0 of (10 20 30) should be 10
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(
        result.as_long(),
        10,
        "Element at index 0 of (10 20 30) should be 10"
    );
}

#[test]
fn test_jit_execute_get_element_middle() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: get element at index 1 of (10 20 30) = 20
    let mut builder = ChunkBuilder::new("e2e_get_element_middle");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(10),
        MettaValue::Long(20),
        MettaValue::Long(30),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // Element at index 1 of (10 20 30) should be 20
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(
        result.as_long(),
        20,
        "Element at index 1 of (10 20 30) should be 20"
    );
}

#[test]
fn test_jit_execute_get_element_last() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: get element at index 2 of (10 20 30) = 30
    let mut builder = ChunkBuilder::new("e2e_get_element_last");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(10),
        MettaValue::Long(20),
        MettaValue::Long(30),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit_byte(Opcode::GetElement, 2); // Get element at index 2
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // Element at index 2 of (10 20 30) should be 30
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(
        result.as_long(),
        30,
        "Element at index 2 of (10 20 30) should be 30"
    );
}

#[test]
fn test_jit_execute_get_element_combined() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: (get-element 1) + (get-element 2) of (10 20 30) = 20 + 30 = 50
    let mut builder = ChunkBuilder::new("e2e_get_element_combined");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(10),
        MettaValue::Long(20),
        MettaValue::Long(30),
    ]);
    let idx = builder.add_constant(sexpr.clone());
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1 -> 20
    let idx2 = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx2);
    builder.emit_byte(Opcode::GetElement, 2); // Get element at index 2 -> 30
    builder.emit(Opcode::Add); // 20 + 30 = 50
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // Element[1] + Element[2] of (10 20 30) should be 20 + 30 = 50
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(
        result.as_long(),
        50,
        "Element[1] + Element[2] of (10 20 30) should be 50"
    );
}

// =========================================================================
// Phase 1: Type Operations Tests (GetType, CheckType, IsType)
// =========================================================================

#[test]
fn test_jit_execute_get_type_long() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: GetType(42) should return "Number" atom
    let mut builder = ChunkBuilder::new("e2e_get_type_long");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetType(42) should return "Number" atom
    let result = JitValue::from_raw(result_bits as u64);
    let metta_val = unsafe { result.to_metta() };
    match metta_val {
        MettaValue::Atom(s) => assert_eq!(s, "Number", "GetType(Long) should return 'Number'"),
        other => panic!("Expected Atom('Number'), got: {:?}", other),
    }
}

#[test]
fn test_jit_execute_get_type_bool() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: GetType(True) should return "Bool" atom
    let mut builder = ChunkBuilder::new("e2e_get_type_bool");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::GetType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetType(True) should return "Bool" atom
    let result = JitValue::from_raw(result_bits as u64);
    let metta_val = unsafe { result.to_metta() };
    match metta_val {
        MettaValue::Atom(s) => assert_eq!(s, "Bool", "GetType(Bool) should return 'Bool'"),
        other => panic!("Expected Atom('Bool'), got: {:?}", other),
    }
}

#[test]
fn test_jit_execute_get_type_sexpr() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: GetType((1 2 3)) should return "Expression" atom
    let mut builder = ChunkBuilder::new("e2e_get_type_sexpr");
    let sexpr = MettaValue::SExpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let idx = builder.add_constant(sexpr);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::GetType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // GetType((1 2 3)) should return "Expression" atom
    let result = JitValue::from_raw(result_bits as u64);
    let metta_val = unsafe { result.to_metta() };
    match metta_val {
        MettaValue::Atom(s) => {
            assert_eq!(s, "Expression", "GetType(SExpr) should return 'Expression'")
        }
        other => panic!("Expected Atom('Expression'), got: {:?}", other),
    }
}

#[test]
fn test_jit_execute_check_type_match() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: CheckType(42, "Number") should return True
    let mut builder = ChunkBuilder::new("e2e_check_type_match");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let type_atom = MettaValue::Atom("Number".to_string());
    let idx = builder.add_constant(type_atom);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::CheckType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // CheckType(42, "Number") should return True
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert_eq!(
        result.as_bool(),
        true,
        "CheckType(Long, 'Number') should return true"
    );
}

#[test]
fn test_jit_execute_check_type_mismatch() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: CheckType(42, "Bool") should return False
    let mut builder = ChunkBuilder::new("e2e_check_type_mismatch");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let type_atom = MettaValue::Atom("Bool".to_string());
    let idx = builder.add_constant(type_atom);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::CheckType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // CheckType(42, "Bool") should return False
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert_eq!(
        result.as_bool(),
        false,
        "CheckType(Long, 'Bool') should return false"
    );
}

#[test]
fn test_jit_execute_is_type_match() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: IsType(True, "Bool") should return True
    let mut builder = ChunkBuilder::new("e2e_is_type_match");
    builder.emit(Opcode::PushTrue);
    let type_atom = MettaValue::Atom("Bool".to_string());
    let idx = builder.add_constant(type_atom);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::IsType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // IsType(True, "Bool") should return True
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert_eq!(
        result.as_bool(),
        true,
        "IsType(Bool, 'Bool') should return true"
    );
}

#[test]
fn test_jit_execute_check_type_variable_matches_any() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: CheckType(42, $T) should return True (type variables match anything)
    let mut builder = ChunkBuilder::new("e2e_check_type_var");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    // Variables are represented as Atom with $ prefix
    let type_var = MettaValue::Atom("$T".to_string());
    let idx = builder.add_constant(type_var);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::CheckType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    // CheckType(42, $T) should return True (type variables are polymorphic)
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
    assert_eq!(
        result.as_bool(),
        true,
        "CheckType with type variable should return true"
    );
}

// =========================================================================
// Phase J: AssertType Tests
// =========================================================================

#[test]
fn test_jit_execute_assert_type_match() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: AssertType(42, "Number") should return 42 (value stays on stack)
    let mut builder = ChunkBuilder::new("e2e_assert_type_match");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let type_atom = MettaValue::Atom("Number".to_string());
    let idx = builder.add_constant(type_atom);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::AssertType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(
        !ctx.bailout,
        "JIT execution should not bailout on type match"
    );

    // AssertType(42, "Number") should return 42 (value unchanged)
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(
        result.as_long(),
        42,
        "AssertType should return the original value"
    );
}

#[test]
fn test_jit_execute_assert_type_mismatch() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: AssertType(42, "Bool") should signal bailout (type mismatch)
    let mut builder = ChunkBuilder::new("e2e_assert_type_mismatch");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let type_atom = MettaValue::Atom("Bool".to_string());
    let idx = builder.add_constant(type_atom);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::AssertType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // AssertType(42, "Bool") should signal bailout due to type mismatch
    assert!(ctx.bailout, "JIT execution should bailout on type mismatch");
}

#[test]
fn test_jit_execute_assert_type_variable_matches_any() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: AssertType(42, $T) should return 42 (type variables match anything)
    let mut builder = ChunkBuilder::new("e2e_assert_type_var");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let type_var = MettaValue::Atom("$T".to_string());
    let idx = builder.add_constant(type_var);
    builder.emit_u16(Opcode::PushConstant, idx);
    builder.emit(Opcode::AssertType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(
        !ctx.bailout,
        "JIT execution should not bailout with type variable"
    );

    // AssertType(42, $T) should return 42 (type variables are polymorphic)
    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result, got: {:?}", result);
    assert_eq!(
        result.as_long(),
        42,
        "AssertType with type variable should return the value"
    );
}

#[test]
fn test_jit_can_compile_assert_type() {
    // Test that AssertType is recognized as JIT compilable
    let mut builder = ChunkBuilder::new("assert_type_compilable");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 1); // Placeholder type
    builder.emit(Opcode::AssertType);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "AssertType should be Stage 1 compilable"
    );
}

// =========================================================================
// Phase 2a: MakeSExpr and ConsAtom Tests
// =========================================================================

#[test]
fn test_jit_can_compile_make_sexpr() {
    // Test that MakeSExpr is recognized as Stage 1 compilable
    let mut builder = ChunkBuilder::new("make_sexpr_test");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit_byte(Opcode::PushLongSmall, 30);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "MakeSExpr should be compilable"
    );
}

#[test]
fn test_jit_can_compile_cons_atom() {
    // Test that ConsAtom is recognized as Stage 1 compilable
    let mut builder = ChunkBuilder::new("cons_atom_test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::PushNil);
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "ConsAtom should be compilable"
    );
}

#[test]
fn test_jit_execute_make_sexpr_empty() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: MakeSExpr(0) -> ()
    let mut builder = ChunkBuilder::new("make_sexpr_empty");
    builder.emit_byte(Opcode::MakeSExpr, 0);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert!(
                items.is_empty(),
                "Expected empty S-expression, got {:?}",
                metta
            );
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_sexpr_single() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: Push 42, MakeSExpr(1) -> (42)
    let mut builder = ChunkBuilder::new("make_sexpr_single");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::MakeSExpr, 1);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 1, "Expected 1 element");
            match &items[0] {
                MettaValue::Long(v) => assert_eq!(*v, 42),
                _ => panic!("Expected Long, got: {:?}", items[0]),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_sexpr_multiple() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: Push 10, 20, 30, MakeSExpr(3) -> (10 20 30)
    let mut builder = ChunkBuilder::new("make_sexpr_multiple");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit_byte(Opcode::PushLongSmall, 30);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3, "Expected 3 elements");
            match (&items[0], &items[1], &items[2]) {
                (MettaValue::Long(a), MettaValue::Long(b), MettaValue::Long(c)) => {
                    assert_eq!((*a, *b, *c), (10, 20, 30));
                }
                _ => panic!("Expected three Longs, got: {:?}", items),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_cons_atom_to_nil() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: Push 1, PushNil, ConsAtom -> (1)
    let mut builder = ChunkBuilder::new("cons_atom_to_nil");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::PushNil);
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 1, "Expected 1 element");
            match &items[0] {
                MettaValue::Long(v) => assert_eq!(*v, 1),
                _ => panic!("Expected Long, got: {:?}", items[0]),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_cons_atom_to_sexpr() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: Create (2 3), then cons 1 to get (1 2 3)
    // Stack: Push 2, Push 3, MakeSExpr(2) -> (2 3)
    // Then: Push 1, Swap, ConsAtom -> (1 2 3)
    let mut builder = ChunkBuilder::new("cons_atom_to_sexpr");
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3, "Expected 3 elements, got {:?}", items);
            match (&items[0], &items[1], &items[2]) {
                (MettaValue::Long(a), MettaValue::Long(b), MettaValue::Long(c)) => {
                    assert_eq!((*a, *b, *c), (1, 2, 3));
                }
                _ => panic!("Expected three Longs, got: {:?}", items),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_sexpr_nested() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create chunk: ((1 2) (3 4))
    let mut builder = ChunkBuilder::new("make_sexpr_nested");
    // Create (1 2)
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    // Create (3 4)
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::PushLongSmall, 4);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    // Create ((1 2) (3 4))
    builder.emit_byte(Opcode::MakeSExpr, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2, "Expected 2 elements");
            // Check first element is (1 2)
            match &items[0] {
                MettaValue::SExpr(inner) => {
                    assert_eq!(inner.len(), 2);
                    match (&inner[0], &inner[1]) {
                        (MettaValue::Long(a), MettaValue::Long(b)) => {
                            assert_eq!((*a, *b), (1, 2));
                        }
                        _ => panic!("Expected Longs in inner, got: {:?}", inner),
                    }
                }
                _ => panic!("Expected inner SExpr, got: {:?}", items[0]),
            }
            // Check second element is (3 4)
            match &items[1] {
                MettaValue::SExpr(inner) => {
                    assert_eq!(inner.len(), 2);
                    match (&inner[0], &inner[1]) {
                        (MettaValue::Long(a), MettaValue::Long(b)) => {
                            assert_eq!((*a, *b), (3, 4));
                        }
                        _ => panic!("Expected Longs in inner, got: {:?}", inner),
                    }
                }
                _ => panic!("Expected inner SExpr, got: {:?}", items[1]),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

// =========================================================================
// Phase 2b: PushUri, MakeList, MakeQuote Tests
// =========================================================================

#[test]
fn test_jit_can_compile_push_uri() {
    use crate::backend::MettaValue;

    // Test that PushUri is compilable
    let mut builder = ChunkBuilder::new("push_uri");
    let idx = builder.add_constant(MettaValue::Atom("test-uri".to_string()));
    builder.emit_u16(Opcode::PushUri, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "PushUri should be compilable"
    );
}

#[test]
fn test_jit_can_compile_make_list() {
    // Test that MakeList is compilable
    let mut builder = ChunkBuilder::new("make_list");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeList, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "MakeList should be compilable"
    );
}

#[test]
fn test_jit_can_compile_make_quote() {
    // Test that MakeQuote is compilable
    let mut builder = ChunkBuilder::new("make_quote");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::MakeQuote);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "MakeQuote should be compilable"
    );
}

#[test]
fn test_jit_execute_push_uri() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("push_uri");
    let idx = builder.add_constant(MettaValue::Atom("http://example.com".to_string()));
    builder.emit_u16(Opcode::PushUri, idx);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::Atom(s) => assert_eq!(s, "http://example.com"),
        _ => panic!("Expected Atom, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_list_empty() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("make_list_empty");
    builder.emit_byte(Opcode::MakeList, 0);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    // Empty list is Nil
    assert!(
        matches!(metta, MettaValue::Nil),
        "Expected Nil, got: {:?}",
        metta
    );
}

#[test]
fn test_jit_execute_make_list_single() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("make_list_single");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::MakeList, 1);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    // Should be (Cons 42 Nil)
    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3, "Expected (Cons elem Nil) structure");
            match (&items[0], &items[1], &items[2]) {
                (MettaValue::Atom(cons), MettaValue::Long(v), MettaValue::Nil) => {
                    assert_eq!(cons, "Cons");
                    assert_eq!(*v, 42);
                }
                _ => panic!("Expected (Cons 42 Nil), got: {:?}", items),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_list_multiple() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("make_list_multiple");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::MakeList, 3);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    // Should be (Cons 1 (Cons 2 (Cons 3 Nil)))
    // Just check the outer structure
    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3, "Expected (Cons elem rest) structure");
            match &items[0] {
                MettaValue::Atom(s) => assert_eq!(s, "Cons"),
                _ => panic!("Expected Cons atom, got: {:?}", items[0]),
            }
            match &items[1] {
                MettaValue::Long(v) => assert_eq!(*v, 1),
                _ => panic!("Expected Long 1, got: {:?}", items[1]),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_quote() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("make_quote");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::MakeQuote);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    // Should be (quote 42)
    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2, "Expected (quote value)");
            match (&items[0], &items[1]) {
                (MettaValue::Atom(q), MettaValue::Long(v)) => {
                    assert_eq!(q, "quote");
                    assert_eq!(*v, 42);
                }
                _ => panic!("Expected (quote 42), got: {:?}", items),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

#[test]
fn test_jit_execute_make_quote_nested() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create (quote (1 2))
    let mut builder = ChunkBuilder::new("make_quote_nested");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    builder.emit(Opcode::MakeQuote);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bailout");

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    // Should be (quote (1 2))
    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2, "Expected (quote expr)");
            match &items[0] {
                MettaValue::Atom(q) => assert_eq!(q, "quote"),
                _ => panic!("Expected quote atom, got: {:?}", items[0]),
            }
            match &items[1] {
                MettaValue::SExpr(inner) => {
                    assert_eq!(inner.len(), 2);
                    match (&inner[0], &inner[1]) {
                        (MettaValue::Long(a), MettaValue::Long(b)) => {
                            assert_eq!((*a, *b), (1, 2));
                        }
                        _ => panic!("Expected (1 2), got: {:?}", inner),
                    }
                }
                _ => panic!("Expected SExpr inside quote, got: {:?}", items[1]),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

// =====================================================================
// Phase 3: Call/TailCall Tests
// =====================================================================

#[test]
fn test_jit_can_compile_call() {
    use crate::backend::MettaValue;

    // Test that Call opcode is compilable
    let mut builder = ChunkBuilder::new("call_test");
    let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 1); // arg1
    builder.emit_byte(Opcode::PushLongSmall, 2); // arg2
    builder.emit_call(head_idx, 2); // Call with 2 args
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Call should be compilable in Stage 1"
    );
}

#[test]
fn test_jit_can_compile_tail_call() {
    use crate::backend::MettaValue;

    // Test that TailCall opcode is compilable
    let mut builder = ChunkBuilder::new("tail_call_test");
    let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 1); // arg1
    builder.emit_tail_call(head_idx, 2); // TailCall with 2 args
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "TailCall should be compilable in Stage 1"
    );
}

#[test]
fn test_jit_execute_call_with_bailout() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (my-func 1 2)
    let mut builder = ChunkBuilder::new("call_bailout");
    let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 1); // arg1
    builder.emit_byte(Opcode::PushLongSmall, 2); // arg2
    builder.emit_call(head_idx, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Call should always set bailout
    assert!(ctx.bailout, "Call should set bailout flag");
    assert_eq!(
        ctx.bailout_reason,
        JitBailoutReason::Call,
        "Bailout reason should be Call"
    );

    // The result should be a heap pointer to the call expression
    // For bailout, the VM will dispatch this expression
}

#[test]
fn test_jit_execute_call_no_args() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (my-func) with no args
    let mut builder = ChunkBuilder::new("call_no_args");
    let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
    builder.emit_call(head_idx, 0); // 0 args
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(ctx.bailout, "Call with no args should set bailout flag");
    assert_eq!(
        ctx.bailout_reason,
        JitBailoutReason::Call,
        "Bailout reason should be Call"
    );
}

#[test]
fn test_jit_execute_tail_call_with_bailout() {
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (my-func 1) as tail call
    let mut builder = ChunkBuilder::new("tail_call_bailout");
    let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_tail_call(head_idx, 1);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(ctx.bailout, "TailCall should set bailout flag");
    assert_eq!(
        ctx.bailout_reason,
        JitBailoutReason::TailCall,
        "Bailout reason should be TailCall"
    );
}

#[test]
fn test_jit_call_builds_correct_expression() {
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (add 5 3)
    let mut builder = ChunkBuilder::new("call_expr");
    let head_idx = builder.add_constant(MettaValue::Atom("add".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_call(head_idx, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(ctx.bailout, "Call should bailout");

    // The result should be the call expression
    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3, "Expected (add 5 3)");
            match &items[0] {
                MettaValue::Atom(s) => assert_eq!(s, "add"),
                _ => panic!("Expected 'add' atom, got: {:?}", items[0]),
            }
            match &items[1] {
                MettaValue::Long(n) => assert_eq!(*n, 5),
                _ => panic!("Expected 5, got: {:?}", items[1]),
            }
            match &items[2] {
                MettaValue::Long(n) => assert_eq!(*n, 3),
                _ => panic!("Expected 3, got: {:?}", items[2]),
            }
        }
        _ => panic!("Expected SExpr, got: {:?}", metta),
    }
}

// =========================================================================
// Phase 2.3: Call with Rule Dispatch Integration Tests
// =========================================================================
// Tests that verify Call/TailCall opcodes construct valid expressions
// for rule dispatch integration with the VM.
// =========================================================================

#[test]
fn test_jit_call_with_mixed_argument_types() {
    // Test Call with mixed argument types: atom, long, bool, nested sexpr
    // This validates the expression structure for rule pattern matching
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (process atom-arg 42 True (nested 1 2))
    let mut builder = ChunkBuilder::new("call_mixed_args");
    let head_idx = builder.add_constant(MettaValue::Atom("process".to_string()));
    let atom_idx = builder.add_constant(MettaValue::Atom("atom-arg".to_string()));
    let nested_idx = builder.add_constant(MettaValue::SExpr(vec![
        MettaValue::Atom("nested".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));

    // Push arguments in order
    builder.emit_u16(Opcode::PushConstant, atom_idx); // atom-arg
    builder.emit_byte(Opcode::PushLongSmall, 42); // 42
    builder.emit(Opcode::PushTrue); // True
    builder.emit_u16(Opcode::PushConstant, nested_idx); // (nested 1 2)
    builder.emit_call(head_idx, 4); // Call with 4 args
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Verify bailout occurred
    assert!(ctx.bailout, "Call should set bailout flag");
    assert_eq!(
        ctx.bailout_reason,
        JitBailoutReason::Call,
        "Bailout reason should be Call"
    );

    // Verify the constructed expression
    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(
                items.len(),
                5,
                "Expected (process atom-arg 42 True (nested 1 2))"
            );

            // Head: process
            match &items[0] {
                MettaValue::Atom(s) => assert_eq!(s, "process"),
                _ => panic!("Expected 'process' atom"),
            }

            // Arg 1: atom-arg
            match &items[1] {
                MettaValue::Atom(s) => assert_eq!(s, "atom-arg"),
                _ => panic!("Expected 'atom-arg' atom"),
            }

            // Arg 2: 42
            match &items[2] {
                MettaValue::Long(n) => assert_eq!(*n, 42),
                _ => panic!("Expected Long(42)"),
            }

            // Arg 3: True
            match &items[3] {
                MettaValue::Bool(b) => assert!(*b),
                _ => panic!("Expected Bool(true)"),
            }

            // Arg 4: (nested 1 2)
            match &items[4] {
                MettaValue::SExpr(nested) => {
                    assert_eq!(nested.len(), 3);
                    match &nested[0] {
                        MettaValue::Atom(s) => assert_eq!(s, "nested"),
                        _ => panic!("Expected 'nested' atom"),
                    }
                    match &nested[1] {
                        MettaValue::Long(n) => assert_eq!(*n, 1),
                        _ => panic!("Expected Long(1)"),
                    }
                    match &nested[2] {
                        MettaValue::Long(n) => assert_eq!(*n, 2),
                        _ => panic!("Expected Long(2)"),
                    }
                }
                _ => panic!("Expected nested SExpr"),
            }
        }
        _ => panic!("Expected SExpr"),
    }
}

#[test]
fn test_jit_call_expression_valid_for_rule_pattern() {
    // Test that JIT-constructed expressions match expected rule patterns
    // Pattern: (fib $n) -> matches (fib 10)
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (fib 10) - a typical recursive call pattern
    let mut builder = ChunkBuilder::new("call_fib");
    let head_idx = builder.add_constant(MettaValue::Atom("fib".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_call(head_idx, 1);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let call_expr = unsafe { result.to_metta() };

    // Verify the expression can be matched against a rule pattern
    // Rule pattern: (= (fib $n) ...)
    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("fib".to_string()),
        MettaValue::Atom("$n".to_string()),
    ]);

    // The call expression should have the same structure
    match (&call_expr, &pattern) {
        (MettaValue::SExpr(expr_items), MettaValue::SExpr(pattern_items)) => {
            assert_eq!(expr_items.len(), pattern_items.len(), "Arity mismatch");

            // Head should match exactly
            match (&expr_items[0], &pattern_items[0]) {
                (MettaValue::Atom(e), MettaValue::Atom(p)) => {
                    assert_eq!(e, p, "Head atoms should match");
                }
                _ => panic!("Expected both heads to be atoms"),
            }

            // Argument should be a Long that would bind to $n
            match &expr_items[1] {
                MettaValue::Long(n) => assert_eq!(*n, 10, "Argument should be 10"),
                _ => panic!("Expected Long argument"),
            }
        }
        _ => panic!("Both should be SExprs"),
    }
}

#[test]
fn test_jit_tail_call_preserves_tco_flag() {
    // Test that TailCall sets the correct bailout reason for TCO
    use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create a tail call with multiple arguments
    let mut builder = ChunkBuilder::new("tail_call_tco");
    let head_idx = builder.add_constant(MettaValue::Atom("recurse".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 5); // countdown
    builder.emit_byte(Opcode::PushLongSmall, 100); // accumulator
    builder.emit_tail_call(head_idx, 2);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Verify bailout with TailCall reason (for TCO)
    assert!(ctx.bailout, "TailCall should set bailout flag");
    assert_eq!(
        ctx.bailout_reason,
        JitBailoutReason::TailCall,
        "Bailout reason should be TailCall for TCO"
    );

    // Verify expression structure
    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3, "Expected (recurse 5 100)");
            match &items[0] {
                MettaValue::Atom(s) => assert_eq!(s, "recurse"),
                _ => panic!("Expected 'recurse' atom"),
            }
            match &items[1] {
                MettaValue::Long(n) => assert_eq!(*n, 5),
                _ => panic!("Expected Long(5)"),
            }
            match &items[2] {
                MettaValue::Long(n) => assert_eq!(*n, 100),
                _ => panic!("Expected Long(100)"),
            }
        }
        _ => panic!("Expected SExpr"),
    }
}

#[test]
fn test_jit_call_with_zero_args_returns_head_only() {
    // Test Call with no arguments returns just the head in an SExpr
    use crate::backend::bytecode::jit::{JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create: (get-value) - call with no args
    let mut builder = ChunkBuilder::new("call_zero_args");
    let head_idx = builder.add_constant(MettaValue::Atom("get-value".to_string()));
    builder.emit_call(head_idx, 0); // 0 args
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match &metta {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 1, "Expected (get-value) with just head");
            match &items[0] {
                MettaValue::Atom(s) => assert_eq!(s, "get-value"),
                _ => panic!("Expected 'get-value' atom"),
            }
        }
        _ => panic!("Expected SExpr"),
    }
}

// =========================================================================
// Phase A: Binding Operations Tests
// =========================================================================

#[test]
fn test_can_compile_binding_opcodes() {
    // Phase A: Binding operations should be compilable
    let mut builder = ChunkBuilder::new("test_bindings");
    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::StoreBinding, 0); // Store to binding 0
    builder.emit_u16(Opcode::LoadBinding, 0); // Load from binding 0
    builder.emit_u16(Opcode::HasBinding, 0); // Check binding 0
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::ClearBindings);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Binding opcodes should be compilable"
    );
}

#[test]
fn test_jit_binding_frame_operations() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::models::metta_value::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: Push frame, store 42, load it, return
    let mut builder = ChunkBuilder::new("test_binding_frame");
    builder.emit(Opcode::PushBindingFrame); // Create new frame
    builder.emit_byte(Opcode::PushLongSmall, 42); // Push 42
    builder.emit_u16(Opcode::StoreBinding, 0); // Store to binding index 0
    builder.emit_u16(Opcode::LoadBinding, 0); // Load from binding index 0
    builder.emit(Opcode::PopBindingFrame); // Pop frame
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Compile to native code
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    // Set up JIT context with binding support
    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    // Allocate binding frames array (capacity 16)
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    // Set up binding frames pointer and capacity
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;

    // Initialize root binding frame
    unsafe { ctx.init_root_binding_frame() };

    // Execute JIT code
    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Result should be 42 (the loaded binding value)
    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Long(n) => assert_eq!(n, 42, "Expected 42 from binding"),
        other => panic!("Expected Long(42), got: {:?}", other),
    }
}

#[test]
fn test_jit_has_binding() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::models::metta_value::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: Push frame, check binding (should be false), store, check again (should be true)
    let mut builder = ChunkBuilder::new("test_has_binding");
    builder.emit(Opcode::PushBindingFrame); // Create new frame
    builder.emit_byte(Opcode::PushLongSmall, 99); // Push 99
    builder.emit_u16(Opcode::StoreBinding, 5); // Store to binding index 5
    builder.emit_u16(Opcode::HasBinding, 5); // Check binding 5 exists
    builder.emit(Opcode::PopBindingFrame); // Pop frame
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Compile to native code
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    // Set up JIT context with binding support
    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    // Allocate binding frames array (capacity 16)
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    // Set up binding frames pointer and capacity
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;

    // Initialize root binding frame
    unsafe { ctx.init_root_binding_frame() };

    // Execute JIT code
    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Result should be True (binding exists)
    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(b, "Expected True for has_binding after store"),
        other => panic!("Expected Bool(true), got: {:?}", other),
    }
}

#[test]
fn test_jit_clear_bindings() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::models::metta_value::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Build bytecode: Store binding, clear, check (should be false)
    let mut builder = ChunkBuilder::new("test_clear_bindings");
    builder.emit(Opcode::PushBindingFrame); // Create new frame
    builder.emit_byte(Opcode::PushLongSmall, 77); // Push 77
    builder.emit_u16(Opcode::StoreBinding, 3); // Store to binding index 3
    builder.emit(Opcode::ClearBindings); // Clear all bindings
    builder.emit_u16(Opcode::HasBinding, 3); // Check binding 3 exists
    builder.emit(Opcode::PopBindingFrame); // Pop frame
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Compile to native code
    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    // Set up JIT context with binding support
    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    // Allocate binding frames array (capacity 16)
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    // Set up binding frames pointer and capacity
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;

    // Initialize root binding frame
    unsafe { ctx.init_root_binding_frame() };

    // Execute JIT code
    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // Result should be False (binding cleared)
    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(!b, "Expected False for has_binding after clear"),
        other => panic!("Expected Bool(false), got: {:?}", other),
    }
}

// =========================================================================
// Phase B: Pattern Matching Tests
// =========================================================================

#[test]
fn test_can_compile_pattern_matching_opcodes() {
    // Test that all pattern matching opcodes are compilable
    let mut builder = ChunkBuilder::new("pattern_match_test");

    // Match opcode (no operands)
    builder.emit(Opcode::PushNil); // pattern
    builder.emit(Opcode::PushNil); // value
    builder.emit(Opcode::Match);

    // MatchBind opcode (no operands)
    builder.emit(Opcode::PushNil); // pattern
    builder.emit(Opcode::PushNil); // value
    builder.emit(Opcode::MatchBind);

    // MatchHead opcode (1 byte operand)
    builder.emit(Opcode::PushNil); // expr
    builder.emit_byte(Opcode::MatchHead, 0); // expected head idx

    // MatchArity opcode (1 byte operand)
    builder.emit(Opcode::PushNil); // expr
    builder.emit_byte(Opcode::MatchArity, 3); // expected arity

    // Unify opcode (no operands)
    builder.emit(Opcode::PushNil); // a
    builder.emit(Opcode::PushNil); // b
    builder.emit(Opcode::Unify);

    // UnifyBind opcode (no operands)
    builder.emit(Opcode::PushNil); // a
    builder.emit(Opcode::PushNil); // b
    builder.emit(Opcode::UnifyBind);

    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Pattern matching opcodes should be compilable"
    );
}

#[test]
fn test_jit_pattern_match_simple() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Test: Match 42 against 42 should return True
    let mut builder = ChunkBuilder::new("match_simple");
    builder.emit_byte(Opcode::PushLongSmall, 42); // pattern
    builder.emit_byte(Opcode::PushLongSmall, 42); // value
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(b, "Matching 42 against 42 should return True"),
        other => panic!("Expected Bool(true), got: {:?}", other),
    }
}

#[test]
fn test_jit_pattern_match_mismatch() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Test: Match 42 against 99 should return False
    let mut builder = ChunkBuilder::new("match_mismatch");
    builder.emit_byte(Opcode::PushLongSmall, 42); // pattern
    builder.emit_byte(Opcode::PushLongSmall, 99); // value
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(!b, "Matching 42 against 99 should return False"),
        other => panic!("Expected Bool(false), got: {:?}", other),
    }
}

#[test]
fn test_jit_unify_simple() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Test: Unify 42 with 42 should return True
    let mut builder = ChunkBuilder::new("unify_simple");
    builder.emit_byte(Opcode::PushLongSmall, 42); // a
    builder.emit_byte(Opcode::PushLongSmall, 42); // b
    builder.emit(Opcode::Unify);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(b, "Unifying 42 with 42 should return True"),
        other => panic!("Expected Bool(true), got: {:?}", other),
    }
}

#[test]
fn test_jit_match_arity() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create an S-expression with 3 elements: (a b c)
    let mut builder = ChunkBuilder::new("match_arity");
    let a_idx = builder.add_constant(MettaValue::Atom("a".to_string()));
    let b_idx = builder.add_constant(MettaValue::Atom("b".to_string()));
    let c_idx = builder.add_constant(MettaValue::Atom("c".to_string()));

    // Push atoms and make S-expr
    builder.emit_u16(Opcode::PushAtom, a_idx);
    builder.emit_u16(Opcode::PushAtom, b_idx);
    builder.emit_u16(Opcode::PushAtom, c_idx);
    builder.emit_byte(Opcode::MakeSExpr, 3);

    // Check arity is 3
    builder.emit_byte(Opcode::MatchArity, 3);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(b, "S-expr (a b c) should have arity 3"),
        other => panic!("Expected Bool(true), got: {:?}", other),
    }
}

#[test]
fn test_jit_match_head() {
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Create an S-expression: (foo bar baz)
    // Then check if head matches "foo"
    let mut builder = ChunkBuilder::new("match_head");
    let foo_idx = builder.add_constant(MettaValue::Atom("foo".to_string()));
    let bar_idx = builder.add_constant(MettaValue::Atom("bar".to_string()));
    let baz_idx = builder.add_constant(MettaValue::Atom("baz".to_string()));

    // Push atoms and make S-expr
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_u16(Opcode::PushAtom, bar_idx);
    builder.emit_u16(Opcode::PushAtom, baz_idx);
    builder.emit_byte(Opcode::MakeSExpr, 3);

    // Match head against "foo" (index 0 in constant pool)
    builder.emit_byte(Opcode::MatchHead, foo_idx as u8);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Bool(b) => assert!(b, "S-expr (foo bar baz) should have head 'foo'"),
        other => panic!("Expected Bool(true), got: {:?}", other),
    }
}

#[test]
fn test_jit_match_bind_variable_extraction() {
    // Test Phase 2.1: MatchBind with variable extraction
    // Pattern: ($x 2) matches against (1 2), should bind $x = 1
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
    use crate::backend::MettaValue;

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("match_bind_var_extract");

    // Add constant for variable name $x (needed for binding lookup)
    let x_var_idx = builder.add_constant(MettaValue::Atom("$x".to_string()));

    // Create pattern S-expression: ($x 2)
    let pattern_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(2),
    ]);
    let pattern_idx = builder.add_constant(pattern_sexpr);

    // Create value S-expression: (1 2)
    let value_sexpr = MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]);
    let value_idx = builder.add_constant(value_sexpr);

    // Bytecode:
    // 1. Push binding frame (for variable bindings)
    // 2. Push pattern
    // 3. Push value
    // 4. MatchBind (should bind $x to 1 and return true)
    // 5. Pop the bool result
    // 6. Load binding for $x (at index x_var_idx)
    // 7. Return the loaded value
    builder.emit(Opcode::PushBindingFrame);
    builder.emit_u16(Opcode::PushConstant, pattern_idx);
    builder.emit_u16(Opcode::PushConstant, value_idx);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::Pop); // Pop the bool result
    builder.emit_u16(Opcode::LoadBinding, x_var_idx);
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    let metta = unsafe { result.to_metta() };

    match metta {
        MettaValue::Long(n) => assert_eq!(n, 1, "MatchBind should have bound $x to 1"),
        other => panic!("Expected Long(1) from bound variable, got: {:?}", other),
    }
}

// =========================================================================
// Phase D: Space Operations Tests
// =========================================================================

#[test]
fn test_can_compile_space_opcodes() {
    // Phase D: Space operations should be compilable
    let mut builder = ChunkBuilder::new("test_space_ops");

    // SpaceAdd opcode (no operands)
    builder.emit(Opcode::PushNil); // space
    builder.emit(Opcode::PushNil); // atom
    builder.emit(Opcode::SpaceAdd);
    builder.emit(Opcode::Pop);

    // SpaceRemove opcode (no operands)
    builder.emit(Opcode::PushNil); // space
    builder.emit(Opcode::PushNil); // atom
    builder.emit(Opcode::SpaceRemove);
    builder.emit(Opcode::Pop);

    // SpaceGetAtoms opcode (no operands)
    builder.emit(Opcode::PushNil); // space
    builder.emit(Opcode::SpaceGetAtoms);
    builder.emit(Opcode::Pop);

    // SpaceMatch opcode (no operands)
    builder.emit(Opcode::PushNil); // space
    builder.emit(Opcode::PushNil); // pattern
    builder.emit(Opcode::PushNil); // template
    builder.emit(Opcode::SpaceMatch);

    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Space operations should be compilable"
    );
}

#[test]
fn test_jit_space_add_returns_result() {
    // Test that SpaceAdd JIT compilation produces valid code
    // Note: With nil space, this should return error/fail gracefully
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("space_add");
    builder.emit(Opcode::PushNil); // space (nil = invalid)
    builder.emit(Opcode::PushNil); // atom
    builder.emit(Opcode::SpaceAdd);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // With nil space, we expect unit (success) or error
    let result = JitValue::from_raw(result_bits as u64);
    // Just verify we got a valid JIT value back (no crash)
    assert!(
        result.is_bool() || result.is_unit() || result.is_nil() || result.is_error(),
        "SpaceAdd should return bool, unit, nil, or error"
    );
}

#[test]
fn test_jit_space_remove_returns_result() {
    // Test that SpaceRemove JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("space_remove");
    builder.emit(Opcode::PushNil); // space (nil = invalid)
    builder.emit(Opcode::PushNil); // atom
    builder.emit(Opcode::SpaceRemove);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_bool() || result.is_unit() || result.is_nil() || result.is_error(),
        "SpaceRemove should return bool, unit, nil, or error"
    );
}

#[test]
fn test_jit_space_get_atoms_returns_result() {
    // Test that SpaceGetAtoms JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("space_get_atoms");
    builder.emit(Opcode::PushNil); // space (nil = invalid)
    builder.emit(Opcode::SpaceGetAtoms);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    // Nil space returns nil, empty list, or unit
    assert!(
        result.is_nil() || result.is_unit() || result.is_heap() || result.is_error(),
        "SpaceGetAtoms should return list, nil, unit, or error"
    );
}

#[test]
fn test_jit_space_match_returns_result() {
    // Test that SpaceMatch JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("space_match");
    builder.emit(Opcode::PushNil); // space (nil = invalid)
    builder.emit(Opcode::PushNil); // pattern
    builder.emit(Opcode::PushNil); // template
    builder.emit(Opcode::SpaceMatch);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    let result = JitValue::from_raw(result_bits as u64);
    // Nil space returns nil, empty results, or unit
    assert!(
        result.is_nil() || result.is_unit() || result.is_heap() || result.is_error(),
        "SpaceMatch should return results list, nil, unit, or error"
    );
}

// =========================================================================
// Phase C: Rule Dispatch Tests
// =========================================================================

#[test]
fn test_can_compile_rule_dispatch_opcodes() {
    // Phase C: Rule dispatch operations should be compilable
    let mut builder = ChunkBuilder::new("test_rule_dispatch_ops");

    // DispatchRules opcode - dispatches rules for an expression
    builder.emit(Opcode::PushNil); // expression to dispatch
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Pop);

    // TryRule opcode with operand (rule index)
    builder.emit_u16(Opcode::TryRule, 0); // try rule at index 0
    builder.emit(Opcode::Pop);

    // NextRule opcode (no operands)
    builder.emit(Opcode::NextRule);

    // CommitRule opcode (no operands)
    builder.emit(Opcode::CommitRule);

    // FailRule opcode (no operands)
    builder.emit(Opcode::FailRule);

    // LookupRules opcode with operand (head index)
    builder.emit_u16(Opcode::LookupRules, 0); // lookup rules for head at index 0
    builder.emit(Opcode::Pop);

    // ApplySubst opcode (no operands)
    builder.emit(Opcode::PushNil); // expression to substitute
    builder.emit(Opcode::ApplySubst);
    builder.emit(Opcode::Pop);

    // DefineRule opcode with operand (pattern index)
    builder.emit(Opcode::PushNil); // pattern
    builder.emit(Opcode::PushNil); // body
    builder.emit_u16(Opcode::DefineRule, 0); // define rule with pattern at index 0

    builder.emit(Opcode::Return);
    let chunk = builder.build();

    assert!(
        JitCompiler::can_compile_stage1(&chunk),
        "Rule dispatch operations should be compilable"
    );
}

#[test]
fn test_jit_dispatch_rules_returns_result() {
    // Test that DispatchRules JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("dispatch_rules");
    builder.emit(Opcode::PushNil); // expression (nil = no rules)
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // DispatchRules returns the count of matching rules (as Long)
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_long() || result.is_nil() || result.is_unit() || result.is_error(),
        "DispatchRules should return count (Long), nil, unit, or error"
    );
}

#[test]
fn test_jit_try_rule_returns_result() {
    // Test that TryRule JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("try_rule");
    builder.emit_u16(Opcode::TryRule, 0); // try rule at index 0
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // TryRule returns the result of applying the rule
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_unit() || result.is_nil() || result.is_heap() || result.is_error(),
        "TryRule should return unit, nil, heap, or error"
    );
}

#[test]
fn test_jit_lookup_rules_returns_result() {
    // Test that LookupRules JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("lookup_rules");
    builder.emit_u16(Opcode::LookupRules, 0); // lookup rules for head at index 0
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // LookupRules returns the count of matching rules (as Long)
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_long() || result.is_nil() || result.is_unit() || result.is_error(),
        "LookupRules should return count (Long), nil, unit, or error"
    );
}

#[test]
fn test_jit_apply_subst_returns_result() {
    // Test that ApplySubst JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("apply_subst");
    builder.emit(Opcode::PushNil); // expression to substitute
    builder.emit(Opcode::ApplySubst);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // ApplySubst returns the substituted expression
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_nil() || result.is_unit() || result.is_heap() || result.is_error(),
        "ApplySubst should return substituted expr, nil, unit, or error"
    );
}

#[test]
fn test_jit_define_rule_returns_result() {
    // Test that DefineRule JIT compilation produces valid code
    use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("define_rule");
    builder.emit(Opcode::PushNil); // pattern
    builder.emit(Opcode::PushNil); // body
    builder.emit_u16(Opcode::DefineRule, 0); // define rule
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
    let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };
    ctx.binding_frames = binding_frames.as_mut_ptr();
    ctx.binding_frames_cap = 16;
    unsafe { ctx.init_root_binding_frame() };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    // DefineRule returns unit on success
    let result = JitValue::from_raw(result_bits as u64);
    assert!(
        result.is_unit() || result.is_nil() || result.is_error(),
        "DefineRule should return unit, nil, or error"
    );
}

// =========================================================================
// Let Binding Scope Cleanup Tests (StackUnderflow fix)
// =========================================================================

#[test]
fn test_jit_let_binding_scope_cleanup() {
    // Test that (let $x 1 0) compiles and executes correctly.
    // This was previously failing with StackUnderflow because:
    // - StoreLocal removes the value from the simulated stack (into locals vec)
    // - The subsequent Swap/Pop scope cleanup expected the value on stack
    // The fix makes Swap/Pop gracefully handle this case.
    use crate::backend::bytecode::jit::{JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    // Bytecode for: (let $x 1 0)
    // Pattern: Push value, StoreLocal, Push body result, Swap (no-op), Pop (no-op), Return
    let mut builder = ChunkBuilder::new("test_let_scope_cleanup");
    builder.set_local_count(1);
    builder.emit_byte(Opcode::PushLongSmall, 1); // Push value for binding
    builder.emit_byte(Opcode::StoreLocal, 0); // Store to local (removes from stack)
    builder.emit_byte(Opcode::PushLongSmall, 0); // Push body result
    builder.emit(Opcode::Swap); // Scope cleanup - should be no-op
    builder.emit(Opcode::Pop); // Scope cleanup - should be no-op
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    // Verify it can compile (was failing before fix)
    assert!(JitCompiler::can_compile_stage1(&chunk));

    let code_ptr = compiler
        .compile(&chunk)
        .expect("JIT compilation should succeed");

    // Execute and verify result
    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bail out");

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 0, "Body result should be 0");
}

#[test]
fn test_jit_nested_let_bindings() {
    // Test nested let bindings: (let $x 1 (let $y 2 $y))
    // Verifies multiple scope cleanup sequences work correctly
    use crate::backend::bytecode::jit::{JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("test_nested_let");
    builder.set_local_count(2);

    // Outer let: bind $x = 1
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::StoreLocal, 0);

    // Inner let: bind $y = 2
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::StoreLocal, 1);

    // Body: load $y (returns 2)
    builder.emit_byte(Opcode::LoadLocal, 1);

    // Inner scope cleanup (no-op since local is in separate storage)
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Pop);

    // Outer scope cleanup (no-op)
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Pop);

    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler
        .compile(&chunk)
        .expect("JIT compilation should succeed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "JIT execution should not bail out");

    let result = JitValue::from_raw(result_bits as u64);
    assert!(result.is_long(), "Expected Long result");
    assert_eq!(result.as_long(), 2, "Should return $y = 2");
}

#[test]
fn test_jit_pop_empty_stack_is_noop() {
    // Test that Pop on empty stack is a no-op (not an error)
    use crate::backend::bytecode::jit::{JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("test_pop_empty");
    builder.emit(Opcode::Pop); // Should be no-op on empty stack
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler
        .compile(&chunk)
        .expect("JIT compilation should succeed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "Pop on empty stack should not bail out");

    let result = JitValue::from_raw(result_bits as u64);
    assert_eq!(result.as_long(), 42, "Should return 42");
}

#[test]
fn test_jit_swap_single_value_is_noop() {
    // Test that Swap with single value on stack is a no-op
    use crate::backend::bytecode::jit::{JitContext, JitValue};

    let mut compiler = JitCompiler::new().expect("Failed to create compiler");

    let mut builder = ChunkBuilder::new("test_swap_single");
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit(Opcode::Swap); // Should be no-op with only one value
    builder.emit(Opcode::Return);
    let chunk = builder.build();

    let code_ptr = compiler
        .compile(&chunk)
        .expect("JIT compilation should succeed");

    let constants = chunk.constants();
    let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

    let mut ctx =
        unsafe { JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len()) };

    let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
        unsafe { std::mem::transmute(code_ptr) };
    let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

    assert!(!ctx.bailout, "Swap with single value should not bail out");

    let result = JitValue::from_raw(result_bits as u64);
    assert_eq!(result.as_long(), 99, "Should return 99 unchanged");
}
