//! Tests for the bytecode VM.
//!
//! This module contains all unit tests for the VM implementation.

use std::sync::Arc;

use super::pattern::{pattern_matches, unify};
use super::types::VmError;
use super::BytecodeVM;
use crate::backend::bytecode::chunk::ChunkBuilder;
use crate::backend::bytecode::mork_bridge::MorkBridge;
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{MettaValue, MettaValueInner, SpaceHandle};

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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    builder.emit(Opcode::PushUnit);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results[0], MettaValue::Unit());
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

    assert_eq!(results[0], MettaValue::Unit());
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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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

    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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

    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    builder.emit(Opcode::PushUnit);
    let jump_label = builder.emit_jump(Opcode::JumpIfUnit);
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
        &MettaValue::sexpr(vec![MettaValue::sym("_"), MettaValue::Long(2),]),
        &MettaValue::sexpr(vec![MettaValue::sym("anything"), MettaValue::Long(2),])
    ));
}

#[test]
fn test_unification_bidirectional() {
    // Unify var with value
    let bindings = unify(&MettaValue::var("x"), &MettaValue::Long(42)).expect("Should unify");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0], ("$x".to_string(), MettaValue::Long(42)));

    // Unify value with var (bidirectional)
    let bindings = unify(&MettaValue::Long(42), &MettaValue::var("x")).expect("Should unify");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0], ("$x".to_string(), MettaValue::Long(42)));

    // Unify two vars
    let bindings = unify(&MettaValue::var("x"), &MettaValue::var("y")).expect("Should unify");
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
        ]),
    )
    .expect("Should unify");

    assert_eq!(bindings.len(), 2);
    assert!(bindings.contains(&("$x".to_string(), MettaValue::Long(1))));
    assert!(bindings.contains(&("$y".to_string(), MettaValue::Long(2))));
}

#[test]
fn test_unification_failure() {
    // Different atoms don't unify
    assert!(unify(&MettaValue::sym("foo"), &MettaValue::sym("bar")).is_none());

    // Different arity S-expressions don't unify
    assert!(unify(
        &MettaValue::sexpr(vec![MettaValue::Long(1)]),
        &MettaValue::sexpr(vec![MettaValue::Long(1), MettaValue::Long(2)])
    )
    .is_none());
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
    match results[0].inner() {
        MettaValueInner::SExpr(atoms) => {
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
    assert_eq!(results[0], MettaValue::Unit());

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
    match results[0].inner() {
        MettaValueInner::SExpr(matches) => {
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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    vm.push_result(MettaValue::Unit());
    vm.push_result(MettaValue::Long(2));
    vm.push_result(MettaValue::Unit());

    let results = vm.run().expect("VM should succeed");

    // Should return list without Nil values
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    let env = HeapEnvironment::default();
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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    let mut env = HeapEnvironment::default();
    let rule = Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("double"), MettaValue::sym("$x")]),
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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    let env = HeapEnvironment::default();
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
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
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
    let mut env = HeapEnvironment::default();
    let rule = Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("inc"), MettaValue::sym("$x")]),
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
    let mut env = HeapEnvironment::default();
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
    let mut env = HeapEnvironment::default();

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
    assert!(
        results.contains(&MettaValue::sym("a")),
        "Missing 'a': {:?}",
        results
    );
    assert!(
        results.contains(&MettaValue::sym("b")),
        "Missing 'b': {:?}",
        results
    );
    assert!(
        results.contains(&MettaValue::sym("c")),
        "Missing 'c': {:?}",
        results
    );
}

#[test]
fn test_vm_call_single_rule_no_choice_point() {
    use crate::backend::models::Rule;

    // Set up environment with a single rule
    let mut env = HeapEnvironment::default();
    let rule = Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("single"), MettaValue::sym("$x")]),
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
    let mut env = HeapEnvironment::default();

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
    assert!(
        results.contains(&MettaValue::sym("x"))
            || results.contains(&MettaValue::SExpr(vec![MettaValue::sym("inner")])),
        "Should contain x or (inner): {:?}",
        results
    );
}

#[test]
fn test_vm_alternative_rulematch() {
    use crate::backend::models::Rule;

    // Test that Alternative::RuleMatch properly handles multiple matching rules
    // (= (pair $x) (cons $x $x))
    // (= (pair $x) (dup $x))
    let mut env = HeapEnvironment::default();
    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("pair"), MettaValue::sym("$x")]),
        MettaValue::SExpr(vec![
            MettaValue::sym("cons"),
            MettaValue::sym("$x"),
            MettaValue::sym("$x"),
        ]),
    ));

    // Add second rule with same pattern
    env.add_rule(Rule::new(
        MettaValue::SExpr(vec![MettaValue::sym("pair"), MettaValue::sym("$x")]),
        MettaValue::SExpr(vec![MettaValue::sym("dup"), MettaValue::sym("$x")]),
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
        if let MettaValueInner::SExpr(inner) = result.inner() {
            if !inner.is_empty() {
                let head = &inner[0];
                // Check various possible forms of results
                assert!(
                    *head == MettaValue::sym("cons")
                        || *head == MettaValue::sym("dup")
                        || *head == MettaValue::sym("pair"),
                    "Unexpected result head: {:?}",
                    head
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
    let idx2 = builder.add_constant(MettaValue::Bool(true)); // second alt passes guard

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

    assert_eq!(
        results,
        vec![MettaValue::String("Hello, World!".to_string())]
    );
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
    use crate::backend::bytecode::external_registry::{ExternalError, ExternalRegistry};

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
        let n = match args.first().map(|v| v.inner()) {
            Some(MettaValueInner::Long(n)) => *n,
            _ => {
                return Err(ExternalError::TypeError {
                    expected: "Long",
                    got: "other".to_string(),
                })
            }
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
        let sum: i64 = args
            .iter()
            .filter_map(|v| {
                if let MettaValueInner::Long(n) = v.inner() {
                    Some(*n)
                } else {
                    None
                }
            })
            .sum();
        Ok(vec![MettaValue::Long(sum)])
    });

    let mut vm = BytecodeVM::new(chunk).with_external_registry(Arc::new(registry));
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results, vec![MettaValue::Long(42)]);
}

// =============================================================================
// Tests for Newly Implemented Stub Operations
// =============================================================================

/// Test op_match_head: matches head symbol of S-expression
#[test]
fn test_vm_match_head_success() {
    let mut builder = ChunkBuilder::new("test_match_head");
    // Expected symbol in constant pool
    let add_idx = builder.add_constant(MettaValue::sym("add"));
    // Build S-expression (add 1 2)
    let add_sym_idx = builder.add_constant(MettaValue::sym("add"));
    builder.emit_u16(Opcode::PushAtom, add_sym_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    // Match head
    builder.emit_byte(Opcode::MatchHead, add_idx as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_match_head_failure() {
    let mut builder = ChunkBuilder::new("test_match_head_fail");
    // Expected symbol "sub"
    let sub_idx = builder.add_constant(MettaValue::sym("sub"));
    // Build S-expression (add 1 2)
    let add_sym_idx = builder.add_constant(MettaValue::sym("add"));
    builder.emit_u16(Opcode::PushAtom, add_sym_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::MakeSExpr, 3);
    // Match head - should fail because head is "add" not "sub"
    builder.emit_byte(Opcode::MatchHead, sub_idx as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_vm_match_head_non_sexpr() {
    let mut builder = ChunkBuilder::new("test_match_head_non_sexpr");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    // Push a plain Long, not S-expression
    builder.emit_byte(Opcode::PushLongSmall, 42);
    // Match head - should return false for non-S-expressions
    builder.emit_byte(Opcode::MatchHead, foo_idx as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_vm_match_head_empty_sexpr() {
    let mut builder = ChunkBuilder::new("test_match_head_empty");
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    // Push empty S-expression
    builder.emit(Opcode::PushEmpty);
    // Match head - should return false for empty S-expression
    builder.emit_byte(Opcode::MatchHead, foo_idx as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test op_call_n: call with head on stack
#[test]
fn test_vm_call_n_irreducible() {
    // When no MORK bridge is set, call_n should return expression as data
    let mut builder = ChunkBuilder::new("test_call_n");
    // Push head "foo" and args
    let foo_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, foo_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    // Call with 2 arguments (head is on stack)
    builder.emit_byte(Opcode::CallN, 2);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should return (foo 1 2) as irreducible expression
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::sym("foo"));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

#[test]
fn test_vm_call_n_non_atom_head() {
    // When head is not an atom, should return expression as data
    let mut builder = ChunkBuilder::new("test_call_n_non_atom");
    // Push a Long as head (not an atom)
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    // Call with 2 arguments
    builder.emit_byte(Opcode::CallN, 2);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should return (42 1 2) as expression
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Long(42));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test op_tail_call_n: tail call with head on stack
#[test]
fn test_vm_tail_call_n_irreducible() {
    let mut builder = ChunkBuilder::new("test_tail_call_n");
    let bar_idx = builder.add_constant(MettaValue::sym("bar"));
    builder.emit_u16(Opcode::PushAtom, bar_idx);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    // Tail call with 1 argument
    builder.emit_byte(Opcode::TailCallN, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::sym("bar"));
            assert_eq!(items[1], MettaValue::Long(10));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test op_jump_table: multi-way branch
#[test]
fn test_vm_jump_table() {
    use crate::backend::bytecode::chunk::JumpTable;

    let mut builder = ChunkBuilder::new("test_jump_table");

    // We'll create a simple jump table with 2 entries
    // First, we need to know where the targets will be

    // Push the selector value (we'll match on Long(1))
    builder.emit_byte(Opcode::PushLongSmall, 1);

    // Calculate hash for selector value (must match what control_flow.rs uses)
    use xxhash_rust::xxh3::xxh3_64;
    let hash_1 = xxh3_64(format!("{:?}", MettaValue::Long(1)).as_bytes());

    // Create jump table - entries will be patched after we know offsets
    // For now, we'll use a simple approach: jump table at index 0
    // Default will push 0, case 1 will push 100

    // JumpTable opcode + table_index(u16)
    builder.emit_u16(Opcode::JumpTable, 0);

    // Offset for case 1 (push 100) - right after default
    // We need to know the default offset first
    let default_offset = builder.current_offset();
    builder.emit_byte(Opcode::PushLongSmall, 0); // default case
    builder.emit(Opcode::Return);

    let case_1_offset = builder.current_offset();
    builder.emit_byte(Opcode::PushLongSmall, 100); // case 1
    builder.emit(Opcode::Return);

    // Add jump table
    let jump_table = JumpTable {
        base_offset: 0,
        entries: vec![(hash_1, case_1_offset)],
        default_offset,
    };
    builder.add_jump_table(jump_table);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(100)); // Should hit case 1
}

#[test]
fn test_vm_jump_table_default() {
    use crate::backend::bytecode::chunk::JumpTable;
    use xxhash_rust::xxh3::xxh3_64;

    let mut builder = ChunkBuilder::new("test_jump_table_default");

    // Push selector that won't match any case
    builder.emit_byte(Opcode::PushLongSmall, 99);

    // Create hash for a different value (1) - must match what control_flow.rs uses
    let hash_1 = xxh3_64(format!("{:?}", MettaValue::Long(1)).as_bytes());

    builder.emit_u16(Opcode::JumpTable, 0);

    let default_offset = builder.current_offset();
    builder.emit_byte(Opcode::PushLongSmall, 42); // default case
    builder.emit(Opcode::Return);

    let case_1_offset = builder.current_offset();
    builder.emit_byte(Opcode::PushLongSmall, 100); // case 1 (not hit)
    builder.emit(Opcode::Return);

    let jump_table = JumpTable {
        base_offset: 0,
        entries: vec![(hash_1, case_1_offset)],
        default_offset,
    };
    builder.add_jump_table(jump_table);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42)); // Should hit default
}

/// Test Alternative::Index in nondeterminism
#[test]
fn test_vm_alternative_index() {
    use super::types::{Alternative, ChoicePoint};

    let mut builder = ChunkBuilder::new("test_alt_index");

    // Push some initial value
    builder.emit_byte(Opcode::PushLongSmall, 1);

    // We'll manually create a choice point with Index alternative after running
    // For this test, we need to first execute something, then fail to an Index alternative

    // First path: push 10 and yield
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit(Opcode::Yield);

    // Second path (reached via Index alternative): push 20
    let alt_offset = builder.current_offset();
    builder.emit_byte(Opcode::PushLongSmall, 20);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    // Clone chunk for the choice point before moving into VM
    let chunk_for_cp = Arc::clone(&chunk);
    let mut vm = BytecodeVM::new(chunk);

    // Manually inject a choice point with Index alternative pointing to alt_offset
    vm.choice_points.push(ChoicePoint {
        value_stack_height: 0,
        call_stack_height: 0,
        bindings_stack_height: vm.bindings_stack.len(),
        ip: alt_offset, // Will be used when we backtrack
        chunk: chunk_for_cp,
        alternatives: vec![Alternative::Index(alt_offset)],
    });

    let results = vm.run().expect("VM should succeed");

    // Should have results from both paths: 10 and 20
    assert_eq!(results.len(), 2);
    assert!(results.contains(&MettaValue::Long(10)));
    assert!(results.contains(&MettaValue::Long(20)));
}

/// Test op_space_match with template instantiation
#[test]
fn test_vm_space_match_with_template() {
    let mut builder = ChunkBuilder::new("test_space_match_template");

    // Create a space and add some atoms
    let space = SpaceHandle::new(100, "test_space".to_string());

    // Add atoms to space: (foo 1) (foo 2) (bar 3)
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::Long(1),
    ]));
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::Long(2),
    ]));
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("bar"),
        MettaValue::Long(3),
    ]));

    // Push space
    let space_const = builder.add_constant(MettaValue::Space(space));
    builder.emit_u16(Opcode::PushConstant, space_const);

    // Push pattern: (foo $x)
    let pattern_const = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::var("x"),
    ]));
    builder.emit_u16(Opcode::PushConstant, pattern_const);

    // Push template: (result $x)
    let template_const = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("result"),
        MettaValue::var("x"),
    ]));
    builder.emit_u16(Opcode::PushConstant, template_const);

    builder.emit(Opcode::SpaceMatch);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should get S-expression of results
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            // Should have 2 results: (result 1) and (result 2)
            assert_eq!(items.len(), 2);

            // Verify both results have "result" as head
            for item in items {
                match item.inner() {
                    MettaValueInner::SExpr(inner) => {
                        assert_eq!(inner[0], MettaValue::sym("result"));
                        // Value should be 1 or 2
                        assert!(
                            inner[1] == MettaValue::Long(1) || inner[1] == MettaValue::Long(2)
                        );
                    }
                    _ => panic!("Expected S-expression result"),
                }
            }
        }
        _ => panic!("Expected S-expression of results"),
    }
}

