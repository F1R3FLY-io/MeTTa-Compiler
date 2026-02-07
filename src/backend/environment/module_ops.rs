//! Module operations for Environment.
//!
//! Provides methods for module registration, lookup, and management.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use super::generic::GenericEnvironment;
use super::HeapEnvironment;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};
use crate::backend::modules::{LoadOptions, ModId};

// ============================================================================
// Generic Module Operations (for GenericEnvironment<V, F>)
// ============================================================================

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Get the current module path (directory of the executing module).
    pub fn current_module_dir(&self) -> Option<&std::path::Path> {
        self.current_module_path.as_deref()
    }

    /// Set the current module path
    pub fn set_current_module_path(&mut self, path: Option<PathBuf>) {
        self.current_module_path = path;
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
            .set_options(options);
    }

    /// Get the number of loaded modules
    pub fn module_count(&self) -> usize {
        self.shared
            .module_registry
            .read()
            .module_count()
    }

    /// Check if strict mode is enabled
    pub fn is_strict_mode(&self) -> bool {
        self.shared
            .module_registry
            .read()
            .options()
            .strict_mode
    }
}

// ============================================================================
// MettaValue-specific Module Operations
// ============================================================================

impl HeapEnvironment {
    /// Check if a module is cached by path
    pub fn get_module_by_path(&self, path: &std::path::Path) -> Option<ModId> {
        self.shared
            .module_registry
            .read()
            .get_by_path(path)
    }

    /// Check if a module is cached by content hash
    pub fn get_module_by_content(&self, content_hash: u64) -> Option<ModId> {
        self.shared
            .module_registry
            .read()
            .get_by_content(content_hash)
    }

    /// Check if a module is currently being loaded (cycle detection)
    pub fn is_module_loading(&self, content_hash: u64) -> bool {
        self.shared
            .module_registry
            .read()
            .is_loading(content_hash)
    }

    /// Mark a module as being loaded
    pub fn mark_module_loading(&self, content_hash: u64) {
        self.shared
            .module_registry
            .write()
            .mark_loading(content_hash);
    }

    /// Unmark a module as loading
    pub fn unmark_module_loading(&self, content_hash: u64) {
        self.shared
            .module_registry
            .write()
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
            .register(mod_path, file_path, content_hash, resource_dir)
    }

    /// Add a path alias for an existing module
    pub fn add_module_path_alias(&self, path: &std::path::Path, mod_id: ModId) {
        self.shared
            .module_registry
            .write()
            .add_path_alias(path, mod_id);
    }

    /// Get a module's space by its ModId.
    ///
    /// Returns an Arc reference to the module's ModuleSpace for live access.
    /// This is used by `mod-space!` to create live space references.
    pub fn get_module_space(
        &self,
        mod_id: ModId,
    ) -> Option<Arc<RwLock<crate::backend::modules::ModuleSpace>>> {
        // parking_lot::RwLock - no .expect()
        let registry = self.shared.module_registry.read();
        registry.get(mod_id).map(|module| module.space().clone())
    }
}
