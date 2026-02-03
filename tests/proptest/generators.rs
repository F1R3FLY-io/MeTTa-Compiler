use mettatron::MettaValue;
use proptest::prelude::*;

#[derive(Debug, PartialEq, Clone)]
enum EnclosedDelimiter {
    OpenParen,
    CloseParen,
}

impl EnclosedDelimiter {
    fn to_str(&self) -> &str {
        match self {
            EnclosedDelimiter::OpenParen => "(",
            EnclosedDelimiter::CloseParen => ")",
        }
    }
}

impl Arbitrary for EnclosedDelimiter {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(EnclosedDelimiter::OpenParen),
            Just(EnclosedDelimiter::CloseParen),
        ]
        .boxed()
    }
}

// TODO -> not sure about naming
fn primitive() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        Just(MettaValue::Nil),
        any::<bool>().prop_map(MettaValue::Bool),
        (-1_000_000i64..1_000_000i64).prop_map(MettaValue::Long),
        (-1e6f64..1e6f64)
            .prop_filter("valid float", |f| f.is_finite())
            .prop_map(MettaValue::Float),
    ]
    .boxed()
}

pub fn empty_strings(limit: usize) -> impl Strategy<Value = String> {
    let regex = format!(" {{0,{}}}", limit);
    prop::string::string_regex(&regex).unwrap()
}

pub fn atom() -> impl Strategy<Value = String> {
    primitive().prop_map(|atom| atom.to_mork_string())
}

pub fn valid_flat_sexpr(limit: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(atom(), 1..limit).prop_map(|vec| _build_sexpr_string("(", ")", vec))
}

pub fn invalid_flat_sexpr(limit: usize) -> impl Strategy<Value = String> {
    let atoms = prop::collection::vec(atom(), 1..limit);
    _wrap_invalid(atoms)
}

pub fn valid_nested_sexpr(
    depth: u32,
    size: u32,
    branch_size: u32,
) -> impl Strategy<Value = String> {
    primitive()
        .prop_recursive(
            depth,
            size,        // Shoot for maximum size of {size} nodes
            branch_size, // Put up to {branch_size} items per collection
            |inner| prop::collection::vec(inner.clone(), 0..10).prop_map(MettaValue::SExpr),
        )
        .prop_map(|sexpr| sexpr.to_mork_string())
}

pub fn invalid_nested_sexpr(
    depth: u32,
    size: u32,
    branch_size: u32,
) -> impl Strategy<Value = String> {
    // Use invalid base case
    let invalid_base = _wrap_invalid(prop::collection::vec(atom(), 0..3));

    invalid_base.prop_recursive(depth, size, branch_size, |inner| {
        let atoms = prop::collection::vec(inner.clone(), 0..10);
        _wrap_invalid(atoms)
    })
}

pub fn multiline_sexpr() -> impl Strategy<Value = String> {
    primitive()
        .prop_recursive(8, 256, 100, |inner| {
            prop::collection::vec(inner.clone(), 0..10).prop_map(MettaValue::SExpr)
        })
        .prop_map(|sexpr| format!("{}\n", sexpr.to_mork_string()))
}

fn _wrap_invalid(
    children_strategy: impl Strategy<Value = Vec<String>>,
) -> impl Strategy<Value = String> {
    (any::<EnclosedDelimiter>(), children_strategy)
        .prop_flat_map(|(open, atoms)| {
            let close = match open {
                EnclosedDelimiter::OpenParen => Just(EnclosedDelimiter::OpenParen).boxed(),
                EnclosedDelimiter::CloseParen => any::<EnclosedDelimiter>(),
            };

            (Just(open), close, Just(atoms))
        })
        .prop_map(|(open, close, atoms)| _build_sexpr_string(open.to_str(), close.to_str(), atoms))
}

fn _build_sexpr_string(open: &str, close: &str, items: Vec<String>) -> String {
    let mut res = String::new();
    res.push_str(open);

    for item in items {
        res.push_str(&item);
        res.push_str(" ")
    }

    res.push_str(close);
    res
}
