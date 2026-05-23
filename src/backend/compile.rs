// Compile function: MeTTa text → PathMap structure
//
// The compile function parses MeTTa source code and produces a PathMap structure
// containing [parsed_sexprs, fact_db] where:
// - parsed_sexprs: List of s-expressions as nested lists preserving original operator symbols
// - fact_db: PathMap instance representing the fact database (initially empty)
//
// Operator symbols like +, -, * are preserved as-is (not normalized to add, sub, mul)

#[cfg(test)]
use crate::backend::models::MettaValueInner;
use crate::backend::models::{
    global_factory, MettaState, MettaValue, MettaValueFactory, MettaValueTrait,
};
use crate::ir::MettaExpr;
use crate::parser::{MettaParser, ValueEmitter};
use crate::tree_sitter_parser::SyntaxError;

use tracing::{debug, error, info, instrument};

// ============================================================================
// S2 BANG-WORD: top-level `(Atom("!"), SExpr(...))` pair folding
// ============================================================================

/// S2 BANG-WORD (2026-05-13): fold consecutive top-level `Atom("!")` followed
/// by any S-expression into a single `(! sexpr)` form. Mirrors HE's runner
/// directive semantics: `!` is a runner-level eval sigil, NOT a parser unary
/// operator. Per the new parser behavior (S2 Part A), `!(expr)` produces two
/// separate top-level tokens `Atom("!")` and `SExpr(...)`; this fold pairs
/// them in compile order.
///
/// Operates only at the TOP LEVEL of the parsed source. Nested occurrences
/// of `!` inside lists are unaffected (e.g., `(!bar)` parses as a 1-element
/// list `[Atom("!bar")]` directly from the parser; `(! foo)` with whitespace
/// parses as `[Atom("!"), Atom("foo")]` and stays that way — the fold does
/// not descend into S-expressions).
///
/// The constructed `(! body)` wrapper preserves source spans for IDE /
/// debugger support: the outer wrapper's span covers from the `!` atom's
/// start through the body's end. Without this, downstream `is_spanned()`
/// queries (`backend/compile.rs::test_compile_span_on_prefix_operator`,
/// language-server hover, error messages) would silently lose location
/// information when the parser-level `!` is paired with its body.
///
/// Type parameters mirror the rest of the compile pipeline:
/// - `V`: any value type implementing `MettaValueTrait`
/// - `F`: factory for constructing the wrapping `(! sexpr)`
pub fn fold_bang_pairs_generic<V, F>(values: Vec<V>, factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    if values.len() < 2 {
        return values;
    }

    // Preallocate at worst-case capacity (no pairs fold). When pairs do fold,
    // the vec will be smaller than capacity — acceptable trade-off vs. a two
    // pass approach (one to count, one to fold).
    let mut out: Vec<V> = Vec::with_capacity(values.len());
    let mut iter = values.into_iter().peekable();
    while let Some(v) = iter.next() {
        // Match a bare `Atom("!")` followed by ANY value. Per S2, the
        // adjacency rule pairs `!` with the next token regardless of its
        // kind so that `! 42` and `! "x"` (degenerate but legal forms) still
        // get the same INTERPRET-mode wrapping as `!(expr)`. HE behaves the
        // same: any token immediately following a bare `!` is treated as
        // the directive's body.
        let is_bare_bang = v.as_atom() == Some("!");
        if is_bare_bang && iter.peek().is_some() {
            // Capture the `!` atom's span before consuming the body so the
            // wrapper can synthesize a covering span.
            let bang_span = v.span().copied();
            let body = iter.next().expect("peek().is_some() guarantees next()");
            let body_span = body.span().copied();
            // Construct `(! body)` as an SExpr with two items. Reuse `v`
            // for the head (it already carries the `!` atom's span via the
            // emit_atom call in the parser).
            let wrapped_inner = factory.sexpr(vec![v, body]);
            // Synthesize the wrapper span from the bang start through the
            // body end. If either side lacks a span (degenerate factory
            // construction), fall back to no span on the wrapper.
            let wrapped = match (bang_span, body_span) {
                (Some(bs), Some(es)) => factory.spanned(
                    wrapped_inner,
                    crate::ir::Span::new(bs.start, es.end),
                ),
                _ => wrapped_inner,
            };
            out.push(wrapped);
        } else {
            // HE-faithful: top-level atoms starting with `!` (e.g. `!foo`)
            // are single symbols per HE's parser word-rule. The runner
            // (eval mode dispatch) uses EXACT-equality `atom == EXEC_SYMBOL`
            // — `!foo` does NOT trigger force-eval, it stays as a stored
            // symbol in ADD mode. See `hyperon-experimental/lib/src/metta/
            // text.rs:638` and `runner/mod.rs:1072`. Do NOT split here.
            out.push(v);
        }
    }
    out
}

