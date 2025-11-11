use mettatron::backend::environment::Environment;
use mettatron::backend::eval::eval;
use mettatron::backend::models::MettaValue;
use mork_expr::Expr;

fn main() {
    let env = Environment::new();

    // Define a rule
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

    println!("Rule definition:");
    println!("{:?}", rule_def);
    println!();

    let (result, new_env) = eval(rule_def.clone(), env);
    println!("Eval result: {:?}", result);
    println!();

    // Check if it's in the fact database
    let found = new_env.has_sexpr_fact(&rule_def);
    println!("Found in fact database: {}", found);
    println!();

    // Show what MORK representation looks like
    println!("Original MeTTa representation:");
    println!("{:?}", rule_def);
    println!();

    println!("MORK string representation:");
    println!("{}", rule_def.to_mork_string());
    println!();

    // Try to list all rules to see what's actually stored
    println!("All rules in environment:");
    for (i, rule) in new_env.iter_rules().enumerate() {
        println!("Rule {}: {:?}", i, rule);
        println!("  LHS MORK: {}", rule.lhs.to_mork_string());
        println!("  RHS MORK: {}", rule.rhs.to_mork_string());
    }
    println!();

    // Test structural equivalence directly
    let stored_rule_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("test-rule".to_string()),
            MettaValue::Atom("$a".to_string()),  // MORK's normalized variable
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("processed".to_string()),
            MettaValue::Atom("$a".to_string()),
        ]),
    ]);

    println!("Testing structural equivalence:");
    println!("  Original: {:?}", rule_def);
    println!("  Stored:   {:?}", stored_rule_sexpr);
    println!("  Equivalent? {}", rule_def.structurally_equivalent(&stored_rule_sexpr));
    println!();

    // Manual linear search to verify what's actually in MORK
    println!("Manual linear search through MORK database:");
    let space = new_env.space.lock().unwrap();
    let mut rz = space.btm.read_zipper();
    let mut count = 0;

    while rz.to_next_val() {
        let expr = Expr {
            ptr: rz.path().as_ptr().cast_mut(),
        };

        if let Ok(stored_value) = Environment::mork_expr_to_metta_value(&expr, &space) {
            println!("  Entry {}: {:?}", count, stored_value);
            println!("    Matches original? {}", rule_def.structurally_equivalent(&stored_value));
            count += 1;
        }
    }
    println!("  Total entries found: {}", count);
}
