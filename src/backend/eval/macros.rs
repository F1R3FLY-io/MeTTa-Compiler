macro_rules! require_args {
    ($op:expr, $items:expr, $expected:expr, $env:expr) => {
        if $items.len() < $expected + 1 {
            let err = MettaValue::Error(
                format!(
                    "{} requires exactly {} argument{}",
                    $op,
                    $expected,
                    if $expected == 1 { "" } else { "s" }
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