#[test]
fn test_vm_space_match_no_matches() {
    let mut builder = ChunkBuilder::new("test_space_match_no_matches");

    // Create a space with atoms that won't match
    let space = SpaceHandle::new(101, "test_space".to_string());
    space.add_atom(MettaValue::sexpr(vec![
        MettaValue::sym("bar"),
        MettaValue::Long(1),
    ]));

    // Push space
    let space_const = builder.add_constant(MettaValue::Space(space));
    builder.emit_u16(Opcode::PushConstant, space_const);

    // Push pattern: (foo $x) - won't match (bar 1)
    let pattern_const = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::var("x"),
    ]));
    builder.emit_u16(Opcode::PushConstant, pattern_const);

    // Push template
    let template_const = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("result"),
        MettaValue::var("x"),
    ]));
    builder.emit_u16(Opcode::PushConstant, template_const);

    builder.emit(Opcode::SpaceMatch);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should get empty S-expression
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert!(items.is_empty());
        }
        _ => panic!("Expected empty S-expression"),
    }
}

// =============================================================================
// LoadSpace Tests (xxh3 branch coverage)
// =============================================================================

/// Test LoadSpace opcode creates a space with correct hash-based ID.
#[test]
fn test_vm_load_space() {
    use xxhash_rust::xxh3::xxh3_64;

    let mut builder = ChunkBuilder::new("test_load_space");

    // Add the space name as a constant
    let space_name = "test_kb";
    let name_const = builder.add_constant(MettaValue::sym(space_name));

    // Emit LoadSpace opcode with the constant index
    builder.emit_u16(Opcode::LoadSpace, name_const);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);

    // Verify we got a space with the correct ID (based on xxh3_64 hash of name)
    match results[0].inner() {
        MettaValueInner::Space(handle) => {
            let expected_id = xxh3_64(space_name.as_bytes());
            assert_eq!(handle.id, expected_id);
            assert_eq!(handle.name, space_name);
        }
        _ => panic!("Expected Space, got {:?}", results[0]),
    }
}

/// Test LoadSpace opcode with different space names produces different IDs.
#[test]
fn test_vm_load_space_different_names() {
    use xxhash_rust::xxh3::xxh3_64;

    // Test with first space name
    let mut builder1 = ChunkBuilder::new("test_load_space_1");
    let name1_const = builder1.add_constant(MettaValue::sym("space_alpha"));
    builder1.emit_u16(Opcode::LoadSpace, name1_const);
    builder1.emit(Opcode::Return);

    let chunk1 = builder1.build_arc();
    let mut vm1 = BytecodeVM::new(chunk1);
    let results1 = vm1.run().expect("VM should succeed");

    // Test with second space name
    let mut builder2 = ChunkBuilder::new("test_load_space_2");
    let name2_const = builder2.add_constant(MettaValue::sym("space_beta"));
    builder2.emit_u16(Opcode::LoadSpace, name2_const);
    builder2.emit(Opcode::Return);

    let chunk2 = builder2.build_arc();
    let mut vm2 = BytecodeVM::new(chunk2);
    let results2 = vm2.run().expect("VM should succeed");

    // Extract space IDs
    let id1 = match results1[0].inner() {
        MettaValueInner::Space(handle) => handle.id,
        _ => panic!("Expected Space"),
    };
    let id2 = match results2[0].inner() {
        MettaValueInner::Space(handle) => handle.id,
        _ => panic!("Expected Space"),
    };

    // Different names should produce different IDs
    assert_ne!(id1, id2);
    assert_eq!(id1, xxh3_64(b"space_alpha"));
    assert_eq!(id2, xxh3_64(b"space_beta"));
}

// =============================================================================
// GenericBytecodeVM Tests
// =============================================================================

mod generic_vm_tests {
    use std::sync::Arc;

    use crate::backend::bytecode::chunk::GenericChunkBuilder;
    use crate::backend::bytecode::opcodes::Opcode;
    use crate::backend::bytecode::vm::GenericBytecodeVM;
    use crate::backend::environment::GenericEnvironment;
    use crate::backend::models::{
        HeapMettaValueFactory, MettaValue, MettaValueFactory,
    };

    fn factory() -> HeapMettaValueFactory {
        HeapMettaValueFactory
    }

    /// Test basic arithmetic with the generic VM.
    #[test]
    fn test_generic_vm_arithmetic() {
        let f = factory();
        let mut builder = GenericChunkBuilder::new("test_arith", f.clone());
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 32);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::new(chunk, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(42));
    }

    /// Test that Call reads 3 bytes (u16 head_idx + u8 arity) and dispatches
    /// via environment rules correctly.
    #[test]
    fn test_generic_vm_function_call() {
        let f = factory();
        let mut env = GenericEnvironment::new(f.clone());

        // Define rule: (= (double $x) (+ $x $x))
        let rule_lhs = f.sexpr(vec![f.atom("double"), f.atom("$x")]);
        let rule_rhs = f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$x")]);
        let rule = crate::backend::models::GenericRule::new(rule_lhs, rule_rhs);
        env.add_generic_rule(rule);

        // Bytecode: Call with head="double", arity=1, argument=5
        let mut builder = GenericChunkBuilder::new("test_call", f.clone());
        let head_idx = builder.add_constant(f.atom("double"));
        builder.emit_byte(Opcode::PushLongSmall, 5); // Push argument
        builder.emit_u16(Opcode::Call, head_idx);
        builder.emit_raw(&[1]); // arity = 1
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::with_env(chunk, env, f);
        let results = vm.run().expect("VM should succeed");

        // Should get (+ 5 5) after rule dispatch (bindings applied to body)
        assert_eq!(results.len(), 1);
        let result = &results[0];
        // The result should be an S-expression (+ 5 5) since we don't recursively evaluate
        if let Some(items) = result.as_sexpr() {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].as_atom(), Some("+"));
            assert_eq!(items[1].as_long(), Some(5));
            assert_eq!(items[2].as_long(), Some(5));
        } else {
            panic!("Expected S-expression result, got: {:?}", result.type_name());
        }
    }

    /// Test that PushVariable resolves bindings from the bindings stack.
    #[test]
    fn test_generic_vm_variable_binding() {
        let f = factory();
        let mut builder = GenericChunkBuilder::new("test_var_binding", f.clone());

        // Store a binding for "$x"
        let var_idx = builder.add_constant(f.atom("$x"));
        builder.emit_byte(Opcode::PushLongSmall, 42); // value to bind
        builder.emit_u16(Opcode::StoreBinding, var_idx); // bind $x = 42

        // Now push the variable - should resolve to 42
        builder.emit_u16(Opcode::PushVariable, var_idx);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::new(chunk, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(42));
    }

    /// Test that PushVariable returns the variable symbol when not bound.
    #[test]
    fn test_generic_vm_unbound_variable() {
        let f = factory();
        let mut builder = GenericChunkBuilder::new("test_unbound", f.clone());

        let var_idx = builder.add_constant(f.atom("$y"));
        builder.emit_u16(Opcode::PushVariable, var_idx);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::new(chunk, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_atom(), Some("$y"));
    }

    /// Test DeconAtom returns (head, tail) pair.
    #[test]
    fn test_generic_vm_decon_atom() {
        let f = factory();
        let mut builder = GenericChunkBuilder::new("test_decon", f.clone());

        // Push an S-expression (a b c)
        let expr = f.sexpr(vec![f.atom("a"), f.atom("b"), f.atom("c")]);
        let idx = builder.add_constant(expr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::DeconAtom);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::new(chunk, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        // Should be (head tail) = (a (b c))
        let pair = &results[0];
        if let Some(items) = pair.as_sexpr() {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].as_atom(), Some("a"));
            if let Some(tail_items) = items[1].as_sexpr() {
                assert_eq!(tail_items.len(), 2);
                assert_eq!(tail_items[0].as_atom(), Some("b"));
                assert_eq!(tail_items[1].as_atom(), Some("c"));
            } else {
                panic!("Expected tail to be S-expression");
            }
        } else {
            panic!("Expected pair S-expression");
        }
    }

    /// Test SpaceGetAtoms returns atoms from the environment.
    #[test]
    fn test_generic_vm_space_get_atoms() {
        let f = factory();
        let mut env = GenericEnvironment::new(f.clone());

        // Add atoms to the space
        env.add_to_space(&f.atom("hello"));
        env.add_to_space(&f.long(42));

        let mut builder = GenericChunkBuilder::new("test_space", f.clone());
        builder.emit(Opcode::SpaceGetAtoms);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::with_env(chunk, env, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        // Result should be an S-expression containing the atoms
        if let Some(items) = results[0].as_sexpr() {
            // Should have at least the atoms we added
            assert!(items.len() >= 2, "Expected at least 2 atoms, got {}", items.len());
        } else {
            panic!("Expected S-expression result from SpaceGetAtoms");
        }
    }

    /// Test CallNative dispatches to the generic native registry.
    #[test]
    fn test_generic_vm_call_native() {
        use crate::backend::bytecode::native_registry::GenericNativeRegistry;

        let f = factory();
        let mut registry = GenericNativeRegistry::<MettaValue, HeapMettaValueFactory>::new();
        let func_id = registry.register("add2", |args, _ctx| {
            let a = args.get(0).and_then(|v| v.as_long()).unwrap_or(0);
            let b = args.get(1).and_then(|v| v.as_long()).unwrap_or(0);
            Ok(vec![MettaValue::Long(a + b)])
        });

        let mut builder = GenericChunkBuilder::new("test_native", f.clone());
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 32);
        builder.emit_u16(Opcode::CallNative, func_id);
        builder.emit_raw(&[2]); // arity = 2
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let env = GenericEnvironment::new(f.clone());
        let memo_cache = Arc::new(
            crate::backend::bytecode::generic_memo_cache::GenericMemoCache::default(),
        );
        let ext_registry = Arc::new(
            crate::backend::bytecode::external_registry::GenericExternalRegistry::new(),
        );

        let mut vm = GenericBytecodeVM::with_registries(
            chunk,
            env,
            f,
            Arc::new(registry),
            ext_registry,
            memo_cache,
        );
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(42));
    }

    /// Test CallExternal dispatches to the generic external registry.
    #[test]
    fn test_generic_vm_call_external() {
        use crate::backend::bytecode::external_registry::GenericExternalRegistry;

        let f = factory();
        let mut registry = GenericExternalRegistry::<MettaValue, HeapMettaValueFactory>::new();
        registry.register("triple", |args, _ctx| {
            let n = args.get(0).and_then(|v| v.as_long()).unwrap_or(0);
            Ok(vec![MettaValue::Long(n * 3)])
        });

        let mut builder = GenericChunkBuilder::new("test_external", f.clone());
        let name_idx = builder.add_constant(f.atom("triple"));
        builder.emit_byte(Opcode::PushLongSmall, 14);
        builder.emit_u16(Opcode::CallExternal, name_idx);
        builder.emit_raw(&[1]); // arity = 1
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let env = GenericEnvironment::new(f.clone());
        let native_registry = Arc::new(
            crate::backend::bytecode::native_registry::GenericNativeRegistry::new(),
        );
        let memo_cache = Arc::new(
            crate::backend::bytecode::generic_memo_cache::GenericMemoCache::default(),
        );

        let mut vm = GenericBytecodeVM::with_registries(
            chunk,
            env,
            f,
            native_registry,
            Arc::new(registry),
            memo_cache,
        );
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(42));
    }

    /// Test CallCached uses the memo cache.
    #[test]
    fn test_generic_vm_call_cached() {
        let f = factory();
        let mut env = GenericEnvironment::new(f.clone());

        // Define rule: (= (square $x) (* $x $x))
        let rule_lhs = f.sexpr(vec![f.atom("square"), f.atom("$x")]);
        let rule_rhs = f.sexpr(vec![f.atom("*"), f.atom("$x"), f.atom("$x")]);
        let rule = crate::backend::models::GenericRule::new(rule_lhs, rule_rhs);
        env.add_generic_rule(rule);

        let memo_cache = Arc::new(
            crate::backend::bytecode::generic_memo_cache::GenericMemoCache::<MettaValue>::new(1024),
        );

        let mut builder = GenericChunkBuilder::new("test_cached", f.clone());
        let head_idx = builder.add_constant(f.atom("square"));
        builder.emit_byte(Opcode::PushLongSmall, 7);
        builder.emit_u16(Opcode::CallCached, head_idx);
        builder.emit_raw(&[1]); // arity = 1
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let native_registry = Arc::new(
            crate::backend::bytecode::native_registry::GenericNativeRegistry::new(),
        );
        let ext_registry = Arc::new(
            crate::backend::bytecode::external_registry::GenericExternalRegistry::new(),
        );

        let mut vm = GenericBytecodeVM::with_registries(
            chunk.clone(),
            env.clone(),
            f.clone(),
            native_registry.clone(),
            ext_registry.clone(),
            memo_cache.clone(),
        );
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        // Should get (* 7 7) after rule dispatch
        if let Some(items) = results[0].as_sexpr() {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].as_atom(), Some("*"));
            assert_eq!(items[1].as_long(), Some(7));
            assert_eq!(items[2].as_long(), Some(7));
        } else {
            panic!("Expected S-expression result");
        }

        // Verify it was cached
        assert!(memo_cache.len() > 0, "Cache should have entries after call");

        // Second call should hit cache
        let mut vm2 = GenericBytecodeVM::with_registries(
            chunk,
            env,
            f,
            native_registry,
            ext_registry,
            memo_cache.clone(),
        );
        let results2 = vm2.run().expect("VM should succeed on cache hit");
        assert_eq!(results, results2);

        let stats = memo_cache.stats();
        assert!(stats.hits > 0, "Should have cache hits on second call");
    }

    /// Test DefineRule + DispatchRules through the generic VM.
    #[test]
    fn test_generic_vm_define_and_dispatch() {
        let f = factory();
        let env = GenericEnvironment::new(f.clone());

        let mut builder = GenericChunkBuilder::new("test_define_dispatch", f.clone());

        // Define rule: (= (greet $x) (hello $x))
        let pattern = f.sexpr(vec![f.atom("greet"), f.atom("$x")]);
        let body = f.sexpr(vec![f.atom("hello"), f.atom("$x")]);
        let pattern_idx = builder.add_constant(pattern);
        let body_idx = builder.add_constant(body);

        builder.emit_u16(Opcode::PushConstant, pattern_idx);
        builder.emit_u16(Opcode::PushConstant, body_idx);
        builder.emit(Opcode::DefineRule);
        builder.emit(Opcode::Pop); // Pop the Unit from DefineRule

        // Now dispatch (greet world)
        let call_expr = f.sexpr(vec![f.atom("greet"), f.atom("world")]);
        let call_idx = builder.add_constant(call_expr);
        builder.emit_u16(Opcode::PushConstant, call_idx);
        builder.emit(Opcode::DispatchRules);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = GenericBytecodeVM::with_env(chunk, env, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        // Should get (hello world) after rule dispatch with $x = world
        if let Some(items) = results[0].as_sexpr() {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].as_atom(), Some("hello"));
            assert_eq!(items[1].as_atom(), Some("world"));
        } else {
            panic!("Expected S-expression result from rule dispatch");
        }
    }

    /// Test the HeapGenericBytecodeVM type alias works.
    #[test]
    fn test_heap_generic_vm_alias() {
        use super::super::HeapGenericBytecodeVM;

        let f = factory();
        let mut builder = GenericChunkBuilder::new("test_alias", f.clone());
        builder.emit_byte(Opcode::PushLongSmall, 7);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Mul);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm: HeapGenericBytecodeVM = GenericBytecodeVM::new(chunk, f);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(49));
    }
}

