//! Two-Pass Module Loader
//!
//! Implements module loading with:
//! - Two-pass loading: Index symbols before evaluation (handles cyclic deps)
//! - Hybrid caching: Path UID + content hash deduplication
//! - Cycle detection: ModuleState tracking (Loading/Loaded)
//!
//! # Two-Pass Loading Strategy
//!
//! **Pass 1 (Index):**
//! - Parse module source
//! - Extract rule definitions `(= lhs rhs)` and type declarations `(: sym type)`
//! - Register symbols in environment (without evaluating RHS)
//! - Mark module as `Loading`
//!
//! **Pass 2 (Evaluate):**
//! - Evaluate all expressions (including rule RHS and nested imports)
//! - Forward references now resolve (symbols indexed in Pass 1)
//! - Mark module as `Loaded`
//!
//! # Cycle Detection
//!
//! When a module is encountered that's already in `Loading` state:
//! - Symbols are already indexed, so forward references work
//! - Return early without re-evaluating (prevents infinite loops)
//! - The cycle is "broken" at the second encounter

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use super::cache::{hash_content, hash_path, ModuleDescriptor};
use super::metta_mod::{MettaMod, ModId, ModuleState};

/// Result type for module loading operations.
pub type LoadResult<T> = Result<T, LoadError>;

/// Errors that can occur during module loading.
#[derive(Debug, Clone)]
pub enum LoadError {
    /// File could not be read.
    FileNotFound(PathBuf, String),
    /// File could not be parsed.
    ParseError(PathBuf, String),
    /// Import constraint violated (e.g., non-submodule import in strict mode).
    ImportConstraint(String),
    /// Module evaluation failed.
    EvalError(String),
    /// Circular import detected (informational, not necessarily an error).
    CircularImport(PathBuf),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::FileNotFound(path, err) => {
                write!(f, "Failed to read '{}': {}", path.display(), err)
            }
            LoadError::ParseError(path, err) => {
                write!(f, "Failed to parse '{}': {}", path.display(), err)
            }
            LoadError::ImportConstraint(msg) => write!(f, "Import constraint: {}", msg),
            LoadError::EvalError(msg) => write!(f, "Evaluation error: {}", msg),
            LoadError::CircularImport(path) => {
                write!(f, "Circular import detected: {}", path.display())
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Module loading options.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Allow imports from non-submodules (permissive mode).
    /// Default: false (strict mode, only submodules allowed).
    pub permissive_imports: bool,

    /// Enable transitive imports (import dependencies of imported modules).
    /// Default: true (HE-compatible).
    pub transitive_imports: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self::strict()
    }
}

impl LoadOptions {
    /// Create options for strict mode (HE-compatible defaults).
    pub fn strict() -> Self {
        Self {
            permissive_imports: false,
            transitive_imports: true,
        }
    }

    /// Create options for permissive mode.
    pub fn permissive() -> Self {
        Self {
            permissive_imports: true,
            transitive_imports: true,
        }
    }
}

/// Module registry for tracking loaded modules.
///
/// This struct manages:
/// - Loaded module storage
/// - Path UID → ModId mapping (fast lookup by path)
/// - Content hash → ModId mapping (deduplication)
/// - Loading state tracking (cycle detection)
pub struct ModuleRegistry {
    /// All loaded modules by ID.
    modules: Vec<MettaMod>,

    /// Path UID → ModId (primary lookup by path).
    path_to_module: HashMap<u64, ModId>,

    /// Content hash → ModId (deduplication across paths).
    content_to_module: HashMap<u64, ModId>,

    /// Content hashes currently being loaded (cycle detection).
    loading_modules: HashSet<u64>,

    /// Counter for generating unique ModIds.
    next_mod_id: u64,

    /// Loading options.
    options: LoadOptions,
}

impl ModuleRegistry {
    /// Create a new empty module registry.
    pub fn new() -> Self {
        Self::with_options(LoadOptions::default())
    }

    /// Create a registry with specific options.
    pub fn with_options(options: LoadOptions) -> Self {
        Self {
            modules: Vec::new(),
            path_to_module: HashMap::new(),
            content_to_module: HashMap::new(),
            loading_modules: HashSet::new(),
            next_mod_id: 0,
            options,
        }
    }

    /// Get loading options.
    pub fn options(&self) -> &LoadOptions {
        &self.options
    }

    /// Set loading options.
    pub fn set_options(&mut self, options: LoadOptions) {
        self.options = options;
    }

    /// Generate a new unique ModId.
    fn next_id(&mut self) -> ModId {
        let id = ModId::new(self.next_mod_id);
        self.next_mod_id += 1;
        id
    }

    /// Check if a module is cached by path.
    pub fn get_by_path(&self, path: &Path) -> Option<ModId> {
        let uid = hash_path(path);
        self.path_to_module.get(&uid).copied()
    }

