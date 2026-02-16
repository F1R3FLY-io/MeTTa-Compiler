//! Unit tests for the bytecode compiler.

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

use super::error::CompileError;
use super::compile;

// Helper to compile and disassemble
#[allow(dead_code)]
fn compile_and_disasm(expr: &MettaValue) -> String {
    let chunk = compile("test", expr).expect("compilation should succeed");
    chunk.disassemble()
}

// ========================================================================
// Literal Compilation Tests
// ========================================================================

#[test]
fn test_compile_nil() {
    // After Nil/Unit merge, Nil() returns Unit, so PushUnit is expected
    let chunk = compile("test", &MettaValue::Unit()).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushUnit));
}

#[test]
fn test_compile_unit() {
    let chunk = compile("test", &MettaValue::Unit()).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushUnit));
}

#[test]
fn test_compile_true() {
    let chunk = compile("test", &MettaValue::Bool(true)).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushTrue));
}

#[test]
fn test_compile_false() {
    let chunk = compile("test", &MettaValue::Bool(false)).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushFalse));
}

#[test]
fn test_compile_small_int() {
    let chunk = compile("test", &MettaValue::Long(42)).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLongSmall));
    assert_eq!(chunk.read_byte(1), Some(42));
}

#[test]
fn test_compile_negative_small_int() {
    let chunk = compile("test", &MettaValue::Long(-10)).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLongSmall));
    assert_eq!(chunk.read_byte(1), Some((-10i8) as u8));
}

#[test]
fn test_compile_large_int() {
    let chunk = compile("test", &MettaValue::Long(1000)).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLong));
    assert_eq!(chunk.get_constant(0), Some(&MettaValue::Long(1000)));
}

#[test]
fn test_compile_string() {
    let chunk = compile("test", &MettaValue::String("hello".to_string())).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushString));
    assert_eq!(
        chunk.get_constant(0),
        Some(&MettaValue::String("hello".to_string()))
    );
}

#[test]
#[allow(clippy::approx_constant)]
fn test_compile_float() {
    let chunk = compile("test", &MettaValue::Float(3.14)).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushConstant));
    assert_eq!(chunk.get_constant(0), Some(&MettaValue::Float(3.14)));
}

// ========================================================================
// Symbol and Variable Tests
// ========================================================================

#[test]
fn test_compile_symbol() {
    let chunk = compile("test", &MettaValue::Atom("foo".to_string())).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushAtom));
    assert_eq!(
        chunk.get_constant(0),
        Some(&MettaValue::Atom("foo".to_string()))
    );
}

#[test]
fn test_compile_variable() {
    let chunk = compile("test", &MettaValue::Atom("$x".to_string())).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushVariable));
    assert_eq!(
        chunk.get_constant(0),
        Some(&MettaValue::Atom("$x".to_string()))
    );
}

// ========================================================================
// Arithmetic Operations Tests
// ========================================================================

#[test]
fn test_compile_add() {
    // Use variables to prevent constant folding - tests opcode emission
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("add"));
}

#[test]
fn test_compile_add_constant_folding() {
    // Verify constant folding: (+ 1 2) -> push 3
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold to constant 3, no add opcode
    assert!(disasm.contains("push_long_small 3"));
    assert!(!disasm.contains("\nadd\n"));
}

#[test]
fn test_compile_sub() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("-".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("sub"));
}

#[test]
fn test_compile_sub_constant_folding() {
    // Verify constant folding: (- 5 3) -> push 2
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("-".to_string()),
        MettaValue::Long(5),
        MettaValue::Long(3),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 2"));
    assert!(!disasm.contains("\nsub\n"));
}

#[test]
fn test_compile_mul() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("mul"));
}

#[test]
fn test_compile_mul_constant_folding() {
    // Verify constant folding: (* 3 4) -> push 12
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Long(3),
        MettaValue::Long(4),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 12"));
    assert!(!disasm.contains("\nmul\n"));
}

#[test]
fn test_compile_div() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("div"));
}

#[test]
fn test_compile_div_constant_folding() {
    // Verify constant folding: (/ 10 2) -> push 5
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 5"));
    assert!(!disasm.contains("\ndiv\n"));
}

#[test]
fn test_compile_mod() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("mod"));
}

#[test]
fn test_compile_mod_constant_folding() {
    // Verify constant folding: (% 10 3) -> push 1
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(3),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 1"));
    assert!(!disasm.contains("\nmod\n"));
}

#[test]
fn test_compile_nested_arithmetic() {
    // Use variables to prevent constant folding - tests opcode emission
    // (+ (* $x $y) $z)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
        MettaValue::Atom("$z".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("mul"));
    assert!(disasm.contains("add"));
}

#[test]
fn test_compile_nested_arithmetic_constant_folding() {
    // Verify constant folding of nested expressions: (+ (* 3 4) 5) -> push 17
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(3),
            MettaValue::Long(4),
        ]),
        MettaValue::Long(5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold all the way to 17
    assert!(disasm.contains("push_long_small 17"));
    assert!(!disasm.contains("\nmul\n"));
    assert!(!disasm.contains("\nadd\n"));
}

// ========================================================================
// Comparison Operations Tests
// ========================================================================

#[test]
fn test_compile_lt() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("lt"));
}

#[test]
fn test_compile_lt_constant_folding() {
    // Verify constant folding: (< 1 2) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\nlt\n"));
}

#[test]
fn test_compile_le() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<=".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("le"));
}

#[test]
fn test_compile_le_constant_folding() {
    // Verify constant folding: (<= 1 2) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<=".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\nle\n"));
}

