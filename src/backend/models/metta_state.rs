use super::MettaValue;
use crate::backend::environment::Environment;

/// MeTTa compilation/evaluation state for PathMap-based REPL integration
/// This structure represents the state of a MeTTa computation session.
///
/// # State Composition
/// - **Compiled state** (fresh from `compile`):
///   - `source`: S-expressions to evaluate
///   - `environment`: Empty atom space
///   - `output`: Empty (no evaluations yet)
///
/// - **Accumulated state** (built over multiple REPL iterations):
///   - `source`: Empty (already evaluated)
///   - `environment`: Accumulated atom space (MORK facts/rules)
///   - `output`: Accumulated evaluation results
///
/// # Usage Pattern
/// ```ignore
/// // Compile MeTTa source
/// let compiled_state = compile(source)?;
///
/// // Run against accumulated state
/// let new_accumulated = accumulated_state.run(&compiled_state)?;
/// ```
#[derive(Clone, Debug)]
pub struct MettaState {
    /// Source s-expressions to be evaluated
    pub source: Vec<MettaValue>,
    /// The atom space (MORK fact database) containing rules and facts
    pub environment: Environment,
    /// Evaluation output results
    pub output: Vec<MettaValue>,
}

impl MettaState {
    /// Create a fresh compiled state from parse results
    pub fn new_compiled(source: Vec<MettaValue>) -> Self {
        MettaState {
            source,
            environment: Environment::new(),
            output: Vec::new(),
        }
    }

    /// Create an empty accumulated state (for REPL initialization)
    pub fn new_empty() -> Self {
        MettaState {
            source: Vec::new(),
            environment: Environment::new(),
            output: Vec::new(),
        }
    }

    /// Create an accumulated state with existing environment and output
    pub fn new_accumulated(environment: Environment, output: Vec<MettaValue>) -> Self {
        MettaState {
            source: Vec::new(),
            environment,
            output,
        }
    }

    /// Convert MettaState to JSON representation for debugging
    ///
    /// Returns a JSON string with the format:
    /// ```json
    /// {
    ///   "source": [...],
    ///   "environment": {"facts_count": N},
    ///   "output": [...]
    /// }
    /// ```
    ///
    /// **Use Case**: Debugging, logging, inspection
    /// **Not Recommended**: Rholang integration (use PathMap Par instead)
    pub fn to_json_string(&self) -> String {
        let source_json: Vec<String> = self
            .source
            .iter()
            .map(|value| value.to_json_string())
            .collect();

        let outputs_json: Vec<String> = self
            .output
            .iter()
            .map(|value| value.to_json_string())
            .collect();

        // For environment, we'll serialize facts count as a placeholder
        // Full serialization of MORK Space would require more complex handling
        let env_json = format!(r#"{{"facts_count":{}}}"#, self.environment.rule_count());

        format!(
            r#"{{"source":[{}],"environment":{},"output":[{}]}}"#,
            source_json.join(","),
            env_json,
            outputs_json.join(",")
        )
    }
}

impl From<MettaValue> for MettaState {
    /// Create a compiled state containing an error s-expression
    /// Used when parsing fails to allow error handling at the evaluation level
    fn from(error_sexpr: MettaValue) -> Self {
        MettaState {
            source: vec![error_sexpr],
            environment: Environment::new(),
            output: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile;
    use crate::backend::models::Rule;

    #[test]
    fn test_to_json_empty() {
        let state = MettaState::new_empty();
        let json = state.to_json_string();

        // Should have empty arrays for source and output, and facts_count 0
        assert_eq!(
            json,
            r#"{"source":[],"environment":{"facts_count":0},"output":[]}"#
        );
    }

    #[test]
    fn test_to_json_with_source() {
        let state = MettaState::new_compiled(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(42),
        ]);
        let json = state.to_json_string();

        // Should contain source values
        assert!(json.contains(r#""source":["#));
        assert!(json.contains(r#"{"type":"atom","value":"test"}"#));
        assert!(json.contains(r#"{"type":"number","value":42}"#));
        assert!(json.contains(r#""environment":{"facts_count":0}"#));
        assert!(json.contains(r#""output":[]"#));
    }

    #[test]
    fn test_to_json_with_output() {
        let state = MettaState::new_accumulated(
            Environment::new(),
            vec![
                MettaValue::Bool(true),
                MettaValue::String("result".to_string()),
            ],
        );
        let json = state.to_json_string();

        // Should contain output values
        assert!(json.contains(r#""source":[]"#));
        assert!(json.contains(r#""environment":{"facts_count":0}"#));
        assert!(json.contains(r#""output":["#));
        assert!(json.contains(r#"{"type":"bool","value":true}"#));
        assert!(json.contains(r#"{"type":"string","value":"result"}"#));
    }

    #[test]
    fn test_to_json_with_environment() {
        let mut env = Environment::new();
        env.add_rule(Rule {
            lhs: MettaValue::Atom("x".to_string()),
            rhs: MettaValue::Long(1),
        });
        env.add_rule(Rule {
            lhs: MettaValue::Atom("y".to_string()),
            rhs: MettaValue::Long(2),
        });

        let state = MettaState::new_accumulated(env, Vec::new());
        let json = state.to_json_string();

        // Should show facts_count as 2
        assert!(json.contains(r#""environment":{"facts_count":2}"#));
    }

    #[test]
    fn test_to_json_complete() {
        let mut env = Environment::new();
        env.add_rule(Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        });

        let state = MettaState {
            source: vec![MettaValue::Atom("test".to_string())],
            environment: env,
            output: vec![MettaValue::Long(10)],
        };

        let json = state.to_json_string();

        // Should contain all fields
        assert!(json.contains(r#""source":["#));
        assert!(json.contains(r#"{"type":"atom","value":"test"}"#));
        assert!(json.contains(r#""environment":{"facts_count":1}"#));
        assert!(json.contains(r#""output":["#));
        assert!(json.contains(r#"{"type":"number","value":10}"#));
    }

    #[test]
    fn test_to_json_sexpr_values() {
        let state = MettaState {
            source: vec![MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ])],
            environment: Environment::new(),
            output: vec![MettaValue::SExpr(vec![
                MettaValue::Atom("result".to_string()),
                MettaValue::Long(3),
            ])],
        };

        let json = state.to_json_string();

        // Should properly serialize s-expressions
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""items""#));
    }

    #[test]
    fn test_to_json() {
        let src = "(+ 1 2)";
        let state = compile(src).unwrap();
        let json = state.to_json_string();

        // Should return full MettaState with source, environment, output
        assert!(json.contains(r#""source""#));
        assert!(json.contains(r#""environment""#));
        assert!(json.contains(r#""output""#));
        assert!(json.contains(r#""type":"sexpr""#));
    }
}
