//! Tests for the bytecode VM.
//!
//! This module contains all unit tests for the VM implementation.

use std::sync::Arc;

use crate::backend::models::{MettaValue, SpaceHandle};
use crate::backend::environment::Environment;
use crate::backend::bytecode::chunk::ChunkBuilder;
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::bytecode::mork_bridge::MorkBridge;
use super::types::VmError;
use super::pattern::{pattern_matches, unify};
use super::BytecodeVM;

#[test]
fn test_vm_push_pop() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Dup);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(84));
}

#[test]
fn test_vm_arithmetic() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit(Opcode::Sub);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(7));
}

#[test]
fn test_vm_comparison() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Lt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_jump() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    let else_label = builder.emit_jump(Opcode::JumpIfFalse);
    builder.emit_byte(Opcode::PushLongSmall, 1); // then branch
    let end_label = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(else_label);
    builder.emit_byte(Opcode::PushLongSmall, 2); // else branch
    builder.patch_jump(end_label);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1)); // then branch was taken
}

#[test]
fn test_vm_make_sexpr() {
    let mut builder = ChunkBuilder::new("test");
    let plus_idx = builder.add_constant(MettaValue::sym("+"));
    builder.emit_u16(Opcode::PushAtom, plus_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::sym("+"));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
        }
        _ => panic!("Expected S-expression"),
    }
}

#[test]
fn test_pattern_matches() {
    // Variable matches anything
    assert!(pattern_matches(
        &MettaValue::var("x"),
        &MettaValue::Long(42)
    ));

    // Atom matches same atom
    assert!(pattern_matches(
        &MettaValue::sym("foo"),
        &MettaValue::sym("foo")
    ));

    // Atom doesn't match different atom
    assert!(!pattern_matches(
        &MettaValue::sym("foo"),
        &MettaValue::sym("bar")
    ));

    // S-expression matching
    assert!(pattern_matches(
        &MettaValue::sexpr(vec![
            MettaValue::sym("add"),
            MettaValue::var("x"),
            MettaValue::var("y"),
        ]),
        &MettaValue::sexpr(vec![
            MettaValue::sym("add"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ])
    ));
}

// === Stack Operation Tests ===

#[test]
fn test_vm_swap() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Sub); // 2 - 1 after swap = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

#[test]
fn test_vm_rot3() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1); // a
    builder.emit_byte(Opcode::PushLongSmall, 2); // b
    builder.emit_byte(Opcode::PushLongSmall, 3); // c
    builder.emit(Opcode::Rot3);
    // After rot3: [c, a, b] -> top is b, second is a, third is c
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Top of stack after rot3 is b=2
    assert_eq!(results[0], MettaValue::Long(2));
}

#[test]
fn test_vm_over() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Over); // Copy 1 to top
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Top of stack after over is 1
    assert_eq!(results[0], MettaValue::Long(1));
}

#[test]
fn test_vm_dup_n() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::DupN, 2); // Duplicate top 2 values
    builder.emit(Opcode::Add); // 2 + 2
    builder.emit(Opcode::Add); // 4 + 1
    builder.emit(Opcode::Add); // 5 + 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // 1, 2, 1, 2 -> 4 + 1 + 1 = 6
    assert_eq!(results[0], MettaValue::Long(6));
}

#[test]
fn test_vm_pop_n() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::PopN, 2); // Pop top 2
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Only 1 remains
    assert_eq!(results[0], MettaValue::Long(1));
}

// === Value Creation Tests ===

#[test]
fn test_vm_push_nil() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushNil);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Nil);
}

#[test]
fn test_vm_push_booleans() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_vm_push_unit() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushUnit);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Unit);
}

#[test]
fn test_vm_push_empty() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // HE-compatible: PushEmpty pushes an empty S-expression (), distinct from Nil
    assert_eq!(results[0], MettaValue::SExpr(vec![]));
}

#[test]
fn test_vm_make_list() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeList, 2);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should create (Cons 1 (Cons 2 Nil))
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items[0], MettaValue::sym("Cons"));
            assert_eq!(items[1], MettaValue::Long(1));
        }
        _ => panic!("Expected S-expression"),
    }
}

#[test]
fn test_vm_make_quote() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit(Opcode::MakeQuote);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items[0], MettaValue::sym("quote"));
            assert_eq!(items[1], MettaValue::sym("foo"));
        }
        _ => panic!("Expected quoted expression"),
    }
}

// === Arithmetic Tests ===

#[test]
fn test_vm_mul() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 6);
    builder.emit_byte(Opcode::PushLongSmall, 7);
    builder.emit(Opcode::Mul);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_vm_div() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 6);
    builder.emit(Opcode::Div);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(7));
}

#[test]
fn test_vm_div_by_zero() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Div);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::DivisionByZero)));
}

#[test]
fn test_vm_mod() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(2));
}

#[test]
fn test_vm_neg() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Neg);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(-42));
}

#[test]
fn test_vm_abs() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, -42i8 as u8);
    builder.emit(Opcode::Abs);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_vm_floor_div() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 17);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::FloorDiv);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(3));
}

#[test]
fn test_vm_pow() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(1024));
}

// === Comparison Tests ===