/// MettaExpr variant of `fold_bang_pairs_generic` for the IR cold path.
///
/// The IR uses `MettaExpr::Atom(String, _)` and `MettaExpr::List(Vec<_>, _)`
/// instead of factory-backed values; we re-implement the same fold rather
/// than generalize over the emitter (the trait would need an introspection
/// method, which would force a public API expansion). The two implementations
/// are kept in lock-step by the parser unit-tests and by `parse_to_ir`'s
/// integration test coverage.
///
/// Preserves source spans: when both `!` and body carry spans, the wrapper
/// list's span covers `bang.start..body.end`.
pub fn fold_bang_pairs_expr(exprs: Vec<MettaExpr>) -> Vec<MettaExpr> {
    if exprs.len() < 2 {
        return exprs;
    }
    let mut out: Vec<MettaExpr> = Vec::with_capacity(exprs.len());
    let mut iter = exprs.into_iter().peekable();
    while let Some(e) = iter.next() {
        let is_bare_bang = matches!(&e, MettaExpr::Atom(s, _) if s == "!");
        if is_bare_bang && iter.peek().is_some() {
            let bang_span = e.span();
            let body = iter.next().expect("peek().is_some() guarantees next()");
            let body_span = body.span();
            // Wrap span covers `(!` through end of body when available.
            let wrapper_span = match (bang_span, body_span) {
                (Some(bs), Some(es)) => Some(crate::ir::Span::new(bs.start, es.end)),
                _ => None,
            };
            out.push(MettaExpr::List(vec![e, body], wrapper_span));
        } else {
            // HE-faithful: do NOT split `!`-prefixed atoms. See note in
            // fold_bang_pairs_generic above.
            out.push(e);
        }
    }
    out
}

// ============================================================================
// Generic Compilation - Zero-Conversion Support
// ============================================================================

/// Convert a MettaExpr to any value type using a factory.
///
/// This generic function enables zero-conversion compilation by delegating
/// value construction to the provided factory (e.g., `GcFactory`).
pub fn expr_to_value_generic<V, F>(expr: &MettaExpr, factory: &F) -> Result<V, String>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    /// Optionally wrap a value in Spanned if a span is present.
    #[inline]
    fn maybe_spanned<V, F>(factory: &F, value: V, span: &Option<crate::ir::Span>) -> V
    where
        V: MettaValueTrait + Clone,
        F: MettaValueFactory<V>,
    {
        match span {
            Some(s) => factory.spanned(value, *s),
            None => value,
        }
    }

    match expr {
        MettaExpr::Atom(s, span) => {
            // Parse literals (MeTTa uses capitalized True/False per hyperon-experimental)
            let value = match s.as_str() {
                "True" => factory.bool(true),
                "False" => factory.bool(false),
                _ => factory.atom(s),
            };
            Ok(maybe_spanned(factory, value, span))
        }
        MettaExpr::String(s, span) => Ok(maybe_spanned(factory, factory.string(s), span)),
        MettaExpr::Integer(n, span) => Ok(maybe_spanned(factory, factory.long(*n), span)),
        MettaExpr::Float(f, span) => Ok(maybe_spanned(factory, factory.float(*f), span)),
        MettaExpr::List(items, span) => {
            if items.is_empty() {
                // HE-compatible: () is an empty S-expression, not unit
                Ok(maybe_spanned(factory, factory.sexpr(vec![]), span))
            } else {
                // NOTE: Comma expressions (, expr1 expr2 ...) are kept as
                // regular S-expressions to match MeTTa HE semantics, where
                // (,) is an inert symbol. Previously these were converted to
                // a Conjunction variant at compile time, but that broke
                // pattern matching (e.g., (cons , $args) in PLN's Direct.metta)
                // and diverged from HE's behavior of returning the expression
                // unchanged.
                {
                    // Check for (quote expr) → Quoted(expr) variant
                    let is_quote = items.len() == 2
                        && matches!(&items[0], MettaExpr::Atom(s, _) if s == "quote");

                    if is_quote {
                        let inner = expr_to_value_generic(&items[1], factory)?;
                        Ok(maybe_spanned(factory, factory.quote(inner), span))
                    } else {
                        // Regular S-expression
                        let mut values = Vec::with_capacity(items.len());
                        for e in items {
                            values.push(expr_to_value_generic(e, factory)?);
                        }
                        Ok(maybe_spanned(factory, factory.sexpr(values), span))
                    }
                }
            }
        }
    }
}

