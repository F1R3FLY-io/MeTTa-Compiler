use mettatron::MettaValue;
use proptest::prelude::*;

pub fn empty_strings(limit: usize) -> impl Strategy<Value = String> {
    let regex = format!(" {{0,{}}}", limit);
    prop::string::string_regex(&regex).unwrap()
}

pub fn primitive() -> impl Strategy<Value = String> {
    metta_primitive().prop_map(|atom| atom.to_mork_string())
}

fn metta_primitive() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        // Basic primitives
        Just(MettaValue::Unit()),
        any::<bool>().prop_map(MettaValue::Bool),
        (-1_000_000i64..1_000_000i64).prop_map(MettaValue::Long),
        (-1e6f64..1e6f64)
            .prop_filter("valid float", |f| f.is_finite())
            .prop_map(MettaValue::Float),
        // Atoms (symbols, variables, keywords)
        metta_atom(),
        // String literals
        metta_simple_string(),
        // Simple errors (non-recursive)
        metta_simple_error(),
        // Simple types (non-recursive)
        metta_simple_type(),
    ]
    .boxed()
}

pub fn metta_atom() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        // Variables (start with $)
        prop::string::string_regex(r"\$[a-zA-Z][a-zA-Z0-9_-]{0,19}")
            .unwrap()
            .prop_map(|s| MettaValue::Atom(s)),
        // Regular symbols
        prop::string::string_regex(r"[a-zA-Z][a-zA-Z0-9_-]{0,19}")
            .unwrap()
            .prop_map(|s| MettaValue::Atom(s)),
        // Special forms
        prop_oneof![
            Just("="),
            Just(":"),
            Just("quote"),
            Just("if"),
            Just("error"),
            Just("is-error"),
            Just("catch"),
            Just("eval"),
            Just("function"),
            Just("return"),
            Just("chain"),
            Just("match"),
            Just("case"),
            Just("switch"),
            Just("let"),
            Just("get-type"),
            Just("check-type"),
            Just("map-atom"),
            Just("filter-atom"),
            Just("foldl-atom"),
        ]
        .prop_map(|s| MettaValue::Atom(s)),
    ]
}

fn metta_simple_string() -> impl Strategy<Value = MettaValue> {
    // Printable ASCII, excluding double quote and backslash
    prop::string::string_regex(r"[\x20-\x21\x23-\x5B\x5D-\x7E]{0,30}")
        .unwrap()
        .prop_map(|s| MettaValue::String(s))
}

fn metta_simple_error() -> impl Strategy<Value = MettaValue> {
    // HE-bisimilar Error(offending, detail): the detail slot typically carries
    // a structured atom like `BadType` (Hyperon spec) or a message string. The
    // offending slot is the atom that triggered the error.
    let detail_atoms = prop_oneof![
        Just("BadType"),
        Just("IncorrectNumberOfArguments"),
        Just("TypeError"),
    ]
    .prop_map(|s| MettaValue::Atom(s));

    (metta_atom(), detail_atoms)
        .prop_map(|(offending, detail)| MettaValue::Error(offending, detail))
}

fn metta_simple_type() -> impl Strategy<Value = MettaValue> {
    prop_oneof![
        // Basic type atoms
        Just(MettaValue::Type(MettaValue::Atom("Type"))),
        Just(MettaValue::Type(MettaValue::Atom("ErrorType"))),
        Just(MettaValue::Type(MettaValue::Atom("SpaceType"))),
    ]
}