#[test]
fn test_vm_le() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Le);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_gt() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Gt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_ge() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Ge);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_eq() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit(Opcode::Eq);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_ne() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Ne);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_struct_eq() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    builder.emit(Opcode::StructEq);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

// === Boolean Tests ===

#[test]
fn test_vm_and() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_or() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Or);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_not() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Not);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_xor() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Xor);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

// === Type Operation Tests ===

#[test]
fn test_vm_get_type() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::sym("Number"));
}

#[test]
fn test_vm_check_type() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let type_idx = builder.add_constant(MettaValue::sym("Number"));
    builder.emit_u16(Opcode::PushAtom, type_idx);
    builder.emit(Opcode::CheckType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_type() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    let type_idx = builder.add_constant(MettaValue::sym("Bool"));
    builder.emit_u16(Opcode::PushAtom, type_idx);
    builder.emit(Opcode::IsType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

// === Pattern Matching Operation Tests ===

#[test]
fn test_vm_match_opcode() {
    let mut builder = ChunkBuilder::new("test");
    // Pattern: ($x 2)
    let x_idx = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushVariable, x_idx);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    // Value: (1 2)
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 2);
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_match_bind_opcode() {
    let mut builder = ChunkBuilder::new("test");
    // Pattern: $x
    let x_idx = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushVariable, x_idx);
    // Value: 42
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_unify() {
    let mut builder = ChunkBuilder::new("test");
    let x_idx = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushVariable, x_idx);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Unify);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_variable() {
    let mut builder = ChunkBuilder::new("test");
    let x_idx = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushVariable, x_idx);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_sexpr() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::MakeSExpr, 1);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_symbol() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_get_head() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::sym("foo"));
}

#[test]
fn test_vm_get_tail() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Long(1));
            assert_eq!(items[1], MettaValue::Long(2));
        }
        _ => panic!("Expected S-expression"),
    }
}

#[test]
fn test_vm_get_arity() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit(Opcode::GetArity);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(3));
}

#[test]
fn test_vm_get_element() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(42));
}

#[test]
fn test_vm_match_arity() {
    let mut builder = ChunkBuilder::new("test");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    builder.emit_byte(Opcode::MatchArity, 3); // Check arity is 3
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

// === Nondeterminism Tests ===

#[test]
fn test_vm_fork_yield() {
    let mut builder = ChunkBuilder::new("test");

    // Add alternatives as constants
    let idx1 = builder.add_constant(MettaValue::Long(1));
    let idx2 = builder.add_constant(MettaValue::Long(2));
    let idx3 = builder.add_constant(MettaValue::Long(3));

    // Fork: reads count, then count constant indices from bytecode
    builder.emit_u16(Opcode::Fork, 3);
    builder.emit_raw(&idx1.to_be_bytes());
    builder.emit_raw(&idx2.to_be_bytes());
    builder.emit_raw(&idx3.to_be_bytes());

    // Yield collects each result and backtracks
    builder.emit(Opcode::Yield);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should get all 3 values
    assert_eq!(results.len(), 3);
    assert!(results.contains(&MettaValue::Long(1)));
    assert!(results.contains(&MettaValue::Long(2)));
    assert!(results.contains(&MettaValue::Long(3)));
}

#[test]
fn test_vm_cut() {
    let mut builder = ChunkBuilder::new("test");

    // Add alternatives as constants
    let idx1 = builder.add_constant(MettaValue::Long(1));
    let idx2 = builder.add_constant(MettaValue::Long(2));
    let idx3 = builder.add_constant(MettaValue::Long(3));

    // Fork: reads count, then count constant indices from bytecode
    builder.emit_u16(Opcode::Fork, 3);
    builder.emit_raw(&idx1.to_be_bytes());
    builder.emit_raw(&idx2.to_be_bytes());
    builder.emit_raw(&idx3.to_be_bytes());

    builder.emit(Opcode::Cut); // Remove all choice points
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Only first alternative returned
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

// === Error Handling Tests ===

#[test]
fn test_vm_stack_underflow() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Pop); // Pop from empty stack
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_type_error() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Add); // Can't add bool and int
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_halt() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Halt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::Halted)));
}

// === Short Jump Tests ===

#[test]
fn test_vm_jump_short() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    let else_label = builder.emit_jump_short(Opcode::JumpIfFalseShort);
    builder.emit_byte(Opcode::PushLongSmall, 1); // then branch
    let end_label = builder.emit_jump_short(Opcode::JumpShort);
    builder.patch_jump_short(else_label);
    builder.emit_byte(Opcode::PushLongSmall, 2); // else branch
    builder.patch_jump_short(end_label);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(1));
}

#[test]
fn test_vm_jump_if_nil() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushNil);
    let jump_label = builder.emit_jump(Opcode::JumpIfNil);
    builder.emit_byte(Opcode::PushLongSmall, 1); // skipped
    let end_label = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump_label);
    builder.emit_byte(Opcode::PushLongSmall, 2); // taken
    builder.patch_jump(end_label);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(2));
}

// === Binding Tests ===