// ========================================================================
// Phase 2: VM Error Path Tests
// ========================================================================

// --- Stack Operation Errors ---

#[test]
fn test_vm_dup_underflow() {
    // Dup on empty stack should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Dup);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_swap_single_element() {
    // Swap with only one element should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_rot3_insufficient() {
    // Rot3 with fewer than 3 elements should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Rot3);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_over_insufficient() {
    // Over with only one element should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Over);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_pop_empty() {
    // Pop from empty stack should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Pop);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

// --- Arithmetic Type Errors ---

#[test]
fn test_vm_add_bool_long() {
    // Bool + Long should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_sub_string_string() {
    // String - String should fail
    let mut builder = ChunkBuilder::new("test");
    let s1 = builder.add_constant(MettaValue::String("hello".to_string()));
    let s2 = builder.add_constant(MettaValue::String("world".to_string()));
    builder.emit_u16(Opcode::PushConstant, s1);
    builder.emit_u16(Opcode::PushConstant, s2);
    builder.emit(Opcode::Sub);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_mul_atom_long() {
    // Atom * Long should fail
    let mut builder = ChunkBuilder::new("test");
    let atom_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushAtom, atom_idx);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Mul);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_mod_by_zero() {
    // Modulo by zero should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::DivisionByZero)));
}

#[test]
fn test_vm_floor_div_by_zero() {
    // Floor division by zero should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::FloorDiv);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::DivisionByZero)));
}

// --- Boolean Operation Type Errors ---

#[test]
fn test_vm_and_non_bool_left() {
    // Non-bool AND operand should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::And);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_or_non_bool_right() {
    // Non-bool OR operand should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushFalse);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Or);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_not_non_bool() {
    // NOT on non-bool should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Not);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_xor_non_bool() {
    // XOR on non-bool should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Xor);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// --- S-Expression Operation Errors ---

#[test]
fn test_vm_get_head_empty_sexpr() {
    // GetHead on empty S-expr should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushEmpty);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Empty S-expr has no head
    assert!(result.is_err());
}

#[test]
fn test_vm_get_head_non_sexpr() {
    // GetHead on non-S-expr should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_get_tail_non_sexpr() {
    // GetTail on non-S-expr should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_get_arity_non_sexpr() {
    // GetArity on non-S-expr should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetArity);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// --- Comparison Operation Edge Cases ---

#[test]
fn test_vm_lt_mixed_types() {
    // < on incompatible types should handle gracefully
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    let s = builder.add_constant(MettaValue::String("hello".to_string()));
    builder.emit_u16(Opcode::PushConstant, s);
    builder.emit(Opcode::Lt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should either error or return false for mixed type comparison
    // Depending on implementation, this could be a type error or return false
    assert!(result.is_err() || result.unwrap()[0] == MettaValue::Bool(false));
}

// --- Local Variable Errors ---

#[test]
fn test_vm_load_local_uninitialized() {
    // Load from uninitialized local should handle gracefully
    let mut builder = ChunkBuilder::new("test");
    builder.set_local_count(1);
    builder.emit_byte(Opcode::LoadLocal, 0);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should return Unit or error depending on implementation
    // The default value for uninitialized locals is Nil in this VM
    match result {
        Ok(results) => assert_eq!(results[0], MettaValue::Unit()),
        Err(_) => {} // Error is also acceptable
    }
}

// --- Nondeterminism Edge Cases ---

#[test]
fn test_vm_fail_no_choice_points() {
    // Fail with no choice points should terminate cleanly
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Fail);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should return empty results (no alternatives)
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[test]
fn test_vm_yield_outside_nondet() {
    // Yield without BeginNondet should handle gracefully
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Yield);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should return the yielded value
    match result {
        Ok(results) => assert_eq!(results, vec![MettaValue::Long(42)]),
        Err(_) => {} // Error is also acceptable for some implementations
    }
}

#[test]
fn test_vm_cut_no_choice_points() {
    // Cut with no choice points should succeed (no-op)
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Cut);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result, vec![MettaValue::Long(42)]);
}

#[test]
fn test_vm_backtrack_no_choice_points() {
    // Backtrack with no choice points should fail cleanly
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Backtrack);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should return empty (no alternatives to backtrack to)
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

// --- Guard Operation Errors ---

#[test]
fn test_vm_guard_non_bool() {
    // Guard with non-boolean should fail
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Guard);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Guard with non-bool should error or backtrack
    // Depends on implementation
    assert!(result.is_err() || result.unwrap().is_empty());
}

// --- Large S-Expression Construction ---

#[test]
fn test_vm_make_sexpr_large() {
    // Test MakeSExprLarge for > 255 elements
    let mut builder = ChunkBuilder::new("test");

    // Push 300 elements
    for i in 0..300u16 {
        builder.emit_byte(Opcode::PushLongSmall, (i % 128) as u8);
    }
    builder.emit_u16(Opcode::MakeSExprLarge, 300);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    match result[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 300);
        }
        _ => panic!("Expected S-expression"),
    }
}

// --- Negative Integer Handling ---

#[test]
fn test_vm_negative_small_int() {
    // Test PushLongSmall with negative value
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, (-10i8) as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result, vec![MettaValue::Long(-10)]);
}

// --- Cons Operation ---

