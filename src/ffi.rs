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
}
