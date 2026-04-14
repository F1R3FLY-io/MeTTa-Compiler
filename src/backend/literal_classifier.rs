//! Unified literal classifier for MeTTa symbols.
//!
//! Mirrors mettail-rust's lexer design pattern: a single linear-scan DFA over
//! the symbol bytes determines the literal kind in O(n), then a single
//! targeted parser call produces the typed value. This is significantly
//! cheaper than try-each-type fallback (which can do up to N independent
//! passes for N possible types) and matches the encoder's contract:
//! every value variant has a textual form whose shape uniquely identifies
//! the original type.
//!
//! ## Encoder contract this classifier mirrors
//!
//! The MORK encoder (`src/backend/mork_convert.rs:485-523`,
//! `src/parser/mod.rs`, and the tree-sitter parser) writes each literal
//! variant in a distinct shape:
//!
//! | Variant   | Encoder        | Recognized form                                    |
//! |-----------|----------------|----------------------------------------------------|
//! | `Long`    | `itoa`         | `[-]?[0-9]+`                                       |
//! | `Float`   | `ryu`          | `[-]?[0-9]+\.[0-9]+([eE][-+]?[0-9]+)?` (or scientific) |
//! | `Bool`    | literal text   | `true` / `false`                                   |
//! | `String`  | quote-wrapped  | `"..."`                                            |
//! | (Atom)    | as-is          | anything else                                      |
//!
//! ## Why a DFA, not regex / try-fallback
//!
//! - **Single linear scan**: O(n) byte walk, no backtracking, no allocations.
//! - **Branch-predictable**: each state transition is a single byte match;
//!   modern CPUs predict the digit-loop branch ~100% on typical workloads.
//! - **No double-parse**: classification finishes BEFORE any `parse::<i64>()`
//!   or `parse::<f64>()` call, so we never pay for a failed parse attempt.
//! - **Inlinable**: marked `#[inline]`; LLVM fuses the classifier with the
//!   caller's `match` arm in optimized builds.
//!
//! ## Edge cases preserved (matches pre-refactor behavior)
//!
//! - Integer overflow → falls through to `Atom` (parser-side `parse::<i64>()`
//!   returns `Err`; caller treats as Atom).
//! - Special floats (`inf`, `nan`) → stay as `Atom` because they have no
//!   leading digit. PeTTa never serializes these via `ryu` in practice.
//! - Hex literals (`0x2A`) → `Atom`. Not supported by the encoder.
//! - Empty / single `-` / `1.2.3` → `Atom`.
//! - `truer`, `falses`, etc. → `Atom` (keyword exact-match only).

/// The classified literal kind. The classifier never allocates and never
/// touches the heap — it returns one of these tags by walking the bytes once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// Integer literal — guaranteed to parse via `i64::from_str` if and only
    /// if the value fits in `i64`. Callers MUST handle the overflow case
    /// (treat as `Atom`).
    Long,
    /// Float literal — guaranteed to parse via `f64::from_str` (the DFA only
    /// emits `Float` for shapes `f64` accepts).
    Float,
    /// `true` keyword — caller produces `factory.bool(true)`.
    BoolTrue,
    /// `false` keyword — caller produces `factory.bool(false)`.
    BoolFalse,
    /// Quoted string — caller strips the surrounding `"` and produces
    /// `factory.string(&s[1..s.len()-1])`. The DFA verifies both quotes
    /// are present and `s.len() >= 2`.
    String,
    /// Anything else — caller produces `factory.atom(s)`.
    Atom,
}

