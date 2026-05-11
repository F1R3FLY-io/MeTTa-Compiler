//! String unescape logic with reusable buffer and memchr fast-path.
//!
//! The fast path uses `memchr` to scan for backslashes: if none are found,
//! the source slice IS the unescaped string (zero-copy). The slow path
//! processes escape sequences into a reusable scratch buffer.

use crate::tree_sitter_parser::SyntaxError;

/// Unescape a string literal's inner content (between the quotes).
///
/// - `content`: the bytes between the opening and closing `"` (exclusive).
/// - `buf`: reusable scratch buffer (cleared on entry, not deallocated).
/// - `line`, `col`: position of the opening `"` for error reporting.
///
/// Returns a `&str` that is either a zero-copy slice of `content` (fast path)
/// or a reference to `buf` (slow path).
pub fn unescape_string_content<'a>(
    content: &'a [u8],
    buf: &'a mut String,
    line: usize,
    col: usize,
) -> Result<&'a str, SyntaxError> {
    // Fast path: if no backslash, content is already valid UTF-8 (ASCII subset)
    if memchr::memchr(b'\\', content).is_none() {
        // Validate UTF-8
        return std::str::from_utf8(content)
            .map_err(|_| make_escape_error(line, col, "invalid UTF-8 in string"));
    }

    // Slow path: process escape sequences
    buf.clear();
    let mut i = 0;
    while i < content.len() {
        if content[i] == b'\\' {
            i += 1;
            if i >= content.len() {
                return Err(make_escape_error(line, col, "unterminated escape sequence"));
            }
            match content[i] {
                b'n' => {
                    buf.push('\n');
                    i += 1;
                }
                b't' => {
                    buf.push('\t');
                    i += 1;
                }
                b'r' => {
                    buf.push('\r');
                    i += 1;
                }
                b'\\' => {
                    buf.push('\\');
                    i += 1;
                }
                b'"' => {
                    buf.push('"');
                    i += 1;
                }
                b'x' => {
                    // Hex escape: \xHH
                    i += 1;
                    if i + 2 > content.len() {
                        return Err(make_escape_error(
                            line,
                            col,
                            "incomplete hex escape sequence",
                        ));
                    }
                    let hi = hex_digit(content[i]).ok_or_else(|| {
                        make_escape_error(line, col, "invalid hex digit in \\x escape")
                    })?;
                    let lo = hex_digit(content[i + 1]).ok_or_else(|| {
                        make_escape_error(line, col, "invalid hex digit in \\x escape")
                    })?;
                    buf.push((hi << 4 | lo) as char);
                    i += 2;
                }
                b'u' => {
                    // Unicode escape: \u{HHHHHH}
                    i += 1;
                    if i >= content.len() || content[i] != b'{' {
                        return Err(make_escape_error(line, col, "expected '{' after \\u"));
                    }
                    i += 1; // skip '{'
                    let start = i;
                    while i < content.len() && content[i] != b'}' {
                        if !content[i].is_ascii_hexdigit() {
                            return Err(make_escape_error(
                                line,
                                col,
                                "invalid character in unicode escape",
                            ));
                        }
                        i += 1;
                    }
                    if i >= content.len() {
                        return Err(make_escape_error(
                            line,
                            col,
                            "unterminated unicode escape sequence",
                        ));
                    }
                    let hex_slice = &content[start..i];
                    if hex_slice.is_empty() {
                        return Err(make_escape_error(
                            line,
                            col,
                            "empty unicode escape sequence",
                        ));
                    }
                    if hex_slice.len() > 6 {
                        return Err(make_escape_error(
                            line,
                            col,
                            "unicode escape too long (max 6 hex digits)",
                        ));
                    }
                    // SAFETY: we verified all bytes are ASCII hex digits above
                    let hex_str = unsafe { std::str::from_utf8_unchecked(hex_slice) };
                    let code_point = u32::from_str_radix(hex_str, 16)
                        .map_err(|_| make_escape_error(line, col, "invalid unicode escape"))?;
                    let ch = char::from_u32(code_point).ok_or_else(|| {
                        make_escape_error(line, col, "invalid unicode code point")
                    })?;
                    buf.push(ch);
                    i += 1; // skip '}'
                }
                other => {
                    // Unknown escape: preserve backslash + character
                    buf.push('\\');
                    buf.push(other as char);
                    i += 1;
                }
            }
        } else {
            // Non-escape byte: determine UTF-8 sequence length and copy
            let byte = content[i];
            if byte < 0x80 {
                buf.push(byte as char);
                i += 1;
            } else {
                // Multi-byte UTF-8: find sequence length and validate
                let seq_len = utf8_sequence_length(byte);
                if i + seq_len > content.len() {
                    return Err(make_escape_error(line, col, "invalid UTF-8 in string"));
                }
                let s = std::str::from_utf8(&content[i..i + seq_len])
                    .map_err(|_| make_escape_error(line, col, "invalid UTF-8 in string"))?;
                buf.push_str(s);
                i += seq_len;
            }
        }
    }

    Ok(buf.as_str())
}

#[inline]
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[inline]
fn utf8_sequence_length(first_byte: u8) -> usize {
    match first_byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1, // invalid, will be caught by from_utf8
    }
}

fn make_escape_error(line: usize, col: usize, msg: &str) -> SyntaxError {
    use crate::tree_sitter_parser::SyntaxErrorKind;
    SyntaxError {
        kind: SyntaxErrorKind::InvalidEscape(msg.to_string()),
        line,
        column: col,
        text: String::new(),
        file_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unescape(s: &str) -> Result<String, SyntaxError> {
        let mut buf = String::new();
        let result = unescape_string_content(s.as_bytes(), &mut buf, 1, 1)?;
        Ok(result.to_string())
    }

    #[test]
    fn test_no_escapes() {
        assert_eq!(unescape("hello world").unwrap(), "hello world");
    }

    #[test]
    fn test_basic_escapes() {
        assert_eq!(unescape(r"hello\nworld").unwrap(), "hello\nworld");
        assert_eq!(unescape(r"tab\there").unwrap(), "tab\there");
        assert_eq!(unescape(r"cr\rhere").unwrap(), "cr\rhere");
        assert_eq!(unescape(r"back\\slash").unwrap(), "back\\slash");
        assert_eq!(unescape(r#"a\"b"#).unwrap(), "a\"b");
    }

    #[test]
    fn test_hex_escape() {
        assert_eq!(unescape(r"\x41").unwrap(), "A");
        assert_eq!(unescape(r"\x48\x65\x6c\x6c\x6f").unwrap(), "Hello");
        assert_eq!(unescape(r"\x1b").unwrap(), "\x1b");
    }

    #[test]
    fn test_unicode_escape() {
        assert_eq!(unescape(r"\u{1F4A1}").unwrap(), "\u{1F4A1}");
        assert_eq!(unescape(r"\u{0041}").unwrap(), "A");
        assert_eq!(unescape(r"\u{3B1}").unwrap(), "\u{3B1}");
    }

    #[test]
    fn test_mixed_content() {
        assert_eq!(unescape(r"Hello \u{1F30D}!").unwrap(), "Hello \u{1F30D}!");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(unescape("").unwrap(), "");
    }

    #[test]
    fn test_unknown_escape_preserved() {
        assert_eq!(unescape(r"\z").unwrap(), "\\z");
    }

    #[test]
    fn test_utf8_passthrough() {
        assert_eq!(unescape("héllo wörld").unwrap(), "héllo wörld");
    }
}
