//! Bytecode Virtual Machine
//!
//! The VM executes compiled bytecode using a stack-based architecture with
//! support for nondeterminism via choice points and backtracking.
//!
//! ## Architecture
//!
//! `GenericBytecodeVM<V, F>` is the generic VM parameterized by value type `V`
//! and factory type `F`. The primary concrete type is:
//!
//! ```text
//! pub type BytecodeVM = GenericBytecodeVM<MettaValue, GcFactory>;
//! ```
//!
//! When `F: Default`, convenience constructors (`new`, `with_config`, `with_env`)
//! are available that use `F::default()` for the factory.
//!
//! ## Submodules
//!
//! - `types`: Core type definitions (VmError, VmConfig, CallFrame, etc.)
//! - `pattern`: Pattern matching helpers

use std::fmt;
use std::marker::Unpin;
use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::trace;

use super::opcodes::Opcode;
use crate::backend::models::MettaValue;

// === Submodules ===

mod pattern;
mod types;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proptests;

// === Re-exports ===

pub use pattern::{pattern_match_bind, pattern_matches, unify};
pub use types::{VmConfig, VmError, VmResult};
// Generic types
pub use types::{
    GenericAlternative, GenericBindingFrame, GenericCallFrame, GenericChoicePoint,
    Alternative, BindingFrame, CallFrame, ChoicePoint,
};

// ============================================================================
// Generic Bytecode VM - Zero-Conversion Support
// ============================================================================

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};
use super::chunk::GenericBytecodeChunk;

/// Generic bytecode virtual machine that works with any value type.
///
/// This is the generic version of `BytecodeVM` that enables zero-conversion
/// evaluation with both heap-allocated (`MettaValue`) and arena-allocated
/// (`MettaValue`) values.
///
/// # Type Parameters
///
/// - `V`: The value type (must implement `MettaValueTrait`)
/// - `F`: The factory type for constructing values
///
/// # Design
///
/// The generic VM uses:
/// - `GenericBytecodeChunk<V>` for constant storage
/// - `GenericBindingFrame<V>` for pattern variable bindings
/// - `GenericChoicePoint<V, GenericBytecodeChunk<V>>` for nondeterminism
/// - `GenericEnvironment<V, F>` for rule storage
/// - `MettaValueFactory<V>` trait methods for value construction
///
/// This ensures NO conversions are needed between value types during execution.
pub struct GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Value stack for operands and results
    pub(crate) value_stack: Vec<V>,

    /// Call stack for function frames
    pub(crate) call_stack: Vec<GenericCallFrame<GenericBytecodeChunk<V>>>,

    /// Bindings stack for pattern variables
    pub(crate) bindings_stack: Vec<GenericBindingFrame<V>>,

    /// Choice points for nondeterminism
    pub(crate) choice_points: Vec<GenericChoicePoint<V, GenericBytecodeChunk<V>>>,

    /// Collected results (for nondeterministic evaluation)
    pub(crate) results: Vec<V>,

    /// Current instruction pointer
    pub(crate) ip: usize,

    /// Current bytecode chunk
    pub(crate) chunk: Arc<GenericBytecodeChunk<V>>,

    /// VM configuration
    pub(crate) config: VmConfig,

    /// Factory for constructing values
    pub(crate) factory: F,

    /// Optional environment for rule definitions and lookups
    pub(crate) env: Option<GenericEnvironment<V, F>>,

    /// Native function registry for CallNative opcode
    pub(crate) native_registry: Arc<super::native_registry::GenericNativeRegistry<V, F>>,

    /// External function registry for CallExternal opcode
    pub(crate) external_registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,

    /// Memoization cache for CallCached opcode
    pub(crate) memo_cache: Arc<super::generic_memo_cache::GenericMemoCache<V>>,
}

impl<V, F> fmt::Debug for GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + fmt::Debug + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenericBytecodeVM")
            .field("value_stack_len", &self.value_stack.len())
            .field("call_stack_len", &self.call_stack.len())
            .field("bindings_stack_len", &self.bindings_stack.len())
            .field("choice_points_len", &self.choice_points.len())
            .field("results_len", &self.results.len())
            .field("ip", &self.ip)
            .field("config", &self.config)
            .field("has_env", &self.env.is_some())
            .finish()
    }
}

