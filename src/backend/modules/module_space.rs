//! Module Space with Layered Queries
//!
//! A `ModuleSpace` wraps a module's main space and provides layered queries
//! that also search dependency spaces.

use std::sync::Arc;

use crate::backend::models::MettaValue;
use crate::backend::Environment;

/// A space wrapper that supports layered queries across dependencies.
///
/// When querying a `ModuleSpace`:
/// 1. First, the main space is queried
/// 2. Then, each dependency space is queried in order
/// 3. Results are combined (with main space results first)
///
/// This allows modules to "see" definitions from their imports
/// while maintaining isolation of their own definitions.
pub struct ModuleSpace {
    /// The module's own definitions.
    /// This is a cloned Environment that contains only this module's rules.
    main_space: Option<Environment>,

    /// Dependency spaces (from imported modules).
    /// Queried after main_space, in order of import.
    dep_spaces: Vec<Arc<ModuleSpace>>,

    /// Atoms added directly to this space (not as rules).
    atoms: Vec<MettaValue>,
}

impl ModuleSpace {
    /// Create a new empty module space.
    pub fn new() -> Self {
        Self {
            main_space: None,
            dep_spaces: Vec::new(),
            atoms: Vec::new(),
        }
    }

    /// Create a new module space with an initial environment.
    pub fn with_environment(env: Environment) -> Self {
        Self {
            main_space: Some(env),
            dep_spaces: Vec::new(),
            atoms: Vec::new(),
        }
    }

    /// Set the main space environment.
    pub fn set_main_space(&mut self, env: Environment) {
        self.main_space = Some(env);
    }

    /// Get a reference to the main space environment.
    pub fn main_space(&self) -> Option<&Environment> {
        self.main_space.as_ref()
    }

    /// Get a mutable reference to the main space environment.
    pub fn main_space_mut(&mut self) -> Option<&mut Environment> {
        self.main_space.as_mut()
    }

    /// Add a dependency space (for transitive imports).
    pub fn add_dependency(&mut self, space: Arc<ModuleSpace>) {
        self.dep_spaces.push(space);
    }

    /// Get the number of dependencies.
    pub fn dependency_count(&self) -> usize {
        self.dep_spaces.len()
    }

    /// Add an atom directly to this space.
    pub fn add_atom(&mut self, atom: MettaValue) {
        self.atoms.push(atom);
    }

    /// Remove an atom from this space.
    /// Returns true if the atom was found and removed.
    pub fn remove_atom(&mut self, atom: &MettaValue) -> bool {
        if let Some(pos) = self.atoms.iter().position(|a| a == atom) {
            self.atoms.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get all atoms in the main space (not dependencies).
    pub fn get_atoms_local(&self) -> Vec<MettaValue> {
        self.atoms.clone()
    }

    /// Query the main space only (no dependencies).
    ///
    /// This is useful when you want to check if something is defined
    /// locally in this module, without considering imports.
    pub fn query_local(&self, _pattern: &MettaValue) -> Vec<MettaValue> {
        // For now, just return matching atoms
        // TODO: Integrate with Environment's pattern matching
        self.atoms.clone()
    }

    /// Query the main space and all dependencies.
    ///
    /// Results are returned in order:
    /// 1. Main space results first
    /// 2. Then each dependency's results, in import order
    pub fn query(&self, pattern: &MettaValue) -> Vec<MettaValue> {
        let mut results = self.query_local(pattern);

        // Query each dependency space
        for dep in &self.dep_spaces {
            results.extend(dep.query(pattern));
        }

        results
    }

    /// Check if an atom exists in this space (main only, not dependencies).
    pub fn contains_local(&self, atom: &MettaValue) -> bool {
        self.atoms.contains(atom)
    }

    /// Check if an atom exists in this space or any dependency.
    pub fn contains(&self, atom: &MettaValue) -> bool {
        if self.contains_local(atom) {
            return true;
        }
        for dep in &self.dep_spaces {
            if dep.contains(atom) {
                return true;
            }
        }
        false
    }

    /// Get all atoms from this space and all dependencies.
    pub fn get_all_atoms(&self) -> Vec<MettaValue> {
        let mut atoms = self.atoms.clone();
        for dep in &self.dep_spaces {
            atoms.extend(dep.get_all_atoms());
        }
        atoms
    }

    /// Clear all atoms from the main space (not dependencies).
    pub fn clear(&mut self) {
        self.atoms.clear();
    }
}

impl Default for ModuleSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ModuleSpace {
    fn clone(&self) -> Self {
        Self {
            main_space: self.main_space.clone(),
            dep_spaces: self.dep_spaces.clone(),
            atoms: self.atoms.clone(),
        }
    }
}

impl std::fmt::Debug for ModuleSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleSpace")
            .field("has_main_space", &self.main_space.is_some())
            .field("dep_count", &self.dep_spaces.len())
            .field("atom_count", &self.atoms.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_module_space() {
        let space = ModuleSpace::new();
        assert!(space.main_space().is_none());
        assert_eq!(space.dependency_count(), 0);
        assert!(space.get_atoms_local().is_empty());
    }

    #[test]
    fn test_add_and_remove_atoms() {
        let mut space = ModuleSpace::new();

        let atom1 = MettaValue::Atom("foo".to_string());
        let atom2 = MettaValue::Atom("bar".to_string());

        space.add_atom(atom1.clone());
        space.add_atom(atom2.clone());

        assert!(space.contains_local(&atom1));
        assert!(space.contains_local(&atom2));
        assert_eq!(space.get_atoms_local().len(), 2);

        assert!(space.remove_atom(&atom1));
        assert!(!space.contains_local(&atom1));
        assert!(space.contains_local(&atom2));

        // Removing non-existent atom returns false
        assert!(!space.remove_atom(&atom1));
    }

    #[test]
    fn test_dependency_layering() {
        let mut space1 = ModuleSpace::new();
        space1.add_atom(MettaValue::Atom("from_space1".to_string()));

        let mut space2 = ModuleSpace::new();
        space2.add_atom(MettaValue::Atom("from_space2".to_string()));

        let mut main_space = ModuleSpace::new();
        main_space.add_atom(MettaValue::Atom("from_main".to_string()));
        main_space.add_dependency(Arc::new(space1));
        main_space.add_dependency(Arc::new(space2));

        assert_eq!(main_space.dependency_count(), 2);

        // Query should return atoms from main + all deps
        let all_atoms = main_space.get_all_atoms();
        assert_eq!(all_atoms.len(), 3);

        // Check contains across dependencies
        assert!(main_space.contains(&MettaValue::Atom("from_main".to_string())));
        assert!(main_space.contains(&MettaValue::Atom("from_space1".to_string())));
        assert!(main_space.contains(&MettaValue::Atom("from_space2".to_string())));

        // Local should only see main
        assert!(main_space.contains_local(&MettaValue::Atom("from_main".to_string())));
        assert!(!main_space.contains_local(&MettaValue::Atom("from_space1".to_string())));
    }

    #[test]
    fn test_clear() {
        let mut space = ModuleSpace::new();
        space.add_atom(MettaValue::Atom("foo".to_string()));
        space.add_atom(MettaValue::Atom("bar".to_string()));

        assert_eq!(space.get_atoms_local().len(), 2);
        space.clear();
        assert!(space.get_atoms_local().is_empty());
    }
}