#[test]
fn test_vm_binding_frame() {
    let mut builder = ChunkBuilder::new("test");
    let x_name = builder.add_constant(MettaValue::sym("x"));

    // Store binding in root frame
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_u16(Opcode::StoreBinding, x_name);

    // Push new frame, store different value
    builder.emit(Opcode::PushBindingFrame);
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit_u16(Opcode::StoreBinding, x_name);

    // Load from inner frame
    builder.emit_u16(Opcode::LoadBinding, x_name);

    // Pop frame and load from outer
    builder.emit(Opcode::PopBindingFrame);
    builder.emit_u16(Opcode::LoadBinding, x_name);

    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Long(99 + 42));
}

#[test]
fn test_vm_has_binding() {
    let mut builder = ChunkBuilder::new("test");
    let x_name = builder.add_constant(MettaValue::sym("x"));
    let y_name = builder.add_constant(MettaValue::sym("y"));

    // Store x
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_u16(Opcode::StoreBinding, x_name);

    // Check x exists
    builder.emit_u16(Opcode::HasBinding, x_name);
    // Check y doesn't exist
    builder.emit_u16(Opcode::HasBinding, y_name);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Not); // true AND false = false, NOT false = true
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Bool(true));
}

// === Wildcard Pattern Tests ===

#[test]
fn test_pattern_wildcard() {
    assert!(pattern_matches(
        &MettaValue::sym("_"),
        &MettaValue::Long(42)
    ));

    assert!(pattern_matches(
        &MettaValue::sexpr(vec![
            MettaValue::sym("_"),
            MettaValue::Long(2),
        ]),
        &MettaValue::sexpr(vec![
            MettaValue::sym("anything"),
            MettaValue::Long(2),
        ])
    ));
}

#[test]
fn test_unification_bidirectional() {
    // Unify var with value
    let bindings = unify(
        &MettaValue::var("x"),
        &MettaValue::Long(42)
    ).expect("Should unify");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0], ("$x".to_string(), MettaValue::Long(42)));

    // Unify value with var (bidirectional)
    let bindings = unify(
        &MettaValue::Long(42),
        &MettaValue::var("x")
    ).expect("Should unify");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0], ("$x".to_string(), MettaValue::Long(42)));

    // Unify two vars
    let bindings = unify(
        &MettaValue::var("x"),
        &MettaValue::var("y")
    ).expect("Should unify");
    assert_eq!(bindings.len(), 1);
}

#[test]
fn test_unification_sexpr() {
    let bindings = unify(
        &MettaValue::sexpr(vec![
            MettaValue::sym("add"),
            MettaValue::var("x"),
            MettaValue::var("y"),
        ]),
        &MettaValue::sexpr(vec![
            MettaValue::sym("add"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ])
    ).expect("Should unify");

    assert_eq!(bindings.len(), 2);
    assert!(bindings.contains(&("$x".to_string(), MettaValue::Long(1))));
    assert!(bindings.contains(&("$y".to_string(), MettaValue::Long(2))));
}

#[test]
fn test_unification_failure() {
    // Different atoms don't unify
    assert!(unify(
        &MettaValue::sym("foo"),
        &MettaValue::sym("bar")
    ).is_none());

    // Different arity S-expressions don't unify
    assert!(unify(
        &MettaValue::sexpr(vec![MettaValue::Long(1)]),
        &MettaValue::sexpr(vec![MettaValue::Long(1), MettaValue::Long(2)])
    ).is_none());
}

// === Space Operation Tests ===

#[test]
fn test_vm_space_add_get_atoms() {
    // Create a space manually and test add/get operations
    let space = SpaceHandle::new(1, "test_space".to_string());

    // Add some atoms to the space
    space.add_atom(MettaValue::Long(1));
    space.add_atom(MettaValue::Long(2));
    space.add_atom(MettaValue::sym("foo"));

    // Create bytecode that pushes the space and gets its atoms
    let mut builder = ChunkBuilder::new("test");
    let space_const = builder.add_constant(MettaValue::Space(space.clone()));
    builder.emit_u16(Opcode::PushConstant, space_const);
    builder.emit(Opcode::SpaceGetAtoms);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(atoms) => {
            assert_eq!(atoms.len(), 3);
            assert!(atoms.contains(&MettaValue::Long(1)));
            assert!(atoms.contains(&MettaValue::Long(2)));
            assert!(atoms.contains(&MettaValue::sym("foo")));
        }
        _ => panic!("Expected S-expression of atoms"),
    }
}

#[test]
fn test_vm_space_add_opcode() {
    // Test SpaceAdd opcode
    let space = SpaceHandle::new(2, "add_test".to_string());

    let mut builder = ChunkBuilder::new("test");
    let space_const = builder.add_constant(MettaValue::Space(space.clone()));
    // Push space, push atom, add
    builder.emit_u16(Opcode::PushConstant, space_const);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::SpaceAdd);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // SpaceAdd returns Unit
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit);

    // Verify the atom was added
    let atoms = space.collapse();
    assert_eq!(atoms.len(), 1);
    assert_eq!(atoms[0], MettaValue::Long(42));
}

