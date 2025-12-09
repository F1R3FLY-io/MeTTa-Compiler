//! Bytecode Virtual Machine
//!
//! The VM executes compiled bytecode using a stack-based architecture with
//! support for nondeterminism via choice points and backtracking.

use std::ops::ControlFlow;
use std::sync::Arc;
use smallvec::SmallVec;

use crate::backend::models::MettaValue;
use super::opcodes::Opcode;
use super::chunk::BytecodeChunk;

/// Result of VM execution
pub type VmResult<T> = Result<T, VmError>;

/// Errors that can occur during VM execution
#[derive(Debug, Clone)]
pub enum VmError {
    /// Stack underflow
    StackUnderflow,
    /// Invalid opcode byte
    InvalidOpcode(u8),
    /// Invalid constant index
    InvalidConstant(u16),
    /// Invalid local variable index
    InvalidLocal(u16),
    /// Invalid binding name
    InvalidBinding(String),
    /// Type error in operation
    TypeError { expected: &'static str, got: &'static str },
    /// Division by zero
    DivisionByZero,
    /// Instruction pointer out of bounds
    IpOutOfBounds,
    /// Call stack overflow
    CallStackOverflow,
    /// Value stack overflow
    ValueStackOverflow,
    /// Halt instruction executed
    Halted,
    /// Runtime error with message
    Runtime(String),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StackUnderflow => write!(f, "Stack underflow"),
            Self::InvalidOpcode(b) => write!(f, "Invalid opcode: 0x{:02x}", b),
            Self::InvalidConstant(i) => write!(f, "Invalid constant index: {}", i),
            Self::InvalidLocal(i) => write!(f, "Invalid local variable index: {}", i),
            Self::InvalidBinding(name) => write!(f, "Invalid binding: {}", name),
            Self::TypeError { expected, got } => {
                write!(f, "Type error: expected {}, got {}", expected, got)
            }
            Self::DivisionByZero => write!(f, "Division by zero"),
            Self::IpOutOfBounds => write!(f, "Instruction pointer out of bounds"),
            Self::CallStackOverflow => write!(f, "Call stack overflow"),
            Self::ValueStackOverflow => write!(f, "Value stack overflow"),
            Self::Halted => write!(f, "Execution halted"),
            Self::Runtime(msg) => write!(f, "Runtime error: {}", msg),
        }
    }
}

impl std::error::Error for VmError {}

/// Call frame on the call stack
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// Return instruction pointer
    pub return_ip: usize,
    /// Return chunk
    pub return_chunk: Arc<BytecodeChunk>,
    /// Base pointer into value stack
    pub base_ptr: usize,
    /// Base pointer into bindings stack
    pub bindings_base: usize,
}

/// Binding frame for pattern variables
#[derive(Debug, Clone)]
pub struct BindingFrame {
    /// Variable bindings: name -> value
    pub bindings: SmallVec<[(String, MettaValue); 8]>,
    /// Scope depth for nested bindings
    pub scope_depth: u32,
}

impl BindingFrame {
    /// Create a new empty binding frame
    pub fn new(scope_depth: u32) -> Self {
        Self {
            bindings: SmallVec::new(),
            scope_depth,
        }
    }

    /// Get a binding by name
    pub fn get(&self, name: &str) -> Option<&MettaValue> {
        self.bindings.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }

    /// Set a binding
    pub fn set(&mut self, name: String, value: MettaValue) {
        // Check if binding already exists
        for (n, v) in self.bindings.iter_mut() {
            if n == &name {
                *v = value;
                return;
            }
        }
        self.bindings.push((name, value));
    }

    /// Check if a binding exists
    pub fn has(&self, name: &str) -> bool {
        self.bindings.iter().any(|(n, _)| n == name)
    }

    /// Clear all bindings
    pub fn clear(&mut self) {
        self.bindings.clear();
    }
}

/// Choice point for nondeterminism
#[derive(Debug, Clone)]
pub struct ChoicePoint {
    /// Saved value stack height
    pub value_stack_height: usize,
    /// Saved call stack height
    pub call_stack_height: usize,
    /// Saved bindings stack height
    pub bindings_stack_height: usize,
    /// Continuation instruction pointer
    pub ip: usize,
    /// Continuation chunk
    pub chunk: Arc<BytecodeChunk>,
    /// Remaining alternatives to try
    pub alternatives: Vec<Alternative>,
}

