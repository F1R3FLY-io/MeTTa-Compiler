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

use mettatron::backend::eval::tier_forced::{
    eval_with_tier, FallbackPolicy, TierEvalOutcome, TierSelection,
};
use mettatron::backend::models::metta_value::float_canonical;
use mettatron::backend::models::ValueView;
use mettatron::backend::*;
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
    eprintln!(
        "    --gc-stats              Print GC statistics to stderr on exit
    --tier-stats            Print tiered compilation stats to stderr on exit
    --pool-stats            Print thread pool statistics to stderr on exit"
    );
    eprintln!("    --startup-timing        Print per-phase startup timing to stderr");
    eprintln!(
        "    --tier <T>              Force evaluation tier (T = 0|1|2|3|treewalker|bytecode|jit1|jit2|auto)
    --on-tier-unavailable <P>  Policy when tier is not applicable: silent-demote|strict (default: silent-demote)
    --cross-tier-check         Run input on all applicable tiers and diff results"
    );
    #[cfg(feature = "trace")]
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
    #[cfg(feature = "trace")]
    trace_output: Option<String>,
    /// Forced evaluation tier (default: Auto = current dispatch behavior).
    tier: TierSelection,
    /// Policy when the requested tier is not applicable for an input.
    on_tier_unavailable: FallbackPolicy,
    /// Run input on all applicable tiers and diff results.
    cross_tier_check: bool,
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
    #[cfg(feature = "trace")]
    let mut trace_output: Option<String> = None;
    let mut tier = TierSelection::Auto;
    let mut on_tier_unavailable = FallbackPolicy::SilentDemote;
    let mut cross_tier_check = false;
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
            "--tier" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing tier specifier after --tier".to_string());
                }
                tier = TierSelection::from_cli(&args[i])?;
            }
            "--on-tier-unavailable" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing policy after --on-tier-unavailable".to_string());
                }
                on_tier_unavailable = match args[i].as_str() {
                    "silent-demote" | "demote" | "fallback" => FallbackPolicy::SilentDemote,
                    "strict" | "error" | "strict-no-fallback" => FallbackPolicy::StrictNoFallback,
                    other => {
                        return Err(format!(
                            "unknown --on-tier-unavailable policy '{}'; expected: silent-demote|strict",
                            other
                        ));
                    }
                };
            }
            "--cross-tier-check" => {
                cross_tier_check = true;
            }
            #[cfg(feature = "trace")]
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
        #[cfg(feature = "trace")]
        trace_output,
        tier,
        on_tier_unavailable,
        cross_tier_check,
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

