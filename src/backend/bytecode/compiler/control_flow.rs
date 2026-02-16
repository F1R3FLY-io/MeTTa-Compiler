//! Quoting compilation for the bytecode compiler.
//!
//! This module implements compilation of quoted expressions.

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::error::CompileResult;
use super::Compiler;

impl Compiler {
    /// Compile a quoted expression (no evaluation)
    pub(crate) fn compile_quoted(&mut self, expr: &MettaValue) -> CompileResult<()> {
        match expr.inner() {
            // Atoms can be pushed directly
            MettaValueInner::Atom(name) => {
                let idx = self.builder.add_constant(MettaValue::Atom(*name));
                if name.starts_with('$') {
                    self.builder.emit_u16(Opcode::PushVariable, idx);
                } else {
                    self.builder.emit_u16(Opcode::PushAtom, idx);
                }
            }
            // S-expressions need to be built
            MettaValueInner::SExpr(items) => {
                for item in *items {
                    self.compile_quoted(item)?;
                }
                if items.len() <= 255 {
                    self.builder.emit_byte(Opcode::MakeSExpr, items.len() as u8);
                } else {
                    self.builder
                        .emit_u16(Opcode::MakeSExprLarge, items.len() as u16);
                }
            }
            // Other values can be compiled normally (they're already values)
            _ => self.compile(expr)?,
        }
        Ok(())
    }
}
