//! Nondeterminism operations for the bytecode VM.
//!
//! This module contains methods for nondeterministic operations
//! like fork, fail, cut, collect, yield, amb, and guard.

use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::trace;

use super::types::{Alternative, ChoicePoint, VmError, VmResult};
use super::BytecodeVM;
use crate::backend::models::MettaValue;

impl BytecodeVM {
    // === Nondeterminism Operations ===

    pub(super) fn op_fork(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "fork");
        let count = self.read_u16()? as usize;
        if count == 0 {
            return self.op_fail().map(|_| ());
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

    pub(super) fn op_fail(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
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

    pub(super) fn op_cut(&mut self) {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, choice_points = self.choice_points.len(), "cut");
        // Remove all choice points
        self.choice_points.clear();
    }

    /// Collect all nondeterministic results from current evaluation.
    /// The chunk_index parameter is reserved for future use (sub-chunk execution).
    /// Currently, this collects all results accumulated via Yield and returns them as SExpr.
    ///
    /// Stack: [] -> [SExpr of collected results]
    pub(super) fn op_collect(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, results = self.results.len(), "collect");
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
    pub(super) fn op_collect_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "collect_n");
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

    pub(super) fn op_yield(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "yield");
        // Save current result and backtrack for more
        let value = self.pop()?;
        self.results.push(value);
        self.op_fail()
    }

    pub(super) fn op_begin_nondet(&mut self) {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "begin_nondet");
        // Mark start of nondeterministic section
        // Could save state for potential rollback
    }

    pub(super) fn op_end_nondet(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "end_nondet");
        // End nondeterministic section
        Ok(())
    }

    /// Guard - backtrack if top of stack is false.
    /// Stack: [bool] -> []
    pub(super) fn op_guard(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "guard");
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
    pub(super) fn op_commit(&mut self) {
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

    /// Amb - ambiguous choice from N alternatives on stack.
    /// Creates a choice point with alternatives 2..N and returns alternative 1.
    /// Stack: [alt1, alt2, ..., altN] -> [selected]
    pub(super) fn op_amb(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "amb");
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
            self.push(alts.into_iter().next().expect("count checked"));
            return Ok(());
        }

        // Create choice point with alternatives 1..N (skipping first)
        // Wrap remaining alternatives in Alternative::Value
        let alternatives: Vec<Alternative> =
            alts[1..].iter().cloned().map(Alternative::Value).collect();

        self.choice_points.push(ChoicePoint {
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
}
