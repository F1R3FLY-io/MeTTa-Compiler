/// Rholang Integration Module - Evaluation Functions
///
/// **PRIMARY INTEGRATION**: Use `pathmap_par_integration` module for Rholang interop
///
/// This module provides:
/// 1. **JSON export** for debugging and inspection (`state_to_json`)
/// 2. **State evaluation** for REPL-style interaction (`run_state`, `run_state_async`)
/// 3. **Error handling** for safe compilation (`compile_safe`)
///
/// **Note**: For Rholang integration, use the PathMap Par functions in
/// `pathmap_par_integration` module, not the JSON functions here.
use crate::backend::fuzzy_match::FuzzyMatcher;
use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};
use std::sync::OnceLock;

#[allow(unused_imports)]
use tracing::{debug, error, info, instrument, trace, warn};

/// MeTTa built-in keywords for syntax-level "did you mean" suggestions
const METTA_KEYWORDS: &[&str] = &[
    // Special forms
    "=",
    "!",
    "quote",
    "if",
    "error",
    "is-error",
    "catch",
    "eval",
    "function",
    "return",
    "chain",
    "match",
    "case",
    "switch",
    "let",
    ":",
    "get-type",
    "check-type",
    "map-atom",
    "filter-atom",
    "foldl-atom",
    // Arithmetic operators
    "+",
    "-",
    "*",
    "/",
    // Comparison operators
    "<",
    "<=",
    ">",
    ">=",
    "==",
    "!=",
];

/// Get fuzzy matcher for MeTTa keywords (lazily initialized)
fn keyword_matcher() -> &'static FuzzyMatcher {
    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    MATCHER.get_or_init(|| FuzzyMatcher::from_terms(METTA_KEYWORDS.iter().copied()))
}

/// Safe compilation wrapper that never fails
///
/// This function wraps the `compile()` function and provides improved error handling
/// for Rholang integration. Instead of returning `Result<MettaState, String>`,
/// it always returns a `MettaState`:
/// - On success: Normal compiled state with parsed expressions
/// - On error: State containing an error s-expression: `(error "message")`
///
/// This allows Rholang contracts to handle syntax errors gracefully without
/// requiring complex error propagation through the Rholang runtime.
///
/// # Error Messages
///
/// The function improves upon Tree-Sitter's raw error messages by:
/// - Extracting line and column information
/// - Providing context about the error type
/// - Suggesting common fixes for known error patterns
///
/// # Example
///
/// ```ignore
/// // Valid MeTTa code
/// let state = compile_safe("(+ 1 2)");
/// assert_eq!(state.source().len(), 1);
///
/// // Invalid syntax - returns error s-expression
/// let state = compile_safe("(+ 1 2");  // Unclosed parenthesis
/// // state.source()[0] == (error "Syntax error at line 1, column 7: ...")
/// ```
pub fn compile_safe(src: &str) -> crate::backend::models::MettaState {
    use crate::backend::compile::compile;
    use crate::backend::models::{MettaState, MettaValueFactory};

    match compile(src) {
        Ok(state) => state,
        Err(error) => {
            // Improve error message with additional context
            let improved_msg = improve_error_message(&error);

            // Create error s-expression in storage arena: (error "message")
            let state = MettaState::new();
            let factory = state.factory();
            let error_sexpr = factory.sexpr(vec![
                factory.atom("error"),
                factory.string(&improved_msg),
            ]);
            state.source_mut().push(error_sexpr);
            state
        }
    }
}

/// Improve error messages with additional context and suggestions using pattern matching
fn improve_error_message(error: &SyntaxError) -> String {
    let base_msg = error.to_string();

    let hint = match &error.kind {
        SyntaxErrorKind::UnclosedDelimiter(c) => {
            Some(format!("check for missing closing '{}'", matching_close(*c)))
        }
        SyntaxErrorKind::ExtraClosingDelimiter(c) => {
            Some(format!("remove extra '{}' or add matching '{}'", c, matching_open(*c)))
        }
        SyntaxErrorKind::UnclosedString => Some("close the string with '\"'".into()),
        SyntaxErrorKind::InvalidEscape(seq) => {
            // Provide specific suggestions based on the invalid escape character
            let suggestion = match seq.chars().next() {
                Some('n') | Some('N') => "use \\n for newline",
                Some('t') | Some('T') => "use \\t for tab",
                Some('r') | Some('R') => "use \\r for carriage return",
                Some('0') => "use \\x00 for null byte (hex escape)",
                Some(c) if c.is_ascii_hexdigit() => "use \\x## format (two hex digits)",
                Some('u') | Some('U') => "use \\u{####} for Unicode codepoint",
                Some('b') | Some('B') => "use \\x08 for backspace (hex escape)",
                Some('f') | Some('F') => "use \\x0C for form feed (hex escape)",
                Some('e') | Some('E') => "use \\x1B for escape (hex escape)",
                _ => "valid escapes: \\n, \\t, \\r, \\\\, \\\", \\x## (hex), \\u{...} (unicode)",
            };
            Some(format!(
                "invalid escape \\{}. Hint: {}",
                seq, suggestion
            ))
        }
        SyntaxErrorKind::UnexpectedToken => {
            // Try to suggest similar keywords
            if !error.text.is_empty() {
                keyword_matcher().did_you_mean(&error.text, 1, 3)
            } else {
                None
            }
        }
        SyntaxErrorKind::UnknownNodeKind(kind) => {
            Some(format!(
                "Parser encountered unknown syntax '{}'. Check for typos or unsupported syntax.",
                kind
            ))
        }
        SyntaxErrorKind::ParserInit(msg) => {
            Some(format!(
                "Parser initialization failed: {}. This may indicate a corrupt grammar file or installation issue.",
                msg
            ))
        }
        SyntaxErrorKind::Generic => {
            Some("Check syntax near the indicated position. Common issues: unclosed parentheses, missing quotes, invalid escape sequences.".into())
        }
    };

    match hint {
        Some(h) => format!("{} (Hint: {})", base_msg, h),
        None => base_msg,
    }
}