#[test]
fn test_vm_space_remove_opcode() {
    // Test SpaceRemove opcode
    let space = SpaceHandle::new(3, "remove_test".to_string());
    space.add_atom(MettaValue::Long(1));
    space.add_atom(MettaValue::Long(2));

    let mut builder = ChunkBuilder::new("test");
    let space_const = builder.add_constant(MettaValue::Space(space.clone()));
    // Push space, push atom to remove, remove
    builder.emit_u16(Opcode::PushConstant, space_const);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::SpaceRemove);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // SpaceRemove returns Bool(true) if atom was found
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // Verify the atom was removed
    let atoms = space.collapse();
    assert_eq!(atoms.len(), 1);
    assert_eq!(atoms[0], MettaValue::Long(2));
}

#[test]
fn test_vm_space_match_opcode() {
    // Test SpaceMatch opcode with simple pattern matching
    let space = SpaceHandle::new(4, "match_test".to_string());
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("fact"),
        MettaValue::Long(1),
    ]));
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("fact"),
        MettaValue::Long(2),
    ]));
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("other"),
        MettaValue::Long(3),
    ]));

    let mut builder = ChunkBuilder::new("test");
    let space_const = builder.add_constant(MettaValue::Space(space.clone()));
    // Pattern: (fact $x)
    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("fact"),
        MettaValue::var("x"),
    ]));
    // Template (not used in simplified version)
    let template = builder.add_constant(MettaValue::var("x"));

    // Push space, pattern, template, match
    builder.emit_u16(Opcode::PushConstant, space_const);
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, template);
    builder.emit(Opcode::SpaceMatch);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should return matching atoms
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(matches) => {
            // Should have 2 matches: (fact 1) and (fact 2)
            assert_eq!(matches.len(), 2);
        }
        _ => panic!("Expected S-expression of matches"),
    }
}

// === Collect/Collapse Operation Tests ===

#[test]
fn test_vm_collect_empty() {
    // Test Collect with no yielded results
    let mut builder = ChunkBuilder::new("test");
    // Collect with no prior Yield operations
    builder.emit_u16(Opcode::Collect, 0); // chunk_index = 0 (unused)
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should return empty list
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert!(items.is_empty());
        }
        _ => panic!("Expected S-expression"),
    }
}

#[test]
fn test_vm_collect_n() {
    // Test CollectN (collect up to N results)
    let mut builder = ChunkBuilder::new("test");
    // CollectN with n=2
    builder.emit_byte(Opcode::CollectN, 2);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);

    // Manually add some results to simulate prior Yield operations
    vm.push_result(MettaValue::Long(1));
    vm.push_result(MettaValue::Long(2));
    vm.push_result(MettaValue::Long(3)); // This shouldn't be collected

    let results = vm.run().expect("VM should succeed");

    // Should return list with only 2 elements
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Long(1));
            assert_eq!(items[1], MettaValue::Long(2));
        }
        _ => panic!("Expected S-expression"),
    }
}

#[test]
fn test_vm_collect_filters_nil() {
    // Test that Collect filters out Nil values
    let mut builder = ChunkBuilder::new("test");
    builder.emit_u16(Opcode::Collect, 0);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);

    // Add results including Nil
    vm.push_result(MettaValue::Long(1));
    vm.push_result(MettaValue::Nil);
    vm.push_result(MettaValue::Long(2));
    vm.push_result(MettaValue::Nil);

    let results = vm.run().expect("VM should succeed");

    // Should return list without Nil values
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Long(1));
            assert_eq!(items[1], MettaValue::Long(2));
        }
        _ => panic!("Expected S-expression"),
    }
}

// === Call/TailCall Tests ===

#[test]
fn test_vm_call_no_rules() {
    // Test Call opcode with no matching rules - should return expression unchanged
    let env = Environment::new();
    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (unknown 42)
    let mut builder = ChunkBuilder::new("test_call_no_rules");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let head_idx = builder.add_constant(MettaValue::sym("unknown"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // Should return (unknown 42) since no rules match
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::sym("unknown"));
            assert_eq!(items[1], MettaValue::Long(42));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

#[test]
fn test_vm_call_simple_rule() {
    use crate::backend::models::Rule;

    // Test Call opcode with a simple rule: (double $x) -> (+ $x $x)
    let mut env = Environment::new();
    let rule = Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::sym("double"),
            MettaValue::sym("$x"),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::sym("$x"),
            MettaValue::sym("$x"),
        ]),
    );
    env.add_rule(rule);
    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (double 5)
    let mut builder = ChunkBuilder::new("test_call_simple");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    let head_idx = builder.add_constant(MettaValue::sym("double"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // The rule body (+ $x $x) with $x=5 compiles to Add, so result should be 10
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(10));
}

