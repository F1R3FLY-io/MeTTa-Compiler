/// PathMap output parser for integration tests (using nom parser combinators)
///
/// Parses PathMap structures from Rholang output and extracts
/// the `source`, `environment`, and `output` fields.
use mettatron::backend::models::MettaValue;
use nom::{
    branch::alt,
    bytes::complete::{escaped, is_not, tag, take_while1},
    character::complete::{char, digit1, multispace0, one_of},
    combinator::{map, map_res, opt, recognize, value},
    multi::separated_list0,
    sequence::{delimited, pair},
    IResult,
};

/// Extension trait to add test-specific helper methods to MettaValue
pub trait MettaValueTestExt {
    /// Convert to string representation for test comparison
    fn to_display_string(&self) -> String;

    /// Check if this value matches a string representation (for test assertions)
    fn matches_str(&self, s: &str) -> bool;
}

impl MettaValueTestExt for MettaValue {
    fn to_display_string(&self) -> String {
        match self {
            MettaValue::Long(n) => n.to_string(),
            MettaValue::Float(f) => f.to_string(),
            MettaValue::Bool(b) => b.to_string(),
            MettaValue::String(s) => format!("\"{}\"", s),
            MettaValue::Atom(s) => s.clone(),
            MettaValue::SExpr(exprs) => {
                let inner: Vec<String> = exprs.iter().map(|e| e.to_display_string()).collect();
                format!("({})", inner.join(" "))
            }
            MettaValue::Error(msg, details) => {
                format!("(error \"{}\" {})", msg, details.to_display_string())
            }
            MettaValue::Nil => "()".to_string(),
            MettaValue::Type(t) => format!("Type({})", t.to_display_string()),
            MettaValue::Conjunction(goals) => {
                let inner: Vec<String> = goals.iter().map(|g| g.to_display_string()).collect();
                format!("(, {})", inner.join(" "))
            }
            MettaValue::Unit => "()".to_string(),
        }
    }

    fn matches_str(&self, s: &str) -> bool {
        match self {
            MettaValue::Long(n) => n.to_string() == s,
            MettaValue::Float(f) => f.to_string() == s,
            MettaValue::Bool(b) => b.to_string() == s,
            MettaValue::String(inner) => {
                // Match with or without quotes
                inner == s || format!("\"{}\"", inner) == s
            }
            MettaValue::Atom(sym) => sym == s,
            MettaValue::SExpr(_) => self.to_display_string() == s,
            MettaValue::Error(_, _) => self.to_display_string() == s,
            MettaValue::Nil => s == "()" || s == "Nil",
            MettaValue::Type(_) => self.to_display_string() == s,
            MettaValue::Conjunction(_) => self.to_display_string() == s,
            MettaValue::Unit => s == "()" || s == "Unit",
        }
    }
}

/// Represents a parsed PathMap structure from Rholang output
#[derive(Debug, Clone, PartialEq)]
pub struct PathMapOutput {
    /// Source expressions (formerly `pending_exprs`)
    pub source: Vec<MettaValue>,
    /// Environment state (space data)
    pub environment: Option<String>,
    /// Evaluation outputs (formerly `eval_outputs`)
    pub output: Vec<MettaValue>,
}

impl PathMapOutput {
    /// Create an empty PathMapOutput
    pub fn empty() -> Self {
        PathMapOutput {
            source: Vec::new(),
            environment: None,
            output: Vec::new(),
        }
    }

    /// Check if the PathMap has no outputs
    pub fn is_output_empty(&self) -> bool {
        self.output.is_empty()
    }

    /// Check if the PathMap has source expressions
    pub fn has_source(&self) -> bool {
        !self.source.is_empty()
    }

    /// Check if the PathMap has environment data
    pub fn has_environment(&self) -> bool {
        self.environment.is_some()
    }
}

// ============================================================================
// Parser Combinators - Direct Literal Parsing
// ============================================================================

