//! Rustyline helper integration
//!
//! Integrates all REPL components into a single Helper trait implementation

use super::indenter::SmartIndenter;
use super::query_highlighter::QueryHighlighter;
use super::state_machine::ReplStateMachine;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Helper};
use std::borrow::Cow;

/// Known MeTTa functions and keywords for completion
const GROUNDED_FUNCTIONS: &[&str] = &["+", "-", "*", "/", "%", "<", "<=", ">", ">=", "==", "!="];

const SPECIAL_FORMS: &[&str] = &[
    "if", "match", "case", "let", "let*", "quote", "unquote", "eval", "error", "catch", "is-error",
];

const TYPE_OPERATIONS: &[&str] = &[":", "get-type", "check-type"];

const CONTROL_FLOW: &[&str] = &["=", "!", "->"];

/// MeTTa REPL helper integrating all components
pub struct MettaHelper {
    highlighter: QueryHighlighter,
    state_machine: ReplStateMachine,
    indenter: SmartIndenter,
    command_history: Vec<String>,
    defined_functions: Vec<String>,
    defined_variables: Vec<String>,
}

impl MettaHelper {
    /// Create new helper
    pub fn new() -> Result<Self, String> {
        let highlighter = QueryHighlighter::new()?;
        let state_machine = ReplStateMachine::new();
        let indenter = SmartIndenter::new()?;

        Ok(Self {
            highlighter,
            state_machine,
            indenter,
            command_history: Vec::new(),
            defined_functions: Vec::new(),
            defined_variables: Vec::new(),
        })
    }

    /// Get reference to state machine
    pub fn state_machine(&self) -> &ReplStateMachine {
        &self.state_machine
    }

    /// Get mutable reference to state machine
    pub fn state_machine_mut(&mut self) -> &mut ReplStateMachine {
        &mut self.state_machine
    }

    /// Get reference to indenter
    pub fn indenter(&self) -> &SmartIndenter {
        &self.indenter
    }

    /// Get mutable reference to indenter
    pub fn indenter_mut(&mut self) -> &mut SmartIndenter {
        &mut self.indenter
    }

    /// Calculate indentation for current buffer
    /// This can be used by external code for manual indentation hints
    pub fn calculate_indent(&mut self, buffer: &str) -> usize {
        self.indenter.calculate_indent(buffer)
    }

    /// Add command to history for hints
    pub fn add_to_history(&mut self, cmd: String) {
        // Keep only last 100 commands for hints
        if self.command_history.len() >= 100 {
            self.command_history.remove(0);
        }
        self.command_history.push(cmd);
    }

    /// Update completions from the environment
    /// Extracts function names from defined rules
    ///
    /// Note: Variable names are NOT extracted because they are normalized to MORK's
    /// internal variable names ($a, $b, etc.) and don't preserve their original names.
    /// Only function names (symbols) remain unchanged after compilation.
    pub fn update_from_environment(&mut self, env: &crate::backend::Environment) {
        use crate::backend::MettaValue;

        // Clear previous definitions
        self.defined_functions.clear();
        self.defined_variables.clear();

        // Extract function names from rules
        for rule in env.iter_rules() {
            // Extract function name from lhs
            match &rule.lhs {
                MettaValue::SExpr(items) if !items.is_empty() => {
                    // Pattern like (fibonacci $n) -> extract "fibonacci"
                    if let MettaValue::Atom(name) = &items[0] {
                        if !name.starts_with('$')
                            && !name.starts_with('&')
                            && !name.starts_with('\'')
                        {
                            // It's a function name, not a variable
                            if !self.defined_functions.contains(name) {
                                self.defined_functions.push(name.clone());
                            }
                        }
                    }
                }
                MettaValue::Atom(name) => {
                    // Simple constant like (= my-const 42) -> extract "my-const"
                    // Variable names like $global-var get normalized to $a, $b, etc.
                    // so we can't reliably complete them. Only constants/functions work.
                    if !name.starts_with('$') && !name.starts_with('&') && !name.starts_with('\'') {
                        // It's a constant/function
                        if !self.defined_functions.contains(name) {
                            self.defined_functions.push(name.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // Sort for consistent ordering
        self.defined_functions.sort();
        // Note: defined_variables is intentionally left empty since we can't
        // extract meaningful variable names from the environment
    }

    /// Get all known completions (static + dynamic)
    fn get_all_completions(&self) -> Vec<String> {
        let mut completions = Vec::new();

        // Add static completions
        completions.extend(GROUNDED_FUNCTIONS.iter().map(|s| s.to_string()));
        completions.extend(SPECIAL_FORMS.iter().map(|s| s.to_string()));
        completions.extend(TYPE_OPERATIONS.iter().map(|s| s.to_string()));
        completions.extend(CONTROL_FLOW.iter().map(|s| s.to_string()));

        // Add dynamic completions
        completions.extend(self.defined_functions.iter().cloned());
        completions.extend(self.defined_variables.iter().cloned());

        completions
    }
}

impl Default for MettaHelper {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

// Implement Completer trait
impl Completer for MettaHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Find the word being completed
        let line_before_cursor = &line[..pos];

        // Find the start of the current word
        let word_start = line_before_cursor
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '{' || c == '}')
            .map(|i| i + 1)
            .unwrap_or(0);

        let partial = &line_before_cursor[word_start..];

        // Skip if empty or just whitespace
        if partial.trim().is_empty() {
            return Ok((pos, vec![]));
        }

        // Get all possible completions (static + user-defined)
        let all_completions = self.get_all_completions();

        // Filter completions that start with the partial word
        let mut matches: Vec<Pair> = all_completions
            .iter()
            .filter(|comp| comp.starts_with(partial))
            .map(|comp| Pair {
                display: comp.clone(),
                replacement: comp.clone(),
            })
            .collect();

        // Sort matches
        matches.sort_by(|a, b| a.display.cmp(&b.display));

        Ok((word_start, matches))
    }
}

// Implement Hinter trait
impl Hinter for MettaHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        // Only provide hints at the end of the line
        if pos < line.len() {
            return None;
        }

        // Search history for commands that start with current line
        // Search in reverse to get most recent matches first
        for cmd in self.command_history.iter().rev() {
            if cmd.starts_with(line) && cmd.len() > line.len() {
                // Return the rest of the command as a hint
                return Some(cmd[line.len()..].to_string());
            }
        }

        None
    }
}

// Implement Highlighter trait (delegate to QueryHighlighter)
impl Highlighter for MettaHelper {
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        self.highlighter.highlight(line, pos)
    }

    fn highlight_char(&self, line: &str, pos: usize, forced: bool) -> bool {
        self.highlighter.highlight_char(line, pos, forced)
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // Dim the hint
        Cow::Owned(format!("\x1b[90m{}\x1b[0m", hint))
    }

    fn highlight_candidate<'c>(
        &self,
        candidate: &'c str,
        _completion: rustyline::CompletionType,
    ) -> Cow<'c, str> {
        // Highlight completions
        Cow::Borrowed(candidate)
    }
}

