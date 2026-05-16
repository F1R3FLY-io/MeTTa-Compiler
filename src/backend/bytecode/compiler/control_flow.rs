//! Quoting compilation for the bytecode compiler.
//!
//! This module implements compilation of quoted expressions.

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::{MettaValue, ValueView};

use super::error::CompileResult;
use super::Compiler;

impl Compiler {
    /// Compile a quoted expression (no evaluation).
    ///
    /// **Stack-safety mandate (2026-05-15)**: refactored to iterative work-list
    /// (audit item T3.1). Was recursive on SExpr children — deeply-nested
    /// quoted forms (e.g., `(quote (quote (... (quote x))))`) would overflow.
    pub(crate) fn compile_quoted(&mut self, expr: &MettaValue) -> CompileResult<()> {
        enum Work {
            Process(MettaValue),
            EmitMakeSExpr(usize),
        }

        let mut work: Vec<Work> = Vec::with_capacity(8);
        work.push(Work::Process(expr.clone()));

        while let Some(w) = work.pop() {
            match w {
                Work::Process(e) => match e.view() {
                    ValueView::Atom(name) => {
                        let idx = self.builder.add_constant(MettaValue::Atom(name));
                        if name.starts_with('$') {
                            self.builder.emit_u16(Opcode::PushVariable, idx);
                        } else {
                            self.builder.emit_u16(Opcode::PushAtom, idx);
                        }
                    }
                    ValueView::SExpr(items) => {
                        let len = items.len();
                        work.push(Work::EmitMakeSExpr(len));
                        // Push children in reverse so the first child is
                        // compiled first (its push opcode emitted first).
                        for item in items.iter().rev() {
                            work.push(Work::Process(item.clone()));
                        }
                    }
                    _ => {
                        // Other values compile normally — they're already values.
                        self.compile(&e)?;
                    }
                },
                Work::EmitMakeSExpr(count) => {
                    if count <= 255 {
                        self.builder.emit_byte(Opcode::MakeSExpr, count as u8);
                    } else {
                        self.builder
                            .emit_u16(Opcode::MakeSExprLarge, count as u16);
                    }
                }
            }
        }
        Ok(())
    }
}
