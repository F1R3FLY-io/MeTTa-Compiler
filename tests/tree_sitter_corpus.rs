/// Tree-Sitter corpus tests integration
///
/// This test runs the Tree-Sitter corpus tests to validate the grammar.
/// Requires tree-sitter-cli to be installed: `npm install -g tree-sitter-cli`

use std::process::Command;

#[test]
fn test_tree_sitter_corpus() {
    // Check if tree-sitter CLI is available
    let check_cli = Command::new("tree-sitter")
        .arg("--version")
        .output();

    if check_cli.is_err() {
        eprintln!("WARNING: tree-sitter CLI not found. Install with: npm install -g tree-sitter-cli");
        eprintln!("Skipping Tree-Sitter corpus tests.");
        return;
    }

    // Run tree-sitter test in the tree-sitter-metta directory
    let output = Command::new("tree-sitter")
        .arg("test")
        .current_dir("tree-sitter-metta")
        .output()
        .expect("Failed to execute tree-sitter test");

    // Print the output for debugging
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stdout.is_empty() {
        println!("Tree-Sitter test output:\n{}", stdout);
    }

    if !stderr.is_empty() {
        eprintln!("Tree-Sitter test errors:\n{}", stderr);
    }

    // Assert that the command succeeded
    assert!(
        output.status.success(),
        "Tree-Sitter corpus tests failed. See output above for details."
    );

    // Verify the success message is in the output
    let output_str = stdout.to_string();
    assert!(
        output_str.contains("success percentage: 100.00%"),
        "Not all Tree-Sitter corpus tests passed. Got:\n{}",
        output_str
    );
}
