//! Bytecode chunk representation
//!
//! A BytecodeChunk contains compiled bytecode along with its constant pool,
//! source location mapping, and metadata needed for execution and debugging.

use std::sync::Arc;
use smallvec::SmallVec;

use crate::backend::models::MettaValue;
use super::opcodes::Opcode;

/// A compiled bytecode chunk
///
/// Contains the bytecode instructions, constant pool, and metadata.
/// Chunks are immutable after compilation and can be shared across threads.
#[derive(Debug, Clone)]
pub struct BytecodeChunk {
    /// The bytecode instructions
    code: Vec<u8>,

    /// Constant pool for values that can't be encoded inline
    constants: Vec<MettaValue>,

    /// Source line information: (byte_offset, line_number)
    /// Sorted by byte_offset for binary search
    line_info: Vec<(usize, u32)>,

    /// Jump tables for switch statements
    jump_tables: Vec<JumpTable>,

    /// Name of this chunk (for debugging)
    name: String,

    /// Number of local slots needed
    local_count: u16,

    /// Number of upvalues captured
    upvalue_count: u16,

    /// Arity (number of parameters) if this is a function
    arity: u8,

    /// Whether this chunk uses varargs
    is_vararg: bool,
}

/// A jump table for multi-way branching
#[derive(Debug, Clone)]
pub struct JumpTable {
    /// Base offset in bytecode for this table
    pub base_offset: usize,
    /// Entries: (value_hash, target_offset)
    pub entries: Vec<(u64, usize)>,
    /// Default target if no match
    pub default_offset: usize,
}

/// Builder for constructing BytecodeChunks
#[derive(Debug)]
pub struct ChunkBuilder {
    code: Vec<u8>,
    constants: Vec<MettaValue>,
    line_info: Vec<(usize, u32)>,
    jump_tables: Vec<JumpTable>,
    name: String,
    local_count: u16,
    upvalue_count: u16,
    arity: u8,
    is_vararg: bool,
    current_line: u32,
}

