use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use std::sync::Arc;
use tracing::trace;

// TODO -> provide docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md
// TODO -> update examples/ directory

/// Cons atom: (cons-atom head tail)
/// Constructs an expression using two arguments
/// Example: (cons-atom a (b c)) -> (a b c)
pub(super) fn eval_cons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_cons_atom", ?items);
    require_args_with_usage!("cons-atom", items, 2, env, "(cons-atom head tail)");

    if !matches!(&items[1], MettaValue::Atom(_)) {
        let err = MettaValue::Error(
            format!("cons-atom: first argument must be an Atom"),
            Arc::new(items[1].clone()),
        );
        return (vec![err], env);
    }

    // TODO ->
    // validate if args[1] is Atom
    // validate if args[2] is SExpr?

    // TODO -> should handle:
    // (cons-atom a b)
    // [(Error (cons-atom a b) expected: (cons-atom <head> (: <tail> Expression)), found: (cons-atom a b))]

    dbg!(items);

    // PLACEHOLDER: Implement cons-atom logic here
    // Should:
    // 1. Extract head (items[1]) and tail (items[2])
    // 2. Verify both are Atoms
    // 3. Return SExpr([head, tail])

    (vec![MettaValue::Nil], env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;
    use crate::eval;

    #[test]
    fn test_cons_atom_basic() {
        let env = Environment::new();

        // let source = "(cons-atom a (+ c d))";
        let source = "(cons-atom True (false))";
        let state = compile(source).unwrap();
        assert_eq!(state.source.len(), 1);
        dbg!(state);

        // let (results, _) = eval(state.source[0].clone(), env);
        // dbg!(results);

        // assert_eq!(
        //     results.len(),
        //     1,
        //     "cons-atom should return exactly one result"
        // );
    }
}
