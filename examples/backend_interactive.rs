// Interactive MeTTa REPL using the arena-based backend

use mettatron::{compile, eval, new_env, MettaValueInner};
use std::io::{self, Write};

fn main() {
    println!("=== MeTTa Backend REPL ===");
    println!("Enter MeTTa expressions. Type 'exit' to quit.\n");

    let mut env = new_env();
    let mut line_num = 1;

    loop {
        // Print prompt
        print!("metta[{}]> ", line_num);
        io::stdout().flush().expect("failed to flush stdout");

        // Read input
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("failed to read line");
        let input = input.trim();

        // Check for exit
        if input == "exit" || input == "quit" {
            println!("Goodbye!");
            break;
        }

        if input.is_empty() {
            continue;
        }

        // Compile and evaluate
        match compile(input) {
            Ok(state) => {
                // Evaluate each expression
                let source_exprs: Vec<_> = state.source().iter().copied().collect();
                for expr in source_exprs {
                    let (results, updated_env) = eval(expr, env.clone(), &state);
                    env = updated_env;

                    // Print results
                    for result in &results {
                        match result.inner() {
                            MettaValueInner::Unit => {}
                            _ => println!("{}", result),
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }

        line_num += 1;
    }
}