impl BytecodeChunk {
    /// Create a new empty chunk
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            code: Vec::new(),
            constants: Vec::new(),
            line_info: Vec::new(),
            jump_tables: Vec::new(),
            name: name.into(),
            local_count: 0,
            upvalue_count: 0,
            arity: 0,
            is_vararg: false,
        }
    }

    /// Create a builder for constructing a chunk
    pub fn builder(name: impl Into<String>) -> ChunkBuilder {
        ChunkBuilder::new(name)
    }

    /// Get the bytecode instructions
    #[inline]
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    /// Get the length of the bytecode
    #[inline]
    pub fn len(&self) -> usize {
        self.code.len()
    }

    /// Check if the chunk is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// Get a byte at the given offset
    #[inline]
    pub fn read_byte(&self, offset: usize) -> Option<u8> {
        self.code.get(offset).copied()
    }

    /// Get an opcode at the given offset
    #[inline]
    pub fn read_opcode(&self, offset: usize) -> Option<Opcode> {
        self.code.get(offset).and_then(|&b| Opcode::from_byte(b))
    }

    /// Read a u16 from the bytecode (big-endian)
    #[inline]
    pub fn read_u16(&self, offset: usize) -> Option<u16> {
        if offset + 1 < self.code.len() {
            Some(u16::from_be_bytes([self.code[offset], self.code[offset + 1]]))
        } else {
            None
        }
    }

    /// Read a signed i16 from the bytecode (big-endian)
    #[inline]
    pub fn read_i16(&self, offset: usize) -> Option<i16> {
        self.read_u16(offset).map(|u| u as i16)
    }

    /// Read a signed i8 from the bytecode
    #[inline]
    pub fn read_i8(&self, offset: usize) -> Option<i8> {
        self.code.get(offset).map(|&b| b as i8)
    }

    /// Get a constant from the pool
    #[inline]
    pub fn get_constant(&self, index: u16) -> Option<&MettaValue> {
        self.constants.get(index as usize)
    }

    /// Get all constants
    #[inline]
    pub fn constants(&self) -> &[MettaValue] {
        &self.constants
    }

    /// Get the number of constants
    #[inline]
    pub fn constant_count(&self) -> usize {
        self.constants.len()
    }

    /// Get the source line for a bytecode offset
    pub fn get_line(&self, offset: usize) -> Option<u32> {
        // Binary search for the line info entry
        match self.line_info.binary_search_by_key(&offset, |&(o, _)| o) {
            Ok(idx) => Some(self.line_info[idx].1),
            Err(idx) if idx > 0 => Some(self.line_info[idx - 1].1),
            _ => None,
        }
    }

    /// Get the chunk name
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the number of local slots
    #[inline]
    pub fn local_count(&self) -> u16 {
        self.local_count
    }

    /// Get the number of upvalues
    #[inline]
    pub fn upvalue_count(&self) -> u16 {
        self.upvalue_count
    }

    /// Get the arity
    #[inline]
    pub fn arity(&self) -> u8 {
        self.arity
    }

    /// Check if vararg
    #[inline]
    pub fn is_vararg(&self) -> bool {
        self.is_vararg
    }

    /// Get a jump table by index
    #[inline]
    pub fn get_jump_table(&self, index: usize) -> Option<&JumpTable> {
        self.jump_tables.get(index)
    }

    /// Disassemble the chunk to a string
    pub fn disassemble(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("=== {} ===\n", self.name));
        output.push_str(&format!(
            "locals: {}, upvalues: {}, arity: {}\n",
            self.local_count, self.upvalue_count, self.arity
        ));
        output.push_str(&format!("constants: {}\n", self.constants.len()));

        let mut offset = 0;
        while offset < self.code.len() {
            let line = self.get_line(offset).map_or(String::new(), |l| format!("{:4} ", l));
            let (disasm, next_offset) = self.disassemble_instruction(offset);
            output.push_str(&format!("{:04x} {} {}\n", offset, line, disasm));
            offset = next_offset;
        }

        output
    }

    /// Disassemble a single instruction, returns (string, next_offset)
    pub fn disassemble_instruction(&self, offset: usize) -> (String, usize) {
        let Some(opcode) = self.read_opcode(offset) else {
            return (format!("??? (0x{:02x})", self.code.get(offset).unwrap_or(&0)), offset + 1);
        };

        let mnemonic = opcode.mnemonic();
        let imm_size = opcode.immediate_size();
        let next_offset = offset + 1 + imm_size;

        let operand_str = match imm_size {
            0 => String::new(),
            1 => {
                let byte = self.code.get(offset + 1).copied().unwrap_or(0);
                match opcode {
                    Opcode::PushLongSmall => format!(" {}", byte as i8),
                    Opcode::JumpShort | Opcode::JumpIfFalseShort | Opcode::JumpIfTrueShort => {
                        let target = (offset as isize + 2 + (byte as i8) as isize) as usize;
                        format!(" -> {:04x}", target)
                    }
                    _ => format!(" {}", byte),
                }
            }
            2 => {
                let value = self.read_u16(offset + 1).unwrap_or(0);
                match opcode {
                    Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue
                    | Opcode::JumpIfNil | Opcode::JumpIfError => {
                        let target = (offset as isize + 3 + (value as i16) as isize) as usize;
                        format!(" -> {:04x}", target)
                    }
                    Opcode::PushLong | Opcode::PushAtom | Opcode::PushString
                    | Opcode::PushUri | Opcode::PushConstant | Opcode::PushVariable => {
                        let const_str = self.constants.get(value as usize)
                            .map(|c| format!("{:?}", c))
                            .unwrap_or_else(|| "???".to_string());
                        format!(" #{} ({})", value, const_str)
                    }
                    _ => format!(" {}", value),
                }
            }
            _ => String::new(),
        };

        (format!("{}{}", mnemonic, operand_str), next_offset)
    }
}

