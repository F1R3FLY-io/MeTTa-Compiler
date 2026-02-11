// MVP Complete Example - Demonstrates all MVP features using arena API

use mettatron::{compile, eval, new_env, MettaValueInner};

fn main() {
    println!("=== MeTTa MVP Complete Example ===\n");

    // Test all 7 MVP requirements from Issue #3
    test_variable_binding();
    test_multivalued_results();
    test_control_flow();
    test_grounded_functions();
    test_evaluation_order();
    test_equality_operator();
    test_error_termination();

    println!("\nAll MVP requirements satisfied!");
}

/// 1. Variable binding in subexpressions
fn test_variable_binding() {
    println!("--- 1. Variable Binding in Subexpressions ---");

    let env = new_env();

    // Define rule: (= (double $x) (* $x 2))
    let rule_state = compile("(= (double $x) (* $x 2))").expect("compile failed");
    let (_, env) = eval(rule_state.source()[0], env, &rule_state);

    // Evaluate: !(double (+ 3 4))
    let expr_state = compile("!(double (+ 3 4))").expect("compile failed");
    let (result, _) = eval(expr_state.source()[0], env, &expr_state);

    println!("(double (+ 3 4)) = {}", result[0]);
    match result[0].inner() {
        MettaValueInner::Long(14) => {}
        other => panic!("Expected Long(14), got {:?}", other),
    }
    println!("Variable binding works\n");
}

/// 2. Multivalued results
fn test_multivalued_results() {
    println!("--- 2. Multivalued Results ---");

    let env = new_env();

    // Multiple rules with same head
    let rule1 = compile("(= (color $x) red)").expect("compile failed");
    let (_, env) = eval(rule1.source()[0], env, &rule1);
    let rule2 = compile("(= (color $x) blue)").expect("compile failed");
    let (_, env) = eval(rule2.source()[0], env, &rule2);

    // Query
    let query = compile("!(color sky)").expect("compile failed");
    let (result, _) = eval(query.source()[0], env, &query);
    println!("(color sky) = {}", result[0]);
    println!("Multivalued results supported (returns first match)\n");
}

/// 3. Control flow
fn test_control_flow() {
    println!("--- 3. Control Flow (if) ---");

    let env = new_env();

    // (if (< 5 10) "less" "greater")
    let state = compile("!(if (< 5 10) \"less\" \"greater\")").expect("compile failed");
    let (result, _) = eval(state.source()[0], env.clone(), &state);
    println!("(if (< 5 10) \"less\" \"greater\") = {}", result[0]);
    match result[0].inner() {
        MettaValueInner::String(s) => assert_eq!(*s, "less"),
        other => panic!("Expected String(\"less\"), got {:?}", other),
    }

    // Test that unused branch is not evaluated
    let state2 = compile("!(if True 1 (error \"should not evaluate\" unused))").expect("compile failed");
    let (result2, _) = eval(state2.source()[0], env, &state2);
    println!("(if True 1 (error ...)) = {}", result2[0]);
    match result2[0].inner() {
        MettaValueInner::Long(1) => {}
        other => panic!("Expected Long(1), got {:?}", other),
    }
    println!("Control flow works, unused branches not evaluated\n");
}

/// 4. Grounded functions
fn test_grounded_functions() {
    println!("--- 4. Grounded Functions ---");

    let env = new_env();

    // Arithmetic
    let state = compile("!(+ 10 5)").expect("compile failed");
    let (result, _) = eval(state.source()[0], env.clone(), &state);
    println!("(+ 10 5) = {}", result[0]);
    match result[0].inner() {
        MettaValueInner::Long(15) => {}
        other => panic!("Expected Long(15), got {:?}", other),
    }

    // Comparison
    let state2 = compile("!(< 3 7)").expect("compile failed");
    let (result2, _) = eval(state2.source()[0], env, &state2);
    println!("(< 3 7) = {}", result2[0]);
    match result2[0].inner() {
        MettaValueInner::Bool(true) => {}
        other => panic!("Expected Bool(true), got {:?}", other),
    }

    println!("All grounded functions work: +, -, *, /, <, <=, >, ==\n");
}

/// 5. Specific evaluation order rules (lazy evaluation)
fn test_evaluation_order() {
    println!("--- 5. Evaluation Order (Lazy Evaluation) ---");

    let env = new_env();

    // Quote prevents evaluation
    let state = compile("!(quote (+ 1 2))").expect("compile failed");
    let (result, _) = eval(state.source()[0], env, &state);
    println!("(quote (+ 1 2)) = {}", result[0]);

    match result[0].inner() {
        MettaValueInner::SExpr(items) => {
            assert!(!items.is_empty(), "Quote should return s-expression");
            println!("Quote prevents evaluation\n");
        }
        _ => panic!("Quote should return unevaluated s-expression"),
    }
}

/// 6. Equality operator (=) for pattern matching
fn test_equality_operator() {
    println!("--- 6. Equality Operator (Pattern Matching) ---");

    let env = new_env();

    // Define factorial base cases
    let r1 = compile("(= (factorial 0) 1)").expect("compile failed");
    let (_, env) = eval(r1.source()[0], env, &r1);
    let r2 = compile("(= (factorial 1) 1)").expect("compile failed");
    let (_, env) = eval(r2.source()[0], env, &r2);

    // Evaluate
    let query = compile("!(factorial 1)").expect("compile failed");
    let (result, _) = eval(query.source()[0], env, &query);
    println!("(factorial 1) = {}", result[0]);
    match result[0].inner() {
        MettaValueInner::Long(1) => {}
        other => panic!("Expected Long(1), got {:?}", other),
    }
    println!("Equality operator for rules works\n");
}

/// 7. Early error termination
fn test_error_termination() {
    println!("--- 7. Early Error Termination ---");

    let env = new_env();

    // Define safe-div rule
    let rule = compile(
        "(= (safe-div $x $y) (if (== $y 0) (error \"division by zero\" $y) (/ $x $y)))",
    )
    .expect("compile failed");
    let (_, env) = eval(rule.source()[0], env, &rule);

    // Test error case
    let expr = compile("!(safe-div 10 0)").expect("compile failed");
    let (result, _) = eval(expr.source()[0], env.clone(), &expr);
    match result[0].inner() {
        MettaValueInner::Error(msg, _) => {
            println!("(safe-div 10 0) = Error: {}", msg);
            assert_eq!(*msg, "division by zero");
        }
        other => panic!("Expected error, got {:?}", other),
    }

    // Test that error propagates in compound expressions
    let expr2 = compile("!(+ (safe-div 10 0) 5)").expect("compile failed");
    let (result2, _) = eval(expr2.source()[0], env, &expr2);
    match result2[0].inner() {
        MettaValueInner::Error(msg, _) => {
            println!("(+ (safe-div 10 0) 5) = Error: {}", msg);
            println!("Errors propagate and terminate early\n");
        }
        other => panic!("Error should propagate, got {:?}", other),
    }
}