// Implement Validator trait (check for complete expressions)
impl Validator for MettaHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        let input = ctx.input();

        // Check if input is complete
        let status = ReplStateMachine::check_completeness(input);

        match status {
            super::state_machine::CompletenessStatus::Complete => Ok(ValidationResult::Valid(None)),
            super::state_machine::CompletenessStatus::Incomplete { .. } => {
                Ok(ValidationResult::Incomplete)
            }
            super::state_machine::CompletenessStatus::Invalid { reason } => {
                Ok(ValidationResult::Invalid(Some(reason)))
            }
        }
    }
}

// Implement Helper trait (combines all traits)
impl Helper for MettaHelper {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helper_creation() {
        let helper = MettaHelper::new();
        assert!(helper.is_ok());
    }

    #[test]
    fn test_validation_complete() {
        let helper = MettaHelper::new().unwrap();
        let input = "(+ 1 2)";

        // Create a minimal validation context
        // Note: ValidationContext is not easily constructible in tests,
        // so we test the state machine directly
        let status = ReplStateMachine::check_completeness(input);
        assert!(matches!(
            status,
            super::super::state_machine::CompletenessStatus::Complete
        ));
    }

    #[test]
    fn test_validation_incomplete() {
        let helper = MettaHelper::new().unwrap();
        let input = "(+ 1";

        let status = ReplStateMachine::check_completeness(input);
        assert!(matches!(
            status,
            super::super::state_machine::CompletenessStatus::Incomplete { .. }
        ));
    }

    #[test]
    fn test_validation_invalid() {
        let helper = MettaHelper::new().unwrap();
        let input = "(+ 1 2))"; // Extra closing paren

        let status = ReplStateMachine::check_completeness(input);
        assert!(matches!(
            status,
            super::super::state_machine::CompletenessStatus::Invalid { .. }
        ));
    }

    #[test]
    fn test_state_machine_access() {
        let helper = MettaHelper::new().unwrap();
        let sm = helper.state_machine();
        assert_eq!(sm.state(), &super::super::state_machine::ReplState::Ready);
    }

    #[test]
    fn test_indenter_access() {
        let mut helper = MettaHelper::new().unwrap();

        // Test indenter is accessible
        let indent = helper.indenter();
        assert_eq!(indent.indent_width(), 2);

        // Test indentation calculation
        let indent_level = helper.calculate_indent("(+ 1");
        assert_eq!(indent_level, 2); // 1 unclosed paren * 2 spaces

        // Test nested indentation
        let nested_indent = helper.calculate_indent("(foo (bar");
        assert_eq!(nested_indent, 4); // 2 unclosed parens * 2 spaces
    }