/// Get the matching closing delimiter
fn matching_close(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        c => c,
    }
}

/// Get the matching opening delimiter
fn matching_open(close: char) -> char {
    match close {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        c => c,
    }
}

/// Convert MettaValue to a JSON-like string representation
/// Used for debugging and human-readable output
fn value_to_json_string(value: &MettaValue) -> String {
    use crate::backend::models::MettaValueInner;
    match value.inner() {
        MettaValueInner::Atom(s) => format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s)),
        MettaValueInner::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
        MettaValueInner::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
        MettaValueInner::Float(f) => format!(r#"{{"type":"number","value":{}}}"#, f),
        MettaValueInner::String(s) => {
            format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s))
        }
        MettaValueInner::Unit => r#"{"type":"unit"}"#.to_string(),
        MettaValueInner::SExpr(items) => {
            let items_json: Vec<String> = items.iter().map(value_to_json_string).collect();
            format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
        }
        MettaValueInner::Error(msg, details) => {
            format!(
                r#"{{"type":"error","message":"{}","details":{}}}"#,
                escape_json(msg),
                value_to_json_string(&details)
            )
        }
        MettaValueInner::Type(t) => {
            format!(
                r#"{{"type":"metatype","value":{}}}"#,
                value_to_json_string(&t)
            )
        }
        MettaValueInner::Conjunction(goals) => {
            let goals_json: Vec<String> = goals.iter().map(value_to_json_string).collect();
            format!(
                r#"{{"type":"conjunction","goals":[{}]}}"#,
                goals_json.join(",")
            )
        }
        MettaValueInner::Space(handle) => {
            format!(
                r#"{{"type":"space","id":{},"name":"{}"}}"#,
                handle.id,
                escape_json(&handle.name)
            )
        }
        MettaValueInner::State(id) => {
            format!(r#"{{"type":"state","id":{}}}"#, id)
        }
        MettaValueInner::Memo(handle) => {
            format!(
                r#"{{"type":"memo","id":{},"name":"{}"}}"#,
                handle.id,
                escape_json(&handle.name)
            )
        }
        MettaValueInner::Empty => r#"{"type":"empty"}"#.to_string(),
    }
}

/// Escape JSON special characters
fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

/// Convert MettaState to JSON representation for debugging
///
/// Returns a JSON string with the format:
/// ```json
/// {
///   "source": [...],
///   "output": [...]
/// }
/// ```
///
/// **Use Case**: Debugging, logging, inspection
/// **Not Recommended**: Rholang integration (use PathMap Par instead)
pub fn state_to_json(state: &MettaState) -> String {
    let source_json: Vec<String> = state
        .source()
        .iter()
        .map(value_to_json_string)
        .collect();

    let outputs_json: Vec<String> = state
        .output()
        .iter()
        .map(value_to_json_string)
        .collect();

    format!(
        r#"{{"source":[{}],"output":[{}]}}"#,
        source_json.join(","),
        outputs_json.join(",")
    )
}

/// Run compiled state against accumulated environment
///
/// This is the core evaluation function for REPL-style interaction.
///
/// Takes:
/// - `env`: Accumulated environment with rules/facts from previous calls
/// - `compiled_state`: MettaState with pending expressions to evaluate
///
/// Returns:
/// - Updated environment (merged with new rules/facts)
/// - Output values (only results from THIS invocation's `!` evaluations)
///
/// **Threading**: Synchronous, single-threaded evaluation
#[instrument(level = "info", skip(env, compiled_state))]
pub fn run_state(
    env: MettaEnvironment,
    compiled_state: &MettaState,
) -> Result<(MettaEnvironment, Vec<MettaValue>), String> {
    info!("Run state");

    let mut env = env;
    let mut outputs = Vec::new();

    let source = compiled_state.source();
    for &expr in source.iter() {
        let is_eval_expr = is_eval_expression(&expr);

        let guard = crate::backend::models::SessionGuard::enter();

        let (results, new_env) = eval(expr, env, compiled_state);
        env = new_env;

        // Consume results WHILE guard alive — values not yet released
        if is_eval_expr {
            outputs.extend(results);
        }

        // Drop guard triggers async release_session()
        drop(guard);
    }

    info!(
        output_count = outputs.len(),
        "Run state completed"
    );

    Ok((env, outputs))
}

