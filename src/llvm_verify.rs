use inkwell::module::Module;
#[cfg(windows)]
use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};

pub fn verify_module(module: &Module, error_prefix: &str) -> Result<(), String> {
    #[cfg(windows)]
    let verify_rc = unsafe {
        LLVMVerifyModule(
            module.as_mut_ptr(),
            LLVMVerifierFailureAction::LLVMReturnStatusAction,
            std::ptr::null_mut(),
        )
    };

    #[cfg(windows)]
    {
        if verify_rc == 0 {
            return Ok(());
        }
        // Re-checked locally on Windows Arm64 on March 23, 2026: asking LLVM
        // to populate the verifier message pointer led to process instability,
        // so keep the Windows path on the null-pointer fallback for now.
        return Err(format!(
            "{error_prefix}: LLVM verifier failed (message pointer unavailable on this platform; rerun on Linux/macOS for detailed verifier diagnostics)"
        ));
    }

    #[cfg(not(windows))]
    {
        module
            .verify()
            .map_err(|err| format!("{error_prefix}: {err}"))
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