impl ChunkBuilder {
    /// Create a new chunk builder
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            code: Vec::with_capacity(256),
            constants: Vec::new(),
            line_info: Vec::new(),
            jump_tables: Vec::new(),
            name: name.into(),
            local_count: 0,
            upvalue_count: 0,
            arity: 0,
            is_vararg: false,
            current_line: 1,
        }
    }

    /// Set the current source line for subsequent instructions
    pub fn set_line(&mut self, line: u32) {
        self.current_line = line;
    }

    /// Set the number of local slots
    pub fn set_local_count(&mut self, count: u16) {
        self.local_count = count;
    }

    /// Set the number of upvalues
    pub fn set_upvalue_count(&mut self, count: u16) {
        self.upvalue_count = count;
    }

    /// Set the arity
    pub fn set_arity(&mut self, arity: u8) {
        self.arity = arity;
    }

    /// Set vararg flag
    pub fn set_vararg(&mut self, is_vararg: bool) {
        self.is_vararg = is_vararg;
    }

    /// Get the current bytecode offset
    #[inline]
    pub fn current_offset(&self) -> usize {
        self.code.len()
    }

    /// Emit a single opcode
    pub fn emit(&mut self, opcode: Opcode) {
        self.emit_line_info();
        self.code.push(opcode.to_byte());
    }

    /// Emit an opcode with a 1-byte operand
    pub fn emit_byte(&mut self, opcode: Opcode, operand: u8) {
        self.emit_line_info();
        self.code.push(opcode.to_byte());
        self.code.push(operand);
    }

    /// Emit an opcode with a 2-byte operand (big-endian)
    pub fn emit_u16(&mut self, opcode: Opcode, operand: u16) {
        self.emit_line_info();
        self.code.push(opcode.to_byte());
        self.code.extend_from_slice(&operand.to_be_bytes());
    }

    /// Emit raw bytes
    pub fn emit_raw(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    /// Add a constant to the pool, returns its index
    pub fn add_constant(&mut self, value: MettaValue) -> u16 {
        // Check if constant already exists
        for (i, existing) in self.constants.iter().enumerate() {
            if existing == &value {
                return i as u16;
            }
        }

        let index = self.constants.len();
        if index > u16::MAX as usize {
            panic!("Too many constants in chunk (max {})", u16::MAX);
        }
        self.constants.push(value);
        index as u16
    }

    /// Emit a constant load
    pub fn emit_constant(&mut self, value: MettaValue) {
        let index = self.add_constant(value);
        self.emit_u16(Opcode::PushConstant, index);
    }

    /// Create a forward jump, returns a label to patch later
    pub fn emit_jump(&mut self, opcode: Opcode) -> JumpLabel {
        debug_assert!(opcode.is_jump());
        self.emit_line_info();
        let offset = self.code.len();
        self.code.push(opcode.to_byte());
        // Placeholder for jump offset
        self.code.extend_from_slice(&[0xFF, 0xFF]);
        JumpLabel { offset: offset + 1 }
    }

    /// Emit a short forward jump (1-byte offset)
    pub fn emit_jump_short(&mut self, opcode: Opcode) -> JumpLabelShort {
        debug_assert!(opcode.is_jump());
        self.emit_line_info();
        let offset = self.code.len();
        self.code.push(opcode.to_byte());
        self.code.push(0xFF); // Placeholder
        JumpLabelShort { offset: offset + 1 }
    }

    /// Patch a jump label to jump to the current position
    pub fn patch_jump(&mut self, label: JumpLabel) {
        let target = self.code.len();
        let jump_from = label.offset + 2; // After the u16 operand
        let offset = (target as isize - jump_from as isize) as i16;
        let bytes = offset.to_be_bytes();
        self.code[label.offset] = bytes[0];
        self.code[label.offset + 1] = bytes[1];
    }

    /// Patch a short jump label
    pub fn patch_jump_short(&mut self, label: JumpLabelShort) {
        let target = self.code.len();
        let jump_from = label.offset + 1; // After the u8 operand
        let offset = (target as isize - jump_from as isize) as i8;
        self.code[label.offset] = offset as u8;
    }

    /// Emit a backward jump to a known target
    pub fn emit_loop(&mut self, target: usize) {
        self.emit_line_info();
        let offset = (target as isize - (self.code.len() as isize + 3)) as i16;
        self.code.push(Opcode::Jump.to_byte());
        self.code.extend_from_slice(&offset.to_be_bytes());
    }

    /// Add a jump table
    pub fn add_jump_table(&mut self, table: JumpTable) -> usize {
        let index = self.jump_tables.len();
        self.jump_tables.push(table);
        index
    }

    /// Record line info for current position
    fn emit_line_info(&mut self) {
        let offset = self.code.len();
        // Only add if line changed from previous entry
        if self.line_info.is_empty() || self.line_info.last().map(|&(_, l)| l) != Some(self.current_line) {
            self.line_info.push((offset, self.current_line));
        }
    }

    /// Build the final chunk
    pub fn build(self) -> BytecodeChunk {
        BytecodeChunk {
            code: self.code,
            constants: self.constants,
            line_info: self.line_info,
            jump_tables: self.jump_tables,
            name: self.name,
            local_count: self.local_count,
            upvalue_count: self.upvalue_count,
            arity: self.arity,
            is_vararg: self.is_vararg,
        }
    }

    /// Build and wrap in Arc
    pub fn build_arc(self) -> Arc<BytecodeChunk> {
        Arc::new(self.build())
    }
}

