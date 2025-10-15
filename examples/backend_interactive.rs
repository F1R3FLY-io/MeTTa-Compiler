// Interactive MeTTa REPL using the new backend

use mettatron::backend::*;
use std::io::{self, Write};

fn main() {
    println!("=== MeTTa Backend REPL ===");
    println!("Enter MeTTa expressions. Type 'exit' to quit.\n");

    let mut env = Environment::new();
    let mut line_num = 1;

    loop {
        // Print prompt
        print!("metta[{}]> ", line_num);
        io::stdout().flush().unwrap();

        // Read input
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
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
                // Merge new environment
                env = env.union(&state.environment);

                // Evaluate each expression
                for sexpr in state.pending_exprs {
                    match eval(sexpr.clone(), env.clone()) {
                        (results, updated_env) => {
                            env = updated_env;

                            // Print results
                            for result in results {
                                println!("{:?}", result);
                            }
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
