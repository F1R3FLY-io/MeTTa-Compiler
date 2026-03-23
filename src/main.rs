/// MeTTaTron - MeTTa Evaluator CLI
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::process;
use std::time::{Duration, Instant};

use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::history::DefaultHistory;
use rustyline::Editor;

use mettatron::backend::*;
use mettatron::backend::models::ValueView;
use mettatron::repl::{MettaHelper, QueryHighlighter};

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
    eprintln!("    --no-gc                 Disable garbage collection");
    #[cfg(feature = "track-stats")]
    eprintln!("    --gc-stats              Print GC statistics to stderr on exit
    --tier-stats            Print tiered compilation stats to stderr on exit
    --pool-stats            Print thread pool statistics to stderr on exit");
    eprintln!("    --startup-timing        Print per-phase startup timing to stderr");
    #[cfg(feature = "eval-trace")]
    eprintln!("    --trace <FILE>          Write binary evaluation trace to FILE");
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
    no_gc: bool,
    #[cfg(feature = "track-stats")]
    gc_stats: bool,
    #[cfg(feature = "track-stats")]
    tier_stats: bool,
    #[cfg(feature = "track-stats")]
    pool_stats: bool,
    startup_timing: bool,
    #[cfg(feature = "eval-trace")]
    trace_output: Option<String>,
}

fn parse_args() -> Result<Options, String> {
    let args: Vec<String> = env::args().collect();

    let mut input = None;
    let mut output = None;
    let mut show_sexpr = false;
    let mut repl_mode = false;
    let mut strict_mode = false;
    let mut no_gc = false;
    #[cfg(feature = "track-stats")]
    let mut gc_stats = false;
    #[cfg(feature = "track-stats")]
    let mut tier_stats = false;
    #[cfg(feature = "track-stats")]
    let mut pool_stats = false;
    let mut startup_timing = false;
    #[cfg(feature = "eval-trace")]
    let mut trace_output: Option<String> = None;
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
            "--no-gc" => {
                no_gc = true;
            }
            #[cfg(feature = "track-stats")]
            "--gc-stats" => {
                gc_stats = true;
            }
            #[cfg(feature = "track-stats")]
            "--tier-stats" => {
                tier_stats = true;
            }
            #[cfg(feature = "track-stats")]
            "--pool-stats" => {
                pool_stats = true;
            }
            "--startup-timing" => {
                startup_timing = true;
            }
            #[cfg(feature = "eval-trace")]
            "--trace" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing trace file after --trace".to_string());
                }
                trace_output = Some(args[i].clone());
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
        no_gc,
        #[cfg(feature = "track-stats")]
        gc_stats,
        #[cfg(feature = "track-stats")]
        tier_stats,
        #[cfg(feature = "track-stats")]
        pool_stats,
        startup_timing,
        #[cfg(feature = "eval-trace")]
        trace_output,
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

/// Format an MettaValue result for display.
fn format_result(value: &MettaValue) -> String {
    match value.view() {
        ValueView::Bool(b) => b.to_string(),
        ValueView::Long(n) => n.to_string(),
        ValueView::Float(f) => f.to_string(),
        ValueView::Unit => "()".to_string(),
        ValueView::Empty => "Empty".to_string(),
        ValueView::Atom(s) => s.to_string(),
        ValueView::String(s) => format!("\"{}\"", s),
        ValueView::Error(msg, details) => {
            format!("(Error {} {})", msg, format_result(&details))
        }
        ValueView::Type(t) => format!("Type({})", format_result(&t)),
        ValueView::SExpr(items) => {
            let formatted: Vec<String> = items.iter().map(format_result).collect();
            format!("({})", formatted.join(" "))
        }
        ValueView::Conjunction(goals) => {
            let formatted: Vec<String> = goals.iter().map(format_result).collect();
            format!("(, {})", formatted.join(" "))
        }
        ValueView::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        ValueView::State(id) => format!("(State {})", id),
        ValueView::Quoted(inner) => format!("(quote {})", format_result(&inner)),
        ValueView::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
    }
}

fn format_results(results: &[MettaValue]) -> String {
    if results.is_empty() {
        return "[]".to_string();
    }
    let formatted: Vec<String> = results.iter().map(format_result).collect();
    format!("[{}]", formatted.join(", "))
}

