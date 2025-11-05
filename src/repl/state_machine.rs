//! State machine for multi-line REPL support
//!
//! Manages REPL state transitions and multi-line input detection

/// REPL states
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplState {
    /// Ready to accept new input
    Ready,
    /// Waiting for more input to complete expression
    Continuation { buffer: String },
    /// Evaluating a complete expression
    Evaluating { input: String },
    /// Displaying evaluation results
    DisplayingResults,
    /// Error state (parse error, evaluation error, etc.)
    Error { message: String },
}

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

            // Continuation state: accumulate input
            (ReplState::Continuation { buffer }, ReplEvent::LineSubmitted(line)) => {
                let mut combined = buffer.clone();
                combined.push('\n');
                combined.push_str(&line);
                self.handle_continuation(combined)
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
                self.state = ReplState::Continuation {
                    buffer: line.clone(),
                };
                StateTransition::TransitionWithPrompt {
                    new_state: ReplState::Continuation { buffer: line },
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

    /// Handle continuation input
    fn handle_continuation(&mut self, combined: String) -> StateTransition {
        match Self::check_completeness(&combined) {
            CompletenessStatus::Complete => {
                self.state = ReplState::Evaluating {
                    input: combined.clone(),
                };
                StateTransition::Transition(ReplState::Evaluating { input: combined })
            }
            CompletenessStatus::Incomplete { .. } => {
                self.state = ReplState::Continuation {
                    buffer: combined.clone(),
                };
                StateTransition::TransitionWithPrompt {
                    new_state: ReplState::Continuation { buffer: combined },
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
        let mut in_block_comment = false;
        let mut escape_next = false;
        let mut chars = input.chars().peekable();

        while let Some(ch) = chars.next() {
            // Handle escape sequences in strings
            if escape_next {
                escape_next = false;
                continue;
            }

            // Line comments end at newline
            if in_line_comment {
                if ch == '\n' {
                    in_line_comment = false;
                }
                continue;
            }

            // Block comments
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next(); // consume '/'
                    in_block_comment = false;
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

            // Start of comments
            if ch == ';' {
                in_line_comment = true;
                continue;
            }

            if ch == '/' {
                if chars.peek() == Some(&'/') {
                    chars.next();
                    in_line_comment = true;
                    continue;
                } else if chars.peek() == Some(&'*') {
                    chars.next();
                    in_block_comment = true;
                    continue;
                }
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
    fn test_completeness_with_block_comment() {
        assert_eq!(
            ReplStateMachine::check_completeness("/* comment */ (+ 1 2)"),
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
}