/// Parse whitespace
fn ws<'a, F, O>(inner: F) -> impl FnMut(&'a str) -> IResult<&'a str, O>
where
    F: FnMut(&'a str) -> IResult<&'a str, O>,
{
    delimited(multispace0, inner, multispace0)
}

/// Parse an integer literal directly into MettaValue::Long
fn parse_integer(input: &str) -> IResult<&str, MettaValue> {
    map_res(recognize(pair(opt(char('-')), digit1)), |s: &str| {
        s.parse::<i64>().map(MettaValue::Long)
    })(input)
}

/// Parse a boolean literal directly into MettaValue::Bool
fn parse_boolean(input: &str) -> IResult<&str, MettaValue> {
    alt((
        value(MettaValue::Bool(true), tag("true")),
        value(MettaValue::Bool(false), tag("false")),
    ))(input)
}

/// Parse a string literal directly into MettaValue::String
fn parse_string_literal(input: &str) -> IResult<&str, MettaValue> {
    map(
        delimited(
            char('"'),
            escaped(is_not("\\\""), '\\', one_of("\"\\")),
            char('"'),
        ),
        |s: &str| MettaValue::String(s.to_string()),
    )(input)
}

/// Parse Nil literal
fn parse_nil(input: &str) -> IResult<&str, MettaValue> {
    alt((
        value(MettaValue::Nil, tag("()")),
        value(MettaValue::Nil, tag("Nil")),
    ))(input)
}

/// Parse a symbol/atom
fn parse_symbol(input: &str) -> IResult<&str, MettaValue> {
    map(
        take_while1(|c: char| {
            c.is_alphanumeric()
                || c == '_'
                || c == '-'
                || c == '+'
                || c == '*'
                || c == '/'
                || c == '<'
                || c == '>'
                || c == '='
                || c == '!'
                || c == '?'
        }),
        |s: &str| MettaValue::Atom(s.to_string()),
    )(input)
}

/// Parse a tuple/S-expression recursively
fn parse_tuple(input: &str) -> IResult<&str, MettaValue> {
    map(
        delimited(
            char('('),
            separated_list0(ws(char(',')), ws(parse_metta_value_recursive)),
            char(')'),
        ),
        |elements| {
            if elements.is_empty() {
                MettaValue::Nil
            } else {
                MettaValue::SExpr(elements)
            }
        },
    )(input)
}

/// Parse any MettaValue using direct nom combinators (recursive for nested structures)
fn parse_metta_value_recursive(input: &str) -> IResult<&str, MettaValue> {
    alt((
        parse_boolean, // Must come before symbol to catch true/false
        parse_integer,
        parse_string_literal,
        parse_nil,
        parse_tuple, // Recursively parse tuples
        parse_symbol,
    ))(input)
}

/// Parse an S-expression (as a string for now, for backward compatibility)
fn parse_sexpr_string(input: &str) -> IResult<&str, String> {
    let (input, _) = char('(')(input)?;

    // Track depth to handle nested parentheses
    let mut depth = 1;
    let mut pos = 0;
    let chars: Vec<char> = input.chars().collect();

    while pos < chars.len() && depth > 0 {
        match chars[pos] {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        pos += 1;
    }

    if depth == 0 {
        let content = &input[..pos - 1];
        let rest = &input[pos..];
        Ok((rest, format!("({})", content)))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Eof,
        )))
    }
}

/// Parse any MettaValue using direct nom combinators (top-level, non-recursive for simple values)
fn parse_metta_value(input: &str) -> IResult<&str, MettaValue> {
    alt((
        parse_boolean, // Must come before symbol to catch true/false
        parse_integer,
        parse_string_literal,
        parse_nil,
        parse_tuple, // Now parses tuples recursively
        map(parse_sexpr_string, |s| {
            // Fallback: store as atom if tuple parser fails
            MettaValue::Atom(s)
        }),
        parse_symbol,
    ))(input)
}

/// Parse array value (containing MettaValues)
fn array_of_values(input: &str) -> IResult<&str, Vec<MettaValue>> {
    delimited(
        char('['),
        separated_list0(ws(char(',')), ws(parse_metta_value)),
        char(']'),
    )(input)
}

/// Parse a tuple value - anything between outer parentheses with depth tracking
fn tuple_value(input: &str) -> IResult<&str, String> {
    let (input, _) = char('(')(input)?;

    // Track depth to handle nested parentheses and handle {|...|}
    let mut depth = 1;
    let mut pos = 0;
    let chars: Vec<char> = input.chars().collect();
    let mut brace_depth = 0; // Track {|...|} nesting
    let mut in_string = false; // Track if we're inside a quoted string
    let mut prev_char = '\0';

    while pos < chars.len() && (depth > 0 || brace_depth > 0) {
        let current_char = chars[pos];

        // Handle quoted strings - toggle in_string on unescaped quotes
        if current_char == '"' && prev_char != '\\' {
            in_string = !in_string;
            prev_char = current_char;
            pos += 1;
            continue;
        }

        // Skip processing special characters when inside a quoted string
        if !in_string {
            if pos + 1 < chars.len() {
                // Check for {| (start of brace structure)
                if chars[pos] == '{' && chars[pos + 1] == '|' {
                    brace_depth += 1;
                    pos += 2;
                    prev_char = '|';
                    continue;
                }
                // Check for |} (end of brace structure)
                if chars[pos] == '|' && chars[pos + 1] == '}' {
                    brace_depth -= 1;
                    pos += 2;
                    prev_char = '}';
                    continue;
                }
            }

            match current_char {
                '(' => depth += 1,
                ')' if brace_depth == 0 => depth -= 1,
                _ => {}
            }
        }

        prev_char = current_char;
        pos += 1;
    }

    if depth == 0 {
        let content = &input[..pos - 1];
        let rest = &input[pos..];
        Ok((rest, content.to_string()))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Eof,
        )))
    }
}