/// Compile MeTTa source code to a generic value type.
///
/// This is the zero-conversion compile function that works with any value type
/// implementing `MettaValueTrait`. It returns a vector of parsed expressions.
///
/// # Type Parameters
///
/// - `V`: The value type (e.g., `MettaValue`)
/// - `F`: The factory type for constructing values
///
/// # Arguments
///
/// - `src`: The MeTTa source code to compile
/// - `factory`: The factory for constructing values
///
/// # Returns
///
/// A vector of compiled expressions in the target value type, or a syntax error.
#[instrument(level = "info", skip(src, factory))]
pub fn compile_generic<V, F>(src: &str, factory: &F) -> Result<Vec<V>, SyntaxError>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    info!(
        line_count = src.lines().count(),
        char_count = src.chars().count(),
        "Compiling MeTTa source (generic)"
    );

    // Parse directly to values using the custom parser with ValueEmitter.
    // This eliminates the tree-sitter C FFI, CST allocation, IR intermediate,
    // and the expr_to_value_generic conversion pass.
    let mut parser = MettaParser::new(src);
    let mut emitter = ValueEmitter::new(factory);
    let values = parser.parse_all(&mut emitter).map_err(|e| {
        error!(
            kind = ?e.kind,
            text = %e,
            "Syntax error from parsing MeTTa source code"
        );
        debug!(src, %e);
        e
    })?;

    // S2 BANG-WORD (2026-05-13): pair top-level bare `!` atoms with their
    // following S-expression argument. The parser emits them as separate
    // tokens (HE word-vs-sigil semantics); the runner directive form
    // `(! expr)` is reconstructed here for downstream INTERPRET-mode dispatch.
    let values = fold_bang_pairs_generic(values, factory);

    info!(expr_count = values.len(), "Generic compilation successful");

    Ok(values)
}

// ============================================================================
// Arena Compilation - Session-Based Dual-Arena Pipeline
// ============================================================================

/// Compile MeTTa source code to an MettaState for session-based evaluation.
///
/// This function creates a session-scoped MettaState, compiles the source
/// expressions into the session's storage arena, and returns the populated
/// state ready for evaluation with `eval` or `eval_trampoline`.
///
/// # Arguments
///
/// - `src`: The MeTTa source code to compile
///
/// # Returns
///
/// An `MettaState` containing the compiled expressions in its storage arena,
/// or a syntax error.
///
/// ## Dual-Arena Model
///
/// The returned `MettaState` owns a session-scoped storage arena:
/// - Source expressions are allocated in the storage arena
/// - O(1) bulk deallocation when MettaState is dropped
/// - Efficient arena pooling reduces allocation overhead
///
/// During evaluation:
/// - **Storage Arena** (session-owned): Holds source, rules, bindings, results
/// - **Eval Arena** (thread-local): Holds intermediates, reset between sessions
///
/// # Example
///
/// ```ignore
/// use mettatron::backend::compile::compile;
/// use mettatron::backend::eval::{eval, trampoline::new_env};
///
/// // Compile to MettaState
/// let state = compile("!(+ 1 2)").unwrap();
///
/// // Create arena environment
/// let mut env = new_env();
///
/// // Evaluate - zero conversions throughout
/// for &expr in state.source() {
///     let (results, new_env) = eval(expr, env, &state);
///     env = new_env;
/// }
///
/// // O(1) bulk deallocation on drop
/// drop(state);
/// ```
#[instrument(level = "info", skip(src))]
pub fn compile(src: &str) -> Result<MettaState, SyntaxError> {
    info!(
        line_count = src.lines().count(),
        char_count = src.chars().count(),
        "Compiling MeTTa source to MettaState"
    );

    // Parse first using global factory — no GC root provider yet, so no
    // contention with GC thread's collect_roots() during population.
    let factory = global_factory();

    // Parse directly to MettaValue using the custom parser with ValueEmitter.
    // This eliminates the tree-sitter C FFI, CST allocation, IR intermediate,
    // and the expr_to_value_generic conversion pass.
    let mut parser = MettaParser::new(src);
    let mut emitter = ValueEmitter::new(&factory);
    let values = parser.parse_all(&mut emitter).map_err(|e| {
        error!(
            kind = ?e.kind,
            text = %e,
            "Syntax error from parsing MeTTa source code"
        );
        debug!(src, %e);
        e
    })?;

    // S2 BANG-WORD (2026-05-13): pair top-level bare `!` atoms with their
    // following S-expression argument. See `fold_bang_pairs_generic` for the
    // full HE-bisimilarity rationale.
    let values = fold_bang_pairs_generic(values, &factory);

    info!(
        expr_count = values.len(),
        "MettaState compilation successful"
    );

    // Create MettaState with fully-populated source — GC registration happens
    // once with the complete Vec, never contended during population.
    let state = MettaState::new_compiled(values);

    Ok(state)
}

