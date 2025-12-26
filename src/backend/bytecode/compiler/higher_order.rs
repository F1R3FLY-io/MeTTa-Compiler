//! Higher-order list operation compilation for the bytecode compiler.
//!
//! This module implements compilation of higher-order list operations:
//! - map-atom: Apply a template to each element
//! - filter-atom: Filter elements by predicate
//! - foldl-atom: Left fold with accumulator

use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::MettaValue;

use super::error::{CompileError, CompileResult};
use super::Compiler;

impl Compiler {
    /// Compile map-atom: (map-atom list $var template)
    pub(crate) fn compile_map_atom(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("map-atom", args.len(), 3)?;

        let list = &args[0];
        let var = &args[1];
        let template = &args[2];

        // Extract variable name (must be $var format)
        let var_name = match var {
            MettaValue::Atom(s) if s.starts_with('$') => s[1..].to_string(),
            _ => return Err(CompileError::InvalidExpression(
                "map-atom variable must be $var".to_string()
            )),
        };

        // Compile list expression first
        self.compile(list)?;

        // Compile template as a sub-chunk with var as parameter
        let template_chunk_idx = self.compile_template_chunk(template, &[var_name])?;

        // Emit MapAtom with chunk index
        self.builder.emit_u16(Opcode::MapAtom, template_chunk_idx);

        Ok(())
    }

    /// Compile filter-atom: (filter-atom list $var predicate)
    pub(crate) fn compile_filter_atom(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("filter-atom", args.len(), 3)?;

        let list = &args[0];
        let var = &args[1];
        let predicate = &args[2];

        // Extract variable name
        let var_name = match var {
            MettaValue::Atom(s) if s.starts_with('$') => s[1..].to_string(),
            _ => return Err(CompileError::InvalidExpression(
                "filter-atom variable must be $var".to_string()
            )),
        };

        // Compile list expression first
        self.compile(list)?;

        // Compile predicate as a sub-chunk with var as parameter
        let predicate_chunk_idx = self.compile_template_chunk(predicate, &[var_name])?;

        // Emit FilterAtom with chunk index
        self.builder.emit_u16(Opcode::FilterAtom, predicate_chunk_idx);

        Ok(())
    }

    /// Compile foldl-atom: (foldl-atom list init $acc $item op)
    pub(crate) fn compile_foldl_atom(&mut self, args: &[MettaValue]) -> CompileResult<()> {
        self.check_arity("foldl-atom", args.len(), 5)?;

        let list = &args[0];
        let init = &args[1];
        let acc_var = &args[2];
        let item_var = &args[3];
        let op = &args[4];

        // Extract variable names
        let acc_name = match acc_var {
            MettaValue::Atom(s) if s.starts_with('$') => s[1..].to_string(),
            _ => return Err(CompileError::InvalidExpression(
                "foldl-atom accumulator must be $var".to_string()
            )),
        };

        let item_name = match item_var {
            MettaValue::Atom(s) if s.starts_with('$') => s[1..].to_string(),
            _ => return Err(CompileError::InvalidExpression(
                "foldl-atom item must be $var".to_string()
            )),
        };

        // Compile list expression first (will be popped second)
        self.compile(list)?;
        // Compile initial value (will be popped first)
        self.compile(init)?;

        // Compile operation as a sub-chunk with acc and item as parameters
        // Parameters: slot 0 = acc, slot 1 = item
        let op_chunk_idx = self.compile_template_chunk(op, &[acc_name, item_name])?;

        // Emit FoldlAtom with chunk index
        self.builder.emit_u16(Opcode::FoldlAtom, op_chunk_idx);

        Ok(())
    }

    /// Compile a template expression as a sub-chunk with parameter bindings
    pub(crate) fn compile_template_chunk(&mut self, template: &MettaValue, params: &[String]) -> CompileResult<u16> {
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
        sub_compiler.builder.set_local_count(sub_compiler.context.local_count());
        let sub_chunk = sub_compiler.builder.build();

        // Add to parent's sub-chunk pool
        let idx = self.builder.add_chunk_constant(sub_chunk);

        Ok(idx)
    }
}