/// Async version of run_state with parallel evaluation of independent expressions
///
/// This function parallelizes evaluation of consecutive `!` (eval) expressions
/// while maintaining sequential execution for rule definitions (`=`) to preserve
/// MeTTa semantics.
///
/// **MeTTa Semantics Preserved:**
/// - Rule definitions execute sequentially (environment threading)
/// - Independent eval expressions execute in parallel
/// - Output ordering is preserved
/// - Environment updates are atomic per batch
///
/// **Threading Model:** Uses Rayon for parallel evaluation
#[instrument(level = "info", skip(env, compiled_state))]
#[cfg(feature = "async")]
pub async fn run_state_async(
    env: MettaEnvironment,
    compiled_state: &MettaState,
) -> Result<(MettaEnvironment, Vec<MettaValue>), String> {
    use crate::backend::models::MettaValueInner;

    info!("Run state async");

    let mut env = env;
    let mut outputs = Vec::new();

    // Batch expressions into parallelizable groups
    let mut current_batch: Vec<(usize, MettaValue, bool)> = Vec::new();

    // Snapshot source expressions to avoid holding MutexGuard across await points
    let source_exprs: Vec<MettaValue> = compiled_state.source().iter().copied().collect();
    for (idx, &expr) in source_exprs.iter().enumerate() {
        let is_eval_expr = is_eval_expression(&expr);
        let is_rule_def = matches!(expr.inner(), MettaValueInner::SExpr(items)
            if items.len() >= 1 && matches!(items[0].inner(), MettaValueInner::Atom("=")));

        // Check if this is a ground fact (S-expression that's not a rule and not an eval)
        let is_ground_fact = expr.is_sexpr() && !is_rule_def && !is_eval_expr;

        // If this is a rule definition or ground fact and we have a batch, evaluate the batch first
        if (is_rule_def || is_ground_fact) && !current_batch.is_empty() {
            let batch_results = evaluate_batch_parallel_arena(
                current_batch, env.clone(),
            ).await;
            for (_batch_idx, results, should_output) in batch_results {
                if should_output {
                    outputs.extend(results);
                }
            }
            current_batch = Vec::new();
        }

        // If this is a rule definition or ground fact, execute it sequentially
        if is_rule_def || is_ground_fact {
            let (_results, new_env) = eval(expr, env, compiled_state);
            env = new_env;
        } else {
            current_batch.push((idx, expr, is_eval_expr));
        }
    }

    // Evaluate any remaining batch
    if !current_batch.is_empty() {
        let batch_results = evaluate_batch_parallel_arena(
            current_batch, env.clone(),
        ).await;
        for (_batch_idx, results, should_output) in batch_results {
            if should_output {
                outputs.extend(results);
            }
        }
    }

    info!(
        output_count = outputs.len(),
        "Run state async completed"
    );

    Ok((env, outputs))
}

/// Helper function to evaluate a batch of arena expressions in parallel.
/// Returns results in original order with their indices.
///
/// Uses Rayon by default for compatibility with Rholang's shared scheduler.
/// When `hybrid-p2-priority-scheduler` feature is enabled, uses the P2 priority
/// scheduler with P² runtime estimation for intelligent task scheduling.
#[cfg(all(feature = "async", feature = "hybrid-p2-priority-scheduler"))]
async fn evaluate_batch_parallel_arena(
    batch: Vec<(usize, MettaValue, bool)>,
    env: MettaEnvironment,
) -> Vec<(usize, Vec<MettaValue>, bool)> {
    use crate::backend::priority_scheduler::global_priority_eval_pool;

    debug!(
        batch_size = batch.len(),
        "Evaluate batch parallel arena (P2 scheduler)"
    );

    let pool = global_priority_eval_pool();

    // MettaValue is Copy+Send, MettaEnvironment is Clone+Send
    // StaticEvalContext uses thread-local leaked Bump arenas (one per thread),
    // so each task gets its own independent arena without sharing &MettaState.
    let receivers: Vec<_> = batch
        .into_iter()
        .map(|(idx, expr, should_output)| {
            let env = env.clone();
            pool.spawn(move || {
                // Track this parallel eval as active (prevents GC during evaluation)
                let _guard = crate::backend::models::EvalGuard::enter();
                // For parallel evaluation, use StaticEvalContext which provides
                // a thread-local leaked Bump arena per thread (Copy, Send-safe)
                use crate::backend::eval::trampoline::{eval_trampoline_generic, StaticEvalContext};
                let ctx = StaticEvalContext::get();
                let (results, _new_env) = eval_trampoline_generic(expr, env, &ctx);
                (idx, results, should_output)
            })
        })
        .collect();

    trace!(
        num_tasks = receivers.len(),
        "Tasks spawned on priority pool"
    );

    let mut results = Vec::with_capacity(receivers.len());
    for receiver in receivers {
        match receiver.recv() {
            Ok(result) => results.push(result),
            Err(e) => {
                error!(
                    text = %e,
                    "Parallel evaluation task failed"
                );
            }
        }
    }

    results.sort_by_key(|(idx, _, _)| *idx);
    results
}

/// Helper function to evaluate a batch of arena expressions in parallel.
/// Returns results in original order with their indices.
///
/// Uses Rayon's work-stealing thread pool for parallel evaluation.
#[cfg(all(feature = "async", not(feature = "hybrid-p2-priority-scheduler")))]
async fn evaluate_batch_parallel_arena(
    batch: Vec<(usize, MettaValue, bool)>,
    env: MettaEnvironment,
) -> Vec<(usize, Vec<MettaValue>, bool)> {
    use rayon::prelude::*;

    debug!(
        batch_size = batch.len(),
        "Evaluate batch parallel arena (Rayon)"
    );

    let mut results: Vec<_> = batch
        .into_par_iter()
        .map(|(idx, expr, should_output)| {
            // Track this parallel eval as active (prevents GC during evaluation)
            let _guard = crate::backend::models::EvalGuard::enter();
            // For parallel evaluation, use StaticEvalContext which provides
            // a thread-local leaked Bump arena per thread (Copy, Send-safe)
            use crate::backend::eval::trampoline::{eval_trampoline_generic, StaticEvalContext};
            let ctx = StaticEvalContext::get();
            let (results, _new_env) = eval_trampoline_generic(expr, env.clone(), &ctx);
            (idx, results, should_output)
        })
        .collect();

    results.sort_by_key(|(idx, _, _)| *idx);
    results
}