impl<V, F> GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Create a new generic VM with the given chunk and explicit factory.
    pub fn with_factory(chunk: Arc<GenericBytecodeChunk<V>>, factory: F) -> Self {
        Self::with_config_and_factory(chunk, VmConfig::default(), factory)
    }

    /// Create a new generic VM with custom configuration and explicit factory.
    pub fn with_config_and_factory(chunk: Arc<GenericBytecodeChunk<V>>, config: VmConfig, factory: F) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            native_registry: Arc::new(super::native_registry::GenericNativeRegistry::with_stdlib(factory.clone())),
            external_registry: Arc::new(super::external_registry::GenericExternalRegistry::new()),
            memo_cache: Arc::new(super::generic_memo_cache::GenericMemoCache::default()),
            factory,
            env: None,
        }
    }

    /// Create a new generic VM with an environment and explicit factory.
    pub fn with_env_and_factory(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
        factory: F,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            native_registry: Arc::new(super::native_registry::GenericNativeRegistry::with_stdlib(factory.clone())),
            external_registry: Arc::new(super::external_registry::GenericExternalRegistry::new()),
            memo_cache: Arc::new(super::generic_memo_cache::GenericMemoCache::default()),
            factory,
            env: Some(env),
        }
    }

    /// Create a new generic VM with full configuration including registries.
    pub fn with_registries(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
        factory: F,
        native_registry: Arc<super::native_registry::GenericNativeRegistry<V, F>>,
        external_registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,
        memo_cache: Arc<super::generic_memo_cache::GenericMemoCache<V>>,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            factory,
            env: Some(env),
            native_registry,
            external_registry,
            memo_cache,
        }
    }

    /// Set the external function registry (builder pattern).
    pub fn with_external_registry(mut self, registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>) -> Self {
        self.external_registry = registry;
        self
    }

    /// Set the environment (builder pattern).
    pub fn with_environment(mut self, env: GenericEnvironment<V, F>) -> Self {
        self.env = Some(env);
        self
    }

    /// Push an initial value onto the stack before execution.
    ///
    /// Used for template execution where a binding value needs to be
    /// available as local slot 0.
    #[inline]
    pub fn push_initial_value(&mut self, value: V) {
        self.value_stack.push(value);
    }

    /// Resume VM execution after JIT bailout for non-determinism.
    ///
    /// Allows JIT to compile deterministic parts and then bail out to VM
    /// for Fork/Choice opcodes that require backtracking.
    pub fn resume_from_bailout(
        &mut self,
        bailout_ip: usize,
        value_stack: Vec<V>,
    ) -> VmResult<Vec<V>> {
        self.ip = bailout_ip;
        self.value_stack = value_stack;
        self.run_without_jit()
    }

    /// Run the VM to completion without attempting JIT execution.
    /// Used for resuming after JIT bailout.
    fn run_without_jit(&mut self) -> VmResult<Vec<V>> {
        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Get the factory reference.
    #[inline]
    pub fn factory(&self) -> &F {
        &self.factory
    }

    /// Get a reference to the environment, if present.
    #[inline]
    pub fn environment(&self) -> Option<&GenericEnvironment<V, F>> {
        self.env.as_ref()
    }

    /// Take ownership of the environment.
    #[inline]
    pub fn take_environment(&mut self) -> Option<GenericEnvironment<V, F>> {
        self.env.take()
    }

    // === Stack Operations ===

    /// Push a value onto the value stack.
    #[inline]
    pub fn push(&mut self, value: V) {
        self.value_stack.push(value);
    }

    /// Pop a value from the value stack.
    #[inline]
    pub fn pop(&mut self) -> VmResult<V> {
        self.value_stack.pop().ok_or(VmError::StackUnderflow)
    }

    /// Peek at the top value on the stack.
    #[inline]
    pub fn peek(&self) -> VmResult<&V> {
        self.value_stack.last().ok_or(VmError::StackUnderflow)
    }

    /// Peek at a value n positions from the top.
    #[inline]
    pub fn peek_n(&self, n: usize) -> VmResult<&V> {
        let len = self.value_stack.len();
        if n >= len {
            return Err(VmError::StackUnderflow);
        }
        Ok(&self.value_stack[len - 1 - n])
    }

    /// Duplicate the top value.
    #[inline]
    pub fn dup(&mut self) -> VmResult<()> {
        let value = self.peek()?.clone();
        self.push(value);
        Ok(())
    }

    /// Swap the top two values.
    #[inline]
    pub fn swap(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 2 {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.swap(len - 1, len - 2);
        Ok(())
    }

    // === Bytecode Reading Helpers ===

    /// Read a single byte and advance IP.
    #[inline]
    pub fn read_u8(&mut self) -> VmResult<u8> {
        let byte = self
            .chunk
            .read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        self.ip += 1;
        Ok(byte)
    }

    /// Read a signed byte and advance IP.
    #[inline]
    pub fn read_i8(&mut self) -> VmResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    /// Read a u16 and advance IP.
    #[inline]
    pub fn read_u16(&mut self) -> VmResult<u16> {
        let value = self.chunk.read_u16(self.ip).ok_or(VmError::IpOutOfBounds)?;
        self.ip += 2;
        Ok(value)
    }

    /// Read a signed i16 and advance IP.
    #[inline]
    pub fn read_i16(&mut self) -> VmResult<i16> {
        Ok(self.read_u16()? as i16)
    }

    // === Value Construction Helpers ===

    /// Create a unit value using the factory.
    #[inline]
    pub fn make_unit(&self) -> V {
        self.factory.unit()
    }

    /// Create a boolean value using the factory.
    #[inline]
    pub fn make_bool(&self, b: bool) -> V {
        self.factory.bool(b)
    }

    /// Create a long value using the factory.
    #[inline]
    pub fn make_long(&self, n: i64) -> V {
        self.factory.long(n)
    }

    /// Create a float value using the factory.
    #[inline]
    pub fn make_float(&self, f: f64) -> V {
        self.factory.float(f)
    }

    /// Create an atom value using the factory.
    #[inline]
    pub fn make_atom(&self, name: &str) -> V {
        self.factory.atom(name)
    }

    /// Create a string value using the factory.
    #[inline]
    pub fn make_string(&self, s: &str) -> V {
        self.factory.string(s)
    }

    /// Create an s-expression using the factory.
    #[inline]
    pub fn make_sexpr(&self, items: Vec<V>) -> V {
        self.factory.sexpr(items)
    }

    /// Create an error value using the factory.
    #[inline]
    pub fn make_error(&self, msg: &str, details: V) -> V {
        self.factory.error(msg, details)
    }

    // === Binding Operations ===

    /// Get a binding by name from the bindings stack.
    pub fn get_binding(&self, name: &str) -> Option<&V> {
        for frame in self.bindings_stack.iter().rev() {
            if let Some(value) = frame.get(name) {
                return Some(value);
            }
        }
        None
    }

    /// Set a binding in the current frame.
    pub fn set_binding(&mut self, name: String, value: V) {
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.set(name, value);
        }
    }

    /// Push a new binding frame.
    pub fn push_binding_frame(&mut self) {
        let depth = self.bindings_stack.len() as u32;
        self.bindings_stack.push(GenericBindingFrame::new(depth));
    }

    /// Pop the current binding frame.
    pub fn pop_binding_frame(&mut self) {
        if self.bindings_stack.len() > 1 {
            self.bindings_stack.pop();
        }
    }

    // === Full Execution ===

    /// Run the VM to completion, returning all results.
    ///
    /// This is the complete generic implementation that handles all opcodes
    /// using trait methods for value construction and inspection. NO conversions
    /// between value types occur during execution.
    pub fn run(&mut self) -> VmResult<Vec<V>> {
        // Pre-allocate local variable slots
        let local_count = self.chunk.local_count() as usize;
        if local_count > 0 && self.value_stack.len() < local_count {
            let nil = self.make_unit();
            self.value_stack.resize(local_count, nil);
        }

        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Run the VM with environment, returning results and modified environment.
    pub fn run_with_env(&mut self) -> VmResult<(Vec<V>, Option<GenericEnvironment<V, F>>)> {
        let results = self.run()?;
        let env = self.env.take();
        Ok((results, env))
    }

    /// Execute a single instruction.
    pub fn step(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Bounds check
        if self.ip >= self.chunk.len() {
            return self.handle_chunk_end();
        }

        // Read opcode
        let opcode_byte = self.read_u8()?;
        let opcode = Opcode::from_byte(opcode_byte)
            .ok_or(VmError::InvalidOpcode(opcode_byte))?;

        // Execute opcode
        match opcode {
            // === Stack Operations ===
            Opcode::Nop => {}
            Opcode::Pop => { self.pop()?; }
            Opcode::Dup => self.dup()?,
            Opcode::Swap => self.swap()?,
            Opcode::Rot3 => self.op_rot3()?,
            Opcode::Over => self.op_over()?,
            Opcode::DupN => self.op_dup_n()?,
            Opcode::PopN => self.op_pop_n()?,

            // === Value Creation ===
            Opcode::PushUnit => self.push(self.make_unit()),
            Opcode::PushTrue => self.push(self.make_bool(true)),
            Opcode::PushFalse => self.push(self.make_bool(false)),
            Opcode::PushEmpty => {
                let empty = self.make_sexpr(vec![]);
                self.push(empty);
            }
            Opcode::PushLongSmall => {
                let n = self.read_i8()? as i64;
                self.push(self.make_long(n));
            }
            Opcode::PushLong | Opcode::PushAtom | Opcode::PushString |
            Opcode::PushUri | Opcode::PushConstant => {
                let index = self.read_u16()?;
                let value = self.chunk.get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                self.push(value);
            }
            Opcode::PushVariable => self.op_push_variable()?,
            Opcode::MakeSExpr => {
                let arity = self.read_u8()? as usize;
                let len = self.value_stack.len();
                if arity > len {
                    return Err(VmError::StackUnderflow);
                }
                let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
                self.push(self.make_sexpr(items));
            }
            Opcode::MakeSExprLarge => {
                let arity = self.read_u16()? as usize;
                let len = self.value_stack.len();
                if arity > len {
                    return Err(VmError::StackUnderflow);
                }
                let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
                self.push(self.make_sexpr(items));
            }
            Opcode::MakeList => self.op_make_list()?,
            Opcode::MakeQuote => self.op_make_quote()?,

            // === Variable Operations ===
            Opcode::LoadLocal => {
                let index = self.read_u8()? as usize;
                let value = self.value_stack.get(index)
                    .ok_or(VmError::InvalidLocal(index as u16))?
                    .clone();
                self.push(value);
            }
            Opcode::StoreLocal => {
                let index = self.read_u8()? as usize;
                let value = self.pop()?;
                if index >= self.value_stack.len() {
                    self.value_stack.resize(index + 1, self.make_unit());
                }
                self.value_stack[index] = value;
            }
            Opcode::LoadLocalWide => {
                let index = self.read_u16()? as usize;
                let value = self.value_stack.get(index)
                    .ok_or(VmError::InvalidLocal(index as u16))?
                    .clone();
                self.push(value);
            }
            Opcode::StoreLocalWide => {
                let index = self.read_u16()? as usize;
                let value = self.pop()?;
                if index >= self.value_stack.len() {
                    self.value_stack.resize(index + 1, self.make_unit());
                }
                self.value_stack[index] = value;
            }
            Opcode::LoadBinding => {
                let index = self.read_u16()?;
                let name = self.chunk.get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                // Search bindings from innermost to outermost
                if let Some(value) = self.get_binding(&name).cloned() {
                    self.push(value);
                } else {
                    return Err(VmError::InvalidBinding(name));
                }
            }
            Opcode::StoreBinding => {
                let index = self.read_u16()?;
                let name = self.chunk.get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let value = self.pop()?;
                self.set_binding(name, value);
            }
            Opcode::HasBinding => {
                let index = self.read_u16()?;
                let name = self.chunk.get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let has = self.get_binding(&name).is_some();
                self.push(self.make_bool(has));
            }
            Opcode::ClearBindings => {
                if let Some(frame) = self.bindings_stack.last_mut() {
                    frame.clear();
                }
            }
            Opcode::PushBindingFrame => self.push_binding_frame(),
            Opcode::PopBindingFrame => {
                if self.bindings_stack.len() <= 1 {
                    return Err(VmError::Runtime("Cannot pop root binding frame".into()));
                }
                self.pop_binding_frame();
            }
            Opcode::LoadUpvalue => {
                // Upvalues are stored as constants in parent scopes
                let index = self.read_u16()?;
                let value = self.chunk.get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                self.push(value);
            }

            // === Control Flow ===
            Opcode::Jump => {
                let offset = self.read_i16()?;
                self.ip = (self.ip as isize + offset as isize) as usize;
            }
            Opcode::JumpIfFalse => {
                let offset = self.read_i16()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(false) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfTrue => {
                let offset = self.read_i16()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(true) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfUnit => {
                let offset = self.read_i16()?;
                let value = self.pop()?;
                if value.is_unit() {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfError => {
                let offset = self.read_i16()?;
                let value = self.peek()?;
                if value.is_error() {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpShort => {
                let offset = self.read_i8()?;
                self.ip = (self.ip as isize + offset as isize) as usize;
            }
            Opcode::JumpIfFalseShort => {
                let offset = self.read_i8()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(false) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfTrueShort => {
                let offset = self.read_i8()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(true) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpTable => self.op_jump_table()?,
            Opcode::Call => self.op_call()?,
            Opcode::TailCall => self.op_tail_call()?,
            Opcode::CallN => self.op_call_n()?,
            Opcode::TailCallN => self.op_tail_call_n()?,
            Opcode::Return => return self.op_return(),
            Opcode::ReturnMulti => return self.op_return_multi(),

            // === Arithmetic ===
            Opcode::Add => self.op_binary_num(|a, b| a.wrapping_add(b), |a, b| a + b)?,
            Opcode::Sub => self.op_binary_num(|a, b| a.wrapping_sub(b), |a, b| a - b)?,
            Opcode::Mul => self.op_binary_num(|a, b| a.wrapping_mul(b), |a, b| a * b)?,
            Opcode::Div => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_long(), b.as_long()) {
                    (Some(_), Some(0)) => return Err(VmError::DivisionByZero),
                    (Some(x), Some(y)) => match x.checked_div(y) {
                        Some(r) => self.push(self.make_long(r)),
                        None => return Err(VmError::ArithmeticOverflow),
                    },
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) => {
                            if y == 0.0 {
                                return Err(VmError::DivisionByZero);
                            }
                            self.push(self.make_float(x / y));
                        }
                        _ => {
                            // Mixed Long/Float type promotion
                            match (a.as_long(), b.as_float()) {
                                (Some(x), Some(y)) => {
                                    if y == 0.0 { return Err(VmError::DivisionByZero); }
                                    self.push(self.make_float(x as f64 / y));
                                }
                                _ => match (a.as_float(), b.as_long()) {
                                    (Some(x), Some(y)) => {
                                        if y == 0 { return Err(VmError::DivisionByZero); }
                                        self.push(self.make_float(x / y as f64));
                                    }
                                    _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                                }
                            }
                        }
                    }
                }
            }
            Opcode::Mod => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_long(), b.as_long()) {
                    (Some(_), Some(0)) => return Err(VmError::DivisionByZero),
                    (Some(x), Some(y)) => match x.checked_rem(y) {
                        Some(r) => self.push(self.make_long(r)),
                        None => return Err(VmError::ArithmeticOverflow),
                    },
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(x % y)),
                        _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                    }
                }
            }
            Opcode::Neg => {
                let a = self.pop()?;
                if let Some(x) = a.as_long() {
                    self.push(self.make_long(-x));
                } else if let Some(x) = a.as_float() {
                    self.push(self.make_float(-x));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Abs => {
                let a = self.pop()?;
                if let Some(x) = a.as_long() {
                    // i64::MIN.abs() overflows because |i64::MIN| > i64::MAX
                    if x == i64::MIN {
                        return Err(VmError::ArithmeticOverflow);
                    }
                    self.push(self.make_long(x.abs()));
                } else if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.abs()));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::FloorDiv => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_long(), b.as_long()) {
                    (Some(_), Some(0)) => return Err(VmError::DivisionByZero),
                    (Some(x), Some(y)) => {
                        self.push(self.make_long(x.div_euclid(y)));
                    }
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) if y != 0.0 => {
                            self.push(self.make_long((x / y).floor() as i64));
                        }
                        (Some(_), Some(_)) => return Err(VmError::DivisionByZero),
                        _ => {
                            // Mixed Long/Float type promotion
                            match (a.as_long(), b.as_float()) {
                                (Some(x), Some(y)) => {
                                    if y == 0.0 { return Err(VmError::DivisionByZero); }
                                    self.push(self.make_long((x as f64 / y).floor() as i64));
                                }
                                _ => match (a.as_float(), b.as_long()) {
                                    (Some(x), Some(y)) => {
                                        if y == 0 { return Err(VmError::DivisionByZero); }
                                        self.push(self.make_long((x / y as f64).floor() as i64));
                                    }
                                    _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                                }
                            }
                        }
                    }
                }
            }
            Opcode::Pow => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_long(), b.as_long()) {
                    (Some(x), Some(y)) if y >= 0 => {
                        self.push(self.make_long(x.pow(y as u32)));
                    }
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(x.powf(y))),
                        _ => match (a.as_long(), b.as_float()) {
                            (Some(x), Some(y)) => self.push(self.make_float((x as f64).powf(y))),
                            _ => match (a.as_float(), b.as_long()) {
                                (Some(x), Some(y)) => self.push(self.make_float(x.powi(y as i32))),
                                _ => return Err(VmError::TypeError {
                                    expected: "number (Long or Float)",
                                    got: "other",
                                }),
                            }
                        }
                    }
                }
            }
            Opcode::Sqrt => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.sqrt()));
                } else if let Some(x) = a.as_long() {
                    self.push(self.make_float((x as f64).sqrt()));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Log => {
                // Binary log(base, value) - stack: [base, value] -> pops value first, then base
                let value = self.pop()?;
                let base = self.pop()?;
                match (base.as_float(), value.as_float()) {
                    (Some(b), Some(v)) => self.push(self.make_float(v.log(b))),
                    _ => match (base.as_long(), value.as_float()) {
                        (Some(b), Some(v)) => self.push(self.make_float(v.log(b as f64))),
                        _ => match (base.as_float(), value.as_long()) {
                            (Some(b), Some(v)) => self.push(self.make_float((v as f64).log(b))),
                            _ => match (base.as_long(), value.as_long()) {
                                (Some(b), Some(v)) => self.push(self.make_float((v as f64).log(b as f64))),
                                _ => return Err(VmError::TypeError { expected: "Float or Long", got: "other" }),
                            }
                        }
                    }
                }
            }
            Opcode::Trunc => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_long(x.trunc() as i64));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Ceil => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_long(x.ceil() as i64));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "Float or Long", got: "other" });
                }
            }
            Opcode::FloorMath => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_long(x.floor() as i64));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "Float or Long", got: "other" });
                }
            }
            Opcode::Round => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_long(x.round() as i64));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "Float or Long", got: "other" });
                }
            }
            Opcode::Sin => self.op_unary_float(f64::sin)?,
            Opcode::Cos => self.op_unary_float(f64::cos)?,
            Opcode::Tan => self.op_unary_float(f64::tan)?,
            Opcode::Asin => self.op_unary_float(f64::asin)?,
            Opcode::Acos => self.op_unary_float(f64::acos)?,
            Opcode::Atan => self.op_unary_float(f64::atan)?,
            Opcode::IsNan => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_bool(x.is_nan()));
                } else if a.as_long().is_some() {
                    self.push(self.make_bool(false)); // integers are never NaN
                } else {
                    return Err(VmError::TypeError { expected: "Float or Long", got: "other" });
                }
            }
            Opcode::IsInf => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_bool(x.is_infinite()));
                } else if a.as_long().is_some() {
                    self.push(self.make_bool(false)); // integers are never infinite
                } else {
                    return Err(VmError::TypeError { expected: "Float or Long", got: "other" });
                }
            }

            // === Comparison ===
            Opcode::Lt => self.op_comparison(|a, b| a < b, |a, b| a < b)?,
            Opcode::Le => self.op_comparison(|a, b| a <= b, |a, b| a <= b)?,
            Opcode::Gt => self.op_comparison(|a, b| a > b, |a, b| a > b)?,
            Opcode::Ge => self.op_comparison(|a, b| a >= b, |a, b| a >= b)?,
            Opcode::Eq => {
                let b = self.pop()?;
                let a = self.pop()?;
                let equal = a.structurally_equivalent(&b);
                self.push(self.make_bool(equal));
            }
            Opcode::Ne => {
                let b = self.pop()?;
                let a = self.pop()?;
                let not_equal = !a.structurally_equivalent(&b);
                self.push(self.make_bool(not_equal));
            }
            Opcode::StructEq => {
                let b = self.pop()?;
                let a = self.pop()?;
                let equal = a.structurally_equivalent(&b);
                self.push(self.make_bool(equal));
            }

            // === Boolean ===
            Opcode::And => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x && y)),
                    _ => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }
            Opcode::Or => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x || y)),
                    _ => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }
            Opcode::Not => {
                let a = self.pop()?;
                match a.as_bool() {
                    Some(x) => self.push(self.make_bool(!x)),
                    None => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }
            Opcode::Xor => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x ^ y)),
                    _ => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }

            // === Type Operations ===
            Opcode::GetType => self.op_get_type()?,
            Opcode::CheckType => self.op_check_type()?,
            Opcode::IsType => self.op_is_type()?,
            Opcode::AssertType => self.op_assert_type()?,

            // === Pattern Matching ===
            Opcode::Match => self.op_match()?,
            Opcode::MatchBind => self.op_match_bind()?,
            Opcode::MatchHead => self.op_match_head()?,
            Opcode::MatchArity => self.op_match_arity()?,
            Opcode::MatchGuard => self.op_match_guard()?,
            Opcode::Unify => self.op_unify()?,
            Opcode::UnifyBind => self.op_unify_bind()?,
            Opcode::IsVariable => {
                let a = self.pop()?;
                let is_var = a.is_variable();
                self.push(self.make_bool(is_var));
            }
            Opcode::IsSExpr => {
                let a = self.pop()?;
                let is_sexpr = a.as_sexpr().is_some();
                self.push(self.make_bool(is_sexpr));
            }
            Opcode::IsSymbol => {
                let a = self.pop()?;
                let is_symbol = a.as_atom().is_some();
                self.push(self.make_bool(is_symbol));
            }
            Opcode::GetHead => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    if let Some(first) = items.first() {
                        self.push(first.clone());
                    } else {
                        return Err(VmError::TypeError {
                            expected: "non-empty S-expression",
                            got: "other",
                        });
                    }
                } else {
                    return Err(VmError::TypeError {
                        expected: "non-empty S-expression",
                        got: "other",
                    });
                }
            }
            Opcode::GetTail => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    if !items.is_empty() {
                        let tail: Vec<V> = items[1..].to_vec();
                        self.push(self.make_sexpr(tail));
                    } else {
                        return Err(VmError::TypeError {
                            expected: "non-empty S-expression",
                            got: "other",
                        });
                    }
                } else {
                    return Err(VmError::TypeError {
                        expected: "non-empty S-expression",
                        got: "other",
                    });
                }
            }
            Opcode::GetArity => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    self.push(self.make_long(items.len() as i64));
                } else {
                    return Err(VmError::TypeError {
                        expected: "S-expression",
                        got: "other",
                    });
                }
            }
            Opcode::GetElement => {
                let index = self.read_u8()? as usize;
                let value = self.pop()?;
                if let Some(items) = value.as_sexpr() {
                    if index < items.len() {
                        self.push(items[index].clone());
                    } else {
                        return Err(VmError::TypeError {
                            expected: "S-expression with valid index",
                            got: "other",
                        });
                    }
                } else {
                    return Err(VmError::TypeError {
                        expected: "S-expression with valid index",
                        got: "other",
                    });
                }
            }
            Opcode::DeconAtom => self.op_decon_atom()?,
            Opcode::Repr => self.op_repr()?,
            Opcode::GetMetaType => self.op_get_metatype()?,
            Opcode::ConsAtom => self.op_cons_atom()?,
            Opcode::MapAtom => self.op_map_atom()?,
            Opcode::FilterAtom => self.op_filter_atom()?,
            Opcode::FoldlAtom => self.op_foldl_atom()?,
            Opcode::IndexAtom => self.op_index_atom()?,
            Opcode::MinAtom => self.op_min_atom()?,
            Opcode::MaxAtom => self.op_max_atom()?,

            // === Nondeterminism ===
            Opcode::Fork => return self.op_fork(),
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
            Opcode::Backtrack => return self.op_fail(),

            // === Advanced Calls ===
            Opcode::CallNative => self.op_call_native()?,
            Opcode::CallExternal => self.op_call_external()?,
            Opcode::CallCached => self.op_call_cached()?,

            // === Environment Operations ===
            Opcode::DefineRule => self.op_define_rule()?,
            Opcode::LoadGlobal => self.op_load_global()?,
            Opcode::StoreGlobal => self.op_store_global()?,
            Opcode::DispatchRules => self.op_dispatch_rules()?,

            // === Space Operations ===
            Opcode::SpaceAdd => self.op_space_add()?,
            Opcode::SpaceRemove => self.op_space_remove()?,
            Opcode::SpaceGetAtoms => self.op_space_get_atoms()?,
            Opcode::SpaceMatch => self.op_space_match()?,
            Opcode::LoadSpace => self.op_load_space()?,

            // === State Operations ===
            Opcode::NewState => self.op_new_state()?,
            Opcode::GetState => self.op_get_state()?,
            Opcode::ChangeState => self.op_change_state()?,

            // === Set Operations & Alpha-Equivalence ===
            Opcode::EvalIfEqual => self.op_eval_if_equal()?,
            Opcode::UniqueAtom => self.op_unique_atom()?,
            Opcode::UnionAtom => self.op_union_atom()?,
            Opcode::IntersectionAtom => self.op_intersection_atom()?,
            Opcode::SubtractionAtom => self.op_subtraction_atom()?,

            // === Debug ===
            Opcode::Breakpoint => self.op_breakpoint()?,
            Opcode::Trace => self.op_trace()?,
            Opcode::Halt => return Err(VmError::Halted),

            // Catch-all for any unhandled opcodes
            _ => {
                return Err(VmError::Runtime(format!(
                    "Opcode {:?} not yet implemented in generic VM",
                    opcode
                )));
            }
        }

        Ok(ControlFlow::Continue(()))
    }

    /// Handle reaching the end of a bytecode chunk.
    fn handle_chunk_end(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller
            let value = self.pop().unwrap_or_else(|_| self.make_unit());
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);

            // Pop binding frame
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

    // === Helper Methods for Opcodes ===

    /// Binary numeric operation helper.
    #[inline]
    fn op_binary_num(
        &mut self,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        match (a.as_long(), b.as_long()) {
            (Some(x), Some(y)) => self.push(self.make_long(int_op(x, y))),
            _ => match (a.as_float(), b.as_float()) {
                (Some(x), Some(y)) => self.push(self.make_float(float_op(x, y))),
                _ => {
                    // Mixed Long/Float type promotion
                    match (a.as_long(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(float_op(x as f64, y))),
                        _ => match (a.as_float(), b.as_long()) {
                            (Some(x), Some(y)) => self.push(self.make_float(float_op(x, y as f64))),
                            _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Unary float operation helper.
    #[inline]
    fn op_unary_float(&mut self, op: impl Fn(f64) -> f64) -> VmResult<()> {
        let a = self.pop()?;
        if let Some(x) = a.as_float() {
            self.push(self.make_float(op(x)));
        } else if let Some(x) = a.as_long() {
            self.push(self.make_float(op(x as f64)));
        } else {
            return Err(VmError::TypeError { expected: "number", got: "other" });
        }
        Ok(())
    }

    /// Comparison operation helper.
    #[inline]
    fn op_comparison(
        &mut self,
        int_cmp: impl Fn(i64, i64) -> bool,
        float_cmp: impl Fn(f64, f64) -> bool,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        match (a.as_long(), b.as_long()) {
            (Some(x), Some(y)) => self.push(self.make_bool(int_cmp(x, y))),
            _ => match (a.as_float(), b.as_float()) {
                (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x, y))),
                _ => {
                    // Mixed Long/Float type promotion
                    match (a.as_long(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x as f64, y))),
                        _ => match (a.as_float(), b.as_long()) {
                            (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x, y as f64))),
                            _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Rot3: rotate top 3 stack elements [a, b, c] -> [c, a, b].
    fn op_rot3(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 3 {
            return Err(VmError::StackUnderflow);
        }
        // [a, b, c] -> [c, a, b]
        let c = self.value_stack.pop().expect("length checked");
        let b = self.value_stack.pop().expect("length checked");
        let a = self.value_stack.pop().expect("length checked");
        self.value_stack.push(c);
        self.value_stack.push(a);
        self.value_stack.push(b);
        Ok(())
    }

    /// Over: copy second element to top (a b -> a b a).
    fn op_over(&mut self) -> VmResult<()> {
        let value = self.peek_n(1)?.clone();
        self.push(value);
        Ok(())
    }

    /// DupN: duplicate top N values.
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

    /// PopN: pop n elements from stack.
    fn op_pop_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.truncate(len - n);
        Ok(())
    }

    // === Stub implementations for complex opcodes ===
    // These need full implementation with trait-based logic

    fn op_push_variable(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let var = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();

        // Check if it's a pattern variable that should be resolved from bindings
        if let Some(name) = var.as_atom() {
            if name.starts_with('$') {
                // Search bindings from innermost to outermost
                for frame in self.bindings_stack.iter().rev() {
                    if let Some(value) = frame.get(name) {
                        self.push(value.clone());
                        return Ok(());
                    }
                }
            }
        }

        // Not found in bindings or not a pattern variable - push as-is
        self.push(var);
        Ok(())
    }

    fn op_make_list(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let elements: Vec<V> = self.value_stack.drain((len - arity)..).collect();
        // Build proper Cons-list by folding right
        let mut list = self.make_unit();
        for elem in elements.into_iter().rev() {
            let cons_atom = self.make_atom("Cons");
            list = self.make_sexpr(vec![cons_atom, elem, list]);
        }
        self.push(list);
        Ok(())
    }

    fn op_make_quote(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let quote_atom = self.make_atom("quote");
        let quoted = self.make_sexpr(vec![quote_atom, value]);
        self.push(quoted);
        Ok(())
    }

    /// Multi-way branch via jump table.
    ///
    /// Reads a table index from bytecode, pops a selector value from stack,
    /// looks up the corresponding offset in the jump table, and jumps to it.
    /// If the selector doesn't match any entry, jumps to the default offset.
    ///
    /// Stack: [selector] -> []
    /// Bytecode: JumpTable table_index:u16
    fn op_jump_table(&mut self) -> VmResult<()> {
        let table_index = self.read_u16()? as usize;
        let selector = self.pop()?;

        // Get jump table from chunk
        let jump_table = self
            .chunk
            .get_jump_table(table_index)
            .ok_or_else(|| VmError::Runtime(format!("Invalid jump table index: {}", table_index)))?;

        // Compute hash of selector value for table lookup
        use xxhash_rust::xxh3::xxh3_64;
        // Hash the selector value using debug repr for consistent hashing
        let selector_hash = xxh3_64(format!("{:?}", selector).as_bytes());

        // Look up in jump table entries
        let target_offset = jump_table
            .entries
            .iter()
            .find(|(hash, _)| *hash == selector_hash)
            .map(|(_, offset)| *offset)
            .unwrap_or(jump_table.default_offset);

        // Jump to target
        self.ip = target_offset;
        Ok(())
    }

    fn op_call(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Build call expression from head + args
        let head = self.chunk.get_constant(head_idx).cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules
        self.push(expr);
        self.op_dispatch_rules()?;

        Ok(())
    }

    fn op_tail_call(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Build call expression from head + args
        let head = self.chunk.get_constant(head_idx).cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules (tail call - no new frame)
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    /// Call with N arguments where the head is on the stack (not constant pool).
    ///
    /// Pops the head and N arguments from the stack, builds a call expression,
    /// and dispatches to environment for rule matching.
    ///
    /// Stack: [head, arg1, arg2, ..., argN] -> [result]
    /// Bytecode: CallN arity:u8
    fn op_call_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_n");
        let arity = self.read_u8()? as usize;

        // Pop arguments and head from stack
        // Stack order: head is pushed first, then args left-to-right
        // So we need to pop args first, then head
        if self.value_stack.len() < arity + 1 {
            return Err(VmError::StackUnderflow);
        }

        // Pop arguments
        let args: Vec<V> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Pop head
        let head = self.pop()?;

        // Extract head symbol if it's an atom
        let is_atom = head.as_atom().is_some();

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        if !is_atom {
            // Head is not an atom - return expression as data
            self.push(expr);
            return Ok(());
        }

        // Dispatch via environment rules
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    /// Tail call with N arguments where the head is on the stack.
    /// Same as CallN but reuses current call frame for TCO.
    ///
    /// Stack: [head, arg1, arg2, ..., argN] -> [result]
    /// Bytecode: TailCallN arity:u8
    fn op_tail_call_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "tail_call_n");
        let arity = self.read_u8()? as usize;

        // Pop arguments and head from stack
        if self.value_stack.len() < arity + 1 {
            return Err(VmError::StackUnderflow);
        }

        // Pop arguments
        let args: Vec<V> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Pop head
        let head = self.pop()?;

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules (tail call - no new frame)
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    fn op_return(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "return");
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

    fn op_return_multi(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Return all values on stack above base_ptr
        let base = self.call_stack.last().map(|f| f.base_ptr).unwrap_or(0);
        let values: Vec<V> = self.value_stack.drain(base..).collect();

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

    fn op_get_type(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let type_name = value.type_name();
        self.push(self.make_atom(type_name));
        Ok(())
    }

    fn op_check_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.pop()?;

        let expected = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError { expected: "type symbol", got: "other" });
        };

        // Type variables match anything (consistent with tree-visitor)
        let matches = if expected.starts_with('$') {
            true
        } else {
            value.type_name() == expected
        };
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_is_type(&mut self) -> VmResult<()> {
        // Same as check_type for now
        self.op_check_type()
    }

    fn op_assert_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.peek()?;

        let expected = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError { expected: "type symbol", got: "other" });
        };

        if value.type_name() != expected {
            return Err(VmError::TypeError {
                expected: "matching type",
                got: value.type_name(),
            });
        }
        Ok(())
    }

    fn op_match(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let pattern = self.pop()?;
        let matches = self.pattern_matches_generic(&pattern, &value);
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_bind(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let pattern = self.pop()?;

        if let Some(bindings) = self.pattern_match_bind_generic(&pattern, &value) {
            for (name, val) in bindings {
                self.set_binding(name, val);
            }
            self.push(self.make_bool(true));
        } else {
            self.push(self.make_bool(false));
        }
        Ok(())
    }

    /// Match head symbol of an S-expression for fast dispatch optimization.
    ///
    /// Reads an expected symbol index from the bytecode, pops a value from the stack,
    /// and checks if the value is an S-expression whose first element matches the
    /// expected symbol. Pushes Bool(true) if it matches, Bool(false) otherwise.
    ///
    /// Stack: [value] -> [bool]
    /// Bytecode: MatchHead expected_index:u8
    fn op_match_head(&mut self) -> VmResult<()> {
        let expected_index = self.read_u8()? as u16;

        // Get expected symbol from constant pool and clone it to avoid borrow issues
        let expected = self
            .chunk
            .get_constant(expected_index)
            .ok_or(VmError::InvalidConstant(expected_index))?
            .clone();

        let value = self.pop()?;

        // Check if value is an S-expression with matching head
        let matches = if let Some(items) = value.as_sexpr() {
            if let Some(head) = items.first() {
                // Compare expected atom against head atom
                if let (Some(exp_sym), Some(head_sym)) = (expected.as_atom(), head.as_atom()) {
                    exp_sym == head_sym
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_arity(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let value = self.pop()?;

        let matches = if let Some(items) = value.as_sexpr() {
            items.len() == arity
        } else {
            false
        };
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_guard(&mut self) -> VmResult<()> {
        let guard_result = self.pop()?;
        if guard_result.as_bool() != Some(true) {
            // Guard failed - push false
            self.push(self.make_bool(false));
        } else {
            self.push(self.make_bool(true));
        }
        Ok(())
    }

    fn op_unify(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let unified = self.unify_generic(&a, &b).is_some();
        self.push(self.make_bool(unified));
        Ok(())
    }

    fn op_unify_bind(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;

        if let Some(bindings) = self.unify_generic(&a, &b) {
            for (name, val) in bindings {
                self.set_binding(name, val);
            }
            self.push(self.make_bool(true));
        } else {
            self.push(self.make_bool(false));
        }
        Ok(())
    }

    fn op_decon_atom(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                return Err(VmError::TypeError {
                    expected: "non-empty S-expression",
                    got: "empty or non-expression",
                });
            }
            let head = items[0].clone();
            let tail = self.make_sexpr(items[1..].to_vec());
            // Return (head tail) pair as S-expression
            self.push(self.factory.sexpr(vec![head, tail]));
        } else {
            // Empty or non-expression: nondeterministic failure
            return Err(VmError::TypeError {
                expected: "non-empty S-expression",
                got: "empty or non-expression",
            });
        }
        Ok(())
    }

    fn op_repr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let repr = value.friendly_repr();
        self.push(self.make_string(&repr));
        Ok(())
    }

    fn op_get_metatype(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let metatype = if value.as_sexpr().is_some() {
            "Expression"
        } else if value.is_variable() {
            "Variable"
        } else if value.as_atom().is_some() {
            "Symbol"
        } else if value.as_bool().is_some() {
            "Bool"
        } else if value.as_long().is_some() || value.as_float().is_some() {
            "Number"
        } else if value.as_string().is_some() {
            "String"
        } else if value.is_unit() {
            "Unit"
        } else if value.is_error() {
            "Error"
        } else if value.as_state().is_some() {
            "State"
        } else {
            "Grounded"
        };
        self.push(self.make_atom(metatype));
        Ok(())
    }

    /// cons-atom: prepend head to tail S-expression
    /// Matches tree-visitor semantics in list_ops.rs:118-126
    fn op_cons_atom(&mut self) -> VmResult<()> {
        let tail = self.pop()?;
        let head = self.pop()?;

        if let Some(tail_items) = tail.as_sexpr() {
            // Prepend head to existing S-expression
            let mut items = Vec::with_capacity(tail_items.len() + 1);
            items.push(head);
            items.extend(tail_items.iter().cloned());
            self.push(self.make_sexpr(items));
        } else if tail.is_unit() {
            // Create single-element S-expression
            self.push(self.make_sexpr(vec![head]));
        } else {
            return Err(VmError::TypeError {
                expected: "S-expression or Nil",
                got: "other",
            });
        }
        Ok(())
    }

    /// Map a template chunk over each element of an S-expression.
    /// Operand: u16 chunk_idx
    /// Stack: [list] -> [mapped_list]
    fn op_map_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let template_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut results = Vec::with_capacity(items.len());
        for item in items {
            let result =
                self.execute_generic_template_with_binding(Arc::clone(&template_chunk), item.clone())?;
            results.push(result);
        }

        self.push(self.factory.sexpr(results));
        Ok(())
    }

    /// Filter elements of an S-expression using a predicate chunk.
    /// Operand: u16 chunk_idx
    /// Stack: [list] -> [filtered_list]
    fn op_filter_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let predicate_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut results = Vec::new();
        for item in items {
            let result = self.execute_generic_template_with_binding(
                Arc::clone(&predicate_chunk),
                item.clone(),
            )?;
            // Check if predicate returned true
            if result.as_bool() == Some(true) {
                results.push(item.clone());
            }
        }

        self.push(self.factory.sexpr(results));
        Ok(())
    }

    /// Left fold over an S-expression using a template chunk.
    /// Operand: u16 chunk_idx
    /// Stack: [list, init] -> [result]
    fn op_foldl_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let init = self.pop()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let op_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut acc = init;
        for item in items {
            acc = self.execute_generic_foldl_template(Arc::clone(&op_chunk), acc, item.clone())?;
        }

        self.push(acc);
        Ok(())
    }

    // === Template Execution Helpers ===

    /// Execute a template chunk with a single bound value (for map/filter).
    /// Saves and restores VM state around execution.
    fn execute_generic_template_with_binding(
        &mut self,
        chunk: Arc<GenericBytecodeChunk<V>>,
        binding: V,
    ) -> VmResult<V> {
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
            let opcode_byte = self
                .chunk
                .read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode =
                Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break;
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or_else(|| self.factory.unit()));
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
        let result = self.pop().unwrap_or_else(|_| self.factory.unit());

        // Restore state
        self.ip = saved_ip;
        self.chunk = saved_chunk;

        // Cleanup any remaining stack entries from template
        self.value_stack.truncate(saved_stack_base);

        Ok(result)
    }

    /// Execute a foldl template chunk with accumulator and item bindings.
    /// Saves and restores VM state around execution.
    fn execute_generic_foldl_template(
        &mut self,
        chunk: Arc<GenericBytecodeChunk<V>>,
        acc: V,
        item: V,
    ) -> VmResult<V> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.push(acc); // Local slot 0: accumulator
        self.push(item); // Local slot 1: item

        // Execute until Return or end of chunk
        loop {
            if self.ip >= self.chunk.len() {
                break;
            }
            let opcode_byte = self
                .chunk
                .read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode =
                Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break;
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or_else(|| self.factory.unit()));
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
        let result = self.pop().unwrap_or_else(|_| self.factory.unit());

        // Restore state
        self.ip = saved_ip;
        self.chunk = saved_chunk;
        self.value_stack.truncate(saved_stack_base);

        Ok(result)
    }

    fn op_index_atom(&mut self) -> VmResult<()> {
        let index = self.pop()?;
        let expr = self.pop()?;

        let idx = match index.as_long() {
            Some(i) => i,
            None => {
                return Err(VmError::TypeError {
                    expected: "Long (index)",
                    got: "other",
                });
            }
        };

        match expr.as_sexpr() {
            Some(items) => {
                if idx < 0 || idx as usize >= items.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: items.len(),
                    });
                }
                self.push(items[idx as usize].clone());
            }
            None => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                });
            }
        }
        Ok(())
    }

    fn op_min_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr.as_sexpr() {
            Some(items) => items,
            None => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                });
            }
        };

        if items.is_empty() {
            return Err(VmError::TypeError {
                expected: "non-empty S-expression",
                got: "empty expression",
            });
        }

        // Find minimum among numeric values
        let mut min_val: Option<f64> = None;
        let mut min_is_long = true;

        for item in items {
            if let Some(x) = item.as_long() {
                let val = x as f64;
                min_val = Some(min_val.map_or(val, |m: f64| m.min(val)));
            } else if let Some(x) = item.as_float() {
                min_is_long = false;
                min_val = Some(min_val.map_or(x, |m: f64| m.min(x)));
            }
            // Skip non-numeric values
        }

        match min_val {
            Some(v) if min_is_long && v == (v as i64) as f64 => {
                self.push(self.make_long(v as i64));
            }
            Some(v) => {
                self.push(self.make_float(v));
            }
            None => {
                return Err(VmError::TypeError {
                    expected: "numeric values in expression",
                    got: "no numeric values",
                });
            }
        }
        Ok(())
    }

    fn op_max_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr.as_sexpr() {
            Some(items) => items,
            None => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                });
            }
        };

        if items.is_empty() {
            return Err(VmError::TypeError {
                expected: "non-empty S-expression",
                got: "empty expression",
            });
        }

        // Find maximum among numeric values
        let mut max_val: Option<f64> = None;
        let mut max_is_long = true;

        for item in items {
            if let Some(x) = item.as_long() {
                let val = x as f64;
                max_val = Some(max_val.map_or(val, |m: f64| m.max(val)));
            } else if let Some(x) = item.as_float() {
                max_is_long = false;
                max_val = Some(max_val.map_or(x, |m: f64| m.max(x)));
            }
            // Skip non-numeric values
        }

        match max_val {
            Some(v) if max_is_long && v == (v as i64) as f64 => {
                self.push(self.make_long(v as i64));
            }
            Some(v) => {
                self.push(self.make_float(v));
            }
            None => {
                return Err(VmError::TypeError {
                    expected: "numeric values in expression",
                    got: "no numeric values",
                });
            }
        }
        Ok(())
    }

    // === Set Operations & Alpha-Equivalence ===

    /// if-equal: alpha-equivalence conditional
    /// Stack: [pred1, pred2, then_val, else_val] -> [result]
    fn op_eval_if_equal(&mut self) -> VmResult<()> {
        let else_val = self.pop()?;
        let then_val = self.pop()?;
        let pred2 = self.pop()?;
        let pred1 = self.pop()?;

        // Use alpha-equivalence comparison (matches MeTTa HE semantics)
        let result = if self.alpha_equiv(&pred1, &pred2) {
            then_val
        } else {
            else_val
        };
        self.push(result);
        Ok(())
    }

    /// unique-atom: deduplicate list by alpha-equivalence
    /// Stack: [list] -> [deduped_list]
    fn op_unique_atom(&mut self) -> VmResult<()> {
        let list = self.pop()?;
        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        // O(n²) alpha-equivalence dedup (matches MeTTa HE)
        let mut unique: Vec<V> = Vec::with_capacity(items.len());
        for item in items {
            let already_seen = unique.iter().any(|seen| self.alpha_equiv(seen, item));
            if !already_seen {
                unique.push(item.clone());
            }
        }
        self.push(self.make_sexpr(unique));
        Ok(())
    }

    /// union-atom: concatenate two lists
    /// Stack: [left, right] -> [combined]
    fn op_union_atom(&mut self) -> VmResult<()> {
        let right = self.pop()?;
        let left = self.pop()?;

        let left_items = left.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let right_items = right.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        let mut combined = Vec::with_capacity(left_items.len() + right_items.len());
        combined.extend(left_items.iter().cloned());
        combined.extend(right_items.iter().cloned());
        self.push(self.make_sexpr(combined));
        Ok(())
    }

    /// intersection-atom: multiset intersection
    /// Stack: [left, right] -> [intersection]
    fn op_intersection_atom(&mut self) -> VmResult<()> {
        let right = self.pop()?;
        let left = self.pop()?;

        let left_items = left.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let right_items = right.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        // Build count map from right list (structural equality)
        let mut right_remaining: Vec<(V, usize)> = Vec::new();
        for item in right_items {
            let mut found = false;
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                right_remaining.push((item.clone(), 1));
            }
        }

        // Iterate left, emit if found in right (decrementing count)
        let mut result = Vec::new();
        for item in left_items {
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item && entry.1 > 0 {
                    entry.1 -= 1;
                    result.push(item.clone());
                    break;
                }
            }
        }
        self.push(self.make_sexpr(result));
        Ok(())
    }

    /// subtraction-atom: multiset subtraction
    /// Stack: [left, right] -> [difference]
    fn op_subtraction_atom(&mut self) -> VmResult<()> {
        let right = self.pop()?;
        let left = self.pop()?;

        let left_items = left.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let right_items = right.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        // Build count map from right list (structural equality)
        let mut right_remaining: Vec<(V, usize)> = Vec::new();
        for item in right_items {
            let mut found = false;
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                right_remaining.push((item.clone(), 1));
            }
        }

        // Iterate left, skip items found in right (decrementing count)
        let mut result = Vec::new();
        for item in left_items {
            let mut subtracted = false;
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item && entry.1 > 0 {
                    entry.1 -= 1;
                    subtracted = true;
                    break;
                }
            }
            if !subtracted {
                result.push(item.clone());
            }
        }
        self.push(self.make_sexpr(result));
        Ok(())
    }

    /// Alpha-equivalence check for VM values.
    /// Two values are alpha-equivalent if they are structurally identical
    /// except that $-prefixed variables can be consistently renamed.
    fn alpha_equiv(&self, a: &V, b: &V) -> bool {
        use std::collections::HashMap;
        let mut l2r: HashMap<String, String> = HashMap::new();
        let mut r2l: HashMap<String, String> = HashMap::new();
        self.alpha_equiv_inner(a, b, &mut l2r, &mut r2l)
    }

    fn alpha_equiv_inner(
        &self,
        a: &V,
        b: &V,
        l2r: &mut std::collections::HashMap<String, String>,
        r2l: &mut std::collections::HashMap<String, String>,
    ) -> bool {
        // Fast path: structural equality
        if a == b {
            return true;
        }

        // Check atoms (including variables)
        if let (Some(sa), Some(sb)) = (a.as_atom(), b.as_atom()) {
            let a_is_var = sa.starts_with('$');
            let b_is_var = sb.starts_with('$');

            if a_is_var && b_is_var {
                // Both variables: check bidirectional mapping
                match l2r.get(sa) {
                    Some(mapped) => {
                        if mapped != sb {
                            return false;
                        }
                    }
                    None => {
                        l2r.insert(sa.to_string(), sb.to_string());
                    }
                }
                match r2l.get(sb) {
                    Some(mapped) => {
                        if mapped != sa {
                            return false;
                        }
                    }
                    None => {
                        r2l.insert(sb.to_string(), sa.to_string());
                    }
                }
                return true;
            }

            // Non-variable atoms: must be identical
            return sa == sb;
        }

        // S-expressions: recursive check
        if let (Some(items_a), Some(items_b)) = (a.as_sexpr(), b.as_sexpr()) {
            if items_a.len() != items_b.len() {
                return false;
            }
            return items_a
                .iter()
                .zip(items_b.iter())
                .all(|(ia, ib)| self.alpha_equiv_inner(ia, ib, l2r, r2l));
        }

        // Booleans
        if let (Some(ba), Some(bb)) = (a.as_bool(), b.as_bool()) {
            return ba == bb;
        }

        // Numbers
        if let (Some(la), Some(lb)) = (a.as_long(), b.as_long()) {
            return la == lb;
        }
        if let (Some(fa), Some(fb)) = (a.as_float(), b.as_float()) {
            return fa == fb;
        }

        // Strings
        if let (Some(sa), Some(sb)) = (a.as_string(), b.as_string()) {
            return sa == sb;
        }

        // Unit
        if a.is_unit() && b.is_unit() {
            return true;
        }

        false
    }

    // === Nondeterminism Stubs ===

    fn op_fork(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "fork");
        let count = self.read_u16()? as usize;
        if count == 0 {
            // Zero alternatives means immediate failure
            return self.op_fail();
        }

        // Read constant indices from bytecode (compiler emits u16 indices after Fork)
        let mut alternatives = Vec::with_capacity(count);
        for _ in 0..count {
            let const_idx = self.read_u16()?;
            let value = self
                .chunk
                .get_constant(const_idx)
                .ok_or(VmError::InvalidConstant(const_idx))?
                .clone();
            alternatives.push(GenericAlternative::Value(value));
        }

        // Save IP pointing past all constant indices (where execution should resume)
        let resume_ip = self.ip;

        // Create choice point with remaining alternatives
        if alternatives.len() > 1 {
            let cp = GenericChoicePoint {
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
        if let GenericAlternative::Value(v) = &alternatives[0] {
            self.push(v.clone());
        }

        Ok(ControlFlow::Continue(()))
    }

    fn op_fail(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, choice_points = self.choice_points.len(), "fail");
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

            // Try next alternative (remove first, preserving order)
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
                GenericAlternative::Value(v) => self.push(v),
                GenericAlternative::Chunk(chunk) => {
                    self.chunk = chunk;
                    self.ip = 0;
                }
                GenericAlternative::Index(offset) => {
                    // Set IP to indexed offset for computed gotos / rule dispatch
                    self.ip = offset;
                }
                GenericAlternative::RuleMatch { chunk, bindings } => {
                    // Apply bindings
                    for (name, val) in bindings.iter() {
                        self.set_binding(name.to_string(), val.clone());
                    }
                    self.chunk = chunk;
                    self.ip = 0;
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
        trace!(target: "mettatron::vm::nondet", ip = self.ip, results = self.results.len(), "collect");
        let _chunk_index = self.read_u16()?;

        // Collect all results accumulated so far via Yield
        // Filter out Unit values (matches collapse semantics)
        let collected: Vec<V> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !v.is_unit())
            .collect();

        // Push the collected results as a single S-expression
        self.push(self.make_sexpr(collected));
        Ok(())
    }

    /// Collect up to N nondeterministic results.
    /// Stack: [] -> [SExpr of collected results (up to N)]
    fn op_collect_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "collect_n");
        let n = self.read_u8()? as usize;

        // Take up to N results, filtering out Unit values
        let collected: Vec<V> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !v.is_unit())
            .take(n)
            .collect();

        // Push the collected results as a single S-expression
        self.push(self.make_sexpr(collected));
        Ok(())
    }

    fn op_yield(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let value = self.pop()?;
        self.results.push(value);
        // Continue to next alternative
        self.op_fail()
    }

    fn op_begin_nondet(&mut self) {
        // Mark beginning of nondeterministic section
        // Nothing to do in simple implementation
    }

    fn op_end_nondet(&mut self) -> VmResult<()> {
        // End nondeterministic section
        Ok(())
    }

    /// Amb - ambiguous choice from N alternatives on stack.
    /// Creates a choice point with alternatives 2..N and returns alternative 1.
    /// Stack: [alt1, alt2, ..., altN] -> [selected]
    fn op_amb(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "amb");
        let count = self.read_u8()? as usize;

        if count == 0 {
            // Empty amb - push Unit (will fail on subsequent op_fail)
            self.push(self.make_unit());
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
            self.push(alts.into_iter().next().expect("count checked"));
            return Ok(());
        }

        // Create choice point with alternatives 1..N (skipping first)
        let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
            alts[1..].iter().cloned().map(GenericAlternative::Value).collect();

        self.choice_points.push(GenericChoicePoint {
            ip: self.ip, // Resume at current IP for alternatives
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(), // After popping alts
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
        });

        // Push first alternative
        self.push(alts.into_iter().next().expect("count checked"));

        Ok(())
    }

    fn op_guard(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let guard = self.pop()?;
        match guard.as_bool() {
            Some(true) => Ok(ControlFlow::Continue(())),
            Some(false) => self.op_fail(),
            None => Err(VmError::TypeError {
                expected: "Bool",
                got: guard.type_name(),
            }),
        }
    }

    /// Commit - remove N choice points (soft cut).
    /// If count is 0, remove all choice points (like full cut).
    /// Stack: [] -> []
    fn op_commit(&mut self) {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "commit");
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

    // === Advanced Calls ===

    /// Call a native Rust function by ID.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_native(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_native (generic)");
        use super::native_registry::GenericNativeContext;

        let func_id = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for native function
        let env = self
            .env
            .clone()
            .unwrap_or_else(|| GenericEnvironment::new(self.factory.clone()));
        let ctx = GenericNativeContext::new(env, self.factory.clone());

        // Call through registry
        let result = self
            .native_registry
            .call(func_id, &args, &ctx)
            .map_err(|e| VmError::Runtime(e.to_string()))?;

        // Push result(s)
        if result.len() == 1 {
            self.push(result.into_iter().next().expect("result has 1 element"));
        } else if result.is_empty() {
            self.push(self.factory.unit());
        } else {
            // Multiple results - push as S-expression
            self.push(self.factory.sexpr(result));
        }

        Ok(())
    }

    /// Call an external FFI function by name.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_external(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_external (generic)");
        use super::external_registry::{ExternalError, GenericExternalContext};

        let symbol_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get function name from constant pool
        let func_name = self
            .chunk
            .get_constant(symbol_idx)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(symbol_idx))?;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for external function
        let env = self
            .env
            .clone()
            .unwrap_or_else(|| GenericEnvironment::new(self.factory.clone()));
        let ctx = GenericExternalContext::new(env, self.factory.clone());

        // Call through registry
        match self.external_registry.call(&func_name, &args, &ctx) {
            Ok(results) => {
                if results.len() == 1 {
                    self.push(results.into_iter().next().expect("results has 1 element"));
                } else if results.is_empty() {
                    self.push(self.factory.unit());
                } else {
                    self.push(self.factory.sexpr(results));
                }
                Ok(())
            }
            Err(ExternalError::NotFound(_)) => Err(VmError::Runtime(format!(
                "External function '{}' not registered",
                func_name
            ))),
            Err(e) => Err(VmError::Runtime(format!("External call error: {}", e))),
        }
    }

    /// Call a function with memoization.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    ///
    /// Checks the memo cache first. On miss, builds the call expression,
    /// dispatches via environment rules, and caches the result.
    fn op_call_cached(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_cached (generic)");

        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        let head = self
            .chunk
            .get_constant(head_idx)
            .cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;

        // Extract head as string for cache key
        let head_str = head
            .as_atom()
            .map(|s| s.to_string())
            .unwrap_or_else(|| head.type_name().to_string());

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

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args.clone());
        let expr = self.factory.sexpr(items);

        // Dispatch via environment rules (push expr, dispatch pops and pushes result)
        self.push(expr);
        self.op_dispatch_rules()?;

        // Cache the result (peek at top of stack)
        if let Some(result) = self.value_stack.last() {
            self.memo_cache.insert(&head_str, &args, result.clone());
        }

        Ok(())
    }

    // === Environment Operations ===

    fn op_define_rule(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::rules", ip = self.ip, "define_rule (generic)");

        let body = self.pop()?;
        let pattern = self.pop()?;

        // Environment is required for DefineRule
        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime(
                "DefineRule requires environment (use GenericBytecodeVM::with_env)".to_string(),
            )
        })?;

        // Add the rule (lhs=pattern, rhs=body)
        env.add_rule(pattern, body);

        // Push Unit to indicate success
        self.push(self.make_unit());
        Ok(())
    }

    fn op_load_global(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();

        // Try to load from environment bindings
        if let Some(ref env) = self.env {
            if let Some(sym) = name.as_atom() {
                if let Some(value) = env.get_binding(sym) {
                    self.push(value);
                    return Ok(());
                }
            }
        }

        // No binding found - push the atom itself
        self.push(name);
        Ok(())
    }

    fn op_store_global(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = self.chunk.get_constant(index)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(index))?;
        let value = self.pop()?;

        // Store in environment using bind
        if let Some(env) = &mut self.env {
            env.bind(&name, value);
        }
        Ok(())
    }

    fn op_dispatch_rules(&mut self) -> VmResult<()> {
        use crate::backend::eval::bindings_generic::apply_bindings_generic;
        trace!(target: "mettatron::vm::rules", ip = self.ip, "dispatch_rules (generic)");

        // Pop the call expression from the stack
        let expr = self.pop()?;

        // Guard: only dispatch on callable expressions (s-exprs with atom head, or bare atoms)
        if let Some(items) = expr.as_sexpr() {
            if items.is_empty() {
                // Empty expression - return unchanged
                self.push(expr);
                return Ok(());
            }
            if items[0].as_atom().is_none() {
                // Head is not an atom - return expression unchanged
                self.push(expr);
                return Ok(());
            }
        } else if expr.as_atom().is_none() {
            // Not a callable expression - return unchanged
            self.push(expr);
            return Ok(());
        }

        // Get environment reference
        let env = match &self.env {
            Some(e) => e,
            None => {
                // No environment - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }
        };

        // Use native byte-level matching via RuleIndex + extract_data
        let matches = env.match_rules_native(&expr, apply_bindings_generic);

        if matches.is_empty() {
            // No rules match - return expression unchanged
            self.push(expr);
            return Ok(());
        }

        if matches.len() == 1 {
            // Single match - push the instantiated body for further evaluation
            let result = matches.into_iter().next().expect("matches has 1 element");

            // Set up bindings in the current binding frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, value) in result.bindings.iter() {
                    frame.set(name.to_string(), value.clone());
                }
            }

            // Push the instantiated body - caller will continue evaluation
            self.push(result.instantiated_rhs);
            return Ok(());
        }

        // Multiple matches - create choice point for nondeterminism
        // First match executes now, others become alternatives
        let mut alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = Vec::with_capacity(matches.len() - 1);
        let mut first_match = None;

        for result in matches {
            if first_match.is_none() {
                first_match = Some(result);
            } else {
                // Store as alternative value
                alternatives.push(GenericAlternative::Value(result.instantiated_rhs));
            }
        }

        // Create choice point for backtracking to alternatives
        self.choice_points.push(GenericChoicePoint {
            ip: self.ip,
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(),
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
        });

        // Execute first match
        if let Some(result) = first_match {
            // Set up bindings in the current binding frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, value) in result.bindings.iter() {
                    frame.set(name.to_string(), value.clone());
                }
            }

            // Push the instantiated body
            self.push(result.instantiated_rhs);
        }

        Ok(())
    }

    // === Space Operations ===

    /// Add an atom to a space.
    /// Stack: [space, atom] -> [Unit]
    fn op_space_add(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;
        if let Some(handle) = space.as_space() {
            handle.add_atom_generic(&atom);
            self.push(self.make_unit());
            Ok(())
        } else {
            Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            })
        }
    }

    /// Remove an atom from a space.
    /// Stack: [space, atom] -> [Bool]
    fn op_space_remove(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;
        if let Some(handle) = space.as_space() {
            let removed = handle.remove_atom_generic(&atom);
            self.push(self.make_bool(removed));
            Ok(())
        } else {
            Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            })
        }
    }

    /// Get all atoms from a space (collapse).
    /// Stack: [space] -> [SExpr with atoms]
    fn op_space_get_atoms(&mut self) -> VmResult<()> {
        if let Some(env) = &self.env {
            let atoms = env.get_all_atoms();
            self.push(self.make_sexpr(atoms));
        } else {
            self.push(self.make_sexpr(vec![]));
        }
        Ok(())
    }

    /// Match pattern against atoms in a space and instantiate template with bindings.
    ///
    /// Stack: [space, pattern, template] -> [results...]
    fn op_space_match(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        if let Some(handle) = space.as_space() {
            let atoms: Vec<V> = handle.collapse_generic(&self.factory);
            let mut results = Vec::new();

            // Match pattern against each atom and instantiate template
            for atom in &atoms {
                if let Some(bindings) = self.pattern_match_bind_generic(&pattern, atom) {
                    // Substitute bindings into template
                    let instantiated = self.substitute_bindings_generic(&template, &bindings);
                    results.push(instantiated);
                }
            }

            // Return results as S-expression
            self.push(self.make_sexpr(results));
            Ok(())
        } else {
            Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            })
        }
    }

    /// Load a space by name from the constant pool.
    fn op_load_space(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let name = self.chunk.get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?
            .clone();

        if let Some(space_name) = name.as_atom() {
            use xxhash_rust::xxh3::xxh3_64;
            use crate::backend::models::SpaceHandle;
            let handle = SpaceHandle::new(xxh3_64(space_name.as_bytes()), space_name.to_string());
            self.push(self.factory.space(handle));
            Ok(())
        } else {
            Err(VmError::TypeError {
                expected: "Atom (space name)",
                got: name.type_name(),
            })
        }
    }

    /// Substitute variable bindings into a template expression.
    fn substitute_bindings_generic(
        &self,
        template: &V,
        bindings: &[(String, V)],
    ) -> V {
        // Variables are substituted with bound values
        if let Some(name) = template.as_atom() {
            if name.starts_with('$') {
                // Look up the variable in bindings
                if let Some((_, v)) = bindings.iter().find(|(n, _)| n == name) {
                    return v.clone();
                }
            }
            return template.clone();
        }

        // S-expressions are recursively substituted
        if let Some(items) = template.as_sexpr() {
            let substituted: Vec<V> = items
                .iter()
                .map(|item| self.substitute_bindings_generic(item, bindings))
                .collect();
            return self.make_sexpr(substituted);
        }

        // All other values pass through unchanged
        template.clone()
    }

    // === State Operations ===

    fn op_new_state(&mut self) -> VmResult<()> {
        let initial = self.pop()?;

        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime("new-state requires environment".to_string())
        })?;

        let state_id = env.create_state(&initial);
        self.push(self.factory.state(state_id));
        Ok(())
    }

    fn op_get_state(&mut self) -> VmResult<()> {
        let state_ref = self.pop()?;

        if let Some(state_id) = state_ref.as_state() {
            let env = self.env.as_ref().ok_or_else(|| {
                VmError::Runtime("get-state requires environment".to_string())
            })?;

            if let Some(value) = env.get_state(state_id) {
                self.push(value);
                Ok(())
            } else {
                Err(VmError::Runtime(format!("get-state: state {} not found", state_id)))
            }
        } else {
            Err(VmError::TypeError { expected: "State", got: state_ref.type_name() })
        }
    }

    fn op_change_state(&mut self) -> VmResult<()> {
        let new_value = self.pop()?;
        let state_ref = self.pop()?;

        if let Some(state_id) = state_ref.as_state() {
            let env = self.env.as_mut().ok_or_else(|| {
                VmError::Runtime("change-state! requires environment".to_string())
            })?;

            if env.change_state(state_id, &new_value) {
                self.push(self.factory.state(state_id));
                Ok(())
            } else {
                Err(VmError::Runtime(format!("change-state!: state {} not found", state_id)))
            }
        } else {
            Err(VmError::TypeError { expected: "State", got: state_ref.type_name() })
        }
    }

    // === Debug Operations ===

    fn op_breakpoint(&mut self) -> VmResult<()> {
        // Breakpoint - just continue
        Ok(())
    }

    fn op_trace(&mut self) -> VmResult<()> {
        // Trace - print stack top
        if let Ok(value) = self.peek() {
            trace!(target: "mettatron::vm::trace", value = %value.friendly_repr());
        }
        Ok(())
    }

    // === Pattern Matching Helpers ===

    /// Check if pattern matches value using trait methods.
    fn pattern_matches_generic(&self, pattern: &V, value: &V) -> bool {
        // Variable matches anything
        if pattern.is_variable() {
            return true;
        }

        // Wildcard matches anything
        if pattern.as_atom() == Some("_") {
            return true;
        }

        // Check structural equality for non-expressions
        if pattern.as_sexpr().is_none() && value.as_sexpr().is_none() {
            return pattern.structurally_equivalent(value);
        }

        // Match s-expressions recursively
        if let (Some(p_items), Some(v_items)) = (pattern.as_sexpr(), value.as_sexpr()) {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !self.pattern_matches_generic(p, v) {
                    return false;
                }
            }
            return true;
        }

        false
    }

    /// Pattern match with binding extraction.
    fn pattern_match_bind_generic(&self, pattern: &V, value: &V) -> Option<Vec<(String, V)>> {
        let mut bindings = Vec::new();
        if self.pattern_match_bind_recursive(pattern, value, &mut bindings) {
            Some(bindings)
        } else {
            None
        }
    }

    fn pattern_match_bind_recursive(
        &self,
        pattern: &V,
        value: &V,
        bindings: &mut Vec<(String, V)>,
    ) -> bool {
        // Variable captures value
        if pattern.is_variable() {
            if let Some(name) = pattern.as_atom() {
                bindings.push((name.to_string(), value.clone()));
            }
            return true;
        }

        // Wildcard matches but doesn't bind
        if pattern.as_atom() == Some("_") {
            return true;
        }

        // Non-expressions must match exactly
        if pattern.as_sexpr().is_none() && value.as_sexpr().is_none() {
            return pattern.structurally_equivalent(value);
        }

        // Match s-expressions recursively
        if let (Some(p_items), Some(v_items)) = (pattern.as_sexpr(), value.as_sexpr()) {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !self.pattern_match_bind_recursive(p, v, bindings) {
                    return false;
                }
            }
            return true;
        }

        false
    }

    /// Unification with binding extraction.
    fn unify_generic(&self, a: &V, b: &V) -> Option<Vec<(String, V)>> {
        let mut bindings = Vec::new();
        if self.unify_recursive(a, b, &mut bindings) {
            Some(bindings)
        } else {
            None
        }
    }

    fn unify_recursive(&self, a: &V, b: &V, bindings: &mut Vec<(String, V)>) -> bool {
        // Variables unify with anything
        if a.is_variable() {
            if let Some(name) = a.as_atom() {
                bindings.push((name.to_string(), b.clone()));
            }
            return true;
        }
        if b.is_variable() {
            if let Some(name) = b.as_atom() {
                bindings.push((name.to_string(), a.clone()));
            }
            return true;
        }

        // Non-expressions must match
        if a.as_sexpr().is_none() && b.as_sexpr().is_none() {
            return a.structurally_equivalent(b);
        }

        // Unify s-expressions recursively
        if let (Some(a_items), Some(b_items)) = (a.as_sexpr(), b.as_sexpr()) {
            if a_items.len() != b_items.len() {
                return false;
            }
            for (x, y) in a_items.iter().zip(b_items.iter()) {
                if !self.unify_recursive(x, y, bindings) {
                    return false;
                }
            }
            return true;
        }

        false
    }
}

