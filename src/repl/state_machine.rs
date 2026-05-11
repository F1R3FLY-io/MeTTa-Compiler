//! State machine for multi-line REPL support
//!
//! Manages REPL state transitions and multi-line input detection

use ropey::Rope;

/// REPL states
#[derive(Debug, Clone)]
pub enum ReplState {
    /// Ready to accept new input
    Ready,
    /// Waiting for more input to complete expression (uses Rope for O(1) clone)
    Continuation { buffer: Rope },
    /// Evaluating a complete expression
    Evaluating { input: String },
    /// Displaying evaluation results
    DisplayingResults,
    /// Error state (parse error, evaluation error, etc.)
    Error { message: String },
}

// Manual PartialEq implementation since Rope doesn't derive it
impl PartialEq for ReplState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ReplState::Ready, ReplState::Ready) => true,
            (ReplState::Continuation { buffer: a }, ReplState::Continuation { buffer: b }) => {
                // Compare Rope contents as strings
                *a == *b
            }
            (ReplState::Evaluating { input: a }, ReplState::Evaluating { input: b }) => a == b,
            (ReplState::DisplayingResults, ReplState::DisplayingResults) => true,
            (ReplState::Error { message: a }, ReplState::Error { message: b }) => a == b,
            _ => false,
        }
    }
}

impl Eq for ReplState {}

/// REPL events
#[derive(Debug, Clone)]
pub enum ReplEvent {
    /// User submitted a line
    LineSubmitted(String),
    /// User interrupted (Ctrl-C)
    Interrupted,
    /// End of input (Ctrl-D)
    Eof,
    /// Evaluation completed successfully
    EvaluationComplete(Vec<String>),
    /// Evaluation failed with error
    EvaluationFailed(String),
    /// Results displayed
    ResultsDisplayed,
}

/// State transition results
#[derive(Debug)]
pub enum StateTransition {
    /// No state change
    NoChange,
    /// Transition to new state
    Transition(ReplState),
    /// Transition with prompt change
    TransitionWithPrompt {
        new_state: ReplState,
        prompt: String,
    },
}

/// Completeness status for expressions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletenessStatus {
    /// Expression is complete and can be evaluated
    Complete,
    /// Expression is incomplete, needs more input
    Incomplete {
        missing_close_parens: usize,
        missing_close_braces: usize,
        unclosed_string: bool,
    },
    /// Expression has mismatched delimiters
    Invalid { reason: String },
}

/// State machine for REPL
pub struct ReplStateMachine {
    state: ReplState,
    continuation_prompt: String,
}

impl ReplStateMachine {
    /// Create new state machine
    pub fn new() -> Self {
        Self {
            state: ReplState::Ready,
            continuation_prompt: "...> ".to_string(),
        }
    }

    /// Get current state
    pub fn state(&self) -> &ReplState {
        &self.state
    }

    /// Get continuation prompt
    pub fn continuation_prompt(&self) -> &str {
        &self.continuation_prompt
    }

    /// Set continuation prompt
    pub fn set_continuation_prompt(&mut self, prompt: String) {
        self.continuation_prompt = prompt;
    }

    /// Process an event and return the transition
    pub fn process_event(&mut self, event: ReplEvent) -> StateTransition {
        match (&self.state, event) {
            // Ready state: accept new input
            (ReplState::Ready, ReplEvent::LineSubmitted(line)) => self.handle_new_input(line),

            // Continuation state: accumulate input (O(1) clone with Rope)
            (ReplState::Continuation { buffer }, ReplEvent::LineSubmitted(line)) => {
                let mut rope = buffer.clone();
                rope.append(Rope::from(format!("\n{}", line)));
                self.handle_continuation(rope)
            }

            // Interrupted: reset to ready
            (_, ReplEvent::Interrupted) => {
                self.state = ReplState::Ready;
                StateTransition::Transition(ReplState::Ready)
            }

            // EOF: exit
            (_, ReplEvent::Eof) => StateTransition::NoChange,

            // Evaluation complete
            (ReplState::Evaluating { .. }, ReplEvent::EvaluationComplete(_results)) => {
                self.state = ReplState::DisplayingResults;
                StateTransition::Transition(ReplState::DisplayingResults)
            }

            // Evaluation failed
            (ReplState::Evaluating { .. }, ReplEvent::EvaluationFailed(msg)) => {
                self.state = ReplState::Error {
                    message: msg.clone(),
                };
                StateTransition::Transition(ReplState::Error { message: msg })
            }

            // Results displayed
            (ReplState::DisplayingResults, ReplEvent::ResultsDisplayed) => {
                self.state = ReplState::Ready;
                StateTransition::Transition(ReplState::Ready)
            }

            // Error state: reset on any event
            (ReplState::Error { .. }, _) => {
                self.state = ReplState::Ready;
                StateTransition::Transition(ReplState::Ready)
            }

            // All other transitions are invalid
            _ => StateTransition::NoChange,
        }
    }