/// Classify a symbol's textual form via a single DFA pass.
///
/// Returns the literal kind in O(`s.len()`) time, no allocations. The
/// caller is expected to dispatch on the result and call the matching
/// factory method.
///
/// # Encoder contract
///
/// The classifier assumes the input was produced by one of MeTTaTron's
/// encoders (`itoa` for Long, `ryu` for Float, literal text for Bool /
/// String / Atom). Inputs that don't match any encoder shape fall through
/// to `Atom`. The classifier is **lossless** for round-trip from a
/// MettaValue → encoded form → classifier — every encoder-produced shape
/// classifies back to the original variant.
///
/// # Examples
///
/// ```ignore
/// use crate::backend::literal_classifier::{classify_symbol, SymbolKind};
///
/// assert_eq!(classify_symbol("42"),     SymbolKind::Long);
/// assert_eq!(classify_symbol("-42"),    SymbolKind::Long);
/// assert_eq!(classify_symbol("2.5"),    SymbolKind::Float);
/// assert_eq!(classify_symbol("-3.14"),  SymbolKind::Float);
/// assert_eq!(classify_symbol("1e10"),   SymbolKind::Float);
/// assert_eq!(classify_symbol("1.5E-3"), SymbolKind::Float);
/// assert_eq!(classify_symbol("true"),   SymbolKind::BoolTrue);
/// assert_eq!(classify_symbol("false"),  SymbolKind::BoolFalse);
/// assert_eq!(classify_symbol("\"hi\""), SymbolKind::String);
/// assert_eq!(classify_symbol("hello"),  SymbolKind::Atom);
/// assert_eq!(classify_symbol("1.2.3"),  SymbolKind::Atom);
/// assert_eq!(classify_symbol(""),       SymbolKind::Atom);
/// assert_eq!(classify_symbol("-"),      SymbolKind::Atom);
/// ```
#[inline]
pub fn classify_symbol(s: &str) -> SymbolKind {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return SymbolKind::Atom;
    }

    // Quick first-byte branch — branch-predictable and lets the cold paths
    // (string, keyword) live in their own basic blocks.
    let first = bytes[0];

    // String: must start AND end with `"` and be at least `""` (len >= 2).
    // Encoder writes strings as `"..."` literally.
    if first == b'"' {
        if len >= 2 && bytes[len - 1] == b'"' {
            return SymbolKind::String;
        }
        return SymbolKind::Atom;
    }

    // Bool keywords: exact match for `true` / `false`. Most symbols don't
    // start with `t` or `f`, so this is a cheap leading-byte filter.
    if first == b't' && len == 4 && &bytes[1..] == b"rue" {
        return SymbolKind::BoolTrue;
    }
    if first == b'f' && len == 5 && &bytes[1..] == b"alse" {
        return SymbolKind::BoolFalse;
    }

    // Numeric: `[-]?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?`
    // The DFA below walks the bytes once. State is implicit in the index `i`
    // and a few flags. No allocations, no recursion.
    let mut i: usize = 0;

    // Optional leading minus.
    if first == b'-' {
        if len < 2 {
            return SymbolKind::Atom; // bare `-` is not a number
        }
        i = 1;
    } else if !first.is_ascii_digit() {
        // Doesn't start with a digit (after optional `-`) → not a number.
        return SymbolKind::Atom;
    }

    // Integer-digits state: at least one digit required.
    // After the optional `-`, the next byte must be `0..=9`.
    if !bytes[i].is_ascii_digit() {
        return SymbolKind::Atom;
    }
    // Consume all leading digits.
    while i < len && bytes[i].is_ascii_digit() {
        i += 1;
    }

    // If we consumed everything → it's an integer.
    if i == len {
        return SymbolKind::Long;
    }

    let mut saw_fraction = false;

    // Optional fractional part: `.[0-9]+`
    if bytes[i] == b'.' {
        i += 1;
        // Must be followed by at least one digit (we don't accept `1.`)
        let frac_start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            // `.` with no digits after → not a number.
            return SymbolKind::Atom;
        }
        saw_fraction = true;
    }

    // Optional exponent: `[eE][-+]?[0-9]+`
    let mut saw_exponent = false;
    if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        // Optional sign
        if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        // At least one digit
        let exp_start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return SymbolKind::Atom;
        }
        saw_exponent = true;
    }

    // Anything left over → not a number.
    if i != len {
        return SymbolKind::Atom;
    }

    if saw_fraction || saw_exponent {
        SymbolKind::Float
    } else {
        // Pure-digit form with no decimal/exponent → integer (we already
        // returned early above for that case, so this is unreachable but
        // kept for clarity).
        SymbolKind::Long
    }
}

