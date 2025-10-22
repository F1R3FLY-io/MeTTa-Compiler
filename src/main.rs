/// MeTTaTron - MeTTa Evaluator CLI
use mettatron::backend::*;
use mettatron::sexpr::*;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_usage() {
    eprintln!("MeTTaTron v{}", VERSION);
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    mettatron [OPTIONS] <INPUT>");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    -h, --help           Print this help message");
    eprintln!("    -v, --version        Print version information");
    eprintln!("    -o, --output <FILE>  Write output to FILE (default: stdout)");
    eprintln!("    --sexpr              Print S-expressions instead of evaluating");
    eprintln!("    --repl               Start interactive REPL");
    eprintln!("    --eval               Evaluate and print results (default)");
    eprintln!();
    eprintln!("ARGUMENTS:");
    eprintln!("    <INPUT>              Input MeTTa file (use '-' for stdin)");
    eprintln!();
    eprintln!("EXAMPLES:");
    eprintln!("    mettatron input.metta");
    eprintln!("    mettatron --repl");
    eprintln!("    mettatron --sexpr input.metta");
    eprintln!("    cat input.metta | mettatron -");
}

fn print_version() {
    println!("MeTTaTron {}", VERSION);
}

struct Options {
    input: Option<String>,
    output: Option<String>,
    show_sexpr: bool,
    repl_mode: bool,
}

fn parse_args() -> Result<Options, String> {
    let args: Vec<String> = env::args().collect();

    let mut input = None;
    let mut output = None;
    let mut show_sexpr = false;
    let mut repl_mode = false;
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                process::exit(0);
            }
            "-v" | "--version" => {
                print_version();
                process::exit(0);
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing output file after -o".to_string());
                }
                output = Some(args[i].clone());
            }
            "--sexpr" => {
                show_sexpr = true;
            }
            "--repl" => {
                repl_mode = true;
            }
            "--eval" => {
                // Default mode, no-op
            }
            arg if arg.starts_with('-') && arg != "-" => {
                return Err(format!("Unknown option: {}", arg));
            }
            arg => {
                if input.is_some() {
                    return Err("Multiple input files specified".to_string());
                }
                input = Some(arg.to_string());
            }
        }
        i += 1;
    }

    Ok(Options {
        input,
        output,
        show_sexpr,
        repl_mode,
    })
}

fn read_input(input: &str) -> Result<String, String> {
    if input == "-" {
        // Read from stdin
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|e| format!("Failed to read from stdin: {}", e))?;
        Ok(buffer)
    } else {
        // Read from file
        let path = Path::new(input);
        if !path.exists() {
            return Err(format!("Input file not found: {}", input));
        }
        fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", input, e))
    }
}

fn write_output(output: Option<&str>, content: &str) -> Result<(), String> {
    match output {
        Some(path) => {
            let mut file = fs::File::create(path)
                .map_err(|e| format!("Failed to create output file '{}': {}", path, e))?;
            file.write_all(content.as_bytes())
                .map_err(|e| format!("Failed to write to output file '{}': {}", path, e))?;
            Ok(())
        }
        None => {
            print!("{}", content);
            Ok(())
        }
    }
}

fn format_result(value: &MettaValue) -> String {
    match value {
        MettaValue::Atom(s) => s.clone(),
        MettaValue::Bool(b) => b.to_string(),
        MettaValue::Long(n) => n.to_string(),
        MettaValue::String(s) => format!("\"{}\"", s),
        MettaValue::Uri(s) => format!("`{}`", s),
        MettaValue::Nil => "Nil".to_string(),
        MettaValue::Error(msg, details) => {
            // Format as (Error "msg" details) to match MeTTa spec
            format!("(Error {} {})", msg, format_result(details))
        }
        MettaValue::Type(t) => format!("Type({})", format_result(t)),
        MettaValue::SExpr(items) => {
            let formatted: Vec<String> = items.iter().map(format_result).collect();
            format!("({})", formatted.join(" "))
        }
    }
}

fn format_results(results: &[MettaValue]) -> String {
    if results.is_empty() {
        return "[]".to_string();
    }
    let formatted: Vec<String> = results.iter().map(format_result).collect();
    format!("[{}]", formatted.join(", "))
}

fn eval_metta(input: &str, options: &Options) -> Result<String, String> {
    // Lexical analysis
    let mut lexer = Lexer::new(input);
    let tokens = lexer
        .tokenize()
        .map_err(|e| format!("Lexical error: {}", e))?;

    if options.show_sexpr {
        let mut parser = Parser::new(tokens);
        let sexprs = parser.parse().map_err(|e| format!("Parse error: {}", e))?;
        let mut output = String::new();
        for sexpr in sexprs {
            output.push_str(&format!("{}\n", sexpr));
        }
        return Ok(output);
    }

    // Compile to MettaValue
    let state = compile(input)?;
    let mut env = state.environment;

    // Evaluate each expression
    let mut output = String::new();
    for sexpr in state.source {
        // Only output results for S-expressions, not atoms or ground types
        let should_output = matches!(sexpr, MettaValue::SExpr(_));

        let (results, new_env) = eval(sexpr, env);
        env = new_env;

        // Print results with list notation (only for S-expressions)
        if should_output && !results.is_empty() {
            output.push_str(&format!("{}\n", format_results(&results)));
        }
    }

    Ok(output)
}

fn run_repl() {
    println!("MeTTaTron REPL v{}", VERSION);
    println!("Enter MeTTa expressions. Type 'exit' or 'quit' to exit.\n");

    let mut env = Environment::new();
    let mut line_num = 1;

    loop {
        print!("metta[{}]> ", line_num);
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();

        if input == "exit" || input == "quit" {
            println!("Goodbye!");
            break;
        }

        if input.is_empty() {
            continue;
        }

        match compile(input) {
            Ok(state) => {
                env = env.union(&state.environment);

                for sexpr in state.source {
                    // Only output results for S-expressions, not atoms or ground types
                    let should_output = matches!(sexpr, MettaValue::SExpr(_));

                    let (results, updated_env) = eval(sexpr.clone(), env.clone());
                    env = updated_env;

                    // Print results with list notation (only for S-expressions)
                    if should_output && !results.is_empty() {
                        println!("{}", format_results(&results));
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

fn main() {
    let options = match parse_args() {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };

    // REPL mode
    if options.repl_mode {
        run_repl();
        return;
    }

    // No input file and not REPL mode - show usage
    if options.input.is_none() {
        eprintln!("Error: Missing input file");
        eprintln!();
        print_usage();
        process::exit(1);
    }

    // File evaluation mode
    let input_content = match read_input(options.input.as_ref().unwrap()) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let output = match eval_metta(&input_content, &options) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    if let Err(e) = write_output(options.output.as_deref(), &output) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
