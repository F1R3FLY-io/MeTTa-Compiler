//! State operations for the bytecode VM.
//!
//! This module contains methods for mutable state operations:
//! - NewState: Create a new mutable state cell
//! - GetState: Get the current value from a state cell
//! - ChangeState: Change the value in a state cell

use super::types::{VmError, VmResult};
use super::BytecodeVM;
use crate::backend::models::MettaValue;

impl BytecodeVM {
    // === State Operations ===

    /// Create a new mutable state cell.
    /// Stack: [initial_value] -> [State(id)]
    pub(super) fn op_new_state(&mut self) -> VmResult<()> {
        let initial_value = self.pop()?;

        let env = self
            .env
            .as_mut()
            .ok_or_else(|| VmError::Runtime("new-state requires environment".to_string()))?;

        let state_id = env.create_state(initial_value);
        self.push(MettaValue::State(state_id));
        Ok(())
    }

    /// Get the current value from a state cell.
    /// Stack: [State(id)] -> [value]
    pub(super) fn op_get_state(&mut self) -> VmResult<()> {
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
                    Err(VmError::Runtime(format!(
                        "get-state: state {} not found",
                        state_id
                    )))
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
    pub(super) fn op_change_state(&mut self) -> VmResult<()> {
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
                    Err(VmError::Runtime(format!(
                        "change-state!: state {} not found",
                        state_id
                    )))
                }
            }
            other => Err(VmError::TypeError {
                expected: "State",
                got: other.type_name(),
            }),
        }
    }
}
