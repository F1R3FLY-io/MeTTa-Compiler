//! Demonstration of fuzzy matching "Did you mean?" suggestions
//!
//! This example shows how MeTTaTron tracks defined symbols and provides
//! helpful suggestions when encountering typos or misspellings.

use mettatron::backend::{compile, eval};

fn main() {
    let source = r#"
        ;; Define some functions
        (= (fibonacci 0) 0)
        (= (fibonacci 1) 1)
        (= (fibonacci $n) (+ (fibonacci (- $n 1)) (fibonacci (- $n 2))))

        (= (factorial 0) 1)
        (= (factorial $n) (* $n (factorial (- $n 1))))

        (= (hello-world) "Hello, World!")
    "#;

    // Compile source to MettaState
    let state = match compile(source) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error compiling: {}", e);
            return;
        }
    };

    // Evaluate to populate environment with rules
    let env = {
        let mut current_env = state.environment.clone();
        for expr in state.source {
            let (_results, new_env) = eval(expr, current_env);
            current_env = new_env;
        }
        current_env
    };

    println!("✓ Loaded function definitions\n");

    // Test fuzzy matching with typos
    println!("=== Fuzzy Matching Demonstrations ===\n");

    // Test 1: Single character substitution
    let typo1 = "fibonaci";  // Missing 'c' in fibonacci
    println!("Typo: '{}'", typo1);
    if let Some(suggestion) = env.did_you_mean(typo1, 2) {
        println!("  → {}\n", suggestion);
    } else {
        println!("  → No suggestions found\n");
    }

    // Test 2: Transposition
    let typo2 = "factoral"; // Transposed 'or' in factorial
    println!("Typo: '{}'", typo2);
    if let Some(suggestion) = env.did_you_mean(typo2, 2) {
        println!("  → {}\n", suggestion);
    } else {
        println!("  → No suggestions found\n");
    }

    // Test 3: Different separator
    let typo3 = "hello_world";  // Underscore instead of hyphen
    println!("Typo: '{}'", typo3);
    if let Some(suggestion) = env.did_you_mean(typo3, 2) {
        println!("  → {}\n", suggestion);
    } else {
        println!("  → No suggestions found\n");
    }

    // Test 4: Abbreviation
    let typo4 = "fib";  // Shortened form
    println!("Typo: '{}'", typo4);
    let suggestions = env.suggest_similar_symbols(typo4, 5);
    if !suggestions.is_empty() {
        println!("  → Found {} suggestions:", suggestions.len());
        for (term, distance) in suggestions.iter().take(3) {
            println!("     - {} (distance: {})", term, distance);
        }
        println!();
    } else {
        println!("  → No suggestions found\n");
    }

    // Test 5: Completely wrong symbol (should find nothing)
    let typo5 = "xyz";
    println!("Typo: '{}'", typo5);
    if let Some(suggestion) = env.did_you_mean(typo5, 2) {
        println!("  → {}\n", suggestion);
    } else {
        println!("  → No suggestions found (as expected)\n");
    }

    // Show all tracked symbols
    println!("=== Symbol Statistics ===");
    let all_suggestions = env.suggest_similar_symbols("", 100);
    println!("Total tracked symbols: {}", all_suggestions.len());
    if !all_suggestions.is_empty() {
        println!("Known symbols: {}",
            all_suggestions.iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
