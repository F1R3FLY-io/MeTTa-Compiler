//! `git-import!` special form
//!
//! PeTTa-compatible `git-import!` that clones a Git repository to a local cache
//! directory and registers the cache directory as a library search path. After
//! `git-import!`, subsequent `(library X Y)` forms (and bare `import!` calls)
//! can resolve files inside the cloned repository.
//!
//! This mirrors PeTTa's `git-import!` Prolog predicate at
//! `<PeTTa>/lib/lib_import.pl:55-64`.
//!
//! ## Usage
//!
//! ```metta
//! ; Clone https://github.com/foo/bar.git into ./repos/bar/ (if not already cached)
//! !(git-import! "https://github.com/foo/bar.git")
//!
//! ; Optional build step (runs `sh -c <build_cmd>` in the cloned directory)
//! !(git-import! "https://github.com/foo/bar.git" "make")
//! ```
//!
//! ## Cache directory
//!
//! Defaults to `./repos/` (relative to the current working directory), matching
//! PeTTa's convention. Override with the `METTA_GIT_CACHE` environment variable.
//!
//! ## Error handling
//!
//! All failures return a graceful error `MettaValue` (never panic). The error
//! message identifies the failure mode and includes relevant context (URL, git
//! stderr, build command, etc.). Failure modes:
//!
//! - Wrong arity (not 1 or 2 args)
//! - Non-string URL argument
//! - Empty or unparseable URL
//! - Failure to create the cache directory
//! - `git` binary not found in `PATH`
//! - `git clone` non-zero exit (network failure, auth error, repo not found)
//! - Build step `sh -c` failure
//!
//! Idempotent: if the local cache directory already exists, the clone is
//! skipped and the path is just registered (or re-registered as a no-op).

use std::path::PathBuf;
use std::process::Command;

use crate::backend::models::{MettaValueFactory, MettaValueTrait};
use crate::backend::modules::path::add_library_path;

/// Evaluate `(git-import! "url")` or `(git-import! "url" "build_cmd")`.
///
/// Returns a single-element `Vec` containing either `Unit` on success or an
/// error `MettaValue` on any failure. Never panics.
pub fn eval_git_import_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    if items.len() < 2 || items.len() > 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "git-import! requires 1 or 2 arguments, got {}. \
                 Usage: (git-import! \"url\" [\"build_cmd\"])",
                items.len() - 1
            )),
        )];
    }

    // Argument 1: URL string. Accept both String literals and atoms (a bare
    // atom URL would be unusual but we handle it gracefully).
    let url = match items[1].as_string() {
        Some(s) => s.to_string(),
        None => match items[1].as_atom() {
            Some(s) => s.to_string(),
            None => {
                return vec![factory.error(
                    items[1].clone(),
                    factory.string("git-import!: first argument must be a URL string"),
                )];
            }
        },
    };

    if url.is_empty() {
        return vec![factory.error(
            items[1].clone(),
            factory.string("git-import!: URL string is empty"),
        )];
    }

    // Optional second arg: build command, also accepted as String or atom.
    let build_cmd: Option<String> = items.get(2).and_then(|v| {
        v.as_string()
            .map(|s| s.to_string())
            .or_else(|| v.as_atom().map(|s| s.to_string()))
    });

    // Cache directory: METTA_GIT_CACHE env var or ./repos/ (PeTTa default).
    let cache_dir = std::env::var("METTA_GIT_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./repos"));

    // Extract repo name from URL: ".../foo.git" → "foo", ".../foo" → "foo".
    let repo_name = match url
        .rsplit('/')
        .next()
        .map(|s| s.strip_suffix(".git").unwrap_or(s))
        .filter(|s| !s.is_empty())
    {
        Some(n) => n.to_string(),
        None => {
            return vec![factory.error(
                items[1].clone(),
                factory.string(&format!(
                    "git-import!: could not extract repo name from URL '{}'",
                    url
                )),
            )];
        }
    };

    let local = cache_dir.join(&repo_name);

    // If the local directory does not yet exist, clone it.
    if !local.exists() {
        // Ensure the parent cache directory exists.
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            return vec![factory.error(
                items[1].clone(),
                factory.string(&format!(
                    "git-import!: failed to create cache directory '{}': {}",
                    cache_dir.display(),
                    e
                )),
            )];
        }

        // Run `git clone --depth 1 <url> <local>`.
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &url])
            .arg(&local)
            .output();

        match output {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                return vec![factory.error(
                    items[1].clone(),
                    factory.string(&format!(
                        "git-import!: 'git clone {}' failed (exit {}): {}",
                        url,
                        o.status.code().unwrap_or(-1),
                        stderr.trim()
                    )),
                )];
            }
            Err(e) => {
                return vec![factory.error(
                    items[1].clone(),
                    factory.string(&format!(
                        "git-import!: failed to invoke 'git' (is it installed and on PATH?): {}",
                        e
                    )),
                )];
            }
        }

        // Optional build step.
        if let Some(cmd) = &build_cmd {
            let bo = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&local)
                .output();
            match bo {
                Ok(o) if o.status.success() => {}
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    return vec![factory.error(
                        items[2].clone(),
                        factory.string(&format!(
                            "git-import!: build step '{}' failed (exit {}) in '{}': {}",
                            cmd,
                            o.status.code().unwrap_or(-1),
                            local.display(),
                            stderr.trim()
                        )),
                    )];
                }
                Err(e) => {
                    return vec![factory.error(
                        items[2].clone(),
                        factory.string(&format!(
                            "git-import!: failed to spawn build shell for '{}': {}",
                            cmd, e
                        )),
                    )];
                }
            }
        }
    }

    // Register the local path with the LIBRARY_PATHS registry. Idempotent: if
    // it was already registered (e.g., from a previous git-import! call), this
    // is a no-op.
    add_library_path(local);

    vec![factory.unit()]
}