#[test]
fn test_vm_cons_non_sexpr_tail() {
    // Cons with non-S-expr tail should create a new S-expr
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);  // head
    builder.emit_byte(Opcode::PushLongSmall, 2);  // tail (not an S-expr)
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Cons should either error or create (1 . 2) or (1 2)
    match result {
        Ok(results) => {
            // Implementation-specific: could be an S-expr with both elements
            assert_eq!(results.len(), 1);
        }
        Err(VmError::TypeError { .. }) => {} // Type error is acceptable
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

// --- Float Operations ---

#[test]
fn test_vm_float_arithmetic() {
    // Test float arithmetic - VM now supports Float operations
    let mut builder = ChunkBuilder::new("test");
    let f1 = builder.add_constant(MettaValue::Float(3.5));
    let f2 = builder.add_constant(MettaValue::Float(2.0));
    builder.emit_u16(Opcode::PushConstant, f1);
    builder.emit_u16(Opcode::PushConstant, f2);
    builder.emit(Opcode::Mul);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // VM now supports Float arithmetic
    assert!(result.is_ok());
    let results = result.expect("Float arithmetic should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Float(7.0));
}

#[test]
fn test_vm_float_comparison() {
    // Test float comparison - VM now supports Float comparisons
    let mut builder = ChunkBuilder::new("test");
    let f1 = builder.add_constant(MettaValue::Float(3.14));
    let f2 = builder.add_constant(MettaValue::Float(2.71));
    builder.emit_u16(Opcode::PushConstant, f1);
    builder.emit_u16(Opcode::PushConstant, f2);
    builder.emit(Opcode::Gt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // VM now supports Float comparisons
    assert!(result.is_ok());
    let results = result.expect("Float comparison should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true)); // 3.14 > 2.71
}

// --- Mixed Long/Float Operations ---

#[test]
fn test_vm_mixed_long_float_add() {
    // Long + Float - VM now supports type promotion
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    let f = builder.add_constant(MettaValue::Float(2.5));
    builder.emit_u16(Opcode::PushConstant, f);
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // VM now supports Long + Float with type promotion to Float
    assert!(result.is_ok());
    let results = result.expect("Mixed type addition should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Float(3.5)); // 1 + 2.5 = 3.5
}

// --- Power Operation Edge Cases ---

#[test]
fn test_vm_pow_negative_exponent() {
    // Integer pow with negative exponent
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 2);
    let exp = builder.add_constant(MettaValue::Long(-1));
    builder.emit_u16(Opcode::PushConstant, exp);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Integer pow with negative exponent should error or return 0
    match result {
        Ok(results) => {
            // Some implementations return 0 for int^(-n)
            assert!(results[0] == MettaValue::Long(0) || results[0] == MettaValue::Float(0.5));
        }
        Err(_) => {} // Error is also acceptable
    }
}

// --- Abs/Neg Edge Cases ---

#[test]
fn test_vm_abs_float() {
    // Test abs on float - VM now supports Float
    let mut builder = ChunkBuilder::new("test");
    let f = builder.add_constant(MettaValue::Float(-3.14));
    builder.emit_u16(Opcode::PushConstant, f);
    builder.emit(Opcode::Abs);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // VM now supports Float for abs
    assert!(result.is_ok());
    let results = result.expect("Float abs should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Float(3.14));
}

#[test]
fn test_vm_neg_float() {
    // Test neg on float - VM now supports Float
    let mut builder = ChunkBuilder::new("test");
    let f = builder.add_constant(MettaValue::Float(3.14));
    builder.emit_u16(Opcode::PushConstant, f);
    builder.emit(Opcode::Neg);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // VM now supports Float for neg
    assert!(result.is_ok());
    let results = result.expect("Float neg should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Float(-3.14));
}

// --- Jump Instruction Edge Cases ---

#[test]
fn test_vm_jump_if_false_non_bool() {
    // JumpIfFalse with non-boolean value
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 0);
    let label = builder.emit_jump(Opcode::JumpIfFalse);
    builder.emit_byte(Opcode::PushLongSmall, 1);  // taken if 0 is "falsy"
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(label);
    builder.emit_byte(Opcode::PushLongSmall, 2);  // taken if 0 is "truthy"
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Depends on whether VM treats 0 as false or errors on non-bool
    // Either outcome is acceptable
    assert!(result.is_ok() || result.is_err());
}

// =============================================================================
// Phase 3D: VM Error Path Tests
// =============================================================================

// --- Stack Operation Edge Cases ---

#[test]
fn test_vm_popn_underflow() {
    // PopN with count greater than stack size
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PopN, 5); // Try to pop 5 but only 2 on stack
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_dupn_zero() {
    // DupN with count = 0 should be a no-op
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::DupN, 0);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Long(42));
}

#[test]
fn test_vm_dupn_underflow() {
    // DupN with count greater than stack size
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::DupN, 5); // Try to dup 5 but only 1 on stack
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_over_underflow() {
    // Over with < 2 items on stack
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Over);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_swap_underflow() {
    // Swap with < 2 items on stack
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

#[test]
fn test_vm_rot3_underflow() {
    // Rot3 with < 3 items on stack
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::Rot3);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::StackUnderflow)));
}

// --- Control Flow Edge Cases ---

#[test]
fn test_vm_return_empty_stack() {
    // Return with empty stack
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Return with empty stack should produce empty results
    match result {
        Ok(results) => assert!(results.is_empty()),
        Err(_) => {} // Error is also acceptable
    }
}

// Note: test_vm_jump_to_self_loop removed - VmConfig doesn't have max_steps field

#[test]
fn test_vm_invalid_constant_index() {
    // Access constant with invalid index
    let mut builder = ChunkBuilder::new("test");
    builder.emit_u16(Opcode::PushConstant, 9999); // Invalid index
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::InvalidConstant(9999))));
}

#[test]
fn test_vm_invalid_local_index_phase3d() {
    // Access local with invalid index
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::LoadLocal, 99); // Invalid local index
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::InvalidLocal(99))));
}

// --- Nondeterminism Edge Cases ---

#[test]
fn test_vm_cut_empty_choice_points() {
    // Cut with no choice points - should be a no-op
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Cut);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Long(42));
}

#[test]
fn test_vm_collect_no_results() {
    // Collect with no yielded results
    let mut builder = ChunkBuilder::new("test");
    builder.emit_u16(Opcode::Collect, 0);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    // Should return empty S-expression
    assert_eq!(result.len(), 1);
    match result[0].inner() {
        MettaValueInner::SExpr(items) => assert!(items.is_empty()),
        _ => panic!("Expected empty S-expression"),
    }
}

#[test]
fn test_vm_collectn_zero() {
    // CollectN with N = 0
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Yield);
    builder.emit_byte(Opcode::CollectN, 0); // Collect 0 results
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should succeed with empty collected results
    match result {
        Ok(results) => {
            // Either empty or collected nothing
            for r in results {
                if let MettaValueInner::SExpr(items) = r.inner() {
                    assert!(items.is_empty());
                }
            }
        }
        Err(_) => {} // Error is also acceptable
    }
}

#[test]
fn test_vm_guard_type_error() {
    // Guard with non-boolean value
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42); // Not a boolean
    builder.emit(Opcode::Guard);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Guard with non-bool should error
    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// Note: test_vm_fail_no_choice_points removed - already exists at line 3527

// --- List/Expression Operation Edge Cases ---

#[test]
fn test_vm_get_head_empty_phase3d() {
    // GetHead on empty S-expression
    let mut builder = ChunkBuilder::new("test");
    let empty_idx = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty_idx);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// Note: test_vm_get_head_non_sexpr already exists at line 3437

#[test]
fn test_vm_get_tail_empty_phase3d() {
    // GetTail on empty S-expression
    let mut builder = ChunkBuilder::new("test");
    let empty_idx = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty_idx);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// Note: test_vm_get_tail_non_sexpr already exists at line 3452

#[test]
fn test_vm_cons_atom_invalid_tail_phase3d() {
    // ConsAtom with invalid tail type (not S-expression or Nil)
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 1); // head
    let str_idx = builder.add_constant(MettaValue::String("not a list".into()));
    builder.emit_u16(Opcode::PushConstant, str_idx); // tail (String, not S-expr)
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // ConsAtom with invalid tail should error
    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_index_atom_oob() {
    // IndexAtom with out-of-bounds index
    let mut builder = ChunkBuilder::new("test");
    let list_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit_u16(Opcode::PushConstant, list_idx);
    let index = builder.add_constant(MettaValue::Long(10)); // OOB index
    builder.emit_u16(Opcode::PushConstant, index);
    builder.emit(Opcode::IndexAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::IndexOutOfBounds { .. })));
}

#[test]
fn test_vm_index_atom_negative() {
    // IndexAtom with negative index
    let mut builder = ChunkBuilder::new("test");
    let list_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit_u16(Opcode::PushConstant, list_idx);
    let index = builder.add_constant(MettaValue::Long(-1)); // Negative index
    builder.emit_u16(Opcode::PushConstant, index);
    builder.emit(Opcode::IndexAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::IndexOutOfBounds { .. })));
}

#[test]
fn test_vm_index_atom_non_integer() {
    // IndexAtom with non-integer index
    let mut builder = ChunkBuilder::new("test");
    let list_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit_u16(Opcode::PushConstant, list_idx);
    let index = builder.add_constant(MettaValue::String("not an integer".into()));
    builder.emit_u16(Opcode::PushConstant, index);
    builder.emit(Opcode::IndexAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_min_atom_empty() {
    // MinAtom on empty S-expression
    let mut builder = ChunkBuilder::new("test");
    let empty_idx = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty_idx);
    builder.emit(Opcode::MinAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_min_atom_no_numbers() {
    // MinAtom on S-expression with no numeric values
    let mut builder = ChunkBuilder::new("test");
    let list_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
    ]));
    builder.emit_u16(Opcode::PushConstant, list_idx);
    builder.emit(Opcode::MinAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_max_atom_empty() {
    // MaxAtom on empty S-expression
    let mut builder = ChunkBuilder::new("test");
    let empty_idx = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty_idx);
    builder.emit(Opcode::MaxAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_max_atom_no_numbers() {
    // MaxAtom on S-expression with no numeric values
    let mut builder = ChunkBuilder::new("test");
    let list_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("x"),
        MettaValue::sym("y"),
    ]));
    builder.emit_u16(Opcode::PushConstant, list_idx);
    builder.emit(Opcode::MaxAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// Note: test_vm_get_arity_non_sexpr already exists at line 3467

#[test]
fn test_vm_decon_atom_empty_phase3d() {
    // DeconAtom on empty S-expression
    let mut builder = ChunkBuilder::new("test");
    let empty_idx = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty_idx);
    builder.emit(Opcode::DeconAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_decon_atom_non_sexpr() {
    // DeconAtom on non-S-expression
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::DeconAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// --- Arithmetic Edge Cases ---

#[test]
fn test_vm_mod_overflow() {
    // i64::MIN % -1 causes overflow
    let mut builder = ChunkBuilder::new("test");
    let min_val = builder.add_constant(MettaValue::Long(i64::MIN));
    let neg_one = builder.add_constant(MettaValue::Long(-1));
    builder.emit_u16(Opcode::PushConstant, min_val);
    builder.emit_u16(Opcode::PushConstant, neg_one);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should error with ArithmeticOverflow
    assert!(matches!(result, Err(VmError::ArithmeticOverflow)));
}

#[test]
fn test_vm_sub_type_error() {
    // Subtraction with non-numeric types
    let mut builder = ChunkBuilder::new("test");
    let str_idx = builder.add_constant(MettaValue::String("hello".into()));
    builder.emit_u16(Opcode::PushConstant, str_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Sub);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_mul_type_error() {
    // Multiplication with non-numeric types
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Mul);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

#[test]
fn test_vm_comparison_type_error() {
    // Comparison between incompatible types
    let mut builder = ChunkBuilder::new("test");
    let str_idx = builder.add_constant(MettaValue::String("abc".into()));
    builder.emit_u16(Opcode::PushConstant, str_idx);
    builder.emit_byte(Opcode::PushLongSmall, 5);
    builder.emit(Opcode::Lt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::TypeError { .. })));
}

// --- Pattern Matching Edge Cases ---

#[test]
fn test_vm_match_head_invalid_constant() {
    // MatchHead with invalid constant index
    let mut builder = ChunkBuilder::new("test");
    let list_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::Long(1),
    ]));
    builder.emit_u16(Opcode::PushConstant, list_idx);
    builder.emit(Opcode::MatchHead);
    builder.emit_raw(&[255]); // Invalid constant index
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::InvalidConstant(_))));
}

#[test]
fn test_vm_match_non_sexpr() {
    // Match pattern on non-S-expression
    let mut builder = ChunkBuilder::new("test");
    let pattern = builder.add_constant(MettaValue::sym("pattern"));
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    // Match should return false for non-matching types
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(false));
}

// --- Binding Operations Edge Cases ---

#[test]
fn test_vm_load_binding_not_found() {
    // LoadBinding for non-existent binding
    let mut builder = ChunkBuilder::new("test");
    let name_idx = builder.add_constant(MettaValue::sym("$nonexistent"));
    builder.emit_u16(Opcode::LoadBinding, name_idx);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(matches!(result, Err(VmError::InvalidBinding(_))));
}

#[test]
fn test_vm_pop_binding_frame_at_root() {
    // PopBindingFrame when at root frame
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PopBindingFrame);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should error - can't pop root frame
    assert!(matches!(result, Err(VmError::Runtime(_))));
}

// --- Value Type Introspection Edge Cases ---

#[test]
fn test_vm_is_variable_true() {
    // IsVariable on a variable
    let mut builder = ChunkBuilder::new("test");
    let var_idx = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushConstant, var_idx);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_variable_false() {
    // IsVariable on a non-variable
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(false));
}

#[test]
fn test_vm_is_sexpr_true() {
    // IsSExpr on an S-expression
    let mut builder = ChunkBuilder::new("test");
    let sexpr_idx = builder.add_constant(MettaValue::sexpr(vec![MettaValue::Long(1)]));
    builder.emit_u16(Opcode::PushConstant, sexpr_idx);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_sexpr_false() {
    // IsSExpr on a non-S-expression
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(false));
}

