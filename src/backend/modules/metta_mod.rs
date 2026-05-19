//! MeTTa Module Structure
//!
//! A `MettaMod` represents a loaded MeTTa module with its own isolated space,
//! tokenizer, and dependency tracking.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use super::module_space::ModuleSpace;
use super::tokenizer::Tokenizer;

/// Unique identifier for a loaded module.
///
/// ModIds are assigned sequentially as modules are loaded.
/// They are used for:
/// - Deduplication (prevent importing the same module twice)
/// - Dependency tracking (which modules depend on which)
/// - Fast lookups in the module registry
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModId(pub u64);

impl ModId {
    /// Create a new ModId with the given value.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw u64 value.
    pub fn value(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ModId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModId({})", self.0)
    }
}

/// The state of a module during loading.
///
/// Used for two-pass loading and cycle detection.
/// Stored as AtomicU8 for lock-free access: 0 = Loading, 1 = Loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ModuleState {
    /// Pass 1 complete, Pass 2 in progress.
    /// Symbols are indexed but not yet evaluated.
    /// Encountering a Loading module during import indicates a cycle.
    Loading = 0,

    /// Fully loaded and evaluated.
    /// Safe to import all definitions.
    Loaded = 1,
}

/// A loaded MeTTa module with its own space and tokenizer.
///
/// Each module has:
/// - An isolated space for its definitions (wrapped in ModuleSpace for layered queries)
/// - A tokenizer for dynamic token bindings (`bind!`)
/// - A list of imported dependencies (for transitive imports)
/// - Optional resource directory for module assets
pub struct MettaMod {
    /// Unique identifier for this module.
    id: ModId,

    /// Hierarchical module path (e.g., "top:parent:child").
    mod_path: String,

    /// The module's isolated space with dependency layering.
    space: Arc<RwLock<ModuleSpace>>,

    /// Per-module tokenizer for token bindings.
    tokenizer: Arc<RwLock<Tokenizer>>,

    /// Imported dependency module IDs.
    /// Used for transitive import tracking and deduplication.
    imported_deps: Arc<RwLock<HashMap<ModId, ()>>>,

    /// Resource directory for module assets.
    resource_dir: Option<PathBuf>,

    /// Current loading state (lock-free AtomicU8: 0 = Loading, 1 = Loaded).
    state: AtomicU8,

    /// Content hash for deduplication.
    /// Modules with the same content hash are considered identical.
    content_hash: u64,
}

impl MettaMod {
    /// Create a new module.
    ///
    /// # Arguments
    /// - `id` - Unique module identifier
    /// - `mod_path` - Hierarchical path (e.g., "top:mylib:utils")
    /// - `content_hash` - Hash of the module's source content
    /// - `resource_dir` - Optional directory for module resources
    pub fn new(
        id: ModId,
        mod_path: String,
        content_hash: u64,
        resource_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            id,
            mod_path,
            space: Arc::new(RwLock::new(ModuleSpace::new())),
            tokenizer: Arc::new(RwLock::new(Tokenizer::new())),
            imported_deps: Arc::new(RwLock::new(HashMap::new())),
            resource_dir,
            state: AtomicU8::new(ModuleState::Loading as u8),
            content_hash,
        }
    }

    /// Get the module's unique ID.
    pub fn id(&self) -> ModId {
        self.id
    }

    /// Get the module's hierarchical path.
    pub fn path(&self) -> &str {
        &self.mod_path
    }

    /// Get the module's name (last component of path).
    pub fn name(&self) -> &str {
        self.mod_path.rsplit(':').next().unwrap_or(&self.mod_path)
    }

    /// Get a reference to the module's space.
    pub fn space(&self) -> &Arc<RwLock<ModuleSpace>> {
        &self.space
    }

    /// Get a mutable reference to the module's space Arc.
    ///
    /// Used by `corelib::load_corelib()` to install a pre-populated ModuleSpace
    /// (containing the populated `MettaEnvironment` with corelib rules).
    pub fn space_mut(&mut self) -> &mut Arc<RwLock<ModuleSpace>> {
        &mut self.space
    }

    /// Get a reference to the module's tokenizer.
    pub fn tokenizer(&self) -> &Arc<RwLock<Tokenizer>> {
        &self.tokenizer
    }

    /// Get the module's resource directory.
    pub fn resource_dir(&self) -> Option<&PathBuf> {
        self.resource_dir.as_ref()
    }

    /// Get the module's current state (lock-free atomic read).
    pub fn state(&self) -> ModuleState {
        match self.state.load(Ordering::Acquire) {
            0 => ModuleState::Loading,
            _ => ModuleState::Loaded,
        }
    }

    /// Set the module's state (lock-free atomic write).
    pub fn set_state(&self, state: ModuleState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// Get the module's content hash.
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// Check if a dependency has been imported.
    pub fn has_imported(&self, mod_id: ModId) -> bool {
        self.imported_deps.read().contains_key(&mod_id)
    }

    /// Mark a dependency as imported.
    pub fn mark_imported(&self, mod_id: ModId) {
        self.imported_deps.write().insert(mod_id, ());
    }

    /// Get all imported dependency IDs.
    pub fn imported_dep_ids(&self) -> Vec<ModId> {
        self.imported_deps.read().keys().copied().collect()
    }

    /// Get the number of imported dependencies.
    pub fn imported_dep_count(&self) -> usize {
        self.imported_deps.read().len()
    }
}

