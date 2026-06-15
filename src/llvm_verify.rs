use inkwell::module::Module;
#[cfg(windows)]
use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};

pub fn verify_module(module: &Module, error_prefix: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        verify_module_windows(module, error_prefix)
    }

    #[cfg(not(windows))]
    {
        verify_module_non_windows(module, error_prefix)
    }
}

#[cfg(windows)]
fn verify_module_windows(module: &Module, error_prefix: &str) -> Result<(), String> {
    let verify_rc = unsafe {
        LLVMVerifyModule(
            module.as_mut_ptr(),
            LLVMVerifierFailureAction::LLVMReturnStatusAction,
            std::ptr::null_mut(),
        )
    };

    if verify_rc == 0 {
        return Ok(());
    }
    // Re-checked locally on Windows Arm64 on March 23, 2026: asking LLVM
    // to populate the verifier message pointer led to process instability,
    // so keep the Windows path on the null-pointer fallback for now.
    Err(format!(
        "{error_prefix}: LLVM verifier failed (message pointer unavailable on this platform; rerun on Linux/macOS for detailed verifier diagnostics)"
    ))
}

#[cfg(not(windows))]
fn verify_module_non_windows(module: &Module, error_prefix: &str) -> Result<(), String> {
    match module.verify() {
        Ok(()) => Ok(()),
        Err(err) => Err(format!("{error_prefix}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        reason = "tests use expect for direct verifier failure messages"
    )]

    use super::verify_module;
    use inkwell::context::Context;

    #[test]
    fn test_verify_module_accepts_well_formed_function() {
        let context = Context::create();
        let module = context.create_module("valid");
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("ok", fn_type, None);
        let entry = context.append_basic_block(func, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);
        builder
            .build_return(None)
            .expect("well-formed function should allow a return terminator");

        verify_module(&module, "verification failed")
            .expect("well-formed function should pass verification");
    }

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