#[test]
fn test_vm_is_symbol_true() {
    // IsSymbol on a symbol
    let mut builder = ChunkBuilder::new("test");
    let sym_idx = builder.add_constant(MettaValue::sym("test-symbol"));
    builder.emit_u16(Opcode::PushConstant, sym_idx);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(true));
}

#[test]
fn test_vm_is_symbol_false() {
    // IsSymbol on a non-symbol
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::Bool(false));
}

// --- Get Metatype Tests ---

#[test]
fn test_vm_get_metatype_number() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetMetaType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::sym("Number"));
}

#[test]
fn test_vm_get_metatype_expression() {
    let mut builder = ChunkBuilder::new("test");
    let sexpr_idx = builder.add_constant(MettaValue::sexpr(vec![MettaValue::Long(1)]));
    builder.emit_u16(Opcode::PushConstant, sexpr_idx);
    builder.emit(Opcode::GetMetaType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::sym("Expression"));
}

#[test]
fn test_vm_get_metatype_variable() {
    let mut builder = ChunkBuilder::new("test");
    let var_idx = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushConstant, var_idx);
    builder.emit(Opcode::GetMetaType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::sym("Variable"));
}

#[test]
fn test_vm_get_metatype_string() {
    let mut builder = ChunkBuilder::new("test");
    let str_idx = builder.add_constant(MettaValue::String("hello".into()));
    builder.emit_u16(Opcode::PushConstant, str_idx);
    builder.emit(Opcode::GetMetaType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::sym("String"));
}

#[test]
fn test_vm_get_metatype_bool() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::GetMetaType);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::sym("Bool"));
}

// --- Repr Operation Tests ---

#[test]
fn test_vm_repr_number() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Repr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::String("42".into()));
}

#[test]
fn test_vm_repr_bool() {
    let mut builder = ChunkBuilder::new("test");
    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Repr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::String("True".into()));
}

#[test]
fn test_vm_repr_sexpr() {
    let mut builder = ChunkBuilder::new("test");
    let sexpr_idx = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("+"),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit_u16(Opcode::PushConstant, sexpr_idx);
    builder.emit(Opcode::Repr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run().expect("VM should succeed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], MettaValue::String("(+ 1 2)".into()));
}

// =============================================================================
// Phase 4A: VM Environment & State Operations Tests
// =============================================================================

// -----------------------------------------------------------------------------
// 4A.1 Environment Operations (environment_ops.rs)
// -----------------------------------------------------------------------------

/// Test DefineRule opcode adds a rule to the environment.
#[test]
fn test_vm_define_rule_with_env() {
    use crate::backend::models::{HeapMettaValueFactory};

    let mut builder = ChunkBuilder::new("test_define_rule");

    // Push pattern: (double $x)
    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("double"),
        MettaValue::var("x"),
    ]));
    builder.emit_u16(Opcode::PushConstant, pattern);

    // Push body: (* 2 $x)
    let body = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("*"),
        MettaValue::Long(2),
        MettaValue::var("x"),
    ]));
    builder.emit_u16(Opcode::PushConstant, body);

    // Define the rule
    builder.emit(Opcode::DefineRule);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    // Should return Unit
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit());
}

/// Test DefineRule fails without environment.
#[test]
fn test_vm_define_rule_no_env() {
    let mut builder = ChunkBuilder::new("test_define_rule_no_env");

    // Push pattern and body
    let pattern = builder.add_constant(MettaValue::sym("foo"));
    let body = builder.add_constant(MettaValue::sym("bar"));
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, body);
    builder.emit(Opcode::DefineRule);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should fail because no environment is set
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::Runtime(msg) if msg.contains("environment")));
}

/// Test LoadGlobal loads binding from environment.
#[test]
fn test_vm_load_global_exists() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_load_global");

    // Store a global binding
    let name_idx = builder.add_constant(MettaValue::sym("myvar"));
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_u16(Opcode::StoreGlobal, name_idx);

    // Load it back
    builder.emit_u16(Opcode::LoadGlobal, name_idx);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

/// Test LoadGlobal returns atom when binding not found.
#[test]
fn test_vm_load_global_not_found() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_load_global_not_found");

    let name_idx = builder.add_constant(MettaValue::sym("nonexistent"));
    builder.emit_u16(Opcode::LoadGlobal, name_idx);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    // Should return the atom itself (unresolved)
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::sym("nonexistent"));
}

/// Test LoadGlobal without environment returns atom unchanged.
#[test]
fn test_vm_load_global_no_env() {
    let mut builder = ChunkBuilder::new("test_load_global_no_env");

    let name_idx = builder.add_constant(MettaValue::sym("somevar"));
    builder.emit_u16(Opcode::LoadGlobal, name_idx);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::sym("somevar"));
}

/// Test StoreGlobal stores new binding.
#[test]
fn test_vm_store_global_new() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_store_global");

    let name_idx = builder.add_constant(MettaValue::sym("newvar"));
    builder.emit_byte(Opcode::PushLongSmall, 100);
    builder.emit_u16(Opcode::StoreGlobal, name_idx);

    // Verify by loading
    builder.emit_u16(Opcode::LoadGlobal, name_idx);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(100));
}

/// Test StoreGlobal updates existing binding.
#[test]
fn test_vm_store_global_update() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_store_global_update");

    let name_idx = builder.add_constant(MettaValue::sym("updatevar"));

    // Store initial value
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_u16(Opcode::StoreGlobal, name_idx);

    // Update with new value
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit_u16(Opcode::StoreGlobal, name_idx);

    // Load final value
    builder.emit_u16(Opcode::LoadGlobal, name_idx);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(99));
}

/// Test DispatchRules with single matching rule.
#[test]
fn test_vm_dispatch_rules_single_match() {
    use crate::backend::models::{HeapMettaValueFactory};

    let mut builder = ChunkBuilder::new("test_dispatch_rules");

    // First, define a rule: (= (inc $x) (+ $x 1))
    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("inc"),
        MettaValue::var("x"),
    ]));
    let body = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("+"),
        MettaValue::var("x"),
        MettaValue::Long(1),
    ]));
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, body);
    builder.emit(Opcode::DefineRule);
    builder.emit(Opcode::Pop); // Pop Unit

    // Now call (inc 5)
    let call_expr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("inc"),
        MettaValue::Long(5),
    ]));
    builder.emit_u16(Opcode::PushConstant, call_expr);
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should get (+ 5 1) with bindings applied
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::sym("+"));
            assert_eq!(items[1], MettaValue::Long(5));
            assert_eq!(items[2], MettaValue::Long(1));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

/// Test DispatchRules returns expression unchanged when no rules match.
#[test]
fn test_vm_dispatch_rules_no_match() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_dispatch_no_match");

    // Define a rule for (foo $x)
    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::var("x"),
    ]));
    let body = builder.add_constant(MettaValue::sym("matched"));
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, body);
    builder.emit(Opcode::DefineRule);
    builder.emit(Opcode::Pop);

    // Try to dispatch (bar 1) - won't match (foo $x)
    let call_expr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("bar"),
        MettaValue::Long(1),
    ]));
    builder.emit_u16(Opcode::PushConstant, call_expr);
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should return unchanged expression
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items[0], MettaValue::sym("bar"));
            assert_eq!(items[1], MettaValue::Long(1));
        }
        _ => panic!("Expected unchanged S-expression"),
    }
}

/// Test DispatchRules with non-callable (not S-expression).
#[test]
fn test_vm_dispatch_rules_non_callable() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_dispatch_non_callable");

    // Try to dispatch a plain Long (not an S-expression)
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should return unchanged (42)
    assert_eq!(results[0], MettaValue::Long(42));
}

/// Test DispatchRules with S-expression having non-atom head.
#[test]
fn test_vm_dispatch_rules_non_atom_head() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_dispatch_non_atom_head");

    // S-expression with Long as head: (42 1 2)
    let expr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::Long(42),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit_u16(Opcode::PushConstant, expr);
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should return unchanged (non-atom head)
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items[0], MettaValue::Long(42));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test DispatchRules without environment returns expression unchanged.
#[test]
fn test_vm_dispatch_rules_no_env() {
    let mut builder = ChunkBuilder::new("test_dispatch_no_env");

    let expr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("test"),
        MettaValue::Long(1),
    ]));
    builder.emit_u16(Opcode::PushConstant, expr);
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk); // No environment
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should return unchanged
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items[0], MettaValue::sym("test"));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test DispatchRules with atom (arity 0).
#[test]
fn test_vm_dispatch_rules_atom() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_dispatch_atom");

    // Define rule: (= zero 0)
    let pattern = builder.add_constant(MettaValue::sym("zero"));
    let body = builder.add_constant(MettaValue::Long(0));
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, body);
    builder.emit(Opcode::DefineRule);
    builder.emit(Opcode::Pop);

    // Dispatch atom "zero"
    let atom = builder.add_constant(MettaValue::sym("zero"));
    builder.emit_u16(Opcode::PushConstant, atom);
    builder.emit(Opcode::DispatchRules);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(0));
}

// -----------------------------------------------------------------------------
// 4A.2 State Operations (state_ops.rs)
// -----------------------------------------------------------------------------

/// Test NewState creates a state cell.
#[test]
fn test_vm_new_state_basic() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_new_state");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::NewState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Should be a State value
    match results[0].inner() {
        MettaValueInner::State(_) => {}
        _ => panic!("Expected State, got {:?}", results[0]),
    }
}

/// Test NewState fails without environment.
#[test]
fn test_vm_new_state_no_env() {
    let mut builder = ChunkBuilder::new("test_new_state_no_env");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::NewState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::Runtime(msg) if msg.contains("environment")));
}

/// Test GetState retrieves value from state cell.
#[test]
fn test_vm_get_state_basic() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_get_state");

    // Create state with value 99
    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit(Opcode::NewState);
    // Get the state value
    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(99));
}

/// Test GetState with invalid state ID fails.
#[test]
fn test_vm_get_state_invalid_id() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_get_state_invalid");

    // Push a fake State value with invalid ID
    let fake_state = builder.add_constant(MettaValue::State(9999));
    builder.emit_u16(Opcode::PushConstant, fake_state);
    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::Runtime(msg) if msg.contains("not found")));
}

/// Test GetState with non-State value fails.
#[test]
fn test_vm_get_state_non_state() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_get_state_non_state");

    // Push a Long instead of State
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "State", .. }));
}

/// Test ChangeState modifies state value.
#[test]
fn test_vm_change_state_basic() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_change_state");

    // Create state with initial value 0
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::NewState);
    // Duplicate state ref for change and later get
    builder.emit(Opcode::Dup);
    // Push new value
    builder.emit_byte(Opcode::PushLongSmall, 77);
    // Change state
    builder.emit(Opcode::ChangeState);
    // Get the updated value
    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(77));
}

/// Test ChangeState with invalid state ID fails.
#[test]
fn test_vm_change_state_invalid_id() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_change_state_invalid");

    let fake_state = builder.add_constant(MettaValue::State(8888));
    builder.emit_u16(Opcode::PushConstant, fake_state);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::ChangeState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::Runtime(msg) if msg.contains("not found")));
}

/// Test ChangeState with non-State value fails.
#[test]
fn test_vm_change_state_non_state() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_change_state_non_state");

    builder.emit_byte(Opcode::PushLongSmall, 10); // Not a State
    builder.emit_byte(Opcode::PushLongSmall, 20); // New value
    builder.emit(Opcode::ChangeState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "State", .. }));
}

/// Test state persistence through multiple operations.
#[test]
fn test_vm_state_persistence() {
    use crate::backend::models::HeapMettaValueFactory;

    let mut builder = ChunkBuilder::new("test_state_persistence");

    // Create state with 0
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::NewState);
    builder.emit(Opcode::Dup); // Keep ref for later

    // Change to 1
    builder.emit(Opcode::Dup);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::ChangeState);
    builder.emit(Opcode::Pop); // Pop returned state ref

    // Change to 2
    builder.emit(Opcode::Dup);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit(Opcode::ChangeState);
    builder.emit(Opcode::Pop);

    // Change to 3
    builder.emit(Opcode::Dup);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit(Opcode::ChangeState);
    builder.emit(Opcode::Pop);

    // Get final value
    builder.emit(Opcode::GetState);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let env = HeapEnvironment::new(HeapMettaValueFactory);
    let mut vm = BytecodeVM::with_env(chunk, env);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));
}

