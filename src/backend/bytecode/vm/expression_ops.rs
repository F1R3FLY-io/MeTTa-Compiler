//! Expression and pattern matching operations for the bytecode VM.
//!
//! This module contains methods for expression manipulation and pattern matching
//! including get_head, get_tail, decon_atom, cons_atom, and higher-order operations.

use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::backend::models::MettaValue;
use crate::backend::bytecode::opcodes::Opcode;
use super::pattern::{pattern_matches, pattern_match_bind, unify};
use super::types::{VmError, VmResult};
use super::BytecodeVM;

impl BytecodeVM {
    // === Pattern Matching Operations ===

    pub(super) fn op_match(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match");
        let value = self.pop()?;
        let pattern = self.pop()?;
        let matches = pattern_matches(&pattern, &value);
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    pub(super) fn op_match_bind(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_bind");
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
            debug!(target: "mettatron::vm::match", ip = self.ip, "match_bind failed");
            self.push(MettaValue::Bool(false));
        }
        Ok(())
    }

    pub(super) fn op_match_head(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_head");
        let _expected_index = self.read_u8()?;
        // TODO: Implement head matching
        Err(VmError::Runtime("MatchHead not yet implemented".into()))
    }

    pub(super) fn op_match_arity(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_arity");
        let expected_arity = self.read_u8()? as usize;
        let value = self.pop()?;
        let matches = match &value {
            MettaValue::SExpr(items) => items.len() == expected_arity,
            _ => false,
        };
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    pub(super) fn op_match_guard(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_guard");
        let _guard_index = self.read_u16()?;
        // TODO: Implement guard evaluation
        Err(VmError::Runtime("MatchGuard not yet implemented".into()))
    }

    pub(super) fn op_unify(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "unify");
        let b = self.pop()?;
        let a = self.pop()?;
        let unifies = unify(&a, &b).is_some();
        self.push(MettaValue::Bool(unifies));
        Ok(())
    }

