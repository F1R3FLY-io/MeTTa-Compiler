//! Space operations for the bytecode VM.
//!
//! This module contains methods for space-related operations:
//! - SpaceAdd: Add an atom to a space
//! - SpaceRemove: Remove an atom from a space
//! - SpaceGetAtoms: Get all atoms from a space
//! - SpaceMatch: Match pattern against atoms in a space
//! - LoadSpace: Load a space by name

use std::hash::{Hash, Hasher};

use super::pattern::pattern_matches;
use super::types::{VmError, VmResult};
use super::BytecodeVM;
use crate::backend::models::{MettaValue, MettaValueInner, SpaceHandle};

impl BytecodeVM {
    // === Space Operations ===

    /// Add an atom to a space.
    /// Stack: [space, atom] -> [Unit]
    pub(super) fn op_space_add(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;
        match space.inner() {
            MettaValueInner::Space(handle) => {
                handle.add_atom(atom);
                self.push(MettaValue::Unit());
                Ok(())
            }
            _ => Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            }),
        }
    }

    /// Remove an atom from a space.
    /// Stack: [space, atom] -> [Bool]
    pub(super) fn op_space_remove(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;
        match space.inner() {
            MettaValueInner::Space(handle) => {
                let removed = handle.remove_atom(&atom);
                self.push(MettaValue::Bool(removed));
                Ok(())
            }
            _ => Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            }),
        }
    }

    /// Get all atoms from a space (collapse).
    /// Stack: [space] -> [SExpr with atoms]
    pub(super) fn op_space_get_atoms(&mut self) -> VmResult<()> {
        let space = self.pop()?;
        match space.inner() {
            MettaValueInner::Space(handle) => {
                let atoms = handle.collapse();
                // Return as an S-expression list
                self.push(MettaValue::SExpr(atoms));
                Ok(())
            }
            _ => Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            }),
        }
    }

    /// Match pattern against atoms in a space.
    /// Stack: [space, pattern, template] -> [results...]
    ///
    /// Note: This is a simplified implementation that doesn't support
    /// full nondeterministic matching with template evaluation yet.
    /// For now, it returns matched atoms without template instantiation.
    pub(super) fn op_space_match(&mut self) -> VmResult<()> {
        // TODO: Full implementation requires recursive evaluation
        // For now, use simplified matching that returns atoms without templates
        let _template = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        match space.inner() {
            MettaValueInner::Space(handle) => {
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
            _ => Err(VmError::TypeError {
                expected: "Space",
                got: space.type_name(),
            }),
        }
    }

    /// Load a space by name from the constant pool.
    /// This operation reads a constant index for the space name.
    ///
    /// Note: Currently limited - full implementation needs Environment access.
    pub(super) fn op_load_space(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let name = self
            .chunk
            .get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?
            .clone();

        match name.inner() {
            MettaValueInner::Atom(space_name) => {
                // Create a placeholder space with the given name
                // In full integration, this would lookup from Environment
                let handle = SpaceHandle::new(
                    std::hash::BuildHasher::build_hasher(
                        &std::collections::hash_map::RandomState::new(),
                    )
                    .finish(),
                    space_name.clone(),
                );
                self.push(MettaValue::Space(handle));
                Ok(())
            }
            _ => Err(VmError::TypeError {
                expected: "Atom (space name)",
                got: name.type_name(),
            }),
        }
    }
}