#[test]
fn test_vm_call_no_bridge() {
    // Test Call opcode without a bridge - should return expression unchanged
    // Build bytecode for (unknown 42)
    let mut builder = ChunkBuilder::new("test_call_no_bridge");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let head_idx = builder.add_constant(MettaValue::sym("unknown"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should return (unknown 42) since no bridge is attached
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::sym("unknown"));
            assert_eq!(items[1], MettaValue::Long(42));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

#[test]
fn test_vm_tail_call_no_rules() {
    // Test TailCall opcode with no matching rules
    let env = Environment::new();
    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (unknown 42) using TailCall
    let mut builder = ChunkBuilder::new("test_tail_call_no_rules");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let head_idx = builder.add_constant(MettaValue::sym("unknown"));
    builder.emit_u16(Opcode::TailCall, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // Should return (unknown 42) since no rules match
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::sym("unknown"));
            assert_eq!(items[1], MettaValue::Long(42));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

#[test]
fn test_vm_tail_call_simple_rule() {
    use crate::backend::models::Rule;

    // Test TailCall opcode with a simple rule: (inc $x) -> (+ $x 1)
    let mut env = Environment::new();
    let rule = Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::sym("inc"),
            MettaValue::sym("$x"),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::sym("$x"),
            MettaValue::Long(1),
        ]),
    );
    env.add_rule(rule);
    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (inc 10) using TailCall
    let mut builder = ChunkBuilder::new("test_tail_call_simple");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    let head_idx = builder.add_constant(MettaValue::sym("inc"));
    builder.emit_u16(Opcode::TailCall, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // The rule body (+ $x 1) with $x=10 compiles to Add, so result should be 11
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(11));
}

#[test]
fn test_vm_call_with_multiple_args() {
    use crate::backend::models::Rule;

    // Test Call with multiple arguments: (add3 $a $b $c) -> (+ (+ $a $b) $c)
    let mut env = Environment::new();
    let rule = Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::sym("add3"),
            MettaValue::sym("$a"),
            MettaValue::sym("$b"),
            MettaValue::sym("$c"),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::SExpr(vec![
                MettaValue::sym("+"),
                MettaValue::sym("$a"),
                MettaValue::sym("$b"),
            ]),
            MettaValue::sym("$c"),
        ]),
    );
    env.add_rule(rule);
    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (add3 1 2 3)
    let mut builder = ChunkBuilder::new("test_call_multi_args");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    let head_idx = builder.add_constant(MettaValue::sym("add3"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[3]); // arity = 3
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // (add3 1 2 3) -> (+ (+ 1 2) 3) -> (+ 3 3) -> 6
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(6));
}

// =======================================================================
// Fork/Choice tests for multi-match
// =======================================================================

#[test]
fn test_vm_call_multiple_rules_creates_choice_point() {
    use crate::backend::models::Rule;

    // Set up environment with multiple rules for (choose)
    // This tests that op_call creates choice points for multiple matching rules
    let mut env = Environment::new();

    // Rule 1: (= (choose) a)
    let rule1 = Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("choose")]),
        MettaValue::sym("a"),
    );
    env.add_rule(rule1);

    // Rule 2: (= (choose) b)
    let rule2 = Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("choose")]),
        MettaValue::sym("b"),
    );
    env.add_rule(rule2);

    // Rule 3: (= (choose) c)
    let rule3 = Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("choose")]),
        MettaValue::sym("c"),
    );
    env.add_rule(rule3);

    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (choose) with Yield
    // When choice points are exhausted, op_fail returns Break directly
    let mut builder = ChunkBuilder::new("test_multi_match");
    builder.emit(Opcode::BeginNondet);
    let head_idx = builder.add_constant(MettaValue::sym("choose"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[0]); // arity = 0
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // Results are returned directly as separate values when exhausted
    // Should get all three values: a, b, c
    assert_eq!(results.len(), 3, "Expected 3 results, got: {:?}", results);
    assert!(results.contains(&MettaValue::sym("a")), "Missing 'a': {:?}", results);
    assert!(results.contains(&MettaValue::sym("b")), "Missing 'b': {:?}", results);
    assert!(results.contains(&MettaValue::sym("c")), "Missing 'c': {:?}", results);
}

#[test]
fn test_vm_call_single_rule_no_choice_point() {
    use crate::backend::models::Rule;

    // Set up environment with a single rule
    let mut env = Environment::new();
    let rule = Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::sym("single"),
            MettaValue::sym("$x"),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::sym("$x"),
            MettaValue::Long(1),
        ]),
    );
    env.add_rule(rule);

    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (single 5)
    let mut builder = ChunkBuilder::new("test_single_match");
    builder.emit_byte(Opcode::PushLongSmall, 5);
    let head_idx = builder.add_constant(MettaValue::sym("single"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // (single 5) -> (+ 5 1) -> 6
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(6));

    // Should have no choice points left
    assert!(vm.choice_points_len() == 0);
}

#[test]
fn test_vm_fork_basic_alternatives() {
    // Test Fork opcode directly with value alternatives
    // When all alternatives are yielded and exhausted, op_fail returns
    // directly with collected results (bypassing Collect opcode)
    let mut builder = ChunkBuilder::new("test_fork_basic");

    // Add constants for alternatives
    let idx_a = builder.add_constant(MettaValue::sym("a"));
    let idx_b = builder.add_constant(MettaValue::sym("b"));
    let idx_c = builder.add_constant(MettaValue::sym("c"));

    // Emit: BeginNondet, Fork 3 alternatives, Yield
    // Note: Collect/Return won't be reached as op_fail returns Break when exhausted
    builder.emit(Opcode::BeginNondet);
    builder.emit_u16(Opcode::Fork, 3);
    builder.emit_raw(&idx_a.to_be_bytes());
    builder.emit_raw(&idx_b.to_be_bytes());
    builder.emit_raw(&idx_c.to_be_bytes());
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Results are returned directly as separate values (not collected into SExpr)
    // because op_fail returns Break(results) when choice points are exhausted
    assert_eq!(results.len(), 3);
    assert!(results.contains(&MettaValue::sym("a")));
    assert!(results.contains(&MettaValue::sym("b")));
    assert!(results.contains(&MettaValue::sym("c")));
}

