//! Unit tests for the bytecode compiler.

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

use super::error::CompileError;
use super::{compile, Compiler};

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
    let chunk = compile("test", &MettaValue::Nil).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushNil));
}

#[test]
fn test_compile_unit() {
    let chunk = compile("test", &MettaValue::Unit).unwrap();
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
    assert_eq!(chunk.get_constant(0), Some(&MettaValue::String("hello".to_string())));
}

#[test]
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
    assert_eq!(chunk.get_constant(0), Some(&MettaValue::Atom("foo".to_string())));
}

#[test]
fn test_compile_variable() {
    let chunk = compile("test", &MettaValue::Atom("$x".to_string())).unwrap();
    assert_eq!(chunk.read_opcode(0), Some(Opcode::PushVariable));
    assert_eq!(chunk.get_constant(0), Some(&MettaValue::Atom("$x".to_string())));
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
        MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
    ]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("get_arity"));
}

#[test]
fn test_compile_empty() {
    // MeTTa semantics: (empty) returns NO results, equivalent to Fail
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("empty".to_string()),
    ]);
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
    let expr = MettaValue::SExpr(vec![]);
    let chunk = compile("test", &expr).unwrap();
    assert!(chunk.disassemble().contains("push_empty"));
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
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
    ]);
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
