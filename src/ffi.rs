/// FFI layer for Rholang integration
/// Provides C-compatible functions for calling from Rholang
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Compile MeTTa source code and return JSON result
///
/// # Safety
/// - src_ptr must be a valid null-terminated C string
/// - The returned pointer must be freed using metta_free_string
#[no_mangle]
pub unsafe extern "C" fn metta_compile(src_ptr: *const c_char) -> *mut c_char {
    if src_ptr.is_null() {
        let error = r#"{"error":"null pointer provided"}"#;
        return match CString::new(error) {
            Ok(s) => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        };
    }

    let src = match CStr::from_ptr(src_ptr).to_str() {
        Ok(s) => s,
        Err(_) => {
            let error = r#"{"error":"invalid UTF-8"}"#;
            return match CString::new(error) {
                Ok(s) => s.into_raw(),
                Err(_) => std::ptr::null_mut(),
            };
        }
    };

    let result = crate::rholang_integration::compile_safe(src);
    match CString::new(result) {
        Ok(s) => s.into_raw(),
        Err(_) => {
            let error = r#"{"error":"result contains null byte"}"#;
            match CString::new(error) {
                Ok(s) => s.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
    }
}

/// Free a string allocated by metta_compile
///
/// # Safety
/// - ptr must be a pointer returned by metta_compile
/// - ptr must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn metta_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_ffi_compile_success() {
        let src = CString::new("(+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            // Should return full MettaState
            assert!(result.contains(r#""pending_exprs""#));
            assert!(result.contains(r#""environment""#));
            assert!(result.contains(r#""eval_outputs""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_compile_error() {
        let src = CString::new("(unclosed").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            // Error format should contain "error" field
            assert!(result.contains(r#""error""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_null_pointer() {
        unsafe {
            let result_ptr = metta_compile(std::ptr::null());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            // Error format should contain "error" field and message
            assert!(result.contains(r#""error""#));
            assert!(result.contains("null pointer"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_free_null() {
        // Should not crash
        unsafe {
            metta_free_string(std::ptr::null_mut());
        }
    }

    #[test]
    fn test_ffi_nested_expression() {
        let src = CString::new("(+ 1 (* 2 3))").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            // Should return full MettaState with nested sexpr
            assert!(result.contains(r#""pending_exprs""#));
            assert!(result.contains(r#""environment""#));
            assert!(result.contains(r#""eval_outputs""#));
            assert!(result.contains(r#""type":"sexpr""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_empty_input() {
        let src = CString::new("").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));
            assert!(result.contains(r#"[]"#) || result.contains(r#""pending_exprs":[]"#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_whitespace_only() {
        let src = CString::new("   \n\t  ").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_multiple_expressions() {
        let src = CString::new("(+ 1 2) (* 3 4) (- 10 5)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));
            let sexpr_count = result.matches(r#""type":"sexpr""#).count();
            assert!(
                sexpr_count >= 3,
                "Expected 3+ sexprs, found {}",
                sexpr_count
            );

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_string_literal() {
        let src = CString::new(r#""hello world""#).unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"string""#));
            assert!(result.contains("hello world"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_string_with_escapes() {
        let src = CString::new(r#""line1\nline2""#).unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"string""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_empty_string() {
        let src = CString::new(r#""""#).unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"string""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_variables() {
        let src = CString::new("$x $y $var").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains("$x") || result.contains("$y") || result.contains("$var"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_wildcard() {
        let src = CString::new("_").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"atom""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_rule_definition() {
        let src = CString::new("(= (double $x) (* $x 2))").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));
            assert!(result.contains("double") || result.contains("="));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_exclaim_operator() {
        let src = CString::new("!(foo)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"sexpr""#));
            assert!(result.contains("!"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_if_statement() {
        let src = CString::new("(if true yes no)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"sexpr""#));
            assert!(result.contains("if"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_type_assertion() {
        let src = CString::new("(: x Number)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"sexpr""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_quote() {
        let src = CString::new("(quote (+ 1 2))").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains("quote"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_semicolon_comment() {
        let src = CString::new("; comment\n(+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));
            assert!(!result.contains("comment"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_double_slash_comment() {
        let src = CString::new("// comment\n(+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_block_comment() {
        let src = CString::new("/* comment */ (+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_mismatched_parens() {
        let src = CString::new("((+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""error""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_extra_close_paren() {
        let src = CString::new("(+ 1 2))").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""error""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_unclosed_string() {
        let src = CString::new(r#""unclosed"#).unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""error""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_unclosed_block_comment() {
        let src = CString::new("/* unclosed comment").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""error""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_deeply_nested() {
        let src = CString::new("(+ 1 (+ 2 (+ 3 (+ 4 5))))").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"sexpr""#));
            // Check for multiple levels of nesting
            let sexpr_count = result.matches(r#""type":"sexpr""#).count();
            assert!(sexpr_count >= 4, "Expected 4+ nested sexprs");

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_boolean_values() {
        let src = CString::new("true false").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"bool""#));
            assert!(result.contains("true") || result.contains("false"));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_multiple_calls() {
        for _ in 0..10 {
            let src = CString::new("(+ 1 2)").unwrap();
            unsafe {
                let result_ptr = metta_compile(src.as_ptr());
                assert!(!result_ptr.is_null());
                metta_free_string(result_ptr);
            }
        }
    }

    #[test]
    fn test_ffi_large_input() {
        // Test with a large input string
        let large_expr = format!(
            "(+ {})",
            (0..1000)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let src = CString::new(large_expr).unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""pending_exprs""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_round_trip() {
        let src = CString::new("(+ 1 2)").unwrap();
        unsafe {
            let result_ptr1 = metta_compile(src.as_ptr());
            assert!(!result_ptr1.is_null());
            metta_free_string(result_ptr1);

            let result_ptr2 = metta_compile(src.as_ptr());
            assert!(!result_ptr2.is_null());
            metta_free_string(result_ptr2);
        }
    }

    #[test]
    fn test_ffi_unicode_string() {
        let src = CString::new(r#""Hello 世界 🌍""#).unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"string""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_unicode_atoms() {
        let src = CString::new("привет").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""type":"atom""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_json_structure() {
        let src = CString::new("(+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();

            assert!(result.starts_with('{'));
            assert!(result.ends_with('}'));
            assert!(result.contains(r#""pending_exprs""#));
            assert!(result.contains(r#""environment""#));
            assert!(result.contains(r#""eval_outputs""#));

            metta_free_string(result_ptr);
        }
    }

    #[test]
    fn test_ffi_error_json_structure() {
        let src = CString::new("(unclosed").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();

            assert!(result.starts_with('{'));
            assert!(result.ends_with('}'));
            assert!(result.contains(r#""error""#));

            metta_free_string(result_ptr);
        }
    }
}
