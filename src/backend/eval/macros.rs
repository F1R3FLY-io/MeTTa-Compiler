/// Require exact argument count with custom usage message
macro_rules! require_args_with_usage {
    ($op:expr, $items:expr, $expected:expr, $env:expr, $usage:expr) => {
        if $items.len() < $expected + 1 {
            let got = $items.len().saturating_sub(1);
            let err = MettaValue::Error(
                format!(
                    "{} requires exactly {} argument{}, got {}. Usage: {}",
                    $op,
                    $expected,
                    if $expected == 1 { "" } else { "s" },
                    got,
                    $usage
                ),
                std::sync::Arc::new(MettaValue::SExpr($items.to_vec())),
            );
            return (vec![err], $env);
        }
    };
}

/// Require exact argument count for builtin functions (returns MettaValue::Error, not EvalResult)
/// Supports optional usage message
macro_rules! require_builtin_args {
    // Version with usage message
    ($op_name:expr, $args:expr, $expected:expr, $usage:expr) => {
        if $args.len() != $expected {
            return MettaValue::Error(
                format!(
                    "{} requires exactly {} argument{}, got {}. Usage: {}",
                    $op_name,
                    $expected,
                    if $expected == 1 { "" } else { "s" },
                    $args.len(),
                    $usage
                ),
                std::sync::Arc::new(MettaValue::Atom("ArityError".to_string())),
            );
        }
    };
    // Version without usage message
    ($op_name:expr, $args:expr, $expected:expr) => {
        if $args.len() != $expected {
            return MettaValue::Error(
                format!(
                    "{} requires exactly {} argument{}, got {}",
                    $op_name,
                    $expected,
                    if $expected == 1 { "" } else { "s" },
                    $args.len()
                ),
                std::sync::Arc::new(MettaValue::Nil),
            );
        }
    };
}