/// Strip the surrounding `"` from a `String`-classified symbol.
///
/// Caller invariant: `classify_symbol(s) == SymbolKind::String`.
/// The DFA guarantees `s.len() >= 2` and both ends are `"`.
#[inline]
pub fn unwrap_string_literal(s: &str) -> &str {
    debug_assert!(s.len() >= 2);
    debug_assert!(s.starts_with('"'));
    debug_assert!(s.ends_with('"'));
    &s[1..s.len() - 1]
}

// ============================================================================
// Single-pass classify-AND-parse — for hot deserialization / parsing paths
// ============================================================================

/// Classified literal with the parsed payload included.
///
/// Returned by [`classify_and_parse`], which performs **a single linear scan**
/// over the input that BOTH classifies the kind AND extracts the parsed
/// value (for Long, accumulated digit-by-digit during the scan; for Float,
/// validated then handed to `f64::from_str`; for Bool/String/Atom, no
/// further parsing needed).
///
/// `Atom` is the catch-all — the caller should use the original input.
/// `LongOverflow` distinguishes "looked like a Long but i64 overflowed"
/// from a generic Atom; callers can choose to fall through to `Atom`
/// (preserving existing behavior) or to `Float` reclassification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClassifiedLiteral<'a> {
    Long(i64),
    Float(f64),
    BoolTrue,
    BoolFalse,
    /// String content with surrounding `"` already stripped.
    String(&'a str),
    /// Looked like an integer but `checked_mul`/`checked_add` saturated.
    /// Callers typically treat this as `Atom` (preserves pre-refactor
    /// semantics) or as `Float` (lossy reclassification).
    LongOverflow,
    /// Catch-all — caller uses the original input as an atom.
    Atom,
}