// ============================================================================
// Session-Based Arena Evaluation API
// ============================================================================

use crate::backend::compile::compile;
use crate::backend::eval::eval;
use crate::backend::eval::trampoline::{new_env, MettaEnvironment};
use crate::backend::models::{MettaState, MettaValue, MettaValueTrait};

/// Evaluate MeTTa source using session-based dual-arena allocation.
///
/// This function provides the high-level API for session-based evaluation with
/// O(1) bulk deallocation. It:
/// 1. Compiles source to MettaState (session-owned storage arena)
/// 2. Evaluates using thread-local eval arena for intermediates
/// 3. Returns results as strings (safe to use after MettaState drops)
///
/// ## Memory Model
///
/// - **Storage Arena**: Session-owned, freed O(1) when MettaState drops
/// - **Eval Arena**: Thread-local, reset lazily on generation change
/// - **Results**: Converted to owned strings before MettaState drops
///
/// ## When to Use
///
/// Use this API when:
/// - You need bounded memory usage (memory freed after each session)
/// - You want O(1) bulk deallocation instead of recursive tree traversal
/// - You're running many independent evaluations (arena pooling reduces allocation)
///
/// ## Example
///
/// ```ignore
/// // Each session has its own bounded memory
/// let results = eval_metta_session("!(+ 1 2)").unwrap();
/// assert_eq!(results, vec!["3"]);
///
/// // Memory is freed instantly (O(1)) after each session
/// for _ in 0..1000 {
///     let _ = eval_metta_session("!(* 6 7)").unwrap();
/// }
/// ```
///
/// # Arguments
///
/// - `src`: MeTTa source code to compile and evaluate
///
/// # Returns
///
/// A vector of result strings, or a syntax error.
#[instrument(level = "info", skip(src))]
pub fn eval_metta_session(src: &str) -> Result<Vec<String>, SyntaxError> {
    info!(
        line_count = src.lines().count(),
        "Evaluating MeTTa source using session arena"
    );

    // Compile to MettaState (acquires storage arena from pool)
    let state = compile(src)?;

    // Create arena environment (uses eval arena factory)
    let mut env = new_env();

    // Take source expressions (we'll iterate over them)
    let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();

    // Evaluate each source expression using arena evaluation with bytecode/JIT tiering
    for expr in source_exprs {
        let is_eval_expr = is_eval_expression(&expr);

        let guard = crate::backend::models::SessionGuard::enter();

        let (results, new_env) = eval(expr, env, &state);
        env = new_env;

        // Consume results WHILE guard alive — values not yet released
        if is_eval_expr {
            for result in &results {
                state.output_mut().push(*result);
            }
        }

        // Drop guard triggers async release_session()
        drop(guard);
    }

    // Convert results to strings BEFORE MettaState drops
    // This ensures we have owned data that survives the arena
    let output = state.output();
    let result_strings: Vec<String> = output
        .iter()
        .map(|v| v.friendly_repr())
        .collect();

    info!(result_count = result_strings.len(), "Session evaluation complete");

    // MettaState drops here: O(1) bulk deallocation
    // - Storage arena returned to pool (reset, not freed)
    // - Eval generation incremented (lazy reset of eval arenas)
    Ok(result_strings)
}

/// Check if an expression is an eval expression (! prefix).
fn is_eval_expression(expr: &MettaValue) -> bool {
    if let Some(items) = expr.as_sexpr() {
        if !items.is_empty() {
            if let Some(head) = items[0].as_atom() {
                return head == "!";
            }
        }
    }
    false
}

