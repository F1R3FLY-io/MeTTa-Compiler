//! Compilation context for tracking local variables and scopes.

use std::collections::HashMap;

use super::error::{CompileError, CompileResult};

/// Upvalue reference
#[derive(Debug, Clone)]
pub struct Upvalue {
    /// Index in parent's locals or upvalues
    pub index: u16,
    /// True if capturing from parent's locals, false if from parent's upvalues
    pub is_local: bool,
}

/// Compilation context for tracking local variables and scopes
#[derive(Debug, Clone)]
pub struct CompileContext {
    /// Local variable names to slot indices
    locals: HashMap<String, u16>,
    /// Stack of scope depths for local variables
    scope_depths: Vec<u16>,
    /// Current scope depth
    current_scope: u16,
    /// Next available local slot
    next_local: u16,
    /// Parent context for nested functions
    parent: Option<Box<CompileContext>>,
    /// Captured variables (upvalues)
    upvalues: Vec<Upvalue>,
}

impl Default for CompileContext {
    fn default() -> Self {
        Self::new()
    }
}

impl CompileContext {
    /// Create a new root context
    pub fn new() -> Self {
        Self {
            locals: HashMap::new(),
            scope_depths: Vec::new(),
            current_scope: 0,
            next_local: 0,
            parent: None,
            upvalues: Vec::new(),
        }
    }

    /// Create a child context for nested functions
    pub fn child(parent: CompileContext) -> Self {
        Self {
            locals: HashMap::new(),
            scope_depths: Vec::new(),
            current_scope: 0,
            next_local: 0,
            parent: Some(Box::new(parent)),
            upvalues: Vec::new(),
        }
    }

    /// Begin a new scope
    pub fn begin_scope(&mut self) {
        self.current_scope += 1;
    }

    /// End current scope, returns number of locals to pop
    pub fn end_scope(&mut self) -> u16 {
        let mut count = 0;
        let scope = self.current_scope;

        // Remove locals from this scope
        self.locals.retain(|_, slot| {
            if self.scope_depths.get(*slot as usize).copied() == Some(scope) {
                count += 1;
                false
            } else {
                true
            }
        });

        // Trim scope_depths
        while self.scope_depths.last().copied() == Some(scope) {
            self.scope_depths.pop();
        }

        self.current_scope -= 1;
        count
    }

    /// Declare a local variable, returns its slot index
    pub fn declare_local(&mut self, name: String) -> CompileResult<u16> {
        if self.next_local >= u16::MAX {
            return Err(CompileError::TooManyLocals);
        }

        let slot = self.next_local;
        self.next_local += 1;
        self.locals.insert(name, slot);
        self.scope_depths.push(self.current_scope);
        Ok(slot)
    }

    /// Resolve a local variable, returns slot index if found
    pub fn resolve_local(&self, name: &str) -> Option<u16> {
        self.locals.get(name).copied()
    }

    /// Resolve an upvalue (captured variable)
    pub fn resolve_upvalue(&mut self, name: &str) -> Option<u16> {
        // Check parent's locals first
        if let Some(parent) = &self.parent {
            if let Some(local_idx) = parent.resolve_local(name) {
                // Add as upvalue capturing from parent's local
                return Some(self.add_upvalue(local_idx, true));
            }
        }

        // Check parent's upvalues
        if let Some(parent) = &mut self.parent {
            if let Some(upvalue_idx) = parent.resolve_upvalue(name) {
                // Add as upvalue capturing from parent's upvalue
                return Some(self.add_upvalue(upvalue_idx, false));
            }
        }

        None
    }

    /// Add an upvalue, returns its index
    fn add_upvalue(&mut self, index: u16, is_local: bool) -> u16 {
        // Check if already captured
        for (i, upvalue) in self.upvalues.iter().enumerate() {
            if upvalue.index == index && upvalue.is_local == is_local {
                return i as u16;
            }
        }

        let idx = self.upvalues.len() as u16;
        self.upvalues.push(Upvalue { index, is_local });
        idx
    }

    /// Get the number of locals
    pub fn local_count(&self) -> u16 {
        self.next_local
    }

    /// Get the number of upvalues
    pub fn upvalue_count(&self) -> u16 {
        self.upvalues.len() as u16
    }
}