    #[test]
    fn test_completion_basic() {
        use rustyline::history::DefaultHistory;

        let helper = MettaHelper::new().unwrap();
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        // Test completing "i" should suggest "if"
        let (start, matches) = helper.complete("(i", 2, &ctx).unwrap();
        assert_eq!(start, 1); // Position after '('
        assert!(matches.iter().any(|m| m.display == "if"));
    }

    #[test]
    fn test_completion_operators() {
        use rustyline::history::DefaultHistory;

        let helper = MettaHelper::new().unwrap();
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        // Test completing "+" should find operators
        let (start, matches) = helper.complete("(+", 2, &ctx).unwrap();
        assert_eq!(start, 1);
        assert!(matches.iter().any(|m| m.display == "+"));
    }

    #[test]
    fn test_hint_from_history() {
        use rustyline::history::DefaultHistory;

        let mut helper = MettaHelper::new().unwrap();
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        // Add some commands to history
        helper.add_to_history("(+ 1 2)".to_string());
        helper.add_to_history("(* 3 4)".to_string());

        // Test hint for partial match
        let hint = helper.hint("(+", 2, &ctx);
        assert_eq!(hint, Some(" 1 2)".to_string()));
    }

    #[test]
    fn test_hint_no_match() {
        use rustyline::history::DefaultHistory;

        let mut helper = MettaHelper::new().unwrap();
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        helper.add_to_history("(+ 1 2)".to_string());

        // Test hint for non-matching input
        let hint = helper.hint("(xyz", 4, &ctx);
        assert_eq!(hint, None);
    }

    #[test]
    fn test_history_limit() {
        let mut helper = MettaHelper::new().unwrap();

        // Add more than 100 commands
        for i in 0..150 {
            helper.add_to_history(format!("(cmd{})", i));
        }

        // Should only keep last 100
        assert_eq!(helper.command_history.len(), 100);
        assert_eq!(helper.command_history[0], "(cmd50)");
        assert_eq!(helper.command_history[99], "(cmd149)");
    }

    #[test]
    fn test_update_from_environment() {
        use crate::backend::{compile, eval, Environment};

        let mut helper = MettaHelper::new().unwrap();
        let mut env = Environment::new();

        // Initially no user-defined functions
        assert_eq!(helper.defined_functions.len(), 0);

        // Define a function
        let code =
            "(= (fibonacci $n) (if (< $n 2) $n (+ (fibonacci (- $n 1)) (fibonacci (- $n 2)))))";
        let state = compile(code).unwrap();
        env = env.union(&state.environment);

        // IMPORTANT: Rules are added to environment during evaluation
        for sexpr in state.source {
            let (_, updated_env) = eval(sexpr, env.clone());
            env = updated_env;
        }

        // Debug: print what's in the environment
        println!("=== Environment rules ===");
        for rule in env.iter_rules() {
            println!("Rule lhs: {:?}", rule.lhs);
            println!("Rule rhs: {:?}", rule.rhs);
        }

        // Update helper from environment
        helper.update_from_environment(&env);

        // Debug: print what was extracted
        println!("=== Extracted functions ===");
        println!("Functions: {:?}", helper.defined_functions);
        println!("Variables: {:?}", helper.defined_variables);

        // Should now have "fibonacci" in completions
        assert!(helper.defined_functions.contains(&"fibonacci".to_string()));

        let completions = helper.get_all_completions();
        assert!(completions.contains(&"fibonacci".to_string()));
    }

    #[test]
    fn test_completion_with_user_defined() {
        use crate::backend::{compile, eval, Environment};
        use rustyline::history::DefaultHistory;

        let mut helper = MettaHelper::new().unwrap();
        let mut env = Environment::new();
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        // Define a function
        let code = "(= (my-func $x) (* 2 $x))";
        let state = compile(code).unwrap();
        env = env.union(&state.environment);

        // Evaluate to add rules to environment
        for sexpr in state.source {
            let (_, updated_env) = eval(sexpr, env.clone());
            env = updated_env;
        }

        helper.update_from_environment(&env);

        // Test completing "my" should suggest "my-func"
        let (start, matches) = helper.complete("(my", 3, &ctx).unwrap();
        assert_eq!(start, 1);
        assert!(matches.iter().any(|m| m.display == "my-func"));
    }

    #[test]
    fn test_constant_completion() {
        use crate::backend::{compile, eval, Environment};
        use rustyline::history::DefaultHistory;

        let mut helper = MettaHelper::new().unwrap();
        let mut env = Environment::new();
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);

        // Define a constant (not a variable, since variable names get normalized)
        let code = "(= my-const 42)";
        let state = compile(code).unwrap();
        env = env.union(&state.environment);

        // Evaluate to add rules to environment
        for sexpr in state.source {
            let (_, updated_env) = eval(sexpr, env.clone());
            env = updated_env;
        }

        helper.update_from_environment(&env);

        // Test completing "my" should suggest "my-const"
        let (start, matches) = helper.complete("my", 2, &ctx).unwrap();
        assert_eq!(start, 0);
        assert!(matches.iter().any(|m| m.display == "my-const"));
    }
}
