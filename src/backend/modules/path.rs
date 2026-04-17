//! Module Path Resolution
//!
//! Supports three path notations:
//! - `self:child` - Relative to current module directory
//! - `top:absolute:path` - Absolute from workspace root
//! - `bare_name` - Treated as `self:bare_name` or file path
//!
//! Also supports PeTTa-style `(library X)` and `(library X Y)` S-expression
//! path forms, plus a runtime-mutable `LIBRARY_PATHS` registry that
//! `git-import!` populates with cloned-repo locations.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::backend::models::MettaValue;
use crate::backend::models::metta_value_trait::MettaValueTrait;

/// Cached value of the `METTA_MODULE_PATH` environment variable.
/// Uses `OnceLock` so the syscall happens at most once per process.
static METTA_MODULE_PATH_CACHED: OnceLock<Option<String>> = OnceLock::new();

fn cached_module_path() -> Option<&'static str> {
    METTA_MODULE_PATH_CACHED
        .get_or_init(|| std::env::var("METTA_MODULE_PATH").ok())
        .as_deref()
}

// =============================================================================
// PeTTa-compatible library path registry
// =============================================================================
//
// PeTTa's `(library X)` and `(library X Y)` forms search a runtime-mutable
// list of directories — Prolog's `library_path/1` dynamic predicate. PeTTa
// seeds this with `<PeTTa>/lib` at startup and `git-import!` adds cloned
// repositories at runtime.
//
// MeTTaTron mirrors this with a global `LIBRARY_PATHS` `RwLock<Vec<PathBuf>>`.
// Reads dominate writes (every `(library …)` resolution is a read; only
// `git-import!` writes), so `RwLock` is the right primitive.

/// Global, thread-safe list of directories searched by `(library X)` forms.
/// Use `add_library_path` to push entries and `library_paths_snapshot` to read.
static LIBRARY_PATHS: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();

fn library_paths_init() -> RwLock<Vec<PathBuf>> {
    let mut paths = Vec::new();

    // 1. Seed from METTA_LIBRARY_PATH (colon-separated), if set.
    if let Ok(v) = std::env::var("METTA_LIBRARY_PATH") {
        for p in v.split(':').filter(|s| !s.is_empty()) {
            paths.push(PathBuf::from(p));
        }
    }

    // 2. Seed bundled stdlib directory(ies) relative to the running executable.
    //    Two locations are tried so the same code works for both `cargo build`
    //    layouts (target/release/<bin> → ../../stdlib) and installed binaries
    //    (<install>/bin/<bin> → <install>/stdlib).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Cargo target layout: target/{debug,release}/<bin> → ../../stdlib
            let cargo_stdlib = dir.join("..").join("..").join("stdlib");
            if cargo_stdlib.is_dir() {
                paths.push(cargo_stdlib);
            }
            // Installed/copied layout: <bin_dir>/stdlib
            let local_stdlib = dir.join("stdlib");
            if local_stdlib.is_dir() {
                paths.push(local_stdlib);
            }
        }
    }

    // 3. Also seed CARGO_MANIFEST_DIR/stdlib at compile-time, useful for tests
    //    and `cargo run` where the executable lives somewhere odd.
    let manifest_stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib");
    if manifest_stdlib.is_dir() && !paths.iter().any(|p| p == &manifest_stdlib) {
        paths.push(manifest_stdlib);
    }

    // 4. Auto-register existing repos/*/ directories from previous git-import!
    //    calls. This allows (library X) to resolve from a previously-cloned repo
    //    without requiring git-import! in every file that uses the library.
    //    Example: after `git-import! PLN`, repos/PLN/ is registered, so
    //    `(library lib_pln)` resolves to repos/PLN/lib_pln.metta.
    if let Ok(cwd) = std::env::current_dir() {
        let repos_dir = cwd.join("repos");
        if repos_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&repos_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && !paths.iter().any(|p| p == &path) {
                        paths.push(path);
                    }
                }
            }
        }
    }

    RwLock::new(paths)
}

/// Append a directory to the library search path.
///
/// Idempotent: adding the same path twice has no effect on the second call.
/// Used by `git-import!` to register cloned-repo directories.
///
/// Tolerates poisoned locks (a panic in another thread leaves the lock in a
/// poisoned state but the data is still readable/writable).
pub fn add_library_path(path: PathBuf) {
    let lock = LIBRARY_PATHS.get_or_init(library_paths_init);
    let mut guard = match lock.write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !guard.iter().any(|p| p == &path) {
        guard.push(path);
    }
}

/// Snapshot the current library search path. Returns a clone so callers
/// don't hold the lock during file-existence checks.
pub fn library_paths_snapshot() -> Vec<PathBuf> {
    let lock = LIBRARY_PATHS.get_or_init(library_paths_init);
    let guard = match lock.read() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clone()
}

/// Resolve a `(library X)` or `(library X Y)` S-expression to a filesystem path.
///
/// Mirrors PeTTa's `library/2` and `library/3` Prolog rules in
/// `<PeTTa>/src/metta.pl:2-3`:
///
///   `library(X, Path) :- library_path(Base), atomic_list_concat([Base, '/', X], Path).`
///   `library(X, Y, Path) :- library_path(Base), atomic_list_concat([Base, '/../', X, '/', Y], Path).`
///
/// **One-arg form `(library X)`**: returns `<library_path>/X.metta` for the
/// first registered `library_path` entry where the file exists.
///
/// **Two-arg form `(library X Y)`**: returns `<library_path>/../X/Y.metta` —
/// the "sibling repo" pattern. After `git-import!` registers `./repos/<repo>`,
/// the parent of that path is `./repos/`, so `(library X Y)` resolves to
/// `./repos/X/Y.metta`.
///
/// Returns `None` if the form doesn't match the expected shape OR if no
/// candidate path exists. Callers should produce an error MettaValue on
/// `None` (graceful failure, never panic).
pub fn resolve_library_form(items: &[MettaValue]) -> Option<PathBuf> {
    resolve_library_form_with_importer(items, None)
}

