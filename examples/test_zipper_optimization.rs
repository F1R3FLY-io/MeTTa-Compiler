use mettatron::backend::types::{Environment, MettaValue};
use mettatron::backend::eval::eval;

fn main() {
    let env = Environment::new();

    // Define a rule: (= (double $x) (* $x 2))
    let rule_def = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("mul".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]),
    ]);

    println!("Original rule_def:");
    println!("  {:?}", rule_def);
    println!("\nOriginal rule_def as MORK:");
    println!("  {}", rule_def.to_mork_string());

    let (result, new_env) = eval(rule_def.clone(), env);

    // Rule definition should return Nil
    println!("\nEval result: {:?}", result[0]);
    assert_eq!(result[0], MettaValue::Nil);

    // Dump the space to see what's actually stored
    let space = new_env.space.lock().unwrap();
    let mut sexprs_bytes = Vec::new();
    space.dump_all_sexpr(&mut sexprs_bytes).unwrap();
    let sexprs_str = String::from_utf8_lossy(&sexprs_bytes);

    println!("\n=== MORK Space dump ===");
    for line in sexprs_str.lines() {
        println!("{}", line);
    }
    println!("=== End ===\n");

    drop(space);

    // Try to find with original variable names
    println!("Checking has_sexpr_fact with original rule_def:");
    let found_original = new_env.has_sexpr_fact(&rule_def);
    println!("  Found: {}", found_original);
    assert!(found_original, "Should find rule with original variable names");

    // Try to find with modified variable names ($a instead of $x)
    let modified_rule_def = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$a".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("mul".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Long(2),
        ]),
    ]);

    println!("\nChecking has_sexpr_fact with $a instead of $x:");
    let found_modified = new_env.has_sexpr_fact(&modified_rule_def);
    println!("  Found: {}", found_modified);
    assert!(found_modified, "Should find rule even with different variable names (structural equivalence)");

    println!("\n✓ All assertions passed! Zipper-based implementation works correctly.");
}