    /// Check if a module is cached by content hash.
    pub fn get_by_content(&self, content_hash: u64) -> Option<ModId> {
        self.content_to_module.get(&content_hash).copied()
    }

    /// Check if a module is currently being loaded (cycle detection).
    pub fn is_loading(&self, content_hash: u64) -> bool {
        self.loading_modules.contains(&content_hash)
    }

    /// Mark a module as being loaded (start of Pass 1).
    pub fn mark_loading(&mut self, content_hash: u64) {
        self.loading_modules.insert(content_hash);
    }

    /// Unmark a module as loading (end of Pass 2).
    pub fn unmark_loading(&mut self, content_hash: u64) {
        self.loading_modules.remove(&content_hash);
    }

    /// Get a module by ID.
    pub fn get(&self, id: ModId) -> Option<&MettaMod> {
        self.modules.get(id.value() as usize)
    }

    /// Get a mutable reference to a module by ID.
    pub fn get_mut(&mut self, id: ModId) -> Option<&mut MettaMod> {
        self.modules.get_mut(id.value() as usize)
    }

    /// Register a new module.
    ///
    /// # Arguments
    /// - `mod_path` - Hierarchical module path (e.g., "top:mylib:utils")
    /// - `file_path` - Filesystem path to the module
    /// - `content_hash` - Hash of the module's content
    /// - `resource_dir` - Optional directory for module resources
    ///
    /// # Returns
    /// The new ModId assigned to this module.
    pub fn register(
        &mut self,
        mod_path: String,
        file_path: &Path,
        content_hash: u64,
        resource_dir: Option<PathBuf>,
    ) -> ModId {
        let id = self.next_id();
        let module = MettaMod::new(id, mod_path, content_hash, resource_dir);

        // Store the module
        self.modules.push(module);

        // Update caches
        let path_uid = hash_path(file_path);
        self.path_to_module.insert(path_uid, id);
        self.content_to_module.insert(content_hash, id);

        id
    }

    /// Create a path alias for an existing module.
    ///
    /// Used when the same content is loaded from a different path.
    pub fn add_path_alias(&mut self, path: &Path, mod_id: ModId) {
        let path_uid = hash_path(path);
        self.path_to_module.insert(path_uid, mod_id);
    }

    /// Get the number of loaded modules.
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Iterate over all loaded modules.
    pub fn iter(&self) -> impl Iterator<Item = &MettaMod> {
        self.modules.iter()
    }

    /// Check if an import is allowed (submodule constraint).
    ///
    /// In strict mode, only submodules can be imported.
    /// In permissive mode, any module can be imported.
    pub fn validate_import(
        &self,
        current_module: &str,
        target_module: &str,
    ) -> Result<(), LoadError> {
        if self.options.permissive_imports {
            return Ok(());
        }

        // Check if target is a submodule of current
        if target_module.starts_with(&format!("{}:", current_module)) {
            Ok(())
        } else if target_module == current_module {
            Err(LoadError::ImportConstraint(format!(
                "Cannot import module '{}' from within itself",
                current_module
            )))
        } else {
            Err(LoadError::ImportConstraint(format!(
                "Module '{}' cannot import '{}': only submodules allowed. \
                 Use --permissive-imports to allow.",
                current_module, target_module
            )))
        }
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ModuleRegistry {
    fn clone(&self) -> Self {
        Self {
            modules: self.modules.clone(),
            path_to_module: self.path_to_module.clone(),
            content_to_module: self.content_to_module.clone(),
            loading_modules: self.loading_modules.clone(),
            next_mod_id: self.next_mod_id,
            options: self.options.clone(),
        }
    }
}

impl std::fmt::Debug for ModuleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleRegistry")
            .field("module_count", &self.modules.len())
            .field("loading_count", &self.loading_modules.len())
            .field("permissive", &self.options.permissive_imports)
            .finish()
    }
}

/// Thread-safe wrapper for ModuleRegistry.
pub type SharedModuleRegistry = Arc<RwLock<ModuleRegistry>>;