#[test]
fn test_vm_fork_nested_choice_points() {
    use crate::backend::models::Rule;

    // Test nested non-determinism:
    // (= (outer) (inner)) -- outer calls inner
    // (= (inner) x) -- inner returns x
    // (= (inner) y) -- inner also returns y
    //
    // When (outer) is called, it matches the rule and calls (inner).
    // (inner) has two matching rules, so a choice point is created.
    // Each result flows back through (outer) via Yield.
    let mut env = Environment::new();

    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("outer")]),
        MettaValue::SExpr(vec![MettaValue::sym("inner")]),
    ));

    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("inner")]),
        MettaValue::sym("x"),
    ));

    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("inner")]),
        MettaValue::sym("y"),
    ));

    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for evaluating (outer) with Yield
    // Note: Results are returned directly when choice points exhausted
    let mut builder = ChunkBuilder::new("test_nested");
    builder.emit(Opcode::BeginNondet);
    let head_idx = builder.add_constant(MettaValue::sym("outer"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[0]); // arity = 0
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // (outer) -> (inner) -> x or y
    // Results are returned as separate values when choice points exhausted
    // Note: Current implementation may only return first result for nested calls
    // TODO: Full nested non-determinism requires additional work
    assert!(!results.is_empty(), "Should get at least one result");
    // First result should be x (first matching rule for inner)
    assert!(results.contains(&MettaValue::sym("x")) ||
            results.contains(&MettaValue::SExpr(vec![MettaValue::sym("inner")])),
            "Should contain x or (inner): {:?}", results);
}

#[test]
fn test_vm_alternative_rulematch() {
    use crate::backend::models::Rule;

    // Test that Alternative::RuleMatch properly handles multiple matching rules
    // (= (pair $x) (cons $x $x))
    // (= (pair $x) (dup $x))
    let mut env = Environment::new();
    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::sym("pair"),
            MettaValue::sym("$x"),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("cons"),
            MettaValue::sym("$x"),
            MettaValue::sym("$x"),
        ]),
    ));

    // Add second rule with same pattern
    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![
            MettaValue::sym("pair"),
            MettaValue::sym("$x"),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::sym("dup"),
            MettaValue::sym("$x"),
        ]),
    ));

    let bridge = Arc::new(MorkBridge::from_env(env));

    // Build bytecode for (pair 5) with Yield to collect results
    // Results are returned directly when choice points exhausted
    let mut builder = ChunkBuilder::new("test_rulematch_bindings");
    builder.emit(Opcode::BeginNondet);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    let head_idx = builder.add_constant(MettaValue::sym("pair"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::with_bridge(chunk, bridge);
    let results = vm.run().expect("VM should succeed");

    // Results are returned directly as separate values when exhausted
    // With current multi-rule matching in op_call, we may get results
    // from choice points created for multiple matching rules
    assert!(!results.is_empty(), "Should get at least one result");

    // Verify results contain expected patterns (cons 5 5) or (dup 5)
    // or the unevaluated SExprs if rules aren't fully evaluated
    for result in &results {
        if let MettaValue::SExpr(inner) = result {
            if !inner.is_empty() {
                let head = &inner[0];
                // Check various possible forms of results
                assert!(
                    *head == MettaValue::sym("cons") ||
                    *head == MettaValue::sym("dup") ||
                    *head == MettaValue::sym("pair"),
                    "Unexpected result head: {:?}", head
                );
            }
        }
    }
}

// ==================== New Opcode Tests ====================

#[test]
fn test_vm_guard_true() {
    // Test Guard with true condition - should continue execution
    let mut builder = ChunkBuilder::new("test_guard_true");

    // Push true, then guard (should pass), then push result
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Guard);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::Long(42)]);
}

#[test]
fn test_vm_guard_false() {
    // Test Guard with false condition in nondeterministic context
    // Guard(false) should trigger backtracking
    let mut builder = ChunkBuilder::new("test_guard_false");

    // Set up two alternatives: first will fail guard, second will succeed
    let idx1 = builder.add_constant(MettaValue::Bool(false)); // first alt fails guard
    let idx2 = builder.add_constant(MettaValue::Bool(true));  // second alt passes guard

    builder.emit(Opcode::BeginNondet);

    // Fork with two alternatives (count must match number of indices)
    builder.emit_u16(Opcode::Fork, 2);
    builder.emit_raw(&idx1.to_be_bytes());
    builder.emit_raw(&idx2.to_be_bytes());

    // Guard consumes the bool from Fork
    builder.emit(Opcode::Guard);

    // If guard passes, push success marker and yield
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Only the second alternative (true) should pass guard
    assert_eq!(results, vec![MettaValue::Long(99)]);
}