/// An alternative in a choice point
#[derive(Debug, Clone)]
pub enum Alternative {
    /// A value to push and continue
    Value(MettaValue),
    /// A bytecode chunk to execute
    Chunk(Arc<BytecodeChunk>),
    /// An index into something (rules, etc)
    Index(usize),
}

/// Configuration for the VM
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Maximum value stack size
    pub max_value_stack: usize,
    /// Maximum call stack size
    pub max_call_stack: usize,
    /// Maximum choice point stack size
    pub max_choice_points: usize,
    /// Enable tracing
    pub trace: bool,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            max_value_stack: 65536,
            max_call_stack: 1024,
            max_choice_points: 4096,
            trace: false,
        }
    }
}

/// The Bytecode Virtual Machine
#[derive(Debug)]
pub struct BytecodeVM {
    /// Value stack for operands and results
    value_stack: Vec<MettaValue>,

    /// Call stack for function frames
    call_stack: Vec<CallFrame>,

    /// Bindings stack for pattern variables
    bindings_stack: Vec<BindingFrame>,

    /// Choice points for nondeterminism
    choice_points: Vec<ChoicePoint>,

    /// Collected results (for nondeterministic evaluation)
    results: Vec<MettaValue>,

    /// Current instruction pointer
    ip: usize,

    /// Current bytecode chunk
    chunk: Arc<BytecodeChunk>,

    /// VM configuration
    config: VmConfig,
}

impl BytecodeVM {
    /// Create a new VM with the given chunk
    pub fn new(chunk: Arc<BytecodeChunk>) -> Self {
        Self::with_config(chunk, VmConfig::default())
    }