/// Label for a forward jump to be patched later
#[derive(Debug, Clone, Copy)]
pub struct JumpLabel {
    offset: usize,
}

/// Label for a short forward jump (1-byte offset)
#[derive(Debug, Clone, Copy)]
pub struct JumpLabelShort {
    offset: usize,
}

/// Compiled pattern for fast matching
#[derive(Debug, Clone)]
pub struct CompiledPattern {
    /// Head symbol if known
    pub head: Option<Arc<str>>,
    /// Expected arity if known
    pub arity: Option<usize>,
    /// Whether the pattern contains variables
    pub has_variables: bool,
    /// Variable positions: (path, symbol)
    /// Path is sequence of indices to reach the variable
    pub variable_positions: SmallVec<[(SmallVec<[u8; 4]>, Arc<str>); 4]>,
    /// Bytecode for guard evaluation
    pub guard: Option<Arc<BytecodeChunk>>,
    /// Bloom filter signature for fast rejection
    pub bloom_signature: u64,
}

impl CompiledPattern {
    /// Create an empty pattern (matches anything)
    pub fn any() -> Self {
        Self {
            head: None,
            arity: None,
            has_variables: false,
            variable_positions: SmallVec::new(),
            guard: None,
            bloom_signature: 0,
        }
    }

    /// Create a pattern matching a specific head symbol
    pub fn with_head(head: impl Into<Arc<str>>) -> Self {
        Self {
            head: Some(head.into()),
            arity: None,
            has_variables: false,
            variable_positions: SmallVec::new(),
            guard: None,
            bloom_signature: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_builder_basic() {
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Return);

        let chunk = builder.build();
        assert_eq!(chunk.len(), 3);
        assert_eq!(chunk.read_opcode(0), Some(Opcode::PushNil));
        assert_eq!(chunk.read_opcode(1), Some(Opcode::PushTrue));
        assert_eq!(chunk.read_opcode(2), Some(Opcode::Return));
    }

    #[test]
    fn test_chunk_constants() {
        let mut builder = ChunkBuilder::new("test");
        let idx1 = builder.add_constant(MettaValue::Long(42));
        let idx2 = builder.add_constant(MettaValue::Bool(true));
        let idx3 = builder.add_constant(MettaValue::Long(42)); // Duplicate

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert_eq!(idx3, 0); // Reuses existing

        let chunk = builder.build();
        assert_eq!(chunk.constant_count(), 2);
        assert_eq!(chunk.get_constant(0), Some(&MettaValue::Long(42)));
        assert_eq!(chunk.get_constant(1), Some(&MettaValue::Bool(true)));
    }

    #[test]
    fn test_chunk_jump_patching() {
        let mut builder = ChunkBuilder::new("test");

        // if (cond) { body } else { else_body }
        builder.emit(Opcode::PushTrue);
        let else_jump = builder.emit_jump(Opcode::JumpIfFalse);
        builder.emit(Opcode::PushNil); // then body
        let end_jump = builder.emit_jump(Opcode::Jump);
        builder.patch_jump(else_jump);
        builder.emit(Opcode::PushFalse); // else body
        builder.patch_jump(end_jump);
        builder.emit(Opcode::Return);

        let chunk = builder.build();

        // Verify the jump targets are correct
        let disasm = chunk.disassemble();
        assert!(disasm.contains("jump_if_false"));
        assert!(disasm.contains("jump"));
    }

    #[test]
    fn test_chunk_line_info() {
        let mut builder = ChunkBuilder::new("test");
        builder.set_line(1);
        builder.emit(Opcode::PushNil);
        builder.set_line(2);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.set_line(5);
        builder.emit(Opcode::Return);

        let chunk = builder.build();
        assert_eq!(chunk.get_line(0), Some(1));
        assert_eq!(chunk.get_line(1), Some(2));
        assert_eq!(chunk.get_line(2), Some(2)); // Same line
        assert_eq!(chunk.get_line(3), Some(5));
    }

    #[test]
    fn test_disassemble() {
        let mut builder = ChunkBuilder::new("example");
        builder.set_arity(2);
        builder.set_local_count(3);

        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        let chunk = builder.build();
        let disasm = chunk.disassemble();

        assert!(disasm.contains("example"));
        assert!(disasm.contains("push_long_small 42"));
        assert!(disasm.contains("add"));
        assert!(disasm.contains("return"));
    }
}