/// Single-pass classify-AND-parse.
///
/// Walks the input bytes ONCE: the integer value is accumulated digit by
/// digit during the same scan that classifies the kind, eliminating the
/// `parse::<i64>` call afterward. Float still requires a final
/// `f64::from_str` (manual `f64` parsing is error-prone), but the shape
/// has already been validated by the scan, so the parse is guaranteed
/// to succeed barring `i64`-overflow-into-`f64`-overflow edge cases.
///
/// This is the API the hot deserialization path
/// (`mork_bytes_to_generic_value`) and the parser (`Parser::try_parse_number`)
/// should call. The kind-only [`classify_symbol`] function is kept as
/// a thin wrapper for callers that don't need the parsed value (e.g.
/// dispatch tables, traces).
///
/// # Invariants
///
/// For every input `s`, `classify_and_parse(s)` and `classify_symbol(s)`
/// agree on the kind:
///
/// | `classify_and_parse(s)` | `classify_symbol(s)` |
/// |---|---|
/// | `Long(_)`               | `SymbolKind::Long` |
/// | `Float(_)`              | `SymbolKind::Float` |
/// | `BoolTrue`              | `SymbolKind::BoolTrue` |
/// | `BoolFalse`             | `SymbolKind::BoolFalse` |
/// | `String(_)`             | `SymbolKind::String` |
/// | `LongOverflow`          | `SymbolKind::Long` (overflow safety belt) |
/// | `Atom`                  | `SymbolKind::Atom` |
#[inline]
pub fn classify_and_parse(s: &str) -> ClassifiedLiteral<'_> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return ClassifiedLiteral::Atom;
    }

    let first = bytes[0];

    // String: `"..."` — caller-side, the most common shape after numbers.
    if first == b'"' {
        if len >= 2 && bytes[len - 1] == b'"' {
            // SAFETY: bytes[1..len-1] is valid UTF-8 because the input was
            // valid UTF-8 and the trim removes only the leading/trailing `"`,
            // both of which are 1-byte ASCII characters.
            let inner = &s[1..len - 1];
            return ClassifiedLiteral::String(inner);
        }
        return ClassifiedLiteral::Atom;
    }

    // Bool keywords. Branch-predictable: most symbols don't start with t/f.
    if first == b't' && len == 4 && &bytes[1..] == b"rue" {
        return ClassifiedLiteral::BoolTrue;
    }
    if first == b'f' && len == 5 && &bytes[1..] == b"alse" {
        return ClassifiedLiteral::BoolFalse;
    }

    // Numeric: combined classify-and-accumulate scan.
    let (is_negative, mut i) = if first == b'-' {
        if len < 2 || !bytes[1].is_ascii_digit() {
            return ClassifiedLiteral::Atom; // bare `-` or `-foo`
        }
        (true, 1)
    } else if first.is_ascii_digit() {
        (false, 0)
    } else {
        return ClassifiedLiteral::Atom;
    };

    // Phase 1: integer digits — accumulate `value` and detect float markers.
    //
    // To represent `i64::MIN` (`-9223372036854775808`) correctly we
    // accumulate **directly into the negative space** when `is_negative`
    // is set. The positive-then-negate strategy fails for `i64::MIN`
    // because its absolute value (`9223372036854775808`) overflows `i64`
    // by one. By accumulating as `value = value*10 - digit`, the range
    // `[i64::MIN, 0]` is reachable in a single pass without intermediate
    // overflow.
    let mut value: i64 = 0;
    let mut overflow = false;
    while i < len {
        let b = bytes[i];
        if !b.is_ascii_digit() {
            break;
        }
        if !overflow {
            let digit = (b - b'0') as i64;
            let next = if is_negative {
                value.checked_mul(10).and_then(|v| v.checked_sub(digit))
            } else {
                value.checked_mul(10).and_then(|v| v.checked_add(digit))
            };
            value = match next {
                Some(v) => v,
                None => {
                    overflow = true;
                    0 // sentinel — unused once overflow flag is set
                }
            };
        }
        i += 1;
    }

    // If the entire input was integer digits, we're done.
    if i == len {
        if overflow {
            return ClassifiedLiteral::LongOverflow;
        }
        // `value` is already correctly signed by the accumulation strategy
        // above; no separate negation step needed.
        return ClassifiedLiteral::Long(value);
    }

    // Phase 2: must be float — validate the rest of the shape, then parse.
    let mut saw_marker = false;

    // Optional fractional part: `\.[0-9]+`
    if bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return ClassifiedLiteral::Atom; // `1.` with no digits
        }
        saw_marker = true;
    }

    // Optional exponent: `[eE][-+]?[0-9]+`
    if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return ClassifiedLiteral::Atom;
        }
        saw_marker = true;
    }

    // Anything left over → not a number.
    if i != len {
        return ClassifiedLiteral::Atom;
    }

    // We validated the shape — now parse via f64::from_str. This second
    // pass is unavoidable for f64 (manual mantissa+exponent parsing is
    // error-prone), but it's bounded by the shape we already validated.
    if saw_marker {
        match s.parse::<f64>() {
            Ok(f) => ClassifiedLiteral::Float(f),
            Err(_) => ClassifiedLiteral::Atom,
        }
    } else {
        // Should be unreachable: if we reached here, i == len and there
        // were no float markers, which means we should have returned in
        // the integer-only branch above. Safe fallback to Atom.
        ClassifiedLiteral::Atom
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parse_longs_directly() {
        assert_eq!(classify_and_parse("0"), ClassifiedLiteral::Long(0));
        assert_eq!(classify_and_parse("42"), ClassifiedLiteral::Long(42));
        assert_eq!(classify_and_parse("-1"), ClassifiedLiteral::Long(-1));
        assert_eq!(classify_and_parse("-42"), ClassifiedLiteral::Long(-42));
        assert_eq!(
            classify_and_parse("9223372036854775807"),
            ClassifiedLiteral::Long(i64::MAX)
        );
        assert_eq!(
            classify_and_parse("-9223372036854775808"),
            ClassifiedLiteral::Long(i64::MIN)
        );
    }

    #[test]
    fn parse_long_overflow() {
        assert_eq!(
            classify_and_parse("99999999999999999999"),
            ClassifiedLiteral::LongOverflow
        );
        // i64::MAX + 1 → overflow.
        assert_eq!(
            classify_and_parse("9223372036854775808"),
            ClassifiedLiteral::LongOverflow
        );
    }

    #[test]
    fn parse_floats() {
        match classify_and_parse("1.5") {
            ClassifiedLiteral::Float(f) => assert!((f - 1.5).abs() < 1e-12),
            other => panic!("expected Float, got {:?}", other),
        }
        match classify_and_parse("-3.14") {
            ClassifiedLiteral::Float(f) => assert!((f - -3.14).abs() < 1e-12),
            other => panic!("expected Float, got {:?}", other),
        }
        match classify_and_parse("1e10") {
            ClassifiedLiteral::Float(f) => assert!((f - 1e10).abs() < 1.0),
            other => panic!("expected Float, got {:?}", other),
        }
    }

    #[test]
    fn parse_bools_and_strings() {
        assert_eq!(classify_and_parse("true"), ClassifiedLiteral::BoolTrue);
        assert_eq!(classify_and_parse("false"), ClassifiedLiteral::BoolFalse);
        assert_eq!(
            classify_and_parse("\"hello\""),
            ClassifiedLiteral::String("hello")
        );
        assert_eq!(
            classify_and_parse("\"\""),
            ClassifiedLiteral::String("")
        );
    }

    #[test]
    fn parse_atoms() {
        assert_eq!(classify_and_parse("hello"), ClassifiedLiteral::Atom);
        assert_eq!(classify_and_parse(""), ClassifiedLiteral::Atom);
        assert_eq!(classify_and_parse("-"), ClassifiedLiteral::Atom);
        assert_eq!(classify_and_parse("1.2.3"), ClassifiedLiteral::Atom);
        assert_eq!(classify_and_parse("1e"), ClassifiedLiteral::Atom);
    }

    #[test]
    fn classify_and_parse_agrees_with_classify_symbol() {
        // Invariant from the doc table: kinds must agree (modulo
        // LongOverflow → SymbolKind::Long).
        let cases = [
            "", "0", "42", "-1", "-42", "i64::MAX", "9223372036854775807",
            "-9223372036854775808", "99999999999999999999",
            "1.5", "-3.14", "1e10", "1.5E-3", "-1.5e+3",
            "true", "false", "True", "FALSE",
            "\"hi\"", "\"\"", "\"unterminated",
            "hello", "-", "1.", ".5", "1.2.3",
            "$x", "&self", "_", "Inheritance",
        ];
        for s in &cases {
            let kind = classify_symbol(s);
            let parsed = classify_and_parse(s);
            let parsed_kind = match parsed {
                ClassifiedLiteral::Long(_) | ClassifiedLiteral::LongOverflow => SymbolKind::Long,
                ClassifiedLiteral::Float(_) => SymbolKind::Float,
                ClassifiedLiteral::BoolTrue => SymbolKind::BoolTrue,
                ClassifiedLiteral::BoolFalse => SymbolKind::BoolFalse,
                ClassifiedLiteral::String(_) => SymbolKind::String,
                ClassifiedLiteral::Atom => SymbolKind::Atom,
            };
            assert_eq!(kind, parsed_kind, "disagreement on {:?}", s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! kind_eq {
        ($input:expr, $expected:expr) => {
            assert_eq!(
                classify_symbol($input),
                $expected,
                "input: {:?}",
                $input
            );
        };
    }

    #[test]
    fn longs() {
        kind_eq!("0", SymbolKind::Long);
        kind_eq!("1", SymbolKind::Long);
        kind_eq!("42", SymbolKind::Long);
        kind_eq!("9223372036854775807", SymbolKind::Long); // i64::MAX
        kind_eq!("-1", SymbolKind::Long);
        kind_eq!("-42", SymbolKind::Long);
        kind_eq!("-9223372036854775808", SymbolKind::Long); // i64::MIN
        // Note: classifier says Long; the parser-side fallback handles
        // i64::MAX+1 by reclassifying as Atom (overflow safety belt).
        kind_eq!("99999999999999999999", SymbolKind::Long);
    }

    #[test]
    fn floats_with_dot() {
        kind_eq!("0.0", SymbolKind::Float);
        kind_eq!("1.5", SymbolKind::Float);
        kind_eq!("-3.14", SymbolKind::Float);
        kind_eq!("100.0", SymbolKind::Float);
        kind_eq!("0.5", SymbolKind::Float);
        kind_eq!("-0.5", SymbolKind::Float);
    }

    #[test]
    fn floats_with_exponent() {
        kind_eq!("1e10", SymbolKind::Float);
        kind_eq!("1E10", SymbolKind::Float);
        kind_eq!("1.5e10", SymbolKind::Float);
        kind_eq!("1.5E-3", SymbolKind::Float);
        kind_eq!("-1.5e+3", SymbolKind::Float);
        kind_eq!("0e0", SymbolKind::Float);
    }

    #[test]
    fn bool_keywords() {
        kind_eq!("true", SymbolKind::BoolTrue);
        kind_eq!("false", SymbolKind::BoolFalse);
    }

    #[test]
    fn bool_keyword_lookalikes_are_atoms() {
        kind_eq!("True", SymbolKind::Atom);   // case-sensitive
        kind_eq!("FALSE", SymbolKind::Atom);
        kind_eq!("trues", SymbolKind::Atom);  // longer → not bool
        kind_eq!("tru", SymbolKind::Atom);    // shorter → not bool
        kind_eq!("falsey", SymbolKind::Atom);
        kind_eq!("fals", SymbolKind::Atom);
    }

    #[test]
    fn strings() {
        kind_eq!("\"\"", SymbolKind::String);
        kind_eq!("\"hello\"", SymbolKind::String);
        kind_eq!("\"with spaces\"", SymbolKind::String);
        kind_eq!("\"42\"", SymbolKind::String); // numeric content stays as String
        kind_eq!("\"true\"", SymbolKind::String);
    }

    #[test]
    fn malformed_strings_are_atoms() {
        kind_eq!("\"unterminated", SymbolKind::Atom);
        kind_eq!("\"", SymbolKind::Atom); // single quote, len 1 → Atom
    }

    #[test]
    fn atoms_default() {
        kind_eq!("hello", SymbolKind::Atom);
        kind_eq!("foo-bar", SymbolKind::Atom);
        kind_eq!("$x", SymbolKind::Atom); // variables are atoms at this layer
        kind_eq!("&self", SymbolKind::Atom);
        kind_eq!("_", SymbolKind::Atom);
        kind_eq!("a1b2c3", SymbolKind::Atom);
        kind_eq!("Inheritance", SymbolKind::Atom);
        kind_eq!("=", SymbolKind::Atom);
        kind_eq!("|-", SymbolKind::Atom);
    }

    #[test]
    fn malformed_numbers_are_atoms() {
        kind_eq!("", SymbolKind::Atom);
        kind_eq!("-", SymbolKind::Atom);
        kind_eq!("1.", SymbolKind::Atom);    // dot with no fraction
        kind_eq!(".5", SymbolKind::Atom);    // no leading digit
        kind_eq!("1.2.3", SymbolKind::Atom); // double dot
        kind_eq!("1e", SymbolKind::Atom);    // exponent with no digits
        kind_eq!("1e+", SymbolKind::Atom);   // exponent with sign but no digits
        kind_eq!("1.5.7e3", SymbolKind::Atom);
        kind_eq!("1.5e3.0", SymbolKind::Atom); // exponent with fractional part
        kind_eq!("0x2A", SymbolKind::Atom); // hex unsupported
        kind_eq!("--1", SymbolKind::Atom);
        kind_eq!("1abc", SymbolKind::Atom);
    }

    #[test]
    fn round_trip_via_classifier_then_parse() {
        // Every Long classified by the DFA must parse to i64 successfully
        // (modulo the overflow safety belt).
        for s in &["0", "1", "-1", "42", "-42", "9223372036854775807"] {
            assert_eq!(classify_symbol(s), SymbolKind::Long, "s={}", s);
            assert!(s.parse::<i64>().is_ok(), "s={}", s);
        }
        // Every Float classified by the DFA must parse to f64 successfully.
        for s in &["0.0", "1.5", "-3.14", "1e10", "1.5E-3", "-1.5e+3"] {
            assert_eq!(classify_symbol(s), SymbolKind::Float, "s={}", s);
            assert!(s.parse::<f64>().is_ok(), "s={}", s);
        }
    }
}