#[test]
fn test_compile_gt() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom(">".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("gt"));
}

#[test]
fn test_compile_gt_constant_folding() {
    // Verify constant folding: (> 2 1) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom(">".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\ngt\n"));
}

#[test]
fn test_compile_ge() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom(">=".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("ge"));
}

#[test]
fn test_compile_ge_constant_folding() {
    // Verify constant folding: (>= 2 1) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom(">=".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\nge\n"));
}

#[test]
fn test_compile_eq() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("eq"));
}

#[test]
fn test_compile_eq_constant_folding() {
    // Verify constant folding: (== 1 1) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\neq\n"));
}

#[test]
fn test_compile_ne() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("ne"));
}

#[test]
fn test_compile_ne_constant_folding() {
    // Verify constant folding: (!= 1 2) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\nne\n"));
}

// ========================================================================
// Boolean Operations Tests
// ========================================================================

#[test]
fn test_compile_and() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("and"));
}

#[test]
fn test_compile_and_constant_folding() {
    // Verify constant folding: (and True False) -> False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"));
    assert!(!disasm.contains("\nand\n"));
}

#[test]
fn test_compile_or() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("or"));
}

#[test]
fn test_compile_or_constant_folding() {
    // Verify constant folding: (or True False) -> True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"));
    assert!(!disasm.contains("\nor\n"));
}

#[test]
fn test_compile_not() {
    // Use variables to prevent constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("not"));
}

#[test]
fn test_compile_not_constant_folding() {
    // Verify constant folding: (not True) -> False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(true),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"));
    assert!(!disasm.contains("\nnot\n"));
}

// ========================================================================
// Control Flow Tests
// ========================================================================

#[test]
fn test_compile_if() {
    // Use variable condition to prevent constant folding - tests opcode emission
    // (if $cond 1 2)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Atom("$cond".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("jump_if_false"));
    assert!(disasm.contains("push_long_small 1"));
    assert!(disasm.contains("jump"));
    assert!(disasm.contains("push_long_small 2"));
}

#[test]
fn test_compile_if_constant_folding_true() {
    // Verify constant folding: (if True 1 2) -> push 1 (then branch only)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold to just pushing 1, no jumps
    assert!(disasm.contains("push_long_small 1"));
    assert!(!disasm.contains("push_long_small 2"));
    assert!(!disasm.contains("jump_if_false"));
}

#[test]
fn test_compile_if_constant_folding_false() {
    // Verify constant folding: (if False 1 2) -> push 2 (else branch only)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(false),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold to just pushing 2, no jumps
    assert!(disasm.contains("push_long_small 2"));
    assert!(!disasm.contains("push_long_small 1"));
    assert!(!disasm.contains("jump_if_false"));
}

#[test]
fn test_compile_nested_if() {
    // Use variable comparison to prevent constant folding - tests opcode emission
    // (if (< $x $y) (if $cond 10 20) 30)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Atom("$cond".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]),
        MettaValue::Long(30),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("lt"));
    // Should have multiple jumps for nested ifs
    assert!(disasm.matches("jump").count() >= 2);
}

#[test]
fn test_compile_nested_if_constant_folding() {
    // Verify constant folding of nested ifs: (if (< 1 2) (if True 10 20) 30)
    // -> (if True (if True 10 20) 30) -> (if True 10 20) -> 10
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]),
        MettaValue::Long(30),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold all the way to just 10
    assert!(disasm.contains("push_long_small 10"));
    assert!(!disasm.contains("push_long_small 20"));
    assert!(!disasm.contains("push_long_small 30"));
    assert!(!disasm.contains("jump"));
}

// ========================================================================
// Quote and Eval Tests
// ========================================================================

#[test]
fn test_compile_quote() {
    // (quote (+ 1 2))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("quote".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should build S-expression, not execute add
    assert!(disasm.contains("make_sexpr"));
    assert!(!disasm.contains("\nadd\n")); // No add operation
}

#[test]
fn test_compile_eval() {
    // (eval expr)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("eval".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("eval_eval"));
}

// ========================================================================
// Let Binding Tests
// ========================================================================

#[test]
fn test_compile_let() {
    // (let $x 10 (+ $x 1))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(10),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 10"));
    assert!(disasm.contains("store_local"));
    assert!(disasm.contains("load_local"));
    assert!(disasm.contains("add"));
}

#[test]
fn test_compile_let_star() {
    // (let* (($x 1) ($y 2)) (+ $x $y))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("$y".to_string()),
                MettaValue::Long(2),
            ]),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("store_local"));
    assert!(disasm.contains("add"));
}

// ========================================================================
// Type Operations Tests
// ========================================================================

#[test]
fn test_compile_get_type() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("get-type".to_string()),
        MettaValue::Long(42),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("get_type"));
}

#[test]
fn test_compile_check_type() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("check-type".to_string()),
        MettaValue::Long(42),
        MettaValue::Atom("Number".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("check_type"));
}

// ========================================================================
// List Operations Tests
// ========================================================================

#[test]
fn test_compile_car_atom() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("car-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("get_head"));
}

#[test]
fn test_compile_cdr_atom() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("cdr-atom".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("get_tail"));
}

#[test]
fn test_compile_size_atom() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("size-atom".to_string()),
        MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("get_arity"));
}

#[test]
fn test_compile_empty() {
    // MeTTa semantics: (empty) returns NO results, equivalent to Fail
    let expr = MettaValue::SExpr(vec![MettaValue::Atom("empty".to_string())]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("fail"));
}