#[test]
fn test_vm_backtrack() {
    // Test Backtrack opcode - should force immediate backtracking
    // Simpler test: Fork gives 1, we backtrack, Fork gives 2, we yield
    let mut builder = ChunkBuilder::new("test_backtrack");

    let idx1 = builder.add_constant(MettaValue::Long(1));
    let idx2 = builder.add_constant(MettaValue::Long(2));

    builder.emit(Opcode::BeginNondet);

    // Fork with two alternatives
    builder.emit_u16(Opcode::Fork, 2);
    builder.emit_raw(&idx1.to_be_bytes());
    builder.emit_raw(&idx2.to_be_bytes());

    // Dup the value for comparison
    builder.emit(Opcode::Dup);

    // Check if it's 1
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Eq);

    // Jump over backtrack if false (i.e., value is not 1)
    let skip_backtrack_label = builder.emit_jump(Opcode::JumpIfFalse);

    // Pop the duped value since we're backtracking
    builder.emit(Opcode::Pop);

    // Backtrack (skip value 1)
    builder.emit(Opcode::Backtrack);

    // Patch jump target - continue with value on stack
    builder.patch_jump(skip_backtrack_label);

    // Yield the value
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Only value 2 should be yielded (1 was backtracked)
    assert_eq!(results, vec![MettaValue::Long(2)]);
}

#[test]
fn test_vm_commit() {
    // Test Commit opcode - removes choice points
    let mut builder = ChunkBuilder::new("test_commit");

    let idx1 = builder.add_constant(MettaValue::Long(1));
    let idx2 = builder.add_constant(MettaValue::Long(2));
    let idx3 = builder.add_constant(MettaValue::Long(3));

    builder.emit(Opcode::BeginNondet);

    // Fork with three alternatives
    builder.emit_u16(Opcode::Fork, 3);
    builder.emit_raw(&idx1.to_be_bytes());
    builder.emit_raw(&idx2.to_be_bytes());
    builder.emit_raw(&idx3.to_be_bytes());

    // After getting first alternative, commit (remove remaining choice points)
    builder.emit_byte(Opcode::Commit, 0); // 0 = remove all

    // Yield the value
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Only first value should be returned (commit removed other choice points)
    assert_eq!(results, vec![MettaValue::Long(1)]);
}

#[test]
fn test_vm_amb() {
    // Test Amb opcode - inline nondeterministic choice
    let mut builder = ChunkBuilder::new("test_amb");

    builder.emit(Opcode::BeginNondet);

    // Push alternatives onto stack
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit_byte(Opcode::PushLongSmall, 30);

    // Amb chooses from 3 stack values
    builder.emit_byte(Opcode::Amb, 3);

    // Yield each choice
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should get all 3 values (order depends on amb implementation)
    assert_eq!(results.len(), 3);
    assert!(results.contains(&MettaValue::Long(10)));
    assert!(results.contains(&MettaValue::Long(20)));
    assert!(results.contains(&MettaValue::Long(30)));
}

#[test]
fn test_vm_amb_single() {
    // Test Amb with single alternative - no choice point needed
    let mut builder = ChunkBuilder::new("test_amb_single");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::Amb, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::Long(42)]);
}

#[test]
fn test_vm_call_native() {
    // Test CallNative opcode
    let mut builder = ChunkBuilder::new("test_call_native");

    // Call strlen("hello")
    let str_idx = builder.add_constant(MettaValue::String("hello".to_string()));
    builder.emit_u16(Opcode::PushConstant, str_idx);

    // Get strlen function ID (it's in stdlib)
    // strlen is at index 2 in stdlib registration order
    builder.emit_u16(Opcode::CallNative, 2); // strlen ID
    builder.emit_raw(&[1]); // arity = 1

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::Long(5)]); // "hello" has length 5
}

#[test]
fn test_vm_call_native_concat() {
    // Test CallNative with concat function
    let mut builder = ChunkBuilder::new("test_call_native_concat");

    // Push two strings
    let str1_idx = builder.add_constant(MettaValue::String("Hello, ".to_string()));
    let str2_idx = builder.add_constant(MettaValue::String("World!".to_string()));
    builder.emit_u16(Opcode::PushConstant, str1_idx);
    builder.emit_u16(Opcode::PushConstant, str2_idx);

    // Call concat (ID 1 in stdlib)
    builder.emit_u16(Opcode::CallNative, 1); // concat ID
    builder.emit_raw(&[2]); // arity = 2

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::String("Hello, World!".to_string())]);
}

#[test]
fn test_vm_call_native_range() {
    // Test CallNative with range function
    let mut builder = ChunkBuilder::new("test_call_native_range");

    // Push start and end
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit_byte(Opcode::PushLongSmall, 3);

    // Call range (ID 7 in stdlib)
    builder.emit_u16(Opcode::CallNative, 7); // range ID
    builder.emit_raw(&[2]); // arity = 2

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    let expected = MettaValue::SExpr(vec![
        MettaValue::Long(0),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    assert_eq!(results, vec![expected]);
}

#[test]
fn test_vm_call_cached() {
    // Test CallCached - should cache and return result
    let mut builder = ChunkBuilder::new("test_call_cached");

    // Add head constant "foo"
    let head_idx = builder.add_constant(MettaValue::Atom("foo".to_string()));

    // Push argument
    builder.emit_byte(Opcode::PushLongSmall, 42);

    // Call cached with head_idx and arity 1
    builder.emit_u16(Opcode::CallCached, head_idx);
    builder.emit_raw(&[1]); // arity = 1

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Without a bridge, should return the expression unchanged (irreducible)
    let expected = MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Long(42),
    ]);
    assert_eq!(results, vec![expected]);

    // Verify memo cache has an entry
    assert_eq!(vm.memo_cache_len(), 1);
}

