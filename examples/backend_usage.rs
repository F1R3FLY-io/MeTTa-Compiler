// Example usage of the MeTTa backend (arena-based API)

use mettatron::{compile, eval, new_env};

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

    let src = "(+ 10 5)";
    let state = compile(src).expect("Compilation failed");

    println!("Source: {}", src);
    println!("Compiled: {:?}", state.source());

    let env = new_env();
    let (results, _new_env) = eval(state.source()[0], env, &state);
    println!("Result: {:?}\n", results);
}

fn example_rules() {
    println!("--- Example 2: Pattern Matching with Rules ---");

    // Define rule via MeTTa source: (= (double $x) (* $x 2))
    let rule_src = "(= (double $x) (* $x 2))";
    let rule_state = compile(rule_src).expect("Compilation failed");
    let env = new_env();
    let (_, env) = eval(rule_state.source()[0], env, &rule_state);

    // Evaluate (double 7)
    let expr_src = "!(double 7)";
    let expr_state = compile(expr_src).expect("Compilation failed");

    println!("Rule: {}", rule_src);
    println!("Expression: (double 7)");

    let (results, _) = eval(expr_state.source()[0], env, &expr_state);
    println!("Result: {:?}\n", results);
}

fn example_environment() {
    println!("--- Example 3: Compositional Environments ---");

    let src1 = "!(+ 1 2)";
    let src2 = "!(* 3 4)";

    let state1 = compile(src1).expect("Compilation failed");
    let state2 = compile(src2).expect("Compilation failed");

    let env = new_env();

    println!("Expression 1: {}", src1);
    let (result1, env_after1) = eval(state1.source()[0], env.clone(), &state1);
    println!("Result 1: {:?}", result1);

    println!("\nExpression 2: {}", src2);
    let (result2, env_after2) = eval(state2.source()[0], env, &state2);
    println!("Result 2: {:?}", result2);

    // Union the environments (compositional)
    let _combined_env = env_after1.union(&env_after2);
    println!("\nCombined environment unioned successfully");
    println!("(All facts stored in MORK Space)");
}