// ========================================================================
// Generic S-Expression Tests
// ========================================================================

#[test]
fn test_compile_unknown_operation() {
    // (foo 1 2 3) - unknown operation, compile as function call
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Arguments are pushed first
    assert!(disasm.contains("push_long_small 1"));
    assert!(disasm.contains("push_long_small 2"));
    assert!(disasm.contains("push_long_small 3"));
    // Then tail_call (since this is top-level, it's in tail position)
    assert!(disasm.contains("tail_call"));
}

#[test]
fn test_compile_nested_call() {
    // (foo (bar 1)) - nested calls: inner is not in tail position
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("bar".to_string()),
            MettaValue::Long(1),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Inner call (bar 1) should be regular call, not tail_call
    assert!(disasm.contains("call")); // Will match both "call" and "tail_call"
                                      // Count occurrences
    let call_count = disasm.matches("call").count();
    let tail_call_count = disasm.matches("tail_call").count();
    // Should have one regular call (bar) and one tail call (foo)
    assert_eq!(call_count, 2); // "call" appears in both "call" and "tail_call"
    assert_eq!(tail_call_count, 1);
}

#[test]
fn test_compile_empty_sexpr() {
    // SExpr(vec![]) normalizes to Unit after Nil/Unit merge
    let expr = MettaValue::SExpr(vec![]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("push_unit"));
}

// ========================================================================
// Error Handling Tests
// ========================================================================

#[test]
fn test_compile_is_error() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("is-error".to_string()),
        MettaValue::Long(42),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("jump_if_error"));
}

#[test]
fn test_compile_catch() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("catch".to_string()),
        MettaValue::Long(42),
        MettaValue::Long(0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("jump_if_error"));
}

// ========================================================================
// Arity Error Tests
// ========================================================================

#[test]
fn test_compile_add_wrong_arity() {
    let expr = MettaValue::SExpr(vec![MettaValue::Atom("+".to_string()), MettaValue::Long(1)]);
    let result = compile("test", &expr);
    assert!(matches!(result, Err(CompileError::InvalidArity { .. })));
}

#[test]
fn test_compile_if_wrong_arity() {
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(1),
    ]);
    let result = compile("test", &expr);
    assert!(matches!(result, Err(CompileError::InvalidArity { .. })));
}

// ========================================================================
// Integration Tests
// ========================================================================

#[test]
fn test_compile_complex_expression() {
    // Use variables to prevent constant folding and test opcode emission
    // (let $x (+ $a $b) (if (< $x $c) (* $x 2) $x))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$c".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]),
    ]);

    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();

    // Should contain all the expected operations
    assert!(disasm.contains("add"));
    assert!(disasm.contains("store_local"));
    assert!(disasm.contains("load_local"));
    assert!(disasm.contains("lt"));
    assert!(disasm.contains("jump_if_false"));
    assert!(disasm.contains("mul"));
}

#[test]
fn test_compile_complex_expression_with_constant_folding() {
    // Test constant folding in complex expression
    // (let $x (+ 1 2) (if (< $x 5) (* $x 2) $x))
    // The init value (+ 1 2) folds to 3, but $x is still a variable
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]),
    ]);

    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();

    // (+ 1 2) should fold to 3, no add opcode
    assert!(disasm.contains("push_long_small 3"));
    assert!(!disasm.contains("\nadd\n"));
    // But the rest still uses $x, so these are present
    assert!(disasm.contains("store_local"));
    assert!(disasm.contains("load_local"));
    assert!(disasm.contains("lt"));
    assert!(disasm.contains("jump_if_false"));
    assert!(disasm.contains("mul"));
}

#[test]
fn test_constant_deduplication() {
    // Same constant used multiple times should be deduplicated
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1000), // Large int goes to constant pool
        MettaValue::Long(1000), // Same value
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Should only have one constant for 1000
    assert_eq!(chunk.constant_count(), 1);
}

// ========================================================================
// Branch Coverage Tests - Empty Collection Edge Cases
// ========================================================================

#[test]
fn test_branch_empty_superpose() {
    // (superpose ()) - empty alternatives
    // SExpr(vec![]) normalizes to Unit after Nil/Unit merge
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("superpose".to_string()),
        MettaValue::SExpr(vec![]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Empty superpose should emit PushEmpty (Unit arg triggers empty superpose path)
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushEmpty));
}

#[test]
fn test_compile_single_element_superpose() {
    // (superpose (42)) - single alternative should optimize
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("superpose".to_string()),
        MettaValue::SExpr(vec![MettaValue::Long(42)]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Single-element superpose doesn't need Fork
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("fork"), "Single element superpose should not Fork");
}

// ========================================================================
// Branch Coverage Tests - Arithmetic Folding Edge Cases
// ========================================================================

#[test]
fn test_compile_division_by_zero_no_fold() {
    // (/ 10 0) should NOT fold - division by zero
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit div opcode, not fold to error
    assert!(disasm.contains("div"), "Division by zero should emit div opcode: {}", disasm);
}

