//! Tests for space operations.

#[cfg(test)]
mod tests {
    use crate::backend::environment::Environment;
    use crate::backend::models::MettaValue;
    use crate::eval;

    #[test]
    fn test_add_missing_arguments() {
        let env = Environment::new();

        // (=) - missing both arguments
        let value = MettaValue::SExpr(vec![MettaValue::Atom("=".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("="));
                assert!(msg.contains("requires exactly 2 arguments")); // Changed (note plural)
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_add_missing_one_argument() {
        let env = Environment::new();

        // (= lhs) - missing rhs
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("lhs".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("="));
                assert!(msg.contains("requires exactly 2 arguments")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_rule_definition() {
        let env = Environment::new();

        // (= (f) 42)
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            MettaValue::Long(42),
        ]);

        let (result, _new_env) = eval(rule_def, env);

        // Rule definition should return empty list
        assert!(result.is_empty());
    }

    #[test]
    fn test_rule_definition_with_function_patterns() {
        let env = Environment::new();

        // Test function rule: (= (double $x) (* $x 2))
        let function_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (result, new_env) = eval(function_rule, env);
        assert!(result.is_empty());

        // Test the function: (double 5) should return 10
        let test_function = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);
        let (results, _) = eval(test_function, new_env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    #[test]
    fn test_rule_definition_with_variable_consistency() {
        let env = Environment::new();

        // Test rule with repeated variables: (= (same $x $x) (duplicate $x))
        let consistency_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("duplicate".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(consistency_rule, env);
        assert!(result.is_empty());

        // Test matching with same values: (same 5 5)
        let test_same = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
        ]);
        let (results, new_env2) = eval(test_same, new_env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items[0], MettaValue::Atom("duplicate".to_string()));
                assert_eq!(items[1], MettaValue::Long(5));
            }
            _ => panic!("Expected S-expression"),
        }

        // Test non-matching with different values: (same 5 7)
        let test_different = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(test_different, new_env2);
        assert_eq!(results.len(), 1);
        // Should return the original expression as it doesn't match any rule
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items[0], MettaValue::Atom("same".to_string()));
                assert_eq!(items[1], MettaValue::Long(5));
                assert_eq!(items[2], MettaValue::Long(7));
            }
            _ => panic!("Expected S-expression"),
        }
    }

    #[test]
    fn test_multiple_rules_same_function() {
        let mut env = Environment::new();

        // Define multiple rules for the same function (factorial example)
        // (= (fact 0) 1)
        let fact_base = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Long(0),
            ]),
            MettaValue::Long(1),
        ]);
        let (_, env1) = eval(fact_base, env);
        env = env1;

        // (= (fact $n) (* $n (fact (- $n 1))))
        let fact_recursive = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("fact".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                ]),
            ]),
        ]);
        let (_, env2) = eval(fact_recursive, env);

        // Test base case: (fact 0) should return 1
        let test_base = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(0),
        ]);
        let (results, env3) = eval(test_base, env2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));

        // Test recursive case: (fact 3) should return 6
        let test_recursive = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(test_recursive, env3);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_rule_with_wildcard_patterns() {
        let env = Environment::new();

        // Test rule with wildcard: (= (ignore _ $x) $x)
        let wildcard_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("ignore".to_string()),
                MettaValue::Atom("_".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (result, new_env) = eval(wildcard_rule, env);
        assert!(result.is_empty());

        // Test with any first argument: (ignore "anything" 42)
        let test_wildcard = MettaValue::SExpr(vec![
            MettaValue::Atom("ignore".to_string()),
            MettaValue::String("anything".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(test_wildcard, new_env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_match_basic_functionality() {
        let mut env = Environment::new();

        // Add some facts to the space
        let fact1 = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("alice".to_string()),
            MettaValue::Long(25),
        ]);
        env.add_to_space(&fact1);

        let fact2 = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("bob".to_string()),
            MettaValue::Long(30),
        ]);
        env.add_to_space(&fact2);

        // Test basic match: (match & self (person $name $age) $name)
        let match_query = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("person".to_string()),
                MettaValue::Atom("$name".to_string()),
                MettaValue::Atom("$age".to_string()),
            ]),
            MettaValue::Atom("$name".to_string()),
        ]);

        let (results, _) = eval(match_query, env);
        assert!(
            results.len() >= 2,
            "Expected >= 2 results, got {:?}",
            results
        ); // Should return both alice and bob
        assert!(results.contains(&MettaValue::Atom("alice".to_string())));
        assert!(results.contains(&MettaValue::Atom("bob".to_string())));
    }

    #[test]
    fn test_match_with_specific_patterns() {
        let mut env = Environment::new();

        // Add some facts
        let facts = vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("alice".to_string()),
                MettaValue::Atom("coffee".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("bob".to_string()),
                MettaValue::Atom("tea".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("alice".to_string()),
                MettaValue::Atom("books".to_string()),
            ]),
        ];

        for fact in facts {
            env.add_to_space(&fact);
        }

        // Test specific match: (match & self (likes alice $what) $what)
        let specific_match = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("alice".to_string()),
                MettaValue::Atom("$what".to_string()),
            ]),
            MettaValue::Atom("$what".to_string()),
        ]);

        let (results, _) = eval(specific_match, env);
        assert!(results.len() >= 2); // Should return coffee and books
        assert!(results.contains(&MettaValue::Atom("coffee".to_string())));
        assert!(results.contains(&MettaValue::Atom("books".to_string())));
        assert!(!results.contains(&MettaValue::Atom("tea".to_string()))); // bob's preference
    }

    #[test]
    fn test_match_with_complex_templates() {
        let mut env = Environment::new();

        // Add facts
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("student".to_string()),
            MettaValue::Atom("john".to_string()),
            MettaValue::Atom("math".to_string()),
            MettaValue::Long(85),
        ]);
        env.add_to_space(&fact);

        // Test complex template: (match & self (student $name $subject $grade) (result $name scored $grade in $subject))
        let complex_match = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("student".to_string()),
                MettaValue::Atom("$name".to_string()),
                MettaValue::Atom("$subject".to_string()),
                MettaValue::Atom("$grade".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("result".to_string()),
                MettaValue::Atom("$name".to_string()),
                MettaValue::Atom("scored".to_string()),
                MettaValue::Atom("$grade".to_string()),
                MettaValue::Atom("in".to_string()),
                MettaValue::Atom("$subject".to_string()),
            ]),
        ]);

        let (results, _) = eval(complex_match, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 6);
                assert_eq!(items[0], MettaValue::Atom("result".to_string()));
                assert_eq!(items[1], MettaValue::Atom("john".to_string()));
                assert_eq!(items[2], MettaValue::Atom("scored".to_string()));
                assert_eq!(items[3], MettaValue::Long(85));
                assert_eq!(items[4], MettaValue::Atom("in".to_string()));
                assert_eq!(items[5], MettaValue::Atom("math".to_string()));
            }
            _ => panic!("Expected complex template result"),
        }
    }

    #[test]
    fn test_match_error_cases() {
        let env = Environment::new();

        // Test match with insufficient arguments
        // (match & self) has 2 args after "match": ["&", "self"]
        // This is missing the pattern and template arguments
        let match_insufficient = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
        ]);
        let (results, _) = eval(match_insufficient, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("match"), "Expected 'match' in: {}", msg);
                assert!(
                    msg.contains("3 or 4 arguments"),
                    "Expected '3 or 4 arguments' in: {}",
                    msg
                );
                // We have 2 args: ["&", "self"]
                assert!(msg.contains("got 2"), "Expected 'got 2' in: {}", msg);
                assert!(msg.contains("Usage:"), "Expected 'Usage:' in: {}", msg);
            }
            _ => panic!("Expected error for insufficient arguments"),
        }

        // Test match with wrong space reference
        let match_wrong_ref = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("wrong".to_string()), // Should be &
            MettaValue::Atom("self".to_string()),
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);
        let (results, _) = eval(match_wrong_ref, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("match requires & as first argument"));
            }
            _ => panic!("Expected error for wrong space reference"),
        }

        // Test match with unsupported space name (new-style syntax)
        // Note: With the new space_ref token, `& other` is preprocessed to `&other`
        // which triggers 3-arg new-style syntax, producing a "must be a space" error
        let match_wrong_space = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&other".to_string()), // Unrecognized space reference
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);
        let (results, _) = eval(match_wrong_space, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("must be a space"),
                    "Expected 'must be a space' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error for unsupported space name"),
        }
    }

    #[test]
    fn test_rule_definition_with_errors_in_rhs() {
        let env = Environment::new();

        // Test rule with error in RHS: (= (error-func $x) (error "always fails" $x))
        let error_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error-func".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("always fails".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(error_rule, env);
        assert!(result.is_empty());

        // Test the error-producing rule
        let test_error = MettaValue::SExpr(vec![
            MettaValue::Atom("error-func".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(test_error, new_env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "always fails");
                assert_eq!(**details, MettaValue::Long(42));
            }
            _ => panic!("Expected error from rule"),
        }
    }

    #[test]
    fn test_rule_precedence_and_specificity() {
        let mut env = Environment::new();

        // Define general rule first: (= (test $x) (general $x))
        let general_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("general".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);
        let (_, env1) = eval(general_rule, env);
        env = env1;

        // Define specific rule: (= (test 42) specific-case)
        let specific_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("specific-case".to_string()),
        ]);
        let (_, env2) = eval(specific_rule, env);

        // Test that specific rule takes precedence: (test 42)
        let test_specific = MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, env3) = eval(test_specific, env2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("specific-case".to_string()));

        // Test that general rule still works for other values: (test 100)
        let test_general = MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(100),
        ]);
        let (results, _) = eval(test_general, env3);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items[0], MettaValue::Atom("general".to_string()));
                assert_eq!(items[1], MettaValue::Long(100));
            }
            _ => panic!("Expected general rule result"),
        }
    }

    #[test]
    fn test_recursive_rules() {
        let env = Environment::new();

        // Define recursive rule: (= (countdown $n) (if (> $n 0) (countdown (- $n 1)) done))
        let recursive_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("countdown".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("countdown".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                ]),
                MettaValue::Atom("done".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(recursive_rule, env);
        assert!(result.is_empty());

        // Test recursive execution: (countdown 0) should return "done"
        let test_base = MettaValue::SExpr(vec![
            MettaValue::Atom("countdown".to_string()),
            MettaValue::Long(0),
        ]);
        let (results, new_env2) = eval(test_base, new_env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("done".to_string()));

        // Test recursive call: (countdown 1) should eventually return "done"
        let test_recursive = MettaValue::SExpr(vec![
            MettaValue::Atom("countdown".to_string()),
            MettaValue::Long(1),
        ]);
        let (results, _) = eval(test_recursive, new_env2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("done".to_string()));
    }

    #[test]
    fn test_match_with_no_results() {
        let env = Environment::new();

        // Test match with pattern that doesn't match anything
        let no_match = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nonexistent".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(no_match, env);
        assert!(results.is_empty()); // No matches should return empty
    }

    #[test]
    fn test_rule_with_different_variable_types() {
        let env = Environment::new();

        // Test rule with different variable prefixes: (= (mixed $a &b 'c) (result $a &b 'c))
        let mixed_vars_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("&b".to_string()),
                MettaValue::Atom("'c".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("result".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("&b".to_string()),
                MettaValue::Atom("'c".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(mixed_vars_rule, env);
        assert!(result.is_empty());

        // Test the mixed variables rule
        let test_mixed = MettaValue::SExpr(vec![
            MettaValue::Atom("mixed".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(test_mixed, new_env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 4);
                assert_eq!(items[0], MettaValue::Atom("result".to_string()));
                assert_eq!(items[1], MettaValue::Long(1));
                assert_eq!(items[2], MettaValue::Long(2));
                assert_eq!(items[3], MettaValue::Long(3));
            }
            _ => panic!("Expected result with mixed variables"),
        }
    }

    #[test]
    fn test_rule_definition_in_fact_database() {
        let env = Environment::new();

        // Define a rule and verify it's added to the fact database
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test-rule".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("processed".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(rule_def.clone(), env);
        assert!(result.is_empty());

        // Verify the rule definition is in the fact database
        assert!(new_env.has_sexpr_fact(&rule_def));
    }

    // Tests for "Did You Mean" space name suggestions

    #[test]
    fn test_space_name_case_sensitivity_suggestion() {
        let env = Environment::new();

        // Test "&Self" (capital S) -> should error (unrecognized space reference)
        // Note: With the new space_ref token, &Self is a single atom, triggering new-style syntax
        let match_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&Self".to_string()), // Wrong case - combined as single token
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);

        let (results, _) = eval(match_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // New-style syntax produces different error (no suggestion)
                assert!(
                    msg.contains("must be a space"),
                    "Expected 'must be a space' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error for unrecognized space reference"),
        }
    }

    #[test]
    fn test_space_name_typo_suggestion() {
        let env = Environment::new();

        // Test "&slef" (typo) -> should error (unrecognized space reference)
        // Note: With the new space_ref token, &slef is a single atom, triggering new-style syntax
        let match_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&slef".to_string()), // Typo - combined as single token
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);

        let (results, _) = eval(match_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // New-style syntax produces different error (no suggestion)
                assert!(
                    msg.contains("must be a space"),
                    "Expected 'must be a space' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error for typo space reference"),
        }
    }

    #[test]
    fn test_space_name_no_suggestion_for_unrelated() {
        let env = Environment::new();

        // Test "foobar" -> no suggestion (too different from "self")
        let match_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("foobar".to_string()), // Completely different
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);

        let (results, _) = eval(match_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // Should NOT contain "Did you mean" for completely unrelated names
                assert!(
                    !msg.contains("Did you mean"),
                    "Should not have suggestion for unrelated name: {}",
                    msg
                );
            }
            _ => panic!("Expected error without suggestion"),
        }
    }

    // =========================================================================
    // Phase G: Advanced Nondeterminism Tests
    // =========================================================================

    #[test]
    fn test_amb_with_multiple_alternatives() {
        let env = Environment::new();

        // (amb 1 2 3) should return 1, 2, 3 as separate results
        let amb_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("amb".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);

        let (results, _) = eval(amb_expr, env);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], MettaValue::Long(1));
        assert_eq!(results[1], MettaValue::Long(2));
        assert_eq!(results[2], MettaValue::Long(3));
    }

    #[test]
    fn test_amb_empty_fails() {
        let env = Environment::new();

        // (amb) with no alternatives should return empty (nondeterministic failure)
        let amb_expr = MettaValue::SExpr(vec![MettaValue::Atom("amb".to_string())]);

        let (results, _) = eval(amb_expr, env);
        assert!(results.is_empty(), "Empty amb should return no results");
    }

    #[test]
    fn test_amb_single_alternative() {
        let env = Environment::new();

        // (amb 42) with single alternative
        let amb_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("amb".to_string()),
            MettaValue::Long(42),
        ]);

        let (results, _) = eval(amb_expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_guard_passes_on_true() {
        let env = Environment::new();

        // (guard True) should return Unit
        let guard_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("guard".to_string()),
            MettaValue::Bool(true),
        ]);

        let (results, _) = eval(guard_expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_guard_fails_on_false() {
        let env = Environment::new();

        // (guard False) should return empty (nondeterministic failure)
        let guard_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("guard".to_string()),
            MettaValue::Bool(false),
        ]);

        let (results, _) = eval(guard_expr, env);
        assert!(
            results.is_empty(),
            "Guard with false should return no results"
        );
    }

    #[test]
    fn test_guard_type_error() {
        let env = Environment::new();

        // (guard 42) should return a type error
        let guard_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("guard".to_string()),
            MettaValue::Long(42),
        ]);

        let (results, _) = eval(guard_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("Bool"),
                    "Error should mention Bool type: {}",
                    msg
                );
            }
            _ => panic!("Expected type error for non-boolean guard condition"),
        }
    }

    #[test]
    fn test_commit_returns_unit() {
        let env = Environment::new();

        // (commit) should return Unit
        let commit_expr = MettaValue::SExpr(vec![MettaValue::Atom("commit".to_string())]);

        let (results, _) = eval(commit_expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);
    }

    #[test]
    fn test_backtrack_returns_empty() {
        let env = Environment::new();

        // (backtrack) should return empty (nondeterministic failure)
        let backtrack_expr = MettaValue::SExpr(vec![MettaValue::Atom("backtrack".to_string())]);

        let (results, _) = eval(backtrack_expr, env);
        assert!(results.is_empty(), "Backtrack should return no results");
    }

    #[test]
    fn test_amb_with_evaluated_expressions() {
        let env = Environment::new();

        // (amb (+ 1 1) (+ 2 2)) should evaluate each alternative
        let amb_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("amb".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(1),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(amb_expr, env);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], MettaValue::Long(2)); // 1+1
        assert_eq!(results[1], MettaValue::Long(4)); // 2+2
    }

    #[test]
    fn test_guard_with_evaluated_condition() {
        let env = Environment::new();

        // (guard (== 2 2)) should pass
        let guard_true_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("guard".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("==".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(guard_true_expr, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);

        // (guard (== 2 3)) should fail
        let guard_false_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("guard".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("==".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);

        let (results, _) = eval(guard_false_expr, env);
        assert!(results.is_empty());
    }
}