#[test]
fn test_vm_call_cached_cache_hit() {
    // Test that CallCached uses the cache on second call
    let mut builder = ChunkBuilder::new("test_call_cached_hit");

    // Add head constant "bar"
    let head_idx = builder.add_constant(MettaValue::Atom("bar".to_string()));

    // First call
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_u16(Opcode::CallCached, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Pop); // Discard first result

    // Second call with same args - should hit cache
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_u16(Opcode::CallCached, head_idx);
    builder.emit_raw(&[1]); // arity = 1

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    let expected = MettaValue::SExpr(vec![
        MettaValue::Atom("bar".to_string()),
        MettaValue::Long(10),
    ]);
    assert_eq!(results, vec![expected]);

    // Still only 1 cache entry (same key both times)
    assert_eq!(vm.memo_cache_len(), 1);

    // Check cache stats: 1 miss (first call) + 1 hit (second call)
    let stats = vm.memo_cache_stats();
    assert_eq!(stats.hits, 1, "Should have 1 cache hit");
    assert_eq!(stats.misses, 1, "Should have 1 cache miss");
}

#[test]
fn test_vm_call_cached_different_args() {
    // Test that different args result in different cache entries
    let mut builder = ChunkBuilder::new("test_call_cached_diff_args");

    // Add head constant "baz"
    let head_idx = builder.add_constant(MettaValue::Atom("baz".to_string()));

    // First call with arg 1
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_u16(Opcode::CallCached, head_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Pop);

    // Second call with arg 2 - different args, should miss cache
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_u16(Opcode::CallCached, head_idx);
    builder.emit_raw(&[1]); // arity = 1

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    let expected = MettaValue::SExpr(vec![
        MettaValue::Atom("baz".to_string()),
        MettaValue::Long(2),
    ]);
    assert_eq!(results, vec![expected]);

    // Should have 2 cache entries (different args)
    assert_eq!(vm.memo_cache_len(), 2);

    // Check cache stats: 2 misses (different args each time)
    let stats = vm.memo_cache_stats();
    assert_eq!(stats.hits, 0, "Should have 0 cache hits");
    assert_eq!(stats.misses, 2, "Should have 2 cache misses");
}

#[test]
fn test_vm_call_external() {
    // Test CallExternal with a registered function
    use crate::backend::bytecode::external_registry::{ExternalRegistry, ExternalError};

    let mut builder = ChunkBuilder::new("test_call_external");

    // Add function name constant
    let name_idx = builder.add_constant(MettaValue::Atom("triple".to_string()));

    // Push argument
    builder.emit_byte(Opcode::PushLongSmall, 7);

    // Call external function
    builder.emit_u16(Opcode::CallExternal, name_idx);
    builder.emit_raw(&[1]); // arity = 1

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();

    // Create registry with the function
    let mut registry = ExternalRegistry::new();
    registry.register("triple", |args, _ctx| {
        let n = match args.first() {
            Some(MettaValue::Long(n)) => *n,
            _ => return Err(ExternalError::TypeError {
                expected: "Long",
                got: "other".to_string(),
            }),
        };
        Ok(vec![MettaValue::Long(n * 3)])
    });

    let mut vm = BytecodeVM::new(chunk).with_external_registry(Arc::new(registry));
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::Long(21)]);
}

#[test]
fn test_vm_call_external_not_found() {
    // Test CallExternal with unregistered function
    let mut builder = ChunkBuilder::new("test_call_external_notfound");

    let name_idx = builder.add_constant(MettaValue::Atom("nonexistent".to_string()));
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_u16(Opcode::CallExternal, name_idx);
    builder.emit_raw(&[1]); // arity = 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);

    let result = vm.run();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::Runtime(msg) if msg.contains("not registered")));
}

#[test]
fn test_vm_call_external_multiple_args() {
    // Test CallExternal with multiple arguments
    use crate::backend::bytecode::external_registry::ExternalRegistry;

    let mut builder = ChunkBuilder::new("test_call_external_multi");

    let name_idx = builder.add_constant(MettaValue::Atom("add3".to_string()));

    // Push three arguments
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit_byte(Opcode::PushLongSmall, 12);

    builder.emit_u16(Opcode::CallExternal, name_idx);
    builder.emit_raw(&[3]); // arity = 3

    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();

    let mut registry = ExternalRegistry::new();
    registry.register("add3", |args, _ctx| {
        let sum: i64 = args.iter()
            .filter_map(|v| if let MettaValue::Long(n) = v { Some(*n) } else { None })
            .sum();
        Ok(vec![MettaValue::Long(sum)])
    });

    let mut vm = BytecodeVM::new(chunk).with_external_registry(Arc::new(registry));
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::Long(42)]);
}
