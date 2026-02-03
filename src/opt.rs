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

fn get_target_machine(target: &str, opt_level: OptimizationLevel) -> Result<TargetMachine, String> {
    // Ensure targets are initialized
    let _ = *TARGET_INIT;

    let target_config = match target {
        "x86-64" => X86_CONFIG,
        "aarch64" => AARCH64_CONFIG,
        "native" => NATIVE_CONFIG,
        _ => return Err(format!("Invalid target architecture: {target}")),
    };
    let reloc_mode = RelocMode::PIC;
    let code_model = CodeModel::Default;
    if target_config == NATIVE_CONFIG {
        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple)
            .map_err(|e| format!("Failed to create target from triple: {e}"))?;
        Ok(target
            .create_target_machine(
                &triple,
                &TargetMachine::get_host_cpu_name().to_string_lossy(),
                &TargetMachine::get_host_cpu_features().to_string_lossy(),
                opt_level,
                reloc_mode,
                code_model,
            )
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
    let (opt, opt_str) = match opt_level {
        0 => (OptimizationLevel::None, "default<O0>"),
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
