use inkwell::module::Module;
use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
#[cfg(not(windows))]
use llvm_sys::core::LLVMDisposeMessage;
#[cfg(not(windows))]
use std::ffi::CStr;
#[cfg(not(windows))]
use std::ffi::c_char;

pub fn verify_module(module: &Module, error_prefix: &str) -> Result<(), String> {
    #[cfg(windows)]
    let verify_rc = unsafe {
        LLVMVerifyModule(
            module.as_mut_ptr(),
            LLVMVerifierFailureAction::LLVMReturnStatusAction,
            std::ptr::null_mut(),
        )
    };

    #[cfg(not(windows))]
    let mut err_ptr: *mut c_char = std::ptr::null_mut();

    #[cfg(not(windows))]
    let verify_rc = unsafe {
        LLVMVerifyModule(
            module.as_mut_ptr(),
            LLVMVerifierFailureAction::LLVMReturnStatusAction,
            &raw mut err_ptr,
        )
    };

    if verify_rc == 0 {
        return Ok(());
    }

    #[cfg(windows)]
    {
        // Re-checked locally on Windows Arm64 on March 23, 2026: asking LLVM
        // to populate the verifier message pointer led to process instability,
        // so keep the Windows path on the null-pointer fallback for now.
        Err(format!(
            "{error_prefix}: LLVM verifier failed (message pointer unavailable on this platform; rerun on Linux/macOS for detailed verifier diagnostics)"
        ))
    }

    #[cfg(not(windows))]
    if err_ptr.is_null() {
        return Err(format!("{error_prefix}: unknown LLVM verifier error"));
    }

    #[cfg(not(windows))]
    {
        let message = unsafe { CStr::from_ptr(err_ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { LLVMDisposeMessage(err_ptr) };
        Err(format!("{error_prefix}: {message}"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::verify_module;
    use inkwell::context::Context;

    #[test]
    fn test_verify_module_rejects_unterminated_function() {
        let context = Context::create();
        let module = context.create_module("invalid");
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("broken", fn_type, None);
        let _ = context.append_basic_block(func, "entry");

        let err = verify_module(&module, "verification failed")
            .expect_err("unterminated function should fail verification");
        assert!(err.contains("verification failed"));
    }
}
