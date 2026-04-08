use inkwell::OptimizationLevel;
use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use std::sync::Mutex;

// Ensure LLVM targets are initialized only once to prevent SIGBUS crashes
static TARGET_INIT: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| {
    Target::initialize_x86(&InitializationConfig::default());
    Target::initialize_aarch64(&InitializationConfig::default());
    #[cfg(not(windows))]
    let _ = Target::initialize_native(&InitializationConfig::default());
    Mutex::new(())
});

type TargetConfig<'a> = (&'a str, &'a str, &'a str, &'a str);

/// Default config for `AArch64` codegen target
const AARCH64_CONFIG: TargetConfig = (
    "aarch64",
    "cortex-a53",
    "aarch64-unknown-linux-gnu",
    "+neon,+fp-armv8,+crypto,+crc",
);

/// Default config for x86-64 codegen target
const X86_CONFIG: TargetConfig = ("x86-64", "x86-64", "x86_64-unknown-linux-gnu", "");

/// Sentinel config for native codegen target
const NATIVE_CONFIG: TargetConfig = ("", "", "", "");

fn get_target_config(target: &str) -> Result<TargetConfig<'_>, String> {
    match target {
        "x86-64" => Ok(X86_CONFIG),
        "aarch64" => Ok(AARCH64_CONFIG),
        "native" => Ok(NATIVE_CONFIG),
        _ => Err(format!("Invalid target architecture: {target}")),
    }
}

fn get_target_machine(target: &str, opt_level: OptimizationLevel) -> Result<TargetMachine, String> {
    // Ensure targets are initialized
    let _ = *TARGET_INIT;

    let target_config = get_target_config(target)?;
    let reloc_mode = RelocMode::PIC;
    let code_model = CodeModel::Default;
    if target_config == NATIVE_CONFIG {
        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple)
            .map_err(|e| format!("Failed to create target from triple: {e}"))?;
        #[cfg(windows)]
        let (cpu, features) = ("generic".to_string(), String::new());
        #[cfg(not(windows))]
        let (cpu, features) = (
            TargetMachine::get_host_cpu_name()
                .to_string_lossy()
                .to_string(),
            TargetMachine::get_host_cpu_features()
                .to_string_lossy()
                .to_string(),
        );
        Ok(target
            .create_target_machine(&triple, &cpu, &features, opt_level, reloc_mode, code_model)
            .ok_or("Failed to create target machine")?)
    } else {
        let (name, cpu, triple, features) = target_config;
        let target =
            Target::from_name(name).ok_or_else(|| format!("Failed to create target: {name}"))?;
        Ok(target
            .create_target_machine(
                &TargetTriple::create(triple),
                cpu,
                features,
                opt_level,
                reloc_mode,
                code_model,
            )
            .ok_or("Failed to create target machine")?)
    }
}

/// Optimize the given LLVM module using the specified optimization level and target architecture.
///
/// # Errors
/// Returns an error if module verification fails
pub fn optimize(module: &Module, opt_level: u32, target: &str) -> Result<(), String> {
    #[cfg(windows)]
    if opt_level > 0 {
        return Err(format!(
            "Optimized QIR-to-QIS conversion is currently unavailable on Windows with the LLVM 21 integration. Re-run with `opt_level=0` and preferably `target=\"native\"` (requested opt_level={opt_level}, target=\"{target}\")."
        ));
    }

    // O0 preserves semantics without running transformation passes.
    // Avoid creating a TargetMachine in this mode; TargetMachine teardown has
    // caused access violations in some Windows environments.
    if opt_level == 0 {
        let target_config =
            get_target_config(target).map_err(|e| format!("Failed to get target machine: {e}"))?;
        #[cfg(not(windows))]
        {
            let triple = if target_config == NATIVE_CONFIG {
                TargetMachine::get_default_triple()
            } else {
                let (_, _, triple, _) = target_config;
                TargetTriple::create(triple)
            };
            module.set_triple(&triple);
        }
        #[cfg(windows)]
        // Keep the Windows O0 path as a no-op after target validation.
        // Attempting to update the module triple here reproduced the same
        // access-violation crash in CI on March 17, 2026 that originally
        // motivated the O0 fast path.
        let _ = (module, target_config);
        return Ok(());
    }

    let (opt, opt_str) = match opt_level {
        1 => (OptimizationLevel::Less, "default<O1>"),
        3 => (OptimizationLevel::Aggressive, "default<O3>"),
        _ => (OptimizationLevel::Default, "default<O2>"),
    };
    let target_machine = get_target_machine(target, opt)
        .map_err(|e| format!("Failed to get target machine: {e}"))?;

    let (data_layout, triple) = {
        (
            target_machine.get_target_data().get_data_layout(),
            target_machine.get_triple(),
        )
    };
    module.set_triple(&triple);
    module.set_data_layout(&data_layout);
    module
        .run_passes(opt_str, &target_machine, PassBuilderOptions::create())
        .map_err(|e| format!("Failed to run passes: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    #[cfg(not(windows))]
    use super::optimize;
    #[cfg(not(windows))]
    use inkwell::context::Context;
    #[cfg(not(windows))]
    use inkwell::targets::TargetMachine;

    #[cfg(not(windows))]
    #[test]
    fn test_optimize_o0_sets_native_and_explicit_triples_differently() {
        let native_ctx = Context::create();
        let native_module = native_ctx.create_module("native");
        optimize(&native_module, 0, "native").expect("native O0 optimize should succeed");

        let aarch64_ctx = Context::create();
        let aarch64_module = aarch64_ctx.create_module("aarch64");
        optimize(&aarch64_module, 0, "aarch64").expect("aarch64 O0 optimize should succeed");

        let native_triple = native_module
            .get_triple()
            .as_str()
            .to_string_lossy()
            .into_owned();
        let aarch64_triple = aarch64_module
            .get_triple()
            .as_str()
            .to_string_lossy()
            .into_owned();
        let default_triple = TargetMachine::get_default_triple()
            .as_str()
            .to_string_lossy()
            .into_owned();

        assert_eq!(native_triple, default_triple);
        assert_eq!(aarch64_triple, "aarch64-unknown-linux-gnu");
        if default_triple != aarch64_triple {
            assert_ne!(native_triple, aarch64_triple);
        }
    }
}