    /// Handle new input from Ready state
    fn handle_new_input(&mut self, line: String) -> StateTransition {
        // Skip empty lines
        if line.trim().is_empty() {
            return StateTransition::NoChange;
        }

        // Check if input is complete
        match Self::check_completeness(&line) {
            CompletenessStatus::Complete => {
                self.state = ReplState::Evaluating {
                    input: line.clone(),
                };
                StateTransition::Transition(ReplState::Evaluating { input: line })
            }
            CompletenessStatus::Incomplete { .. } => {
                let rope = Rope::from(line.as_str());
                self.state = ReplState::Continuation {
                    buffer: rope.clone(),
                };
                StateTransition::TransitionWithPrompt {
                    new_state: ReplState::Continuation { buffer: rope },
                    prompt: self.continuation_prompt.clone(),
                }
            }
            CompletenessStatus::Invalid { reason } => {
                self.state = ReplState::Error {
                    message: reason.clone(),
                };
                StateTransition::Transition(ReplState::Error { message: reason })
            }
        }
    }

    /// Handle continuation input (using Rope for efficient buffer management)
    fn handle_continuation(&mut self, buffer: Rope) -> StateTransition {
        let combined = buffer.to_string();
        match Self::check_completeness(&combined) {
            CompletenessStatus::Complete => {
                self.state = ReplState::Evaluating {
                    input: combined.clone(),
                };
                StateTransition::Transition(ReplState::Evaluating { input: combined })
            }
            CompletenessStatus::Incomplete { .. } => {
                self.state = ReplState::Continuation {
                    buffer: buffer.clone(),
                };
                StateTransition::TransitionWithPrompt {
                    new_state: ReplState::Continuation { buffer },
                    prompt: self.continuation_prompt.clone(),
                }
            }
            CompletenessStatus::Invalid { reason } => {
                self.state = ReplState::Error {
                    message: reason.clone(),
                };
                StateTransition::Transition(ReplState::Error { message: reason })
            }
        }
    }

