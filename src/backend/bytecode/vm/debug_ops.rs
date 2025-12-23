//! Debug operations for the bytecode VM.
//!
//! This module contains methods for debugging operations:
//! - Breakpoint: Set a breakpoint
//! - Trace: Trace execution

use tracing::{debug, trace};

use super::types::VmResult;
use super::BytecodeVM;

impl BytecodeVM {
    // === Debug Operations ===

    /// Handle a breakpoint instruction.
    /// Currently just logs the breakpoint for debugging.
    pub(super) fn op_breakpoint(&mut self) -> VmResult<()> {
        debug!(target: "mettatron::vm::breakpoint", ip = self.ip);
        Ok(())
    }

    /// Trace the current value on top of stack.
    /// Logs the value without modifying the stack.
    pub(super) fn op_trace(&mut self) -> VmResult<()> {
        let value = self.peek()?;
        trace!(target: "mettatron::vm::trace", ?value);
        Ok(())
    }
}
