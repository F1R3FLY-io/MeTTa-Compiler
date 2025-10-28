use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, Rule};

use super::EvalOutput;

/// Rule definition: (= lhs rhs) - add to MORK Space and rule cache
pub(super) fn eval_add(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("=", items, env);

    let lhs = items[1].clone();
    let rhs = items[2].clone();
    let mut new_env = env.clone();

    // Add rule using add_rule (stores in both rule_cache and MORK Space)
    new_env.add_rule(Rule { lhs, rhs });

    // Return empty list (rule definitions don't produce output)
    return (vec![], new_env);
}

/// Evaluate match: (match <space-ref> <space-name> <pattern> <template>)
/// Searches the space for all atoms matching the pattern and returns instantiated templates
///
/// Optimized to use Environment::match_space which performs pattern matching
/// directly on MORK expressions without unnecessary intermediate allocations
pub(super) fn eval_match(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    let args = &items[1..];

    if args.len() < 4 {
        let err = MettaValue::Error(
            "match requires 4 arguments: &, space-name, pattern, and template".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let space_ref = &args[0];
    let space_name = &args[1];
    let pattern = &args[2];
    let template = &args[3];

    // Check that first arg is & (space reference operator)
    match space_ref {
        MettaValue::Atom(s) if s == "&" => {
            // Check space name (for now, only support "self")
            match space_name {
                MettaValue::Atom(name) if name == "self" => {
                    // Use optimized match_space method that works directly with MORK
                    let results = env.match_space(pattern, template);
                    (results, env)
                }
                _ => {
                    let err = MettaValue::Error(
                        format!(
                            "match only supports 'self' as space name, got: {:?}",
                            space_name
                        ),
                        Box::new(MettaValue::SExpr(args.to_vec())),
                    );
                    (vec![err], env)
                }
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!("match requires & as first argument, got: {:?}", space_ref),
                Box::new(MettaValue::SExpr(args.to_vec())),
            );
            (vec![err], env)
        }
    }
}

// TODO -> tests
