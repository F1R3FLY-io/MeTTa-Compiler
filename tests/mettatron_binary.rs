/// Integration tests for the mettatron binary executable
///
/// Tests the standalone mettatron binary to ensure it works correctly
/// with various command-line options and MeTTa files.
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Find the mettatron binary
fn find_mettatron_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Try release build first, then debug
    let candidates = vec![
        manifest_dir.join("target/release/mettatron"),
        manifest_dir.join("target/debug/mettatron"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    panic!(
        "mettatron binary not found. Build it first:\n  cargo build --release\nTried:\n{}",
        candidates
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Get examples directory
fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

// ============================================================================
// Binary Existence Tests
// ============================================================================

#[test]
fn test_binary_exists() {
    let binary = find_mettatron_binary();
    assert!(binary.exists(), "Binary not found at: {}", binary.display());

    // Verify it's executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&binary).expect("Failed to get binary metadata");
        let permissions = metadata.permissions();
        assert!(permissions.mode() & 0o111 != 0, "Binary is not executable");
    }
}

#[test]
fn test_binary_runs() {
    let binary = find_mettatron_binary();
    let output = Command::new(&binary)
        .arg("--help")
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    assert!(
        output.status.success() || output.status.code() == Some(0),
        "Binary --help failed with status: {:?}",
        output.status
    );
}

// ============================================================================
// Basic Evaluation Tests
// ============================================================================

#[test]
fn test_evaluate_simple_metta() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("simple.metta");

    assert!(
        test_file.exists(),
        "Test file not found: {}",
        test_file.display()
    );

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed to evaluate simple.metta:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // Should produce some output
    assert!(!stdout.is_empty(), "No output produced");
}

#[test]
fn test_evaluate_advanced_metta() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("advanced.metta");

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed to evaluate advanced.metta:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_evaluate_mvp_test() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("mvp_test.metta");

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed to evaluate mvp_test.metta:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_evaluate_type_system_demo() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("type_system_demo.metta");

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed to evaluate type_system_demo.metta:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_evaluate_pathmap_demo() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("pathmap_demo.metta");

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed to evaluate pathmap_demo.metta:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

// ============================================================================
// Command-Line Options Tests
// ============================================================================

#[test]
fn test_sexpr_option() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("simple.metta");

    let output = Command::new(&binary)
        .arg("--sexpr")
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed with --sexpr option:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // S-expression output should contain parentheses
    assert!(
        stdout.contains('(') && stdout.contains(')'),
        "S-expression output doesn't look right: {}",
        stdout
    );
}

#[test]
fn test_output_to_file() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("simple.metta");

    // Create temporary output file
    let temp_dir = env::temp_dir();
    let output_file = temp_dir.join(format!("mettatron_test_output_{}.txt", std::process::id()));

    // Clean up if it exists
    let _ = fs::remove_file(&output_file);

    let output = Command::new(&binary)
        .arg(&test_file)
        .arg("-o")
        .arg(&output_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed with -o option:\nSTDERR:\n{}",
        stderr
    );

    // Verify output file was created and has content
    assert!(
        output_file.exists(),
        "Output file not created: {}",
        output_file.display()
    );

    let contents = fs::read_to_string(&output_file).expect("Failed to read output file");

    assert!(!contents.is_empty(), "Output file is empty");

    // Clean up
    let _ = fs::remove_file(&output_file);
}

#[test]
fn test_stdin_input() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let binary = find_mettatron_binary();

    let mut child = Command::new(&binary)
        .arg("-")
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn binary");

    // Write simple MeTTa expression to stdin.
    // S1 TOPLEVEL (2026-05-13): bare top-level S-exprs are silent ADD-mode
    // facts under HE bisimilarity. Use `!(...)` to put the runner into
    // INTERPRET mode so the reduction emits an observable result line.
    {
        let stdin = child.stdin.as_mut().expect("Failed to open stdin");
        stdin
            .write_all(b"!(+ 1 2)\n")
            .expect("Failed to write to stdin");
    }

    let output = child.wait_with_output().expect("Failed to read output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Failed with stdin input:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // Should produce some output
    assert!(!stdout.is_empty(), "No output from stdin evaluation");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_nonexistent_file() {
    let binary = find_mettatron_binary();
    let nonexistent = PathBuf::from("this_file_does_not_exist.metta");

    let output = Command::new(&binary)
        .arg(&nonexistent)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    // Should fail for nonexistent file
    assert!(!output.status.success(), "Should fail for nonexistent file");
}

#[test]
fn test_invalid_metta_syntax() {
    use std::io::Write;

    let binary = find_mettatron_binary();

    // Create temporary file with invalid syntax
    let temp_dir = env::temp_dir();
    let temp_file = temp_dir.join(format!("invalid_syntax_{}.metta", std::process::id()));

    {
        let mut file = fs::File::create(&temp_file).expect("Failed to create temp file");
        file.write_all(b"(+ 1 2")
            .expect("Failed to write to temp file"); // Missing closing paren
    }

    let output = Command::new(&binary)
        .arg(&temp_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should either fail or handle the error gracefully
    // (depending on implementation, error might be in output)
    let has_error = !output.status.success()
        || stdout.contains("error")
        || stdout.contains("Error")
        || stderr.contains("error")
        || stderr.contains("Error");

    assert!(has_error, "Should report error for invalid syntax");

    // Clean up
    let _ = fs::remove_file(&temp_file);
}

// ============================================================================
// All Examples Test
// ============================================================================

#[test]
fn test_all_metta_examples() {
    let binary = find_mettatron_binary();
    let examples = examples_dir();

    let mut metta_files: Vec<_> = fs::read_dir(&examples)
        .expect("Failed to read examples directory")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext == "metta")
                .unwrap_or(false)
        })
        .collect();

    assert!(
        !metta_files.is_empty(),
        "No .metta files found in examples directory"
    );

    // Sort for deterministic ordering (fs::read_dir order is non-deterministic)
    metta_files.sort_by_key(|e| e.path());

    for entry in metta_files {
        let path = entry.path();
        let mut output = Command::new(&binary)
            .arg(&path)
            .env("METTATRON_MAX_WORK_THREADS", "4")
            .output()
            .expect("Failed to execute binary");

        // Retry once if killed by signal (transient resource contention)
        if !output.status.success() {
            #[cfg(unix)]
            let was_signal = {
                use std::os::unix::process::ExitStatusExt;
                output.status.signal().is_some()
            };
            #[cfg(not(unix))]
            let was_signal = false;

            if was_signal {
                output = Command::new(&binary)
                    .arg(&path)
                    .env("METTATRON_MAX_WORK_THREADS", "4")
                    .output()
                    .expect("Failed to execute binary on retry");
            }
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert!(
                output.status.success(),
                "Failed to evaluate {} (exit code: {:?}, signal: {:?}):\nSTDOUT:\n{}\nSTDERR:\n{}",
                path.display(),
                output.status.code(),
                output.status.signal(),
                stdout,
                stderr
            );
        }
        #[cfg(not(unix))]
        {
            assert!(
                output.status.success(),
                "Failed to evaluate {} (exit code: {:?}):\nSTDOUT:\n{}\nSTDERR:\n{}",
                path.display(),
                output.status.code(),
                stdout,
                stderr
            );
        }
    }
}

