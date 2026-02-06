//! Space operations for the bytecode VM.
//!
//! This module contains methods for space-related operations:
//! - SpaceAdd: Add an atom to a space
//! - SpaceRemove: Remove an atom from a space
//! - SpaceGetAtoms: Get all atoms from a space
//! - SpaceMatch: Match pattern against atoms in a space
//! - LoadSpace: Load a space by name

use xxhash_rust::xxh3::xxh3_64;

use super::pattern::{pattern_match_bind, pattern_matches};
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

    /// Match pattern against atoms in a space and instantiate template with bindings.
    ///
    /// Stack: [space, pattern, template] -> [results...]
    ///
    /// For each atom in the space that matches the pattern:
    /// 1. Extract variable bindings from the pattern match
    /// 2. Substitute bindings into the template
    /// 3. Add the instantiated template to results
    ///
    /// This is the full implementation with template instantiation and binding extraction.
    pub(super) fn op_space_match(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        match space.inner() {
            MettaValueInner::Space(handle) => {
                let atoms = handle.collapse();
                let mut results = Vec::new();

                // Match pattern against each atom and instantiate template
                for atom in &atoms {
                    if let Some(bindings) = pattern_match_bind(&pattern, atom) {
                        // Substitute bindings into template
                        let instantiated = self.substitute_bindings(&template, &bindings);
                        results.push(instantiated);
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

    /// Substitute variable bindings into a template expression.
    ///
    /// Replaces all variable references (atoms starting with '$') in the template
    /// with their corresponding values from the bindings.
    fn substitute_bindings(
        &self,
        template: &MettaValue,
        bindings: &[(String, MettaValue)],
    ) -> MettaValue {
        match template.inner() {
            // Variables are substituted with bound values
            MettaValueInner::Atom(name) if name.starts_with('$') => {
                // Look up the variable in bindings
                bindings
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| template.clone())
            }
            // S-expressions are recursively substituted
            MettaValueInner::SExpr(items) => {
                let substituted: Vec<MettaValue> = items
                    .iter()
                    .map(|item| self.substitute_bindings(item, bindings))
                    .collect();
                MettaValue::SExpr(substituted)
            }
            // All other values pass through unchanged
            _ => template.clone(),
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
                let handle = SpaceHandle::new(xxh3_64(space_name.as_bytes()), space_name.clone());
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