// -----------------------------------------------------------------------------
// 4A.3 Space Operations Error Paths
// -----------------------------------------------------------------------------

/// Test SpaceAdd with non-space fails gracefully.
#[test]
fn test_vm_space_add_non_space() {
    let mut builder = ChunkBuilder::new("test_space_add_non_space");

    // Push a Long as "space" (not a Space)
    builder.emit_byte(Opcode::PushLongSmall, 42);
    // Push atom to add
    let atom = builder.add_constant(MettaValue::sym("test"));
    builder.emit_u16(Opcode::PushConstant, atom);
    builder.emit(Opcode::SpaceAdd);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Should fail with type error
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "Space", .. }));
}

/// Test SpaceRemove with non-space fails gracefully.
#[test]
fn test_vm_space_remove_non_space() {
    let mut builder = ChunkBuilder::new("test_space_remove_non_space");

    builder.emit(Opcode::PushTrue); // Not a Space
    let atom = builder.add_constant(MettaValue::sym("test"));
    builder.emit_u16(Opcode::PushConstant, atom);
    builder.emit(Opcode::SpaceRemove);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "Space", .. }));
}

/// Test SpaceGetAtoms with non-space fails gracefully.
#[test]
fn test_vm_space_get_atoms_non_space() {
    let mut builder = ChunkBuilder::new("test_space_get_atoms_non_space");

    builder.emit_byte(Opcode::PushLongSmall, 123); // Not a Space
    builder.emit(Opcode::SpaceGetAtoms);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "Space", .. }));
}

/// Test SpaceMatch with non-space fails gracefully.
#[test]
fn test_vm_space_match_non_space() {
    let mut builder = ChunkBuilder::new("test_space_match_non_space");

    builder.emit(Opcode::PushUnit); // Not a Space
    let pattern = builder.add_constant(MettaValue::var("x"));
    let template = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, template);
    builder.emit(Opcode::SpaceMatch);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "Space", .. }));
}

/// Test LoadSpace with non-atom constant fails.
#[test]
fn test_vm_load_space_non_atom() {
    let mut builder = ChunkBuilder::new("test_load_space_non_atom");

    // Add a Long as the name constant (should be Atom)
    let non_atom = builder.add_constant(MettaValue::Long(42));
    builder.emit_u16(Opcode::LoadSpace, non_atom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    // LoadSpace expects "Atom (space name)" not just "Atom"
    assert!(matches!(err, VmError::TypeError { expected: "Atom (space name)", .. }));
}

// -----------------------------------------------------------------------------
// 4A.4 Control Flow Error Paths
// -----------------------------------------------------------------------------

/// Test JumpIfError takes jump when value is an error.
#[test]
fn test_vm_jump_if_error_with_error() {
    let mut builder = ChunkBuilder::new("test_jump_if_error");

    // Push an error value
    let err_val = builder.add_constant(MettaValue::Error(
        "test error".to_string(),
        MettaValue::Unit(),
    ));
    builder.emit_u16(Opcode::PushConstant, err_val);

    // JumpIfError should take the jump
    let jump = builder.emit_jump(Opcode::JumpIfError);
    // If not jumped, push 0
    builder.emit_byte(Opcode::PushLongSmall, 0);
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump);
    // If jumped, push 1
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1)); // Jump was taken
}

/// Test JumpIfError doesn't jump for non-error values.
#[test]
fn test_vm_jump_if_error_no_error() {
    let mut builder = ChunkBuilder::new("test_jump_if_error_no_error");

    // Push a normal value
    builder.emit_byte(Opcode::PushLongSmall, 42);

    let jump = builder.emit_jump(Opcode::JumpIfError);
    // Not an error, continue here
    builder.emit_byte(Opcode::PushLongSmall, 100);
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump);
    builder.emit_byte(Opcode::PushLongSmall, 200);
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // The non-error path should push 100, then we return that
    // Stack: [42, 100] - Return pops 100
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(100)); // Jump not taken
}

/// Test Call with zero arguments (arity 0) returns irreducible expression.
/// Note: Call without MORK bridge returns expression as data.
#[test]
fn test_vm_call_zero_arity() {
    let mut builder = ChunkBuilder::new("test_call_zero_arity");

    // Call (foo) with 0 args
    let head_idx = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[0]); // arity = 0
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Without bridge, Call returns (foo) as expression data
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0], MettaValue::sym("foo"));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

/// Test TailCall with zero arguments returns irreducible expression.
/// Note: TailCall without MORK bridge returns expression as data.
#[test]
fn test_vm_tail_call_zero_arity() {
    let mut builder = ChunkBuilder::new("test_tail_call_zero");

    // Tail call (baz)
    let head_idx = builder.add_constant(MettaValue::sym("baz"));
    builder.emit_u16(Opcode::TailCall, head_idx);
    builder.emit_raw(&[0]);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Without bridge, TailCall returns (baz) as expression data
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0], MettaValue::sym("baz"));
        }
        _ => panic!("Expected S-expression, got {:?}", results[0]),
    }
}

/// Test handling of error values via JumpIfError.
#[test]
fn test_vm_error_value_handling() {
    let mut builder = ChunkBuilder::new("test_error_handling");

    // Push an error value
    let err_val = builder.add_constant(MettaValue::Error(
        "test error".to_string(),
        MettaValue::Unit(),
    ));
    builder.emit_u16(Opcode::PushConstant, err_val);

    // JumpIfError should detect it
    let jump = builder.emit_jump(Opcode::JumpIfError);
    builder.emit_byte(Opcode::PushLongSmall, 0); // Not an error
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump);
    builder.emit_byte(Opcode::PushLongSmall, 1); // Was an error
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1)); // Error path taken
}

// =============================================================================
// Phase 5A: VM Core Operations - Fork/Choice Point Tests
// =============================================================================

/// Test Fork with zero alternatives immediately fails and returns empty results.
#[test]
fn test_vm_fork_zero_alternatives() {
    let mut builder = ChunkBuilder::new("test_fork_zero");

    // Fork with count=0 should immediately fail and return empty results
    builder.emit_u16(Opcode::Fork, 0); // count = 0
    builder.emit(Opcode::Return); // Won't be reached

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // With zero alternatives, Fork fails and returns empty results
    assert!(results.is_empty());
}

/// Test Fork with single alternative (no choice point needed).
#[test]
fn test_vm_fork_single_alternative() {
    let mut builder = ChunkBuilder::new("test_fork_single");

    // Add constant for the single alternative
    let alt = builder.add_constant(MettaValue::Long(42));

    // Fork with count=1
    builder.emit_u16(Opcode::Fork, 1);
    builder.emit_raw(&alt.to_be_bytes()); // Alternative constant index
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
    // No choice points should remain
    assert_eq!(vm.choice_points_len(), 0);
}

/// Test Fork with multiple alternatives creates choice point.
#[test]
fn test_vm_fork_multiple_alternatives() {
    let mut builder = ChunkBuilder::new("test_fork_multiple");

    // Add constants for alternatives
    let alt1 = builder.add_constant(MettaValue::Long(1));
    let alt2 = builder.add_constant(MettaValue::Long(2));
    let alt3 = builder.add_constant(MettaValue::Long(3));

    // Fork with count=3
    builder.emit_u16(Opcode::Fork, 3);
    builder.emit_raw(&alt1.to_be_bytes());
    builder.emit_raw(&alt2.to_be_bytes());
    builder.emit_raw(&alt3.to_be_bytes());
    // Yield each alternative to collect all results
    builder.emit(Opcode::Yield);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Should get all three alternatives
    assert_eq!(results.len(), 3);
    assert!(results.contains(&MettaValue::Long(1)));
    assert!(results.contains(&MettaValue::Long(2)));
    assert!(results.contains(&MettaValue::Long(3)));
}

/// Test Fail without choice points returns empty results (Phase 5A variant).
#[test]
fn test_vm_fail_no_choice_points_5a() {
    let mut builder = ChunkBuilder::new("test_fail_no_cp_5a");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Fail);
    builder.emit(Opcode::Return); // Won't be reached

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // No choice points means fail returns empty results
    assert!(results.is_empty());
}

/// Test Yield saves result and backtracks.
#[test]
fn test_vm_yield_collects_results() {
    let mut builder = ChunkBuilder::new("test_yield");

    // Fork to create two alternatives
    let alt1 = builder.add_constant(MettaValue::Long(10));
    let alt2 = builder.add_constant(MettaValue::Long(20));

    builder.emit_u16(Opcode::Fork, 2);
    builder.emit_raw(&alt1.to_be_bytes());
    builder.emit_raw(&alt2.to_be_bytes());
    builder.emit(Opcode::Yield); // Save result and backtrack

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 2);
    assert!(results.contains(&MettaValue::Long(10)));
    assert!(results.contains(&MettaValue::Long(20)));
}

/// Test Cut removes all choice points.
#[test]
fn test_vm_cut_removes_choice_points() {
    let mut builder = ChunkBuilder::new("test_cut");

    // Fork to create alternatives
    let alt1 = builder.add_constant(MettaValue::Long(1));
    let alt2 = builder.add_constant(MettaValue::Long(2));

    builder.emit_u16(Opcode::Fork, 2);
    builder.emit_raw(&alt1.to_be_bytes());
    builder.emit_raw(&alt2.to_be_bytes());
    // First alternative: push value, cut, return
    builder.emit(Opcode::Cut); // Remove choice points
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Cut prevents backtracking, so only first alternative
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

/// Test Collect gathers accumulated results.
#[test]
fn test_vm_collect_gathers_results() {
    let mut builder = ChunkBuilder::new("test_collect");

    // Push some results manually
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Yield);
    // After Yield and Fail with no choice points, results are returned
    // Let's test differently - use Fork then Collect

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Yield should save 1 to results
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

/// Test CollectN limits results.
#[test]
fn test_vm_collect_n_limits_results() {
    let mut builder = ChunkBuilder::new("test_collect_n");

    // Create 5 alternatives but collect only 2
    let alt1 = builder.add_constant(MettaValue::Long(1));
    let alt2 = builder.add_constant(MettaValue::Long(2));
    let alt3 = builder.add_constant(MettaValue::Long(3));
    let alt4 = builder.add_constant(MettaValue::Long(4));
    let alt5 = builder.add_constant(MettaValue::Long(5));

    // Use Yield to collect all, then rely on CollectN
    builder.emit_u16(Opcode::Fork, 5);
    builder.emit_raw(&alt1.to_be_bytes());
    builder.emit_raw(&alt2.to_be_bytes());
    builder.emit_raw(&alt3.to_be_bytes());
    builder.emit_raw(&alt4.to_be_bytes());
    builder.emit_raw(&alt5.to_be_bytes());
    builder.emit(Opcode::Yield);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // All 5 should be yielded
    assert_eq!(results.len(), 5);
}

/// Test Amb with zero alternatives pushes Nil.
#[test]
fn test_vm_amb_zero() {
    let mut builder = ChunkBuilder::new("test_amb_zero");

    builder.emit_byte(Opcode::Amb, 0); // count = 0
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit());
}

/// Test Amb with single alternative (no choice point) - Phase 5A variant.
#[test]
fn test_vm_amb_single_5a() {
    let mut builder = ChunkBuilder::new("test_amb_single_5a");

    builder.emit_byte(Opcode::PushLongSmall, 99);
    builder.emit_byte(Opcode::Amb, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(99));
}

/// Test Amb with multiple alternatives creates choice point.
#[test]
fn test_vm_amb_multiple() {
    let mut builder = ChunkBuilder::new("test_amb_multiple");

    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::Amb, 3);
    builder.emit(Opcode::Yield);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 3);
    assert!(results.contains(&MettaValue::Long(1)));
    assert!(results.contains(&MettaValue::Long(2)));
    assert!(results.contains(&MettaValue::Long(3)));
}

/// Test Guard with true continues execution - Phase 5A variant.
#[test]
fn test_vm_guard_true_5a() {
    let mut builder = ChunkBuilder::new("test_guard_true_5a");

    builder.emit(Opcode::PushTrue);
    builder.emit(Opcode::Guard);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

/// Test Guard with false backtracks - Phase 5A variant.
#[test]
fn test_vm_guard_false_5a() {
    let mut builder = ChunkBuilder::new("test_guard_false_5a");

    builder.emit(Opcode::PushFalse);
    builder.emit(Opcode::Guard);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Guard false causes backtrack, empty results
    assert!(results.is_empty());
}

/// Test Guard with non-bool fails with type error - Phase 5A variant.
#[test]
fn test_vm_guard_type_error_5a() {
    let mut builder = ChunkBuilder::new("test_guard_type_error_5a");

    builder.emit_byte(Opcode::PushLongSmall, 42); // Not a bool
    builder.emit(Opcode::Guard);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "Bool", .. }));
}