/// Parse a quoted string (for environment field) - handles escaped quotes
fn quoted_string(input: &str) -> IResult<&str, &str> {
    let (input, _) = char('"')(input)?;

    let mut pos = 0;
    let chars: Vec<char> = input.chars().collect();
    let mut prev_char = '\0';

    while pos < chars.len() {
        let current_char = chars[pos];

        // End quote found (not escaped)
        if current_char == '"' && prev_char != '\\' {
            let content = &input[..pos];
            let rest = &input[pos + 1..];
            return Ok((rest, content));
        }

        prev_char = current_char;
        pos += 1;
    }

    // No closing quote found
    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

/// Parse a simple token (like `...` or placeholders)
fn simple_token(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c != ',' && c != ')' && c != '(' && !c.is_whitespace())(input)
}

/// Parse a field tuple: ("fieldname", value)
fn field_tuple<'a>(field_name: &'a str) -> impl FnMut(&'a str) -> IResult<&'a str, String> {
    move |input: &'a str| {
        let (input, _) = ws(char('('))(input)?;
        let (input, _) = ws(char('"'))(input)?;
        let (input, _) = tag(field_name)(input)?;
        let (input, _) = ws(char('"'))(input)?;
        let (input, _) = ws(char(','))(input)?;

        // Parse the value - could be quoted string, tuple, or simple token
        let (input, value) = ws(alt((
            map(tuple_value, |s| s),
            map(quoted_string, |s| format!("\"{}\"", s)),
            map(simple_token, |s| s.to_string()),
        )))(input)?;

        let (input, _) = ws(char(')'))(input)?;

        Ok((input, value))
    }
}

/// Parse source or output array field (returns Vec<MettaValue>)
fn array_field<'a>(
    field_name: &'a str,
) -> impl FnMut(&'a str) -> IResult<&'a str, Vec<MettaValue>> {
    move |input: &'a str| {
        let (input, _) = ws(char('('))(input)?;
        let (input, _) = ws(char('"'))(input)?;
        let (input, _) = tag(field_name)(input)?;
        let (input, _) = ws(char('"'))(input)?;
        let (input, _) = ws(char(','))(input)?;

        // Parse array of MettaValues
        let (input, values) = ws(array_of_values)(input)?;

        let (input, _) = ws(char(')'))(input)?;

        Ok((input, values))
    }
}

/// Parse a complete PathMap structure
fn pathmap_structure(input: &str) -> IResult<&str, PathMapOutput> {
    // Parse {|
    let (input, _) = ws(tag("{|"))(input)?;
    let (input, _) = ws(char('('))(input)?;

    // We need to parse fields in any order
    // Try to parse each field type
    let mut source = Vec::new();
    let mut environment = None;
    let mut output = Vec::new();

    let mut remaining = input;
    let mut first = true;

    loop {
        // Try comma separator (not on first iteration)
        if !first {
            match ws::<_, _>(char(','))(remaining) {
                Ok((rest, _)) => remaining = rest,
                Err(_) => break,
            }
        }
        first = false;

        // Try parsing each field type
        if let Ok((rest, vals)) = array_field("source")(remaining) {
            source = vals;
            remaining = rest;
        } else if let Ok((rest, vals)) = array_field("output")(remaining) {
            output = vals;
            remaining = rest;
        } else if let Ok((rest, val)) = field_tuple("environment")(remaining) {
            environment = Some(val);
            remaining = rest;
        } else {
            break;
        }
    }

    let (input, _) = ws(char(')'))(remaining)?;
    let (input, _) = ws(tag("|}"))(input)?;

    Ok((
        input,
        PathMapOutput {
            source,
            environment,
            output,
        },
    ))
}

// ============================================================================
// Public API
// ============================================================================

