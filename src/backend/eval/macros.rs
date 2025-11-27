macro_rules! require_args {
    ($op:expr, $items:expr, $expected:expr, $env:expr) => {
        if $items.len() < $expected + 1 {
            let got = $items.len().saturating_sub(1);
            let args_str = (1..=$expected)
                .map(|i| format!("arg{}", i))
                .collect::<Vec<_>>()
                .join(" ");
            let err = MettaValue::Error(
                format!(
                    "{} requires exactly {} argument{}, got {}. Usage: ({} {})",
                    $op,
                    $expected,
                    if $expected == 1 { "" } else { "s" },
                    got,
                    $op,
                    args_str
                ),
                Box::new(MettaValue::SExpr($items.to_vec())),
            );
            return (vec![err], $env);
        }
    };
}

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

macro_rules! require_one_arg {
    ($op:expr, $items:expr, $env:expr) => {
        require_args!($op, $items, 1, $env)
    };
}

macro_rules! require_two_args {
    ($op:expr, $items:expr, $env:expr) => {
        require_args!($op, $items, 2, $env)
    };
}

macro_rules! require_three_args {
    ($op:expr, $items:expr, $env:expr) => {
        require_args!($op, $items, 3, $env)
    };
}
