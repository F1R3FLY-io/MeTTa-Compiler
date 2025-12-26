//! Advanced call operations for the bytecode VM.
//!
//! This module contains methods for advanced call operations:
//! - CallNative: Call native Rust functions
//! - CallExternal: Call external FFI functions
//! - CallCached: Call with memoization

use std::sync::Arc;
use tracing::trace;

use crate::backend::models::MettaValue;
use crate::backend::Environment;
use super::types::{VmError, VmResult};
use super::BytecodeVM;
use crate::backend::bytecode::native_registry::NativeContext;
use crate::backend::bytecode::external_registry::ExternalContext;

impl BytecodeVM {
    // === Advanced Calls ===

    /// Call a native Rust function by ID.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    pub(super) fn op_call_native(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_native");
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
            self.push(result.into_iter().next().expect("result has 1 element"));
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
    pub(super) fn op_call_external(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_external");
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
                    self.push(results.into_iter().next().expect("results has 1 element"));
                } else {
                    self.push(MettaValue::SExpr(results));
                }
                Ok(())
            }
            Err(crate::backend::bytecode::external_registry::ExternalError::NotFound(_)) => {
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
    pub(super) fn op_call_cached(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_cached");
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
}