/// Create a new shared module registry.
pub fn new_shared_registry() -> SharedModuleRegistry {
    Arc::new(RwLock::new(ModuleRegistry::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = ModuleRegistry::new();
        assert_eq!(registry.module_count(), 0);
        assert!(!registry.options().permissive_imports);
        assert!(registry.options().transitive_imports);
    }

    #[test]
    fn test_register_module() {
        let mut registry = ModuleRegistry::new();
        let path = PathBuf::from("/test/module.metta");
        let content_hash = hash_content("(= (foo) bar)");

        let id = registry.register(
            "top:test:module".to_string(),
            &path,
            content_hash,
            None,
        );

        assert_eq!(id, ModId::new(0));
        assert_eq!(registry.module_count(), 1);

        // Should be cached by path
        assert_eq!(registry.get_by_path(&path), Some(id));

        // Should be cached by content
        assert_eq!(registry.get_by_content(content_hash), Some(id));
    }

    #[test]
    fn test_content_deduplication() {
        let mut registry = ModuleRegistry::new();
        let path1 = PathBuf::from("/path/a/module.metta");
        let path2 = PathBuf::from("/path/b/module.metta");
        let content_hash = hash_content("(= (foo) bar)");

        // Register first path
        let id1 = registry.register(
            "top:a:module".to_string(),
            &path1,
            content_hash,
            None,
        );

        // Check content is already loaded
        assert_eq!(registry.get_by_content(content_hash), Some(id1));

        // Add alias for second path (same content)
        registry.add_path_alias(&path2, id1);

        // Both paths should resolve to same module
        assert_eq!(registry.get_by_path(&path1), Some(id1));
        assert_eq!(registry.get_by_path(&path2), Some(id1));

        // Only one module exists
        assert_eq!(registry.module_count(), 1);
    }

    #[test]
    fn test_loading_state() {
        let mut registry = ModuleRegistry::new();
        let content_hash = hash_content("module content");

        assert!(!registry.is_loading(content_hash));

        registry.mark_loading(content_hash);
        assert!(registry.is_loading(content_hash));

        registry.unmark_loading(content_hash);
        assert!(!registry.is_loading(content_hash));
    }

    #[test]
    fn test_import_validation_strict() {
        let registry = ModuleRegistry::new();

        // Submodule import allowed
        assert!(registry
            .validate_import("top:parent", "top:parent:child")
            .is_ok());

        // Deep submodule allowed
        assert!(registry
            .validate_import("top:parent", "top:parent:child:grandchild")
            .is_ok());

        // Self-import not allowed
        assert!(registry
            .validate_import("top:parent", "top:parent")
            .is_err());

        // Non-submodule not allowed
        assert!(registry
            .validate_import("top:parent", "top:other")
            .is_err());

        // Parent not allowed
        assert!(registry
            .validate_import("top:parent:child", "top:parent")
            .is_err());
    }

    #[test]
    fn test_import_validation_permissive() {
        let registry = ModuleRegistry::with_options(LoadOptions::permissive());

        // Everything allowed in permissive mode
        assert!(registry
            .validate_import("top:parent", "top:other")
            .is_ok());
        assert!(registry
            .validate_import("top:a", "top:b:c:d")
            .is_ok());
    }

    #[test]
    fn test_get_module() {
        let mut registry = ModuleRegistry::new();
        let path = PathBuf::from("/test/module.metta");
        let content_hash = hash_content("content");

        let id = registry.register(
            "top:test".to_string(),
            &path,
            content_hash,
            None,
        );

        let module = registry.get(id).unwrap();
        assert_eq!(module.path(), "top:test");
        assert_eq!(module.state(), ModuleState::Loading); // Default state
    }

    #[test]
    fn test_multiple_modules() {
        let mut registry = ModuleRegistry::new();

        let id1 = registry.register(
            "top:module1".to_string(),
            &PathBuf::from("/path1.metta"),
            hash_content("content1"),
            None,
        );

        let id2 = registry.register(
            "top:module2".to_string(),
            &PathBuf::from("/path2.metta"),
            hash_content("content2"),
            None,
        );

        let id3 = registry.register(
            "top:module3".to_string(),
            &PathBuf::from("/path3.metta"),
            hash_content("content3"),
            None,
        );

        assert_eq!(registry.module_count(), 3);
        assert_eq!(id1, ModId::new(0));
        assert_eq!(id2, ModId::new(1));
        assert_eq!(id3, ModId::new(2));

        // All modules accessible
        assert!(registry.get(id1).is_some());
        assert!(registry.get(id2).is_some());
        assert!(registry.get(id3).is_some());
    }

    #[test]
    fn test_shared_registry() {
        let registry = new_shared_registry();

        {
            let mut reg = registry.write().unwrap();
            reg.register(
                "top:test".to_string(),
                &PathBuf::from("/test.metta"),
                12345,
                None,
            );
        }

        {
            let reg = registry.read().unwrap();
            assert_eq!(reg.module_count(), 1);
        }
    }

    #[test]
    fn test_load_error_display() {
        let err = LoadError::FileNotFound(
            PathBuf::from("/test.metta"),
            "No such file".to_string(),
        );
        assert!(err.to_string().contains("/test.metta"));
        assert!(err.to_string().contains("No such file"));

        let err = LoadError::ParseError(
            PathBuf::from("/test.metta"),
            "Syntax error".to_string(),
        );
        assert!(err.to_string().contains("Syntax error"));

        let err = LoadError::ImportConstraint("Not a submodule".to_string());
        assert!(err.to_string().contains("Not a submodule"));
    }
}