/// Resolve `(library …)` with an optional importing-file directory that
/// takes priority over registered `library_paths`. This is the path used
/// by the `import!` implementation: the directory of the `.metta` file
/// doing the import becomes the primary search root, so libraries
/// alongside the importer resolve without relying on external config,
/// `repos/` auto-registration, or symlinked trees.
pub fn resolve_library_form_with_importer(
    items: &[MettaValue],
    importer_dir: Option<&Path>,
) -> Option<PathBuf> {
    let head = items.first()?.as_atom()?;
    if head != "library" {
        return None;
    }

    let registered = library_paths_snapshot();

    // Assemble the search list. Importer-relative resolution is the
    // primary "project root of the importing file" semantics: we walk
    // upward from the importing file's directory, adding each ancestor
    // as a search base, so layouts like
    //   <project>/examples/Direct.metta + <project>/lib_pln.metta
    // resolve without relying on external config, `repos/` auto-register,
    // or symlinks. Walk cap: 16 levels — enough for any reasonable
    // project depth, bounded so we don't walk to / in pathological cases.
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(d) = importer_dir {
        let mut cur: Option<&Path> = Some(d);
        let mut hops = 0;
        while let Some(p) = cur {
            bases.push(p.to_path_buf());
            hops += 1;
            if hops >= 16 {
                break;
            }
            cur = p.parent();
        }
    }
    bases.extend(registered.into_iter());

    match items.len() {
        2 => {
            // (library X) → <base>/X.metta
            let name = items[1].as_atom()?;
            let filename = if name.ends_with(".metta") {
                name.to_string()
            } else {
                format!("{}.metta", name)
            };
            bases.iter()
                .map(|base| base.join(&filename))
                .find(|p| p.exists())
        }
        3 => {
            // (library X Y) → <base>/X/Y.metta (importer-relative) OR
            //                 <base>/../X/Y.metta (sibling-repo, PeTTa style).
            // Try the importer-relative shape first for each base, then
            // fall back to the sibling-repo shape. This makes layouts
            // like `<project>/Direct.metta` + `<project>/PLN/lib_pln.metta`
            // resolve naturally via the importer_dir base.
            let x = items[1].as_atom()?;
            let y = items[2].as_atom()?;
            let filename = if y.ends_with(".metta") {
                y.to_string()
            } else {
                format!("{}.metta", y)
            };
            // First pass: <base>/X/Y.metta
            if let Some(p) = bases
                .iter()
                .map(|base| base.join(x).join(&filename))
                .find(|p| p.exists())
            {
                return Some(p);
            }
            // Second pass: <base>/../X/Y.metta (PeTTa sibling-repo shape)
            bases.iter()
                .map(|base| {
                    let parent = base.parent().unwrap_or(base);
                    parent.join(x).join(&filename)
                })
                .find(|p| p.exists())
        }
        _ => None,
    }
}

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
        // Bare name: search multiple locations
        let relative = path.replace(':', "/");
        let name_with_ext = if relative.ends_with(".metta") {
            relative.clone()
        } else {
            format!("{}.metta", relative)
        };

        // 1. {current_dir}/{name}.metta (original behavior)
        if let Some(dir) = current_dir {
            let candidate = dir.join(&name_with_ext);
            if candidate.exists() {
                return candidate;
            }

            // 2. {current_dir}/{name}/{name}.metta (directory-based module)
            let dir_candidate = dir.join(&relative).join(&name_with_ext);
            if dir_candidate.exists() {
                return dir_candidate;
            }

            // 3. Walk up parent directories
            let mut ancestor = dir.parent();
            while let Some(parent) = ancestor {
                let candidate = parent.join(&name_with_ext);
                if candidate.exists() {
                    return candidate;
                }
                // Also check directory-based module in parent
                let dir_candidate = parent.join(&relative).join(&name_with_ext);
                if dir_candidate.exists() {
                    return dir_candidate;
                }
                ancestor = parent.parent();
            }
        }

        // 4. Search METTA_MODULE_PATH directories (cached — one syscall per process)
        if let Some(module_path) = cached_module_path() {
            for search_dir in module_path.split(':') {
                let search_path = Path::new(search_dir);
                let candidate = search_path.join(&name_with_ext);
                if candidate.exists() {
                    return candidate;
                }
                // Also check directory-based module
                let dir_candidate = search_path.join(&relative).join(&name_with_ext);
                if dir_candidate.exists() {
                    return dir_candidate;
                }
            }
        }

        // 5. Search the LIBRARY_PATHS registry (used by both PeTTa-style
        //    `(library X)` forms and any HE-style import that wants to find a
        //    module under a `git-import!`-cloned repo or under
        //    `<MeTTaTron>/stdlib`). This unifies HE-style and PeTTa-style
        //    resolution under one search list.
        for search_path in library_paths_snapshot() {
            let candidate = search_path.join(&name_with_ext);
            if candidate.exists() {
                return candidate;
            }
            // Also check directory-based module
            let dir_candidate = search_path.join(&relative).join(&name_with_ext);
            if dir_candidate.exists() {
                return dir_candidate;
            }
        }

        // Fallback: return the original {current_dir}/{name}.metta path
        // (will produce a clear "file not found" error)
        if let Some(dir) = current_dir {
            dir.join(name_with_ext)
        } else {
            PathBuf::from(name_with_ext)
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
