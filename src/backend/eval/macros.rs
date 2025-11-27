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
                Box::new(MettaValue::SExpr($items.to_vec())),
            );
            return (vec![err], $env);
        }
    };
}