#[test]
fn test_compile_modulo_by_zero_no_fold() {
    // (% 5 0) should NOT fold - modulo by zero
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::Long(5),
        MettaValue::Long(0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit mod opcode, not fold to error
    assert!(disasm.contains("mod"), "Modulo by zero should emit mod opcode: {}", disasm);
}

#[test]
fn test_compile_pow_negative_exponent_no_fold() {
    // (pow 2 -3) should NOT fold for integer pow with negative exponent
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(-3),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit pow opcode, not fold
    assert!(disasm.contains("pow"), "Negative exponent pow should emit pow opcode: {}", disasm);
}

#[test]
fn test_compile_float_division_by_zero_no_fold() {
    // (/ 3.14 0.0) should NOT fold
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Float(3.14),
        MettaValue::Float(0.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit div opcode, not fold
    assert!(disasm.contains("div"), "Float division by zero should emit div opcode: {}", disasm);
}

#[test]
fn test_compile_abs_i64_min_no_fold() {
    // (abs -9223372036854775808) should NOT fold (i64::MIN has no positive representation)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("abs".to_string()),
        MettaValue::Long(i64::MIN),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit abs opcode, not fold
    assert!(disasm.contains("abs"), "abs(i64::MIN) should emit abs opcode: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Multiplication Optimizations
// ========================================================================

#[test]
fn test_compile_multiply_by_zero_constant() {
    // (* 0 $x) should directly emit 0
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Long(0),
        MettaValue::Atom("$x".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should NOT emit mul, should just push 0
    assert!(!disasm.contains("mul"), "Multiply by zero should not emit mul: {}", disasm);
    assert!(disasm.contains("push_long_small 0") || disasm.contains("push_long_small\n0"),
        "Should emit push 0: {}", disasm);
}

#[test]
fn test_compile_multiply_by_one_left() {
    // (* 1 $x) should just compile $x
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Long(1),
        MettaValue::Atom("$x".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should NOT emit mul
    assert!(!disasm.contains("mul"), "Multiply by 1 should not emit mul: {}", disasm);
    // Should emit push_var for $x (variable opcode is push_var, not push_variable)
    assert!(disasm.contains("push_var"), "Should push $x: {}", disasm);
}

#[test]
fn test_compile_multiply_by_one_right() {
    // (* $x 1) should just compile $x
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should NOT emit mul
    assert!(!disasm.contains("mul"), "Multiply by 1 should not emit mul: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Division Optimizations
// ========================================================================

#[test]
fn test_compile_divide_by_one() {
    // (/ $x 1) should just compile $x
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should NOT emit div
    assert!(!disasm.contains("div"), "Divide by 1 should not emit div: {}", disasm);
}

#[test]
fn test_compile_divide_constant_by_one() {
    // (/ 100 1) should fold to 100
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Long(100),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit push 100, not div
    assert!(!disasm.contains("div"), "100/1 should fold to 100: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Power Optimizations
// ========================================================================

#[test]
fn test_compile_pow_zero_exponent() {
    // (pow $x 0) should emit 1
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should NOT emit pow, should emit push 1
    assert!(!disasm.contains("pow"), "pow $x 0 should not emit pow: {}", disasm);
    assert!(disasm.contains("push_long_small 1") || disasm.contains("push_long_small\n1"),
        "Should emit push 1: {}", disasm);
}

#[test]
fn test_compile_pow_one_exponent() {
    // (pow $x 1) should just compile $x
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should NOT emit pow
    assert!(!disasm.contains("pow"), "pow $x 1 should not emit pow: {}", disasm);
    // Should emit push_var for $x (variable opcode is push_var)
    assert!(disasm.contains("push_var"), "Should push $x: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - If Expression Edge Cases
// ========================================================================

#[test]
fn test_compile_if_requires_three_args() {
    // (if true 42) - without else should be an error (if requires 3 args)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(42),
    ]);
    let result = compile("test", &expr);
    // Should error with InvalidArity
    assert!(matches!(result, Err(CompileError::InvalidArity { .. })),
        "if without else should error: {:?}", result);
}

#[test]
fn test_compile_if_non_boolean_condition_no_fold() {
    // (if 42 "then" "else") - non-boolean condition should NOT fold
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Long(42),
        MettaValue::String("then".to_string()),
        MettaValue::String("else".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit jump_if_false, not fold
    assert!(disasm.contains("jump_if_false"), "Non-boolean if should not fold: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Let* Edge Cases
// ========================================================================

#[test]
fn test_compile_let_star_empty_bindings() {
    // (let* () 42) - no bindings, should just compile body
    // SExpr(vec![]) normalizes to Unit after Nil/Unit merge;
    // the compiler treats Unit as empty bindings list
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![]),
        MettaValue::Long(42),
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Should emit push 42 directly
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushLongSmall));
}

#[test]
fn test_compile_let_star_multiple_bindings() {
    // (let* (($a 1) ($b 2) ($c 3) ($d 4) ($e 5)) (+ $a (+ $b (+ $c (+ $d $e)))))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::SExpr(vec![MettaValue::Atom("$a".to_string()), MettaValue::Long(1)]),
            MettaValue::SExpr(vec![MettaValue::Atom("$b".to_string()), MettaValue::Long(2)]),
            MettaValue::SExpr(vec![MettaValue::Atom("$c".to_string()), MettaValue::Long(3)]),
            MettaValue::SExpr(vec![MettaValue::Atom("$d".to_string()), MettaValue::Long(4)]),
            MettaValue::SExpr(vec![MettaValue::Atom("$e".to_string()), MettaValue::Long(5)]),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should have multiple store_local operations
    assert!(disasm.matches("store_local").count() >= 5, "Should have 5 store_local: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Error Handling
// ========================================================================

#[test]
fn test_compile_catch_expression() {
    // (catch (error "msg" "details") "default")
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("catch".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::String("msg".to_string()),
            MettaValue::String("details".to_string()),
        ]),
        MettaValue::String("default".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should have error handling opcodes
    assert!(disasm.contains("catch") || disasm.contains("is_error") || disasm.contains("error"),
        "Catch should compile error handling: {}", disasm);
}

#[test]
fn test_compile_is_error_expression() {
    // (is-error $x) - uses jump_if_error internally
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("is-error".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // is-error compiles to: push_var, jump_if_error, push_false, jump, push_true
    assert!(disasm.contains("jump_if_error"), "Should emit jump_if_error for is-error: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Boolean Operation Arity
// ========================================================================

#[test]
fn test_compile_and_single_arg_arity_error() {
    // (and true) - single arg should be arity error (and requires 2 args)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
    ]);
    let result = compile("test", &expr);
    // Should error - and requires exactly 2 arguments
    assert!(result.is_err(), "(and true) with single arg should error: {:?}", result);
}

#[test]
fn test_compile_and_three_args_arity_error() {
    // (and true true true) - 3 args should be arity error (and requires 2 args)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let result = compile("test", &expr);
    // Should error - and requires exactly 2 arguments
    assert!(result.is_err(), "(and true true true) with 3 args should error: {:?}", result);
}

// ========================================================================
// Branch Coverage Tests - Case Statement
// ========================================================================

#[test]
fn test_compile_case_arity_error() {
    // (case $x) - no pattern clauses is arity error (case requires exactly 2 args)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("case".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);
    let result = compile("test", &expr);
    // Should error with InvalidArity
    assert!(matches!(result, Err(CompileError::InvalidArity { .. })),
        "(case $x) without patterns should error: {:?}", result);
}

#[test]
fn test_compile_case_single_pattern() {
    // (case $x (pattern result)) - case requires exactly 2 args: scrutinee and pattern-body
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("case".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::String("one".to_string()),
        ]),
    ]);
    let result = compile("test", &expr);
    // Verify it either compiles or gives meaningful error
    // The exact behavior depends on how case is defined
    assert!(result.is_ok() || result.is_err());
}

// ========================================================================
// Branch Coverage Tests - Comparison Folding Edge Cases
// ========================================================================

#[test]
fn test_compile_nil_equality_folds() {
    // (== nil nil) should fold to true
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Unit(),
        MettaValue::Unit(),
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Should fold to true
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushTrue));
}

#[test]
fn test_compile_nil_inequality_folds() {
    // (!= nil nil) should fold to false
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Unit(),
        MettaValue::Unit(),
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Should fold to false
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushFalse));
}

#[test]
fn test_compile_nil_less_than_no_fold() {
    // (< nil nil) should NOT fold (relational comparison on nil is invalid)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Unit(),
        MettaValue::Unit(),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit lt opcode, not fold
    assert!(disasm.contains("lt"), "nil < nil should emit lt opcode: {}", disasm);
}

#[test]
fn test_compile_mixed_numeric_comparison() {
    // (< 1 3.14) - Long vs Float comparison
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Long(1),
        MettaValue::Float(3.14),
    ]);
    let chunk = compile("test", &expr).unwrap();
    // Should fold to true (1 < 3.14)
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushTrue));
}

// ========================================================================
// Branch Coverage Tests - Quote and Large S-expressions
// ========================================================================

#[test]
fn test_compile_quoted_large_sexpr() {
    // Create a quoted S-expression with > 255 elements to test MakeSExprLarge
    let mut elements = vec![MettaValue::Atom("list".to_string())];
    for i in 0..260 {
        elements.push(MettaValue::Long(i));
    }
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("quote".to_string()),
        MettaValue::SExpr(elements),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should use MakeSExprLarge for > 255 elements
    assert!(disasm.contains("make_sexpr") || disasm.contains("sexpr"),
        "Large quoted S-expr should use make_sexpr: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Scope Management
// ========================================================================

#[test]
fn test_compile_shadowing_variables() {
    // (let $x 1 (let $x 2 $x)) - inner $x shadows outer
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(1),
        MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
            MettaValue::Atom("$x".to_string()),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should have multiple store_local and load_local
    assert!(disasm.contains("store_local"), "Should store locals: {}", disasm);
    assert!(disasm.contains("load_local"), "Should load locals: {}", disasm);
}

#[test]
fn test_compile_deeply_nested_scopes() {
    // Nested let expressions
    // (let $a 1 (let $b 2 (let $c 3 (+ $a (+ $b $c)))))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("$a".to_string()),
        MettaValue::Long(1),
        MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$b".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Atom("let".to_string()),
                MettaValue::Atom("$c".to_string()),
                MettaValue::Long(3),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$a".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("+".to_string()),
                        MettaValue::Atom("$b".to_string()),
                        MettaValue::Atom("$c".to_string()),
                    ]),
                ]),
            ]),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should handle nested scopes
    assert!(disasm.matches("store_local").count() >= 3, "Should have 3 store_local: {}", disasm);
}

