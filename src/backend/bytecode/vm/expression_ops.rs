//! Expression and pattern matching operations for the bytecode VM.
//!
//! This module contains methods for expression manipulation and pattern matching
//! including get_head, get_tail, decon_atom, cons_atom, and higher-order operations.

use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::{debug, trace};

use super::pattern::{pattern_match_bind, pattern_matches, unify};
use super::types::{VmError, VmResult};
use super::BytecodeVM;
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::models::{MettaValue, MettaValueInner};

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

    /// Match head symbol of an S-expression for fast dispatch optimization.
    ///
    /// Reads an expected symbol index from the bytecode, pops a value from the stack,
    /// and checks if the value is an S-expression whose first element matches the
    /// expected symbol. Pushes Bool(true) if it matches, Bool(false) otherwise.
    ///
    /// Stack: [value] -> [bool]
    /// Bytecode: MatchHead expected_index:u8
    pub(super) fn op_match_head(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_head");
        let expected_index = self.read_u8()? as u16;

        // Get expected symbol from constant pool and clone it to avoid borrow issues
        let expected = self
            .chunk
            .get_constant(expected_index)
            .ok_or(VmError::InvalidConstant(expected_index))?
            .clone();

        let value = self.pop()?;

        // Check if value is an S-expression with matching head
        let matches = match (expected.inner(), value.inner()) {
            (MettaValueInner::Atom(exp_sym), MettaValueInner::SExpr(items)) if !items.is_empty() => {
                matches!(items[0].inner(), MettaValueInner::Atom(head_sym) if head_sym == exp_sym)
            }
            _ => false,
        };

        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    pub(super) fn op_match_arity(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_arity");
        let expected_arity = self.read_u8()? as usize;
        let value = self.pop()?;
        let matches = match value.inner() {
            MettaValueInner::SExpr(items) => items.len() == expected_arity,
            _ => false,
        };
        self.push(MettaValue::Bool(matches));
        Ok(())
    }

    /// Evaluate a guard expression and backtrack if it returns false.
    ///
    /// Reads a chunk index from the bytecode, executes the guard chunk,
    /// and if the result is Bool(false), triggers a backtrack via op_fail().
    /// If the result is Bool(true), execution continues normally.
    ///
    /// Stack: [] -> [] (guard success) or backtrack (guard failure)
    /// Bytecode: MatchGuard guard_chunk_index:u16
    pub(super) fn op_match_guard(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::match", ip = self.ip, "match_guard");
        let guard_index = self.read_u16()?;

        // Get the guard chunk from the sub-chunk pool
        let guard_chunk = self
            .chunk
            .get_chunk_constant(guard_index)
            .ok_or(VmError::InvalidConstant(guard_index))?;

        // Execute the guard chunk and get result
        // We use a dummy binding value since the guard should use bindings from the current frame
        let result = self.execute_template_with_binding(guard_chunk, MettaValue::Unit())?;

        // Check if guard passed
        match result.inner() {
            MettaValueInner::Bool(true) => {
                // Guard passed, continue execution
                Ok(())
            }
            MettaValueInner::Bool(false) => {
                // Guard failed, backtrack
                // We need to trigger a fail, but op_fail returns ControlFlow
                // Instead, we return an error that will be caught and converted to backtracking
                Err(VmError::GuardFailed)
            }
            _ => {
                // Guard returned non-boolean, treat as failure
                debug!(target: "mettatron::vm::match", ip = self.ip, "match_guard returned non-boolean: {:?}", result);
                Err(VmError::GuardFailed)
            }
        }
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
        let is_sexpr = matches!(value.inner(), MettaValueInner::SExpr(_));
        self.push(MettaValue::Bool(is_sexpr));
        Ok(())
    }

    pub(super) fn op_is_symbol(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_sym = matches!(value.inner(), MettaValueInner::Atom(_));
        self.push(MettaValue::Bool(is_sym));
        Ok(())
    }

    // === Expression Introspection ===

    pub(super) fn op_get_head(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value.inner() {
            MettaValueInner::SExpr(items) if !items.is_empty() => {
                self.push(items[0].clone());
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "non-empty S-expression",
                    got: "other",
                })
            }
        }
        Ok(())
    }

    pub(super) fn op_get_tail(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value.inner() {
            MettaValueInner::SExpr(items) if !items.is_empty() => {
                self.push(MettaValue::sexpr(items[1..].to_vec()));
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "non-empty S-expression",
                    got: "other",
                })
            }
        }
        Ok(())
    }

    pub(super) fn op_get_arity(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value.inner() {
            MettaValueInner::SExpr(items) => {
                self.push(MettaValue::Long(items.len() as i64));
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                })
            }
        }
        Ok(())
    }

    pub(super) fn op_get_element(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        let value = self.pop()?;
        match value.inner() {
            MettaValueInner::SExpr(items) if index < items.len() => {
                self.push(items[index].clone());
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "S-expression with valid index",
                    got: "other",
                })
            }
        }
        Ok(())
    }

    pub(super) fn op_decon_atom(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        match value.inner() {
            MettaValueInner::SExpr(items) if !items.is_empty() => {
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

        let result = match tail.inner() {
            MettaValueInner::SExpr(elements) => {
                // Prepend head to existing S-expression
                let mut new_elements = Vec::with_capacity(elements.len() + 1);
                new_elements.push(head);
                new_elements.extend(elements.iter().cloned());
                MettaValue::SExpr(new_elements)
            }
            MettaValueInner::Nil => {
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
        match value.inner() {
            MettaValueInner::Long(n) => n.to_string(),
            MettaValueInner::Float(f) => f.to_string(),
            MettaValueInner::Bool(b) => {
                if *b {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            MettaValueInner::String(s) => format!("\"{}\"", s),
            MettaValueInner::Atom(a) => a.clone(),
            MettaValueInner::SExpr(items) => {
                let inner: Vec<String> = items.iter().map(|v| self.atom_repr(v)).collect();
                format!("({})", inner.join(" "))
            }
            MettaValueInner::Unit => "()".to_string(),
            MettaValueInner::Nil => "Nil".to_string(),
            MettaValueInner::Error(msg, _) => format!("(Error {})", msg),
            MettaValueInner::Type(t) => format!("(: {})", self.atom_repr(t)),
            MettaValueInner::Space(_) => "<space>".to_string(),
            MettaValueInner::State(_) => "<state>".to_string(),
            MettaValueInner::Conjunction(items) => {
                let inner: Vec<String> = items.iter().map(|v| self.atom_repr(v)).collect();
                format!("[{}]", inner.join(" "))
            }
            MettaValueInner::Memo(_) => "<memo>".to_string(),
            MettaValueInner::Empty => "Empty".to_string(),
        }
    }

    pub(super) fn op_get_metatype(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let metatype = match value.inner() {
            MettaValueInner::SExpr(_) => "Expression",
            MettaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            MettaValueInner::Atom(_) => "Symbol",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::Long(_) => "Number",
            MettaValueInner::Float(_) => "Number",
            MettaValueInner::String(_) => "String",
            MettaValueInner::Nil => "Nil",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
        };
        self.push(MettaValue::sym(metatype));
        Ok(())
    }

    // === Higher-Order Operations ===

    pub(super) fn op_map_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = match list.inner() {
            MettaValueInner::SExpr(items) => items.clone(),
            _ => {
                return Err(VmError::TypeError {
                    expected: "list/S-expression",
                    got: "other",
                })
            }
        };

        let template_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
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

        let items = match list.inner() {
            MettaValueInner::SExpr(items) => items.clone(),
            _ => {
                return Err(VmError::TypeError {
                    expected: "list/S-expression",
                    got: "other",
                })
            }
        };

        let predicate_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;
        let mut results = Vec::new();

        for item in items {
            let result =
                self.execute_template_with_binding(Arc::clone(&predicate_chunk), item.clone())?;
            // Check if predicate returned true
            if matches!(result.inner(), MettaValueInner::Bool(true)) {
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

        let items = match list.inner() {
            MettaValueInner::SExpr(items) => items.clone(),
            _ => {
                return Err(VmError::TypeError {
                    expected: "list/S-expression",
                    got: "other",
                })
            }
        };

        let op_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
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

        let idx = match index.inner() {
            MettaValueInner::Long(i) => *i,
            _ => {
                return Err(VmError::TypeError {
                    expected: "Long (index)",
                    got: "other",
                })
            }
        };

        let result = match expr.inner() {
            MettaValueInner::SExpr(items) => {
                if idx < 0 || idx as usize >= items.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: items.len(),
                    });
                }
                items[idx as usize].clone()
            }
            _ => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_min_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr.inner() {
            MettaValueInner::SExpr(items) => items,
            _ => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                })
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
            let val = match item.inner() {
                MettaValueInner::Long(x) => *x as f64,
                MettaValueInner::Float(x) => {
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
            None => {
                return Err(VmError::TypeError {
                    expected: "numeric values in expression",
                    got: "no numeric values",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    pub(super) fn op_max_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr.inner() {
            MettaValueInner::SExpr(items) => items,
            _ => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                })
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
            let val = match item.inner() {
                MettaValueInner::Long(x) => *x as f64,
                MettaValueInner::Float(x) => {
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
            None => {
                return Err(VmError::TypeError {
                    expected: "numeric values in expression",
                    got: "no numeric values",
                })
            }
        };
        self.push(result);
        Ok(())
    }

    // === Template Execution Helpers ===

    /// Execute a template chunk with a single bound value (for map/filter)
    pub(super) fn execute_template_with_binding(
        &mut self,
        chunk: Arc<crate::backend::bytecode::chunk::BytecodeChunk>,
        binding: MettaValue,
    ) -> VmResult<MettaValue> {
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

            // Execute one step
            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    // Restore and return first result
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or(MettaValue::Unit()));
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
        let result = self.pop().unwrap_or(MettaValue::Unit());

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
    pub(super) fn execute_foldl_template(
        &mut self,
        chunk: Arc<crate::backend::bytecode::chunk::BytecodeChunk>,
        acc: MettaValue,
        item: MettaValue,
    ) -> VmResult<MettaValue> {
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
                    return Ok(results.into_iter().next().unwrap_or(MettaValue::Unit()));
                }
                Err(e) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Err(e);
                }
            }
        }

        let result = self.pop().unwrap_or(MettaValue::Unit());
        self.ip = saved_ip;
        self.chunk = saved_chunk;
        while self.value_stack.len() > saved_stack_base {
            let _ = self.pop();
        }

        Ok(result)
    }
}
