use proptest::prelude::*;

#[derive(Debug, PartialEq, Clone)]
pub enum EnclosedDelimiter {
    OpenParen,
    CloseParen,
}

impl EnclosedDelimiter {
    pub fn to_str(&self) -> &str {
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