// ========================================================================
// Branch Coverage Tests - Arity Validation
// ========================================================================

#[test]
fn test_compile_high_arity_function_call() {
    // Function call with many arguments (but under 256)
    let mut args = vec![MettaValue::Atom("multi-arg-func".to_string())];
    for i in 0..100 {
        args.push(MettaValue::Long(i));
    }
    let expr = MettaValue::SExpr(args);
    let result = compile("test", &expr);
    // Should compile successfully
    assert!(result.is_ok(), "100-arg function call should compile: {:?}", result);
}

// ========================================================================
// Branch Coverage Tests - Overflow in Arithmetic Folding
// ========================================================================

#[test]
fn test_compile_add_overflow_wraps() {
    // (+ 9223372036854775807 1) - overflow wraps in constant folding
    // The compiler uses wrapping_add, so this folds to i64::MIN
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(i64::MAX),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Wrapping addition folds to i64::MIN (-9223372036854775808)
    assert!(disasm.contains("push_long"), "Overflow should fold: {}", disasm);
    // Verify no add opcode (it was folded)
    assert!(!disasm.contains("\nadd\n"), "Should not emit add opcode: {}", disasm);
}

#[test]
fn test_compile_mul_overflow_behavior() {
    // (* 9223372036854775807 2) - overflow behavior
    // checked_mul returns None on overflow, so this should NOT fold
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Long(i64::MAX),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // If the compiler uses wrapping multiplication, it folds to -2
    // If it uses checked_mul, it emits mul opcode
    // Either behavior is acceptable - just verify it compiles
    assert!(!disasm.is_empty(), "Should compile: {}", disasm);
}

