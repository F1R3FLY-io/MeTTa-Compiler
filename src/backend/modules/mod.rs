//! Module System Infrastructure
//!
//! This module provides the core types and utilities for MeTTaTron's module system:
//! - `ModId` - Unique module identifier
//! - `MettaMod` - A loaded module with its own space and tokenizer
//! - `ModuleSpace` - Layered space with dependency queries
//! - `Tokenizer` - Per-module token registration
//! - `ModuleDescriptor` - Caching descriptor with path UID and content hash
//! - `ModuleRegistry` - Registry for tracking and caching loaded modules
//! - `LoadOptions` - Configuration for module loading behavior
//! - `PackageInfo` - Package manifest (metta.toml) parsing and version constraints
//! - Path resolution utilities for `self:` and `top:` notation

mod cache;
mod loader;
mod metta_mod;
mod module_space;
mod package;
mod path;
mod tokenizer;

pub use cache::{hash_content, hash_path, ModuleDescriptor};
pub use loader::{
    new_shared_registry, LoadError, LoadOptions, LoadResult, ModuleRegistry, SharedModuleRegistry,
};
pub use metta_mod::{MettaMod, ModId, ModuleState};
pub use module_space::ModuleSpace;
pub use package::{Dependency, DependencyDetail, ExportConfig, PackageInfo, PackageMeta};
pub use path::{is_submodule, normalize_module_path, parent_module_path, resolve_module_path};
pub use tokenizer::Tokenizer;