/// Collects per-phase wall-clock timings during startup.
/// Each `mark()` records (name, cumulative elapsed time from t_start).
struct StartupTimings {
    t_start: Instant,
    phases: Vec<(&'static str, Duration)>,
}

impl StartupTimings {
    fn new(t_start: Instant) -> Self {
        Self {
            t_start,
            phases: Vec::with_capacity(12),
        }
    }

    /// Record the cumulative elapsed time for a named phase.
    fn mark(&mut self, name: &'static str) {
        self.phases.push((name, self.t_start.elapsed()));
    }

    /// Print the timing table to stderr.
    fn print(&self) {
        eprintln!();
        eprintln!("=== MeTTaTron Startup Timing ===");
        eprintln!("{:<24} {:>10}   {:>10}", "Phase", "Wall (ms)", "Delta (ms)");
        eprintln!("{}", "\u{2500}".repeat(50));

        let mut prev = Duration::ZERO;
        for &(name, cumulative) in &self.phases {
            let delta = cumulative.saturating_sub(prev);
            eprintln!(
                "{:<24} {:>10.3}   {:>10.3}",
                name,
                cumulative.as_secs_f64() * 1000.0,
                delta.as_secs_f64() * 1000.0,
            );
            prev = cumulative;
        }
        eprintln!("{}", "\u{2500}".repeat(50));
        if let Some(&(_, total)) = self.phases.last() {
            eprintln!("Total wall time:         {:>10.3} ms", total.as_secs_f64() * 1000.0);
        }
        eprintln!();
    }
}

fn eval_metta(input: &str, options: &Options, timings: &mut StartupTimings) -> Result<String, String> {
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

    // Common setup: file path for error messages
    let file_path = options
        .input
        .as_ref()
        .filter(|p| *p != "-")
        .map(|s| s.as_str());

    // Create arena environment (uses eval arena factory)
    let mut env = new_env();
    timings.mark("new_env");

    // Set the current module path for relative includes
    if let Some(ref input_path) = options.input {
        if input_path != "-" {
            let path = Path::new(input_path);
            if let Ok(canonical) = path.canonicalize() {
                if let Some(parent) = canonical.parent() {
                    env.set_current_module_path(Some(parent.to_path_buf()));
                }
            } else if let Some(parent) = path.parent() {
                env.set_current_module_path(Some(parent.to_path_buf()));
            }
        }
    }

    // Configure strict mode if requested
    if options.strict_mode {
        env.set_strict_mode(true);
    }

    // Compile to MettaState (acquires storage arena from pool)
    let state = compile_with_path(input, file_path)
        .map_err(|e| e.to_string())?;
    timings.mark("compile");

    // Create trace collector if --trace was specified (eval-trace feature only).
    #[cfg(feature = "eval-trace")]
    let trace_collector = {
        match &options.trace_output {
            Some(trace_path) => {
                let source_name = file_path.unwrap_or("<stdin>");
                let collector = mettatron::trace::TraceCollector::new(trace_path, source_name)
                    .map_err(|e| format!("Failed to create trace file '{}': {}", trace_path, e))?;
                Some(collector)
            }
            None => None,
        }
    };

    // Snapshot source expressions (MettaValue is Copy)
    let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();

    // Evaluate each expression using arena evaluation with bytecode/JIT tiering.
    // Each expression gets its own SessionGuard — values allocated during eval
    // are tagged with the session's context ID and released asynchronously on
    // a background thread when the guard drops (after results are formatted).
    let mut output = String::new();
    let mut first_eval_marked = false;
    for expr in source_exprs {
        // Only output results for S-expressions, not atoms or ground types
        let should_output = expr.is_sexpr();

        let guard = SessionGuard::enter();

        // Use trace-aware eval when a trace collector is active.
        #[cfg(feature = "eval-trace")]
        let (results, new_env) = {
            if let Some(ref collector) = trace_collector {
                mettatron::eval_with_trace(expr, env, &state, collector)
            } else {
                eval(expr, env, &state)
            }
        };
        #[cfg(not(feature = "eval-trace"))]
        let (results, new_env) = eval(expr, env, &state);

        env = new_env;

        if !first_eval_marked {
            timings.mark("first_eval");
            first_eval_marked = true;
        }

        // Format results WHILE guard is alive — values are not yet released.
        let filtered_results: Vec<MettaValue> = results
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect();

        if should_output {
            output.push_str(&format!("{}\n", format_results(&filtered_results)));
        }

        // Drop guard triggers async release_session() on background thread
        drop(guard);
    }
    timings.mark("all_evals");

    // ── I-16: AAM static analysis pipeline (opt-in) ──
    // Run after all rules are loaded and top-level expressions evaluated.
    // Enabled via METTATRON_AAM_ANALYSIS=1 environment variable.
    if std::env::var("METTATRON_AAM_ANALYSIS").map_or(false, |v| v == "1") {
        // Activate I-13/I-14 hot-path tracking for subsequent evaluations.
        mettatron::backend::activate_analysis();
        let source_exprs_snapshot: Vec<mettatron::backend::models::MettaValue> =
            state.source().iter().copied().collect();

        let analysis_config = mettatron::backend::analysis::AnalysisConfig::default();
        let analysis_result = mettatron::backend::analysis::fixpoint::run_analysis(
            &source_exprs_snapshot, &env, &analysis_config,
        );
        let derived = mettatron::backend::analysis::derived::derive_analysis(&analysis_result);

        // Report analysis results to stderr
        eprintln!("[analysis] AAM converged in {} iterations ({} ms)",
            analysis_result.iterations, analysis_result.analysis_time_ms);
        eprintln!("[analysis] Dead rules: {}, Deterministic dispatches: {}, Pure exprs: {}",
            derived.dead_rules.len(), derived.deterministic_dispatch.len(), derived.pure_expressions.len());

        // I-16: Run post-pass analyses
        let env_snapshot = mettatron::backend::analysis::fixpoint::snapshot_environment(&env);

        let pushdown_config = mettatron::backend::analysis::pushdown::PushdownConfig::default();
        let pushdown_result = mettatron::backend::analysis::pushdown::run_pushdown_analysis(
            &source_exprs_snapshot, &env_snapshot, &pushdown_config,
        );
        eprintln!("[analysis] Pushdown: {} states, {} recursive exprs, converged={}",
            pushdown_result.states.len(), pushdown_result.recursive_exprs.len(), pushdown_result.converged);

        let module_reach = mettatron::backend::analysis::module_dce::ModuleReachability::compute(
            std::collections::HashMap::new(), // Module→rule mapping not yet wired
            &derived.dead_rules,
        );
        eprintln!("[analysis] Module reachability: {} live, {} dead",
            module_reach.live_modules.len(), module_reach.dead_modules.len());

        let race_result = mettatron::backend::analysis::race_detection::detect_races(
            &analysis_result, &derived,
        );
        eprintln!("[analysis] Race detection: {} potential races, {} safe parallel exprs",
            race_result.potential_races.len(), race_result.safe_parallel.len());

        // I-12: Install rule filter from analysis for subsequent evaluations.
        // The CompressedRuleFilter is used by match_rules_native to skip dead rules.
        let rule_filter = mettatron::backend::eval::cesk::continuation_compression::CompressedRuleFilter::from_analysis(
            &derived, env_snapshot.total_rules,
        );
        mettatron::backend::eval::cesk::continuation_compression::install_rule_filter(rule_filter);

        // Build and install the WFST/WPDS scheduler automaton from analysis results.
        // This provides automata-based scheduling with expression-aware priority,
        // context-dependent weight refinement, and wavefront parallelism.
        let scheduler_automaton = mettatron::backend::scheduler::aam_builder::build_scheduler_automaton(
            &derived,
            Some(mettatron::backend::models::work_pool::global_eval_pool().runtime_tracker()),
        );
        let scheduler_hints_count = derived.scheduler_hints.len();
        match mettatron::backend::scheduler::install_scheduler(scheduler_automaton) {
            Ok(()) => {
                eprintln!("[analysis] WFST scheduler automaton installed ({} expression hints)",
                    scheduler_hints_count);
            }
            Err(_) => {
                eprintln!("[analysis] WFST scheduler automaton already installed (using existing)");
            }
        }

        timings.mark("analysis");
    }

    // Finalize trace collector — flush remaining events and write footer.
    #[cfg(feature = "eval-trace")]
    {
        if let Some(collector) = trace_collector {
            match collector.finalize() {
                Ok(event_count) => {
                    eprintln!(
                        "[trace] Wrote {} events to '{}'",
                        event_count,
                        options.trace_output.as_deref().unwrap_or("?"),
                    );
                }
                Err(e) => {
                    eprintln!("[trace] Failed to finalize trace file: {}", e);
                }
            }
        }
    }

    // MettaState drops here — values remain in global slab allocator
    // and will be reclaimed by GC when no longer referenced.
    drop(state);
    drop(env);
    timings.mark("total_output");

    Ok(output)
}

/// Check if stdout is a TTY (for conditional color output)
fn is_stdout_tty() -> bool {
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

    let mut env = new_env();

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
                        // Snapshot source expressions (MettaValue is Copy)
                        let source_exprs: Vec<MettaValue> =
                            state.source().iter().copied().collect();

                        for expr in source_exprs {
                            // Only output results for S-expressions, not atoms or ground types
                            let should_output = expr.is_sexpr();

                            let guard = SessionGuard::enter();

                            let (results, updated_env) = eval(expr, env, &state);
                            env = updated_env;

                            // Format results WHILE guard is alive — values not yet released.
                            let filtered_results: Vec<MettaValue> = results
                                .into_iter()
                                .filter(|v| !v.is_empty())
                                .collect();

                            if should_output {
                                let output = format_results(&filtered_results);
                                let highlighted =
                                    highlight_output(&output, output_highlighter.as_ref());
                                println!("{}", highlighted);
                            }

                            // Drop guard triggers async release_session()
                            drop(guard);
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
    let t_start = Instant::now();
    let mut timings = StartupTimings::new(t_start);

    // Limit glibc per-thread malloc arenas. Since jemalloc is the global
    // allocator (via PathMap), glibc's arenas are only used internally by
    // pthread_getattr_np during thread creation. Without this limit, each
    // new thread reserves 64 MB of virtual address space for a glibc arena,
    // wasting ~2+ GB of virtual memory with many threads.
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        const M_ARENA_MAX: std::ffi::c_int = -8;
        // SAFETY: mallopt is thread-safe and called before any threads are spawned.
        unsafe {
            mallopt(M_ARENA_MAX, 2);
        }
    }
    timings.mark("mallopt");

    // Install signal-triggered diagnostic handlers (SIGTERM/SIGUSR1) early.
    // Also auto-installed by global_allocator(), but explicit call ensures
    // coverage even if main() fails before first allocation.
    mettatron::backend::diagnostics::install_signal_handlers();
    timings.mark("signal_handlers");

    // Eagerly initialize thread pools — workers spawn asynchronously in background.
    // Pools also self-initialize on first access, so this just starts it sooner.
    mettatron::init_thread_pools();
    timings.mark("thread_pools");

    let options = match parse_args() {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };
    timings.mark("arg_parsing");

    // Disable GC if requested
    if options.no_gc {
        disable_gc();
    }

    // REPL mode
    if options.repl_mode {
        run_repl(&options);
        #[cfg(feature = "track-stats")]
        {
            if options.gc_stats {
                mettatron::backend::diagnostics::print_gc_stats();
            }
            if options.tier_stats {
                mettatron::backend::diagnostics::print_tier_stats();
            }
            if options.pool_stats {
                mettatron::backend::diagnostics::print_pool_stats();
            }
        }
        if options.startup_timing {
            timings.print();
        }
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
    timings.mark("file_read");

    let output = match eval_metta(&input_content, &options, &mut timings) {
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

    #[cfg(feature = "track-stats")]
    {
        if options.gc_stats {
            mettatron::backend::diagnostics::print_gc_stats();
        }
        if options.tier_stats {
            mettatron::backend::diagnostics::print_tier_stats();
        }
        if options.pool_stats {
            mettatron::backend::diagnostics::print_pool_stats();
        }
    }
    if options.startup_timing {
        timings.print();
    }
}