    pub(super) fn op_unify_bind(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "unify_bind");
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
            debug!(target: "mettatron::vm::match", ip = self.ip, "unify_bind failed");
            self.push(MettaValue::Bool(false));
        }
        Ok(())
    }

    // === Type Check Operations ===

    pub(super) fn op_is_variable(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_var = value.is_variable();
        self.push(MettaValue::Bool(is_var));
        Ok(())
    }

    pub(super) fn op_is_sexpr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_sexpr = matches!(&value, MettaValue::SExpr(_));
        self.push(MettaValue::Bool(is_sexpr));
        Ok(())
    }

    pub(super) fn op_is_symbol(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_sym = matches!(&value, MettaValue::Atom(_));
        self.push(MettaValue::Bool(is_sym));
        Ok(())
    }

    // === Expression Introspection ===

    pub(super) fn op_get_head(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) if !items.is_empty() => {
                self.push(items[0].clone());
            }
            _ => return Err(VmError::TypeError { expected: "non-empty S-expression", got: "other" }),
        }
        Ok(())
    }

    pub(super) fn op_get_tail(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) if !items.is_empty() => {
                self.push(MettaValue::sexpr(items[1..].to_vec()));
            }
            _ => return Err(VmError::TypeError { expected: "non-empty S-expression", got: "other" }),
        }
        Ok(())
    }

    pub(super) fn op_get_arity(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value {
            MettaValue::SExpr(items) => {
                self.push(MettaValue::Long(items.len() as i64));
            }
            _ => return Err(VmError::TypeError { expected: "S-expression", got: "other" }),
        }
        Ok(())
    }

    pub(super) fn op_get_element(&mut self) -> VmResult<()> {
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

    pub(super) fn op_decon_atom(&mut self) -> VmResult<()> {
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

    pub(super) fn op_repr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let repr_str = self.atom_repr(&value);
        self.push(MettaValue::String(repr_str));
        Ok(())
    }

    /// cons-atom: prepend head to tail S-expression
    /// Matches tree-visitor semantics in list_ops.rs:118-126
    pub(super) fn op_cons_atom(&mut self) -> VmResult<()> {
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

    pub(super) fn atom_repr(&self, value: &MettaValue) -> String {
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

    pub(super) fn op_get_metatype(&mut self) -> VmResult<()> {
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

    // === Higher-Order Operations ===

    pub(super) fn op_map_atom(&mut self) -> VmResult<()> {
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

    pub(super) fn op_filter_atom(&mut self) -> VmResult<()> {
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

    pub(super) fn op_foldl_atom(&mut self) -> VmResult<()> {
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

    // === Expression Manipulation Operations (PR #63) ===

    pub(super) fn op_index_atom(&mut self) -> VmResult<()> {
        let index = self.pop()?;
        let expr = self.pop()?;

        let idx = match index {
            MettaValue::Long(i) => i,
            _ => return Err(VmError::TypeError { expected: "Long (index)", got: "other" }),
        };

        let result = match expr {
            MettaValue::SExpr(items) => {
                if idx < 0 || idx as usize >= items.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: items.len(),
                    });
                }
                items[idx as usize].clone()
            }
            _ => return Err(VmError::TypeError { expected: "S-expression", got: "other" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_min_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr {
            MettaValue::SExpr(items) => items,
            _ => return Err(VmError::TypeError { expected: "S-expression", got: "other" }),
        };

        if items.is_empty() {
            return Err(VmError::TypeError { expected: "non-empty S-expression", got: "empty expression" });
        }

        // Find minimum among numeric values
        let mut min_val: Option<f64> = None;
        let mut min_is_long = true;

        for item in &items {
            let val = match item {
                MettaValue::Long(x) => *x as f64,
                MettaValue::Float(x) => {
                    min_is_long = false;
                    *x
                }
                _ => continue, // Skip non-numeric values
            };
            min_val = Some(min_val.map_or(val, |m: f64| m.min(val)));
        }

        let result = match min_val {
            Some(v) if min_is_long && v == (v as i64) as f64 => MettaValue::Long(v as i64),
            Some(v) => MettaValue::Float(v),
            None => return Err(VmError::TypeError { expected: "numeric values in expression", got: "no numeric values" }),
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_max_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr {
            MettaValue::SExpr(items) => items,
            _ => return Err(VmError::TypeError { expected: "S-expression", got: "other" }),
        };

        if items.is_empty() {
            return Err(VmError::TypeError { expected: "non-empty S-expression", got: "empty expression" });
        }

        // Find maximum among numeric values
        let mut max_val: Option<f64> = None;
        let mut max_is_long = true;

        for item in &items {
            let val = match item {
                MettaValue::Long(x) => *x as f64,
                MettaValue::Float(x) => {
                    max_is_long = false;
                    *x
                }
                _ => continue, // Skip non-numeric values
            };
            max_val = Some(max_val.map_or(val, |m: f64| m.max(val)));
        }

        let result = match max_val {
            Some(v) if max_is_long && v == (v as i64) as f64 => MettaValue::Long(v as i64),
            Some(v) => MettaValue::Float(v),
            None => return Err(VmError::TypeError { expected: "numeric values in expression", got: "no numeric values" }),
        };
        self.push(result);
        Ok(())
    }

    // === Template Execution Helpers ===

    /// Execute a template chunk with a single bound value (for map/filter)
    pub(super) fn execute_template_with_binding(&mut self, chunk: Arc<crate::backend::bytecode::chunk::BytecodeChunk>, binding: MettaValue) -> VmResult<MettaValue> {
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
    pub(super) fn execute_foldl_template(&mut self, chunk: Arc<crate::backend::bytecode::chunk::BytecodeChunk>, acc: MettaValue, item: MettaValue) -> VmResult<MettaValue> {
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
}
