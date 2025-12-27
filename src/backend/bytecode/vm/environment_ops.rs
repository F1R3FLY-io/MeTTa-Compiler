//! Environment operations for the bytecode VM.
//!
//! This module contains methods for environment-based operations:
//! - DefineRule: Define a new rule in the environment
//! - LoadGlobal: Load a global value
//! - StoreGlobal: Store a global value
//! - DispatchRules: Dispatch rules using the environment

use std::sync::Arc;
use tracing::trace;

use super::pattern::pattern_match_bind;
use super::types::{Alternative, ChoicePoint, VmError, VmResult};
use super::BytecodeVM;
use crate::backend::models::MettaValue;

impl BytecodeVM {
    // === Environment Operations ===

    /// Define a new rule in the environment.
    ///
    /// This opcode requires an environment to be set via `with_env()`.
    /// Stack: [pattern, body] -> [Unit]
    ///
    /// The rule `(= pattern body)` is added to the environment for later
    /// pattern matching during rule dispatch.
    pub(super) fn op_define_rule(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::rules", ip = self.ip, "define_rule");
        use crate::backend::models::Rule;

        let body = self.pop()?;
        let pattern = self.pop()?;

        // Environment is required for DefineRule
        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime(
                "DefineRule requires environment (use BytecodeVM::with_env)".to_string(),
            )
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
    pub(super) fn op_load_global(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let name = self
            .chunk
            .get_constant(const_idx)
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
    pub(super) fn op_store_global(&mut self) -> VmResult<()> {
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
    pub(super) fn op_dispatch_rules(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::rules", ip = self.ip, "dispatch_rules");
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
            let (body, bindings) = matches.into_iter().next().expect("matches has 1 element");

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
}