// ============================================================================
// Convenience Constructors (F: Default)
// ============================================================================

impl<V, F> GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + Default + 'static,
{
    /// Create a new VM with the given chunk, using the default factory.
    pub fn new(chunk: Arc<GenericBytecodeChunk<V>>) -> Self {
        Self::with_factory(chunk, F::default())
    }

    /// Create a new VM with custom configuration, using the default factory.
    pub fn with_config(chunk: Arc<GenericBytecodeChunk<V>>, config: VmConfig) -> Self {
        Self::with_config_and_factory(chunk, config, F::default())
    }

    /// Create a new VM with an environment, using the default factory.
    pub fn with_env(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
    ) -> Self {
        Self::with_env_and_factory(chunk, env, F::default())
    }

    /// Push a value onto the results vector (for testing).
    #[cfg(test)]
    pub fn push_result(&mut self, value: V) {
        self.results.push(value);
    }

    /// Get the number of choice points (for testing).
    #[cfg(test)]
    pub fn choice_points_len(&self) -> usize {
        self.choice_points.len()
    }

    /// Get the number of entries in the memo cache (for testing).
    #[cfg(test)]
    pub fn memo_cache_len(&self) -> usize {
        self.memo_cache.len()
    }

    /// Get memo cache statistics (for testing).
    #[cfg(test)]
    pub fn memo_cache_stats(&self) -> super::generic_memo_cache::GenericCacheStats {
        self.memo_cache.stats()
    }
}

// ============================================================================
// Type Aliases
// ============================================================================

use crate::backend::models::GcFactory;

/// The primary bytecode VM type, backed by the global GC slab allocator.
///
/// This is a type alias for `GenericBytecodeVM<MettaValue, GcFactory>`.
/// All existing `BytecodeVM::new(chunk)` call sites continue to work
/// because `GcFactory` implements `Default` (returning `global_factory()`).
pub type BytecodeVM = GenericBytecodeVM<MettaValue, GcFactory>;