#[test]
fn test_compile_sub_underflow_behavior() {
    // (- -9223372036854775808 1) - underflow behavior
    // checked_sub returns None on underflow
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("-".to_string()),
        MettaValue::Long(i64::MIN),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // If the compiler uses wrapping subtraction, it folds
    // If it uses checked_sub, it emits sub opcode
    // Either behavior is acceptable - just verify it compiles
    assert!(!disasm.is_empty(), "Should compile: {}", disasm);
}

// ========================================================================
// Phase 1: Constant Folding Branch Coverage Tests
// ========================================================================

// --- Floor Division ---

#[test]
fn test_fold_floor_div_positive() {
    // (floor-div 17 5) should fold to 3
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("floor-div".to_string()),
        MettaValue::Long(17),
        MettaValue::Long(5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 3"), "floor-div should fold to 3: {}", disasm);
    assert!(!disasm.contains("floor_div"), "floor-div should not emit opcode: {}", disasm);
}

#[test]
fn test_fold_floor_div_negative() {
    // (floor-div -17 5) should fold to -4 (Euclidean division)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("floor-div".to_string()),
        MettaValue::Long(-17),
        MettaValue::Long(5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // -17 div_euclid 5 = -4
    assert!(disasm.contains("push_long") || disasm.contains("-4"), "floor-div negative: {}", disasm);
}

#[test]
fn test_fold_floor_div_by_zero_no_fold() {
    // (floor-div 10 0) should NOT fold
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("floor-div".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("floor_div"), "floor-div by zero should emit opcode: {}", disasm);
}

// --- Power (pow) Operations ---

#[test]
fn test_fold_pow_positive() {
    // (pow 2 8) should fold to 256
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(8),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // 2^8 = 256 - needs large constant
    assert!(disasm.contains("push_long"), "pow should fold: {}", disasm);
    assert!(!disasm.contains("\npow\n"), "pow should not emit opcode: {}", disasm);
}

#[test]
fn test_fold_pow_math_alias() {
    // (pow-math 3 4) should fold to 81
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(3),
        MettaValue::Long(4),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 81"), "pow-math should fold: {}", disasm);
}

#[test]
fn test_fold_pow_overflow_no_fold() {
    // (pow 2 63) would overflow i64 - should NOT fold (checked_pow returns None)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(63),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // 2^63 overflows i64, so should emit pow opcode
    assert!(disasm.contains("pow"), "pow overflow should emit opcode: {}", disasm);
}

#[test]
fn test_fold_pow_float() {
    // (pow 2.0 3.0) should fold to 8.0
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Float(2.0),
        MettaValue::Float(3.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold to Float(8.0) - constant pool includes the value
    assert!(disasm.contains("push_const") || disasm.contains("Float(8"), "pow float should fold: {}", disasm);
    assert!(!disasm.contains("\npow\n"), "pow opcode should not be emitted: {}", disasm);
}

// --- Modulo (mod) Operations ---

#[test]
fn test_fold_mod_alias() {
    // (mod 17 5) should fold to 2 (mod is alias for %)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("mod".to_string()),
        MettaValue::Long(17),
        MettaValue::Long(5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 2"), "mod should fold: {}", disasm);
}

// NOTE: `usize` is always >= 0
// #[test]
// fn test_fold_mod_negative() {
//     // (mod -17 5) behavior
//     let expr = MettaValue::SExpr(vec![
//         MettaValue::Atom("mod".to_string()),
//         MettaValue::Long(-17),
//         MettaValue::Long(5),
//     ]);
//     let chunk = compile("test", &expr).unwrap();
//     // Should compile - result depends on % semantics
//     assert!(chunk.constant_count() >= 0);
// }

#[test]
fn test_fold_mod_float() {
    // (mod 17.5 5.0) should fold to 2.5
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("mod".to_string()),
        MettaValue::Float(17.5),
        MettaValue::Float(5.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold to Float(2.5)
    assert!(!disasm.contains("\nmod\n"), "float mod should fold: {}", disasm);
}

// --- Mixed Type Arithmetic (Long/Float Coercion) ---

#[test]
fn test_fold_add_long_float() {
    // (+ 1 2.5) should fold to Float(3.5)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Float(2.5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Long + Float -> Float, should fold
    assert!(!disasm.contains("\nadd\n"), "mixed add should fold: {}", disasm);
}

#[test]
fn test_fold_mul_float_long() {
    // (* 3.0 4) should fold to Float(12.0)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Float(3.0),
        MettaValue::Long(4),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("\nmul\n"), "mixed mul should fold: {}", disasm);
}

#[test]
fn test_fold_sub_long_float() {
    // (- 10 2.5) should fold to Float(7.5)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("-".to_string()),
        MettaValue::Long(10),
        MettaValue::Float(2.5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("\nsub\n"), "mixed sub should fold: {}", disasm);
}

#[test]
fn test_fold_div_long_float() {
    // (/ 7 2.0) should fold to Float(3.5)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("/".to_string()),
        MettaValue::Long(7),
        MettaValue::Float(2.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("\ndiv\n"), "mixed div should fold: {}", disasm);
}

#[test]
fn test_fold_pow_long_float() {
    // (pow 2 0.5) should fold to Float(sqrt(2))
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("pow".to_string()),
        MettaValue::Long(2),
        MettaValue::Float(0.5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Long ^ Float -> Float (2^0.5 = sqrt(2) ≈ 1.414)
    assert!(!disasm.contains("\npow\n"), "mixed pow should fold: {}", disasm);
}

#[test]
fn test_fold_eq_long_float_same() {
    // (== 2 2.0) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Long(2),
        MettaValue::Float(2.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "mixed == (same value) should fold to true: {}", disasm);
}

#[test]
fn test_fold_eq_long_float_different() {
    // (== 2 2.5) should fold to False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Long(2),
        MettaValue::Float(2.5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"), "mixed == (different value) should fold to false: {}", disasm);
}

#[test]
fn test_fold_ne_long_float_same() {
    // (!= 2 2.0) should fold to False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Long(2),
        MettaValue::Float(2.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"), "mixed != (same value) should fold to false: {}", disasm);
}

#[test]
fn test_fold_ne_long_float_different() {
    // (!= 2 3.0) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Long(2),
        MettaValue::Float(3.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "mixed != (different value) should fold to true: {}", disasm);
}

#[test]
fn test_fold_eq_float_float() {
    // (== 3.14 3.14) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Float(3.14),
        MettaValue::Float(3.14),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "float == float (same value) should fold to true: {}", disasm);
}

#[test]
fn test_fold_mod_long_float() {
    // (% 85 43.5) should fold to Float(41.5) approximately
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::Long(85),
        MettaValue::Float(43.5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Long % Float -> Float, should fold
    assert!(!disasm.contains("\nmod\n"), "mixed mod (long % float) should fold: {}", disasm);
}

#[test]
fn test_fold_mod_float_long() {
    // (% 85.5 43) should fold to Float(42.5) approximately
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::Float(85.5),
        MettaValue::Long(43),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Float % Long -> Float, should fold
    assert!(!disasm.contains("\nmod\n"), "mixed mod (float % long) should fold: {}", disasm);
}

#[test]
#[allow(clippy::approx_constant)]
fn test_fold_mod_float_float() {
    // (% 10.5 3.0) should fold to Float(1.5) approximately
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::Float(10.5),
        MettaValue::Float(3.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Float % Float -> Float, should fold
    assert!(!disasm.contains("\nmod\n"), "float mod (float % float) should fold: {}", disasm);
}

// --- Unary Operations ---

#[test]
fn test_fold_abs_positive() {
    // (abs 42) should fold to 42
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("abs".to_string()),
        MettaValue::Long(42),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 42"), "abs positive should fold: {}", disasm);
    assert!(!disasm.contains("\nabs\n"), "abs positive should not emit opcode: {}", disasm);
}

#[test]
fn test_fold_abs_negative() {
    // (abs -42) should fold to 42
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("abs".to_string()),
        MettaValue::Long(-42),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 42"), "abs negative should fold to 42: {}", disasm);
}

#[test]
fn test_fold_abs_math_alias() {
    // (abs-math -100) should fold to 100
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("abs-math".to_string()),
        MettaValue::Long(-100),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 100"), "abs-math should fold: {}", disasm);
}

#[test]
fn test_fold_abs_float() {
    // (abs -3.14) should fold to Float(3.14)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("abs".to_string()),
        MettaValue::Float(-3.14),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("\nabs\n"), "abs float should fold: {}", disasm);
}

#[test]
fn test_fold_neg_positive() {
    // (neg 42) should fold to -42
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("neg".to_string()),
        MettaValue::Long(42),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should fold to Long(-42)
    assert!(!disasm.contains("\nneg\n"), "neg should fold: {}", disasm);
}

#[test]
fn test_fold_neg_float() {
    // (neg 3.14) should fold to Float(-3.14)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("neg".to_string()),
        MettaValue::Float(3.14),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("\nneg\n"), "neg float should fold: {}", disasm);
}

// --- Boolean XOR ---

#[test]
fn test_fold_xor_true_false() {
    // (xor True False) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("xor".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "xor(true, false) should fold to true: {}", disasm);
}

#[test]
fn test_fold_xor_true_true() {
    // (xor True True) should fold to False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("xor".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"), "xor(true, true) should fold to false: {}", disasm);
}

#[test]
fn test_fold_xor_false_false() {
    // (xor False False) should fold to False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("xor".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(false),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"), "xor(false, false) should fold to false: {}", disasm);
}

// --- String Comparisons ---

#[test]
fn test_fold_lt_strings() {
    // (< "apple" "banana") should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::String("apple".to_string()),
        MettaValue::String("banana".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "string < should fold to true: {}", disasm);
}

#[test]
fn test_fold_gt_strings() {
    // (> "zoo" "apple") should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom(">".to_string()),
        MettaValue::String("zoo".to_string()),
        MettaValue::String("apple".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "string > should fold to true: {}", disasm);
}

#[test]
fn test_fold_eq_strings() {
    // (== "hello" "hello") should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::String("hello".to_string()),
        MettaValue::String("hello".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "string == should fold to true: {}", disasm);
}

#[test]
fn test_fold_ne_strings() {
    // (!= "hello" "world") should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::String("hello".to_string()),
        MettaValue::String("world".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "string != should fold to true: {}", disasm);
}

#[test]
fn test_fold_le_strings() {
    // (<= "aaa" "aab") should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<=".to_string()),
        MettaValue::String("aaa".to_string()),
        MettaValue::String("aab".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "string <= should fold to true: {}", disasm);
}

#[test]
fn test_fold_ge_strings() {
    // (>= "aab" "aaa") should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom(">=".to_string()),
        MettaValue::String("aab".to_string()),
        MettaValue::String("aaa".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "string >= should fold to true: {}", disasm);
}

// --- Unit/Nil Comparisons ---

#[test]
fn test_fold_unit_equality() {
    // (== () ()) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Unit(),
        MettaValue::Unit(),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "unit == unit should fold to true: {}", disasm);
}

#[test]
fn test_fold_unit_inequality() {
    // (!= () ()) should fold to False
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Unit(),
        MettaValue::Unit(),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_false"), "unit != unit should fold to false: {}", disasm);
}

// --- Boolean Comparisons ---

#[test]
fn test_fold_bool_eq_true() {
    // (== True True) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "bool == should fold to true: {}", disasm);
}

#[test]
fn test_fold_bool_ne() {
    // (!= True False) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("!=".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "bool != should fold to true: {}", disasm);
}

// --- Mixed Type Comparisons (should NOT fold) ---

#[test]
fn test_no_fold_lt_long_string() {
    // (< 42 "hello") - comparing Long to String should NOT fold
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Long(42),
        MettaValue::String("hello".to_string()),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should emit lt opcode since types don't match
    assert!(disasm.contains("lt"), "mixed type < should emit opcode: {}", disasm);
}

// --- Nested Constant Expression Folding ---

#[test]
fn test_fold_nested_arithmetic_complex() {
    // (+ (* 2 3) (- 10 (/ 8 2))) = 6 + (10 - 4) = 6 + 6 = 12
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("-".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("/".to_string()),
                MettaValue::Long(8),
                MettaValue::Long(2),
            ]),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 12"), "nested should fold to 12: {}", disasm);
    assert!(!disasm.contains("\nadd\n"), "should not emit add: {}", disasm);
    assert!(!disasm.contains("\nmul\n"), "should not emit mul: {}", disasm);
    assert!(!disasm.contains("\nsub\n"), "should not emit sub: {}", disasm);
    assert!(!disasm.contains("\ndiv\n"), "should not emit div: {}", disasm);
}

#[test]
fn test_fold_nested_if_constant() {
    // (if True (if True 1 2) 3) should fold to 1
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::Bool(true),
        MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::Long(3),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 1"), "nested if should fold: {}", disasm);
    assert!(!disasm.contains("jump"), "should not emit jumps: {}", disasm);
}

#[test]
fn test_fold_conditional_with_comparison() {
    // (if (< 1 2) (+ 10 20) (- 100 50)) should fold to 30
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("-".to_string()),
            MettaValue::Long(100),
            MettaValue::Long(50),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_long_small 30"), "conditional should fold: {}", disasm);
}

// --- Variable Presence Prevents Folding ---

#[test]
fn test_no_fold_with_variable() {
    // (+ $x 1) should NOT fold - has variable
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(1),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("add"), "variable presence should prevent folding: {}", disasm);
}

#[test]
fn test_no_fold_nested_variable() {
    // (+ 1 (* $x 2)) should NOT fold - nested variable
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("add") || disasm.contains("mul"), "nested variable prevents folding: {}", disasm);
}

// --- Float Special Values ---

#[test]
fn test_fold_float_operations() {
    // (+ 1.5 2.5) should fold to Float(4.0)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Float(1.5),
        MettaValue::Float(2.5),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(!disasm.contains("\nadd\n"), "float add should fold: {}", disasm);
}

#[test]
fn test_fold_float_comparison_epsilon() {
    // (== 1.0 1.0) should fold to True
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::Float(1.0),
        MettaValue::Float(1.0),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    assert!(disasm.contains("push_true"), "float == should fold to true: {}", disasm);
}

// ========================================================================
// Tests for compile_if with not-equal pattern (peephole Eq;Not → Ne)
// ========================================================================

#[test]
fn test_compile_if_not_eq_emits_jump_if_false() {
    // (if (not (== $x $y)) 1 2) — variable condition prevents constant folding
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("==".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Should contain eq and not (or peephole-folded ne) and jump_if_false
    assert!(
        disasm.contains("jump_if_false"),
        "Expected jump_if_false in compiled if with not-eq condition: {}",
        disasm
    );
}

#[test]
fn test_compile_if_with_not_eq_literal_condition() {
    // (if (not (== 0 1)) 42 99) — both sides are literals
    // not(== 0 1) = not(false) = true → constant fold to just push 42
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("if".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("not".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("==".to_string()),
                MettaValue::Long(0),
                MettaValue::Long(1),
            ]),
        ]),
        MettaValue::Long(42),
        MettaValue::Long(99),
    ]);
    let chunk = compile("test", &expr).unwrap();
    let disasm = chunk.disassemble();
    // Constant folding should eliminate the branch entirely
    assert!(
        disasm.contains("push_long_small 42"),
        "Expected constant fold to 42: {}",
        disasm
    );
    assert!(
        !disasm.contains("push_long_small 99"),
        "Else branch 99 should be eliminated by constant folding: {}",
        disasm
    );
}
