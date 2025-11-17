use std::path::Path;
use std::process::Command;

fn main() {
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
