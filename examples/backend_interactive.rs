// Interactive MeTTa REPL using the arena-based backend

use mettatron::{compile_arena, eval_arena, new_arena_env, ArenaValueInner};
use std::io::{self, Write};

fn main() {
    println!("=== MeTTa Backend REPL ===");
    println!("Enter MeTTa expressions. Type 'exit' to quit.\n");

    let mut env = new_arena_env();
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
        match compile_arena(input) {
            Ok(state) => {
                // Evaluate each expression
                for &expr in state.source() {
                    let (results, updated_env) = eval_arena(expr, env.clone(), &state);
                    env = updated_env;

                    // Print results
                    for result in &results {
                        match result.inner() {
                            ArenaValueInner::Nil => {}
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
