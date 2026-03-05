use inkwell::module::Module;
use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
#[cfg(not(windows))]
use llvm_sys::core::LLVMDisposeMessage;
#[cfg(not(windows))]
use std::ffi::CStr;
use std::ffi::c_char;

pub fn verify_module(module: &Module, error_prefix: &str) -> Result<(), String> {
    let mut err_ptr: *mut c_char = std::ptr::null_mut();
    let verify_rc = unsafe {
        LLVMVerifyModule(
            module.as_mut_ptr(),
            LLVMVerifierFailureAction::LLVMReturnStatusAction,
            &mut err_ptr,
        )
    };

    if verify_rc == 0 {
        return Ok(());
    }

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

    #[cfg(windows)]
    {
        // Intentionally avoid both reading and disposing verifier message pointers on Windows:
        // these pointers have triggered access violations in affected environments.
        Err(format!(
            "{error_prefix}: LLVM verifier failed (message pointer unavailable on this platform)"
        ))
    }
}