/// Evaluate MeTTa source using session-based allocation with raw MettaValue output.
///
/// Unlike `eval_metta_session()` which returns strings, this function returns
/// the MettaState containing the raw MettaValue results. The caller is responsible
/// for extracting results before the MettaState is dropped.
///
/// ## Warning
///
/// The returned MettaValue references become invalid after MettaState drops!
/// Always extract data you need before dropping the state.
///
/// ## Example
///
/// ```ignore
/// let state = eval_metta_session_raw("!(+ 1 2)").unwrap();
///
/// // Access results while state is alive
/// for result in state.output() {
///     println!("{}", result.friendly_repr());
/// }
///
/// // After this point, state is dropped and results are invalid
/// drop(state);
/// ```
#[instrument(level = "info", skip(src))]
pub fn eval_metta_session_raw(src: &str) -> Result<MettaState, SyntaxError> {
    info!(
        line_count = src.lines().count(),
        "Evaluating MeTTa source using session arena (raw output)"
    );

    // Compile to MettaState (acquires storage arena from pool)
    let state = compile(src)?;

    // Create arena environment (uses eval arena factory)
    let mut env = new_env();

    // Take source expressions (we'll iterate over them)
    let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();

    // Evaluate each source expression using arena evaluation with bytecode/JIT tiering
    for expr in source_exprs {
        let is_eval_expr = is_eval_expression(&expr);

        let guard = crate::backend::models::SessionGuard::enter();

        let (results, new_env) = eval(expr, env, &state);
        env = new_env;

        // Consume results WHILE guard alive — values not yet released
        if is_eval_expr {
            for result in &results {
                state.output_mut().push(*result);
            }
        }

        // Drop guard triggers async release_session()
        drop(guard);
    }

    info!(result_count = state.output().len(), "Session evaluation complete (raw)");

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_to_json() {
        let src = "(+ 1 2)";
        let state = compile(src).expect("compile failed");
        let json = state_to_json(&state);

        // Should return MettaState with source and output
        assert!(json.contains(r#""source""#));
        assert!(json.contains(r#""output""#));
        assert!(json.contains(r#""type":"sexpr""#));
    }

    #[test]
    fn test_value_atom_json() {
        use crate::backend::models::{global_factory, MettaValueFactory};
        let f = global_factory();
        let value = f.atom("test");
        let json = value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"atom","value":"test"}"#);
    }

    #[test]
    fn test_value_number_json() {
        use crate::backend::models::{global_factory, MettaValueFactory};
        let f = global_factory();
        let value = f.long(42);
        let json = value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"number","value":42}"#);
    }

    #[test]
    fn test_value_bool_json() {
        use crate::backend::models::{global_factory, MettaValueFactory};
        let f = global_factory();
        let value = f.bool(true);
        let json = value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"bool","value":true}"#);
    }

    #[test]
    fn test_value_string_json() {
        use crate::backend::models::{global_factory, MettaValueFactory};
        let f = global_factory();
        let value = f.string("hello");
        let json = value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"string","value":"hello"}"#);
    }

    #[test]
    fn test_value_unit_json() {
        use crate::backend::models::{global_factory, MettaValueFactory};
        let f = global_factory();
        let value = f.unit();
        let json = value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"unit"}"#);
    }

    #[test]
    fn test_value_sexpr_json() {
        use crate::backend::models::{global_factory, MettaValueFactory};
        let f = global_factory();
        let value = f.sexpr(vec![f.atom("+"), f.long(1), f.long(2)]);
        let json = value_to_json_string(&value);
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""items""#));
    }

    #[test]
    fn test_escape_json() {
        let escaped = escape_json("hello\n\"world\"\\test");
        assert_eq!(escaped, r#"hello\n\"world\"\\test"#);
    }

    #[test]
    fn test_compile_safe_success() {
        use crate::backend::models::MettaValueInner;
        let state = compile_safe("(+ 1 2)");
        let source = state.source();
        assert_eq!(source.len(), 1);
        // Should be a valid S-expression, not an error
        match source[0].inner() {
            MettaValueInner::SExpr(items) => {
                assert_eq!(items.len(), 3);
                match items[0].inner() {
                    MettaValueInner::Atom("+") => {}
                    other => panic!("Expected Atom('+'), got {:?}", other),
                }
            }
            _ => panic!("Expected SExpr for valid input"),
        }
    }

    #[test]
    fn test_compile_safe_syntax_error() {
        use crate::backend::models::MettaValueInner;
        let state = compile_safe("(+ 1 2");
        let source = state.source();
        assert_eq!(source.len(), 1);
        // Should be an error s-expression
        match source[0].inner() {
            MettaValueInner::SExpr(items) => {
                assert_eq!(items.len(), 2);
                match items[0].inner() {
                    MettaValueInner::Atom("error") => {}
                    other => panic!("Expected Atom('error'), got {:?}", other),
                }
                // Error message should be a string
                assert!(matches!(items[1].inner(), MettaValueInner::String(_)));
                // Error message should mention the syntax issue
                if let MettaValueInner::String(msg) = items[1].inner() {
                    assert!(msg.contains("Syntax error") || msg.contains("unexpected"));
                }
            }
            _ => panic!("Expected error s-expression for syntax error"),
        }
    }

    #[test]
    fn test_compile_safe_improves_error_message() {
        use crate::backend::models::MettaValueInner;
        let state = compile_safe("(+ 1 2");
        let source = state.source();
        match source[0].inner() {
            MettaValueInner::SExpr(items) => {
                if let MettaValueInner::String(msg) = items[1].inner() {
                    // Should include hint about unclosed parenthesis
                    assert!(msg.contains("Hint") && msg.contains("unclosed"));
                }
            }
            _ => panic!("Expected error s-expression"),
        }
    }

    #[test]
    fn test_run_state_simple() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile("!(+ 1 2)").expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
        match outputs[0].inner() {
            MettaValueInner::Long(3) => {}
            other => panic!("Expected Long(3), got {:?}", other),
        }
    }

    #[test]
    fn test_run_state_with_rules() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile(
            r#"
            (= (double $x) (* $x 2))
            !(double 21)
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
        match outputs[0].inner() {
            MettaValueInner::Long(42) => {}
            other => panic!("Expected Long(42), got {:?}", other),
        }
    }

    // Async Parallel Evaluation Tests
    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_run_state_async_simple() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile("!(+ 1 2)").expect("compile failed");

        let (_env, outputs) = run_state_async(env, &state).await.expect("run_state_async failed");

        // Should have output
        assert!(!outputs.is_empty());
        match outputs[0].inner() {
            MettaValueInner::Long(3) => {}
            other => panic!("Expected Long(3), got {:?}", other),
        }
    }

    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_run_state_async_parallel() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile(
            r#"
            !(+ 1 1)
            !(+ 2 2)
            !(+ 3 3)
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state_async(env, &state).await.expect("run_state_async failed");

        // Should have all outputs
        assert_eq!(outputs.len(), 3);
        match outputs[0].inner() {
            MettaValueInner::Long(2) => {}
            other => panic!("Expected Long(2), got {:?}", other),
        }
        match outputs[1].inner() {
            MettaValueInner::Long(4) => {}
            other => panic!("Expected Long(4), got {:?}", other),
        }
        match outputs[2].inner() {
            MettaValueInner::Long(6) => {}
            other => panic!("Expected Long(6), got {:?}", other),
        }
    }

    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_run_state_async_with_rules() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile(
            r#"
            (= (double $x) (* $x 2))
            !(double 5)
            !(double 10)
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state_async(env, &state).await.expect("run_state_async failed");

        // Should have outputs (parallel evaluation of both double calls)
        assert_eq!(outputs.len(), 2);
        match outputs[0].inner() {
            MettaValueInner::Long(10) => {}
            other => panic!("Expected Long(10), got {:?}", other),
        }
        match outputs[1].inner() {
            MettaValueInner::Long(20) => {}
            other => panic!("Expected Long(20), got {:?}", other),
        }
    }

    #[test]
    fn test_ground_facts_not_in_output() {
        // Regression test: verify ground facts are NOT added to output
        let mut env = new_env();

        // Add ground facts
        let state1 = compile("(connected room_a room_b) (connected room_b room_c)").expect("compile failed");
        let (new_env, outputs1) = run_state(env, &state1).expect("run_state failed");
        env = new_env;
        // Ground facts should NOT produce output
        assert_eq!(outputs1.len(), 0);

        // Verify ground facts are in environment (can be queried)
        let state2 = compile("!(match &self (connected $from $to) ($from $to))").expect("compile failed");
        let (_env, outputs2) = run_state(env, &state2).expect("run_state failed");
        // Now output should contain query results (2 matches)
        assert_eq!(outputs2.len(), 2);
    }

    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_run_state_async_multiple_rules_sequential() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile(
            r#"
            (= (square $x) (* $x $x))
            !(square 3)
            (= (double $x) (* $x 2))
            !(double 3)
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state_async(env, &state).await.expect("run_state_async failed");

        assert_eq!(outputs.len(), 2);
        match outputs[0].inner() {
            MettaValueInner::Long(9) => {}
            other => panic!("Expected Long(9), got {:?}", other),
        }
        match outputs[1].inner() {
            MettaValueInner::Long(6) => {}
            other => panic!("Expected Long(6), got {:?}", other),
        }
    }

    #[test]
    fn test_run_state_accumulated_state() {
        use crate::backend::models::MettaValueInner;
        // Test that rules persist across multiple run_state calls
        let env = new_env();
        let state1 = compile("(= (double $x) (* $x 2))").expect("compile failed");
        let (env, _outputs) = run_state(env, &state1).expect("run_state failed");

        let state2 = compile("!(double 5)").expect("compile failed");
        let (_env, outputs) = run_state(env, &state2).expect("run_state failed");

        assert!(!outputs.is_empty());
        match outputs[0].inner() {
            MettaValueInner::Long(10) => {}
            other => panic!("Expected Long(10), got {:?}", other),
        }
    }

    #[test]
    fn test_run_state_rule_ordering() {
        let env = new_env();
        let state = compile(
            r#"
            (= (f special-value) catched)
            (= (f $x) $x)
            !(f A)
            !(f special-value)
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should have outputs for both calls
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_complex_nested() {
        let env = new_env();
        let state = compile(
            r#"
            (= (triple $x) ($x $x $x))
            (= (grid3x3 $x) (triple (triple $x)))
            !(grid3x3 (square (+ 1 2)))
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_recursive_function() {
        let env = new_env();
        let state = compile(
            r#"
            (= (factorial 0) 1)
            (= (factorial $x) (* $x (factorial (- $x 1))))
            !(factorial 5)
            "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
        // Factorial of 5 should be 120
    }

    // Space Operations Tests - Adding Facts
    #[test]
    fn test_run_state_add_facts_to_space() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                !(+ 1 1)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Facts are added to space (no output), only eval expression produces output
        assert_eq!(outputs.len(), 1);
        match outputs[0].inner() {
            MettaValueInner::Long(2) => {}
            other => panic!("Expected Long(2), got {:?}", other),
        }
    }

    #[test]
    fn test_run_state_facts_persist_across_runs() {
        // First run: add facts
        let env = new_env();
        let state1 = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                "#,
        )
        .expect("compile failed");
        let (env, _outputs) = run_state(env, &state1).expect("run_state failed");

        // Second run: use facts via rules
        let state2 = compile(
            r#"
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .expect("compile failed");
        let (_env, outputs) = run_state(env, &state2).expect("run_state failed");

        // Should be able to query the facts
        assert!(!outputs.is_empty());
    }

    // Pattern Matching and Queries
    #[test]
    fn test_run_state_simple_pattern_match() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (= (get-parents $child)
                   (match &self (Parent $parent $child) $parent))
                !(get-parents Bob)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should find parents of Bob
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_pattern_match_with_variables() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Tom Liz)
                (= (find-parents $parent)
                   (match &self (Parent $parent $child) ($parent $child)))
                !(find-parents Tom)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should find all children of Tom
        assert!(!outputs.is_empty());
    }

    // Family Relationship Tests
    #[test]
    fn test_run_state_family_relationships() {
        let env = new_env();
        let state = compile(
            r#"
                (parent Tom Bob)
                (parent Pam Bob)
                (parent Bob Ann)
                (parent Bob Pat)
                (female Pam)
                (female Ann)
                (male Tom)
                (male Bob)
                (= (grandparent $gp $gc)
                   (match &self (parent $gp $p)
                          (match &self (parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_recursive_ancestor_relation() {
        let env = new_env();
        let state = compile(
            r#"
                (parent Tom Bob)
                (parent Bob Ann)
                (parent Ann Sara)
                (= (ancestor $a $d)
                   (match &self (parent $a $d) True))
                (= (ancestor $a $d)
                   (match &self (parent $a $p)
                          (ancestor $p $d)))
                !(ancestor Tom Sara)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should find that Tom is an ancestor of Sara
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_complex_family_query() {
        let env = new_env();
        let state = compile(
            r#"
                (parent Tom Bob)
                (parent Pam Bob)
                (parent Bob Ann)
                (parent Bob Pat)
                (parent Pat Jim)
                (female Pam)
                (female Ann)
                (male Tom)
                (= (sibling $s1 $s2)
                   (match &self (parent $p $s1)
                          (match &self (parent $p $s2)
                                 (if (== $s1 $s2) (empty) True))))
                !(sibling Ann Pat)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
    }

    // Constraint Solving Tests
    #[test]
    fn test_run_state_nondeterministic_choice() {
        use crate::backend::models::MettaValueInner;
        let env = new_env();
        let state = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                !(small-digit)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should produce multiple results (nondeterministic)
        assert!(!outputs.is_empty());
        // All results should be valid digits
        for output in &outputs {
            if let MettaValueInner::Long(n) = output.inner() {
                assert!(*n >= 1 && *n <= 3);
            }
        }
    }

    #[test]
    fn test_run_state_constraint_solving_pair() {
        let env = new_env();
        let state = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                (= (not-equal $x $y)
                   (if (== $x $y) (empty) True))
                (= (solve-pair)
                   (let $x (small-digit)
                        (let $y (small-digit)
                             (if (not-equal $x $y)
                                 ($x $y)
                                 (empty)))))
                !(solve-pair)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should produce pairs where x != y
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_constraint_solving_triple() {
        let env = new_env();
        let state = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                (= (not-equal $x $y)
                   (if (== $x $y) (empty) True))
                (= (solve-triple)
                   (let $x (small-digit)
                        (if (== $x 1)
                            (let $y (small-digit)
                                 (if (not-equal $x $y)
                                     (let $z (small-digit)
                                          (if (and (not-equal $x $z) (not-equal $y $z))
                                              ($x $y $z)
                                              (empty)))
                                     (empty)))
                            (let $y (small-digit)
                                 (if (not-equal $x $y)
                                     ($x $y 1)
                                     (empty))))))
                !(solve-triple)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should produce valid triples with constraints
        assert!(!outputs.is_empty());
    }

    // Knowledge Base Operations
    #[test]
    fn test_run_state_entity_relations() {
        let env = new_env();
        let state = compile(
            r#"
                (works alice acme)
                (works bob beta)
                (friends alice carol)
                (located acme SF)
                (located beta NYC)
                (= (find-colleagues $person)
                   (match &self (works $person $company)
                          (match &self (works $other $company)
                                 (if (== $person $other) (empty) $other))))
                !(find-colleagues alice)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_complex_pattern_matching() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Tom Liz)
                (Parent Bob Ann)
                (= (get-parent-entries $parent $child)
                   (match &self (Parent $parent $child) (Parent $parent $child)))
                !(get-parent-entries Tom $child)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should find all children of Tom
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_nested_queries() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (Likes Bob Ann)
                (= (find-liked-grandchildren $grandparent)
                   (match &self (Parent $grandparent $parent)
                          (match &self (Parent $parent $child)
                                 (match &self (Likes $parent $child) $child))))
                !(find-liked-grandchildren Tom)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_rule_with_multiple_matches() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (Parent Bob Pat)
                (= (get-all-children $parent)
                   (match &self (Parent $parent $child) $child))
                !(get-all-children Bob)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Should find both Ann and Pat
        assert!(!outputs.is_empty());
    }

    // Async tests for space operations
    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_run_state_async_add_facts_then_query() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state_async(env, &state).await.expect("run_state_async failed");

        assert!(!outputs.is_empty());
    }

    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_ground_facts_not_in_output_async() {
        // Regression test: verify ground facts are NOT added to output (async version)
        let mut env = new_env();

        // Add ground facts
        let state1 = compile("(connected room_a room_b) (connected room_b room_c)").expect("compile failed");
        let (new_env, outputs1) = run_state_async(env, &state1).await.expect("run_state_async failed");
        env = new_env;
        // Ground facts should NOT produce output
        assert_eq!(outputs1.len(), 0);

        // Verify ground facts are in environment (can be queried)
        let state2 = compile("!(match &self (connected $from $to) ($from $to))").expect("compile failed");
        let (_env, outputs2) = run_state_async(env, &state2).await.expect("run_state_async failed");
        // Now output should contain query results (2 matches)
        assert_eq!(outputs2.len(), 2);
    }

    #[tokio::test]
    #[cfg(feature = "async")]
    async fn test_run_state_async_parallel_queries() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (= (get-parents $child)
                   (match &self (Parent $parent $child) $parent))
                !(get-parents Bob)
                !(get-parents Ann)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state_async(env, &state).await.expect("run_state_async failed");

        // Both queries should execute in parallel
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_run_state_facts_only_no_output() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        // Facts are added to space but produce no output
        assert_eq!(outputs.len(), 0);
    }

    #[test]
    fn test_run_state_mixed_facts_and_rules() {
        let env = new_env();
        let state = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .expect("compile failed");

        let (_env, outputs) = run_state(env, &state).expect("run_state failed");

        assert!(!outputs.is_empty());
    }

    // Tests for improve_error_message with different SyntaxErrorKind variants

    #[test]
    fn test_improve_error_message_unclosed_paren() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::UnclosedDelimiter('('),
            line: 1,
            column: 7,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"));
        assert!(msg.contains("missing closing ')'"));
    }

    #[test]
    fn test_improve_error_message_extra_close_paren() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::ExtraClosingDelimiter(')'),
            line: 1,
            column: 8,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"));
        assert!(msg.contains("remove extra ')'") || msg.contains("add matching '('"));
    }

    #[test]
    fn test_improve_error_message_unclosed_string() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::UnclosedString,
            line: 1,
            column: 10,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"));
        assert!(msg.contains("close the string"));
    }

    #[test]
    fn test_improve_error_message_invalid_escape() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::InvalidEscape("z".to_string()),
            line: 1,
            column: 5,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"));
        assert!(msg.contains("valid escapes"));
    }

    #[test]
    fn test_improve_error_message_generic_has_hint() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::Generic,
            line: 1,
            column: 1,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        // Generic errors now have a helpful hint
        assert!(msg.contains("Hint"), "Expected 'Hint' in: {}", msg);
        assert!(
            msg.contains("Common issues"),
            "Expected 'Common issues' in: {}",
            msg
        );
    }

    #[test]
    fn test_improve_error_message_unclosed_bracket() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::UnclosedDelimiter('['),
            line: 1,
            column: 5,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"));
        assert!(msg.contains("missing closing ']'"));
    }

    #[test]
    fn test_improve_error_message_unclosed_brace() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::UnclosedDelimiter('{'),
            line: 1,
            column: 5,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"));
        assert!(msg.contains("missing closing '}'"));
    }

    #[test]
    fn test_keyword_suggestion_quota_to_quote() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        // "quota" is close to "quote"
        let error = SyntaxError {
            kind: SyntaxErrorKind::UnexpectedToken,
            line: 1,
            column: 1,
            text: "quota".to_string(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(
            msg.contains("Did you mean"),
            "Expected suggestion in: {}",
            msg
        );
        assert!(msg.contains("quote"), "Expected 'quote' in: {}", msg);
    }

    #[test]
    fn test_keyword_suggestion_iff_to_if() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        // "iff" is close to "if"
        let error = SyntaxError {
            kind: SyntaxErrorKind::UnexpectedToken,
            line: 1,
            column: 1,
            text: "iff".to_string(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(
            msg.contains("Did you mean"),
            "Expected suggestion in: {}",
            msg
        );
        assert!(msg.contains("if"), "Expected 'if' in: {}", msg);
    }

    #[test]
    fn test_keyword_suggestion_no_match() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        // "xyzzy" is not close to any keyword
        let error = SyntaxError {
            kind: SyntaxErrorKind::UnexpectedToken,
            line: 1,
            column: 1,
            text: "xyzzy".to_string(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        // Should not contain "Did you mean" when no similar keyword
        assert!(
            !msg.contains("Did you mean"),
            "Unexpected suggestion in: {}",
            msg
        );
    }

    #[test]
    fn test_keyword_suggestion_empty_text() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        // Empty text should not produce a suggestion
        let error = SyntaxError {
            kind: SyntaxErrorKind::UnexpectedToken,
            line: 1,
            column: 1,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(
            !msg.contains("Did you mean"),
            "Unexpected suggestion for empty text: {}",
            msg
        );
    }

    #[test]
    fn test_improve_error_message_unknown_node_kind() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::UnknownNodeKind("weird_node".to_string()),
            line: 1,
            column: 5,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"), "Expected 'Hint' in: {}", msg);
        assert!(
            msg.contains("weird_node"),
            "Expected node kind name in: {}",
            msg
        );
        assert!(
            msg.contains("unknown syntax") || msg.contains("unsupported"),
            "Expected helpful message in: {}",
            msg
        );
    }

    #[test]
    fn test_improve_error_message_parser_init() {
        use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind};

        let error = SyntaxError {
            kind: SyntaxErrorKind::ParserInit("failed to load grammar".to_string()),
            line: 0,
            column: 0,
            text: String::new(),
            file_path: None,
        };
        let msg = improve_error_message(&error);
        assert!(msg.contains("Hint"), "Expected 'Hint' in: {}", msg);
        assert!(
            msg.contains("failed to load grammar"),
            "Expected original message in: {}",
            msg
        );
        assert!(
            msg.contains("initialization") || msg.contains("grammar file"),
            "Expected helpful context in: {}",
            msg
        );
    }
}