    /// Check if input is complete by counting delimiters
    pub fn check_completeness(input: &str) -> CompletenessStatus {
        let mut paren_depth = 0;
        let mut brace_depth = 0;
        let mut in_string = false;
        let mut in_line_comment = false;
        let mut escape_next = false;
        let chars = input.chars().peekable();

        for ch in chars {
            // Handle escape sequences in strings
            if escape_next {
                escape_next = false;
                continue;
            }

            // Line comments (;) end at newline
            if in_line_comment {
                if ch == '\n' {
                    in_line_comment = false;
                }
                continue;
            }

            // String handling
            if in_string {
                if ch == '\\' {
                    escape_next = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            // Start of line comment (MeTTa uses ; for comments)
            if ch == ';' {
                in_line_comment = true;
                continue;
            }

            // Start of string
            if ch == '"' {
                in_string = true;
                continue;
            }

            // Count delimiters
            match ch {
                '(' => paren_depth += 1,
                ')' => paren_depth -= 1,
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }

            // Check for negative depth (too many closing delimiters)
            if paren_depth < 0 {
                return CompletenessStatus::Invalid {
                    reason: "Unexpected closing parenthesis ')'".to_string(),
                };
            }
            if brace_depth < 0 {
                return CompletenessStatus::Invalid {
                    reason: "Unexpected closing brace '}'".to_string(),
                };
            }
        }

        // Check final state
        if in_string {
            return CompletenessStatus::Incomplete {
                missing_close_parens: 0,
                missing_close_braces: 0,
                unclosed_string: true,
            };
        }

        if paren_depth > 0 || brace_depth > 0 {
            return CompletenessStatus::Incomplete {
                missing_close_parens: paren_depth as usize,
                missing_close_braces: brace_depth as usize,
                unclosed_string: false,
            };
        }

        CompletenessStatus::Complete
    }
}

impl Default for ReplStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completeness_simple_complete() {
        assert_eq!(
            ReplStateMachine::check_completeness("(+ 1 2)"),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_completeness_incomplete_paren() {
        let status = ReplStateMachine::check_completeness("(+ 1 2");
        match status {
            CompletenessStatus::Incomplete {
                missing_close_parens,
                ..
            } => assert_eq!(missing_close_parens, 1),
            _ => panic!("Expected incomplete status"),
        }
    }

    #[test]
    fn test_completeness_nested_complete() {
        assert_eq!(
            ReplStateMachine::check_completeness("(foo (bar (baz)))"),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_completeness_with_string() {
        assert_eq!(
            ReplStateMachine::check_completeness(r#"(print "hello world")"#),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_completeness_unclosed_string() {
        let status = ReplStateMachine::check_completeness(r#"(print "hello"#);
        match status {
            CompletenessStatus::Incomplete {
                unclosed_string, ..
            } => assert!(unclosed_string),
            _ => panic!("Expected incomplete with unclosed string"),
        }
    }

    #[test]
    fn test_completeness_with_line_comment() {
        assert_eq!(
            ReplStateMachine::check_completeness("; comment\n(+ 1 2)"),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_completeness_invalid_extra_closing() {
        let status = ReplStateMachine::check_completeness("(+ 1 2))");
        match status {
            CompletenessStatus::Invalid { .. } => {}
            _ => panic!("Expected invalid status for extra closing paren"),
        }
    }

    #[test]
    fn test_state_machine_ready_to_evaluating() {
        let mut sm = ReplStateMachine::new();
        assert_eq!(sm.state(), &ReplState::Ready);

        let transition = sm.process_event(ReplEvent::LineSubmitted("(+ 1 2)".to_string()));
        match transition {
            StateTransition::Transition(ReplState::Evaluating { .. }) => {}
            _ => panic!("Expected transition to Evaluating"),
        }
    }

    #[test]
    fn test_state_machine_ready_to_continuation() {
        let mut sm = ReplStateMachine::new();

        let transition = sm.process_event(ReplEvent::LineSubmitted("(+ 1".to_string()));
        match transition {
            StateTransition::TransitionWithPrompt {
                new_state: ReplState::Continuation { .. },
                ..
            } => {}
            _ => panic!("Expected transition to Continuation"),
        }
    }

    #[test]
    fn test_state_machine_continuation_to_evaluating() {
        let mut sm = ReplStateMachine::new();

        // First line incomplete
        sm.process_event(ReplEvent::LineSubmitted("(+ 1".to_string()));

        // Second line completes it
        let transition = sm.process_event(ReplEvent::LineSubmitted("2)".to_string()));
        match transition {
            StateTransition::Transition(ReplState::Evaluating { input }) => {
                assert_eq!(input, "(+ 1\n2)");
            }
            _ => panic!("Expected transition to Evaluating"),
        }
    }

    #[test]
    fn test_state_machine_interrupt() {
        let mut sm = ReplStateMachine::new();

        // Start continuation
        sm.process_event(ReplEvent::LineSubmitted("(+ 1".to_string()));

        // Interrupt
        let transition = sm.process_event(ReplEvent::Interrupted);
        match transition {
            StateTransition::Transition(ReplState::Ready) => {}
            _ => panic!("Expected transition to Ready"),
        }
    }

    #[test]
    fn test_completeness_with_braces() {
        assert_eq!(
            ReplStateMachine::check_completeness("{expr1 expr2}"),
            CompletenessStatus::Complete
        );

        let status = ReplStateMachine::check_completeness("{expr1");
        match status {
            CompletenessStatus::Incomplete {
                missing_close_braces,
                ..
            } => assert_eq!(missing_close_braces, 1),
            _ => panic!("Expected incomplete status"),
        }
    }

    // ==========================================================================
    // Additional Branch Coverage Tests
    // ==========================================================================

    #[test]
    fn test_repl_state_equality() {
        // Test Ready == Ready
        assert_eq!(ReplState::Ready, ReplState::Ready);

        // Test Evaluating equality
        assert_eq!(
            ReplState::Evaluating {
                input: "test".to_string()
            },
            ReplState::Evaluating {
                input: "test".to_string()
            }
        );
        assert_ne!(
            ReplState::Evaluating {
                input: "test".to_string()
            },
            ReplState::Evaluating {
                input: "other".to_string()
            }
        );

        // Test DisplayingResults equality
        assert_eq!(ReplState::DisplayingResults, ReplState::DisplayingResults);

        // Test Error equality
        assert_eq!(
            ReplState::Error {
                message: "err".to_string()
            },
            ReplState::Error {
                message: "err".to_string()
            }
        );
        assert_ne!(
            ReplState::Error {
                message: "err1".to_string()
            },
            ReplState::Error {
                message: "err2".to_string()
            }
        );

        // Test different states are not equal
        assert_ne!(ReplState::Ready, ReplState::DisplayingResults);
        assert_ne!(
            ReplState::Ready,
            ReplState::Evaluating {
                input: "test".to_string()
            }
        );
    }

    #[test]
    fn test_continuation_state_equality() {
        let rope1 = Rope::from("(+ 1");
        let rope2 = Rope::from("(+ 1");
        let rope3 = Rope::from("(+ 2");

        assert_eq!(
            ReplState::Continuation {
                buffer: rope1.clone()
            },
            ReplState::Continuation { buffer: rope2 }
        );
        assert_ne!(
            ReplState::Continuation { buffer: rope1 },
            ReplState::Continuation { buffer: rope3 }
        );
    }

    #[test]
    fn test_empty_line_no_change() {
        let mut sm = ReplStateMachine::new();

        // Empty line should cause no change
        let transition = sm.process_event(ReplEvent::LineSubmitted("".to_string()));
        assert!(matches!(transition, StateTransition::NoChange));
        assert_eq!(sm.state(), &ReplState::Ready);

        // Whitespace-only line should also cause no change
        let transition = sm.process_event(ReplEvent::LineSubmitted("   \t  ".to_string()));
        assert!(matches!(transition, StateTransition::NoChange));
    }

    #[test]
    fn test_eof_event() {
        let mut sm = ReplStateMachine::new();

        let transition = sm.process_event(ReplEvent::Eof);
        assert!(matches!(transition, StateTransition::NoChange));
    }

    #[test]
    fn test_evaluation_complete() {
        let mut sm = ReplStateMachine::new();

        // Get to Evaluating state
        sm.process_event(ReplEvent::LineSubmitted("(+ 1 2)".to_string()));

        // Complete evaluation
        let transition = sm.process_event(ReplEvent::EvaluationComplete(vec!["3".to_string()]));
        match transition {
            StateTransition::Transition(ReplState::DisplayingResults) => {}
            _ => panic!("Expected transition to DisplayingResults"),
        }
    }

    #[test]
    fn test_evaluation_failed() {
        let mut sm = ReplStateMachine::new();

        // Get to Evaluating state
        sm.process_event(ReplEvent::LineSubmitted("(+ 1 2)".to_string()));

        // Fail evaluation
        let transition = sm.process_event(ReplEvent::EvaluationFailed("test error".to_string()));
        match transition {
            StateTransition::Transition(ReplState::Error { message }) => {
                assert_eq!(message, "test error");
            }
            _ => panic!("Expected transition to Error"),
        }
    }

    #[test]
    fn test_results_displayed() {
        let mut sm = ReplStateMachine::new();

        // Get to DisplayingResults state
        sm.process_event(ReplEvent::LineSubmitted("(+ 1 2)".to_string()));
        sm.process_event(ReplEvent::EvaluationComplete(vec!["3".to_string()]));

        // Display results
        let transition = sm.process_event(ReplEvent::ResultsDisplayed);
        match transition {
            StateTransition::Transition(ReplState::Ready) => {}
            _ => panic!("Expected transition to Ready"),
        }
    }

    #[test]
    fn test_error_state_reset() {
        let mut sm = ReplStateMachine::new();

        // Get to Error state via invalid input
        sm.process_event(ReplEvent::LineSubmitted("(+ 1 2))".to_string()));
        assert!(matches!(sm.state(), ReplState::Error { .. }));

        // Any event should reset to Ready
        let transition = sm.process_event(ReplEvent::LineSubmitted("(+ 1 2)".to_string()));
        match transition {
            StateTransition::Transition(ReplState::Ready) => {}
            _ => panic!("Expected transition to Ready from Error state"),
        }
    }

    #[test]
    fn test_escape_sequences_in_strings() {
        // String with escaped quote
        assert_eq!(
            ReplStateMachine::check_completeness(r#"(print "hello \"world\"")"#),
            CompletenessStatus::Complete
        );

        // String with escaped backslash
        assert_eq!(
            ReplStateMachine::check_completeness(r#"(print "path\\file")"#),
            CompletenessStatus::Complete
        );

        // String with escaped newline
        assert_eq!(
            ReplStateMachine::check_completeness(r#"(print "line1\nline2")"#),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_invalid_extra_closing_brace() {
        let status = ReplStateMachine::check_completeness("{a}}");
        match status {
            CompletenessStatus::Invalid { reason } => {
                assert!(reason.contains("brace") || reason.contains("'}'"));
            }
            _ => panic!("Expected invalid status for extra closing brace"),
        }
    }

    #[test]
    fn test_default_implementation() {
        let sm = ReplStateMachine::default();
        assert_eq!(sm.state(), &ReplState::Ready);
        assert_eq!(sm.continuation_prompt(), "...> ");
    }

    #[test]
    fn test_set_continuation_prompt() {
        let mut sm = ReplStateMachine::new();
        sm.set_continuation_prompt(">>> ".to_string());
        assert_eq!(sm.continuation_prompt(), ">>> ");
    }

    #[test]
    fn test_continuation_to_invalid() {
        let mut sm = ReplStateMachine::new();

        // Start continuation
        sm.process_event(ReplEvent::LineSubmitted("(+ 1".to_string()));

        // Add line that causes invalid state (extra closing paren)
        let transition = sm.process_event(ReplEvent::LineSubmitted("2))".to_string()));
        match transition {
            StateTransition::Transition(ReplState::Error { .. }) => {}
            _ => panic!("Expected transition to Error"),
        }
    }

    #[test]
    fn test_comment_at_end_of_line() {
        // Complete expression followed by comment
        assert_eq!(
            ReplStateMachine::check_completeness("(+ 1 2) ; result is 3"),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_only_comment() {
        // Only a comment (no actual expression)
        assert_eq!(
            ReplStateMachine::check_completeness("; this is a comment"),
            CompletenessStatus::Complete
        );
    }

    #[test]
    fn test_nested_parens_and_braces() {
        assert_eq!(
            ReplStateMachine::check_completeness("(foo {bar (baz)})"),
            CompletenessStatus::Complete
        );

        let status = ReplStateMachine::check_completeness("(foo {bar (baz)}");
        match status {
            CompletenessStatus::Incomplete {
                missing_close_parens,
                ..
            } => assert_eq!(missing_close_parens, 1),
            _ => panic!("Expected incomplete status"),
        }
    }

    #[test]
    fn test_continuation_stays_incomplete() {
        let mut sm = ReplStateMachine::new();

        // First line incomplete
        sm.process_event(ReplEvent::LineSubmitted("(+ 1".to_string()));

        // Second line still incomplete
        let transition = sm.process_event(ReplEvent::LineSubmitted("(* 2".to_string()));
        match transition {
            StateTransition::TransitionWithPrompt {
                new_state: ReplState::Continuation { .. },
                ..
            } => {}
            _ => panic!("Expected to stay in Continuation"),
        }
    }

    #[test]
    fn test_invalid_event_in_ready_state() {
        let mut sm = ReplStateMachine::new();

        // EvaluationComplete in Ready state should be NoChange
        let transition = sm.process_event(ReplEvent::EvaluationComplete(vec![]));
        assert!(matches!(transition, StateTransition::NoChange));
    }

    #[test]
    fn test_invalid_event_in_continuation_state() {
        let mut sm = ReplStateMachine::new();

        // Get to Continuation state
        sm.process_event(ReplEvent::LineSubmitted("(+ 1".to_string()));

        // EvaluationComplete in Continuation state should be NoChange
        let transition = sm.process_event(ReplEvent::EvaluationComplete(vec![]));
        assert!(matches!(transition, StateTransition::NoChange));
    }
}
