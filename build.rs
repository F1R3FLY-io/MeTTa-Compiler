use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

fn main() {
    // Part 1: Tree-Sitter parser generation
    regenerate_tree_sitter_parser();

    // Part 2: Rholang-cli auto-rebuild for integration tests
    ensure_rholang_cli_updated();
}

/// Regenerate Tree-Sitter parser from grammar.js if needed
fn regenerate_tree_sitter_parser() {
    let grammar_path = "tree-sitter-metta/grammar.js";
    let parser_dir = "tree-sitter-metta";

    // Tell cargo to rerun this build script if the grammar changes
    println!("cargo:rerun-if-changed={}", grammar_path);

    // Check if tree-sitter CLI is available
    let tree_sitter_check = Command::new("tree-sitter").arg("--version").output();

    match tree_sitter_check {
        Ok(output) if output.status.success() => {
            // tree-sitter is available, regenerate parser
            eprintln!("Regenerating Tree-Sitter parser from grammar...");

            let status = Command::new("tree-sitter")
                .arg("generate")
                .current_dir(parser_dir)
                .status()
                .expect("Failed to execute tree-sitter generate");

            if !status.success() {
                eprintln!("Warning: tree-sitter generate failed, using existing parser");
            } else {
                eprintln!("Tree-Sitter parser regenerated successfully");
            }
        }
        _ => {
            // tree-sitter CLI not available
            if !Path::new(&format!("{}/src/parser.c", parser_dir)).exists() {
                panic!(
                    "tree-sitter CLI not found and parser.c doesn't exist.\n\
                     Install tree-sitter CLI: npm install -g tree-sitter-cli\n\
                     Or generate parser manually: cd {} && tree-sitter generate",
                    parser_dir
                );
            }
            eprintln!("tree-sitter CLI not found, using existing parser");
        }
    }
}

/// Ensure rholang-cli is up-to-date for integration tests
fn ensure_rholang_cli_updated() {
    // Skip if we're being built as a dependency (prevents circular builds)
    // CARGO_PRIMARY_PACKAGE is only set when building the primary package
    if std::env::var("CARGO_PRIMARY_PACKAGE").is_err() {
        return; // We're a dependency, skip rholang-cli rebuild to prevent cycles
    }

    // Tell Cargo to rerun this build script when these directories change
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tree-sitter-metta/src");
    println!("cargo:rerun-if-changed=tree-sitter-metta/grammar.js");
    println!("cargo:rerun-if-changed=../f1r3node/rholang/src");

    let f1r3node_path = Path::new("../f1r3node");
    let rholang_cli_path = f1r3node_path.join("target/release/rholang-cli");

    // Skip if f1r3node doesn't exist (e.g., CI environment)
    if !f1r3node_path.exists() {
        println!(
            "cargo:warning=f1r3node not found at ../f1r3node, skipping rholang-cli build check"
        );
        return;
    }

    // Determine if we need to rebuild
    let should_rebuild = if !rholang_cli_path.exists() {
        println!("cargo:warning=rholang-cli binary not found, will build");
        true
    } else {
        // Get the binary's modification time
        let binary_mtime = match fs::metadata(&rholang_cli_path).and_then(|m| m.modified()) {
            Ok(time) => time,
            Err(e) => {
                println!(
                    "cargo:warning=Failed to get rholang-cli mtime: {}, rebuilding",
                    e
                );
                SystemTime::UNIX_EPOCH
            }
        };

        // Check if MeTTa-Compiler source is newer than binary
        let mettatron_src_newer = is_dir_newer("src", binary_mtime);

        // Check if tree-sitter-metta is newer (mettatron depends on it)
        let tree_sitter_src_newer = is_dir_newer("tree-sitter-metta/src", binary_mtime);
        let tree_sitter_grammar_newer = is_file_newer("tree-sitter-metta/grammar.js", binary_mtime);

        // Check if rholang source is newer than binary
        let rholang_src_newer = is_dir_newer("../f1r3node/rholang/src", binary_mtime);

        // Check if models source is newer (rholang depends on it)
        let models_src_newer = is_dir_newer("../f1r3node/models/src", binary_mtime);

        if mettatron_src_newer {
            println!("cargo:warning=MeTTa-Compiler source changed, rebuilding rholang-cli");
            true
        } else if tree_sitter_src_newer {
            println!("cargo:warning=tree-sitter-metta source changed, rebuilding rholang-cli");
            true
        } else if tree_sitter_grammar_newer {
            println!("cargo:warning=tree-sitter-metta grammar changed, rebuilding rholang-cli");
            true
        } else if rholang_src_newer {
            println!("cargo:warning=Rholang source changed, rebuilding rholang-cli");
            true
        } else if models_src_newer {
            println!("cargo:warning=Models source changed, rebuilding rholang-cli");
            true
        } else {
            false
        }
    };

    if should_rebuild {
        println!("cargo:warning=Building rholang-cli...");

        let status = Command::new("cargo")
            .args(["build", "--release", "--bin", "rholang-cli"])
            .current_dir(f1r3node_path)
            .status()
            .expect("Failed to execute cargo build for rholang-cli");

        if !status.success() {
            panic!("Failed to build rholang-cli - integration tests may fail");
        }

        println!("cargo:warning=rholang-cli built successfully");
    }
}

/// Check if a single file is newer than the given timestamp
fn is_file_newer(file_path: &str, than: SystemTime) -> bool {
    let path = Path::new(file_path);

    if !path.exists() {
        return false;
    }

    if let Ok(metadata) = fs::metadata(path) {
        if let Ok(modified) = metadata.modified() {
            return modified > than;
        }
    }

    false
}

/// Check if any file in a directory tree is newer than the given timestamp
fn is_dir_newer(dir_path: &str, than: SystemTime) -> bool {
    let path = Path::new(dir_path);

    if !path.exists() {
        return false;
    }

    check_path_recursive(path, than)
}

/// Recursively check if any file in path is newer than timestamp
fn check_path_recursive(path: &Path, than: SystemTime) -> bool {
    if !path.exists() {
        return false;
    }

    // Check the path itself
    if let Ok(metadata) = fs::metadata(path) {
        if let Ok(modified) = metadata.modified() {
            if modified > than {
                return true;
            }
        }
    }

    // If it's a directory, check all entries
    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();

                // Skip target directories to avoid false positives
                if entry_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == "target")
                    .unwrap_or(false)
                {
                    continue;
                }

                if check_path_recursive(&entry_path, than) {
                    return true;
                }
            }
        }
    }

    false
}
