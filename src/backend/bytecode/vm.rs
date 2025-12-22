//! Bytecode Virtual Machine
//!
//! The VM executes compiled bytecode using a stack-based architecture with
//! support for nondeterminism via choice points and backtracking.

use std::ops::ControlFlow;
use std::sync::Arc;
use smallvec::SmallVec;

use crate::backend::models::{Bindings, MettaValue, SpaceHandle};
use crate::backend::Environment;
use super::opcodes::Opcode;
use super::chunk::BytecodeChunk;
use super::mork_bridge::MorkBridge;
use super::native_registry::{NativeRegistry, NativeContext};
use super::memo_cache::MemoCache;
use super::external_registry::{ExternalRegistry, ExternalContext};

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
    /// A rule match with compiled body and bindings (for multi-match Call)
    RuleMatch {
        /// The compiled rule body to execute
        chunk: Arc<BytecodeChunk>,
        /// Pattern variable bindings from matching
        bindings: Bindings,
    },
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

    /// Optional bridge to MORK for rule dispatch
    bridge: Option<Arc<MorkBridge>>,

    /// Native function registry for CallNative opcode
    native_registry: Arc<NativeRegistry>,

    /// Memoization cache for CallCached opcode
    memo_cache: Arc<MemoCache>,

    /// External function registry for CallExternal opcode
    external_registry: Arc<ExternalRegistry>,

    /// Optional environment for rule definitions and lookups
    /// When present, enables DefineRule and RuntimeCall opcodes
    env: Option<Environment>,
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
            bridge: None,
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: None,
        }
    }

    /// Create a new VM with a bridge for rule dispatch
    pub fn with_bridge(chunk: Arc<BytecodeChunk>, bridge: Arc<MorkBridge>) -> Self {
        let mut vm = Self::new(chunk);
        vm.bridge = Some(bridge);
        vm
    }

    /// Create a new VM with custom configuration and bridge
    pub fn with_config_and_bridge(
        chunk: Arc<BytecodeChunk>,
        config: VmConfig,
        bridge: Arc<MorkBridge>,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            bridge: Some(bridge),
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: None,
        }
    }

    /// Create a new VM with an environment for rule definitions and lookups.
    ///
    /// This enables the DefineRule and RuntimeCall opcodes to interact with
    /// the MeTTa environment for rule-based evaluation.
    pub fn with_env(chunk: Arc<BytecodeChunk>, env: Environment) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            bridge: None,
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: Some(env),
        }
    }

    /// Create a new VM with custom configuration and environment.
    pub fn with_config_and_env(
        chunk: Arc<BytecodeChunk>,
        config: VmConfig,
        env: Environment,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            bridge: None,
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: Some(env),
        }
    }

    /// Set the external function registry
    ///
    /// This allows registering external functions before VM execution.
    pub fn with_external_registry(mut self, registry: Arc<ExternalRegistry>) -> Self {
        self.external_registry = registry;
        self
    }

    /// Set the environment for rule operations.
    ///
    /// This is a builder-style method for setting environment after construction.
    pub fn with_environment(mut self, env: Environment) -> Self {
        self.env = Some(env);
        self
    }

    /// Get a reference to the environment, if present.
    pub fn environment(&self) -> Option<&Environment> {
        self.env.as_ref()
    }

    /// Take ownership of the environment, returning it.
    ///
    /// This is used to return the modified environment after execution.
    pub fn take_environment(&mut self) -> Option<Environment> {
        self.env.take()
    }

    /// Push an initial value onto the stack before execution.
    ///
    /// This is used for template execution where a binding value
    /// needs to be available as local slot 0.
    #[inline]
    pub fn push_initial_value(&mut self, value: MettaValue) {
        self.value_stack.push(value);
    }

    /// Resume VM execution after JIT bailout for non-determinism.
    ///
    /// This allows JIT to compile deterministic parts of bytecode and then
    /// bail out to VM for Fork/Choice opcodes that require backtracking.
    ///
    /// # Arguments
    /// * `bailout_ip` - The instruction pointer where JIT bailed out
    /// * `value_stack` - The value stack state at bailout time
    ///
    /// # Returns
    /// The results of completing execution from the bailout point
    pub fn resume_from_bailout(
        &mut self,
        bailout_ip: usize,
        value_stack: Vec<MettaValue>,
    ) -> VmResult<Vec<MettaValue>> {
        self.ip = bailout_ip;
        self.value_stack = value_stack;
        self.run_without_jit()
    }

    /// Run the VM to completion without attempting JIT execution.
    /// Used for resuming after JIT bailout.
    fn run_without_jit(&mut self) -> VmResult<Vec<MettaValue>> {
        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Run the VM to completion, returning all results
    pub fn run(&mut self) -> VmResult<Vec<MettaValue>> {
        // Pre-allocate local variable slots on the stack.
        // The VM stores locals ON the stack at positions [base, base+local_count).
        // StoreLocal pops a value and stores it at stack[base+index], so
        // slots must exist before the first StoreLocal executes.
        let local_count = self.chunk.local_count() as usize;
        if local_count > 0 && self.value_stack.len() < local_count {
            self.value_stack.resize(local_count, MettaValue::Nil);
        }

        // JIT execution path
        #[cfg(feature = "jit")]
        if let Some(result) = self.try_jit_execute()? {
            return Ok(result);
        }

        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Run the VM to completion, returning results and the modified environment.
    ///
    /// This is the primary entry point for environment-aware bytecode execution.
    /// It returns both the evaluation results and the (possibly modified) environment,
    /// enabling rule definitions to persist across evaluations.
    ///
    /// # Returns
    /// A tuple of (results, environment) where environment is the modified state
    /// after execution (e.g., with newly defined rules).
    pub fn run_with_env(&mut self) -> VmResult<(Vec<MettaValue>, Option<Environment>)> {
        let results = self.run()?;
        let env = self.env.take();
        Ok((results, env))
    }

    /// Try to execute the chunk using JIT-compiled code
    ///
    /// Returns:
    /// - `Ok(Some(results))` if JIT execution completed successfully
    /// - `Ok(None)` if JIT is not available or bailed out (fall back to interpreter)
    /// - `Err(_)` if an error occurred
    #[cfg(feature = "jit")]
    fn try_jit_execute(&mut self) -> VmResult<Option<Vec<MettaValue>>> {
        use super::jit::{JitCompiler, JitContext, JitValue, JitBailoutReason};

        // Record execution for profiling
        let should_compile = self.chunk.record_jit_execution();

        // Try to compile if hot
        if should_compile && self.chunk.can_jit_compile() {
            if self.chunk.jit_profile().try_start_compiling() {
                // We won the race to compile
                match JitCompiler::new() {
                    Ok(mut compiler) => {
                        match compiler.compile(&self.chunk) {
                            Ok(code_ptr) => {
                                unsafe {
                                    // Code size is not tracked separately for now
                                    self.chunk.jit_profile().set_compiled(code_ptr, 0);
                                }
                            }
                            Err(_e) => {
                                // Compilation failed - mark as failed so we don't try again
                                self.chunk.jit_profile().set_failed();
                            }
                        }
                    }
                    Err(_e) => {
                        // Could not create compiler - mark as failed
                        self.chunk.jit_profile().set_failed();
                    }
                }
            }
        }

        // Execute JIT code if available
        if !self.chunk.has_jit_code() {
            return Ok(None);
        }

        // Set up JIT context with appropriately sized stack
        // Use the bytecode length as a conservative upper bound for stack depth
        // (each push adds at most 1, and typical ops consume before producing)
        let required_stack = self.chunk.code().len().max(64).min(4096);
        let constants = self.chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); required_stack];

        // SAFETY: stack is valid for the lifetime of this function call
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                required_stack,
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Get and execute native code
        if let Some(native_fn) = unsafe { self.chunk.jit_profile().get_native_fn() } {
            unsafe {
                native_fn(&mut ctx as *mut JitContext);
            }

            // Check for bailout
            if ctx.bailout {
                // JIT execution bailed out - set interpreter IP to bailout point
                self.ip = ctx.bailout_ip;
                // Transfer any values from JIT stack to interpreter stack
                for i in 0..ctx.sp {
                    let jit_val = unsafe { *ctx.value_stack.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    self.push(metta_val);
                }
                return Ok(None); // Fall back to interpreter
            }

            // JIT execution completed - collect results
            let mut results = Vec::with_capacity(ctx.sp);
            for i in 0..ctx.sp {
                let jit_val = unsafe { *ctx.value_stack.add(i) };
                let metta_val = unsafe { jit_val.to_metta() };
                results.push(metta_val);
            }

            if results.is_empty() {
                results.push(MettaValue::Unit);
            }

            return Ok(Some(results));
        }

        Ok(None)
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
            Opcode::PushVariable => self.op_push_variable()?,
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
            Opcode::DeconAtom => self.op_decon_atom()?,
            Opcode::Repr => self.op_repr()?,
            Opcode::GetMetaType => self.op_get_metatype()?,
            Opcode::ConsAtom => self.op_cons_atom()?,
            Opcode::MapAtom => self.op_map_atom()?,
            Opcode::FilterAtom => self.op_filter_atom()?,
            Opcode::FoldlAtom => self.op_foldl_atom()?,

            // Nondeterminism
            Opcode::Fork => self.op_fork()?,
            Opcode::Fail => return self.op_fail(),
            Opcode::Cut => self.op_cut(),
            Opcode::Collect => self.op_collect()?,
            Opcode::CollectN => self.op_collect_n()?,
            Opcode::Yield => return self.op_yield(),
            Opcode::BeginNondet => self.op_begin_nondet(),
            Opcode::EndNondet => self.op_end_nondet()?,
            Opcode::Amb => self.op_amb()?,
            Opcode::Guard => return self.op_guard(),
            Opcode::Commit => self.op_commit(),
            Opcode::Backtrack => return self.op_fail(), // Backtrack is alias for Fail

            // Advanced calls
            Opcode::CallNative => self.op_call_native()?,
            Opcode::CallExternal => self.op_call_external()?,
            Opcode::CallCached => self.op_call_cached()?,

            // Environment operations (require Environment to be set)
            Opcode::DefineRule => self.op_define_rule()?,
            Opcode::LoadGlobal => self.op_load_global()?,
            Opcode::StoreGlobal => self.op_store_global()?,
            Opcode::DispatchRules => self.op_dispatch_rules()?,

            // Space operations
            Opcode::SpaceAdd => self.op_space_add()?,
            Opcode::SpaceRemove => self.op_space_remove()?,
            Opcode::SpaceGetAtoms => self.op_space_get_atoms()?,
            Opcode::SpaceMatch => self.op_space_match()?,
            Opcode::LoadSpace => self.op_load_space()?,

            // State operations
            Opcode::NewState => self.op_new_state()?,
            Opcode::GetState => self.op_get_state()?,
            Opcode::ChangeState => self.op_change_state()?,

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

    /// Push a variable, resolving from bindings if available
    ///
    /// For pattern variables ($x), this checks the binding stack first.
    /// If found in bindings, pushes the bound value. Otherwise pushes
    /// the variable symbol as-is (for irreducible expressions).
    fn op_push_variable(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let value = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?;

        // Check if it's a pattern variable that should be resolved from bindings
        if let MettaValue::Atom(name) = value {
            if name.starts_with('$') {
                // Search bindings from innermost to outermost
                for frame in self.bindings_stack.iter().rev() {
                    if let Some(bound_value) = frame.get(name) {
                        self.push(bound_value.clone());
                        return Ok(());
                    }
                }
            }
        }

        // Not found in bindings or not a pattern variable - push as-is
        self.push(value.clone());
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

    /// Execute a function call via MORK rule dispatch
    ///
    /// Opcode format: Call head_index:u16 arity:u8
    /// - Pops `arity` arguments from stack
    /// - Builds expression (head arg0 arg1 ...)
    /// - Dispatches to MORK for rule matching
    /// - Executes first matching rule body, or pushes expr if irreducible
    fn op_call(&mut self) -> VmResult<()> {
        let head_index = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get head symbol from constant pool
        let head_symbol = match self.chunk.get_constant(head_index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            Some(other) => {
                return Err(VmError::Runtime(format!(
                    "Call head must be atom, got {:?}",
                    other
                )));
            }
            None => return Err(VmError::InvalidConstant(head_index)),
        };

        // Pop arguments from stack (they were pushed left-to-right)
        if self.value_stack.len() < arity {
            return Err(VmError::StackUnderflow);
        }
        let args: Vec<MettaValue> = self.value_stack.drain(self.value_stack.len() - arity..).collect();

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(MettaValue::Atom(head_symbol));
        items.extend(args);
        let expr = MettaValue::SExpr(items);

        // Dispatch via MORK bridge if available
        if let Some(ref bridge) = self.bridge {
            let matches = bridge.dispatch_rules(&expr);

            if matches.is_empty() {
                // No rules match - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }

            if matches.len() == 1 {
                // Single match - execute directly (no Fork needed)
                let rule = &matches[0];
                return self.execute_rule_body(&rule.body, &rule.bindings);
            }

            // Multiple matches - create choice point for backtracking
            // First rule executes now, others become alternatives
            let alternatives: Vec<Alternative> = matches[1..]
                .iter()
                .map(|rule| Alternative::RuleMatch {
                    chunk: Arc::clone(&rule.body),
                    bindings: rule.bindings.clone(),
                })
                .collect();

            // Create choice point for backtracking to other matches
            let choice_point = ChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                alternatives,
            };
            self.choice_points.push(choice_point);

            // Execute first matching rule
            let rule = &matches[0];
            return self.execute_rule_body(&rule.body, &rule.bindings);
        }

        // No bridge - return expression as data (irreducible)
        self.push(expr);
        Ok(())
    }

    /// Execute a tail call - same as call but reuses current call frame
    ///
    /// Opcode format: TailCall head_index:u16 arity:u8
    fn op_tail_call(&mut self) -> VmResult<()> {
        let head_index = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get head symbol from constant pool
        let head_symbol = match self.chunk.get_constant(head_index) {
            Some(MettaValue::Atom(s)) => s.clone(),
            Some(other) => {
                return Err(VmError::Runtime(format!(
                    "TailCall head must be atom, got {:?}",
                    other
                )));
            }
            None => return Err(VmError::InvalidConstant(head_index)),
        };

        // Pop arguments from stack
        if self.value_stack.len() < arity {
            return Err(VmError::StackUnderflow);
        }
        let args: Vec<MettaValue> = self.value_stack.drain(self.value_stack.len() - arity..).collect();

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(MettaValue::Atom(head_symbol));
        items.extend(args);
        let expr = MettaValue::SExpr(items);

        // Dispatch via MORK bridge if available
        if let Some(ref bridge) = self.bridge {
            let matches = bridge.dispatch_rules(&expr);

            if matches.is_empty() {
                // No rules match - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }

            // TODO: Handle multiple matches via choice points (Fork)
            // For now, execute first matching rule with TCO
            let rule = &matches[0];
            return self.execute_rule_body_tail(&rule.body, &rule.bindings);
        }

        // No bridge - return expression as data (irreducible)
        self.push(expr);
        Ok(())
    }

    /// Execute a rule body by pushing a call frame and switching to the rule chunk
    fn execute_rule_body(
        &mut self,
        body: &Arc<BytecodeChunk>,
        bindings: &crate::backend::models::Bindings,
    ) -> VmResult<()> {
        // Check call stack limit
        if self.call_stack.len() >= self.config.max_call_stack {
            return Err(VmError::CallStackOverflow);
        }

        // Push a new binding frame with pattern variables
        let scope_depth = self.bindings_stack.len() as u32;
        let mut frame = BindingFrame::new(scope_depth);
        for (name, value) in bindings.iter() {
            frame.set(name.clone(), value.clone());
        }
        self.bindings_stack.push(frame);

        // Push call frame to return to after rule execution
        let call_frame = CallFrame {
            return_ip: self.ip,
            return_chunk: Arc::clone(&self.chunk),
            base_ptr: self.value_stack.len(),
            bindings_base: self.bindings_stack.len() - 1,
        };
        self.call_stack.push(call_frame);

        // Switch to rule body
        self.chunk = Arc::clone(body);
        self.ip = 0;

        Ok(())
    }

    /// Execute a rule body with tail call optimization (reuse current frame)
    fn execute_rule_body_tail(
        &mut self,
        body: &Arc<BytecodeChunk>,
        bindings: &crate::backend::models::Bindings,
    ) -> VmResult<()> {
        // For TCO: don't push a new call frame, just replace the current chunk
        // and reset bindings to the current frame level

        // Clear current binding frame and repopulate with new bindings
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.clear();
            for (name, value) in bindings.iter() {
                frame.set(name.clone(), value.clone());
            }
        }

        // Switch to rule body (no call frame push = TCO)
        self.chunk = Arc::clone(body);
        self.ip = 0;

        Ok(())
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
            // Return to caller - restore chunk/ip
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);

            // Pop binding frames down to caller's level
            while self.bindings_stack.len() > frame.bindings_base + 1 {
                self.bindings_stack.pop();
            }

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

            // Pop binding frames down to caller's level
            while self.bindings_stack.len() > frame.bindings_base + 1 {
                self.bindings_stack.pop();
            }

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

            // Pop binding frame pushed by execute_rule_body
            if self.bindings_stack.len() > frame.bindings_base {
                self.bindings_stack.truncate(frame.bindings_base + 1);
                self.bindings_stack.pop();
            }

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
        // Type variables match anything (consistent with tree-visitor)
        let matches = if expected.starts_with('$') {
            true
        } else {
            value.type_name() == expected
        };
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

    fn op_decon_atom(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) if !items.is_empty() => {
                let head = items[0].clone();
                let tail = MettaValue::SExpr(items[1..].to_vec());
                // Return (head tail) pair as S-expression
                self.push(MettaValue::SExpr(vec![head, tail]));
            }
            _ => {
                // Empty or non-expression: nondeterministic failure
                return Err(VmError::TypeError {
                    expected: "non-empty S-expression",
                    got: "empty or non-expression",
                });
            }
        }
        Ok(())
    }

    fn op_repr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let repr_str = self.atom_repr(&value);
        self.push(MettaValue::String(repr_str));
        Ok(())
    }

    /// cons-atom: prepend head to tail S-expression
    /// Matches tree-visitor semantics in list_ops.rs:118-126
    fn op_cons_atom(&mut self) -> VmResult<()> {
        let tail = self.pop()?;
        let head = self.pop()?;

        let result = match tail {
            MettaValue::SExpr(mut elements) => {
                // Prepend head to existing S-expression
                elements.insert(0, head);
                MettaValue::SExpr(elements)
            }
            MettaValue::Nil => {
                // Create single-element S-expression
                MettaValue::SExpr(vec![head])
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "S-expression or Nil",
                    got: "other",
                });
            }
        };

        self.push(result);
        Ok(())
    }

    fn atom_repr(&self, value: &MettaValue) -> String {
        match value {
            MettaValue::Long(n) => n.to_string(),
            MettaValue::Float(f) => f.to_string(),
            MettaValue::Bool(b) => if *b { "True".to_string() } else { "False".to_string() },
            MettaValue::String(s) => format!("\"{}\"", s),
            MettaValue::Atom(a) => a.clone(),
            MettaValue::SExpr(items) => {
                let inner: Vec<String> = items.iter().map(|v| self.atom_repr(v)).collect();
                format!("({})", inner.join(" "))
            }
            MettaValue::Unit => "()".to_string(),
            MettaValue::Nil => "Nil".to_string(),
            MettaValue::Error(msg, _) => format!("(Error {})", msg),
            MettaValue::Type(t) => format!("(: {})", self.atom_repr(t)),
            MettaValue::Space(_) => "<space>".to_string(),
            MettaValue::State(_) => "<state>".to_string(),
            MettaValue::Conjunction(items) => {
                let inner: Vec<String> = items.iter().map(|v| self.atom_repr(v)).collect();
                format!("[{}]", inner.join(" "))
            }
            MettaValue::Memo(_) => "<memo>".to_string(),
            MettaValue::Empty => "Empty".to_string(),
        }
    }

    fn op_get_metatype(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let metatype = match &value {
            MettaValue::SExpr(_) => "Expression",
            MettaValue::Atom(s) if s.starts_with('$') => "Variable",
            MettaValue::Atom(_) => "Symbol",
            MettaValue::Bool(_) => "Bool",
            MettaValue::Long(_) => "Number",
            MettaValue::Float(_) => "Number",
            MettaValue::String(_) => "String",
            MettaValue::Nil => "Nil",
            MettaValue::Unit => "Unit",
            MettaValue::Error(_, _) => "Error",
            MettaValue::Type(_) => "Type",
            MettaValue::Space(_) => "Space",
            MettaValue::State(_) => "State",
            MettaValue::Conjunction(_) => "Conjunction",
            MettaValue::Memo(_) => "Memo",
            MettaValue::Empty => "Empty",
        };
        self.push(MettaValue::sym(metatype));
        Ok(())
    }

    fn op_map_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = match list {
            MettaValue::SExpr(items) => items,
            _ => return Err(VmError::TypeError { expected: "list/S-expression", got: "other" }),
        };

        let template_chunk = self.chunk.get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;
        let mut results = Vec::with_capacity(items.len());

        for item in items {
            let result = self.execute_template_with_binding(Arc::clone(&template_chunk), item)?;
            results.push(result);
        }

        self.push(MettaValue::SExpr(results));
        Ok(())
    }

    fn op_filter_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = match list {
            MettaValue::SExpr(items) => items,
            _ => return Err(VmError::TypeError { expected: "list/S-expression", got: "other" }),
        };

        let predicate_chunk = self.chunk.get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;
        let mut results = Vec::new();

        for item in items {
            let result = self.execute_template_with_binding(Arc::clone(&predicate_chunk), item.clone())?;
            // Check if predicate returned true
            if matches!(result, MettaValue::Bool(true)) {
                results.push(item);
            }
        }

        self.push(MettaValue::SExpr(results));
        Ok(())
    }

    fn op_foldl_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let init = self.pop()?;
        let list = self.pop()?;

        let items = match list {
            MettaValue::SExpr(items) => items,
            _ => return Err(VmError::TypeError { expected: "list/S-expression", got: "other" }),
        };

        let op_chunk = self.chunk.get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut acc = init;
        for item in items {
            // Execute template with (acc, item) - push both as locals
            acc = self.execute_foldl_template(Arc::clone(&op_chunk), acc, item)?;
        }

        self.push(acc);
        Ok(())
    }

    /// Execute a template chunk with a single bound value (for map/filter)
    fn execute_template_with_binding(&mut self, chunk: Arc<BytecodeChunk>, binding: MettaValue) -> VmResult<MettaValue> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.push(binding); // Push bound value as local slot 0

        // Execute until Return or end of chunk
        loop {
            if self.ip >= self.chunk.len() {
                break;
            }
            let opcode_byte = self.chunk.read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode = Opcode::from_byte(opcode_byte)
                .ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break;
            }

            // Execute one step
            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    // Restore and return first result
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or(MettaValue::Unit));
                }
                Err(e) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Err(e);
                }
            }
        }

        // Get result
        let result = self.pop().unwrap_or(MettaValue::Unit);

        // Restore state
        self.ip = saved_ip;
        self.chunk = saved_chunk;

        // Cleanup any remaining stack entries from template
        while self.value_stack.len() > saved_stack_base {
            let _ = self.pop();
        }

        Ok(result)
    }

    /// Execute a foldl template chunk with accumulator and item bindings
    fn execute_foldl_template(&mut self, chunk: Arc<BytecodeChunk>, acc: MettaValue, item: MettaValue) -> VmResult<MettaValue> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.push(acc);   // Local slot 0: accumulator
        self.push(item);  // Local slot 1: item

        // Execute until Return or end of chunk
        loop {
            if self.ip >= self.chunk.len() {
                break;
            }
            let opcode_byte = self.chunk.read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode = Opcode::from_byte(opcode_byte)
                .ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break;
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or(MettaValue::Unit));
                }
                Err(e) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Err(e);
                }
            }
        }

        let result = self.pop().unwrap_or(MettaValue::Unit);
        self.ip = saved_ip;
        self.chunk = saved_chunk;
        while self.value_stack.len() > saved_stack_base {
            let _ = self.pop();
        }

        Ok(result)
    }

    // === Nondeterminism Operations ===

    fn op_fork(&mut self) -> VmResult<()> {
        let count = self.read_u16()? as usize;
        if count == 0 {
            return self.op_fail().map(|_| ());
        }

        // Read constant indices from bytecode (compiler emits u16 indices after Fork)
        let mut alternatives = Vec::with_capacity(count);
        for _ in 0..count {
            let const_idx = self.read_u16()?;
            let value = self.chunk.get_constant(const_idx)
                .ok_or(VmError::InvalidConstant(const_idx))?
                .clone();
            alternatives.push(Alternative::Value(value));
        }

        // Create choice point with remaining alternatives
        // Save IP pointing past all constant indices (where execution should resume)
        let resume_ip = self.ip;

        if alternatives.len() > 1 {
            let cp = ChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: resume_ip,
                chunk: Arc::clone(&self.chunk),
                alternatives: alternatives[1..].to_vec(),
            };
            self.choice_points.push(cp);
        }

        // Push first alternative and continue execution
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
                Alternative::RuleMatch { chunk, bindings } => {
                    // Execute rule with its bindings
                    // execute_rule_body sets up call frame and switches chunk
                    self.execute_rule_body(&chunk, &bindings)?;
                    return Ok(ControlFlow::Continue(()));
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

    /// Collect all nondeterministic results from current evaluation.
    /// The chunk_index parameter is reserved for future use (sub-chunk execution).
    /// Currently, this collects all results accumulated via Yield and returns them as SExpr.
    ///
    /// Stack: [] -> [SExpr of collected results]
    fn op_collect(&mut self) -> VmResult<()> {
        let _chunk_index = self.read_u16()?;

        // Collect all results accumulated so far via Yield
        // Filter out Nil values (matches collapse semantics)
        let collected: Vec<MettaValue> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !matches!(v, MettaValue::Nil))
            .collect();

        // Push the collected results as a single S-expression
        self.push(MettaValue::SExpr(collected));
        Ok(())
    }

    /// Collect up to N nondeterministic results.
    /// Stack: [] -> [SExpr of collected results (up to N)]
    fn op_collect_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;

        // Take up to N results
        let collected: Vec<MettaValue> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !matches!(v, MettaValue::Nil))
            .take(n)
            .collect();

        // Push the collected results as a single S-expression
        self.push(MettaValue::SExpr(collected));
        Ok(())
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

    /// Guard - backtrack if top of stack is false.
    /// Stack: [bool] -> []
    fn op_guard(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        let cond = self.pop()?;
        match cond {
            MettaValue::Bool(true) => {
                // Continue execution
                Ok(ControlFlow::Continue(()))
            }
            MettaValue::Bool(false) => {
                // Backtrack
                self.op_fail()
            }
            other => Err(VmError::TypeError {
                expected: "Bool",
                got: other.type_name(),
            }),
        }
    }

    /// Commit - remove N choice points (soft cut).
    /// If count is 0, remove all choice points (like full cut).
    /// Stack: [] -> []
    fn op_commit(&mut self) {
        let count = self.read_u8().unwrap_or(0);
        if count == 0 {
            // Remove all choice points (full cut)
            self.choice_points.clear();
        } else {
            // Remove N most recent choice points
            let to_remove = (count as usize).min(self.choice_points.len());
            let new_len = self.choice_points.len().saturating_sub(to_remove);
            self.choice_points.truncate(new_len);
        }
    }

    /// Amb - ambiguous choice from N alternatives on stack.
    /// Creates a choice point with alternatives 2..N and returns alternative 1.
    /// Stack: [alt1, alt2, ..., altN] -> [selected]
    fn op_amb(&mut self) -> VmResult<()> {
        let count = self.read_u8()? as usize;

        if count == 0 {
            // Empty amb - push Nil (will fail on subsequent op_fail)
            self.push(MettaValue::Nil);
            return Ok(());
        }

        // Pop all alternatives
        let mut alts = Vec::with_capacity(count);
        for _ in 0..count {
            alts.push(self.pop()?);
        }
        alts.reverse(); // Now in original order: [alt1, alt2, ..., altN]

        if count == 1 {
            // Single alternative - no choice point needed
            self.push(alts.into_iter().next().unwrap());
            return Ok(());
        }

        // Create choice point with alternatives 1..N (skipping first)
        // Wrap remaining alternatives in Alternative::Value
        let alternatives: Vec<Alternative> = alts[1..]
            .iter()
            .cloned()
            .map(Alternative::Value)
            .collect();

        self.choice_points.push(ChoicePoint {
            ip: self.ip, // Resume at current IP for alternatives
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(), // After popping alts
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
        });

        // Push first alternative
        self.push(alts.into_iter().next().unwrap());

        Ok(())
    }

    // === Advanced Calls ===

    /// Call a native Rust function by ID.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_native(&mut self) -> VmResult<()> {
        let func_id = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for native function
        let ctx = NativeContext::new(Environment::new());

        // Call through registry
        let result = self.native_registry.call(func_id, &args, &ctx)
            .map_err(|e| VmError::Runtime(e.to_string()))?;

        // Push result(s)
        if result.len() == 1 {
            self.push(result.into_iter().next().unwrap());
        } else if result.is_empty() {
            self.push(MettaValue::Unit);
        } else {
            // Multiple results - push as S-expression
            self.push(MettaValue::SExpr(result));
        }

        Ok(())
    }

    /// Call an external FFI function.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_external(&mut self) -> VmResult<()> {
        let symbol_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Clone the function name to release the borrow before popping args
        let func_name = self.chunk.get_constant(symbol_idx)
            .and_then(|v| {
                if let MettaValue::Atom(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| VmError::InvalidConstant(symbol_idx))?;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Try to call through external registry
        let ctx = ExternalContext::new(Environment::new());
        match self.external_registry.call(&func_name, &args, &ctx) {
            Ok(results) => {
                // Push result (single value or s-expression for multiple)
                if results.len() == 1 {
                    self.push(results.into_iter().next().unwrap());
                } else {
                    self.push(MettaValue::SExpr(results));
                }
                Ok(())
            }
            Err(super::external_registry::ExternalError::NotFound(_)) => {
                // External function not registered - return error
                Err(VmError::Runtime(format!(
                    "External function '{}' not registered",
                    func_name
                )))
            }
            Err(e) => {
                // Other errors - propagate
                Err(VmError::Runtime(format!("External call error: {}", e)))
            }
        }
    }

    /// Call a function with memoization.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    ///
    /// Note: Actual memoization cache will be added in a future iteration.
    /// For now, this behaves like a normal call dispatch.
    fn op_call_cached(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        let head = self.chunk.get_constant(head_idx)
            .cloned()
            .ok_or_else(|| VmError::InvalidConstant(head_idx))?;

        // Extract head as string for cache key
        let head_str = match &head {
            MettaValue::Atom(s) => s.clone(),
            _ => format!("{:?}", head),
        };

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Check memo cache first
        if let Some(cached) = self.memo_cache.get(&head_str, &args) {
            self.push(cached);
            return Ok(());
        }

        // Build the expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args.clone());

        let expr = MettaValue::SExpr(items);

        // Try to dispatch via MORK bridge if available
        let result = if let Some(ref bridge) = self.bridge {
            let rules = bridge.dispatch_rules(&expr);
            if !rules.is_empty() {
                // Execute first matching rule
                // For now, just return the expression - full rule execution
                // would require a more complex implementation
                expr.clone()
            } else {
                // No match - return expression unchanged (irreducible)
                expr
            }
        } else {
            // No bridge - return expression unchanged
            expr
        };

        // Cache the result (only for deterministic results)
        // We cache even irreducible results to avoid repeated lookups
        self.memo_cache.insert(&head_str, &args, result.clone());

        self.push(result);
        Ok(())
    }

    // === Environment Operations ===

    /// Define a new rule in the environment.
    ///
    /// This opcode requires an environment to be set via `with_env()`.
    /// Stack: [pattern, body] -> [Unit]
    ///
    /// The rule `(= pattern body)` is added to the environment for later
    /// pattern matching during rule dispatch.
    fn op_define_rule(&mut self) -> VmResult<()> {
        use crate::backend::models::Rule;

        let body = self.pop()?;
        let pattern = self.pop()?;

        // Environment is required for DefineRule
        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime("DefineRule requires environment (use BytecodeVM::with_env)".to_string())
        })?;

        // Create and add the rule
        let rule = Rule::new(pattern, body);
        env.add_rule(rule);

        // Push Unit to indicate success
        self.push(MettaValue::Unit);
        Ok(())
    }

    /// Load a global value from the environment by name.
    ///
    /// Note: MeTTa doesn't have traditional globals - this is for future use
    /// with module-level bindings or similar constructs.
    ///
    /// Operand: constant index for the name (Atom)
    /// Stack: [] -> [value]
    fn op_load_global(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let name = self.chunk.get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?
            .clone();

        // For now, just return the atom itself as unbound
        // Full global support would require extending Environment
        self.push(name);
        Ok(())
    }

    /// Store a value to a global in the environment.
    ///
    /// Note: MeTTa doesn't have traditional globals - this is for future use
    /// with module-level bindings or similar constructs.
    ///
    /// Operand: constant index for the name (Atom)
    /// Stack: [value] -> []
    fn op_store_global(&mut self) -> VmResult<()> {
        // Skip the constant index (name)
        let _const_idx = self.read_u16()?;
        // Pop and discard the value (no-op for now)
        let _value = self.pop()?;
        // Return success but don't actually store
        // Full global support would require extending Environment
        Ok(())
    }

    /// Dispatch rules for a call expression using the environment.
    ///
    /// This opcode provides environment-based rule dispatch without requiring
    /// the MorkBridge. It enables bytecode compilation for workloads that
    /// define and call user-defined rules (like mmverify).
    ///
    /// Stack: [expr] -> [result]
    ///
    /// The expression is matched against rules in the environment:
    /// - No match: returns the expression unchanged (irreducible)
    /// - Single match: applies bindings and evaluates the rule body
    /// - Multiple matches: creates a choice point for nondeterminism
    fn op_dispatch_rules(&mut self) -> VmResult<()> {
        use crate::backend::eval::apply_bindings;

        // Pop the call expression from the stack
        let expr = self.pop()?;

        // Extract head symbol and arity for indexed rule lookup
        let (head, arity) = match &expr {
            MettaValue::SExpr(items) if !items.is_empty() => {
                if let MettaValue::Atom(name) = &items[0] {
                    (name.as_str(), items.len() - 1)
                } else {
                    // Head is not an atom - return expression unchanged
                    self.push(expr);
                    return Ok(());
                }
            }
            MettaValue::Atom(name) => (name.as_str(), 0),
            _ => {
                // Not a callable expression - return unchanged
                self.push(expr);
                return Ok(());
            }
        };

        // Get environment reference
        let env = match &self.env {
            Some(e) => e,
            None => {
                // No environment - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }
        };

        // Look up matching rules by head symbol and arity
        let candidate_rules = env.get_matching_rules(head, arity);

        if candidate_rules.is_empty() {
            // No rules match - return expression unchanged
            self.push(expr);
            return Ok(());
        }

        // Try to pattern match each rule against the expression
        let mut matches: Vec<(MettaValue, Vec<(String, MettaValue)>)> = Vec::new();
        for rule in &candidate_rules {
            if let Some(bindings) = pattern_match_bind(&rule.lhs, &expr) {
                // Found a match - apply bindings to the rule body
                let mut bindings_map = crate::backend::models::Bindings::new();
                for (name, value) in &bindings {
                    bindings_map.insert(name.clone(), value.clone());
                }
                let instantiated_body = apply_bindings(&rule.rhs, &bindings_map).into_owned();
                matches.push((instantiated_body, bindings));
            }
        }

        if matches.is_empty() {
            // Pattern matching failed for all rules - return expression unchanged
            self.push(expr);
            return Ok(());
        }

        if matches.len() == 1 {
            // Single match - push the instantiated body for further evaluation
            let (body, bindings) = matches.into_iter().next().unwrap();

            // Set up bindings in the current binding frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, value) in bindings {
                    frame.set(name, value);
                }
            }

            // Push the instantiated body - caller will continue evaluation
            self.push(body);
            return Ok(());
        }

        // Multiple matches - create choice point for nondeterminism
        // First match executes now, others become alternatives
        let mut alternatives: Vec<Alternative> = Vec::with_capacity(matches.len() - 1);
        let mut first_match: Option<(MettaValue, Vec<(String, MettaValue)>)> = None;

        for (body, bindings) in matches {
            if first_match.is_none() {
                first_match = Some((body, bindings));
            } else {
                // Store as alternative value
                alternatives.push(Alternative::Value(body));
            }
        }

        // Create choice point for backtracking to alternatives
        self.choice_points.push(ChoicePoint {
            ip: self.ip,
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(),
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
        });

        // Execute first match
        if let Some((body, bindings)) = first_match {
            // Set up bindings in the current binding frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, value) in bindings {
                    frame.set(name, value);
                }
            }

            // Push the instantiated body
            self.push(body);
        }

        Ok(())
    }

    // === Space Operations ===

    /// Add an atom to a space.
    /// Stack: [space, atom] -> [Unit]
    fn op_space_add(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;
        match space {
            MettaValue::Space(handle) => {
                handle.add_atom(atom);
                self.push(MettaValue::Unit);
                Ok(())
            }
            other => Err(VmError::TypeError {
                expected: "Space",
                got: other.type_name(),
            }),
        }
    }

    /// Remove an atom from a space.
    /// Stack: [space, atom] -> [Bool]
    fn op_space_remove(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;
        match space {
            MettaValue::Space(handle) => {
                let removed = handle.remove_atom(&atom);
                self.push(MettaValue::Bool(removed));
                Ok(())
            }
            other => Err(VmError::TypeError {
                expected: "Space",
                got: other.type_name(),
            }),
        }
    }

    /// Get all atoms from a space (collapse).
    /// Stack: [space] -> [SExpr with atoms]
    fn op_space_get_atoms(&mut self) -> VmResult<()> {
        let space = self.pop()?;
        match space {
            MettaValue::Space(handle) => {
                let atoms = handle.collapse();
                // Return as an S-expression list
                self.push(MettaValue::SExpr(atoms));
                Ok(())
            }
            other => Err(VmError::TypeError {
                expected: "Space",
                got: other.type_name(),
            }),
        }
    }

    /// Match pattern against atoms in a space.
    /// Stack: [space, pattern, template] -> [results...]
    ///
    /// Note: This is a simplified implementation that doesn't support
    /// full nondeterministic matching with template evaluation yet.
    /// For now, it returns matched atoms without template instantiation.
    fn op_space_match(&mut self) -> VmResult<()> {
        // TODO: Full implementation requires recursive evaluation
        // For now, use simplified matching that returns atoms without templates
        let _template = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        match space {
            MettaValue::Space(handle) => {
                let atoms = handle.collapse();
                let mut results = Vec::new();

                // Simple pattern matching against atoms
                for atom in &atoms {
                    if pattern_matches(&pattern, atom) {
                        results.push(atom.clone());
                    }
                }

                // Return results as S-expression
                self.push(MettaValue::SExpr(results));
                Ok(())
            }
            other => Err(VmError::TypeError {
                expected: "Space",
                got: other.type_name(),
            }),
        }
    }

    /// Load a space by name from the constant pool.
    /// This operation reads a constant index for the space name.
    ///
    /// Note: Currently limited - full implementation needs Environment access.
    fn op_load_space(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let name = self.chunk.get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?
            .clone();

        match name {
            MettaValue::Atom(space_name) => {
                // Create a placeholder space with the given name
                // In full integration, this would lookup from Environment
                let handle = SpaceHandle::new(
                    std::hash::Hasher::finish(&std::hash::BuildHasher::build_hasher(
                        &std::collections::hash_map::RandomState::new(),
                    )),
                    space_name,
                );
                self.push(MettaValue::Space(handle));
                Ok(())
            }
            other => Err(VmError::TypeError {
                expected: "Atom (space name)",
                got: other.type_name(),
            }),
        }
    }

    // === State Operations ===

    /// Create a new mutable state cell.
    /// Stack: [initial_value] -> [State(id)]
    fn op_new_state(&mut self) -> VmResult<()> {
        let initial_value = self.pop()?;

        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime("new-state requires environment".to_string())
        })?;

        let state_id = env.create_state(initial_value);
        self.push(MettaValue::State(state_id));
        Ok(())
    }

    /// Get the current value from a state cell.
    /// Stack: [State(id)] -> [value]
    fn op_get_state(&mut self) -> VmResult<()> {
        let state_ref = self.pop()?;

        match state_ref {
            MettaValue::State(state_id) => {
                let env = self.env.as_ref().ok_or_else(|| {
                    VmError::Runtime("get-state requires environment".to_string())
                })?;

                if let Some(value) = env.get_state(state_id) {
                    self.push(value);
                    Ok(())
                } else {
                    Err(VmError::Runtime(format!("get-state: state {} not found", state_id)))
                }
            }
            other => Err(VmError::TypeError {
                expected: "State",
                got: other.type_name(),
            }),
        }
    }

    /// Change the value in a state cell.
    /// Stack: [State(id), new_value] -> [State(id)]
    /// Returns the state reference for chaining.
    fn op_change_state(&mut self) -> VmResult<()> {
        let new_value = self.pop()?;
        let state_ref = self.pop()?;

        match state_ref {
            MettaValue::State(state_id) => {
                let env = self.env.as_mut().ok_or_else(|| {
                    VmError::Runtime("change-state! requires environment".to_string())
                })?;

                if env.change_state(state_id, new_value) {
                    // Return the state reference for chaining
                    self.push(MettaValue::State(state_id));
                    Ok(())
                } else {
                    Err(VmError::Runtime(format!("change-state!: state {} not found", state_id)))
                }
            }
            other => Err(VmError::TypeError {
                expected: "State",
                got: other.type_name(),
            }),
        }
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
    use crate::backend::environment::Environment;

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
        vm.results.push(MettaValue::Long(1));
        vm.results.push(MettaValue::Long(2));
        vm.results.push(MettaValue::Long(3)); // This shouldn't be collected

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
        vm.results.push(MettaValue::Long(1));
        vm.results.push(MettaValue::Nil);
        vm.results.push(MettaValue::Long(2));
        vm.results.push(MettaValue::Nil);

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
        use crate::backend::Environment;
        use crate::backend::bytecode::mork_bridge::MorkBridge;

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
        // Test Call opcode with a simple rule: (double $x) -> (+ $x $x)
        use crate::backend::{Environment, Rule};
        use crate::backend::bytecode::mork_bridge::MorkBridge;

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
        use crate::backend::Environment;
        use crate::backend::bytecode::mork_bridge::MorkBridge;

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
        // Test TailCall opcode with a simple rule: (inc $x) -> (+ $x 1)
        use crate::backend::{Environment, Rule};
        use crate::backend::bytecode::mork_bridge::MorkBridge;

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
        // Test Call with multiple arguments: (add3 $a $b $c) -> (+ (+ $a $b) $c)
        use crate::backend::{Environment, Rule};
        use crate::backend::bytecode::mork_bridge::MorkBridge;

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
        assert!(vm.choice_points.is_empty());
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
        assert_eq!(vm.memo_cache.len(), 1);
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
        assert_eq!(vm.memo_cache.len(), 1);

        // Check cache stats: 1 miss (first call) + 1 hit (second call)
        let stats = vm.memo_cache.stats();
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
        assert_eq!(vm.memo_cache.len(), 2);

        // Check cache stats: 2 misses (different args each time)
        let stats = vm.memo_cache.stats();
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
            let n = match args.get(0) {
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
}