    /// Create a new VM with custom configuration
    pub fn with_config(chunk: Arc<BytecodeChunk>, config: VmConfig) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
        }
    }

    /// Run the VM to completion, returning all results
    pub fn run(&mut self) -> VmResult<Vec<MettaValue>> {
        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Execute a single instruction
    pub fn step(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        // Bounds check
        if self.ip >= self.chunk.len() {
            // End of chunk - return results or value on stack
            return self.handle_chunk_end();
        }

        // Read opcode
        let opcode_byte = self.chunk.read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        let opcode = Opcode::from_byte(opcode_byte)
            .ok_or(VmError::InvalidOpcode(opcode_byte))?;

        // Trace if enabled
        if self.config.trace {
            let (disasm, _) = self.chunk.disassemble_instruction(self.ip);
            eprintln!("[VM] {:04x}: {} | stack: {:?}", self.ip, disasm, self.value_stack);
        }

        // Advance IP past opcode
        self.ip += 1;

        // Execute opcode
        match opcode {
            // Stack operations
            Opcode::Nop => {}
            Opcode::Pop => { self.pop()?; }
            Opcode::Dup => self.op_dup()?,
            Opcode::Swap => self.op_swap()?,
            Opcode::Rot3 => self.op_rot3()?,
            Opcode::Over => self.op_over()?,
            Opcode::DupN => self.op_dup_n()?,
            Opcode::PopN => self.op_pop_n()?,

            // Value creation
            Opcode::PushNil => self.push(MettaValue::Nil),
            Opcode::PushTrue => self.push(MettaValue::Bool(true)),
            Opcode::PushFalse => self.push(MettaValue::Bool(false)),
            Opcode::PushUnit => self.push(MettaValue::Unit),
            Opcode::PushEmpty => self.push(MettaValue::sexpr(vec![])),
            Opcode::PushLongSmall => self.op_push_long_small()?,
            Opcode::PushLong => self.op_push_constant()?,
            Opcode::PushAtom => self.op_push_constant()?,
            Opcode::PushString => self.op_push_constant()?,
            Opcode::PushUri => self.op_push_constant()?,
            Opcode::PushConstant => self.op_push_constant()?,
            Opcode::PushVariable => self.op_push_constant()?,
            Opcode::MakeSExpr => self.op_make_sexpr()?,
            Opcode::MakeSExprLarge => self.op_make_sexpr_large()?,
            Opcode::MakeList => self.op_make_list()?,
            Opcode::MakeQuote => self.op_make_quote()?,

            // Variable operations
            Opcode::LoadLocal => self.op_load_local()?,
            Opcode::StoreLocal => self.op_store_local()?,
            Opcode::LoadLocalWide => self.op_load_local_wide()?,
            Opcode::StoreLocalWide => self.op_store_local_wide()?,
            Opcode::LoadBinding => self.op_load_binding()?,
            Opcode::StoreBinding => self.op_store_binding()?,
            Opcode::HasBinding => self.op_has_binding()?,
            Opcode::ClearBindings => self.op_clear_bindings(),
            Opcode::PushBindingFrame => self.op_push_binding_frame(),
            Opcode::PopBindingFrame => self.op_pop_binding_frame()?,
            Opcode::LoadUpvalue => self.op_load_upvalue()?,

            // Control flow
            Opcode::Jump => self.op_jump()?,
            Opcode::JumpIfFalse => self.op_jump_if_false()?,
            Opcode::JumpIfTrue => self.op_jump_if_true()?,
            Opcode::JumpIfNil => self.op_jump_if_nil()?,
            Opcode::JumpIfError => self.op_jump_if_error()?,
            Opcode::JumpShort => self.op_jump_short()?,
            Opcode::JumpIfFalseShort => self.op_jump_if_false_short()?,
            Opcode::JumpIfTrueShort => self.op_jump_if_true_short()?,
            Opcode::JumpTable => self.op_jump_table()?,
            Opcode::Call => self.op_call()?,
            Opcode::TailCall => self.op_tail_call()?,
            Opcode::CallN => self.op_call_n()?,
            Opcode::TailCallN => self.op_tail_call_n()?,
            Opcode::Return => return self.op_return(),
            Opcode::ReturnMulti => return self.op_return_multi(),

            // Arithmetic
            Opcode::Add => self.op_add()?,
            Opcode::Sub => self.op_sub()?,
            Opcode::Mul => self.op_mul()?,
            Opcode::Div => self.op_div()?,
            Opcode::Mod => self.op_mod()?,
            Opcode::Neg => self.op_neg()?,
            Opcode::Abs => self.op_abs()?,
            Opcode::FloorDiv => self.op_floor_div()?,
            Opcode::Pow => self.op_pow()?,

            // Comparison
            Opcode::Lt => self.op_lt()?,
            Opcode::Le => self.op_le()?,
            Opcode::Gt => self.op_gt()?,
            Opcode::Ge => self.op_ge()?,
            Opcode::Eq => self.op_eq()?,
            Opcode::Ne => self.op_ne()?,
            Opcode::StructEq => self.op_struct_eq()?,

            // Boolean
            Opcode::And => self.op_and()?,
            Opcode::Or => self.op_or()?,
            Opcode::Not => self.op_not()?,
            Opcode::Xor => self.op_xor()?,

            // Type operations
            Opcode::GetType => self.op_get_type()?,
            Opcode::CheckType => self.op_check_type()?,
            Opcode::IsType => self.op_is_type()?,
            Opcode::AssertType => self.op_assert_type()?,

            // Pattern matching
            Opcode::Match => self.op_match()?,
            Opcode::MatchBind => self.op_match_bind()?,
            Opcode::MatchHead => self.op_match_head()?,
            Opcode::MatchArity => self.op_match_arity()?,
            Opcode::MatchGuard => self.op_match_guard()?,
            Opcode::Unify => self.op_unify()?,
            Opcode::UnifyBind => self.op_unify_bind()?,
            Opcode::IsVariable => self.op_is_variable()?,
            Opcode::IsSExpr => self.op_is_sexpr()?,
            Opcode::IsSymbol => self.op_is_symbol()?,
            Opcode::GetHead => self.op_get_head()?,
            Opcode::GetTail => self.op_get_tail()?,
            Opcode::GetArity => self.op_get_arity()?,
            Opcode::GetElement => self.op_get_element()?,

            // Nondeterminism
            Opcode::Fork => self.op_fork()?,
            Opcode::Fail => return self.op_fail(),
            Opcode::Cut => self.op_cut(),
            Opcode::Collect => self.op_collect()?,
            Opcode::CollectN => self.op_collect_n()?,
            Opcode::Yield => return self.op_yield(),
            Opcode::BeginNondet => self.op_begin_nondet(),
            Opcode::EndNondet => self.op_end_nondet()?,

            // Debug
            Opcode::Breakpoint => self.op_breakpoint()?,
            Opcode::Trace => self.op_trace()?,
            Opcode::Halt => return Err(VmError::Halted),

            // Not yet implemented
            _ => {
                return Err(VmError::Runtime(format!(
                    "Opcode {} not yet implemented",
                    opcode.mnemonic()
                )));
            }
        }

        Ok(ControlFlow::Continue(()))
    }

    // === Stack Operations ===

    #[inline]
    fn push(&mut self, value: MettaValue) {
        self.value_stack.push(value);
    }

    #[inline]
    fn pop(&mut self) -> VmResult<MettaValue> {
        self.value_stack.pop().ok_or(VmError::StackUnderflow)
    }

    #[inline]
    fn peek(&self) -> VmResult<&MettaValue> {
        self.value_stack.last().ok_or(VmError::StackUnderflow)
    }

    #[inline]
    fn peek_n(&self, n: usize) -> VmResult<&MettaValue> {
        let len = self.value_stack.len();
        if n >= len {
            return Err(VmError::StackUnderflow);
        }
        Ok(&self.value_stack[len - 1 - n])
    }

    fn op_dup(&mut self) -> VmResult<()> {
        let value = self.peek()?.clone();
        self.push(value);
        Ok(())
    }

    fn op_swap(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 2 {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.swap(len - 1, len - 2);
        Ok(())
    }

    fn op_rot3(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 3 {
            return Err(VmError::StackUnderflow);
        }
        // [a, b, c] -> [c, a, b]
        let c = self.value_stack.pop().unwrap();
        let b = self.value_stack.pop().unwrap();
        let a = self.value_stack.pop().unwrap();
        self.value_stack.push(c);
        self.value_stack.push(a);
        self.value_stack.push(b);
        Ok(())
    }

    fn op_over(&mut self) -> VmResult<()> {
        let value = self.peek_n(1)?.clone();
        self.push(value);
        Ok(())
    }

    fn op_dup_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        for i in (len - n)..len {
            let value = self.value_stack[i].clone();
            self.push(value);
        }
        Ok(())
    }

    fn op_pop_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.truncate(len - n);
        Ok(())
    }

    // === Value Creation ===

    fn op_push_long_small(&mut self) -> VmResult<()> {
        let value = self.read_i8()? as i64;
        self.push(MettaValue::Long(value));
        Ok(())
    }

    fn op_push_constant(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let value = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();
        self.push(value);
        Ok(())
    }

    fn op_make_sexpr(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        self.make_sexpr_impl(arity)
    }

    fn op_make_sexpr_large(&mut self) -> VmResult<()> {
        let arity = self.read_u16()? as usize;
        self.make_sexpr_impl(arity)
    }

    fn make_sexpr_impl(&mut self, arity: usize) -> VmResult<()> {
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let elements: Vec<MettaValue> = self.value_stack.drain((len - arity)..).collect();
        self.push(MettaValue::sexpr(elements));
        Ok(())
    }

    fn op_make_list(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let elements: Vec<MettaValue> = self.value_stack.drain((len - arity)..).collect();
        // Build proper list
        let mut list = MettaValue::Nil;
        for elem in elements.into_iter().rev() {
            list = MettaValue::sexpr(vec![
                MettaValue::sym("Cons"),
                elem,
                list,
            ]);
        }
        self.push(list);
        Ok(())
    }

    fn op_make_quote(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        self.push(MettaValue::sexpr(vec![
            MettaValue::sym("quote"),
            value,
        ]));
        Ok(())
    }

    // === Variable Operations ===

    fn op_load_local(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        self.load_local_impl(index)
    }

    fn op_load_local_wide(&mut self) -> VmResult<()> {
        let index = self.read_u16()? as usize;
        self.load_local_impl(index)
    }

    fn load_local_impl(&mut self, index: usize) -> VmResult<()> {
        let base = self.call_stack.last()
            .map(|f| f.base_ptr)
            .unwrap_or(0);
        let abs_index = base + index;
        if abs_index >= self.value_stack.len() {
            return Err(VmError::InvalidLocal(index as u16));
        }
        let value = self.value_stack[abs_index].clone();
        self.push(value);
        Ok(())
    }

    fn op_store_local(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        self.store_local_impl(index)
    }

    fn op_store_local_wide(&mut self) -> VmResult<()> {
        let index = self.read_u16()? as usize;
        self.store_local_impl(index)
    }

    fn store_local_impl(&mut self, index: usize) -> VmResult<()> {
        let value = self.pop()?;
        let base = self.call_stack.last()
            .map(|f| f.base_ptr)
            .unwrap_or(0);
        let abs_index = base + index;
        if abs_index >= self.value_stack.len() {
            return Err(VmError::InvalidLocal(index as u16));
        }
        self.value_stack[abs_index] = value;
        Ok(())
    }

    fn op_load_binding(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = match self.chunk.get_constant(index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            _ => return Err(VmError::InvalidConstant(index)),
        };
        // Search bindings from innermost to outermost
        for frame in self.bindings_stack.iter().rev() {
            if let Some(value) = frame.get(&name) {
                self.push(value.clone());
                return Ok(());
            }
        }
        Err(VmError::InvalidBinding(name.clone()))
    }

    fn op_store_binding(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let value = self.pop()?;
        let name = match self.chunk.get_constant(index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            _ => return Err(VmError::InvalidConstant(index)),
        };
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.set(name.clone(), value);
        }
        Ok(())
    }

    fn op_has_binding(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = match self.chunk.get_constant(index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            _ => return Err(VmError::InvalidConstant(index)),
        };
        let exists = self.bindings_stack.iter()
            .rev()
            .any(|frame| frame.has(&name));
        self.push(MettaValue::Bool(exists));
        Ok(())
    }

    fn op_clear_bindings(&mut self) {
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.clear();
        }
    }

    fn op_push_binding_frame(&mut self) {
        let depth = self.bindings_stack.len() as u32;
        self.bindings_stack.push(BindingFrame::new(depth));
    }

    fn op_pop_binding_frame(&mut self) -> VmResult<()> {
        if self.bindings_stack.len() <= 1 {
            return Err(VmError::Runtime("Cannot pop root binding frame".into()));
        }
        self.bindings_stack.pop();
        Ok(())
    }

    fn op_load_upvalue(&mut self) -> VmResult<()> {
        let _operand = self.read_u16()?;
        // TODO: Implement upvalue loading
        Err(VmError::Runtime("Upvalues not yet implemented".into()))
    }

    // === Control Flow ===

    fn op_jump(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        self.ip = (self.ip as isize + offset as isize) as usize;
        Ok(())
    }

    fn op_jump_if_false(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.pop()?;
        if matches!(cond, MettaValue::Bool(false)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    fn op_jump_if_true(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.pop()?;
        if matches!(cond, MettaValue::Bool(true)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    fn op_jump_if_nil(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.pop()?;
        if matches!(cond, MettaValue::Nil) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    fn op_jump_if_error(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let cond = self.peek()?;
        if matches!(cond, MettaValue::Error { .. }) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    fn op_jump_short(&mut self) -> VmResult<()> {
        let offset = self.read_i8()?;
        self.ip = (self.ip as isize + offset as isize) as usize;
        Ok(())
    }

    fn op_jump_if_false_short(&mut self) -> VmResult<()> {
        let offset = self.read_i8()?;
        let cond = self.pop()?;
        if matches!(cond, MettaValue::Bool(false)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    fn op_jump_if_true_short(&mut self) -> VmResult<()> {
        let offset = self.read_i8()?;
        let cond = self.pop()?;
        if matches!(cond, MettaValue::Bool(true)) {
            self.ip = (self.ip as isize + offset as isize) as usize;
        }
        Ok(())
    }

    fn op_jump_table(&mut self) -> VmResult<()> {
        let _table_index = self.read_u16()?;
        // TODO: Implement jump table
        Err(VmError::Runtime("Jump table not yet implemented".into()))
    }

    fn op_call(&mut self) -> VmResult<()> {
        let _index = self.read_u16()?;
        // TODO: Implement call
        Err(VmError::Runtime("Call not yet implemented".into()))
    }

    fn op_tail_call(&mut self) -> VmResult<()> {
        let _index = self.read_u16()?;
        // TODO: Implement tail call
        Err(VmError::Runtime("Tail call not yet implemented".into()))
    }

    fn op_call_n(&mut self) -> VmResult<()> {
        let _n = self.read_u8()?;
        // TODO: Implement call with N args
        Err(VmError::Runtime("CallN not yet implemented".into()))
    }

    fn op_tail_call_n(&mut self) -> VmResult<()> {
        let _n = self.read_u8()?;
        // TODO: Implement tail call with N args
        Err(VmError::Runtime("TailCallN not yet implemented".into()))
    }

    fn op_return(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        let value = self.pop()?;
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // Return from top-level
            self.results.push(value);
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    fn op_return_multi(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        // Return all values on stack above base_ptr
        let base = self.call_stack.last()
            .map(|f| f.base_ptr)
            .unwrap_or(0);
        let values: Vec<MettaValue> = self.value_stack.drain(base..).collect();

        if let Some(frame) = self.call_stack.pop() {
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            for v in values {
                self.push(v);
            }
            Ok(ControlFlow::Continue(()))
        } else {
            self.results.extend(values);
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    fn handle_chunk_end(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller
            let value = self.pop().unwrap_or(MettaValue::Nil);
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // End of top-level
            if !self.value_stack.is_empty() {
                self.results.extend(self.value_stack.drain(..));
            }
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    // === Arithmetic Operations ===

    fn op_add(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x + y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_sub(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x - y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_mul(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x * y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_div(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(_), MettaValue::Long(0)) => return Err(VmError::DivisionByZero),
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x / y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_mod(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(_), MettaValue::Long(0)) => return Err(VmError::DivisionByZero),
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x % y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_neg(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Long(x) => MettaValue::Long(-x),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_abs(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Long(x) => MettaValue::Long(x.abs()),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_floor_div(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(_), MettaValue::Long(0)) => return Err(VmError::DivisionByZero),
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Long(x.div_euclid(*y)),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_pow(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) if *y >= 0 => {
                MettaValue::Long(x.pow(*y as u32))
            }
            _ => return Err(VmError::TypeError { expected: "Long with non-negative exponent", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    // === Comparison Operations ===

    fn op_lt(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x < y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_le(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x <= y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_gt(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x > y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_ge(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(x >= y),
            _ => return Err(VmError::TypeError { expected: "Long", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_eq(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(MettaValue::Bool(a == b));
        Ok(())
    }

    fn op_ne(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(MettaValue::Bool(a != b));
        Ok(())
    }

    fn op_struct_eq(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        // Structural equality (same as ==  for now)
        self.push(MettaValue::Bool(a == b));
        Ok(())
    }

    // === Boolean Operations ===

    fn op_and(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Bool(x), MettaValue::Bool(y)) => MettaValue::Bool(*x && *y),
            _ => return Err(VmError::TypeError { expected: "Bool", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_or(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Bool(x), MettaValue::Bool(y)) => MettaValue::Bool(*x || *y),
            _ => return Err(VmError::TypeError { expected: "Bool", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_not(&mut self) -> VmResult<()> {
        let a = self.pop()?;
        let result = match a {
            MettaValue::Bool(x) => MettaValue::Bool(!x),
            _ => return Err(VmError::TypeError { expected: "Bool", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    fn op_xor(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = match (&a, &b) {
            (MettaValue::Bool(x), MettaValue::Bool(y)) => MettaValue::Bool(*x ^ *y),
            _ => return Err(VmError::TypeError { expected: "Bool", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    // === Type Operations ===

    fn op_get_type(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let type_name = value.type_name();
        self.push(MettaValue::sym(type_name));
        Ok(())
    }

    fn op_check_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.pop()?;
        let expected = match &type_val {
            MettaValue::Atom(s) => s.as_str(),
            _ => return Err(VmError::TypeError { expected: "type symbol", got: "other" }),
        };
        let matches = value.type_name() == expected;
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    fn op_is_type(&mut self) -> VmResult<()> {
        // Same as check_type for now
        self.op_check_type()
    }

    fn op_assert_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.peek()?;
        let expected = match &type_val {
            MettaValue::Atom(s) => s.as_str(),
            _ => return Err(VmError::TypeError { expected: "type symbol", got: "other" }),
        };
        if value.type_name() != expected {
            return Err(VmError::TypeError {
                expected: "matching type",
                got: value.type_name()
            });
        }
        Ok(())
    }

    // === Pattern Matching Operations ===

    fn op_match(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let pattern = self.pop()?;
        let matches = pattern_matches(&pattern, &value);
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    fn op_match_bind(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let pattern = self.pop()?;
        if let Some(bindings) = pattern_match_bind(&pattern, &value) {
            // Add bindings to current frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, val) in bindings {
                    frame.set(name, val);
                }
            }
            self.push(MettaValue::Bool(true));
        } else {
            self.push(MettaValue::Bool(false));
        }
        Ok(())
    }

    fn op_match_head(&mut self) -> VmResult<()> {
        let _expected_index = self.read_u8()?;
        // TODO: Implement head matching
        Err(VmError::Runtime("MatchHead not yet implemented".into()))
    }

    fn op_match_arity(&mut self) -> VmResult<()> {
        let expected_arity = self.read_u8()? as usize;
        let value = self.pop()?;
        let matches = match &value {
            MettaValue::SExpr(items) => items.len() == expected_arity,
            _ => false,
        };
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    fn op_match_guard(&mut self) -> VmResult<()> {
        let _guard_index = self.read_u16()?;
        // TODO: Implement guard evaluation
        Err(VmError::Runtime("MatchGuard not yet implemented".into()))
    }

    fn op_unify(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let unifies = unify(&a, &b).is_some();
        self.push(MettaValue::Bool(unifies));
        Ok(())
    }

    fn op_unify_bind(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        if let Some(bindings) = unify(&a, &b) {
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, val) in bindings {
                    frame.set(name, val);
                }
            }
            self.push(MettaValue::Bool(true));
        } else {
            self.push(MettaValue::Bool(false));
        }
        Ok(())
    }

    fn op_is_variable(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_var = value.is_variable();
        self.push(MettaValue::Bool(is_var));
        Ok(())
    }

    fn op_is_sexpr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_sexpr = matches!(&value, MettaValue::SExpr(_));
        self.push(MettaValue::Bool(is_sexpr));
        Ok(())
    }

    fn op_is_symbol(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_sym = matches!(&value, MettaValue::Atom(_));
        self.push(MettaValue::Bool(is_sym));
        Ok(())
    }

    fn op_get_head(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) if !items.is_empty() => {
                self.push(items[0].clone());
            }
            _ => return Err(VmError::TypeError { expected: "non-empty S-expression", got: "other" }),
        }
        Ok(())
    }

    fn op_get_tail(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) if !items.is_empty() => {
                self.push(MettaValue::sexpr(items[1..].to_vec()));
            }
            _ => return Err(VmError::TypeError { expected: "non-empty S-expression", got: "other" }),
        }
        Ok(())
    }

    fn op_get_arity(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) => {
                self.push(MettaValue::Long(items.len() as i64));
            }
            _ => return Err(VmError::TypeError { expected: "S-expression", got: "other" }),
        }
        Ok(())
    }

    fn op_get_element(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) if index < items.len() => {
                self.push(items[index].clone());
            }
            _ => return Err(VmError::TypeError { expected: "S-expression with valid index", got: "other" }),
        }
        Ok(())
    }

    // === Nondeterminism Operations ===

    fn op_fork(&mut self) -> VmResult<()> {
        let count = self.read_u16()? as usize;
        if count == 0 {
            return self.op_fail().map(|_| ());
        }

        // Pop alternatives from stack
        let len = self.value_stack.len();
        if count > len {
            return Err(VmError::StackUnderflow);
        }

        let alternatives: Vec<Alternative> = self.value_stack
            .drain((len - count)..)
            .map(Alternative::Value)
            .collect();

        // Create choice point
        let cp = ChoicePoint {
            value_stack_height: self.value_stack.len(),
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            ip: self.ip,
            chunk: Arc::clone(&self.chunk),
            alternatives: alternatives[1..].to_vec(),
        };

        self.choice_points.push(cp);

        // Push first alternative
        if let Alternative::Value(v) = &alternatives[0] {
            self.push(v.clone());
        }

        Ok(())
    }

    fn op_fail(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        // Backtrack to most recent choice point
        while let Some(mut cp) = self.choice_points.pop() {
            // Restore state
            self.value_stack.truncate(cp.value_stack_height);
            self.call_stack.truncate(cp.call_stack_height);
            self.bindings_stack.truncate(cp.bindings_stack_height);

            if cp.alternatives.is_empty() {
                // No more alternatives at this choice point
                continue;
            }

            // Try next alternative
            let alt = cp.alternatives.remove(0);

            // Restore instruction pointer and chunk from choice point
            self.ip = cp.ip;
            self.chunk = Arc::clone(&cp.chunk);

            // Put choice point back if more alternatives remain
            if !cp.alternatives.is_empty() {
                self.choice_points.push(cp);
            }

            // Process alternative
            match alt {
                Alternative::Value(v) => self.push(v),
                Alternative::Chunk(chunk) => {
                    self.chunk = chunk;
                    self.ip = 0;
                }
                Alternative::Index(_) => {
                    // TODO: Handle index alternatives
                }
            }

            return Ok(ControlFlow::Continue(()));
        }

        // No more choice points - return collected results
        Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
    }

    fn op_cut(&mut self) {
        // Remove all choice points
        self.choice_points.clear();
    }

    fn op_collect(&mut self) -> VmResult<()> {
        let _chunk_index = self.read_u16()?;
        // TODO: Implement collect
        Err(VmError::Runtime("Collect not yet implemented".into()))
    }

    fn op_collect_n(&mut self) -> VmResult<()> {
        let _n = self.read_u8()?;
        // TODO: Implement collect_n
        Err(VmError::Runtime("CollectN not yet implemented".into()))
    }

    fn op_yield(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        // Save current result and backtrack for more
        let value = self.pop()?;
        self.results.push(value);
        self.op_fail()
    }

    fn op_begin_nondet(&mut self) {
        // Mark start of nondeterministic section
        // Could save state for potential rollback
    }

    fn op_end_nondet(&mut self) -> VmResult<()> {
        // End nondeterministic section
        Ok(())
    }

    // === Debug Operations ===

    fn op_breakpoint(&mut self) -> VmResult<()> {
        // TODO: Implement breakpoint handling
        eprintln!("[BREAKPOINT] ip={:04x}", self.ip);
        Ok(())
    }

    fn op_trace(&mut self) -> VmResult<()> {
        let value = self.peek()?;
        eprintln!("[TRACE] {:?}", value);
        Ok(())
    }

    // === Helpers ===

    #[inline]
    fn read_u8(&mut self) -> VmResult<u8> {
        let byte = self.chunk.read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        self.ip += 1;
        Ok(byte)
    }

    #[inline]
    fn read_i8(&mut self) -> VmResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    #[inline]
    fn read_u16(&mut self) -> VmResult<u16> {
        let value = self.chunk.read_u16(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        self.ip += 2;
        Ok(value)
    }

    #[inline]
    fn read_i16(&mut self) -> VmResult<i16> {
        Ok(self.read_u16()? as i16)
    }
}

// === Pattern Matching Helpers ===

/// Check if a value is a variable (Atom starting with $)
#[inline]
fn is_variable(value: &MettaValue) -> bool {
    matches!(value, MettaValue::Atom(s) if s.starts_with('$'))
}

/// Get the variable name from a variable atom (strips the $ prefix)
#[inline]
fn get_variable_name(value: &MettaValue) -> Option<&str> {
    match value {
        MettaValue::Atom(s) if s.starts_with('$') => Some(&s[1..]),
        _ => None,
    }
}

/// Check if pattern matches value (without binding)
fn pattern_matches(pattern: &MettaValue, value: &MettaValue) -> bool {
    match (pattern, value) {
        // Variable matches anything (Atom starting with $)
        (MettaValue::Atom(s), _) if s.starts_with('$') => true,
        // Wildcard matches anything
        (MettaValue::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // S-expression matching
        (MettaValue::SExpr(ps), MettaValue::SExpr(vs)) => {
            ps.len() == vs.len() && ps.iter().zip(vs.iter()).all(|(p, v)| pattern_matches(p, v))
        }
        _ => false,
    }
}

/// Pattern match with variable binding
fn pattern_match_bind(pattern: &MettaValue, value: &MettaValue) -> Option<Vec<(String, MettaValue)>> {
    let mut bindings = Vec::new();
    if pattern_match_bind_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn pattern_match_bind_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Vec<(String, MettaValue)>,
) -> bool {
    match (pattern, value) {
        // Variable binds to value (Atom starting with $)
        (MettaValue::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Wildcard matches without binding
        (MettaValue::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // S-expression matching
        (MettaValue::SExpr(ps), MettaValue::SExpr(vs)) => {
            ps.len() == vs.len() && ps.iter().zip(vs.iter())
                .all(|(p, v)| pattern_match_bind_impl(p, v, bindings))
        }
        _ => false,
    }
}

/// Unification with variable binding
fn unify(a: &MettaValue, b: &MettaValue) -> Option<Vec<(String, MettaValue)>> {
    let mut bindings = Vec::new();
    if unify_impl(a, b, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn unify_impl(
    a: &MettaValue,
    b: &MettaValue,
    bindings: &mut Vec<(String, MettaValue)>,
) -> bool {
    match (a, b) {
        // Variables unify with anything (Atom starting with $)
        (MettaValue::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        (val, MettaValue::Atom(name)) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Same structure
        (MettaValue::Atom(x), MettaValue::Atom(y)) => x == y,
        (MettaValue::Long(x), MettaValue::Long(y)) => x == y,
        (MettaValue::Bool(x), MettaValue::Bool(y)) => x == y,
        (MettaValue::String(x), MettaValue::String(y)) => x == y,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        (MettaValue::SExpr(xs), MettaValue::SExpr(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter())
                .all(|(x, y)| unify_impl(x, y, bindings))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::chunk::ChunkBuilder;

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

        assert_eq!(results[0], MettaValue::Nil);
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
        // Push 3 alternatives
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_u16(Opcode::Fork, 3);
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
        // Push 3 alternatives
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_u16(Opcode::Fork, 3);
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
}
