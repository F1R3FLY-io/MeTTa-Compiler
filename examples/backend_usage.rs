// Example usage of the new MeTTa backend

use mettatron::backend::*;

fn main() {
    println!("=== MeTTa Backend Usage Examples ===\n");

    // Example 1: Basic arithmetic
    example_arithmetic();

    // Example 2: Pattern matching with rules
    example_rules();

    // Example 3: Working with environments
    example_environment();
}

fn example_arithmetic() {
    println!("--- Example 1: Basic Arithmetic ---");

    // Compile MeTTa source code
    let src = "(+ 10 5)";
    let state = compile(src).expect("Compilation failed");

    println!("Source: {}", src);
    println!("Compiled: {:?}", state.source);

    // Evaluate the expression
    let (results, _new_env) = eval(state.source[0].clone(), state.environment);
    println!("Result: {:?}\n", results);
}

fn example_rules() {
    println!("--- Example 2: Pattern Matching with Rules ---");

    // Create an environment and add a rule: (= (double $x) (mul $x 2))
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

    // Evaluate (double 7)
    let expr = MettaValue::SExpr(vec![
        MettaValue::Atom("double".to_string()),
        MettaValue::Long(7),
    ]);

    println!("Rule: (= (double $x) (mul $x 2))");
    println!("Expression: (double 7)");

    let (results, _) = eval(expr, env);
    println!("Result: {:?}\n", results);
}

fn example_environment() {
    println!("--- Example 3: Compositional Environments ---");

    // Compile multiple expressions
    let src1 = "(+ 1 2)";
    let src2 = "(* 3 4)";

    let state1 = compile(src1).expect("Compilation failed");
    let state2 = compile(src2).expect("Compilation failed");

    println!("Expression 1: {}", src1);
    let (result1, env_after1) = eval(state1.source[0].clone(), state1.environment);
    println!("Result 1: {:?}", result1);

    println!("\nExpression 2: {}", src2);
    let (result2, env_after2) = eval(state2.source[0].clone(), state2.environment);
    println!("Result 2: {:?}", result2);

    // Union the environments (compositional)
    let _combined_env = env_after1.union(&env_after2);
    println!("\nCombined environment unioned successfully");
    println!("(All facts stored in MORK Space)");
}
