//! Bytecode compilation and execution for MeTTa
//!
//! This module provides a bytecode representation for MeTTa expressions,
//! enabling faster execution through a virtual machine interpreter.
//!
//! # Module Structure
//!
//! - `opcodes`: Bytecode instruction definitions

pub mod opcodes;

pub use opcodes::Opcode;