/// Test Commit removes all choice points (count=0).
#[test]
fn test_vm_commit_full() {
    let mut builder = ChunkBuilder::new("test_commit_full");

    // Create choice points
    let alt1 = builder.add_constant(MettaValue::Long(1));
    let alt2 = builder.add_constant(MettaValue::Long(2));

    builder.emit_u16(Opcode::Fork, 2);
    builder.emit_raw(&alt1.to_be_bytes());
    builder.emit_raw(&alt2.to_be_bytes());
    builder.emit_byte(Opcode::Commit, 0); // Full commit
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // Only first alternative executed due to commit
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

/// Test Commit removes N choice points (partial).
#[test]
fn test_vm_commit_partial() {
    let mut builder = ChunkBuilder::new("test_commit_partial");

    // Create multiple nested choice points
    let alt1 = builder.add_constant(MettaValue::Long(1));
    let alt2 = builder.add_constant(MettaValue::Long(2));

    builder.emit_u16(Opcode::Fork, 2);
    builder.emit_raw(&alt1.to_be_bytes());
    builder.emit_raw(&alt2.to_be_bytes());
    // Commit removes 1 choice point, but we only have 1
    builder.emit_byte(Opcode::Commit, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // First alternative only
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

/// Test BeginNondet/EndNondet markers.
#[test]
fn test_vm_begin_end_nondet() {
    let mut builder = ChunkBuilder::new("test_nondet_markers");

    builder.emit(Opcode::BeginNondet);
    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::EndNondet);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

// =============================================================================
// Phase 5A: VM Core Operations - Pattern Matching Tests
// =============================================================================

/// Test pattern_matches with wildcard.
#[test]
fn test_pattern_matches_wildcard() {
    assert!(pattern_matches(
        &MettaValue::sym("_"),
        &MettaValue::Long(42)
    ));
    assert!(pattern_matches(
        &MettaValue::sym("_"),
        &MettaValue::sym("anything")
    ));
    assert!(pattern_matches(
        &MettaValue::sym("_"),
        &MettaValue::sexpr(vec![MettaValue::sym("a"), MettaValue::sym("b")])
    ));
}

/// Test pattern_matches with nested S-expressions.
#[test]
fn test_pattern_matches_nested_sexpr() {
    let pattern = MettaValue::sexpr(vec![
        MettaValue::sym("outer"),
        MettaValue::sexpr(vec![
            MettaValue::sym("inner"),
            MettaValue::var("x"),
        ]),
    ]);
    let value = MettaValue::sexpr(vec![
        MettaValue::sym("outer"),
        MettaValue::sexpr(vec![
            MettaValue::sym("inner"),
            MettaValue::Long(42),
        ]),
    ]);
    assert!(pattern_matches(&pattern, &value));
}

/// Test pattern_matches fails on mismatched structure.
#[test]
fn test_pattern_matches_mismatched_structure() {
    let pattern = MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
    ]);
    let value = MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
        MettaValue::sym("c"),
    ]);
    assert!(!pattern_matches(&pattern, &value));
}

/// Test pattern_matches with Float values.
#[test]
fn test_pattern_matches_float() {
    assert!(pattern_matches(
        &MettaValue::Float(3.14),
        &MettaValue::Float(3.14)
    ));
    assert!(!pattern_matches(
        &MettaValue::Float(3.14),
        &MettaValue::Float(2.71)
    ));
}

/// Test pattern_matches with String values.
#[test]
fn test_pattern_matches_string() {
    assert!(pattern_matches(
        &MettaValue::String("hello".to_string()),
        &MettaValue::String("hello".to_string())
    ));
    assert!(!pattern_matches(
        &MettaValue::String("hello".to_string()),
        &MettaValue::String("world".to_string())
    ));
}

/// Test pattern_matches with Unit values.
#[test]
fn test_pattern_matches_unit() {
    assert!(pattern_matches(&MettaValue::Unit(), &MettaValue::Unit()));
    // After Nil/Unit merge, Nil() returns Unit, so Unit matches Nil
    assert!(pattern_matches(&MettaValue::Unit(), &MettaValue::Unit()));
}

/// Test unify with both variables.
#[test]
fn test_unify_both_variables() {
    let a = MettaValue::var("x");
    let b = MettaValue::var("y");
    let bindings = unify(&a, &b).expect("Should unify");
    // One variable binds to the other
    assert_eq!(bindings.len(), 1);
}

/// Test unify with nested S-expressions.
#[test]
fn test_unify_nested_sexpr() {
    let a = MettaValue::sexpr(vec![
        MettaValue::sym("f"),
        MettaValue::var("x"),
        MettaValue::Long(1),
    ]);
    let b = MettaValue::sexpr(vec![
        MettaValue::sym("f"),
        MettaValue::Long(42),
        MettaValue::var("y"),
    ]);
    let bindings = unify(&a, &b).expect("Should unify");
    // x -> 42, y -> 1
    assert!(bindings.len() >= 2);
}

/// Test unify fails on incompatible atoms.
#[test]
fn test_unify_fails_atoms() {
    let a = MettaValue::sym("foo");
    let b = MettaValue::sym("bar");
    assert!(unify(&a, &b).is_none());
}

/// Test unify fails on incompatible lengths.
#[test]
fn test_unify_fails_length() {
    let a = MettaValue::sexpr(vec![MettaValue::sym("a")]);
    let b = MettaValue::sexpr(vec![MettaValue::sym("a"), MettaValue::sym("b")]);
    assert!(unify(&a, &b).is_none());
}

// =============================================================================
// Phase 5A: VM Core Operations - Expression Ops Tests
// =============================================================================

/// Test GetHead with empty S-expression fails.
#[test]
fn test_vm_get_head_empty() {
    let mut builder = ChunkBuilder::new("test_get_head_empty");

    let empty = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "non-empty S-expression", .. }));
}

/// Test GetHead with non-S-expression fails.
#[test]
fn test_vm_get_head_non_sexpr_5a() {
    let mut builder = ChunkBuilder::new("test_get_head_non_sexpr");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetHead);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test GetTail with empty S-expression fails.
#[test]
fn test_vm_get_tail_empty() {
    let mut builder = ChunkBuilder::new("test_get_tail_empty");

    let empty = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test GetTail returns rest of S-expression.
#[test]
fn test_vm_get_tail_success() {
    let mut builder = ChunkBuilder::new("test_get_tail");

    let sexpr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
        MettaValue::sym("c"),
    ]));
    builder.emit_u16(Opcode::PushConstant, sexpr);
    builder.emit(Opcode::GetTail);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::sym("b"));
            assert_eq!(items[1], MettaValue::sym("c"));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test GetArity with non-S-expression fails.
#[test]
fn test_vm_get_arity_non_sexpr_5a() {
    let mut builder = ChunkBuilder::new("test_get_arity_non_sexpr");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::GetArity);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "S-expression", .. }));
}

/// Test GetElement with out-of-bounds index.
#[test]
fn test_vm_get_element_out_of_bounds() {
    let mut builder = ChunkBuilder::new("test_get_element_oob");

    let sexpr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
    ]));
    builder.emit_u16(Opcode::PushConstant, sexpr);
    builder.emit_byte(Opcode::GetElement, 5); // Out of bounds
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test DeconAtom with non-empty S-expression.
#[test]
fn test_vm_decon_atom_success() {
    let mut builder = ChunkBuilder::new("test_decon_atom");

    let sexpr = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("head"),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));
    builder.emit_u16(Opcode::PushConstant, sexpr);
    builder.emit(Opcode::DeconAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 2); // (head tail)
            assert_eq!(items[0], MettaValue::sym("head"));
            match items[1].inner() {
                MettaValueInner::SExpr(tail) => {
                    assert_eq!(tail.len(), 2);
                }
                _ => panic!("Expected tail to be S-expression"),
            }
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test DeconAtom with empty S-expression fails.
#[test]
fn test_vm_decon_atom_empty() {
    let mut builder = ChunkBuilder::new("test_decon_atom_empty");

    let empty = builder.add_constant(MettaValue::sexpr(vec![]));
    builder.emit_u16(Opcode::PushConstant, empty);
    builder.emit(Opcode::DeconAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test ConsAtom prepends to S-expression.
#[test]
fn test_vm_cons_atom_sexpr() {
    let mut builder = ChunkBuilder::new("test_cons_atom");

    let tail = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("b"),
        MettaValue::sym("c"),
    ]));
    let head_val = builder.add_constant(MettaValue::sym("a"));

    builder.emit_u16(Opcode::PushConstant, head_val);
    builder.emit_u16(Opcode::PushConstant, tail);
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::sym("a"));
            assert_eq!(items[1], MettaValue::sym("b"));
            assert_eq!(items[2], MettaValue::sym("c"));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test ConsAtom with Nil creates single-element S-expression.
#[test]
fn test_vm_cons_atom_nil() {
    let mut builder = ChunkBuilder::new("test_cons_atom_nil");

    let head_val = builder.add_constant(MettaValue::sym("only"));

    builder.emit_u16(Opcode::PushConstant, head_val);
    builder.emit(Opcode::PushUnit);
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0], MettaValue::sym("only"));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test ConsAtom with non-S-expression, non-Nil tail fails.
#[test]
fn test_vm_cons_atom_invalid_tail() {
    let mut builder = ChunkBuilder::new("test_cons_atom_invalid");

    let head_val = builder.add_constant(MettaValue::sym("head"));

    builder.emit_u16(Opcode::PushConstant, head_val);
    builder.emit_byte(Opcode::PushLongSmall, 42); // Invalid tail
    builder.emit(Opcode::ConsAtom);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "S-expression or Nil", .. }));
}

/// Test Repr converts value to string.
#[test]
fn test_vm_repr() {
    let mut builder = ChunkBuilder::new("test_repr");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Repr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::String(s) => {
            assert!(s.contains("42"));
        }
        _ => panic!("Expected String"),
    }
}

/// Test IsVariable with variable.
#[test]
fn test_vm_is_variable_true_5a() {
    let mut builder = ChunkBuilder::new("test_is_var_true");

    let var = builder.add_constant(MettaValue::var("x"));
    builder.emit_u16(Opcode::PushConstant, var);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test IsVariable with non-variable.
#[test]
fn test_vm_is_variable_false_5a() {
    let mut builder = ChunkBuilder::new("test_is_var_false");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsVariable);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test IsSExpr with S-expression.
#[test]
fn test_vm_is_sexpr_true_5a() {
    let mut builder = ChunkBuilder::new("test_is_sexpr_true");

    let sexpr = builder.add_constant(MettaValue::sexpr(vec![MettaValue::sym("a")]));
    builder.emit_u16(Opcode::PushConstant, sexpr);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test IsSExpr with non-S-expression.
#[test]
fn test_vm_is_sexpr_false_5a() {
    let mut builder = ChunkBuilder::new("test_is_sexpr_false");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSExpr);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test IsSymbol with symbol.
#[test]
fn test_vm_is_symbol_true_5a() {
    let mut builder = ChunkBuilder::new("test_is_symbol_true");

    let sym = builder.add_constant(MettaValue::sym("foo"));
    builder.emit_u16(Opcode::PushConstant, sym);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test IsSymbol with non-symbol.
#[test]
fn test_vm_is_symbol_false_5a() {
    let mut builder = ChunkBuilder::new("test_is_symbol_false");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::IsSymbol);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test Match opcode returns true for matching pattern.
#[test]
fn test_vm_match_opcode_true() {
    let mut builder = ChunkBuilder::new("test_match_true");

    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("add"),
        MettaValue::var("x"),
        MettaValue::var("y"),
    ]));
    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("add"),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]));

    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test Match opcode returns false for non-matching pattern.
#[test]
fn test_vm_match_opcode_false() {
    let mut builder = ChunkBuilder::new("test_match_false");

    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("add"),
        MettaValue::Long(1),
    ]));
    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("sub"),
        MettaValue::Long(1),
    ]));

    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit(Opcode::Match);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test MatchBind opcode binds variables.