/// Parse PathMap output from Rholang stdout
///
/// Extracts the PathMap structure: {|(("source", [...]), ("environment", ...), ("output", [...]))|}.
pub fn parse_pathmap(output: &str) -> Vec<PathMapOutput> {
    let mut results = Vec::new();
    let mut remaining = output;

    // Find all PathMap structures
    while let Some(start_pos) = remaining.find("{|") {
        let candidate = &remaining[start_pos..];

        match pathmap_structure(candidate) {
            Ok((rest, pathmap)) => {
                results.push(pathmap);
                // Move past the parsed PathMap
                let consumed = candidate.len() - rest.len();
                remaining = &remaining[start_pos + consumed..];
            }
            Err(_) => {
                // Skip this {| and continue searching
                remaining = &remaining[start_pos + 2..];
            }
        }
    }

    results
}

/// Extract all evaluation outputs from stdout as MettaValues
///
/// This is a convenience function that parses all PathMaps and extracts their outputs.
pub fn extract_all_outputs(output: &str) -> Vec<MettaValue> {
    let pathmaps = parse_pathmap(output);
    let mut all_outputs = Vec::new();

    for pathmap in pathmaps {
        all_outputs.extend(pathmap.output);
    }

    all_outputs
}

/// Extract all evaluation outputs from stdout as strings
///
/// Convenience function for backward compatibility with tests expecting strings.
pub fn extract_all_outputs_as_strings(output: &str) -> Vec<String> {
    extract_all_outputs(output)
        .iter()
        .map(|v| v.to_display_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_integer() {
        assert_eq!(parse_integer("42"), Ok(("", MettaValue::Long(42))));
        assert_eq!(parse_integer("-10"), Ok(("", MettaValue::Long(-10))));
    }

    #[test]
    fn test_parse_boolean() {
        assert_eq!(parse_boolean("true"), Ok(("", MettaValue::Bool(true))));
        assert_eq!(parse_boolean("false"), Ok(("", MettaValue::Bool(false))));
    }

    #[test]
    fn test_parse_string_literal() {
        let result = parse_string_literal("\"hello\"");
        assert!(result.is_ok());
        if let Ok((_, MettaValue::String(s))) = result {
            assert_eq!(s, "hello");
        }
    }

    #[test]
    fn test_parse_simple_pathmap() {
        let output = r#"{|(("source", [(+ 1 2)]), ("environment", "..."), ("output", [3]))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].output, vec![MettaValue::Long(3)]);
    }

    #[test]
    fn test_parse_empty_output() {
        let output = r#"{|(("source", []), ("environment", "..."), ("output", []))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert!(result[0].is_output_empty());
    }

    #[test]
    fn test_parse_multiple_outputs() {
        let output = r#"{|(("source", []), ("environment", "..."), ("output", [10, 15, 20]))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].output.len(), 3);
        assert_eq!(result[0].output[0], MettaValue::Long(10));
        assert_eq!(result[0].output[1], MettaValue::Long(15));
        assert_eq!(result[0].output[2], MettaValue::Long(20));
    }

    #[test]
    fn test_parse_nested_expressions() {
        let output =
            r#"{|(("source", [(+ 1 (* 2 3))]), ("environment", "..."), ("output", [(+ 1 6)]))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert!(result[0].has_source());
        // S-expressions are stored as Atoms (strings) for now
        assert!(matches!(result[0].output[0], MettaValue::Atom(_)));
    }

    #[test]
    fn test_extract_all_outputs() {
        let output = r#"
            Test 1: {|(("source", []), ("output", [3]))|}
            Test 2: {|(("source", []), ("output", [7, 10]))|}  "#;
        let results = extract_all_outputs(output);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0], MettaValue::Long(3));
        assert_eq!(results[1], MettaValue::Long(7));
        assert_eq!(results[2], MettaValue::Long(10));
    }

    #[test]
    fn test_parse_with_environment() {
        let output = r#"{|(("source", []), ("environment", "..."), ("output", []))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert!(result[0].has_environment());
        // Note: quoted strings get the quotes included
        assert_eq!(result[0].environment, Some(r#""...""#.to_string()));
    }

    #[test]
    fn test_parse_with_nested_braces() {
        // Real Rholang output format with nested {||}
        let output = r#"{|(("source", []), ("environment", (("space", {||}), ("multiplicities", {}))), ("output", [12]))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert!(result[0].has_environment());
        assert_eq!(result[0].output, vec![MettaValue::Long(12)]);
    }

    #[test]
    fn test_parse_boolean_values() {
        let output = r#"{|(("source", []), ("output", [true, false]))|}  "#;
        let result = parse_pathmap(output);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].output.len(), 2);
        assert_eq!(result[0].output[0], MettaValue::Bool(true));
        assert_eq!(result[0].output[1], MettaValue::Bool(false));
    }
}
