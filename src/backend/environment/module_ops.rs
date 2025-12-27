//! Module operations for Environment.
//!
//! Provides methods for module registration, lookup, and management.

use std::path::PathBuf;
use std::sync::RwLock;

use super::Environment;
use crate::backend::modules::{LoadOptions, ModId};

impl Environment {
    /// Get the current module path (directory of the executing module)
    pub fn current_module_dir(&self) -> Option<&std::path::Path> {
        self.current_module_path.as_deref()
    }

    /// Set the current module path
    pub fn set_current_module_path(&mut self, path: Option<PathBuf>) {
        self.current_module_path = path;
    }

    /// Check if a module is cached by path
    pub fn get_module_by_path(&self, path: &std::path::Path) -> Option<ModId> {
        self.shared
            .module_registry
            .read()
            .expect("module_registry lock poisoned")
            .get_by_path(path)
    }

    /// Check if a module is cached by content hash
    pub fn get_module_by_content(&self, content_hash: u64) -> Option<ModId> {
        self.shared
            .module_registry
            .read()
            .expect("module_registry lock poisoned")
            .get_by_content(content_hash)
    }

    /// Check if a module is currently being loaded (cycle detection)
    pub fn is_module_loading(&self, content_hash: u64) -> bool {
        self.shared
            .module_registry
            .read()
            .expect("module_registry lock poisoned")
            .is_loading(content_hash)
    }

    /// Mark a module as being loaded
    pub fn mark_module_loading(&self, content_hash: u64) {
        self.shared
            .module_registry
            .write()
            .expect("module_registry lock poisoned")
            .mark_loading(content_hash);
    }

    /// Unmark a module as loading
    pub fn unmark_module_loading(&self, content_hash: u64) {
        self.shared
            .module_registry
            .write()
            .expect("module_registry lock poisoned")
            .unmark_loading(content_hash);
    }

    /// Register a new module in the registry
    pub fn register_module(
        &self,
        mod_path: String,
        file_path: &std::path::Path,
        content_hash: u64,
        resource_dir: Option<PathBuf>,
    ) -> ModId {
        self.shared
            .module_registry
            .write()
            .expect("module_registry lock poisoned")
            .register(mod_path, file_path, content_hash, resource_dir)
    }

    /// Add a path alias for an existing module
    pub fn add_module_path_alias(&self, path: &std::path::Path, mod_id: ModId) {
        self.shared
            .module_registry
            .write()
            .expect("module_registry lock poisoned")
            .add_path_alias(path, mod_id);
    }

    /// Get the number of loaded modules
    pub fn module_count(&self) -> usize {
        self.shared
            .module_registry
            .read()
            .expect("module_registry lock poisoned")
            .module_count()
    }

    /// Get a module's space by its ModId.
    ///
    /// Returns an Arc reference to the module's ModuleSpace for live access.
    /// This is used by `mod-space!` to create live space references.
    pub fn get_module_space(
        &self,
        mod_id: ModId,
    ) -> Option<std::sync::Arc<RwLock<crate::backend::modules::ModuleSpace>>> {
        let registry = self
            .shared
            .module_registry
            .read()
            .expect("module_registry lock poisoned");
        registry.get(mod_id).map(|module| module.space().clone())
    }

    /// Get the current module's space as a SpaceHandle ("&self" reference).
    ///
    /// Returns a SpaceHandle for the current module's space, or a new empty
    /// space if not currently inside a module evaluation.
    ///
    /// This is used to implement the `&self` token for match and space operations.
    pub fn self_space(&self) -> crate::backend::models::SpaceHandle {
        use crate::backend::models::SpaceHandle;

        // If we're inside a module, return its space
        if let Some(mod_path) = &self.current_module_path {
            if let Some(mod_id) = self.get_module_by_path(mod_path) {
                if let Some(space) = self.get_module_space(mod_id) {
                    return SpaceHandle::for_module(mod_id, "self".to_string(), space);
                }
            }
        }

        // Fallback: return the "self" named space if it exists, otherwise create empty
        // Use ID 0 for the global "self" space
        SpaceHandle::new(0, "self".to_string())
    }

    /// Check if strict mode is enabled
    pub fn is_strict_mode(&self) -> bool {
        self.shared
            .module_registry
            .read()
            .expect("module_registry lock poisoned")
            .options()
            .strict_mode
    }

    /// Enable or disable strict mode.
    ///
    /// When enabled:
    /// - Only submodules can be imported
    /// - Transitive imports are disabled
    /// - Cyclic imports are disallowed
    ///
    /// When disabled: HE-compatible permissive mode
    pub fn set_strict_mode(&mut self, strict: bool) {
        self.make_owned();
        let options = if strict {
            LoadOptions::strict()
        } else {
            LoadOptions::permissive()
        };
        self.shared
            .module_registry
            .write()
            .expect("module_registry lock poisoned")
            .set_options(options);
    }
}