/// S15a parse-string-escape (2026-05-15): re-escape a raw string literal's
/// inner content so the printed form round-trips through the lexer's
/// `parse_string`. HE only escapes `\\` and `\"` (other control characters
/// are emitted verbatim per §A.5); we mirror that subset.
fn format_string_escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Format an MettaValue result for display.
fn format_result(value: &MettaValue) -> String {
    match value.view() {
        ValueView::Bool(b) => {
            if b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        ValueView::Long(n) => n.to_string(),
        ValueView::Float(f) => float_canonical(f),
        ValueView::Unit => "()".to_string(),
        ValueView::Empty => "Empty".to_string(),
        ValueView::NotReducible => "NotReducible".to_string(),
        ValueView::Atom(s) => s.to_string(),
        ValueView::String(s) => format_string_escaped(s),
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

/// Convert a `TierEvalOutcome` into the `(results, env)` tuple produced by
/// the auto-dispatch `eval()`. Exits with code 4 if the requested tier was
/// not applicable and the user requested strict mode (`--on-tier-unavailable strict`).
fn outcome_to_results(
    outcome: TierEvalOutcome,
    requested: TierSelection,
) -> (
    smallvec::SmallVec<[MettaValue; 2]>,
    mettatron::backend::eval::MettaEnvironment,
) {
    match outcome {
        TierEvalOutcome::Ok { results, env, .. } => (results.into_iter().collect(), env),
        TierEvalOutcome::Demoted { results, env, .. } => (results.into_iter().collect(), env),
        TierEvalOutcome::NotApplicable { reason } => {
            eprintln!(
                "Error: tier {} is not applicable for this input: {:?}",
                requested.label(),
                reason
            );
            process::exit(4);
        }
    }
}

/// Canonicalize a result list for cross-tier comparison.
///
/// Per spec §20.1.2 reduction-set semantics, observation comparison is
/// **multiset** — sort lexicographically before diffing so iteration-order
/// differences between tiers don't surface as false positives. Floats use
/// `float_canonical` for bit-equal comparison (per spec §14.2.2 / H4).
fn canonicalize_results(results: &[MettaValue]) -> Vec<String> {
    let mut s: Vec<String> = results
        .iter()
        .filter(|v| !v.is_empty())
        .map(format_result)
        .collect();
    s.sort();
    s
}

/// Run a single expression through every applicable evaluation tier and
/// report divergences. Returns `(formatted_output, mismatch_detected)`.
///
/// For each expression:
///   1. Evaluate on T0 (always applicable) — produces the canonical
///      observation.
///   2. For each of T1 / T2 / T3, if `tier_applicable` returns Ok, evaluate
///      and compare the canonicalized multiset against T0's.
///   3. Print per-tier PASS / MISMATCH and (on mismatch) the diff.
///
/// Returns `true` (in the second tuple field) if any cross-tier mismatch
/// was observed. Caller exits with code 3 when this is true.
fn run_cross_tier_check(
    expr: MettaValue,
    env: mettatron::backend::eval::MettaEnvironment,
    state: &mettatron::backend::models::MettaState,
) -> (
    smallvec::SmallVec<[MettaValue; 2]>,
    mettatron::backend::eval::MettaEnvironment,
    bool,
) {
    use mettatron::backend::eval::tier_forced::{tier_applicable, FallbackPolicy};

    // 1. T0 — canonical observation.
    let t0_outcome = eval_with_tier(
        expr,
        env.clone(),
        state,
        TierSelection::Treewalker,
        FallbackPolicy::SilentDemote,
    );
    let (t0_results, t0_env) = match t0_outcome {
        TierEvalOutcome::Ok { results, env, .. } | TierEvalOutcome::Demoted { results, env, .. } => {
            (results, env)
        }
        TierEvalOutcome::NotApplicable { reason } => {
            eprintln!("Internal error: T0 should always be applicable; got {:?}", reason);
            process::exit(5);
        }
    };
    let t0_canonical = canonicalize_results(&t0_results);

    // 2. Higher tiers — only compare when applicable.
    let mut mismatch = false;
    for tier in [
        TierSelection::Bytecode,
        TierSelection::JitStage1,
        TierSelection::JitStage2,
    ] {
        match tier_applicable(&expr, &env, tier) {
            Ok(()) => {
                let outcome = eval_with_tier(
                    expr,
                    env.clone(),
                    state,
                    tier,
                    FallbackPolicy::SilentDemote,
                );
                match outcome {
                    TierEvalOutcome::Ok { results, .. }
                    | TierEvalOutcome::Demoted { results, .. } => {
                        let tier_canonical = canonicalize_results(&results);
                        if tier_canonical != t0_canonical {
                            mismatch = true;
                            eprintln!(
                                "[cross-tier] MISMATCH on {}:\n  T0:        {:?}\n  {}: {:?}",
                                tier.label(),
                                t0_canonical,
                                tier.label(),
                                tier_canonical
                            );
                        }
                    }
                    TierEvalOutcome::NotApplicable { reason } => {
                        eprintln!(
                            "[cross-tier] {} skipped: {:?}",
                            tier.label(),
                            reason
                        );
                    }
                }
            }
            Err(reason) => {
                // Tier carves itself out — not a divergence.
                eprintln!("[cross-tier] {} not applicable: {:?}", tier.label(), reason);
            }
        }
    }

    (t0_results.into_iter().collect(), t0_env, mismatch)
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
            eprintln!(
                "Total wall time:         {:>10.3} ms",
                total.as_secs_f64() * 1000.0
            );
        }
        eprintln!();
    }
}

fn eval_metta(
    input: &str,
    options: &Options,
    timings: &mut StartupTimings,
) -> Result<String, String> {
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
    let state = compile_with_path(input, file_path).map_err(|e| e.to_string())?;
    timings.mark("compile");

    // Create trace collector if --trace was specified (trace feature only).
    #[cfg(feature = "trace")]
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
    // BUG-T0-009 / cross-tier-check: tracks whether any per-expression
    // cross-tier divergence was detected. The caller exits with code 3 if
    // so. Initialized to false; only `run_cross_tier_check` may set it.
    let mut cross_tier_mismatch_detected = false;
    for expr in source_exprs {
        // S1 TOPLEVEL (2026-05-13): HE two-mode runner — only `(! expr)`
        // directives emit observable output lines. Bare top-level
        // S-exprs are ADD-mode silent side-effects (HE
        // MettaRunnerMode::ADD). Mirrors mtt_conformance.rs runner so
        // the CLI shows the same multiset that conformance fixtures
        // verify. See spec §S1.
        //
        // S16 (2026-05-15): HE empirical behavior — confirmed via
        // decomposed metta-repl probes against HE commit 3f76dc46
        // (`hyperon-experimental/lib/src/metta/runner/mod.rs:1028-1117`,
        // `RunContext::step()`) — emits exactly ONE bracketed `[…]`
        // line per `!` directive. Non-`!` top-level forms (decls like
        // `(= …)` / `(: …)`, side-effect ops like `(pragma! …)`,
        // bare ground atoms) run in ADD mode and emit NOTHING.
        // MTT matches HE-actual. Spec-side fixture inconsistencies
        // (46 fixtures that encoded an aspirational "decl emits []"
        // style) were resolved lockstep in metta-specification
        // (2026-05-15) by removing the spurious `- atoms: []`
        // entries — HE-Kernel +15, HE-Core +31, HE-Full +38.
        let is_bang = expr
            .as_sexpr()
            .and_then(|items| items.first())
            .and_then(|h| h.as_atom())
            .is_some_and(|s| s == "!");
        let should_output = is_bang;

        let guard = SessionGuard::enter();
        // Hold ACTIVE_EVALUATORS > 0 across eval+format to prevent
        // session-release GC from freeing result values between
        // eval() returning (EvalGuard drops) and formatting.
        let gc_hold = GcHoldGuard::enter();

        // Use trace-aware eval when a trace collector is active.
        // Tier-forced execution goes through eval_with_tier; Auto delegates to eval().
        // --cross-tier-check runs through all applicable tiers and reports divergences.
        #[cfg(feature = "trace")]
        let (results, new_env) = {
            if let Some(ref collector) = trace_collector {
                mettatron::eval_with_trace(expr, env, &state, collector)
            } else if options.cross_tier_check {
                let (results, env, mismatch) = run_cross_tier_check(expr, env, &state);
                if mismatch {
                    cross_tier_mismatch_detected = true;
                }
                (results, env)
            } else if matches!(options.tier, TierSelection::Auto) {
                eval(expr, env, &state)
            } else {
                let outcome = eval_with_tier(
                    expr,
                    env,
                    &state,
                    options.tier,
                    options.on_tier_unavailable,
                );
                outcome_to_results(outcome, options.tier)
            }
        };
        #[cfg(not(feature = "trace"))]
        let (results, new_env) = if options.cross_tier_check {
            let (results, env, mismatch) = run_cross_tier_check(expr, env, &state);
            if mismatch {
                cross_tier_mismatch_detected = true;
            }
            (results, env)
        } else if matches!(options.tier, TierSelection::Auto) {
            eval(expr, env, &state)
        } else {
            let outcome = eval_with_tier(
                expr,
                env,
                &state,
                options.tier,
                options.on_tier_unavailable,
            );
            outcome_to_results(outcome, options.tier)
        };

        env = new_env;

        if !first_eval_marked {
            timings.mark("first_eval");
            first_eval_marked = true;
        }

        // Format results WHILE guard is alive — values are not yet released.
        let filtered_results: Vec<MettaValue> =
            results.into_iter().filter(|v| !v.is_empty()).collect();

        // Root results against GC between eval() and format_results().
        //
        // After eval() returns, EvalGuard is dropped (ACTIVE_EVALUATORS == 0).
        // A pending SessionRelease from a PREVIOUS iteration may be waiting for
        // quiescence on a GC pool worker thread. When ACTIVE_EVALUATORS hits 0,
        // that worker wakes, traces the surviving set, and frees values from the
        // previous session that are NOT in the surviving set.
        //
        // Problem: thread-local caches (EVAL_MEMO, MATCH_RESULT_CACHE, subgoal
        // table, thunk table) may hold MettaValues from the previous session.
        // If the current eval() returned cached values from those caches, the
        // results contain MettaValues with the previous session's context_id.
        // Those values are NOT in the surviving set (trace_surviving_set() cannot
        // access thread-local caches from the worker thread), so the session
        // release frees them. When format_results() dereferences the freed slab
        // slots, ASAN reports use-after-poison.
        //
        // Fix: register the results as temporary safepoint roots. The session
        // release worker merges safepoint roots into the surviving set via
        // trace_safepoint_live_set(), so registered values are promoted to
        // persistent (context_id=0) instead of being freed.
        let _result_roots = if !filtered_results.is_empty() {
            Some(register_temporary_roots(filtered_results.clone()))
        } else {
            None
        };

        // S2 BANG-WORD (2026-05-13): emit one observable line ONLY for
        // `!` directives. Bare top-level S-exprs are HE ADD-mode silent
        // side-effecting facts — they produce no `[...]` line at all per
        // HE's runner semantics (`hyperon-experimental/lib/src/metta/runner/
        // mod.rs:1076-1109` — INTERPRET mode emits, ADD mode does not).
        //
        // The conformance harness (verification/conformance_harness/
        // runner.py:71-95) counts `[...]` lines as per-directive results.
        // Emitting `[]` for ADD-mode directives inflates the directive
        // count and breaks `len(cleaned_directives) == len(expected_results)`
        // for fixtures like T01/005 where the source mixes ADD and `!`.
        if should_output {
            output.push_str(&format!("{}\n", format_results(&filtered_results)));
        }

        // Drop order: result_roots first (unregister temporary roots),
        // then gc_hold (allow session-release GC to run),
        // then guard (enqueue async release_session).
        drop(_result_roots);
        drop(gc_hold);
        drop(guard);
    }
    timings.mark("all_evals");

    // --cross-tier-check: exit with code 3 on any cross-tier divergence.
    // Print output first (so the user sees BOTH the program's normal output
    // and the divergence diagnostics on stderr).
    if cross_tier_mismatch_detected {
        if !output.is_empty() {
            print!("{}", output);
        }
        eprintln!("[cross-tier] FAIL: one or more expressions diverged across tiers");
        process::exit(3);
    }

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
            &source_exprs_snapshot,
            &env,
            &analysis_config,
        );
        let derived = mettatron::backend::analysis::derived::derive_analysis(&analysis_result);

        // Report analysis results to stderr
        eprintln!(
            "[analysis] AAM converged in {} iterations ({} ms)",
            analysis_result.iterations, analysis_result.analysis_time_ms
        );
        eprintln!(
            "[analysis] Dead rules: {}, Deterministic dispatches: {}, Pure exprs: {}",
            derived.dead_rules.len(),
            derived.deterministic_dispatch.len(),
            derived.pure_expressions.len()
        );

        // I-16: Run post-pass analyses
        let env_snapshot = mettatron::backend::analysis::fixpoint::snapshot_environment(&env);

        let pushdown_config = mettatron::backend::analysis::pushdown::PushdownConfig::default();
        let pushdown_result = mettatron::backend::analysis::pushdown::run_pushdown_analysis(
            &source_exprs_snapshot,
            &env_snapshot,
            &pushdown_config,
        );
        eprintln!(
            "[analysis] Pushdown: {} states, {} recursive exprs, converged={}",
            pushdown_result.states.len(),
            pushdown_result.recursive_exprs.len(),
            pushdown_result.converged
        );

        let module_reach = mettatron::backend::analysis::module_dce::ModuleReachability::compute(
            std::collections::HashMap::new(), // Module→rule mapping not yet wired
            &derived.dead_rules,
        );
        eprintln!(
            "[analysis] Module reachability: {} live, {} dead",
            module_reach.live_modules.len(),
            module_reach.dead_modules.len()
        );

        let race_result =
            mettatron::backend::analysis::race_detection::detect_races(&analysis_result, &derived);
        eprintln!(
            "[analysis] Race detection: {} potential races, {} safe parallel exprs",
            race_result.potential_races.len(),
            race_result.safe_parallel.len()
        );

        // I-12: Install rule filter from analysis for subsequent evaluations.
        // The CompressedRuleFilter is used by match_rules_native to skip dead rules.
        let rule_filter = mettatron::backend::eval::cesk::continuation_compression::CompressedRuleFilter::from_analysis(
            &derived, env_snapshot.total_rules,
        );
        mettatron::backend::eval::cesk::continuation_compression::install_rule_filter(rule_filter);

        // Build and install the WFST/WPDS scheduler automaton from analysis results.
        // This provides automata-based scheduling with expression-aware priority,
        // context-dependent weight refinement, and wavefront parallelism.
        let scheduler_automaton =
            mettatron::backend::scheduler::aam_builder::build_scheduler_automaton(
                &derived,
                Some(mettatron::backend::models::work_pool::global_eval_pool().runtime_tracker()),
            );
        let scheduler_hints_count = derived.scheduler_hints.len();
        match mettatron::backend::scheduler::install_scheduler(scheduler_automaton) {
            Ok(()) => {
                eprintln!(
                    "[analysis] WFST scheduler automaton installed ({} expression hints)",
                    scheduler_hints_count
                );
            }
            Err(_) => {
                eprintln!("[analysis] WFST scheduler automaton already installed (using existing)");
            }
        }

        timings.mark("analysis");
    }

    // Finalize trace collector — flush remaining events and write footer.
    #[cfg(feature = "trace")]
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
        Some(h) => h.highlight(text, text.len()).to_string(),
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
                            let gc_hold = GcHoldGuard::enter();

                            let (results, updated_env) = eval(expr, env, &state);
                            env = updated_env;

                            // Format results WHILE guard is alive — values not yet released.
                            let filtered_results: Vec<MettaValue> =
                                results.into_iter().filter(|v| !v.is_empty()).collect();

                            // Root results against GC between eval() and formatting.
                            // See the comment in eval_metta() for the full race description:
                            // session release from a prior iteration can free cached values
                            // that are part of the current results.
                            let _result_roots = if !filtered_results.is_empty() {
                                Some(register_temporary_roots(filtered_results.clone()))
                            } else {
                                None
                            };

                            if should_output {
                                let output = format_results(&filtered_results);
                                let highlighted =
                                    highlight_output(&output, output_highlighter.as_ref());
                                println!("{}", highlighted);
                            }

                            drop(_result_roots);
                            drop(gc_hold);
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
    mettatron::backend::interrupt::install_signal_handler();
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

    mettatron::backend::interrupt::reset();
    let output = match eval_metta(&input_content, &options, &mut timings) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    if mettatron::backend::interrupt::is_interrupted() {
        process::exit(130);
    }

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