/// Compile MeTTa source code to MettaState with a file path for error reporting.
///
/// Like `compile`, but includes the file path in any syntax error messages.
pub fn compile_with_path(src: &str, file_path: Option<&str>) -> Result<MettaState, SyntaxError> {
    compile(src).map_err(|e| match file_path {
        Some(path) => e.with_file_path(path),
        None => e,
    })
}

/// Helper function to create an error value.
///
/// HE-bisimilar shape: `Error(offending_expr, "msg")`. The message is wrapped
/// as a `String` value; the offending expression is the first slot.
pub fn make_error(msg: &str, offending: MettaValue) -> MettaValue {
    let factory = crate::backend::models::global_factory();
    MettaValue::Error(offending, factory.string(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::eval::eval;
    use crate::backend::eval::trampoline::{eval_trampoline, new_env};

    #[test]
    fn test_compile_empty_input() {
        let result = compile("");
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.source().len(), 0);
    }

    #[test]
    fn test_compile_simple() {
        let src = "(+ 1 2)";
        let result = compile(src);
        assert!(result.is_ok());

        let state = result.unwrap();
        assert_eq!(state.source().len(), 1);
        // Environment is empty at compile time (facts added during eval)
        assert_eq!(state.environment.rule_count(), 0);
        assert!(state.output().is_empty());

        // Should be: (+ 1 2) - operator symbol preserved
        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom("+".to_string()));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_compile_multiple_expressions() {
        let src = "(+ 1 2) (* 3 4)";
        let state = compile(src).unwrap();
        assert_eq!(state.source().len(), 2);
    }

    #[test]
    fn test_compile_operators() {
        // Operators should be preserved as-is (not normalized)
        let operators = vec![
            ("+", "+"),
            ("-", "-"),
            ("*", "*"),
            ("/", "/"),
            ("<", "<"),
            ("<=", "<="),
            ("==", "=="),
        ];

        for (op, expected) in operators {
            let src = format!("({} 1 2)", op);
            let state = compile(&src).unwrap();
            if let MettaValueInner::SExpr(items) = {
                let s = state.source();
                s[0]
            }
            .inner()
            {
                assert_eq!(
                    items[0],
                    MettaValue::Atom(expected.to_string()),
                    "Failed for operator {}",
                    op
                );
            }
        }
    }

    #[test]
    fn test_compile_gt() {
        // Test > operator - should be preserved as-is
        let src = "(> 1 2)";
        let state = compile(src).unwrap();
        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items[0], MettaValue::Atom(">".to_string()));
        }

        // Note: >= is tokenized by the lexer as two separate tokens: Symbol(">") and Equals
        // This would need to be fixed in sexpr.rs to handle >= as a single operator
        // For now, >= is not supported as a single operator
    }

    #[test]
    fn test_compile_negative_numbers() {
        let src = "(+ -5 -10)";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items[0], MettaValue::Atom("+".to_string()));
            assert_eq!(items[1], MettaValue::Long(-5));
            assert_eq!(items[2], MettaValue::Long(-10));
        } else {
            panic!("Expected SExpr with negative numbers");
        }
    }

    #[test]
    fn test_compile_zero() {
        let src = "0";
        let state = compile(src).unwrap();

        assert_eq!(state.source().len(), 1);
        assert_eq!(
            {
                let s = state.source();
                s[0]
            },
            MettaValue::Long(0)
        );
    }

    #[test]
    fn test_compile_literals() {
        let src = "(True False 42 \"hello\")";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
            assert_eq!(items[2], MettaValue::Long(42));
            assert_eq!(items[3], MettaValue::String("hello".to_string()));
        }
    }

    #[test]
    fn test_compile_mixed_literals() {
        let src = "(list 42 -7 0 True False \"text\" ())";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items[0], MettaValue::Atom("list".to_string()));
            assert_eq!(items[1], MettaValue::Long(42));
            assert_eq!(items[2], MettaValue::Long(-7));
            assert_eq!(items[3], MettaValue::Long(0));
            assert_eq!(items[4], MettaValue::Bool(true));
            assert_eq!(items[5], MettaValue::Bool(false));
            assert_eq!(items[6], MettaValue::String("text".to_string()));
            // () compiles to empty SExpr for HE compatibility
            assert_eq!(items[7], MettaValue::SExpr(vec![]));
        } else {
            panic!("Expected SExpr with mixed literals");
        }
    }

    #[test]
    fn test_boolean_case_sensitivity() {
        // Phase 1.5 PT alignment (2026-05-22): parser is permissive —
        // both `True`/`False` and `true`/`false` parse as Bool literals.
        // PT translator emits lowercase canonical; capitalized retained
        // for backward compatibility.
        let src = "(true false)";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
        } else {
            panic!("Expected SExpr with Bool literals");
        }

        // Verify capitalized versions ARE treated as booleans
        let src = "(True False)";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
        } else {
            panic!("Expected SExpr with boolean values");
        }
    }

    #[test]
    fn test_compile_with_comments() {
        let src = r#"
            ; Single line comment
            (+ 1 2)
            ; Another comment
            (* 3 4)
        "#;
        let state = compile(src).unwrap();
        assert_eq!(state.source().len(), 2);
    }

    #[test]
    fn test_compile_type_assertion() {
        let src = "(: x Number)";
        let state = compile(src).unwrap();

        assert_eq!(state.source().len(), 1);

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom(":".to_string()));
            assert_eq!(items[1], MettaValue::Atom("x".to_string()));
            assert_eq!(items[2], MettaValue::Atom("Number".to_string()));
        } else {
            panic!("Expected SExpr for type assertion");
        }
    }

    #[test]
    fn test_compile_exclaim_operator() {
        let src = "!(double 5)";
        let state = compile(src).unwrap();

        assert_eq!(state.source().len(), 1);

        if let MettaValueInner::SExpr(items) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Atom("!".to_string()));

            if let MettaValueInner::SExpr(inner) = items[1].inner() {
                assert_eq!(inner[0], MettaValue::Atom("double".to_string()));
                assert_eq!(inner[1], MettaValue::Long(5));
            } else {
                panic!("Expected SExpr inside !");
            }
        } else {
            panic!("Expected SExpr for ! operator");
        }
    }

    #[test]
    fn test_compile_dollar_variable() {
        let src = "$x";
        let state = compile(src).unwrap();

        assert_eq!(state.source().len(), 1);
        assert_eq!(
            {
                let s = state.source();
                s[0]
            },
            MettaValue::Atom("$x".to_string())
        );
    }

    #[test]
    fn test_compile_quote_variable() {
        let src = "'quoted";
        let state = compile(src).unwrap();

        assert_eq!(state.source().len(), 1);
        // Parser treats 'quoted as a prefixed expression: (quote quoted)
        // compile.rs detects (quote X) and produces Quoted(X)
        assert_eq!(
            {
                let s = state.source();
                s[0]
            },
            MettaValue::Quoted(MettaValue::Atom("quoted".to_string()))
        );
    }

    #[test]
    fn test_compile_deeply_nested() {
        let src = "(+ 1 (+ 2 (+ 3 (+ 4 5))))";
        let state = compile(src).unwrap();

        assert_eq!(state.source().len(), 1);

        // Outer: (+ 1 ...)
        if let MettaValueInner::SExpr(outer) = {
            let s = state.source();
            s[0]
        }
        .inner()
        {
            assert_eq!(outer[0], MettaValue::Atom("+".to_string()));
            assert_eq!(outer[1], MettaValue::Long(1));

            // Level 2: (+ 2 ...)
            if let MettaValueInner::SExpr(level2) = outer[2].inner() {
                assert_eq!(level2[0], MettaValue::Atom("+".to_string()));
                assert_eq!(level2[1], MettaValue::Long(2));

                // Level 3: (+ 3 ...)
                if let MettaValueInner::SExpr(level3) = level2[2].inner() {
                    assert_eq!(level3[0], MettaValue::Atom("+".to_string()));
                    assert_eq!(level3[1], MettaValue::Long(3));

                    // Level 4: (+ 4 5)
                    if let MettaValueInner::SExpr(level4) = level3[2].inner() {
                        assert_eq!(level4[0], MettaValue::Atom("+".to_string()));
                        assert_eq!(level4[1], MettaValue::Long(4));
                        assert_eq!(level4[2], MettaValue::Long(5));
                    } else {
                        panic!("Expected SExpr at level 4");
                    }
                } else {
                    panic!("Expected SExpr at level 3");
                }
            } else {
                panic!("Expected SExpr at level 2");
            }
        } else {
            panic!("Expected SExpr for outer expression");
        }
    }

    #[test]
    fn test_invalid_syntax_unclosed_paren() {
        let input = "(+ 1 2";
        let result = compile(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_syntax_extra_close_paren() {
        let input = "(+ 1 2))";
        let result = compile(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_syntax_mismatched_parens() {
        let input = "((+ 1 2)";
        let result = compile(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_error_with_atom_message() {
        let input = r#"!(error failure-code 42)"#;
        let state = compile(input).expect("compile failed");
        let env = new_env();
        let (results, _env) = eval(
            {
                let s = state.source();
                s[0]
            },
            env,
            &state,
        );

        assert_eq!(results.len(), 1);
        // Phase 1.1 PT-canonical: source `(error <message-atom> <offending>)`
        // maps to internal Error(Type=failure-code-atom, Ctx=42).
        // The (error) special form preserves atom-vs-string distinction —
        // the user passed an atom for the message, so Type stays an atom.
        if let MettaValueInner::Error(type_val, _) = results[0].inner() {
            assert_eq!(type_val.as_atom(), Some("failure-code"));
        } else {
            panic!("Expected error, got: {:?}", results[0]);
        }
    }

    // =========================================================================
    // Span Tracking Tests
    // =========================================================================

    #[test]
    fn test_compile_span_on_atom() {
        // Source:  "$x"
        // Offsets: 0123
        let state = compile("$x").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        // Value should be a Spanned atom
        assert!(val.is_spanned(), "compiled atom should have a span");
        let span = val.span().expect("atom should carry a span");
        assert_eq!(span.start.row, 0);
        assert_eq!(span.start.column, 0);
        assert_eq!(span.end.row, 0);
        assert_eq!(span.end.column, 2);
        assert_eq!(span.start.byte_offset, 0);
        assert_eq!(span.end.byte_offset, 2);

        // inner() should strip the Spanned wrapper transparently
        assert!(val.is_atom());
        assert_eq!(val.as_atom(), Some("$x"));
    }

    #[test]
    fn test_compile_span_on_integer() {
        // Source:  "42"
        // Offsets: 01
        let state = compile("42").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        assert!(val.is_spanned());
        let span = val.span().expect("integer should carry a span");
        assert_eq!(span.start.byte_offset, 0);
        assert_eq!(span.end.byte_offset, 2);
        assert!(val.is_long());
        assert_eq!(val.as_long(), Some(42));
    }

    #[test]
    fn test_compile_span_on_string() {
        // Source:  '"hello"'
        // Offsets: 0123456
        let state = compile("\"hello\"").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        assert!(val.is_spanned());
        let span = val.span().expect("string should carry a span");
        assert_eq!(span.start.byte_offset, 0);
        assert_eq!(span.end.byte_offset, 7);
        assert!(val.is_string());
        assert_eq!(val.as_string(), Some("hello"));
    }

    #[test]
    fn test_compile_span_on_sexpr() {
        // Source:  "(+ 1 2)"
        // Offsets: 0123456
        let state = compile("(+ 1 2)").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        // Outer expression should have a span covering the full S-expression
        assert!(val.is_spanned());
        let span = val.span().expect("sexpr should carry a span");
        assert_eq!(span.start.byte_offset, 0);
        assert_eq!(span.end.byte_offset, 7);

        // Inner items should also have spans
        if let MettaValueInner::SExpr(items) = val.inner() {
            // "+" at offset 1
            assert!(items[0].is_spanned(), "operator atom should have a span");
            let op_span = items[0].span().expect("+ should have a span");
            assert_eq!(op_span.start.byte_offset, 1);
            assert_eq!(op_span.end.byte_offset, 2);

            // "1" at offset 3
            let one_span = items[1].span().expect("1 should have a span");
            assert_eq!(one_span.start.byte_offset, 3);
            assert_eq!(one_span.end.byte_offset, 4);

            // "2" at offset 5
            let two_span = items[2].span().expect("2 should have a span");
            assert_eq!(two_span.start.byte_offset, 5);
            assert_eq!(two_span.end.byte_offset, 6);
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_compile_span_on_prefix_operator() {
        // Source:  "!(+ 1 2)"
        // Offsets: 012345678
        let state = compile("!(+ 1 2)").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        // Outer span should cover the full "!(+ 1 2)"
        assert!(val.is_spanned());
        let span = val.span().expect("prefix expr should carry a span");
        assert_eq!(span.start.byte_offset, 0);
        assert_eq!(span.end.byte_offset, 8);

        // Inner structure: (! (+ 1 2))
        if let MettaValueInner::SExpr(items) = val.inner() {
            // "!" operator at offset 0
            let bang_span = items[0].span().expect("! should have a span");
            assert_eq!(bang_span.start.byte_offset, 0);
            assert_eq!(bang_span.end.byte_offset, 1);

            // "(+ 1 2)" at offsets 1-8
            let inner_span = items[1].span().expect("inner sexpr should have a span");
            assert_eq!(inner_span.start.byte_offset, 1);
            assert_eq!(inner_span.end.byte_offset, 8);
        } else {
            panic!("Expected SExpr for prefix operator");
        }
    }

    #[test]
    fn test_compile_span_multiline() {
        // Source:
        //   line 0: "(+ 1 2)\n"  (bytes 0-7, newline at 7)
        //   line 1: "(* 3 4)"    (bytes 8-14)
        let state = compile("(+ 1 2)\n(* 3 4)").unwrap();
        let src = state.source();

        let span0 = src[0].span().expect("first expr should have a span");
        assert_eq!(span0.start.row, 0);
        assert_eq!(span0.start.column, 0);
        assert_eq!(span0.end.row, 0);
        assert_eq!(span0.end.column, 7);

        let span1 = src[1].span().expect("second expr should have a span");
        assert_eq!(span1.start.row, 1);
        assert_eq!(span1.start.column, 0);
        assert_eq!(span1.end.row, 1);
        assert_eq!(span1.end.column, 7);
    }

    #[test]
    fn test_compile_span_transparent_equality() {
        // Spanned values should equal non-spanned values (span-transparent equality)
        let state = compile("42").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        // val is Spanned(Long(42), span) but should equal bare Long(42)
        assert_eq!(val, MettaValue::Long(42));
    }

    #[test]
    fn test_compile_span_on_quote_prefix() {
        // Source:  "'foo"
        // Offsets: 0123
        let state = compile("'foo").unwrap();
        let val = {
            let s = state.source();
            s[0]
        };

        // Should be Spanned(Quoted(Spanned(Atom("foo"), ...)), full_span)
        assert!(val.is_spanned());
        let span = val.span().expect("quoted value should have a span");
        assert_eq!(span.start.byte_offset, 0);
        assert_eq!(span.end.byte_offset, 4);

        // inner() strips Spanned → should see Quoted
        assert!(val.is_quoted());
    }

    // =========================================================================
    // Arena Compilation Tests
    // =========================================================================

    #[test]
    fn test_compile_with_eval() {
        let src = "!(+ 1 2)";
        let state = compile(src).unwrap();
        let env = new_env();

        // Evaluate the expression
        let (results, _env) = eval_trampoline(
            {
                let s = state.source();
                s[0]
            },
            env,
            &state,
        );

        // Results should contain [3]
        assert_eq!(results.len(), 1);
        assert!(results[0].0.is_long());
        assert_eq!(results[0].0.as_long(), Some(3));
    }
}
