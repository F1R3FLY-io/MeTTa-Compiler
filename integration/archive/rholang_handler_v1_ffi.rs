// =====================================================================
// MeTTa Compiler Handler for Rholang
// =====================================================================
// This file contains the code that should be added to:
// f1r3node/rholang/src/rust/interpreter/system_processes.rs
//
// Add this code to the SystemProcesses impl block.
// =====================================================================

/// MeTTa compiler handler
/// Compiles MeTTa source code and returns the result as a JSON string
///
/// # Usage from Rholang
/// ```rholang
/// new mettaCompile in {
///   @"rho:metta:compile"!(*mettaCompile) |
///   contract @(*mettaCompile)(source, return) = {
///     // source: String containing MeTTa code
///     // return: Channel to send the JSON result
///   } |
///
///   @(*mettaCompile)("(+ 1 2)", "resultChan") |
///   for (@result <- @"resultChan") {
///     stdoutAck!(result, *ack)
///   }
/// }
/// ```
///
/// # Return Format
/// Success:
/// ```json
/// {
///   "success": true,
///   "exprs": [
///     {"type":"sexpr","items":[...]}
///   ]
/// }
/// ```
///
/// Error:
/// ```json
/// {
///   "success": false,
///   "error": "Parse error message"
/// }
/// ```
pub async fn metta_compile(
    &self,
    contract_args: (Vec<ListParWithRandom>, bool, Vec<Par>),
) -> Result<Vec<Par>, InterpreterError> {
    // Unpack contract arguments
    let Some((_, _, _, args)) = self.is_contract_call().unapply(contract_args) else {
        return Err(illegal_argument_error("metta_compile"));
    };

    // Expect exactly one argument: the MeTTa source code string
    let [arg] = args.as_slice() else {
        return Err(InterpreterError::IllegalArgumentException {
            message: "metta_compile: expected 1 argument (source code string)".to_string(),
        });
    };

    // Extract the source code string
    let src = self.pretty_printer.build_string_from_message(arg);

    // Call the MeTTa compiler via FFI
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    // Declare the FFI functions
    extern "C" {
        fn metta_compile(src: *const c_char) -> *mut c_char;
        fn metta_free_string(ptr: *mut c_char);
    }

    let src_cstr = CString::new(src.as_str())
        .map_err(|_| InterpreterError::IllegalArgumentException {
            message: "Invalid MeTTa source string (contains null byte)".to_string(),
        })?;

    let result_json = unsafe {
        let result_ptr = metta_compile(src_cstr.as_ptr());
        if result_ptr.is_null() {
            return Err(InterpreterError::IllegalArgumentException {
                message: "MeTTa compiler returned null".to_string(),
            });
        }
        let json_str = CStr::from_ptr(result_ptr).to_str()
            .map_err(|_| InterpreterError::IllegalArgumentException {
                message: "Invalid UTF-8 from MeTTa compiler".to_string(),
            })?
            .to_string();
        metta_free_string(result_ptr);
        json_str
    };

    // Return the JSON result as a Rholang string
    Ok(vec![RhoString::create_par(result_json)])
}
