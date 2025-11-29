//! Module Path Resolution
//!
//! Supports three path notations:
//! - `self:child` - Relative to current module directory
//! - `top:absolute:path` - Absolute from workspace root
//! - `bare_name` - Treated as `self:bare_name` or file path

use std::path::{Path, PathBuf};

/// Resolve a module path to a filesystem path.
///
/// # Path Notation
/// - `self:child` - Relative to `current_dir`
/// - `self:child:grandchild` - Nested relative path
/// - `top:absolute:path` - Absolute from workspace root
/// - `bare_name` - Relative to `current_dir` (same as `self:bare_name`)
/// - `"path/to/file.metta"` - Direct file path (string literal)
///
/// # Arguments
/// - `path` - The module path string (using `:` as separator)
/// - `current_dir` - The directory of the currently-executing module (for relative paths)
///
/// # Returns
/// The resolved filesystem path with `.metta` extension added if needed.
pub fn resolve_module_path(path: &str, current_dir: Option<&Path>) -> PathBuf {
    // Handle direct file paths (already contain / or .metta)
    if path.contains('/') || path.ends_with(".metta") {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            return p;
        }
        // Relative file path
        return current_dir.unwrap_or(Path::new(".")).join(p);
    }

    if path.starts_with("self:") {
        // Relative path: resolve against current module directory
        let relative = path.strip_prefix("self:").unwrap().replace(':', "/");
        let relative_with_ext = if relative.ends_with(".metta") {
            relative
        } else {
            format!("{}.metta", relative)
        };
        current_dir
            .unwrap_or(Path::new("."))
            .join(relative_with_ext)
    } else if path.starts_with("top:") {
        // Absolute path from workspace root
        let absolute = path.strip_prefix("top:").unwrap().replace(':', "/");
        let absolute_with_ext = if absolute.ends_with(".metta") {
            absolute
        } else {
            format!("{}.metta", absolute)
        };
        PathBuf::from(absolute_with_ext)
    } else {
        // Bare name: treat as self:name
        let relative = path.replace(':', "/");
        let relative_with_ext = if relative.ends_with(".metta") {
            relative
        } else {
            format!("{}.metta", relative)
        };
        if let Some(dir) = current_dir {
            dir.join(relative_with_ext)
        } else {
            PathBuf::from(relative_with_ext)
        }
    }
}

/// Normalize a module name to an absolute path starting with `top:`.
///
/// # Arguments
/// - `base_path` - The current module's path (e.g., "top:parent:current")
/// - `mod_name` - The module name to normalize
///
/// # Returns
/// An absolute module path (e.g., "top:parent:current:child")
pub fn normalize_module_path(base_path: &str, mod_name: &str) -> String {
    if mod_name.starts_with("top:") {
        // Already absolute
        mod_name.to_string()
    } else if mod_name.starts_with("self:") {
        // Relative to base_path
        let relative = mod_name.strip_prefix("self:").unwrap();
        if relative.is_empty() {
            base_path.to_string()
        } else {
            format!("{}:{}", base_path, relative)
        }
    } else {
        // Bare name: treat as self:name
        format!("{}:{}", base_path, mod_name)
    }
}

/// Extract the parent module path from a full module path.
///
/// # Examples
/// - `"top:parent:child"` -> `Some("top:parent")`
/// - `"top"` -> `None`
pub fn parent_module_path(path: &str) -> Option<&str> {
    path.rfind(':').map(|idx| &path[..idx])
}

/// Check if `target` is a submodule of `base`.
///
/// A module is a submodule if it starts with `base:`.
///
/// # Examples
/// - `is_submodule("top:parent", "top:parent:child")` -> `true`
/// - `is_submodule("top:parent", "top:other")` -> `false`
/// - `is_submodule("top:parent", "top:parent")` -> `false` (same module)
pub fn is_submodule(base: &str, target: &str) -> bool {
    target.starts_with(&format!("{}:", base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_self_path() {
        let current = Path::new("/home/user/project/lib");
        let result = resolve_module_path("self:utils", Some(current));
        assert_eq!(result, PathBuf::from("/home/user/project/lib/utils.metta"));
    }

    #[test]
    fn test_resolve_nested_self_path() {
        let current = Path::new("/home/user/project/lib");
        let result = resolve_module_path("self:math:trig", Some(current));
        assert_eq!(
            result,
            PathBuf::from("/home/user/project/lib/math/trig.metta")
        );
    }

    #[test]
    fn test_resolve_top_path() {
        let current = Path::new("/home/user/project/lib");
        let result = resolve_module_path("top:stdlib:core", Some(current));
        assert_eq!(result, PathBuf::from("stdlib/core.metta"));
    }

    #[test]
    fn test_resolve_bare_name() {
        let current = Path::new("/home/user/project/lib");
        let result = resolve_module_path("utils", Some(current));
        assert_eq!(result, PathBuf::from("/home/user/project/lib/utils.metta"));
    }

    #[test]
    fn test_resolve_bare_name_no_current() {
        let result = resolve_module_path("utils", None);
        assert_eq!(result, PathBuf::from("utils.metta"));
    }

    #[test]
    fn test_resolve_direct_file_path() {
        let current = Path::new("/home/user/project");
        let result = resolve_module_path("lib/utils.metta", Some(current));
        assert_eq!(result, PathBuf::from("/home/user/project/lib/utils.metta"));
    }

    #[test]
    fn test_normalize_absolute() {
        let result = normalize_module_path("top:mylib", "top:stdlib:core");
        assert_eq!(result, "top:stdlib:core");
    }

    #[test]
    fn test_normalize_relative() {
        let result = normalize_module_path("top:mylib", "self:utils");
        assert_eq!(result, "top:mylib:utils");
    }

    #[test]
    fn test_normalize_bare() {
        let result = normalize_module_path("top:mylib", "utils");
        assert_eq!(result, "top:mylib:utils");
    }

    #[test]
    fn test_is_submodule() {
        assert!(is_submodule("top:parent", "top:parent:child"));
        assert!(is_submodule("top:parent", "top:parent:child:grandchild"));
        assert!(!is_submodule("top:parent", "top:parent")); // same, not sub
        assert!(!is_submodule("top:parent", "top:other"));
        assert!(!is_submodule("top:parent", "top:parentish")); // prefix but not submodule
    }

    #[test]
    fn test_parent_module_path() {
        assert_eq!(parent_module_path("top:parent:child"), Some("top:parent"));
        assert_eq!(parent_module_path("top"), None);
    }
}
