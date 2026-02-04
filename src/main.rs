/// MeTTaTron - MeTTa Evaluator CLI
use mettatron::backend::*;
use mettatron::repl::{MettaHelper, QueryHighlighter};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
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
    eprintln!("    -h, --help              Print this help message");
    eprintln!("    -v, --version           Print version information");
    eprintln!("    -o, --output <FILE>     Write output to FILE (default: stdout)");
    eprintln!("    --sexpr                 Print S-expressions instead of evaluating");
    eprintln!("    --repl                  Start interactive REPL");
    eprintln!("    --eval                  Evaluate and print results (default)");
    eprintln!("    --strict-mode           Disable transitive imports (explicit deps only)");
    eprintln!();
    eprintln!("ARGUMENTS:");
    eprintln!("    <INPUT>                 Input MeTTa file (use '-' for stdin)");
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
    strict_mode: bool,
}

fn parse_args() -> Result<Options, String> {
    let args: Vec<String> = env::args().collect();

    let mut input = None;
    let mut output = None;
    let mut show_sexpr = false;
    let mut repl_mode = false;
    let mut strict_mode = false;
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
            "--strict-mode" => {
                strict_mode = true;
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
        strict_mode,
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
    match value.inner() {
        MettaValueInner::Atom(s) => s.clone(),
        MettaValueInner::Bool(b) => b.to_string(),
        MettaValueInner::Long(n) => n.to_string(),
        MettaValueInner::Float(f) => f.to_string(),
        MettaValueInner::String(s) => format!("\"{}\"", s),
        MettaValueInner::Nil => "Nil".to_string(),
        MettaValueInner::Error(msg, details) => {
            // Format as (Error "msg" details) to match MeTTa spec
            format!("(Error {} {})", msg, format_result(details))
        }
        MettaValueInner::Type(t) => format!("Type({})", format_result(t)),
        MettaValueInner::SExpr(items) => {
            let formatted: Vec<String> = items.iter().map(format_result).collect();
            format!("({})", formatted.join(" "))
        }
        MettaValueInner::Conjunction(goals) => {
            let formatted: Vec<String> = goals.iter().map(format_result).collect();
            format!("(, {})", formatted.join(" "))
        }
        MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValueInner::State(id) => format!("(State {})", id),
        MettaValueInner::Unit => "()".to_string(),
        MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        MettaValueInner::Empty => "Empty".to_string(),
    }
}

fn format_results(results: &[MettaValue]) -> String {
    if results.is_empty() {
        return "[]".to_string();
    }
    let formatted: Vec<String> = results.iter().map(format_result).collect();
    format!("[{}]", formatted.join(", "))
}

// ============================================================================
// Arena-based evaluation functions (zero-conversion path)
// ============================================================================

/// Format an ArenaValue result for display (mirrors format_result for MettaValue)
fn format_result_arena(value: &ArenaValue) -> String {
    match value.inner() {
        ArenaValueInner::Atom(s) => s.to_string(),
        ArenaValueInner::Bool(b) => b.to_string(),
        ArenaValueInner::Long(n) => n.to_string(),
        ArenaValueInner::Float(f) => f.to_string(),
        ArenaValueInner::String(s) => format!("\"{}\"", s),
        ArenaValueInner::Nil => "Nil".to_string(),
        ArenaValueInner::Error(msg, details) => {
            format!("(Error {} {})", msg, format_result_arena(details))
        }
        ArenaValueInner::Type(t) => format!("Type({})", format_result_arena(t)),
        ArenaValueInner::SExpr(items) => {
            let formatted: Vec<String> = items.iter().map(format_result_arena).collect();
            format!("({})", formatted.join(" "))
        }
        ArenaValueInner::Conjunction(goals) => {
            let formatted: Vec<String> = goals.iter().map(format_result_arena).collect();
            format!("(, {})", formatted.join(" "))
        }
        ArenaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        ArenaValueInner::State(id) => format!("(State {})", id),
        ArenaValueInner::Unit => "()".to_string(),
        ArenaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        ArenaValueInner::Empty => "Empty".to_string(),
    }
}

fn format_results_arena(results: &[ArenaValue]) -> String {
    if results.is_empty() {
        return "[]".to_string();
    }
    let formatted: Vec<String> = results.iter().map(format_result_arena).collect();
    format!("[{}]", formatted.join(", "))
}

fn eval_metta(input: &str, options: &Options) -> Result<String, String> {
    if options.show_sexpr {
        // Parse with Tree-Sitter and show S-expressions
        let mut parser = mettatron::TreeSitterMettaParser::new()
            .map_err(|e| format!("Failed to initialize parser: {}", e))?;
        let sexprs = parser.parse(input).map_err(|e| e.to_string())?;
        let mut output = String::new();
        for sexpr in sexprs {
            output.push_str(&format!("{}\n", sexpr));
        }
        return Ok(output);
    }

    // Choose evaluation mode based on METTA_USE_ARENA environment variable
    if is_arena_mode_enabled() {
        eval_metta_arena(input, options)
    } else {
        eval_metta_heap(input, options)
    }
}

/// Heap-based evaluation (default mode)
///
/// Uses MettaValue throughout: compile → MettaValue → eval → MettaValue
fn eval_metta_heap(input: &str, options: &Options) -> Result<String, String> {
    // Common setup: file path for error messages
    let file_path = options
        .input
        .as_ref()
        .filter(|p| *p != "-")
        .map(|s| s.as_str());

    // Create environment
    let mut env = Environment::default();

    // Set the current module path for relative includes
    if let Some(ref input_path) = options.input {
        if input_path != "-" {
            let path = Path::new(input_path);
            // Canonicalize to get absolute path, then get parent directory
            if let Ok(canonical) = path.canonicalize() {
                if let Some(parent) = canonical.parent() {
                    env.set_current_module_path(Some(parent.to_path_buf()));
                }
            } else if let Some(parent) = path.parent() {
                // Fallback if file doesn't exist yet (shouldn't happen, but be safe)
                env.set_current_module_path(Some(parent.to_path_buf()));
            }
        }
    }

    // Configure strict mode if requested
    if options.strict_mode {
        env.set_strict_mode(true);
    }

    // Standard MettaValue evaluation
    let state = compile_with_path(input, file_path).map_err(|e| e.to_string())?;
    // Merge any rules from compilation into our environment
    env = env.union(&state.environment);

    // Evaluate each expression
    let mut output = String::new();
    for sexpr in state.source {
        // Only output results for S-expressions, not atoms or ground types
        let should_output = matches!(sexpr.inner(), MettaValueInner::SExpr(_));

        let (results, new_env) = eval(sexpr, env);
        env = new_env;

        // Filter out Empty sentinels (HE-compatible: Empty is filtered at result collection)
        let filtered_results: Vec<MettaValue> = results
            .into_iter()
            .filter(|v| !matches!(v.inner(), MettaValueInner::Empty))
            .collect();

        // Print results with list notation (only for S-expressions)
        // HE-compatible: print [] for empty result sets
        if should_output {
            output.push_str(&format!("{}\n", format_results(&filtered_results)));
        }
    }

    Ok(output)
}

/// Arena-based evaluation (zero-conversion mode)
///
/// Uses ArenaValue<'static> throughout: compile_arena → ArenaValue → eval_arena → ArenaValue
/// No conversions between value types occur in this mode.
///
/// With bytecode/JIT tiering enabled:
/// - First execution triggers background bytecode compilation
/// - Subsequent executions use bytecode VM if ready
/// - Falls back to tree-walker interpreter for cold code
///
/// ## Environment Persistence
///
/// Uses `StaticArenaContext::get_or_create_env()` to maintain state (rules, facts,
/// bindings) across sequential evaluations, matching heap mode behavior.
fn eval_metta_arena(input: &str, options: &Options) -> Result<String, String> {

    // Common setup: file path for error messages
    let file_path = options
        .input
        .as_ref()
        .filter(|p| *p != "-")
        .map(|s| s.as_str());

    // Get or create persistent arena environment.
    // This maintains state across sequential evaluations, matching heap mode behavior.
    // Uses thread-local storage to persist rules, facts, and bindings.
    let mut env = StaticArenaContext::get_or_create_env();

    // Set the current module path for relative includes
    if let Some(ref input_path) = options.input {
        if input_path != "-" {
            let path = Path::new(input_path);
            // Canonicalize to get absolute path, then get parent directory
            if let Ok(canonical) = path.canonicalize() {
                if let Some(parent) = canonical.parent() {
                    env.set_current_module_path(Some(parent.to_path_buf()));
                }
            } else if let Some(parent) = path.parent() {
                // Fallback if file doesn't exist yet (shouldn't happen, but be safe)
                env.set_current_module_path(Some(parent.to_path_buf()));
            }
        }
    }

    // Configure strict mode if requested
    if options.strict_mode {
        env.set_strict_mode(true);
    }

    // Compile directly to ArenaValue<'static>
    let exprs = compile_arena_with_path(input, file_path).map_err(|e| e.to_string())?;

    // Evaluate each expression using bytecode/JIT tiering
    let mut output = String::new();
    for expr in exprs {
        // Only output results for S-expressions, not atoms or ground types
        let should_output = expr.is_sexpr();

        // Use eval_arena for bytecode/JIT tiering (zero-conversion throughout)
        let (results, new_env) = eval_arena(expr, env);
        env = new_env;

        // Filter out Empty sentinels (HE-compatible: Empty is filtered at result collection)
        let filtered_results: Vec<ArenaValue> = results
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect();

        // Print results with list notation (only for S-expressions)
        // HE-compatible: print [] for empty result sets
        if should_output {
            output.push_str(&format!("{}\n", format_results_arena(&filtered_results)));
        }
    }

    // Persist the final environment state for subsequent evaluations.
    // This is critical for arena mode correctness: without this, state changes
    // (rules, facts, bindings) would be lost between evaluation batches.
    StaticArenaContext::update_env(env);

    Ok(output)
}

/// Check if stdout is a TTY (for conditional color output)
fn is_stdout_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Create a colorized prompt for the REPL
fn create_prompt(line_num: usize) -> String {
    if is_stdout_tty() {
        format!("\x1b[36mmetta\x1b[97m[{}]\x1b[35m>\x1b[0m ", line_num)
    } else {
        format!("metta[{}]> ", line_num)
    }
}

/// Apply syntax highlighting to output text
fn highlight_output(text: &str, highlighter: Option<&QueryHighlighter>) -> String {
    if !is_stdout_tty() {
        return text.to_string();
    }
    match highlighter {
        Some(h) => {
            use rustyline::highlight::Highlighter;
            h.highlight(text, text.len()).to_string()
        }
        None => text.to_string(),
    }
}

fn run_repl(options: &Options) {
    println!("MeTTaTron REPL v{}", VERSION);
    println!("Enter MeTTa expressions. Type 'exit' or 'quit' to exit.");
    println!("Multi-line input: Press ENTER on incomplete expressions to continue.\n");

    // Create rustyline editor with MettaHelper
    let mut editor: Editor<MettaHelper, DefaultHistory> = Editor::new().unwrap();
    let helper = MettaHelper::new().expect("Failed to create MettaHelper");
    editor.set_helper(Some(helper));

    // Create output highlighter
    let output_highlighter = QueryHighlighter::new().ok();

    let mut env = Environment::default();

    // Configure strict mode if requested
    if options.strict_mode {
        env.set_strict_mode(true);
    }
    let mut line_num = 1;

    loop {
        let prompt = create_prompt(line_num);
        let readline = editor.readline(&prompt);

        match readline {
            Ok(input) => {
                let input = input.trim();

                if input == "exit" || input == "quit" {
                    println!("Goodbye!");
                    break;
                }

                if input.is_empty() {
                    continue;
                }

                // Add to history
                editor.add_history_entry(input).ok();

                // Add to helper's history for inline hints
                if let Some(helper) = editor.helper_mut() {
                    helper.add_to_history(input.to_string());
                }

                match compile(input) {
                    Ok(state) => {
                        env = env.union(&state.environment);

                        for sexpr in state.source {
                            // Only output results for S-expressions, not atoms or ground types
                            let should_output = matches!(sexpr.inner(), MettaValueInner::SExpr(_));

                            let (results, updated_env) = eval(sexpr.clone(), env.clone());
                            env = updated_env;

                            // Filter out Empty sentinels (HE-compatible: Empty is filtered at result collection)
                            let filtered_results: Vec<MettaValue> = results
                                .into_iter()
                                .filter(|v| !matches!(v.inner(), MettaValueInner::Empty))
                                .collect();

                            // Print results with syntax highlighting (only for S-expressions)
                            // HE-compatible: print [] for empty result sets
                            if should_output {
                                let output = format_results(&filtered_results);
                                let highlighted =
                                    highlight_output(&output, output_highlighter.as_ref());
                                println!("{}", highlighted);
                            }
                        }

                        // Update completions with newly defined functions
                        if let Some(helper) = editor.helper_mut() {
                            helper.update_from_environment(&env);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                    }
                }

                line_num += 1;
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("^D");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
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
        run_repl(&options);
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