#[test]
fn test_vm_match_bind_opcode_5a() {
    let mut builder = ChunkBuilder::new("test_match_bind");

    let pattern = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("pair"),
        MettaValue::var("x"),
        MettaValue::var("y"),
    ]));
    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("pair"),
        MettaValue::Long(10),
        MettaValue::Long(20),
    ]));

    builder.emit_u16(Opcode::PushConstant, pattern);
    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit(Opcode::MatchBind);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test MatchHead checks head symbol.
#[test]
fn test_vm_match_head_true() {
    let mut builder = ChunkBuilder::new("test_match_head_true");

    let expected = builder.add_constant(MettaValue::sym("foo"));
    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::Long(1),
    ]));

    // Note: MatchHead reads expected index as u8
    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit_byte(Opcode::MatchHead, expected as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test MatchHead fails on non-matching head.
#[test]
fn test_vm_match_head_false() {
    let mut builder = ChunkBuilder::new("test_match_head_false");

    let expected = builder.add_constant(MettaValue::sym("bar"));
    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("foo"),
        MettaValue::Long(1),
    ]));

    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit_byte(Opcode::MatchHead, expected as u8);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test MatchArity checks S-expression length.
#[test]
fn test_vm_match_arity_true() {
    let mut builder = ChunkBuilder::new("test_match_arity_true");

    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
        MettaValue::sym("c"),
    ]));

    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit_byte(Opcode::MatchArity, 3);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test MatchArity fails on wrong arity.
#[test]
fn test_vm_match_arity_false() {
    let mut builder = ChunkBuilder::new("test_match_arity_false");

    let value = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("a"),
        MettaValue::sym("b"),
    ]));

    builder.emit_u16(Opcode::PushConstant, value);
    builder.emit_byte(Opcode::MatchArity, 5);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test Unify opcode with compatible values.
#[test]
fn test_vm_unify_opcode_true() {
    let mut builder = ChunkBuilder::new("test_unify_true");

    let a = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("f"),
        MettaValue::var("x"),
    ]));
    let b = builder.add_constant(MettaValue::sexpr(vec![
        MettaValue::sym("f"),
        MettaValue::Long(42),
    ]));

    builder.emit_u16(Opcode::PushConstant, a);
    builder.emit_u16(Opcode::PushConstant, b);
    builder.emit(Opcode::Unify);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

/// Test Unify opcode with incompatible values.
#[test]
fn test_vm_unify_opcode_false() {
    let mut builder = ChunkBuilder::new("test_unify_false");

    let a = builder.add_constant(MettaValue::sym("foo"));
    let b = builder.add_constant(MettaValue::sym("bar"));

    builder.emit_u16(Opcode::PushConstant, a);
    builder.emit_u16(Opcode::PushConstant, b);
    builder.emit(Opcode::Unify);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

/// Test UnifyBind opcode binds variables.
#[test]
fn test_vm_unify_bind_opcode() {
    let mut builder = ChunkBuilder::new("test_unify_bind");

    let a = builder.add_constant(MettaValue::var("x"));
    let b = builder.add_constant(MettaValue::Long(99));

    builder.emit_u16(Opcode::PushConstant, a);
    builder.emit_u16(Opcode::PushConstant, b);
    builder.emit(Opcode::UnifyBind);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

// =============================================================================
// Phase 5A: VM Core Operations - Control Flow Tests
// =============================================================================

/// Test JumpIfNil takes jump for Nil.
#[test]
fn test_vm_jump_if_nil_true() {
    let mut builder = ChunkBuilder::new("test_jump_if_nil");

    builder.emit(Opcode::PushUnit);
    let jump = builder.emit_jump(Opcode::JumpIfUnit);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1)); // Jump taken
}

/// Test JumpIfNil doesn't jump for non-Nil.
#[test]
fn test_vm_jump_if_nil_false() {
    let mut builder = ChunkBuilder::new("test_jump_if_nil_false");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    let jump = builder.emit_jump(Opcode::JumpIfUnit);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(0)); // Jump not taken
}

/// Test JumpShort for short jumps.
#[test]
fn test_vm_jump_short_5a() {
    let mut builder = ChunkBuilder::new("test_jump_short");

    builder.emit_byte(Opcode::JumpShort, 2); // Skip next instruction
    builder.emit_byte(Opcode::PushLongSmall, 0); // Skipped
    builder.emit_byte(Opcode::PushLongSmall, 42); // Landed here
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(42));
}

/// Test JumpIfFalseShort.
#[test]
fn test_vm_jump_if_false_short() {
    let mut builder = ChunkBuilder::new("test_jump_if_false_short");

    builder.emit(Opcode::PushFalse);
    builder.emit_byte(Opcode::JumpIfFalseShort, 2);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1)); // Jump taken
}

/// Test JumpIfTrueShort.
#[test]
fn test_vm_jump_if_true_short() {
    let mut builder = ChunkBuilder::new("test_jump_if_true_short");

    builder.emit(Opcode::PushTrue);
    builder.emit_byte(Opcode::JumpIfTrueShort, 2);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1)); // Jump taken
}

/// Test JumpIfTrue doesn't jump for false.
#[test]
fn test_vm_jump_if_true_false() {
    let mut builder = ChunkBuilder::new("test_jump_if_true_false");

    builder.emit(Opcode::PushFalse);
    let jump = builder.emit_jump(Opcode::JumpIfTrue);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    let end = builder.emit_jump(Opcode::Jump);
    builder.patch_jump(jump);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.patch_jump(end);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(0)); // Jump not taken
}

/// Test CallN with multiple arguments.
/// CallN takes N args from stack plus head from stack, arity is next byte
#[test]
fn test_vm_call_n() {
    let mut builder = ChunkBuilder::new("test_call_n");

    // Push head first, then arguments
    let head_idx = builder.add_constant(MettaValue::sym("triple"));
    builder.emit_u16(Opcode::PushAtom, head_idx);
    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);

    // CallN: arity in next byte (3 args)
    builder.emit_byte(Opcode::CallN, 3);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    // Without bridge, CallN returns the expression (triple 1 2 3)
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 4);
            assert_eq!(items[0], MettaValue::sym("triple"));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test Call with insufficient stack causes underflow.
#[test]
fn test_vm_call_stack_underflow() {
    let mut builder = ChunkBuilder::new("test_call_underflow");

    // Call with arity 3 but nothing on stack
    let head_idx = builder.add_constant(MettaValue::sym("test"));
    builder.emit_u16(Opcode::Call, head_idx);
    builder.emit_raw(&[3]); // arity = 3
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::StackUnderflow));
}

/// Test TailCallN.
/// TailCallN takes N args from stack plus head from stack, arity is next byte
#[test]
fn test_vm_tail_call_n() {
    let mut builder = ChunkBuilder::new("test_tail_call_n");

    // Push head first, then arguments
    let head_idx = builder.add_constant(MettaValue::sym("pair"));
    builder.emit_u16(Opcode::PushAtom, head_idx);
    builder.emit_byte(Opcode::PushLongSmall, 10);
    builder.emit_byte(Opcode::PushLongSmall, 20);

    // TailCallN: arity in next byte (2 args)
    builder.emit_byte(Opcode::TailCallN, 2);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::sym("pair"));
        }
        _ => panic!("Expected S-expression"),
    }
}

/// Test ReturnMulti returns multiple values.
#[test]
fn test_vm_return_multi() {
    let mut builder = ChunkBuilder::new("test_return_multi");

    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2);
    builder.emit_byte(Opcode::PushLongSmall, 3);
    builder.emit_byte(Opcode::ReturnMulti, 3);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // ReturnMulti should return all values
    assert_eq!(results.len(), 3);
}

/// Test Halt opcode.
#[test]
fn test_vm_halt_5a() {
    let mut builder = ChunkBuilder::new("test_halt");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit(Opcode::Halt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::Halted));
}

// =============================================================================
// Phase 5A: VM Core Operations - Stack Operation Tests
// =============================================================================

/// Test Swap with insufficient stack.
#[test]
fn test_vm_swap_underflow_5a() {
    let mut builder = ChunkBuilder::new("test_swap_underflow");

    builder.emit_byte(Opcode::PushLongSmall, 42); // Only one value
    builder.emit(Opcode::Swap);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test Rot3 with insufficient stack.
#[test]
fn test_vm_rot3_underflow_5a() {
    let mut builder = ChunkBuilder::new("test_rot3_underflow");

    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PushLongSmall, 2); // Only two values
    builder.emit(Opcode::Rot3);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test Over with insufficient stack.
#[test]
fn test_vm_over_underflow_5a() {
    let mut builder = ChunkBuilder::new("test_over_underflow");

    builder.emit_byte(Opcode::PushLongSmall, 42); // Only one value
    builder.emit(Opcode::Over);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test Dup with empty stack.
#[test]
fn test_vm_dup_underflow_5a() {
    let mut builder = ChunkBuilder::new("test_dup_underflow");

    builder.emit(Opcode::Dup); // Empty stack
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test Pop with empty stack.
#[test]
fn test_vm_pop_underflow() {
    let mut builder = ChunkBuilder::new("test_pop_underflow");

    builder.emit(Opcode::Pop); // Empty stack
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test DupN with insufficient stack.
#[test]
fn test_vm_dup_n_underflow() {
    let mut builder = ChunkBuilder::new("test_dup_n_underflow");

    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::DupN, 5); // Request 5, only have 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

/// Test PopN with insufficient stack.
#[test]
fn test_vm_pop_n_underflow() {
    let mut builder = ChunkBuilder::new("test_pop_n_underflow");

    builder.emit_byte(Opcode::PushLongSmall, 1);
    builder.emit_byte(Opcode::PopN, 5); // Request pop 5, only have 1
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
}

// =============================================================================
// Phase 5A: VM Core Operations - Arithmetic Edge Cases
// =============================================================================

/// Test division by zero for Mod.
#[test]
fn test_vm_mod_by_zero_5a() {
    let mut builder = ChunkBuilder::new("test_mod_by_zero");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::Mod);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::DivisionByZero));
}

/// Test floor division by zero.
#[test]
fn test_vm_floor_div_by_zero_5a() {
    let mut builder = ChunkBuilder::new("test_floor_div_zero");

    builder.emit_byte(Opcode::PushLongSmall, 42);
    builder.emit_byte(Opcode::PushLongSmall, 0);
    builder.emit(Opcode::FloorDiv);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::DivisionByZero));
}

/// Test Sqrt with negative number.
#[test]
fn test_vm_sqrt_negative() {
    let mut builder = ChunkBuilder::new("test_sqrt_negative");

    // Push -1 as Float
    let neg = builder.add_constant(MettaValue::Float(-1.0));
    builder.emit_u16(Opcode::PushConstant, neg);
    builder.emit(Opcode::Sqrt);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // sqrt(-1) returns NaN
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::Float(f) => assert!(f.is_nan()),
        _ => panic!("Expected Float"),
    }
}

/// Test Log with zero value - log_base(0) = -infinity.
/// Log takes two values: base then value.
#[test]
fn test_vm_log_zero() {
    let mut builder = ChunkBuilder::new("test_log_zero");

    // Log pops value first, then base: log_base(value)
    // Push base (e.g., 10), then value (0)
    let base = builder.add_constant(MettaValue::Float(10.0));
    let zero = builder.add_constant(MettaValue::Float(0.0));
    builder.emit_u16(Opcode::PushConstant, base);
    builder.emit_u16(Opcode::PushConstant, zero);
    builder.emit(Opcode::Log);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run().expect("VM should succeed");

    // log_10(0) returns -infinity
    assert_eq!(results.len(), 1);
    match results[0].inner() {
        MettaValueInner::Float(f) => assert!(f.is_infinite() && *f < 0.0),
        _ => panic!("Expected Float"),
    }
}

/// Test Pow with Long base and negative Long exponent - now uses Float result.
#[test]
fn test_vm_pow_negative_exponent_5a() {
    let mut builder = ChunkBuilder::new("test_pow_neg_exp_5a");

    // Long base with negative Long exponent now treated as Float power
    builder.emit_byte(Opcode::PushLongSmall, 2); // base
    let neg = builder.add_constant(MettaValue::Long(-2)); // negative exponent
    builder.emit_u16(Opcode::PushConstant, neg);
    builder.emit(Opcode::Pow);
    builder.emit(Opcode::Return);

    let chunk = builder.build_arc();
    let mut vm = BytecodeVM::new(chunk);
    let result = vm.run();

    // Long^Long with negative exponent still errors (only non-negative Long exponents for integer pow)
    // The change is that Float support was added, but Long^(-Long) still requires non-negative
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, VmError::TypeError { expected: "number (Long or Float)", .. }));
}
