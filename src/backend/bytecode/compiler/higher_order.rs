//! Higher-order list operation compilation for the bytecode compiler.
//!
//! This module provides template chunk compilation used by the iterative
//! compiler for higher-order list operations (map-atom, filter-atom, foldl-atom).

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

use super::error::CompileResult;
use super::Compiler;

impl Compiler {
    /// Compile a template expression as a sub-chunk with parameter bindings
    pub(crate) fn compile_template_chunk(
        &mut self,
        template: &MettaValue,
        params: &[String],
    ) -> CompileResult<u16> {
        // Create a new compiler for the sub-chunk
        let mut sub_compiler = Compiler::new(format!("{}_template", self.builder.name()));

        // Declare parameters as locals (in order)
        for param in params {
            sub_compiler.context.declare_local(param.clone())?;
        }

        // Compile the template expression
        sub_compiler.compile(template)?;

        // Add return
        sub_compiler.builder.emit(Opcode::Return);

        // Build the sub-chunk
        sub_compiler
            .builder
            .set_local_count(sub_compiler.context.local_count());
        let sub_chunk = sub_compiler.builder.build();

        // Add to parent's sub-chunk pool
        let idx = self.builder.add_chunk_constant(sub_chunk);

        Ok(idx)
    }
}