// ============================================================================
// Demand-Driven Branch Pruning Validation Tests
//
// These tests verify that the Demand::AtLeast(1) optimization on case
// scrutinee evaluation produces correct results for mmverify and PLN
// workloads. The optimization makes nondeterministic dispatch lazy,
// stopping after the first branch that produces results.
// ============================================================================

#[test]
fn test_mmverify_demo0_correct_proof() {
    let binary = find_mettatron_binary();
    let test_file = examples_dir().join("mmverify/demo0/verify_demo0.metta");

    if !test_file.exists() {
        eprintln!("Skipping mmverify test: {} not found", test_file.display());
        return;
    }

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "mmverify demo0 failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    assert!(
        stdout.contains("Correct proof"),
        "mmverify demo0 did not produce 'Correct proof':\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_pln_robot() {
    let binary = find_mettatron_binary();
    let test_file = PathBuf::from("/Users/dylon/Workspace/f1r3fly.io/PLN/examples/Robot.metta");

    if !test_file.exists() {
        eprintln!("Skipping PLN Robot test: {} not found", test_file.display());
        return;
    }

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "PLN Robot failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // The PLN test framework (`eval_test_generic` in src/backend/eval/testing_ops.rs)
    // emits exactly `is X, should Y. ✅` on pass and `... ❌` on failure. The
    // `Passed: true` legacy marker no longer exists in the emitter; assert on the
    // checkmark + the absence of the failure mark.
    assert!(
        stdout.contains("✅") && !stdout.contains("❌"),
        "PLN Robot did not pass (no ✅, or ❌ present):\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_pln_flying_raven() {
    let binary = find_mettatron_binary();
    let test_file =
        PathBuf::from("/Users/dylon/Workspace/f1r3fly.io/PLN/examples/FlyingRaven.metta");

    if !test_file.exists() {
        eprintln!(
            "Skipping PLN FlyingRaven test: {} not found",
            test_file.display()
        );
        return;
    }

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "PLN FlyingRaven failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // See test_pln_robot for the rationale on `✅` / `❌` vs the legacy
    // `Passed: true` marker.
    assert!(
        stdout.contains("✅") && !stdout.contains("❌"),
        "PLN FlyingRaven did not pass (no ✅, or ❌ present):\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_pln_smokes() {
    let binary = find_mettatron_binary();
    let test_file = PathBuf::from("/Users/dylon/Workspace/f1r3fly.io/PLN/examples/Smokes.metta");

    if !test_file.exists() {
        eprintln!(
            "Skipping PLN Smokes test: {} not found",
            test_file.display()
        );
        return;
    }

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "PLN Smokes failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // See test_pln_robot for the rationale on `✅` / `❌` vs the legacy
    // `Passed: true` marker.
    assert!(
        stdout.contains("✅") && !stdout.contains("❌"),
        "PLN Smokes did not pass (no ✅, or ❌ present):\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}

#[test]
fn test_pln_toothbrush() {
    let binary = find_mettatron_binary();
    let test_file =
        PathBuf::from("/Users/dylon/Workspace/f1r3fly.io/PLN/examples/Toothbrush.metta");

    if !test_file.exists() {
        eprintln!(
            "Skipping PLN Toothbrush test: {} not found",
            test_file.display()
        );
        return;
    }

    let output = Command::new(&binary)
        .arg(&test_file)
        .env("METTATRON_MAX_WORK_THREADS", "4")
        .output()
        .expect("Failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "PLN Toothbrush failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // PLN Toothbrush is now passing on this branch — assert ✅ + no ❌ to lock
    // it in. The legacy "Passed: true" marker is no longer emitted; the test
    // framework prints `is X, should Y. ✅` (see eval_test_generic).
    assert!(
        stdout.contains("✅") && !stdout.contains("❌"),
        "PLN Toothbrush did not pass (no ✅, or ❌ present):\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );
}