// Concrete-type rule lookup for corelib/dependency module chaining.
// Per Phase 5 (2026-05-19): user envs hold an `Option<Arc<MettaMod>>` reference
// to the corelib MettaMod. After a user env's `match_rules_native_inner` collects
// its own matches, it consults this module's rule_index via `lookup_rules` for
// HE-equivalent stdlib-helper visibility (per [[bisim-baseline-2026-05-19]] and
// the corelib architecture in `docs/CORELIB_ARCHITECTURE.md`).
//
// Mirrors HE's `ModuleSpace::query` which walks `main + deps` while `visit`
// (used by `get-atoms`) walks only `main`.
impl MettaMod {
    /// Look up rules in this module's main_space rule_index for the given expression.
    ///
    /// Returns all matching `RuleMatchResult`s; empty if no main_space env is set
    /// or no rules match. Delegates to the underlying `MettaEnvironment::match_rules_native`.
    ///
    /// The `apply_bindings` closure is taken as `&dyn Fn` (type-erased) rather than
    /// `impl Fn` to break a monomorphization recursion: callers of `lookup_rules`
    /// are themselves inside generic `match_rules_native<F>`, and a generic closure
    /// parameter would cause `lookup_rules<F>` to monomorphize anew per caller,
    /// which then re-instantiates `match_rules_native<G>` etc. forever. The
    /// `&dyn Fn` signature is a single function-pointer type, terminating recursion.
    pub fn lookup_rules(
        &self,
        expr: &crate::backend::models::MettaValue,
        apply_bindings: &dyn Fn(
            &crate::backend::models::MettaValue,
            &crate::backend::models::GenericBindings<crate::backend::models::MettaValue>,
            &crate::backend::models::GcFactory,
        ) -> crate::backend::models::MettaValue,
        outer_carrying: &crate::backend::models::GenericBindings<
            crate::backend::models::MettaValue,
        >,
    ) -> Vec<
        crate::backend::environment::rule_management::RuleMatchResult<
            crate::backend::models::MettaValue,
        >,
    > {
        let space = self.space.read();
        match space.main_space() {
            Some(env) => env.match_rules_native(expr, apply_bindings, outer_carrying),
            None => Vec::new(),
        }
    }
}

impl Clone for MettaMod {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            mod_path: self.mod_path.clone(),
            space: Arc::clone(&self.space),
            tokenizer: Arc::clone(&self.tokenizer),
            imported_deps: Arc::clone(&self.imported_deps),
            resource_dir: self.resource_dir.clone(),
            state: AtomicU8::new(self.state.load(Ordering::Acquire)),
            content_hash: self.content_hash,
        }
    }
}

impl std::fmt::Debug for MettaMod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MettaMod")
            .field("id", &self.id)
            .field("mod_path", &self.mod_path)
            .field("state", &self.state())
            .field("content_hash", &format!("{:016x}", self.content_hash))
            .field("imported_deps", &self.imported_dep_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mod_id() {
        let id1 = ModId::new(1);
        let id2 = ModId::new(1);
        let id3 = ModId::new(2);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_eq!(id1.value(), 1);
    }

    #[test]
    fn test_metta_mod_creation() {
        let module = MettaMod::new(
            ModId::new(1),
            "top:mylib:utils".to_string(),
            0x123456789abcdef0,
            None,
        );

        assert_eq!(module.id(), ModId::new(1));
        assert_eq!(module.path(), "top:mylib:utils");
        assert_eq!(module.name(), "utils");
        assert_eq!(module.state(), ModuleState::Loading);
        assert_eq!(module.content_hash(), 0x123456789abcdef0);
    }

    #[test]
    fn test_module_state_transitions() {
        let module = MettaMod::new(ModId::new(1), "top:test".to_string(), 0, None);

        assert_eq!(module.state(), ModuleState::Loading);
        module.set_state(ModuleState::Loaded);
        assert_eq!(module.state(), ModuleState::Loaded);
    }

    #[test]
    fn test_dependency_tracking() {
        let module = MettaMod::new(ModId::new(1), "top:test".to_string(), 0, None);

        let dep1 = ModId::new(2);
        let dep2 = ModId::new(3);

        assert!(!module.has_imported(dep1));
        module.mark_imported(dep1);
        assert!(module.has_imported(dep1));
        assert!(!module.has_imported(dep2));

        module.mark_imported(dep2);
        assert_eq!(module.imported_dep_count(), 2);

        let deps = module.imported_dep_ids();
        assert!(deps.contains(&dep1));
        assert!(deps.contains(&dep2));
    }
}
